//! Activation ownership is (NetworkManager's unique bus name, ActiveConnection path).
//! An interface or saved-profile UUID is not an activation and is never a cancel target.
use std::{
    collections::HashMap,
    future::{poll_fn, Future},
    io,
    sync::{
        atomic::{AtomicBool, AtomicU8, Ordering},
        mpsc, Arc, Mutex,
    },
    task::{Poll, Waker},
    thread,
    time::{Duration, Instant},
};
use zbus::{interface, message::Header, Connection, OwnedObjectPath, OwnedVariant, Str};

const NM: &str = "org.freedesktop.NetworkManager";
const ROOT: &str = "/org/freedesktop/NetworkManager";
const DEVICE: &str = "org.freedesktop.NetworkManager.Device";
const WIFI: &str = "org.freedesktop.NetworkManager.Device.Wireless";
const AP: &str = "org.freedesktop.NetworkManager.AccessPoint";
const ACTIVE: &str = "org.freedesktop.NetworkManager.Connection.Active";
const PROFILE: &str = "org.freedesktop.NetworkManager.Settings.Connection";
const AGENT_MANAGER: &str = "org.freedesktop.NetworkManager.AgentManager";
const AGENT_PATH: &str = "/org/freedesktop/NetworkManager/SecretAgent";
const WIRELESS: &str = "802-11-wireless";
const SECURITY: &str = "802-11-wireless-security";
const CALL_TIMEOUT: Duration = Duration::from_secs(10);
const OPERATION_TIMEOUT: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(80);
type Properties = HashMap<String, OwnedVariant>;
type Settings = HashMap<String, Properties>;

/// The UI can revoke credentials even while a mutating D-Bus call awaits its reply.
struct Control {
    stop: AtomicU8,
    secret: Mutex<Option<String>>,
    reader: Mutex<Option<Waker>>,
}
impl Control {
    fn new(password: &str) -> Self {
        Self {
            stop: AtomicU8::new(0),
            secret: Mutex::new((!password.is_empty()).then(|| password.to_owned())),
            reader: Mutex::new(None),
        }
    }
    fn stop(&self, timeout: bool) {
        self.revoke();
        let _ = self.stop.compare_exchange(
            0,
            if timeout { 2 } else { 1 },
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        if let Some(waker) = self.reader.lock().unwrap().take() {
            waker.wake();
        }
    }
    fn revoke(&self) {
        self.secret.lock().unwrap().take();
    }
    fn check(&self) -> io::Result<()> {
        match self.stop.load(Ordering::Acquire) {
            0 => Ok(()),
            1 => Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Wi-Fi activation cancelled",
            )),
            _ => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Wi-Fi activation timed out",
            )),
        }
    }
    async fn read<T>(&self, future: impl Future<Output = zbus::Result<T>>) -> io::Result<T> {
        let mut future = std::pin::pin!(future);
        poll_fn(|cx| {
            // Register first so cancellation cannot fall between the check and registration.
            *self.reader.lock().unwrap() = Some(cx.waker().clone());
            if let Err(err) = self.check() {
                return Poll::Ready(Err(err));
            }
            future
                .as_mut()
                .poll(cx)
                .map(|result| result.map_err(bus_error))
        })
        .await
    }
}

pub(super) fn connect(
    iface: &str,
    ssid: &str,
    password: &str,
    cancel: &AtomicBool,
) -> io::Result<()> {
    connect_using(iface, ssid, password, cancel, OPERATION_TIMEOUT, || async {
        zbus::connection::Builder::address("unix:path=/run/dbus/system_bus_socket")
            .method_timeout(CALL_TIMEOUT)
            .build()
            .await
    })
}

// Private connection injection keeps fixtures off the production system bus.
fn connect_using<F: Future<Output = zbus::Result<Connection>>>(
    iface: &str,
    ssid: &str,
    password: &str,
    cancel: &AtomicBool,
    timeout: Duration,
    open: impl FnOnce() -> F + Send + 'static,
) -> io::Result<()> {
    let control = Arc::new(Control::new(password));
    let worker_control = control.clone();
    let iface = iface.to_owned();
    let ssid = ssid.to_owned();
    let (done, result) = mpsc::channel();
    thread::spawn(move || {
        let outcome = zbus::block_on(async {
            let conn = worker_control.read(open()).await?;
            connect_on_connection(&conn, &iface, &ssid, worker_control.clone()).await
        });
        worker_control.revoke();
        let _ = done.send(outcome);
    });
    let deadline = Instant::now() + timeout;
    loop {
        let timed_out = Instant::now() >= deadline;
        if cancel.load(Ordering::Acquire) || timed_out {
            control.stop(timed_out);
            // Reads stop immediately. An issued activation is retained until its bounded
            // reply so a late handle can still be deactivated. No credentials are retained.
            return match result.recv_timeout(Duration::from_millis(200)) {
                Ok(outcome) => outcome,
                Err(_) => Err(io::Error::new(
                    if timed_out {
                        io::ErrorKind::TimedOut
                    } else {
                        io::ErrorKind::Interrupted
                    },
                    "Wi-Fi cancellation requested; NetworkManager cleanup is not yet confirmed",
                )),
            };
        }
        match result.recv_timeout(POLL) {
            Ok(outcome) => return outcome,
            Err(mpsc::RecvTimeoutError::Timeout) => (),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(io::Error::other("Wi-Fi worker stopped"))
            }
        }
    }
}

fn bus_error(err: zbus::Error) -> io::Error {
    // Never include service-supplied diagnostics: a rejected settings dictionary may
    // contain credentials. The D-Bus error name is enough to distinguish denial.
    let denied = match &err {
        zbus::Error::MethodError(name, _, _) => {
            name.as_str().ends_with("PermissionDenied") || name.as_str().ends_with("NotAuthorized")
        }
        zbus::Error::FDO(err) => matches!(
            err.as_ref(),
            zbus::fdo::Error::AccessDenied(_) | zbus::fdo::Error::AuthFailed(_)
        ),
        _ => false,
    };
    if denied {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "NetworkManager permission denied",
        )
    } else {
        io::Error::other("NetworkManager request failed or service unavailable")
    }
}
fn malformed() -> io::Error {
    io::Error::other("Invalid NetworkManager response")
}

fn activation_error(err: zbus::Error, control: &Control) -> io::Error {
    let no_reply = match &err {
        zbus::Error::MethodError(_, _, _) => false,
        zbus::Error::FDO(err) => matches!(
            err.as_ref(),
            zbus::fdo::Error::NoReply(_)
                | zbus::fdo::Error::Timeout(_)
                | zbus::fdo::Error::Disconnected(_)
        ),
        _ => true,
    };
    if no_reply {
        let kind = control.check().err().map(|error| error.kind()).unwrap_or_else(|| {
            if matches!(&err, zbus::Error::InputOutput(error) if error.kind() == io::ErrorKind::TimedOut) {
                io::ErrorKind::TimedOut
            } else { io::ErrorKind::Other }
        });
        io::Error::new(kind, "Wi-Fi activation outcome is unconfirmed: NetworkManager did not return an activation handle")
    } else {
        control.check().err().unwrap_or_else(|| bus_error(err))
    }
}
fn text(value: &str) -> OwnedVariant {
    OwnedVariant::from(Str::from(value))
}
fn string<'a>(properties: &'a Properties, key: &str) -> Option<&'a str> {
    properties
        .get(key)
        .and_then(|value| <&str>::try_from(value).ok())
}
fn number(properties: &Properties, key: &str) -> Option<u32> {
    properties
        .get(key)
        .and_then(|value| u32::try_from(value).ok())
}
fn bytes(properties: &Properties, key: &str) -> Option<Vec<u8>> {
    properties
        .get(key)
        .and_then(|value| Vec::<u8>::try_from(value.try_clone().ok()?).ok())
}
fn hardware_address(value: &str) -> Option<[u8; 6]> {
    let mut octets = value.split(':');
    let mut address = [0; 6];
    for byte in &mut address {
        let octet = octets.next()?;
        if octet.len() != 2 || !octet.bytes().all(|value| value.is_ascii_hexdigit()) {
            return None;
        }
        *byte = u8::from_str_radix(octet, 16).ok()?;
    }
    octets.next().is_none().then_some(address)
}
fn paths(properties: &Properties, key: &str) -> io::Result<Vec<OwnedObjectPath>> {
    Vec::try_from(
        properties
            .get(key)
            .ok_or_else(malformed)?
            .try_clone()
            .map_err(bus_error)?,
    )
    .map_err(bus_error)
}
fn path(properties: &Properties, key: &str) -> io::Result<OwnedObjectPath> {
    OwnedObjectPath::try_from(
        properties
            .get(key)
            .ok_or_else(malformed)?
            .try_clone()
            .map_err(bus_error)?,
    )
    .map_err(bus_error)
}
async fn properties(
    conn: &Connection,
    owner: &str,
    object: &str,
    interface: &str,
    control: &Control,
) -> io::Result<Properties> {
    control
        .read(conn.call_method(
            Some(owner),
            object,
            Some("org.freedesktop.DBus.Properties"),
            "GetAll",
            &(interface,),
        ))
        .await?
        .body()
        .deserialize()
        .map_err(bus_error)
}
async fn settings(
    conn: &Connection,
    owner: &str,
    object: &str,
    control: &Control,
) -> io::Result<Settings> {
    control
        .read(conn.call_method(Some(owner), object, Some(PROFILE), "GetSettings", &()))
        .await?
        .body()
        .deserialize()
        .map_err(bus_error)
}

struct AccessPoint {
    path: OwnedObjectPath,
    properties: Properties,
}
impl AccessPoint {
    fn security_flags(&self) -> u32 {
        number(&self.properties, "WpaFlags").unwrap_or(0)
            | number(&self.properties, "RsnFlags").unwrap_or(0)
    }
    fn private(&self) -> bool {
        number(&self.properties, "Flags").unwrap_or(0) & 1 != 0
    }
    fn matches(&self, profile: &Settings, ssid: &str) -> bool {
        let Some(wifi) = profile.get(WIRELESS) else {
            return false;
        };
        if bytes(wifi, "ssid").as_deref() != Some(ssid.as_bytes()) {
            return false;
        }
        if wifi.contains_key("bssid") {
            // NetworkManager exports this constraint as ay; accept the textual
            // representation too, but never ignore a present malformed constraint.
            let expected = bytes(wifi, "bssid")
                .and_then(|value| <[u8; 6]>::try_from(value).ok())
                .or_else(|| string(wifi, "bssid").and_then(hardware_address));
            let actual = string(&self.properties, "HwAddress").and_then(hardware_address);
            if expected.is_none() || expected != actual {
                return false;
            }
        }
        if string(wifi, "mode").is_some_and(|mode| {
            (mode == "infrastructure" && number(&self.properties, "Mode") != Some(2))
                || (mode == "adhoc" && number(&self.properties, "Mode") != Some(1))
                || !matches!(mode, "infrastructure" | "adhoc")
        }) {
            return false;
        }
        let flags = self.security_flags();
        match profile
            .get(SECURITY)
            .and_then(|security| string(security, "key-mgmt"))
        {
            None => !self.private() && flags == 0,
            Some("none") => self.private() && flags == 0,
            Some("wpa-psk") => flags & 0x100 != 0,
            Some("sae") => flags & 0x400 != 0,
            Some("owe") => flags & 0x1800 != 0,
            Some("wpa-eap" | "ieee8021x" | "wpa-eap-suite-b-192") => flags & 0x2200 != 0,
            _ => false,
        }
    }
    fn new_profile(&self, ssid: &str, uuid: &str) -> io::Result<Settings> {
        let mut profile = Settings::from([
            (
                "connection".into(),
                Properties::from([
                    ("id".into(), text(ssid)),
                    ("uuid".into(), text(uuid)),
                    ("type".into(), text(WIRELESS)),
                ]),
            ),
            (
                WIRELESS.into(),
                Properties::from([(
                    "ssid".into(),
                    OwnedVariant::try_from(zbus::Variant::from(ssid.as_bytes().to_vec()))
                        .map_err(bus_error)?,
                )]),
            ),
        ]);
        let flags = self.security_flags();
        let key = if flags & 0x100 != 0 {
            Some("wpa-psk")
        } else if flags & 0x400 != 0 {
            Some("sae")
        } else if flags & 0x1800 != 0 {
            Some("owe")
        } else if flags != 0 {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "An existing profile is required for enterprise Wi-Fi",
            ));
        } else if self.private() {
            Some("none")
        } else {
            None
        };
        if let Some(key) = key {
            let mut security = Properties::from([("key-mgmt".into(), text(key))]);
            if key == "none" {
                security.insert("wep-key-type".into(), 1u32.into());
            }
            profile.insert(SECURITY.into(), security);
        }
        Ok(profile)
    }
}

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.freedesktop.NetworkManager.SecretAgent.Error")]
enum SecretError {
    PermissionDenied(String),
    UserCanceled(String),
    NoSecrets(String),
}
struct SecretAgent {
    owner: String,
    profile: Option<OwnedObjectPath>,
    uuid: String,
    ssid: String,
    control: Arc<Control>,
    supplied: AtomicBool,
}
impl SecretAgent {
    fn verify(&self, header: &Header<'_>, profile: &OwnedObjectPath) -> Result<(), SecretError> {
        if header.sender().map(|sender| sender.as_str()) != Some(self.owner.as_str())
            || self
                .profile
                .as_ref()
                .is_some_and(|expected| expected != profile)
        {
            return Err(SecretError::PermissionDenied(
                "Unexpected secret request".into(),
            ));
        }
        self.control
            .check()
            .map_err(|_| SecretError::UserCanceled("Activation cancelled".into()))
    }
}
#[interface(name = "org.freedesktop.NetworkManager.SecretAgent")]
impl SecretAgent {
    fn get_secrets(
        &self,
        connection: Settings,
        connection_path: OwnedObjectPath,
        setting_name: &str,
        _hints: Vec<String>,
        _flags: u32,
        #[zbus(header)] header: Header<'_>,
    ) -> Result<Settings, SecretError> {
        self.verify(&header, &connection_path)?;
        if connection.get("connection").and_then(|p| string(p, "uuid")) != Some(self.uuid.as_str())
            || connection
                .get(WIRELESS)
                .and_then(|p| bytes(p, "ssid"))
                .as_deref()
                != Some(self.ssid.as_bytes())
            || setting_name != SECURITY
        {
            return Err(SecretError::NoSecrets(
                "No secrets for this connection".into(),
            ));
        }
        let key = match connection.get(SECURITY).and_then(|p| string(p, "key-mgmt")) {
            Some("wpa-psk" | "sae") => "psk",
            Some("none") => "wep-key0",
            _ => return Err(SecretError::NoSecrets("Unsupported secret type".into())),
        };
        let secret = self.control.secret.lock().unwrap();
        if self.control.check().is_err() {
            return Err(SecretError::UserCanceled("Activation cancelled".into()));
        }
        let Some(password) = secret.as_ref() else {
            return Err(SecretError::NoSecrets("No password supplied".into()));
        };
        if self.supplied.swap(true, Ordering::AcqRel) {
            return Err(SecretError::NoSecrets("Password already tried".into()));
        }
        Ok(Settings::from([(
            SECURITY.into(),
            Properties::from([(key.into(), text(password))]),
        )]))
    }
    fn cancel_get_secrets(
        &self,
        connection_path: OwnedObjectPath,
        _setting_name: &str,
        #[zbus(header)] header: Header<'_>,
    ) -> Result<(), SecretError> {
        self.verify(&header, &connection_path)
    }
    fn save_secrets(
        &self,
        _connection: Settings,
        connection_path: OwnedObjectPath,
        #[zbus(header)] header: Header<'_>,
    ) -> Result<(), SecretError> {
        self.verify(&header, &connection_path)?;
        Err(SecretError::NoSecrets(
            "NetworkManager owns persistent secrets".into(),
        ))
    }
    fn delete_secrets(
        &self,
        _connection: Settings,
        connection_path: OwnedObjectPath,
        #[zbus(header)] header: Header<'_>,
    ) -> Result<(), SecretError> {
        self.verify(&header, &connection_path)?;
        Ok(())
    }
}

struct Activation {
    owner: String,
    path: OwnedObjectPath,
}
impl Activation {
    async fn deactivate(&self, conn: &Connection) -> io::Result<()> {
        match conn
            .call_method(
                Some(self.owner.as_str()),
                ROOT,
                Some(NM),
                "DeactivateConnection",
                &(&self.path,),
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(zbus::Error::MethodError(name, _, _))
                if name.as_str() == "org.freedesktop.NetworkManager.ConnectionNotActive" =>
            {
                Ok(())
            }
            Err(zbus::Error::FDO(err))
                if matches!(
                    err.as_ref(),
                    zbus::fdo::Error::NameHasNoOwner(_)
                        | zbus::fdo::Error::ServiceUnknown(_)
                        | zbus::fdo::Error::UnknownObject(_)
                ) =>
            {
                Ok(())
            }
            Err(_) => Err(io::Error::other(
                "Wi-Fi cleanup could not be confirmed by NetworkManager",
            )),
        }
    }
}

async fn connect_on_connection(
    conn: &Connection,
    iface: &str,
    ssid: &str,
    control: Arc<Control>,
) -> io::Result<()> {
    control.check()?;
    let owner: String = control
        .read(conn.call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "GetNameOwner",
            &(NM,),
        ))
        .await?
        .body()
        .deserialize()
        .map_err(bus_error)?;
    let device: OwnedObjectPath = control
        .read(conn.call_method(
            Some(owner.as_str()),
            ROOT,
            Some(NM),
            "GetDeviceByIpIface",
            &(iface,),
        ))
        .await?
        .body()
        .deserialize()
        .map_err(bus_error)?;
    let device_properties = properties(conn, &owner, &device, DEVICE, &control).await?;
    if number(&device_properties, "DeviceType") != Some(2) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Not a Wi-Fi device",
        ));
    }
    let previous = path(&device_properties, "ActiveConnection")?;
    if previous.as_str() != "/" {
        let active = properties(conn, &owner, &previous, ACTIVE, &control).await?;
        if number(&active, "State") == Some(2) {
            let profile = settings(conn, &owner, &path(&active, "Connection")?, &control).await?;
            if profile
                .get(WIRELESS)
                .and_then(|p| bytes(p, "ssid"))
                .as_deref()
                == Some(ssid.as_bytes())
            {
                return control.check(); // already connected: this request owns nothing
            }
        }
    }
    let access_points: Vec<OwnedObjectPath> = control
        .read(conn.call_method(
            Some(owner.as_str()),
            &device,
            Some(WIFI),
            "GetAllAccessPoints",
            &(),
        ))
        .await?
        .body()
        .deserialize()
        .map_err(bus_error)?;
    let mut points = Vec::new();
    for ap_path in access_points {
        let data = properties(conn, &owner, &ap_path, AP, &control).await?;
        if bytes(&data, "Ssid").as_deref() == Some(ssid.as_bytes()) {
            points.push(AccessPoint {
                path: ap_path,
                properties: data,
            });
        }
    }
    points.sort_by_key(|ap| {
        std::cmp::Reverse(
            ap.properties
                .get("Strength")
                .and_then(|v| u8::try_from(v).ok())
                .unwrap_or(0),
        )
    });
    let mut selected = None;
    for profile_path in paths(&device_properties, "AvailableConnections")? {
        let profile = settings(conn, &owner, &profile_path, &control).await?;
        if let Some(index) = points.iter().position(|ap| ap.matches(&profile, ssid)) {
            selected = Some((profile_path, profile, index));
            break;
        }
    }
    let (saved, profile, ap) = if let Some((profile_path, profile, index)) = selected {
        (Some(profile_path), profile, points.swap_remove(index))
    } else {
        let ap = points.into_iter().next().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "Wi-Fi access point disappeared")
        })?;
        let uuid = std::fs::read_to_string("/proc/sys/kernel/random/uuid")?;
        let profile = ap.new_profile(ssid, uuid.trim())?;
        (None, profile, ap)
    };
    let uuid = profile
        .get("connection")
        .and_then(|p| string(p, "uuid"))
        .ok_or_else(malformed)?
        .to_owned();
    let before = paths(
        &properties(conn, &owner, ROOT, NM, &control).await?,
        "ActiveConnections",
    )?;
    let agent = SecretAgent {
        owner: owner.clone(),
        profile: saved.clone(),
        uuid,
        ssid: ssid.to_owned(),
        control: control.clone(),
        supplied: AtomicBool::new(false),
    };
    control
        .read(conn.object_server().at(AGENT_PATH, agent))
        .await?;
    control
        .read(conn.call_method(
            Some(owner.as_str()),
            "/org/freedesktop/NetworkManager/AgentManager",
            Some(AGENT_MANAGER),
            "RegisterWithCapabilities",
            &("org.alcard.Settings", 0u32),
        ))
        .await?;
    control.check()?;
    // Do not race this mutating call with cancel: its reply identifies our activation.
    // The connection's timeout bounds the retained request; cancellation revokes secrets.
    let issued = if let Some(saved) = saved {
        conn.call_method(
            Some(owner.as_str()),
            ROOT,
            Some(NM),
            "ActivateConnection",
            &(&saved, &device, &ap.path),
        )
        .await
        .and_then(|reply| reply.body().deserialize::<OwnedObjectPath>())
    } else {
        conn.call_method(
            Some(owner.as_str()),
            ROOT,
            Some(NM),
            "AddAndActivateConnection",
            &(&profile, &device, &ap.path),
        )
        .await
        .and_then(|reply| {
            reply
                .body()
                .deserialize::<(OwnedObjectPath, OwnedObjectPath)>()
        })
        .map(|(_, active)| active)
    };
    let active = issued.map_err(|err| activation_error(err, &control))?;
    if before.contains(&active) {
        // Defensive against a service reusing an already existing activation.
        control.check()?;
        return Err(io::Error::other(
            "NetworkManager returned an existing activation",
        ));
    }
    let activation = Activation {
        owner,
        path: active,
    };
    let outcome = loop {
        let state =
            match properties(conn, &activation.owner, &activation.path, ACTIVE, &control).await {
                Ok(p) => number(&p, "State"),
                Err(err) => break Err(err),
            };
        match state {
            Some(2) => break control.check(),
            Some(3 | 4) => break Err(io::Error::other("Wi-Fi activation failed")),
            Some(0 | 1) => thread::sleep(POLL),
            _ => break Err(malformed()),
        }
    };
    control.revoke();
    if outcome.is_err() {
        // Never look up a replacement by device/UUID, even when this object disappeared.
        activation.deactivate(conn).await?;
    }
    outcome
}

#[cfg(test)]
#[path = "../integration/network.rs"]
mod integration;
#[cfg(test)]
#[path = "../unit_tests/activation.rs"]
mod tests;
