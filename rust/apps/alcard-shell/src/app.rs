//! Shell application orchestration, independent of window/event-loop types.
use crate::{
    battery, battery_journal, controls, desktop,
    i18n::Locale,
    launch,
    layer::Banner,
    locking,
    model::{
        Action, ApplicationView, ControlsRequest, Dialog, Key, LaunchPhase, LaunchStatus, MenuApp,
        RadioRequest, Request, Screen, Shell, SystemRequest, SystemView, WindowInfo,
        WindowOperation, WindowOperationStatus,
    },
    notifications::{self, NotificationEvent, Snapshot},
    radios, sleep,
    sleep_intent::{Coordinator as SleepCoordinator, Effect as SleepEffect, LockPurpose},
    status::{BatteryObservation, StatusSnapshot},
    system,
};
use alcard_ui::{
    dialog::TextInput,
    preferences::{BatteryPreferences, ConfigError, ConfigSource, IdlePreferences, Language},
};
use chrono::{Local, Timelike};
use std::time::{Duration, Instant};

const WINDOW_RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);
const BATTERY_REFRESH_TIMEOUT: Duration = Duration::from_secs(8);

struct BatterySleep {
    episode: u64,
    request: Option<u64>,
    refresh_deadline: Instant,
    sample_barrier: Option<Instant>,
}

#[derive(Clone, Copy)]
struct BatteryWarning {
    level: battery::Level,
    percent: u8,
    episode: u64,
    failure: Option<system::Failure>,
}

struct PendingWindowOperation {
    id: u64,
    kind: WindowOperation,
    deadline: Option<Instant>,
}

#[derive(PartialEq, Eq)]
pub struct InputAnchor {
    screen: Screen,
    dialog: Option<Dialog>,
    dialog_target: Option<(u64, String)>,
    search_input: Option<TextInput>,
    system_view: Option<SystemView>,
    system_request: Option<SystemRequest>,
    system_snapshot: Option<system::Snapshot>,
    controls_request: Option<ControlsRequest>,
    controls_snapshot: Option<controls::Snapshot>,
    controls_failure: Option<controls::Failure>,
    radios_request: Option<RadioRequest>,
    radios_snapshot: Option<radios::Snapshot>,
    radios_failure: Option<radios::Failure>,
    radio_entries: Option<Vec<radios::Entry>>,
    sleep_snapshot: Option<sleep::Snapshot>,
    sleep_progress: Option<sleep::Progress>,
    lock_state: Option<locking::State>,
    view: ApplicationView,
    actions: [Action; 5],
    window: Option<(u64, String)>,
    desktop: Option<String>,
}

pub struct Application {
    pub state: Shell,
    pub dirty: bool,
    pub clock: String,
    desktop_apps: Vec<desktop::DesktopApp>,
    launches: launch::Tracker,
    launch_snapshot: Vec<launch::LaunchStatus>,
    preferences: Option<ConfigSource>,
    idle_preferences: IdlePreferences,
    battery_preferences: BatteryPreferences,
    battery_policy: battery::Policy,
    battery_observation: Option<BatteryObservation>,
    battery_warning: Option<BatteryWarning>,
    battery_failure: Option<(u64, system::Failure)>,
    battery_sleep: Option<BatterySleep>,
    battery_refresh_requested: bool,
    battery_journal: Option<battery_journal::Journal>,
    battery_journal_inhibit: Option<battery::Inhibit>,
    config_error: Option<ConfigError>,
    pending_config_notice: bool,
    window_operations: Vec<PendingWindowOperation>,
    focused: bool,
    input_generation: u64,
    launch_intent: Option<(String, u64, u64)>,
    notifications: Option<notifications::Client>,
    system: Option<system::Client>,
    controls: Option<controls::Client>,
    radios: Option<radios::Client>,
    sleep_client: Option<sleep::Client>,
    sleep_coordinator: SleepCoordinator,
    resume_requested: bool,
    locking: Option<locking::Client>,
    automatic_lock_request: Option<u64>,
    banner: Option<(NotificationEvent, Instant)>,
    notification_generation: u64,
    last_date: String,
    authentication_active: bool,
    lock_presence: bool,
    lock_journal: bool,
}
impl Application {
    pub fn new(mut state: Shell) -> Self {
        let desktop_apps = desktop::discover(state.locale);
        state.set_applications(
            desktop_apps
                .iter()
                .map(|app| MenuApp {
                    desktop_id: app.id.clone(),
                    name: app.name.clone(),
                    icon: app.icon.clone(),
                    startup_wm_class: app.startup_wm_class.clone(),
                })
                .collect(),
        );
        Self {
            state,
            dirty: true,
            clock: Local::now().format("%H:%M").to_string(),
            last_date: Local::now().format("%Y-%m-%d").to_string(),
            desktop_apps,
            launches: launch::Tracker::new(),
            launch_snapshot: Vec::new(),
            preferences: None,
            idle_preferences: IdlePreferences::default(),
            battery_preferences: BatteryPreferences::default(),
            battery_policy: battery::Policy::new(BatteryPreferences::default()),
            battery_observation: None,
            battery_warning: None,
            battery_failure: None,
            battery_sleep: None,
            battery_refresh_requested: false,
            battery_journal: None,
            battery_journal_inhibit: None,
            config_error: None,
            pending_config_notice: false,
            window_operations: Vec::new(),
            focused: false,
            input_generation: 0,
            launch_intent: None,
            notifications: None,
            system: None,
            controls: None,
            radios: None,
            sleep_client: None,
            sleep_coordinator: SleepCoordinator::default(),
            resume_requested: false,
            locking: None,
            automatic_lock_request: None,
            banner: None,
            notification_generation: 0,
            authentication_active: true,
            lock_presence: false,
            lock_journal: false,
        }
    }
    pub fn set_notifications(&mut self, client: notifications::Client) {
        self.notifications = Some(client);
        if self.battery_warning.is_some() {
            self.publish_battery_warning(false);
        }
        if std::mem::take(&mut self.pending_config_notice) {
            if let Some(error) = &self.config_error {
                self.configuration_notice(error);
            }
        }
    }

    pub fn set_system(&mut self, client: system::Client) {
        self.system = Some(client);
    }

    pub fn set_controls(&mut self, client: controls::Client) {
        self.controls = Some(client);
    }

    pub fn receive_controls(&mut self, message: controls::Message) {
        if self.state.controls_message(message) {
            // A reply changes data only. Never reopen a panel or request focus.
            self.dirty = true;
        }
    }

    pub fn set_radios(&mut self, client: radios::Client) {
        self.radios = Some(client);
    }

    pub fn receive_radios(&mut self, message: radios::Message) {
        if self.state.radios_message(message) {
            self.dirty = true;
        }
    }

    fn submit_control(&mut self, request: Request) {
        let accepted = match request {
            Request::RefreshControls { request } => self
                .controls
                .as_ref()
                .is_some_and(|client| client.refresh(request)),
            Request::AdjustControl { request, operation } => self
                .controls
                .as_ref()
                .is_some_and(|client| client.execute(request, operation)),
            Request::None => true,
            _ => unreachable!("only a controls request"),
        };
        if !accepted {
            self.state.request_failed(request);
        }
    }

    fn submit_radio(&mut self, request: Request) {
        let accepted = match request {
            Request::RadioOperation {
                request,
                operation: radios::Operation::Refresh,
            } => self
                .radios
                .as_ref()
                .is_some_and(|client| client.refresh(request)),
            Request::RadioOperation { request, operation } => self
                .radios
                .as_ref()
                .is_some_and(|client| client.execute(request, operation)),
            Request::None => true,
            _ => unreachable!("only a radio request"),
        };
        if !accepted {
            self.state.request_failed(request);
        }
    }

    pub fn set_sleep(&mut self, client: sleep::Client) {
        self.sleep_client = Some(client);
    }

    pub fn receive_sleep(&mut self, message: sleep::Message) {
        let effects = self.sleep_coordinator.receive(message, Instant::now());
        self.sync_sleep_view();
        self.apply_sleep_effects(effects);
        if let sleep::Message::Refreshed { request, snapshot } = message {
            if self.battery_sleep.as_ref().is_some_and(|automatic| {
                automatic.episode == request && automatic.request.is_none()
            }) {
                if snapshot.monitor == sleep::MonitorState::Ready
                    && snapshot.capability == system::Capability::Allowed
                    && self
                        .battery_policy
                        .is_action_current(request, Instant::now())
                {
                    let Request::PrepareSuspend { request: id } =
                        self.state.begin_battery_suspend()
                    else {
                        self.finish_battery_sleep(Some(system::Failure::Blocked), false);
                        return;
                    };
                    self.battery_sleep
                        .as_mut()
                        .expect("matching battery episode")
                        .request = Some(id);
                    self.begin_suspend(id);
                } else {
                    self.finish_battery_sleep(Some(system::Failure::Unavailable), false);
                }
            }
        } else if matches!(message, sleep::Message::Lost { .. }) && self.battery_sleep.is_some() {
            self.finish_battery_sleep(Some(system::Failure::Unavailable), true);
        }
    }

    fn sync_sleep_view(&mut self) {
        self.dirty |= self
            .state
            .set_sleep_snapshot(self.sleep_coordinator.snapshot());
        self.dirty |= self
            .state
            .set_sleep_progress(self.sleep_coordinator.progress());
        let terminal = self.battery_sleep.as_ref().and_then(|automatic| {
            self.sleep_coordinator.progress().filter(|progress| {
                automatic.request == Some(progress.request)
                    && matches!(
                        progress.stage,
                        sleep::Stage::Failed
                            | sleep::Stage::Unknown
                            | sleep::Stage::Cancelled
                            | sleep::Stage::Ended
                    )
            })
        });
        if let Some(progress) = terminal {
            let failure = progress
                .failure
                .filter(|failure| *failure != system::Failure::Cancelled);
            self.finish_battery_sleep(failure, false);
        }
    }

    fn refresh_sleep(&mut self) {
        if let Some(client) = &self.sleep_client {
            // A busy observer can coalesce/refuse this read-only request.
            // Retain its established context; never reconnect and mutate.
            let _ = client.refresh();
            self.sync_sleep_view();
        } else {
            self.receive_sleep(sleep::Message::Snapshot(sleep::Snapshot {
                epoch: self.sleep_coordinator.snapshot().epoch,
                capability: system::Capability::Unavailable,
                monitor: sleep::MonitorState::Unavailable,
                delay_ms: 0,
            }));
        }
    }

    fn begin_suspend(&mut self, request: u64) {
        if !self.sleep_coordinator.begin(request, Instant::now()) {
            self.state
                .request_failed(Request::PrepareSuspend { request });
            if self
                .battery_sleep
                .as_ref()
                .is_some_and(|automatic| automatic.request == Some(request))
            {
                self.finish_battery_sleep(Some(system::Failure::Unavailable), false);
            }
            return;
        }
        if !self
            .sleep_client
            .as_ref()
            .is_some_and(|client| client.prepare(request))
        {
            self.sleep_coordinator
                .submission_failed(request, system::Failure::Unavailable);
        }
        self.sync_sleep_view();
    }

    fn cancel_suspend(&mut self, request: u64) {
        let effects = self.sleep_coordinator.cancel(request);
        self.apply_sleep_effects(effects);
    }

    fn apply_sleep_effects(&mut self, effects: Vec<SleepEffect>) {
        for effect in effects {
            match effect {
                SleepEffect::NeedLock(purpose) => {
                    let now = Instant::now();
                    if let Some(state) = self
                        .state
                        .lock_state
                        .filter(|state| state.phase != locking::Phase::Failed)
                    {
                        // Only a canonical lifecycle already observed by the
                        // model can be reused. Presence/journal never enter here.
                        self.sleep_coordinator.lock_started(purpose, state.id, now);
                        self.sleep_coordinator
                            .receive_lock(locking::Message::Changed(state));
                        continue;
                    }
                    let own_request = match purpose {
                        LockPurpose::Own(request) => Some(request),
                        LockPurpose::External { .. } => None,
                    };
                    let request = self.state.begin_sleep_lock(own_request);
                    if let Request::LockSession { request: id } = request {
                        self.sleep_coordinator.lock_started(purpose, id, now);
                        self.new_intent();
                        if !self.submit_lock(id) {
                            self.sleep_coordinator.lock_failed(purpose);
                            if let (Some(client), Some(request)) = (&self.sleep_client, own_request)
                            {
                                client.cancel(request);
                            }
                        }
                    } else {
                        self.sleep_coordinator.lock_failed(purpose);
                        if let (Some(client), Some(request)) = (&self.sleep_client, own_request) {
                            client.cancel(request);
                        }
                    }
                }
                SleepEffect::Commit {
                    request,
                    epoch,
                    permit,
                } => {
                    let permit = if self
                        .battery_sleep
                        .as_ref()
                        .is_some_and(|automatic| automatic.request == Some(request))
                    {
                        let Some(observation) = self.battery_observation else {
                            permit.revoke();
                            self.finish_battery_sleep(Some(system::Failure::Unavailable), true);
                            continue;
                        };
                        // Worker-side expiry remains effective if the UI stops
                        // dispatching while login1 preflight is still pending.
                        permit.until(observation.observed_at + battery::MAX_SAMPLE_AGE)
                    } else {
                        permit
                    };
                    if !self
                        .sleep_client
                        .as_ref()
                        .is_some_and(|client| client.commit(request, epoch, permit))
                    {
                        self.sleep_coordinator
                            .submission_failed(request, system::Failure::Unavailable);
                        if let Some(client) = &self.sleep_client {
                            client.cancel(request);
                        }
                    }
                }
                SleepEffect::ReleaseDelay {
                    epoch,
                    cycle,
                    permit,
                } => {
                    if !self
                        .sleep_client
                        .as_ref()
                        .is_some_and(|client| client.lock_ready(epoch, cycle, permit))
                    {
                        eprintln!("Sleep delay release unavailable for the current cycle");
                    }
                }
                SleepEffect::Cancel { request } => {
                    if let Some(client) = &self.sleep_client {
                        client.cancel(request);
                    }
                }
                SleepEffect::Resume => {
                    self.resume_requested = true;
                    let effects = self.battery_policy.resume(Instant::now());
                    self.apply_battery_effects(effects);
                    self.sync_battery_journal();
                    self.battery_refresh_requested = true;
                    self.battery_observation = None;
                    self.new_intent();
                    self.banner = None;
                    self.dirty = true;
                }
            }
        }
        self.sync_sleep_view();
    }

    /// Called only after a complete dispatch batch. A LOCKED event followed by
    /// UNLOCKED/Uncertain in that batch cannot issue a suspend or release delay.
    pub fn flush_sleep_intent(&mut self) {
        let now = Instant::now();
        let effects = self.battery_policy.tick(now);
        self.apply_battery_effects(effects);
        let allow_commit = self.battery_commit_ready(now);
        let effects = if allow_commit {
            self.sleep_coordinator.flush(now)
        } else {
            self.sleep_coordinator.flush_when(now, false)
        };
        self.sync_sleep_view();
        self.apply_sleep_effects(effects);
    }

    pub fn take_resume_request(&mut self) -> bool {
        std::mem::take(&mut self.resume_requested)
    }

    pub fn set_locking(&mut self, client: locking::Client) {
        // This synchronous initial snapshot suppresses activation on the very
        // first frame after Shell restart, before channel callbacks run.
        self.lock_journal = client.present();
        self.locking = Some(client);
    }

    pub fn idle_preferences(&self) -> IdlePreferences {
        self.idle_preferences
    }

    pub fn set_idle_preferences(&mut self, preferences: IdlePreferences) {
        self.idle_preferences = preferences;
    }

    pub fn set_battery_preferences(&mut self, preferences: BatteryPreferences) {
        self.battery_preferences = preferences;
        self.dirty |= self.state.battery_preferences != preferences;
        self.state.battery_preferences = preferences;
        let effects = self.battery_policy.reconfigure(preferences, Instant::now());
        self.apply_battery_effects(effects);
        self.battery_refresh_requested = true;
    }

    /// Restore the per-login guard before any real observations are delivered.
    /// Failure disables automatic actions while leaving all warnings available.
    pub fn initialize_battery_journal(&mut self) {
        match battery_journal::Journal::from_environment() {
            Ok(journal) => {
                let inhibit = journal.inhibit();
                self.battery_journal = Some(journal);
                self.battery_journal_inhibit = inhibit;
                if let Some(inhibit) = inhibit {
                    let effects = self.battery_policy.restore_inhibit(inhibit, Instant::now());
                    self.apply_battery_effects(effects);
                }
            }
            Err(error) => eprintln!("Battery automatic-action journal unavailable: {error}"),
        }
    }

    pub fn observe_battery(&mut self, observation: BatteryObservation) {
        let now = Instant::now();
        if self
            .battery_observation
            .is_none_or(|old| observation.sequence > old.sequence)
        {
            self.battery_observation = Some(observation);
        }
        let effects = self.battery_policy.observe(observation, now);
        self.apply_battery_effects(effects);
        self.sync_battery_journal();
    }

    fn apply_battery_effects(&mut self, effects: Vec<battery::Effect>) {
        for effect in effects {
            match effect {
                battery::Effect::Warn {
                    level,
                    percent,
                    episode,
                    announce,
                } => {
                    let failure = self
                        .battery_failure
                        .filter(|(previous, _)| *previous == episode)
                        .map(|(_, failure)| failure);
                    if self
                        .battery_failure
                        .is_some_and(|(previous, _)| previous != episode)
                    {
                        self.battery_failure = None;
                    }
                    self.battery_warning = Some(BatteryWarning {
                        level,
                        percent,
                        episode,
                        failure,
                    });
                    self.publish_battery_warning(announce);
                }
                battery::Effect::ClearWarning => {
                    self.battery_warning = None;
                    self.publish_battery_warning(false);
                }
                battery::Effect::RequestSuspend { episode } => {
                    self.request_battery_sleep(episode);
                }
                battery::Effect::CancelSuspend { episode } => {
                    if self
                        .battery_sleep
                        .as_ref()
                        .is_some_and(|automatic| automatic.episode == episode)
                    {
                        self.finish_battery_sleep(None, true);
                    }
                }
            }
        }
    }

    fn sync_battery_journal(&mut self) {
        let desired = self.battery_policy.inhibit();
        if desired == self.battery_journal_inhibit {
            return;
        }
        let Some(journal) = &self.battery_journal else {
            return;
        };
        let result = match desired {
            None => journal.clear_if(self.battery_journal_inhibit),
            Some(battery::Inhibit::Resume) => journal.resume().map(|_| true),
            Some(battery::Inhibit::Attempt) => journal.latch(),
        };
        let actual = journal.inhibit();
        if let Err(error) = result {
            eprintln!("Battery automatic-action guard could not be persisted: {error}");
            // Keep the failed guard unavailable until the next login. Never
            // interpret a broken journal as permission to retry an action.
            self.battery_journal = None;
        } else {
            self.battery_journal_inhibit = actual;
            if actual != desired {
                if let Some(inhibit) = actual {
                    let effects = self.battery_policy.restore_inhibit(inhibit, Instant::now());
                    self.apply_battery_effects(effects);
                }
            }
        }
    }

    fn request_battery_sleep(&mut self, episode: u64) {
        if self.battery_sleep.is_some()
            || !self
                .battery_policy
                .is_action_current(episode, Instant::now())
        {
            self.battery_policy.action_finished(episode);
            return;
        }
        let recorded = self.battery_journal.as_ref().map(|journal| journal.latch());
        match recorded {
            Some(Ok(true)) => self.battery_journal_inhibit = Some(battery::Inhibit::Attempt),
            _ => {
                self.battery_policy.action_finished(episode);
                self.battery_sleep_failed(episode, system::Failure::Unavailable);
                return;
            }
        }
        self.battery_sleep = Some(BatterySleep {
            episode,
            request: None,
            refresh_deadline: Instant::now() + BATTERY_REFRESH_TIMEOUT,
            sample_barrier: None,
        });
        // Refresh is correlated with this episode. The eventual prepare and
        // commit each recheck permissions; an old Allowed snapshot is not proof.
        if !self
            .sleep_client
            .as_ref()
            .is_some_and(|client| client.refresh_for(episode))
        {
            self.finish_battery_sleep(Some(system::Failure::Unavailable), false);
        }
    }

    fn finish_battery_sleep(&mut self, failure: Option<system::Failure>, cancel: bool) {
        let Some(automatic) = self.battery_sleep.take() else {
            return;
        };
        self.battery_policy.action_finished(automatic.episode);
        if cancel {
            if let Some(request) = automatic.request {
                self.cancel_suspend(request);
            }
        }
        if let Some(failure) = failure {
            self.battery_sleep_failed(automatic.episode, failure);
        }
    }

    fn battery_sleep_failed(&mut self, episode: u64, failure: system::Failure) {
        self.battery_failure = Some((episode, failure));
        if let Some(warning) = self
            .battery_warning
            .as_mut()
            .filter(|warning| warning.episode == episode)
        {
            warning.failure = Some(failure);
            self.publish_battery_warning(true);
        }
    }

    /// Start a new sensor read only after the canonical lock was observed.
    /// The coordinator still validates the exact lock at the final flush.
    fn battery_commit_ready(&mut self, now: Instant) -> bool {
        let Some(automatic) = self.battery_sleep.as_ref() else {
            return true;
        };
        if !self
            .battery_policy
            .is_action_current(automatic.episode, now)
        {
            self.finish_battery_sleep(None, true);
            return false;
        }
        let locking = self.sleep_coordinator.progress().is_some_and(|progress| {
            automatic.request == Some(progress.request) && progress.stage == sleep::Stage::Locking
        });
        if !locking {
            return true;
        }
        if !self
            .state
            .lock_state
            .is_some_and(|state| state.phase == locking::Phase::Locked)
        {
            return false;
        }
        let automatic = self
            .battery_sleep
            .as_mut()
            .expect("known automatic request");
        let Some(barrier) = automatic.sample_barrier else {
            automatic.sample_barrier = Some(now);
            self.battery_refresh_requested = true;
            return false;
        };
        self.battery_observation
            .is_some_and(|observation| observation.observed_at >= barrier)
    }

    pub fn take_battery_refresh_request(&mut self) -> bool {
        std::mem::take(&mut self.battery_refresh_requested)
    }

    pub fn battery_refresh_failed(&mut self) {
        self.finish_battery_sleep(Some(system::Failure::Unavailable), true);
    }

    fn publish_battery_warning(&self, announce: bool) {
        let Some(client) = &self.notifications else {
            return;
        };
        let notice = self.battery_warning.map(|warning| {
            let critical = warning.level == battery::Level::Critical;
            let title = if critical {
                localized(self.state.locale, "Критический заряд", "Critical battery", "Batería crítica")
            } else {
                localized(self.state.locale, "Низкий заряд", "Low battery", "Batería baja")
            };
            let charge = localized(self.state.locale, "Заряд", "Charge", "Carga");
            let connect = localized(self.state.locale, "Подключите питание.", "Connect a charger.", "Conecte el cargador.");
            let mut body = format!("{charge}: {}%. {connect}", warning.percent);
            if let Some(failure) = warning.failure.filter(|_| critical) {
                let detail = if failure == system::Failure::Unknown {
                    localized(self.state.locale, "Результат автосна неизвестен. Автоматический повтор отключён.", "The sleep result is unknown. Automatic retry is disabled.", "No se conoce el resultado de la suspensión. No se reintentará automáticamente.")
                } else {
                    localized(self.state.locale, "Автосон недоступен. Сохраните работу; при необходимости выключите устройство вручную.", "Automatic sleep is unavailable. Save your work; shut down manually if needed.", "La suspensión automática no está disponible. Guarde su trabajo; apague el equipo manualmente si hace falta.")
                };
                body.push('\n');
                body.push_str(detail);
            } else if critical && self.battery_preferences.suspend_on_critical {
                body.push('\n');
                body.push_str(localized(self.state.locale, "Автосон включён. Повтор после пробуждения не выполняется.", "Automatic sleep is enabled. Waking does not repeat it.", "La suspensión automática está activada. No se repite al despertar."));
            }
            notifications::BatteryNotice { title: title.into(), body, critical, announce }
        });
        if let Err(error) = client.request(notifications::Command::Battery(notice)) {
            eprintln!("Could not record battery notification: {error}");
        }
    }

    /// Idle belongs to the whole seat, so this path does not depend on Shell
    /// focus and never opens Home, a status card, or a new activation request.
    pub fn request_idle_lock(&mut self) -> bool {
        if self.lock_inhibits_activation() || self.state.system_busy() {
            return false;
        }
        let Request::LockSession { request } = self.state.begin_lock(false) else {
            return false;
        };
        self.automatic_lock_request = Some(request);
        self.new_intent();
        self.dirty = true;
        self.submit_lock(request)
    }

    fn submit_lock(&mut self, id: u64) -> bool {
        self.banner = None;
        self.launch_intent = None;
        self.sleep_coordinator
            .receive_lock(locking::Message::Changed(locking::State {
                id,
                phase: locking::Phase::Starting,
            }));
        let accepted = self
            .locking
            .as_ref()
            .is_some_and(|client| client.lock(id, self.state.locale.code()));
        if !accepted {
            self.state
                .request_failed(Request::LockSession { request: id });
            self.report_idle_lock_failure(id);
            self.sleep_coordinator
                .receive_lock(locking::Message::Changed(locking::State {
                    id,
                    phase: locking::Phase::Failed,
                }));
        }
        self.sync_sleep_view();
        accepted
    }

    fn report_idle_lock_failure(&mut self, id: u64) {
        if self.automatic_lock_request != Some(id) {
            return;
        }
        // Consume the exact automatic request before publishing. Duplicate or
        // late lifecycle packets must not create more notices or steal focus.
        self.automatic_lock_request = None;
        let title = localized(
            self.state.locale,
            "Автоблокировка",
            "Automatic lock",
            "Bloqueo automático",
        );
        let body = localized(
            self.state.locale,
            "Не удалось запустить автоблокировку. Экран остаётся включённым. Повторите блокировку вручную через меню быстрых действий.",
            "Automatic lock could not start. The screen stays on. Retry Lock from Quick settings.",
            "No se pudo iniciar el bloqueo automático. La pantalla permanece encendida. Vuelva a intentar el bloqueo desde el menú de acciones rápidas.",
        );
        self.internal_notice(title.into(), body.into());
    }

    pub fn lock_inhibits_shell(&self) -> bool {
        self.lock_presence
            || self.lock_journal
            || self
                .state
                .lock_state
                .is_some_and(|state| state.phase == locking::Phase::Locked)
    }

    pub fn lock_inhibits_activation(&self) -> bool {
        // Home IPC is out-of-band; it must not queue activation while a lock
        // is starting or after a crash with an unconfirmed compositor state.
        self.lock_presence || self.lock_journal || self.state.lock_busy()
    }

    pub fn receive_lock(&mut self, message: locking::Message) {
        // Revocation is immediate even for a commit already queued on the
        // worker; a new commit is only considered by the end-of-batch flush.
        self.sleep_coordinator.receive_lock(message);
        let changed = match message {
            locking::Message::Presence(active) => {
                let changed = self.lock_presence != active;
                self.lock_presence = active;
                changed
            }
            locking::Message::Journal(active) => {
                let changed = self.lock_journal != active;
                self.lock_journal = active;
                changed
            }
            locking::Message::Changed(state) => {
                let changed = self.state.lock_changed(state);
                if changed && state.phase == locking::Phase::Failed {
                    self.report_idle_lock_failure(state.id);
                }
                changed
            }
            locking::Message::Unlocked { id } => {
                let changed = self.state.lock_unlocked(id);
                if changed && self.automatic_lock_request == Some(id) {
                    self.automatic_lock_request = None;
                }
                changed
            }
        };
        if changed {
            self.banner = None;
            self.launch_intent = None;
            self.dirty = true;
        }
        self.sync_sleep_view();
    }

    pub fn receive_system(&mut self, message: system::Message) {
        match message {
            system::Message::Snapshot(snapshot) => {
                if self.state.system_snapshot != snapshot {
                    self.state.sync_system(snapshot);
                    self.dirty = true;
                }
            }
            system::Message::Finished {
                request,
                operation,
                result,
            } => {
                if self.state.system_finished(request, operation, result) {
                    self.dirty = true;
                }
            }
        }
    }

    pub fn set_preferences(&mut self, source: ConfigSource, startup_error: Option<ConfigError>) {
        self.preferences = Some(source);
        if let Some(error) = startup_error {
            self.configuration_failed(error);
        }
    }

    /// Called by the runtime's explicit SIGHUP source, never by a polling timer.
    /// Parsing and theme resolution finish before any live preference changes.
    pub fn reload_preferences(&mut self) -> bool {
        let Some(source) = &self.preferences else {
            return false;
        };
        let candidate = match source.load() {
            Ok(candidate) => candidate,
            Err(error) => {
                self.configuration_failed(error);
                return false;
            }
        };
        let locale = resolve_locale(candidate.locale);
        let applications = (locale != self.state.locale).then(|| desktop::discover(locale));
        self.state.apply_preferences(locale, candidate.theme);
        self.idle_preferences = candidate.idle;
        self.set_battery_preferences(candidate.battery);
        if let Some(applications) = applications {
            self.desktop_apps = applications;
            self.state.set_applications(
                self.desktop_apps
                    .iter()
                    .map(|app| MenuApp {
                        desktop_id: app.id.clone(),
                        name: app.name.clone(),
                        icon: app.icon.clone(),
                        startup_wm_class: app.startup_wm_class.clone(),
                    })
                    .collect(),
            );
        }
        // Re-translate existing diagnostics without replaying failure events.
        self.state.sync_launches(self.localized_launches());
        self.config_error = None;
        self.pending_config_notice = false;
        if self.battery_warning.is_some() {
            self.publish_battery_warning(false);
        }
        crate::icon_cache::clear();
        self.dirty = true;
        true
    }

    fn configuration_failed(&mut self, error: ConfigError) {
        if self.config_error.as_ref() == Some(&error) {
            return;
        }
        eprintln!("Shell preferences were not applied: {error}");
        self.config_error = Some(error.clone());
        if self.notifications.is_some() {
            self.configuration_notice(&error);
        } else {
            self.pending_config_notice = true;
        }
    }

    fn configuration_notice(&self, error: &ConfigError) {
        let title = localized(
            self.state.locale,
            "Настройки Shell",
            "Shell settings",
            "Configuración de Shell",
        );
        let summary = localized(
            self.state.locale,
            "Неверные настройки не применены. Shell продолжает работу с корректными значениями.",
            "Invalid settings were not applied. Shell continues with valid preferences.",
            "No se aplicó la configuración no válida. Shell conserva valores válidos.",
        );
        self.internal_notice(title.into(), format!("{summary}\n{error}"));
    }
    pub fn status(&mut self, status: StatusSnapshot) {
        if self.state.status != status {
            self.state.status = status;
            self.dirty = true;
        }
    }
    fn refresh_applications(&mut self) {
        self.desktop_apps = desktop::discover(self.state.locale);
        self.state.set_applications(
            self.desktop_apps
                .iter()
                .map(|app| MenuApp {
                    desktop_id: app.id.clone(),
                    name: app.name.clone(),
                    icon: app.icon.clone(),
                    startup_wm_class: app.startup_wm_class.clone(),
                })
                .collect(),
        );
    }
    pub fn home(&mut self) {
        if self.lock_inhibits_activation() {
            return;
        }
        if let Some(progress) = self.sleep_coordinator.progress().filter(|progress| {
            matches!(
                progress.stage,
                sleep::Stage::Preparing | sleep::Stage::Locking
            )
        }) {
            self.cancel_suspend(progress.request);
        }
        self.new_intent();
        self.state.go_home();
        self.dirty = true;
    }
    pub fn failed(&mut self, request: Request) {
        if let Request::ActivateWindow(id) | Request::CloseWindow(id) = request {
            self.window_operations
                .retain(|operation| operation.id != id);
            self.sync_window_operations();
        }
        self.state.request_failed(request);
        if self.state.dialog.is_some() {
            self.banner = None;
        }
        self.dirty = true;
    }

    /// Window requests are handled by the same Wayland runtime as our surfaces.
    /// Launch and notification requests never become shell-interpreted commands.
    pub fn apply(&mut self, key: Key) -> Request {
        if self.lock_inhibits_shell() {
            return Request::None;
        }
        self.new_intent();
        if self.state.screen == Screen::Home
            && self.state.dialog.is_none()
            && self.state.system_view.is_none()
            && key == Key::Soft(2)
        {
            self.refresh_applications();
        }
        let request = self.state.on_key(key);
        if self.suppress_banner() {
            // A later return from a dialog must not replay an old banner.
            self.banner = None;
        }
        self.dirty = true;
        match &request {
            Request::LockSession { request: id } => {
                self.submit_lock(*id);
            }
            Request::RefreshSystem => {
                if !self.system.as_ref().is_some_and(system::Client::refresh) {
                    self.state.request_failed(request.clone());
                }
                self.refresh_sleep();
                let controls = self.state.refresh_controls();
                self.submit_control(controls);
                let radios = self.state.refresh_radios();
                self.submit_radio(radios);
            }
            Request::RefreshSleep => self.refresh_sleep(),
            Request::PrepareSuspend { request: id } => self.begin_suspend(*id),
            Request::CancelSuspend { request: id } => self.cancel_suspend(*id),
            Request::RefreshControls { .. } | Request::AdjustControl { .. } => {
                self.submit_control(request.clone());
            }
            Request::RadioOperation { .. } => {
                self.submit_radio(request.clone());
            }
            Request::SystemOperation {
                request: id,
                operation,
            } => {
                if !self
                    .system
                    .as_ref()
                    .is_some_and(|client| client.execute(*id, *operation))
                {
                    self.state.request_failed(request.clone());
                }
            }
            Request::LaunchDesktop(id) => return self.launch(id, None),
            Request::RetryLaunch {
                desktop_id,
                attempt,
            } => return self.launch(desktop_id, Some(*attempt)),
            Request::ActivateWindow(_) | Request::CloseWindow(_) => {
                return self.begin_window_operation(request, Instant::now());
            }
            Request::DismissNotification(_)
            | Request::InvokeNotification { .. }
            | Request::MarkNotificationRead { .. }
            | Request::ClearNotifications
            | Request::RetryNotifications => {
                let command = match &request {
                    Request::DismissNotification(id) => notifications::Command::Dismiss(*id),
                    Request::InvokeNotification {
                        id,
                        generation,
                        action,
                    } => notifications::Command::Invoke {
                        id: *id,
                        generation: *generation,
                        action: action.clone(),
                    },
                    Request::MarkNotificationRead { id, generation } => {
                        notifications::Command::MarkRead {
                            id: *id,
                            generation: *generation,
                        }
                    }
                    Request::ClearNotifications => notifications::Command::Clear,
                    Request::RetryNotifications => notifications::Command::Retry,
                    _ => unreachable!(),
                };
                if let Err(error) = self
                    .notifications
                    .as_ref()
                    .ok_or_else(|| "Notifications service unavailable".to_string())
                    .and_then(|client| client.request(command))
                {
                    eprintln!("Notification request failed: {error}");
                    self.state.request_failed(request.clone());
                }
            }
            Request::None => {}
        }
        // Launch/command failures may open a dialog after on_key returned.
        if self.suppress_banner() {
            self.banner = None;
        }
        Request::None
    }

    fn new_intent(&mut self) {
        self.input_generation = self.input_generation.wrapping_add(1);
        self.launch_intent = None;
    }

    pub fn input_anchor(&self) -> InputAnchor {
        let controls_visible = self
            .state
            .system_view
            .as_ref()
            .and_then(SystemView::control)
            .is_some();
        let radios_visible = self
            .state
            .system_view
            .as_ref()
            .and_then(SystemView::radio)
            .is_some();
        InputAnchor {
            screen: self.state.screen,
            dialog: self.state.dialog.clone(),
            dialog_target: match self.state.dialog {
                Some(Dialog::ConfirmClose(id)) => self
                    .state
                    .live_windows
                    .iter()
                    .find(|window| window.id == id)
                    .map(|window| (window.id, window.app_id.clone())),
                _ => None,
            },
            search_input: self.state.search_input.clone(),
            system_view: self.state.system_view.clone(),
            system_request: self
                .state
                .system_view
                .as_ref()
                .and(self.state.system_request),
            system_snapshot: self
                .state
                .system_view
                .as_ref()
                .map(|_| self.state.system_snapshot),
            controls_request: controls_visible
                .then_some(self.state.controls_request)
                .flatten(),
            controls_snapshot: controls_visible.then_some(self.state.controls_snapshot),
            controls_failure: self
                .state
                .system_view
                .as_ref()
                .and_then(SystemView::control)
                .and_then(|control| self.state.control_failure(control)),
            radios_request: radios_visible
                .then_some(self.state.radios_request)
                .flatten(),
            radios_snapshot: radios_visible.then(|| self.state.radios_snapshot.clone()),
            radios_failure: self
                .state
                .system_view
                .as_ref()
                .and_then(SystemView::radio)
                .and_then(|kind| self.state.radio_failure(kind)),
            radio_entries: radios_visible.then(|| self.state.radio_entries.clone()),
            sleep_snapshot: self
                .state
                .system_view
                .as_ref()
                .map(|_| self.state.sleep_snapshot),
            sleep_progress: self
                .state
                .system_view
                .as_ref()
                .and(self.state.sleep_progress),
            lock_state: self.state.system_view.as_ref().and(self.state.lock_state),
            view: self.state.application_view.clone(),
            actions: self.state.actions(),
            window: if self.state.screen == Screen::Recent {
                self.state
                    .window_at(self.state.selected)
                    .map(|(window, _)| (window.id, window.app_id.clone()))
            } else {
                None
            },
            desktop: if self.state.screen == Screen::Menu {
                self.state
                    .menu_app(self.state.selected)
                    .map(|app| app.desktop_id.clone())
            } else {
                None
            },
        }
    }

    pub fn input_target_unchanged(&mut self, before: InputAnchor, key: Key) -> bool {
        if before.search_input.is_some() || self.state.is_text_editing() {
            // Catalog/window updates may change a result behind the editor,
            // but cannot turn its Enter into a newly revealed launch/confirm.
            if before.screen != self.state.screen || before.search_input != self.state.search_input
            {
                self.new_intent();
                return false;
            }
            return true;
        }
        let current = self.input_anchor();
        let control_arrow = matches!(key, Key::Left | Key::Right)
            && (before
                .system_view
                .as_ref()
                .and_then(SystemView::control)
                .is_some()
                || current
                    .system_view
                    .as_ref()
                    .and_then(SystemView::control)
                    .is_some());
        if (matches!(key, Key::Enter | Key::Soft(0..=3)) || control_arrow) && before != current {
            // A queued key must not choose a new command that was revealed by
            // the preceding Wayland event in this same dispatch batch.
            self.new_intent();
            false
        } else {
            true
        }
    }

    pub fn system_input_matches_frame(&mut self, shown: Option<&InputAnchor>, key: Key) -> bool {
        let current = self.input_anchor();
        let control_arrow = matches!(key, Key::Left | Key::Right)
            && (current
                .system_view
                .as_ref()
                .and_then(SystemView::control)
                .is_some()
                || shown.is_some_and(|anchor| {
                    anchor
                        .system_view
                        .as_ref()
                        .and_then(SystemView::control)
                        .is_some()
                }));
        if !matches!(key, Key::Enter | Key::Soft(0..=3)) && !control_arrow {
            return true;
        }
        let system_context = current.system_view.is_some()
            || shown.is_some_and(|anchor| anchor.system_view.is_some());
        if system_context && shown != Some(&current) {
            // A worker reply or a preceding key may already have changed the
            // model before this keyboard callback starts. Until the runtime
            // commits that view, its new command has not been shown to the user.
            self.new_intent();
            false
        } else {
            true
        }
    }

    pub fn focus(&mut self, focused: bool) {
        self.focused = focused;
        if !focused {
            // An error must not reopen a card after the user switched away.
            self.launch_intent = None;
        }
    }

    fn suppress_banner(&self) -> bool {
        self.lock_presence
            || self.lock_journal
            || self.state.lock_busy()
            || self.state.modal_active()
            || self.state.application_view != ApplicationView::List
            || self.state.screen == Screen::Notifications
    }

    fn resolved_windows(&self) -> Vec<(u64, String)> {
        self.state
            .live_windows
            .iter()
            .map(|window| {
                (
                    window.id,
                    self.state
                        .resolve_app(&window.app_id)
                        .map(|app| app.desktop_id.clone())
                        .unwrap_or_default(),
                )
            })
            .collect()
    }

    fn launch(&mut self, desktop_id: &str, retry: Option<u64>) -> Request {
        let old_name = self
            .state
            .applications
            .iter()
            .find(|app| app.desktop_id == desktop_id)
            .map(|app| app.name.clone())
            .unwrap_or_else(|| desktop_id.to_owned());
        self.refresh_applications();
        let windows = self.resolved_windows();
        let app = self.desktop_apps.iter().find(|app| app.id == desktop_id);
        let name = app.map(|app| app.name.as_str()).unwrap_or(&old_name);
        let spawn = || {
            app.ok_or(desktop::LaunchError::MissingDesktopEntry)
                .and_then(desktop::DesktopApp::launch)
        };
        let outcome = match retry {
            Some(attempt) => {
                self.launches
                    .retry(desktop_id, name, attempt, &windows, Instant::now(), spawn)
            }
            None => self
                .launches
                .start(desktop_id, name, &windows, Instant::now(), spawn),
        };
        match outcome {
            launch::Outcome::Started(attempt) | launch::Outcome::Suppressed(attempt) => {
                self.launch_intent = Some((desktop_id.into(), attempt, self.input_generation));
            }
            launch::Outcome::ExistingWindow(id) => {
                self.sync_launches();
                return self.begin_window_operation(Request::ActivateWindow(id), Instant::now());
            }
            launch::Outcome::Rejected(
                launch::Rejection::StaleAttempt | launch::Rejection::RetryUnavailable,
            ) => {}
            launch::Outcome::Rejected(reason) => {
                eprintln!("Launch request rejected: {reason:?}");
                let body = localized(self.state.locale,
                    "Нельзя начать ещё один запуск. Дождитесь уже запущенных приложений и повторите.",
                    "Another launch cannot start. Wait for pending applications and try again.",
                    "No se puede iniciar otra aplicación. Espere los inicios pendientes y reintente.");
                self.internal_notice(name.to_owned(), body.to_owned());
                self.failed(Request::LaunchDesktop(desktop_id.to_owned()));
            }
        }
        self.sync_launches();
        Request::None
    }

    fn sync_launches(&mut self) {
        let Some(statuses) = self.launches.take_update() else {
            return;
        };
        self.launch_snapshot = statuses;
        let statuses = self.localized_launches();
        let changed: Vec<_> = statuses
            .iter()
            .filter(|status| {
                status.phase != LaunchPhase::Starting
                    && !self.state.launches.iter().any(|old| old == *status)
            })
            .cloned()
            .collect();
        self.state.sync_launches(statuses);
        for status in changed {
            self.internal_notice(status.name.clone(), status.reason.clone());
            let same_intent =
                self.launch_intent
                    .as_ref()
                    .is_some_and(|(id, attempt, generation)| {
                        id == &status.desktop_id
                            && *attempt == status.attempt
                            && *generation == self.input_generation
                    });
            if self.focused
                && same_intent
                && !self.authentication_active
                && !self.lock_inhibits_shell()
                && !self.state.modal_active()
            {
                self.state
                    .show_launch_error(&status.desktop_id, status.attempt);
                self.banner = None;
            }
        }
        self.dirty = true;
    }

    fn localized_launches(&self) -> Vec<LaunchStatus> {
        self.launch_snapshot
            .iter()
            .map(|status| LaunchStatus {
                desktop_id: status.desktop_id.clone(),
                name: status.name.clone(),
                attempt: status.attempt,
                phase: match status.phase {
                    launch::Phase::Starting => LaunchPhase::Starting,
                    launch::Phase::Unconfirmed => LaunchPhase::Unconfirmed,
                    launch::Phase::Failed(_) => LaunchPhase::Failed,
                },
                reason: launch_reason(self.state.locale, status.phase),
            })
            .collect()
    }

    fn internal_notice(&self, title: String, body: String) {
        // The existing bounded worker queue is also usable while D-Bus is down.
        // A full queue is logged once here; never recursively create an error.
        if let Some(client) = &self.notifications {
            if let Err(error) = client.request(notifications::Command::PostInternal { title, body })
            {
                eprintln!("Could not record Shell notification: {error}");
            }
        }
    }

    fn begin_window_operation(&mut self, request: Request, now: Instant) -> Request {
        let (id, kind) = match &request {
            Request::ActivateWindow(id) => (*id, WindowOperation::Activating),
            Request::CloseWindow(id) => (*id, WindowOperation::Closing),
            _ => return Request::None,
        };
        if self
            .window_operations
            .iter()
            .any(|operation| operation.id == id && operation.deadline.is_some())
        {
            return Request::None;
        }
        if !self.state.live_windows.iter().any(|window| window.id == id) {
            self.failed(request);
            return Request::None;
        }
        self.window_operations
            .retain(|operation| operation.id != id);
        self.window_operations.push(PendingWindowOperation {
            id,
            kind,
            deadline: Some(now + WINDOW_RESPONSE_TIMEOUT),
        });
        self.sync_window_operations();
        request
    }

    fn sync_window_operations(&mut self) {
        self.state.sync_window_operations(
            self.window_operations
                .iter()
                .map(|operation| WindowOperationStatus {
                    id: operation.id,
                    kind: operation.kind,
                })
                .collect(),
        );
        self.dirty = true;
    }

    fn tick_window_operations(&mut self, now: Instant) {
        let mut expired = Vec::new();
        for operation in &mut self.window_operations {
            if operation.deadline.is_some_and(|deadline| now >= deadline) {
                operation.deadline = None;
                operation.kind = match operation.kind {
                    WindowOperation::Activating => WindowOperation::ActivationUnconfirmed,
                    _ => WindowOperation::CloseUnconfirmed,
                };
                expired.push((operation.id, operation.kind));
            }
        }
        if !expired.is_empty() {
            for (id, kind) in expired {
                if let Some(window) = self
                    .state
                    .live_windows
                    .iter()
                    .find(|window| window.id == id)
                {
                    let body = if kind == WindowOperation::ActivationUnconfirmed {
                        localized(
                            self.state.locale,
                            "Переключение пока не подтверждено.",
                            "Switching to the window has not been confirmed yet.",
                            "El cambio a la ventana aún no está confirmado.",
                        )
                    } else {
                        localized(self.state.locale, "Окно ещё открыто. Возможно, приложение ожидает сохранения. Переключитесь к нему для проверки.", "The window is still open. The application may be waiting for you to save. Switch to it to check.", "La ventana sigue abierta. Quizá la aplicación espere que guarde los cambios. Ábrala para comprobarlo.")
                    };
                    self.internal_notice(window.title.clone(), body.to_owned());
                }
            }
            self.sync_window_operations();
        }
    }
    pub fn receive_notification(&mut self, message: notifications::Message) {
        match message {
            notifications::Message::AuthenticationActive(active) => {
                self.authentication_active = active;
                if active {
                    self.banner = None;
                    self.launch_intent = None;
                }
            }
            notifications::Message::Changed => {
                if let Some(snapshot) = self
                    .notifications
                    .as_ref()
                    .and_then(|client| client.take_update())
                {
                    self.receive_snapshot(snapshot);
                }
            }
            notifications::Message::Available => self.state.notification_service(true),
            notifications::Message::Unavailable(error) => {
                eprintln!("Notifications service unavailable: {error}");
                self.state.notification_service(false);
                self.authentication_active = true;
                self.banner = None;
            }
            notifications::Message::CommandFailed(error) => {
                eprintln!("Notification action failed: {error}");
                self.state.notification_failed();
                self.banner = None;
            }
        }
        self.dirty = true;
    }
    fn receive_snapshot(&mut self, snapshot: Snapshot) {
        let is_new = snapshot
            .latest_event
            .is_some_and(|event| event.generation > self.notification_generation);
        self.notification_generation = self.notification_generation.max(snapshot.event_generation);
        self.state.sync_notices(snapshot.notices);
        if is_new && !self.authentication_active && !self.suppress_banner() {
            if let Some(event) = snapshot.latest_event.filter(|event| {
                self.state.notices.iter().any(|notice| {
                    notice.id == event.id && notice.generation == event.generation && notice.active
                })
            }) {
                self.banner = Some((event, Instant::now() + Duration::from_secs(4)));
            }
        }
        if self.banner.as_ref().is_some_and(|(event, _)| {
            !self.state.notices.iter().any(|notice| {
                notice.id == event.id && notice.generation == event.generation && notice.active
            })
        }) {
            self.banner = None;
        }
    }
    pub fn windows(&mut self, windows: Vec<WindowInfo>) {
        if self.state.live_windows != windows {
            self.state.sync_windows(windows);
            self.launches.windows(&self.resolved_windows());
            self.sync_launches();
            let count = self.window_operations.len();
            self.window_operations.retain(|operation| {
                self.state
                    .live_windows
                    .iter()
                    .find(|window| window.id == operation.id)
                    .is_some_and(|window| {
                        !(window.active
                            && matches!(
                                operation.kind,
                                WindowOperation::Activating
                                    | WindowOperation::ActivationUnconfirmed
                            ))
                    })
            });
            if count != self.window_operations.len() {
                self.sync_window_operations();
            }
            self.dirty = true;
        }
    }
    pub fn windows_unavailable(&mut self) {
        self.window_operations.clear();
        self.sync_window_operations();
        self.state.lose_window_service();
        self.dirty = true;
    }
    pub fn banner(&self) -> Option<Banner> {
        if self.authentication_active || self.suppress_banner() {
            return None;
        }
        let (event, deadline) = self.banner.as_ref()?;
        if Instant::now() >= *deadline {
            return None;
        }
        let notice = self
            .state
            .notices
            .iter()
            .find(|n| n.id == event.id && n.generation == event.generation && n.active)?;
        Some(Banner {
            title: notice.title.clone(),
            background: self.state.theme.surface,
            foreground: self.state.theme.text,
            accent: self.state.theme.accent,
        })
    }
    pub fn tick(&mut self) {
        if self
            .banner
            .as_ref()
            .is_some_and(|(_, until)| Instant::now() >= *until)
        {
            self.banner = None;
            self.dirty = true;
        }
        let monotonic = Instant::now();
        let effects = self.battery_policy.tick(monotonic);
        self.apply_battery_effects(effects);
        if self.battery_sleep.as_ref().is_some_and(|automatic| {
            automatic.request.is_none() && monotonic >= automatic.refresh_deadline
        }) {
            self.finish_battery_sleep(Some(system::Failure::Timeout), false);
        }
        self.launches.tick(monotonic);
        self.sync_launches();
        self.tick_window_operations(monotonic);
        let now = Local::now();
        let clock = now.format("%H:%M").to_string();
        let date = now.format("%Y-%m-%d").to_string();
        if self.clock != clock || self.last_date != date {
            self.clock = clock;
            self.last_date = date;
            self.dirty = true;
        }
    }
    pub fn wait_duration(&self) -> Duration {
        let now = Local::now();
        let mut wait = Duration::from_secs(60 - u64::from(now.second()))
            .saturating_sub(Duration::from_nanos(u64::from(now.nanosecond())));
        if let Some((_, deadline)) = self.banner {
            wait = wait.min(deadline.saturating_duration_since(Instant::now()));
        }
        // Reap only when we actually have child processes. Idle Shell has no
        // 100ms service/window polling; channels and Wayland wake calloop.
        if let Some(launch_wait) = self.launches.next_wakeup(Instant::now()) {
            wait = wait.min(launch_wait);
        }
        for operation in &self.window_operations {
            if let Some(deadline) = operation.deadline {
                wait = wait.min(deadline.saturating_duration_since(Instant::now()));
            }
        }
        if let Some(deadline) = self.sleep_coordinator.deadline() {
            wait = wait.min(deadline.saturating_duration_since(Instant::now()));
        }
        if let Some(deadline) = self.battery_policy.deadline() {
            wait = wait.min(deadline.saturating_duration_since(Instant::now()));
        }
        if let Some(automatic) = self
            .battery_sleep
            .as_ref()
            .filter(|automatic| automatic.request.is_none())
        {
            wait = wait.min(
                automatic
                    .refresh_deadline
                    .saturating_duration_since(Instant::now()),
            );
        }
        wait.max(Duration::from_millis(1))
    }
}

pub fn resolve_locale(language: Language) -> Locale {
    match language {
        Language::Auto => Locale::detect(),
        Language::En => Locale::En,
        Language::Ru => Locale::Ru,
        Language::Es => Locale::Es,
    }
}

fn localized<'a>(locale: Locale, ru: &'a str, en: &'a str, es: &'a str) -> &'a str {
    match locale {
        Locale::Ru => ru,
        Locale::En => en,
        Locale::Es => es,
    }
}

fn launch_reason(locale: Locale, phase: launch::Phase) -> String {
    use desktop::LaunchError as Error;
    use launch::{FailureReason as Failure, Phase};
    let message = match phase {
        Phase::Starting => localized(locale, "Запускается…", "Starting…", "Iniciando…"),
        Phase::Unconfirmed => localized(locale,
            "Окно пока не появилось. Приложение может ещё запускаться. Повторный запуск может создать второй экземпляр.",
            "No window has appeared yet. The application may still be starting. Retrying may create another instance.",
            "Aún no apareció una ventana. La aplicación puede seguir iniciándose. Reintentar podría crear otra instancia."),
        Phase::Failed(Failure::Spawn(error)) => match error {
            Error::MissingDesktopEntry => localized(locale, "Приложение больше не найдено в меню.", "The application is no longer available in the menu.", "La aplicación ya no está disponible en el menú."),
            Error::MissingProgram => localized(locale, "Не найден исполняемый файл приложения.", "The application executable was not found.", "No se encontró el ejecutable de la aplicación."),
            Error::PermissionDenied => localized(locale, "Недостаточно прав для запуска приложения.", "Permission to start the application was denied.", "No tiene permiso para iniciar la aplicación."),
            Error::InvalidWorkingDirectory => localized(locale, "В ярлыке задана неверная рабочая папка.", "The application entry specifies an invalid working directory.", "La entrada indica una carpeta de trabajo no válida."),
            Error::UnavailableWorkingDirectory => localized(locale, "Рабочая папка приложения недоступна.", "The application's working directory is unavailable.", "La carpeta de trabajo de la aplicación no está disponible."),
            Error::MissingTerminal => localized(locale, "Для приложения нужен терминал Foot, но он не найден.", "This application requires the Foot terminal, which was not found.", "Esta aplicación necesita el terminal Foot, pero no se encontró."),
            Error::MissingTerminalConfig => localized(locale, "Не найден профиль терминала Alcard.", "The Alcard terminal profile was not found.", "No se encontró el perfil del terminal Alcard."),
            Error::InvalidTerminalConfig => localized(locale, "Профиль терминала Alcard недоступен или задан неверно.", "The Alcard terminal profile is unavailable or invalid.", "El perfil del terminal Alcard no está disponible o no es válido."),
            Error::InvalidDesktopIdentity => localized(locale, "В ярлыке указан неверный идентификатор приложения.", "The application entry has an invalid identity.", "La entrada tiene un identificador de aplicación no válido."),
            Error::Spawn(_) => localized(locale, "Система не смогла запустить приложение.", "The system could not start the application.", "El sistema no pudo iniciar la aplicación."),
        },
        Phase::Failed(Failure::Observation) => localized(locale, "Не удалось проверить завершение процесса. Проверьте, не открылось ли приложение.", "The process result could not be checked. Check whether the application opened.", "No se pudo comprobar el resultado del proceso. Compruebe si se abrió la aplicación."),
        Phase::Failed(Failure::EarlyExit { code, signal }) => {
            let reason = localized(locale,
                "Приложение завершилось до появления окна.",
                "The application exited before a window appeared.",
                "La aplicación terminó antes de mostrar una ventana.");
            return if let Some(code) = code {
                format!("{reason} {}: {code}.", localized(locale, "Код", "Code", "Código"))
            } else if let Some(signal) = signal {
                format!("{reason} {}: {signal}.", localized(locale, "Сигнал", "Signal", "Señal"))
            } else { reason.into() };
        }
    };
    message.into()
}

#[cfg(test)]
#[path = "unit_tests/app_battery.rs"]
mod battery_tests;

#[cfg(test)]
#[path = "unit_tests/app_sleep.rs"]
mod sleep_tests;

#[cfg(test)]
#[path = "unit_tests/app_locking.rs"]
mod locking_tests;

#[cfg(test)]
#[path = "unit_tests/app_controls.rs"]
mod controls_tests;

#[cfg(test)]
#[path = "unit_tests/app_radio.rs"]
mod radio_tests;

#[cfg(test)]
#[path = "unit_tests/app_notification.rs"]
mod notification_tests;

#[cfg(test)]
#[path = "unit_tests/app_lifecycle.rs"]
mod lifecycle_tests;

#[cfg(test)]
#[path = "unit_tests/app_preferences.rs"]
mod preferences_tests;
