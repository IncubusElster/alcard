//! Private compositor acceptance fixture, never installed in a runtime APK.
//! Hold one output-power lease until stdin says "release", then observe real
//! wl_output removal/addition. No set_mode, keyboard, activation or lock request.
use std::{
    collections::BTreeMap,
    error::Error,
    ffi::OsStr,
    io::{self, BufRead, Write},
    os::unix::{fs::MetadataExt, net::UnixStream},
    path::{Component, Path, PathBuf},
};

use serde_json::json;
use wayland_client::{
    delegate_noop,
    globals::{registry_queue_init, GlobalListContents},
    protocol::{wl_output, wl_registry},
    Connection, Dispatch, Proxy, QueueHandle, WEnum,
};
use wayland_protocols_wlr::output_power_management::v1::client::{
    zwlr_output_power_manager_v1::ZwlrOutputPowerManagerV1 as Manager,
    zwlr_output_power_v1::{self as power, ZwlrOutputPowerV1 as Power},
};

struct Output {
    proxy: wl_output::WlOutput,
    name: Option<String>,
    control: Option<Power>,
    on: bool,
}

struct Fixture {
    manager: Manager,
    outputs: BTreeMap<u32, Output>,
    target: String,
    hold: bool,
    failed: bool,
}

fn emit(value: serde_json::Value) {
    let mut stdout = io::stdout().lock();
    writeln!(stdout, "{value}").expect("fixture receipt pipe");
    stdout.flush().expect("flush fixture receipt");
}

impl Fixture {
    fn bind_output(
        &mut self,
        registry: &wl_registry::WlRegistry,
        name: u32,
        version: u32,
        qh: &QueueHandle<Self>,
    ) {
        if version < 4 {
            self.failed = true;
            emit(json!({"event": "unsupported_output", "global": name, "version": version}));
            return;
        }
        let proxy = registry.bind::<wl_output::WlOutput, _, _>(name, 4, qh, name);
        emit(json!({"event": "global_add", "global": name, "object": proxy.id().protocol_id()}));
        self.outputs.insert(
            name,
            Output {
                proxy,
                name: None,
                control: None,
                on: false,
            },
        );
    }

    fn release(&mut self) {
        self.hold = false;
        for (global, output) in &mut self.outputs {
            if let Some(control) = output.control.take() {
                emit(json!({"event": "release_control", "global": global,
                            "object": control.id().protocol_id()}));
                control.destroy();
            }
        }
    }

    fn snapshot(&self) -> serde_json::Value {
        self.outputs
            .iter()
            .map(|(global, output)| {
                json!({"global": global, "object": output.proxy.id().protocol_id(),
                       "name": output.name})
            })
            .collect::<Vec<_>>()
            .into()
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Fixture {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } if interface == "wl_output" => state.bind_output(registry, name, version, qh),
            wl_registry::Event::GlobalRemove { name } => {
                if let Some(output) = state.outputs.remove(&name) {
                    emit(json!({"event": "global_remove", "global": name, "name": output.name}));
                    if let Some(control) = output.control {
                        control.destroy();
                    }
                    output.proxy.release();
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_output::WlOutput, u32> for Fixture {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        global: &u32,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = event {
            let Some(output) = state.outputs.get_mut(global) else {
                return;
            };
            emit(json!({"event": "output_name", "global": global, "name": name}));
            if !name.starts_with("HEADLESS-") {
                state.failed = true;
                return;
            }
            if state.hold && name == state.target {
                let control = state.manager.get_output_power(&output.proxy, qh, *global);
                emit(json!({"event": "hold_control", "global": global,
                            "object": control.id().protocol_id()}));
                output.control = Some(control);
            }
            output.name = Some(name);
        }
    }
}

impl Dispatch<Power, u32> for Fixture {
    fn event(
        state: &mut Self,
        proxy: &Power,
        event: power::Event,
        global: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            power::Event::Mode {
                mode: WEnum::Value(mode),
            } => {
                emit(json!({"event": "power_mode", "global": global,
                            "object": proxy.id().protocol_id(), "mode": mode as u32}));
                if let Some(output) = state.outputs.get_mut(global) {
                    output.on = mode == power::Mode::On;
                }
            }
            power::Event::Failed => {
                emit(json!({"event": "power_failed", "global": global,
                            "object": proxy.id().protocol_id()}));
                state.failed = true;
            }
            _ => {}
        }
    }
}
delegate_noop!(Fixture: ignore Manager);

fn run() -> Result<(), Box<dyn Error>> {
    let arguments: Vec<_> = std::env::args().collect();
    if std::env::var_os("ALCARD_OUTPUT_POWER_FIXTURE").as_deref() != Some(OsStr::new("1"))
        || arguments.len() != 2
        || !arguments[1].starts_with("HEADLESS-")
        || !arguments[1]
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        || nix::unistd::getuid().is_root()
    {
        return Err(io::Error::other(
            "requires ordinary UID, explicit fixture opt-in and HEADLESS output name",
        )
        .into());
    }
    let runtime = PathBuf::from(
        std::env::var_os("XDG_RUNTIME_DIR")
            .ok_or_else(|| io::Error::other("requires explicit XDG_RUNTIME_DIR"))?,
    );
    let display = std::env::var_os("WAYLAND_DISPLAY")
        .ok_or_else(|| io::Error::other("requires explicit WAYLAND_DISPLAY"))?;
    let display = Path::new(&display);
    let metadata = runtime.symlink_metadata()?;
    if !runtime.is_absolute()
        || !metadata.is_dir()
        || metadata.uid() != nix::unistd::getuid().as_raw()
        || metadata.mode() & 0o777 != 0o700
        || display.components().count() != 1
        || !matches!(display.components().next(), Some(Component::Normal(_)))
    {
        return Err(io::Error::other(
            "requires owned private runtime directory and socket basename",
        )
        .into());
    }
    let connection = Connection::from_socket(UnixStream::connect(runtime.join(display))?)?;
    let (globals, mut queue) = registry_queue_init::<Fixture>(&connection)?;
    let qh = queue.handle();
    let manager = globals.bind::<Manager, _, _>(&qh, 1..=1, ())?;
    let mut state = Fixture {
        manager,
        outputs: BTreeMap::new(),
        target: arguments[1].clone(),
        hold: true,
        failed: false,
    };
    for global in globals.contents().clone_list() {
        if global.interface == "wl_output" {
            state.bind_output(globals.registry(), global.name, global.version, &qh);
        }
    }
    // The first roundtrip receives output names; their controls' initial mode
    // arrives on the second. Ready means the compositor accepted our lease.
    queue.roundtrip(&mut state)?;
    queue.roundtrip(&mut state)?;
    if state.failed
        || state.outputs.len() != 2
        || !state.outputs.values().any(|output| {
            output.name.as_ref() == Some(&state.target) && output.control.is_some() && output.on
        })
    {
        return Err(
            io::Error::other("expected two headless outputs and one accepted On lease").into(),
        );
    }
    emit(json!({"event": "ready", "target": state.target, "outputs": state.snapshot()}));
    for command in io::stdin().lock().lines().take(8) {
        let command = command?;
        match command.as_str() {
            "release" => state.release(),
            "sync" => {}
            "quit" => {
                state.release();
                queue.roundtrip(&mut state)?;
                return Ok(());
            }
            _ => return Err(io::Error::other("unknown fixture command").into()),
        }
        queue.roundtrip(&mut state)?;
        queue.roundtrip(&mut state)?;
        if state.failed {
            return Err(io::Error::other("fixture control failed").into());
        }
        emit(json!({"event": "synced", "command": command, "outputs": state.snapshot()}));
    }
    state.release();
    queue.roundtrip(&mut state)?;
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("Private output-power fixture failed: {error}");
        std::process::exit(1);
    }
}
