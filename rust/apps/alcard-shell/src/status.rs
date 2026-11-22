//! Read-only Linux status observations. A route is never an Internet probe.
//! No command execution, credentials, network traffic or privileged writes.
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::{self, Read},
    net::{Ipv4Addr, Ipv6Addr},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChargeState {
    #[default]
    Unknown,
    Charging,
    Discharging,
    Full,
    NotCharging,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BatteryState {
    #[default]
    Unknown,
    Absent,
    Present {
        percent: Option<u8>,
        charge: ChargeState,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LinkState {
    #[default]
    Unknown,
    Absent,
    Disabled,
    Disconnected,
    Connecting,
    /// Carrier exists, but no non-link-local unicast address is configured.
    LinkOnly,
    /// A configured address exists; no matching ordinary default route exists.
    Addressed,
    /// Address and default route match this interface and address family.
    /// This does not establish DNS, captive-portal or Internet reachability.
    Routed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NetworkStatus {
    pub state: LinkState,
    /// Zero to four bars derived from a measured negative Wi-Fi dBm value.
    /// None is unknown, not zero signal. Ethernet never has signal bars.
    pub strength: Option<u8>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StatusSnapshot {
    pub battery: BatteryState,
    pub wifi: NetworkStatus,
    pub ethernet: NetworkStatus,
}

/// A completed read whose timestamp is the START of the observation. A sample
/// already in flight when refresh is requested must not satisfy that barrier.
/// A source epoch exists only for one coherent, identifiable system battery;
/// unavailable, ambiguous or changing power data cannot authorize an action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BatteryObservation {
    pub state: BatteryState,
    pub observed_at: Instant,
    pub sequence: u64,
    pub source_epoch: Option<u64>,
    /// None means no external supply exposes an online observation. A failed
    /// or malformed reading also invalidates source_epoch, never implies AC off.
    pub external_power: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Observation {
    pub status: StatusSnapshot,
    pub battery: BatteryObservation,
}

struct Mailbox {
    update: Mutex<Option<Observation>>,
    pending: AtomicBool,
    stopped: AtomicBool,
}

enum Command {
    Refresh,
    Stop,
}

/// One worker and one coalesced observation; a slow UI cannot accumulate polls.
/// Every read is delivered to battery policy, even when the visible snapshot
/// did not change. The UI compares StatusSnapshot before requesting a redraw.
pub struct Worker {
    mailbox: Arc<Mailbox>,
    commands: mpsc::SyncSender<Command>,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    pub fn start(wake: impl Fn() + Send + 'static) -> io::Result<Self> {
        let mailbox = Arc::new(Mailbox {
            update: Mutex::new(None),
            pending: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
        });
        let shared = Arc::clone(&mailbox);
        let (commands, receiver) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("alcard-status".into())
            .spawn(move || {
                let sources = Sources::system();
                let mut sampler = Sampler::default();
                while !shared.stopped.load(Ordering::Acquire) {
                    let Some(observation) = sampler.sample(&sources, &addresses()) else {
                        // Monotonic identities cannot wrap and authorize an old
                        // observation again, even over an impossible lifetime.
                        break;
                    };
                    if shared.stopped.load(Ordering::Acquire) {
                        break;
                    }
                    publish(&shared, observation, &wake);
                    match receiver.recv_timeout(Duration::from_secs(5)) {
                        Ok(Command::Refresh) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Ok(Command::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
            })?;
        Ok(Self {
            mailbox,
            commands,
            thread: Some(thread),
        })
    }

    pub fn take_update(&self) -> Option<Observation> {
        take_update(&self.mailbox)
    }

    /// Request a new read without waiting on sysfs or queueing unbounded work.
    /// Full means a refresh is already pending and therefore also satisfies
    /// this request once its START timestamp crosses the caller's barrier.
    pub fn refresh(&self) -> bool {
        if self.mailbox.stopped.load(Ordering::Acquire) {
            return false;
        }
        matches!(
            self.commands.try_send(Command::Refresh),
            Ok(()) | Err(mpsc::TrySendError::Full(Command::Refresh))
        )
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.mailbox.stopped.store(true, Ordering::Release);
        let _ = self.commands.try_send(Command::Stop);
        // Never wait for a five-second poll or an unresponsive kernel driver
        // on the Wayland thread. The stop flag suppresses an in-flight result;
        // recv_timeout wakes immediately and releases all worker resources.
        if let Some(thread) = self.thread.take() {
            if thread.is_finished() {
                let _ = thread.join();
            }
        }
    }
}

fn publish(mailbox: &Mailbox, observation: Observation, wake: &impl Fn()) {
    let should_wake = {
        let mut update = mailbox.update.lock().unwrap_or_else(|e| e.into_inner());
        *update = Some(observation);
        !mailbox.pending.swap(true, Ordering::AcqRel)
    };
    if should_wake && !mailbox.stopped.load(Ordering::Acquire) {
        wake();
    }
}

fn take_update(mailbox: &Mailbox) -> Option<Observation> {
    let mut update = mailbox.update.lock().unwrap_or_else(|e| e.into_inner());
    let snapshot = update.take();
    // Clear under the same lock as publication: a concurrent publication
    // either belongs to this snapshot or creates a fresh wake afterwards.
    mailbox.pending.store(false, Ordering::Release);
    snapshot
}

#[derive(Default)]
struct Sampler {
    sequence: u64,
    epoch: u64,
    source: Option<SourceIdentity>,
}

impl Sampler {
    fn sample(&mut self, sources: &Sources, addresses: &AddressSnapshot) -> Option<Observation> {
        let observed_at = Instant::now();
        let power = power_reading(&sources.power);
        let (wifi, ethernet) = sources.networks(addresses);
        self.record(power, wifi, ethernet, observed_at)
    }

    fn record(
        &mut self,
        power: PowerReading,
        wifi: NetworkStatus,
        ethernet: NetworkStatus,
        observed_at: Instant,
    ) -> Option<Observation> {
        self.sequence = self.sequence.checked_add(1)?;
        let source_epoch = if let Some(source) = power.source {
            if self.source.as_ref() != Some(&source) {
                self.epoch = self.epoch.checked_add(1)?;
                self.source = Some(source);
            }
            Some(self.epoch)
        } else {
            // Keep the last known identity across unreadable/ambiguous data,
            // but publish no authority for this sample. Recovering the same
            // source is not a new discharge episode or another warning.
            None
        };
        Some(Observation {
            status: StatusSnapshot {
                battery: power.state,
                wifi,
                ethernet,
            },
            battery: BatteryObservation {
                state: power.state,
                observed_at,
                sequence: self.sequence,
                source_epoch,
                external_power: power.external_power,
            },
        })
    }
}

struct Sources {
    power: PathBuf,
    net: PathBuf,
    rfkill: PathBuf,
    wireless: PathBuf,
    route4: PathBuf,
    route6: PathBuf,
}

impl Sources {
    fn system() -> Self {
        Self {
            power: "/sys/class/power_supply".into(),
            net: "/sys/class/net".into(),
            rfkill: "/sys/class/rfkill".into(),
            wireless: "/proc/net/wireless".into(),
            route4: "/proc/net/route".into(),
            route6: "/proc/net/ipv6_route".into(),
        }
    }

    fn networks(&self, addresses: &AddressSnapshot) -> (NetworkStatus, NetworkStatus) {
        let Ok(entries) = fs::read_dir(&self.net) else {
            return (NetworkStatus::default(), NetworkStatus::default());
        };
        let wireless = read_text(&self.wireless, 256 * 1024)
            .map(|text| wireless_strengths(&text))
            .unwrap_or_default();
        let radio = radios(&self.rfkill);
        let routes = Routes {
            ipv4: read_text(&self.route4, 256 * 1024)
                .ok()
                .and_then(|text| parse_routes4(&text)),
            ipv6: read_text(&self.route6, 256 * 1024)
                .ok()
                .and_then(|text| parse_routes6(&text)),
        };
        let mut wifi = Vec::new();
        let mut ethernet = Vec::new();
        let mut unknown = false;
        for entry in entries {
            let Ok(entry) = entry else {
                unknown = true;
                continue;
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == "lo" {
                continue;
            }
            let path = entry.path();
            let wireless_device = exists(&path.join("wireless"))
                .and_then(|a| exists(&path.join("phy80211")).map(|b| a || b));
            let is_wifi = match wireless_device {
                Ok(found) => found || wireless.contains_key(&name),
                Err(_) => {
                    unknown = true;
                    continue;
                }
            };
            if is_wifi {
                let blocked = phy_blocked(&path.join("phy80211")).or(radio.blocked);
                wifi.push(network(
                    &path,
                    &name,
                    true,
                    blocked,
                    wireless.get(&name).copied().flatten(),
                    addresses,
                    &routes,
                ));
            } else {
                // ARPHRD_ETHER also includes veth, bridges and tunnels. Only
                // device-backed interfaces describe a physical Ethernet link.
                match (small(&path.join("type")), exists(&path.join("device"))) {
                    (Ok(kind), Ok(true)) if kind.trim() == "1" => {
                        ethernet.push(network(&path, &name, false, None, None, addresses, &routes));
                    }
                    (Err(_), _) | (_, Err(_)) => unknown = true,
                    _ => {}
                }
            }
        }
        let mut wifi_status = strongest(&wifi, unknown || radio.failed);
        if wifi.is_empty() && radio.has_wlan {
            wifi_status.state = if radio.blocked == Some(true) {
                LinkState::Disabled
            } else {
                LinkState::Unknown
            };
        }
        (wifi_status, strongest(&ethernet, unknown))
    }
}

fn read_text(path: &Path, limit: usize) -> io::Result<String> {
    let mut output = String::new();
    File::open(path)?
        .take(limit as u64 + 1)
        .read_to_string(&mut output)?;
    if output.len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "status file too large",
        ));
    }
    Ok(output)
}

fn small(path: &Path) -> io::Result<String> {
    read_text(path, 4096)
}

fn exists(path: &Path) -> io::Result<bool> {
    match fs::metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn bit(text: &str) -> Option<bool> {
    match text.trim() {
        "0" => Some(false),
        "1" => Some(true),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SourceIdentity {
    path: PathBuf,
    device: u64,
    inode: u64,
}

fn source_identity(path: &Path) -> io::Result<SourceIdentity> {
    let canonical = fs::canonicalize(path)?;
    let metadata = fs::metadata(path)?;
    if !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "power source is not a directory",
        ));
    }
    Ok(SourceIdentity {
        path: canonical,
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

#[derive(Debug, PartialEq, Eq)]
struct SupplyReading {
    source: SourceIdentity,
    /// None is a peripheral or an external power supply, not a system battery.
    battery: Option<BatteryState>,
    online: Option<bool>,
    reliable: bool,
}

struct PowerReading {
    state: BatteryState,
    source: Option<SourceIdentity>,
    external_power: Option<bool>,
}

impl Default for PowerReading {
    fn default() -> Self {
        Self {
            state: BatteryState::Unknown,
            source: None,
            external_power: None,
        }
    }
}

fn optional_small(path: &Path) -> io::Result<Option<String>> {
    match small(path) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn read_supply(path: &Path, source: SourceIdentity) -> io::Result<SupplyReading> {
    let kind = small(&path.join("type"))?;
    let scope = optional_small(&path.join("scope"))?;
    let mut reading = SupplyReading {
        source,
        battery: None,
        online: None,
        reliable: true,
    };
    match scope.as_deref().map(str::trim) {
        Some("Device") => return Ok(reading),
        None | Some("System") => {}
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unknown power scope",
            ))
        }
    }
    if kind.trim() != "Battery" {
        // A declared system power supply with unreadable/absent online data
        // must not be interpreted as unplugged. With no external supplies at
        // all, the explicit Discharging battery status remains usable.
        reading.online = optional_small(&path.join("online"))?
            .as_deref()
            .and_then(bit);
        reading.reliable = reading.online.is_some();
        return Ok(reading);
    }
    let present = optional_small(&path.join("present"))?
        .as_deref()
        .map(|text| {
            bit(text).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "unknown battery presence")
            })
        })
        .transpose()?;
    if present == Some(false) {
        reading.battery = Some(BatteryState::Absent);
        return Ok(reading);
    }
    let percent = small(&path.join("capacity"))
        .ok()
        .and_then(|text| text.trim().parse::<u8>().ok())
        .filter(|value| *value <= 100);
    let charge = match small(&path.join("status")).as_deref().map(str::trim) {
        Ok("Charging") => ChargeState::Charging,
        Ok("Discharging") => ChargeState::Discharging,
        Ok("Full") => ChargeState::Full,
        Ok("Not charging") => ChargeState::NotCharging,
        _ => ChargeState::Unknown,
    };
    reading.reliable = percent.is_some() && charge != ChargeState::Unknown;
    // The present attribute is optional, but a missing attribute never
    // invents a battery or a charge percentage.
    reading.battery = Some(
        if present == Some(true) || percent.is_some() || charge != ChargeState::Unknown {
            BatteryState::Present { percent, charge }
        } else {
            BatteryState::Unknown
        },
    );
    Ok(reading)
}

fn power_paths(root: &Path) -> io::Result<Vec<PathBuf>> {
    const MAX_SUPPLIES: usize = 64;
    let mut paths = Vec::new();
    for entry in fs::read_dir(root)? {
        if paths.len() >= MAX_SUPPLIES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "too many power supplies",
            ));
        }
        paths.push(entry?.path());
    }
    paths.sort();
    Ok(paths)
}

fn power_pass(root: &Path) -> io::Result<Vec<SupplyReading>> {
    let paths = power_paths(root)?;
    let mut readings = Vec::with_capacity(paths.len());
    for path in &paths {
        let source = source_identity(path)?;
        let reading = read_supply(path, source.clone())?;
        if source_identity(path)? != source {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "power source changed during read",
            ));
        }
        readings.push(reading);
    }
    if power_paths(root)? != paths {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "power supplies changed during read",
        ));
    }
    Ok(readings)
}

fn combine_power_passes(before: &[SupplyReading], after: &[SupplyReading]) -> PowerReading {
    let mut result = PowerReading {
        state: BatteryState::Absent,
        ..PowerReading::default()
    };
    let mut present = 0;
    let mut unknown = false;
    let coherent = before == after;
    let reliable = coherent && after.iter().all(|reading| reading.reliable);
    for reading in after {
        match reading.battery {
            Some(state @ BatteryState::Present { .. }) => {
                present += 1;
                if present == 1 {
                    // Retain the stable display choice; multiple batteries
                    // never authorize an action through an arbitrary first one.
                    result.state = state;
                    result.source = Some(reading.source.clone());
                }
            }
            Some(BatteryState::Unknown) => unknown = true,
            Some(BatteryState::Absent) | None => {}
        }
        if let Some(online) = reading.online {
            result.external_power = Some(result.external_power.unwrap_or(false) || online);
        }
    }
    if present == 0 && unknown {
        result.state = BatteryState::Unknown;
    }
    if !reliable || unknown || present != 1 {
        result.source = None;
    }
    if !coherent {
        // A transition can cancel a pending action, but cannot reset the
        // discharge episode as if stable external power had been confirmed.
        result.external_power = None;
    }
    result
}

fn power_reading(root: &Path) -> PowerReading {
    // Double observation catches a source replacement/hotplug, AC transition,
    // presence or charging-state change during the read. Sysfs has no atomic
    // multi-attribute snapshot; disagreement is never permission to suspend.
    let Ok(before) = power_pass(root) else {
        return PowerReading::default();
    };
    let Ok(after) = power_pass(root) else {
        return PowerReading::default();
    };
    combine_power_passes(&before, &after)
}

#[cfg(test)]
fn battery(root: &Path) -> BatteryState {
    power_reading(root).state
}

#[derive(Default)]
struct RadioState {
    has_wlan: bool,
    blocked: Option<bool>,
    failed: bool,
}

fn blocked(path: &Path) -> Option<bool> {
    let soft = small(&path.join("soft")).ok().and_then(|text| bit(&text));
    let hard = small(&path.join("hard")).ok().and_then(|text| bit(&text));
    match (soft, hard) {
        (Some(true), _) | (_, Some(true)) => Some(true),
        (Some(false), Some(false)) => Some(false),
        _ => None,
    }
}

fn radios(root: &Path) -> RadioState {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) => {
            return RadioState {
                failed: error.kind() != io::ErrorKind::NotFound,
                ..RadioState::default()
            }
        }
    };
    let mut result = RadioState::default();
    let mut values = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else {
            result.failed = true;
            continue;
        };
        match small(&entry.path().join("type")) {
            Ok(kind) if kind.trim() == "wlan" => {
                result.has_wlan = true;
                values.push(blocked(&entry.path()));
            }
            Err(_) => result.failed = true,
            _ => {}
        }
    }
    // A mixed set cannot be attributed to an arbitrary interface. A per-phy
    // rfkill child, when available, takes precedence over this global fallback.
    if !values.is_empty() && !result.failed && values.iter().all(|v| *v == values[0]) {
        result.blocked = values[0];
    }
    result
}

fn phy_blocked(phy: &Path) -> Option<bool> {
    let entries = fs::read_dir(phy).ok()?;
    let values: Vec<_> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("rfkill"))
        .map(|entry| blocked(&entry.path()))
        .collect();
    if values.contains(&Some(true)) {
        Some(true)
    } else if !values.is_empty() && values.iter().all(|value| *value == Some(false)) {
        Some(false)
    } else {
        None
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Families {
    ipv4: bool,
    ipv6: bool,
}

enum AddressSnapshot {
    Known(HashMap<String, Families>),
    Unknown,
}

fn configured_v4(ip: Ipv4Addr) -> bool {
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_multicast()
        || ip.is_broadcast())
}

fn configured_v6(ip: Ipv6Addr) -> bool {
    !(ip.is_unspecified() || ip.is_loopback() || ip.is_unicast_link_local() || ip.is_multicast())
}

fn addresses() -> AddressSnapshot {
    let Ok(entries) = nix::ifaddrs::getifaddrs() else {
        return AddressSnapshot::Unknown;
    };
    let mut result = HashMap::<String, Families>::new();
    for entry in entries {
        let Some(address) = entry.address else {
            continue;
        };
        let family = result.entry(entry.interface_name).or_default();
        if let Some(address) = address.as_sockaddr_in() {
            family.ipv4 |= configured_v4(address.ip());
        } else if let Some(address) = address.as_sockaddr_in6() {
            family.ipv6 |= configured_v6(address.ip());
        }
    }
    AddressSnapshot::Known(result)
}

struct Routes {
    ipv4: Option<HashSet<String>>,
    ipv6: Option<HashSet<String>>,
}

fn parse_routes4(text: &str) -> Option<HashSet<String>> {
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let header = lines.next()?;
    if !header.split_whitespace().eq([
        "Iface",
        "Destination",
        "Gateway",
        "Flags",
        "RefCnt",
        "Use",
        "Metric",
        "Mask",
        "MTU",
        "Window",
        "IRTT",
    ]) {
        return None;
    }
    let mut routes = HashSet::new();
    for line in lines {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() < 8 {
            return None;
        }
        let destination = u32::from_str_radix(fields[1], 16).ok()?;
        let flags = u32::from_str_radix(fields[3], 16).ok()?;
        let mask = u32::from_str_radix(fields[7], 16).ok()?;
        if destination == 0 && mask == 0 && flags & 1 != 0 && flags & 0x200 == 0 {
            routes.insert(fields[0].into());
        }
    }
    Some(routes)
}

fn parse_routes6(text: &str) -> Option<HashSet<String>> {
    let mut routes = HashSet::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() != 10 {
            return None;
        }
        let destination = u128::from_str_radix(fields[0], 16).ok()?;
        let prefix = u8::from_str_radix(fields[1], 16).ok()?;
        let source_prefix = u8::from_str_radix(fields[3], 16).ok()?;
        let flags = u32::from_str_radix(fields[8], 16).ok()?;
        if destination == 0
            && prefix == 0
            && source_prefix == 0
            && flags & 1 != 0
            && flags & 0x200 == 0
        {
            routes.insert(fields[9].into());
        }
    }
    Some(routes)
}

fn signal_bars(level: &str) -> Option<u8> {
    let dbm = level.trim_end_matches('.').parse::<i16>().ok()?;
    if !(-127..=-1).contains(&dbm) {
        return None;
    }
    Some(match dbm {
        -50..=-1 => 4,
        -65..=-51 => 3,
        -75..=-66 => 2,
        -85..=-76 => 1,
        _ => 0,
    })
}

fn wireless_strengths(text: &str) -> HashMap<String, Option<u8>> {
    text.lines()
        .filter_map(|line| {
            let (name, values) = line.split_once(':')?;
            let name = name.trim();
            if name.is_empty() || name.chars().any(char::is_whitespace) {
                return None;
            }
            Some((
                name.into(),
                values.split_whitespace().nth(2).and_then(signal_bars),
            ))
        })
        .collect()
}

#[allow(clippy::too_many_arguments)] // Explicit read-only observation inputs.
fn network(
    path: &Path,
    name: &str,
    wifi: bool,
    radio_blocked: Option<bool>,
    strength: Option<u8>,
    addresses: &AddressSnapshot,
    routes: &Routes,
) -> NetworkStatus {
    let flags = small(&path.join("flags"))
        .ok()
        .and_then(|text| u32::from_str_radix(text.trim().trim_start_matches("0x"), 16).ok());
    let state = if radio_blocked == Some(true) || flags.is_some_and(|flags| flags & 1 == 0) {
        LinkState::Disabled
    } else {
        let carrier = small(&path.join("carrier"))
            .ok()
            .and_then(|text| bit(&text));
        let operstate = small(&path.join("operstate")).unwrap_or_default();
        let operstate = operstate.trim();
        if wifi && operstate == "dormant" {
            LinkState::Connecting
        } else if carrier == Some(false)
            || matches!(operstate, "down" | "lowerlayerdown" | "notpresent")
        {
            LinkState::Disconnected
        } else if carrier == Some(true) || operstate == "up" {
            addressed(name, addresses, routes)
        } else {
            LinkState::Unknown
        }
    };
    NetworkStatus {
        state,
        strength: if wifi
            && matches!(
                state,
                LinkState::LinkOnly | LinkState::Addressed | LinkState::Routed
            ) {
            strength
        } else {
            None
        },
    }
}

fn addressed(name: &str, addresses: &AddressSnapshot, routes: &Routes) -> LinkState {
    let AddressSnapshot::Known(addresses) = addresses else {
        return LinkState::Unknown;
    };
    let family = addresses.get(name).copied().unwrap_or_default();
    if !family.ipv4 && !family.ipv6 {
        return LinkState::LinkOnly;
    }
    if (family.ipv4
        && routes
            .ipv4
            .as_ref()
            .is_some_and(|routes| routes.contains(name)))
        || (family.ipv6
            && routes
                .ipv6
                .as_ref()
                .is_some_and(|routes| routes.contains(name)))
    {
        LinkState::Routed
    } else if (family.ipv4 && routes.ipv4.is_none()) || (family.ipv6 && routes.ipv6.is_none()) {
        LinkState::Unknown
    } else {
        LinkState::Addressed
    }
}

fn strongest(networks: &[NetworkStatus], incomplete: bool) -> NetworkStatus {
    let rank = |state| match state {
        LinkState::Routed => 6,
        LinkState::Addressed => 5,
        LinkState::LinkOnly => 4,
        LinkState::Connecting => 3,
        LinkState::Disconnected => 2,
        LinkState::Disabled => 1,
        LinkState::Unknown | LinkState::Absent => 0,
    };
    let best = networks
        .iter()
        .max_by_key(|item| (rank(item.state), item.strength));
    if let Some(best) = best {
        if rank(best.state) >= 3 {
            return *best;
        }
    }
    if incomplete || networks.iter().any(|item| item.state == LinkState::Unknown) {
        NetworkStatus::default()
    } else {
        best.copied().unwrap_or(NetworkStatus {
            state: LinkState::Absent,
            strength: None,
        })
    }
}

#[cfg(test)]
#[path = "unit_tests/status.rs"]
mod tests;
