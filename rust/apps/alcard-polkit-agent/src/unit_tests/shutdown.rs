use super::*;
#[test]
fn helper_inherits_unblocked_termination_signals() {
    let event_loop =
        smithay_client_toolkit::reexports::calloop::EventLoop::<()>::try_new().unwrap();
    let watcher = Watcher::new(event_loop.get_signal()).unwrap();
    let mut mask = SigSet::empty();
    pthread_sigmask(SigmaskHow::SIG_SETMASK, None, Some(&mut mask)).unwrap();
    assert!(!mask.contains(Signal::SIGTERM));
    assert!(!mask.contains(Signal::SIGINT));
    assert!(!watcher.requested());
}
