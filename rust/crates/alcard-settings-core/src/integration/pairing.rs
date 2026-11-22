//! Production pairing/Agent1 acceptance against a simulated BlueZ D-Bus peer.
//! The fixture never connects to a host bus and does not exercise Bluetooth RF.
use super::*;
use std::{
    io::{BufRead, BufReader},
    os::unix::fs::DirBuilderExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
    time::Instant,
};

const DEVICE: &str = "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF";
const ADDRESS: &str = "AA:BB:CC:DD:EE:FF";
const SERVICE: &str = "00001124-0000-1000-8000-00805f9b34fb";
const WAIT: Duration = Duration::from_secs(3);

struct PrivateBus {
    child: Child,
    directory: PathBuf,
    address: String,
}

impl PrivateBus {
    fn start() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "alcard-pairing-bus-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .expect("create private pairing bus directory");
        let child = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .arg(format!("--address=unix:path={}/bus", directory.display()))
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("pairing integration requires dbus-daemon");
        let mut bus = Self {
            child,
            directory,
            address: String::new(),
        };
        let output = bus.child.stdout.take().unwrap();
        let (sender, receiver) = mpsc::sync_channel(1);
        let reader = thread::spawn(move || {
            let mut address = String::new();
            let result = BufReader::new(output)
                .read_line(&mut address)
                .map(|_| address);
            let _ = sender.send(result);
        });
        bus.address = receiver
            .recv_timeout(WAIT)
            .expect("private D-Bus startup deadline")
            .expect("read private D-Bus address")
            .trim()
            .to_owned();
        reader.join().unwrap();
        assert!(
            bus.address.starts_with("unix:path="),
            "private D-Bus did not start"
        );
        bus
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

fn connect(address: &str) -> zbus::Connection {
    zbus::block_on(
        zbus::connection::Builder::address(address)
            .method_timeout(WAIT)
            .build(),
    )
    .expect("connect to private pairing bus")
}

#[derive(Clone, Debug)]
struct Registration {
    destination: String,
    path: OwnedObjectPath,
}

#[derive(Default)]
struct Calls {
    register: AtomicUsize,
    unregister: AtomicUsize,
    pair: AtomicUsize,
    cancel: AtomicUsize,
}

struct Manager {
    registered: mpsc::Sender<Registration>,
    registration_gate: Receiver<()>,
    calls: Arc<Calls>,
    current: Arc<Mutex<Option<Registration>>>,
    deny: bool,
}

#[interface(name = "org.bluez.AgentManager1")]
impl Manager {
    async fn register_agent(
        &self,
        agent: OwnedObjectPath,
        capability: &str,
        #[zbus(header)] header: Header<'_>,
    ) -> fdo::Result<()> {
        self.calls.register.fetch_add(1, Ordering::SeqCst);
        assert_eq!(agent.as_str(), AGENT);
        assert_eq!(capability, "KeyboardDisplay");
        if self.deny {
            return Err(fdo::Error::AccessDenied(
                "Simulated agent registration denial".into(),
            ));
        }
        let registration = Registration {
            destination: header.sender().unwrap().as_str().to_owned(),
            path: agent,
        };
        *self.current.lock().unwrap() = Some(registration.clone());
        self.registered.send(registration).unwrap();
        self.registration_gate
            .recv()
            .await
            .map_err(|_| fdo::Error::Failed("Simulated registration stopped".into()))?;
        Ok(())
    }

    fn unregister_agent(
        &self,
        agent: OwnedObjectPath,
        #[zbus(header)] header: Header<'_>,
    ) -> fdo::Result<()> {
        let registration = self.current.lock().unwrap().take().unwrap();
        assert_eq!(registration.path, agent);
        assert_eq!(header.sender().unwrap().as_str(), registration.destination);
        self.calls.unregister.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct Device {
    started: mpsc::Sender<()>,
    finish: Receiver<bool>,
    calls: Arc<Calls>,
}

#[interface(name = "org.bluez.Device1")]
impl Device {
    async fn pair(&self) -> fdo::Result<()> {
        self.calls.pair.fetch_add(1, Ordering::SeqCst);
        self.started.send(()).unwrap();
        if self.finish.recv().await.unwrap_or(false) {
            Ok(())
        } else {
            Err(fdo::Error::Failed("Simulated pairing rejection".into()))
        }
    }

    fn cancel_pairing(&self) {
        self.calls.cancel.fetch_add(1, Ordering::SeqCst);
        self.finish.close();
    }
}

struct Objects(Vec<&'static str>);

#[interface(name = "org.freedesktop.DBus.ObjectManager")]
impl Objects {
    fn get_managed_objects(
        &self,
    ) -> HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedVariant>>> {
        self.0
            .iter()
            .map(|path| {
                (
                    OwnedObjectPath::try_from(*path).unwrap(),
                    HashMap::from([(
                        "org.bluez.Device1".into(),
                        HashMap::from([(
                            "Address".into(),
                            OwnedVariant::from(zbus::Str::from(ADDRESS)),
                        )]),
                    )]),
                )
            })
            .collect()
    }
}

#[derive(Clone, Copy)]
enum Presence {
    Present,
    Missing,
    Duplicate,
    RemovedAfterDiscovery,
}

struct Peer {
    conn: zbus::Connection,
    registered: mpsc::Receiver<Registration>,
    started: mpsc::Receiver<()>,
    finish: Sender<bool>,
    registration_gate: Sender<()>,
    calls: Arc<Calls>,
    current: Arc<Mutex<Option<Registration>>>,
}

impl Peer {
    fn start(address: &str) -> Self {
        Self::configured(address, Presence::Present, false, false)
    }

    fn configured(
        address: &str,
        presence: Presence,
        deny_registration: bool,
        pause_registration: bool,
    ) -> Self {
        let (registered_tx, registered) = mpsc::channel();
        let (started_tx, started) = mpsc::channel();
        let (finish, outcome) = async_channel::bounded(1);
        let (registration_gate, gate) = async_channel::bounded(1);
        if !pause_registration {
            registration_gate.try_send(()).unwrap();
        }
        let calls = Arc::new(Calls::default());
        let current = Arc::new(Mutex::new(None));
        let paths = match presence {
            Presence::Missing => vec![],
            Presence::Duplicate => vec![DEVICE, "/org/bluez/hci1/dev_AA_BB_CC_DD_EE_FF"],
            _ => vec![DEVICE],
        };
        let mut builder = zbus::connection::Builder::address(address)
            .method_timeout(WAIT)
            .name("org.bluez")
            .serve_at("/", Objects(paths))
            .serve_at(
                "/org/bluez",
                Manager {
                    registered: registered_tx,
                    registration_gate: gate,
                    calls: calls.clone(),
                    current: current.clone(),
                    deny: deny_registration,
                },
            );
        if matches!(presence, Presence::Present | Presence::Duplicate) {
            builder = builder.serve_at(
                DEVICE,
                Device {
                    started: started_tx,
                    finish: outcome,
                    calls: calls.clone(),
                },
            );
        }
        let conn = zbus::block_on(builder.build()).expect("start simulated BlueZ peer");
        Self {
            conn,
            registered,
            started,
            finish,
            registration_gate,
            calls,
            current,
        }
    }

    fn registration(&self) -> Registration {
        let registration = self
            .registered
            .recv_timeout(WAIT)
            .expect("agent registration deadline");
        self.started.recv_timeout(WAIT).expect("Pair call deadline");
        registration
    }

    fn control(&self, registration: &Registration, method: &str) -> zbus::Result<()> {
        zbus::block_on(async {
            self.conn
                .call_method(
                    Some(registration.destination.as_str()),
                    registration.path.as_str(),
                    Some("org.bluez.Agent1"),
                    method,
                    &(),
                )
                .await?
                .body()
                .deserialize()
        })
    }

    fn counts(&self, register: usize, pair: usize, cancel: usize, unregister: usize) {
        assert_eq!(
            self.calls.register.load(Ordering::SeqCst),
            register,
            "RegisterAgent count"
        );
        assert_eq!(self.calls.pair.load(Ordering::SeqCst), pair, "Pair count");
        assert_eq!(
            self.calls.cancel.load(Ordering::SeqCst),
            cancel,
            "CancelPairing count"
        );
        assert_eq!(
            self.calls.unregister.load(Ordering::SeqCst),
            unregister,
            "UnregisterAgent count"
        );
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.finish.close();
        zbus::block_on(async {
            let _ = self.conn.release_name("org.bluez").await;
            let _ = self.conn.clone().close().await;
        });
    }
}

struct Run {
    session: Option<Session>,
    done: mpsc::Receiver<bool>,
    name: String,
}

impl Run {
    fn start(address: &str) -> Self {
        let conn = connect(address);
        let name = conn.unique_name().unwrap().as_str().to_owned();
        let (events, receiver) = mpsc::channel();
        let (cancel, stop) = async_channel::bounded(1);
        let (sender, done) = mpsc::channel();
        thread::spawn(move || {
            let result = zbus::block_on(pair_on_connection(conn, ADDRESS, events.clone(), stop))
                .map_err(io::Error::other);
            let success = result.is_ok();
            let _ = events.send(Event::Done(result));
            let _ = sender.send(success);
        });
        Self {
            session: Some(Session {
                events: receiver,
                cancel,
            }),
            done,
            name,
        }
    }

    fn event(&self) -> Event {
        self.session
            .as_ref()
            .unwrap()
            .events
            .recv_timeout(WAIT)
            .expect("pairing UI event deadline")
    }

    fn reply(&self, input: Option<&str>, check: impl FnOnce(Prompt)) {
        let reply = match self.event() {
            Event::Prompt(prompt, Some(reply)) => {
                check(prompt);
                reply
            }
            _ => panic!("expected interactive pairing prompt"),
        };
        if let Some(input) = input {
            reply.try_send(input.to_owned()).unwrap();
        }
    }

    fn closed_prompt(&self) {
        assert!(matches!(self.event(), Event::PromptClosed));
    }

    fn complete(&self, expected: bool) {
        loop {
            match self.event() {
                Event::PromptClosed => continue,
                Event::Done(result) => {
                    assert_eq!(result.is_ok(), expected);
                    break;
                }
                Event::Prompt(_, _) => panic!("unexpected prompt at pairing completion"),
            }
        }
        assert_eq!(self.done.recv_timeout(WAIT).unwrap(), expected);
    }

    fn drop_session(&mut self) {
        self.session.take();
        assert!(!self
            .done
            .recv_timeout(WAIT)
            .expect("session-drop cancellation deadline"));
    }

    fn assert_gone(&self, monitor: &zbus::Connection) {
        let deadline = Instant::now() + WAIT;
        loop {
            let has_owner: bool = zbus::block_on(async {
                monitor
                    .call_method(
                        Some("org.freedesktop.DBus"),
                        "/org/freedesktop/DBus",
                        Some("org.freedesktop.DBus"),
                        "NameHasOwner",
                        &(self.name.as_str(),),
                    )
                    .await
                    .unwrap()
                    .body()
                    .deserialize()
                    .unwrap()
            });
            if !has_owner {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "pairing agent connection was not cleaned up"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }
}

#[derive(Clone, Copy)]
enum Request {
    Device(&'static str, &'static str),
    Confirmation(u32),
    Service,
    DisplayPin,
    DisplayPasskey(u16),
}

struct PendingCall {
    receiver: mpsc::Receiver<zbus::Result<zbus::Message>>,
    worker: thread::JoinHandle<()>,
}

impl PendingCall {
    fn receive(self) -> zbus::Result<zbus::Message> {
        let result = self
            .receiver
            .recv_timeout(WAIT)
            .expect("Agent1 callback response deadline");
        self.worker.join().unwrap();
        result
    }
}

fn request(connection: &zbus::Connection, target: &Registration, request: Request) -> PendingCall {
    let conn = connection.clone();
    let target = target.clone();
    let (sender, receiver) = mpsc::channel();
    let worker = thread::spawn(move || {
        let result = zbus::block_on(async {
            let proxy = zbus::Proxy::new(
                &conn,
                target.destination.as_str(),
                target.path.as_str(),
                "org.bluez.Agent1",
            )
            .await?;
            let device = OwnedObjectPath::try_from(DEVICE).unwrap();
            match request {
                Request::Device(method, path) => {
                    proxy
                        .call_method(method, &(OwnedObjectPath::try_from(path).unwrap(),))
                        .await
                }
                Request::Confirmation(code) => {
                    proxy
                        .call_method("RequestConfirmation", &(device, code))
                        .await
                }
                Request::Service => {
                    proxy
                        .call_method("AuthorizeService", &(device, SERVICE))
                        .await
                }
                Request::DisplayPin => proxy.call_method("DisplayPinCode", &(device, "1234")).await,
                Request::DisplayPasskey(entered) => {
                    proxy
                        .call_method("DisplayPasskey", &(device, 42_u32, entered))
                        .await
                }
            }
        });
        let _ = sender.send(result);
    });
    PendingCall { receiver, worker }
}

fn error_name(result: zbus::Result<zbus::Message>, expected: &str) {
    match result {
        Err(zbus::Error::MethodError(name, _, _)) => assert_eq!(name.as_str(), expected),
        Err(zbus::Error::FDO(error)) => {
            assert_eq!(zbus::DBusError::name(&*error).as_str(), expected);
        }
        _ => panic!("expected a named Agent1 error"),
    }
}

fn successful_prompts(address: &str, monitor: &zbus::Connection) {
    let peer = Peer::start(address);
    let run = Run::start(address);
    let agent = peer.registration();
    let pin = request(
        &peer.conn,
        &agent,
        Request::Device("RequestPinCode", DEVICE),
    );
    run.reply(Some("1234"), |prompt| {
        assert!(matches!(prompt, Prompt::Pin))
    });
    assert_eq!(
        pin.receive()
            .unwrap()
            .body()
            .deserialize::<String>()
            .unwrap(),
        "1234"
    );
    let passkey = request(
        &peer.conn,
        &agent,
        Request::Device("RequestPasskey", DEVICE),
    );
    run.reply(Some("000042"), |prompt| {
        assert!(matches!(prompt, Prompt::Passkey))
    });
    assert_eq!(
        passkey
            .receive()
            .unwrap()
            .body()
            .deserialize::<u32>()
            .unwrap(),
        42
    );
    let confirm = request(&peer.conn, &agent, Request::Confirmation(42));
    run.reply(Some(""), |prompt| {
        assert!(matches!(prompt, Prompt::Confirm(code) if code == "000042"))
    });
    confirm
        .receive()
        .unwrap()
        .body()
        .deserialize::<()>()
        .unwrap();
    let authorize = request(
        &peer.conn,
        &agent,
        Request::Device("RequestAuthorization", DEVICE),
    );
    run.reply(Some(""), |prompt| {
        assert!(matches!(prompt, Prompt::Confirm(code) if code.is_empty()))
    });
    authorize.receive().unwrap();
    let service = request(&peer.conn, &agent, Request::Service);
    run.reply(Some(""), |prompt| {
        assert!(matches!(prompt, Prompt::Confirm(uuid) if uuid == SERVICE))
    });
    service.receive().unwrap();
    for (callback, expected) in [
        (Request::DisplayPin, "1234"),
        (Request::DisplayPasskey(0), "000042"),
        (Request::DisplayPasskey(5), "000042"),
    ] {
        request(&peer.conn, &agent, callback).receive().unwrap();
        assert!(
            matches!(run.event(), Event::Prompt(Prompt::Display(code), None) if code == expected)
        );
        peer.control(&agent, "Cancel").unwrap();
        run.closed_prompt();
        peer.counts(1, 1, 0, 0);
    }
    peer.finish.try_send(true).unwrap();
    run.complete(true);
    peer.counts(1, 1, 0, 1);
    run.assert_gone(monitor);
    println!("PAIRING CASE PASS: pin-passkey-confirm-authorization-display-and-cleanup");
}

fn rejected_prompts(address: &str, monitor: &zbus::Connection) {
    let peer = Peer::start(address);
    let run = Run::start(address);
    let agent = peer.registration();
    for (method, input) in [
        ("RequestPinCode", "bad\npin"),
        ("RequestPasskey", "1000000"),
    ] {
        let callback = request(&peer.conn, &agent, Request::Device(method, DEVICE));
        run.reply(Some(input), |_| {});
        error_name(callback.receive(), "org.bluez.Error.Rejected");
    }
    let confirm = request(&peer.conn, &agent, Request::Confirmation(42));
    run.reply(None, |prompt| assert!(matches!(prompt, Prompt::Confirm(_))));
    error_name(confirm.receive(), "org.bluez.Error.Rejected");
    peer.finish.try_send(false).unwrap();
    run.complete(false);
    peer.counts(1, 1, 0, 1);
    run.assert_gone(monitor);
    println!("PAIRING CASE PASS: invalid-values-ui-denial-and-cleanup");
}

fn caller_and_device_isolation(address: &str, monitor: &zbus::Connection) {
    let peer = Peer::start(address);
    let run = Run::start(address);
    let agent = peer.registration();
    let stranger = connect(address);
    error_name(
        request(&stranger, &agent, Request::Device("RequestPinCode", DEVICE)).receive(),
        "org.freedesktop.DBus.Error.AccessDenied",
    );
    error_name(
        request(
            &peer.conn,
            &agent,
            Request::Device("RequestPinCode", "/other"),
        )
        .receive(),
        "org.freedesktop.DBus.Error.AccessDenied",
    );
    for method in ["Cancel", "Release"] {
        let result = zbus::block_on(stranger.call_method(
            Some(agent.destination.as_str()),
            AGENT,
            Some("org.bluez.Agent1"),
            method,
            &(),
        ));
        error_name(result, "org.freedesktop.DBus.Error.AccessDenied");
    }
    assert!(matches!(
        run.session.as_ref().unwrap().events.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    peer.finish.try_send(true).unwrap();
    run.complete(true);
    peer.counts(1, 1, 0, 1);
    run.assert_gone(monitor);
    println!("PAIRING CASE PASS: owner-device-and-control-isolation");
}

fn pending_controls(address: &str, monitor: &zbus::Connection) {
    for control in ["Cancel", "Release", "SessionDrop", "ServiceDisappearance"] {
        let peer = Peer::start(address);
        let mut run = Run::start(address);
        let agent = peer.registration();
        let callback = request(
            &peer.conn,
            &agent,
            Request::Device("RequestPinCode", DEVICE),
        );
        let held_reply = match run.event() {
            Event::Prompt(Prompt::Pin, Some(reply)) => reply,
            _ => panic!("expected PIN prompt before lifecycle transition"),
        };
        match control {
            "Cancel" => {
                peer.control(&agent, "Cancel").unwrap();
                run.closed_prompt();
                error_name(callback.receive(), "org.bluez.Error.Canceled");
                peer.finish.try_send(false).unwrap();
                run.complete(false);
                peer.counts(1, 1, 0, 1);
            }
            "Release" => {
                // BlueZ has already unregistered this agent when it sends Release.
                peer.current.lock().unwrap().take();
                peer.control(&agent, "Release").unwrap();
                run.closed_prompt();
                error_name(callback.receive(), "org.bluez.Error.Canceled");
                run.complete(false);
                peer.counts(1, 1, 0, 0);
            }
            "SessionDrop" => {
                run.drop_session();
                error_name(callback.receive(), "org.bluez.Error.Canceled");
                peer.counts(1, 1, 1, 1);
            }
            "ServiceDisappearance" => {
                zbus::block_on(peer.conn.clone().close()).unwrap();
                run.complete(false);
                // The caller's socket is closed, so an Agent1 error reply cannot arrive.
                assert!(peer.conn.is_closed());
                assert!(callback.receive().is_err());
                peer.counts(1, 1, 0, 0);
            }
            _ => unreachable!(),
        }
        assert!(
            held_reply.is_closed(),
            "pending UI reply survived its session"
        );
        run.assert_gone(monitor);
        println!("PAIRING CASE PASS: {control}-pending-prompt-and-cleanup");
    }
}

fn discovery_and_registration_failures(address: &str, monitor: &zbus::Connection) {
    for (presence, deny, label) in [
        (
            Presence::Missing,
            false,
            "device-disappeared-before-discovery",
        ),
        (
            Presence::Duplicate,
            false,
            "ambiguous-device-on-two-adapters",
        ),
        (
            Presence::RemovedAfterDiscovery,
            false,
            "device-disappeared-after-discovery",
        ),
        (Presence::Present, true, "registration-denied"),
    ] {
        let peer = Peer::configured(address, presence, deny, false);
        let run = Run::start(address);
        run.complete(false);
        match presence {
            Presence::RemovedAfterDiscovery => peer.counts(1, 0, 0, 1),
            Presence::Present => peer.counts(1, 0, 0, 0),
            _ => peer.counts(0, 0, 0, 0),
        }
        run.assert_gone(monitor);
        println!("PAIRING CASE PASS: {label}-and-cleanup");
    }
}

fn cancelled_registration_never_starts_pair(address: &str, monitor: &zbus::Connection) {
    let peer = Peer::configured(address, Presence::Present, false, true);
    let mut run = Run::start(address);
    peer.registered
        .recv_timeout(WAIT)
        .expect("paused registration deadline");
    // Queue cancellation while RegisterAgent is in flight, then allow its reply.
    run.session.take();
    peer.registration_gate.try_send(()).unwrap();
    assert!(!run
        .done
        .recv_timeout(WAIT)
        .expect("cancelled registration cleanup deadline"));
    peer.counts(1, 0, 0, 1);
    run.assert_gone(monitor);
    println!("PAIRING CASE PASS: queued-cancel-during-registration-no-pair-and-cleanup");
}

fn lifecycle(address: &str) {
    let monitor = connect(address);
    let missing = Run::start(address);
    missing.complete(false);
    missing.assert_gone(&monitor);
    println!("PAIRING CASE PASS: bluez-service-absent");
    successful_prompts(address, &monitor);
    rejected_prompts(address, &monitor);
    caller_and_device_isolation(address, &monitor);
    pending_controls(address, &monitor);
    discovery_and_registration_failures(address, &monitor);
    cancelled_registration_never_starts_pair(address, &monitor);
}

#[test]
#[ignore = "requires dbus-daemon; simulated BlueZ protocol acceptance, no radio"]
fn isolated_bluez_pairing_lifecycle() {
    // The test thread owns the daemon, so even a worker deadline kills the bus.
    let bus = PrivateBus::start();
    let address = bus.address.clone();
    let (sender, receiver) = mpsc::sync_channel(1);
    let worker = thread::spawn(move || {
        let result = std::panic::catch_unwind(|| lifecycle(&address));
        let _ = sender.send(result);
    });
    let result = receiver
        .recv_timeout(Duration::from_secs(60))
        .expect("isolated Bluetooth pairing acceptance exceeded its deadline");
    worker.join().unwrap();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
