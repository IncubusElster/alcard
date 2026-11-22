use super::*;
use std::{collections::VecDeque, task::Poll};
use zbus::Variant;

const XML: &str = r#"<node>
      <interface name="org.freedesktop.login1.Manager">
        <method name="GetSessionByPID"><arg type="u" direction="in"/><arg type="o" direction="out"/></method>
        <method name="TerminateSession"><arg type="s" direction="in"/></method>
        <method name="RebootWithFlags"><arg type="t" direction="in"/></method>
        <method name="PowerOffWithFlags"><arg type="t" direction="in"/></method>
      </interface>
    </node>"#;

fn local_session() -> Session {
    Session {
        id: "c212".into(),
        path: "/org/freedesktop/login1/session/c212".into(),
        uid: 1000,
    }
}

struct Fake {
    owner: String,
    owners: VecDeque<String>,
    methods: Methods,
    session: Result<Session, Failure>,
    capabilities: [Capability; 3],
    inhibited: Result<bool, Failure>,
    mutation_result: Result<(), Failure>,
    destinations: Vec<String>,
    mutations: Vec<(String, Operation, Option<Session>)>,
}
impl Default for Fake {
    fn default() -> Self {
        Self {
            owner: ":1.212".into(),
            owners: VecDeque::new(),
            methods: Methods([true; 3]),
            session: Ok(local_session()),
            capabilities: [Capability::Allowed; 3],
            inhibited: Ok(false),
            mutation_result: Ok(()),
            destinations: Vec::new(),
            mutations: Vec::new(),
        }
    }
}
impl Login1 for Fake {
    fn owner(&mut self, _: &Budget<'_>) -> Result<String, Failure> {
        Ok(self
            .owners
            .pop_front()
            .unwrap_or_else(|| self.owner.clone()))
    }
    fn methods(&mut self, owner: &str, _: &Budget<'_>) -> Result<Methods, Failure> {
        self.destinations.push(owner.into());
        Ok(self.methods)
    }
    fn session(&mut self, owner: &str, _: &Budget<'_>) -> Result<Session, Failure> {
        self.destinations.push(owner.into());
        self.session.clone()
    }
    fn capability(
        &mut self,
        owner: &str,
        operation: Operation,
        _: &Budget<'_>,
    ) -> Result<Capability, Failure> {
        self.destinations.push(owner.into());
        Ok(self.capabilities[operation.index()])
    }
    fn inhibited(&mut self, owner: &str, _: &Budget<'_>) -> Result<bool, Failure> {
        self.destinations.push(owner.into());
        self.inhibited
    }
    fn mutate(
        &mut self,
        owner: &str,
        operation: Operation,
        session: Option<&Session>,
        _: &Budget<'_>,
    ) -> Result<(), Failure> {
        self.mutations
            .push((owner.into(), operation, session.cloned()));
        self.mutation_result
    }
}

fn pin(fake: &mut Fake) -> Pinned {
    probe(fake, &Budget::new(&AtomicBool::new(false), READ_TIMEOUT))
        .unwrap()
        .0
}

#[test]
fn capability_answers_are_not_optimistically_guessed() {
    for (answer, expected) in [
        ("yes", Capability::Allowed),
        ("challenge", Capability::Authentication),
        ("no", Capability::Denied),
        ("na", Capability::Unsupported),
        ("", Capability::Unavailable),
        ("Yes", Capability::Unavailable),
        ("future-value", Capability::Unavailable),
    ] {
        assert_eq!(permission(answer), expected);
    }
    let mut fake = Fake {
        capabilities: [
            Capability::Allowed,
            Capability::Authentication,
            Capability::Denied,
        ],
        ..Fake::default()
    };
    let (_, snapshot) = probe(
        &mut fake,
        &Budget::new(&AtomicBool::new(false), READ_TIMEOUT),
    )
    .unwrap();
    assert_eq!(snapshot.capabilities, fake.capabilities);
}

#[test]
fn only_exact_supported_mutating_signatures_are_enabled() {
    assert_eq!(supported_methods(XML), Ok(Methods([true; 3])));
    let old_api = XML.replace("WithFlags", "");
    assert_eq!(
        supported_methods(&old_api),
        Ok(Methods([true, false, false]))
    );
    let wrong_type = XML.replace("type=\"t\"", "type=\"b\"");
    assert_eq!(
        supported_methods(&wrong_type),
        Ok(Methods([true, false, false]))
    );
    let output = XML.replace(
        "type=\"t\" direction=\"in\"",
        "type=\"t\" direction=\"out\"",
    );
    assert_eq!(
        supported_methods(&output),
        Ok(Methods([true, false, false]))
    );
    let quoted = XML.replace('"', "'").replace("direction='in'", "");
    assert_eq!(supported_methods(&quoted), Ok(Methods([true; 3])));
    let comment = format!("<node><!-- {XML} --></node>");
    assert_eq!(supported_methods(&comment), Ok(Methods([false; 3])));
    let other_interface = XML.replace(MANAGER, "org.alcard.Other");
    assert_eq!(supported_methods(&other_interface), Ok(Methods([false; 3])));
    assert_eq!(
        supported_methods(&" ".repeat(MAX_REPLY_BYTES + 1)),
        Err(Failure::Unavailable)
    );
    assert_eq!(supported_methods("<node><!--"), Err(Failure::Unavailable));
}

#[test]
fn inhibitors_include_weak_blocks_and_fail_closed_on_unreadable_policy() {
    let record = |what: &str, mode: &str| {
        (
            what.into(),
            "app".into(),
            "unsaved work".into(),
            mode.into(),
            1000,
            123,
        )
    };
    for mode in ["block", "block-weak"] {
        assert_eq!(
            shutdown_inhibited(&[record("shutdown:sleep", mode)]),
            Ok(true)
        );
    }
    assert_eq!(
        shutdown_inhibited(&[record("shutdown", "delay")]),
        Ok(false)
    );
    assert_eq!(shutdown_inhibited(&[record("sleep", "block")]), Ok(false));
    assert_eq!(
        shutdown_inhibited(&[record("notshutdown", "block")]),
        Ok(false)
    );
    assert_eq!(
        shutdown_inhibited(&[record("shutdown", "future")]),
        Err(Failure::Unavailable)
    );
    assert_eq!(
        shutdown_inhibited(&vec![record("sleep", "block"); MAX_INHIBITORS + 1]),
        Err(Failure::Unavailable)
    );

    let mut fake = Fake {
        inhibited: Err(Failure::Unavailable),
        ..Fake::default()
    };
    let (_, snapshot) = probe(
        &mut fake,
        &Budget::new(&AtomicBool::new(false), READ_TIMEOUT),
    )
    .unwrap();
    assert_eq!(
        snapshot.capabilities,
        [
            Capability::Allowed,
            Capability::Unavailable,
            Capability::Unavailable
        ]
    );
}

fn properties(id: &str, uid: u32, kind: &str, remote: bool) -> HashMap<String, OwnedVariant> {
    let user = OwnedObjectPath::try_from("/org/freedesktop/login1/user/_1000").unwrap();
    HashMap::from([
        (
            "Id".into(),
            OwnedVariant::try_from(Variant::from(id)).unwrap(),
        ),
        (
            "User".into(),
            OwnedVariant::try_from(Variant::from((uid, user))).unwrap(),
        ),
        (
            "Type".into(),
            OwnedVariant::try_from(Variant::from(kind)).unwrap(),
        ),
        ("Remote".into(), OwnedVariant::from(remote)),
    ])
}

#[test]
fn logout_requires_own_local_graphical_session_and_validated_id() {
    let path = OwnedObjectPath::try_from(local_session().path).unwrap();
    for kind in ["wayland", "x11"] {
        assert_eq!(
            session_from_properties(&path, properties("c212", 1000, kind, false), 1000),
            Ok(local_session())
        );
    }
    for (id, uid, kind, remote) in [
        ("c212", 1001, "wayland", false),
        ("c212", 1000, "wayland", true),
        ("c212", 1000, "tty", false),
        ("self", 1000, "wayland", false),
        ("auto", 1000, "wayland", false),
        ("", 1000, "wayland", false),
        ("c212/other", 1000, "wayland", false),
    ] {
        assert_eq!(
            session_from_properties(&path, properties(id, uid, kind, remote), 1000),
            Err(Failure::SessionUnavailable)
        );
    }
    let mut missing = properties("c212", 1000, "wayland", false);
    missing.remove("Remote");
    assert_eq!(
        session_from_properties(&path, missing, 1000),
        Err(Failure::SessionUnavailable)
    );
    let mut wrong_type = properties("c212", 1000, "wayland", false);
    wrong_type.insert("User".into(), OwnedVariant::from(1000u32));
    assert_eq!(
        session_from_properties(&path, wrong_type, 1000),
        Err(Failure::SessionUnavailable)
    );
    let wrong_path = OwnedObjectPath::try_from("/org/alcard/session/c212").unwrap();
    assert_eq!(
        session_from_properties(
            &wrong_path,
            properties("c212", 1000, "wayland", false),
            1000
        ),
        Err(Failure::SessionUnavailable)
    );
}

#[test]
fn owner_replacement_before_or_during_preflight_never_mutates() {
    for during_preflight in [false, true] {
        let mut fake = Fake::default();
        let context = pin(&mut fake);
        if during_preflight {
            fake.owners.push_back(context.owner.clone());
        }
        fake.owner = ":1.213".into();
        let execution = execute_pinned(
            &mut fake,
            &context,
            Operation::Reboot,
            &AtomicBool::new(false),
        );
        assert_eq!(execution.result, Err(Failure::Unavailable));
        assert!(execution.invalidated);
        assert!(fake.mutations.is_empty());
    }
}

#[test]
fn a_fresh_refresh_may_explicitly_pin_the_replacement_service() {
    let mut fake = Fake::default();
    let previous = pin(&mut fake);
    fake.owner = ":1.213".into();
    assert_eq!(
        execute_pinned(
            &mut fake,
            &previous,
            Operation::PowerOff,
            &AtomicBool::new(false)
        )
        .result,
        Err(Failure::Unavailable)
    );
    let refreshed = pin(&mut fake);
    assert_eq!(
        execute_pinned(
            &mut fake,
            &refreshed,
            Operation::PowerOff,
            &AtomicBool::new(false)
        )
        .result,
        Ok(())
    );
    assert_eq!(
        fake.mutations,
        vec![(":1.213".into(), Operation::PowerOff, None)]
    );
}

#[test]
fn probe_does_not_publish_a_snapshot_across_an_owner_change() {
    let mut fake = Fake {
        owner: ":1.213".into(),
        owners: VecDeque::from([":1.212".into()]),
        ..Fake::default()
    };
    assert!(matches!(
        probe(
            &mut fake,
            &Budget::new(&AtomicBool::new(false), READ_TIMEOUT)
        ),
        Err(Failure::Unavailable)
    ));
    assert!(fake.mutations.is_empty());
}

#[test]
fn fresh_policy_and_inhibitors_are_checked_before_every_mutation() {
    for (capability, inhibited, expected) in [
        (Capability::Denied, Ok(false), Failure::Denied),
        (Capability::Allowed, Ok(true), Failure::Blocked),
        (
            Capability::Authentication,
            Err(Failure::Unavailable),
            Failure::Unavailable,
        ),
        (Capability::Allowed, Err(Failure::Timeout), Failure::Timeout),
    ] {
        let mut fake = Fake::default();
        let context = pin(&mut fake);
        fake.capabilities[Operation::PowerOff.index()] = capability;
        fake.inhibited = inhibited;
        assert_eq!(
            execute_pinned(
                &mut fake,
                &context,
                Operation::PowerOff,
                &AtomicBool::new(false)
            )
            .result,
            Err(expected)
        );
        assert!(fake.mutations.is_empty());
    }
}

#[test]
fn missing_safe_method_never_falls_back_to_boolean_power_method() {
    let mut fake = Fake {
        methods: Methods([true, false, false]),
        ..Fake::default()
    };
    let (context, snapshot) = probe(
        &mut fake,
        &Budget::new(&AtomicBool::new(false), READ_TIMEOUT),
    )
    .unwrap();
    assert_eq!(
        snapshot.capabilities,
        [
            Capability::Allowed,
            Capability::Unsupported,
            Capability::Unsupported
        ]
    );
    assert_eq!(
        execute_pinned(
            &mut fake,
            &context,
            Operation::Reboot,
            &AtomicBool::new(false)
        )
        .result,
        Err(Failure::Unavailable)
    );
    assert!(fake.mutations.is_empty());
}

#[test]
fn logout_revalidates_exact_session_and_sends_only_that_id() {
    let mut fake = Fake::default();
    let context = pin(&mut fake);
    fake.session.as_mut().unwrap().id = "c213".into();
    assert_eq!(
        execute_pinned(
            &mut fake,
            &context,
            Operation::Logout,
            &AtomicBool::new(false)
        )
        .result,
        Err(Failure::SessionUnavailable)
    );
    assert!(fake.mutations.is_empty());
    fake.session = Ok(local_session());
    assert_eq!(
        execute_pinned(
            &mut fake,
            &context,
            Operation::Logout,
            &AtomicBool::new(false)
        )
        .result,
        Ok(())
    );
    assert_eq!(
        fake.mutations,
        vec![(
            context.owner.clone(),
            Operation::Logout,
            Some(local_session())
        )]
    );
    assert!(fake
        .destinations
        .iter()
        .all(|owner| owner == &context.owner));
}

fn method_error(name: &str) -> zbus::Error {
    let call = zbus::Message::method_call(MANAGER_PATH, "PowerOffWithFlags")
        .build(&(1u64,))
        .unwrap();
    zbus::Message::error(&call.header(), name)
        .build(&("test reply",))
        .unwrap()
        .into()
}

#[test]
fn mutation_errors_distinguish_a_refusal_from_an_unknown_outcome() {
    for (name, expected) in [
        ("org.freedesktop.DBus.Error.AccessDenied", Failure::Denied),
        (
            "org.freedesktop.PolicyKit1.Error.Cancelled",
            Failure::Cancelled,
        ),
        ("org.freedesktop.login1.Inhibited", Failure::Blocked),
        (
            "org.freedesktop.login1.NoSuchSession",
            Failure::SessionUnavailable,
        ),
        (
            "org.freedesktop.DBus.Error.UnknownMethod",
            Failure::Unavailable,
        ),
        (
            "org.freedesktop.DBus.Error.InvalidArgs",
            Failure::Unavailable,
        ),
        (
            "org.freedesktop.DBus.Error.NameHasNoOwner",
            Failure::Unavailable,
        ),
        ("org.freedesktop.DBus.Error.NoReply", Failure::Unknown),
        ("org.alcard.UnknownError", Failure::Unknown),
    ] {
        assert_eq!(bus_failure(&method_error(name), true), expected, "{name}");
    }
    assert_eq!(
        bus_failure(&method_error("org.freedesktop.DBus.Error.NoReply"), false),
        Failure::Unavailable
    );
    assert_eq!(
        bus_failure(&zbus::Error::InvalidReply, true),
        Failure::Unknown
    );
}

#[test]
fn deadlines_distinguish_read_only_timeout_from_uncertain_mutation() {
    for mutating in [false, true] {
        let stopped = AtomicBool::new(false);
        let budget = Budget::new(&stopped, Duration::from_millis(2));
        let result = budget.dbus(std::future::pending::<zbus::Result<()>>(), mutating);
        assert_eq!(
            result,
            Err(if mutating {
                Failure::Unknown
            } else {
                Failure::Timeout
            })
        );
    }
    let stopped = AtomicBool::new(true);
    let budget = Budget::new(&stopped, READ_TIMEOUT);
    let never_poll = std::future::poll_fn(|_| -> Poll<()> { panic!("stopped future was polled") });
    assert_eq!(budget.wait(never_poll), Err(WaitFailure::Stopped));
    assert_eq!(
        budget.dbus(std::future::pending::<zbus::Result<()>>(), false),
        Err(Failure::Cancelled)
    );
}

#[test]
fn async_wakes_progress_on_the_single_worker() {
    let stopped = AtomicBool::new(false);
    let mut polls = 0;
    let future = std::future::poll_fn(|context| {
        polls += 1;
        if polls == 1 {
            context.waker().wake_by_ref();
            Poll::Pending
        } else {
            Poll::Ready(42)
        }
    });
    assert_eq!(Budget::new(&stopped, READ_TIMEOUT).wait(future), Ok(42));
    assert_eq!(polls, 2);
}

#[test]
fn accepted_and_uncertain_requests_keep_the_global_duplicate_latch() {
    for result in [
        Ok(()),
        Err(Failure::Unknown),
        Err(Failure::Denied),
        Err(Failure::Timeout),
    ] {
        let mut fake = Fake {
            mutation_result: result,
            ..Fake::default()
        };
        let context = pin(&mut fake);
        let execution = execute_pinned(
            &mut fake,
            &context,
            Operation::Reboot,
            &AtomicBool::new(false),
        );
        let control = Control::default();
        control.busy.store(true, Ordering::Release);
        settle(&control, execution.result);
        assert_eq!(
            control.busy.load(Ordering::Acquire),
            matches!(result, Ok(()) | Err(Failure::Unknown))
        );
        assert_eq!(fake.mutations.len(), 1);
    }
}

#[test]
fn client_coalesces_refresh_and_rejects_duplicate_execution_without_workers() {
    let (commands, incoming) = mpsc::sync_channel(2);
    let control = Arc::new(Control::default());
    let client = Client {
        commands,
        control: control.clone(),
    };
    assert!(client.refresh());
    assert!(client.refresh());
    assert!(matches!(incoming.try_recv(), Ok(Work::Refresh)));
    assert!(incoming.try_recv().is_err());
    control.refreshing.store(false, Ordering::Release);
    assert!(client.execute(3, Operation::Logout));
    assert!(!client.execute(4, Operation::PowerOff));
    assert!(!client.refresh());
    assert!(matches!(
        incoming.try_recv(),
        Ok(Work::Execute {
            request: 3,
            operation: Operation::Logout
        })
    ));
    assert!(incoming.try_recv().is_err());
    settle(&control, Err(Failure::Denied));
    assert!(client.execute(5, Operation::PowerOff));
    drop(client);
    assert!(control.stopped.load(Ordering::Acquire));
}

#[test]
fn oversized_or_wrongly_typed_dbus_bodies_are_rejected() {
    let call = zbus::Message::method_call(MANAGER_PATH, "Introspect")
        .build(&())
        .unwrap();
    let oversized = zbus::Message::method_return(&call.header())
        .build(&"x".repeat(MAX_REPLY_BYTES))
        .unwrap();
    assert_eq!(decode::<String>(oversized), Err(Failure::Unavailable));
    let wrong_type = zbus::Message::method_return(&call.header())
        .build(&12u32)
        .unwrap();
    assert_eq!(decode::<String>(wrong_type), Err(Failure::Unavailable));
}
