//! Ratatui Settings and first-time Alpine setup.
//! Privileged provisioning is confined to a separate root-only subprocess.
mod firstboot;
mod network;
mod settings;
mod ui;

use alcard_input::{interpret, Intent, Key};
use alcard_l10n::{tr, Locale};
use alcard_setup_core::Wizard;
use alcard_setup_protocol::{session_runtime, Session};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::io::{self, IsTerminal};

use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver},
    Arc,
};
enum NetworkUpdate {
    Inventory(Option<(String, bool)>, Option<String>, Option<Vec<String>>),
    Connected(bool),
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum NetworkJobKind {
    Inventory,
    Connection,
}
struct NetworkJob {
    updates: Receiver<NetworkUpdate>,
    kind: NetworkJobKind,
}
pub(crate) struct App {
    pub wizard: Wizard,
    pub should_exit: bool,
    network_job: Option<NetworkJob>,
    cancel: Arc<AtomicBool>,
}

impl App {
    fn new_firstboot() -> Self {
        let mut wizard = Wizard::new();
        wizard.set_session_availability(
            session_runtime::check(Session::Graphics).is_ok(),
            session_runtime::check(Session::GraphicsMinimal).is_ok(),
        );
        wizard.enable_firstboot();
        Self {
            wizard,
            should_exit: false,
            network_job: None,
            cancel: Arc::default(),
        }
    }

    fn handle(&mut self, intent: Intent) {
        if let Some(job) = &self.network_job {
            // Discovery only reads interface state; do not disable navigation
            // while it runs. Discard its obsolete reply on leaving Network.
            // Connection attempts retain their cancellation handshake.
            let leave_inventory = job.kind == NetworkJobKind::Inventory
                && self.wizard.step == alcard_setup_core::Step::Network
                && !self.wizard.wifi_menu_open()
                && matches!(
                    intent,
                    Intent::Soft(0) | Intent::Soft(2) | Intent::Soft(4) | Intent::Quit
                );
            if leave_inventory {
                self.cancel.store(true, Ordering::Release);
                self.network_job = None;
                self.wizard.handle(intent);
                if self.wizard.should_exit {
                    self.should_exit = true;
                }
            } else if matches!(intent, Intent::Soft(4) | Intent::Quit) {
                self.cancel.store(true, Ordering::Release);
            }
            return;
        }
        if intent == Intent::Quit {
            self.should_exit = true;
            return;
        }
        if !self.wizard.is_editing()
            && self.wizard.step == alcard_setup_core::Step::Finish
            && intent == Intent::Soft(0)
        {
            let result = firstboot::apply(&self.wizard);
            self.wizard.set_apply_result(result.is_ok());
            if result.is_ok() {
                self.should_exit = true;
            }
            return;
        }
        let was_network = self.wizard.step == alcard_setup_core::Step::Network;
        let wifi_menu_before = self.wizard.wifi_menu_open();
        let scan_requested = was_network
            && ((self.wizard.focus == 1 && !wifi_menu_before && intent == Intent::Soft(1))
                || (wifi_menu_before && intent == Intent::Soft(2)));
        self.wizard.handle(intent);
        if let Some((iface, ssid, password)) = self.wizard.take_wifi_request() {
            self.start_network(NetworkJobKind::Connection, move |cancel| {
                NetworkUpdate::Connected(
                    network::connect(&iface, &ssid, &password, &cancel).is_ok(),
                )
            });
        } else if self.wizard.step == alcard_setup_core::Step::Network
            && (!was_network || scan_requested)
        {
            self.start_network(NetworkJobKind::Inventory, move |_| {
                let (wired, wifi) = network::detect();
                let networks = if scan_requested {
                    wifi.as_deref()
                        .map(|i| network::scan(i).unwrap_or_default())
                } else {
                    None
                };
                NetworkUpdate::Inventory(wired, wifi, networks)
            });
        }
        if self.wizard.should_exit {
            self.should_exit = true;
        }
    }
    fn start_network(
        &mut self,
        kind: NetworkJobKind,
        task: impl FnOnce(Arc<AtomicBool>) -> NetworkUpdate + Send + 'static,
    ) {
        let (tx, rx) = mpsc::channel();
        self.network_job = Some(NetworkJob { updates: rx, kind });
        self.cancel = Arc::default();
        let cancel = self.cancel.clone();
        std::thread::spawn(move || {
            let _ = tx.send(task(cancel));
        });
    }
    fn poll_network(&mut self) {
        let Some(job) = &self.network_job else { return };
        match job.updates.try_recv() {
            Ok(NetworkUpdate::Inventory(wired, wifi, nets)) => {
                self.wizard.set_network_interfaces(wired, wifi);
                if let Some(n) = nets {
                    self.wizard.set_wifi_networks(n);
                }
            }
            Ok(NetworkUpdate::Connected(ok)) => self.wizard.set_wifi_result(ok),
            Err(mpsc::TryRecvError::Empty) => return,
            Err(_) => self.wizard.set_wifi_result(false),
        }
        self.network_job = None;
    }
}

fn map_key(key: KeyEvent) -> Option<Key> {
    if key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
    {
        return Some(Key::CtrlC);
    }

    Some(match key.code {
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Escape,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::BackTab => Key::BackTab,
        KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => Key::BackTab,
        KeyCode::Tab => Key::Tab,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Char(' ') => Key::Space,
        KeyCode::Char(c) if c.is_ascii_digit() => Key::Digit(c as u8 - b'0'),
        KeyCode::Char(c) => Key::Text(c),
        _ => return None,
    })
}

fn cli_locale() -> Locale {
    Locale::detect()
}

#[derive(Clone, Copy)]
enum LaunchMode {
    Settings,
    Firstboot,
}

fn arguments() -> Result<Option<LaunchMode>, String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [] => Ok(Some(LaunchMode::Settings)),
        [arg] if arg == "--hello" || arg == "--firstboot" => Ok(Some(LaunchMode::Firstboot)),
        [arg] if arg == "-h" || arg == "--help" => {
            println!("{}", tr(cli_locale(), "cli.usage"));
            Ok(None)
        }
        _ => Err(tr(cli_locale(), "cli.usage").to_owned()),
    }
}

fn run(terminal: &mut ratatui::DefaultTerminal, mode: LaunchMode) -> io::Result<()> {
    let mut app = match mode {
        LaunchMode::Firstboot => App::new_firstboot(),
        LaunchMode::Settings => return settings::run(terminal),
    };
    loop {
        app.poll_network();
        terminal.draw(|frame| ui::render(frame, &app))?;
        if app.network_job.is_some() && !event::poll(std::time::Duration::from_millis(100))? {
            continue;
        }
        match event::read()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if let Some(key) = map_key(key) {
                    let intent = interpret(key, app.wizard.is_editing());
                    app.handle(intent);
                }
            }
            Event::Resize(_, _) => {} // Next render uses the new size.
            _ => {}
        }
        if app.should_exit {
            break Ok(());
        }
    }
}

fn main() -> io::Result<()> {
    let mode = match arguments() {
        Ok(Some(mode)) => mode,
        Ok(None) => return Ok(()),
        Err(problem) => {
            eprintln!("{problem}");
            std::process::exit(2);
        }
    };
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::other(tr(cli_locale(), "cli.needs_terminal")));
    }
    if matches!(mode, LaunchMode::Firstboot) {
        let status = std::process::Command::new("/usr/sbin/alcard-setupd")
            .arg("--arm-explicit-root")
            .status()?;
        if !status.success() {
            return Err(io::Error::other(tr(cli_locale(), "cli.hello_refused")));
        }
    }
    ratatui::run(|terminal| run(terminal, mode))
}

#[cfg(test)]
#[path = "unit_tests/main.rs"]
mod tests;
