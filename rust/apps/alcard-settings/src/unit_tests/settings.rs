use super::*;
use ratatui::{backend::TestBackend, Terminal};
#[test]
fn all_eight_sections_and_navigation_fit_the_device_in_every_language() {
    for locale in Locale::ALL {
        let mut app = Browser::new();
        app.locale = locale;
        let mut tty = Terminal::new(TestBackend::new(40, 21)).unwrap();
        tty.draw(|frame| app.render(frame)).unwrap();
        let screen = tty.backend().buffer();
        let line = |y| (0..40).map(|x| screen[(x, y)].symbol()).collect::<String>();
        assert_eq!(app.rows.len(), 8);
        for (i, row) in app.rows.iter().enumerate() {
            assert!(line(i as u16 + 1).contains(tr(locale, row.key)));
        }
        assert!(line(20).contains(tr(locale, "s.nav")));
    }
}
#[test]
fn password_is_masked_on_every_frame() {
    let mut app = Browser::new();
    app.dialog = Dialog::Edit {
        prompt: "Wi-Fi".into(),
        operation: Operation::Wifi {
            iface: "wlan0".into(),
            ssid: "fixture".into(),
        },
        text: Zeroizing::new("only-fixture".into()),
        secret: true,
    };
    let mut tty = Terminal::new(TestBackend::new(40, 21)).unwrap();
    tty.draw(|f| app.render(f)).unwrap();
    let buffer = tty.backend().buffer();
    let screen: String = buffer.content().iter().map(|c| c.symbol()).collect();
    assert!(screen.contains("••••••••••••"));
    assert!(!screen.contains("only-fixture"));
}

#[test]
fn a_long_ipv4_value_remains_visible_on_the_device() {
    let mut app = Browser::new();
    app.rows = vec![Row {
        key: "s.address",
        value: "192.168.100.100/24".into(),
        action: Action::Read,
    }];
    let mut tty = Terminal::new(TestBackend::new(40, 21)).unwrap();
    tty.draw(|frame| app.render(frame)).unwrap();
    let buffer = tty.backend().buffer();
    let status: String = (0..40).map(|x| buffer[(x, 17)].symbol()).collect();
    assert!(status.contains("192.168.100.100/24"));
}
