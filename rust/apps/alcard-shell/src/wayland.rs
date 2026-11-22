//! One SCTK connection and calloop for Home, notifications and foreign windows.
//! labwc still owns focus, shortcuts, compositing and physical output.
mod idle;
use crate::{
    app::{Application, InputAnchor},
    controls, home,
    input::Input,
    layer::Popup,
    locking,
    model::Request,
    notifications, radios, render,
    shm::{self, Buffers},
    sleep, status, system, toplevel,
};
use idle::IdleRuntime;
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, FrameCallbackData},
    delegate_dispatch2, delegate_registry,
    output::{OutputHandler, OutputState},
    reexports::{
        calloop::{
            channel,
            signals::{Signal, Signals},
            EventLoop,
        },
        calloop_wayland_source::WaylandSource,
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers},
        Capability, SeatHandler, SeatState,
    },
    shell::{
        wlr_layer::{LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure},
        xdg::{
            window::{Window, WindowConfigure, WindowDecorations, WindowHandler},
            XdgShell,
        },
        WaylandSurface,
    },
    shm::{Shm, ShmHandler},
};
use std::{error::Error, io};
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
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1::{self as handle, ZwlrForeignToplevelHandleV1 as Handle},
    zwlr_foreign_toplevel_manager_v1::{self as manager, ZwlrForeignToplevelManagerV1 as Manager},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
struct Keyboard {
    seat: WlSeat,
    keyboard: WlKeyboard,
    input: Input,
}

pub struct Runtime {
    app: Application,
    registry: RegistryState,
    seats: SeatState,
    outputs: OutputState,
    compositor: CompositorState,
    shm: Shm,
    window: Window,
    size: (u32, u32),
    scale: i32,
    configured: bool,
    frame_pending: bool,
    committed_input_anchor: Option<InputAnchor>,
    image: Pixmap,
    buffers: Buffers,
    layer_shell: Option<LayerShell>,
    popup: Option<Popup>,
    overlay_failed: bool,
    keyboards: Vec<Keyboard>,
    last_seat: Option<WlSeat>,
    windows: toplevel::State,
    manager: Option<Manager>,
    error: Option<String>,
    status_worker: Option<status::Worker>,
    idle: IdleRuntime,
}

pub fn run(app: Application) -> Result<()> {
    // Block HUP before starting workers so every thread inherits the mask.
    // A signalfd source wakes the existing loop; no polling or input shortcut.
    let reload = Signals::new(&[Signal::SIGHUP])?;
    let connection = Connection::connect_to_env()?;
    let (globals, queue) = registry_queue_init::<Runtime>(&connection)?;
    let qh = queue.handle();
    let mut event_loop: EventLoop<Runtime> = EventLoop::try_new()?;
    event_loop
        .handle()
        .insert_source(reload, |event, _, runtime| {
            if event.signal() == Signal::SIGHUP && runtime.app.reload_preferences() {
                runtime.idle.reload();
            }
        })?;
    WaylandSource::new(connection.clone(), queue).insert(event_loop.handle())?;
    let compositor = CompositorState::bind(&globals, &qh)?;
    let xdg = XdgShell::bind(&globals, &qh)?;
    let shm = Shm::bind(&globals, &qh)?;
    let window = xdg.create_window(
        compositor.create_surface(&qh),
        WindowDecorations::RequestServer,
        &qh,
    );
    window.set_app_id("alcard-shell");
    window.set_title("Alcard Shell");
    window.set_min_size(Some((render::WIDTH as u32, render::HEIGHT as u32)));
    window.commit();
    let buffers = Buffers::new(&shm)?;
    let image = Pixmap::new(render::WIDTH as u32, render::HEIGHT as u32)
        .ok_or_else(|| io::Error::other("Shell image allocation failed"))?;
    let manager = globals.bind::<Manager, _, _>(&qh, 1..=3, ToplevelData).ok();
    let mut windows = toplevel::State::new();
    if manager.is_none() {
        windows.stop();
    }
    let (status_tx, status_rx) = channel::channel::<()>();
    let status_worker = match status::Worker::start(move || {
        let _ = status_tx.send(());
    }) {
        Ok(worker) => Some(worker),
        Err(error) => {
            eprintln!("System status worker unavailable: {error}");
            None
        }
    };
    let idle = IdleRuntime::new(&connection, &globals, &qh, app.idle_preferences());
    let mut runtime = Runtime {
        app,
        registry: RegistryState::new(&globals),
        seats: SeatState::new(&globals, &qh),
        outputs: OutputState::new(&globals, &qh),
        compositor,
        shm,
        window,
        size: (render::WIDTH as u32, render::HEIGHT as u32),
        scale: 1,
        configured: false,
        frame_pending: false,
        committed_input_anchor: None,
        image,
        buffers,
        layer_shell: LayerShell::bind(&globals, &qh).ok(),
        popup: None,
        overlay_failed: false,
        keyboards: Vec::new(),
        last_seat: None,
        windows,
        manager,
        error: None,
        status_worker,
        idle,
    };
    event_loop
        .handle()
        .insert_source(status_rx, |event, _, state| {
            if let channel::Event::Msg(()) = event {
                if let Some(snapshot) = state
                    .status_worker
                    .as_ref()
                    .and_then(status::Worker::take_update)
                {
                    state.app.status(snapshot.status);
                    state.app.observe_battery(snapshot.battery);
                }
            }
        })?;
    let (home_tx, home_rx) = channel::channel::<()>();
    // Bind the single-instance socket before starting a notification service.
    home::start(home_tx)?;
    event_loop
        .handle()
        .insert_source(home_rx, |event, _, state| {
            if let channel::Event::Msg(()) = event {
                if state.app.lock_inhibits_activation() {
                    return;
                }
                state.app.home();
                if state.manager.is_some() {
                    state.windows.request_home();
                } else {
                    eprintln!("Home focus unavailable: compositor has no foreign-toplevel manager");
                }
            }
        })?;
    let (notification_tx, notification_rx) = channel::channel::<notifications::Message>();
    event_loop
        .handle()
        .insert_source(notification_rx, |event, _, state| {
            if let channel::Event::Msg(message) = event {
                state.app.receive_notification(message);
            }
        })?;
    runtime
        .app
        .set_notifications(notifications::Client::start(notification_tx));
    let (system_tx, system_rx) = channel::channel::<system::Message>();
    event_loop
        .handle()
        .insert_source(system_rx, |event, _, state| {
            if let channel::Event::Msg(message) = event {
                state.app.receive_system(message);
            }
        })?;
    runtime.app.set_system(system::Client::start(system_tx));
    let (controls_tx, controls_rx) = channel::channel::<controls::Message>();
    event_loop
        .handle()
        .insert_source(controls_rx, |event, _, state| {
            if let channel::Event::Msg(message) = event {
                state.app.receive_controls(message);
            }
        })?;
    runtime
        .app
        .set_controls(controls::Client::start(controls_tx));
    let (radios_tx, radios_rx) = channel::channel::<radios::Message>();
    event_loop
        .handle()
        .insert_source(radios_rx, |event, _, state| {
            if let channel::Event::Msg(message) = event {
                state.app.receive_radios(message);
            }
        })?;
    runtime.app.set_radios(radios::Client::start(radios_tx));
    let (lock_tx, lock_rx) = channel::channel::<locking::Message>();
    event_loop
        .handle()
        .insert_source(lock_rx, |event, _, state| {
            if let channel::Event::Msg(message) = event {
                state.idle.receive_lock(&message);
                state.app.receive_lock(message);
            }
        })?;
    runtime
        .app
        .set_locking(locking::Client::start(lock_tx, &connection)?);
    let (sleep_tx, sleep_rx) = channel::channel::<sleep::Message>();
    event_loop
        .handle()
        .insert_source(sleep_rx, |event, _, state| {
            if let channel::Event::Msg(message) = event {
                state.app.receive_sleep(message);
            }
        })?;
    runtime.app.set_sleep(sleep::Client::start(sleep_tx));
    for seat in runtime.seats.seats() {
        runtime.idle.add_seat(seat, &qh);
    }
    for output in runtime.outputs.outputs() {
        runtime.idle.add_output(output, &qh);
    }
    loop {
        if let Some(error) = runtime.error.take() {
            return Err(io::Error::other(error).into());
        }
        runtime.observe_windows();
        // Observe already dispatched window events before an exit grace/timeout.
        runtime.app.tick();
        // All lock lifecycle events in the last dispatch batch have now been
        // reduced. Never commit a sleep from an individual LOCKED callback.
        runtime.app.flush_sleep_intent();
        if runtime.app.take_battery_refresh_request()
            && !runtime
                .status_worker
                .as_ref()
                .is_some_and(status::Worker::refresh)
        {
            runtime.app.battery_refresh_failed();
        }
        if runtime.app.take_resume_request() {
            runtime.idle.resume();
            runtime.committed_input_anchor = None;
            runtime.windows.cancel_shell_activation();
        }
        if runtime.idle.synchronize(
            runtime.app.idle_preferences(),
            runtime.app.lock_inhibits_activation() || runtime.app.state.system_busy(),
            &qh,
        ) {
            runtime.app.request_idle_lock();
        }
        let seat = runtime.activation_seat();
        if !runtime.app.lock_inhibits_activation() {
            runtime.windows.request_shell_activation(seat.as_ref());
        } else {
            runtime.windows.cancel_shell_activation();
        }
        runtime.update_popup(&qh)?;
        runtime.draw(&qh)?;
        let wait = runtime.app.wait_duration();
        event_loop.dispatch(wait, &mut runtime)?;
    }
}

impl Runtime {
    fn observe_windows(&mut self) {
        if let Some(update) = self.windows.take_update() {
            match update {
                Ok(windows) => self.app.windows(windows),
                Err(()) => {
                    self.manager = None;
                    self.app.windows_unavailable();
                }
            }
        }
    }
    fn activation_seat(&self) -> Option<WlSeat> {
        self.last_seat
            .clone()
            .or_else(|| self.keyboards.first().map(|k| k.seat.clone()))
            .or_else(|| self.seats.seats().next())
    }
    fn key(&mut self, key: crate::model::Key) -> bool {
        let before = self.app.input_anchor();
        // Window and keyboard events may arrive in the same Wayland batch.
        // Commit already observed window changes before choosing a command.
        self.observe_windows();
        if !self.app.input_target_unchanged(before, key) {
            return false;
        }
        if !self
            .app
            .system_input_matches_frame(self.committed_input_anchor.as_ref(), key)
        {
            return false;
        }
        let request = self.app.apply(key);
        let result = match &request {
            Request::ActivateWindow(id) => {
                self.windows.activate(*id, self.activation_seat().as_ref())
            }
            Request::CloseWindow(id) => self.windows.close(*id),
            _ => return true,
        };
        match result {
            // Request transmission is not a compositor acknowledgement. Keep
            // the source view; Application observes active/closed events.
            Ok(()) => {}
            Err(error) => {
                eprintln!("Window request failed: {error}");
                self.app.failed(request);
            }
        }
        true
    }
    fn update_popup(&mut self, qh: &QueueHandle<Self>) -> Result<()> {
        let banner = self.app.banner();
        match banner {
            None => {
                if self.popup.take().is_some() {
                    self.app.dirty = true;
                }
            }
            Some(banner) => {
                if let Some(popup) = self.popup.as_mut() {
                    popup.update(banner);
                } else if !self.overlay_failed {
                    if let Some(layer) = self.layer_shell.as_ref() {
                        match Popup::new(layer, &self.compositor, &self.shm, qh, banner) {
                            Ok(popup) => self.popup = Some(popup),
                            Err(error) => {
                                eprintln!("Notification layer unavailable: {error}");
                                self.overlay_failed = true;
                                self.app.dirty = true;
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
    fn draw(&mut self, qh: &QueueHandle<Self>) -> Result<()> {
        if let Some(popup) = self.popup.as_mut() {
            if popup.configured && popup.dirty && !popup.frame_pending {
                popup.paint();
                let surface = popup.layer.wl_surface();
                if popup
                    .buffers
                    .attach(&popup.image, surface, popup.size, popup.scale)?
                {
                    present(surface, qh, popup.size, popup.scale);
                    popup.frame_pending = true;
                    popup.dirty = false;
                    if !popup.mapped {
                        popup.mapped = true;
                        self.app.dirty = true;
                    }
                }
            }
        }
        if self.configured && self.app.dirty && !self.frame_pending {
            let fallback = if self.popup.as_ref().is_some_and(|p| p.mapped) {
                None
            } else {
                self.app.banner()
            };
            render::paint(
                &mut self.image,
                &self.app.state,
                &self.app.clock,
                fallback.as_ref().map(|b| b.title.as_str()),
            );
            let surface = self.window.wl_surface();
            if self
                .buffers
                .attach(&self.image, surface, self.size, self.scale)?
            {
                present(surface, qh, self.size, self.scale);
                self.committed_input_anchor = Some(self.app.input_anchor());
                self.frame_pending = true;
                self.app.dirty = false;
            }
        }
        Ok(())
    }
    fn remove_keyboard(&mut self, seat: &WlSeat) {
        self.keyboards.retain(|binding| {
            if &binding.seat == seat {
                if binding.keyboard.version() >= 3 {
                    binding.keyboard.release();
                }
                false
            } else {
                true
            }
        });
        if self.last_seat.as_ref() == Some(seat) {
            self.last_seat = None;
            self.app.focus(false);
        }
    }
}
fn present(surface: &WlSurface, qh: &QueueHandle<Runtime>, size: (u32, u32), scale: i32) {
    if surface.version() >= 3 {
        surface.set_buffer_scale(scale);
    }
    if surface.version() >= 4 {
        surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
    } else {
        surface.damage(0, 0, size.0 as i32, size.1 as i32);
    }
    surface.frame(qh, FrameCallbackData(surface.clone()));
    surface.commit();
}

impl CompositorHandler for Runtime {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &WlSurface,
        factor: i32,
    ) {
        let scale = if surface.version() >= 3 {
            factor.max(1)
        } else {
            1
        };
        if surface == self.window.wl_surface() {
            if let Err(error) = shm::dimensions(self.size, scale) {
                self.error = Some(error.to_string());
                return;
            }
            self.scale = scale;
            self.app.dirty = true;
        } else if let Some(popup) = self
            .popup
            .as_mut()
            .filter(|p| p.layer.wl_surface() == surface)
        {
            if let Err(error) = shm::dimensions(popup.size, scale) {
                self.error = Some(error.to_string());
                return;
            }
            popup.scale = scale;
            popup.dirty = true;
        }
    }
    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: wl_output::Transform,
    ) {
        // Normal buffer transform is valid; the compositor applies output rotation.
    }
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &WlSurface, _: u32) {
        if surface == self.window.wl_surface() {
            self.frame_pending = false;
        } else if let Some(popup) = self
            .popup
            .as_mut()
            .filter(|p| p.layer.wl_surface() == surface)
        {
            popup.frame_pending = false;
        }
        // No dirty flag here: a frame callback is permission, not an animation.
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
impl WindowHandler for Runtime {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Window) {
        eprintln!("Ignoring a close request for the session navigation Shell");
    }
    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &Window,
        config: WindowConfigure,
        _: u32,
    ) {
        let size = (
            config.new_size.0.map_or(render::WIDTH as u32, |n| n.get()),
            config.new_size.1.map_or(render::HEIGHT as u32, |n| n.get()),
        );
        if let Err(error) = shm::dimensions(size, self.scale) {
            self.error = Some(error.to_string());
            return;
        }
        self.size = size;
        self.configured = true;
        self.app.dirty = true;
    }
}
impl LayerShellHandler for Runtime {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        if self
            .popup
            .as_ref()
            .is_some_and(|p| p.layer.wl_surface() == layer.wl_surface())
        {
            self.popup = None;
            self.overlay_failed = true;
            self.app.dirty = true;
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
        if let Some(popup) = self
            .popup
            .as_mut()
            .filter(|p| p.layer.wl_surface() == layer.wl_surface())
        {
            let size = (
                if config.new_size.0 == 0 {
                    render::WIDTH as u32
                } else {
                    config.new_size.0
                },
                if config.new_size.1 == 0 {
                    crate::layer::HEIGHT
                } else {
                    config.new_size.1
                },
            );
            if let Err(error) = shm::dimensions(size, popup.scale) {
                self.error = Some(error.to_string());
                return;
            }
            popup.size = size;
            popup.configured = true;
            popup.dirty = true;
        }
    }
}
impl OutputHandler for Runtime {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }
    fn new_output(&mut self, _: &Connection, qh: &QueueHandle<Self>, output: WlOutput) {
        self.idle.add_output(output, qh);
    }
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, output: WlOutput) {
        self.idle.remove_output(&output);
    }
}
impl SeatHandler for Runtime {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seats
    }
    fn new_seat(&mut self, _: &Connection, qh: &QueueHandle<Self>, seat: WlSeat) {
        self.idle.add_seat(seat, qh);
    }
    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard && !self.keyboards.iter().any(|k| k.seat == seat) {
            match self.seats.get_keyboard::<Self>(qh, &seat, None) {
                Ok(keyboard) => self.keyboards.push(Keyboard {
                    seat,
                    keyboard,
                    input: Input::default(),
                }),
                Err(error) => {
                    self.error = Some(format!("Wayland keyboard initialization: {error}"))
                }
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
        self.idle.remove_seat(&seat);
        self.remove_keyboard(&seat);
    }
}
impl KeyboardHandler for Runtime {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        keyboard: &WlKeyboard,
        surface: &WlSurface,
        _: u32,
        held: &[u32],
        _: &[Keysym],
    ) {
        if let Some(binding) = self.keyboards.iter_mut().find(|b| &b.keyboard == keyboard) {
            if surface == self.window.wl_surface() {
                binding.input.enter(held);
                self.last_seat = Some(binding.seat.clone());
                self.app.focus(true);
            } else {
                binding.input.leave();
                self.app.focus(false);
            }
        }
    }
    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        keyboard: &WlKeyboard,
        _: &WlSurface,
        _: u32,
    ) {
        if let Some(binding) = self.keyboards.iter_mut().find(|b| &b.keyboard == keyboard) {
            binding.input.leave();
            self.app.focus(false);
        }
    }
    fn press_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        keyboard: &WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        // Classify one physical press once. Opening an editor must not turn
        // that same key into text; a completed compose sequence may contain
        // several scalars but must never continue into a changed input target.
        let editing = self.app.state.is_text_editing();
        let actions = self
            .keyboards
            .iter_mut()
            .find(|b| &b.keyboard == keyboard)
            .map(|binding| {
                binding
                    .input
                    .press(event.raw_code, event.keysym, event.utf8.as_deref(), editing)
            })
            .unwrap_or_default();
        for action in actions {
            if !self.key(action) || self.app.state.is_text_editing() != editing {
                break;
            }
        }
    }
    fn repeat_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: u32,
        _: KeyEvent,
    ) {
    }
    fn release_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        keyboard: &WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        if let Some(binding) = self.keyboards.iter_mut().find(|b| &b.keyboard == keyboard) {
            binding.input.release(event.raw_code);
        }
    }
    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        keyboard: &WlKeyboard,
        _: u32,
        modifiers: Modifiers,
        _: RawModifiers,
        _: u32,
    ) {
        if let Some(binding) = self.keyboards.iter_mut().find(|b| &b.keyboard == keyboard) {
            binding.input.update_modifiers(modifiers);
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
    registry_handlers![OutputState, SeatState, IdleRuntime];
}
struct ToplevelData;
impl smithay_client_toolkit::dispatch2::Dispatch2<Manager, Runtime> for ToplevelData {
    fn event(
        &self,
        state: &mut Runtime,
        _: &Manager,
        event: manager::Event,
        _: &Connection,
        _: &QueueHandle<Runtime>,
    ) {
        state.windows.manager_event(event);
    }
    fn event_created_child(
        opcode: u16,
        qh: &QueueHandle<Runtime>,
    ) -> std::sync::Arc<dyn wayland_client::backend::ObjectData> {
        match opcode {
            manager::EVT_TOPLEVEL_OPCODE => qh.make_data::<Handle, _>(ToplevelData),
            _ => unreachable!("unknown foreign-toplevel child event"),
        }
    }
}
impl smithay_client_toolkit::dispatch2::Dispatch2<Handle, Runtime> for ToplevelData {
    fn event(
        &self,
        state: &mut Runtime,
        proxy: &Handle,
        event: handle::Event,
        _: &Connection,
        _: &QueueHandle<Runtime>,
    ) {
        state.windows.handle_event(proxy, event);
    }
}
delegate_dispatch2!(Runtime);
delegate_registry!(Runtime);
