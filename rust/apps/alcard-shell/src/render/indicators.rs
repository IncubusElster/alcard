//! Status-bar battery, connectivity and unread-notification indicators.

use super::{
    chrome, font, line, rect, BatteryPreferences, BatteryState, ChargeState, LinkState,
    NetworkStatus, Pixmap, Screen, Shell, Theme, WIDTH,
};
use crate::i18n::ShellLabels;

pub(super) fn battery_icon(
    canvas: &mut Pixmap,
    state: BatteryState,
    t: &Theme,
    preferences: BatteryPreferences,
) {
    const X: i32 = 234;
    let charging = matches!(
        state,
        BatteryState::Present {
            charge: ChargeState::Charging | ChargeState::Full,
            ..
        }
    );
    let percent = match state {
        BatteryState::Present { percent, .. } => percent,
        _ => None,
    };
    let critical = !charging && percent.is_some_and(|value| value <= preferences.critical_percent);
    let low = !charging && percent.is_some_and(|value| value <= preferences.low_percent);
    let border = if charging {
        t.success
    } else if critical {
        t.error
    } else if low {
        t.warning
    } else {
        t.muted
    };
    // At 0–5% the proportional fill would round to zero pixels. Keep the
    // critical indication visible even at a real zero; unknown stays neutral.
    chrome::panel(canvas, (X, 4, 22, 10), t.surface, border);
    rect(canvas, X + 22, 7, 2, 4, border);
    match state {
        BatteryState::Unknown => font::text(canvas, X + 7, 5, "?", t.text, 1),
        BatteryState::Absent => line(canvas, (X + 2, 12), (X + 19, 5), t.muted),
        BatteryState::Present { percent, charge } => {
            if let Some(percent) = percent {
                let mut width = i32::from(percent.min(100)) * 18 / 100;
                if low && percent > 0 {
                    width = width.max(1);
                }
                let color = if critical {
                    t.error
                } else if low {
                    t.warning
                } else {
                    t.success
                };
                if width > 0 {
                    chrome::gradient(
                        canvas,
                        (X + 2, 6, width, 6),
                        0,
                        chrome::mix(color, t.text, 22),
                        color,
                    );
                }
            } else {
                let offset = if charge == ChargeState::Charging {
                    2
                } else {
                    7
                };
                font::text(canvas, X + offset, 5, "?", t.text, 1);
            }
            if charge == ChargeState::Charging {
                let bolt = X + if percent.is_none() { 16 } else { 10 };
                line(canvas, (bolt + 2, 2), (bolt - 2, 8), t.text);
                line(canvas, (bolt - 2, 8), (bolt + 3, 8), t.text);
                line(canvas, (bolt + 3, 8), (bolt - 1, 15), t.text);
            }
        }
    }
}

pub(super) fn network_icon(
    canvas: &mut Pixmap,
    x: i32,
    state: NetworkStatus,
    wifi: bool,
    t: &Theme,
) {
    let linked = matches!(
        state.state,
        LinkState::LinkOnly | LinkState::Addressed | LinkState::Routed
    );
    let color = match state.state {
        LinkState::Routed | LinkState::Connecting => t.accent,
        LinkState::Addressed => t.text,
        _ => t.muted,
    };
    if wifi {
        // Three crisp arcs and a dot; signal strength never changes link state.
        let strength = state.strength.unwrap_or(0).min(4);
        for (threshold, points) in [
            (4, [(0, 6), (4, 3), (10, 3), (14, 6)]),
            (3, [(3, 9), (6, 7), (8, 7), (11, 9)]),
            (2, [(5, 12), (7, 10), (7, 10), (9, 12)]),
        ] {
            let arc_color = if linked && strength >= threshold {
                color
            } else {
                t.disabled
            };
            for pair in points.windows(2) {
                if pair[0] != pair[1] {
                    line(
                        canvas,
                        (x + pair[0].0, pair[0].1),
                        (x + pair[1].0, pair[1].1),
                        arc_color,
                    );
                }
            }
        }
        rect(
            canvas,
            x + 6,
            14,
            3,
            2,
            if linked { color } else { t.disabled },
        );
    } else {
        // RJ-style socket: distinct from wireless signal and window badges.
        chrome::panel(canvas, (x, 4, 16, 10), t.surface, color);
        for pin in 0..3 {
            rect(canvas, x + 4 + pin * 3, 5, 1, 3, color);
        }
        rect(canvas, x + 5, 12, 6, 2, color);
        rect(canvas, x + 6, 14, 4, 2, color);
    }
    match state.state {
        LinkState::Unknown => font::text(canvas, x + 15, 5, "?", t.muted, 1),
        LinkState::Absent | LinkState::Disabled | LinkState::Disconnected => {
            line(canvas, (x, 15), (x + 14, 2), t.muted);
        }
        LinkState::LinkOnly => font::text(canvas, x + 15, 5, "!", t.warning, 1),
        LinkState::Connecting => {
            rect(canvas, x + 17, 6, 2, 2, t.accent);
            rect(canvas, x + 17, 11, 2, 2, t.accent);
        }
        LinkState::Addressed | LinkState::Routed => {
            if wifi && state.strength.is_none() {
                font::text(canvas, x + 15, 5, "?", t.muted, 1);
            }
        }
    }
}

pub(super) fn bell(canvas: &mut Pixmap, x: i32, y: i32, color: u32) {
    rect(canvas, x + 5, y, 2, 1, color);
    line(canvas, (x + 2, y + 4), (x + 4, y + 2), color);
    line(canvas, (x + 4, y + 2), (x + 7, y + 2), color);
    line(canvas, (x + 7, y + 2), (x + 9, y + 4), color);
    line(canvas, (x + 2, y + 4), (x + 2, y + 8), color);
    line(canvas, (x + 9, y + 4), (x + 9, y + 8), color);
    rect(canvas, x, y + 9, 12, 1, color);
    rect(canvas, x + 5, y + 11, 2, 1, color);
}

pub(super) fn notification_icon(canvas: &mut Pixmap, s: &Shell) {
    let t = &s.theme;
    let unread = s.unread_count();
    let color = if unread > 0 { t.accent } else { t.muted };
    bell(canvas, 127, 3, color);
    if unread > 0 {
        font::text(canvas, 143, 5, &unread.min(99).to_string(), color, 2);
    } else if s.notifications_unavailable {
        font::text(canvas, 143, 5, "?", t.muted, 1);
    }
}

pub(super) fn status(canvas: &mut Pixmap, s: &Shell, now: &str) {
    let t = &s.theme;
    chrome::bar(canvas, (0, 0, WIDTH as i32, 18), t);
    rect(canvas, 0, 17, WIDTH as i32, 1, t.border);
    font::text(
        canvas,
        8,
        5,
        if s.screen == Screen::Home {
            "ALCARD"
        } else {
            s.locale.screen(s.screen)
        },
        t.accent,
        14,
    );
    notification_icon(canvas, s);
    network_icon(canvas, 175, s.status.wifi, true, t);
    network_icon(canvas, 205, s.status.ethernet, false, t);
    battery_icon(canvas, s.status.battery, t, s.battery_preferences);
    font::text(canvas, 272, 5, now, t.text, 5);
}
// One compact row at the physical bottom edge: no numeric key hints.
// Five 64px slots; up to six 8px glyphs per label leave 8px padding per side.
