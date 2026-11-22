use super::*;

#[test]
fn missing_bus_reply_can_be_cancelled_without_waiting_for_io() {
    let established = AtomicBool::new(false);
    let stopped = AtomicBool::new(true);
    assert!(drive(future::pending(), &established, &stopped, INITIAL_TIMEOUT).is_none());
}

#[test]
fn initial_connection_and_name_query_have_a_deadline() {
    let established = AtomicBool::new(false);
    let stopped = AtomicBool::new(false);
    assert!(matches!(
        drive(future::pending(), &established, &stopped, Duration::ZERO),
        Some(Err(_))
    ));
    established.store(true, Ordering::Release);
    assert!(matches!(
        drive(
            future::ready(Ok(())),
            &established,
            &stopped,
            Duration::ZERO
        ),
        Some(Ok(()))
    ));
}
