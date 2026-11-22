//! 40x21 terminal layout; every visible UI label comes from alcard-l10n.
use crate::App;
use alcard_l10n::tr;
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};
use unicode_width::UnicodeWidthChar;

fn clipped(text: &str, max_columns: usize) -> String {
    let mut output = String::new();
    let mut columns = 0;
    for c in text.chars() {
        let width = UnicodeWidthChar::width(c).unwrap_or(0);
        if columns + width > max_columns {
            break;
        }
        output.push(c);
        columns += width;
    }
    output
}

pub(crate) fn render(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();
    let locale = app.wizard.locale;
    let t = |key| tr(locale, key);
    if area.width < 40 || area.height < 21 {
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(t("app.too_small")),
                Line::from(t("app.resize")),
            ])
            .style(Style::default().fg(Color::Yellow)),
            area,
        );
        return;
    }

    let sections = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(area);

    let title = t("app.hello");
    frame.render_widget(
        Paragraph::new(title).style(Style::default().fg(Color::Cyan)),
        sections[0],
    );
    {
        let progress = format!(
            "{}/{}",
            app.wizard.step.index() + 1,
            alcard_setup_core::Step::ALL.len()
        );
        frame.render_widget(
            Paragraph::new(progress).style(Style::default().fg(Color::Gray)),
            Rect::new(area.x + 36, sections[0].y, 4, 1),
        );
    }
    frame.render_widget(
        Paragraph::new("────────────────────────────────────────")
            .style(Style::default().fg(Color::DarkGray)),
        sections[1],
    );

    let rows = {
        let mut lines = vec![
            Line::styled(
                clipped(app.wizard.title(), 40),
                Style::default().fg(Color::Cyan),
            ),
            Line::from(""),
        ];
        for row in app.wizard.rows() {
            let prefix = if row.selected { "> " } else { "  " };
            let color = if row.selected {
                Color::Yellow
            } else {
                Color::White
            };
            if let Some((secret, editing)) = app.wizard.revealed_secret(&row) {
                // Borrow plaintext only for the explicitly revealed terminal frame.
                // It is never copied into the public Row.text or setup JSON.
                let mut spans = vec![
                    Span::styled(
                        format!("{prefix}{}", clipped(&row.text, 38)),
                        Style::default().fg(color),
                    ),
                    Span::styled(secret, Style::default().fg(color)),
                ];
                if editing {
                    spans.push(Span::styled("_", Style::default().fg(color)));
                }
                lines.push(Line::from(spans));
            } else {
                lines.push(Line::styled(
                    format!("{prefix}{}", clipped(&row.text, 38)),
                    Style::default().fg(color),
                ));
            }
        }
        lines
    };
    frame.render_widget(Paragraph::new(rows), sections[2]);

    let status = if app.network_job.is_some() {
        t("s.wait")
    } else {
        app.wizard.status()
    };
    frame.render_widget(
        Paragraph::new(clipped(status, 40)).style(Style::default().fg(Color::Gray)),
        sections[3],
    );
    frame.render_widget(
        Paragraph::new("────────────────────────────────────────")
            .style(Style::default().fg(Color::DarkGray)),
        sections[4],
    );

    for i in 0..5 {
        let (label, enabled) = {
            let soft = app.wizard.softkeys()[i];
            if soft.enabled {
                (format!("{} {}", soft.number, t(soft.label_key)), true)
            } else {
                (String::new(), false)
            }
        };
        frame.render_widget(
            Paragraph::new(clipped(&label, 8)).style(Style::default().fg(if enabled {
                Color::Cyan
            } else {
                Color::DarkGray
            })),
            Rect::new(area.x + i as u16 * 8, sections[5].y, 8, 1),
        );
    }
}

#[cfg(test)]
#[path = "unit_tests/ui.rs"]
mod tests;
