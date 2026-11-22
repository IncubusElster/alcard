//! Shared settings page/action model and frontend-independent operations.
mod inventory;
mod operation;
mod services;

pub use inventory::rows;
pub use operation::reload_shell;

pub const BT: &str = "/usr/bin/bluetoothctl";
const WP: &str = "/usr/bin/wpctl";
const SINK: &str = "@DEFAULT_AUDIO_SINK@";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Page {
    Home,
    System,
    Language,
    Layouts,
    Clock,
    Network,
    Wifi(String),
    Device(String),
    Profiles,
    Profile(String),
    Bluetooth,
    BtDevice(String),
    Power,
    Sound,
    Appearance,
}
impl Page {
    pub fn title(&self) -> &'static str {
        match self {
            Self::Home => "app.settings",
            Self::System => "s.system",
            Self::Language => "s.language",
            Self::Layouts => "s.layouts",
            Self::Clock => "step.time",
            Self::Network => "step.network",
            Self::Wifi(_) => "s.nearby",
            Self::Device(_) => "s.interface",
            Self::Profiles => "s.profiles",
            Self::Profile(_) => "s.profile",
            Self::Bluetooth => "s.bluetooth",
            Self::BtDevice(_) => "s.device",
            Self::Power => "s.power",
            Self::Sound => "s.sound",
            Self::Appearance => "s.appearance",
        }
    }
}
#[derive(Clone, Debug)]
pub enum Operation {
    System(&'static str),
    Preference(Option<&'static str>, &'static str),
    Radio,
    Wifi { iface: String, ssid: String },
    Disconnect(String),
    ActivateDevice(String),
    Profile { uuid: String, field: &'static str },
    Bluetooth(&'static str, String),
    Volume,
    Mute,
    Output,
}
#[derive(Clone, Debug)]
pub enum Action {
    Read,
    Open(Page),
    Edit(Operation),
    Password(Operation),
    Select(Operation, Vec<String>),
    Apply(Operation, String),
    Confirm(Operation, String),
}
#[derive(Clone, Debug)]
pub struct Row {
    pub key: &'static str,
    pub value: String,
    pub action: Action,
}
