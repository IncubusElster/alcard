use super::*;
use std::sync::atomic::AtomicUsize;
static NEXT: AtomicUsize = AtomicUsize::new(0);
const HEADER: &str = "Iface Destination Gateway Flags RefCnt Use Metric Mask MTU Window IRTT\n";

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "alcard-status-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn write(&self, path: &str, value: &str) {
        let path = self.0.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, value).unwrap();
    }
    fn dir(&self, path: &str) {
        fs::create_dir_all(self.0.join(path)).unwrap();
    }
    fn system_battery(&self, name: &str, percent: u8, charge: &str) {
        self.write(&format!("power/{name}/type"), "Battery\n");
        self.write(&format!("power/{name}/present"), "1\n");
        self.write(&format!("power/{name}/capacity"), &percent.to_string());
        self.write(&format!("power/{name}/status"), charge);
    }
    fn sources(&self) -> Sources {
        Sources {
            power: self.0.join("power"),
            net: self.0.join("net"),
            rfkill: self.0.join("rfkill"),
            wireless: self.0.join("wireless"),
            route4: self.0.join("route4"),
            route6: self.0.join("route6"),
        }
    }
    fn interface(&self, name: &str, wifi: bool) {
        self.write(&format!("net/{name}/type"), "1\n");
        self.write(&format!("net/{name}/flags"), "0x1003\n");
        self.write(&format!("net/{name}/carrier"), "1\n");
        self.write(&format!("net/{name}/operstate"), "up\n");
        self.dir(&format!(
            "net/{name}/{}",
            if wifi { "wireless" } else { "device" }
        ));
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn known(name: &str, ipv4: bool, ipv6: bool) -> AddressSnapshot {
    AddressSnapshot::Known(HashMap::from([(name.into(), Families { ipv4, ipv6 })]))
}

#[test]
fn battery_absent_unknown_and_real_zero_are_distinct() {
    let f = Fixture::new();
    assert_eq!(battery(&f.0.join("power")), BatteryState::Unknown);
    f.dir("power");
    assert_eq!(battery(&f.0.join("power")), BatteryState::Absent);
    f.write("power/BAT0/type", "Battery\n");
    f.write("power/BAT0/present", "1\n");
    f.write("power/BAT0/capacity", "0\n");
    f.write("power/BAT0/status", "Discharging\n");
    assert_eq!(
        battery(&f.0.join("power")),
        BatteryState::Present {
            percent: Some(0),
            charge: ChargeState::Discharging
        }
    );
    f.write("power/BAT0/present", "0\n");
    assert_eq!(battery(&f.0.join("power")), BatteryState::Absent);
}

#[test]
fn battery_rejects_invalid_capacity_and_unknown_presence() {
    let f = Fixture::new();
    f.write("power/BAT0/type", "Battery");
    f.write("power/BAT0/present", "1");
    f.write("power/BAT0/capacity", "101");
    f.write("power/BAT0/status", "Charging");
    assert_eq!(
        battery(&f.0.join("power")),
        BatteryState::Present {
            percent: None,
            charge: ChargeState::Charging
        }
    );
    f.write("power/BAT0/capacity", "-1");
    f.write("power/BAT0/present", "unreadable");
    assert_eq!(battery(&f.0.join("power")), BatteryState::Unknown);
}

#[test]
fn optional_present_uses_actual_reading_not_an_invented_charge() {
    let f = Fixture::new();
    f.write("power/BAT0/type", "Battery");
    assert_eq!(battery(&f.0.join("power")), BatteryState::Unknown);
    f.write("power/BAT0/capacity", "42");
    assert_eq!(
        battery(&f.0.join("power")),
        BatteryState::Present {
            percent: Some(42),
            charge: ChargeState::Unknown
        }
    );
    f.write("power/BAT0/scope", "Device");
    assert_eq!(battery(&f.0.join("power")), BatteryState::Absent);
}

#[test]
fn read_errors_and_oversized_data_do_not_look_like_absent_hardware() {
    let f = Fixture::new();
    f.dir("power/BAT0/type"); // EISDIR is reproducible even in root CI.
    assert_eq!(battery(&f.0.join("power")), BatteryState::Unknown);
    f.write("oversized", &"0".repeat(4097));
    assert_eq!(
        small(&f.0.join("oversized")).unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
    let (wifi, ethernet) = f
        .sources()
        .networks(&AddressSnapshot::Known(HashMap::new()));
    assert_eq!(wifi.state, LinkState::Unknown);
    assert_eq!(ethernet.state, LinkState::Unknown);
}

#[test]
fn repeated_battery_values_still_confirm_fresh_monotonic_observations() {
    let f = Fixture::new();
    f.system_battery("BAT0", 5, "Discharging");
    let mut sampler = Sampler::default();
    let addresses = AddressSnapshot::Known(HashMap::new());
    let before = Instant::now();
    let first = sampler.sample(&f.sources(), &addresses).unwrap();
    let after = Instant::now();
    let second = sampler.sample(&f.sources(), &addresses).unwrap();
    assert_eq!(first.status, second.status);
    assert!(first.battery.observed_at >= before && first.battery.observed_at <= after);
    assert!(second.battery.observed_at >= after);
    assert!(second.battery.sequence > first.battery.sequence);
    assert_eq!(first.battery.source_epoch, Some(1));
    assert_eq!(first.battery.source_epoch, second.battery.source_epoch);
    assert_eq!(first.battery.external_power, None);
}

#[test]
fn source_replacement_changes_epoch_but_an_unknown_gap_does_not_rearm_the_same_source() {
    let f = Fixture::new();
    f.system_battery("BAT0", 4, "Discharging");
    let mut sampler = Sampler::default();
    let addresses = AddressSnapshot::Known(HashMap::new());
    let first = sampler.sample(&f.sources(), &addresses).unwrap();
    // Keep the old inode alive outside the inventory: this is an actual
    // replacement, without depending on a filesystem's inode reuse timing.
    fs::rename(f.0.join("power/BAT0"), f.0.join("old-battery")).unwrap();
    f.system_battery("BAT0", 4, "Discharging");
    let replacement = sampler.sample(&f.sources(), &addresses).unwrap();
    assert_eq!(first.status, replacement.status);
    assert!(replacement.battery.source_epoch > first.battery.source_epoch);
    f.write("power/BAT0/status", "unknown");
    let invalid = sampler.sample(&f.sources(), &addresses).unwrap();
    assert_eq!(invalid.battery.source_epoch, None);
    f.write("power/BAT0/status", "Discharging");
    let restored = sampler.sample(&f.sources(), &addresses).unwrap();
    assert_eq!(
        restored.battery.source_epoch,
        replacement.battery.source_epoch
    );
    assert!(restored.battery.sequence > invalid.battery.sequence);
}

#[test]
fn multiple_system_batteries_and_invalid_scope_cannot_authorize_an_action() {
    let f = Fixture::new();
    f.system_battery("BAT0", 3, "Discharging");
    f.system_battery("BAT1", 80, "Discharging");
    let multiple = power_reading(&f.0.join("power"));
    assert!(matches!(
        multiple.state,
        BatteryState::Present {
            percent: Some(3),
            ..
        }
    ));
    assert!(multiple.source.is_none());
    f.write("power/BAT1/scope", "Device");
    assert!(power_reading(&f.0.join("power")).source.is_some());
    f.write("power/BAT0/scope", "invalid");
    let invalid = power_reading(&f.0.join("power"));
    assert_eq!(invalid.state, BatteryState::Unknown);
    assert!(invalid.source.is_none());
}

#[test]
fn external_power_online_is_observed_and_unknown_online_never_means_unplugged() {
    let f = Fixture::new();
    f.system_battery("BAT0", 3, "Discharging");
    f.write("power/AC/type", "Mains");
    f.write("power/AC/online", "0");
    let unplugged = power_reading(&f.0.join("power"));
    assert_eq!(unplugged.external_power, Some(false));
    assert!(unplugged.source.is_some());
    f.write("power/AC/online", "1");
    let plugged = power_reading(&f.0.join("power"));
    assert_eq!(plugged.external_power, Some(true));
    assert!(plugged.source.is_some());
    f.write("power/AC/online", "?");
    let unknown = power_reading(&f.0.join("power"));
    assert_eq!(unknown.external_power, None);
    assert!(unknown.source.is_none());
    fs::remove_file(f.0.join("power/AC/online")).unwrap();
    assert!(power_reading(&f.0.join("power")).source.is_none());
}

#[test]
fn charging_and_source_transitions_between_reads_do_not_produce_a_trusted_sample() {
    let f = Fixture::new();
    f.system_battery("BAT0", 3, "Discharging");
    let before = power_pass(&f.0.join("power")).unwrap();
    f.write("power/BAT0/status", "Charging");
    let charging = power_pass(&f.0.join("power")).unwrap();
    assert!(combine_power_passes(&before, &charging).source.is_none());
    fs::rename(f.0.join("power/BAT0"), f.0.join("old-battery")).unwrap();
    f.system_battery("BAT0", 3, "Discharging");
    let replacement = power_pass(&f.0.join("power")).unwrap();
    assert_eq!(before[0].battery, replacement[0].battery);
    assert_ne!(before[0].source, replacement[0].source);
    assert!(combine_power_passes(&before, &replacement).source.is_none());
    f.write("power/AC/type", "USB");
    f.write("power/AC/online", "1");
    let inserted = power_pass(&f.0.join("power")).unwrap();
    let result = combine_power_passes(&replacement, &inserted);
    assert_eq!(result.external_power, None);
    assert!(result.source.is_none());
}

#[test]
fn absent_present_attribute_requires_complete_battery_data_for_action_permission() {
    let f = Fixture::new();
    f.system_battery("BAT0", 0, "Discharging");
    fs::remove_file(f.0.join("power/BAT0/present")).unwrap();
    assert!(power_reading(&f.0.join("power")).source.is_some());
    f.write("power/BAT0/capacity", "101");
    assert!(power_reading(&f.0.join("power")).source.is_none());
    f.write("power/BAT0/capacity", "0");
    fs::remove_file(f.0.join("power/BAT0/status")).unwrap();
    assert!(power_reading(&f.0.join("power")).source.is_none());
}

#[test]
fn sequence_and_source_epoch_exhaustion_never_wrap() {
    let f = Fixture::new();
    f.system_battery("BAT0", 1, "Discharging");
    let addresses = AddressSnapshot::Known(HashMap::new());
    let mut sequence = Sampler {
        sequence: u64::MAX,
        ..Sampler::default()
    };
    assert!(sequence.sample(&f.sources(), &addresses).is_none());
    let mut epoch = Sampler {
        epoch: u64::MAX,
        ..Sampler::default()
    };
    assert!(epoch.sample(&f.sources(), &addresses).is_none());
}

#[test]
fn refresh_is_bounded_coalesced_and_refuses_a_stopped_worker() {
    let (commands, incoming) = mpsc::sync_channel(1);
    let worker = Worker {
        mailbox: Arc::new(Mailbox {
            update: Mutex::new(None),
            pending: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
        }),
        commands,
        thread: None,
    };
    for _ in 0..100 {
        assert!(worker.refresh());
    }
    assert!(matches!(incoming.try_recv(), Ok(Command::Refresh)));
    assert!(matches!(
        incoming.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    worker.mailbox.stopped.store(true, Ordering::Release);
    assert!(!worker.refresh());
}

#[test]
fn no_adapters_and_virtual_ethernet_are_not_physical_connectivity() {
    let f = Fixture::new();
    f.dir("net");
    f.write("net/veth0/type", "1");
    f.write("net/lo/type", "772");
    let (wifi, ethernet) = f
        .sources()
        .networks(&AddressSnapshot::Known(HashMap::new()));
    assert_eq!(wifi.state, LinkState::Absent);
    assert_eq!(ethernet.state, LinkState::Absent);
}

#[test]
fn negative_rssi_boundaries_do_not_use_positive_or_unknown_levels() {
    for (value, bars) in [
        ("-1", 4),
        ("-50.", 4),
        ("-51", 3),
        ("-65", 3),
        ("-66", 2),
        ("-75", 2),
        ("-76", 1),
        ("-85", 1),
        ("-86", 0),
        ("-127", 0),
    ] {
        assert_eq!(signal_bars(value), Some(bars), "{value}");
    }
    for value in ["0", "70", "216", "-128", "-256", "unknown", ""] {
        assert_eq!(signal_bars(value), None, "{value}");
    }
    let values =
        wireless_strengths("Inter-| sta\n wlan0: 0000 70. -65. -256 0 0\n wlan1: 0000 0. 0. 0 0\n");
    assert_eq!(values.get("wlan0"), Some(&Some(3)));
    assert_eq!(values.get("wlan1"), Some(&None));
}

#[test]
fn wifi_link_address_route_and_ethernet_are_independent() {
    let f = Fixture::new();
    f.interface("wlan0", true);
    f.interface("eth0", false);
    f.write("wireless", "wlan0: 0000 60. -65. -256 0\n");
    f.write(
        "route4",
        &(HEADER.to_owned() + "eth0 00000000 0101A8C0 0003 0 0 10 00000000 0 0 0\n"),
    );
    f.write("route6", "");
    let addr = known("eth0", true, false);
    let (wifi, ethernet) = f.sources().networks(&addr);
    assert_eq!(
        wifi,
        NetworkStatus {
            state: LinkState::LinkOnly,
            strength: Some(3)
        }
    );
    assert_eq!(
        ethernet,
        NetworkStatus {
            state: LinkState::Routed,
            strength: None
        }
    );
    let (wifi, _) = f.sources().networks(&known("wlan0", true, false));
    assert_eq!(wifi.state, LinkState::Addressed);
}

#[test]
fn rfkill_admin_down_disconnected_and_connecting_remain_distinct() {
    let f = Fixture::new();
    f.interface("wlan0", true);
    let addr = AddressSnapshot::Known(HashMap::new());
    f.write("net/wlan0/carrier", "0");
    f.write("net/wlan0/operstate", "down");
    assert_eq!(f.sources().networks(&addr).0.state, LinkState::Disconnected);
    f.write("net/wlan0/operstate", "dormant");
    assert_eq!(f.sources().networks(&addr).0.state, LinkState::Connecting);
    f.write("net/wlan0/flags", "0x1002");
    assert_eq!(f.sources().networks(&addr).0.state, LinkState::Disabled);
    f.write("net/wlan0/flags", "0x1003");
    f.write("rfkill/rfkill0/type", "wlan");
    f.write("rfkill/rfkill0/soft", "1");
    f.write("rfkill/rfkill0/hard", "0");
    assert_eq!(f.sources().networks(&addr).0.state, LinkState::Disabled);
}

#[test]
fn per_radio_rfkill_does_not_disable_a_different_wifi_adapter() {
    let f = Fixture::new();
    f.interface("wlan0", true);
    f.write("rfkill/rfkill0/type", "wlan");
    f.write("rfkill/rfkill0/soft", "1");
    f.write("rfkill/rfkill0/hard", "0");
    f.write("net/wlan0/phy80211/rfkill1/soft", "0");
    f.write("net/wlan0/phy80211/rfkill1/hard", "0");
    assert_eq!(
        f.sources()
            .networks(&AddressSnapshot::Known(HashMap::new()))
            .0
            .state,
        LinkState::LinkOnly
    );
}

#[test]
fn carrier_read_error_and_address_read_error_never_claim_disconnected_or_routed() {
    let f = Fixture::new();
    f.interface("eth0", false);
    assert_eq!(
        f.sources().networks(&AddressSnapshot::Unknown).1.state,
        LinkState::Unknown
    );
    f.write("net/eth0/carrier", "?");
    f.write("net/eth0/operstate", "unknown");
    assert_eq!(
        f.sources().networks(&known("eth0", true, false)).1.state,
        LinkState::Unknown
    );
}

#[test]
fn address_filters_keep_lan_ula_and_global_but_not_link_local_or_multicast() {
    for ip in ["192.168.1.2", "10.0.0.2", "8.8.8.8"] {
        assert!(configured_v4(ip.parse().unwrap()));
    }
    for ip in [
        "0.0.0.0",
        "127.0.0.1",
        "169.254.1.2",
        "224.0.0.1",
        "255.255.255.255",
    ] {
        assert!(!configured_v4(ip.parse().unwrap()));
    }
    for ip in ["fd00::1", "2001:db8::1"] {
        assert!(configured_v6(ip.parse().unwrap()));
    }
    for ip in ["::", "::1", "fe80::1", "ff02::1"] {
        assert!(!configured_v6(ip.parse().unwrap()));
    }
}

#[test]
fn route_requires_default_up_unrejected_and_matching_address_family() {
    let text = HEADER.to_owned()
        + "eth0 00000000 00000000 0001 0 0 0 00000000 0 0 0\n"
        + "wlan0 00000000 00000000 0201 0 0 0 00000000 0 0 0\n"
        + "down0 00000000 00000000 0000 0 0 0 00000000 0 0 0\n"
        + "lan0 0001A8C0 00000000 0001 0 0 0 00FFFFFF 0 0 0\n";
    let routes = Routes {
        ipv4: parse_routes4(&text),
        ipv6: Some(HashSet::new()),
    };
    assert_eq!(
        routes.ipv4.as_ref().unwrap(),
        &HashSet::from(["eth0".into()])
    );
    assert_eq!(
        addressed("eth0", &known("eth0", false, true), &routes),
        LinkState::Addressed
    );
    assert_eq!(
        addressed("eth0", &known("eth0", true, false), &routes),
        LinkState::Routed
    );
    assert!(parse_routes4("bad header").is_none());
    assert!(parse_routes4(&(HEADER.to_owned() + "truncated\n")).is_none());
}

#[test]
fn ipv6_default_route_excludes_kernel_reject_and_source_specific_entries() {
    let zero = "00000000000000000000000000000000";
    let text = format!("{zero} 00 {zero} 00 {zero} 00000400 00000000 00000000 00000001 eth0\n{zero} 00 {zero} 00 {zero} ffffffff 00000000 00000000 00200200 lo\n{zero} 00 {zero} 40 {zero} 00000400 00000000 00000000 00000001 wlan0\n");
    assert_eq!(parse_routes6(&text), Some(HashSet::from(["eth0".into()])));
    assert_eq!(parse_routes6(""), Some(HashSet::new()));
    assert!(parse_routes6("invalid").is_none());
    let routes = Routes {
        ipv4: None,
        ipv6: parse_routes6(&text),
    };
    assert_eq!(
        addressed("eth0", &known("eth0", false, true), &routes),
        LinkState::Routed
    );
    assert_eq!(
        addressed("eth0", &known("eth0", true, false), &routes),
        LinkState::Unknown
    );
}

#[test]
fn mailbox_coalesces_changes_and_rearms_after_consumption() {
    let mailbox = Mailbox {
        update: Mutex::new(None),
        pending: AtomicBool::new(false),
        stopped: AtomicBool::new(false),
    };
    let wakes = AtomicUsize::new(0);
    let wake = || {
        wakes.fetch_add(1, Ordering::Relaxed);
    };
    let first = Observation {
        status: StatusSnapshot::default(),
        battery: BatteryObservation {
            state: BatteryState::Unknown,
            observed_at: Instant::now(),
            sequence: 1,
            source_epoch: None,
            external_power: None,
        },
    };
    let second = Observation {
        battery: BatteryObservation {
            sequence: 2,
            ..first.battery
        },
        ..first
    };
    assert_eq!(first.status, second.status);
    publish(&mailbox, first, &wake);
    publish(&mailbox, second, &wake);
    assert_eq!(wakes.load(Ordering::Relaxed), 1);
    assert_eq!(take_update(&mailbox), Some(second));
    assert_eq!(take_update(&mailbox), None);
    publish(&mailbox, first, &wake);
    assert_eq!(wakes.load(Ordering::Relaxed), 2);
    assert_eq!(take_update(&mailbox), Some(first));
}
