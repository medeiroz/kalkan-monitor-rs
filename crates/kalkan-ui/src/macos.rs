//! macOS application hooks that neither Slint nor winit expose.
//!
//! - [`activate`] brings the app to the front. Showing a hidden window from
//!   the menu-bar icon or from a second launch is not enough on its own: an
//!   app that is not active keeps its window behind the others.
//! - [`install_reopen_handler`] makes a click on the Dock icon (or opening the
//!   app again from Finder or Spotlight, which macOS delivers to the running
//!   instance instead of starting a second one) show the window again when
//!   it is hidden in the menu bar.
//!
//! Both talk to the Objective-C runtime through a few raw calls, so no binding
//! crate is needed. They are best effort: when something is missing they do
//! nothing, and the menu-bar icon still opens the window.

use std::ffi::{c_char, c_void};
use std::sync::OnceLock;

#[link(name = "objc")]
extern "C" {
    fn objc_getClass(name: *const c_char) -> *mut c_void;
    fn sel_registerName(name: *const c_char) -> *mut c_void;
    fn object_getClass(object: *mut c_void) -> *mut c_void;
    fn class_addMethod(class: *mut c_void, name: *mut c_void, imp: *const c_void, types: *const c_char) -> i8;
    /// Always called through a cast to the real signature (see [`msg_send0`]).
    fn objc_msgSend();
}

// Makes sure AppKit (and so `NSApplication`) is loaded.
#[link(name = "AppKit", kind = "framework")]
extern "C" {}

fn sel(name: &'static [u8]) -> *mut c_void {
    debug_assert!(name.ends_with(b"\0"));
    // SAFETY: `name` is a NUL-terminated static string.
    unsafe { sel_registerName(name.as_ptr() as *const c_char) }
}

/// `[receiver selector]` for a selector without arguments that returns an object.
///
/// # Safety
/// `receiver` must be a valid object (or null) that implements `selector`.
unsafe fn msg_send0(receiver: *mut c_void, selector: *mut c_void) -> *mut c_void {
    let f: unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void =
        std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
    f(receiver, selector)
}

/// `[NSApplication sharedApplication]`.
fn shared_app() -> *mut c_void {
    // SAFETY: `NSApplication` is a class object and answers `sharedApplication`.
    unsafe {
        let class = objc_getClass(b"NSApplication\0".as_ptr() as *const c_char);
        if class.is_null() {
            return std::ptr::null_mut();
        }
        msg_send0(class, sel(b"sharedApplication\0"))
    }
}

/// Makes the app the active one (`[NSApp activateIgnoringOtherApps:YES]`), so
/// a window shown from the background comes to the front. Call it from the
/// main thread.
pub fn activate() {
    let app = shared_app();
    if app.is_null() {
        return;
    }
    // SAFETY: `app` is the shared NSApplication; the selector takes one BOOL.
    unsafe {
        let f: unsafe extern "C" fn(*mut c_void, *mut c_void, i8) =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        f(app, sel(b"activateIgnoringOtherApps:\0"), 1);
    }
}

static ON_REOPEN: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

/// `-[NSApplicationDelegate applicationShouldHandleReopen:hasVisibleWindows:]`.
extern "C" fn should_handle_reopen(_this: *mut c_void, _cmd: *mut c_void, _app: *mut c_void, has_visible: i8) -> i8 {
    if has_visible != 0 {
        return 1; // a window is already showing: the default behavior
    }
    if let Some(show) = ON_REOPEN.get() {
        show();
    }
    0
}

/// Calls `on_reopen` when the user clicks the Dock icon (or reopens the app)
/// while no window is visible. Adds the delegate method to the application
/// delegate winit installed, so it must run after the event loop has started.
/// Returns whether the hook is in place.
pub fn install_reopen_handler(on_reopen: impl Fn() + Send + Sync + 'static) -> bool {
    if ON_REOPEN.set(Box::new(on_reopen)).is_err() {
        return false;
    }
    let app = shared_app();
    if app.is_null() {
        return false;
    }
    // SAFETY: `delegate` is the object NSApplication calls back; adding a
    // method to its class is what Objective-C categories do. The IMP has the
    // signature the selector's type encoding (`c@:@c`) describes. If the class
    // already implements the selector, `class_addMethod` refuses and nothing changes.
    unsafe {
        let delegate = msg_send0(app, sel(b"delegate\0"));
        if delegate.is_null() {
            return false;
        }
        let class = object_getClass(delegate);
        class_addMethod(
            class,
            sel(b"applicationShouldHandleReopen:hasVisibleWindows:\0"),
            should_handle_reopen as *const c_void,
            b"c@:@c\0".as_ptr() as *const c_char,
        ) != 0
    }
}
