//! Independent login1 owner and sleep-cycle monitoring with owned delay inhibitors.

use super::{
    login1::{decode, Login1, SystemBus, MANAGER, MANAGER_PATH, SERVICE},
    send, Budget, Capability, Context, Failure, Message, MonitorState, Sender, Shared, Snapshot,
    READ_TIMEOUT,
};
use futures_lite::{future, StreamExt};
use std::{
    num::NonZeroUsize,
    os::fd::OwnedFd,
    sync::{atomic::Ordering, Arc},
    time::Duration,
};
use zbus::message::Sequence;

pub(super) fn lose_epoch(shared: &Shared, updates: &Sender<Message>) {
    let (epoch, snapshot, delay) = {
        let mut state = shared.lock();
        let epoch = state.snapshot.epoch;
        if let Some(pending) = state.pending.take() {
            pending.revoke();
        }
        state.context = None;
        state.connection = None;
        state.cycle = None;
        let delay = state.delay.take();
        state.snapshot.monitor = MonitorState::Unavailable;
        state.snapshot.capability = Capability::Unavailable;
        (epoch, state.snapshot, delay)
    };
    drop(delay);
    let _ = send(updates, Message::Lost { epoch });
    let _ = send(updates, Message::Snapshot(snapshot));
}

pub(super) fn begin_epoch(
    shared: &Shared,
    updates: &Sender<Message>,
    mut context: Context,
    connection: zbus::Connection,
) -> Result<Context, Failure> {
    let snapshot = {
        let mut state = shared.lock();
        if shared.stopped.load(Ordering::Acquire) {
            return Err(Failure::Cancelled);
        }
        context.epoch = state
            .snapshot
            .epoch
            .checked_add(1)
            .ok_or(Failure::Unavailable)?;
        state.snapshot = Snapshot {
            epoch: context.epoch,
            ..Snapshot::default()
        };
        state.cycle = None;
        state.next_cycle = 0;
        state.delay = None;
        state.context = Some(context.clone());
        state.connection = Some(connection);
        state.snapshot
    };
    send(updates, Message::Snapshot(snapshot))?;
    Ok(context)
}

pub(super) fn update_policy(
    shared: &Shared,
    epoch: u64,
    capability: Capability,
    delay_ms: u64,
) -> Result<(), Failure> {
    let mut state = shared.lock();
    if state.snapshot.epoch != epoch || state.context.is_none() {
        return Err(Failure::Unavailable);
    }
    state.snapshot.capability = capability;
    state.snapshot.delay_ms = delay_ms;
    Ok(())
}

pub(super) fn begin_cycle(
    shared: &Shared,
    updates: &Sender<Message>,
    epoch: u64,
) -> Result<(), Failure> {
    let event = {
        let mut state = shared.lock();
        if state.snapshot.epoch != epoch || state.context.is_none() {
            return Err(Failure::Unavailable);
        }
        if state.cycle.is_some() {
            None
        } else {
            let cycle = state
                .next_cycle
                .checked_add(1)
                .ok_or(Failure::Unavailable)?;
            state.next_cycle = cycle;
            state.cycle = Some(cycle);
            state.snapshot.monitor = MonitorState::Preparing;
            Some((cycle, state.snapshot))
        }
    };
    if let Some((cycle, snapshot)) = event {
        send(updates, Message::Snapshot(snapshot))?;
        send(updates, Message::Preparing { epoch, cycle })?;
    }
    Ok(())
}

pub(super) fn end_cycle(
    shared: &Shared,
    updates: &Sender<Message>,
    epoch: u64,
) -> Result<bool, Failure> {
    let ended = {
        let mut state = shared.lock();
        if state.snapshot.epoch != epoch || state.context.is_none() {
            return Err(Failure::Unavailable);
        }
        state.cycle.take().map(|cycle| {
            let delay = state.delay.take();
            if let Some(pending) = state.pending.take() {
                pending.revoke();
            }
            state.snapshot.monitor = MonitorState::Connecting;
            (cycle, state.snapshot, delay)
        })
    };
    let Some((cycle, snapshot, delay)) = ended else {
        return Ok(false);
    };
    drop(delay);
    send(
        updates,
        Message::CycleEnded {
            epoch,
            cycle: Some(cycle),
        },
    )?;
    send(updates, Message::Snapshot(snapshot))?;
    Ok(true)
}

pub(super) fn install_delay(shared: &Shared, epoch: u64, delay: OwnedFd) -> Result<(), Failure> {
    let mut state = shared.lock();
    if shared.stopped.load(Ordering::Acquire)
        || state.snapshot.epoch != epoch
        || state.context.is_none()
        || state.delay.is_some()
    {
        return Err(Failure::Unavailable);
    }
    state.delay = Some(delay);
    Ok(())
}

pub(super) fn ready(shared: &Shared, updates: &Sender<Message>, epoch: u64) -> Result<(), Failure> {
    let snapshot = {
        let mut state = shared.lock();
        if shared.stopped.load(Ordering::Acquire)
            || state.snapshot.epoch != epoch
            || state.context.is_none()
            || state.delay.is_none()
            || state.cycle.is_some()
        {
            return Err(Failure::Unavailable);
        }
        state.snapshot.monitor = MonitorState::Ready;
        state.snapshot
    };
    send(updates, Message::Snapshot(snapshot))
}

pub(super) async fn refresh_epoch(
    bus: &SystemBus,
    context: &Context,
    shared: &Shared,
    updates: &Sender<Message>,
) -> Result<Sequence, Failure> {
    let budget = Budget::new(shared, None, READ_TIMEOUT);
    if bus.owner(&budget).await? != context.owner {
        return Err(Failure::Unavailable);
    }
    if bus.session(&context.owner, &budget).await? != context.session {
        return Err(Failure::SessionUnavailable);
    }
    let mut capability = bus.capability(&context.owner, &budget).await?;
    if bus.inhibited(&context.owner, &budget).await? && capability == Capability::Allowed {
        capability = Capability::Blocked;
    }
    let (preparing, delay_ms, position) = bus.preparing(&context.owner, &budget).await?;
    update_policy(shared, context.epoch, capability, delay_ms)?;
    if preparing {
        begin_cycle(shared, updates, context.epoch)?;
        return Ok(position);
    }
    // A queried false can also settle a cycle whose false signal is queued.
    end_cycle(shared, updates, context.epoch)?;
    let have_delay = shared.lock().delay.is_some();
    let mut final_position = position;
    if !have_delay {
        let delay = match bus.inhibit(&context.owner, &budget).await {
            Ok(delay) => delay,
            Err(Failure::Blocked) => {
                // elogind refuses a new delay inhibitor once sleep has begun.
                // Observe that state instead of announcing unprotected Ready.
                let (preparing, delay_ms, position) =
                    bus.preparing(&context.owner, &budget).await?;
                update_policy(shared, context.epoch, capability, delay_ms)?;
                if preparing {
                    begin_cycle(shared, updates, context.epoch)?;
                    return Ok(position);
                }
                return Err(Failure::Blocked);
            }
            Err(error) => return Err(error),
        };
        let (preparing, delay_ms, position) = bus.preparing(&context.owner, &budget).await?;
        if bus.owner(&budget).await? != context.owner {
            return Err(Failure::Unavailable);
        }
        update_policy(shared, context.epoch, capability, delay_ms)?;
        install_delay(shared, context.epoch, delay)?;
        if preparing {
            begin_cycle(shared, updates, context.epoch)?;
            return Ok(position);
        }
        final_position = position;
    } else if bus.owner(&budget).await? != context.owner {
        return Err(Failure::Unavailable);
    }
    ready(shared, updates, context.epoch)?;
    Ok(final_position)
}

enum MonitorEvent {
    Sleep(Option<zbus::Result<zbus::Message>>),
    Owner(Option<zbus::Result<zbus::Message>>),
    Closed,
    Wake,
}

pub(super) async fn refresh_capability(
    bus: &impl Login1,
    context: &Context,
    shared: &Shared,
    updates: &Sender<Message>,
) -> Result<(), Failure> {
    let budget = Budget::new(shared, None, READ_TIMEOUT);
    if bus.owner(&budget).await? != context.owner {
        return Err(Failure::Unavailable);
    }
    if bus.session(&context.owner, &budget).await? != context.session {
        return Err(Failure::SessionUnavailable);
    }
    let mut capability = bus.capability(&context.owner, &budget).await?;
    if bus.inhibited(&context.owner, &budget).await? && capability == Capability::Allowed {
        capability = Capability::Blocked;
    }
    if bus.owner(&budget).await? != context.owner {
        return Err(Failure::Unavailable);
    }
    let snapshot = {
        let mut state = shared.lock();
        if state.snapshot.epoch != context.epoch || state.context.is_none() {
            return Err(Failure::Unavailable);
        }
        state.snapshot.capability = capability;
        state.snapshot
    };
    // On an established subscription only signals advance sleep state. A
    // property snapshot here could mask a queued true/false pair belonging to
    // our dispatched request and leave the intent pending forever.
    send(updates, Message::Snapshot(snapshot))
}

pub(super) async fn monitor_once(
    shared: &Shared,
    updates: &Sender<Message>,
) -> Result<(), Failure> {
    let budget = Budget::new(shared, None, READ_TIMEOUT);
    let connection = budget
        .dbus(
            zbus::connection::Builder::system()
                .max_queued(NonZeroUsize::new(32).unwrap())
                .build(),
            false,
        )
        .await?;
    let bus = SystemBus {
        connection: connection.clone(),
    };
    // Only signal-specific queues are created. An unfiltered MessageStream
    // would also retain Inhibit method replies and their hidden FD duplicates.
    let owner_rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.freedesktop.DBus")
        .path("/org/freedesktop/DBus")
        .interface("org.freedesktop.DBus")
        .member("NameOwnerChanged")
        .add_arg(SERVICE)
        .build()
        .map_err(|_| Failure::Unavailable)?;
    let mut owner_changes = budget
        .dbus(
            zbus::MessageStream::for_match_rule(
                owner_rule,
                &connection,
                Some(NonZeroUsize::new(16).unwrap()),
            ),
            false,
        )
        .await?;
    let owner = bus.owner(&budget).await?;
    let sleep_rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender(owner.as_str())
        .path(MANAGER_PATH)
        .interface(MANAGER)
        .member("PrepareForSleep")
        .build()
        .map_err(|_| Failure::Unavailable)?;
    let mut sleep_changes = budget
        .dbus(
            zbus::MessageStream::for_match_rule(
                sleep_rule,
                &connection,
                Some(NonZeroUsize::new(16).unwrap()),
            ),
            false,
        )
        .await?;
    bus.methods(&owner, &budget).await?;
    let session = bus.session(&owner, &budget).await?;
    if bus.owner(&budget).await? != owner {
        return Err(Failure::Unavailable);
    }
    let context = begin_epoch(
        shared,
        updates,
        Context {
            epoch: 0,
            owner,
            session,
        },
        connection.clone(),
    )?;
    let mut floor = refresh_epoch(&bus, &context, shared, updates).await?;
    loop {
        if shared.stopped.load(Ordering::Acquire) {
            return Ok(());
        }
        if shared.refreshing.swap(false, Ordering::AcqRel) {
            let request = shared.refresh_request.swap(0, Ordering::AcqRel);
            refresh_capability(&bus, &context, shared, updates).await?;
            if request != 0 {
                let snapshot = shared.lock().snapshot;
                send(updates, Message::Refreshed { request, snapshot })?;
            }
            continue;
        }
        let event = future::or(
            async {
                connection.closed().await;
                MonitorEvent::Closed
            },
            future::or(
                async { MonitorEvent::Owner(owner_changes.next().await) },
                future::or(
                    async { MonitorEvent::Sleep(sleep_changes.next().await) },
                    async {
                        shared.notify.wait().await;
                        MonitorEvent::Wake
                    },
                ),
            ),
        )
        .await;
        match event {
            MonitorEvent::Wake => {}
            MonitorEvent::Closed | MonitorEvent::Owner(None) | MonitorEvent::Sleep(None) => {
                return Err(Failure::Unavailable);
            }
            MonitorEvent::Owner(Some(message)) => {
                let message = message.map_err(|_| Failure::Unavailable)?;
                let (name, old, new): (String, String, String) = decode(message)?;
                if name != SERVICE || new != context.owner || old == context.owner {
                    return Err(Failure::Unavailable);
                }
            }
            MonitorEvent::Sleep(Some(message)) => {
                let message = message.map_err(|_| Failure::Unavailable)?;
                if message.header().sender().map(|sender| sender.as_str())
                    != Some(context.owner.as_str())
                {
                    return Err(Failure::Unavailable);
                }
                let position = message.recv_position();
                let preparing: bool = decode(message)?;
                // A fresh PreparingForSleep property reply supersedes queued
                // older signals from this SAME connection. Serial numbers are
                // intentionally not used as ordering evidence.
                if position <= floor {
                    continue;
                }
                floor = position;
                if preparing {
                    begin_cycle(shared, updates, context.epoch)?;
                } else if end_cycle(shared, updates, context.epoch)? {
                    floor = refresh_epoch(&bus, &context, shared, updates).await?;
                }
            }
        }
        future::yield_now().await;
    }
}

pub(super) async fn monitor_forever(shared: Arc<Shared>, updates: Sender<Message>) {
    let runtime = match zruntime::SharedRuntime::current() {
        Ok(runtime) => runtime,
        Err(_) => {
            lose_epoch(&shared, &updates);
            shared.stop();
            return;
        }
    };
    let mut retry = Duration::from_secs(1);
    while !shared.stopped.load(Ordering::Acquire) {
        let result = monitor_once(&shared, &updates).await;
        if shared.stopped.load(Ordering::Acquire) {
            break;
        }
        lose_epoch(&shared, &updates);
        if result == Err(Failure::Cancelled) {
            shared.stop();
            break;
        }
        // The only recurring timer is failure recovery. A healthy monitor
        // waits exclusively for signals, explicit refresh, and shutdown.
        future::or(
            async {
                runtime.sleep(retry).await;
            },
            shared.notify.wait(),
        )
        .await;
        if finish_recovery_refresh(&shared, &updates) {
            retry = Duration::from_secs(1);
        } else {
            retry = (retry * 2).min(Duration::from_secs(30));
        }
    }
}

/// A refresh accepted during recovery cannot wait for a healthy-monitor loop
/// whose wake flag this backoff just consumed. Settle its exact token as
/// unavailable and free the bounded slot, without turning reconnection into an
/// automatic retry of a battery action. A later explicit episode may try once.
pub(super) fn finish_recovery_refresh(shared: &Shared, updates: &Sender<Message>) -> bool {
    if !shared.refreshing.swap(false, Ordering::AcqRel) {
        return false;
    }
    let request = shared.refresh_request.swap(0, Ordering::AcqRel);
    if request != 0 {
        let snapshot = Snapshot {
            monitor: MonitorState::Unavailable,
            capability: Capability::Unavailable,
            ..shared.lock().snapshot
        };
        let _ = send(updates, Message::Refreshed { request, snapshot });
    }
    true
}
