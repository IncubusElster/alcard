//! Opt-in native integration in a disposable PAM container and private labwc.
//! No fixture credentials, unlock controls or screenshot hooks are compiled
//! into the production binary. Ordinary-window probes retain counts, not keys.
use crate::{
    gui::{self, TestProbe},
    protocol::Failure,
    ui::Language,
};
use alcard_ui::{shm::Buffers, theme::Theme};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_dispatch2, delegate_registry,
    dispatch2::Dispatch2,
    output::{OutputHandler, OutputState},
    reexports::{
        calloop::EventLoop,
        calloop_wayland_source::WaylandSource,
        protocols::wp::idle_inhibit::zv1::client::{
            zwp_idle_inhibit_manager_v1::{self as inhibit_manager, ZwpIdleInhibitManagerV1},
            zwp_idle_inhibitor_v1::{self as inhibitor, ZwpIdleInhibitorV1},
        },
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers},
        Capability, SeatHandler, SeatState,
    },
    shell::{
        xdg::{
            window::{Window, WindowConfigure, WindowDecorations, WindowHandler},
            XdgShell,
        },
        WaylandSurface,
    },
    shm::{Shm, ShmHandler},
};
use std::{
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        atomic::{
            AtomicBool, AtomicUsize,
            Ordering::{Acquire, Release},
        },
        mpsc, Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use wayland_client::{
    globals::registry_queue_init,
    protocol::{
        wl_callback::{self, WlCallback},
        wl_keyboard::WlKeyboard,
        wl_output::{self, WlOutput},
        wl_seat::WlSeat,
        wl_surface::WlSurface,
    },
    Connection, Proxy, QueueHandle,
};
use zeroize::{Zeroize, Zeroizing};

const STEP: Duration = Duration::from_secs(25);
const LOCKER: &str = "/opt/alcard/alcard-lock";
const SHELL: &str = "/opt/alcard/alcard-shell";

fn wait(description: &str, mut condition: impl FnMut() -> bool) {
    eprintln!("Checking: {description}");
    let end = Instant::now() + STEP;
    while !condition() {
        assert!(Instant::now() < end, "Timed out: {description}");
        thread::sleep(Duration::from_millis(20));
    }
}
fn wait_for_probe(description: &str, probe: &TestProbe, mut condition: impl FnMut() -> bool) {
    let mut previous = None;
    wait(description, || {
        // Public lifecycle only: no identity, prompt, response or key data.
        let snapshot = (
            probe.ready.load(Acquire),
            probe.locked_ack.load(Acquire),
            probe.focused.load(Acquire),
            probe.surfaces.load(Acquire),
            probe.commits.load(Acquire),
            probe.finished.load(Acquire),
            probe.closed.load(Acquire),
            *probe.last_failure.lock().unwrap(),
        );
        if previous != Some(snapshot) {
            eprintln!(
                "Lock bootstrap: ready={} locked={} focused={} surfaces={} commits={} finished={} closed={} failure={:?}",
                snapshot.0, snapshot.1, snapshot.2, snapshot.3, snapshot.4, snapshot.5, snapshot.6, snapshot.7
            );
            previous = Some(snapshot);
        }
        assert!(!snapshot.6, "Lock runtime exited before {description}");
        condition()
    });
}
fn settle() {
    thread::sleep(Duration::from_millis(180));
}
fn record(text: &str) {
    if let Some(directory) = std::env::var_os("ALCARD_LOCK_SCREENSHOT_DIR") {
        writeln!(
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(Path::new(&directory).join("manifest.txt"))
                .unwrap(),
            "Verified: {text}"
        )
        .unwrap();
    }
}
fn key(name: &str) {
    assert!(
        Command::new("timeout")
            .args(["5", "wtype", "-s", "80", "-k", name, "-s", "80"])
            .stdin(Stdio::null())
            .status()
            .unwrap()
            .success(),
        "native key injection failed"
    );
    settle();
}
fn quick_menu_down() {
    // Shell navigation intentionally uses physical evdev arrows. A plain
    // wtype -k Down receives dynamic code1 and is not the physical KEY_DOWN.
    // Reserve codes1..107 with release-only symbols, as the Shell fixtures do.
    let mut args = vec!["5".to_string(), "wtype".into(), "-s".into(), "80".into()];
    for code in 1..108 {
        args.extend(["-p".into(), format!("U{:04X}", 0xe000 + code)]);
    }
    args.extend(["-P", "Down", "-s", "30", "-p", "Down", "-s", "80"].map(String::from));
    let log =
        PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap()).join("quick-down-producer.log");
    // This producer has no keyboard receiver and sends only public arrow
    // navigation while Shell is unlocked; no credential tracing is enabled.
    assert!(Command::new("timeout")
        .args(args)
        .env("WAYLAND_DEBUG", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(File::create(&log).unwrap())
        .status()
        .unwrap()
        .success());
    assert!(fs::read_to_string(log)
        .unwrap()
        .lines()
        .any(|line| { line.contains("zwp_virtual_keyboard_v1") && line.contains(", 108, 1)") }));
    settle();
}
fn physical_digits() {
    // wtype allocates evdev codes from 1. Four release-only private symbols
    // reserve codes 1..4, then the public digits use physical codes 5..9.
    let mut args = vec!["5".to_string(), "wtype".into(), "-s".into(), "80".into()];
    for index in 1..5 {
        args.extend(["-p".into(), format!("U{:04X}", 0xe000 + index)]);
    }
    for digit in ["4", "5", "6", "7", "8"] {
        args.extend(["-P".into(), digit.into(), "-p".into(), digit.into()]);
    }
    args.extend(["-s".into(), "80".into()]);
    assert!(Command::new("timeout")
        .args(args)
        .stdin(Stdio::null())
        .status()
        .unwrap()
        .success());
    settle();
}
fn type_stdin(text: &str) {
    let mut input = Command::new("timeout")
        .args(["8", "wtype", "-s", "80", "-d", "12", "-", "-s", "80"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    input
        .stdin
        .take()
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
    assert!(input.wait().unwrap().success());
    settle();
}
fn fixture_secret() -> Zeroizing<String> {
    let secret = Zeroizing::new(
        fs::read_to_string(Path::new(&std::env::var_os("HOME").unwrap()).join(".lock-test-secret"))
            .unwrap(),
    );
    assert!(
        secret.starts_with("45678") && secret.len() > 20,
        "wrong disposable fixture format"
    );
    secret
}
fn correct_answer() {
    let secret = fixture_secret();
    physical_digits();
    type_stdin(&secret[5..]);
    key("Return");
}
fn home(binary: &Path) {
    assert!(Command::new("timeout")
        .arg("5")
        .arg(binary)
        .arg("--home")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .status()
        .unwrap()
        .success());
    settle();
}
fn presence() -> bool {
    zbus::block_on(async {
        let connection = zbus::Connection::session().await.unwrap();
        let proxy = zbus::Proxy::new(
            &connection,
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
        )
        .await
        .unwrap();
        proxy
            .call::<_, _, bool>("NameHasOwner", &("org.alcard.SessionLock",))
            .await
            .unwrap()
    })
}

struct ChildGuard(Child);
impl ChildGuard {
    fn spawn(mut command: Command) -> Self {
        Self(command.stdin(Stdio::null()).spawn().unwrap())
    }
    fn status(&mut self) -> Option<ExitStatus> {
        self.0.try_wait().unwrap()
    }
    fn finish(&mut self) -> ExitStatus {
        let mut result = None;
        wait("child process exit", || {
            result = self.status();
            result.is_some()
        });
        result.unwrap()
    }
    fn kill(&mut self) {
        if self.status().is_none() {
            self.0.kill().unwrap();
            let _ = self.0.wait();
        }
    }
}
impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.kill();
    }
}

struct LockerChild {
    process: ChildGuard,
    messages: mpsc::Receiver<String>,
    reader: Option<JoinHandle<()>>,
    received: Vec<String>,
    keyboard: ChildGuard,
}
impl LockerChild {
    fn start() -> Self {
        Self::start_on_bus(None)
    }
    fn start_on_bus(address: Option<&str>) -> Self {
        // labwc may retain the last ephemeral wtype device as current. Supply
        // an acknowledged live keymap before a new credential client binds.
        let keyboard = fresh_idle_keyboard();
        let mut command = Command::new(LOCKER);
        command
            .args(["--notify", "--locale", "en"])
            .stdout(Stdio::piped());
        if let Some(address) = address {
            command.env("DBUS_SESSION_BUS_ADDRESS", address);
        }
        let mut process = ChildGuard::spawn(command);
        let output = process.0.stdout.take().unwrap();
        let (send, messages) = mpsc::sync_channel(4);
        let reader = thread::spawn(move || {
            let mut input = BufReader::new(output);
            loop {
                let mut line = String::new();
                let result = input.by_ref().take(32).read_line(&mut line);
                if !matches!(result, Ok(1..)) {
                    break;
                }
                if send.try_send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            process,
            messages,
            reader: Some(reader),
            received: Vec::new(),
            keyboard,
        }
    }
    fn drain(&mut self) {
        while let Ok(line) = self.messages.try_recv() {
            assert!(
                matches!(line.as_str(), "LOCKED\n" | "UNLOCKED\n" | "FAILED\n"),
                "unexpected public locker receipt"
            );
            self.received.push(line);
            assert!(self.received.len() <= 2, "too many lifecycle receipts");
        }
    }
    fn locked(&mut self) {
        wait("production compositor LOCKED receipt", || {
            self.drain();
            assert!(
                self.process.status().is_none(),
                "locker exited before confirmation"
            );
            self.received == ["LOCKED\n"]
        });
        settle();
    }
    fn unlock(&mut self) {
        key("Return");
        // Production exposes no prompt probe. Empty screencopy is optional;
        // allow the real PAM challenge to commit before sending a response.
        thread::sleep(Duration::from_millis(500));
        correct_answer();
        assert!(self.process.finish().success());
        self.reader.take().unwrap().join().unwrap();
        self.drain();
        assert_eq!(self.received, ["LOCKED\n", "UNLOCKED\n"]);
    }
}
impl Drop for LockerChild {
    fn drop(&mut self) {
        self.process.kill();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        self.keyboard.kill();
    }
}

fn capture(probe: &TestProbe, name: &str) {
    let directory = PathBuf::from(
        std::env::var_os("ALCARD_LOCK_SCREENSHOT_DIR")
            .unwrap_or_else(|| std::env::var_os("XDG_RUNTIME_DIR").unwrap()),
    );
    let path = directory.join(name);
    let count = probe.captures.load(Acquire);
    *probe.capture.lock().unwrap() = Some(path.clone());
    wait("empty committed runtime SHM capture", || {
        assert!(
            probe.capture_error.lock().unwrap().is_none(),
            "empty capture failed"
        );
        probe.captures.load(Acquire) > count
    });
    png_size(&path);
    record(&format!("{name}: actual locked client render committed to opaque SHM; 320x170; not compositor screencopy; empty mask only"));
}
fn png_size(path: &Path) {
    let mut header = [0_u8; 24];
    File::open(path).unwrap().read_exact(&mut header).unwrap();
    assert_eq!(&header[..8], b"\x89PNG\r\n\x1a\n");
    assert_eq!(u32::from_be_bytes(header[16..20].try_into().unwrap()), 320);
    assert_eq!(u32::from_be_bytes(header[20..24].try_into().unwrap()), 170);
}
fn empty_screencopy() {
    let Some(directory) = std::env::var_os("ALCARD_LOCK_SCREENSHOT_DIR") else {
        return;
    };
    let path = Path::new(&directory).join("03-empty-compositor.png");
    let output = std::env::var("ALCARD_LOCK_TEST_OUTPUT").unwrap();
    let result = Command::new("timeout")
        .args(["3", "grim", "-o", &output, "-t", "png"])
        .arg(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    if result.success() {
        png_size(&path);
        record("03-empty-compositor.png: real compositor screencopy of empty lock, allowed by this labwc build");
    } else {
        let _ = fs::remove_file(path);
        record("Compositor screencopy of empty lock unavailable/denied; SHM captures are explicitly separate evidence");
    }
}
fn begin_prompt(probe: &TestProbe, name: &str) {
    let prompts = probe.prompts.load(Acquire);
    key("Return");
    wait("current-user PAM prompt", || {
        probe.prompts.load(Acquire) > prompts && probe.focused.load(Acquire)
    });
    capture(probe, name);
}

#[derive(Default)]
struct ObserverProbe {
    configured: AtomicBool,
    focused: AtomicBool,
    keys: AtomicUsize,
    inhibit_requested: AtomicBool,
    inhibit_applied: AtomicBool,
    stop: AtomicBool,
    error: Mutex<Option<String>>,
}
struct Observer {
    probe: Arc<ObserverProbe>,
    worker: Option<JoinHandle<()>>,
    keyboard: ChildGuard,
}
impl Observer {
    fn start() -> Self {
        // An observer is a new Wayland keyboard client too. Keep its baseline
        // keymap alive so blocked-input assertions cannot pass because the
        // compositor's previous ephemeral device vanished before binding.
        let keyboard = fresh_idle_keyboard();
        let probe = Arc::new(ObserverProbe::default());
        let state = probe.clone();
        let worker = thread::spawn(move || {
            if let Err(error) = observe(state.clone()) {
                *state.error.lock().unwrap() = Some(error.to_string());
            }
        });
        let result = Self {
            probe,
            worker: Some(worker),
            keyboard,
        };
        wait("ordinary Wayland application configured", || {
            result.healthy();
            result.probe.configured.load(Acquire)
        });
        result
    }
    fn healthy(&self) {
        assert!(
            !self.probe.stop.load(Acquire),
            "ordinary input observer was closed"
        );
        assert!(
            self.probe.error.lock().unwrap().is_none(),
            "ordinary input observer failed"
        );
    }
    fn receives(&self) {
        wait("ordinary application keyboard focus", || {
            self.healthy();
            self.probe.focused.load(Acquire)
        });
        let before = self.count();
        key("a");
        wait("ordinary application receives native input", || {
            self.count() > before
        });
    }
    fn count(&self) -> usize {
        self.healthy();
        self.probe.keys.load(Acquire)
    }
    fn inhibit(&self, active: bool) {
        self.probe.inhibit_requested.store(active, Release);
        wait(
            "compositor acknowledged ordinary application's idle inhibitor change",
            || {
                self.healthy();
                self.probe.inhibit_applied.load(Acquire) == active
            },
        );
    }
    fn blocked(&self, count: usize) {
        settle();
        self.healthy();
        assert!(
            !self.probe.focused.load(Acquire),
            "ordinary application got focus while locked"
        );
        assert_eq!(
            self.count(),
            count,
            "ordinary application received locked-session input"
        );
    }
}
impl Drop for Observer {
    fn drop(&mut self) {
        self.probe.stop.store(true, Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        self.keyboard.kill();
    }
}
struct Ordinary {
    registry: RegistryState,
    outputs: OutputState,
    seats: SeatState,
    shm: Shm,
    window: Window,
    keyboard: Option<WlKeyboard>,
    buffers: Buffers,
    image: tiny_skia::Pixmap,
    probe: Arc<ObserverProbe>,
}
fn observe(probe: Arc<ObserverProbe>) -> Result<(), Box<dyn std::error::Error>> {
    let connection = Connection::connect_to_env()?;
    let (globals, queue) = registry_queue_init::<Ordinary>(&connection)?;
    let qh = queue.handle();
    let mut event_loop = EventLoop::<Ordinary>::try_new()?;
    WaylandSource::new(connection.clone(), queue).insert(event_loop.handle())?;
    let inhibit_manager = globals
        .bind::<ZwpIdleInhibitManagerV1, _, _>(&qh, 1..=1, InhibitData)
        .ok();
    let mut inhibitor: Option<ZwpIdleInhibitorV1> = None;
    let compositor = CompositorState::bind(&globals, &qh)?;
    let xdg = XdgShell::bind(&globals, &qh)?;
    let shm = Shm::bind(&globals, &qh)?;
    let window = xdg.create_window(
        compositor.create_surface(&qh),
        WindowDecorations::RequestServer,
        &qh,
    );
    window.set_app_id("alcard-lock-input-observer");
    window.set_title("Disposable input-count observer");
    window.commit();
    let buffers = Buffers::new(&shm)?;
    let mut image = tiny_skia::Pixmap::new(320, 170).unwrap();
    image.fill(tiny_skia::Color::from_rgba8(28, 40, 60, 255));
    let mut state = Ordinary {
        registry: RegistryState::new(&globals),
        outputs: OutputState::new(&globals, &qh),
        seats: SeatState::new(&globals, &qh),
        shm,
        window,
        keyboard: None,
        buffers,
        image,
        probe,
    };
    while !state.probe.stop.load(Acquire) {
        let active = state.probe.inhibit_requested.load(Acquire);
        if active != inhibitor.is_some() {
            if active {
                inhibitor = Some(
                    inhibit_manager
                        .as_ref()
                        .ok_or("compositor has no zwp_idle_inhibit_manager_v1")?
                        .create_inhibitor(state.window.wl_surface(), &qh, InhibitData),
                );
            } else if let Some(inhibitor) = inhibitor.take() {
                inhibitor.destroy();
            }
            // Done is ordered after create/destroy on this live connection.
            // The event-loop keeps this bounded; no blocking roundtrip/join.
            connection.display().sync(&qh, InhibitSync { active });
        }
        event_loop.dispatch(Some(Duration::from_millis(50)), &mut state)?;
    }
    if let Some(inhibitor) = inhibitor {
        inhibitor.destroy();
    }
    if let Some(manager) = inhibit_manager {
        manager.destroy();
    }
    connection.flush()?;
    Ok(())
}

struct InhibitData;
impl Dispatch2<ZwpIdleInhibitManagerV1, Ordinary> for InhibitData {
    fn event(
        &self,
        _: &mut Ordinary,
        _: &ZwpIdleInhibitManagerV1,
        _: inhibit_manager::Event,
        _: &Connection,
        _: &QueueHandle<Ordinary>,
    ) {
    }
}
impl Dispatch2<ZwpIdleInhibitorV1, Ordinary> for InhibitData {
    fn event(
        &self,
        _: &mut Ordinary,
        _: &ZwpIdleInhibitorV1,
        _: inhibitor::Event,
        _: &Connection,
        _: &QueueHandle<Ordinary>,
    ) {
    }
}
struct InhibitSync {
    active: bool,
}
impl Dispatch2<WlCallback, Ordinary> for InhibitSync {
    fn event(
        &self,
        state: &mut Ordinary,
        _: &WlCallback,
        event: wl_callback::Event,
        _: &Connection,
        _: &QueueHandle<Ordinary>,
    ) {
        if matches!(event, wl_callback::Event::Done { .. }) {
            state.probe.inhibit_applied.store(self.active, Release);
        }
    }
}
impl WindowHandler for Ordinary {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Window) {
        self.probe.stop.store(true, Release);
    }
    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &Window,
        configure: WindowConfigure,
        _: u32,
    ) {
        let size = (
            configure.new_size.0.map_or(320, |v| v.get()),
            configure.new_size.1.map_or(170, |v| v.get()),
        );
        let surface = self.window.wl_surface();
        match self.buffers.attach(&self.image, surface, size, 1) {
            Ok(true) => {
                surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
                surface.commit();
                self.probe.configured.store(true, Release);
            }
            Ok(false) => {}
            Err(error) => {
                *self.probe.error.lock().unwrap() = Some(error.to_string());
            }
        }
    }
}
impl CompositorHandler for Ordinary {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: i32,
    ) {
    }
    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: wl_output::Transform,
    ) {
    }
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: u32) {}
    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: &WlOutput,
    ) {
    }
    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: &WlOutput,
    ) {
    }
}
impl OutputHandler for Ordinary {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}
}
impl SeatHandler for Ordinary {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seats
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat) {}
    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            self.keyboard = Some(self.seats.get_keyboard::<Self>(qh, &seat, None).unwrap());
        }
    }
    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard {
            if let Some(keyboard) = self.keyboard.take() {
                if keyboard.version() >= 3 {
                    keyboard.release();
                }
            }
            self.probe.focused.store(false, Release);
        }
    }
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat) {}
}
impl KeyboardHandler for Ordinary {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        surface: &WlSurface,
        _: u32,
        _: &[u32],
        _: &[Keysym],
    ) {
        self.probe
            .focused
            .store(surface == self.window.wl_surface(), Release);
    }
    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: &WlSurface,
        _: u32,
    ) {
        self.probe.focused.store(false, Release);
    }
    fn press_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: u32,
        mut event: KeyEvent,
    ) {
        self.probe.keys.fetch_add(1, Release);
        if let Some(text) = &mut event.utf8 {
            text.zeroize();
        }
    }
    fn repeat_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: u32,
        mut event: KeyEvent,
    ) {
        if let Some(text) = &mut event.utf8 {
            text.zeroize();
        }
    }
    fn release_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: u32,
        mut event: KeyEvent,
    ) {
        if let Some(text) = &mut event.utf8 {
            text.zeroize();
        }
    }
    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: u32,
        _: Modifiers,
        _: RawModifiers,
        _: u32,
    ) {
    }
}
impl ShmHandler for Ordinary {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}
impl ProvidesRegistryState for Ordinary {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState, SeatState];
}
delegate_dispatch2!(Ordinary);
delegate_registry!(Ordinary);

#[test]
#[ignore = "requires disposable PAM users and private labwc; scripts/test-lock-native.sh"]
fn real_session_lock_pam_and_fail_closed() {
    assert_eq!(
        std::env::var("ALCARD_LOCK_TEST_CONTAINER").as_deref(),
        Ok("1")
    );
    assert!(Path::new("/.dockerenv").is_file());
    assert!(!nix::unistd::Uid::current().is_root());
    assert!(std::env::var_os("WAYLAND_DEBUG").is_none());
    assert_eq!(
        std::env::var("ALCARD_LOCK_TEST_HELPER").as_deref(),
        Ok(LOCKER)
    );
    let shell_log = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap()).join("shell.log");
    let mut shell = start_shell(Path::new(SHELL), &shell_log);
    let observer = Observer::start();
    observer.receives();
    let before = observer.count();
    let runtime_keyboard = fresh_idle_keyboard();
    let probe = Arc::new(TestProbe::default());
    let state = probe.clone();
    let driver = thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            wait_for_probe(
                "confirmed compositor lock, two configured outputs and focused ready PAM",
                &state,
                || {
                    state.ready.load(Acquire)
                        && state.locked_ack.load(Acquire)
                        && state.surfaces.load(Acquire) >= 2
                        && state.commits.load(Acquire) >= 2
                        && state.focused.load(Acquire)
                },
            );
            wait("real test locker presentation presence", presence);
            capture(&state, "01-lock-idle-client.png");
            observer.blocked(before);
            begin_prompt(&state, "02-password-empty-client.png");
            empty_screencopy();
            let cancelled = state.cancelled.load(Acquire);
            physical_digits();
            assert_eq!(
                state.cancelled.load(Acquire),
                cancelled,
                "password digit acted as Cancel"
            );
            home(Path::new(SHELL));
            assert!(
                state.focused.load(Acquire),
                "Home IPC displaced the locked credential surface"
            );
            observer.blocked(before);
            key("Escape");
            wait("Escape cancels authentication only", || {
                state.cancelled.load(Acquire) == cancelled + 1
            });
            assert!(!state.unlocked.load(Acquire));
            capture(&state, "04-cancelled-still-locked-client.png");
            let failures = state.failures.load(Acquire);
            begin_prompt(&state, "05-before-wrong-answer-client.png");
            let other_secret = Zeroizing::new(
                fs::read_to_string(
                    Path::new(&std::env::var_os("HOME").unwrap()).join(".lock-other-secret"),
                )
                .unwrap(),
            );
            type_stdin(&other_secret);
            key("Return");
            wait("real PAM rejects wrong password", || {
                state.failures.load(Acquire) > failures
            });
            assert!(!state.unlocked.load(Acquire));
            assert_eq!(*state.last_failure.lock().unwrap(), Some(Failure::Denied));
            observer.blocked(before);
            capture(&state, "06-wrong-password-still-locked-client.png");
            begin_prompt(&state, "07-retry-password-empty-client.png");
            correct_answer();
            wait("authenticated unlock acknowledged by Wayland sync", || {
                state.unlocked.load(Acquire)
            });
            observer.receives();
            record("current UID real PAM: Enter, physical password digits4–8, Escape wipes/cancels without unlocking, another real user's valid password was rejected for this UID, correct password + Wayland sync restored ordinary input");
            record("two real lock surfaces; ordinary window got no input; production Shell --home could not displace password focus");
        }));
        if result.is_err() {
            state.stop.store(true, Release);
        }
        result
    });
    let gui = gui::run_test(Language::Ru, Theme::classic(), probe.clone());
    probe.closed.store(true, Release);
    if let Err(error) = &gui {
        eprintln!("Lock runtime exited with error: {error}");
    }
    let driven = driver.join().unwrap();
    if let Err(panic) = driven {
        std::panic::resume_unwind(panic);
    }
    gui.expect("real graphical lock runtime");
    drop(runtime_keyboard);
    wait("test locker presence released after unlock", || !presence());

    // The existing staging shim observes only canonical public receipts from
    // the unchanged real locker. It also lets us prove that the replacement
    // came from Shell's fixed-sibling recovery path, not an explicit test launch.
    shell.kill();
    let runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap());
    let staged = runtime.join("staged/alcard-shell");
    let mut shell = start_shell(&staged, &runtime.join("recovery-shell.log"));

    let observer = Observer::start();
    observer.receives();
    let before = observer.count();
    let mut original = LockerChild::start();
    original.locked();
    observer.blocked(before);
    let mut duplicate = LockerChild::start();
    assert!(!duplicate.process.finish().success());
    duplicate.reader.take().unwrap().join().unwrap();
    duplicate.drain();
    assert!(
        duplicate.received == ["FAILED\n"],
        "duplicate locker claimed ownership"
    );
    assert!(original.process.status().is_none());
    key("Escape");
    observer.blocked(before);
    record("second real production locker refused without LOCKED/UNLOCKED; original lock remained active");
    let original_pid = original.process.0.id();
    shell.kill();
    assert!(original.process.status().is_none());
    let mut shell = start_shell(&staged, &runtime.join("restarted-while-locked.log"));
    home(&staged);
    assert!(original.process.status().is_none());
    observer.blocked(before);
    assert!(
        !runtime.join("shell-lock.receipts").exists(),
        "Shell restarted a healthy locker"
    );
    record("Shell SIGKILL and restart adopted the existing compositor-scoped lease; original real locker PID survived and Home could not displace it");
    // Hold only our disposable Shell process while testing the abandoned
    // interval. Otherwise its automatic replacement can bind precisely while
    // this fixture is replacing ephemeral virtual keyboards. A real physical
    // keyboard does not disappear between these probe commands.
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(shell.process.0.id() as i32),
        nix::sys::signal::Signal::SIGSTOP,
    )
    .unwrap();
    original.process.kill();
    original.reader.take().unwrap().join().unwrap();
    original.drain();
    assert_eq!(original.received, ["LOCKED\n"]);
    key("a");
    observer.blocked(before);
    let while_abandoned = Observer::start();
    key("a");
    while_abandoned.blocked(0);
    observer.blocked(before);
    record("with the exact test Shell temporarily stopped, SIGKILL after LOCKED did not restore ordinary input; a newly mapped ordinary window also remained isolated during the abandoned interval");
    // Retain this acknowledged keymap through the automatic client's bind.
    // No wtype device is replaced again until its LOCKED receipt is observed.
    let recovery_keyboard = fresh_idle_keyboard();
    let mut request = Command::new(SHELL);
    request
        .arg("--home")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut pending_home = ChildGuard::spawn(request);
    thread::sleep(Duration::from_millis(100));
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(shell.process.0.id() as i32),
        nix::sys::signal::Signal::SIGCONT,
    )
    .unwrap();
    assert!(pending_home.finish().success());
    while_abandoned.blocked(0);
    let receipts = runtime.join("shell-lock.receipts");
    wait(
        "Shell automatically recovered abandoned lock with native LOCKED receipt",
        || {
            assert!(shell.status().is_none());
            fs::read_to_string(&receipts).is_ok_and(|text| text == "LOCKED\n")
        },
    );
    // Match the normal LockerChild::locked readiness allowance after the
    // receipt, before injecting any new virtual-keyboard input.
    settle();
    let replacement_pid: u32 = fs::read_to_string(runtime.join("shell-lock.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_ne!(replacement_pid, original_pid);
    assert!(fs::read(format!("/proc/{replacement_pid}/cmdline"))
        .unwrap()
        .starts_with(format!("{LOCKER}\0--notify\0--locale\0auto\0").as_bytes()));
    key("Escape");
    while_abandoned.blocked(0);
    key("Return");
    thread::sleep(Duration::from_millis(500));
    correct_answer();
    let completed = std::panic::catch_unwind(|| {
        wait(
            "automatic replacement completed only after current-user PAM",
            || fs::read_to_string(&receipts).is_ok_and(|text| text == "LOCKED\nUNLOCKED\n"),
        );
    });
    if let Err(panic) = completed {
        // This Shell has no WAYLAND_DEBUG. Its stderr contains public service
        // errors only; no password, keyboard event or PAM response is logged.
        eprintln!(
            "Recovery public receipts: {:?}",
            fs::read_to_string(&receipts)
        );
        if let Ok(log) = fs::read_to_string(runtime.join("restarted-while-locked.log")) {
            eprintln!("Recovery Shell service log:\n{log}");
        }
        std::panic::resume_unwind(panic);
    }
    while_abandoned.receives();
    wait(
        "production presence released after authenticated recovery",
        || !presence(),
    );
    record("Shell automatically launched a new production locker after SIGKILL; labwc accepted the abandoned lock, Escape stayed locked, and only current-user PAM restored input with ordered LOCKED/UNLOCKED receipts");
    shell.kill();
    drop(recovery_keyboard);
    fs::remove_file(receipts).unwrap();
    fs::remove_file(runtime.join("shell-lock.pid")).unwrap();
    shell_quick_launch();
    standalone_lock_with_other_notification_daemon();
    session_bus_loss_and_recovery();
}

// Each Shell needs a fresh current seat keymap. Short-lived wtype devices can
// replace labwc's current keyboard even while an older idle device survives.
// Keep the new producer alive until this specific Shell has finished.
struct ShellChild {
    process: ChildGuard,
    keyboard: ChildGuard,
}
impl ShellChild {
    fn status(&mut self) -> Option<ExitStatus> {
        assert!(
            self.keyboard.status().is_none(),
            "Shell idle keyboard exited early"
        );
        self.process.status()
    }
    fn kill(&mut self) {
        self.process.kill();
        self.keyboard.kill();
    }
}
impl Drop for ShellChild {
    fn drop(&mut self) {
        self.kill();
    }
}
fn fresh_idle_keyboard() -> ChildGuard {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let index = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let log = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap())
        .join(format!("client-idle-keyboard-{index}.log"));
    let mut command = Command::new("wtype");
    // This idle virtual-keyboard producer has null stdin, no key commands and
    // no wl_keyboard receiver. Its log contains the keymap/sync handshake;
    // neither the Shell nor the locker inherits WAYLAND_DEBUG from this child.
    command
        .args(["-s", "300000"])
        .env("WAYLAND_DEBUG", "1")
        .stdout(Stdio::null())
        .stderr(File::create(&log).unwrap());
    let mut child = ChildGuard::spawn(command);
    wait(
        "fresh idle keyboard keymap acknowledged before client startup",
        || {
            assert!(
                child.status().is_none(),
                "idle keyboard exited before keymap acknowledgement"
            );
            let text = fs::read_to_string(&log).unwrap();
            let mut keymap = false;
            for line in text.lines() {
                if keymap && line.contains("wl_callback") && line.contains(".done(") {
                    return true;
                }
                if line.contains("zwp_virtual_keyboard_v1") && line.contains(".keymap(") {
                    keymap = true;
                }
            }
            false
        },
    );
    child
}

fn start_shell(binary: &Path, log: &Path) -> ShellChild {
    start_shell_logging(binary, log, false)
}
fn start_shell_logging(binary: &Path, log: &Path, protocol_log: bool) -> ShellChild {
    start_shell_on_bus(binary, log, protocol_log, None)
}
fn start_shell_on_bus(
    binary: &Path,
    log: &Path,
    protocol_log: bool,
    address: Option<&str>,
) -> ShellChild {
    let keyboard = fresh_idle_keyboard();
    let mut command = Command::new(binary);
    command
        .args(["--locale", "en"])
        .stdout(Stdio::null())
        .stderr(File::create(log).unwrap());
    if protocol_log {
        command.env("WAYLAND_DEBUG", "1");
    }
    if let Some(address) = address {
        command.env("DBUS_SESSION_BUS_ADDRESS", address);
    }
    let mut child = ShellChild {
        process: ChildGuard::spawn(command),
        keyboard,
    };
    wait("fresh production Shell endpoint", || {
        assert!(child.status().is_none());
        Command::new("timeout")
            .arg("2")
            .arg(binary)
            .arg("--home")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
    });
    settle();
    child
}
fn quick_lock(binary: &Path) -> ChildGuard {
    home(binary);
    key("space");
    for _ in 0..3 {
        quick_menu_down();
    }
    // This final key launches a new credential client asynchronously. Retain
    // its current virtual device until that client exits, including slow PAM
    // startup; replacing it after launch would miss the same keymap race.
    let mut command = Command::new("wtype");
    command.args(["-s", "80", "-k", "space", "-s", "300000"]);
    let keyboard = ChildGuard::spawn(command);
    settle();
    keyboard
}
fn shell_quick_launch() {
    let runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap());
    let missing = runtime.join("missing-helper/alcard-shell");
    let log = runtime.join("missing-helper.log");
    let mut shell = start_shell(&missing, &log);
    let launch_keyboard = quick_lock(&missing);
    wait("missing fixed sibling reports a launch failure", || {
        fs::read_to_string(&log)
            .unwrap()
            .contains("Could not start the session locker")
    });
    assert!(shell.status().is_none());
    assert!(!presence());
    record("production Shell Quick Lock with missing fixed sibling reported launch failure and remained usable; no lock presence or success was claimed");
    shell.kill();
    drop(launch_keyboard);
    let staged = runtime.join("staged/alcard-shell");
    let mut shell = start_shell(&staged, &runtime.join("staged-shell.log"));
    let launch_keyboard = quick_lock(&staged);
    let receipts = runtime.join("shell-lock.receipts");
    wait("Quick Lock real production LOCKED receipt", || {
        fs::read_to_string(&receipts).is_ok_and(|text| text == "LOCKED\n")
    });
    let pid = fs::read_to_string(runtime.join("shell-lock.pid"))
        .unwrap()
        .trim()
        .parse::<i32>()
        .unwrap();
    assert!(pid > 1);
    let command = fs::read(format!("/proc/{pid}/cmdline")).unwrap();
    assert!(
        command.starts_with(format!("{LOCKER}\0--notify\0--locale\0en\0").as_bytes()),
        "staging did not execute the expected production helper"
    );
    wait("production lock presence", presence);
    let observer = Observer::start();
    home(&staged);
    key("a");
    observer.blocked(0);
    assert!(shell.status().is_none());
    // Losing the Shell parent must not turn into an unlock or terminate PAM.
    shell.kill();
    assert!(nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok());
    key("Escape");
    observer.blocked(0);
    key("Return");
    thread::sleep(Duration::from_millis(500));
    correct_answer();
    wait(
        "Quick-launched helper authenticated after Shell exit",
        || fs::read_to_string(&receipts).is_ok_and(|text| text == "LOCKED\nUNLOCKED\n"),
    );
    wait(
        "Quick-launched helper released presentation presence",
        || !presence(),
    );
    drop(launch_keyboard);
    observer.receives();
    record("unchanged production Shell Home5 -> Quick row4 -> Select launched the real sibling locker; wrapper observed only canonical public lifecycle, real helper PID verified; Home and Shell exit did not unlock; valid current-user PAM restored normal input");
}

fn native_guard() {
    assert_eq!(
        std::env::var("ALCARD_LOCK_TEST_CONTAINER").as_deref(),
        Ok("1")
    );
    assert!(Path::new("/.dockerenv").is_file());
    assert!(!nix::unistd::Uid::current().is_root());
    assert!(std::env::var_os("WAYLAND_DEBUG").is_none());
    assert_eq!(
        std::env::var("ALCARD_LOCK_TEST_HELPER").as_deref(),
        Ok(LOCKER)
    );
}

struct IdleTestPreferences {
    path: PathBuf,
    previous: Option<Vec<u8>>,
}
impl IdleTestPreferences {
    fn install(runtime: &Path) -> Self {
        let config = PathBuf::from(std::env::var_os("XDG_CONFIG_HOME").unwrap());
        assert!(config.is_absolute() && config.starts_with(runtime));
        let path = config.join("alcard/shell.toml");
        let previous = match fs::read(&path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => panic!("Cannot preserve private idle fixture config: {error}"),
        };
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "locale = 'en'\n[idle]\nlock_after_seconds = 5\nscreen_off_after_seconds = 10\n",
        )
        .unwrap();
        Self { path, previous }
    }
}
impl Drop for IdleTestPreferences {
    fn drop(&mut self) {
        let restored = if let Some(bytes) = &self.previous {
            fs::write(&self.path, bytes)
        } else {
            fs::remove_file(&self.path)
        };
        assert!(
            restored.is_ok(),
            "Cannot restore private idle fixture config"
        );
    }
}

fn idle_trace(path: &Path) -> Vec<String> {
    let lines: Vec<String> = fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    assert!(
        !lines.iter().any(|line| {
            line == "ALCARD_IDLE_LIFECYCLE FAILED"
                || (line.contains("zwlr_output_power_v1") && line.contains(".failed"))
        }),
        "Native idle acceptance requires a successful locker and real output-power support"
    );
    lines
}

fn idle_power_event(line: &str, operation: &str) -> Option<(u32, bool)> {
    let suffix = line
        .split_once("zwlr_output_power_v1@")
        .or_else(|| line.split_once("zwlr_output_power_v1#"))?
        .1;
    let (id, message) = suffix.split_once('.')?;
    let (method, argument) = message.split_once('(')?;
    if method.trim_end_matches([',', ' ']) != operation {
        return None;
    }
    let expected_direction = if operation == "set_mode" {
        " -> "
    } else {
        " <- "
    };
    assert!(line.contains(expected_direction));
    let on = match argument.strip_suffix(')')?.trim() {
        "0" => false,
        "1" => true,
        _ => panic!("Unexpected public output-power mode"),
    };
    Some((id.parse().unwrap(), on))
}

fn idle_power_ids(
    lines: &[String],
    operation: &str,
    on: bool,
    since: usize,
) -> std::collections::BTreeSet<u32> {
    lines
        .iter()
        .skip(since)
        .filter_map(|line| idle_power_event(line, operation))
        .filter_map(|(id, mode)| (mode == on).then_some(id))
        .collect()
}

fn idled_notification_ids(lines: &[String], since: usize) -> std::collections::BTreeSet<u32> {
    lines
        .iter()
        .skip(since)
        .filter_map(|line| {
            let suffix = line
                .split_once("ext_idle_notification_v1@")
                .or_else(|| line.split_once("ext_idle_notification_v1#"))?
                .1;
            let (id, event) = suffix.split_once('.')?;
            let (event, _) = event.split_once('(')?;
            (event.trim_end_matches([',', ' ']) == "idled" && line.contains(" <- "))
                .then(|| id.parse().unwrap())
        })
        .collect()
}

fn idle_off_requires_locked_receipt(lines: &[String]) {
    let mut confirmed = false;
    for line in lines {
        match line.as_str() {
            "ALCARD_IDLE_LIFECYCLE LOCKED" => confirmed = true,
            "ALCARD_IDLE_LIFECYCLE UNLOCKED" | "ALCARD_IDLE_LIFECYCLE FAILED" => confirmed = false,
            _ => {}
        }
        if matches!(idle_power_event(line, "set_mode"), Some((_, false))) {
            assert!(
                confirmed,
                "Output Off was requested before the real production LOCKED receipt"
            );
        }
    }
}

fn idle_receipts(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn idle_locker_lifetime(runtime: &Path) -> (i32, u64) {
    let pid = fs::read_to_string(runtime.join("shell-lock.pid"))
        .unwrap()
        .trim()
        .parse::<i32>()
        .unwrap();
    assert!(pid > 1);
    // The trusted launcher records its own lifetime immediately before exec
    // of the unchanged root-owned LOCKER. Call only after canonical LOCKED:
    // the production process is now nondumpable, so /proc/PID/exe is denied.
    let receipt = fs::read_to_string(runtime.join("shell-lock.identity.txt")).unwrap();
    let fields: Vec<_> = receipt.split_whitespace().collect();
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[0].parse::<i32>().unwrap(), pid);
    let start = fields[1].parse::<u64>().unwrap();
    let same_lifetime = || {
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
        let fields: Vec<_> = stat
            .rsplit_once(") ")
            .unwrap()
            .1
            .split_whitespace()
            .collect();
        assert!(!matches!(fields[0], "Z" | "X" | "x"));
        assert_eq!(fields[19].parse::<u64>().unwrap(), start);
    };
    same_lifetime();
    // App's normal idle-lock path passes the resolved Shell locale; both the
    // private preferences and start_shell_on_bus explicitly select English.
    assert_eq!(
        fs::read(format!("/proc/{pid}/cmdline")).unwrap(),
        format!("{LOCKER}\0--notify\0--locale\0en\0").as_bytes()
    );
    same_lifetime();
    assert!(nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok());
    (pid, start)
}

#[test]
#[ignore = "opt-in disposable PAM/two-output labwc: ALCARD_IDLE_TEST=1 scripts/test-lock-native.sh"]
fn real_idle_lock_and_output_power() {
    native_guard();
    assert_eq!(std::env::var("ALCARD_IDLE_TEST").as_deref(), Ok("1"));
    let runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap());
    assert!(runtime.is_absolute());
    // Declared before children so unwinding stops our processes before restoring
    // only this disposable session's previous preferences.
    let _preferences = IdleTestPreferences::install(&runtime);
    let staged = runtime.join("staged/alcard-shell");
    assert!(runtime.join("staged/alcard-shell-real").is_file());
    assert!(runtime.join("idle-protocol-filter.py").is_file());
    let receipts = runtime.join("shell-lock.receipts");
    assert!(
        !receipts.exists(),
        "Idle case must start in its own session"
    );
    let log = runtime.join("idle-shell-protocol.log");
    let mut shell = start_shell_logging(&staged, &log, true);
    let observer = Observer::start();
    observer.inhibit(true);
    observer.receives();
    let ordinary_keys = observer.count();
    wait(
        "live idle subscriptions and both initially-On outputs",
        || {
            let trace = idle_trace(&log);
            trace
                .iter()
                .filter(|line| line.contains(".get_idle_notification("))
                .count()
                >= 2
                && idle_power_ids(&trace, "mode", true, 0).len() == 2
        },
    );
    let inhibited_boundary = idle_trace(&log).len();
    // No input is generated in this interval, which exceeds both configured
    // timers. The inhibitor belongs to a real visible ordinary client.
    let inhibited_until = Instant::now() + Duration::from_secs(12);
    while Instant::now() < inhibited_until {
        assert!(shell.status().is_none());
        observer.healthy();
        assert!(observer.probe.focused.load(Acquire));
        assert_eq!(observer.count(), ordinary_keys);
        assert!(idle_receipts(&receipts).is_empty());
        let trace = idle_trace(&log);
        assert!(idled_notification_ids(&trace, inhibited_boundary).is_empty());
        assert!(idle_power_ids(&trace, "set_mode", false, 0).is_empty());
        thread::sleep(Duration::from_millis(20));
    }
    observer.inhibit(false);
    record("live visible ordinary-client idle inhibitor, compositor sync acknowledged: no input for twelve seconds exceeded both five/ten-second timers with no Idled, helper receipt or Off; destroy was separately sync acknowledged before normal idle acceptance");
    wait(
        "automatic real LOCKED while an ordinary application had focus",
        || {
            assert!(shell.status().is_none());
            idle_off_requires_locked_receipt(&idle_trace(&log));
            idle_receipts(&receipts) == ["LOCKED"]
        },
    );
    let first_locker = idle_locker_lifetime(&runtime);
    observer.blocked(ordinary_keys);
    wait("both outputs acknowledge real idle power Off", || {
        assert!(shell.status().is_none());
        let trace = idle_trace(&log);
        idle_off_requires_locked_receipt(&trace);
        idle_power_ids(&trace, "mode", false, 0).len() >= 2
    });
    let trace = idle_trace(&log);
    let outputs = idle_power_ids(&trace, "mode", false, 0);
    assert_eq!(outputs.len(), 2);
    assert_eq!(idle_power_ids(&trace, "set_mode", false, 0), outputs);
    assert!(presence());
    record("idle with ordinary application focused: real production LOCKED precedes every Off request in one ordered public stream; two output-power Mode(Off) acknowledgements observed");

    let wake_boundary = trace.len();
    key("Shift_L");
    wait("input wakes both outputs without unlocking", || {
        idle_power_ids(&idle_trace(&log), "mode", true, wake_boundary) == outputs
    });
    assert_eq!(idle_receipts(&receipts), ["LOCKED"]);
    assert!(presence());
    observer.blocked(ordinary_keys);
    empty_screencopy(); // No credential or text has been entered.
    record("public modifier input produced Mode(On) for both outputs, retained LOCKED/presentation presence and delivered no input to the ordinary application");

    // Rearm must be proved in the original Shell, which owns the actual
    // helper's canonical receipt pipe. A restarted Shell only adopts advisory
    // presence/lease state and must not invent a new LOCKED/UNLOCKED proof.
    let subscriptions = idle_trace(&log)
        .iter()
        .filter(|line| line.contains(".get_idle_notification("))
        .count();
    assert!(subscriptions >= 2);
    key("Return");
    thread::sleep(Duration::from_millis(500));
    correct_answer();
    wait("current-user PAM unlock in the owning Shell", || {
        idle_receipts(&receipts) == ["LOCKED", "UNLOCKED"] && !presence()
    });
    wait(
        "authenticated unlock creates fresh idle notifications",
        || {
            idle_trace(&log)
                .iter()
                .filter(|line| line.contains(".get_idle_notification("))
                .count()
                >= subscriptions + 2
        },
    );
    let fresh_boundary = idle_trace(&log).len();
    let ordinary_keys_after_unlock = observer.count();
    let grace = Instant::now() + Duration::from_secs(4);
    while Instant::now() < grace {
        assert!(shell.status().is_none());
        assert_eq!(idle_receipts(&receipts), ["LOCKED", "UNLOCKED"]);
        assert!(idle_power_ids(&idle_trace(&log), "set_mode", false, fresh_boundary).is_empty());
        thread::sleep(Duration::from_millis(20));
    }
    wait(
        "fresh configured interval can automatically lock again",
        || {
            assert!(shell.status().is_none());
            idle_receipts(&receipts) == ["LOCKED", "UNLOCKED", "LOCKED"]
        },
    );
    let locker = idle_locker_lifetime(&runtime);
    assert_ne!(locker, first_locker);
    observer.blocked(ordinary_keys_after_unlock);
    wait("fresh lock precedes a second two-output Off cycle", || {
        let trace = idle_trace(&log);
        idle_off_requires_locked_receipt(&trace);
        idle_power_ids(&trace, "mode", false, fresh_boundary) == outputs
    });
    record("own current-user PAM unlock rearmed notifications; no immediate relock/off during a four-second guard inside the configured five-second interval, then a second real automatic LOCKED preceded both output Off acknowledgements");

    shell.kill(); // SIGKILL must not run orderly output/locker cleanup.
    assert_eq!(idle_locker_lifetime(&runtime), locker);
    let restart_log = runtime.join("idle-restarted-protocol.log");
    let mut restarted = start_shell_logging(&staged, &restart_log, true);
    wait("restarted Shell restores initially-Off outputs", || {
        assert!(restarted.status().is_none());
        let trace = idle_trace(&restart_log);
        let initial_off = idle_power_ids(&trace, "mode", false, 0);
        initial_off.len() == 2
            && idle_power_ids(&trace, "set_mode", true, 0) == initial_off
            && idle_power_ids(&trace, "mode", true, 0) == initial_off
    });
    assert_eq!(idle_receipts(&receipts), ["LOCKED", "UNLOCKED", "LOCKED"]);
    assert_eq!(idle_locker_lifetime(&runtime), locker);
    assert!(presence());
    observer.blocked(ordinary_keys_after_unlock);
    assert!(idle_power_ids(&idle_trace(&restart_log), "set_mode", false, 0).is_empty());
    record("SIGKILL/restart while both outputs were Off: initial Mode(Off) followed by requested and acknowledged On; same healthy real locker PID survived, with no replacement or unlock; advisory adoption issued no Off");

    key("Return");
    thread::sleep(Duration::from_millis(500));
    correct_answer();
    wait("second real PAM unlock completes native idle case", || {
        idle_receipts(&receipts) == ["LOCKED", "UNLOCKED", "LOCKED", "UNLOCKED"] && !presence()
    });
    restarted.kill();
    record("surviving real locker accepted current-user PAM after Shell restart; no canonical rearm claim is made for the adopting Shell");
    drop(observer);
    independent_lock_does_not_authorize_idle_power(&runtime, &staged);
}

fn independent_lock_does_not_authorize_idle_power(runtime: &Path, staged: &Path) {
    let receipts = runtime.join("shell-lock.receipts");
    let previous_receipts = idle_receipts(&receipts);
    let previous_lifetime = fs::read(runtime.join("shell-lock.identity.txt")).unwrap();
    let log = runtime.join("idle-foreign-protocol.log");
    // This is an ordinary new Shell started while the session is unlocked.
    // It has no previous owned locker, pipe, request or adopted live lease.
    let mut shell = start_shell_logging(staged, &log, true);
    let observer = Observer::start();
    observer.inhibit(true);
    observer.receives();
    let before = observer.count();
    wait(
        "fresh Shell owns two healthy initially-On power objects",
        || idle_power_ids(&idle_trace(&log), "mode", true, 0).len() == 2,
    );
    let mut foreign = LockerChild::start();
    // This direct child's canonical pipe belongs only to the test, never to
    // Shell's sibling launcher. Shell sees its public presence/journal only.
    foreign.locked();
    wait("independent real locker presentation presence", presence);
    observer.blocked(before);
    let boundary = idle_trace(&log).len();
    observer.inhibit(false);
    let mut check = || {
        assert!(shell.status().is_none());
        assert!(foreign.process.status().is_none());
        foreign.drain();
        assert_eq!(foreign.received, ["LOCKED\n"]);
        assert!(presence());
        assert_eq!(idle_receipts(&receipts), previous_receipts);
        assert_eq!(
            fs::read(runtime.join("shell-lock.identity.txt")).unwrap(),
            previous_lifetime
        );
        assert!(idle_power_ids(&idle_trace(&log), "set_mode", false, 0).is_empty());
    };
    wait(
        "both real idle timers fire with an independent lock but no owned receipt",
        || {
            check();
            idled_notification_ids(&idle_trace(&log), boundary).len() >= 2
        },
    );
    // Keep the forbidden condition in force across multiple Shell dispatches,
    // not just the instant the last idle event entered the public trace.
    let guard = Instant::now() + Duration::from_secs(1);
    while Instant::now() < guard {
        check();
        thread::sleep(Duration::from_millis(20));
    }
    key("Shift_L");
    observer.blocked(before);
    record("fresh unlocked Shell followed by an independently launched real locker: both compositor idle timers fired while only the test owned canonical LOCKED; no Shell helper launch or Off request, original child remained live and ordinary input stayed blocked");
    foreign.unlock();
    wait("independent locker released its public presence", || {
        !presence()
    });
    assert_eq!(idle_receipts(&receipts), previous_receipts);
    shell.kill();
    observer.receives();
    record("independent locker released the session only after current-user PAM, with its own ordered LOCKED/UNLOCKED pipe; Shell never gained a canonical unlock receipt");
}

fn worker_pid() -> i32 {
    for task in fs::read_dir("/proc/self/task").unwrap().flatten() {
        if let Ok(children) = fs::read_to_string(task.path().join("children")) {
            for child in children.split_whitespace() {
                let Ok(pid) = child.parse::<i32>() else {
                    continue;
                };
                if pid > 1
                    && fs::read(format!("/proc/{pid}/cmdline"))
                        .is_ok_and(|args| args == format!("{LOCKER}\0--pam-worker\0").as_bytes())
                {
                    return pid;
                }
            }
        }
    }
    panic!("No owned production PAM worker was found");
}
fn negative_pam_case(expired: bool) {
    native_guard();
    let observer = Observer::start();
    observer.receives();
    let before = observer.count();
    let _runtime_keyboard = fresh_idle_keyboard();
    let probe = Arc::new(TestProbe::default());
    let state = probe.clone();
    let driver = thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            wait_for_probe("negative-case real lock ready", &state, || {
                state.ready.load(Acquire)
                    && state.locked_ack.load(Acquire)
                    && state.focused.load(Acquire)
            });
            begin_prompt(
                &state,
                if expired {
                    "10-expired-account-empty-client.png"
                } else {
                    "08-helper-loss-empty-client.png"
                },
            );
            if expired {
                correct_answer();
            } else {
                physical_digits();
                let pid = worker_pid();
                nix::sys::signal::kill(
                    nix::unistd::Pid::from_raw(pid),
                    nix::sys::signal::Signal::SIGKILL,
                )
                .unwrap();
            }
            let expected = if expired {
                Failure::AccountDenied
            } else {
                Failure::Unavailable
            };
            wait("real PAM negative result", || {
                *state.last_failure.lock().unwrap() == Some(expected)
            });
            assert!(!state.unlocked.load(Acquire));
            observer.blocked(before);
            capture(
                &state,
                if expired {
                    "11-expired-account-still-locked-client.png"
                } else {
                    "09-helper-loss-still-locked-client.png"
                },
            );
            key("Escape");
            key("a");
            observer.blocked(before);
            assert!(!state.unlocked.load(Acquire));
            record(if expired {
                "valid current-user password passed authentication but expired account was rejected by real pam_acct_mgmt; AccountDenied observed and ordinary input remained blocked"
            } else {
                "SIGKILL of the exact owned production PAM worker during a challenge produced Unavailable and did not unlock or pass keys to an ordinary application"
            });
        }));
        state.stop.store(true, Release);
        result
    });
    let result = gui::run_test(Language::En, Theme::classic(), probe.clone());
    probe.closed.store(true, Release);
    if let Err(error) = &result {
        eprintln!("Negative-case lock runtime exited: {error}");
    }
    let driven = driver.join().unwrap();
    if let Err(panic) = driven {
        std::panic::resume_unwind(panic);
    }
    assert!(
        result.is_err(),
        "negative-case teardown must disconnect without unlocking"
    );
    assert!(!probe.unlocked.load(Acquire));
    let after = Observer::start();
    key("a");
    after.blocked(0);
}
#[test]
#[ignore = "requires disposable PAM users and private labwc; scripts/test-lock-native.sh"]
fn real_lock_pam_worker_loss_stays_locked() {
    negative_pam_case(false);
}
#[test]
#[ignore = "requires expired disposable account and private labwc; scripts/test-lock-native.sh"]
fn real_lock_denies_expired_account() {
    negative_pam_case(true);
}

fn activations(log: &Path) -> usize {
    fs::read_to_string(log)
        .unwrap()
        .lines()
        .filter(|line| {
            line.contains("zwlr_foreign_toplevel_handle_v1") && line.contains(".activate")
        })
        .count()
}
fn standalone_lock_with_other_notification_daemon() {
    let holder = zbus::block_on(zbus::Connection::session()).unwrap();
    zbus::block_on(holder.request_name_with_flags(
        "org.freedesktop.Notifications",
        zbus::fdo::RequestNameFlags::DO_NOT_QUEUE,
    ))
    .unwrap();
    let log = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap())
        .join("standalone-shell-protocol.log");
    // This specific Shell logs only public activation traffic. It is stopped
    // before any credential is typed; the real standalone locker never enables
    // WAYLAND_DEBUG and never inherits it from this unrelated process.
    let mut shell = start_shell_logging(Path::new(SHELL), &log, true);
    home(Path::new(SHELL));
    wait("observable unlocked Home activation request", || {
        activations(&log) > 0
    });
    let observer = Observer::start();
    observer.receives();
    let before = observer.count();
    let mut locker = LockerChild::start();
    locker.locked();
    wait("standalone lock presentation presence", presence);
    settle();
    let requests = activations(&log);
    home(Path::new(SHELL));
    settle();
    assert_eq!(
        activations(&log),
        requests,
        "Shell sent Home activation despite standalone lock and another notification daemon"
    );
    observer.blocked(before);
    shell.kill();
    locker.unlock();
    observer.receives();
    record("another fixture client owned org.freedesktop.Notifications before Shell startup; standalone real lock independently suppressed Shell Home activation (native request count unchanged); protocol-logged Shell stopped before all credential input");
}

// A separate, ordinary-user bus lets this test stop/restart its own daemon
// without touching either the host or the outer native fixture's session bus.
struct PrivateBus {
    address: String,
    path: PathBuf,
    process: Option<ChildGuard>,
}
impl PrivateBus {
    fn new() -> Self {
        let path = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap())
            .join("lock-recovery-private-bus");
        let mut bus = Self {
            address: format!("unix:path={}", path.display()),
            path,
            process: None,
        };
        bus.start();
        bus
    }
    fn start(&mut self) {
        assert!(self.process.is_none());
        // This is exclusively our fixture pathname after its exact owned
        // daemon has been reaped. No production runtime file is removed.
        if self.path.exists() {
            fs::remove_file(&self.path).unwrap();
        }
        let mut command = Command::new("dbus-daemon");
        command
            .args(["--session", "--nofork", "--nopidfile"])
            .arg(format!("--address={}", self.address))
            .stdout(Stdio::null());
        self.process = Some(ChildGuard::spawn(command));
        wait("private session bus ready", || {
            assert!(self.process.as_mut().unwrap().status().is_none());
            private_name_present(&self.address, "org.freedesktop.DBus")
        });
    }
    fn stop(&mut self) {
        if let Some(mut process) = self.process.take() {
            process.kill();
        }
    }
}
impl Drop for PrivateBus {
    fn drop(&mut self) {
        self.stop();
        let _ = fs::remove_file(&self.path);
    }
}

fn private_name_present(address: &str, name: &str) -> bool {
    let output = Command::new("timeout")
        .args([
            "2",
            "gdbus",
            "call",
            "--address",
            address,
            "--dest",
            "org.freedesktop.DBus",
            "--object-path",
            "/org/freedesktop/DBus",
            "--method",
            "org.freedesktop.DBus.NameHasOwner",
            name,
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .expect("run the bounded private-bus fixture observer");
    assert!(
        !matches!(output.status.code(), Some(125..=127)),
        "private-bus fixture could not execute gdbus through timeout: {}",
        output.status
    );
    output.status.success() && output.stdout == b"(true,)\n"
}

fn session_bus_loss_and_recovery() {
    native_guard();
    let runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap());
    let mut bus = PrivateBus::new();
    let mut shell = start_shell_on_bus(
        Path::new(SHELL),
        &runtime.join("private-bus-shell.log"),
        false,
        Some(&bus.address),
    );
    let observer = Observer::start();
    observer.receives();
    let before = observer.count();
    let mut locker = LockerChild::start_on_bus(Some(&bus.address));
    locker.locked();
    let original_pid = locker.process.0.id();
    wait("real locker owns name on private session bus", || {
        private_name_present(&bus.address, "org.alcard.SessionLock")
    });
    bus.stop();
    shell.kill();
    let mut shell = start_shell_on_bus(
        Path::new(SHELL),
        &runtime.join("no-bus-restarted-shell.log"),
        false,
        Some(&bus.address),
    );
    assert!(locker.process.status().is_none());
    assert_eq!(locker.process.0.id(), original_pid);
    // labwc raises each newly mapped toplevel even while the lock prevents it
    // from receiving focus. On unlock it focuses the topmost view, which would
    // otherwise be the restarted Shell rather than our earlier observer.
    // Map this ordinary window while still locked: it must receive no input
    // until the same real locker completes PAM and its native unlock sync.
    let after_restart = Observer::start();
    home(Path::new(SHELL));
    key("Escape");
    observer.blocked(before);
    after_restart.blocked(0);
    locker.unlock();
    after_restart.receives();
    assert!(shell.status().is_none());
    record("private session bus SIGKILL plus Shell restart preserved the exact live locker; both existing and newly mapped ordinary windows stayed blocked; Home/Escape did not unlock; real current-user PAM and ordered UNLOCKED restored input to the new ordinary window while the bus was still absent");

    bus.start();
    let observer = Observer::start();
    observer.receives();
    let before = observer.count();
    let mut locker = LockerChild::start_on_bus(Some(&bus.address));
    locker.locked();
    let original_pid = locker.process.0.id();
    wait("second real locker presence before daemon restart", || {
        private_name_present(&bus.address, "org.alcard.SessionLock")
    });
    bus.stop();
    observer.blocked(before);
    bus.start();
    wait(
        "same real locker reclaims presentation name on replacement bus",
        || {
            assert!(locker.process.status().is_none());
            private_name_present(&bus.address, "org.alcard.SessionLock")
        },
    );
    assert_eq!(locker.process.0.id(), original_pid);
    home(Path::new(SHELL));
    observer.blocked(before);
    locker.unlock();
    observer.receives();
    record("real session bus replacement restored advisory presence for the same live locker without restarting PAM or creating a second lock; ordinary input resumed only after current-user PAM");
    shell.kill();
}
