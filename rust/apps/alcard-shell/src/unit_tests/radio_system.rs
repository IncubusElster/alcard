use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

fn switch(soft: bool, hard: bool) -> Switch {
    Switch {
        index: 3,
        soft,
        hard,
    }
}

#[test]
fn power_states_distinguish_missing_unknown_mixed_and_hard_blocked() {
    let names = vec!["wlan0".into()];
    assert_eq!(wifi_state(&[], &[]), absent());
    assert_eq!(wifi_state(&[], &names).powered, None);
    assert_eq!(
        wifi_state(&[switch(false, false)], &names).powered,
        Some(true)
    );
    assert_eq!(
        wifi_state(&[switch(true, false)], &names).powered,
        Some(false)
    );
    assert_eq!(
        wifi_state(&[switch(false, false), switch(true, false)], &names).powered,
        None
    );
    assert_eq!(
        wifi_state(&[switch(false, true)], &names).availability,
        Availability::Blocked
    );
}

#[test]
fn rfkill_messages_cannot_select_an_arbitrary_radio_type() {
    assert_eq!(rfkill_event(Kind::Wifi, false), [0, 0, 0, 0, 1, 3, 1, 0]);
    assert_eq!(
        rfkill_event(Kind::Bluetooth, true),
        [0, 0, 0, 0, 2, 3, 0, 0]
    );
    for name in [
        "",
        "../wlan0",
        "--help",
        "wlan0;id",
        "abcdefghijklmnop",
        "..",
    ] {
        assert!(!interface_name(name));
    }
    assert!(interface_name("wlan0"));
}

#[test]
fn scan_parser_handles_hidden_ssids_signal_security_and_duplicate_bss() {
    let sample = "00\\:11\\:22\\:33\\:44\\:55:Home wifi:80:2412 MHz:RSN\n00\\:11\\:22\\:33\\:44\\:66::65:2412 MHz:\n00\\:11\\:22\\:33\\:44\\:55:duplicate:60:2412 MHz:RSN";
    let entries = scan_entries(sample, "wlan0").unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].name, "Home wifi");
    assert!(entries[0].detail.contains("2412 MHz"));
    assert!(entries[0].detail.contains("RSN"));
    assert_eq!(entries[1].name, "00:11:22:33:44:66");
    assert!(scan_entries("", "wlan0").unwrap().is_empty());
}

#[test]
fn scan_parser_rejects_malformed_unbounded_or_nonfinite_data() {
    for sample in [
        "command failed",
        "BSS wrong\n",
        "BSS 00:11:22:33:44:55\n\tsignal: NaN dBm",
        "BSS 00:11:22:33:44:55\n\tfreq: 0",
    ] {
        assert!(scan_entries(sample, "wlan0").is_err());
    }
    assert!(scan_entries(&"x".repeat(MAX_IO + 1), "wlan0").is_err());
}

struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        static SERIAL: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "alcard-radio-fixture-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn rfkill_fixture_reads_only_bounded_regular_exact_boolean_attributes() {
    let fixture = Fixture::new();
    let radio = fixture.0.join("rfkill3");
    fs::create_dir(&radio).unwrap();
    fs::write(radio.join("type"), "wlan\n").unwrap();
    fs::write(radio.join("soft"), "0\n").unwrap();
    fs::write(radio.join("hard"), "1\n").unwrap();
    assert_eq!(
        switches(&fixture.0, Kind::Wifi).unwrap(),
        vec![switch(false, true)]
    );
    assert!(switches(&fixture.0, Kind::Bluetooth).unwrap().is_empty());
    fs::write(radio.join("hard"), "2").unwrap();
    assert!(switches(&fixture.0, Kind::Wifi).is_err());
    fs::remove_file(radio.join("hard")).unwrap();
    std::os::unix::fs::symlink("soft", radio.join("hard")).unwrap();
    assert!(switches(&fixture.0, Kind::Wifi).is_err());
}

fn adapter_properties(powered: bool) -> Properties {
    HashMap::from([
        (
            "Address".into(),
            OwnedVariant::try_from(Variant::from("00:11:22:33:44:55")).unwrap(),
        ),
        (
            "Alias".into(),
            OwnedVariant::try_from(Variant::from("Test adapter")).unwrap(),
        ),
        ("Powered".into(), OwnedVariant::from(powered)),
    ])
}

#[test]
fn bluez_snapshot_requires_actual_power_and_valid_adapter_identity() {
    let make = |path: &str, properties| {
        HashMap::from([(
            OwnedObjectPath::try_from(path).unwrap(),
            HashMap::from([(ADAPTER.into(), properties)]),
        )])
    };
    let inventory = inventory(make("/org/bluez/hci0", adapter_properties(true))).unwrap();
    assert_eq!(
        bluetooth_state(&[switch(false, false)], &inventory).powered,
        Some(true)
    );
    assert_eq!(
        bluetooth_state(&[switch(true, false)], &inventory).powered,
        None
    );
    assert_eq!(
        bluetooth_state(&[switch(false, true)], &inventory).availability,
        Availability::Blocked
    );
    assert!(super::inventory(make("/org/attacker/hci0", adapter_properties(true))).is_err());
    let mut missing = adapter_properties(true);
    missing.remove("Powered");
    assert!(super::inventory(make("/org/bluez/hci0", missing)).is_err());
}

#[test]
fn soft_blocked_controller_stays_off_and_can_be_enabled_without_a_bluez_adapter() {
    let empty = Inventory {
        adapters: Vec::new(),
        devices: Vec::new(),
    };
    let state = bluetooth_state(&[switch(true, false)], &empty);
    assert_eq!(state.availability, Availability::Available);
    assert_eq!(state.powered, Some(false));
    // After unblock, absence of the adapter is uncertain until registration.
    assert_eq!(
        bluetooth_state(&[switch(false, false)], &empty).availability,
        Availability::Unavailable
    );
    assert_eq!(
        bluetooth_state(&[switch(true, true)], &empty).availability,
        Availability::Blocked
    );
    assert!(!all_soft_blocked(&[]));
    assert!(!all_soft_blocked(&[
        switch(true, false),
        switch(false, false)
    ]));
    let registered = Inventory {
        adapters: vec![Adapter {
            path: "/org/bluez/hci0".into(),
            address: "00:11:22:33:44:55".into(),
            name: "Adapter".into(),
            powered: true,
        }],
        devices: Vec::new(),
    };
    assert_eq!(
        bluetooth_state(&[switch(false, false)], &registered).powered,
        Some(true)
    );
}
