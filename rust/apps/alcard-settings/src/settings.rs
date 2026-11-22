//! Keyboard-driven settings frontend. Services run outside the render loop.
use alcard_input::{interpret, Intent};
use alcard_l10n::{tr, Locale};
use alcard_settings_core::pairing;
use alcard_settings_core::{
    preferences,
    settings::{self, Action, Operation, Page, Row},
};
use crossterm::{
    event::{self, Event, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    layout::{Constraint, Layout},
    style::{Color, Style},
    widgets::{List, ListItem, ListState, Paragraph, Wrap},
    DefaultTerminal, Frame,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::{
    io,
    process::Command,
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};
use zeroize::Zeroizing;

enum Dialog {
    List,
    Edit {
        prompt: String,
        operation: Operation,
        text: Zeroizing<String>,
        secret: bool,
    },
    Choose {
        prompt: String,
        operation: Operation,
        values: Vec<String>,
        index: usize,
    },
    Confirm {
        prompt: String,
        operation: Operation,
        value: String,
    },
    Detail(String),
    Pair {
        prompt: pairing::Prompt,
        reply: Option<pairing::Reply>,
        text: Zeroizing<String>,
    },
}
enum ResultEvent {
    Rows(io::Result<Vec<Row>>),
    Applied(io::Result<()>),
}
struct Browser {
    trail: Vec<(Page, usize)>,
    rows: Vec<Row>,
    dialog: Dialog,
    status: String,
    job: Option<Receiver<ResultEvent>>,
    loading: bool,
    leave_after_apply: bool,
    cancel_wifi: Option<Arc<AtomicBool>>,
    pairing: Option<pairing::Session>,
    locale: Locale,
}
fn locale() -> Locale {
    preferences::system_language()
        .and_then(Locale::from_code)
        .unwrap_or_else(super::cli_locale)
}

fn clean(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}
fn value(locale: Locale, s: &str) -> String {
    match s {
        "on" => tr(locale, "s.on"),
        "off" => tr(locale, "s.off"),
        "en" => tr(locale, "language.en"),
        "ru" => tr(locale, "language.ru"),
        "es" => tr(locale, "language.es"),
        _ => return clean(s),
    }
    .into()
}
fn label(locale: Locale, r: &Row) -> String {
    let name = if r.key.is_empty() {
        ""
    } else {
        tr(locale, r.key)
    };
    let v = if r.key == "s.syslang"
        || matches!(r.action, Action::Apply(_, _) | Action::Confirm(_, _))
    {
        value(locale, &r.value)
    } else {
        clean(&r.value)
    };
    match (name.is_empty(), v.is_empty()) {
        (true, _) => v,
        (_, true) => name.into(),
        _ => format!("{name}  {v}"),
    }
}
fn worker(f: impl FnOnce() -> ResultEvent + Send + 'static) -> Receiver<ResultEvent> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx
}
impl Browser {
    fn new() -> Self {
        Self {
            trail: vec![(Page::Home, 0)],
            rows: settings::rows(&Page::Home).expect("static home menu"),
            dialog: Dialog::List,
            status: String::new(),
            job: None,
            loading: false,
            leave_after_apply: false,
            cancel_wifi: None,
            pairing: None,
            locale: locale(),
        }
    }
    fn reload(&mut self) {
        let page = self.trail.last().unwrap().0.clone();
        self.loading = true;
        self.job = Some(worker(move || ResultEvent::Rows(settings::rows(&page))));
    }
    fn poll(&mut self) {
        if let Some(session) = &self.pairing {
            match session.events.try_recv() {
                Ok(pairing::Event::Prompt(prompt, reply)) => {
                    self.dialog = Dialog::Pair {
                        prompt,
                        reply,
                        text: Zeroizing::default(),
                    }
                }
                Ok(pairing::Event::PromptClosed) => {
                    if matches!(self.dialog, Dialog::Pair { .. }) {
                        self.dialog = Dialog::List;
                    }
                }
                Ok(pairing::Event::Done(result)) => {
                    self.pairing = None;
                    self.dialog = Dialog::List;
                    self.finished(result);
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.pairing = None;
                    self.dialog = Dialog::List;
                    self.finished(Err(io::Error::other("BlueZ agent closed")));
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let Some(result) = self.job.as_ref().and_then(|rx| match rx.try_recv() {
            Ok(e) => Some(e),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(_) => Some(ResultEvent::Applied(Err(io::Error::other(
                "Worker stopped",
            )))),
        }) else {
            return;
        };
        self.job = None;
        self.loading = false;
        self.cancel_wifi = None;
        match result {
            ResultEvent::Rows(Ok(rows)) => {
                self.rows = rows;
                let current = &mut self.trail.last_mut().unwrap().1;
                *current = (*current).min(self.rows.len().saturating_sub(1));
            }
            ResultEvent::Rows(Err(error)) => {
                self.rows.clear();
                self.status = self.problem(error);
            }
            ResultEvent::Applied(r) => self.finished(r),
        }
    }
    fn problem(&self, e: io::Error) -> String {
        format!(
            "{} · {}",
            tr(self.locale, "s.unavailable"),
            clean(&e.to_string())
        )
    }
    fn finished(&mut self, r: io::Result<()>) {
        if std::mem::take(&mut self.leave_after_apply) && r.is_ok() && self.trail.len() > 1 {
            self.trail.pop();
        }
        self.locale = locale();
        self.status = match r {
            Ok(()) => tr(self.locale, "s.saved").into(),
            Err(e) => self.problem(e),
        };
        self.reload();
    }
    fn execute(
        &mut self,
        op: Operation,
        data: Zeroizing<String>,
        terminal: &mut DefaultTerminal,
    ) -> io::Result<()> {
        self.leave_after_apply = matches!(
            &op,
            Operation::Profile {
                field: "forget",
                ..
            } | Operation::Bluetooth("remove", _)
        );
        if let Operation::Bluetooth("pair", addr) = op {
            self.pairing = Some(pairing::Session::start(addr));
            self.status = tr(self.locale, "s.wait").into();
        } else if let Some(mut cmd) = match op.terminal_command(&data) {
            Ok(cmd) => cmd,
            Err(e) => {
                self.finished(Err(e));
                return Ok(());
            }
        } {
            let result = external(terminal, &mut cmd);
            if result.is_ok() && matches!(op, Operation::System("language")) {
                settings::reload_shell();
            }
            self.finished(result);
        } else {
            if let Operation::Wifi { iface, ssid } = op {
                let cancel = Arc::new(AtomicBool::new(false));
                self.cancel_wifi = Some(cancel.clone());
                self.job = Some(worker(move || {
                    ResultEvent::Applied(alcard_settings_core::network::connect_until(
                        &iface, &ssid, &data, &cancel,
                    ))
                }));
                return Ok(());
            }
            self.loading = false;
            self.job = Some(worker(move || ResultEvent::Applied(op.apply(&data))));
        }
        Ok(())
    }
    fn handle(&mut self, intent: Intent, terminal: &mut DefaultTerminal) -> io::Result<bool> {
        if self.pairing.is_some() {
            if matches!(intent, Intent::Quit | Intent::CancelEdit | Intent::Soft(4)) {
                self.pairing = None;
                self.dialog = Dialog::List;
                self.status = tr(self.locale, "s.cancelled").into();
                self.reload();
            } else if let Dialog::Pair {
                prompt,
                reply,
                text,
            } = &mut self.dialog
            {
                match intent {
                    Intent::Insert(c)
                        if text.len() < 16
                            && (!matches!(prompt, pairing::Prompt::Passkey)
                                || c.is_ascii_digit()) =>
                    {
                        text.push(c)
                    }
                    Intent::Erase => {
                        text.pop();
                    }
                    Intent::CommitEdit | Intent::Soft(0) if reply.is_some() => {
                        let valid = match prompt {
                            pairing::Prompt::Pin => !text.is_empty() && text.len() <= 16,
                            pairing::Prompt::Passkey => !text.is_empty() && text.len() <= 6,
                            pairing::Prompt::Confirm(_) => true,
                            _ => false,
                        };
                        if valid {
                            if let Some(sender) = reply.take() {
                                let _ = sender.try_send(text.to_string());
                            }
                            self.dialog = Dialog::List;
                        }
                    }
                    _ => {}
                }
            }
            return Ok(false);
        }
        if self.job.is_some() {
            if matches!(intent, Intent::Soft(4) | Intent::Quit) {
                if self.loading {
                    self.job = None;
                    self.loading = false;
                    if self.trail.len() > 1 {
                        self.trail.pop();
                        self.reload();
                    } else {
                        return Ok(true);
                    }
                } else if let Some(cancel) = &self.cancel_wifi {
                    cancel.store(true, Ordering::Release);
                }
            }
            return Ok(false);
        }
        if intent == Intent::Quit {
            return Ok(true);
        }
        let mut execute = None;
        match &mut self.dialog {
            Dialog::List => {
                let (page, index) = self.trail.last_mut().unwrap();
                match intent {
                    Intent::Soft(4) => {
                        if *page == Page::Home {
                            return Ok(true);
                        }
                        self.trail.pop();
                        self.status.clear();
                        self.reload();
                    }
                    Intent::Soft(2) => {
                        self.status.clear();
                        self.reload();
                    }
                    Intent::FocusNext | Intent::Down => {
                        if !self.rows.is_empty() {
                            *index = (*index + 1) % self.rows.len();
                        }
                    }
                    Intent::Up => {
                        *index = if *index == 0 {
                            self.rows.len().saturating_sub(1)
                        } else {
                            *index - 1
                        };
                    }
                    Intent::Soft(0) | Intent::Soft(1) => {
                        let Some(r) = self.rows.get(*index) else {
                            if !self.status.is_empty() {
                                self.dialog = Dialog::Detail(self.status.clone());
                            }
                            return Ok(false);
                        };
                        let prompt = label(self.locale, r);
                        self.dialog = match r.action.clone() {
                            Action::Read => Dialog::Detail(prompt),
                            Action::Open(next) => {
                                self.trail.push((next, 0));
                                self.rows.clear();
                                self.status.clear();
                                self.reload();
                                Dialog::List
                            }
                            Action::Edit(operation) => Dialog::Edit {
                                prompt: tr(self.locale, r.key).into(),
                                operation,
                                text: Zeroizing::new(r.value.clone()),
                                secret: false,
                            },
                            Action::Password(operation) => Dialog::Edit {
                                prompt: format!("{} · {prompt}", tr(self.locale, "s.password")),
                                operation,
                                text: Zeroizing::new(String::new()),
                                secret: true,
                            },
                            Action::Select(operation, values) => {
                                let index = values.iter().position(|v| v == &r.value).unwrap_or(0);
                                Dialog::Choose {
                                    prompt,
                                    operation,
                                    values,
                                    index,
                                }
                            }
                            Action::Apply(op, data) => {
                                execute = Some((op, Zeroizing::new(data)));
                                Dialog::List
                            }
                            Action::Confirm(operation, value) => Dialog::Confirm {
                                prompt,
                                operation,
                                value,
                            },
                        };
                    }
                    _ => {}
                }
            }
            Dialog::Edit {
                operation, text, ..
            } => match intent {
                Intent::Insert(c) if text.len() + c.len_utf8() <= 4096 => text.push(c),
                Intent::Erase => {
                    text.pop();
                }
                Intent::CommitEdit => {
                    execute = Some((operation.clone(), std::mem::take(text)));
                    self.dialog = Dialog::List;
                }
                Intent::CancelEdit => self.dialog = Dialog::List,
                _ => {}
            },
            Dialog::Choose {
                operation,
                values,
                index,
                ..
            } => match intent {
                Intent::FocusNext | Intent::Down => *index = (*index + 1) % values.len(),
                Intent::Up => {
                    *index = if *index == 0 {
                        values.len() - 1
                    } else {
                        *index - 1
                    }
                }
                Intent::Soft(0) => {
                    execute = Some((operation.clone(), Zeroizing::new(values[*index].clone())));
                    self.dialog = Dialog::List;
                }
                Intent::Soft(4) => self.dialog = Dialog::List,
                _ => {}
            },
            Dialog::Confirm {
                operation, value, ..
            } => match intent {
                Intent::Soft(0) => {
                    execute = Some((operation.clone(), Zeroizing::new(std::mem::take(value))));
                    self.dialog = Dialog::List;
                }
                Intent::Soft(4) => self.dialog = Dialog::List,
                _ => {}
            },
            Dialog::Detail(_) => self.dialog = Dialog::List,
            Dialog::Pair { .. } => {}
        }
        if let Some((op, data)) = execute {
            self.execute(op, data, terminal)?;
        }
        Ok(false)
    }
    fn render(&self, frame: &mut Frame<'_>) {
        let t = |k| tr(self.locale, k);
        let (page, index) = self.trail.last().unwrap();
        let area = frame.area();
        let [title, body, status, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .areas(area);
        frame.render_widget(
            Paragraph::new(t(page.title())).style(Style::default().fg(Color::Cyan)),
            title,
        );
        let footer_key = match &self.dialog {
            Dialog::List => {
                let items = self
                    .rows
                    .iter()
                    .map(|r| ListItem::new(label(self.locale, r)));
                frame.render_stateful_widget(
                    List::new(items)
                        .highlight_symbol("› ")
                        .highlight_style(Style::default().fg(Color::Yellow)),
                    body,
                    &mut ListState::default().with_selected(Some(*index)),
                );
                if self.cancel_wifi.is_some() || self.pairing.is_some() {
                    "s.cancel_nav"
                } else {
                    "s.nav"
                }
            }
            Dialog::Edit {
                prompt,
                text,
                secret,
                ..
            } => {
                let shown = if *secret {
                    "•".repeat(text.chars().count())
                } else {
                    text.to_string()
                };
                frame.render_widget(
                    Paragraph::new(format!("{prompt}\n\n{shown}▏")).wrap(Wrap { trim: false }),
                    body,
                );
                "s.edit_nav"
            }
            Dialog::Choose {
                prompt,
                values,
                index,
                ..
            } => {
                let [heading, list] =
                    Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).areas(body);
                frame.render_widget(Paragraph::new(prompt.as_str()), heading);
                frame.render_stateful_widget(
                    List::new(values.iter().map(|s| value(self.locale, s)))
                        .highlight_symbol("› ")
                        .highlight_style(Style::default().fg(Color::Yellow)),
                    list,
                    &mut ListState::default().with_selected(Some(*index)),
                );
                "s.pick_nav"
            }
            Dialog::Confirm { prompt, .. } => {
                frame.render_widget(
                    Paragraph::new(format!("{}\n\n{prompt}", t("s.confirm")))
                        .wrap(Wrap { trim: false }),
                    body,
                );
                "s.edit_nav"
            }
            Dialog::Pair { prompt, text, .. } => {
                let shown = match prompt {
                    pairing::Prompt::Confirm(code) => {
                        format!("{}\n{}", t("s.btconfirm"), clean(code))
                    }
                    pairing::Prompt::Pin | pairing::Prompt::Passkey => {
                        format!("{}\n{}▏", t("s.btpin"), "•".repeat(text.chars().count()))
                    }
                    pairing::Prompt::Display(code) => format!("{}\n{}", t("s.btshow"), clean(code)),
                };
                frame.render_widget(Paragraph::new(shown).wrap(Wrap { trim: false }), body);
                "s.edit_nav"
            }
            Dialog::Detail(text) => {
                frame.render_widget(
                    Paragraph::new(text.as_str()).wrap(Wrap { trim: false }),
                    body,
                );
                "s.back_nav"
            }
        };
        // On the 40-column device a translated label can hide most of its value.
        let mut status_text = self.status.clone();
        if self.job.is_some() {
            status_text = t("s.wait").into();
        } else if matches!(self.dialog, Dialog::List) {
            if let Some(row) = self.rows.get(*index) {
                let label = ratatui::text::Line::from(label(self.locale, row));
                if label.width() + 2 > usize::from(body.width) && !row.value.is_empty() {
                    if !status_text.is_empty() {
                        status_text.push('\n');
                    }
                    status_text.push_str(&clean(&row.value));
                }
            }
        }
        frame.render_widget(
            Paragraph::new(status_text)
                .style(Style::default().fg(Color::Yellow))
                .wrap(Wrap { trim: false }),
            status,
        );
        frame.render_widget(
            Paragraph::new(t(footer_key)).style(Style::default().fg(Color::Cyan)),
            footer,
        );
    }
}
fn external(terminal: &mut DefaultTerminal, cmd: &mut Command) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    // A Ctrl+C at an authentication prompt belongs to that foreground command.
    // Caught signal handlers reset to the default in the child at exec.
    extern "C" fn interrupted(_: libc::c_int) {}
    let mut old = std::mem::MaybeUninit::<libc::sigaction>::uninit();
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction = interrupted as *const () as usize;
    action.sa_flags = libc::SA_RESTART;
    let changed = unsafe { libc::sigaction(libc::SIGINT, &action, old.as_mut_ptr()) } == 0;
    let result = cmd.status();
    if changed {
        unsafe {
            libc::sigaction(libc::SIGINT, old.as_ptr(), std::ptr::null_mut());
        }
    }

    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen)?;
    terminal.hide_cursor()?;
    let area = terminal.get_frame().area();
    terminal.resize(area)?;
    let status = result?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(status.to_string()))
    }
}
pub fn run(terminal: &mut DefaultTerminal) -> io::Result<()> {
    let mut browser = Browser::new();
    loop {
        browser.poll();
        terminal.draw(|f| browser.render(f))?;
        if (browser.job.is_some() || browser.pairing.is_some())
            && !event::poll(Duration::from_millis(120))?
        {
            continue;
        }
        let Event::Key(k) = event::read()? else {
            continue;
        };
        if k.kind == KeyEventKind::Release {
            continue;
        }
        if let Some(key) = super::map_key(k) {
            let intent = interpret(
                key,
                matches!(
                    browser.dialog,
                    Dialog::Edit { .. }
                        | Dialog::Pair {
                            prompt: pairing::Prompt::Pin | pairing::Prompt::Passkey,
                            ..
                        }
                ),
            );
            if browser.handle(intent, terminal)? {
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
#[path = "unit_tests/settings.rs"]
mod tests;
