//! Bounded login1 transport, identity checks and suspend authorization preflight.

use super::{Budget, Capability, Failure, Gate, Session};
use nix::unistd::Uid;
use std::{
    collections::HashMap,
    future::Future,
    os::fd::{AsRawFd, OwnedFd},
};
use zbus::{
    message::Sequence,
    proxy::{CacheProperties, MethodFlags},
    wire::DynamicDeserialize,
    DBusError, OwnedObjectPath, OwnedVariant,
};

pub(super) const SERVICE: &str = "org.freedesktop.login1";
pub(super) const MANAGER_PATH: &str = "/org/freedesktop/login1";
pub(super) const MANAGER: &str = "org.freedesktop.login1.Manager";
pub(super) const SESSION: &str = "org.freedesktop.login1.Session";
pub(super) const SEAT: &str = "org.freedesktop.login1.Seat";
pub(super) const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
pub(super) const MAX_REPLY_BYTES: usize = 64 * 1024;
pub(super) const MAX_INHIBITORS: usize = 512;
pub(super) const ROOT_CHECK_INHIBITORS: u64 = 1;

pub(super) fn bus_failure(error: &zbus::Error, mutating: bool) -> Failure {
    let name = match error {
        zbus::Error::MethodError(name, _, _) => name.as_str().to_owned(),
        zbus::Error::FDO(error) => error.name().as_str().to_owned(),
        zbus::Error::DBus(error) => error.name().as_str().to_owned(),
        _ => String::new(),
    };
    match name.as_str() {
        "org.freedesktop.DBus.Error.AccessDenied"
        | "org.freedesktop.DBus.Error.AuthFailed"
        | "org.freedesktop.DBus.Error.InteractiveAuthorizationRequired"
        | "org.freedesktop.PolicyKit1.Error.NotAuthorized" => Failure::Denied,
        "org.freedesktop.DBus.Error.Cancelled"
        | "org.freedesktop.DBus.Error.Canceled"
        | "org.freedesktop.PolicyKit1.Error.Cancelled"
        | "org.freedesktop.PolicyKit1.Error.Canceled"
        | "org.freedesktop.login1.Cancelled"
        | "org.freedesktop.login1.ActionCancelled" => Failure::Cancelled,
        "org.freedesktop.login1.OperationInProgress"
        | "org.freedesktop.login1.ShutdownInProgress"
        | "org.freedesktop.login1.Inhibited" => Failure::Blocked,
        "org.freedesktop.login1.NoSessionForPID" | "org.freedesktop.login1.NoSuchSession" => {
            Failure::SessionUnavailable
        }
        "org.freedesktop.DBus.Error.UnknownMethod"
        | "org.freedesktop.DBus.Error.UnknownInterface"
        | "org.freedesktop.DBus.Error.UnknownObject"
        | "org.freedesktop.DBus.Error.InvalidArgs"
        | "org.freedesktop.DBus.Error.ServiceUnknown"
        | "org.freedesktop.DBus.Error.NameHasNoOwner" => Failure::Unavailable,
        _ if mutating => Failure::Unknown,
        _ => Failure::Unavailable,
    }
}

pub(super) fn permission(value: &str) -> Capability {
    match value {
        "yes" => Capability::Allowed,
        "challenge" => Capability::Authentication,
        "no" => Capability::Denied,
        "na" => Capability::Unsupported,
        "inhibited" | "inhibitor-blocked" | "challenge-inhibitor-blocked" => Capability::Blocked,
        _ => Capability::Unavailable,
    }
}

type Inhibitor = (String, String, String, String, u32, u32);
pub(super) fn sleep_inhibited(inhibitors: &[Inhibitor]) -> Result<bool, Failure> {
    if inhibitors.len() > MAX_INHIBITORS {
        return Err(Failure::Unavailable);
    }
    for (what, _, _, mode, _, _) in inhibitors {
        if what.split(':').any(|kind| kind == "sleep") {
            match mode.as_str() {
                "block" | "block-weak" => return Ok(true),
                "delay" => {}
                _ => return Err(Failure::Unavailable),
            }
        }
    }
    Ok(false)
}

pub(super) trait Login1: Sync {
    fn owner(&self, budget: &Budget<'_>) -> impl Future<Output = Result<String, Failure>> + Send;
    fn session(
        &self,
        owner: &str,
        budget: &Budget<'_>,
    ) -> impl Future<Output = Result<Session, Failure>> + Send;
    fn capability(
        &self,
        owner: &str,
        budget: &Budget<'_>,
    ) -> impl Future<Output = Result<Capability, Failure>> + Send;
    fn inhibited(
        &self,
        owner: &str,
        budget: &Budget<'_>,
    ) -> impl Future<Output = Result<bool, Failure>> + Send;
    fn preparing(
        &self,
        owner: &str,
        budget: &Budget<'_>,
    ) -> impl Future<Output = Result<(bool, u64, Sequence), Failure>> + Send;
}

pub(super) async fn preflight(
    bus: &impl Login1,
    owner: &str,
    session: &Session,
    budget: &Budget<'_>,
) -> Result<(), Failure> {
    if bus.owner(budget).await? != owner {
        return Err(Failure::Unavailable);
    }
    if &bus.session(owner, budget).await? != session {
        return Err(Failure::SessionUnavailable);
    }
    match bus.capability(owner, budget).await? {
        Capability::Allowed => {}
        Capability::Denied | Capability::Authentication => return Err(Failure::Denied),
        Capability::Blocked => return Err(Failure::Blocked),
        _ => return Err(Failure::Unavailable),
    }
    if bus.inhibited(owner, budget).await? || bus.preparing(owner, budget).await?.0 {
        return Err(Failure::Blocked);
    }
    if bus.owner(budget).await? != owner {
        return Err(Failure::Unavailable);
    }
    budget.check(false)
}

pub(super) struct SystemBus {
    pub(super) connection: zbus::Connection,
}

pub(super) fn decode<T: for<'d> DynamicDeserialize<'d>>(
    message: zbus::Message,
) -> Result<T, Failure> {
    if message.body().len() > MAX_REPLY_BYTES || !message.data().fds().is_empty() {
        return Err(Failure::Unavailable);
    }
    message
        .body()
        .deserialize()
        .map_err(|_| Failure::Unavailable)
}

pub(super) fn decode_delay(message: zbus::Message) -> Result<OwnedFd, Failure> {
    if message.body().len() > MAX_REPLY_BYTES || message.data().fds().len() != 1 {
        return Err(Failure::Unavailable);
    }
    let owned: zbus::OwnedFd = message
        .body()
        .deserialize()
        .map_err(|_| Failure::Unavailable)?;
    // OwnedFd deserialization duplicates the received FD. Drop the complete
    // reply now; otherwise that hidden duplicate would prolong inhibition.
    drop(message);
    let fd: OwnedFd = owned.into();
    validate_delay_fd(fd)
}

pub(super) fn validate_delay_fd(fd: OwnedFd) -> Result<OwnedFd, Failure> {
    // SAFETY: only flags/metadata of this owned, live descriptor are accessed.
    let flags = unsafe { nix::libc::fcntl(fd.as_raw_fd(), nix::libc::F_GETFD) };
    if flags < 0
        || unsafe {
            nix::libc::fcntl(
                fd.as_raw_fd(),
                nix::libc::F_SETFD,
                flags | nix::libc::FD_CLOEXEC,
            )
        } < 0
    {
        return Err(Failure::Unavailable);
    }
    let mut metadata = std::mem::MaybeUninit::<nix::libc::stat>::uninit();
    if unsafe { nix::libc::fstat(fd.as_raw_fd(), metadata.as_mut_ptr()) } < 0 {
        return Err(Failure::Unavailable);
    }
    let metadata = unsafe { metadata.assume_init() };
    if metadata.st_mode & nix::libc::S_IFMT != nix::libc::S_IFIFO {
        return Err(Failure::Unavailable);
    }
    Ok(fd)
}

impl SystemBus {
    pub(super) async fn properties(
        &self,
        owner: &str,
        path: &str,
        interface: &str,
        budget: &Budget<'_>,
    ) -> Result<(HashMap<String, OwnedVariant>, Sequence), Failure> {
        let message = budget
            .dbus(
                self.connection.call_method(
                    Some(owner),
                    path,
                    Some(PROPERTIES),
                    "GetAll",
                    &(interface,),
                ),
                false,
            )
            .await?;
        let position = message.recv_position();
        let properties: HashMap<String, OwnedVariant> = decode(message)?;
        if properties.len() > 128 {
            return Err(Failure::Unavailable);
        }
        Ok((properties, position))
    }

    pub(super) async fn methods(&self, owner: &str, budget: &Budget<'_>) -> Result<(), Failure> {
        let xml: String = decode(
            budget
                .dbus(
                    self.connection.call_method(
                        Some(owner),
                        MANAGER_PATH,
                        Some("org.freedesktop.DBus.Introspectable"),
                        "Introspect",
                        &(),
                    ),
                    false,
                )
                .await?,
        )?;
        supported_api(&xml)
    }

    pub(super) async fn inhibit(
        &self,
        owner: &str,
        budget: &Budget<'_>,
    ) -> Result<OwnedFd, Failure> {
        // No interactive-authorization header and no global block/force flag.
        let message = budget
            .dbus(
                self.connection.call_method(
                    Some(owner),
                    MANAGER_PATH,
                    Some(MANAGER),
                    "Inhibit",
                    &(
                        "sleep",
                        "Alcard Shell",
                        "Lock the session before sleep",
                        "delay",
                    ),
                ),
                false,
            )
            .await?;
        decode_delay(message)
    }

    pub(super) async fn suspend(
        &self,
        owner: &str,
        gate: &Gate<'_>,
        budget: &Budget<'_>,
    ) -> Result<(), Failure> {
        let proxy: zbus::Proxy<'_> = budget
            .dbus(
                async {
                    zbus::proxy::Builder::new(&self.connection)
                        .destination(owner)
                        .path(MANAGER_PATH)
                        .interface(MANAGER)
                        .cache_properties(CacheProperties::No)
                        .build()
                        .await
                },
                false,
            )
            .await?;
        budget.check(false)?;
        gate.dispatch()?;
        let reply: Option<()> = budget
            .dbus(
                proxy.call_with_flags(
                    "SuspendWithFlags",
                    MethodFlags::empty(),
                    &(ROOT_CHECK_INHIBITORS,),
                ),
                true,
            )
            .await?;
        reply.ok_or(Failure::Unknown)
    }
}

impl Login1 for SystemBus {
    async fn owner(&self, budget: &Budget<'_>) -> Result<String, Failure> {
        let owner: String = decode(
            budget
                .dbus(
                    self.connection.call_method(
                        Some("org.freedesktop.DBus"),
                        "/org/freedesktop/DBus",
                        Some("org.freedesktop.DBus"),
                        "GetNameOwner",
                        &(SERVICE,),
                    ),
                    false,
                )
                .await?,
        )?;
        if owner.len() > 255 || zbus::names::UniqueName::try_from(owner.as_str()).is_err() {
            return Err(Failure::Unavailable);
        }
        Ok(owner)
    }
    async fn session(&self, owner: &str, budget: &Budget<'_>) -> Result<Session, Failure> {
        let path: OwnedObjectPath = decode(
            budget
                .dbus(
                    self.connection.call_method(
                        Some(owner),
                        MANAGER_PATH,
                        Some(MANAGER),
                        "GetSessionByPID",
                        &(std::process::id(),),
                    ),
                    false,
                )
                .await?,
        )?;
        let (properties, _) = self
            .properties(owner, path.as_str(), SESSION, budget)
            .await?;
        let session = session_from_properties(&path, properties, Uid::current().as_raw())?;
        let (seat, _) = self
            .properties(owner, &session.seat_path, SEAT, budget)
            .await?;
        validate_seat(&session, seat)?;
        Ok(session)
    }
    async fn capability(&self, owner: &str, budget: &Budget<'_>) -> Result<Capability, Failure> {
        let value: String = decode(
            budget
                .dbus(
                    self.connection.call_method(
                        Some(owner),
                        MANAGER_PATH,
                        Some(MANAGER),
                        "CanSuspend",
                        &(),
                    ),
                    false,
                )
                .await?,
        )?;
        Ok(permission(&value))
    }
    async fn inhibited(&self, owner: &str, budget: &Budget<'_>) -> Result<bool, Failure> {
        let values: Vec<Inhibitor> = decode(
            budget
                .dbus(
                    self.connection.call_method(
                        Some(owner),
                        MANAGER_PATH,
                        Some(MANAGER),
                        "ListInhibitors",
                        &(),
                    ),
                    false,
                )
                .await?,
        )?;
        sleep_inhibited(&values)
    }
    async fn preparing(
        &self,
        owner: &str,
        budget: &Budget<'_>,
    ) -> Result<(bool, u64, Sequence), Failure> {
        let (mut props, position) = self
            .properties(owner, MANAGER_PATH, MANAGER, budget)
            .await?;
        let preparing = bool::try_from(
            props
                .remove("PreparingForSleep")
                .ok_or(Failure::Unavailable)?,
        )
        .map_err(|_| Failure::Unavailable)?;
        let delay_us = u64::try_from(
            props
                .remove("InhibitDelayMaxUSec")
                .ok_or(Failure::Unavailable)?,
        )
        .map_err(|_| Failure::Unavailable)?;
        Ok((preparing, delay_us / 1000, position))
    }
}

pub(super) fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && !matches!(id, "self" | "auto")
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
}
pub(super) fn session_from_properties(
    path: &OwnedObjectPath,
    mut props: HashMap<String, OwnedVariant>,
    uid: u32,
) -> Result<Session, Failure> {
    let invalid = || Failure::SessionUnavailable;
    if props.len() > 64
        || path.as_str().len() > 512
        || !path
            .as_str()
            .starts_with("/org/freedesktop/login1/session/")
    {
        return Err(invalid());
    }
    let id = String::try_from(props.remove("Id").ok_or_else(invalid)?).map_err(|_| invalid())?;
    let (owner, _): (u32, OwnedObjectPath) = props
        .remove("User")
        .ok_or_else(invalid)?
        .try_into()
        .map_err(|_| invalid())?;
    let kind =
        String::try_from(props.remove("Type").ok_or_else(invalid)?).map_err(|_| invalid())?;
    let class =
        String::try_from(props.remove("Class").ok_or_else(invalid)?).map_err(|_| invalid())?;
    let remote =
        bool::try_from(props.remove("Remote").ok_or_else(invalid)?).map_err(|_| invalid())?;
    let active =
        bool::try_from(props.remove("Active").ok_or_else(invalid)?).map_err(|_| invalid())?;
    let (seat_id, seat_path): (String, OwnedObjectPath) = props
        .remove("Seat")
        .ok_or_else(invalid)?
        .try_into()
        .map_err(|_| invalid())?;
    if !safe_id(&id)
        || owner != uid
        || remote
        || !active
        || !matches!(kind.as_str(), "wayland" | "x11")
        || !matches!(class.as_str(), "user" | "user-early")
        || !safe_id(&seat_id)
        || seat_path.as_str().len() > 512
        || !seat_path
            .as_str()
            .starts_with("/org/freedesktop/login1/seat/")
    {
        return Err(invalid());
    }
    Ok(Session {
        id,
        path: path.as_str().into(),
        uid: owner,
        kind,
        seat_id,
        seat_path: seat_path.as_str().into(),
    })
}
pub(super) fn validate_seat(
    session: &Session,
    mut props: HashMap<String, OwnedVariant>,
) -> Result<(), Failure> {
    let invalid = || Failure::SessionUnavailable;
    if props.len() > 64 {
        return Err(invalid());
    }
    let id = String::try_from(props.remove("Id").ok_or_else(invalid)?).map_err(|_| invalid())?;
    let graphical =
        bool::try_from(props.remove("CanGraphical").ok_or_else(invalid)?).map_err(|_| invalid())?;
    let (active, path): (String, OwnedObjectPath) = props
        .remove("ActiveSession")
        .ok_or_else(invalid)?
        .try_into()
        .map_err(|_| invalid())?;
    if !graphical || id != session.seat_id || active != session.id || path.as_str() != session.path
    {
        return Err(invalid());
    }
    Ok(())
}

/// Only the fixed introspection subset is read; no XML entities or DTDs are
/// expanded and no network/filesystem parser is involved.
pub(super) fn supported_api(xml: &str) -> Result<(), Failure> {
    const REQUIRED: [(&str, &str, &str, &str); 8] = [
        ("method", "GetSessionByPID", "u", "o"),
        ("method", "CanSuspend", "", "s"),
        ("method", "SuspendWithFlags", "t", ""),
        ("method", "ListInhibitors", "", "a(ssssuu)"),
        ("method", "Inhibit", "ssss", "h"),
        ("signal", "PrepareForSleep", "", "b"),
        ("property", "PreparingForSleep", "", "b"),
        ("property", "InhibitDelayMaxUSec", "", "t"),
    ];
    if xml.len() > MAX_REPLY_BYTES {
        return Err(Failure::Unavailable);
    }
    let mut valid = [false; 8];
    let mut seen = [false; 8];
    let mut remaining = xml;
    let mut manager = false;
    let mut manager_seen = false;
    let mut current: Option<(usize, String, String)> = None;
    let mut count = 0;
    while let Some(start) = remaining.find('<') {
        remaining = &remaining[start + 1..];
        if remaining.starts_with("!--") {
            let end = remaining.find("-->").ok_or(Failure::Unavailable)?;
            remaining = &remaining[end + 3..];
            continue;
        }
        let end = remaining.find('>').ok_or(Failure::Unavailable)?;
        let tag = remaining[..end].trim();
        remaining = &remaining[end + 1..];
        count += 1;
        if count > 4096 {
            return Err(Failure::Unavailable);
        }
        let name = tag
            .split_ascii_whitespace()
            .next()
            .unwrap_or_default()
            .trim_end_matches('/');
        match name {
            "interface" => {
                manager = attribute(tag, "name") == Some(MANAGER);
                current = None;
                if manager && std::mem::replace(&mut manager_seen, true) {
                    return Err(Failure::Unavailable);
                }
            }
            "/interface" => {
                if current.is_some() {
                    return Err(Failure::Unavailable);
                }
                manager = false;
            }
            "method" | "signal" | "property" if manager => {
                if current.is_some() {
                    return Err(Failure::Unavailable);
                }
                let index = REQUIRED.iter().position(|(kind, wanted, _, _)| {
                    *kind == name && attribute(tag, "name") == Some(*wanted)
                });
                if let Some(index) = index {
                    if std::mem::replace(&mut seen[index], true) {
                        return Err(Failure::Unavailable);
                    }
                    if name == "property" {
                        valid[index] = attribute(tag, "type") == Some(REQUIRED[index].3)
                            && matches!(attribute(tag, "access"), Some("read" | "readwrite"));
                    } else if tag.ends_with('/') {
                        valid[index] = REQUIRED[index].2.is_empty() && REQUIRED[index].3.is_empty();
                    } else {
                        current = Some((index, String::new(), String::new()));
                    }
                }
            }
            "arg" if manager => {
                if let Some((index, inputs, outputs)) = current.as_mut() {
                    let signature = attribute(tag, "type").ok_or(Failure::Unavailable)?;
                    if signature.len() > 32 {
                        return Err(Failure::Unavailable);
                    }
                    let direction =
                        attribute(tag, "direction").unwrap_or(if REQUIRED[*index].0 == "signal" {
                            "out"
                        } else {
                            "in"
                        });
                    match direction {
                        "in" if REQUIRED[*index].0 == "method" => inputs.push_str(signature),
                        "out" => outputs.push_str(signature),
                        _ => return Err(Failure::Unavailable),
                    }
                    if inputs.len() > 64 || outputs.len() > 64 {
                        return Err(Failure::Unavailable);
                    }
                }
            }
            "/method" | "/signal" if manager => {
                if let Some((index, inputs, outputs)) = current.take() {
                    valid[index] = inputs == REQUIRED[index].2
                        && outputs == REQUIRED[index].3
                        && name.trim_start_matches('/') == REQUIRED[index].0;
                }
            }
            _ => {}
        }
    }
    if manager || current.is_some() || valid.iter().any(|valid| !valid) {
        Err(Failure::Unavailable)
    } else {
        Ok(())
    }
}

pub(super) fn attribute<'a>(tag: &'a str, wanted: &str) -> Option<&'a str> {
    let mut remaining = tag
        .find(char::is_whitespace)
        .map(|index| &tag[index..])?
        .trim_start();
    while !remaining.is_empty() && remaining != "/" {
        let equal = remaining.find('=')?;
        let key = remaining[..equal].trim();
        remaining = remaining[equal + 1..].trim_start();
        let quote = remaining.chars().next()?;
        if quote != '"' && quote != '\'' {
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
