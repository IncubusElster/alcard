//! Pure suspend intent reduction. No permission to sleep or release a delay
//! inhibitor leaves this module until the complete event batch is flushed.
//! Locker presence and its journal are advisory, never lock confirmation.

use crate::{
    locking::{self, Phase, State},
    sleep::{self, MonitorState, Permit, Progress, Snapshot, Stage},
    system::{Capability, Failure},
};
use std::time::{Duration, Instant};

const PREPARE_TIMEOUT: Duration = Duration::from_secs(8);
const LOCK_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockPurpose {
    Own(u64),
    External { epoch: u64, cycle: u64 },
}

#[derive(Clone, Debug)]
pub enum Effect {
    NeedLock(LockPurpose),
    Commit {
        request: u64,
        epoch: u64,
        permit: Permit,
    },
    ReleaseDelay {
        epoch: u64,
        cycle: u64,
        permit: Permit,
    },
    Cancel {
        request: u64,
    },
    Resume,
}

struct Binding {
    id: u64,
    permit: Permit,
    observed_locked: bool,
}

impl Binding {
    fn new(state: State) -> Self {
        Self {
            id: state.id,
            permit: Permit::new(),
            observed_locked: state.phase == Phase::Locked,
        }
    }

    fn revoke(&self) {
        self.permit.revoke();
    }
}

struct Intent {
    progress: Progress,
    epoch: u64,
    deadline: Option<Instant>,
    lock: Option<Binding>,
    dispatched: bool,
    cycle: Option<u64>,
}

struct Cycle {
    epoch: u64,
    id: u64,
    lock: Option<Binding>,
    awaiting_binding: bool,
    requested_lock: bool,
    released: bool,
}

#[derive(Default)]
pub struct Coordinator {
    snapshot: Snapshot,
    own: Option<Intent>,
    last_progress: Option<Progress>,
    cycle: Option<Cycle>,
    lock: Option<State>,
    newest_lock: u64,
    newest_request: u64,
    newest_cycle: Option<u64>,
    false_seen: bool,
    pending: Vec<Effect>,
}

impl Coordinator {
    pub fn begin(&mut self, request: u64, now: Instant) -> bool {
        if request == 0
            || request <= self.newest_request
            || self.own.is_some()
            || self.cycle.is_some()
            || self.snapshot.monitor != MonitorState::Ready
            || self.snapshot.capability != Capability::Allowed
        {
            return false;
        }
        self.newest_request = request;
        self.last_progress = None;
        self.own = Some(Intent {
            progress: Progress {
                request,
                stage: Stage::Preparing,
                failure: None,
            },
            epoch: self.snapshot.epoch,
            deadline: Some(now + PREPARE_TIMEOUT),
            lock: None,
            dispatched: false,
            cycle: None,
        });
        true
    }

    pub fn receive(&mut self, message: sleep::Message, now: Instant) -> Vec<Effect> {
        self.expire(now);
        match message {
            sleep::Message::Snapshot(snapshot) | sleep::Message::Refreshed { snapshot, .. } => {
                self.receive_snapshot(snapshot)
            }
            sleep::Message::Prepared {
                request,
                epoch,
                result,
            } => {
                if self.own.as_ref().is_some_and(|own| {
                    own.progress.request == request
                        && own.epoch == epoch
                        && own.progress.stage == Stage::Preparing
                        && epoch == self.snapshot.epoch
                }) {
                    match result {
                        Ok(()) => {
                            if self.snapshot.monitor == MonitorState::Ready
                                && self.snapshot.capability == Capability::Allowed
                                && self.cycle.is_none()
                            {
                                let own = self.own.as_mut().expect("matching intent");
                                own.progress.stage = Stage::Locking;
                                // Bound even if a caller fails to attach the
                                // requested locker. Binding starts its own 10s.
                                own.deadline = Some(now + LOCK_TIMEOUT);
                                self.pending
                                    .push(Effect::NeedLock(LockPurpose::Own(request)));
                            } else {
                                self.finish(Stage::Failed, Some(Failure::Unavailable));
                            }
                        }
                        Err(failure) => self.finish(Self::failed_stage(failure), Some(failure)),
                    }
                }
            }
            sleep::Message::Preparing { epoch, cycle } => self.preparing(epoch, cycle),
            sleep::Message::CycleEnded { epoch, cycle } => self.cycle_ended(epoch, cycle),
            sleep::Message::Finished {
                request,
                epoch,
                result,
            } => {
                if epoch == self.snapshot.epoch
                    && self.snapshot.monitor != MonitorState::Unavailable
                    && self.own.as_ref().is_some_and(|own| {
                        own.progress.request == request && own.epoch == epoch && own.dispatched
                    })
                {
                    match result {
                        Ok(()) => {
                            let own = self.own.as_mut().expect("matching intent");
                            own.progress.stage = Stage::Requested;
                            own.progress.failure = None;
                        }
                        Err(Failure::Unknown) => self.unknown(),
                        Err(failure) => self.finish(Self::failed_stage(failure), Some(failure)),
                    }
                }
            }
            sleep::Message::Lost { epoch } if epoch == self.snapshot.epoch => {
                self.snapshot.monitor = MonitorState::Unavailable;
                self.snapshot.capability = Capability::Unavailable;
                self.invalidate_epoch();
            }
            sleep::Message::Lost { .. } => {}
        }
        self.take_pending()
    }

    pub fn lock_started(&mut self, purpose: LockPurpose, id: u64, now: Instant) {
        self.expire(now);
        match purpose {
            LockPurpose::Own(request) => {
                let Some(own) = self.own.as_ref().filter(|own| {
                    own.progress.request == request && own.progress.stage == Stage::Locking
                }) else {
                    return;
                };
                if let Some(bound) = &own.lock {
                    if bound.id != id {
                        self.abort(Failure::SessionUnavailable);
                    }
                    return;
                }
                let Some(state) = self.bindable(id, false) else {
                    self.abort(Failure::SessionUnavailable);
                    return;
                };
                let own = self.own.as_mut().expect("matching intent");
                own.lock = Some(Binding::new(state));
                own.deadline = Some(now + LOCK_TIMEOUT);
            }
            LockPurpose::External { epoch, cycle } => {
                if !self.cycle.as_ref().is_some_and(|current| {
                    current.epoch == epoch && current.id == cycle && !current.released
                }) {
                    return;
                }
                let Some(state) = self.bindable(id, true) else {
                    self.lock_failed(purpose);
                    return;
                };
                let current = self.cycle.as_mut().expect("matching cycle");
                if let Some(old) = current.lock.take() {
                    old.revoke();
                }
                current.lock = Some(Binding::new(state));
                current.awaiting_binding = false;
                current.requested_lock = true;
            }
        }
    }

    pub fn lock_failed(&mut self, purpose: LockPurpose) {
        match purpose {
            LockPurpose::Own(request)
                if self.own.as_ref().is_some_and(|own| {
                    own.progress.request == request && own.progress.stage == Stage::Locking
                }) =>
            {
                self.abort(Failure::Unavailable);
            }
            LockPurpose::External { epoch, cycle } => {
                if let Some(current) = self
                    .cycle
                    .as_mut()
                    .filter(|current| current.epoch == epoch && current.id == cycle)
                {
                    if let Some(bound) = current.lock.take() {
                        bound.revoke();
                    }
                    current.awaiting_binding = false;
                    current.requested_lock = true;
                }
            }
            LockPurpose::Own(_) => {}
        }
    }

    pub fn receive_lock(&mut self, message: locking::Message) {
        match message {
            locking::Message::Presence(_) | locking::Message::Journal(_) => return,
            locking::Message::Changed(state) => {
                if state.id == 0 {
                    if self
                        .lock
                        .is_some_and(|current| current.id != 0 && current.phase != Phase::Failed)
                    {
                        return;
                    }
                } else {
                    if state.id < self.newest_lock
                        || (state.id == self.newest_lock
                            && self.lock.is_none_or(|current| current.id != state.id))
                    {
                        return;
                    }
                    self.newest_lock = state.id;
                }
                if self.lock.is_some_and(|current| {
                    current.id == state.id && state.id != 0 && current.phase == Phase::Failed
                }) {
                    return;
                }
                self.lock = Some(state);
            }
            locking::Message::Unlocked { id } => {
                if self.lock.is_none_or(|current| current.id != id) {
                    return;
                }
                self.lock = None;
            }
        }

        let invalid_own = self.own.as_mut().is_some_and(|own| {
            own.lock.as_mut().is_some_and(|bound| {
                match self.lock.filter(|state| state.id == bound.id) {
                    Some(State {
                        phase: Phase::Locked,
                        ..
                    }) => {
                        bound.observed_locked = true;
                        false
                    }
                    Some(State {
                        phase: Phase::Starting,
                        ..
                    }) if !bound.observed_locked => false,
                    _ => true,
                }
            })
        });
        if invalid_own {
            self.abort(Failure::SessionUnavailable);
        }
        self.update_cycle_lock();
    }

    pub fn flush(&mut self, now: Instant) -> Vec<Effect> {
        self.flush_when(now, true)
    }

    /// The battery caller can defer its own commit until a new post-lock
    /// measurement arrives. External delay release is never held by this gate.
    pub fn flush_when(&mut self, now: Instant, allow_own_commit: bool) -> Vec<Effect> {
        self.expire(now);
        let mut effects = self.take_pending();
        if let Some(own) = self.own.as_mut() {
            if allow_own_commit
                && own.progress.stage == Stage::Locking
                && own.epoch == self.snapshot.epoch
                && self.snapshot.monitor == MonitorState::Ready
                && self.snapshot.capability == Capability::Allowed
                && self.cycle.is_none()
            {
                if let Some(bound) = own.lock.as_mut().filter(|bound| {
                    bound.permit.valid()
                        && self.lock
                            == Some(State {
                                id: bound.id,
                                phase: Phase::Locked,
                            })
                }) {
                    bound.observed_locked = true;
                    own.progress.stage = Stage::Queued;
                    own.deadline = None;
                    own.dispatched = true;
                    effects.push(Effect::Commit {
                        request: own.progress.request,
                        epoch: own.epoch,
                        permit: bound.permit.clone(),
                    });
                }
            }
        }
        if let Some(cycle) = self.cycle.as_mut() {
            if !cycle.released && cycle.epoch == self.snapshot.epoch {
                if let Some(bound) = cycle.lock.as_mut().filter(|bound| {
                    bound.permit.valid()
                        && self.lock
                            == Some(State {
                                id: bound.id,
                                phase: Phase::Locked,
                            })
                }) {
                    bound.observed_locked = true;
                    cycle.released = true;
                    effects.push(Effect::ReleaseDelay {
                        epoch: cycle.epoch,
                        cycle: cycle.id,
                        permit: bound.permit.clone(),
                    });
                }
            }
        }
        effects
    }

    pub fn cancel(&mut self, request: u64) -> Vec<Effect> {
        if self
            .own
            .as_ref()
            .is_some_and(|own| own.progress.request == request)
        {
            self.abort(Failure::Cancelled);
        }
        self.take_pending()
    }

    pub fn progress(&self) -> Option<Progress> {
        self.own
            .as_ref()
            .map(|own| own.progress)
            .or(self.last_progress)
    }

    pub fn snapshot(&self) -> Snapshot {
        self.snapshot
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.own.as_ref().and_then(|own| own.deadline)
    }

    pub fn submission_failed(&mut self, request: u64, failure: Failure) {
        if self
            .own
            .as_ref()
            .is_some_and(|own| own.progress.request == request)
        {
            if failure == Failure::Unknown {
                self.unknown();
            } else {
                // The caller knows the queue rejected the work. Unlike a
                // cancellation after dispatch, this proves no new submission.
                self.finish(Self::failed_stage(failure), Some(failure));
            }
        }
    }

    fn receive_snapshot(&mut self, snapshot: Snapshot) {
        if snapshot.epoch < self.snapshot.epoch {
            return;
        }
        if snapshot.epoch > self.snapshot.epoch {
            self.invalidate_epoch();
            self.newest_cycle = None;
            self.false_seen = false;
        }
        self.snapshot = snapshot;
        if self.own.as_ref().is_some_and(|own| {
            own.dispatched && own.epoch < snapshot.epoch && snapshot.monitor == MonitorState::Ready
        }) {
            // A new monitor epoch reports Ready only after observing false
            // and acquiring fresh protection. The old result stays unknown;
            // it is not a completed or successful sleep cycle.
            self.finish(Stage::Failed, Some(Failure::Unknown));
        } else if self.own.as_ref().is_some_and(|own| {
            !own.dispatched
                && (own.epoch != snapshot.epoch
                    || snapshot.monitor != MonitorState::Ready
                    || snapshot.capability != Capability::Allowed)
        }) {
            self.abort(Failure::Unavailable);
        } else if self.own.as_ref().is_some_and(|own| {
            own.dispatched
                && matches!(
                    snapshot.monitor,
                    MonitorState::Connecting | MonitorState::Unavailable
                )
        }) {
            self.abort(Failure::SessionUnavailable);
        } else if snapshot.monitor == MonitorState::Preparing {
            if let Some(bound) = self.own.as_ref().and_then(|own| own.lock.as_ref()) {
                // The cycle signal follows this snapshot. Close the queued
                // commit gate immediately, even before that message arrives.
                bound.revoke();
            }
        }
        if self.cycle.is_some() && self.snapshot.monitor == MonitorState::Ready {
            self.snapshot.monitor = MonitorState::Preparing;
        }
    }

    fn preparing(&mut self, epoch: u64, id: u64) {
        if epoch != self.snapshot.epoch
            || self.snapshot.monitor == MonitorState::Unavailable
            || self.newest_cycle.is_some_and(|newest| id <= newest)
        {
            return;
        }
        if let Some(old) = self.cycle.take() {
            if let Some(bound) = old.lock {
                bound.revoke();
            }
            if self
                .own
                .as_ref()
                .is_some_and(|own| own.cycle == Some(old.id))
            {
                self.unknown();
            }
        }
        self.newest_cycle = Some(id);
        self.false_seen = false;
        self.snapshot.monitor = MonitorState::Preparing;
        if self.own.as_ref().is_some_and(|own| !own.dispatched) {
            // An external transition won before our request was dispatched.
            // Keep any real locker, but never submit a second sleep request.
            self.finish(Stage::Cancelled, Some(Failure::Cancelled));
        }
        if let Some(own) = self.own.as_mut().filter(|own| own.epoch == epoch) {
            if own.cycle.is_none() {
                own.cycle = Some(id);
            }
            if let Some(bound) = &own.lock {
                // A queued commit must not run later if somebody else began
                // this cycle first. An already submitted RPC cannot be undone.
                bound.revoke();
            }
        }
        let lock = self
            .lock
            .filter(|state| state.phase != Phase::Failed)
            .map(Binding::new);
        let awaiting_binding = lock.is_none();
        self.cycle = Some(Cycle {
            epoch,
            id,
            lock,
            awaiting_binding,
            requested_lock: awaiting_binding,
            released: false,
        });
        if awaiting_binding {
            self.pending
                .push(Effect::NeedLock(LockPurpose::External { epoch, cycle: id }));
        }
    }

    fn cycle_ended(&mut self, epoch: u64, id: Option<u64>) {
        if epoch != self.snapshot.epoch || self.snapshot.monitor == MonitorState::Unavailable {
            return;
        }
        if let Some(current) = &self.cycle {
            if id != Some(current.id) {
                return;
            }
            let current = self.cycle.take().expect("matching cycle");
            if let Some(bound) = current.lock {
                bound.revoke();
            }
            if self
                .own
                .as_ref()
                .is_some_and(|own| own.epoch == epoch && own.cycle == Some(current.id))
            {
                self.finish(Stage::Ended, None);
            }
        } else {
            if self.false_seen
                || id.is_some_and(|id| self.newest_cycle.is_some_and(|last| id <= last))
            {
                return;
            }
            if let Some(id) = id {
                self.newest_cycle = Some(id);
            }
        }
        self.false_seen = true;
        // The backend publishes Ready only after reacquiring its delay FD.
        self.snapshot.monitor = MonitorState::Connecting;
        self.pending.push(Effect::Resume);
    }

    fn bindable(&mut self, id: u64, uncertain_allowed: bool) -> Option<State> {
        if let Some(state) = self.lock.filter(|state| state.id == id) {
            return (matches!(state.phase, Phase::Starting | Phase::Locked)
                || (uncertain_allowed && state.phase == Phase::Uncertain))
                .then_some(state);
        }
        // Zero is an adopted lifecycle, never a new lock request. A completed
        // nonzero ID cannot be reused by a late binding or receipt.
        if id == 0 || id <= self.newest_lock {
            return None;
        }
        self.newest_lock = id;
        let state = State {
            id,
            phase: Phase::Starting,
        };
        self.lock = Some(state);
        Some(state)
    }

    fn update_cycle_lock(&mut self) {
        let Some(cycle) = self.cycle.as_mut() else {
            return;
        };
        let state = self.lock;
        if let Some(bound) = cycle.lock.as_mut() {
            if state
                != Some(State {
                    id: bound.id,
                    phase: Phase::Locked,
                })
            {
                bound.revoke();
                bound.observed_locked = false;
            }
        }
        if cycle.released {
            return;
        }
        match state {
            Some(state) if state.phase != Phase::Failed => {
                if cycle.lock.as_ref().is_none_or(|bound| {
                    bound.id != state.id || (state.phase == Phase::Locked && !bound.permit.valid())
                }) {
                    if let Some(old) = cycle.lock.take() {
                        old.revoke();
                    }
                    cycle.lock = Some(Binding::new(state));
                }
                cycle.awaiting_binding = false;
            }
            _ => {
                if let Some(bound) = cycle.lock.take() {
                    bound.revoke();
                }
                if !cycle.requested_lock {
                    cycle.awaiting_binding = true;
                    cycle.requested_lock = true;
                    self.pending.push(Effect::NeedLock(LockPurpose::External {
                        epoch: cycle.epoch,
                        cycle: cycle.id,
                    }));
                }
            }
        }
    }

    fn invalidate_epoch(&mut self) {
        if let Some(cycle) = self.cycle.take() {
            if let Some(bound) = cycle.lock {
                bound.revoke();
            }
        }
        self.abort(Failure::SessionUnavailable);
    }

    fn abort(&mut self, failure: Failure) {
        let Some(own) = &self.own else {
            return;
        };
        let request = own.progress.request;
        if own.dispatched {
            self.unknown();
            self.queue_cancel(request);
        } else {
            self.finish(Self::failed_stage(failure), Some(failure));
        }
    }

    fn unknown(&mut self) {
        if let Some(own) = &mut self.own {
            if let Some(bound) = &own.lock {
                bound.revoke();
            }
            own.progress.stage = Stage::Unknown;
            own.progress.failure = Some(Failure::Unknown);
            own.deadline = None;
        }
    }

    fn finish(&mut self, stage: Stage, failure: Option<Failure>) {
        if let Some(own) = self.own.take() {
            if let Some(bound) = own.lock {
                bound.revoke();
            }
            let request = own.progress.request;
            self.last_progress = Some(Progress {
                request,
                stage,
                failure,
            });
            self.queue_cancel(request);
        }
    }

    fn queue_cancel(&mut self, request: u64) {
        if !self
            .pending
            .iter()
            .any(|effect| matches!(effect, Effect::Cancel { request: id } if *id == request))
        {
            self.pending.push(Effect::Cancel { request });
        }
    }

    fn expire(&mut self, now: Instant) {
        if self
            .own
            .as_ref()
            .is_some_and(|own| own.deadline.is_some_and(|at| now >= at))
        {
            self.abort(Failure::Timeout);
        }
    }

    fn failed_stage(failure: Failure) -> Stage {
        if failure == Failure::Cancelled {
            Stage::Cancelled
        } else {
            Stage::Failed
        }
    }

    fn take_pending(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.pending)
            .into_iter()
            .filter(|effect| match effect {
                Effect::NeedLock(LockPurpose::Own(request)) => {
                    self.own.as_ref().is_some_and(|own| {
                        own.progress.request == *request && own.progress.stage == Stage::Locking
                    })
                }
                Effect::NeedLock(LockPurpose::External { epoch, cycle }) => {
                    self.cycle.as_ref().is_some_and(|current| {
                        current.epoch == *epoch
                            && current.id == *cycle
                            && current.awaiting_binding
                            && !current.released
                    })
                }
                _ => true,
            })
            .collect()
    }
}

impl Drop for Coordinator {
    fn drop(&mut self) {
        if let Some(bound) = self.own.as_ref().and_then(|own| own.lock.as_ref()) {
            bound.revoke();
        }
        if let Some(bound) = self.cycle.as_ref().and_then(|cycle| cycle.lock.as_ref()) {
            bound.revoke();
        }
    }
}

#[cfg(test)]
#[path = "unit_tests/sleep_intent.rs"]
mod tests;
