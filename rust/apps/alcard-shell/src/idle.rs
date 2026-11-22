//! Event-driven idle policy. The compositor observes activity on every seat;
//! this module never infers global idle from Shell keyboard focus or a clock.
//! A display-off decision requires an actual locker lifecycle receipt, never
//! the advisory D-Bus name or recovery journal.
use crate::locking::{Message, Phase, State};
use alcard_ui::preferences::IdlePreferences;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Timer {
    Lock,
    ScreenOff,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Decision {
    pub request_lock: bool,
    pub screen_off: bool,
}

#[derive(Default)]
struct Seat {
    lock_idle: bool,
    screen_idle: bool,
}

pub struct Policy {
    config: IdlePreferences,
    seats: BTreeMap<u32, Seat>,
    attempted: bool,
    lifecycle: Option<State>,
    // Nonzero IDs increase within this Shell. Zero is reserved by the lock
    // worker for adopting an abandoned external lock, not a UI request ID.
    newest_request: u64,
}

impl Policy {
    pub fn new(config: IdlePreferences) -> Self {
        Self {
            config,
            seats: BTreeMap::new(),
            attempted: false,
            lifecycle: None,
            newest_request: 0,
        }
    }

    pub fn reconfigure(&mut self, config: IdlePreferences) {
        self.config = config;
        self.restart_idle_period();
        // A preference change cannot unlock or invalidate a native lock.
        // Its valid receipt remains useful after the new idle interval.
    }

    pub fn add_seat(&mut self, seat: u32) {
        if let std::collections::btree_map::Entry::Vacant(entry) = self.seats.entry(seat) {
            entry.insert(Seat::default());
            // A newly connected seat has not been idle for either timeout.
            self.attempted = false;
        }
    }

    pub fn remove_seat(&mut self, seat: u32) {
        self.seats.remove(&seat);
    }

    pub fn notify(&mut self, seat: u32, timer: Timer, idled: bool) {
        let Some(state) = self.seats.get_mut(&seat) else {
            return;
        };
        if !idled {
            // Either resumed notification proves activity for the whole seat.
            // In particular it defeats a late lock receipt after screen idle.
            *state = Seat::default();
            self.attempted = false;
            return;
        }
        match timer {
            Timer::Lock => state.lock_idle = true,
            Timer::ScreenOff => state.screen_idle = true,
        }
    }

    /// Returns true only when the canonical lifecycle for the current lock
    /// finishes. The caller then recreates compositor notifications so the
    /// authenticated session starts a fresh idle interval and old-generation
    /// events cannot immediately lock it again.
    pub fn receive_lock(&mut self, message: &Message) -> bool {
        match *message {
            Message::Presence(_) | Message::Journal(_) => {}
            Message::Changed(state) => {
                if state.id == 0 {
                    if self
                        .lifecycle
                        .is_some_and(|current| current.id != 0 && current.phase != Phase::Failed)
                    {
                        return false;
                    }
                } else {
                    if state.id < self.newest_request
                        || (state.id == self.newest_request
                            && self.lifecycle.is_none_or(|current| current.id != state.id))
                    {
                        return false;
                    }
                    self.newest_request = state.id;
                }
                // A safe pre-request failure is terminal for that request ID.
                // An uncertain request, however, may later confirm its lock
                // or be recovered by a fresh locker with the same ID.
                if self.lifecycle.is_some_and(|current| {
                    current.id == state.id && current.phase == Phase::Failed && state.id != 0
                }) {
                    return false;
                }
                self.lifecycle = Some(state);
            }
            Message::Unlocked { id } => {
                if self.lifecycle.is_some_and(|current| {
                    current.id == id && matches!(current.phase, Phase::Locked | Phase::Uncertain)
                }) {
                    self.lifecycle = None;
                    self.restart_idle_period();
                    return true;
                }
            }
        }
        false
    }

    pub fn evaluate(&mut self, lock_busy: bool) -> Decision {
        if self.seats.is_empty() {
            return Decision::default();
        }
        let lifecycle_busy = self
            .lifecycle
            .is_some_and(|state| state.phase != Phase::Failed);
        let request_lock = self.config.lock_after_seconds != 0
            && self.seats.values().all(|seat| seat.lock_idle)
            && !self.attempted
            && !lock_busy
            && !lifecycle_busy;
        if request_lock {
            // Set before handing control to the caller. A missing executable
            // or other immediate failure must not spin the dispatch loop.
            self.attempted = true;
        }
        Decision {
            request_lock,
            screen_off: self.config.screen_off_after_seconds != 0
                && self.seats.values().all(|seat| seat.screen_idle)
                && self
                    .lifecycle
                    .is_some_and(|state| state.phase == Phase::Locked),
        }
    }

    fn restart_idle_period(&mut self) {
        for seat in self.seats.values_mut() {
            *seat = Seat::default();
        }
        self.attempted = false;
    }
}

#[cfg(test)]
#[path = "unit_tests/idle.rs"]
mod tests;
