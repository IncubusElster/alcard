//! A per-operation BlueZ agent. Only the owner of org.bluez and the selected
//! device can request input. Dropping the UI session cancels an outstanding pair.
use async_channel::{Receiver, Sender};
use std::{
    collections::HashMap,
    io,
    sync::{mpsc, Arc, Mutex},
    thread,
    time::Duration,
};
use zbus::{fdo, interface, message::Header, OwnedObjectPath, OwnedVariant};
const AGENT: &str = "/org/alcard/Settings/Agent";
#[cfg(test)]
#[path = "integration/pairing.rs"]
mod integration;
#[cfg(test)]
#[path = "unit_tests/pairing.rs"]
mod tests;
pub enum Prompt {
    Confirm(String),
    Pin,
    Passkey,
    Display(String),
}
pub type Reply = Sender<String>;
pub enum Event {
    Prompt(Prompt, Option<Reply>),
    PromptClosed,
    Done(io::Result<()>),
}
pub struct Session {
    pub events: mpsc::Receiver<Event>,
    cancel: Sender<()>,
}
impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.cancel.try_send(());
    }
}
impl Session {
    pub fn start(address: String) -> Self {
        let (events, rx) = mpsc::channel();
        let (cancel, stop) = async_channel::bounded(1);
        thread::spawn(move || {
            let result =
                zbus::block_on(pair(&address, events.clone(), stop)).map_err(io::Error::other);
            let _ = events.send(Event::Done(result));
        });
        Self { events: rx, cancel }
    }
}
struct Agent {
    device: OwnedObjectPath,
    owner: String,
    events: mpsc::Sender<Event>,
    pending: Arc<Mutex<Option<Receiver<String>>>>,
    released: Sender<()>,
}
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.bluez.Error")]
enum AgentError {
    Rejected(String),
    Canceled(String),
    #[zbus(error)]
    Other(zbus::Error),
}
impl From<fdo::Error> for AgentError {
    fn from(error: fdo::Error) -> Self {
        Self::Other(error.into())
    }
}
impl Agent {
    fn verify_owner(&self, h: &Header<'_>) -> fdo::Result<()> {
        if h.sender().map(|n| n.as_str()) != Some(self.owner.as_str()) {
            return Err(fdo::Error::AccessDenied(
                "Unexpected pairing request".into(),
            ));
        }
        Ok(())
    }
    fn verify(&self, h: &Header<'_>, dev: &OwnedObjectPath) -> Result<(), AgentError> {
        self.verify_owner(h)?;
        if &self.device != dev {
            return Err(fdo::Error::AccessDenied("Unexpected pairing device".into()).into());
        }
        if self.released.is_closed() {
            return Err(AgentError::Canceled("Pairing agent released".into()));
        }
        Ok(())
    }
    fn close_prompt(&self, release_agent: bool) {
        let mut pending = self.pending.lock().unwrap();
        if let Some(reply) = pending.take() {
            reply.close();
        }
        let _ = self.events.send(Event::PromptClosed);
        if release_agent {
            // Publish prompt closure before waking the pairing worker. Keep
            // the same lock so ask/show cannot insert another prompt between
            // closure and release; they recheck this flag under pending.
            self.released.close();
        }
    }
    async fn ask(
        &self,
        h: Header<'_>,
        dev: OwnedObjectPath,
        prompt: Prompt,
    ) -> Result<String, AgentError> {
        self.verify(&h, &dev)?;
        let (tx, rx) = async_channel::bounded(1);
        {
            let mut pending = self.pending.lock().unwrap();
            if self.released.is_closed() {
                return Err(AgentError::Canceled("Pairing agent released".into()));
            }
            if pending.is_some() {
                return Err(AgentError::Rejected(
                    "Pairing prompt already pending".into(),
                ));
            }
            *pending = Some(rx.clone());
            if self.events.send(Event::Prompt(prompt, Some(tx))).is_err() {
                pending.take();
                return Err(AgentError::Canceled("Pairing closed".into()));
            }
        }
        let reply = rx.recv().await;
        let mut pending = self.pending.lock().unwrap();
        if !pending.as_ref().is_some_and(|p| p.same_channel(&rx)) {
            return Err(AgentError::Canceled("Pairing cancelled".into()));
        }
        pending.take();
        reply.map_err(|_| AgentError::Rejected("Pairing rejected".into()))
    }
    fn show(&self, h: Header<'_>, dev: OwnedObjectPath, text: String) -> Result<(), AgentError> {
        self.verify(&h, &dev)?;
        let pending = self.pending.lock().unwrap();
        if self.released.is_closed() {
            return Err(AgentError::Canceled("Pairing agent released".into()));
        }
        if pending.is_some() {
            return Err(AgentError::Rejected("Pairing prompt unavailable".into()));
        }
        self.events
            .send(Event::Prompt(Prompt::Display(text), None))
            .map_err(|_| AgentError::Canceled("Pairing closed".into()))
    }
}
#[interface(name = "org.bluez.Agent1")]
impl Agent {
    fn release(&self, #[zbus(header)] h: Header<'_>) -> fdo::Result<()> {
        self.verify_owner(&h)?;
        self.close_prompt(true);
        Ok(())
    }
    fn cancel(&self, #[zbus(header)] h: Header<'_>) -> fdo::Result<()> {
        self.verify_owner(&h)?;
        self.close_prompt(false);
        Ok(())
    }
    async fn request_pin_code(
        &self,
        device: OwnedObjectPath,
        #[zbus(header)] h: Header<'_>,
    ) -> Result<String, AgentError> {
        let pin = self.ask(h, device, Prompt::Pin).await?;
        if pin.is_empty() || pin.len() > 16 || pin.chars().any(char::is_control) {
            return Err(AgentError::Rejected("Invalid PIN".into()));
        }
        Ok(pin)
    }
    async fn request_passkey(
        &self,
        device: OwnedObjectPath,
        #[zbus(header)] h: Header<'_>,
    ) -> Result<u32, AgentError> {
        self.ask(h, device, Prompt::Passkey)
            .await?
            .parse::<u32>()
            .ok()
            .filter(|n| *n <= 999999)
            .ok_or_else(|| AgentError::Rejected("Invalid passkey".into()))
    }
    async fn request_confirmation(
        &self,
        device: OwnedObjectPath,
        passkey: u32,
        #[zbus(header)] h: Header<'_>,
    ) -> Result<(), AgentError> {
        self.ask(h, device, Prompt::Confirm(format!("{passkey:06}")))
            .await
            .map(|_| ())
    }
    async fn request_authorization(
        &self,
        device: OwnedObjectPath,
        #[zbus(header)] h: Header<'_>,
    ) -> Result<(), AgentError> {
        self.ask(h, device, Prompt::Confirm(String::new()))
            .await
            .map(|_| ())
    }
    async fn authorize_service(
        &self,
        device: OwnedObjectPath,
        uuid: String,
        #[zbus(header)] h: Header<'_>,
    ) -> Result<(), AgentError> {
        self.ask(h, device, Prompt::Confirm(uuid)).await.map(|_| ())
    }
    fn display_pin_code(
        &self,
        device: OwnedObjectPath,
        pincode: String,
        #[zbus(header)] h: Header<'_>,
    ) -> Result<(), AgentError> {
        self.show(h, device, pincode)
    }
    fn display_passkey(
        &self,
        device: OwnedObjectPath,
        passkey: u32,
        _entered: u16,
        #[zbus(header)] h: Header<'_>,
    ) -> Result<(), AgentError> {
        self.show(h, device, format!("{passkey:06}"))
    }
}
fn close_pending(pending: &Mutex<Option<Receiver<String>>>) {
    if let Some(reply) = pending.lock().unwrap().take() {
        reply.close();
    }
}
async fn pair(address: &str, events: mpsc::Sender<Event>, stop: Receiver<()>) -> zbus::Result<()> {
    let conn = zbus::connection::Builder::address("unix:path=/run/dbus/system_bus_socket")
        .method_timeout(Duration::from_secs(90))
        .build()
        .await?;
    pair_on_connection(conn, address, events, stop).await
}
async fn pair_on_connection(
    conn: zbus::Connection,
    address: &str,
    events: mpsc::Sender<Event>,
    stop: Receiver<()>,
) -> zbus::Result<()> {
    let owner: String = conn
        .call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "GetNameOwner",
            &("org.bluez",),
        )
        .await?
        .body()
        .deserialize()?;
    type Objects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedVariant>>>;
    let objects: Objects = conn
        .call_method(
            Some(owner.as_str()),
            "/",
            Some("org.freedesktop.DBus.ObjectManager"),
            "GetManagedObjects",
            &(),
        )
        .await?
        .body()
        .deserialize()?;
    let mut devices = objects.into_iter().filter_map(|(path, interfaces)| {
        let p = interfaces.get("org.bluez.Device1")?.get("Address")?;
        (<&str>::try_from(p).ok()?.eq_ignore_ascii_case(address)).then_some(path)
    });
    let device = devices
        .next()
        .ok_or_else(|| zbus::Error::Failure("Bluetooth device disappeared".into()))?;
    if devices.next().is_some() {
        return Err(zbus::Error::Failure(
            "Device belongs to multiple adapters".into(),
        ));
    }
    let path = zbus::ObjectPath::try_from(AGENT)?;
    let manager = zbus::Proxy::new(
        &conn,
        owner.as_str(),
        "/org/bluez",
        "org.bluez.AgentManager1",
    )
    .await?;
    let proxy =
        zbus::Proxy::new(&conn, owner.as_str(), device.as_str(), "org.bluez.Device1").await?;
    let pending = Arc::new(Mutex::new(None));
    let (release, released) = async_channel::bounded(1);
    let agent = Agent {
        device: device.clone(),
        owner: owner.clone(),
        events,
        pending: pending.clone(),
        released: release,
    };
    conn.object_server().at(AGENT, agent).await?;
    let registration: zbus::Result<()> = manager
        .call("RegisterAgent", &(&path, "KeyboardDisplay"))
        .await;
    if let Err(error) = registration {
        close_pending(&pending);
        let _ = conn.object_server().remove::<Agent, _>(AGENT).await;
        return Err(error);
    }
    // Registration can finish after the user has already closed Settings.
    // Honor the queued stop before the Pair future can send a new request.
    if stop.try_recv().is_ok() || stop.is_closed() || released.is_closed() {
        close_pending(&pending);
        if !released.is_closed() {
            let _: zbus::Result<()> = manager.call("UnregisterAgent", &(&path,)).await;
        }
        let _ = conn.object_server().remove::<Agent, _>(AGENT).await;
        return Err(zbus::Error::Failure("Pairing cancelled".into()));
    }
    enum Completion {
        Pair(zbus::Result<()>),
        Cancel,
        Release,
    }
    let result = futures_lite::future::or(
        async { Completion::Pair(proxy.call::<_, _, ()>("Pair", &()).await) },
        futures_lite::future::or(
            async {
                let _ = stop.recv().await;
                Completion::Cancel
            },
            async {
                let _ = released.recv().await;
                Completion::Release
            },
        ),
    )
    .await;
    close_pending(&pending);
    if matches!(result, Completion::Cancel) {
        let _: zbus::Result<()> = proxy.call("CancelPairing", &()).await;
    }
    if !released.is_closed() {
        let _: zbus::Result<()> = manager.call("UnregisterAgent", &(&path,)).await;
    }
    let _ = conn.object_server().remove::<Agent, _>(AGENT).await;
    match result {
        Completion::Pair(result) => result,
        Completion::Cancel => Err(zbus::Error::Failure("Pairing cancelled".into())),
        Completion::Release => Err(zbus::Error::Failure("Pairing agent released".into())),
    }
}
