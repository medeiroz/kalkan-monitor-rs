//! Single instance (Windows).
//!
//! The app can already be running hidden in the tray (e.g. started at logon)
//! when the user opens it again from the Start menu. The second instance must
//! not fight the first one for the display's serial port: it signals a named
//! event and exits, and the first instance shows its window.
//!
//! The check runs before the UAC elevation, so reopening never prompts. The
//! event is created by the (elevated) first instance with a security
//! descriptor that lets non-elevated processes signal it.

use core::ffi::c_void;

const EVENT_NAME: &str = "Local\\KalkanMonitor.Show";
const EVENT_MODIFY_STATE: u32 = 0x0002;
const INFINITE: u32 = 0xFFFF_FFFF;
const WAIT_OBJECT_0: u32 = 0;
/// Everyone may signal it; low integrity label so non-elevated callers can too.
const EVENT_SDDL: &str = "D:(A;;GA;;;WD)S:(ML;;NW;;;LW)";

#[repr(C)]
struct SecurityAttributes {
    n_length: u32,
    security_descriptor: *mut c_void,
    inherit_handle: i32,
}

#[link(name = "kernel32")]
extern "system" {
    fn OpenEventW(access: u32, inherit: i32, name: *const u16) -> *mut c_void;
    fn CreateEventW(
        attrs: *mut SecurityAttributes,
        manual_reset: i32,
        initial_state: i32,
        name: *const u16,
    ) -> *mut c_void;
    fn SetEvent(h: *mut c_void) -> i32;
    fn WaitForSingleObject(h: *mut c_void, ms: u32) -> u32;
    fn CloseHandle(h: *mut c_void) -> i32;
    fn LocalFree(mem: *mut c_void) -> *mut c_void;
}
#[link(name = "advapi32")]
extern "system" {
    fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
        sddl: *const u16,
        revision: u32,
        sd: *mut *mut c_void,
        size: *mut u32,
    ) -> i32;
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// If another instance is running, asks it to show its window and returns true.
pub fn signal_existing() -> bool {
    let name = wide(EVENT_NAME);
    // SAFETY: plain FFI calls; the handle is closed after use.
    unsafe {
        let h = OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr());
        if h.is_null() {
            return false;
        }
        SetEvent(h);
        CloseHandle(h);
    }
    true
}

/// Claims the instance and calls `on_show` (from a background thread) each
/// time another instance asks for the window.
pub fn listen(on_show: impl Fn() + Send + 'static) {
    let name = wide(EVENT_NAME);
    let sddl = wide(EVENT_SDDL);
    // SAFETY: plain FFI calls; the security descriptor is freed after the
    // event is created, and the handle lives for the rest of the process.
    let h = unsafe {
        let mut sd: *mut c_void = core::ptr::null_mut();
        let mut attrs = SecurityAttributes {
            n_length: core::mem::size_of::<SecurityAttributes>() as u32,
            security_descriptor: core::ptr::null_mut(),
            inherit_handle: 0,
        };
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut sd,
            core::ptr::null_mut(),
        ) != 0
        {
            attrs.security_descriptor = sd;
        }
        let h = CreateEventW(&mut attrs, 0, 0, name.as_ptr());
        if !sd.is_null() {
            LocalFree(sd);
        }
        h
    };
    if h.is_null() {
        return;
    }
    let h = h as usize; // raw handles aren't Send
    std::thread::spawn(move || loop {
        // SAFETY: the handle stays open for the whole process lifetime.
        if unsafe { WaitForSingleObject(h as *mut c_void, INFINITE) } != WAIT_OBJECT_0 {
            break;
        }
        on_show();
    });
}
