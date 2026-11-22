//! Credential presentation only. Entered text is never drawn, even for PAM echo-on prompts.
use crate::ui::{Form, Language, Phase};
use alcard_ui::{chrome, font, theme::Theme};
use tiny_skia::{Color, Paint, Pixmap, Rect, Transform};

fn rect(image: &mut Pixmap, bounds: (f32, f32, f32, f32), color: u32) {
    let mut paint = Paint {
        anti_alias: false,
        ..Paint::default()
    };
    paint.set_color_rgba8((color >> 16) as u8, (color >> 8) as u8, color as u8, 255);
    if let Some(rect) = Rect::from_xywh(bounds.0, bounds.1, bounds.2, bounds.3) {
        image.fill_rect(rect, &paint, Transform::identity(), None);
    }
}
fn lines(image: &mut Pixmap, y: i32, text: &str, color: u32, rows: usize) {
    let chars: Vec<char> = text.chars().collect();
    for (index, chunk) in chars.chunks(37).take(rows).enumerate() {
        let line: String = chunk.iter().collect();
        font::text(image, 12, y + index as i32 * 11, &line, color, 37);
    }
}
pub fn paint(image: &mut Pixmap, form: &Form, language: Language, theme: &Theme) {
    image.fill(Color::from_rgba8(
        (theme.background >> 16) as u8,
        (theme.background >> 8) as u8,
        theme.background as u8,
        255,
    ));
    chrome::bar(image, (0, 0, 320, 20), theme);
    rect(image, (0., 19., 320., 1.), theme.border);
    font::text(
        image,
        8,
        6,
        language.text("Authorization", "Авторизация", "Autorizacion"),
        theme.accent,
        24,
    );
    let state = format!(
        "{} {}",
        if form.caps_lock { "CAPS" } else { "" },
        form.layout + 1
    );
    font::text(image, 260, 6, &state, theme.muted, 7);
    if form.details {
        let details = form.detail_lines();
        for (index, line) in details.iter().skip(form.scroll).take(10).enumerate() {
            font::text(image, 12, 25 + index as i32 * 11, line, theme.text, 37);
        }
        chrome::bar(image, (0, 149, 320, 21), theme);
        rect(image, (0., 149., 320., 1.), theme.border);
        font::text(
            image,
            8,
            156,
            language.text(
                "Arrows: scroll   F1/Enter: back",
                "Стрелки: листать F1/Enter: назад",
                "Flechas: mover  F1/Enter: volver",
            ),
            theme.text,
            38,
        );
        return;
    }
    font::text(image, 12, 25, &form.action, theme.muted, 37);
    lines(image, 38, &form.reason, theme.text, 2);
    if form.phase == Phase::Choosing {
        font::text(
            image,
            12,
            63,
            language.text(
                "Choose a user:",
                "Выберите пользователя:",
                "Seleccione usuario:",
            ),
            theme.accent,
            37,
        );
        let start = form.selected.unwrap_or(0).saturating_sub(4);
        for (row, (index, user)) in form
            .identities
            .iter()
            .enumerate()
            .skip(start)
            .take(5)
            .enumerate()
        {
            let y = 77 + row as i32 * 13;
            if form.selected == Some(index) {
                chrome::selection(image, (8, y - 2, 304, 12), theme);
            }
            font::text(image, 12, y, &user.label, theme.text, 37);
        }
    } else {
        if let Some(user) = form.selected.and_then(|index| form.identities.get(index)) {
            font::text(image, 12, 65, &user.label, theme.accent, 37);
        }
        lines(image, 79, &form.prompt, theme.text, 2);
        chrome::panel(image, (8, 105, 304, 21), theme.surface, theme.border);
        match form.phase {
            Phase::Prompt => {
                let count = form.masked_len();
                let mask = if count > 35 {
                    format!("<{}", "*".repeat(35))
                } else {
                    "*".repeat(count)
                };
                font::text(image, 12, 112, &mask, theme.text, 37);
            }
            Phase::Waiting => font::text(
                image,
                12,
                112,
                language.text("Waiting for PAM...", "Ожидание PAM...", "Esperando PAM..."),
                theme.muted,
                37,
            ),
            Phase::Retry => font::text(
                image,
                12,
                112,
                language.text("Enter: try again", "Enter: повторить", "Enter: reintentar"),
                theme.text,
                37,
            ),
            Phase::Finished => font::text(
                image,
                12,
                112,
                language.text(
                    "Request finished",
                    "Запрос завершён",
                    "Solicitud finalizada",
                ),
                theme.muted,
                37,
            ),
            Phase::Choosing => {}
        }
        font::text(
            image,
            12,
            134,
            &form.notice,
            if form.error { theme.error } else { theme.muted },
            37,
        );
    }
    chrome::bar(image, (0, 149, 320, 21), theme);
    rect(image, (0., 149., 320., 1.), theme.border);
    font::text(
        image,
        4,
        156,
        language.text("Enter: OK", "Enter: Да", "Enter: OK"),
        theme.text,
        12,
    );
    font::text(
        image,
        120,
        156,
        language.text("F1: Info", "F1: Инфо", "F1: Info"),
        theme.text,
        11,
    );
    font::text(
        image,
        224,
        156,
        language.text("Esc: Cancel", "Esc: Отмена", "Esc: Volver"),
        theme.text,
        12,
    );
}
#[cfg(test)]
#[path = "unit_tests/render.rs"]
mod tests;
