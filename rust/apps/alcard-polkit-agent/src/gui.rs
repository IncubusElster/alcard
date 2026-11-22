//! Independent credential surface. No keyboard focus outside an active request.
use crate::{
    native::{Registration, Worker},
    protocol::{Command, Event},
    render,
    ui::{Form, Language, Phase},
};
use alcard_ui::{
    preferences::ConfigSource,
    shm::{self, Buffers},
    theme::Theme,
};
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
    shell::{
        wlr_layer::{
            KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
        WaylandSurface,
    },
    shm::{Shm, ShmHandler},
};
use std::{
    collections::HashSet,
    error::Error,
    io,
    time::{Duration, Instant},
};
use tiny_skia::Pixmap;
use wayland_client::{
    globals::registry_queue_init,
    protocol::{
        wl_keyboard::WlKeyboard,
        wl_output::{self, WlOutput},
        wl_seat::WlSeat,
        wl_surface::WlSurface,
    },
    Connection, Proxy, QueueHandle,
};
use zeroize::Zeroize;

type Result<T> = std::result::Result<T, Box<dyn Error>>;
#[cfg(test)]
#[derive(Default)]
pub struct TestProbe {
    pub ready: std::sync::atomic::AtomicBool,
    pub focused: std::sync::atomic::AtomicBool,
    pub choosing: std::sync::atomic::AtomicBool,
    pub prompt: std::sync::atomic::AtomicUsize,
    pub echo: std::sync::atomic::AtomicBool,
    pub retry: std::sync::atomic::AtomicUsize,
    pub closed: std::sync::atomic::AtomicUsize,
    pub stop: std::sync::atomic::AtomicBool,
}
struct Options {
    registration: Registration,
    preferences: Option<ConfigSource>,
    #[cfg(test)]
    probe: Option<std::sync::Arc<TestProbe>>,
}
struct Keyboard {
    seat: WlSeat,
    object: WlKeyboard,
    focused: bool,
    held: HashSet<u32>,
    ctrl: bool,
    alt: bool,
    logo: bool,
}
struct Runtime {
    registry: RegistryState,
    seats: SeatState,
    outputs: OutputState,
    compositor: CompositorState,
    layers: LayerShell,
    shm: Shm,
    layer: Option<LayerSurface>,
    form: Option<Form>,
    language: Language,
    theme: Theme,
    preferences: Option<ConfigSource>,
    keyboards: Vec<Keyboard>,
    size: (u32, u32),
    scale: i32,
    configured: bool,
    frame_pending: bool,
    dirty: bool,
    image: Pixmap,
    buffers: Buffers,
    worker: Worker,
    error: Option<String>,
    session_lost: bool,
    exit: bool,
    close_after: Option<Instant>,
    #[cfg(test)]
    probe: Option<std::sync::Arc<TestProbe>>,
}
pub fn run(language: Language, theme: Theme, preferences: ConfigSource) -> Result<()> {
    run_registered(
        language,
        theme,
        Options {
            registration: Registration::LoginSession,
            preferences: Some(preferences),
            #[cfg(test)]
            probe: None,
        },
    )
}
#[cfg(test)]
pub fn run_test(probe: std::sync::Arc<TestProbe>) -> Result<()> {
    run_registered(
        Language::En,
        Theme::classic(),
        Options {
            registration: Registration::OwnProcess,
            preferences: None,
            probe: Some(probe),
        },
    )
}
fn run_registered(language: Language, theme: Theme, options: Options) -> Result<()> {
    for variable in ["POLKIT_DEBUG", "G_DBUS_DEBUG", "WAYLAND_DEBUG"] {
        if std::env::var_os(variable).is_some() {
            return Err(format!("Refusing credential input with {variable} enabled").into());
        }
    }
    let connection = Connection::connect_to_env()?;
    let (globals, queue) = registry_queue_init::<Runtime>(&connection)?;
    let qh = queue.handle();
    let mut event_loop: EventLoop<Runtime> = EventLoop::try_new()?;
    let shutdown = crate::shutdown::Watcher::new(event_loop.get_signal())?;
    WaylandSource::new(connection.clone(), queue).insert(event_loop.handle())?;
    let compositor = CompositorState::bind(&globals, &qh)?;
    let layers = LayerShell::bind(&globals, &qh)?;
    let shm = Shm::bind(&globals, &qh)?;
    let buffers = Buffers::new(&shm)?;
    let (events, incoming) = channel::channel();
    let callbacks = qh.clone();
    event_loop
        .handle()
        .insert_source(incoming, move |event, _, state| match event {
            channel::Event::Msg(event) => state.receive(event, &callbacks),
            channel::Event::Closed => {
                state.error = Some("Native authentication worker stopped".into());
            }
        })?;
    let worker = Worker::start(events, options.registration)?;
    let mut state = Runtime {
        registry: RegistryState::new(&globals),
        seats: SeatState::new(&globals, &qh),
        outputs: OutputState::new(&globals, &qh),
        compositor,
        layers,
        shm,
        layer: None,
        form: None,
        language,
        theme,
        preferences: options.preferences,
        keyboards: Vec::new(),
        size: (320, 170),
        scale: 1,
        configured: false,
        frame_pending: false,
        dirty: false,
        image: Pixmap::new(320, 170).ok_or("Cannot allocate credential surface")?,
        buffers,
        worker,
        error: None,
        session_lost: false,
        exit: false,
        close_after: None,
        #[cfg(test)]
        probe: options.probe,
    };
    loop {
        if shutdown.requested() {
            state.exit = true;
        }
        #[cfg(test)]
        if state
            .probe
            .as_ref()
            .is_some_and(|p| p.stop.load(std::sync::atomic::Ordering::Acquire))
        {
            state.exit = true;
        }
        if state.exit || state.error.is_some() {
            break;
        }
        if state
            .close_after
            .is_some_and(|until| Instant::now() >= until)
        {
            state.close();
        }
        state.draw(&qh)?;
        let wait = state
            .close_after
            .map(|until| until.saturating_duration_since(Instant::now()));
        #[cfg(test)]
        let wait = if state.probe.is_some() {
            Some(
                wait.unwrap_or(Duration::from_millis(100))
                    .min(Duration::from_millis(100)),
            )
        } else {
            wait
        };
        event_loop.dispatch(wait, &mut state)?;
    }
    state.cancel();
    if state.session_lost {
        Err(crate::protocol::SessionLost.into())
    } else if let Some(error) = state.error.take() {
        Err(io::Error::other(error).into())
    } else {
        Ok(())
    }
}
impl Runtime {
    fn receive(&mut self, event: Event, qh: &QueueHandle<Self>) {
        #[cfg(test)]
        if let Some(p) = self.probe.as_ref() {
            use std::sync::atomic::Ordering::Release;
            eprintln!(
                "Agent event: {}",
                match &event {
                    Event::Ready => "ready",
                    Event::Open { .. } => "open",
                    Event::Challenge { .. } => "challenge",
                    Event::Notice { .. } => "notice",
                    Event::Retry { .. } => "retry",
                    Event::Closed { .. } => "closed",
                    Event::Fatal(_) => "fatal",
                    Event::SessionLost => "session-lost",
                }
            );
            match &event {
                Event::Ready => p.ready.store(true, Release),
                Event::Open { identities, .. } => p.choosing.store(identities.len() > 1, Release),
                Event::Challenge { echo, .. } => {
                    p.echo.store(*echo, Release);
                    p.choosing.store(false, Release);
                    p.prompt.fetch_add(1, Release);
                }
                Event::Retry { .. } => {
                    p.retry.fetch_add(1, Release);
                }
                Event::Closed { .. } => {
                    p.closed.fetch_add(1, Release);
                }
                _ => {}
            }
        }
        match event {
            Event::Ready => eprintln!("Alcard graphical polkit agent registered"),
            Event::Fatal(message) => {
                self.error = Some(message);
            }
            Event::SessionLost => {
                self.session_lost = true;
                self.exit = true;
            }
            Event::Open {
                id,
                action,
                reason,
                identities,
            } => {
                self.close();
                let surface = self.compositor.create_surface(qh);
                let layer = self.layers.create_layer_surface(
                    qh,
                    surface,
                    Layer::Overlay,
                    Some("alcard-polkit-agent"),
                    None,
                );
                layer.set_size(320, 170);
                layer.set_exclusive_zone(-1);
                layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
                layer.commit();
                self.layer = Some(layer);
                self.configured = false;
                self.frame_pending = false;
                self.size = (320, 170);
                self.scale = 1;
                self.dirty = true;
                // Resolve presentation only between requests. Never replace
                // a live credential form, input buffer, or authentication state.
                if let Some(source) = &self.preferences {
                    match source.load() {
                        Ok(resolved) => {
                            self.language = resolved.locale.resolve();
                            self.theme = resolved.theme;
                        }
                        Err(error) => {
                            eprintln!("Authentication presentation preferences unchanged: {error}")
                        }
                    }
                }
                let mut form = Form::new(id, action, reason, identities);
                let command = form.auto_select();
                self.form = Some(form);
                if let Some(command) = command {
                    self.send(command);
                }
            }
            Event::Closed {
                id,
                authorized: true,
            } if self.form.as_ref().is_some_and(|f| f.id == id) => self.close(),
            Event::Closed {
                id,
                authorized: false,
            } if self.form.as_ref().is_some_and(|f| f.id == id) => {
                if let Some(form) = self.form.as_mut() {
                    form.receive(
                        Event::Closed {
                            id,
                            authorized: false,
                        },
                        self.language,
                    );
                }
                self.close_after = Some(Instant::now() + Duration::from_millis(1500));
                self.dirty = true;
            }
            other => {
                if let Some(form) = self.form.as_mut() {
                    form.receive(other, self.language);
                    self.dirty = true;
                }
            }
        }
    }
    fn send(&mut self, command: Command) {
        if !self.worker.send(command) {
            self.error = Some("Authentication backend disconnected".into());
        }
    }
    fn cancel(&mut self) {
        if let Some(form) = self.form.as_mut() {
            let command = form.cancel();
            self.send(command);
        }
        self.close();
    }
    fn close(&mut self) {
        if let Some(form) = self.form.as_mut() {
            form.clear();
        }
        self.form = None;
        self.layer = None;
        self.close_after = None;
        self.configured = false;
        self.frame_pending = false;
        self.dirty = false;
        for keyboard in &mut self.keyboards {
            keyboard.focused = false;
            keyboard.held.clear();
        }
        #[cfg(test)]
        if let Some(p) = self.probe.as_ref() {
            p.focused.store(false, std::sync::atomic::Ordering::Release);
        }
    }
    fn draw(&mut self, qh: &QueueHandle<Self>) -> Result<()> {
        if !self.configured || !self.dirty || self.frame_pending {
            return Ok(());
        }
        let (Some(form), Some(layer)) = (self.form.as_ref(), self.layer.as_ref()) else {
            return Ok(());
        };
        render::paint(&mut self.image, form, self.language, &self.theme);
        let surface = layer.wl_surface();
        if self
            .buffers
            .attach(&self.image, surface, self.size, self.scale)?
        {
            if surface.version() >= 3 {
                surface.set_buffer_scale(self.scale);
            }
            if surface.version() >= 4 {
                surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
            } else {
                surface.damage(0, 0, self.size.0 as i32, self.size.1 as i32);
            }
            surface.frame(qh, FrameCallbackData(surface.clone()));
            surface.commit();
            self.frame_pending = true;
            self.dirty = false;
        }
        Ok(())
    }
    fn remove_keyboard(&mut self, seat: &WlSeat) {
        let active = self.keyboards.iter().any(|k| &k.seat == seat && k.focused);
        self.keyboards.retain(|k| {
            if &k.seat == seat {
                if k.object.version() >= 3 {
                    k.object.release();
                }
                false
            } else {
                true
            }
        });
        if active {
            self.cancel();
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
        if self
            .layer
            .as_ref()
            .is_some_and(|l| l.wl_surface() == surface)
        {
            self.scale = if surface.version() >= 3 {
                factor.max(1)
            } else {
                1
            };
            if let Err(error) = shm::dimensions(self.size, self.scale) {
                self.error = Some(error.to_string());
            }
            self.dirty = true;
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
        if self
            .layer
            .as_ref()
            .is_some_and(|l| l.wl_surface() == surface)
        {
            self.frame_pending = false;
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
impl LayerShellHandler for Runtime {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        if self
            .layer
            .as_ref()
            .is_some_and(|l| l.wl_surface() == layer.wl_surface())
        {
            self.cancel();
        }
    }
    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        layer: &LayerSurface,
        config: LayerSurfaceConfigure,
        _: u32,
    ) {
        if self
            .layer
            .as_ref()
            .is_none_or(|l| l.wl_surface() != layer.wl_surface())
        {
            return;
        }
        self.size = (
            if config.new_size.0 == 0 {
                320
            } else {
                config.new_size.0
            },
            if config.new_size.1 == 0 {
                170
            } else {
                config.new_size.1
            },
        );
        if let Err(error) = shm::dimensions(self.size, self.scale) {
            self.error = Some(error.to_string());
        }
        self.configured = true;
        self.dirty = true;
    }
}
impl OutputHandler for Runtime {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {
        if self.form.is_some() {
            self.cancel();
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
        if capability == Capability::Keyboard && !self.keyboards.iter().any(|k| k.seat == seat) {
            match self.seats.get_keyboard::<Self>(qh, &seat, None) {
                Ok(object) => self.keyboards.push(Keyboard {
                    seat,
                    object,
                    focused: false,
                    held: HashSet::new(),
                    ctrl: false,
                    alt: false,
                    logo: false,
                }),
                Err(_) => self.error = Some("Keyboard initialization failed".into()),
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
        if let Some(k) = self.keyboards.iter_mut().find(|k| &k.object == object) {
            k.focused = self
                .layer
                .as_ref()
                .is_some_and(|l| l.wl_surface() == surface);
            k.held = held.iter().copied().collect();
            #[cfg(test)]
            if let Some(p) = self.probe.as_ref() {
                p.focused
                    .store(k.focused, std::sync::atomic::Ordering::Release);
            }
        }
    }
    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        object: &WlKeyboard,
        _: &WlSurface,
        _: u32,
    ) {
        let focused = self
            .keyboards
            .iter_mut()
            .find(|k| &k.object == object)
            .is_some_and(|k| {
                let active = k.focused;
                k.focused = false;
                k.held.clear();
                active
            });
        if focused {
            self.cancel();
        }
    }
    fn press_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        object: &WlKeyboard,
        _: u32,
        mut event: KeyEvent,
    ) {
        let allowed = self
            .keyboards
            .iter_mut()
            .find(|k| &k.object == object)
            .and_then(|k| {
                if k.focused && k.held.insert(event.raw_code) {
                    Some((k.ctrl, k.alt, k.logo))
                } else {
                    None
                }
            });
        if let Some((ctrl, alt, logo)) = allowed {
            if event.keysym == Keysym::Escape {
                self.cancel();
            } else if let Some(form) = self.form.as_mut() {
                match event.keysym {
                    Keysym::Return | Keysym::KP_Enter => {
                        if form.phase == Phase::Finished {
                            self.close();
                        } else if let Some(command) = form.enter() {
                            self.send(command);
                        }
                    }
                    Keysym::F1 => {
                        form.details = !form.details;
                        form.scroll = 0;
                    }
                    Keysym::Up => form.navigate(false),
                    Keysym::Down => form.navigate(true),
                    Keysym::BackSpace => form.backspace(),
                    Keysym::u | Keysym::U if ctrl => form.clear(),
                    _ if !ctrl && !alt && !logo => {
                        if let Some(text) = event.utf8.as_deref() {
                            form.insert(text);
                        }
                    }
                    _ => {}
                }
                self.dirty = true;
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
        if let Some(k) = self.keyboards.iter_mut().find(|k| &k.object == object) {
            k.held.remove(&event.raw_code);
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
        if let Some(k) = self.keyboards.iter_mut().find(|k| &k.object == object) {
            k.ctrl = modifiers.ctrl;
            k.alt = modifiers.alt;
            k.logo = modifiers.logo;
        }
        if let Some(form) = self.form.as_mut() {
            form.caps_lock = modifiers.caps_lock;
            form.layout = group;
            self.dirty = true;
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
