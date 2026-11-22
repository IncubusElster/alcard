use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use zbus::Variant;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "alcard-backlight-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::create_dir(path.join("panel")).unwrap();
        fs::write(path.join("panel/max_brightness"), "100\n").unwrap();
        fs::write(path.join("panel/brightness"), "75\n").unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn session_value() -> Session {
    Session {
        owner: ":1.21".into(),
        path: "/org/freedesktop/login1/session/c1".into(),
        id: "c1".into(),
        uid: 1000,
        seat: "seat0".into(),
    }
}
struct Fake {
    current: Session,
    root: PathBuf,
    writes: Vec<u32>,
    confirm: bool,
}
impl Login for Fake {
    fn current(&mut self, _: &Budget<'_>) -> Result<Session, Failure> {
        Ok(self.current.clone())
    }
    fn set(&mut self, _: &Session, name: &str, value: u32, _: &Budget<'_>) -> Result<(), Failure> {
        self.writes.push(value);
        if self.confirm {
            fs::write(self.root.join(name).join("brightness"), value.to_string()).unwrap();
        }
        Ok(())
    }
}

#[test]
fn brightness_steps_never_black_out_the_panel_and_handle_small_ranges() {
    assert_eq!(adjusted(75, 100, Direction::Increase), 80);
    assert_eq!(adjusted(3, 100, Direction::Decrease), 1);
    assert_eq!(adjusted(0, 100, Direction::Increase), 5);
    assert_eq!(adjusted(99, 100, Direction::Increase), 100);
    assert_eq!(adjusted(1, 1, Direction::Decrease), 1);
    assert_eq!(adjusted(100, 65535, Direction::Decrease), 100);
}

#[test]
fn brightness_requires_current_identity_and_observed_readback() {
    let fixture = Fixture::new();
    let stop = AtomicBool::new(false);
    let budget = Budget::new(&stop, super::super::READ_TIMEOUT);
    let mut fake = Fake {
        current: session_value(),
        root: fixture.0.clone(),
        writes: Vec::new(),
        confirm: true,
    };
    let (binding, observed) = refresh(&fixture.0, &mut fake, &budget);
    assert_eq!(observed.percent, Some(75));
    let binding = binding.unwrap();
    let (observed, result) = adjust(
        &fixture.0,
        &mut fake,
        &binding,
        Direction::Increase,
        &budget,
    );
    assert_eq!(result, Ok(()));
    assert_eq!(observed.percent, Some(80));
    fake.confirm = false;
    let (observed, result) = adjust(
        &fixture.0,
        &mut fake,
        &binding,
        Direction::Increase,
        &budget,
    );
    assert_eq!(result, Err(Failure::Unknown));
    assert_eq!(observed.percent, Some(80));
    fake.current.owner = ":1.22".into();
    let before = fake.writes.len();
    assert_eq!(
        adjust(
            &fixture.0,
            &mut fake,
            &binding,
            Direction::Increase,
            &budget
        )
        .1,
        Err(Failure::Changed)
    );
    assert_eq!(fake.writes.len(), before);
    fs::create_dir(fixture.0.join("second")).unwrap();
    assert_eq!(discover(&fixture.0), Err(Failure::Unavailable));
}

#[test]
fn brightness_method_requires_the_exact_session_signature() {
    let xml = format!("<node><interface name='{SESSION}'><method name='SetBrightness'><arg type='s'/><arg type='s'/><arg type='u'/></method></interface></node>");
    assert_eq!(supports_brightness(&xml), Ok(true));
    assert_eq!(
        supports_brightness(&xml.replace("type='u'", "type='b'")),
        Ok(false)
    );
    assert_eq!(
        supports_brightness(&xml.replace("type='u'", "type='u' direction='out'")),
        Ok(false)
    );
    assert_eq!(
        supports_brightness(&xml.replace(SESSION, MANAGER)),
        Ok(false)
    );
    assert_eq!(supports_brightness(&format!("<!--{xml}-->")), Ok(false));
}

#[test]
fn same_path_replacement_is_a_different_backlight() {
    let fixture = Fixture::new();
    let first = discover(&fixture.0).unwrap().0;
    let retained = std::fs::File::open(fixture.0.join("panel")).unwrap();
    fs::remove_dir_all(fixture.0.join("panel")).unwrap();
    fs::create_dir(fixture.0.join("panel")).unwrap();
    fs::write(fixture.0.join("panel/max_brightness"), "100\n").unwrap();
    fs::write(fixture.0.join("panel/brightness"), "75\n").unwrap();
    let second = discover(&fixture.0).unwrap().0;
    assert_eq!(first.canonical, second.canonical);
    assert_ne!(first, second);
    drop(retained);
}

#[test]
fn brightness_requires_owned_active_local_graphical_session() {
    let props = |active: bool, uid: u32| {
        HashMap::from([
            (
                "Id".into(),
                OwnedVariant::try_from(Variant::from("c1")).unwrap(),
            ),
            (
                "User".into(),
                OwnedVariant::try_from(Variant::from((
                    uid,
                    OwnedObjectPath::try_from("/user").unwrap(),
                )))
                .unwrap(),
            ),
            (
                "Type".into(),
                OwnedVariant::try_from(Variant::from("wayland")).unwrap(),
            ),
            (
                "Seat".into(),
                OwnedVariant::try_from(Variant::from((
                    "seat0",
                    OwnedObjectPath::try_from("/seat").unwrap(),
                )))
                .unwrap(),
            ),
            ("Active".into(), OwnedVariant::from(active)),
            ("Remote".into(), OwnedVariant::from(false)),
        ])
    };
    let path = OwnedObjectPath::try_from(session_value().path).unwrap();
    assert!(session(":1.21".into(), &path, props(true, 1000), 1000).is_ok());
    assert_eq!(
        session(":1.21".into(), &path, props(false, 1000), 1000),
        Err(Failure::Denied)
    );
    assert_eq!(
        session(":1.21".into(), &path, props(true, 1001), 1000),
        Err(Failure::Denied)
    );
    let mut remote = props(true, 1000);
    remote.insert("Remote".into(), OwnedVariant::from(true));
    assert_eq!(
        session(":1.21".into(), &path, remote, 1000),
        Err(Failure::Unavailable)
    );
}
