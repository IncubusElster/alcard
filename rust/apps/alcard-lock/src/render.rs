//! Opaque 320x170 credential presentation. No entered response is rendered.
use crate::ui::{Form, Language, Phase};
use alcard_ui::{chrome, font, theme::Theme};
use tiny_skia::{FilterQuality, Pixmap, PixmapPaint, Transform};

pub const WIDTH: u32 = 320;
pub const HEIGHT: u32 = 170;
const FOOTER: i32 = 154;

fn shortened(value: &str, columns: usize) -> String {
    let mut chars = value.chars();
    let mut result: String = chars.by_ref().take(columns).collect();
    if chars.next().is_some() && columns > 0 {
        result.pop();
        result.push('…');
    }
    result
}

fn wrapped(value: &str, columns: usize) -> Vec<String> {
    let mut result = Vec::new();
    let mut line = String::new();
    let mut used = 0;
    for word in value.split_whitespace() {
        if used > 0 {
            if used + 1 + word.chars().count() <= columns {
                line.push(' ');
                used += 1;
            } else {
                result.push(std::mem::take(&mut line));
                used = 0;
            }
        }
        for ch in word.chars() {
            if used == columns {
                result.push(std::mem::take(&mut line));
                used = 0;
            }
            line.push(ch);
            used += 1;
        }
    }
    result.push(line);
    result
}

fn lines(image: &mut Pixmap, y: i32, text: &str, color: u32, rows: usize) {
    let lines = wrapped(text, 36);
    for (row, text) in lines.iter().take(rows).enumerate() {
        let value = if row + 1 == rows && lines.len() > rows {
            shortened(&format!("{text}…"), 36)
        } else {
            text.clone()
        };
        font::text(image, 16, y + row as i32 * 11, &value, color, 36);
    }
}

fn lock_emblem(image: &mut Pixmap, theme: &Theme) {
    chrome::panel(image, (152, 25, 16, 16), theme.background, theme.accent);
    chrome::rounded(image, (148, 36, 24, 17), 2, theme.accent);
    chrome::rounded(image, (158, 40, 4, 4), 1, theme.background);
    chrome::rounded(image, (159, 43, 2, 5), 0, theme.background);
}

fn clock(image: &mut Pixmap, value: &str, theme: &Theme) {
    // A deliberate integer 2x enlargement keeps every clock glyph crisp.
    if let Some(mut text) = Pixmap::new(40, 8) {
        font::text(&mut text, 0, 0, value, theme.text, 5);
        image.draw_pixmap(
            0,
            0,
            text.as_ref(),
            &PixmapPaint {
                quality: FilterQuality::Nearest,
                ..PixmapPaint::default()
            },
            Transform::from_row(2.0, 0.0, 0.0, 2.0, 120.0, 65.0),
            None,
        );
    }
}

fn footer(image: &mut Pixmap, form: &Form, language: Language, theme: &Theme) {
    chrome::bar(image, (0, FOOTER, WIDTH as i32, 16), theme);
    chrome::rounded(image, (0, FOOTER, WIDTH as i32, 1), 0, theme.border);
    let primary = match form.phase {
        Phase::Idle => language.text("Unlock", "Пароль", "Entrar"),
        Phase::Prompt => language.text("Unlock", "Вход", "Entrar"),
        _ => "",
    };
    if !primary.is_empty() {
        chrome::selection(image, (1, FOOTER + 1, 62, 15), theme);
        font::centered(image, 32, FOOTER + 4, primary, theme.text, 6);
    }
    if matches!(form.phase, Phase::Waiting | Phase::Prompt) {
        font::centered(
            image,
            288,
            FOOTER + 4,
            language.text("Back", "Назад", "Volver"),
            theme.text,
            6,
        );
    }
}

pub fn paint(
    image: &mut Pixmap,
    form: &Form,
    language: Language,
    theme: &Theme,
    now: &str,
    date: &str,
) {
    image.fill(chrome::rgb(theme.background));
    chrome::bar(image, (0, 0, WIDTH as i32, 18), theme);
    let title = match form.phase {
        Phase::Securing => language.text("Locking", "Блокировка", "Bloqueando"),
        Phase::Waiting | Phase::Prompt | Phase::Unlocking => {
            language.text("Unlock", "Разблокировка", "Desbloquear")
        }
        _ => language.text("Locked", "Заблокировано", "Bloqueado"),
    };
    font::text(image, 8, 5, title, theme.accent, 27);
    font::text(image, 272, 5, now, theme.text, 5);
    match form.phase {
        Phase::Idle | Phase::Securing | Phase::Unavailable => {
            lock_emblem(image, theme);
            clock(image, now, theme);
            font::centered(image, 160, 88, date, theme.muted, 20);
            font::centered(image, 160, 107, &shortened(&form.owner, 36), theme.text, 36);
            let message = if !form.notice.is_empty() {
                form.notice.as_str()
            } else if form.phase == Phase::Securing {
                language.text(
                    "Securing the session…",
                    "Блокировка сеанса…",
                    "Protegiendo la sesión…",
                )
            } else {
                language.text(
                    "Your applications remain open.",
                    "Приложения остаются открытыми.",
                    "Las aplicaciones siguen abiertas.",
                )
            };
            lines(
                image,
                127,
                message,
                if form.error {
                    theme.warning
                } else {
                    theme.muted
                },
                2,
            );
        }
        Phase::Prompt => {
            font::text(image, 16, 26, &shortened(&form.owner, 36), theme.text, 36);
            let prompt = match form.prompt.trim() {
                // Translate the standard PAM password label. Preserve every
                // other challenge verbatim so another factor is not mislabeled.
                "" | "Password" | "Password:" => {
                    language.text("Password:", "Пароль:", "Contraseña:")
                }
                _ => &form.prompt,
            };
            lines(image, 43, prompt, theme.muted, 3);
            chrome::panel(image, (16, 79, 288, 22), theme.surface, theme.accent);
            let mask = "*".repeat(form.masked_len().min(33));
            font::text(image, 24, 86, &mask, theme.text, 33);
            let layout = format!(
                "{}{} {}",
                if form.caps_lock { "CAPS  " } else { "" },
                language.text("Layout", "Раскладка", "Teclado"),
                form.layout.saturating_add(1)
            );
            font::text(image, 16, 109, &layout, theme.muted, 36);
            lines(
                image,
                124,
                &form.notice,
                if form.error {
                    theme.warning
                } else {
                    theme.muted
                },
                2,
            );
        }
        Phase::Waiting | Phase::Unlocking => {
            lock_emblem(image, theme);
            font::centered(image, 160, 73, &shortened(&form.owner, 36), theme.text, 36);
            let message = if form.phase == Phase::Unlocking {
                language.text("Unlocking…", "Разблокировка…", "Desbloqueando…")
            } else {
                language.text("Verifying…", "Проверка…", "Verificando…")
            };
            font::centered(image, 160, 96, message, theme.accent, 36);
            lines(
                image,
                122,
                &form.notice,
                if form.error {
                    theme.warning
                } else {
                    theme.muted
                },
                2,
            );
        }
    }
    footer(image, form, language, theme);
}

#[cfg(test)]
#[path = "unit_tests/render.rs"]
mod tests;
