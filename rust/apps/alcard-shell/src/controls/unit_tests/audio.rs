use super::*;
use serde_json::json;
use std::{ffi::OsStr, sync::atomic::AtomicBool, time::Duration};

const INCREASE: Operation = Operation::Adjust {
    control: Control::Volume,
    direction: Direction::Increase,
};
const DECREASE: Operation = Operation::Adjust {
    control: Control::Volume,
    direction: Direction::Decrease,
};

#[derive(Clone)]
enum Event {
    Restart,
    ReplaceNode,
    ReplaceDevice,
    NewDefault,
    NewRoute,
    Channels(Vec<f64>),
}

struct Fake {
    cookie: u32,
    name: String,
    serial: u64,
    device_serial: u64,
    route: Option<u32>,
    channels: Vec<f64>,
    muted: bool,
    writable: bool,
    reads: usize,
    event: Option<(usize, Event)>,
    writes: Vec<(u32, Change)>,
    apply_writes: bool,
    only_first_channel: bool,
    write_error: Option<Failure>,
    missing_after_write: bool,
    empty_after_write: bool,
}

impl Default for Fake {
    fn default() -> Self {
        Self {
            cookie: 2_900_000_001,
            name: "alsa_output.platform-ES8389Audio.stereo-fallback".into(),
            serial: 100,
            device_serial: 22,
            route: None,
            channels: vec![0.125, 0.125],
            muted: false,
            writable: true,
            reads: 0,
            event: None,
            writes: Vec::new(),
            apply_writes: true,
            only_first_channel: false,
            write_error: None,
            missing_after_write: false,
            empty_after_write: false,
        }
    }
}

impl Fake {
    fn event(&mut self, event: Event) {
        match event {
            Event::Restart => self.cookie += 1,
            Event::ReplaceNode => self.serial += 1,
            Event::ReplaceDevice => self.device_serial += 1,
            Event::NewDefault => self.name = "bluez_output.new-default".into(),
            Event::NewRoute => self.route = Some(self.route.unwrap_or(0) + 1),
            Event::Channels(channels) => self.channels = channels,
        }
    }

    fn metadata(&self) -> Value {
        json!({
            "id": 31, "type": METADATA, "permissions": ["r", "w", "x", "m"],
            "props": {"metadata.name": "default", "object.serial": 33},
            "metadata": [
                {"subject": 0, "key": "default.configured.audio.sink",
                 "type": "Spa:String:JSON", "value": {"name": "configured-but-absent"}},
                {"subject": 0, "key": "default.audio.sink",
                 "type": "Spa:String:JSON", "value": {"name": self.name}}
            ]
        })
    }

    fn node(&self) -> Value {
        let mut node = json!({
            "id": 42, "type": NODE,
            "permissions": if self.writable { vec!["r", "w", "x", "m"] } else { vec!["r", "x"] },
            "info": {
                "state": "suspended",
                "props": {
                    "object.serial": self.serial,
                    "node.name": self.name, "media.class": "Audio/Sink"
                },
                "params": {
                    "Props": [{
                        "volume": 1.0, "mute": self.muted,
                        "channelVolumes": self.channels, "channelMap": ["FL", "FR"],
                        "softVolumes": [1.0, 1.0], "softMute": false
                    }]
                }
            }
        });
        if self.route.is_some() {
            node["info"]["props"]["device.id"] = json!("20");
            node["info"]["props"]["card.profile.device"] = json!(0);
            // A plausible software stage is not the mixer-api Route level.
            node["info"]["params"]["Props"][0]["channelVolumes"] = json!([1.0, 1.0]);
        }
        node
    }

    fn device(&self) -> Value {
        json!({
            "id": 20, "type": DEVICE, "permissions": ["r", "w", "x", "m"],
            "info": {
                "props": {"object.serial": self.device_serial, "media.class": "Audio/Device"},
                "params": {"Route": [
                    {"index": 9, "device": 1, "props": {"mute": false, "channelVolumes": [1.5]}},
                    {"index": self.route.unwrap_or(1), "device": 0,
                     "props": {"mute": self.muted, "channelVolumes": self.channels}}
                ]}
            }
        })
    }
}

impl Transport for Fake {
    fn dump(&mut self, _: &Budget<'_>) -> Result<String, Failure> {
        self.reads += 1;
        if self.event.as_ref().is_some_and(|(at, _)| *at == self.reads) {
            let (_, event) = self.event.take().unwrap();
            self.event(event);
        }
        if !self.writes.is_empty() {
            if self.missing_after_write {
                return Err(Failure::Unavailable);
            }
            if self.empty_after_write {
                return Ok(String::new());
            }
        }
        let mut objects = vec![
            json!({"id": 0, "type": CORE, "info": {"cookie": self.cookie}}),
            self.metadata(),
            self.node(),
        ];
        if self.route.is_some() {
            objects.push(self.device());
            // An unrelated object's serial is not a global node ID.
            let mut collision = self.node();
            collision["id"] = json!(55);
            collision["info"]["props"]["object.serial"] = json!(20);
            collision["info"]["props"]["node.name"] = json!("unrelated-output");
            objects.push(collision);
        }
        let value = json!(objects);
        Ok(value.to_string())
    }

    fn change(&mut self, node: u32, change: Change, _: &Budget<'_>) -> Result<(), Failure> {
        self.writes.push((node, change));
        if self.apply_writes {
            match change {
                Change::Volume(level) => {
                    if self.only_first_channel {
                        self.channels[0] = level.powi(3);
                    } else {
                        self.channels.fill(level.powi(3));
                    }
                }
                Change::Mute(muted) => self.muted = muted,
            }
        }
        self.write_error.map_or(Ok(()), Err)
    }
}

fn budget(stopped: &AtomicBool) -> Budget<'_> {
    Budget::new(stopped, Duration::from_secs(5))
}

#[test]
fn native_json_selects_actual_default_and_maximum_channel_with_route_precedence() {
    let stopped = AtomicBool::new(false);
    for route in [None, Some(1)] {
        let mut transport = Fake {
            route,
            channels: vec![0.008, 0.125],
            ..Fake::default()
        };
        let mut binding = None;
        let reading = refresh(&mut transport, &mut binding, &budget(&stopped));
        assert_eq!(reading.percent, Some(50));
        assert_eq!(reading.muted, Some(false));
        assert_eq!(reading.availability, Availability::Available);
        assert_eq!(binding.unwrap().target.route, route);
    }
    // Numeric strings from property dictionaries are accepted losslessly.
    let mut node = Fake::default().node();
    node["info"]["props"]["object.serial"] = json!("18446744073709551615");
    assert_eq!(
        object_key(&node, &node["info"]["props"]).unwrap().serial,
        u64::MAX
    );
}

#[test]
fn no_configured_first_sink_or_ambiguous_metadata_fallback() {
    let fake = Fake::default();
    let mut metadata = fake.metadata();
    metadata["metadata"].as_array_mut().unwrap().remove(1);
    assert_eq!(default_output(&[metadata]), Err(Failure::Unavailable));
    let mut metadata = fake.metadata();
    metadata["metadata"][1]["subject"] = json!(42);
    assert_eq!(default_output(&[metadata]), Err(Failure::Unavailable));
    let mut metadata = fake.metadata();
    metadata["metadata"][1]["value"] = json!("{\"name\":\"wrong-encoded-shape\"}");
    assert_eq!(default_output(&[metadata]), Err(Failure::Invalid));
    let mut duplicate = fake.metadata();
    duplicate["id"] = json!(32);
    assert_eq!(
        default_output(&[fake.metadata(), duplicate]),
        Err(Failure::Invalid)
    );
    let mut ambiguous = fake.metadata();
    let duplicate_entry = ambiguous["metadata"][1].clone();
    ambiguous["metadata"]
        .as_array_mut()
        .unwrap()
        .push(duplicate_entry);
    assert_eq!(default_output(&[ambiguous]), Err(Failure::Invalid));
}

#[test]
fn malformed_oversized_or_incomplete_json_never_becomes_a_control() {
    for text in [
        "",
        "{}",
        "[",
        "[{}]",
        "[{\"id\":1,\"type\":\"x\"},{\"id\":1,\"type\":\"x\"}]",
    ] {
        assert!(matches!(parse_dump(text), Err(Failure::Invalid)));
    }
    assert!(matches!(
        parse_dump(&" ".repeat(MAX_GRAPH_BYTES + 1)),
        Err(Failure::Invalid)
    ));
    let objects: Vec<_> = (0..=MAX_OBJECTS)
        .map(|id| json!({"id":id, "type":NODE}))
        .collect();
    assert!(matches!(
        parse_dump(&json!(objects).to_string()),
        Err(Failure::Invalid)
    ));
    for channels in [
        json!([]),
        json!([-0.1]),
        json!([null]),
        json!(["0.5"]),
        json!(vec![0.5; MAX_CHANNELS + 1]),
    ] {
        assert_eq!(
            parse_levels(&json!({"mute":false,"channelVolumes":channels})),
            Err(Failure::Invalid)
        );
    }
    assert_eq!(parse_levels(&json!({"volume":0.5})), Ok(None));
    assert_eq!(
        parse_levels(&json!({"mute":"false","channelVolumes":[0.125]})),
        Err(Failure::Invalid)
    );
    assert!(!valid_name("name\nanother"));
    assert!(!valid_name(&"x".repeat(513)));
}

#[test]
fn active_route_selection_is_exact_and_duplicate_routes_are_rejected() {
    let fake = Fake {
        route: Some(3),
        ..Fake::default()
    };
    let device = fake.device();
    let (index, levels) = route_levels(&device, 0).unwrap().unwrap();
    assert_eq!(index, 3);
    assert_eq!(levels.channels, [0.125, 0.125]);
    assert_eq!(route_levels(&device, 5), Ok(None));
    let mut duplicate = device.clone();
    let extra = duplicate["info"]["params"]["Route"][1].clone();
    duplicate["info"]["params"]["Route"]
        .as_array_mut()
        .unwrap()
        .push(extra);
    assert_eq!(route_levels(&duplicate, 0), Err(Failure::Invalid));
    let mut missing = device;
    missing["info"]["params"]["Route"][1]["props"] = json!({});
    assert_eq!(route_levels(&missing, 0), Ok(None));
}

#[test]
fn commands_pin_remote_and_output_names_never_become_arguments() {
    let stopped = AtomicBool::new(false);
    let mut transport = Fake {
        name: "-sink*[a]?@(x|y);literal-output".into(),
        ..Fake::default()
    };
    let mut binding = None;
    assert_eq!(
        refresh(&mut transport, &mut binding, &budget(&stopped)).percent,
        Some(50)
    );
    assert_eq!(dump_arguments(), ["-N", "-r", "pipewire-0"]);
    assert_eq!(
        change_arguments(42, Change::Volume(0.55)),
        ["set-volume", "42", "0.550000", "--limit", "1.0"]
    );
    assert_eq!(
        change_arguments(42, Change::Mute(false)),
        ["set-mute", "42", "0"]
    );
    let mut command = command("/usr/bin/wpctl", Path::new("/run/user/1000"));
    command.args(change_arguments(42, Change::Mute(true)));
    assert_eq!(command.get_program(), OsStr::new("/usr/bin/wpctl"));
    let environment: Vec<_> = command.get_envs().collect();
    assert!(environment.contains(&(
        OsStr::new("PIPEWIRE_REMOTE"),
        Some(OsStr::new("pipewire-0"))
    )));
    assert!(environment.contains(&(OsStr::new("PIPEWIRE_RUNTIME_DIR"), None)));
    assert!(environment.contains(&(OsStr::new("LC_ALL"), Some(OsStr::new("C")))));
}

#[test]
fn denied_objects_remain_readable_but_never_receive_a_write() {
    let stopped = AtomicBool::new(false);
    let mut transport = Fake {
        writable: false,
        ..Fake::default()
    };
    let mut binding = None;
    let reading = refresh(&mut transport, &mut binding, &budget(&stopped));
    assert_eq!(reading.availability, Availability::Denied);
    assert_eq!(reading.percent, Some(50));
    let (reading, result) = execute(&mut transport, &mut binding, INCREASE, &budget(&stopped));
    assert_eq!(result, Err(Failure::Denied));
    assert_eq!(reading.percent, Some(50));
    assert!(transport.writes.is_empty());
}

#[test]
fn software_mute_and_unity_limit_cover_all_channels_including_hidden_amplification() {
    let stopped = AtomicBool::new(false);
    let mut transport = Fake {
        channels: vec![0.125, 1.000_001],
        muted: true,
        ..Fake::default()
    };
    let mut binding = None;
    let reading = refresh(&mut transport, &mut binding, &budget(&stopped));
    assert_eq!(reading.percent, Some(100));
    assert!(reading.above_safe_limit);
    for operation in [INCREASE, Operation::ToggleMute] {
        let (_, result) = execute(&mut transport, &mut binding, operation, &budget(&stopped));
        assert_eq!(result, Err(Failure::Invalid));
    }
    assert!(transport.writes.is_empty());
    let (reading, result) = execute(&mut transport, &mut binding, DECREASE, &budget(&stopped));
    assert_eq!(result, Ok(()));
    assert_eq!(reading.percent, Some(95));
    assert_eq!(reading.muted, Some(true));
    assert!(!reading.above_safe_limit);
    let (reading, result) = execute(
        &mut transport,
        &mut binding,
        Operation::ToggleMute,
        &budget(&stopped),
    );
    assert_eq!(result, Ok(()));
    assert_eq!(reading.muted, Some(false));
    assert_eq!(
        transport.writes,
        [(42, Change::Volume(0.95)), (42, Change::Mute(false))]
    );
    assert_eq!(
        plan(
            &Levels {
                channels: vec![0.0],
                muted: false
            },
            DECREASE
        ),
        Err(Failure::Invalid)
    );
    assert_eq!(
        plan(
            &Levels {
                channels: vec![1.0],
                muted: false
            },
            INCREASE
        ),
        Err(Failure::Invalid)
    );
}

#[test]
fn relative_change_uses_one_fresh_snapshot_and_unmute_checks_every_channel() {
    let stopped = AtomicBool::new(false);
    let mut transport = Fake::default();
    let mut binding = None;
    refresh(&mut transport, &mut binding, &budget(&stopped));
    assert_eq!(transport.reads, 1);
    // The displayed value is stale; the fresh whole graph is authoritative.
    transport.event = Some((transport.reads + 1, Event::Channels(vec![0.216, 0.216])));
    let (reading, result) = execute(&mut transport, &mut binding, INCREASE, &budget(&stopped));
    assert_eq!(result, Ok(()));
    assert_eq!(transport.reads, 3);
    assert_eq!(reading.percent, Some(65));
    assert_eq!(transport.writes, [(42, Change::Volume(0.65))]);

    transport.writes.clear();
    transport.muted = true;
    transport.event = Some((transport.reads + 1, Event::Channels(vec![0.125, 1.1])));
    let (_, result) = execute(
        &mut transport,
        &mut binding,
        Operation::ToggleMute,
        &budget(&stopped),
    );
    assert_eq!(result, Err(Failure::Invalid));
    assert!(transport.writes.is_empty());
}

#[test]
fn changing_server_default_serial_or_route_requires_an_explicit_refresh() {
    let stopped = AtomicBool::new(false);
    for event in [
        Event::Restart,
        Event::NewDefault,
        Event::ReplaceNode,
        Event::ReplaceDevice,
        Event::NewRoute,
    ] {
        let mut transport = Fake {
            route: Some(1),
            ..Fake::default()
        };
        let mut binding = None;
        refresh(&mut transport, &mut binding, &budget(&stopped));
        transport.event(event);
        let (reading, result) = execute(&mut transport, &mut binding, INCREASE, &budget(&stopped));
        assert_eq!(result, Err(Failure::Changed));
        assert_eq!(reading.availability, Availability::Unavailable);
        assert!(transport.writes.is_empty());
        assert!(binding.is_none());
        assert_eq!(
            refresh(&mut transport, &mut binding, &budget(&stopped)).availability,
            Availability::Available
        );
        assert!(binding.is_some());
    }
}

#[test]
fn changed_fresh_snapshot_is_not_written_and_changed_readback_is_unknown() {
    let stopped = AtomicBool::new(false);
    for (route, event) in [
        (None, Event::Restart),
        (None, Event::NewDefault),
        (None, Event::ReplaceNode),
        (Some(1), Event::ReplaceDevice),
        (Some(1), Event::NewRoute),
    ] {
        for after_write in [false, true] {
            let mut transport = Fake {
                route,
                ..Fake::default()
            };
            let mut binding = None;
            refresh(&mut transport, &mut binding, &budget(&stopped));
            transport.event = Some((
                transport.reads + if after_write { 2 } else { 1 },
                event.clone(),
            ));
            let (_, result) = execute(&mut transport, &mut binding, INCREASE, &budget(&stopped));
            assert_eq!(
                result,
                Err(if after_write {
                    Failure::Unknown
                } else {
                    Failure::Changed
                })
            );
            assert_eq!(transport.writes.len(), usize::from(after_write));
            assert!(binding.is_none());
        }
    }
}

#[test]
fn success_exit_without_confirmed_complete_state_is_unknown() {
    let stopped = AtomicBool::new(false);
    for partial in [false, true] {
        let mut transport = Fake {
            apply_writes: partial,
            only_first_channel: partial,
            ..Fake::default()
        };
        let mut binding = None;
        refresh(&mut transport, &mut binding, &budget(&stopped));
        let (reading, result) = execute(&mut transport, &mut binding, INCREASE, &budget(&stopped));
        assert_eq!(result, Err(Failure::Unknown));
        assert_eq!(reading.percent, Some(if partial { 55 } else { 50 }));
        assert_eq!(transport.writes.len(), 1);
    }
    let before = Levels {
        channels: vec![0.125, 0.125],
        muted: false,
    };
    let changed_volume = Levels {
        channels: vec![0.216, 0.125],
        muted: true,
    };
    assert!(!confirmed(&before, &changed_volume, Change::Mute(true)));
}

#[test]
fn uncertain_write_has_one_attempt_and_never_reuses_stale_percent() {
    let stopped = AtomicBool::new(false);
    for (missing, empty) in [(false, false), (true, false), (false, true)] {
        let mut transport = Fake {
            write_error: Some(Failure::Unknown),
            missing_after_write: missing,
            empty_after_write: empty,
            ..Fake::default()
        };
        let mut binding = None;
        refresh(&mut transport, &mut binding, &budget(&stopped));
        let (reading, result) = execute(&mut transport, &mut binding, INCREASE, &budget(&stopped));
        assert_eq!(result, Err(Failure::Unknown));
        assert_eq!(
            reading.percent,
            if missing || empty { None } else { Some(55) }
        );
        assert_eq!(transport.writes.len(), 1);
    }
}

#[test]
fn no_default_no_endpoint_or_cancelled_budget_never_synthesizes_a_device() {
    struct Absent;
    impl Transport for Absent {
        fn dump(&mut self, _: &Budget<'_>) -> Result<String, Failure> {
            Err(Failure::Unavailable)
        }
        fn change(&mut self, _: u32, _: Change, _: &Budget<'_>) -> Result<(), Failure> {
            panic!("an unavailable service cannot receive a write");
        }
    }
    let stopped = AtomicBool::new(false);
    let mut binding = None;
    assert_eq!(
        refresh(&mut Absent, &mut binding, &budget(&stopped)).percent,
        None
    );
    assert_eq!(
        execute(&mut Absent, &mut binding, INCREASE, &budget(&stopped)).1,
        Err(Failure::Unavailable)
    );
    let stopped = AtomicBool::new(true);
    let mut transport = Fake::default();
    let reading = refresh(&mut transport, &mut binding, &budget(&stopped));
    assert_eq!(reading.percent, None);
    assert_eq!(transport.reads, 0);
}
