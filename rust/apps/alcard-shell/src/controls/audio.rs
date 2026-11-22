//! The current PipeWire default output, controlled through WirePlumber.
//!
//! pw-dump provides the complete channel values that wpctl get-volume rounds
//! away. These parsers follow PipeWire 1.6.8 and WirePlumber 0.5.15
//! (module-mixer-api.c): active Device Route props take precedence over Node
//! Props, and wpctl uses the cube root of the linear channel gain.
//!
//! Separate CLI connections cannot make a compare-and-set atomic. We pin the
//! core cookie, actual default, object serials and route in a single complete
//! snapshot before writing, and require a fresh read-back. There is no ALSA
//! fallback, daemon startup, polling, retry, or hardware dB claim.

use super::{
    io_failure, run_command, run_command_with_output_limit, Availability, Budget, Control,
    Direction, Failure, Operation, Reading,
};
use serde_json::Value;
use std::{
    collections::HashSet,
    env, fs,
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::Path,
    process::Command,
};

const REMOTE: &str = "pipewire-0";
const CORE: &str = "PipeWire:Interface:Core";
const METADATA: &str = "PipeWire:Interface:Metadata";
const NODE: &str = "PipeWire:Interface:Node";
const DEVICE: &str = "PipeWire:Interface:Device";
const MAX_GRAPH_BYTES: usize = 1024 * 1024;
const MAX_OBJECTS: usize = 1024;
const MAX_PARAMS: usize = 128;
const MAX_CHANNELS: usize = 64;
const STEP: f64 = 0.05;
// 0.02 percentage points in wpctl's cubic scale; this tolerance never allows
// amplification above unity. The JSON values originate as SPA floats.
const READBACK_TOLERANCE: f64 = 0.0002;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ObjectKey {
    id: u32,
    serial: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DefaultOutput {
    metadata: ObjectKey,
    name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TargetKey {
    node: ObjectKey,
    node_device: Option<u32>,
    profile_device: Option<u32>,
    device: Option<ObjectKey>,
    route: Option<u32>,
    channels: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Identity {
    cookie: u32,
    default: DefaultOutput,
    target: TargetKey,
}

#[derive(Clone, Debug, PartialEq)]
struct Levels {
    channels: Vec<f64>,
    muted: bool,
}

impl Levels {
    fn level(&self) -> f64 {
        self.channels.iter().copied().fold(0.0, f64::max).cbrt()
    }

    fn above_unity(&self) -> bool {
        self.channels.iter().any(|value| *value > 1.0)
    }
}

#[derive(Clone, Debug)]
struct Target {
    key: TargetKey,
    levels: Levels,
    writable: bool,
}

impl Target {
    fn reading(&self) -> Reading {
        let level = self.levels.level().min(1.0);
        // Keep a positive but very quiet level adjustable down to real zero.
        let percent = if level > 0.0 {
            (level * 100.0).round().max(1.0) as u8
        } else {
            0
        };
        Reading {
            availability: if self.writable {
                Availability::Available
            } else {
                Availability::Denied
            },
            percent: Some(percent),
            muted: Some(self.levels.muted),
            above_safe_limit: self.levels.above_unity(),
        }
    }
}

struct Observation {
    identity: Identity,
    target: Target,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Change {
    Volume(f64),
    Mute(bool),
}

trait Transport {
    fn dump(&mut self, budget: &Budget<'_>) -> Result<String, Failure>;
    fn change(&mut self, node: u32, change: Change, budget: &Budget<'_>) -> Result<(), Failure>;
}

struct PipeWire;

fn command(program: &str, runtime: &Path) -> Command {
    let mut command = Command::new(program);
    command
        .env("LC_ALL", "C")
        .env("PIPEWIRE_REMOTE", REMOTE)
        .env("XDG_RUNTIME_DIR", runtime)
        .env_remove("PIPEWIRE_RUNTIME_DIR")
        .env_remove("PIPEWIRE_DAEMON");
    command
}

fn session_command(program: &str) -> Result<Command, Failure> {
    let runtime = env::var_os("XDG_RUNTIME_DIR").ok_or(Failure::Unavailable)?;
    let runtime = Path::new(&runtime);
    if !runtime.is_absolute() {
        return Err(Failure::Invalid);
    }
    let directory = fs::symlink_metadata(runtime).map_err(|error| io_failure(&error))?;
    // SAFETY: geteuid has no arguments, pointers, or side effects.
    let uid = unsafe { nix::libc::geteuid() };
    if !directory.is_dir() || directory.uid() != uid || directory.mode() & 0o077 != 0 {
        return Err(Failure::Denied);
    }
    let socket = fs::symlink_metadata(runtime.join(REMOTE)).map_err(|error| io_failure(&error))?;
    if !socket.file_type().is_socket() || socket.uid() != uid {
        return Err(Failure::Denied);
    }
    Ok(command(program, runtime))
}

fn dump_arguments() -> [&'static str; 3] {
    ["-N", "-r", REMOTE]
}

fn change_arguments(node: u32, change: Change) -> Vec<String> {
    match change {
        Change::Volume(level) => vec![
            "set-volume".into(),
            node.to_string(),
            format!("{level:.6}"),
            "--limit".into(),
            "1.0".into(),
        ],
        Change::Mute(muted) => vec![
            "set-mute".into(),
            node.to_string(),
            if muted { "1" } else { "0" }.into(),
        ],
    }
}

impl Transport for PipeWire {
    fn dump(&mut self, budget: &Budget<'_>) -> Result<String, Failure> {
        let mut command = session_command("/usr/bin/pw-dump")?;
        command.args(dump_arguments());
        run_command_with_output_limit(&mut command, budget, false, MAX_GRAPH_BYTES)
    }

    fn change(&mut self, node: u32, change: Change, budget: &Budget<'_>) -> Result<(), Failure> {
        let mut command = session_command("/usr/bin/wpctl")?;
        command.args(change_arguments(node, change));
        run_command(&mut command, budget, true).map(|_| ())
    }
}

pub(super) struct Backend {
    transport: PipeWire,
    binding: Option<Identity>,
}

impl Backend {
    pub(super) fn system() -> Self {
        Self {
            transport: PipeWire,
            binding: None,
        }
    }

    pub(super) fn refresh(&mut self, budget: &Budget<'_>) -> Reading {
        refresh(&mut self.transport, &mut self.binding, budget)
    }

    pub(super) fn execute(
        &mut self,
        operation: Operation,
        budget: &Budget<'_>,
    ) -> (Reading, Result<(), Failure>) {
        execute(&mut self.transport, &mut self.binding, operation, budget)
    }
}

fn refresh(
    transport: &mut impl Transport,
    binding: &mut Option<Identity>,
    budget: &Budget<'_>,
) -> Reading {
    match observe(transport, budget) {
        Ok(observed) => {
            *binding = Some(observed.identity);
            observed.target.reading()
        }
        Err(error) => {
            *binding = None;
            Reading::failed(error)
        }
    }
}

fn execute(
    transport: &mut impl Transport,
    binding: &mut Option<Identity>,
    operation: Operation,
    budget: &Budget<'_>,
) -> (Reading, Result<(), Failure>) {
    let Some(expected) = binding.as_ref() else {
        return (
            Reading::failed(Failure::Unavailable),
            Err(Failure::Unavailable),
        );
    };
    let observed = match observe(transport, budget) {
        Ok(observed) => observed,
        Err(error) => {
            *binding = None;
            return (Reading::failed(error), Err(error));
        }
    };
    if observed.identity != *expected {
        *binding = None;
        return changed(observed.target.reading());
    }

    // The fresh default, node/device serials, route and all channel values
    // belong to one pw-dump connection and one core cookie. Relative changes
    // never use the older values displayed by the UI.
    let fresh = &observed.target;
    if !fresh.writable {
        return (fresh.reading(), Err(Failure::Denied));
    }
    let requested = match plan(&fresh.levels, operation) {
        Ok(change) => change,
        Err(error) => return (fresh.reading(), Err(error)),
    };
    if let Err(error) = budget.check(false) {
        return (fresh.reading(), Err(error));
    }
    let outcome = transport.change(fresh.key.node.id, requested, budget);
    // A read-back is required even after a failed process: a timed-out child
    // may already have changed the server. Never issue the mutation again.
    let after = match observe(transport, budget) {
        Ok(after) => after,
        Err(_) => {
            *binding = None;
            let error = outcome.err().unwrap_or(Failure::Unknown);
            return (Reading::failed(error), Err(error));
        }
    };
    let reading = after.target.reading();
    if after.identity != observed.identity {
        *binding = None;
        let mut reading = reading;
        reading.availability = Availability::Unavailable;
        return (reading, Err(Failure::Unknown));
    }
    if let Err(error) = outcome {
        return (reading, Err(error));
    }
    if !confirmed(&fresh.levels, &after.target.levels, requested) {
        return (reading, Err(Failure::Unknown));
    }
    (reading, Ok(()))
}

fn changed(mut reading: Reading) -> (Reading, Result<(), Failure>) {
    reading.availability = Availability::Unavailable;
    (reading, Err(Failure::Changed))
}

fn plan(levels: &Levels, operation: Operation) -> Result<Change, Failure> {
    match operation {
        Operation::Adjust {
            control: Control::Volume,
            direction,
        } => {
            let current = levels.level();
            let target = match direction {
                Direction::Increase if !levels.above_unity() && current < 1.0 => {
                    (current + STEP).min(1.0)
                }
                Direction::Decrease if current > 0.0 => (current.min(1.0) - STEP).max(0.0),
                _ => return Err(Failure::Invalid),
            };
            // Match the decimal passed to wpctl.
            Ok(Change::Volume((target * 1_000_000.0).round() / 1_000_000.0))
        }
        Operation::ToggleMute if !levels.muted || !levels.above_unity() => {
            Ok(Change::Mute(!levels.muted))
        }
        _ => Err(Failure::Invalid),
    }
}

fn confirmed(before: &Levels, after: &Levels, requested: Change) -> bool {
    if before.channels.len() != after.channels.len() {
        return false;
    }
    match requested {
        Change::Volume(level) => {
            before.muted == after.muted
                && !after.above_unity()
                && after
                    .channels
                    .iter()
                    .all(|value| (value.cbrt() - level).abs() <= READBACK_TOLERANCE)
        }
        Change::Mute(muted) => {
            after.muted == muted
                && (muted || !after.above_unity())
                && before
                    .channels
                    .iter()
                    .zip(&after.channels)
                    .all(|(before, after)| {
                        (before.cbrt() - after.cbrt()).abs() <= READBACK_TOLERANCE
                    })
        }
    }
}

fn observe(transport: &mut impl Transport, budget: &Budget<'_>) -> Result<Observation, Failure> {
    budget.check(false)?;
    // A pattern only filters pw-dump's final output; upstream still binds and
    // enumerates every global. One bounded full snapshot avoids repeating that
    // graph walk for each field and keeps the core identity on one connection.
    let objects = parse_dump(&transport.dump(budget)?)?;
    let cookie = core_cookie(&objects)?;
    let default = default_output(&objects)?;
    let target = read_target(&objects, &default.name)?;
    Ok(Observation {
        identity: Identity {
            cookie,
            default,
            target: target.key.clone(),
        },
        target,
    })
}

fn parse_dump(text: &str) -> Result<Vec<Value>, Failure> {
    if text.len() > MAX_GRAPH_BYTES {
        return Err(Failure::Invalid);
    }
    let Value::Array(objects) = serde_json::from_str(text).map_err(|_| Failure::Invalid)? else {
        return Err(Failure::Invalid);
    };
    if objects.len() > MAX_OBJECTS {
        return Err(Failure::Invalid);
    }
    let mut ids = HashSet::new();
    for object in &objects {
        if !ids.insert(number_u32(&object["id"])?) || !object["type"].is_string() {
            return Err(Failure::Invalid);
        }
    }
    Ok(objects)
}

fn one<'a>(mut values: impl Iterator<Item = &'a Value>) -> Result<&'a Value, Failure> {
    let value = values.next().ok_or(Failure::Unavailable)?;
    if values.next().is_some() {
        return Err(Failure::Invalid);
    }
    Ok(value)
}

fn core_cookie(objects: &[Value]) -> Result<u32, Failure> {
    let core = one(objects
        .iter()
        .filter(|object| object["type"] == CORE && object["id"].as_u64() == Some(0)))?;
    number_u32(&core["info"]["cookie"])
}

fn default_output(objects: &[Value]) -> Result<DefaultOutput, Failure> {
    let metadata = one(objects.iter().filter(|object| {
        object["type"] == METADATA && object["props"]["metadata.name"] == "default"
    }))?;
    let entries = array(&metadata["metadata"])?;
    let actual = one(entries.iter().filter(|entry| {
        entry["subject"].as_u64() == Some(0) && entry["key"] == "default.audio.sink"
    }))?;
    if actual["type"] != "Spa:String:JSON" {
        return Err(Failure::Invalid);
    }
    let name = actual["value"]["name"].as_str().ok_or(Failure::Invalid)?;
    if !valid_name(name) {
        return Err(Failure::Invalid);
    }
    Ok(DefaultOutput {
        metadata: object_key(metadata, &metadata["props"])?,
        name: name.into(),
    })
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 512 && !name.chars().any(char::is_control)
}

fn read_target(objects: &[Value], name: &str) -> Result<Target, Failure> {
    let node = one(objects.iter().filter(|object| {
        object["type"] == NODE
            && object["info"]["props"]["media.class"] == "Audio/Sink"
            && object["info"]["props"]["node.name"] == name
    }))?;
    if node["info"]["state"] == "error" {
        return Err(Failure::Unavailable);
    }
    let props = &node["info"]["props"];
    let node_key = object_key(node, props)?;
    let node_device = optional_id(props.get("device.id"))?;
    let profile_device = optional_index(props.get("card.profile.device"))?;
    let mut writable = can_write(node)?;
    let mut device_key = None;
    let mut route = None;
    let mut levels = None;
    if let (Some(id), Some(profile)) = (node_device, profile_device) {
        let device = one(objects.iter().filter(|object| {
            object["type"] == DEVICE && object["id"].as_u64() == Some(u64::from(id))
        }))?;
        if device["info"]["props"]["media.class"] != "Audio/Device" {
            return Err(Failure::Unavailable);
        }
        device_key = Some(object_key(device, &device["info"]["props"])?);
        if let Some((index, volume)) = route_levels(device, profile)? {
            route = Some(index);
            levels = Some(volume);
            writable &= can_write(device)?;
        }
    }
    let levels = match levels {
        Some(levels) => levels,
        None => node_levels(node)?,
    };
    Ok(Target {
        key: TargetKey {
            node: node_key,
            node_device,
            profile_device,
            device: device_key,
            route,
            channels: levels.channels.len(),
        },
        levels,
        writable,
    })
}

fn route_levels(device: &Value, profile: u32) -> Result<Option<(u32, Levels)>, Failure> {
    let Some(routes) = device["info"]["params"].get("Route") else {
        return Ok(None);
    };
    let mut selected = None;
    for route in array(routes)? {
        let Some(device) = optional_index(route.get("device"))? else {
            continue;
        };
        if device != profile {
            continue;
        }
        let Some(props) = route.get("props") else {
            continue;
        };
        let Some(levels) = parse_levels(props)? else {
            continue;
        };
        let index = optional_index(route.get("index"))?.ok_or(Failure::Invalid)?;
        if selected.replace((index, levels)).is_some() {
            return Err(Failure::Invalid);
        }
    }
    Ok(selected)
}

fn node_levels(node: &Value) -> Result<Levels, Failure> {
    let props = node["info"]["params"]
        .get("Props")
        .ok_or(Failure::Unavailable)?;
    let mut selected = None;
    for props in array(props)? {
        if let Some(levels) = parse_levels(props)? {
            if selected.replace(levels).is_some() {
                return Err(Failure::Invalid);
            }
        }
    }
    selected.ok_or(Failure::Unavailable)
}

fn parse_levels(props: &Value) -> Result<Option<Levels>, Failure> {
    let (Some(mute), Some(volumes)) = (props.get("mute"), props.get("channelVolumes")) else {
        // This is also the support test used by WirePlumber's mixer-api.
        return Ok(None);
    };
    let muted = mute.as_bool().ok_or(Failure::Invalid)?;
    let values = volumes.as_array().ok_or(Failure::Invalid)?;
    if values.is_empty() || values.len() > MAX_CHANNELS {
        return Err(Failure::Invalid);
    }
    let channels = values
        .iter()
        .map(|value| {
            let value = value.as_f64().ok_or(Failure::Invalid)?;
            if !value.is_finite() || value < 0.0 {
                return Err(Failure::Invalid);
            }
            Ok(value)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some(Levels { channels, muted }))
}

fn can_write(object: &Value) -> Result<bool, Failure> {
    let permissions = object["permissions"].as_array().ok_or(Failure::Invalid)?;
    if permissions.len() > 16 || permissions.iter().any(|permission| !permission.is_string()) {
        return Err(Failure::Invalid);
    }
    // pw-dump does not expose SPA_PARAM_INFO_WRITE separately. These are
    // necessary global permissions; actual acceptance still needs read-back.
    Ok(["r", "w", "x"].iter().all(|required| {
        permissions
            .iter()
            .any(|permission| permission.as_str() == Some(*required))
    }))
}

fn object_key(object: &Value, props: &Value) -> Result<ObjectKey, Failure> {
    let id = number_u32(&object["id"])?;
    let serial = property_u64(&props["object.serial"])?;
    if id == 0 || id == u32::MAX || serial == 0 {
        return Err(Failure::Invalid);
    }
    Ok(ObjectKey { id, serial })
}

fn number_u32(value: &Value) -> Result<u32, Failure> {
    value
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .ok_or(Failure::Invalid)
}

fn property_u64(value: &Value) -> Result<u64, Failure> {
    if let Some(value) = value.as_u64() {
        return Ok(value);
    }
    let value = value.as_str().ok_or(Failure::Invalid)?;
    if value.is_empty() || value.len() > 20 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(Failure::Invalid);
    }
    value.parse().map_err(|_| Failure::Invalid)
}

fn optional_id(value: Option<&Value>) -> Result<Option<u32>, Failure> {
    value
        .map(|value| {
            let id = u32::try_from(property_u64(value)?).map_err(|_| Failure::Invalid)?;
            if id == 0 || id == u32::MAX {
                return Err(Failure::Invalid);
            }
            Ok(id)
        })
        .transpose()
}

fn optional_index(value: Option<&Value>) -> Result<Option<u32>, Failure> {
    value
        .map(|value| {
            let index = u32::try_from(property_u64(value)?).map_err(|_| Failure::Invalid)?;
            if index > i32::MAX as u32 {
                return Err(Failure::Invalid);
            }
            Ok(index)
        })
        .transpose()
}

fn array(value: &Value) -> Result<&Vec<Value>, Failure> {
    let values = value.as_array().ok_or(Failure::Invalid)?;
    if values.len() > MAX_PARAMS {
        return Err(Failure::Invalid);
    }
    Ok(values)
}

#[cfg(test)]
#[path = "unit_tests/audio.rs"]
mod tests;
