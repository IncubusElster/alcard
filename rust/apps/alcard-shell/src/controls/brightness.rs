//! Read the kernel backlight setting, but delegate writes to the current
//! foreground session's login1 service. No sysfs chmod, privileged helper or
//! direct brightness write is used in production.

use super::{
    availability, decode, io_failure, read_small, safe_name, Availability, Budget, Direction,
    Failure, Reading, MAX_REPLY_BYTES,
};
use nix::unistd::Uid;
use std::{
    collections::HashMap,
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};
use zbus::{OwnedObjectPath, OwnedVariant};

const SERVICE: &str = "org.freedesktop.login1";
const MANAGER: &str = "org.freedesktop.login1.Manager";
const SESSION: &str = "org.freedesktop.login1.Session";
const MANAGER_PATH: &str = "/org/freedesktop/login1";

#[derive(Clone, Debug, PartialEq, Eq)]
struct Device {
    name: String,
    canonical: PathBuf,
    device_id: u64,
    inode: u64,
    maximum: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Session {
    owner: String,
    path: String,
    id: String,
    uid: u32,
    seat: String,
}

#[derive(Clone)]
struct Binding {
    device: Device,
    session: Session,
}

trait Login {
    fn current(&mut self, budget: &Budget<'_>) -> Result<Session, Failure>;
    fn set(
        &mut self,
        session: &Session,
        device: &str,
        value: u32,
        budget: &Budget<'_>,
    ) -> Result<(), Failure>;
}

#[derive(Default)]
struct Bus {
    connection: Option<zbus::Connection>,
}

pub(super) struct Backend {
    root: PathBuf,
    bus: Bus,
    binding: Option<Binding>,
}

impl Backend {
    pub(super) fn system() -> Self {
        Self {
            root: "/sys/class/backlight".into(),
            bus: Bus::default(),
            binding: None,
        }
    }

    pub(super) fn refresh(&mut self, budget: &Budget<'_>) -> Reading {
        // Explicit discovery is also the recovery path for a disconnected
        // system bus. Never reconnect and replay a pending mutation.
        self.bus.connection = None;
        let (binding, reading) = refresh(&self.root, &mut self.bus, budget);
        self.binding = binding;
        reading
    }

    pub(super) fn adjust(
        &mut self,
        direction: Direction,
        budget: &Budget<'_>,
    ) -> (Reading, Result<(), Failure>) {
        let Some(binding) = self.binding.as_ref() else {
            return (
                Reading::failed(Failure::Unavailable),
                Err(Failure::Unavailable),
            );
        };
        let (reading, result) = adjust(&self.root, &mut self.bus, binding, direction, budget);
        if matches!(
            result,
            Err(Failure::Changed | Failure::Unavailable | Failure::Denied)
        ) {
            self.binding = None;
        }
        (reading, result)
    }
}

fn refresh(
    root: &Path,
    service: &mut impl Login,
    budget: &Budget<'_>,
) -> (Option<Binding>, Reading) {
    let (device, raw) = match discover(root) {
        Ok(value) => value,
        Err(error) => return (None, Reading::failed(error)),
    };
    let mut reading = reading(raw, device.maximum);
    match service.current(budget) {
        Ok(session) => (Some(Binding { device, session }), reading),
        Err(error) => {
            reading.availability = availability(error);
            (None, reading)
        }
    }
}

fn adjust(
    root: &Path,
    service: &mut impl Login,
    binding: &Binding,
    direction: Direction,
    budget: &Budget<'_>,
) -> (Reading, Result<(), Failure>) {
    let mut attempted = false;
    let result = (|| {
        budget.check(false)?;
        let (current, raw) = discover(root)?;
        if current != binding.device || service.current(budget)? != binding.session {
            return Err(Failure::Changed);
        }
        let target = adjusted(raw, current.maximum, direction);
        if target == raw {
            return Ok(());
        }
        // `current` is pinned again immediately before the only mutation. The
        // service itself checks UID, foreground session and device seat.
        if discover(root)?.0 != current {
            return Err(Failure::Changed);
        }
        budget.check(false)?;
        attempted = true;
        service.set(&binding.session, &current.name, target, budget)?;
        let (observed, value) = discover(root).map_err(|_| Failure::Unknown)?;
        if observed != current || value != target {
            return Err(Failure::Unknown);
        }
        Ok(())
    })();
    let result = result.map_err(|error| {
        if attempted
            && matches!(
                error,
                Failure::Changed | Failure::Timeout | Failure::Invalid
            )
        {
            Failure::Unknown
        } else {
            error
        }
    });
    let mut latest = match discover(root) {
        Ok((device, raw)) if device == binding.device => reading(raw, device.maximum),
        _ => Reading::failed(Failure::Unavailable),
    };
    if let Err(error @ (Failure::Denied | Failure::Unavailable | Failure::Changed)) = result {
        latest.availability = availability(error);
    }
    (latest, result)
}

fn discover(root: &Path) -> Result<(Device, u32), Failure> {
    let mut selected = None;
    for (count, entry) in fs::read_dir(root)
        .map_err(|error| io_failure(&error))?
        .enumerate()
    {
        if count >= 32 {
            return Err(Failure::Invalid);
        }
        let entry = entry.map_err(|error| io_failure(&error))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| Failure::Invalid)?;
        if !safe_name(&name) || selected.is_some() {
            // Multiple devices need an explicit device selector; do not pick
            // a random output or silently change target after hotplug.
            return Err(Failure::Unavailable);
        }
        let canonical = fs::canonicalize(entry.path()).map_err(|error| io_failure(&error))?;
        let metadata = fs::metadata(entry.path()).map_err(|error| io_failure(&error))?;
        let maximum: u32 = read_small(&entry.path().join("max_brightness"))?
            .parse()
            .map_err(|_| Failure::Invalid)?;
        let raw: u32 = read_small(&entry.path().join("brightness"))?
            .parse()
            .map_err(|_| Failure::Invalid)?;
        if maximum == 0 || maximum > i32::MAX as u32 || raw > maximum {
            return Err(Failure::Invalid);
        }
        let after = fs::metadata(entry.path()).map_err(|error| io_failure(&error))?;
        if metadata.dev() != after.dev() || metadata.ino() != after.ino() {
            return Err(Failure::Changed);
        }
        selected = Some((
            Device {
                name,
                canonical,
                device_id: metadata.dev(),
                inode: metadata.ino(),
                maximum,
            },
            raw,
        ));
    }
    selected.ok_or(Failure::Unavailable)
}

fn adjusted(raw: u32, maximum: u32, direction: Direction) -> u32 {
    let minimum = maximum.div_ceil(100).max(1);
    let step = maximum.div_ceil(20).max(1);
    match direction {
        Direction::Decrease if raw <= minimum => raw,
        Direction::Decrease => raw.saturating_sub(step).max(minimum).min(maximum),
        Direction::Increase => raw.saturating_add(step).max(minimum).min(maximum),
    }
}

fn reading(raw: u32, maximum: u32) -> Reading {
    let percent = ((u64::from(raw) * 100 + u64::from(maximum) / 2) / u64::from(maximum)) as u8;
    Reading {
        availability: Availability::Available,
        percent: Some(if raw == 0 { 0 } else { percent.max(1) }),
        muted: None,
        above_safe_limit: false,
    }
}

impl Bus {
    fn connection(&mut self, budget: &Budget<'_>) -> Result<&zbus::Connection, Failure> {
        if self.connection.is_none() {
            self.connection = Some(budget.dbus(zbus::Connection::system(), false)?);
        }
        self.connection.as_ref().ok_or(Failure::Unavailable)
    }
}

fn owner(connection: &zbus::Connection, budget: &Budget<'_>) -> Result<String, Failure> {
    let value: String = decode(budget.dbus(
        connection.call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "GetNameOwner",
            &(SERVICE,),
        ),
        false,
    )?)?;
    if value.len() > 255 || zbus::names::UniqueName::try_from(value.as_str()).is_err() {
        return Err(Failure::Invalid);
    }
    Ok(value)
}

impl Login for Bus {
    fn current(&mut self, budget: &Budget<'_>) -> Result<Session, Failure> {
        let connection = self.connection(budget)?;
        let expected_owner = owner(connection, budget)?;
        let path: OwnedObjectPath = decode(budget.dbus(
            connection.call_method(
                Some(expected_owner.as_str()),
                MANAGER_PATH,
                Some(MANAGER),
                "GetSessionByPID",
                &(std::process::id(),),
            ),
            false,
        )?)?;
        let properties = decode(budget.dbus(
            connection.call_method(
                Some(expected_owner.as_str()),
                path.as_str(),
                Some("org.freedesktop.DBus.Properties"),
                "GetAll",
                &(SESSION,),
            ),
            false,
        )?)?;
        let session = session(
            expected_owner.clone(),
            &path,
            properties,
            Uid::current().as_raw(),
        )?;
        let xml: String = decode(budget.dbus(
            connection.call_method(
                Some(expected_owner.as_str()),
                path.as_str(),
                Some("org.freedesktop.DBus.Introspectable"),
                "Introspect",
                &(),
            ),
            false,
        )?)?;
        if !supports_brightness(&xml)? {
            return Err(Failure::Unavailable);
        }
        if owner(connection, budget)? != expected_owner {
            return Err(Failure::Changed);
        }
        Ok(session)
    }

    fn set(
        &mut self,
        session: &Session,
        device: &str,
        value: u32,
        budget: &Budget<'_>,
    ) -> Result<(), Failure> {
        let connection = self.connection(budget)?;
        if owner(connection, budget)? != session.owner {
            return Err(Failure::Changed);
        }
        decode(budget.dbus(
            connection.call_method(
                Some(session.owner.as_str()),
                session.path.as_str(),
                Some(SESSION),
                "SetBrightness",
                &("backlight", device, value),
            ),
            true,
        )?)
    }
}

fn session(
    owner: String,
    path: &OwnedObjectPath,
    mut properties: HashMap<String, OwnedVariant>,
    uid: u32,
) -> Result<Session, Failure> {
    let invalid = || Failure::Unavailable;
    if properties.len() > 64
        || path.as_str().len() > 512
        || !path
            .as_str()
            .starts_with("/org/freedesktop/login1/session/")
    {
        return Err(invalid());
    }
    let id =
        String::try_from(properties.remove("Id").ok_or_else(invalid)?).map_err(|_| invalid())?;
    let (owner_uid, _): (u32, OwnedObjectPath) = properties
        .remove("User")
        .ok_or_else(invalid)?
        .try_into()
        .map_err(|_| invalid())?;
    let active =
        bool::try_from(properties.remove("Active").ok_or_else(invalid)?).map_err(|_| invalid())?;
    let remote =
        bool::try_from(properties.remove("Remote").ok_or_else(invalid)?).map_err(|_| invalid())?;
    let kind =
        String::try_from(properties.remove("Type").ok_or_else(invalid)?).map_err(|_| invalid())?;
    let (seat, _): (String, OwnedObjectPath) = properties
        .remove("Seat")
        .ok_or_else(invalid)?
        .try_into()
        .map_err(|_| invalid())?;
    if !safe_name(&id)
        || matches!(id.as_str(), "self" | "auto")
        || !safe_name(&seat)
        || remote
        || !matches!(kind.as_str(), "wayland" | "x11")
    {
        return Err(invalid());
    }
    if owner_uid != uid || !active {
        return Err(Failure::Denied);
    }
    Ok(Session {
        owner,
        path: path.as_str().into(),
        id,
        uid,
        seat,
    })
}

/// A bounded subset of introspection XML, with exact input signature. Entities,
/// DTDs and external references are never interpreted.
fn supports_brightness(xml: &str) -> Result<bool, Failure> {
    if xml.len() > MAX_REPLY_BYTES {
        return Err(Failure::Invalid);
    }
    let mut remaining = xml;
    let mut interface = false;
    let mut method = None;
    let mut found = false;
    let mut count = 0;
    while let Some(start) = remaining.find('<') {
        remaining = &remaining[start + 1..];
        if remaining.starts_with("!--") {
            let end = remaining.find("-->").ok_or(Failure::Invalid)?;
            remaining = &remaining[end + 3..];
            continue;
        }
        let end = remaining.find('>').ok_or(Failure::Invalid)?;
        let tag = remaining[..end].trim();
        remaining = &remaining[end + 1..];
        count += 1;
        if count > 4096 {
            return Err(Failure::Invalid);
        }
        match tag
            .split_ascii_whitespace()
            .next()
            .unwrap_or_default()
            .trim_end_matches('/')
        {
            "interface" => {
                interface = attribute(tag, "name") == Some(SESSION);
                method = None;
            }
            "/interface" => {
                interface = false;
                method = None;
            }
            "method" if interface => {
                method =
                    (attribute(tag, "name") == Some("SetBrightness")).then_some((0_usize, true));
            }
            "arg" if interface => {
                if let Some((count, valid)) = method.as_mut() {
                    let expected = ["s", "s", "u"].get(*count).copied();
                    *valid &= attribute(tag, "direction").unwrap_or("in") == "in"
                        && attribute(tag, "type") == expected
                        && expected.is_some();
                    *count += 1;
                }
            }
            "/method" if interface => {
                if let Some((count, valid)) = method.take() {
                    found |= count == 3 && valid;
                }
            }
            _ => {}
        }
    }
    if interface || method.is_some() {
        return Err(Failure::Invalid);
    }
    Ok(found)
}

fn attribute<'a>(tag: &'a str, wanted: &str) -> Option<&'a str> {
    let mut remaining = tag.get(tag.find(char::is_whitespace)?..)?.trim_start();
    while !remaining.is_empty() && remaining != "/" {
        let equal = remaining.find('=')?;
        let key = remaining[..equal].trim();
        remaining = remaining[equal + 1..].trim_start();
        let quote = remaining.chars().next()?;
        if !matches!(quote, '"' | '\'') {
            return None;
        }
        remaining = &remaining[1..];
        let end = remaining.find(quote)?;
        let value = &remaining[..end];
        if key == wanted {
            return Some(value);
        }
        remaining = remaining[end + 1..].trim_start();
    }
    None
}

#[cfg(test)]
#[path = "unit_tests/brightness.rs"]
mod tests;
