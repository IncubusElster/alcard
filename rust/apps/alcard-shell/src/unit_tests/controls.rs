use super::*;

#[test]
fn only_one_refresh_or_mutation_is_pending_and_full_queue_does_not_block() {
    let (commands, incoming) = mpsc::sync_channel(1);
    let client = Client {
        commands,
        latches: Arc::new(Latches::default()),
    };
    assert!(client.refresh(1));
    assert!(!client.refresh(2));
    assert!(!client.execute(3, Operation::ToggleMute));
    assert!(matches!(incoming.try_recv(), Ok(Work::Refresh(1))));
    client.latches.busy.store(false, Ordering::Release);
    assert!(client.execute(4, Operation::ToggleMute));
    drop(incoming);
    client.latches.busy.store(false, Ordering::Release);
    assert!(!client.refresh(5));
    assert!(!client.latches.busy.load(Ordering::Acquire));
}

#[test]
fn command_timeout_and_output_bounds_do_not_claim_a_write_succeeded() {
    let stopped = AtomicBool::new(false);
    for mutating in [false, true] {
        let result = run_command(
            Command::new("/bin/sh").args(["-c", "while :; do :; done"]),
            &Budget::new(&stopped, Duration::from_millis(40)),
            mutating,
        );
        assert_eq!(
            result,
            Err(if mutating {
                Failure::Unknown
            } else {
                Failure::Timeout
            })
        );
        let result = run_command(
            Command::new("/bin/sh").args(["-c", "while :; do printf '0123456789abcdef'; done"]),
            &Budget::new(&stopped, Duration::from_secs(2)),
            mutating,
        );
        assert_eq!(
            result,
            Err(if mutating {
                Failure::Unknown
            } else {
                Failure::Invalid
            })
        );
    }
}

#[test]
fn larger_pipewire_snapshot_does_not_expand_diagnostics_or_allow_unbounded_output() {
    let stopped = AtomicBool::new(false);
    let output = "i=0; while [ \"$i\" -lt 5000 ]; do printf '0123456789abcdef'; i=$((i+1)); done";
    let result = run_command_with_output_limit(
        Command::new("/bin/sh").args(["-c", output]),
        &Budget::new(&stopped, Duration::from_secs(2)),
        false,
        1024 * 1024,
    )
    .unwrap();
    assert_eq!(result.len(), 80_000);
    assert_eq!(
        run_command_with_output_limit(
            Command::new("/bin/sh").args(["-c", &format!("{{ {output}; }} >&2")]),
            &Budget::new(&stopped, Duration::from_secs(2)),
            false,
            1024 * 1024,
        ),
        Err(Failure::Invalid)
    );
    for limit in [0, 1024 * 1024 + 1] {
        assert_eq!(
            run_command_with_output_limit(
                &mut Command::new("/not-present"),
                &Budget::new(&stopped, READ_TIMEOUT),
                false,
                limit,
            ),
            Err(Failure::Invalid)
        );
    }
}

#[test]
fn cancelled_command_is_not_spawned_and_sysfs_names_cannot_escape() {
    let stopped = AtomicBool::new(true);
    assert_eq!(
        run_command(
            &mut Command::new("/not-present"),
            &Budget::new(&stopped, READ_TIMEOUT),
            true
        ),
        Err(Failure::Timeout)
    );
    for name in [
        "",
        ".",
        "..",
        "../backlight",
        "backlight/../other",
        "test\nother",
    ] {
        assert!(!safe_name(name));
    }
    assert!(safe_name("backlight-pwm.0"));
}
