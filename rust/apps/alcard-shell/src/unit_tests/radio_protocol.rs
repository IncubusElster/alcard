use super::*;

fn reply(operation: Operation) -> Reply {
    Reply {
        operation,
        snapshot: Snapshot {
            wifi: State {
                availability: Availability::Available,
                powered: Some(true),
                detail: "wlan0".into(),
            },
            bluetooth: State {
                availability: Availability::Absent,
                powered: None,
                detail: String::new(),
            },
        },
        entries: Vec::new(),
        result: Ok(()),
    }
}

#[test]
fn wire_roundtrip_correlates_operation_and_real_power_readback() {
    let operation = Operation::SetPowered {
        kind: Kind::Wifi,
        powered: true,
    };
    let encoded = reply(operation).encode();
    assert!(Reply::parse(&encoded, operation).is_ok());
    assert!(Reply::parse(&encoded, Operation::Refresh).is_err());
    let mut inconsistent = reply(operation);
    inconsistent.snapshot.wifi.powered = Some(false);
    assert!(Reply::parse(&inconsistent.encode(), operation).is_err());
}

#[test]
fn absent_or_unknown_hardware_cannot_be_presented_as_powered() {
    let mut candidate = reply(Operation::Refresh);
    candidate.snapshot.bluetooth.powered = Some(false);
    assert!(Reply::parse(&candidate.encode(), Operation::Refresh).is_err());
    candidate.snapshot.bluetooth.powered = None;
    candidate.snapshot.wifi.availability = Availability::Checking;
    assert!(Reply::parse(&candidate.encode(), Operation::Refresh).is_err());
}

#[test]
fn absent_nullable_fields_are_not_silently_treated_as_success_or_unknown() {
    let mut value: Value = serde_json::from_str(&reply(Operation::Refresh).encode()).unwrap();
    let object = value.as_object_mut().unwrap();
    object.remove("error");
    object.insert("extra".into(), Value::Null);
    assert!(Reply::parse(&value.to_string(), Operation::Refresh).is_err());
    let mut value: Value = serde_json::from_str(&reply(Operation::Refresh).encode()).unwrap();
    let state = value["bluetooth"].as_object_mut().unwrap();
    state.remove("powered");
    state.insert("extra".into(), Value::Null);
    assert!(Reply::parse(&value.to_string(), Operation::Refresh).is_err());
}

#[test]
fn network_names_never_carry_controls_or_duplicate_identity() {
    let operation = Operation::List { kind: Kind::Wifi };
    let mut candidate = reply(operation);
    let entry = Entry {
        id: "wlan0/00:11:22:33:44:55".into(),
        name: "Example".into(),
        detail: "-42 dBm\n2412 MHz".into(),
    };
    candidate.entries = vec![entry.clone()];
    assert!(Reply::parse(&candidate.encode(), operation).is_ok());
    candidate.entries.push(entry);
    assert!(Reply::parse(&candidate.encode(), operation).is_err());
    candidate.entries.truncate(1);
    candidate.entries[0].name.push('\x1b');
    assert!(Reply::parse(&candidate.encode(), operation).is_err());
    assert_eq!(public_text("a\n\x1bбв", 4), "aб");
}

#[test]
fn helper_arguments_are_a_closed_set_without_paths_or_arbitrary_commands() {
    for value in [
        "status",
        "wifi-on",
        "wifi-off",
        "wifi-list",
        "bluetooth-on",
        "bluetooth-off",
        "bluetooth-list",
    ] {
        assert_eq!(Operation::parse(value).unwrap().argument(), value);
    }
    for value in [
        "",
        "--help",
        "all-on",
        "wlan0",
        "wifi-on;id",
        "/tmp/config",
        "wifi-list ",
    ] {
        assert!(Operation::parse(value).is_err());
    }
}
