//! Public presentation-only signal: the polkit agent owns this session-bus
//! name while a prompt is open. This never carries credentials or authorization
//! results. A spoofed owner can only suppress banners, not grant privileges.
use crate::notifications::Message;
use futures_lite::StreamExt;
use smithay_client_toolkit::reexports::calloop::channel::Sender;
const PRESENCE: &str = "org.alcard.Authentication";

pub async fn watch(connection: zbus::Connection, updates: Sender<Message>) -> zbus::Result<()> {
    let proxy = zbus::Proxy::new(
        &connection,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
    )
    .await?;
    // Subscribe first so an authentication starting during initial discovery
    // cannot be missed. The bus releases the name automatically after a crash.
    let mut changes = proxy.receive_signal("NameOwnerChanged").await?;
    let active: bool = proxy.call("NameHasOwner", &(PRESENCE,)).await?;
    let _ = updates.send(Message::AuthenticationActive(active));
    while let Some(message) = changes.next().await {
        let (name, _old, new): (String, String, String) = message.body().deserialize()?;
        if name == PRESENCE {
            let _ = updates.send(Message::AuthenticationActive(!new.is_empty()));
        }
    }
    // Losing the monitor must not allow a banner to cover a credential form.
    let _ = updates.send(Message::AuthenticationActive(true));
    Ok(())
}
