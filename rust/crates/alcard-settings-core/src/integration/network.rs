//! Isolated NetworkManager protocol fixture. No real network interface is changed.
use super::*;
use std::{
    io::{BufRead, BufReader},
    os::unix::fs::DirBuilderExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU64, AtomicUsize},
};

const DEV: &str = "/org/freedesktop/NetworkManager/Devices/3";
const ACCESS_POINT: &str = "/org/freedesktop/NetworkManager/AccessPoint/7";
const WRONG_ACCESS_POINT: &str = "/org/freedesktop/NetworkManager/AccessPoint/8";
const SAVED: &str = "/org/freedesktop/NetworkManager/Settings/10";
const OLD_PROFILE: &str = "/org/freedesktop/NetworkManager/Settings/9";
const OLD: &str = "/org/freedesktop/NetworkManager/ActiveConnection/30";
const OURS: &str = "/org/freedesktop/NetworkManager/ActiveConnection/31";
const OTHER: &str = "/org/freedesktop/NetworkManager/ActiveConnection/32";
const SSID: &str = "Alcard protocol fixture";
const UUID: &str = "11111111-2222-4333-8444-555555555555";
const WAIT: Duration = Duration::from_secs(4);

fn object(value: &str) -> OwnedObjectPath {
    OwnedObjectPath::try_from(value).unwrap()
}

struct PrivateBus {
    child: Child,
    directory: PathBuf,
    address: String,
}
impl PrivateBus {
    fn start() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "alcard-network-bus-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        let child = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .arg(format!("--address=unix:path={}/bus", directory.display()))
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("network integration requires dbus-daemon");
        let mut bus = Self {
            child,
            directory,
            address: String::new(),
        };
        let output = bus.child.stdout.take().unwrap();
        let (send, receive) = mpsc::sync_channel(1);
        let reader = thread::spawn(move || {
            let mut address = String::new();
            let result = BufReader::new(output)
                .read_line(&mut address)
                .map(|_| address);
            let _ = send.send(result);
        });
        bus.address = receive
            .recv_timeout(WAIT)
            .expect("private bus startup deadline")
            .unwrap()
            .trim()
            .to_owned();
        reader.join().unwrap();
        assert!(bus.address.starts_with("unix:path="));
        bus
    }
    fn connect(&self) -> Connection {
        zbus::block_on(
            zbus::connection::Builder::address(self.address.as_str())
                .method_timeout(WAIT)
                .build(),
        )
        .unwrap()
    }
}
impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

#[derive(Default)]
struct Gate {
    released: AtomicBool,
    waker: Mutex<Option<Waker>>,
}
impl Gate {
    async fn wait(&self) {
        poll_fn(|cx| {
            *self.waker.lock().unwrap() = Some(cx.waker().clone());
            if self.released.load(Ordering::Acquire) {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await
    }
    fn release(&self) {
        self.released.store(true, Ordering::Release);
        if let Some(waker) = self.waker.lock().unwrap().take() {
            waker.wake();
        }
    }
}

#[derive(Clone, Copy)]
struct Case {
    saved: bool,
    encrypted: bool,
    ask_secret: bool,
    already_connected: bool,
    restricted_bssid: bool,
    delayed: bool,
    concurrent: bool,
    reused: bool,
    denied: bool,
    final_state: u32,
}
impl Default for Case {
    fn default() -> Self {
        Self {
            saved: true,
            encrypted: true,
            ask_secret: false,
            already_connected: false,
            restricted_bssid: false,
            delayed: false,
            concurrent: false,
            reused: false,
            denied: false,
            final_state: 2,
        }
    }
}
struct State {
    case: Case,
    agent: Option<String>,
    activated: Vec<String>,
    added: usize,
    deactivated: Vec<String>,
    current: String,
    started: bool,
    secret_observed: bool,
}
struct Shared {
    state: Mutex<State>,
    gate: Gate,
    started: mpsc::Sender<()>,
    reads: AtomicUsize,
}
fn profile(case: Case, old: bool) -> Settings {
    let ssid = if old && !case.already_connected {
        "Previous fixture network"
    } else {
        SSID
    };
    let ap = AccessPoint {
        path: object(ACCESS_POINT),
        properties: Properties::from([
            ("Flags".into(), u32::from(case.encrypted).into()),
            (
                "WpaFlags".into(),
                (if case.encrypted { 0x100u32 } else { 0 }).into(),
            ),
            ("RsnFlags".into(), 0u32.into()),
        ]),
    };
    let mut result = ap.new_profile(ssid, UUID).unwrap();
    result.insert(
        "ipv4".into(),
        Properties::from([("method".into(), text("manual"))]),
    );
    if case.restricted_bssid && !old {
        result.get_mut(WIRELESS).unwrap().insert(
            "bssid".into(),
            OwnedVariant::try_from(zbus::Variant::from(vec![0u8, 0x11, 0x22, 0x33, 0x44, 0x55]))
                .unwrap(),
        );
    }
    result
}
struct Manager(Arc<Shared>);
impl Manager {
    async fn activate(
        &self,
        conn: &Connection,
        connection: Settings,
        saved: Option<&OwnedObjectPath>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        let (case, agent) = {
            let mut state = self.0.state.lock().unwrap();
            if state.case.denied {
                return Err(zbus::fdo::Error::AccessDenied("fixture denial".into()));
            }
            if let Some(saved) = saved {
                assert_eq!(saved.as_str(), SAVED);
                state.activated.push(saved.as_str().into());
            } else {
                state.added += 1;
                assert_eq!(
                    connection
                        .get(WIRELESS)
                        .and_then(|p| bytes(p, "ssid"))
                        .as_deref(),
                    Some(SSID.as_bytes())
                );
                assert!(!connection.values().any(|p| p.contains_key("psk")));
            }
            state.started = true;
            if state.case.concurrent {
                state.current = OTHER.into();
            }
            (state.case, state.agent.clone().unwrap())
        };
        if case.ask_secret {
            let reply = conn
                .call_method(
                    Some(agent.as_str()),
                    AGENT_PATH,
                    Some("org.freedesktop.NetworkManager.SecretAgent"),
                    "GetSecrets",
                    &(
                        connection,
                        object(SAVED),
                        SECURITY,
                        Vec::<String>::new(),
                        5u32,
                    ),
                )
                .await
                .map_err(|_| zbus::fdo::Error::Failed("fixture secret request failed".into()))?;
            let secrets: Settings = reply.body().deserialize().unwrap();
            assert_eq!(string(&secrets[SECURITY], "psk"), Some("fixture-password"));
            self.0.state.lock().unwrap().secret_observed = true;
        }
        let _ = self.0.started.send(());
        if case.delayed {
            self.0.gate.wait().await;
        }
        Ok(object(if case.reused { OLD } else { OURS }))
    }
}
#[interface(name = "org.freedesktop.NetworkManager")]
impl Manager {
    fn get_device_by_ip_iface(&self, iface: &str) -> zbus::fdo::Result<OwnedObjectPath> {
        assert_eq!(iface, "fixture-wlan0");
        Ok(object(DEV))
    }
    #[zbus(property)]
    fn active_connections(&self) -> Vec<OwnedObjectPath> {
        let state = self.0.state.lock().unwrap();
        let mut result = vec![object(OLD)];
        if state.started {
            result.push(object(OURS));
        }
        if state.case.concurrent && state.started {
            result.push(object(OTHER));
        }
        result
    }
    async fn activate_connection(
        &self,
        connection: OwnedObjectPath,
        device: OwnedObjectPath,
        specific_object: OwnedObjectPath,
        #[zbus(connection)] conn: &Connection,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        assert_eq!(device.as_str(), DEV);
        assert_eq!(specific_object.as_str(), ACCESS_POINT);
        let case = self.0.state.lock().unwrap().case;
        self.activate(conn, profile(case, false), Some(&connection))
            .await
    }
    async fn add_and_activate_connection(
        &self,
        connection: Settings,
        device: OwnedObjectPath,
        specific_object: OwnedObjectPath,
        #[zbus(connection)] conn: &Connection,
    ) -> zbus::fdo::Result<(OwnedObjectPath, OwnedObjectPath)> {
        assert_eq!(device.as_str(), DEV);
        assert_eq!(specific_object.as_str(), ACCESS_POINT);
        self.activate(conn, connection, None)
            .await
            .map(|active| (object(SAVED), active))
    }
    fn deactivate_connection(&self, active_connection: OwnedObjectPath) {
        let mut state = self.0.state.lock().unwrap();
        state.deactivated.push(active_connection.as_str().into());
        if state.current == active_connection.as_str() {
            state.current = "/".into();
        }
    }
}
struct Device(Arc<Shared>);
#[interface(name = "org.freedesktop.NetworkManager.Device")]
impl Device {
    #[zbus(property)]
    fn device_type(&self) -> u32 {
        2
    }
    #[zbus(property)]
    fn active_connection(&self) -> OwnedObjectPath {
        object(&self.0.state.lock().unwrap().current)
    }
    #[zbus(property)]
    fn available_connections(&self) -> Vec<OwnedObjectPath> {
        if self.0.state.lock().unwrap().case.saved {
            vec![object(SAVED)]
        } else {
            Vec::new()
        }
    }
}
struct Wireless(bool);
#[interface(name = "org.freedesktop.NetworkManager.Device.Wireless")]
impl Wireless {
    fn get_all_access_points(&self) -> Vec<OwnedObjectPath> {
        if self.0 {
            vec![object(WRONG_ACCESS_POINT), object(ACCESS_POINT)]
        } else {
            vec![object(ACCESS_POINT)]
        }
    }
}
struct Point(Case, bool);
#[interface(name = "org.freedesktop.NetworkManager.AccessPoint")]
impl Point {
    #[zbus(property)]
    fn ssid(&self) -> Vec<u8> {
        SSID.as_bytes().to_vec()
    }
    #[zbus(property)]
    fn flags(&self) -> u32 {
        u32::from(self.0.encrypted)
    }
    #[zbus(property)]
    fn wpa_flags(&self) -> u32 {
        if self.0.encrypted {
            0x100
        } else {
            0
        }
    }
    #[zbus(property)]
    fn rsn_flags(&self) -> u32 {
        0
    }
    #[zbus(property)]
    fn mode(&self) -> u32 {
        2
    }
    #[zbus(property)]
    fn strength(&self) -> u8 {
        if self.1 {
            99
        } else {
            90
        }
    }
    #[zbus(property)]
    fn hw_address(&self) -> &str {
        if self.1 {
            "AA:BB:CC:DD:EE:FF"
        } else {
            "00:11:22:33:44:55"
        }
    }
}
struct Saved {
    case: Case,
    old: bool,
}
#[interface(name = "org.freedesktop.NetworkManager.Settings.Connection")]
impl Saved {
    fn get_settings(&self) -> Settings {
        profile(self.case, self.old)
    }
}
struct Active {
    shared: Arc<Shared>,
    old: bool,
}
#[interface(name = "org.freedesktop.NetworkManager.Connection.Active")]
impl Active {
    #[zbus(property)]
    fn state(&self) -> u32 {
        if self.old {
            2
        } else {
            self.shared.reads.fetch_add(1, Ordering::SeqCst);
            self.shared.state.lock().unwrap().case.final_state
        }
    }
    #[zbus(property)]
    fn connection(&self) -> OwnedObjectPath {
        object(if self.old { OLD_PROFILE } else { SAVED })
    }
}
struct Agents(Arc<Shared>);
#[interface(name = "org.freedesktop.NetworkManager.AgentManager")]
impl Agents {
    fn register_with_capabilities(
        &self,
        identifier: &str,
        capabilities: u32,
        #[zbus(header)] header: Header<'_>,
    ) {
        assert_eq!(identifier, "org.alcard.Settings");
        assert_eq!(capabilities, 0);
        self.0.state.lock().unwrap().agent = Some(header.sender().unwrap().as_str().into());
    }
}
struct Peer {
    conn: Connection,
    shared: Arc<Shared>,
    started: mpsc::Receiver<()>,
}
impl Peer {
    fn start(bus: &PrivateBus, case: Case) -> Self {
        let conn = bus.connect();
        let (started, receive) = mpsc::channel();
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                case,
                agent: None,
                activated: Vec::new(),
                added: 0,
                deactivated: Vec::new(),
                current: OLD.into(),
                started: false,
                secret_observed: false,
            }),
            gate: Gate::default(),
            started,
            reads: AtomicUsize::new(0),
        });
        zbus::block_on(async {
            conn.object_server()
                .at(ROOT, Manager(shared.clone()))
                .await
                .unwrap();
            conn.object_server()
                .at(DEV, Device(shared.clone()))
                .await
                .unwrap();
            conn.object_server()
                .at(DEV, Wireless(case.restricted_bssid))
                .await
                .unwrap();
            conn.object_server()
                .at(ACCESS_POINT, Point(case, false))
                .await
                .unwrap();
            if case.restricted_bssid {
                conn.object_server()
                    .at(WRONG_ACCESS_POINT, Point(case, true))
                    .await
                    .unwrap();
            }
            conn.object_server()
                .at(SAVED, Saved { case, old: false })
                .await
                .unwrap();
            conn.object_server()
                .at(OLD_PROFILE, Saved { case, old: true })
                .await
                .unwrap();
            conn.object_server()
                .at(
                    OLD,
                    Active {
                        shared: shared.clone(),
                        old: true,
                    },
                )
                .await
                .unwrap();
            conn.object_server()
                .at(
                    OURS,
                    Active {
                        shared: shared.clone(),
                        old: false,
                    },
                )
                .await
                .unwrap();
            conn.object_server()
                .at(
                    OTHER,
                    Active {
                        shared: shared.clone(),
                        old: false,
                    },
                )
                .await
                .unwrap();
            conn.object_server()
                .at(
                    "/org/freedesktop/NetworkManager/AgentManager",
                    Agents(shared.clone()),
                )
                .await
                .unwrap();
            conn.request_name(NM).await.unwrap();
        });
        Self {
            conn,
            shared,
            started: receive,
        }
    }
    fn wait_started(&self) {
        self.started
            .recv_timeout(WAIT)
            .expect("activation request deadline");
    }
    fn assert_cleanup(&self) {
        wait_until(|| !self.shared.state.lock().unwrap().deactivated.is_empty());
        let state = self.shared.state.lock().unwrap();
        assert_eq!(state.deactivated, [OURS]);
        assert_eq!(
            state.current,
            if state.case.concurrent { OTHER } else { OLD }
        );
    }
    fn assert_agent_closed(&self, bus: &PrivateBus) {
        if let Some(agent) = self.shared.state.lock().unwrap().agent.clone() {
            let observer = bus.connect();
            wait_until(|| {
                !zbus::block_on(observer.call_method(
                    Some("org.freedesktop.DBus"),
                    "/org/freedesktop/DBus",
                    Some("org.freedesktop.DBus"),
                    "NameHasOwner",
                    &(agent.as_str(),),
                ))
                .unwrap()
                .body()
                .deserialize::<bool>()
                .unwrap()
            });
        }
    }
}
fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !condition() {
        assert!(Instant::now() < deadline, "fixture deadline");
        thread::sleep(Duration::from_millis(10));
    }
}
fn run(
    bus: &PrivateBus,
    password: &str,
    cancel: Arc<AtomicBool>,
    timeout: Duration,
    call_timeout: Duration,
) -> thread::JoinHandle<io::Result<()>> {
    let address = bus.address.clone();
    let password = password.to_owned();
    thread::spawn(move || {
        connect_using(
            "fixture-wlan0",
            SSID,
            &password,
            &cancel,
            timeout,
            move || async move {
                zbus::connection::Builder::address(address.as_str())
                    .method_timeout(call_timeout)
                    .build()
                    .await
            },
        )
    })
}
fn cancel_case(case: Case, timeout: bool) {
    let bus = PrivateBus::start();
    let peer = Peer::start(&bus, case);
    let cancel = Arc::new(AtomicBool::new(false));
    let worker = run(
        &bus,
        "fixture-password",
        cancel.clone(),
        if timeout {
            Duration::from_millis(250)
        } else {
            WAIT
        },
        WAIT,
    );
    peer.wait_started();
    if !timeout {
        cancel.store(true, Ordering::Release);
    }
    let started = Instant::now();
    let error = worker.join().unwrap().unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(
        error.kind(),
        if timeout {
            io::ErrorKind::TimedOut
        } else {
            io::ErrorKind::Interrupted
        }
    );
    peer.assert_cleanup();
    peer.assert_agent_closed(&bus);
}

#[test]
#[ignore = "requires a private dbus-daemon; run in isolated Alpine acceptance"]
fn isolated_network_activation_lifecycle() {
    lifecycle();
}

fn lifecycle() {
    for (name, case, password) in [
        (
            "open new profile",
            Case {
                saved: false,
                encrypted: false,
                ..Case::default()
            },
            "",
        ),
        (
            "protected new profile and scoped SecretAgent",
            Case {
                saved: false,
                ask_secret: true,
                ..Case::default()
            },
            "fixture-password",
        ),
        (
            "saved profile reused without overwriting IPv4 or secrets",
            Case::default(),
            "",
        ),
        (
            "saved binary BSSID constraint skips a stronger mismatched access point",
            Case {
                restricted_bssid: true,
                ..Case::default()
            },
            "",
        ),
        (
            "already connected network left intact",
            Case {
                already_connected: true,
                ..Case::default()
            },
            "",
        ),
    ] {
        let bus = PrivateBus::start();
        let peer = Peer::start(&bus, case);
        run(&bus, password, Arc::new(AtomicBool::new(false)), WAIT, WAIT)
            .join()
            .unwrap()
            .unwrap();
        let state = peer.shared.state.lock().unwrap();
        assert!(state.deactivated.is_empty());
        assert_eq!(state.added, usize::from(!case.saved));
        assert_eq!(
            state.activated.len(),
            usize::from(case.saved && !case.already_connected)
        );
        assert_eq!(state.secret_observed, case.ask_secret);
        drop(state);
        peer.assert_agent_closed(&bus);
        println!("PASS NetworkManager protocol: {name}");
    }
    for (name, case, timeout) in [
        (
            "cancel preserves prior activation",
            Case {
                final_state: 1,
                ..Case::default()
            },
            false,
        ),
        (
            "cancel preserves concurrent same-profile activation",
            Case {
                final_state: 1,
                concurrent: true,
                ..Case::default()
            },
            false,
        ),
        (
            "timeout deactivates exact operation only",
            Case {
                final_state: 1,
                concurrent: true,
                ..Case::default()
            },
            true,
        ),
    ] {
        cancel_case(case, timeout);
        println!("PASS NetworkManager protocol: {name}");
    }

    {
        let bus = PrivateBus::start();
        let peer = Peer::start(
            &bus,
            Case {
                delayed: true,
                concurrent: true,
                final_state: 1,
                ..Case::default()
            },
        );
        let cancel = Arc::new(AtomicBool::new(false));
        let worker = run(&bus, "fixture-password", cancel.clone(), WAIT, WAIT);
        peer.wait_started();
        cancel.store(true, Ordering::Release);
        let started = Instant::now();
        let error = worker.join().unwrap().unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(error.to_string().contains("not yet confirmed"));
        let agent = peer.shared.state.lock().unwrap().agent.clone().unwrap();
        let secret = zbus::block_on(peer.conn.call_method(
            Some(agent.as_str()),
            AGENT_PATH,
            Some("org.freedesktop.NetworkManager.SecretAgent"),
            "GetSecrets",
            &(
                profile(Case::default(), false),
                object(SAVED),
                SECURITY,
                Vec::<String>::new(),
                5u32,
            ),
        ));
        assert!(
            matches!(secret, Err(zbus::Error::MethodError(name, _, _)) if name.as_str().ends_with("UserCanceled"))
        );
        peer.shared.gate.release();
        peer.assert_cleanup();
        peer.assert_agent_closed(&bus);
        println!("PASS NetworkManager protocol: delayed reply is cleaned after prompt cancellation and password revocation");
    }
    {
        let bus = PrivateBus::start();
        let peer = Peer::start(
            &bus,
            Case {
                delayed: true,
                final_state: 1,
                ..Case::default()
            },
        );
        let cancel = Arc::new(AtomicBool::new(false));
        let worker = run(
            &bus,
            "fixture-password",
            cancel.clone(),
            WAIT,
            Duration::from_millis(500),
        );
        peer.wait_started();
        cancel.store(true, Ordering::Release);
        assert!(worker
            .join()
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("not yet confirmed"));
        peer.assert_agent_closed(&bus);
        peer.shared.gate.release();
        assert!(peer.shared.state.lock().unwrap().deactivated.is_empty());
        println!("PASS NetworkManager protocol: missing activation reply stays unconfirmed and cleanup lifetime is bounded");
    }
    {
        let bus = PrivateBus::start();
        let peer = Peer::start(
            &bus,
            Case {
                final_state: 1,
                ..Case::default()
            },
        );
        let cancel = Arc::new(AtomicBool::new(false));
        let worker = run(&bus, "", cancel.clone(), WAIT, WAIT);
        peer.wait_started();
        wait_until(|| peer.shared.reads.load(Ordering::SeqCst) > 0);
        zbus::block_on(peer.conn.clone().close()).unwrap();
        let replacement = Peer::start(
            &bus,
            Case {
                concurrent: true,
                final_state: 1,
                ..Case::default()
            },
        );
        cancel.store(true, Ordering::Release);
        assert!(worker.join().unwrap().is_err());
        assert!(replacement
            .shared
            .state
            .lock()
            .unwrap()
            .deactivated
            .is_empty());
        peer.assert_agent_closed(&bus);
        println!("PASS NetworkManager protocol: daemon replacement cannot receive cancellation for a reused object path");
    }
    for (name, case) in [
        (
            "wrong credential state fails and cleans own activation",
            Case {
                ask_secret: true,
                final_state: 4,
                ..Case::default()
            },
        ),
        (
            "permission denied before activation",
            Case {
                denied: true,
                ..Case::default()
            },
        ),
        (
            "preexisting returned handle is never owned",
            Case {
                reused: true,
                ..Case::default()
            },
        ),
    ] {
        let bus = PrivateBus::start();
        let peer = Peer::start(&bus, case);
        let error = run(
            &bus,
            "fixture-password",
            Arc::new(AtomicBool::new(false)),
            WAIT,
            WAIT,
        )
        .join()
        .unwrap()
        .unwrap_err();
        if case.denied {
            assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        }
        if case.denied || case.reused {
            assert!(peer.shared.state.lock().unwrap().deactivated.is_empty());
        } else {
            peer.assert_cleanup();
        }
        peer.assert_agent_closed(&bus);
        println!("PASS NetworkManager protocol: {name}");
    }
    {
        let bus = PrivateBus::start();
        assert!(run(&bus, "", Arc::new(AtomicBool::new(false)), WAIT, WAIT)
            .join()
            .unwrap()
            .is_err());
        println!("PASS NetworkManager protocol: absent service returns an error");
    }
}
