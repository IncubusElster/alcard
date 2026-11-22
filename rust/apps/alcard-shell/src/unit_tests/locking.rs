use super::*;

#[test]
fn missing_final_receipt_requires_fresh_pam_instead_of_latching_or_claiming_unlock() {
    let pending = Some(Attempt {
        id: 7,
        locale: "en",
        recovering: true,
    });
    // The client can acknowledge native unlock and clear intent, then die
    // before its final receipt. Clear is not authenticated completion for
    // Shell: a conservative fresh lock/PAM replaces an indefinite latch.
    assert!(recovery_needed(pending, Some(Observation::Clear)));
    assert!(recovery_needed(pending, Some(Observation::Abandoned)));
    // Another live owner is never replaced, and invalid state is not a
    // permission to start a concurrent lock request.
    assert!(!recovery_needed(pending, Some(Observation::Held)));
    assert!(!recovery_needed(pending, None));
    // A complete canonical receipt removes the attempt; ordinary unlock
    // must not trigger an automatic second authentication conversation.
    assert!(!recovery_needed(None, Some(Observation::Clear)));
    assert!(recovery_needed(None, Some(Observation::Abandoned)));
}

#[test]
fn only_ordered_complete_lifecycle_can_report_unlock() {
    for input in [
        b"UNLOCKED\n".as_slice(),
        b"LOCKED\n",
        b"LOCKED\nUNLOCKED",
        b"LOCKED\nLOCKED\nUNLOCKED\n",
        b"LOCKED\nUNLOCKED\nextra\n",
        b"FAILED\nLOCKED\nUNLOCKED\n",
    ] {
        let mut protocol = Protocol::default();
        protocol.feed(input);
        assert!(!protocol.complete());
    }
    let mut protocol = Protocol::default();
    for input in [b"LOC".as_slice(), b"KED\nUN", b"LOCKED\n"] {
        protocol.feed(input);
    }
    assert!(protocol.complete());
    let mut rejected = Protocol::default();
    rejected.feed(b"FAILED\n");
    assert!(rejected.failed());
    assert!(!rejected.complete());
    assert!(!Protocol::default().failed());
}

#[test]
fn public_status_protocol_is_bounded_and_rejects_extra_data() {
    let mut protocol = Protocol::default();
    protocol.feed(&[b'x'; 4096]);
    assert!(protocol.invalid);
    assert!(protocol.pending.len() <= 16);
    protocol.feed(b"LOCKED\nUNLOCKED\n");
    assert!(!protocol.complete());
}
