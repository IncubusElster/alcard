//! Validated firstboot request serialization into one zeroizing buffer.
use crate::{secrets::Slot, KeyboardLayout, Step, Wizard};
use alcard_l10n::Locale;
use alcard_setup_protocol::{
    is_valid_timezone, AccountPlan, KeyboardPlan, Language, Layout, SetupPlan,
};
use serde::Serialize;
use zeroize::Zeroizing;

impl Wizard {
    /// The explicit --hello/--firstboot entry point enables request generation.
    pub fn enable_firstboot(&mut self) {
        self.firstboot = true;
    }

    /// Serialize a borrowed secret payload directly into one zeroizing buffer.
    /// The public SetupPlan itself contains no passwords or password hashes.
    pub fn firstboot_request(&self) -> Result<Zeroizing<Vec<u8>>, &'static str> {
        if !self.firstboot || self.step != Step::Finish || self.editing.is_some() {
            return Err("firstboot_not_ready");
        }
        if !is_valid_timezone(&self.timezone)
            || self.username.is_empty()
            || self.passwords.validate(self.doas_for_user).is_err()
        {
            return Err("firstboot_validation_failed");
        }
        if !self.session_available(self.session) {
            return Err("selected_session_unavailable");
        }
        let locale = match self.locale {
            Locale::En => Language::En,
            Locale::Ru => Language::Ru,
            Locale::Es => Language::Es,
        };
        let layouts = self
            .keyboard_layouts
            .iter()
            .map(|layout| match layout {
                KeyboardLayout::Us => Layout::Us,
                KeyboardLayout::Ru => Layout::Ru,
                KeyboardLayout::Es => Layout::Es,
                KeyboardLayout::De => Layout::De,
            })
            .collect::<Vec<_>>();
        let plan = SetupPlan {
            schema_version: 1,
            language: locale,
            account: AccountPlan {
                username: self.username.clone(),
                display_name: self.display_name.clone(),
                doas_enabled: self.doas_for_user,
            },
            keyboard: KeyboardPlan { layouts },
            session: self.session,
            timezone: self.timezone.clone(),
            ntp_enabled: self.ntp_enabled,
        };
        #[derive(Serialize)]
        struct Credentials<'a> {
            user_password: &'a str,
            user_confirm: &'a str,
            root_password: &'a str,
            root_confirm: &'a str,
        }
        #[derive(Serialize)]
        struct Request<'a> {
            plan: SetupPlan,
            credentials: Credentials<'a>,
        }
        let credentials = Credentials {
            user_password: self.passwords.borrow_for_display(Slot::UserPassword),
            user_confirm: self.passwords.borrow_for_display(Slot::UserConfirm),
            root_password: self.passwords.borrow_for_display(Slot::RootPassword),
            root_confirm: self.passwords.borrow_for_display(Slot::RootConfirm),
        };
        serde_json::to_vec(&Request { plan, credentials })
            .map(Zeroizing::new)
            .map_err(|_| "firstboot_serialization_failed")
    }
}
