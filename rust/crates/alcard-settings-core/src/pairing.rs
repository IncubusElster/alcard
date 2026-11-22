//! A per-operation BlueZ agent. Only the owner of org.bluez and the selected
//! device can request input. Dropping the UI session cancels an outstanding pair.
use async_channel::{Receiver, Sender};
use std::{collections::HashMap, io, sync::mpsc, thread, time::Duration};
use zbus::{fdo, interface, message::Header, OwnedObjectPath, OwnedVariant};
const AGENT: &str = "/org/alcard/Settings/Agent";
pub enum Prompt {
    Confirm(String),
    Pin,
    Passkey,
    Display(String),
}
pub type Reply = Sender<String>;
pub enum Event {
    Prompt(Prompt, Option<Reply>),
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
}
impl Agent {
    fn verify(&self, h: &Header<'_>, dev: &OwnedObjectPath) -> fdo::Result<()> {
        if &self.device != dev || h.sender().map(|n| n.as_str()) != Some(self.owner.as_str()) {
            return Err(fdo::Error::AccessDenied(
                "Unexpected pairing request".into(),
            ));
        }
        Ok(())
    }
    async fn ask(
        &self,
        h: Header<'_>,
        dev: OwnedObjectPath,
        prompt: Prompt,
    ) -> fdo::Result<String> {
        self.verify(&h, &dev)?;
        let (tx, rx) = async_channel::bounded(1);
        self.events
            .send(Event::Prompt(prompt, Some(tx)))
            .map_err(|_| fdo::Error::Failed("Pairing closed".into()))?;
        rx.recv()
            .await
            .map_err(|_| fdo::Error::Failed("Pairing cancelled".into()))
    }
    fn show(&self, h: Header<'_>, dev: OwnedObjectPath, text: String) -> fdo::Result<()> {
        self.verify(&h, &dev)?;
        self.events
            .send(Event::Prompt(Prompt::Display(text), None))
            .map_err(|_| fdo::Error::Failed("Pairing closed".into()))
    }
}
#[interface(name = "org.bluez.Agent1")]
impl Agent {
    fn release(&self) {}
    fn cancel(&self) {}
    async fn request_pin_code(
        &self,
        device: OwnedObjectPath,
        #[zbus(header)] h: Header<'_>,
    ) -> fdo::Result<String> {
        let pin = self.ask(h, device, Prompt::Pin).await?;
        if pin.is_empty() || pin.len() > 16 || pin.chars().any(char::is_control) {
            return Err(fdo::Error::InvalidArgs("Invalid PIN".into()));
        }
        Ok(pin)
    }
    async fn request_passkey(
        &self,
        device: OwnedObjectPath,
        #[zbus(header)] h: Header<'_>,
    ) -> fdo::Result<u32> {
        self.ask(h, device, Prompt::Passkey)
            .await?
            .parse::<u32>()
            .ok()
            .filter(|n| *n <= 999999)
            .ok_or_else(|| fdo::Error::InvalidArgs("Invalid passkey".into()))
    }
    async fn request_confirmation(
        &self,
        device: OwnedObjectPath,
        passkey: u32,
        #[zbus(header)] h: Header<'_>,
    ) -> fdo::Result<()> {
        self.ask(h, device, Prompt::Confirm(format!("{passkey:06}")))
            .await
            .map(|_| ())
    }
    async fn request_authorization(
        &self,
        device: OwnedObjectPath,
        #[zbus(header)] h: Header<'_>,
    ) -> fdo::Result<()> {
        self.ask(h, device, Prompt::Confirm(String::new()))
            .await
            .map(|_| ())
    }
    async fn authorize_service(
        &self,
        device: OwnedObjectPath,
        uuid: String,
        #[zbus(header)] h: Header<'_>,
    ) -> fdo::Result<()> {
        self.ask(h, device, Prompt::Confirm(uuid)).await.map(|_| ())
    }
    fn display_pin_code(
        &self,
        device: OwnedObjectPath,
        pincode: String,
        #[zbus(header)] h: Header<'_>,
    ) -> fdo::Result<()> {
        self.show(h, device, pincode)
    }
    fn display_passkey(
        &self,
        device: OwnedObjectPath,
        passkey: u32,
        _entered: u16,
        #[zbus(header)] h: Header<'_>,
    ) -> fdo::Result<()> {
        self.show(h, device, format!("{passkey:06}"))
    }
}
async fn pair(address: &str, events: mpsc::Sender<Event>, stop: Receiver<()>) -> zbus::Result<()> {
    let conn = zbus::connection::Builder::address("unix:path=/run/dbus/system_bus_socket")
        .method_timeout(Duration::from_secs(90))
        .build()
        .await?;
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
    let agent = Agent {
        device: device.clone(),
        owner: owner.clone(),
        events,
    };
    conn.object_server().at(AGENT, agent).await?;
    let path = zbus::ObjectPath::try_from(AGENT)?;
    let manager = zbus::Proxy::new(
        &conn,
        owner.as_str(),
        "/org/bluez",
        "org.bluez.AgentManager1",
    )
    .await?;
    let _: () = manager
        .call("RegisterAgent", &(&path, "KeyboardDisplay"))
        .await?;
    let proxy =
        zbus::Proxy::new(&conn, owner.as_str(), device.as_str(), "org.bluez.Device1").await?;
    let result = futures_lite::future::or(
        async { Some(proxy.call::<_, _, ()>("Pair", &()).await) },
        async {
            let _ = stop.recv().await;
            None
        },
    )
    .await;
    if result.is_none() {
        let _: zbus::Result<()> = proxy.call("CancelPairing", &()).await;
    }
    let _: zbus::Result<()> = manager.call("UnregisterAgent", &(&path,)).await;
    result.unwrap_or_else(|| Err(zbus::Error::Failure("Pairing cancelled".into())))
}
