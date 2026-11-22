//! Pure low-charge policy. Observations never execute a system operation: an
//! automatic request must pass through the existing lock-before-sleep path.
use crate::status::{BatteryObservation, BatteryState, ChargeState};
use alcard_ui::preferences::BatteryPreferences;
use std::time::{Duration, Instant};

pub const MAX_SAMPLE_AGE: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Low,
    Critical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inhibit {
    Attempt,
    Resume,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    Warn {
        level: Level,
        percent: u8,
        episode: u64,
        announce: bool,
    },
    ClearWarning,
    RequestSuspend {
        episode: u64,
    },
    CancelSuspend {
        episode: u64,
    },
}

pub struct Policy {
    preferences: BatteryPreferences,
    latest: Option<BatteryObservation>,
    sequence: Option<u64>,
    observed_at: Option<Instant>,
    resume_barrier: Option<Instant>,
    source_epoch: Option<u64>,
    episode: u64,
    low_warned: bool,
    critical_warned: bool,
    critical_band: bool,
    visible_warning: Option<(Level, u8)>,
    action_latched: bool,
    resume_blocked: bool,
    action_pending: bool,
    recovering: bool,
}

impl Policy {
    pub fn new(preferences: BatteryPreferences) -> Self {
        Self {
            preferences: preferences.validate().unwrap_or_default(),
            latest: None,
            sequence: None,
            observed_at: None,
            resume_barrier: None,
            source_epoch: None,
            episode: 1,
            low_warned: false,
            critical_warned: false,
            critical_band: false,
            visible_warning: None,
            action_latched: false,
            resume_blocked: false,
            action_pending: false,
            recovering: false,
        }
    }

    pub fn observe(&mut self, observation: BatteryObservation, now: Instant) -> Vec<Effect> {
        let mut effects = self.tick(now);
        // Duplicate or delayed delivery cannot replace a newer observation or
        // refresh its age. The worker numbers every sample, even equal values.
        if self
            .sequence
            .is_some_and(|sequence| observation.sequence <= sequence)
        {
            return effects;
        }
        self.sequence = Some(observation.sequence);
        if !fresh(observation, now)
            || self
                .observed_at
                .is_some_and(|last| observation.observed_at < last)
            || self
                .resume_barrier
                .is_some_and(|barrier| observation.observed_at < barrier)
        {
            self.invalidate(&mut effects);
            return effects;
        }
        self.observed_at = Some(observation.observed_at);

        if let Some(source) = observation.source_epoch {
            if self.source_epoch.is_some_and(|old| old != source) {
                self.invalidate(&mut effects);
                // A hotplug is not permission to retry an earlier failed or
                // cancelled sleep. Only real charge/recovery clears that latch.
                self.advance_episode();
                self.low_warned = false;
                self.critical_warned = false;
                self.critical_band = false;
                self.recovering = false;
            }
            self.source_epoch = Some(source);
        }

        let charging = observation.source_epoch.is_some()
            && matches!(
                observation.state,
                BatteryState::Present {
                    charge: ChargeState::Charging | ChargeState::Full,
                    ..
                }
            );
        if observation.external_power == Some(true) || charging {
            self.recover(&mut effects);
            return effects;
        }
        let Some(percent) = discharging(observation) else {
            self.invalidate(&mut effects);
            return effects;
        };
        self.latest = Some(observation);
        if percent > self.preferences.low_percent && percent >= self.preferences.recovery_percent()
        {
            self.recover(&mut effects);
            return effects;
        }
        self.recovering = false;
        let critical_recovery = self.preferences.critical_percent.saturating_add(3).min(100);
        if percent >= critical_recovery {
            self.resume_blocked = false;
        }
        if percent <= self.preferences.critical_percent {
            self.critical_band = true;
        } else if percent >= critical_recovery {
            self.critical_band = false;
        }
        let level = if self.critical_band {
            Some(Level::Critical)
        } else if percent <= self.preferences.low_percent || self.low_warned {
            Some(Level::Low)
        } else {
            None
        };
        if let Some(level) = level {
            if self.visible_warning != Some((level, percent)) {
                let announce = match level {
                    Level::Low => !self.low_warned,
                    Level::Critical => !self.critical_warned,
                };
                self.low_warned = true;
                self.critical_warned |= level == Level::Critical;
                self.visible_warning = Some((level, percent));
                effects.push(Effect::Warn {
                    level,
                    percent,
                    episode: self.episode,
                    announce,
                });
            }
        }

        if percent > self.preferences.critical_percent {
            self.cancel_action(&mut effects);
        } else if self.preferences.suspend_on_critical
            && !self.action_latched
            && !self.resume_blocked
        {
            // Latch before handing off. Failure, denial, loss of telemetry and
            // cancellation must never create an automatic retry loop.
            self.action_latched = true;
            self.action_pending = true;
            effects.push(Effect::RequestSuspend {
                episode: self.episode,
            });
        }
        effects
    }

    pub fn tick(&mut self, now: Instant) -> Vec<Effect> {
        let mut effects = Vec::new();
        if self
            .latest
            .is_some_and(|observation| !fresh(observation, now))
        {
            self.invalidate(&mut effects);
        }
        effects
    }

    /// A manual wake must not immediately put the user back to sleep. A fresh
    /// reading above the critical band releases this temporary inhibition;
    /// a consumed automatic attempt remains latched for the whole episode.
    pub fn resume(&mut self, now: Instant) -> Vec<Effect> {
        let mut effects = Vec::new();
        self.invalidate(&mut effects);
        self.resume_barrier = Some(now);
        self.resume_blocked = true;
        self.recovering = false;
        effects
    }

    pub fn restore_inhibit(&mut self, inhibit: Inhibit, now: Instant) -> Vec<Effect> {
        let effects = self.resume(now);
        if inhibit == Inhibit::Attempt {
            self.action_latched = true;
        }
        effects
    }

    pub fn inhibit(&self) -> Option<Inhibit> {
        if self.action_latched() {
            Some(Inhibit::Attempt)
        } else if self.resume_blocked {
            Some(Inhibit::Resume)
        } else {
            None
        }
    }

    /// Preferences arrive as a complete validated candidate. Reconfiguration
    /// neither replays an old sample nor rearms a consumed automatic attempt.
    pub fn reconfigure(&mut self, preferences: BatteryPreferences, now: Instant) -> Vec<Effect> {
        let Ok(preferences) = preferences.validate() else {
            return Vec::new();
        };
        if preferences == self.preferences {
            return self.tick(now);
        }
        let mut effects = Vec::new();
        self.invalidate(&mut effects);
        self.preferences = preferences;
        self.resume_barrier = Some(now);
        effects
    }

    /// Recheck immediately before committing a request, after the complete
    /// event batch has applied charging, invalidation and resume observations.
    pub fn is_action_current(&self, episode: u64, now: Instant) -> bool {
        self.action_pending
            && self.episode == episode
            && self.preferences.suspend_on_critical
            && !self.resume_blocked
            && self.latest.is_some_and(|observation| {
                fresh(observation, now)
                    && discharging(observation)
                        .is_some_and(|percent| percent <= self.preferences.critical_percent)
            })
    }

    /// Completion includes failure or refusal. A new sample does not retry it.
    pub fn action_finished(&mut self, episode: u64) {
        if episode == self.episode {
            self.action_pending = false;
        }
    }

    pub fn action_latched(&self) -> bool {
        self.action_latched
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.latest
            .filter(|_| self.visible_warning.is_some() || self.action_pending)
            .and_then(|observation| observation.observed_at.checked_add(MAX_SAMPLE_AGE))
    }

    fn cancel_action(&mut self, effects: &mut Vec<Effect>) {
        if std::mem::take(&mut self.action_pending) {
            effects.push(Effect::CancelSuspend {
                episode: self.episode,
            });
        }
    }

    fn invalidate(&mut self, effects: &mut Vec<Effect>) {
        self.latest = None;
        self.cancel_action(effects);
        if self.visible_warning.take().is_some() {
            effects.push(Effect::ClearWarning);
        }
    }

    fn advance_episode(&mut self) {
        // Never recycle a token which might still be held by an async caller.
        if let Some(next) = self.episode.checked_add(1) {
            self.episode = next;
        } else {
            self.action_latched = true;
        }
    }

    fn recover(&mut self, effects: &mut Vec<Effect>) {
        self.invalidate(effects);
        self.resume_blocked = false;
        self.critical_band = false;
        if !self.recovering {
            self.advance_episode();
            self.low_warned = false;
            self.critical_warned = false;
            self.action_latched = self.episode == u64::MAX;
        }
        self.recovering = true;
    }
}

fn fresh(observation: BatteryObservation, now: Instant) -> bool {
    now.checked_duration_since(observation.observed_at)
        .is_some_and(|age| age < MAX_SAMPLE_AGE)
}

fn discharging(observation: BatteryObservation) -> Option<u8> {
    if observation.source_epoch.is_none() || observation.external_power == Some(true) {
        return None;
    }
    match observation.state {
        BatteryState::Present {
            percent: Some(percent @ 0..=100),
            charge: ChargeState::Discharging,
        } => Some(percent),
        _ => None,
    }
}

#[cfg(test)]
#[path = "unit_tests/battery.rs"]
mod tests;
