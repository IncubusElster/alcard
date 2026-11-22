//! Bounded launch observation on the Shell event loop. A spawned process is
//! not a ready window; neither timeout nor leaving Shell terminates a process.
use crate::desktop::LaunchError;
use std::{
    collections::{HashMap, HashSet},
    os::unix::process::ExitStatusExt,
    process::Child,
    time::{Duration, Instant},
};

const START_TIMEOUT: Duration = Duration::from_secs(15);
const EXIT_GRACE: Duration = Duration::from_millis(350);
const REAP_INTERVAL: Duration = Duration::from_secs(1);
const MAX_RECORDS: usize = 32;
const MAX_CHILDREN: usize = 64;
const MAX_APP_CHILDREN: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureReason {
    Spawn(LaunchError),
    EarlyExit {
        code: Option<i32>,
        signal: Option<i32>,
    },
    Observation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Starting,
    Unconfirmed,
    Failed(FailureReason),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchStatus {
    pub desktop_id: String,
    pub name: String,
    pub attempt: u64,
    pub phase: Phase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rejection {
    InvalidIdentity,
    TooManyRecords,
    TooManyChildren,
    TooManyAttempts,
    StaleAttempt,
    RetryUnavailable,
    AttemptOverflow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Started(u64),
    Suppressed(u64),
    ExistingWindow(u64),
    Rejected(Rejection),
}

struct Record {
    status: LaunchStatus,
    started: Instant,
    baseline: HashSet<u64>,
    failure: Option<(FailureReason, Instant)>,
}

struct TrackedChild {
    desktop_id: String,
    attempt: u64,
    child: Child,
}

pub struct Tracker {
    records: HashMap<String, Record>,
    children: Vec<TrackedChild>,
    next_attempt: u64,
    next_reap: Option<Instant>,
    dirty: bool,
}

impl Default for Tracker {
    fn default() -> Self {
        Self::new()
    }
}

impl Tracker {
    pub fn new() -> Self {
        Self {
            records: HashMap::new(),
            children: Vec::new(),
            next_attempt: 1,
            next_reap: None,
            dirty: false,
        }
    }

    pub fn start(
        &mut self,
        desktop_id: &str,
        name: &str,
        windows: &[(u64, String)],
        now: Instant,
        spawn: impl FnOnce() -> Result<Child, LaunchError>,
    ) -> Outcome {
        self.begin(desktop_id, name, None, windows, now, spawn)
    }

    pub fn retry(
        &mut self,
        desktop_id: &str,
        name: &str,
        expected_attempt: u64,
        windows: &[(u64, String)],
        now: Instant,
        spawn: impl FnOnce() -> Result<Child, LaunchError>,
    ) -> Outcome {
        self.begin(
            desktop_id,
            name,
            Some(expected_attempt),
            windows,
            now,
            spawn,
        )
    }

    #[allow(clippy::too_many_arguments)] // Explicit attempt, identity and observation boundaries.
    fn begin(
        &mut self,
        desktop_id: &str,
        name: &str,
        retry: Option<u64>,
        windows: &[(u64, String)],
        now: Instant,
        spawn: impl FnOnce() -> Result<Child, LaunchError>,
    ) -> Outcome {
        if desktop_id.is_empty()
            || desktop_id.len() > 4096
            || desktop_id.chars().any(char::is_control)
        {
            return Outcome::Rejected(Rejection::InvalidIdentity);
        }
        if let Some(expected) = retry {
            let Some(record) = self.records.get(desktop_id) else {
                return Outcome::Rejected(Rejection::StaleAttempt);
            };
            if record.status.attempt != expected {
                return Outcome::Rejected(Rejection::StaleAttempt);
            }
            if !matches!(record.status.phase, Phase::Failed(_) | Phase::Unconfirmed) {
                return Outcome::Rejected(Rejection::RetryUnavailable);
            }
        }
        // Root supplies already matched identities in MRU order. Never guess
        // from process name, title or PID, and never activate a window here.
        if let Some((id, _)) = windows.iter().find(|(_, app)| app == desktop_id) {
            self.dirty |= self.records.remove(desktop_id).is_some();
            return Outcome::ExistingWindow(*id);
        }
        if retry.is_none() {
            if let Some(record) = self.records.get(desktop_id) {
                return Outcome::Suppressed(record.status.attempt);
            }
        }
        if self.children.len() >= MAX_CHILDREN {
            return Outcome::Rejected(Rejection::TooManyChildren);
        }
        if self
            .children
            .iter()
            .filter(|child| child.desktop_id == desktop_id)
            .count()
            >= MAX_APP_CHILDREN
        {
            return Outcome::Rejected(Rejection::TooManyAttempts);
        }
        let attempt = self.next_attempt;
        let Some(next) = attempt.checked_add(1) else {
            return Outcome::Rejected(Rejection::AttemptOverflow);
        };
        if !self.records.contains_key(desktop_id) && self.records.len() >= MAX_RECORDS {
            // Keep all outstanding/uncertain launches guarded. Only a finished
            // failure without surviving earlier children can be evicted.
            let expired = self
                .records
                .iter()
                .filter(|(id, record)| {
                    matches!(record.status.phase, Phase::Failed(_))
                        && !self.children.iter().any(|child| &child.desktop_id == *id)
                })
                .min_by_key(|(_, record)| record.status.attempt)
                .map(|(id, _)| id.clone());
            if let Some(expired) = expired {
                self.records.remove(&expired);
            } else {
                return Outcome::Rejected(Rejection::TooManyRecords);
            }
        }
        self.next_attempt = next;
        let phase = match spawn() {
            Ok(child) => {
                self.children.push(TrackedChild {
                    desktop_id: desktop_id.into(),
                    attempt,
                    child,
                });
                self.next_reap = Some(self.next_reap.map_or(now, |deadline| deadline.min(now)));
                Phase::Starting
            }
            Err(error) => Phase::Failed(FailureReason::Spawn(error)),
        };
        self.records.insert(
            desktop_id.into(),
            Record {
                status: LaunchStatus {
                    desktop_id: desktop_id.into(),
                    name: name
                        .chars()
                        .filter(|ch| !ch.is_control())
                        .take(96)
                        .collect(),
                    attempt,
                    phase,
                },
                started: now,
                // Save every existing ID, including currently unmatched windows.
                // A pre-existing window changing its app_id is not launch readiness.
                baseline: windows.iter().map(|(id, _)| *id).take(128).collect(),
                failure: None,
            },
        );
        self.dirty = true;
        Outcome::Started(attempt)
    }

    /// Observe only authoritative desktop IDs matched by the caller. Ready
    /// removes the launch record; old children are still reaped independently.
    pub fn windows(&mut self, windows: &[(u64, String)]) {
        let before = self.records.len();
        self.records.retain(|desktop_id, record| {
            !windows
                .iter()
                .any(|(id, app)| app == desktop_id && !record.baseline.contains(id))
        });
        self.dirty |= self.records.len() != before;
    }

    fn failure(&mut self, desktop_id: &str, attempt: u64, reason: FailureReason, now: Instant) {
        if let Some(record) = self
            .records
            .get_mut(desktop_id)
            .filter(|record| record.status.attempt == attempt)
        {
            record.failure = Some((reason, now + EXIT_GRACE));
        }
    }

    pub fn tick(&mut self, now: Instant) {
        if self.next_reap.is_some_and(|deadline| now >= deadline) {
            let mut outcomes = Vec::new();
            self.children
                .retain_mut(|tracked| match tracked.child.try_wait() {
                    Ok(Some(status)) => {
                        if !status.success() {
                            outcomes.push((
                                tracked.desktop_id.clone(),
                                tracked.attempt,
                                FailureReason::EarlyExit {
                                    code: status.code(),
                                    signal: status.signal(),
                                },
                            ));
                        }
                        false
                    }
                    Ok(None) => true,
                    Err(_) => {
                        outcomes.push((
                            tracked.desktop_id.clone(),
                            tracked.attempt,
                            FailureReason::Observation,
                        ));
                        false
                    }
                });
            for (id, attempt, reason) in outcomes {
                self.failure(&id, attempt, reason, now);
            }
            self.next_reap = (!self.children.is_empty()).then_some(now + REAP_INTERVAL);
        }
        for record in self.records.values_mut() {
            if let Some((reason, deadline)) = record.failure {
                if now >= deadline {
                    record.failure = None;
                    record.status.phase = Phase::Failed(reason);
                    self.dirty = true;
                }
            } else if record.status.phase == Phase::Starting
                && now.saturating_duration_since(record.started) >= START_TIMEOUT
            {
                record.status.phase = Phase::Unconfirmed;
                self.dirty = true;
            }
        }
    }

    pub fn take_update(&mut self) -> Option<Vec<LaunchStatus>> {
        if !std::mem::take(&mut self.dirty) {
            return None;
        }
        let mut statuses: Vec<_> = self
            .records
            .values()
            .map(|record| record.status.clone())
            .collect();
        statuses.sort_by(|a, b| a.desktop_id.cmp(&b.desktop_id));
        Some(statuses)
    }

    pub fn next_wakeup(&self, now: Instant) -> Option<Duration> {
        let mut deadline = self.next_reap;
        for record in self.records.values() {
            let next = record.failure.map(|(_, when)| when).or_else(|| {
                (record.status.phase == Phase::Starting).then_some(record.started + START_TIMEOUT)
            });
            if let Some(next) = next {
                deadline = Some(deadline.map_or(next, |old| old.min(next)));
            }
        }
        deadline.map(|when| when.saturating_duration_since(now))
    }
}

#[cfg(test)]
#[path = "unit_tests/launch.rs"]
mod tests;
