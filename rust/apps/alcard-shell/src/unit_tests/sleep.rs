use super::*;
use smithay_client_toolkit::reexports::calloop::channel;
use std::{collections::VecDeque, os::fd::FromRawFd, sync::atomic::AtomicUsize};
use zbus::Variant;

const XML: &str = r#"<node><interface name="org.freedesktop.login1.Manager">
      <method name="GetSessionByPID"><arg type="u" direction="in"/><arg type="o" direction="out"/></method>
      <method name="CanSuspend"><arg type="s" direction="out"/></method>
      <method name="SuspendWithFlags"><arg type="t" direction="in"/></method>
      <method name="ListInhibitors"><arg type="a(ssssuu)" direction="out"/></method>
      <method name="Inhibit"><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="h" direction="out"/></method>
      <signal name="PrepareForSleep"><arg type="b"/></signal>
      <property name="PreparingForSleep" type="b" access="read"/>
      <property name="InhibitDelayMaxUSec" type="t" access="read"/>
    </interface></node>"#;

fn session() -> Session {
    Session {
        id: "c42".into(),
        path: "/org/freedesktop/login1/session/c42".into(),
        uid: 1000,
        kind: "wayland".into(),
        seat_id: "seat0".into(),
        seat_path: "/org/freedesktop/login1/seat/seat0".into(),
    }
}

fn context() -> Context {
    Context {
        epoch: 7,
        owner: ":1.42".into(),
        session: session(),
    }
}

fn pipe() -> (OwnedFd, OwnedFd) {
    let mut fds = [-1; 2];
    // SAFETY: pipe2 initializes exactly these two outputs. Each successful
    // descriptor is transferred once into an OwnedFd; no host service is used.
    assert_eq!(
        unsafe {
            nix::libc::pipe2(
                fds.as_mut_ptr(),
                nix::libc::O_CLOEXEC | nix::libc::O_NONBLOCK,
            )
        },
        0
    );
    unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) }
}

fn has_writer(reader: &OwnedFd) -> bool {
    let mut byte = [0_u8; 1];
    // SAFETY: the buffer is writable and the FD is owned. O_NONBLOCK makes
    // this a bounded observation of only the test's private pipe.
    let result = unsafe { nix::libc::read(reader.as_raw_fd(), byte.as_mut_ptr().cast(), 1) };
    if result < 0 {
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(nix::libc::EAGAIN)
        );
        true
    } else {
        assert_eq!(result, 0);
        false
    }
}

fn shared_with_delay() -> (Arc<Shared>, OwnedFd) {
    let (reader, writer) = pipe();
    let shared = Arc::new(Shared::default());
    {
        let mut state = shared.lock();
        state.context = Some(context());
        state.snapshot = Snapshot {
            epoch: 7,
            capability: Capability::Allowed,
            monitor: MonitorState::Ready,
            delay_ms: 5000,
        };
        state.delay = Some(writer);
    }
    (shared, reader)
}

fn queue_intent(shared: &Shared, request: u64) -> (Permit, Permit) {
    let cancel = Permit::new();
    let lock = Permit::new();
    shared.lock().pending = Some(Pending {
        request,
        epoch: 7,
        cancel: cancel.clone(),
        lock: Some(lock.clone()),
        phase: PendingPhase::Queued,
        prepared: Some(session()),
    });
    (cancel, lock)
}

fn messages(receiver: &channel::Channel<Message>) -> Vec<Message> {
    let mut messages = Vec::new();
    while let Ok(message) = receiver.try_recv() {
        messages.push(message);
    }
    messages
}

struct Fake {
    owners: Mutex<VecDeque<String>>,
    capability: Capability,
    session: Result<Session, Failure>,
    inhibited: Result<bool, Failure>,
    preparing: bool,
    calls: Mutex<Vec<&'static str>>,
    owner_calls: AtomicUsize,
    revoke_at_owner: Option<(usize, Permit)>,
}
impl Default for Fake {
    fn default() -> Self {
        Self {
            owners: Mutex::new(VecDeque::new()),
            capability: Capability::Allowed,
            session: Ok(session()),
            inhibited: Ok(false),
            preparing: false,
            calls: Mutex::new(Vec::new()),
            owner_calls: AtomicUsize::new(0),
            revoke_at_owner: None,
        }
    }
}
impl Fake {
    fn record(&self, call: &'static str, budget: &Budget<'_>) -> Result<(), Failure> {
        budget.check(false)?;
        self.calls.lock().unwrap().push(call);
        Ok(())
    }
}
impl Login1 for Fake {
    async fn owner(&self, budget: &Budget<'_>) -> Result<String, Failure> {
        self.record("owner", budget)?;
        let count = self.owner_calls.fetch_add(1, Ordering::AcqRel) + 1;
        if let Some((at, permit)) = &self.revoke_at_owner {
            if *at == count {
                permit.revoke();
            }
        }
        Ok(self
            .owners
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| ":1.42".into()))
    }
    async fn session(&self, _: &str, budget: &Budget<'_>) -> Result<Session, Failure> {
        self.record("session", budget)?;
        self.session.clone()
    }
    async fn capability(&self, _: &str, budget: &Budget<'_>) -> Result<Capability, Failure> {
        self.record("capability", budget)?;
        Ok(self.capability)
    }
    async fn inhibited(&self, _: &str, budget: &Budget<'_>) -> Result<bool, Failure> {
        self.record("inhibited", budget)?;
        self.inhibited
    }
    async fn preparing(
        &self,
        _: &str,
        budget: &Budget<'_>,
    ) -> Result<(bool, u64, Sequence), Failure> {
        self.record("preparing", budget)?;
        Ok((self.preparing, 5000, Sequence::default()))
    }
}

#[test]
fn only_exact_suspend_delay_signal_and_property_api_is_supported() {
    assert_eq!(supported_api(XML), Ok(()));
    for invalid in [
        XML.replace("SuspendWithFlags", "Suspend"),
        XML.replace("type=\"t\" direction=\"in\"", "type=\"b\" direction=\"in\""),
        XML.replace("type=\"h\"", "type=\"u\""),
        XML.replace("type=\"a(ssssuu)\"", "type=\"a(ssssu)\""),
        XML.replace("PrepareForSleep", "PrepareForShutdown"),
        XML.replace(
            "PreparingForSleep\" type=\"b\"",
            "PreparingForSleep\" type=\"s\"",
        ),
        XML.replace("access=\"read\"", "access=\"write\""),
        XML.replace(
            "</interface>",
            "<method name=\"CanSuspend\"><arg type=\"s\" direction=\"out\"/></method></interface>",
        ),
    ] {
        assert_eq!(supported_api(&invalid), Err(Failure::Unavailable));
    }
    assert_eq!(
        supported_api(&" ".repeat(MAX_REPLY_BYTES + 1)),
        Err(Failure::Unavailable)
    );
}

fn session_properties() -> HashMap<String, OwnedVariant> {
    let user = OwnedObjectPath::try_from("/org/freedesktop/login1/user/_1000").unwrap();
    let seat = OwnedObjectPath::try_from(session().seat_path).unwrap();
    HashMap::from([
        (
            "Id".into(),
            OwnedVariant::try_from(Variant::from("c42")).unwrap(),
        ),
        (
            "User".into(),
            OwnedVariant::try_from(Variant::from((1000_u32, user))).unwrap(),
        ),
        (
            "Type".into(),
            OwnedVariant::try_from(Variant::from("wayland")).unwrap(),
        ),
        (
            "Class".into(),
            OwnedVariant::try_from(Variant::from("user")).unwrap(),
        ),
        (
            "Seat".into(),
            OwnedVariant::try_from(Variant::from(("seat0", seat))).unwrap(),
        ),
        ("Remote".into(), OwnedVariant::from(false)),
        ("Active".into(), OwnedVariant::from(true)),
    ])
}
fn seat_properties(active: &str, graphical: bool) -> HashMap<String, OwnedVariant> {
    let path =
        OwnedObjectPath::try_from(format!("/org/freedesktop/login1/session/{active}")).unwrap();
    HashMap::from([
        (
            "Id".into(),
            OwnedVariant::try_from(Variant::from("seat0")).unwrap(),
        ),
        ("CanGraphical".into(), OwnedVariant::from(graphical)),
        (
            "ActiveSession".into(),
            OwnedVariant::try_from(Variant::from((active, path))).unwrap(),
        ),
    ])
}

#[test]
fn suspend_requires_own_active_local_graphical_session_and_matching_seat() {
    let path = OwnedObjectPath::try_from(session().path).unwrap();
    assert_eq!(
        session_from_properties(&path, session_properties(), 1000),
        Ok(session())
    );
    assert_eq!(
        session_from_properties(&path, session_properties(), 1001),
        Err(Failure::SessionUnavailable)
    );
    for (key, value) in [
        ("Active", OwnedVariant::from(false)),
        ("Remote", OwnedVariant::from(true)),
        (
            "Type",
            OwnedVariant::try_from(Variant::from("tty")).unwrap(),
        ),
        (
            "Class",
            OwnedVariant::try_from(Variant::from("greeter")).unwrap(),
        ),
        (
            "Seat",
            OwnedVariant::try_from(Variant::from(("", OwnedObjectPath::try_from("/").unwrap())))
                .unwrap(),
        ),
    ] {
        let mut props = session_properties();
        props.insert(key.into(), value);
        assert_eq!(
            session_from_properties(&path, props, 1000),
            Err(Failure::SessionUnavailable)
        );
    }
    assert_eq!(
        validate_seat(&session(), seat_properties("c42", true)),
        Ok(())
    );
    assert_eq!(
        validate_seat(&session(), seat_properties("c43", true)),
        Err(Failure::SessionUnavailable)
    );
    assert_eq!(
        validate_seat(&session(), seat_properties("c42", false)),
        Err(Failure::SessionUnavailable)
    );
}

#[test]
fn only_sleep_blockers_prevent_requests_and_challenge_never_prompts() {
    let item = |what: &str, mode: &str| (what.into(), "".into(), "".into(), mode.into(), 1000, 42);
    assert_eq!(
        sleep_inhibited(&[item("shutdown", "block"), item("sleep", "delay")]),
        Ok(false)
    );
    for what in ["sleep", "idle:sleep", "shutdown:sleep"] {
        for mode in ["block", "block-weak"] {
            assert_eq!(sleep_inhibited(&[item(what, mode)]), Ok(true));
        }
    }
    assert_eq!(
        sleep_inhibited(&[item("sleep", "invented")]),
        Err(Failure::Unavailable)
    );
    assert_eq!(
        sleep_inhibited(&vec![item("sleep", "delay"); MAX_INHIBITORS + 1]),
        Err(Failure::Unavailable)
    );
    let shared = Shared::default();
    for capability in [Capability::Authentication, Capability::Denied] {
        let fake = Fake {
            capability,
            ..Fake::default()
        };
        assert_eq!(
            future::block_on(preflight(
                &fake,
                ":1.42",
                &session(),
                &Budget::new(&shared, None, READ_TIMEOUT)
            )),
            Err(Failure::Denied)
        );
        assert!(!fake.calls.lock().unwrap().contains(&"preparing"));
    }
    assert_eq!(permission("challenge"), Capability::Authentication);
    assert_eq!(permission("inhibitor-blocked"), Capability::Blocked);
}

#[test]
fn preflight_rechecks_owner_session_and_actual_sleep_state() {
    let shared = Shared::default();
    let mut replacement = session();
    replacement.id = "c43".into();
    for (fake, expected) in [
        (
            Fake {
                owners: Mutex::new(VecDeque::from([":1.42".into(), ":1.99".into()])),
                ..Fake::default()
            },
            Failure::Unavailable,
        ),
        (
            Fake {
                session: Ok(replacement),
                ..Fake::default()
            },
            Failure::SessionUnavailable,
        ),
        (
            Fake {
                inhibited: Ok(true),
                ..Fake::default()
            },
            Failure::Blocked,
        ),
        (
            Fake {
                preparing: true,
                ..Fake::default()
            },
            Failure::Blocked,
        ),
    ] {
        assert_eq!(
            future::block_on(preflight(
                &fake,
                ":1.42",
                &session(),
                &Budget::new(&shared, None, READ_TIMEOUT)
            )),
            Err(expected)
        );
    }
}

#[test]
fn revocation_during_last_preflight_await_and_duplicate_dispatch_cannot_mutate() {
    let (shared, _reader) = shared_with_delay();
    let (cancel, permit) = queue_intent(&shared, 9);
    let fake = Fake {
        revoke_at_owner: Some((2, permit.clone())),
        ..Fake::default()
    };
    assert_eq!(
        future::block_on(preflight(
            &fake,
            ":1.42",
            &session(),
            &Budget::new(&shared, Some(&cancel), READ_TIMEOUT)
        )),
        Ok(())
    );
    let gate = Gate {
        shared: &shared,
        request: 9,
        epoch: 7,
        permit: &permit,
    };
    assert_eq!(gate.dispatch(), Err(Failure::Cancelled));
    assert_eq!(
        shared.lock().pending.as_ref().unwrap().phase,
        PendingPhase::Queued
    );

    let (_, permit) = queue_intent(&shared, 10);
    let gate = Gate {
        shared: &shared,
        request: 10,
        epoch: 7,
        permit: &permit,
    };
    assert_eq!(gate.dispatch(), Ok(()));
    assert_eq!(gate.dispatch(), Err(Failure::Cancelled));
}

#[test]
fn queued_cancel_revokes_permit_synchronously_and_submission_is_bounded() {
    let (shared, _reader) = shared_with_delay();
    let (commands, incoming) = mpsc::sync_channel(2);
    let client = Client {
        commands,
        shared: shared.clone(),
    };
    assert!(client.prepare(9));
    assert!(!client.prepare(10));
    client.cancel(9);
    let Work::Prepare { cancel, .. } = incoming.try_recv().unwrap() else {
        panic!();
    };
    assert!(!cancel.valid());
    clear_pending(&shared, 9, 7);
    let (_, permit) = queue_intent(&shared, 11);
    shared.lock().pending.as_mut().unwrap().phase = PendingPhase::Prepared;
    assert!(client.commit(11, 7, permit.clone()));
    client.cancel(11);
    let Work::Commit {
        permit: queued,
        cancel,
        ..
    } = incoming.try_recv().unwrap()
    else {
        panic!();
    };
    assert!(!queued.valid());
    assert!(!permit.valid());
    assert!(!cancel.valid());
    clear_pending(&shared, 11, 7);
    client
        .commands
        .try_send(Work::Stop)
        .unwrap_or_else(|_| panic!());
    client
        .commands
        .try_send(Work::Stop)
        .unwrap_or_else(|_| panic!());
    assert!(!client.prepare(12));
    assert!(shared.lock().pending.is_none());
}

#[test]
fn recovery_refresh_settles_its_token_and_cannot_block_the_next_episode() {
    let (shared, _reader) = shared_with_delay();
    let (updates, received) = channel::channel();
    let (commands, incoming) = mpsc::sync_channel(2);
    let client = Client {
        commands,
        shared: shared.clone(),
    };
    lose_epoch(&shared, &updates);
    let _ = messages(&received);

    // This request arrives AFTER Lost, while the observer is in its
    // recovery wait. A menu refresh may coalesce with it, but cannot leave
    // its slot occupied after the wait consumes the shared wake flag.
    assert!(client.refresh_for(41));
    assert!(client.refresh());
    assert!(!client.refresh_for(42));
    assert!(finish_recovery_refresh(&shared, &updates));
    assert_eq!(
        messages(&received),
        vec![Message::Refreshed {
            request: 41,
            snapshot: shared.lock().snapshot,
        }]
    );
    assert_eq!(shared.refresh_request.load(Ordering::Acquire), 0);
    assert!(!finish_recovery_refresh(&shared, &updates));
    assert!(messages(&received).is_empty());

    // Even if reconnect still fails, a new explicit discharge episode
    // obtains its own correlated refusal instead of a permanently busy slot.
    assert!(client.refresh_for(42));
    assert!(finish_recovery_refresh(&shared, &updates));
    assert!(matches!(
        messages(&received).as_slice(),
        [Message::Refreshed {
            request: 42,
            snapshot: Snapshot {
                monitor: MonitorState::Unavailable,
                capability: Capability::Unavailable,
                ..
            }
        }]
    ));
    assert!(matches!(
        incoming.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
}

#[test]
fn delay_is_cloexec_and_dropping_it_leaves_no_message_owned_duplicate() {
    let (reader, writer) = pipe();
    let original: zbus::OwnedFd = writer.into();
    let call = zbus::Message::method_call(MANAGER_PATH, "Inhibit")
        .build(&())
        .unwrap();
    let reply = zbus::Message::method_return(&call.header())
        .build(&original)
        .unwrap();
    drop(original);
    let owned = decode_delay(reply).unwrap();
    let flags = unsafe { nix::libc::fcntl(owned.as_raw_fd(), nix::libc::F_GETFD) };
    assert_ne!(flags & nix::libc::FD_CLOEXEC, 0);
    assert!(has_writer(&reader));
    drop(owned);
    assert!(!has_writer(&reader));
}

#[test]
fn delay_release_requires_current_epoch_cycle_and_live_lock_permit() {
    let (shared, reader) = shared_with_delay();
    let (updates, _receiver) = channel::channel();
    begin_cycle(&shared, &updates, 7).unwrap();
    let permit = Permit::new();
    assert!(!release_delay(&shared, 6, 1, &permit));
    assert!(!release_delay(&shared, 7, 2, &permit));
    permit.revoke();
    assert!(!release_delay(&shared, 7, 1, &permit));
    assert!(has_writer(&reader));
    assert!(release_delay(&shared, 7, 1, &Permit::new()));
    assert!(!has_writer(&reader));
    assert!(release_delay(&shared, 7, 1, &Permit::new()));
}

#[test]
fn initial_preparing_is_never_ready_and_false_requires_fresh_delay_before_ready() {
    let (shared, reader) = shared_with_delay();
    shared.lock().snapshot.monitor = MonitorState::Connecting;
    let (updates, receiver) = channel::channel();
    begin_cycle(&shared, &updates, 7).unwrap();
    begin_cycle(&shared, &updates, 7).unwrap();
    assert_eq!(ready(&shared, &updates, 7), Err(Failure::Unavailable));
    assert_eq!(
        messages(&receiver)
            .iter()
            .filter(|message| matches!(message, Message::Preparing { .. }))
            .count(),
        1
    );
    assert!(end_cycle(&shared, &updates, 7).unwrap());
    assert!(!has_writer(&reader));
    assert_eq!(ready(&shared, &updates, 7), Err(Failure::Unavailable));
    assert!(!end_cycle(&shared, &updates, 7).unwrap());
    let (next_reader, next_writer) = pipe();
    install_delay(&shared, 7, next_writer).unwrap();
    ready(&shared, &updates, 7).unwrap();
    assert_eq!(shared.lock().snapshot.monitor, MonitorState::Ready);
    assert_eq!(shared.lock().snapshot.epoch, 7);
    assert!(has_writer(&next_reader));
    let messages = messages(&receiver);
    assert!(matches!(
        messages[0],
        Message::CycleEnded { cycle: Some(1), .. }
    ));
    assert!(matches!(
        messages.last(),
        Some(Message::Snapshot(Snapshot {
            monitor: MonitorState::Ready,
            ..
        }))
    ));
}

#[test]
fn ordinary_refresh_preserves_queued_true_false_and_unknown_pending_cycle() {
    let (shared, reader) = shared_with_delay();
    let (_, permit) = queue_intent(&shared, 9);
    Gate {
        shared: &shared,
        request: 9,
        epoch: 7,
        permit: &permit,
    }
    .dispatch()
    .unwrap();
    let (updates, receiver) = channel::channel();
    let fake = Fake::default();
    // A fresh property false would be newer than queued true/false here.
    // Regular refresh must not query that property or supersede the queue.
    future::block_on(refresh_capability(&fake, &context(), &shared, &updates)).unwrap();
    assert!(!fake.calls.lock().unwrap().contains(&"preparing"));
    assert_eq!(shared.lock().snapshot.epoch, 7);
    assert!(shared.lock().pending.is_some());
    begin_cycle(&shared, &updates, 7).unwrap();
    assert!(end_cycle(&shared, &updates, 7).unwrap());
    assert!(shared.lock().pending.is_none());
    assert!(!has_writer(&reader));
    let messages = messages(&receiver);
    assert_eq!(
        messages
            .iter()
            .filter(|message| matches!(message, Message::Preparing { .. }))
            .count(),
        1
    );
    assert_eq!(
        messages
            .iter()
            .filter(|message| matches!(message, Message::CycleEnded { cycle: Some(1), .. }))
            .count(),
        1
    );
}

#[test]
fn observer_releases_delay_while_mutation_rpc_is_still_pending() {
    let runtime = zruntime::SharedRuntime::current().unwrap();
    let (shared, reader) = shared_with_delay();
    let (cancel, permit) = queue_intent(&shared, 9);
    Gate {
        shared: &shared,
        request: 9,
        epoch: 7,
        permit: &permit,
    }
    .dispatch()
    .unwrap();
    let (updates, _receiver) = channel::channel();
    let observer_shared = shared.clone();
    let rpc_started = Arc::new(Notify::default());
    let observer_started = rpc_started.clone();
    let observer = runtime.spawn("fake-sleep-observer", async move {
        observer_started.wait().await;
        begin_cycle(&observer_shared, &updates, 7).unwrap();
        assert!(release_delay(&observer_shared, 7, 1, &permit));
        assert!(!has_writer(&reader));
        end_cycle(&observer_shared, &updates, 7).unwrap();
    });
    let budget = Budget::new(&shared, Some(&cancel), Duration::from_secs(1));
    let result = runtime.block_on(budget.dbus(
        async {
            rpc_started.wake();
            future::pending::<zbus::Result<()>>().await
        },
        true,
    ));
    assert_eq!(result, Err(Failure::Unknown));
    runtime.block_on(observer).unwrap();
    assert!(shared.lock().pending.is_none());
}

#[test]
fn owner_loss_revokes_queued_and_dispatched_requests_and_closes_delay() {
    for dispatched in [false, true] {
        let (shared, reader) = shared_with_delay();
        let (cancel, permit) = queue_intent(&shared, 9);
        if dispatched {
            Gate {
                shared: &shared,
                request: 9,
                epoch: 7,
                permit: &permit,
            }
            .dispatch()
            .unwrap();
        }
        let (updates, receiver) = channel::channel();
        lose_epoch(&shared, &updates);
        assert!(!cancel.valid());
        assert!(!permit.valid());
        assert!(!has_writer(&reader));
        assert!(shared.lock().context.is_none());
        assert_eq!(shared.lock().snapshot.monitor, MonitorState::Unavailable);
        assert!(matches!(messages(&receiver)[0], Message::Lost { epoch: 7 }));
    }
}

#[test]
fn uncertain_rpc_timeout_keeps_the_dispatched_intent_and_read_deadline_is_distinct() {
    let runtime = zruntime::SharedRuntime::current().unwrap();
    let (shared, _reader) = shared_with_delay();
    let (cancel, permit) = queue_intent(&shared, 9);
    Gate {
        shared: &shared,
        request: 9,
        epoch: 7,
        permit: &permit,
    }
    .dispatch()
    .unwrap();
    let budget = Budget::new(&shared, Some(&cancel), Duration::from_millis(2));
    assert_eq!(
        runtime.block_on(budget.dbus(future::pending::<zbus::Result<()>>(), true)),
        Err(Failure::Unknown)
    );
    assert_eq!(
        shared.lock().pending.as_ref().unwrap().phase,
        PendingPhase::Dispatched
    );
    let budget = Budget::new(&shared, None, Duration::ZERO);
    assert_eq!(
        runtime.block_on(budget.dbus(future::pending::<zbus::Result<()>>(), false)),
        Err(Failure::Timeout)
    );
}

#[test]
fn permit_clones_are_revocable_and_only_nonterminal_progress_is_busy() {
    let permit = Permit::new();
    let queued = permit.clone();
    permit.revoke();
    assert!(!queued.valid());
    for stage in [
        Stage::Preparing,
        Stage::Locking,
        Stage::Queued,
        Stage::Requested,
        Stage::Unknown,
    ] {
        assert!(Progress {
            request: 1,
            stage,
            failure: None
        }
        .busy());
    }
    for stage in [Stage::Ended, Stage::Failed, Stage::Cancelled] {
        assert!(!Progress {
            request: 1,
            stage,
            failure: None
        }
        .busy());
    }
}

#[test]
fn receipt_deadline_is_enforced_at_dispatch_without_a_ui_revocation() {
    let (shared, _reader) = shared_with_delay();
    let (_, manual) = queue_intent(&shared, 29);
    let deadline = Instant::now();
    let expired = manual
        .clone()
        .until(deadline)
        .until(deadline + Duration::from_secs(60));
    // A later deadline cannot extend an already bounded receipt. No UI
    // callback or revoke occurred, yet the actual dispatch gate refuses it.
    assert!(!expired.valid());
    assert!(manual.valid());
    assert_eq!(
        Gate {
            shared: &shared,
            request: 29,
            epoch: 7,
            permit: &expired,
        }
        .dispatch(),
        Err(Failure::Cancelled)
    );
    assert_eq!(
        shared.lock().pending.as_ref().unwrap().phase,
        PendingPhase::Queued
    );

    let bounded = manual
        .clone()
        .until(Instant::now() + Duration::from_secs(60));
    assert!(bounded.valid());
    manual.revoke();
    assert!(!bounded.valid());
}
