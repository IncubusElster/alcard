//! Linux NetworkManager and standard BlueZ implementation used only by the fixed
//! radio helper. Production roots and executable paths are not configurable.

use crate::radio_protocol::{
    public_text, Availability, Entry, Failure, Kind, Operation, Reply, Snapshot, State, MAX_ENTRIES,
};
use std::{
    collections::{BTreeSet, HashMap},
    fs::{self, OpenOptions},
    future::Future,
    io::{self, Read, Write},
    os::{
        fd::AsRawFd,
        unix::{
            fs::{FileTypeExt, MetadataExt, OpenOptionsExt},
            process::CommandExt,
        },
    },
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use zbus::{wire::DynamicDeserialize, DBusError, OwnedObjectPath, OwnedVariant, Variant};

const RFKILL: &str = "/sys/class/rfkill";
const NET: &str = "/sys/class/net";
const BLUETOOTH: &str = "/sys/class/bluetooth";
use alcard_settings_core::network::{fields, NMCLI};
const BLUEZ: &str = "org.bluez";
const ADAPTER: &str = "org.bluez.Adapter1";
const DEVICE: &str = "org.bluez.Device1";
const MAX_IO: usize = 256 * 1024;
const POLL: Duration = Duration::from_millis(20);

#[derive(Clone, Copy)]
struct Budget(Instant);

impl Budget {
    fn new(seconds: u64) -> Self {
        Self(Instant::now() + Duration::from_secs(seconds))
    }
    fn check(self) -> Result<(), Failure> {
        if Instant::now() >= self.0 {
            Err(Failure::Timeout)
        } else {
            Ok(())
        }
    }
    fn dbus<T>(self, future: impl Future<Output = zbus::Result<T>>) -> Result<T, Failure> {
        self.check()?;
        let runtime = zruntime::SharedRuntime::current().map_err(|_| Failure::Unavailable)?;
        runtime.block_on(futures_lite::future::or(
            async {
                loop {
                    self.check()?;
                    runtime.sleep(POLL).await;
                }
            },
            async { future.await.map_err(bus_failure) },
        ))
    }
}

fn io_failure(error: io::Error) -> Failure {
    match error.kind() {
        io::ErrorKind::PermissionDenied => Failure::Permission,
        io::ErrorKind::NotFound | io::ErrorKind::NotConnected => Failure::Unavailable,
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => Failure::Timeout,
        _ => Failure::Failed,
    }
}

fn bus_failure(error: zbus::Error) -> Failure {
    let name = match &error {
        zbus::Error::MethodError(name, _, _) => name.as_str().to_owned(),
        zbus::Error::FDO(error) => error.name().as_str().to_owned(),
        zbus::Error::DBus(error) => error.name().as_str().to_owned(),
        _ => String::new(),
    };
    match name.as_str() {
        "org.freedesktop.DBus.Error.AccessDenied"
        | "org.freedesktop.DBus.Error.AuthFailed"
        | "org.bluez.Error.NotAuthorized" => Failure::Permission,
        "org.freedesktop.DBus.Error.ServiceUnknown"
        | "org.freedesktop.DBus.Error.NameHasNoOwner"
        | "org.freedesktop.DBus.Error.UnknownObject"
        | "org.freedesktop.DBus.Error.UnknownMethod"
        | "org.bluez.Error.NotReady" => Failure::Unavailable,
        "org.freedesktop.DBus.Error.Timeout" | "org.freedesktop.DBus.Error.NoReply" => {
            Failure::Timeout
        }
        _ => Failure::Failed,
    }
}

fn unavailable() -> State {
    State {
        availability: Availability::Unavailable,
        powered: None,
        detail: String::new(),
    }
}
fn absent() -> State {
    State {
        availability: Availability::Absent,
        powered: None,
        detail: String::new(),
    }
}
fn blocked(detail: String) -> State {
    State {
        availability: Availability::Blocked,
        powered: Some(false),
        detail,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Switch {
    index: u32,
    soft: bool,
    hard: bool,
}

fn read_small(path: &Path) -> Result<String, Failure> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK | nix::libc::O_CLOEXEC)
        .open(path)
        .map_err(io_failure)?;
    if !file.metadata().map_err(io_failure)?.is_file() {
        return Err(Failure::InvalidResponse);
    }
    let mut text = String::new();
    file.take(257)
        .read_to_string(&mut text)
        .map_err(io_failure)?;
    if text.len() > 256 {
        return Err(Failure::InvalidResponse);
    }
    Ok(text.trim().to_owned())
}

fn bit(text: &str) -> Result<bool, Failure> {
    match text {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(Failure::InvalidResponse),
    }
}

fn switches(root: &Path, kind: Kind) -> Result<Vec<Switch>, Failure> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(io_failure(error)),
    };
    let mut result = Vec::new();
    for (count, entry) in entries.enumerate() {
        if count >= 64 {
            return Err(Failure::InvalidResponse);
        }
        let entry = entry.map_err(io_failure)?;
        let name = entry.file_name();
        let name = name.to_str().ok_or(Failure::InvalidResponse)?;
        let index = name
            .strip_prefix("rfkill")
            .ok_or(Failure::InvalidResponse)?
            .parse::<u32>()
            .map_err(|_| Failure::InvalidResponse)?;
        let path = entry.path();
        let radio_type = read_small(&path.join("type"))?;
        if radio_type
            != match kind {
                Kind::Wifi => "wlan",
                Kind::Bluetooth => "bluetooth",
            }
        {
            continue;
        }
        result.push(Switch {
            index,
            soft: bit(&read_small(&path.join("soft"))?)?,
            hard: bit(&read_small(&path.join("hard"))?)?,
        });
    }
    result.sort_by_key(|switch| switch.index);
    Ok(result)
}

fn interface_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() < 16
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|value| value.is_ascii_alphanumeric() || b"_-.:".contains(&value))
        && !name.starts_with('-')
}

fn interfaces(root: &Path) -> Result<Vec<String>, Failure> {
    let mut result = Vec::new();
    for (count, entry) in fs::read_dir(root).map_err(io_failure)?.enumerate() {
        if count >= 512 {
            return Err(Failure::InvalidResponse);
        }
        let entry = entry.map_err(io_failure)?;
        if !entry.path().join("wireless").exists() && !entry.path().join("phy80211").exists() {
            continue;
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| Failure::InvalidResponse)?;
        if !interface_name(&name) || result.len() >= 8 {
            return Err(Failure::InvalidResponse);
        }
        result.push(name);
    }
    result.sort();
    Ok(result)
}

fn hardware_present(root: &Path) -> Result<bool, Failure> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(io_failure(error)),
    };
    for entry in entries.take(65) {
        let name = entry.map_err(io_failure)?.file_name();
        if name.to_str().is_some_and(|name| {
            name.strip_prefix("hci").is_some_and(|suffix| {
                !suffix.is_empty() && suffix.bytes().all(|v| v.is_ascii_digit())
            })
        }) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn uniform(values: impl IntoIterator<Item = bool>) -> Option<bool> {
    let mut values = values.into_iter();
    let first = values.next()?;
    values.all(|value| value == first).then_some(first)
}

fn wifi_state(switches: &[Switch], interfaces: &[String]) -> State {
    let detail = public_text(&interfaces.join(", "), 128);
    if switches.iter().any(|switch| switch.hard) {
        return blocked(detail);
    }
    if switches.is_empty() && interfaces.is_empty() {
        return absent();
    }
    if switches.is_empty() || interfaces.is_empty() {
        return unavailable();
    }
    State {
        availability: Availability::Available,
        powered: uniform(switches.iter().map(|switch| !switch.soft)),
        detail,
    }
}

fn wifi_snapshot() -> State {
    match (
        switches(Path::new(RFKILL), Kind::Wifi),
        interfaces(Path::new(NET)),
    ) {
        (Ok(switches), Ok(interfaces)) => wifi_state(&switches, &interfaces),
        _ => unavailable(),
    }
}

fn rfkill_event(kind: Kind, powered: bool) -> [u8; 8] {
    // linux/uapi/rfkill.h: idx=0 ignored for CHANGE_ALL=3; WLAN=1, BT=2.
    [
        0,
        0,
        0,
        0,
        match kind {
            Kind::Wifi => 1,
            Kind::Bluetooth => 2,
        },
        3,
        u8::from(!powered),
        0,
    ]
}

fn write_rfkill(
    kind: Kind,
    powered: bool,
    before: &[Switch],
    budget: Budget,
) -> Result<(), Failure> {
    budget.check()?;
    if before.is_empty() || (powered && before.iter().any(|switch| switch.hard)) {
        return Err(Failure::Unavailable);
    }
    if switches(Path::new(RFKILL), kind)? != before {
        return Err(Failure::Failed);
    }
    let mut file = OpenOptions::new()
        .write(true)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK | nix::libc::O_CLOEXEC)
        .open("/dev/rfkill")
        .map_err(io_failure)?;
    let metadata = file.metadata().map_err(io_failure)?;
    if !metadata.file_type().is_char_device() || metadata.uid() != 0 {
        return Err(Failure::InvalidResponse);
    }
    file.write_all(&rfkill_event(kind, powered))
        .map_err(io_failure)?;
    loop {
        budget.check()?;
        let after = switches(Path::new(RFKILL), kind)?;
        if after
            .iter()
            .map(|switch| switch.index)
            .ne(before.iter().map(|switch| switch.index))
        {
            return Err(Failure::Failed);
        }
        if powered && after.iter().any(|switch| switch.hard) {
            return Err(Failure::Unavailable);
        }
        if after.iter().all(|switch| switch.soft != powered) {
            return Ok(());
        }
        thread::sleep(POLL);
    }
}

fn wifi_power(powered: bool, budget: Budget) -> Result<(), Failure> {
    nmcli(
        &["radio", "wifi", if powered { "on" } else { "off" }],
        budget,
    )
    .map(|_| ())
}

struct Running(Child);
impl Drop for Running {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

fn nonblocking(pipe: &impl AsRawFd) -> Result<(), Failure> {
    // SAFETY: fcntl uses a live pipe descriptor and no pointer arguments.
    let flags = unsafe { nix::libc::fcntl(pipe.as_raw_fd(), nix::libc::F_GETFL) };
    if flags < 0
        || unsafe {
            nix::libc::fcntl(
                pipe.as_raw_fd(),
                nix::libc::F_SETFL,
                flags | nix::libc::O_NONBLOCK,
            )
        } < 0
    {
        return Err(Failure::Unavailable);
    }
    Ok(())
}

fn drain(pipe: &mut impl Read, output: &mut Vec<u8>) -> Result<(), Failure> {
    let mut bytes = [0; 4096];
    loop {
        match pipe.read(&mut bytes) {
            Ok(0) => return Ok(()),
            Ok(size) => {
                if output.len() + size > MAX_IO {
                    return Err(Failure::InvalidResponse);
                }
                output.extend_from_slice(&bytes[..size]);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(io_failure(error)),
        }
    }
}

fn nmcli(args: &[&str], budget: Budget) -> Result<String, Failure> {
    budget.check()?;
    let parent = std::process::id();
    let mut command = Command::new(NMCLI);
    command
        .args(["--colors", "no", "--wait", "8"])
        .args(args)
        .env_clear()
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // SAFETY: only async-signal-safe syscalls in this post-fork closure. Root's
    // scan dies with its helper even if the helper watchdog exits during nmcli.
    unsafe {
        command.pre_exec(move || {
            if nix::libc::prctl(
                nix::libc::PR_SET_PDEATHSIG,
                nix::libc::SIGKILL as nix::libc::c_ulong,
                0 as nix::libc::c_ulong,
                0 as nix::libc::c_ulong,
                0 as nix::libc::c_ulong,
            ) < 0
            {
                return Err(io::Error::last_os_error());
            }
            if nix::libc::getppid() as u32 != parent {
                nix::libc::_exit(124);
            }
            Ok(())
        });
    }
    let mut running = Running(command.spawn().map_err(io_failure)?);
    let mut stdout = running.0.stdout.take().ok_or(Failure::Unavailable)?;
    let mut stderr = running.0.stderr.take().ok_or(Failure::Unavailable)?;
    nonblocking(&stdout)?;
    nonblocking(&stderr)?;
    let mut output = Vec::new();
    let mut errors = Vec::new();
    loop {
        budget.check()?;
        drain(&mut stdout, &mut output)?;
        drain(&mut stderr, &mut errors)?;
        if let Some(status) = running.0.try_wait().map_err(io_failure)? {
            drain(&mut stdout, &mut output)?;
            drain(&mut stderr, &mut errors)?;
            if !status.success() {
                return Err(Failure::Failed);
            }
            return String::from_utf8(output).map_err(|_| Failure::InvalidResponse);
        }
        thread::sleep(POLL);
    }
}

fn mac(value: &str) -> bool {
    value.len() == 17
        && value.bytes().enumerate().all(|(index, byte)| {
            if index % 3 == 2 {
                byte == b':'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

fn scan_entries(text: &str, interface: &str) -> Result<Vec<Entry>, Failure> {
    if text.len() > MAX_IO || !interface_name(interface) {
        return Err(Failure::InvalidResponse);
    }
    let mut entries = Vec::new();
    let mut seen = BTreeSet::new();
    for line in text.lines() {
        let row = fields(line);
        let [address, ssid, signal, freq, security] = row.as_slice() else {
            return Err(Failure::InvalidResponse);
        };
        if !mac(address) || signal.parse::<u8>().ok().filter(|s| *s <= 100).is_none() {
            return Err(Failure::InvalidResponse);
        }
        if seen.insert(address.clone()) {
            entries.push(Entry {
                id: format!("{interface}/{address}"),
                name: public_text(if ssid.is_empty() { address } else { ssid }, 128),
                detail: public_text(
                    &format!("{address} · {interface} · {signal}% · {freq} · {security}"),
                    256,
                ),
            });
        }
        if entries.len() == MAX_ENTRIES {
            break;
        }
    }
    Ok(entries)
}
fn wifi_list(budget: Budget) -> Result<Vec<Entry>, Failure> {
    let before = switches(Path::new(RFKILL), Kind::Wifi)?;
    let names = interfaces(Path::new(NET))?;
    if wifi_state(&before, &names).powered != Some(true) {
        return Err(Failure::Unavailable);
    }
    let mut entries = Vec::new();
    for name in &names {
        entries.extend(scan_entries(
            &nmcli(
                &[
                    "-t",
                    "-f",
                    "BSSID,SSID,SIGNAL,FREQ,SECURITY",
                    "device",
                    "wifi",
                    "list",
                    "ifname",
                    name,
                    "--rescan",
                    "yes",
                ],
                budget,
            )?,
            name,
        )?);
        if entries.len() >= MAX_ENTRIES {
            break;
        }
    }
    if switches(Path::new(RFKILL), Kind::Wifi)? != before || interfaces(Path::new(NET))? != names {
        return Err(Failure::Failed);
    }
    entries.truncate(MAX_ENTRIES);
    Ok(entries)
}

type Properties = HashMap<String, OwnedVariant>;
type Objects = HashMap<OwnedObjectPath, HashMap<String, Properties>>;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Adapter {
    path: String,
    address: String,
    name: String,
    powered: bool,
}
struct Inventory {
    adapters: Vec<Adapter>,
    devices: Vec<Entry>,
}

fn decode<T: for<'d> DynamicDeserialize<'d>>(message: zbus::Message) -> Result<T, Failure> {
    if message.body().len() > MAX_IO {
        return Err(Failure::InvalidResponse);
    }
    message
        .body()
        .deserialize()
        .map_err(|_| Failure::InvalidResponse)
}

fn boolean(properties: &mut Properties, name: &str) -> Result<bool, Failure> {
    bool::try_from(properties.remove(name).ok_or(Failure::InvalidResponse)?)
        .map_err(|_| Failure::InvalidResponse)
}

fn string(properties: &mut Properties, name: &str) -> Result<String, Failure> {
    let value = String::try_from(properties.remove(name).ok_or(Failure::InvalidResponse)?)
        .map_err(|_| Failure::InvalidResponse)?;
    if value.len() > 4096 {
        return Err(Failure::InvalidResponse);
    }
    Ok(value)
}

fn inventory(objects: Objects) -> Result<Inventory, Failure> {
    if objects.len() > 512 {
        return Err(Failure::InvalidResponse);
    }
    let mut adapters = Vec::new();
    let mut devices = Vec::new();
    for (path, mut interfaces) in objects {
        if interfaces.len() > 32 {
            return Err(Failure::InvalidResponse);
        }
        if let Some(mut properties) = interfaces.remove(ADAPTER) {
            if properties.len() > 64 || !adapter_path(path.as_str()) || adapters.len() >= 8 {
                return Err(Failure::InvalidResponse);
            }
            let address = string(&mut properties, "Address")?;
            if !mac(&address) {
                return Err(Failure::InvalidResponse);
            }
            let name = public_text(&string(&mut properties, "Alias")?, 128);
            adapters.push(Adapter {
                path: path.to_string(),
                address,
                name,
                powered: boolean(&mut properties, "Powered")?,
            });
        }
        if let Some(mut properties) = interfaces.remove(DEVICE) {
            if properties.len() > 64 || path.as_str().len() > 128 {
                return Err(Failure::InvalidResponse);
            }
            let address = string(&mut properties, "Address")?;
            if !mac(&address) {
                return Err(Failure::InvalidResponse);
            }
            let adapter = OwnedObjectPath::try_from(
                properties
                    .remove("Adapter")
                    .ok_or(Failure::InvalidResponse)?,
            )
            .map_err(|_| Failure::InvalidResponse)?;
            if !adapter_path(adapter.as_str())
                || !path.as_str().starts_with(&format!("{adapter}/dev_"))
            {
                return Err(Failure::InvalidResponse);
            }
            let alias = public_text(&string(&mut properties, "Alias")?, 128);
            let mut detail = vec![address.clone()];
            // Technical property names stay recognizable without injecting
            // backend English error text into the localized UI.
            if boolean(&mut properties, "Paired")? {
                detail.push("Paired".into());
            }
            if boolean(&mut properties, "Connected")? {
                detail.push("Connected".into());
            }
            if let Some(value) = properties.remove("RSSI") {
                let rssi = i16::try_from(value).map_err(|_| Failure::InvalidResponse)?;
                detail.push(format!("{rssi} dBm"));
            }
            devices.push(Entry {
                id: path.to_string(),
                name: if alias.is_empty() { address } else { alias },
                detail: detail.join(" · "),
            });
        }
    }
    adapters.sort_by(|a, b| a.path.cmp(&b.path));
    devices.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    devices.truncate(MAX_ENTRIES);
    Ok(Inventory { adapters, devices })
}

fn adapter_path(path: &str) -> bool {
    path.strip_prefix("/org/bluez/hci").is_some_and(|suffix| {
        !suffix.is_empty()
            && suffix.len() < 10
            && suffix.bytes().all(|value| value.is_ascii_digit())
    })
}

struct Bluez {
    connection: zbus::Connection,
    owner: String,
}

impl Bluez {
    fn open(budget: Budget) -> Result<Self, Failure> {
        // Fixed system socket: DBUS_SYSTEM_BUS_ADDRESS / caller environment
        // cannot redirect a root helper to an attacker-controlled service.
        let builder = zbus::connection::Builder::address("unix:path=/run/dbus/system_bus_socket");
        let connection = budget.dbus(builder.build())?;
        // Activation is standard BlueZ, configured AutoEnable=false in APK.
        let _: u32 = decode(budget.dbus(connection.call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "StartServiceByName",
            &(BLUEZ, 0u32),
        ))?)?;
        let owner = Self::owner(&connection, budget)?;
        Ok(Self { connection, owner })
    }

    fn owner(connection: &zbus::Connection, budget: Budget) -> Result<String, Failure> {
        let owner: String = decode(budget.dbus(connection.call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "GetNameOwner",
            &(BLUEZ,),
        ))?)?;
        if owner.len() > 255 || zbus::names::UniqueName::try_from(owner.as_str()).is_err() {
            return Err(Failure::InvalidResponse);
        }
        Ok(owner)
    }

    fn current(&self, budget: Budget) -> Result<Inventory, Failure> {
        let objects: Objects = decode(budget.dbus(self.connection.call_method(
            Some(self.owner.as_str()),
            "/",
            Some("org.freedesktop.DBus.ObjectManager"),
            "GetManagedObjects",
            &(),
        ))?)?;
        self.check_owner(budget)?;
        inventory(objects)
    }

    fn check_owner(&self, budget: Budget) -> Result<(), Failure> {
        if Self::owner(&self.connection, budget)? != self.owner {
            Err(Failure::Failed)
        } else {
            Ok(())
        }
    }

    fn power(&self, adapter: &Adapter, powered: bool, budget: Budget) -> Result<(), Failure> {
        self.check_owner(budget)?;
        let _: () = decode(budget.dbus(self.connection.call_method(
            Some(self.owner.as_str()),
            adapter.path.as_str(),
            Some("org.freedesktop.DBus.Properties"),
            "Set",
            &(ADAPTER, "Powered", Variant::from(powered)),
        ))?)?;
        Ok(())
    }

    fn discovery(&self, adapter: &Adapter, start: bool, budget: Budget) -> Result<(), Failure> {
        let _: () = decode(budget.dbus(self.connection.call_method(
            Some(self.owner.as_str()),
            adapter.path.as_str(),
            Some(ADAPTER),
            if start {
                "StartDiscovery"
            } else {
                "StopDiscovery"
            },
            &(),
        ))?)?;
        Ok(())
    }
}

fn bluetooth_state(switches: &[Switch], inventory: &Inventory) -> State {
    let detail = public_text(
        &inventory
            .adapters
            .iter()
            .map(|adapter| adapter.name.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        128,
    );
    if switches.iter().any(|switch| switch.hard) {
        return blocked(detail);
    }
    if inventory.adapters.is_empty() {
        if all_soft_blocked(switches) {
            return State {
                availability: Availability::Available,
                powered: Some(false),
                detail,
            };
        }
        return if switches.is_empty() {
            absent()
        } else {
            unavailable()
        };
    }
    let powered = uniform(inventory.adapters.iter().map(|adapter| adapter.powered));
    // BlueZ and rfkill disagreement is an uncertain state, not a guessed Off.
    if powered == Some(true) && switches.iter().any(|switch| switch.soft) {
        return unavailable();
    }
    State {
        availability: Availability::Available,
        powered,
        detail,
    }
}

fn all_soft_blocked(switches: &[Switch]) -> bool {
    !switches.is_empty() && switches.iter().all(|switch| switch.soft && !switch.hard)
}

fn bluetooth_snapshot(budget: Budget) -> State {
    let switches = match switches(Path::new(RFKILL), Kind::Bluetooth) {
        Ok(value) => value,
        Err(_) => return unavailable(),
    };
    if switches.iter().any(|switch| switch.hard) {
        return blocked(String::new());
    }
    // rfkill itself proves Off even if a driver removed its BlueZ adapter.
    // Preserve the explicit On control so the user can unblock it again.
    if all_soft_blocked(&switches) {
        return State {
            availability: Availability::Available,
            powered: Some(false),
            detail: String::new(),
        };
    }
    if switches.is_empty() {
        match hardware_present(Path::new(BLUETOOTH)) {
            Ok(false) => return absent(),
            Ok(true) => {}
            Err(_) => return unavailable(),
        }
    }
    match Bluez::open(budget).and_then(|bluez| bluez.current(budget)) {
        Ok(inventory) => bluetooth_state(&switches, &inventory),
        Err(_) => unavailable(),
    }
}

fn bluetooth_power(powered: bool, budget: Budget) -> Result<(), Failure> {
    let switches = switches(Path::new(RFKILL), Kind::Bluetooth)?;
    if switches.iter().any(|switch| switch.hard) {
        return Err(Failure::Unavailable);
    }
    if switches.is_empty() && !hardware_present(Path::new(BLUETOOTH))? {
        return Err(Failure::Unavailable);
    }
    // Some controllers cannot register with BlueZ while rfkill is blocked.
    // Unblock first only for the explicit ON operation.
    if powered && !switches.is_empty() {
        write_rfkill(Kind::Bluetooth, true, &switches, budget)?;
    }
    let bluez = Bluez::open(budget)?;
    let mut before = bluez.current(budget)?;
    while before.adapters.is_empty() {
        if !powered {
            return Err(Failure::Unavailable);
        }
        // Unblocking and adapter registration are asynchronous. This wait is
        // within the operation's existing budget, never a second full timeout.
        budget.check()?;
        thread::sleep(POLL);
        before = bluez.current(budget)?;
    }
    for adapter in &before.adapters {
        bluez.power(adapter, powered, budget)?;
    }
    loop {
        budget.check()?;
        let after = bluez.current(budget)?;
        if after
            .adapters
            .iter()
            .map(|a| (&a.path, &a.address))
            .ne(before.adapters.iter().map(|a| (&a.path, &a.address)))
        {
            return Err(Failure::Failed);
        }
        if after
            .adapters
            .iter()
            .all(|adapter| adapter.powered == powered)
        {
            break;
        }
        thread::sleep(POLL);
    }
    if !powered && !switches.is_empty() {
        let current = self::switches(Path::new(RFKILL), Kind::Bluetooth)?;
        if current
            .iter()
            .map(|s| s.index)
            .ne(switches.iter().map(|s| s.index))
        {
            return Err(Failure::Failed);
        }
        write_rfkill(Kind::Bluetooth, false, &current, budget)?;
    }
    Ok(())
}

fn bluetooth_list(budget: Budget) -> Result<Vec<Entry>, Failure> {
    let switches = switches(Path::new(RFKILL), Kind::Bluetooth)?;
    if switches.iter().any(|switch| switch.hard) {
        return Err(Failure::Unavailable);
    }
    if switches.is_empty() && !hardware_present(Path::new(BLUETOOTH))? {
        return Err(Failure::Unavailable);
    }
    let bluez = Bluez::open(budget)?;
    let before = bluez.current(budget)?;
    if before.adapters.is_empty() {
        return Err(Failure::Unavailable);
    }
    let mut started = Vec::new();
    let scan_result = (|| {
        for adapter in &before.adapters {
            if adapter.powered {
                bluez.discovery(adapter, true, budget)?;
                started.push(adapter);
            }
        }
        if !started.is_empty() {
            let until = Instant::now() + Duration::from_secs(2);
            while Instant::now() < until {
                budget.check()?;
                thread::sleep(POLL);
            }
        }
        let after = bluez.current(budget)?;
        if after.adapters != before.adapters {
            return Err(Failure::Failed);
        }
        Ok(after.devices)
    })();
    // BlueZ discovery is shared: stop only sessions started on THIS connection.
    // A failed/expired Stop is still cleaned up by dropping this connection;
    // never switch Discoverable or stop another application's discovery.
    let cleanup = Budget::new(1);
    let mut cleanup_result = Ok(());
    for adapter in started {
        if let Err(error) = bluez.discovery(adapter, false, cleanup) {
            cleanup_result = Err(error);
        }
    }
    if let Err(error) = cleanup.dbus(bluez.connection.close()) {
        cleanup_result = Err(error);
    }
    match scan_result {
        Ok(entries) => {
            cleanup_result?;
            Ok(entries)
        }
        Err(error) => Err(error),
    }
}

pub fn execute(operation: Operation) -> Reply {
    let budget = Budget::new(6);
    let mut entries = Vec::new();
    let mut result = match operation {
        Operation::Refresh => Ok(()),
        Operation::SetPowered {
            kind: Kind::Wifi,
            powered,
        } => wifi_power(powered, budget),
        Operation::SetPowered {
            kind: Kind::Bluetooth,
            powered,
        } => bluetooth_power(powered, budget),
        Operation::List { kind } => match if kind == Kind::Wifi {
            wifi_list(budget)
        } else {
            bluetooth_list(budget)
        } {
            Ok(found) => {
                entries = found;
                Ok(())
            }
            Err(error) => Err(error),
        },
    };
    // Every response carries a fresh read-back, including failure/partial-write
    // paths. No successful command alone can turn a menu label into On/Off.
    let snapshot = Snapshot {
        wifi: wifi_snapshot(),
        bluetooth: bluetooth_snapshot(Budget::new(if operation == Operation::Refresh {
            6
        } else {
            2
        })),
    };
    if let Operation::SetPowered { kind, powered } = operation {
        let state = snapshot.state(kind);
        if result.is_ok()
            && (state.availability != Availability::Available || state.powered != Some(powered))
        {
            result = Err(Failure::Failed);
        }
    }
    Reply {
        operation,
        snapshot,
        entries,
        result,
    }
}

#[cfg(test)]
#[path = "unit_tests/radio_system.rs"]
mod tests;
