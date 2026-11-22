use super::*;

fn incoming(title: &str) -> Incoming<'_> {
    Incoming {
        internal: false,
        replaces_id: 0,
        app_name: "Example",
        title,
        body: "Full message",
        actions: &[],
        hints: Hints::default(),
        expire_timeout: -1,
    }
}
#[test]
fn unread_survives_expiration_and_only_matching_generation_marks_read() {
    let mut store = Store::default();
    let now = Instant::now();
    let id = store.notify(incoming("Message"), now, 123).unwrap().0;
    let first = store.snapshot();
    let generation = first.notices[0].generation;
    assert_eq!(first.notices[0].created_at, 123);
    assert!(first.notices[0].unread);
    assert_eq!(store.expire(now + Duration::from_secs(6)).closed, vec![id]);
    let expired = store.snapshot();
    assert!(!expired.notices[0].active);
    assert!(expired.notices[0].unread);
    assert_eq!(expired.event_generation, first.event_generation);
    assert!(!store.mark_read(id, generation + 1));
    assert!(store.mark_read(id, generation));
    assert!(!store.snapshot().notices[0].unread);
    assert!(!store.mark_read(id, generation));
}
#[test]
fn replacement_keeps_id_but_is_new_unread_generation() {
    let mut store = Store::default();
    let now = Instant::now();
    let id = store.notify(incoming("First"), now, 123).unwrap().0;
    let old = store.snapshot().notices[0].generation;
    store.mark_read(id, old);
    let mut replacement = incoming("Second");
    replacement.replaces_id = id;
    assert_eq!(store.notify(replacement, now, 456).unwrap().0, id);
    let snapshot = store.snapshot();
    assert_eq!(snapshot.notices.len(), 1);
    assert!(snapshot.notices[0].unread);
    assert!(snapshot.notices[0].generation > old);
    assert_eq!(snapshot.notices[0].created_at, 456);
    assert!(!store.mark_read(id, old));
    assert!(store.snapshot().notices[0].unread);
}
#[test]
fn deleting_newest_does_not_publish_an_older_arrival() {
    let mut store = Store::default();
    let now = Instant::now();
    let first = store.notify(incoming("A"), now, 1).unwrap().0;
    let second = store.notify(incoming("B"), now, 2).unwrap().0;
    let event = store.snapshot().latest_event;
    let generation = store.snapshot().event_generation;
    store.dismiss(second);
    let snapshot = store.snapshot();
    assert_eq!(snapshot.notices[0].id, first);
    assert_eq!(snapshot.event_generation, generation);
    assert_eq!(snapshot.latest_event, event);
    store.clear();
    assert_eq!(store.snapshot().event_generation, generation);
}
#[test]
fn named_action_requires_exact_key_and_generation_and_closes_once() {
    let mut store = Store::default();
    let now = Instant::now();
    let actions = vec![
        "retry".into(),
        "Try again".into(),
        "default".into(),
        "Open".into(),
    ];
    let mut item = incoming("Download");
    item.actions = &actions;
    let id = store.notify(item, now, 1).unwrap().0;
    let notice = store.snapshot().notices.remove(0);
    assert_eq!(notice.actions.len(), 2);
    assert_eq!(notice.actions[0].label, "Try again");
    assert!(store.invoke(id, notice.generation + 1, "retry").is_none());
    assert!(store.invoke(id, notice.generation, "missing").is_none());
    assert_eq!(
        store.invoke(id, notice.generation, "retry"),
        Some(vec![
            Signal::Action(id, "retry".into()),
            Signal::Closed(id, 2)
        ])
    );
    let retained = store.snapshot();
    assert!(!retained.notices[0].unread);
    assert!(!retained.notices[0].active);
    assert!(store.invoke(id, notice.generation, "retry").is_none());
}
#[test]
fn resident_action_remains_live_and_transient_expiry_removes_history() {
    let mut store = Store::default();
    let now = Instant::now();
    let actions = vec!["pause".into(), "Pause".into()];
    let mut resident = incoming("Player");
    resident.actions = &actions;
    resident.hints.resident = true;
    let id = store.notify(resident, now, 1).unwrap().0;
    let generation = store.snapshot().notices[0].generation;
    assert_eq!(
        store.invoke(id, generation, "pause"),
        Some(vec![Signal::Action(id, "pause".into())])
    );
    assert!(store.snapshot().notices[0].active);
    assert!(!store.expire(now + Duration::from_secs(60)).changed);
    let mut transient = incoming("Volume");
    transient.hints.transient = true;
    transient.expire_timeout = 10;
    let transient_id = store.notify(transient, now, 2).unwrap().0;
    assert_eq!(
        store.expire(now + Duration::from_millis(11)).closed,
        vec![transient_id]
    );
    assert_eq!(store.snapshot().notices.len(), 1);
    assert_eq!(store.snapshot().notices[0].id, id);
}
#[test]
fn critical_notifications_wait_for_explicit_close() {
    let mut store = Store::default();
    let now = Instant::now();
    let mut item = incoming("Low battery");
    item.hints.urgency = 2;
    item.expire_timeout = 100;
    let id = store.notify(item, now, 1).unwrap().0;
    assert!(!store.expire(now + Duration::from_secs(100)).changed);
    assert_eq!(store.close(id), Some(true));
    assert_eq!(store.close(id), Some(false));
    assert!(store.snapshot().notices[0].unread);
}
#[test]
fn sender_recall_removes_unread_history_without_a_new_arrival() {
    let mut store = Store::default();
    let id = store
        .notify(incoming("Recall me"), Instant::now(), 1)
        .unwrap()
        .0;
    let generation = store.event_generation;
    assert_eq!(store.withdraw(id), Some(true));
    assert!(store.snapshot().notices.is_empty());
    assert_eq!(store.event_generation, generation);
    assert_eq!(store.withdraw(id), None);
}
#[test]
fn history_and_text_are_bounded_without_silent_truncation() {
    let mut store = Store::default();
    let now = Instant::now();
    let body = "я".repeat(BODY_LIMIT + 50);
    for n in 0..100 {
        let mut item = incoming("Message");
        item.body = &body;
        store.notify(item, now, n).unwrap();
    }
    let snapshot = store.snapshot();
    assert_eq!(snapshot.notices.len(), HISTORY_LIMIT);
    assert_eq!(snapshot.notices[0].description.chars().count(), BODY_LIMIT);
    assert!(snapshot.notices[0].description.ends_with('…'));
    assert_eq!(plain("first\nsecond\tline", 50, true), "first\nsecond line");
    assert_eq!(plain("first\nsecond", 50, false), "first second");
}
#[test]
fn invalid_actions_are_rejected_before_replacing_existing_content() {
    let mut store = Store::default();
    let now = Instant::now();
    let id = store.notify(incoming("Keep this"), now, 1).unwrap().0;
    let mut actions = Vec::new();
    for n in 0..=ACTION_LIMIT {
        actions.push(format!("action-{n}"));
        actions.push("x".repeat(128));
    }
    let mut replacement = incoming("Must not replace");
    replacement.replaces_id = id;
    replacement.actions = &actions;
    assert!(store.notify(replacement, now, 2).is_err());
    assert_eq!(store.snapshot().notices[0].title, "Keep this");
    actions.truncate(ACTION_LIMIT * 2);
    let bounded = bounded_actions(&actions).unwrap();
    assert_eq!(bounded.len(), ACTION_LIMIT);
    assert_eq!(bounded[0].key, "action-0");
    assert_eq!(bounded[0].label.chars().count(), 64);
    assert!(bounded[0].label.ends_with('…'));
    assert!(bounded_actions(&["unpaired".into()]).is_err());
    assert!(bounded_actions(&["bad\nkey".into(), "Bad".into()]).is_err());
    assert!(bounded_actions(&[
        "default".into(),
        "Open".into(),
        "default".into(),
        "Duplicate".into()
    ])
    .is_err());
}
#[test]
fn updates_coalesce_until_snapshot_is_consumed() {
    use smithay_client_toolkit::reexports::calloop::channel;
    let (updates, receiver) = channel::channel();
    let shared = Arc::new(Shared::default());
    let publisher = Publisher {
        shared: shared.clone(),
        updates,
    };
    let (commands, _requests) = mpsc::sync_channel(COMMAND_LIMIT);
    let client = Client {
        commands: commands.clone(),
        shared: shared.clone(),
        stopped: Arc::new(AtomicBool::new(false)),
        wake: Waker {
            commands,
            pending: Arc::new(AtomicBool::new(false)),
        },
    };
    for n in 0..1000 {
        let mut store = shared.store.lock().unwrap();
        store
            .notify(incoming("Message"), Instant::now(), n)
            .unwrap();
        publisher.changed();
    }
    assert!(matches!(receiver.try_recv(), Ok(Message::Changed)));
    assert!(receiver.try_recv().is_err());
    let snapshot = client.take_update().unwrap();
    assert_eq!(snapshot.event_generation, 1000);
    assert_eq!(snapshot.notices.len(), HISTORY_LIMIT);
    assert!(client.take_update().is_none());
    {
        let _store = shared.store.lock().unwrap();
        publisher.changed();
    }
    assert!(matches!(receiver.try_recv(), Ok(Message::Changed)));
}
#[test]
fn commands_and_internal_wakes_are_bounded_and_nonblocking() {
    let (commands, requests) = mpsc::sync_channel(2);
    let wake = Waker {
        commands: commands.clone(),
        pending: Arc::new(AtomicBool::new(false)),
    };
    for _ in 0..1000 {
        wake.wake();
    }
    assert!(matches!(requests.try_recv(), Ok(Work::Wake)));
    assert!(requests.try_recv().is_err());
    wake.consumed();
    commands.try_send(Work::User(Command::Clear)).unwrap();
    commands.try_send(Work::User(Command::Clear)).unwrap();
    assert!(matches!(
        commands.try_send(Work::User(Command::Clear)),
        Err(mpsc::TrySendError::Full(_))
    ));
    wake.wake(); // A full command queue already wakes the worker.
    assert!(!wake.pending.load(Ordering::Acquire));
}
#[test]
fn internal_errors_are_bounded_coalesced_and_keep_external_history_offline() {
    use smithay_client_toolkit::reexports::calloop::channel;
    let (updates, receiver) = channel::channel();
    let shared = Arc::new(Shared::default());
    let publisher = Publisher {
        shared: shared.clone(),
        updates,
    };
    let (commands, requests) = mpsc::sync_channel(COMMAND_LIMIT);
    let client = Client {
        commands: commands.clone(),
        shared: shared.clone(),
        stopped: Arc::new(AtomicBool::new(false)),
        wake: Waker {
            commands,
            pending: Arc::new(AtomicBool::new(false)),
        },
    };
    let external = {
        let mut store = shared.store.lock().unwrap();
        store
            .notify(
                incoming("Existing application message"),
                Instant::now(),
                123,
            )
            .unwrap();
        store.snapshot().notices[0].clone()
    };
    for _ in 0..COMMAND_LIMIT {
        client
            .request(Command::PostInternal {
                title: "я".repeat(TITLE_LIMIT + 30),
                body: "x".repeat(BODY_LIMIT + 30),
            })
            .unwrap();
    }
    assert!(client
        .request(Command::PostInternal {
            title: "Queue full".into(),
            body: "Still nonblocking".into(),
        })
        .is_err());
    for _ in 0..COMMAND_LIMIT {
        let Work::User(command) = requests.try_recv().unwrap() else {
            panic!("expected a bounded internal command");
        };
        let Command::PostInternal { title, body } = &command else {
            panic!("expected an internal error");
        };
        assert_eq!(title.chars().count(), TITLE_LIMIT);
        assert_eq!(body.chars().count(), BODY_LIMIT);
        assert!(title.ends_with('…') && body.ends_with('…'));
        assert!(!handle_command(command, &publisher, None).unwrap());
    }
    assert!(matches!(receiver.try_recv(), Ok(Message::Changed)));
    assert!(receiver.try_recv().is_err());
    let snapshot = client.take_update().unwrap();
    assert_eq!(snapshot.notices.len(), COMMAND_LIMIT + 1);
    assert_eq!(snapshot.notices.last(), Some(&external));
    assert!(snapshot.notices[..COMMAND_LIMIT].iter().all(|notice| {
        notice.app_name == "Alcard Shell"
            && notice.active
            && notice.unread
            && notice.actions.is_empty()
            && notice.desktop_entry.is_none()
    }));
    let generation = snapshot.event_generation;
    {
        let mut store = shared.store.lock().unwrap();
        assert!(store.disconnect());
        assert!(!store.disconnect());
        assert!(store
            .entries
            .iter()
            .filter(|entry| entry.internal)
            .all(|entry| entry.notice.active));
    }
    // The same expiry path runs during reconnect backoff, without a bus.
    expire_notifications(&publisher, None, Instant::now() + Duration::from_secs(6)).unwrap();
    let expired = client.take_update().unwrap();
    assert_eq!(expired.event_generation, generation);
    assert_eq!(expired.notices.len(), COMMAND_LIMIT + 1);
    assert!(expired
        .notices
        .iter()
        .all(|notice| !notice.active && notice.unread));
    assert_eq!(expired.notices.last().unwrap().title, external.title);
    let notice = &expired.notices[0];
    handle_command(
        Command::MarkRead {
            id: notice.id,
            generation: notice.generation,
        },
        &publisher,
        None,
    )
    .unwrap();
    let read = client.take_update().unwrap();
    assert!(!read.notices[0].unread);
    assert!(read.notices.last().unwrap().unread);
}
#[test]
fn internal_entries_do_not_leak_a_dbus_lifetime_or_accept_external_replacement() {
    let mut store = Store::default();
    let now = Instant::now();
    let mut internal = incoming("Shell launch error");
    internal.internal = true;
    let id = store.notify(internal, now, 1).unwrap().0;
    assert_eq!(store.withdraw(id), None);
    let mut external = incoming("External update");
    external.replaces_id = id;
    let external_id = store.notify(external, now, 2).unwrap().0;
    assert_ne!(external_id, id);
    assert_eq!(store.entries.len(), 2);
    let expired = store.expire(now + Duration::from_secs(6));
    assert!(expired.changed);
    assert_eq!(expired.closed, vec![external_id]);
    assert_eq!(store.entries.len(), 2);
    assert_eq!(store.dismiss(id), Some(false));

    let mut next = incoming("Another Shell error");
    next.internal = true;
    store.notify(next, now, 3).unwrap();
    assert!(store.clear().is_empty());
}
#[test]
fn disconnect_preserves_unread_history_and_invalidates_actions() {
    let mut store = Store::default();
    let now = Instant::now();
    let id = store.notify(incoming("Message"), now, 1).unwrap().0;
    let generation = store.snapshot().event_generation;
    assert!(store.disconnect());
    let snapshot = store.snapshot();
    assert_eq!(snapshot.notices[0].id, id);
    assert!(snapshot.notices[0].unread);
    assert!(!snapshot.notices[0].active);
    assert_eq!(snapshot.event_generation, generation);
    assert!(!store.disconnect());
}
#[test]
fn source_and_supported_hints_are_kept_without_file_paths() {
    let mut hints = HashMap::new();
    hints.insert("urgency".into(), OwnedVariant::from(2_u8));
    hints.insert("resident".into(), OwnedVariant::from(true));
    hints.insert("transient".into(), OwnedVariant::from(true));
    hints.insert(
        "desktop-entry".into(),
        OwnedVariant::from(zbus::Str::from("org.example.Mail")),
    );
    let parsed = Hints::parse(&hints);
    assert_eq!(parsed.desktop_entry.as_deref(), Some("org.example.Mail"));
    assert_eq!(parsed.urgency, 2);
    assert!(parsed.resident && parsed.transient);
    hints.insert(
        "desktop-entry".into(),
        OwnedVariant::from(zbus::Str::from("../../secret")),
    );
    assert!(Hints::parse(&hints).desktop_entry.is_none());
}

struct PrivateBus {
    child: std::process::Child,
    directory: std::path::PathBuf,
    address: String,
}
impl PrivateBus {
    fn start() -> Self {
        use std::{
            io::{BufRead, BufReader},
            os::unix::fs::DirBuilderExt,
            process::{Command as ProcessCommand, Stdio},
            sync::atomic::AtomicU64,
        };
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "alcard-notification-bus-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .expect("create private notification test directory");
        let child = ProcessCommand::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .arg(format!("--address=unix:path={}/bus", directory.display()))
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("isolated notification test requires dbus-daemon");
        let mut bus = Self {
            child,
            directory,
            address: String::new(),
        };
        let output = bus.child.stdout.take().unwrap();
        let (sender, receiver) = mpsc::sync_channel(1);
        let reader = thread::spawn(move || {
            let mut address = String::new();
            let result = BufReader::new(output)
                .read_line(&mut address)
                .map(|_| address);
            let _ = sender.send(result);
        });
        bus.address = receiver
            .recv_timeout(Duration::from_secs(3))
            .expect("private dbus-daemon startup timeout")
            .expect("read private dbus-daemon address")
            .trim()
            .to_owned();
        reader.join().unwrap();
        assert!(
            bus.address.starts_with("unix:path="),
            "private dbus-daemon did not report a socket: {:?}; status: {:?}",
            bus.address,
            bus.child.try_wait()
        );
        bus
    }
}
impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
fn wait_snapshot(client: &Client, predicate: impl Fn(&Snapshot) -> bool) -> Snapshot {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(snapshot) = client.take_update() {
            if predicate(&snapshot) {
                return snapshot;
            }
        }
        assert!(Instant::now() < deadline, "notification snapshot deadline");
        thread::sleep(Duration::from_millis(5));
    }
}
fn wait_message(
    receiver: &smithay_client_toolkit::reexports::calloop::channel::Channel<Message>,
    predicate: impl Fn(&Message) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        while let Ok(message) = receiver.try_recv() {
            if predicate(&message) {
                return;
            }
        }
        assert!(
            Instant::now() < deadline,
            "notification service status deadline"
        );
        thread::sleep(Duration::from_millis(5));
    }
}
fn next_signal(
    proxy: &zbus::Proxy<'_>,
    name: &'static str,
) -> (Receiver<zbus::Message>, thread::JoinHandle<()>) {
    let mut signals = zbus::block_on(proxy.receive_signal(name)).unwrap();
    let (sender, receiver) = mpsc::sync_channel(1);
    let worker = thread::spawn(move || {
        if let Some(message) = zbus::block_on(signals.next()) {
            let _ = sender.send(message);
        }
    });
    (receiver, worker)
}
fn private_bus_scenario(address: &str) {
    use smithay_client_toolkit::reexports::calloop::channel;
    let owner = zbus::block_on(Builder::address(address).build()).unwrap();
    zbus::block_on(owner.request_name_with_flags(
        BUS_NAME,
        zbus::fdo::RequestNameFlags::ALLOW_REPLACEMENT | zbus::fdo::RequestNameFlags::DO_NOT_QUEUE,
    ))
    .unwrap();
    let expected_owner = owner.unique_name().unwrap().to_string();
    let (updates, receiver) = channel::channel();
    let client = Client::start_at(updates, Some(address.to_owned()));
    wait_message(&receiver, |message| {
        assert!(
            !matches!(message, Message::Available),
            "backend stole another server's name"
        );
        matches!(message, Message::Unavailable(_))
    });
    let bus = zbus::block_on(zbus::Proxy::new(
        &owner,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
    ))
    .unwrap();
    let actual_owner: String = zbus::block_on(bus.call("GetNameOwner", &(BUS_NAME,))).unwrap();
    assert_eq!(actual_owner, expected_owner);
    zbus::block_on(owner.release_name(BUS_NAME)).unwrap();
    client.request(Command::Retry).unwrap();
    wait_message(&receiver, |message| matches!(message, Message::Available));

    let connection = zbus::block_on(Builder::address(address).build()).unwrap();
    let proxy = zbus::block_on(zbus::Proxy::new(
        &connection,
        BUS_NAME,
        OBJECT_PATH,
        INTERFACE,
    ))
    .unwrap();
    let capabilities: Vec<String> = zbus::block_on(proxy.call("GetCapabilities", &())).unwrap();
    assert!(capabilities
        .iter()
        .any(|capability| capability == "actions"));
    let actions = vec![
        "default".to_owned(),
        "Open".to_owned(),
        "retry".to_owned(),
        "Retry".to_owned(),
    ];
    let mut hints = HashMap::new();
    hints.insert(
        "desktop-entry".to_owned(),
        OwnedVariant::from(zbus::Str::from("org.example.Mail")),
    );
    let body = "A complete notification body much longer than the previous thirty-three character preview.\nSecond paragraph.";
    let id: u32 = zbus::block_on(proxy.call(
        "Notify",
        &(
            "Mail",
            0_u32,
            "",
            "A message",
            body,
            &actions,
            &hints,
            0_i32,
        ),
    ))
    .unwrap();
    let first = wait_snapshot(&client, |snapshot| {
        snapshot.notices.iter().any(|notice| notice.id == id)
    });
    let notice = first.notices.iter().find(|notice| notice.id == id).unwrap();
    assert!(notice.active && notice.unread);
    assert_eq!(notice.app_name, "Mail");
    assert_eq!(notice.desktop_entry.as_deref(), Some("org.example.Mail"));
    assert_eq!(notice.description, body);
    let old_generation = notice.generation;
    client
        .request(Command::MarkRead {
            id,
            generation: old_generation,
        })
        .unwrap();
    wait_snapshot(&client, |snapshot| {
        snapshot
            .notices
            .iter()
            .any(|notice| notice.id == id && !notice.unread)
    });
    let replacement: u32 = zbus::block_on(proxy.call(
        "Notify",
        &(
            "Mail",
            id,
            "",
            "Updated message",
            body,
            &actions,
            &hints,
            0_i32,
        ),
    ))
    .unwrap();
    assert_eq!(replacement, id);
    let replaced = wait_snapshot(&client, |snapshot| {
        snapshot
            .notices
            .iter()
            .any(|notice| notice.id == id && notice.generation > old_generation)
    });
    let notice = replaced
        .notices
        .iter()
        .find(|notice| notice.id == id)
        .unwrap();
    assert!(notice.unread);
    let generation = notice.generation;
    let (action_signal, action_worker) = next_signal(&proxy, "ActionInvoked");
    let (closed_signal, closed_worker) = next_signal(&proxy, "NotificationClosed");
    client
        .request(Command::Invoke {
            id,
            generation,
            action: "retry".into(),
        })
        .unwrap();
    let action: (u32, String) = action_signal
        .recv_timeout(Duration::from_secs(3))
        .unwrap()
        .body()
        .deserialize()
        .unwrap();
    let closed: (u32, u32) = closed_signal
        .recv_timeout(Duration::from_secs(3))
        .unwrap()
        .body()
        .deserialize()
        .unwrap();
    action_worker.join().unwrap();
    closed_worker.join().unwrap();
    assert_eq!(action, (id, "retry".into()));
    assert_eq!(closed, (id, 2));
    wait_snapshot(&client, |snapshot| {
        snapshot
            .notices
            .iter()
            .any(|notice| notice.id == id && !notice.active && !notice.unread)
    });

    let (expired_signal, expired_worker) = next_signal(&proxy, "NotificationClosed");
    let expired_id: u32 = zbus::block_on(proxy.call(
        "Notify",
        &(
            "Clock",
            0_u32,
            "",
            "Expired but unread",
            "Keep this body readable",
            Vec::<String>::new(),
            HashMap::<String, OwnedVariant>::new(),
            150_i32,
        ),
    ))
    .unwrap();
    let expired: (u32, u32) = expired_signal
        .recv_timeout(Duration::from_secs(3))
        .unwrap()
        .body()
        .deserialize()
        .unwrap();
    expired_worker.join().unwrap();
    assert_eq!(expired, (expired_id, 1));
    let history = wait_snapshot(&client, |snapshot| {
        snapshot
            .notices
            .iter()
            .any(|notice| notice.id == expired_id && !notice.active)
    });
    assert!(
        history
            .notices
            .iter()
            .find(|notice| notice.id == expired_id)
            .unwrap()
            .unread
    );
    let generation = history.event_generation;
    client.request(Command::Clear).unwrap();
    let cleared = wait_snapshot(&client, |snapshot| snapshot.notices.is_empty());
    assert_eq!(cleared.event_generation, generation);
}
#[test]
#[ignore = "requires dbus-daemon; creates its own private bus and never uses a desktop session"]
fn isolated_dbus_notification_lifecycle() {
    let bus = PrivateBus::start();
    let address = bus.address.clone();
    let (sender, receiver) = mpsc::sync_channel(1);
    let worker = thread::spawn(move || {
        let result = std::panic::catch_unwind(|| private_bus_scenario(&address));
        let _ = sender.send(result);
    });
    // Every wait has a deadline. On failure the owning guard kills the
    // private daemon, which also unblocks any outstanding D-Bus operation.
    match receiver.recv_timeout(Duration::from_secs(20)) {
        Ok(Ok(())) => worker.join().unwrap(),
        Ok(Err(panic)) => std::panic::resume_unwind(panic),
        Err(error) => panic!("isolated D-Bus scenario did not finish: {error}"),
    }
}
