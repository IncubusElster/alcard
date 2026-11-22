use super::*;

const UUID: &str = "11111111-2222-4333-8444-555555555555";
const SSID: &str = "Acceptance network";
const SAVED: &str = "/org/freedesktop/NetworkManager/Settings/7";

fn access_point(flags: u32, private: bool) -> AccessPoint {
    AccessPoint {
        path: OwnedObjectPath::try_from("/org/freedesktop/NetworkManager/AccessPoint/3").unwrap(),
        properties: Properties::from([
            ("WpaFlags".into(), flags.into()),
            ("RsnFlags".into(), 0u32.into()),
            ("Flags".into(), u32::from(private).into()),
            ("Mode".into(), 2u32.into()),
            ("HwAddress".into(), text("00:11:22:33:44:55")),
        ]),
    }
}
fn message(owner: &str) -> zbus::Message {
    zbus::Message::method_call(AGENT_PATH, "GetSecrets")
        .sender(owner)
        .build(&())
        .unwrap()
}
fn agent(password: &str) -> SecretAgent {
    SecretAgent {
        owner: ":1.9".into(),
        profile: Some(OwnedObjectPath::try_from(SAVED).unwrap()),
        uuid: UUID.into(),
        ssid: SSID.into(),
        control: Arc::new(Control::new(password)),
        supplied: AtomicBool::new(false),
    }
}
fn request(agent: &SecretAgent, owner: &str, profile: Settings) -> Result<Settings, SecretError> {
    let msg = message(owner);
    agent.get_secrets(
        profile,
        OwnedObjectPath::try_from(SAVED).unwrap(),
        SECURITY,
        Vec::new(),
        5,
        msg.header(),
    )
}

#[test]
fn secret_agent_rejects_other_senders_profiles_and_security_types() {
    let agent = agent("fixture-password");
    let ap = access_point(0x100, true);
    assert!(matches!(
        request(&agent, ":1.8", ap.new_profile(SSID, UUID).unwrap()),
        Err(SecretError::PermissionDenied(_))
    ));
    assert!(matches!(
        request(&agent, ":1.9", ap.new_profile("other", UUID).unwrap()),
        Err(SecretError::NoSecrets(_))
    ));
    assert!(matches!(
        request(&agent, ":1.9", ap.new_profile(SSID, "other").unwrap()),
        Err(SecretError::NoSecrets(_))
    ));
    let msg = message(":1.9");
    assert!(matches!(
        agent.get_secrets(
            ap.new_profile(SSID, UUID).unwrap(),
            OwnedObjectPath::try_from("/other").unwrap(),
            SECURITY,
            Vec::new(),
            5,
            msg.header()
        ),
        Err(SecretError::PermissionDenied(_))
    ));
    let mut enterprise = ap.new_profile(SSID, UUID).unwrap();
    enterprise
        .get_mut(SECURITY)
        .unwrap()
        .insert("key-mgmt".into(), text("wpa-eap"));
    assert!(matches!(
        request(&agent, ":1.9", enterprise),
        Err(SecretError::NoSecrets(_))
    ));
    let secrets = request(&agent, ":1.9", ap.new_profile(SSID, UUID).unwrap()).unwrap();
    assert_eq!(string(&secrets[SECURITY], "psk"), Some("fixture-password"));
    assert!(matches!(
        request(&agent, ":1.9", ap.new_profile(SSID, UUID).unwrap()),
        Err(SecretError::NoSecrets(_))
    ));
}

#[test]
fn cancellation_revokes_password_before_any_late_secret_request() {
    for timeout in [false, true] {
        let agent = agent("fixture-password");
        agent.control.stop(timeout);
        assert!(agent.control.secret.lock().unwrap().is_none());
        let profile = access_point(0x100, true).new_profile(SSID, UUID).unwrap();
        assert!(matches!(
            request(&agent, ":1.9", profile),
            Err(SecretError::UserCanceled(_))
        ));
        assert_eq!(
            agent.control.check().unwrap_err().kind(),
            if timeout {
                io::ErrorKind::TimedOut
            } else {
                io::ErrorKind::Interrupted
            }
        );
    }
}

#[test]
fn cancel_wakes_a_pending_read_without_waiting_for_service() {
    let control = Arc::new(Control::new("fixture-password"));
    let copy = control.clone();
    let worker = thread::spawn(move || {
        zbus::block_on(copy.read(std::future::pending::<zbus::Result<()>>()))
    });
    control.stop(false);
    assert_eq!(
        worker.join().unwrap().unwrap_err().kind(),
        io::ErrorKind::Interrupted
    );
}

#[test]
fn stalled_bus_handshake_is_dropped_on_cancel_and_timeout() {
    struct Finished(Arc<AtomicBool>);
    impl Drop for Finished {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    for timeout in [false, true] {
        let cancel = Arc::new(AtomicBool::new(false));
        let started = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let worker_started = started.clone();
        let worker_finished = finished.clone();
        let worker = thread::spawn(move || {
            connect_using(
                "fixture-wlan0",
                "fixture",
                "fixture-password",
                &worker_cancel,
                if timeout {
                    Duration::from_millis(100)
                } else {
                    Duration::from_secs(2)
                },
                move || async move {
                    let _finished = Finished(worker_finished);
                    worker_started.store(true, Ordering::Release);
                    std::future::pending::<zbus::Result<Connection>>().await
                },
            )
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        while !started.load(Ordering::Acquire) {
            assert!(Instant::now() < deadline);
            thread::yield_now();
        }
        if !timeout {
            cancel.store(true, Ordering::Release);
        }
        let error = worker.join().unwrap().unwrap_err();
        assert_eq!(
            error.kind(),
            if timeout {
                io::ErrorKind::TimedOut
            } else {
                io::ErrorKind::Interrupted
            }
        );
        assert!(finished.load(Ordering::Acquire));
    }
}

#[test]
fn saved_profile_matching_preserves_security_and_bssid_constraints() {
    let ap = access_point(0x100, true);
    let mut profile = ap.new_profile(SSID, UUID).unwrap();
    profile.insert(
        "ipv4".into(),
        Properties::from([("method".into(), text("manual"))]),
    );
    assert!(ap.matches(&profile, SSID));
    assert_eq!(string(&profile["ipv4"], "method"), Some("manual"));
    assert!(!access_point(0, false).matches(&profile, SSID));
    assert!(!ap.matches(&profile, "other"));
    profile
        .get_mut(WIRELESS)
        .unwrap()
        .insert("bssid".into(), text("AA:BB:CC:DD:EE:FF"));
    assert!(!ap.matches(&profile, SSID));
    profile
        .get_mut(WIRELESS)
        .unwrap()
        .insert("bssid".into(), text("00:11:22:33:44:55"));
    assert!(ap.matches(&profile, SSID));
    profile
        .get_mut(WIRELESS)
        .unwrap()
        .insert("mode".into(), text("adhoc"));
    assert!(!ap.matches(&profile, SSID));
}

#[test]
fn binary_bssid_constraint_rejects_wrong_or_malformed_addresses() {
    let mut ap = access_point(0x100, true);
    let mut profile = ap.new_profile(SSID, UUID).unwrap();
    for address in [Vec::new(), vec![0u8; 5], vec![0u8; 7], vec![0xAAu8; 6]] {
        profile.get_mut(WIRELESS).unwrap().insert(
            "bssid".into(),
            OwnedVariant::try_from(zbus::Variant::from(address)).unwrap(),
        );
        assert!(!ap.matches(&profile, SSID));
    }
    profile.get_mut(WIRELESS).unwrap().insert(
        "bssid".into(),
        OwnedVariant::try_from(zbus::Variant::from(vec![0u8, 0x11, 0x22, 0x33, 0x44, 0x55]))
            .unwrap(),
    );
    assert!(ap.matches(&profile, SSID));
    ap.properties
        .insert("HwAddress".into(), text("AA:BB:CC:DD:EE:FF"));
    assert!(!ap.matches(&profile, SSID));
    for address in [
        "00:11:22:33:44",
        "00:11:22:33:44:55:66",
        "+0:11:22:33:44:55",
        "00:11:22:33:44:gg",
    ] {
        ap.properties.insert("HwAddress".into(), text(address));
        assert!(!ap.matches(&profile, SSID));
    }
    ap.properties
        .insert("HwAddress".into(), text("00:11:22:33:44:55"));
    for constraint in [text("invalid"), 7u32.into()] {
        profile
            .get_mut(WIRELESS)
            .unwrap()
            .insert("bssid".into(), constraint);
        assert!(!ap.matches(&profile, SSID));
    }
}

#[test]
fn new_profiles_support_personal_security_without_carrying_secrets() {
    for (flags, private, key) in [
        (0, false, None),
        (0, true, Some("none")),
        (0x100, true, Some("wpa-psk")),
        (0x400, true, Some("sae")),
        (0x800, true, Some("owe")),
    ] {
        let ap = access_point(flags, private);
        let profile = ap.new_profile(SSID, UUID).unwrap();
        assert_eq!(
            profile.get(SECURITY).and_then(|p| string(p, "key-mgmt")),
            key
        );
        assert!(ap.matches(&profile, SSID));
        assert!(!profile
            .values()
            .any(|p| p.contains_key("psk") || p.contains_key("wep-key0")));
    }
    assert_eq!(
        access_point(0x200, true)
            .new_profile(SSID, UUID)
            .unwrap_err()
            .kind(),
        io::ErrorKind::Unsupported
    );
}

#[test]
fn no_password_is_invented_for_saved_profiles() {
    let agent = agent("");
    let profile = access_point(0x100, true).new_profile(SSID, UUID).unwrap();
    assert!(matches!(
        request(&agent, ":1.9", profile),
        Err(SecretError::NoSecrets(_))
    ));
}

#[test]
fn a_lost_activation_reply_is_never_reported_as_confirmed_failure_or_cancel() {
    let control = Control::new("fixture-password");
    for cancelled in [false, true] {
        if cancelled {
            control.stop(false);
        }
        let error = activation_error(
            zbus::Error::from(io::Error::new(
                io::ErrorKind::TimedOut,
                "fixture service diagnostics",
            )),
            &control,
        );
        assert!(error.to_string().contains("unconfirmed"));
        assert!(!error.to_string().contains("fixture"));
        assert_eq!(
            error.kind(),
            if cancelled {
                io::ErrorKind::Interrupted
            } else {
                io::ErrorKind::TimedOut
            }
        );
    }
}
