//! Headless capture fixture: park the compositor cursor away from visible text.
//! Requires explicit opt-in and an explicit private Wayland socket; never uses
//! the host session defaults or sends keyboard, button or activation requests.
use std::{
    error::Error,
    ffi::OsStr,
    io,
    os::unix::net::UnixStream,
    path::{Component, Path, PathBuf},
};
use wayland_client::{
    delegate_noop,
    globals::{registry_queue_init, GlobalListContents},
    protocol::wl_registry,
    Connection, Dispatch, QueueHandle,
};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1,
    zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
};

struct Fixture;

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Fixture {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
delegate_noop!(Fixture: ignore ZwlrVirtualPointerManagerV1);
delegate_noop!(Fixture: ignore ZwlrVirtualPointerV1);

fn run() -> Result<(), Box<dyn Error>> {
    if std::env::var_os("ALCARD_POINTER_FIXTURE").as_deref() != Some(OsStr::new("1"))
        || std::env::args_os().len() != 1
    {
        return Err(io::Error::other("requires ALCARD_POINTER_FIXTURE=1 and no arguments").into());
    }
    let runtime = PathBuf::from(
        std::env::var_os("XDG_RUNTIME_DIR")
            .ok_or_else(|| io::Error::other("requires explicit XDG_RUNTIME_DIR"))?,
    );
    let display = std::env::var_os("WAYLAND_DISPLAY")
        .ok_or_else(|| io::Error::other("requires explicit WAYLAND_DISPLAY"))?;
    let display_path = Path::new(&display);
    if !runtime.is_absolute()
        || display_path.components().count() != 1
        || !matches!(display_path.components().next(), Some(Component::Normal(_)))
    {
        return Err(io::Error::other("requires absolute runtime and a socket basename").into());
    }

    // Explicit connection prevents inherited WAYLAND_SOCKET or a missing
    // display variable from selecting an unrelated desktop session.
    let connection = Connection::from_socket(UnixStream::connect(runtime.join(display_path))?)?;
    let (globals, mut queue) = registry_queue_init::<Fixture>(&connection)?;
    let qh = queue.handle();
    let manager: ZwlrVirtualPointerManagerV1 = globals.bind(&qh, 1..=1, ())?;
    let pointer = manager.create_virtual_pointer(None, &qh, ());
    // The lower edge contains softkeys, and labwc's resize cursor extends
    // inward from its hotspot. Keep it at the far upper status edge instead.
    pointer.motion_absolute(0, 65535, 0, 65536, 65536);
    pointer.frame();
    queue.roundtrip(&mut Fixture)?;
    pointer.destroy();
    manager.destroy();
    queue.roundtrip(&mut Fixture)?;
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("Headless cursor fixture failed: {error}");
        std::process::exit(1);
    }
}
