//! Native polkit session worker. The standard D-Bus agent endpoint is exported
//! with GIO; passwords go ONLY to libpolkit-agent's trusted PAM helper pipe.
//! No blocking terminal input and no Rust password/authorization engine.
use crate::polkit::{self, Session};
use crate::protocol::{plain, Command, Event, Identity};
use async_channel::{Receiver, Sender};
use glib::{prelude::*, variant::ToVariant};
use smithay_client_toolkit::reexports::calloop::channel;
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    error::Error,
    rc::Rc,
    thread,
    time::Duration,
};
use zeroize::Zeroizing;

const AUTHORITY: &str = "org.freedesktop.PolicyKit1";
const LOGIN: &str = "org.freedesktop.login1";
const AUTHORITY_PATH: &str = "/org/freedesktop/PolicyKit1/Authority";
const AUTHORITY_IFACE: &str = "org.freedesktop.PolicyKit1.Authority";
const AGENT_PATH: &str = "/org/alcard/PolicyKit1/AuthenticationAgent";
const AGENT_IFACE: &str = "org.freedesktop.PolicyKit1.AuthenticationAgent";
pub const PRESENCE: &str = "org.alcard.Authentication";
const CANCELLED: &str = "org.freedesktop.PolicyKit1.Error.Cancelled";
const FAILED: &str = "org.freedesktop.PolicyKit1.Error.Failed";
const XML: &str = r#"<node><interface name="org.freedesktop.PolicyKit1.AuthenticationAgent">
<method name="BeginAuthentication"><arg type="s" direction="in"/><arg type="s" direction="in"/>
<arg type="s" direction="in"/><arg type="a{ss}" direction="in"/><arg type="s" direction="in"/>
<arg type="a(sa{sv})" direction="in"/></method>
<method name="CancelAuthentication"><arg type="s" direction="in"/></method>
</interface></node>"#;

type Result<T> = std::result::Result<T, Box<dyn Error>>;
type RawIdentities = Vec<(String, HashMap<String, glib::Variant>)>;
type BeginArgs = (
    String,
    String,
    String,
    HashMap<String, String>,
    String,
    RawIdentities,
);

#[derive(Clone, Copy)]
pub enum Registration {
    LoginSession,
    #[cfg(test)]
    OwnProcess,
}
enum Work {
    User(Command),
    Prompt {
        id: u64,
        generation: u64,
        text: String,
        echo: bool,
    },
    Notice {
        id: u64,
        generation: u64,
        text: String,
        error: bool,
    },
    Completed {
        id: u64,
        generation: u64,
        gained: bool,
    },
    Timeout(u64),
    LostAuthority,
    LostSession,
}

pub struct Worker {
    commands: Sender<Work>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Worker {
    pub fn start(
        events: channel::Sender<Event>,
        registration: Registration,
    ) -> std::io::Result<Self> {
        let (commands, receive) = async_channel::unbounded();
        let sender = commands.clone();
        let thread = thread::Builder::new()
            .name("alcard-polkit-native".into())
            .spawn(move || {
                if let Err(error) = run(events.clone(), sender, receive, registration) {
                    // Initialization errors contain no authentication cookie or PAM response.
                    let _ = if error.is::<crate::protocol::SessionLost>() {
                        events.send(Event::SessionLost)
                    } else {
                        events.send(Event::Fatal(format!("Native polkit unavailable: {error}")))
                    };
                }
            })?;
        Ok(Self {
            commands,
            thread: Some(thread),
        })
    }
    pub fn send(&self, command: Command) -> bool {
        self.commands.try_send(Work::User(command)).is_ok()
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.commands.try_send(Work::User(Command::Shutdown));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Challenge {
    id: u64,
    cookie: Zeroizing<String>,
    identities: Vec<polkit::UnixUser>,
    selected: Option<usize>,
    invocation: Option<gio::DBusMethodInvocation>,
    session: Option<Session>,
    generation: u64,
    serial: u64,
    waiting: Option<u64>,
    retry: bool,
    attempts: u8,
    timeout: glib::JoinHandle<()>,
}
struct Core {
    active: Option<Challenge>,
    next_id: u64,
    events: channel::Sender<Event>,
    sender: Sender<Work>,
    session_bus: gio::DBusConnection,
    context: glib::MainContext,
}

fn dbus_call(
    connection: &gio::DBusConnection,
    method: &str,
    parameters: &glib::Variant,
) -> Result<glib::Variant> {
    Ok(connection.call_sync(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        method,
        Some(parameters),
        None,
        gio::DBusCallFlags::NONE,
        3000,
        gio::Cancellable::NONE,
    )?)
}
fn name_owner(connection: &gio::DBusConnection, name: &str) -> Result<String> {
    let (owner,) = dbus_call(connection, "GetNameOwner", &(name,).to_variant())?
        .get::<(String,)>()
        .ok_or("No current service owner")?;
    if !owner.starts_with(':') {
        return Err("Service did not have a unique D-Bus owner".into());
    }
    Ok(owner)
}

fn session_error(_error: impl Error + 'static, registration: Registration) -> Box<dyn Error> {
    match registration {
        Registration::LoginSession => Box::new(crate::protocol::SessionLost),
        #[cfg(test)]
        Registration::OwnProcess => Box::new(_error),
    }
}

fn authority_result<T>(
    result: Result<T>,
    system: &gio::DBusConnection,
    session: &gio::DBusConnection,
    login_owner: Option<&str>,
) -> Result<T> {
    match result {
        Err(_)
            if login_owner.is_some_and(|owner| {
                system.is_closed()
                    || session.is_closed()
                    || !name_owner(system, LOGIN).is_ok_and(|current| current == owner)
            }) =>
        {
            Err(Box::new(crate::protocol::SessionLost))
        }
        other => other,
    }
}
fn public_identities(raw: RawIdentities) -> Option<(Vec<polkit::UnixUser>, Vec<Identity>)> {
    if raw.is_empty() || raw.len() > 16 {
        return None;
    }
    let mut users = Vec::new();
    let mut labels = Vec::new();
    let mut seen = HashSet::new();
    for (kind, attributes) in raw {
        if kind != "unix-user" {
            return None;
        }
        let uid = attributes.get("uid")?.get::<u32>()?;
        if uid > i32::MAX as u32 || !seen.insert(uid) {
            return None;
        }
        let user = polkit::UnixUser::new(uid as i32)?;
        let name = user
            .name()
            .map(|s| plain(s.as_str(), 96))
            .unwrap_or_else(|| "user".into());
        labels.push(Identity {
            index: users.len(),
            label: format!("{name} (uid {uid})"),
        });
        users.push(user);
    }
    Some((users, labels))
}
impl Core {
    fn presence(&self, enabled: bool) -> Result<()> {
        if enabled {
            let result = dbus_call(
                &self.session_bus,
                "RequestName",
                &(PRESENCE, 4_u32).to_variant(),
            )?;
            let (code,) = result
                .get::<(u32,)>()
                .ok_or("Invalid D-Bus name response")?;
            if code != 1 && code != 4 {
                return Err("Another authentication dialog owns the session".into());
            }
        } else {
            dbus_call(&self.session_bus, "ReleaseName", &(PRESENCE,).to_variant())?;
        }
        Ok(())
    }
    fn begin(&mut self, parameters: glib::Variant, invocation: gio::DBusMethodInvocation) {
        if self.active.is_some() {
            invocation.return_dbus_error(FAILED, "Another authentication is in progress");
            return;
        }
        let Some((action, reason, _, _, cookie, identities)) = parameters.get::<BeginArgs>() else {
            invocation.return_dbus_error(FAILED, "Invalid authentication request");
            return;
        };
        let cookie = Zeroizing::new(cookie);
        if cookie.is_empty() || cookie.len() > 4096 {
            invocation.return_dbus_error(FAILED, "Invalid authentication request");
            return;
        }
        let Some((users, labels)) = public_identities(identities) else {
            invocation.return_dbus_error(FAILED, "Unsupported authentication identities");
            return;
        };
        if self.presence(true).is_err() {
            invocation.return_dbus_error(FAILED, "Authentication UI is unavailable");
            return;
        }
        self.next_id = self.next_id.wrapping_add(1).max(1);
        let id = self.next_id;
        let send = self.sender.clone();
        let timeout = self.context.spawn_local(async move {
            glib::timeout_future(Duration::from_secs(300)).await;
            let _ = send.try_send(Work::Timeout(id));
        });
        self.active = Some(Challenge {
            id,
            cookie,
            identities: users,
            selected: None,
            invocation: Some(invocation),
            session: None,
            generation: 0,
            serial: 0,
            waiting: None,
            retry: false,
            attempts: 0,
            timeout,
        });
        if self
            .events
            .send(Event::Open {
                id,
                action: plain(&action, 256),
                reason: plain(&reason, 2048),
                identities: labels,
            })
            .is_err()
        {
            self.finish(false, true);
        }
    }
    fn finish(&mut self, authorized: bool, cancelled: bool) {
        let Some(mut active) = self.active.take() else {
            return;
        };
        active.timeout.abort();
        // Active state is removed BEFORE cancellation. Synchronous GObject
        // callbacks only queue events, so no reentrant borrow or double reply.
        if let Some(session) = active.session.take() {
            session.cancel();
        }
        if let Some(invocation) = active.invocation.take() {
            if cancelled {
                invocation.return_dbus_error(CANCELLED, "Authentication cancelled");
            } else {
                invocation.return_value(None);
            }
        }
        let _ = self.presence(false);
        let _ = self.events.send(Event::Closed {
            id: active.id,
            authorized,
        });
    }
    fn start_session(&mut self) {
        let Some(active) = self.active.as_mut() else {
            return;
        };
        let Some(index) = active.selected else {
            return;
        };
        if active.session.is_some() || active.attempts >= 3 {
            return;
        }
        active.attempts += 1;
        active.retry = false;
        active.waiting = None;
        active.generation += 1;
        let (id, generation) = (active.id, active.generation);
        let Some(session) = Session::new(&active.identities[index], active.cookie.as_str()) else {
            self.finish(false, true);
            return;
        };
        let send = self.sender.clone();
        session.connect_request(move |text, echo| {
            let _ = send.try_send(Work::Prompt {
                id,
                generation,
                text: plain(text, 256),
                echo,
            });
        });
        let send = self.sender.clone();
        session.connect_show_info(move |text| {
            let _ = send.try_send(Work::Notice {
                id,
                generation,
                text: plain(text, 2048),
                error: false,
            });
        });
        let send = self.sender.clone();
        session.connect_show_error(move |text| {
            let _ = send.try_send(Work::Notice {
                id,
                generation,
                text: plain(text, 2048),
                error: true,
            });
        });
        let send = self.sender.clone();
        session.connect_completed(move |gained| {
            let _ = send.try_send(Work::Completed {
                id,
                generation,
                gained,
            });
        });
        active.session = Some(session.clone());
        session.initiate();
    }
    /// Return true to stop the native event loop.
    fn work(&mut self, work: Work) -> bool {
        match work {
            Work::LostSession => {
                self.finish(false, true);
                let _ = self.events.send(Event::SessionLost);
                return true;
            }
            Work::LostAuthority => {
                self.finish(false, true);
                // GIO may retain callback closures (and our event sender) until
                // another GLib iteration after the native loop has stopped.
                // Notify the frontend explicitly so its supervisor can restart
                // against the new authority without ending this login session.
                let _ = self.events.send(Event::Fatal(
                    "Polkit authority owner was lost; restarting authentication agent".into(),
                ));
                return true;
            }
            Work::User(Command::Shutdown) => {
                self.finish(false, true);
                return true;
            }
            Work::User(Command::Cancel { id }) | Work::Timeout(id) => {
                if self.active.as_ref().is_some_and(|a| a.id == id) {
                    self.finish(false, true);
                }
            }
            Work::User(Command::Choose { id, index }) => {
                if let Some(active) = self
                    .active
                    .as_mut()
                    .filter(|a| a.id == id && a.selected.is_none() && index < a.identities.len())
                {
                    active.selected = Some(index);
                    self.start_session();
                }
            }
            Work::User(Command::Retry { id }) => {
                if self
                    .active
                    .as_ref()
                    .is_some_and(|a| a.id == id && a.retry && a.session.is_none())
                {
                    self.start_session();
                }
            }
            Work::User(Command::Respond { id, serial, secret }) => {
                if let Some(active) = self
                    .active
                    .as_mut()
                    .filter(|a| a.id == id && a.waiting == Some(serial))
                {
                    active.waiting = None;
                    if let Some(session) = active.session.as_ref() {
                        session.response(secret.as_str());
                    }
                }
                // Secret's entire fixed allocation is wiped here, even for stale replies.
            }
            Work::Prompt {
                id,
                generation,
                text,
                echo,
            } => {
                if let Some(active) = self
                    .active
                    .as_mut()
                    .filter(|a| a.id == id && a.generation == generation && a.session.is_some())
                {
                    if active.waiting.is_some() {
                        self.finish(false, true);
                        return false;
                    }
                    active.serial += 1;
                    active.waiting = Some(active.serial);
                    if self
                        .events
                        .send(Event::Challenge {
                            id,
                            serial: active.serial,
                            prompt: text,
                            echo,
                        })
                        .is_err()
                    {
                        self.finish(false, true);
                    }
                }
            }
            Work::Notice {
                id,
                generation,
                text,
                error,
            } => {
                if self
                    .active
                    .as_ref()
                    .is_some_and(|a| a.id == id && a.generation == generation)
                {
                    let _ = self.events.send(Event::Notice { id, text, error });
                }
            }
            Work::Completed {
                id,
                generation,
                gained,
            } => {
                if let Some(active) = self
                    .active
                    .as_mut()
                    .filter(|a| a.id == id && a.generation == generation && a.session.is_some())
                {
                    // Processed from the queue AFTER signal emission: safely release native session.
                    active.session.take();
                    active.waiting = None;
                    if gained {
                        self.finish(true, false);
                    } else if active.attempts >= 3 {
                        self.finish(false, false);
                    } else {
                        active.retry = true;
                        let _ = self.events.send(Event::Retry { id });
                    }
                }
            }
        }
        false
    }
}

fn subject(registration: Registration) -> Result<(String, HashMap<String, glib::Variant>)> {
    let mut details = HashMap::new();
    match registration {
        Registration::LoginSession => {
            let id = polkit::login_session_id(nix::unistd::getpid().as_raw())?;
            if id.is_empty() {
                return Err("No authenticated login session".into());
            }
            details.insert("session-id".into(), id.as_str().to_variant());
            Ok(("unix-session".into(), details))
        }
        #[cfg(test)]
        Registration::OwnProcess => {
            let stat = std::fs::read_to_string("/proc/self/stat")?;
            let start: u64 = stat
                .rsplit_once(')')
                .ok_or("Invalid process stat")?
                .1
                .split_whitespace()
                .nth(19)
                .ok_or("Missing start time")?
                .parse()?;
            details.insert("pid".into(), std::process::id().to_variant());
            details.insert("start-time".into(), start.to_variant());
            details.insert(
                "uid".into(),
                (nix::unistd::Uid::current().as_raw() as i32).to_variant(),
            );
            Ok(("unix-process".into(), details))
        }
    }
}
fn run(
    events: channel::Sender<Event>,
    sender: Sender<Work>,
    receiver: Receiver<Work>,
    registration: Registration,
) -> Result<()> {
    let context = glib::MainContext::new();
    context.with_thread_default(|| -> Result<()> {
        let main_loop = glib::MainLoop::new(Some(&context), false);
        let system = gio::bus_get_sync(gio::BusType::System, gio::Cancellable::NONE)
            .map_err(|error| session_error(error, registration))?;
        let session_bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)
            .map_err(|error| session_error(error, registration))?;
        system.set_exit_on_close(false);
        session_bus.set_exit_on_close(false);
        let session_monitor = session_bus.clone();
        let session_sender = sender.clone();
        let critical_session = matches!(registration, Registration::LoginSession);
        let session_closed = session_monitor.connect_closed(move |_, _, _| {
            let _ = session_sender.try_send(if critical_session {
                Work::LostSession
            } else {
                Work::LostAuthority
            });
        });
        // A login1 replacement invalidates our authenticated-session binding,
        // even when polkit itself and the system bus have survived. Resolve
        // the real session again in a fresh process, never infer a replacement.
        let login_owner = match registration {
            Registration::LoginSession => Some(
                name_owner(&system, LOGIN)
                    .map_err(|_| Box::new(crate::protocol::SessionLost) as Box<dyn Error>)?,
            ),
            #[cfg(test)]
            Registration::OwnProcess => None,
        };
        let subject = subject(registration).map_err(|error| {
            if critical_session {
                Box::new(crate::protocol::SessionLost) as Box<dyn Error>
            } else {
                error
            }
        })?;
        let core = Rc::new(RefCell::new(Core {
            active: None,
            next_id: 0,
            events: events.clone(),
            sender: sender.clone(),
            session_bus: session_bus.clone(),
            context: context.clone(),
        }));
        // Activate the real authority, then bind method admission to its unique bus owner.
        authority_result(
            system
                .call_sync(
                    Some(AUTHORITY),
                    AUTHORITY_PATH,
                    "org.freedesktop.DBus.Peer",
                    "Ping",
                    None,
                    None,
                    gio::DBusCallFlags::NONE,
                    5000,
                    gio::Cancellable::NONE,
                )
                .map_err(Into::into),
            &system,
            &session_bus,
            login_owner.as_deref(),
        )?;
        let owner = authority_result(
            name_owner(&system, AUTHORITY),
            &system,
            &session_bus,
            login_owner.as_deref(),
        )?;
        let info = gio::DBusNodeInfo::for_xml(XML)?
            .lookup_interface(AGENT_IFACE)
            .ok_or("Invalid agent interface")?;
        let state = core.clone();
        let expected = owner.clone();
        let object = system
            .register_object(AGENT_PATH, &info)
            .method_call(move |_, caller, _, _, method, parameters, invocation| {
                if caller != Some(expected.as_str()) {
                    invocation.return_dbus_error(
                        "org.freedesktop.DBus.Error.AccessDenied",
                        "Only the registered polkit authority may request authentication",
                    );
                    return;
                }
                let mut core = state.borrow_mut();
                match method {
                    "BeginAuthentication" => core.begin(parameters, invocation),
                    "CancelAuthentication" => {
                        if let Some((cookie,)) = parameters.get::<(String,)>() {
                            let cookie = Zeroizing::new(cookie);
                            if core
                                .active
                                .as_ref()
                                .is_some_and(|a| a.cookie.as_str() == cookie.as_str())
                            {
                                core.finish(false, true);
                            }
                            invocation.return_value(None);
                        } else {
                            invocation.return_dbus_error(FAILED, "Invalid cancellation request");
                        }
                    }
                    _ => invocation.return_dbus_error(
                        "org.freedesktop.DBus.Error.UnknownMethod",
                        "Unknown agent method",
                    ),
                }
            })
            .build()?;
        let send = sender.clone();
        let expected_owner = owner.clone();
        let subscription = system.subscribe_to_signal(
            Some("org.freedesktop.DBus"),
            Some("org.freedesktop.DBus"),
            Some("NameOwnerChanged"),
            Some("/org/freedesktop/DBus"),
            Some(AUTHORITY),
            gio::DBusSignalFlags::NONE,
            move |signal| {
                if let Some((_, old, new)) = signal.parameters.get::<(String, String, String)>() {
                    if old == expected_owner && new != expected_owner {
                        let _ = send.try_send(Work::LostAuthority);
                    }
                }
            },
        );
        let login_subscription = login_owner.as_ref().map(|owner| {
            let owner = owner.clone();
            let send = sender.clone();
            system.subscribe_to_signal(
                Some("org.freedesktop.DBus"),
                Some("org.freedesktop.DBus"),
                Some("NameOwnerChanged"),
                Some("/org/freedesktop/DBus"),
                Some(LOGIN),
                gio::DBusSignalFlags::NONE,
                move |signal| {
                    if let Some((_, old, new)) = signal.parameters.get::<(String, String, String)>()
                    {
                        if old == owner && new != owner {
                            let _ = send.try_send(Work::LostSession);
                        }
                    }
                },
            )
        });
        let send = sender.clone();
        let closed = system.connect_closed(move |_, _, _| {
            let _ = send.try_send(if critical_session {
                Work::LostSession
            } else {
                Work::LostAuthority
            });
        });
        authority_result(
            system
                .call_sync(
                    Some(&owner),
                    AUTHORITY_PATH,
                    AUTHORITY_IFACE,
                    "RegisterAuthenticationAgent",
                    Some(&(subject.clone(), "C.UTF-8", AGENT_PATH).to_variant()),
                    None,
                    gio::DBusCallFlags::NONE,
                    5000,
                    gio::Cancellable::NONE,
                )
                .map_err(Into::into),
            &system,
            &session_bus,
            login_owner.as_deref(),
        )?;
        // Close the query/subscription/registration startup race. Both the
        // registration destination and later admission are pinned to this
        // exact owner; a well-known-name replacement cannot inherit either.
        if login_owner.as_ref().is_some_and(|owner| {
            !name_owner(&system, LOGIN).is_ok_and(|current| current.as_str() == owner.as_str())
        }) {
            return Err(Box::new(crate::protocol::SessionLost));
        }
        if authority_result(
            name_owner(&system, AUTHORITY),
            &system,
            &session_bus,
            login_owner.as_deref(),
        )? != owner
        {
            return Err("Authentication services changed during registration".into());
        }
        let state = core.clone();
        let loop_control = main_loop.clone();
        let task = context.spawn_local(async move {
            while let Ok(work) = receiver.recv().await {
                if state.borrow_mut().work(work) {
                    break;
                }
            }
            state.borrow_mut().finish(false, true);
            loop_control.quit();
        });
        let _ = events.send(Event::Ready);
        main_loop.run();
        task.abort();
        core.borrow_mut().finish(false, true);
        let _ = system.call_sync(
            Some(&owner),
            AUTHORITY_PATH,
            AUTHORITY_IFACE,
            "UnregisterAuthenticationAgent",
            Some(&(subject, AGENT_PATH).to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            1000,
            gio::Cancellable::NONE,
        );
        drop(subscription);
        drop(login_subscription);
        system.disconnect(closed);
        session_monitor.disconnect(session_closed);
        let _ = system.unregister_object(object);
        Ok(())
    })?
}

#[cfg(test)]
#[path = "unit_tests/native.rs"]
mod tests;

#[cfg(test)]
#[path = "unit_tests/native_wire.rs"]
mod wire_tests;
