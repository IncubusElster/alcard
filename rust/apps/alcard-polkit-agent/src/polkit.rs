//! Minimal binding to the stable libpolkit identity/session C API.
//!
//! GObject owns every native reference and signal connection. These handles stay
//! on the native GLib worker thread; libpolkit-agent still owns the trusted PAM
//! helper, authentication protocol and authorization result.
use glib::{
    prelude::*,
    translate::{from_glib_full, from_glib_none},
};
use std::{
    ffi::{c_char, c_void},
    marker::PhantomData,
    rc::Rc,
};
use zeroize::Zeroizing;

unsafe extern "C" {
    fn polkit_unix_user_new(uid: i32) -> *mut c_void;
    fn polkit_unix_user_get_name(user: *mut c_void) -> *const c_char;
    fn polkit_unix_session_new_for_process_sync(
        pid: i32,
        cancellable: *mut gio::ffi::GCancellable,
        error: *mut *mut glib::ffi::GError,
    ) -> *mut c_void;
    fn polkit_unix_session_get_session_id(session: *mut c_void) -> *const c_char;
    fn polkit_agent_session_new(identity: *mut c_void, cookie: *const c_char) -> *mut c_void;
    fn polkit_agent_session_initiate(session: *mut c_void);
    fn polkit_agent_session_response(session: *mut c_void, response: *const c_char);
    fn polkit_agent_session_cancel(session: *mut c_void);
}

pub fn login_session_id(pid: i32) -> Result<glib::GString, glib::Error> {
    let mut error = std::ptr::null_mut();
    // SAFETY: this synchronous native constructor accepts a PID and an
    // optional cancellable; on failure it transfers a GError, on success a
    // PolkitUnixSession reference. Session resolution remains in libpolkit.
    let pointer =
        unsafe { polkit_unix_session_new_for_process_sync(pid, std::ptr::null_mut(), &mut error) };
    if !error.is_null() {
        return Err(unsafe { from_glib_full(error) });
    }
    let unavailable =
        || glib::Error::new(gio::IOErrorEnum::Failed, "No authenticated login session");
    if pointer.is_null() {
        return Err(unavailable());
    }
    // SAFETY: the successful constructor transfers exactly one GObject
    // reference. Copy the borrowed ID while the object is still live.
    let object: glib::Object =
        unsafe { from_glib_full(pointer.cast::<glib::gobject_ffi::GObject>()) };
    let id: Option<glib::GString> =
        unsafe { from_glib_none(polkit_unix_session_get_session_id(object.as_ptr().cast())) };
    id.ok_or_else(unavailable)
}

#[derive(Clone)]
pub struct UnixUser {
    object: glib::Object,
    _thread: PhantomData<Rc<()>>,
}

impl UnixUser {
    pub fn new(uid: i32) -> Option<Self> {
        // SAFETY: the constructor accepts a validated nonnegative UID and
        // transfers its newly allocated GObject reference to the caller.
        let pointer = unsafe { polkit_unix_user_new(uid) };
        if pointer.is_null() {
            return None;
        }
        Some(Self {
            object: unsafe { from_glib_full(pointer.cast::<glib::gobject_ffi::GObject>()) },
            _thread: PhantomData,
        })
    }

    pub fn name(&self) -> Option<glib::GString> {
        // SAFETY: this live object was created by polkit_unix_user_new. The
        // returned, nullable borrowed name is copied before returning to Rust.
        unsafe { from_glib_none(polkit_unix_user_get_name(self.object.as_ptr().cast())) }
    }
}

#[derive(Clone)]
pub struct Session {
    object: glib::Object,
    _thread: PhantomData<Rc<()>>,
}

fn c_string(value: &str) -> Option<Zeroizing<Vec<u8>>> {
    if value.as_bytes().contains(&0) {
        return None;
    }
    let mut bytes = Zeroizing::new(Vec::with_capacity(value.len() + 1));
    bytes.extend_from_slice(value.as_bytes());
    bytes.push(0);
    Some(bytes)
}

impl Session {
    pub fn new(identity: &UnixUser, cookie: &str) -> Option<Self> {
        let cookie = c_string(cookie)?;
        // SAFETY: the identity is a live PolkitUnixUser implementing
        // PolkitIdentity. libpolkit copies the NUL-terminated cookie and returns
        // a new session reference; the temporary copy is wiped on return.
        let pointer = unsafe {
            polkit_agent_session_new(identity.object.as_ptr().cast(), cookie.as_ptr().cast())
        };
        if pointer.is_null() {
            return None;
        }
        Some(Self {
            object: unsafe { from_glib_full(pointer.cast::<glib::gobject_ffi::GObject>()) },
            _thread: PhantomData,
        })
    }

    pub fn initiate(&self) {
        // SAFETY: this is a live session on its owning GLib context thread.
        unsafe { polkit_agent_session_initiate(self.object.as_ptr().cast()) }
    }

    pub fn response(&self, response: &str) {
        let Some(response) = c_string(response) else {
            self.cancel();
            return;
        };
        // SAFETY: libpolkit synchronously writes the NUL-terminated response to
        // its helper pipe. The temporary Rust allocation is wiped immediately.
        unsafe {
            polkit_agent_session_response(self.object.as_ptr().cast(), response.as_ptr().cast())
        }
    }

    pub fn cancel(&self) {
        // SAFETY: libpolkit accepts cancellation of a live session, including
        // an already completed session; it emits completion at most once.
        unsafe { polkit_agent_session_cancel(self.object.as_ptr().cast()) }
    }

    pub fn connect_request(&self, callback: impl Fn(&str, bool) + 'static) {
        self.object.connect_local("request", false, move |values| {
            callback(
                values[1].get::<&str>().expect("libpolkit request text"),
                values[2].get::<bool>().expect("libpolkit request echo"),
            );
            None
        });
    }

    pub fn connect_show_info(&self, callback: impl Fn(&str) + 'static) {
        self.connect_text("show-info", callback);
    }

    pub fn connect_show_error(&self, callback: impl Fn(&str) + 'static) {
        self.connect_text("show-error", callback);
    }

    fn connect_text(&self, signal: &str, callback: impl Fn(&str) + 'static) {
        self.object.connect_local(signal, false, move |values| {
            callback(values[1].get::<&str>().expect("libpolkit message text"));
            None
        });
    }

    pub fn connect_completed(&self, callback: impl Fn(bool) + 'static) {
        self.object
            .connect_local("completed", false, move |values| {
                callback(
                    values[1]
                        .get::<bool>()
                        .expect("libpolkit authorization result"),
                );
                None
            });
    }
}
