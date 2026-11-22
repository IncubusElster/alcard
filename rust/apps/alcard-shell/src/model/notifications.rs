//! Notification history reconciliation and generation-bound interactions.

use super::{
    scroll_position, Action, Dialog, InfoKind, Key, Notice, NotificationOption, NotificationView,
    Request, Screen, Shell, NOTICE_ROWS,
};

impl Shell {
    pub fn sync_notices(&mut self, notices: Vec<Notice>) {
        let selected = if self.screen == Screen::Notifications {
            self.notices.get(self.selected).map(|notice| notice.id)
        } else {
            None
        };
        self.notices = notices;
        if self.screen == Screen::Notifications {
            self.selected = selected
                .and_then(|id| self.notices.iter().position(|notice| notice.id == id))
                .unwrap_or_else(|| self.selected.min(self.notices.len().saturating_sub(1)));
        }
        self.reconcile_notification_view();
    }

    pub fn unread_count(&self) -> usize {
        self.notices.iter().filter(|notice| notice.unread).count()
    }

    pub fn viewed_notice(&self) -> Option<&Notice> {
        let (id, generation) = match &self.notification_view {
            NotificationView::List => return None,
            NotificationView::Detail { id, generation, .. } => (*id, *generation),
            NotificationView::Options {
                id: Some(id),
                generation,
                ..
            } => (*id, *generation),
            NotificationView::Options { id: None, .. } => return None,
        };
        self.notices
            .iter()
            .find(|notice| notice.id == id && notice.generation == generation)
    }

    pub(super) fn notice_options(
        &self,
        id: Option<u32>,
        generation: u64,
    ) -> Vec<NotificationOption> {
        if self.notifications_unavailable {
            return vec![NotificationOption::Retry];
        }
        let mut entries = Vec::new();
        if let Some(notice) = self
            .notices
            .iter()
            .find(|n| Some(n.id) == id && n.generation == generation)
        {
            if notice.active {
                entries.extend(
                    notice
                        .actions
                        .iter()
                        .map(|action| NotificationOption::Invoke {
                            key: action.key.clone(),
                            label: action.label.clone(),
                        }),
                );
            }
            entries.push(NotificationOption::Delete);
        }
        if !self.notices.is_empty() {
            entries.push(NotificationOption::Clear);
        }
        entries
    }

    pub(super) fn notification_options_back(
        &mut self,
        id: Option<u32>,
        generation: u64,
        scroll: usize,
        from_detail: bool,
    ) {
        self.notification_view = if from_detail
            && self
                .notices
                .iter()
                .any(|n| Some(n.id) == id && n.generation == generation)
        {
            NotificationView::Detail {
                id: id.unwrap(),
                generation,
                scroll,
            }
        } else {
            NotificationView::List
        };
    }

    pub(super) fn reconcile_notification_view(&mut self) {
        match self.notification_view.clone() {
            NotificationView::List => {}
            NotificationView::Detail {
                id,
                generation,
                scroll,
            } => {
                let Some(notice) = self.viewed_notice() else {
                    self.notification_view = NotificationView::List;
                    return;
                };
                let scroll = scroll.min(
                    notice
                        .text_lines(self.locale)
                        .len()
                        .saturating_sub(NOTICE_ROWS),
                );
                self.notification_view = NotificationView::Detail {
                    id,
                    generation,
                    scroll,
                };
            }
            NotificationView::Options {
                id,
                generation,
                scroll,
                from_detail,
                entries,
                selected,
            } => {
                if id.is_some() && self.viewed_notice().is_none() {
                    self.notification_view = NotificationView::List;
                    return;
                }
                let next = self.notice_options(id, generation);
                // A removed action never transfers focus to Delete/Clear or to
                // another named action. Return to its originating view instead.
                if let Some(index) = entries
                    .get(selected)
                    .and_then(|old| next.iter().position(|item| item == old))
                {
                    self.notification_view = NotificationView::Options {
                        id,
                        generation,
                        scroll,
                        from_detail,
                        entries: next,
                        selected: index,
                    };
                } else {
                    self.notification_options_back(id, generation, scroll, from_detail);
                }
            }
        }
    }

    pub fn notification_service(&mut self, available: bool) {
        self.notifications_unavailable = !available;
        self.reconcile_notification_view();
    }

    pub fn notification_failed(&mut self) {
        self.open_dialog(Dialog::Info(InfoKind::NotificationError));
    }

    pub(super) fn open_notification_options(&mut self) {
        let (id, generation, scroll, from_detail) = match &self.notification_view {
            NotificationView::List => (
                self.notices.get(self.selected).map(|n| n.id),
                self.notices.get(self.selected).map_or(0, |n| n.generation),
                0,
                false,
            ),
            NotificationView::Detail {
                id,
                generation,
                scroll,
            } => (Some(*id), *generation, *scroll, true),
            NotificationView::Options { .. } => return,
        };
        let entries = self.notice_options(id, generation);
        if !entries.is_empty() {
            self.notification_view = NotificationView::Options {
                id,
                generation,
                scroll,
                from_detail,
                entries,
                selected: 0,
            };
        }
    }

    pub(super) fn notification_key(&mut self, key: Key) -> Request {
        let before = self.notification_view.clone();
        self.reconcile_notification_view();
        if before != self.notification_view {
            return Request::None;
        }
        if key == Key::Home {
            self.go_home();
            return Request::None;
        }
        let view = self.notification_view.clone();
        if matches!(key, Key::Up | Key::Down | Key::Left | Key::Right) {
            match &view {
                NotificationView::Detail { scroll, .. } => {
                    let maximum = self.viewed_notice().map_or(0, |notice| {
                        notice
                            .text_lines(self.locale)
                            .len()
                            .saturating_sub(NOTICE_ROWS)
                    });
                    let next = scroll_position(*scroll, maximum, key);
                    if let NotificationView::Detail { scroll, .. } = &mut self.notification_view {
                        *scroll = next;
                    }
                }
                NotificationView::Options {
                    entries, selected, ..
                } => {
                    let next = match key {
                        Key::Up => selected.saturating_sub(1),
                        Key::Down => (*selected + 1).min(entries.len().saturating_sub(1)),
                        _ => *selected,
                    };
                    if let NotificationView::Options { selected, .. } = &mut self.notification_view
                    {
                        *selected = next;
                    }
                }
                NotificationView::List => {}
            }
            return Request::None;
        }
        match (view, self.key_action(key)) {
            (NotificationView::Detail { id, .. }, Action::Back) => {
                self.notification_view = NotificationView::List;
                if let Some(index) = self.notices.iter().position(|notice| notice.id == id) {
                    self.selected = index;
                }
            }
            (
                NotificationView::Options {
                    id,
                    generation,
                    scroll,
                    from_detail,
                    ..
                },
                Action::Back,
            ) => self.notification_options_back(id, generation, scroll, from_detail),
            (NotificationView::Detail { .. }, Action::Actions) => self.open_notification_options(),
            (
                NotificationView::Options {
                    id,
                    generation,
                    entries,
                    selected,
                    ..
                },
                Action::Select,
            ) => match entries.get(selected) {
                Some(NotificationOption::Invoke { key, .. }) => {
                    if !self.notifications_unavailable
                        && self.viewed_notice().is_some_and(|notice| {
                            notice.active && notice.actions.iter().any(|action| action.key == *key)
                        })
                    {
                        return Request::InvokeNotification {
                            id: id.unwrap(),
                            generation,
                            action: key.clone(),
                        };
                    }
                }
                Some(NotificationOption::Delete)
                    if !self.notifications_unavailable && self.viewed_notice().is_some() =>
                {
                    return Request::DismissNotification(id.unwrap())
                }
                Some(NotificationOption::Clear)
                    if !self.notifications_unavailable && !self.notices.is_empty() =>
                {
                    self.open_dialog(Dialog::ConfirmClear)
                }
                Some(NotificationOption::Retry) if self.notifications_unavailable => {
                    return Request::RetryNotifications
                }
                _ => {}
            },
            _ => {}
        }
        Request::None
    }
}
