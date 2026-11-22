//! Compositor-wide idle and output power on the existing Shell connection.
//! No keyboard snooping, focused-window timer, shell command or extra daemon.
use super::Runtime;
use crate::{
    idle::{Policy, Timer},
    locking,
};
use alcard_ui::preferences::IdlePreferences;
use smithay_client_toolkit::{
    dispatch2::Dispatch2,
    reexports::protocols::ext::idle_notify::v1::client::{
        ext_idle_notification_v1::{self as notification, ExtIdleNotificationV1 as Notification},
        ext_idle_notifier_v1::{self as notifier, ExtIdleNotifierV1 as Notifier},
    },
    registry::RegistryHandler,
};
use wayland_client::{
    globals::GlobalList,
    protocol::{wl_output::WlOutput, wl_seat::WlSeat},
    Connection, Proxy, QueueHandle, WEnum,
};
use wayland_protocols_wlr::output_power_management::v1::client::{
    zwlr_output_power_manager_v1::{self as manager, ZwlrOutputPowerManagerV1 as Manager},
    zwlr_output_power_v1::{self as power, ZwlrOutputPowerV1 as Power},
};

pub(super) struct IdleRuntime {
    preferences: IdlePreferences,
    policy: Policy,
    notifier: Option<(u32, Notifier)>,
    manager: Option<(u32, Manager)>,
    generation: u64,
    seats: Vec<Seat>,
    outputs: Vec<Output>,
    rearm: bool,
    refresh: bool,
    connection: Connection,
}

struct Seat {
    seat: WlSeat,
    notifications: Vec<Notification>,
}

struct Output {
    output: WlOutput,
    // Retain a failed output entry until hotplug, resume or explicit reload. Repeated
    // frame/idle callbacks must not fight another client's exclusive control.
    control: Option<Power>,
    state: PowerState,
}

/// Desired mode is separate from the last compositor acknowledgement. In
/// particular, an On request must supersede an in-flight Off even if the most
/// recent acknowledged state is still On. No timer retries a rejected mode.
#[derive(Default)]
struct PowerState {
    observed: Option<bool>,
    requested: Option<bool>,
    desired: Option<bool>,
}

impl PowerState {
    fn observe(&mut self, on: bool) {
        self.observed = Some(on);
        if self.requested == Some(on) {
            self.requested = None;
        }
    }

    fn request(&mut self, on: bool) -> bool {
        if self.observed.is_none() || self.desired == Some(on) {
            return false;
        }
        self.desired = Some(on);
        if self.observed != Some(on) || self.requested.is_some() {
            self.requested = Some(on);
            true
        } else {
            false
        }
    }

    fn needs_restore(&self) -> bool {
        self.observed == Some(false) || self.requested == Some(false) || self.desired == Some(false)
    }
}

impl IdleRuntime {
    pub(super) fn new(
        connection: &Connection,
        globals: &GlobalList,
        qh: &QueueHandle<Runtime>,
        preferences: IdlePreferences,
    ) -> Self {
        let names = globals.contents().clone_list();
        let notifier = names
            .iter()
            .find(|global| global.interface == Notifier::interface().name)
            .and_then(|global| {
                globals
                    .bind::<Notifier, _, _>(qh, 1..=1, ManagerData)
                    .ok()
                    .map(|proxy| (global.name, proxy))
            });
        let manager = names
            .iter()
            .find(|global| global.interface == Manager::interface().name)
            .and_then(|global| {
                globals
                    .bind::<Manager, _, _>(qh, 1..=1, ManagerData)
                    .ok()
                    .map(|proxy| (global.name, proxy))
            });
        if notifier.is_none() && preferences.lock_after_seconds != 0 {
            eprintln!("Automatic lock unavailable: compositor has no ext-idle-notify-v1");
        }
        if manager.is_none() && preferences.screen_off_after_seconds != 0 {
            eprintln!(
                "Idle screen power unavailable: compositor has no wlr-output-power-management-v1"
            );
        }
        Self {
            preferences,
            policy: Policy::new(preferences),
            notifier,
            manager,
            generation: 0,
            seats: Vec::new(),
            outputs: Vec::new(),
            rearm: false,
            refresh: false,
            connection: connection.clone(),
        }
    }

    pub(super) fn add_seat(&mut self, seat: WlSeat, qh: &QueueHandle<Runtime>) {
        if self.seats.iter().any(|binding| binding.seat == seat) {
            return;
        }
        self.policy.add_seat(seat.id().protocol_id());
        let notifications = self.subscribe(&seat, qh);
        self.seats.push(Seat {
            seat,
            notifications,
        });
    }

    fn subscribe(&self, seat: &WlSeat, qh: &QueueHandle<Runtime>) -> Vec<Notification> {
        let Some((_, notifier)) = &self.notifier else {
            return Vec::new();
        };
        [
            (Timer::Lock, self.preferences.lock_after_seconds),
            (Timer::ScreenOff, self.preferences.screen_off_after_seconds),
        ]
        .into_iter()
        .filter_map(|(timer, seconds)| {
            let timeout = seconds.checked_mul(1000).filter(|timeout| *timeout != 0)?;
            Some(notifier.get_idle_notification(
                timeout,
                seat,
                qh,
                NotificationData {
                    seat: seat.clone(),
                    generation: self.generation,
                    timer,
                },
            ))
        })
        .collect()
    }

    pub(super) fn remove_seat(&mut self, seat: &WlSeat) {
        if let Some(index) = self.seats.iter().position(|binding| &binding.seat == seat) {
            let binding = self.seats.remove(index);
            for notification in binding.notifications {
                notification.destroy();
            }
            self.policy.remove_seat(seat.id().protocol_id());
        }
    }

    pub(super) fn add_output(&mut self, output: WlOutput, qh: &QueueHandle<Runtime>) {
        if self.outputs.iter().any(|binding| binding.output == output) {
            return;
        }
        let control = self.manager.as_ref().map(|(_, manager)| {
            manager.get_output_power(
                &output,
                qh,
                OutputData {
                    output: output.clone(),
                },
            )
        });
        self.outputs.push(Output {
            output,
            control,
            state: PowerState::default(),
        });
    }

    pub(super) fn remove_output(&mut self, output: &WlOutput) {
        if let Some(index) = self
            .outputs
            .iter()
            .position(|binding| &binding.output == output)
        {
            // The compositor has removed this output. Never send power requests
            // through a removed wl_output, but release its control object.
            if let Some(control) = self.outputs.remove(index).control {
                control.destroy();
            }
        }
    }

    pub(super) fn receive_lock(&mut self, message: &locking::Message) {
        self.rearm |= self.policy.receive_lock(message);
    }

    pub(super) fn reload(&mut self) {
        self.refresh = true;
    }

    pub(super) fn resume(&mut self) {
        // The pre-sleep compositor interval and output acknowledgement are
        // stale. Recreate both without changing the canonical lock lifecycle.
        self.refresh = true;
        self.rearm = true;
    }

    fn reset_notifications(&mut self, qh: &QueueHandle<Runtime>) {
        // Recreating the protocol object starts a new compositor idle interval.
        // Reject already queued events belonging to the old interval.
        self.generation = self.generation.wrapping_add(1);
        self.policy.reconfigure(self.preferences);
        for seat in &mut self.seats {
            for notification in seat.notifications.drain(..) {
                notification.destroy();
            }
        }
        for index in 0..self.seats.len() {
            let notifications = self.subscribe(&self.seats[index].seat, qh);
            self.seats[index].notifications = notifications;
        }
        self.rearm = false;
    }

    pub(super) fn synchronize(
        &mut self,
        preferences: IdlePreferences,
        lock_busy: bool,
        qh: &QueueHandle<Runtime>,
    ) -> bool {
        let refresh = std::mem::take(&mut self.refresh);
        if self.preferences != preferences || refresh {
            self.preferences = preferences;
            self.rearm = true;
            // A valid reload or observed sleep-cycle end can retry Failed.
            // Invalid config leaves the current interval untouched.
            self.release_outputs();
            for binding in &mut self.outputs {
                binding.state = PowerState::default();
                binding.control = self.manager.as_ref().map(|(_, manager)| {
                    manager.get_output_power(
                        &binding.output,
                        qh,
                        OutputData {
                            output: binding.output.clone(),
                        },
                    )
                });
            }
        }
        if self.rearm {
            self.reset_notifications(qh);
        }
        let decision = self.policy.evaluate(lock_busy);
        for binding in &mut self.outputs {
            let Some(control) = &binding.control else {
                continue;
            };
            if binding.state.request(!decision.screen_off) {
                control.set_mode(if decision.screen_off {
                    power::Mode::Off
                } else {
                    power::Mode::On
                });
            }
            // Disabled power policy still reconciles an already-Off screen
            // once on startup/reload, then releases exclusive ownership. This
            // also recovers a prior Shell process which died while blanked.
            if self.preferences.screen_off_after_seconds == 0
                && binding.state.observed == Some(true)
                && binding.state.requested.is_none()
            {
                if let Some(control) = binding.control.take() {
                    control.destroy();
                }
            }
        }
        decision.request_lock
    }

    fn release_outputs(&mut self) {
        for binding in &mut self.outputs {
            if let Some(control) = binding.control.take() {
                if binding.state.needs_restore() {
                    control.set_mode(power::Mode::On);
                }
                control.destroy();
            }
        }
    }
}

impl Drop for IdleRuntime {
    fn drop(&mut self) {
        // Destroying an output-power object does not promise to turn the
        // display back on. Restore first on orderly shutdown/error paths.
        self.release_outputs();
        let _ = self.connection.flush();
    }
}

struct ManagerData;
impl Dispatch2<Notifier, Runtime> for ManagerData {
    fn event(
        &self,
        _: &mut Runtime,
        _: &Notifier,
        _: notifier::Event,
        _: &Connection,
        _: &QueueHandle<Runtime>,
    ) {
    }
}
impl Dispatch2<Manager, Runtime> for ManagerData {
    fn event(
        &self,
        _: &mut Runtime,
        _: &Manager,
        _: manager::Event,
        _: &Connection,
        _: &QueueHandle<Runtime>,
    ) {
    }
}

struct NotificationData {
    seat: WlSeat,
    generation: u64,
    timer: Timer,
}
impl Dispatch2<Notification, Runtime> for NotificationData {
    fn event(
        &self,
        runtime: &mut Runtime,
        _: &Notification,
        event: notification::Event,
        _: &Connection,
        _: &QueueHandle<Runtime>,
    ) {
        let idle = &mut runtime.idle;
        if self.generation != idle.generation
            || !idle.seats.iter().any(|binding| binding.seat == self.seat)
        {
            return;
        }
        let idled = match event {
            notification::Event::Idled => true,
            notification::Event::Resumed => false,
            _ => return,
        };
        idle.policy
            .notify(self.seat.id().protocol_id(), self.timer, idled);
    }
}

struct OutputData {
    output: WlOutput,
}
impl Dispatch2<Power, Runtime> for OutputData {
    fn event(
        &self,
        runtime: &mut Runtime,
        proxy: &Power,
        event: power::Event,
        _: &Connection,
        _: &QueueHandle<Runtime>,
    ) {
        let Some(binding) = runtime.idle.outputs.iter_mut().find(|binding| {
            binding.output == self.output && binding.control.as_ref() == Some(proxy)
        }) else {
            return;
        };
        match event {
            power::Event::Mode {
                mode: WEnum::Value(mode),
            } => match mode {
                power::Mode::On => binding.state.observe(true),
                power::Mode::Off => binding.state.observe(false),
                _ => {}
            },
            power::Event::Failed => {
                eprintln!(
                    "Idle screen power unavailable for output {}: compositor rejected control",
                    self.output.id().protocol_id()
                );
                if let Some(control) = binding.control.take() {
                    control.destroy();
                }
            }
            _ => {}
        }
    }
}

impl RegistryHandler<Runtime> for IdleRuntime {
    fn new_global(
        runtime: &mut Runtime,
        _: &Connection,
        qh: &QueueHandle<Runtime>,
        name: u32,
        interface: &str,
        _: u32,
    ) {
        if interface == Notifier::interface().name && runtime.idle.notifier.is_none() {
            if let Ok(notifier) =
                runtime
                    .registry
                    .bind_specific::<Notifier, _, _>(qh, name, 1..=1, ManagerData)
            {
                runtime.idle.notifier = Some((name, notifier));
                runtime.idle.rearm = true;
            }
        }
        if interface == Manager::interface().name && runtime.idle.manager.is_none() {
            if let Ok(manager) =
                runtime
                    .registry
                    .bind_specific::<Manager, _, _>(qh, name, 1..=1, ManagerData)
            {
                for binding in &mut runtime.idle.outputs {
                    binding.state = PowerState::default();
                    binding.control = Some(manager.get_output_power(
                        &binding.output,
                        qh,
                        OutputData {
                            output: binding.output.clone(),
                        },
                    ));
                }
                runtime.idle.manager = Some((name, manager));
            }
        }
    }

    fn remove_global(
        runtime: &mut Runtime,
        _: &Connection,
        qh: &QueueHandle<Runtime>,
        name: u32,
        _: &str,
    ) {
        if runtime
            .idle
            .notifier
            .as_ref()
            .is_some_and(|(current, _)| *current == name)
        {
            if let Some((_, notifier)) = runtime.idle.notifier.take() {
                notifier.destroy();
            }
            runtime.idle.reset_notifications(qh);
            eprintln!("Automatic lock unavailable: compositor removed idle notifier");
        }
        if runtime
            .idle
            .manager
            .as_ref()
            .is_some_and(|(current, _)| *current == name)
        {
            runtime.idle.release_outputs();
            if let Some((_, manager)) = runtime.idle.manager.take() {
                manager.destroy();
            }
            eprintln!("Idle screen power unavailable: compositor removed output power manager");
        }
    }
}

#[cfg(test)]
#[path = "unit_tests/idle.rs"]
mod tests;
