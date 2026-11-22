//! UI composition using tiny-skia. No handwritten pixel rasterizer.
use crate::i18n::ShellLabels;
#[cfg(test)]
use crate::system::Operation;
use crate::{
    controls::{Availability, Control},
    font,
    modal::{View, MODAL_COLUMNS, MODAL_ROWS},
    model::{
        Action, AppTarget, ApplicationView, Dialog, LaunchPhase, NotificationView, QuickAction,
        Screen, Shell, SystemView, NOTICE_COLUMNS, NOTICE_ROWS,
    },
    radios, sleep,
    status::{BatteryState, ChargeState, LinkState, NetworkStatus},
    system::Capability,
    theme::Theme,
};
use alcard_ui::{
    chrome,
    icons::{self, IconKind},
    preferences::BatteryPreferences,
};
use tiny_skia::{Color, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform};

mod indicators;
mod quick_settings;

use indicators::{bell, status};
use quick_settings::system_view;

pub const WIDTH: usize = 320;
pub const HEIGHT: usize = 170;

fn rgb(value: u32) -> Color {
    Color::from_rgba8((value >> 16) as u8, (value >> 8) as u8, value as u8, 255)
}
fn paint_color(value: u32) -> Paint<'static> {
    let mut paint = Paint {
        anti_alias: false,
        ..Paint::default()
    };
    paint.set_color(rgb(value));
    paint
}
fn rect(target: &mut Pixmap, x: i32, y: i32, w: i32, h: i32, color: u32) {
    if let Some(rect) = Rect::from_xywh(x as f32, y as f32, w as f32, h as f32) {
        target.fill_rect(rect, &paint_color(color), Transform::identity(), None);
    }
}
fn dim(target: &mut Pixmap, x: i32, y: i32, w: i32, h: i32) {
    if let Some(rect) = Rect::from_xywh(x as f32, y as f32, w as f32, h as f32) {
        let mut paint = Paint {
            anti_alias: false,
            ..Paint::default()
        };
        paint.set_color_rgba8(0, 0, 0, 128);
        target.fill_rect(rect, &paint, Transform::identity(), None);
    }
}
fn line(target: &mut Pixmap, from: (i32, i32), to: (i32, i32), color: u32) {
    let mut path = PathBuilder::new();
    path.move_to(from.0 as f32 + 0.5, from.1 as f32 + 0.5);
    path.line_to(to.0 as f32 + 0.5, to.1 as f32 + 0.5);
    if let Some(path) = path.finish() {
        target.stroke_path(
            &path,
            &paint_color(color),
            &Stroke {
                width: 1.0,
                ..Stroke::default()
            },
            Transform::identity(),
            None,
        );
    }
}

fn abbreviated(value: &str, columns: usize) -> String {
    let mut chars = value
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch });
    let mut result: String = chars.by_ref().take(columns).collect();
    if chars.next().is_some() && columns > 0 {
        result.pop();
        result.push('…');
    }
    result
}

pub fn paint_notification(
    target: &mut Pixmap,
    title: &str,
    background: u32,
    foreground: u32,
    accent: u32,
) {
    assert_eq!((target.width(), target.height()), (WIDTH as u32, 40));
    target.fill(Color::TRANSPARENT);
    chrome::shadow(target, (4, 4, 312, 31));
    chrome::panel(target, (4, 4, 312, 31), background, accent);
    chrome::gradient(
        target,
        (5, 5, 310, 29),
        1,
        chrome::mix(background, foreground, 8),
        background,
    );
    bell(target, 13, 13, accent);
    font::text(target, 33, 16, &abbreviated(title, 34), foreground, 34);
}

const SOFTKEY_HEIGHT: i32 = 16;
const SOFTKEY_TOP: i32 = HEIGHT as i32 - SOFTKEY_HEIGHT;
const SOFTKEY_MAX_GLYPHS: usize = 6;

fn softkeys(canvas: &mut Pixmap, s: &Shell) {
    let t = &s.theme;
    chrome::bar(canvas, (0, SOFTKEY_TOP, WIDTH as i32, SOFTKEY_HEIGHT), t);
    rect(canvas, 0, SOFTKEY_TOP, WIDTH as i32, 1, t.border);
    if s.is_text_editing() {
        // Digits and physical softkeys are text in this mode. Name the real
        // keyboard actions instead of implying that 4/8 apply or cancel.
        chrome::rounded(canvas, (4, SOFTKEY_TOP + 2, 152, 12), 2, t.selected);
        for (center, label) in [
            (80, s.locale.search_apply_key()),
            (240, s.locale.search_cancel_key()),
        ] {
            font::centered(canvas, center, SOFTKEY_TOP + 4, label, t.text, 18);
        }
        return;
    }
    let actions = s.actions();
    for (i, action) in actions.iter().enumerate() {
        if *action == Action::None {
            continue;
        }
        let x = (i * 64) as i32;
        let primary = if s.screen == Screen::Home && !s.modal_active() {
            2
        } else {
            actions
                .iter()
                .position(|action| *action != Action::None)
                .unwrap_or_default()
        };
        if i == primary {
            chrome::rounded(canvas, (x + 4, SOFTKEY_TOP + 2, 56, 12), 2, t.selected);
        }
        font::centered(
            canvas,
            x + 32,
            SOFTKEY_TOP + 4,
            s.locale.action(*action),
            t.text,
            SOFTKEY_MAX_GLYPHS,
        );
    }
}
fn home(canvas: &mut Pixmap, s: &Shell, now: &str) {
    let t = &s.theme;
    chrome::gradient(
        canvas,
        (0, 18, WIDTH as i32, 136),
        0,
        chrome::mix(t.background, t.surface, 55),
        t.background,
    );
    let clock_columns = now.chars().take(5).count() as i32;
    font::text_scaled(canvas, (160 - clock_columns * 12, 35), now, t.text, 5, 3);
    let date = chrono::Local::now().format("%d.%m.%Y").to_string();
    font::centered(canvas, 160, 68, &date, t.muted, 18);
    let unread = s.unread_count();
    let notice = match s.locale {
        crate::i18n::Locale::Ru => format!("Новых уведомлений: {unread}"),
        crate::i18n::Locale::Es => format!("Avisos nuevos: {unread}"),
        crate::i18n::Locale::En => format!("New notifications: {unread}"),
    };
    let color = if unread > 0 { t.accent } else { t.muted };
    font::centered(canvas, 160, 88, &notice, color, 34);
    rect(canvas, 16, 105, 288, 1, t.border);
    font::text(
        canvas,
        16,
        112,
        &s.locale.battery(s.status.battery),
        t.muted,
        36,
    );
    font::text(
        canvas,
        16,
        124,
        &format!("Wi-Fi: {}", s.locale.link(s.status.wifi.state)),
        t.muted,
        36,
    );
    font::text(
        canvas,
        16,
        136,
        &format!("Ethernet: {}", s.locale.link(s.status.ethernet.state)),
        t.muted,
        36,
    );
}
fn search_caption(s: &Shell, count: usize) -> String {
    let prefix = format!("{}: ", s.locale.action(Action::Search));
    let suffix = format!(" ({count})");
    let columns = 38_usize.saturating_sub(prefix.chars().count() + suffix.chars().count());
    format!("{prefix}{}{suffix}", abbreviated(&s.search_query, columns))
}

fn menu(canvas: &mut Pixmap, s: &Shell) {
    let t = &s.theme;
    let apps = s.menu_apps();
    let count = apps.len();
    let filtered = !s.search_query.is_empty();
    if filtered {
        font::text(canvas, 8, 20, &search_caption(s, count), t.accent, 38);
    } else if let Some(app) = apps.get(s.selected) {
        font::text(canvas, 8, 20, &abbreviated(&app.name, 38), t.text, 38);
    }
    if count == 0 {
        font::centered(
            canvas,
            160,
            78,
            if filtered {
                s.locale.search_empty()
            } else {
                s.locale.empty()
            },
            t.muted,
            36,
        );
        return;
    }
    let start = s.page * 8;
    for (index, app) in apps.iter().enumerate().skip(start).take(8) {
        let i = index - start;
        let col = i % 4;
        let row = i / 4;
        let x = 8 + col as i32 * 78;
        let y = 31 + row as i32 * 52;
        if s.selected == index {
            chrome::selection(canvas, (x, y, 72, 47), t);
        }
        crate::icon_cache::draw_application(
            canvas,
            x + 24,
            y + 3,
            24,
            app.icon.as_deref(),
            &app.desktop_id,
            t,
        );
        let windows = s.running_count(index);
        if let Some(status) = s.launch_status(&app.desktop_id) {
            let badge = match status.phase {
                LaunchPhase::Starting => t.accent,
                LaunchPhase::Unconfirmed => t.warning,
                LaunchPhase::Failed => t.error,
            };
            chrome::rounded(canvas, (x + 57, y + 2, 12, 12), 2, badge);
            let mark = match status.phase {
                LaunchPhase::Starting => "…",
                LaunchPhase::Unconfirmed => "?",
                LaunchPhase::Failed => "!",
            };
            font::text(canvas, x + 59, y + 4, mark, t.background, 1);
        } else if windows > 0 {
            // Small 1..9 badge: indicates running windows without obscuring
            // app name or requiring another status bar.
            chrome::rounded(canvas, (x + 57, y + 2, 12, 12), 2, t.accent);
            font::text(
                canvas,
                x + 59,
                y + 4,
                &windows.min(9).to_string(),
                t.background,
                1,
            );
        }
        let name = app.name.as_str();
        // Limit each glyph row to the width of one 4x2 menu tile.
        font::centered(canvas, x + 36, y + 34, &abbreviated(name, 8), t.text, 8);
    }
    if let Some(status) = apps
        .get(s.selected)
        .and_then(|app| s.launch_status(&app.desktop_id))
    {
        font::text(
            canvas,
            8,
            136,
            s.locale.launch_phase(status.phase),
            t.accent,
            30,
        );
    }
    font::centered(
        canvas,
        288,
        136,
        &format!("{}/{}", s.page + 1, count.div_ceil(8)),
        t.muted,
        6,
    );
}
fn list(canvas: &mut Pixmap, s: &Shell) {
    let t = &s.theme;
    let count = s.count();
    if count == 0 {
        let message = if s.screen == Screen::Notifications && s.notifications_unavailable {
            s.locale.notification_unavailable()
        } else if s.screen == Screen::Recent && s.toplevel_unavailable {
            s.locale.window_manager_unavailable()
        } else {
            s.locale.empty()
        };
        font::centered(canvas, 160, 77, message, t.muted, 36);
        return;
    }
    let start_y = if s.screen == Screen::Notifications && s.notifications_unavailable {
        font::text(
            canvas,
            16,
            22,
            s.locale.notification_unavailable(),
            t.accent,
            36,
        );
        36
    } else {
        30
    };
    let first = if s.selected < 5 { 0 } else { s.selected - 4 };
    let last = (first + 5).min(count);
    for index in first..last {
        let y = start_y + (index - first) as i32 * 22;
        if s.selected == index {
            chrome::selection(canvas, (8, y, 304, 21), t);
        } else {
            chrome::rounded(canvas, (8, y, 304, 21), 2, t.surface);
        }
        match s.screen {
            Screen::Recent => {
                if let Some((item, running)) = s.window_at(index) {
                    let app = s.resolve_app(&item.app_id);
                    crate::icon_cache::draw_application(
                        canvas,
                        16,
                        y + 2,
                        16,
                        app.and_then(|entry| entry.icon.as_deref()),
                        app.map_or(item.app_id.as_str(), |entry| entry.desktop_id.as_str()),
                        t,
                    );
                    let title = if item.title.is_empty() {
                        &item.app_id
                    } else {
                        &item.title
                    };
                    font::text(canvas, 42, y + 2, &abbreviated(title, 33), t.text, 33);
                    let state = if let Some(operation) = s.window_operation(item.id) {
                        s.locale.window_operation(operation)
                    } else if let Some(status) = s
                        .resolve_app(&item.app_id)
                        .and_then(|app| s.launch_status(&app.desktop_id))
                    {
                        s.locale.launch_phase(status.phase)
                    } else if running {
                        s.locale.running()
                    } else {
                        s.locale.closed()
                    };
                    font::text(canvas, 42, y + 11, &abbreviated(state, 33), t.muted, 33);
                }
            }
            Screen::Notifications => {
                let item = &s.notices[index];
                let kind = if item.urgency >= 2 {
                    IconKind::Warning
                } else {
                    IconKind::Notifications
                };
                icons::draw_icon(canvas, 14, y + 2, 16, kind, t);
                if item.unread {
                    rect(canvas, 10, y + 9, 2, 2, t.accent);
                }
                let title = if item.title.is_empty() {
                    s.locale.notification_title()
                } else {
                    &item.title
                };
                font::text(canvas, 36, y + 2, &abbreviated(title, 32), t.text, 32);
                font::text(
                    canvas,
                    36,
                    y + 11,
                    &abbreviated(&item.description, 32),
                    t.muted,
                    32,
                );
                if item.urgency >= 2 {
                    font::text(canvas, 300, y + 2, "!", t.warning, 1);
                }
            }
            _ => {}
        }
    }
}

fn scroll_indicator(
    canvas: &mut Pixmap,
    y: i32,
    height: i32,
    offset: usize,
    shown: usize,
    total: usize,
    t: &Theme,
) {
    if total <= shown {
        return;
    }
    rect(canvas, 313, y, 2, height, t.border);
    let handle = ((height as usize * shown / total).max(4) as i32).min(height);
    let travel = height - handle;
    let top = travel as usize * offset.min(total - shown) / (total - shown);
    rect(canvas, 312, y + top as i32, 4, handle, t.accent);
}

fn notification_detail(canvas: &mut Pixmap, s: &Shell, scroll: usize) {
    let Some(notice) = s.viewed_notice() else {
        return;
    };
    let t = &s.theme;
    let lines = notice.text_lines(s.locale);
    let scroll = scroll.min(lines.len().saturating_sub(NOTICE_ROWS));
    let header = if s.notifications_unavailable {
        s.locale.notification_unavailable().to_string()
    } else {
        format!(
            "{}  {}-{}/{}",
            s.locale.action(Action::Read),
            scroll + 1,
            (scroll + NOTICE_ROWS).min(lines.len()),
            lines.len(),
        )
    };
    font::text(canvas, 16, 22, &header, t.muted, 36);
    for (row, text) in lines.iter().skip(scroll).take(NOTICE_ROWS).enumerate() {
        font::text(
            canvas,
            16,
            34 + row as i32 * 12,
            text,
            t.text,
            NOTICE_COLUMNS,
        );
    }
    scroll_indicator(canvas, 34, 116, scroll, NOTICE_ROWS, lines.len(), t);
}

fn context_rows(
    canvas: &mut Pixmap,
    s: &Shell,
    header: &str,
    rows: &[(String, bool)],
    selected: usize,
) {
    let t = &s.theme;
    font::text(canvas, 16, 22, &abbreviated(header, 36), t.accent, 36);
    let first = selected.saturating_sub(4);
    for (index, (label, enabled)) in rows.iter().enumerate().skip(first).take(5) {
        let y = 36 + (index - first) as i32 * 22;
        if index == selected {
            chrome::selection(canvas, (8, y, 300, 21), t);
        } else {
            chrome::rounded(canvas, (8, y, 300, 21), 2, t.surface);
        }
        let chars: Vec<char> = abbreviated(label, 70).chars().collect();
        for (row, chunk) in chars.chunks(35).take(2).enumerate() {
            let text: String = chunk.iter().collect();
            font::text(
                canvas,
                16,
                y + 2 + row as i32 * 9,
                &text,
                if *enabled { t.text } else { t.disabled },
                35,
            );
        }
    }
    scroll_indicator(canvas, 36, 110, first, 5, rows.len(), t);
}

fn notification_options(
    canvas: &mut Pixmap,
    s: &Shell,
    entries: &[crate::model::NotificationOption],
    selected: usize,
) {
    let rows: Vec<_> = entries
        .iter()
        .map(|entry| (s.locale.notification_option(entry).to_string(), true))
        .collect();
    context_rows(canvas, s, s.locale.notification_actions(), &rows, selected);
}

fn notifications(canvas: &mut Pixmap, s: &Shell) {
    match &s.notification_view {
        NotificationView::List => list(canvas, s),
        NotificationView::Detail { scroll, .. } => notification_detail(canvas, s, *scroll),
        NotificationView::Options {
            entries, selected, ..
        } => notification_options(canvas, s, entries, *selected),
    }
}

fn application_detail(canvas: &mut Pixmap, s: &Shell, target: &AppTarget, scroll: usize) {
    let t = &s.theme;
    let lines = s.application_lines(target);
    let scroll = scroll.min(lines.len().saturating_sub(NOTICE_ROWS));
    font::text(
        canvas,
        16,
        22,
        &format!(
            "{}  {}-{}/{}",
            s.locale.application_details(),
            scroll + 1,
            (scroll + NOTICE_ROWS).min(lines.len()),
            lines.len()
        ),
        t.muted,
        36,
    );
    for (row, line) in lines.iter().skip(scroll).take(NOTICE_ROWS).enumerate() {
        font::text(
            canvas,
            16,
            34 + row as i32 * 12,
            line,
            t.text,
            NOTICE_COLUMNS,
        );
    }
    scroll_indicator(canvas, 34, 116, scroll, NOTICE_ROWS, lines.len(), t);
}

fn application(canvas: &mut Pixmap, s: &Shell) {
    match &s.application_view {
        ApplicationView::List => match s.screen {
            Screen::Menu => menu(canvas, s),
            _ => list(canvas, s),
        },
        ApplicationView::Options {
            target,
            entries,
            selected,
        } => {
            let rows: Vec<_> = entries
                .iter()
                .map(|entry| {
                    (
                        s.locale.application_option(entry).to_string(),
                        s.application_option_enabled(target, entry),
                    )
                })
                .collect();
            let header = format!(
                "{}: {}",
                s.locale.action(Action::Actions),
                s.target_name(target)
            );
            context_rows(canvas, s, &header, &rows, *selected);
        }
        ApplicationView::Windows { desktop_id, choice } => {
            let windows = s.app_windows(desktop_id);
            let selected = windows
                .iter()
                .position(|window| Some(window.id) == choice.selected().copied())
                .unwrap_or_default();
            let rows: Vec<_> = windows
                .iter()
                .map(|window| {
                    let title = if window.title.is_empty() {
                        &window.app_id
                    } else {
                        &window.title
                    };
                    (title.clone(), true)
                })
                .collect();
            context_rows(canvas, s, s.locale.application_windows(), &rows, selected);
        }
        ApplicationView::Details { target, scroll } => {
            application_detail(canvas, s, target, *scroll)
        }
        ApplicationView::ConfirmRetry { desktop_id, .. } => {
            application_detail(canvas, s, &AppTarget::Desktop(desktop_id.clone()), 0);
        }
    }
}

fn dialog(canvas: &mut Pixmap, s: &Shell, d: &Dialog) {
    let view = View::for_dialog(d, s.locale, &s.live_windows, s.notices.len());
    modal_view(canvas, s, &view, s.dialog_scroll);
}

fn modal_view(canvas: &mut Pixmap, s: &Shell, view: &View, scroll: usize) {
    let t = &s.theme;
    let lines = view.lines();
    let scroll = scroll.min(lines.len().saturating_sub(MODAL_ROWS));
    let shown = lines.len().clamp(1, MODAL_ROWS);
    let height = (44 + shown as i32 * 12).clamp(80, 116);
    let top = 18 + (SOFTKEY_TOP - 18 - height) / 2;
    // Every confirmation uses only the bottom action row. The panel holds
    // readable context; no second Yes/No or Clear/Cancel controls live here.
    dim(canvas, 0, 18, WIDTH as i32, SOFTKEY_TOP - 18);
    chrome::shadow(canvas, (12, top, 296, height));
    chrome::panel(canvas, (12, top, 296, height), t.surface, t.border);
    chrome::gradient(
        canvas,
        (13, top + 1, 294, 20),
        1,
        chrome::mix(t.selected, t.text, 12),
        t.selected,
    );
    rect(canvas, 13, top + 21, 294, 1, t.border);
    let marker = if view.information {
        IconKind::Info
    } else if view.destructive {
        IconKind::Warning
    } else {
        IconKind::Info
    };
    icons::draw_icon(canvas, 20, top + 3, 16, marker, t);
    font::text(
        canvas,
        42,
        top + 7,
        &abbreviated(&view.title, 31),
        t.text,
        31,
    );
    for (row, line) in lines.iter().skip(scroll).take(MODAL_ROWS).enumerate() {
        font::text(
            canvas,
            20,
            top + 30 + row as i32 * 12,
            line,
            t.text,
            MODAL_COLUMNS,
        );
    }
    if lines.len() > MODAL_ROWS {
        let track = MODAL_ROWS as i32 * 12;
        let handle = (track as usize * MODAL_ROWS / lines.len()).max(4) as i32;
        let offset = (track - handle) as usize * scroll / (lines.len() - MODAL_ROWS);
        rect(canvas, 300, top + 28, 1, track, t.border);
        rect(canvas, 299, top + 28 + offset as i32, 3, handle, t.accent);
    }
}

const SEARCH_COLUMNS: usize = 32;
const SEARCH_FIELD_X: i32 = 20;
const SEARCH_FIELD_Y: i32 = 71;
const SEARCH_TEXT_X: i32 = 32;
const SEARCH_TEXT_Y: i32 = 78;
const SEARCH_CARET_Y: i32 = 77;

fn search_dialog(canvas: &mut Pixmap, s: &Shell) {
    let Some(input) = s.search_input.as_ref() else {
        return;
    };
    let t = &s.theme;
    dim(canvas, 0, 18, WIDTH as i32, SOFTKEY_TOP - 18);
    chrome::shadow(canvas, (12, 32, 296, 112));
    chrome::panel(canvas, (12, 32, 296, 112), t.surface, t.border);
    chrome::gradient(
        canvas,
        (13, 33, 294, 20),
        1,
        chrome::mix(t.selected, t.text, 12),
        t.selected,
    );
    rect(canvas, 13, 53, 294, 1, t.border);
    font::text(canvas, 20, 39, s.locale.search_title(), t.text, 35);
    font::text(canvas, 20, 58, s.locale.search_hint(), t.muted, 34);
    chrome::panel(
        canvas,
        (SEARCH_FIELD_X, SEARCH_FIELD_Y, 280, 22),
        t.background,
        t.accent,
    );
    let viewport = input.viewport(SEARCH_COLUMNS);
    font::text(
        canvas,
        SEARCH_TEXT_X,
        SEARCH_TEXT_Y,
        &viewport.text,
        t.text,
        SEARCH_COLUMNS,
    );
    if viewport.offset > 0 {
        font::text(canvas, 24, SEARCH_TEXT_Y, "<", t.muted, 1);
    }
    if viewport.offset + viewport.text.chars().count() < input.value().chars().count() {
        font::text(canvas, 288, SEARCH_TEXT_Y, ">", t.muted, 1);
    }
    // The shared viewport reserves a blank final cell when the cursor is at
    // the end, keeping the insertion point inside the field for long UTF-8.
    let caret_x = SEARCH_TEXT_X + viewport.cursor.min(SEARCH_COLUMNS - 1) as i32 * 8;
    rect(canvas, caret_x, SEARCH_CARET_Y, 1, 11, t.accent);
    font::text(
        canvas,
        20,
        102,
        &s.locale.search_matches(s.search_match_count()),
        t.text,
        34,
    );
    font::text(canvas, 20, 124, s.locale.search_clear_hint(), t.muted, 34);
}
pub fn paint(canvas: &mut Pixmap, s: &Shell, clock: &str, banner: Option<&str>) {
    let t: &Theme = &s.theme;
    canvas.fill(rgb(t.background));
    status(canvas, s, clock);
    match s.screen {
        Screen::Home => home(canvas, s, clock),
        Screen::Menu | Screen::Recent => application(canvas, s),
        Screen::Notifications => notifications(canvas, s),
    }
    softkeys(canvas, s);
    if !s.modal_active() && s.application_view == ApplicationView::List {
        if let Some(text) = banner {
            let t = &s.theme;
            chrome::shadow(canvas, (8, 21, 304, 22));
            chrome::selection(canvas, (8, 21, 304, 22), t);
            font::text(canvas, 16, 28, &abbreviated(text, 35), t.text, 35);
        }
    }
    if let ApplicationView::ConfirmRetry {
        desktop_id, scroll, ..
    } = &s.application_view
    {
        modal_view(
            canvas,
            s,
            &View::for_retry(
                s.locale,
                &s.target_name(&AppTarget::Desktop(desktop_id.clone())),
            ),
            *scroll,
        );
    }
    if let Some(view) = s.system_view.as_ref() {
        system_view(canvas, s, view);
    }
    if let Some(d) = s.dialog.as_ref() {
        dialog(canvas, s, d);
    }
    if s.is_text_editing() {
        search_dialog(canvas, s);
    }
}

#[cfg(test)]
#[path = "unit_tests/render.rs"]
mod tests;
