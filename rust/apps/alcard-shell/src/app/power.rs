//! Lock, suspend and battery-policy orchestration for the Shell application.

use super::{
    battery, battery_journal, localized, locking, notifications, sleep, system, Application,
    BatteryObservation, BatteryPreferences, BatterySleep, BatteryWarning, IdlePreferences, Instant,
    LockPurpose, Request, SleepEffect, BATTERY_REFRESH_TIMEOUT,
};

impl Application {
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

    pub(super) fn sync_sleep_view(&mut self) {
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

    pub(super) fn refresh_sleep(&mut self) {
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

    pub(super) fn begin_suspend(&mut self, request: u64) {
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

    pub(super) fn cancel_suspend(&mut self, request: u64) {
        let effects = self.sleep_coordinator.cancel(request);
        self.apply_sleep_effects(effects);
    }

    pub(super) fn apply_sleep_effects(&mut self, effects: Vec<SleepEffect>) {
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

    pub(super) fn apply_battery_effects(&mut self, effects: Vec<battery::Effect>) {
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

    pub(super) fn sync_battery_journal(&mut self) {
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

    pub(super) fn request_battery_sleep(&mut self, episode: u64) {
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

    pub(super) fn finish_battery_sleep(&mut self, failure: Option<system::Failure>, cancel: bool) {
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

    pub(super) fn battery_sleep_failed(&mut self, episode: u64, failure: system::Failure) {
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
    pub(super) fn battery_commit_ready(&mut self, now: Instant) -> bool {
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

    pub(super) fn publish_battery_warning(&self, announce: bool) {
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

    pub(super) fn submit_lock(&mut self, id: u64) -> bool {
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

    pub(super) fn report_idle_lock_failure(&mut self, id: u64) {
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
}
