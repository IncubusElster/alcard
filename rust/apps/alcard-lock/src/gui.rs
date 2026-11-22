//! Real compositor-owned session locking. No layer-shell/fullscreen fallback.
//! Dropping a live lock never calls unlock: fatal paths fail closed.
use crate::{
    authentication::Worker,
    protocol::{Command, Event, Failure},
    render,
    ui::{Form, InputAnchor, Language, Phase},
};
use alcard_lock_state::{Lease, Session};
use alcard_ui::{
    shm::{self, Buffers},
    theme::Theme,
};
use chrono::{Local, Timelike};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, FrameCallbackData},
    delegate_dispatch2, delegate_registry,
    output::{OutputHandler, OutputState},
    reexports::{
        calloop::{channel, EventLoop},
        calloop_wayland_source::WaylandSource,
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers},
        Capability, SeatHandler, SeatState,
    },
    session_lock::{
        SessionLock, SessionLockHandler, SessionLockState, SessionLockSurface,
        SessionLockSurfaceConfigure,
    },
    shm::{Shm, ShmHandler},
};
use std::{
    collections::HashSet,
    error::Error,
    io,
    sync::Arc,
    time::{Duration, Instant},
};
use tiny_skia::Pixmap;
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
use zeroize::Zeroize;

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const START_TIMEOUT: Duration = Duration::from_secs(20);
const UNLOCK_SYNC_TIMEOUT: Duration = Duration::from_secs(5);

#[cfg(test)]
#[derive(Default)]
pub struct TestProbe {
    pub ready: std::sync::atomic::AtomicBool,
    pub locked_ack: std::sync::atomic::AtomicBool,
    pub focused: std::sync::atomic::AtomicBool,
    pub unlocked: std::sync::atomic::AtomicBool,
    pub finished: std::sync::atomic::AtomicBool,
    pub closed: std::sync::atomic::AtomicBool,
    /// Teardown disconnects fail-closed. It never sends an unlock request.
    pub stop: std::sync::atomic::AtomicBool,
    pub surfaces: std::sync::atomic::AtomicUsize,
    pub commits: std::sync::atomic::AtomicUsize,
    pub prompts: std::sync::atomic::AtomicUsize,
    pub failures: std::sync::atomic::AtomicUsize,
    pub last_failure: std::sync::Mutex<Option<Failure>>,
    pub cancelled: std::sync::atomic::AtomicUsize,
    pub attempt: std::sync::atomic::AtomicU64,
    /// Empty-mask, actual client render used for SHM. Not compositor screencopy.
    pub capture: std::sync::Mutex<Option<std::path::PathBuf>>,
    pub captures: std::sync::atomic::AtomicUsize,
    pub capture_error: std::sync::Mutex<Option<String>>,
}

struct Options {
    notify: bool,
    #[cfg(test)]
    probe: Option<std::sync::Arc<TestProbe>>,
}

struct Keyboard {
    seat: WlSeat,
    object: WlKeyboard,
    focus: Option<WlSurface>,
    held: HashSet<u32>,
    ctrl: bool,
    alt: bool,
    logo: bool,
}

struct LockOutput {
    output: WlOutput,
    surface: SessionLockSurface,
    buffers: Buffers,
    size: (u32, u32),
    scale: i32,
    configured: bool,
    frame_pending: bool,
    dirty: bool,
    shown: Option<InputAnchor>,
}

#[derive(Clone, Copy)]
struct UnlockSync {
    attempt: u64,
}

struct Runtime {
    connection: Connection,
    registry: RegistryState,
    seats: SeatState,
    outputs: OutputState,
    compositor: CompositorState,
    shm: Shm,
    lock_state: SessionLockState,
    lock: Option<SessionLock>,
    surfaces: Vec<LockOutput>,
    keyboards: Vec<Keyboard>,
    worker: Worker,
    lease: Arc<Lease>,
    ready_uid: Option<u32>,
    locked_ack: bool,
    unlock_attempt: Option<u64>,
    unlock_deadline: Option<Instant>,
    unlocked: bool,
    startup_deadline: Instant,
    form: Form,
    language: Language,
    theme: Theme,
    image: Pixmap,
    paint_dirty: bool,
    clock: String,
    date: String,
    error: Option<String>,
    notify: bool,
    #[cfg(test)]
    probe: Option<std::sync::Arc<TestProbe>>,
}

pub fn run(language: Language, theme: Theme, notify: bool) -> Result<()> {
    run_options(
        language,
        theme,
        Options {
            notify,
            #[cfg(test)]
            probe: None,
        },
    )
}

#[cfg(test)]
pub fn run_test(language: Language, theme: Theme, probe: std::sync::Arc<TestProbe>) -> Result<()> {
    // The integration entry retains the same ordinary-user memory hardening.
    crate::harden()?;
    let result = run_options(
        language,
        theme,
        Options {
            notify: false,
            probe: Some(probe.clone()),
        },
    );
    probe
        .closed
        .store(true, std::sync::atomic::Ordering::Release);
    result
}

fn run_options(language: Language, theme: Theme, options: Options) -> Result<()> {
    if std::env::var_os("WAYLAND_DEBUG").is_some() {
        return Err("Refusing credential input with WAYLAND_DEBUG enabled".into());
    }
    let connection = Connection::connect_to_env()?;
    let session = Session::from_wayland_socket(&connection)?;
    // A journal observer holds its probe lock only briefly. Allow that read to
    // finish without turning a legitimate initial lock into a spurious refusal.
    let mut lease = None;
    for _ in 0..4 {
        lease = session.try_lease()?;
        if lease.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let lease = Arc::new(lease.ok_or("Another locker already owns this compositor")?);
    let _presence = crate::Presence::start();
    // Keep this outer lease alive until EVERY Wayland object/connection in the
    // inner runtime has been dropped. A crash still releases it in the kernel.
    run_connection(connection, language, theme, options, lease.clone())
}

fn run_connection(
    connection: Connection,
    language: Language,
    theme: Theme,
    options: Options,
    lease: Arc<Lease>,
) -> Result<()> {
    let (globals, queue) = registry_queue_init::<Runtime>(&connection)?;
    if !globals.contents().with_list(|globals| {
        globals
            .iter()
            .any(|global| global.interface == "ext_session_lock_manager_v1" && global.version >= 1)
    }) {
        return Err("Compositor does not support secure session locking".into());
    }
    let qh = queue.handle();
    let mut event_loop: EventLoop<Runtime> = EventLoop::try_new()?;
    WaylandSource::new(connection.clone(), queue).insert(event_loop.handle())?;
    let compositor = CompositorState::bind(&globals, &qh)?;
    let shm = Shm::bind(&globals, &qh)?;
    let (updates, incoming) = channel::channel::<Event>();
    let callbacks = qh.clone();
    event_loop
        .handle()
        .insert_source(incoming, move |event, _, state| match event {
            channel::Event::Msg(event) => state.receive(event, &callbacks),
            channel::Event::Closed => state.backend_failed(Failure::Unavailable),
        })?;
    let worker = Worker::start(updates)?;
    let now = Local::now();
    let mut state = Runtime {
        connection: connection.clone(),
        registry: RegistryState::new(&globals),
        seats: SeatState::new(&globals, &qh),
        outputs: OutputState::new(&globals, &qh),
        compositor,
        shm,
        lock_state: SessionLockState::new(&globals, &qh),
        lock: None,
        surfaces: Vec::new(),
        keyboards: Vec::new(),
        worker,
        lease,
        ready_uid: None,
        locked_ack: false,
        unlock_attempt: None,
        unlock_deadline: None,
        unlocked: false,
        startup_deadline: Instant::now() + START_TIMEOUT,
        form: Form::default(),
        language,
        theme,
        image: Pixmap::new(render::WIDTH, render::HEIGHT)
            .ok_or("Cannot allocate lock presentation")?,
        paint_dirty: true,
        clock: now.format("%H:%M").to_string(),
        date: now.format("%Y-%m-%d").to_string(),
        error: None,
        notify: options.notify,
        #[cfg(test)]
        probe: options.probe,
    };
    loop {
        #[cfg(test)]
        if state
            .probe
            .as_ref()
            .is_some_and(|probe| probe.stop.load(std::sync::atomic::Ordering::Acquire))
        {
            // Intentionally no SessionLock::unlock on teardown or fatal paths.
            return Err("Test teardown disconnected the locked client".into());
        }
        if let Some(error) = state.error.take() {
            return Err(io::Error::other(error).into());
        }
        if state.unlocked {
            crate::report_unlocked(state.notify);
            return Ok(());
        }
        if !state.locked_ack && Instant::now() >= state.startup_deadline {
            return Err("No confirmed compositor lock before the startup deadline".into());
        }
        if state
            .unlock_deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err("Compositor did not acknowledge the authenticated unlock".into());
        }
        state.update_clock();
        state.draw(&qh)?;
        #[cfg(test)]
        state.capture_empty_render();
        let mut wait = Duration::from_secs(u64::from(60 - Local::now().second()));
        if !state.locked_ack {
            wait = wait.min(
                state
                    .startup_deadline
                    .saturating_duration_since(Instant::now()),
            );
        }
        if let Some(deadline) = state.unlock_deadline {
            wait = wait.min(deadline.saturating_duration_since(Instant::now()));
        }
        #[cfg(test)]
        if state.probe.is_some() {
            wait = wait.min(Duration::from_millis(50));
        }
        event_loop.dispatch(Some(wait), &mut state)?;
    }
}

impl Runtime {
    fn dirty(&mut self) {
        self.paint_dirty = true;
        for output in &mut self.surfaces {
            output.dirty = true;
        }
    }

    fn update_clock(&mut self) {
        let now = Local::now();
        let clock = now.format("%H:%M").to_string();
        let date = now.format("%Y-%m-%d").to_string();
        if self.clock != clock || self.date != date {
            self.clock = clock;
            self.date = date;
            self.dirty();
        }
    }

    fn add_output(&mut self, output: WlOutput, qh: &QueueHandle<Self>) -> Result<()> {
        if self.surfaces.iter().any(|current| current.output == output)
            || self.unlock_attempt.is_some()
        {
            return Ok(());
        }
        let Some(lock) = self.lock.as_ref() else {
            return Ok(());
        };
        let wl_surface = self.compositor.create_surface(qh);
        // No buffer/commit is allowed until this surface's first configure.
        let surface = lock.create_lock_surface(wl_surface, &output, qh);
        let scale = self
            .outputs
            .info(&output)
            .map_or(1, |info| info.scale_factor.max(1));
        self.surfaces.push(LockOutput {
            output,
            surface,
            buffers: Buffers::new(&self.shm)?,
            size: (render::WIDTH, render::HEIGHT),
            scale,
            configured: false,
            frame_pending: false,
            dirty: true,
            shown: None,
        });
        #[cfg(test)]
        if let Some(probe) = &self.probe {
            probe
                .surfaces
                .store(self.surfaces.len(), std::sync::atomic::Ordering::Release);
        }
        Ok(())
    }

    fn receive(&mut self, event: Event, qh: &QueueHandle<Self>) {
        match event {
            Event::Ready { uid, user } => {
                if self.ready_uid.is_some()
                    || uid != nix::unistd::Uid::current().as_raw()
                    || user.is_empty()
                {
                    self.backend_failed(Failure::IdentityChanged);
                    return;
                }
                self.ready_uid = Some(uid);
                self.form.ready(&user);
                #[cfg(test)]
                if let Some(probe) = &self.probe {
                    probe
                        .ready
                        .store(true, std::sync::atomic::Ordering::Release);
                }
                if let Err(error) = self.lease.mark_requested() {
                    self.error = Some(format!("Could not retain lock recovery intent: {error}"));
                    return;
                }
                // From this point an abrupt failure is ambiguous. The durable
                // intent precedes the actual request; no FAILED receipt may
                // assert that a subsequent error was safely before locking.
                crate::mark_lock_requested();
                match self.lock_state.lock(qh) {
                    Ok(lock) => {
                        self.lock = Some(lock);
                    }
                    Err(_) => {
                        self.error =
                            Some("Compositor does not support secure session locking".into());
                        return;
                    }
                }
                self.startup_deadline = Instant::now() + START_TIMEOUT;
                // The compositor may withhold `locked` until these first opaque
                // frames are presented. Creating surfaces cannot wait for ack.
                for output in self.outputs.outputs().collect::<Vec<_>>() {
                    if let Err(error) = self.add_output(output, qh) {
                        self.error = Some(error.to_string());
                        break;
                    }
                }
                self.dirty();
            }
            Event::Fatal(failure) => self.backend_failed(failure),
            event => {
                #[cfg(test)]
                let previous = self.form.anchor();
                #[cfg(test)]
                let failure = match &event {
                    Event::Finished {
                        attempt,
                        result: Err(failure),
                    } if self.form.attempt() == Some(*attempt) => Some(*failure),
                    _ => None,
                };
                let authenticated = self.form.receive(event, self.language);
                #[cfg(test)]
                if let Some(probe) = &self.probe {
                    use std::sync::atomic::Ordering::Release;
                    if previous.phase != Phase::Prompt && self.form.phase == Phase::Prompt {
                        probe.prompts.fetch_add(1, Release);
                    }
                    if previous.attempt.is_some()
                        && self.form.attempt().is_none()
                        && self.form.error
                    {
                        *probe
                            .last_failure
                            .lock()
                            .expect("last authentication failure") = failure;
                        probe.failures.fetch_add(1, Release);
                    }
                }
                self.dirty();
                if let Some(attempt) = authenticated {
                    self.request_unlock(attempt, qh);
                }
            }
        }
    }

    fn backend_failed(&mut self, failure: Failure) {
        if self.unlock_attempt.is_some() {
            return;
        }
        self.form.unavailable(failure, self.language);
        self.dirty();
        if !self.locked_ack {
            self.error = Some("Current-user authentication is unavailable before locking".into());
        }
        #[cfg(test)]
        if let Some(probe) = &self.probe {
            *probe
                .last_failure
                .lock()
                .expect("last authentication failure") = Some(failure);
            probe
                .failures
                .fetch_add(1, std::sync::atomic::Ordering::Release);
        }
    }

    fn send(&mut self, command: Command) {
        if !self.worker.send(command) {
            self.backend_failed(Failure::Unavailable);
        }
    }

    fn cancel_attempt(&mut self) {
        if let Some(command) = self.form.cancel() {
            self.send(command);
            #[cfg(test)]
            if let Some(probe) = &self.probe {
                probe
                    .cancelled
                    .fetch_add(1, std::sync::atomic::Ordering::Release);
            }
        }
        self.dirty();
    }

    fn request_unlock(&mut self, attempt: u64, qh: &QueueHandle<Self>) {
        if !self.locked_ack
            || self.unlock_attempt.is_some()
            || self.form.phase != Phase::Unlocking
            || self.form.attempt() != Some(attempt)
        {
            return;
        }
        let Some(lock) = self.lock.as_ref().filter(|lock| lock.is_locked()) else {
            self.error = Some("Authenticated result has no active compositor lock".into());
            return;
        };
        self.unlock_attempt = Some(attempt);
        self.unlock_deadline = Some(Instant::now() + UNLOCK_SYNC_TIMEOUT);
        lock.unlock();
        // This callback is ordered after unlock_and_destroy on the same
        // connection. Only its Done may make the process report UNLOCKED/exit.
        self.connection.display().sync(qh, UnlockSync { attempt });
    }

    fn draw(&mut self, qh: &QueueHandle<Self>) -> Result<()> {
        if self.unlock_attempt.is_some() {
            return Ok(());
        }
        if self.paint_dirty {
            render::paint(
                &mut self.image,
                &self.form,
                self.language,
                &self.theme,
                &self.clock,
                &self.date,
            );
            self.paint_dirty = false;
        }
        let anchor = self.form.anchor();
        for output in &mut self.surfaces {
            if !output.configured || !output.dirty || output.frame_pending {
                continue;
            }
            let surface = output.surface.wl_surface();
            if output
                .buffers
                .attach(&self.image, surface, output.size, output.scale)?
            {
                if surface.version() >= 3 {
                    surface.set_buffer_scale(output.scale);
                }
                if surface.version() >= 4 {
                    surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
                } else {
                    surface.damage(0, 0, output.size.0 as i32, output.size.1 as i32);
                }
                surface.frame(qh, FrameCallbackData(surface.clone()));
                surface.commit();
                output.frame_pending = true;
                output.dirty = false;
                output.shown = Some(anchor);
                #[cfg(test)]
                if let Some(probe) = &self.probe {
                    probe
                        .commits
                        .fetch_add(1, std::sync::atomic::Ordering::Release);
                }
            }
        }
        Ok(())
    }

    fn remove_keyboard(&mut self, seat: &WlSeat) {
        let active = self
            .keyboards
            .iter()
            .any(|keyboard| &keyboard.seat == seat && keyboard.focus.is_some());
        self.keyboards.retain(|keyboard| {
            if &keyboard.seat == seat {
                if keyboard.object.version() >= 3 {
                    keyboard.object.release();
                }
                false
            } else {
                true
            }
        });
        if active {
            self.cancel_attempt();
        }
        self.probe_focus();
    }

    fn probe_focus(&self) {
        #[cfg(test)]
        if let Some(probe) = &self.probe {
            probe.focused.store(
                self.keyboards
                    .iter()
                    .any(|keyboard| keyboard.focus.is_some()),
                std::sync::atomic::Ordering::Release,
            );
        }
    }

    #[cfg(test)]
    fn capture_empty_render(&self) {
        let Some(probe) = &self.probe else {
            return;
        };
        if !self.locked_ack
            || self.form.masked_len() != 0
            || self.paint_dirty
            || !self
                .surfaces
                .iter()
                .any(|surface| surface.shown == Some(self.form.anchor()))
        {
            return;
        }
        let Some(path) = probe.capture.lock().expect("capture request").take() else {
            return;
        };
        let capture = || -> Result<()> {
            let file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?;
            let mut encoder = png::Encoder::new(file, render::WIDTH, render::HEIGHT);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header()?;
            writer.write_image_data(self.image.data())?;
            writer.finish()?;
            Ok(())
        };
        match capture() {
            Ok(()) => {
                probe
                    .captures
                    .fetch_add(1, std::sync::atomic::Ordering::Release);
            }
            Err(error) => {
                *probe.capture_error.lock().expect("capture error") = Some(error.to_string());
            }
        }
    }
}

impl SessionLockHandler for Runtime {
    fn locked(&mut self, _: &Connection, _: &QueueHandle<Self>, _: SessionLock) {
        if self.locked_ack || self.ready_uid.is_none() {
            return;
        }
        self.locked_ack = true;
        self.form.locked();
        self.dirty();
        crate::report_locked(self.notify);
        #[cfg(test)]
        if let Some(probe) = &self.probe {
            probe
                .locked_ack
                .store(true, std::sync::atomic::Ordering::Release);
        }
    }

    fn finished(&mut self, _: &Connection, _: &QueueHandle<Self>, _: SessionLock) {
        // A compositor refusal/revocation is not a current-user PAM success.
        // Disconnecting here cannot request an unauthenticated unlock.
        self.form.clear();
        self.error = Some(
            if self.locked_ack {
                "Compositor stopped the session lock"
            } else {
                "Compositor refused the session lock"
            }
            .into(),
        );
        #[cfg(test)]
        if let Some(probe) = &self.probe {
            probe
                .finished
                .store(true, std::sync::atomic::Ordering::Release);
        }
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: SessionLockSurface,
        configure: SessionLockSurfaceConfigure,
        _: u32,
    ) {
        if let Some(output) = self
            .surfaces
            .iter_mut()
            .find(|output| output.surface.wl_surface() == surface.wl_surface())
        {
            output.size = configure.new_size;
            if let Err(error) = shm::dimensions(output.size, output.scale) {
                self.error = Some(error.to_string());
                return;
            }
            output.configured = true;
            // A configure needs its exact-size buffer promptly, independent of
            // an earlier-size frame callback that may no longer be scheduled.
            output.frame_pending = false;
            output.dirty = true;
            output.shown = None;
        }
    }
}

impl smithay_client_toolkit::dispatch2::Dispatch2<WlCallback, Runtime> for UnlockSync {
    fn event(
        &self,
        state: &mut Runtime,
        _: &WlCallback,
        event: wl_callback::Event,
        _: &Connection,
        _: &QueueHandle<Runtime>,
    ) {
        if matches!(event, wl_callback::Event::Done { .. })
            && state.locked_ack
            && state.unlock_attempt == Some(self.attempt)
            && state.form.phase == Phase::Unlocking
            && state.form.attempt() == Some(self.attempt)
        {
            if let Err(error) = state.lease.authenticated_unlock_acknowledged() {
                state.error = Some(format!(
                    "Could not acknowledge lock recovery completion: {error}"
                ));
                return;
            }
            state.unlocked = true;
            #[cfg(test)]
            if let Some(probe) = &state.probe {
                probe
                    .unlocked
                    .store(true, std::sync::atomic::Ordering::Release);
            }
        }
    }
}

impl CompositorHandler for Runtime {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &WlSurface,
        factor: i32,
    ) {
        if let Some(output) = self
            .surfaces
            .iter_mut()
            .find(|output| output.surface.wl_surface() == surface)
        {
            output.scale = if surface.version() >= 3 {
                factor.max(1)
            } else {
                1
            };
            if let Err(error) = shm::dimensions(output.size, output.scale) {
                self.error = Some(error.to_string());
            }
            output.dirty = true;
        }
    }
    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: wl_output::Transform,
    ) {
    }
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &WlSurface, _: u32) {
        if let Some(output) = self
            .surfaces
            .iter_mut()
            .find(|output| output.surface.wl_surface() == surface)
        {
            output.frame_pending = false;
        }
    }
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

impl OutputHandler for Runtime {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }
    fn new_output(&mut self, _: &Connection, qh: &QueueHandle<Self>, output: WlOutput) {
        if let Err(error) = self.add_output(output, qh) {
            self.error = Some(error.to_string());
        }
    }
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, output: WlOutput) {
        let removed: Vec<_> = self
            .surfaces
            .iter()
            .filter(|current| current.output == output)
            .map(|current| current.surface.wl_surface().clone())
            .collect();
        let active = self.keyboards.iter().any(|keyboard| {
            keyboard
                .focus
                .as_ref()
                .is_some_and(|focus| removed.contains(focus))
        });
        for keyboard in &mut self.keyboards {
            if keyboard
                .focus
                .as_ref()
                .is_some_and(|focus| removed.contains(focus))
            {
                keyboard.focus = None;
                keyboard.held.clear();
            }
        }
        self.surfaces.retain(|current| current.output != output);
        if active {
            self.cancel_attempt();
        }
        self.probe_focus();
        #[cfg(test)]
        if let Some(probe) = &self.probe {
            probe
                .surfaces
                .store(self.surfaces.len(), std::sync::atomic::Ordering::Release);
        }
    }
}

impl SeatHandler for Runtime {
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
        if capability == Capability::Keyboard
            && !self.keyboards.iter().any(|keyboard| keyboard.seat == seat)
        {
            match self.seats.get_keyboard::<Self>(qh, &seat, None) {
                Ok(object) => self.keyboards.push(Keyboard {
                    seat,
                    object,
                    focus: None,
                    held: HashSet::new(),
                    ctrl: false,
                    alt: false,
                    logo: false,
                }),
                Err(_) => self.error = Some("Locker keyboard initialization failed".into()),
            }
        }
    }
    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        seat: WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard {
            self.remove_keyboard(&seat);
        }
    }
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, seat: WlSeat) {
        self.remove_keyboard(&seat);
    }
}

impl KeyboardHandler for Runtime {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        object: &WlKeyboard,
        surface: &WlSurface,
        _: u32,
        held: &[u32],
        _: &[Keysym],
    ) {
        let ours = self
            .surfaces
            .iter()
            .any(|output| output.surface.wl_surface() == surface);
        if let Some(keyboard) = self
            .keyboards
            .iter_mut()
            .find(|keyboard| &keyboard.object == object)
        {
            keyboard.focus = ours.then(|| surface.clone());
            keyboard.held = held.iter().copied().collect();
        }
        self.probe_focus();
    }
    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        object: &WlKeyboard,
        _: &WlSurface,
        _: u32,
    ) {
        let active = self
            .keyboards
            .iter_mut()
            .find(|keyboard| &keyboard.object == object)
            .is_some_and(|keyboard| {
                let active = keyboard.focus.take().is_some();
                keyboard.held.clear();
                active
            });
        if active {
            self.cancel_attempt();
        }
        self.probe_focus();
    }
    fn press_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        object: &WlKeyboard,
        _: u32,
        mut event: KeyEvent,
    ) {
        let permitted = self
            .keyboards
            .iter_mut()
            .find(|keyboard| &keyboard.object == object)
            .and_then(|keyboard| {
                if keyboard.held.insert(event.raw_code) {
                    keyboard.focus.as_ref().map(|surface| {
                        (surface.clone(), keyboard.ctrl, keyboard.alt, keyboard.logo)
                    })
                } else {
                    None
                }
            });
        if let Some((surface, ctrl, alt, logo)) = permitted {
            let shown = self
                .surfaces
                .iter()
                .find(|output| output.surface.wl_surface() == &surface)
                .and_then(|output| output.shown);
            if self.locked_ack && self.unlock_attempt.is_none() {
                if event.keysym == Keysym::Escape {
                    self.cancel_attempt();
                } else if shown == Some(self.form.anchor()) {
                    match self.form.phase {
                        Phase::Idle
                            if !ctrl
                                && !alt
                                && !logo
                                && (matches!(event.keysym, Keysym::Return | Keysym::KP_Enter)
                                    || event.raw_code == 5) =>
                        {
                            if let Some(command) = self.form.begin() {
                                #[cfg(test)]
                                if let Some(probe) = &self.probe {
                                    probe.attempt.store(
                                        self.form.attempt().unwrap_or(0),
                                        std::sync::atomic::Ordering::Release,
                                    );
                                }
                                self.send(command);
                            }
                            self.dirty();
                        }
                        Phase::Prompt => {
                            match event.keysym {
                                Keysym::Return | Keysym::KP_Enter if !ctrl && !alt && !logo => {
                                    if let Some(command) = self.form.submit() {
                                        self.send(command);
                                    }
                                }
                                Keysym::BackSpace if !alt && !logo => self.form.backspace(),
                                _ if ctrl
                                    && !alt
                                    && !logo
                                    && (event.raw_code == 22
                                        || matches!(event.keysym, Keysym::u | Keysym::U)) =>
                                {
                                    self.form.clear()
                                }
                                _ if !ctrl && !alt && !logo => {
                                    if let Some(value) = event.utf8.as_deref() {
                                        self.form.insert(value);
                                    }
                                }
                                _ => {}
                            }
                            self.dirty();
                        }
                        _ => {}
                    }
                }
            }
        }
        if let Some(text) = event.utf8.as_mut() {
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
        if let Some(text) = event.utf8.as_mut() {
            text.zeroize();
        }
    }
    fn release_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        object: &WlKeyboard,
        _: u32,
        mut event: KeyEvent,
    ) {
        if let Some(keyboard) = self
            .keyboards
            .iter_mut()
            .find(|keyboard| &keyboard.object == object)
        {
            keyboard.held.remove(&event.raw_code);
        }
        if let Some(text) = event.utf8.as_mut() {
            text.zeroize();
        }
    }
    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        object: &WlKeyboard,
        _: u32,
        modifiers: Modifiers,
        _: RawModifiers,
        group: u32,
    ) {
        if let Some(keyboard) = self
            .keyboards
            .iter_mut()
            .find(|keyboard| &keyboard.object == object)
        {
            keyboard.ctrl = modifiers.ctrl;
            keyboard.alt = modifiers.alt;
            keyboard.logo = modifiers.logo;
            if keyboard.focus.is_some() {
                self.form.caps_lock = modifiers.caps_lock;
                self.form.layout = group;
                self.dirty();
            }
        }
    }
}

impl ShmHandler for Runtime {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}
impl ProvidesRegistryState for Runtime {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState, SeatState];
}
delegate_dispatch2!(Runtime);
delegate_registry!(Runtime);
