//! Display-independent inventory and actions. Both frontends use these operations.
use crate::{
    network,
    preferences::{self, ConfigSource, Overrides},
    theme::IconStyle,
};
use std::{
    fs, io,
    net::{IpAddr, Ipv4Addr},
    path::Path,
    process::{Command, Stdio},
};
pub const BT: &str = "/usr/bin/bluetoothctl";
const WP: &str = "/usr/bin/wpctl";
const SINK: &str = "@DEFAULT_AUDIO_SINK@";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Page {
    Home,
    System,
    Language,
    Layouts,
    Clock,
    Network,
    Wifi(String),
    Device(String),
    Profiles,
    Profile(String),
    Bluetooth,
    BtDevice(String),
    Power,
    Sound,
    Appearance,
}
impl Page {
    pub fn title(&self) -> &'static str {
        match self {
            Self::Home => "app.settings",
            Self::System => "s.system",
            Self::Language => "s.language",
            Self::Layouts => "s.layouts",
            Self::Clock => "step.time",
            Self::Network => "step.network",
            Self::Wifi(_) => "s.nearby",
            Self::Device(_) => "s.interface",
            Self::Profiles => "s.profiles",
            Self::Profile(_) => "s.profile",
            Self::Bluetooth => "s.bluetooth",
            Self::BtDevice(_) => "s.device",
            Self::Power => "s.power",
            Self::Sound => "s.sound",
            Self::Appearance => "s.appearance",
        }
    }
}
#[derive(Clone, Debug)]
pub enum Operation {
    System(&'static str),
    Preference(Option<&'static str>, &'static str),
    Radio,
    Wifi { iface: String, ssid: String },
    Disconnect(String),
    ActivateDevice(String),
    Profile { uuid: String, field: &'static str },
    Bluetooth(&'static str, String),
    Volume,
    Mute,
    Output,
}
#[derive(Clone, Debug)]
pub enum Action {
    Read,
    Open(Page),
    Edit(Operation),
    Password(Operation),
    Select(Operation, Vec<String>),
    Apply(Operation, String),
    Confirm(Operation, String),
}
#[derive(Clone, Debug)]
pub struct Row {
    pub key: &'static str,
    pub value: String,
    pub action: Action,
}
fn row(key: &'static str, value: impl ToString, action: Action) -> Row {
    Row {
        key,
        value: value.to_string(),
        action,
    }
}
fn open(page: Page) -> Row {
    row(page.title(), "", Action::Open(page))
}
fn read(key: &'static str, value: impl ToString) -> Row {
    row(key, value, Action::Read)
}
fn edit(key: &'static str, value: impl ToString, op: Operation) -> Row {
    row(key, value, Action::Edit(op))
}
fn choices(op: Operation, values: &[&str]) -> Action {
    Action::Select(op, values.iter().map(|s| s.to_string()).collect())
}
fn toggle(key: &'static str, enabled: bool, op: Operation) -> Row {
    row(
        key,
        if enabled { "on" } else { "off" },
        Action::Apply(op, if enabled { "off" } else { "on" }.into()),
    )
}
fn file(path: &str) -> String {
    fs::read_to_string(path)
        .unwrap_or_default()
        .trim()
        .to_owned()
}
fn output(program: &str, args: &[&str]) -> io::Result<String> {
    let result = Command::new("/usr/bin/timeout")
        .args(["-k", "2", "20", program])
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .output()?;
    if !result.status.success() {
        return Err(io::Error::other(format!(
            "{}: {} {}",
            program,
            result.status,
            String::from_utf8_lossy(&result.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&result.stdout).trim().into())
}
fn bt(args: &[&str]) -> io::Result<String> {
    let text = output(BT, args)?;
    if text.contains("No default controller available")
        || text.lines().any(|l| l.trim_start().starts_with("Failed "))
    {
        return Err(io::Error::other(text));
    }
    Ok(text)
}
fn on_ntp() -> bool {
    ["chronyd", "ntpd"].iter().any(|s| {
        Path::new("/etc/runlevels/default").join(s).exists()
            || Path::new("/run/openrc/started").join(s).exists()
    })
}
fn layout_list() -> Vec<String> {
    let list = file("/etc/alcard-keyboards");
    if list.is_empty() {
        return vec!["us".into()];
    }
    list.split(',').map(String::from).collect()
}
fn nm(args: &[&str]) -> io::Result<String> {
    network::query(args).map(|s| s.trim_end().to_owned())
}
fn zero_or_rows(rows: Vec<Row>) -> Vec<Row> {
    if rows.is_empty() {
        vec![read("s.none", "")]
    } else {
        rows
    }
}
pub fn rows(page: &Page) -> io::Result<Vec<Row>> {
    use Operation as O;
    Ok(match page {
        Page::Home => [
            Page::System,
            Page::Language,
            Page::Clock,
            Page::Network,
            Page::Bluetooth,
            Page::Power,
            Page::Sound,
            Page::Appearance,
        ]
        .into_iter()
        .map(open)
        .collect(),
        Page::System => vec![
            edit(
                "home.hostname",
                file("/etc/hostname"),
                O::System("hostname"),
            ),
            read(
                "s.user",
                output("/usr/bin/id", &["-un"]).or_else(|_| output("/bin/id", &["-un"]))?,
            ),
            read(
                "home.session",
                match file("/etc/alcard/default-session").as_str() {
                    "alcard-shell" => "Graphics",
                    "labwc" => "Graphics Minimal",
                    "console" => "Console",
                    _ => "—",
                },
            ),
            read("s.alpine", file("/etc/alpine-release")),
            read("s.kernel", file("/proc/sys/kernel/osrelease")),
        ],
        Page::Language => {
            let mut rows = vec![
                row(
                    "s.syslang",
                    preferences::system_language().unwrap_or("—"),
                    choices(O::System("language"), &["en", "ru", "es"]),
                ),
                row(
                    "s.layouts",
                    layout_list().join(", "),
                    Action::Open(Page::Layouts),
                ),
                read("s.bootlayout", "US"),
            ];
            if !file("/etc/alcard/default-session").eq("console") {
                rows.push(read(
                    "s.xkb",
                    if layout_list().len() > 1 {
                        "Alt+Shift"
                    } else {
                        "US"
                    },
                ));
            }
            if fs::read_link("/proc/self/fd/0")
                .ok()
                .and_then(|p| p.to_str()?.strip_prefix("/dev/tty").map(str::to_owned))
                .is_some_and(|s| !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit()))
            {
                let active = file("/run/alcard-active-layout");
                rows.push(row(
                    "s.activevt",
                    if active.is_empty() { "us" } else { &active },
                    Action::Select(O::System("active-layout"), layout_list()),
                ));
            }
            rows
        }
        Page::Layouts => {
            let selected = layout_list();
            let mut rows = vec![read("s.original", "US")];
            for l in ["ru", "es", "de"] {
                let enabled = selected.iter().any(|s| s == l);
                let mut next = selected.clone();
                if enabled {
                    next.retain(|s| s != l)
                } else {
                    next.push(l.to_owned())
                }
                rows.push(row(
                    "",
                    format!(
                        "{}  [{}]",
                        l.to_uppercase(),
                        if enabled { "×" } else { " " }
                    ),
                    Action::Apply(O::System("layouts"), next.join(",")),
                ));
            }
            rows
        }
        Page::Clock => {
            let ntp = on_ntp();
            vec![
                edit("s.timezone", file("/etc/timezone"), O::System("timezone")),
                toggle("s.ntp", ntp, O::System("ntp")),
                row(
                    "s.datetime",
                    output("/bin/date", &["+%Y-%m-%d %H:%M:%S"])?,
                    if ntp {
                        Action::Read
                    } else {
                        Action::Edit(O::System("datetime"))
                    },
                ),
            ]
        }
        Page::Network => {
            let devices = network::devices()?;
            let mut rows = vec![open(Page::Profiles)];
            if devices.iter().any(|d| d[1] == "wifi") {
                rows.push(toggle(
                    "s.radio",
                    nm(&["radio", "wifi"])? == "enabled",
                    O::Radio,
                ));
            }
            for d in devices
                .iter()
                .filter(|d| matches!(d[1].as_str(), "ethernet" | "wifi"))
            {
                rows.push(row(
                    "s.interface",
                    format!("{} · {} · {} {}", d[0], d[1], d[2], d[3]),
                    Action::Open(Page::Device(d[0].clone())),
                ));
                if d[1] == "wifi" && d[2] != "unmanaged" {
                    rows.push(row(
                        "s.nearby",
                        &d[0],
                        Action::Open(Page::Wifi(d[0].clone())),
                    ));
                }
            }
            rows
        }
        Page::Wifi(iface) => {
            let privileged = network_admin(&["network-control", "settings.modify.system"]);
            let text = network::scan_fields(iface, "SSID,SECURITY,SIGNAL")?;
            let mut seen = std::collections::HashSet::new();
            zero_or_rows(
                text.lines()
                    .map(network::fields)
                    .filter_map(|f| {
                        let [ssid, security, signal] = f.as_slice() else {
                            return None;
                        };
                        if !network::valid_ssid(ssid) || !seen.insert(ssid.clone()) {
                            return None;
                        }
                        let op = O::Wifi {
                            iface: iface.clone(),
                            ssid: ssid.clone(),
                        };
                        let action = if privileged || security.is_empty() || security == "--" {
                            Action::Confirm(op, String::new())
                        } else {
                            Action::Password(op)
                        };
                        Some(row("", format!("{ssid} · {signal}% {security}"), action))
                    })
                    .collect(),
            )
        }
        Page::Device(iface) => {
            let mut rows = vec![
                row(
                    "s.activate",
                    iface,
                    Action::Apply(O::ActivateDevice(iface.clone()), String::new()),
                ),
                row(
                    "s.disconnect",
                    iface,
                    Action::Confirm(O::Disconnect(iface.clone()), String::new()),
                ),
            ];
            for line in nm(&["-t", "-f", "GENERAL,IP4,IP6", "device", "show", iface])?.lines() {
                let f = network::fields(line);
                if f.len() == 2 && !f[1].is_empty() {
                    rows.push(read("", format!("{} {}", f[0], f[1])));
                }
            }
            rows
        }
        Page::Profiles => zero_or_rows(
            nm(&["-t", "-f", "UUID,NAME,TYPE", "connection", "show"])?
                .lines()
                .map(network::fields)
                .filter_map(|f| {
                    let [uuid, name, kind] = f.as_slice() else {
                        return None;
                    };
                    matches!(
                        kind.as_str(),
                        "802-11-wireless" | "802-3-ethernet" | "wifi" | "ethernet"
                    )
                    .then(|| row("", name, Action::Open(Page::Profile(uuid.clone()))))
                })
                .collect(),
        ),
        Page::Profile(uuid) => {
            let op = |field| O::Profile {
                uuid: uuid.clone(),
                field,
            };
            let props=nm(&["-t","-f","connection.id,connection.autoconnect,ipv4.method,ipv4.addresses,ipv4.gateway,ipv4.dns","connection","show","uuid",uuid])?;
            let props: std::collections::HashMap<_, _> = props
                .lines()
                .map(network::fields)
                .filter(|v| v.len() == 2)
                .map(|v| (v[0].clone(), v[1].clone()))
                .collect();
            let get = |k: &str| props.get(k).map(String::as_str).unwrap_or("");
            vec![
                read("s.profile", get("connection.id")),
                row("s.activate", "", Action::Apply(op("up"), String::new())),
                toggle("s.auto", get("connection.autoconnect") == "yes", op("auto")),
                read("s.ipmode", get("ipv4.method")),
                row("s.dhcp", "", Action::Confirm(op("dhcp"), String::new())),
                edit("s.address", get("ipv4.addresses"), op("address")),
                edit("s.gateway", get("ipv4.gateway"), op("gateway")),
                edit("s.dns", get("ipv4.dns"), op("dns")),
                row("s.forget", "", Action::Confirm(op("forget"), String::new())),
            ]
        }
        Page::Bluetooth => {
            let show = bt(&["show"])?;
            let powered = show.lines().any(|l| l.trim() == "Powered: yes");
            let mut rows = vec![toggle(
                "s.bt_power",
                powered,
                O::Bluetooth("power", String::new()),
            )];
            if powered {
                rows.push(row(
                    "s.discover",
                    "",
                    Action::Apply(O::Bluetooth("scan", String::new()), String::new()),
                ));
                for line in bt(&["devices"])?.lines() {
                    if let Some((addr, name)) =
                        line.strip_prefix("Device ").and_then(|s| s.split_once(' '))
                    {
                        rows.push(row("", name, Action::Open(Page::BtDevice(addr.to_owned()))));
                    }
                }
            }
            rows
        }
        Page::BtDevice(addr) => {
            let info = bt(&["info", addr])?;
            let op = |cmd| O::Bluetooth(cmd, addr.clone());
            let mut rows = vec![read("s.device", addr)];
            for line in info.lines().map(str::trim).filter(|s| {
                s.starts_with("Name:") || s.starts_with("Paired:") || s.starts_with("Connected:")
            }) {
                rows.push(read("", line));
            }
            if !info.lines().any(|s| s.trim() == "Paired: yes") {
                rows.push(row(
                    "s.pair",
                    "",
                    Action::Confirm(op("pair"), String::new()),
                ));
            }
            let connected = info.lines().any(|s| s.trim() == "Connected: yes");
            rows.push(row(
                if connected {
                    "s.disconnect"
                } else {
                    "s.activate"
                },
                "",
                Action::Apply(
                    op(if connected { "disconnect" } else { "connect" }),
                    String::new(),
                ),
            ));
            rows.push(row(
                "s.forget",
                "",
                Action::Confirm(op("remove"), String::new()),
            ));
            rows
        }
        Page::Appearance | Page::Power => preferences_rows(page)?,
        Page::Sound => {
            let outputs = sinks()?;
            let mut rows = Vec::new();
            if outputs.is_empty() {
                return Ok(vec![read("s.no_output", "")]);
            }
            if let Ok(volume) = output(WP, &["get-volume", SINK]) {
                if let Some(level) = volume
                    .strip_prefix("Volume: ")
                    .and_then(|v| v.split_whitespace().next())
                    .and_then(|v| v.parse::<f32>().ok())
                {
                    rows.push(edit("s.volume", (level * 100.).round(), O::Volume));
                    rows.push(toggle("s.mute", volume.contains("[MUTED]"), O::Mute));
                }
            }
            let default = output(WP, &["inspect", SINK])
                .unwrap_or_default()
                .lines()
                .next()
                .and_then(|l| l.strip_prefix("id "))
                .and_then(|s| s.split_once(','))
                .map(|(id, _)| id.to_owned());
            for (id, name) in outputs {
                let name = if default.as_deref() == Some(&id) {
                    format!("● {name}")
                } else {
                    name
                };
                rows.push(row("s.output", name, Action::Apply(O::Output, id)));
            }
            rows
        }
    })
}
fn sinks() -> io::Result<Vec<(String, String)>> {
    let text = output("/usr/bin/pw-dump", &[])?;
    let nodes: Vec<serde_json::Value> = serde_json::from_str(&text)?;
    Ok(nodes
        .iter()
        .filter_map(|node| {
            let props = node.pointer("/info/props")?;
            if props.get("media.class")?.as_str()? != "Audio/Sink" {
                return None;
            }
            let name = props
                .get("node.description")
                .or_else(|| props.get("node.nick"))
                .or_else(|| props.get("node.name"))?
                .as_str()?;
            Some((node.get("id")?.as_u64()?.to_string(), name.to_owned()))
        })
        .collect())
}
fn preferences_rows(page: &Page) -> io::Result<Vec<Row>> {
    let shell = Path::new("/usr/bin/alcard-shell").is_file();
    let (prefs, error) = ConfigSource::from_environment(Overrides::default()).startup();
    use Operation::Preference as P;
    if page == &Page::Appearance {
        if shell {
            if let Some(e) = error {
                return Err(io::Error::other(e));
            }
        }
        return Ok(if shell {
            vec![
                row(
                    "s.theme",
                    prefs.theme.id,
                    choices(P(None, "theme"), &["classic", "emerald"]),
                ),
                edit(
                    "s.accent",
                    format!("#{:06X}", prefs.theme.accent),
                    P(None, "accent"),
                ),
                row(
                    "s.icons",
                    match prefs.theme.icon_style {
                        IconStyle::Solid => "solid",
                        IconStyle::Outline => "outline",
                    },
                    choices(P(None, "icon_style"), &["solid", "outline"]),
                ),
            ]
        } else {
            vec![read("s.shellonly", "")]
        });
    }
    let mut rows = Vec::new();
    if let Ok(data) = output("/usr/bin/brightnessctl", &["-c", "backlight", "-m", "info"]) {
        if let Some(level) = data.split(',').nth(3) {
            rows.push(edit(
                "s.brightness",
                level.trim_end_matches('%'),
                Operation::System("brightness"),
            ));
        }
    }
    if let Ok(power) = fs::read_dir("/sys/class/power_supply") {
        for entry in power.flatten() {
            if fs::read_to_string(entry.path().join("type")).is_ok_and(|t| t.trim() == "Battery") {
                if let Ok(value) = fs::read_to_string(entry.path().join("capacity")) {
                    rows.push(read("s.charge", format!("{}%", value.trim())));
                }
            }
        }
    }
    if shell {
        if let Some(err) = error {
            rows.push(read("s.unavailable", err));
            return Ok(rows);
        }
        rows.extend([
            edit(
                "s.lock",
                prefs.idle.lock_after_seconds,
                P(Some("idle"), "lock_after_seconds"),
            ),
            edit(
                "s.screenoff",
                prefs.idle.screen_off_after_seconds,
                P(Some("idle"), "screen_off_after_seconds"),
            ),
            edit(
                "s.low",
                prefs.battery.low_percent,
                P(Some("battery"), "low_percent"),
            ),
            edit(
                "s.critical",
                prefs.battery.critical_percent,
                P(Some("battery"), "critical_percent"),
            ),
            toggle(
                "s.suspend",
                prefs.battery.suspend_on_critical,
                P(Some("battery"), "suspend_on_critical"),
            ),
        ]);
    }
    Ok(zero_or_rows(rows))
}

fn network_admin(required: &[&str]) -> bool {
    if unsafe { libc::geteuid() } == 0 {
        return false;
    }
    let Ok(text) = nm(&["-t", "-f", "PERMISSION,VALUE", "general", "permissions"]) else {
        return true;
    };
    !required.iter().all(|r| {
        text.lines()
            .any(|line| line == format!("org.freedesktop.NetworkManager.{r}:yes"))
    })
}
fn admin_command(program: &str) -> Command {
    if unsafe { libc::geteuid() } == 0 {
        return Command::new(program);
    }
    // Alpine restricts traversal of /etc/doas.d. Probe the effective account
    // policy with a harmless command, as Hello does when provisioning it.
    let has_doas = Command::new("/usr/bin/doas")
        .args(["-n", "-u", "root", "/bin/ash", "-c", "exit 0"])
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .output()
        .is_ok_and(|r| r.status.success() || r.stderr == b"doas: Authentication required\n");
    if has_doas {
        let mut cmd = Command::new("/usr/bin/doas");
        cmd.arg(program);
        cmd
    } else {
        // User values are passed as separate positional parameters to the fixed command.
        let mut cmd = Command::new("/bin/su");
        cmd.args(["-c", r#"exec "$@""#, "--", "root", "alcard", program]);
        cmd
    }
}

impl Operation {
    fn network_args<'a>(&'a self, value: &'a str) -> io::Result<Vec<&'a str>> {
        let invalid = || io::Error::new(io::ErrorKind::InvalidInput, "Invalid network setting");
        Ok(match self {
            Self::Radio if matches!(value, "on" | "off") => vec!["radio", "wifi", value],
            Self::ActivateDevice(iface) => vec!["device", "connect", iface],
            Self::Disconnect(iface) => vec!["device", "disconnect", iface],
            Self::Wifi { iface, ssid } => vec!["device", "wifi", "connect", ssid, "ifname", iface],
            Self::Profile { uuid, field } => {
                let modify = |props: &[&'a str]| {
                    let mut args = vec!["connection", "modify", "uuid", uuid.as_str()];
                    args.extend_from_slice(props);
                    args
                };
                match *field {
                    "auto" => modify(&[
                        "connection.autoconnect",
                        if value == "on" { "yes" } else { "no" },
                    ]),
                    "dhcp" => modify(&[
                        "ipv4.method",
                        "auto",
                        "ipv4.addresses",
                        "",
                        "ipv4.gateway",
                        "",
                        "ipv4.dns",
                        "",
                        "ipv4.ignore-auto-dns",
                        "no",
                    ]),
                    "address" => {
                        if !value.split_once('/').is_some_and(|(ip, prefix)| {
                            ip.parse::<Ipv4Addr>().is_ok()
                                && prefix.parse::<u8>().is_ok_and(|n| n <= 32)
                        }) {
                            return Err(invalid());
                        }
                        modify(&["ipv4.addresses", value, "ipv4.method", "manual"])
                    }
                    "gateway" => {
                        if !value.is_empty() && value.parse::<Ipv4Addr>().is_err() {
                            return Err(invalid());
                        }
                        modify(&["ipv4.gateway", value])
                    }
                    "dns" => {
                        if value
                            .split([',', ' '])
                            .filter(|s| !s.is_empty())
                            .any(|s| !matches!(s.parse::<IpAddr>(), Ok(IpAddr::V4(_))))
                        {
                            return Err(invalid());
                        }
                        modify(&[
                            "ipv4.dns",
                            value,
                            "ipv4.ignore-auto-dns",
                            if value.trim().is_empty() { "no" } else { "yes" },
                        ])
                    }
                    "up" => vec!["connection", "up", "uuid", uuid],
                    "forget" => vec!["connection", "delete", "uuid", uuid],
                    _ => return Err(invalid()),
                }
            }
            _ => return Err(invalid()),
        })
    }
    pub fn terminal_command(&self, value: &str) -> io::Result<Option<Command>> {
        if let Self::System(key) = self {
            let mut command = admin_command("/usr/sbin/alcard-setupd");
            command.args(["--setting", key, value]);
            return Ok(Some(command));
        }
        let permissions: &[&str] = match self {
            Self::Radio => &["enable-disable-wifi"],
            Self::Wifi { .. } => &["network-control", "settings.modify.system"],
            Self::Disconnect(_) | Self::ActivateDevice(_) | Self::Profile { field: "up", .. } => {
                &["network-control"]
            }
            Self::Profile { .. } => &["settings.modify.system"],
            _ => return Ok(None),
        };
        if network_admin(permissions) {
            let mut command = admin_command(network::NMCLI);
            command
                .args(["--wait", "30", "--ask"])
                .args(self.network_args(value)?);
            return Ok(Some(command));
        }
        Ok(None)
    }
    pub fn apply(&self, value: &str) -> io::Result<()> {
        let invalid = || io::Error::new(io::ErrorKind::InvalidInput, "Invalid setting value");
        match self {
            Self::Preference(group, key) => {
                let v = match *key {
                    "theme" | "icon_style" | "accent" => toml_edit::Value::from(value),
                    "suspend_on_critical" => toml_edit::Value::from(value == "on"),
                    _ => toml_edit::Value::from(value.parse::<i64>().map_err(|_| invalid())?),
                };
                ConfigSource::from_environment(Overrides::default()).save(*group, key, v)?;
                reload_shell();
            }
            Self::Wifi { iface, ssid } => network::connect(iface, ssid, value)?,
            Self::Radio | Self::Disconnect(_) | Self::ActivateDevice(_) | Self::Profile { .. } => {
                nm(&self.network_args(value)?)?;
            }
            Self::Bluetooth(cmd, addr) => {
                match *cmd {
                    "power" => bt(&["power", value])?,
                    "scan" => bt(&["--timeout", "8", "scan", "on"])?,
                    "connect" | "disconnect" | "remove" => bt(&[cmd, addr])?,
                    _ => return Err(invalid()),
                };
            }
            Self::Volume => {
                if !value.parse::<u8>().is_ok_and(|n| n <= 100) {
                    return Err(invalid());
                }
                output(WP, &["set-volume", SINK, &format!("{value}%")])?;
            }
            Self::Mute => {
                output(
                    WP,
                    &["set-mute", SINK, if value == "on" { "1" } else { "0" }],
                )?;
            }
            Self::Output => {
                if !sinks()?.iter().any(|(id, _)| id == value) {
                    return Err(invalid());
                }
                output(WP, &["set-default", value])?;
            }
            Self::System(_) => return Err(invalid()),
        }
        Ok(())
    }
}
pub fn reload_shell() {
    // Limit reload to real Shell executables belonging to the invoking user.
    use std::os::unix::fs::MetadataExt;
    let Ok(entries) = fs::read_dir("/proc") else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<i32>() else {
            continue;
        };
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
        if fd < 0 {
            continue;
        }
        // Pin this process across the checks and signal; its numeric PID can be reused.
        let handle = unsafe { OwnedFd::from_raw_fd(fd as i32) };
        if entry
            .metadata()
            .is_ok_and(|m| m.uid() == unsafe { libc::getuid() })
            && fs::read_link(entry.path().join("exe"))
                .is_ok_and(|p| p == Path::new("/usr/bin/alcard-shell"))
        {
            unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal,
                    handle.as_raw_fd(),
                    libc::SIGHUP,
                    std::ptr::null::<libc::siginfo_t>(),
                    0,
                );
            }
        }
    }
}

#[cfg(test)]
#[path = "unit_tests/settings.rs"]
mod tests;
