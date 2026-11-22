use super::*;
use alcard_l10n::Locale;
use alcard_setup_core::Step;
use ratatui::{backend::TestBackend, Terminal};

#[test]
fn all_screens_show_the_current_step_in_three_languages() {
    for locale in Locale::ALL {
        for step in Step::ALL {
            let backend = TestBackend::new(40, 21);
            let mut terminal = Terminal::new(backend).expect("test terminal");
            let mut app = App::new_firstboot();
            app.wizard.locale = locale;
            app.wizard.step = step;
            terminal.draw(|frame| render(frame, &app)).expect("render");
            let buffer = terminal.backend().buffer();
            let heading: String = (0..40).map(|x| buffer[(x, 2)].symbol()).collect();
            assert_eq!(heading.trim_end(), clipped(app.wizard.title(), 40));
        }
    }
}

#[test]
fn undersized_terminal_shows_resize_hint() {
    let backend = TestBackend::new(32, 16);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    let app = App::new_firstboot();
    terminal
        .draw(|frame| render(frame, &app))
        .expect("resize hint");
    let buffer = terminal.backend().buffer();
    let first: String = (0..32).map(|x| buffer[(x, 0)].symbol()).collect();
    assert_eq!(first.trim_end(), tr(app.wizard.locale, "app.too_small"));
}

#[test]
fn clipping_preserves_unicode_character_width() {
    assert_eq!(clipped("абвг", 3), "абв");
}
