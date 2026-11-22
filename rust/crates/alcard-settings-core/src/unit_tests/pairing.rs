use super::*;
use std::future::Future;

const DEVICE: &str = "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF";
const OWNER: &str = ":1.42";

fn device() -> OwnedObjectPath {
    OwnedObjectPath::try_from(DEVICE).unwrap()
}

fn message(owner: &str) -> zbus::Message {
    zbus::Message::method_call(AGENT, "RequestPinCode")
        .sender(owner)
        .build(&())
        .unwrap()
}

fn agent() -> (Agent, mpsc::Receiver<Event>, Receiver<()>) {
    let (events, receiver) = mpsc::channel();
    let (release, released) = async_channel::bounded(1);
    (
        Agent {
            device: device(),
            owner: OWNER.into(),
            events,
            pending: Arc::new(Mutex::new(None)),
            released: release,
        },
        receiver,
        released,
    )
}

fn pin_reply(receiver: &mpsc::Receiver<Event>) -> Reply {
    match receiver.try_recv().unwrap() {
        Event::Prompt(Prompt::Pin, Some(reply)) => reply,
        _ => panic!("expected a PIN prompt"),
    }
}

fn assert_closed(receiver: &mpsc::Receiver<Event>) {
    assert!(matches!(receiver.try_recv(), Ok(Event::PromptClosed)));
    assert!(matches!(
        receiver.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
}

fn answer<T>(
    receiver: &mpsc::Receiver<Event>,
    request: impl Future<Output = Result<T, AgentError>>,
    input: Option<&str>,
    check_prompt: impl FnOnce(Prompt),
) -> Result<T, AgentError> {
    zbus::block_on(async {
        let mut request = Box::pin(request);
        assert!(futures_lite::future::poll_once(request.as_mut())
            .await
            .is_none());
        let reply = match receiver.try_recv().unwrap() {
            Event::Prompt(prompt, Some(reply)) => {
                check_prompt(prompt);
                reply
            }
            _ => panic!("expected an interactive pairing prompt"),
        };
        if let Some(input) = input {
            reply.try_send(input.to_owned()).unwrap();
        }
        drop(reply);
        futures_lite::future::poll_once(request.as_mut())
            .await
            .expect("UI response must resolve the pending callback")
    })
}

fn error_name(error: &impl zbus::DBusError, expected: &str) {
    let call = message(OWNER);
    let reply = error.create_reply(&call.header()).unwrap();
    assert_eq!(reply.header().error_name().unwrap().as_str(), expected);
}

#[test]
fn daemon_cancel_interrupts_pending_request_without_user_input() {
    let (agent, receiver, released) = agent();
    let message = message(OWNER);
    zbus::block_on(async {
        let mut request = Box::pin(agent.request_pin_code(device(), message.header()));
        assert!(futures_lite::future::poll_once(request.as_mut())
            .await
            .is_none());
        // The UI retains its reply channel until the daemon dismisses the dialog.
        let held_reply = pin_reply(&receiver);
        agent.cancel(message.header()).unwrap();
        let result = futures_lite::future::poll_once(request.as_mut())
            .await
            .expect("BlueZ Cancel must resolve the callback without waiting for UI input");
        error_name(&result.unwrap_err(), "org.bluez.Error.Canceled");
        assert!(held_reply.is_closed());
        assert!(
            !released.is_closed(),
            "Cancel must not unregister the agent"
        );
        assert_closed(&receiver);
    });
}

#[test]
fn daemon_release_interrupts_prompt_and_ends_agent_lifetime() {
    let (agent, receiver, released) = agent();
    let message = message(OWNER);
    zbus::block_on(async {
        let mut request = Box::pin(agent.request_pin_code(device(), message.header()));
        assert!(futures_lite::future::poll_once(request.as_mut())
            .await
            .is_none());
        let held_reply = pin_reply(&receiver);
        agent.release(message.header()).unwrap();
        let result = futures_lite::future::poll_once(request.as_mut())
            .await
            .expect("BlueZ Release must resolve the pending callback");
        error_name(&result.unwrap_err(), "org.bluez.Error.Canceled");
        assert!(held_reply.is_closed());
        assert!(released.is_closed());
        assert_closed(&receiver);
        error_name(
            &agent
                .request_pin_code(device(), message.header())
                .await
                .unwrap_err(),
            "org.bluez.Error.Canceled",
        );
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
    });
}

#[test]
fn daemon_release_closes_prompt_before_waking_completion() {
    use std::{
        sync::OnceLock,
        task::{Context, Wake, Waker},
    };

    struct CompletionWake {
        events: Mutex<mpsc::Receiver<Event>>,
        prompt_closed: OnceLock<bool>,
    }
    impl Wake for CompletionWake {
        fn wake(self: Arc<Self>) {
            self.wake_by_ref();
        }
        fn wake_by_ref(self: &Arc<Self>) {
            // Observe the release wake directly; do not race two OS threads or
            // acquire pending, which the releasing callback may still hold.
            self.prompt_closed.get_or_init(|| {
                matches!(
                    self.events.lock().unwrap().try_recv(),
                    Ok(Event::PromptClosed)
                )
            });
        }
    }

    let (agent, events, released) = agent();
    let observer = Arc::new(CompletionWake {
        events: Mutex::new(events),
        prompt_closed: OnceLock::new(),
    });
    let waker = Waker::from(observer.clone());
    let mut context = Context::from_waker(&waker);
    let mut completion = Box::pin(released.recv());
    assert!(completion.as_mut().poll(&mut context).is_pending());
    agent.release(message(OWNER).header()).unwrap();
    assert!(completion.as_mut().poll(&mut context).is_ready());
    assert_eq!(
        observer.prompt_closed.get(),
        Some(&true),
        "Release woke pairing completion before publishing PromptClosed"
    );
}

#[test]
fn other_bus_clients_cannot_cancel_release_or_prompt() {
    let (agent, receiver, released) = agent();
    let owner = message(OWNER);
    let stranger = message(":1.99");
    zbus::block_on(async {
        let mut request = Box::pin(agent.request_pin_code(device(), owner.header()));
        assert!(futures_lite::future::poll_once(request.as_mut())
            .await
            .is_none());
        let held_reply = pin_reply(&receiver);
        assert!(matches!(
            agent.cancel(stranger.header()),
            Err(fdo::Error::AccessDenied(_))
        ));
        assert!(matches!(
            agent.release(stranger.header()),
            Err(fdo::Error::AccessDenied(_))
        ));
        assert!(!held_reply.is_closed());
        assert!(!released.is_closed());
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        held_reply.try_send("1234".into()).unwrap();
        assert_eq!(request.await.unwrap(), "1234");
        error_name(
            &agent
                .request_pin_code(device(), stranger.header())
                .await
                .unwrap_err(),
            "org.freedesktop.DBus.Error.AccessDenied",
        );
        error_name(
            &agent
                .request_pin_code(OwnedObjectPath::try_from("/other").unwrap(), owner.header())
                .await
                .unwrap_err(),
            "org.freedesktop.DBus.Error.AccessDenied",
        );
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
    });
}

#[test]
fn cancellation_cannot_steal_the_following_request() {
    let (agent, receiver, _released) = agent();
    let message = message(OWNER);
    zbus::block_on(async {
        let mut first = Box::pin(agent.request_pin_code(device(), message.header()));
        assert!(futures_lite::future::poll_once(first.as_mut())
            .await
            .is_none());
        let _first_reply = pin_reply(&receiver);
        agent.cancel(message.header()).unwrap();
        assert_closed(&receiver);
        let mut second = Box::pin(agent.request_pin_code(device(), message.header()));
        assert!(futures_lite::future::poll_once(second.as_mut())
            .await
            .is_none());
        let second_reply = pin_reply(&receiver);
        error_name(&first.await.unwrap_err(), "org.bluez.Error.Canceled");
        second_reply.try_send("9876".into()).unwrap();
        assert_eq!(second.await.unwrap(), "9876");
    });
}

#[test]
fn display_cancel_preserves_the_pairing_operation() {
    let (agent, receiver, released) = agent();
    let message = message(OWNER);
    agent
        .display_passkey(device(), 42, 1, message.header())
        .unwrap();
    assert!(
        matches!(receiver.try_recv(), Ok(Event::Prompt(Prompt::Display(value), None)) if value == "000042")
    );
    agent.cancel(message.header()).unwrap();
    assert_closed(&receiver);
    assert!(!released.is_closed());
    agent
        .display_pin_code(device(), "1234".into(), message.header())
        .unwrap();
    assert!(
        matches!(receiver.try_recv(), Ok(Event::Prompt(Prompt::Display(value), None)) if value == "1234")
    );
}

#[test]
fn pin_and_passkey_validation_and_reply_denial() {
    let (agent, receiver, _released) = agent();
    let message = message(OWNER);
    for pin in ["1", "1234567890123456"] {
        assert_eq!(
            answer(
                &receiver,
                agent.request_pin_code(device(), message.header()),
                Some(pin),
                |prompt| assert!(matches!(prompt, Prompt::Pin))
            )
            .unwrap(),
            pin
        );
    }
    for pin in ["", "12345678901234567", "12\n3"] {
        error_name(
            &answer(
                &receiver,
                agent.request_pin_code(device(), message.header()),
                Some(pin),
                |prompt| assert!(matches!(prompt, Prompt::Pin)),
            )
            .unwrap_err(),
            "org.bluez.Error.Rejected",
        );
    }
    for (input, expected) in [("0", 0), ("000042", 42), ("999999", 999999)] {
        assert_eq!(
            answer(
                &receiver,
                agent.request_passkey(device(), message.header()),
                Some(input),
                |prompt| assert!(matches!(prompt, Prompt::Passkey))
            )
            .unwrap(),
            expected
        );
    }
    for input in ["", "-1", "1000000", "abc"] {
        error_name(
            &answer(
                &receiver,
                agent.request_passkey(device(), message.header()),
                Some(input),
                |prompt| assert!(matches!(prompt, Prompt::Passkey)),
            )
            .unwrap_err(),
            "org.bluez.Error.Rejected",
        );
    }
    error_name(
        &answer(
            &receiver,
            agent.request_pin_code(device(), message.header()),
            None,
            |prompt| assert!(matches!(prompt, Prompt::Pin)),
        )
        .unwrap_err(),
        "org.bluez.Error.Rejected",
    );
}

#[test]
fn confirmation_authorization_and_service_prompts() {
    let (agent, receiver, _released) = agent();
    let message = message(OWNER);
    answer(
        &receiver,
        agent.request_confirmation(device(), 42, message.header()),
        Some(""),
        |prompt| assert!(matches!(prompt, Prompt::Confirm(value) if value == "000042")),
    )
    .unwrap();
    answer(
        &receiver,
        agent.request_authorization(device(), message.header()),
        Some(""),
        |prompt| assert!(matches!(prompt, Prompt::Confirm(value) if value.is_empty())),
    )
    .unwrap();
    answer(
        &receiver,
        agent.authorize_service(device(), "test-service".into(), message.header()),
        Some(""),
        |prompt| assert!(matches!(prompt, Prompt::Confirm(value) if value == "test-service")),
    )
    .unwrap();
    error_name(
        &answer(
            &receiver,
            agent.request_confirmation(device(), 42, message.header()),
            None,
            |prompt| assert!(matches!(prompt, Prompt::Confirm(_))),
        )
        .unwrap_err(),
        "org.bluez.Error.Rejected",
    );
}
