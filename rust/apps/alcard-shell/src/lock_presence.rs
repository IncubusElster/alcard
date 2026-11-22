//! Presentation-only session-lock presence, independent of notification ownership.
//! This observer never acquires a name, accepts a command or reports an unlock.
use crate::locking::Message;
use futures_lite::{future, StreamExt};
use smithay_client_toolkit::reexports::calloop::channel::Sender;
use std::{
    future::Future,
    io,
    num::NonZeroUsize,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

const NAME: &str = "org.alcard.SessionLock";
const INITIAL_TIMEOUT: Duration = Duration::from_secs(8);
const CANCEL_INTERVAL: Duration = Duration::from_millis(100);
const RETRY_INTERVAL: Duration = Duration::from_secs(1);

pub fn start(updates: Sender<Message>, stopped: Arc<AtomicBool>) -> io::Result<()> {
    thread::Builder::new()
        .name("alcard-lock-presence".into())
        .spawn(move || run(updates, stopped))
        .map(|_| ())
}

fn run(updates: Sender<Message>, stopped: Arc<AtomicBool>) {
    let mut last_presence = None;
    let mut last_error = None;
    while !stopped.load(Ordering::Acquire) {
        let established = AtomicBool::new(false);
        let observe = observe(&updates, &established, &mut last_presence);
        let error = match drive(observe, &established, &stopped, INITIAL_TIMEOUT) {
            None => return,
            Some(Ok(())) => "Session-lock presence stream ended".to_string(),
            Some(Err(error)) => error.to_string(),
        };
        if last_error.as_ref() != Some(&error) {
            eprintln!("Session-lock presentation monitor unavailable: {error}");
            last_error = Some(error);
        }
        // This connection no longer observes the public name. Dropping only
        // its presentation hint is not an unlock: the compositor-scoped
        // journal and an owned lifecycle remain independent guards. Keeping a
        // stale name forever would hide an already-authenticated Shell when
        // the session bus remains unavailable after the real locker exits.
        if publish(&updates, &mut last_presence, false).is_err() {
            return;
        }
        let retry_at = Instant::now() + RETRY_INTERVAL;
        while !stopped.load(Ordering::Acquire) {
            let now = Instant::now();
            if now >= retry_at {
                break;
            }
            thread::park_timeout((retry_at - now).min(CANCEL_INTERVAL));
        }
    }
}

/// Polling happens only on our worker. Connection/authentication and initial
/// subscription/query have one deadline; stop remains observable even if a
/// broken bus never answers or wakes the pending future.
fn drive(
    observe: impl Future<Output = zbus::Result<()>>,
    established: &AtomicBool,
    stopped: &AtomicBool,
    initial_timeout: Duration,
) -> Option<zbus::Result<()>> {
    let deadline = Instant::now() + initial_timeout;
    let runtime = match zruntime::SharedRuntime::current() {
        Ok(runtime) => runtime,
        Err(error) => return Some(Err(error.into())),
    };
    runtime.block_on(future::or(
        async {
            loop {
                if stopped.load(Ordering::Acquire) {
                    return None;
                }
                if !established.load(Ordering::Acquire) && Instant::now() >= deadline {
                    return Some(Err(zbus::Error::Failure(
                        "Initial session-lock presence query timed out".into(),
                    )));
                }
                runtime.sleep(CANCEL_INTERVAL).await;
            }
        },
        async { Some(observe.await) },
    ))
}

async fn observe(
    updates: &Sender<Message>,
    established: &AtomicBool,
    last_presence: &mut Option<bool>,
) -> zbus::Result<()> {
    let connection = zbus::connection::Builder::session()
        .max_queued(NonZeroUsize::new(16).unwrap())
        .build()
        .await?;
    let proxy = zbus::Proxy::new(
        &connection,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
    )
    .await?;
    // Subscribe before discovery; filter at the bus instead of receiving every
    // application name change in the session. zbus uses the runtime driven by
    // drive(); dropping this future releases the connection and its observers.
    let mut changes = proxy
        .receive_signal_with_args("NameOwnerChanged", &[(0, NAME)])
        .await?;
    let active: bool = proxy.call("NameHasOwner", &(NAME,)).await?;
    publish(updates, last_presence, active)?;
    established.store(true, Ordering::Release);
    while let Some(message) = changes.next().await {
        let (name, _old, new): (String, String, String) = message.body().deserialize()?;
        if name == NAME {
            publish(updates, last_presence, !new.is_empty())?;
        }
        // Keep cancellation observable even if this name is repeatedly
        // acquired/released while its queue stays ready.
        future::yield_now().await;
    }
    Ok(())
}

fn publish(updates: &Sender<Message>, last: &mut Option<bool>, active: bool) -> zbus::Result<()> {
    if *last != Some(active) {
        updates
            .send(Message::Presence(active))
            .map_err(|_| zbus::Error::Failure("Shell event loop is closed".into()))?;
        *last = Some(active);
    }
    Ok(())
}

#[cfg(test)]
#[path = "unit_tests/lock_presence.rs"]
mod tests;
