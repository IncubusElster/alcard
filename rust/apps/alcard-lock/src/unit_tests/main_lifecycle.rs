use super::*;

#[test]
fn failed_receipt_requires_a_returned_error_before_any_lock_request() {
    for notify in [false, true] {
        for requested in [false, true] {
            for succeeds in [false, true] {
                let requested = AtomicBool::new(requested);
                let mut output = Vec::new();
                let result = run_reported(notify, &requested, &mut output, || {
                    if succeeds {
                        Ok(())
                    } else {
                        Err(7)
                    }
                });
                assert_eq!(result.is_ok(), succeeds);
                assert_eq!(
                    output.as_slice(),
                    if notify && !requested.load(Ordering::Relaxed) && !succeeds {
                        &b"FAILED\n"[..]
                    } else {
                        &[]
                    }
                );
            }
        }
    }
}

#[test]
fn a_late_error_or_panic_cannot_report_a_safe_prelock_failure() {
    let requested = AtomicBool::new(false);
    let mut output = Vec::new();
    let result = run_reported(true, &requested, &mut output, || {
        requested.store(true, Ordering::Release);
        Err::<(), _>("error after requesting the native lock")
    });
    assert!(result.is_err());
    assert!(output.is_empty());

    let requested = AtomicBool::new(false);
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_reported(true, &requested, &mut output, || -> Result<(), ()> {
            panic!("simulated abrupt exit before an acknowledgement")
        })
    }));
    assert!(panic.is_err());
    assert!(output.is_empty());
}

#[test]
fn notification_mode_never_writes_help_into_its_status_stream() {
    assert!(run(vec!["--notify".into(), "--help".into()], true).is_err());
    assert!(run(vec!["--help".into(), "--notify".into()], true).is_err());
}
