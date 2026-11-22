//! The small, bounded wire contract shared by Shell and its fixed radio helper.
//! No command line, interface name, object path or credential comes from the UI.
// Each binary uses a different half of the shared wire implementation.
#![allow(dead_code)]

use serde_json::{json, Value};
use std::collections::HashSet;

pub const MAX_REPLY: usize = 64 * 1024;
pub const MAX_ENTRIES: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Wifi,
    Bluetooth,
}

impl Kind {
    pub const ALL: [Self; 2] = [Self::Wifi, Self::Bluetooth];

    pub fn name(self) -> &'static str {
        match self {
            Self::Wifi => "wifi",
            Self::Bluetooth => "bluetooth",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Availability {
    #[default]
    Checking,
    Available,
    Absent,
    Unavailable,
    Blocked,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub availability: Availability,
    pub powered: Option<bool>,
    /// An interface, adapter or connected device name, never a command error.
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub name: String,
    /// Bounded public radio facts. Passwords and pairing secrets are not read.
    pub detail: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub wifi: State,
    pub bluetooth: State,
}

impl Snapshot {
    pub fn state(&self, kind: Kind) -> &State {
        match kind {
            Kind::Wifi => &self.wifi,
            Kind::Bluetooth => &self.bluetooth,
        }
    }

    pub fn state_mut(&mut self, kind: Kind) -> &mut State {
        match kind {
            Kind::Wifi => &mut self.wifi,
            Kind::Bluetooth => &mut self.bluetooth,
        }
    }

    pub fn failed(&mut self, operation: Operation) {
        for kind in Kind::ALL {
            if operation.kind().is_none_or(|target| target == kind) {
                *self.state_mut(kind) = State {
                    availability: Availability::Unavailable,
                    powered: None,
                    detail: String::new(),
                };
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Refresh,
    SetPowered { kind: Kind, powered: bool },
    List { kind: Kind },
}

impl Operation {
    pub fn kind(self) -> Option<Kind> {
        match self {
            Self::Refresh => None,
            Self::SetPowered { kind, .. } | Self::List { kind } => Some(kind),
        }
    }

    pub fn argument(self) -> &'static str {
        match self {
            Self::Refresh => "status",
            Self::SetPowered {
                kind: Kind::Wifi,
                powered: true,
            } => "wifi-on",
            Self::SetPowered {
                kind: Kind::Wifi,
                powered: false,
            } => "wifi-off",
            Self::SetPowered {
                kind: Kind::Bluetooth,
                powered: true,
            } => "bluetooth-on",
            Self::SetPowered {
                kind: Kind::Bluetooth,
                powered: false,
            } => "bluetooth-off",
            Self::List { kind: Kind::Wifi } => "wifi-list",
            Self::List {
                kind: Kind::Bluetooth,
            } => "bluetooth-list",
        }
    }

    pub fn parse(argument: &str) -> Result<Self, Failure> {
        match argument {
            "status" => Ok(Self::Refresh),
            "wifi-on" => Ok(Self::SetPowered {
                kind: Kind::Wifi,
                powered: true,
            }),
            "wifi-off" => Ok(Self::SetPowered {
                kind: Kind::Wifi,
                powered: false,
            }),
            "bluetooth-on" => Ok(Self::SetPowered {
                kind: Kind::Bluetooth,
                powered: true,
            }),
            "bluetooth-off" => Ok(Self::SetPowered {
                kind: Kind::Bluetooth,
                powered: false,
            }),
            "wifi-list" => Ok(Self::List { kind: Kind::Wifi }),
            "bluetooth-list" => Ok(Self::List {
                kind: Kind::Bluetooth,
            }),
            _ => Err(Failure::InvalidResponse),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    Unavailable,
    Permission,
    Timeout,
    Failed,
    InvalidResponse,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub request: u64,
    pub operation: Operation,
    pub snapshot: Snapshot,
    pub entries: Vec<Entry>,
    pub result: Result<(), Failure>,
}

pub struct Reply {
    pub operation: Operation,
    pub snapshot: Snapshot,
    pub entries: Vec<Entry>,
    pub result: Result<(), Failure>,
}

fn availability_name(value: Availability) -> &'static str {
    match value {
        Availability::Checking => "checking",
        Availability::Available => "available",
        Availability::Absent => "absent",
        Availability::Unavailable => "unavailable",
        Availability::Blocked => "blocked",
    }
}

fn failure_name(value: Failure) -> &'static str {
    match value {
        Failure::Unavailable => "unavailable",
        Failure::Permission => "permission",
        Failure::Timeout => "timeout",
        Failure::Failed => "failed",
        Failure::InvalidResponse => "invalid",
    }
}

fn state_json(state: &State) -> Value {
    json!({"availability": availability_name(state.availability), "powered": state.powered, "detail": state.detail})
}

impl Reply {
    pub fn encode(&self) -> String {
        json!({
            "schema": 1,
            "operation": self.operation.argument(),
            "wifi": state_json(&self.snapshot.wifi),
            "bluetooth": state_json(&self.snapshot.bluetooth),
            "entries": self.entries.iter().map(|entry| json!({"id":entry.id,"name":entry.name,"detail":entry.detail})).collect::<Vec<_>>(),
            "error": self.result.err().map(failure_name),
        }).to_string()
    }

    pub fn parse(text: &str, expected: Operation) -> Result<Self, Failure> {
        if text.len() > MAX_REPLY {
            return Err(Failure::InvalidResponse);
        }
        let value: Value = serde_json::from_str(text).map_err(|_| Failure::InvalidResponse)?;
        let object = value.as_object().ok_or(Failure::InvalidResponse)?;
        if object.len() != 6
            || [
                "schema",
                "operation",
                "wifi",
                "bluetooth",
                "entries",
                "error",
            ]
            .iter()
            .any(|key| !object.contains_key(*key))
            || value["schema"].as_u64() != Some(1)
            || value["operation"].as_str() != Some(expected.argument())
        {
            return Err(Failure::InvalidResponse);
        }
        let result = match &value["error"] {
            Value::Null => Ok(()),
            Value::String(error) => Err(match error.as_str() {
                "unavailable" => Failure::Unavailable,
                "permission" => Failure::Permission,
                "timeout" => Failure::Timeout,
                "failed" => Failure::Failed,
                "invalid" => Failure::InvalidResponse,
                _ => return Err(Failure::InvalidResponse),
            }),
            _ => return Err(Failure::InvalidResponse),
        };
        let raw = value["entries"]
            .as_array()
            .ok_or(Failure::InvalidResponse)?;
        if raw.len() > MAX_ENTRIES
            || (!raw.is_empty() && !matches!(expected, Operation::List { .. }))
        {
            return Err(Failure::InvalidResponse);
        }
        let mut seen = HashSet::new();
        let mut entries = Vec::new();
        for item in raw {
            if item.as_object().is_none_or(|o| o.len() != 3) {
                return Err(Failure::InvalidResponse);
            }
            let id = bounded_text(&item["id"], 128, false)?;
            let name = bounded_text(&item["name"], 128, false)?;
            let detail = bounded_text(&item["detail"], 512, true)?;
            if id.is_empty() || name.is_empty() || !seen.insert(id.clone()) {
                return Err(Failure::InvalidResponse);
            }
            entries.push(Entry { id, name, detail });
        }
        let snapshot = Snapshot {
            wifi: parse_state(&value["wifi"])?,
            bluetooth: parse_state(&value["bluetooth"])?,
        };
        if let Operation::SetPowered { kind, powered } = expected {
            let state = snapshot.state(kind);
            if result.is_ok()
                && (state.availability != Availability::Available || state.powered != Some(powered))
            {
                return Err(Failure::InvalidResponse);
            }
        }
        Ok(Self {
            operation: expected,
            snapshot,
            entries,
            result,
        })
    }
}

fn bounded_text(value: &Value, maximum: usize, multiline: bool) -> Result<String, Failure> {
    let text = value.as_str().ok_or(Failure::InvalidResponse)?;
    if text.len() > maximum
        || text
            .chars()
            .any(|c| c.is_control() && !(multiline && c == '\n'))
    {
        return Err(Failure::InvalidResponse);
    }
    Ok(text.into())
}

fn parse_state(value: &Value) -> Result<State, Failure> {
    if value.as_object().is_none_or(|object| {
        object.len() != 3
            || ["availability", "powered", "detail"]
                .iter()
                .any(|key| !object.contains_key(*key))
    }) {
        return Err(Failure::InvalidResponse);
    }
    let availability = match value["availability"].as_str() {
        Some("available") => Availability::Available,
        Some("absent") => Availability::Absent,
        Some("unavailable") => Availability::Unavailable,
        Some("blocked") => Availability::Blocked,
        _ => return Err(Failure::InvalidResponse),
    };
    let powered = if value["powered"].is_null() {
        None
    } else {
        Some(value["powered"].as_bool().ok_or(Failure::InvalidResponse)?)
    };
    if matches!(
        availability,
        Availability::Absent | Availability::Unavailable
    ) && powered.is_some()
        || availability == Availability::Blocked && powered != Some(false)
    {
        return Err(Failure::InvalidResponse);
    }
    Ok(State {
        availability,
        powered,
        detail: bounded_text(&value["detail"], 128, false)?,
    })
}

/// Radio names and SSIDs are untrusted public text, never markup or commands.
pub fn public_text(text: &str, max_bytes: usize) -> String {
    let mut result = String::new();
    for c in text.chars().filter(|c| !c.is_control()) {
        if result.len() + c.len_utf8() > max_bytes {
            break;
        }
        result.push(c);
    }
    result
}

#[cfg(test)]
#[path = "unit_tests/radio_protocol.rs"]
mod tests;
