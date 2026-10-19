//! Administrator rights on Windows.
//!
//! The sensor driver and the autostart scheduled task need an elevated
//! process. At startup, a non-elevated process relaunches itself through UAC
//! (`ShellExecuteW` with the `runas` verb, same arguments and working
//! directory) and exits. If the user declines the prompt, the app keeps
//! running degraded and the UI shows a warning. Set `KALKAN_NO_ELEVATE=1` to
//! skip all this (development). On other systems nothing happens.

/// What the startup check ended up with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Already elevated (or elevation is not needed/disabled): just run.
    Run,
    /// An elevated copy was started: this process must exit right away.
    Relaunched,
    /// Not elevated and the relaunch was declined or failed: run degraded.
    Degraded,
}

/// Decision logic. `relaunch` returns true when the elevated copy started.
/// It is called at most once, and only when not elevated and not disabled,
/// so a declined prompt never loops.
pub fn resolve(elevated: bool, disabled: bool, relaunch: impl FnOnce() -> bool) -> Outcome {
    if elevated || disabled {
        Outcome::Run
    } else if relaunch() {
        Outcome::Relaunched
    } else {
        Outcome::Degraded
    }
}

/// Quotes one argument following the `CommandLineToArgvW` rules, so paths and
/// values with spaces survive the relaunch.
pub fn quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '\x0b', '"']) {
        return arg.to_string();
    }
    let mut out = String::from("\"");
    let mut backslashes = 0usize;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.extend(std::iter::repeat('\\').take(backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                out.extend(std::iter::repeat('\\').take(backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    // Trailing backslashes must be doubled so they don't escape the closing quote.
    out.extend(std::iter::repeat('\\').take(backslashes * 2));
    out.push('"');
    out
}

/// Builds the parameter string of the relaunch from the original arguments.
pub fn build_params<I, S>(args: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    args.into_iter().map(|a| quote_arg(a.as_ref())).collect::<Vec<_>>().join(" ")
}

/// Startup check. Returns [`Outcome::Run`] on every non-Windows system.
#[cfg(windows)]
pub fn ensure() -> Outcome {
    let disabled = std::env::var_os("KALKAN_NO_ELEVATE").is_some();
    resolve(win::is_elevated(), disabled, win::relaunch_elevated)
}

#[cfg(not(windows))]
pub fn ensure() -> Outcome {
    Outcome::Run
}

#[cfg(windows)]
mod win {
    use core::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;

    const TOKEN_QUERY: u32 = 0x0008;
    const TOKEN_ELEVATION: i32 = 20;
    const SW_SHOWNORMAL: i32 = 1;

    #[link(name = "advapi32")]
    extern "system" {
        fn OpenProcessToken(process: *mut c_void, access: u32, token: *mut *mut c_void) -> i32;
        fn GetTokenInformation(
            token: *mut c_void,
            class: i32,
            info: *mut c_void,
            len: u32,
            ret_len: *mut u32,
        ) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> *mut c_void;
        fn CloseHandle(h: *mut c_void) -> i32;
    }
    #[link(name = "shell32")]
    extern "system" {
        fn ShellExecuteW(
            hwnd: *mut c_void,
            op: *const u16,
            file: *const u16,
            params: *const u16,
            dir: *const u16,
            show: i32,
        ) -> *mut c_void;
    }

    fn wide(s: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
        s.as_ref().encode_wide().chain(std::iter::once(0)).collect()
    }

    pub fn is_elevated() -> bool {
        // SAFETY: standard FFI calls; the token is closed after use.
        unsafe {
            let mut token: *mut c_void = core::ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return false;
            }
            let mut elev: u32 = 0;
            let mut ret_len: u32 = 0;
            let ok = GetTokenInformation(
                token,
                TOKEN_ELEVATION,
                &mut elev as *mut u32 as *mut c_void,
                4,
                &mut ret_len,
            ) != 0;
            CloseHandle(token);
            ok && elev != 0
        }
    }

    /// Starts an elevated copy of this exe with the same arguments and working
    /// directory. False when the user declines the UAC prompt (the call then
    /// returns an error code, e.g. ERROR_CANCELLED) or the launch fails.
    pub fn relaunch_elevated() -> bool {
        let Ok(exe) = std::env::current_exe() else { return false };
        let file = wide(&exe);
        let op = wide("runas");
        let params = wide(super::build_params(std::env::args().skip(1)));
        let dir = std::env::current_dir().ok().map(wide);
        // SAFETY: all pointers are NUL-terminated and outlive the call.
        let r = unsafe {
            ShellExecuteW(
                core::ptr::null_mut(),
                op.as_ptr(),
                file.as_ptr(),
                params.as_ptr(),
                dir.as_ref().map_or(core::ptr::null(), |d| d.as_ptr()),
                SW_SHOWNORMAL,
            )
        };
        // > 32 means success: the elevated instance took over.
        (r as isize) > 32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_args_are_not_quoted() {
        assert_eq!(quote_arg("--tray"), "--tray");
    }

    #[test]
    fn args_with_spaces_or_empty_are_quoted() {
        assert_eq!(quote_arg(r"C:\Program Files\Kalkan"), r#""C:\Program Files\Kalkan""#);
        assert_eq!(quote_arg(""), r#""""#);
    }

    #[test]
    fn quotes_and_trailing_backslashes_are_escaped() {
        assert_eq!(quote_arg(r#"a"b"#), r#""a\"b""#);
        assert_eq!(quote_arg(r"C:\dir with space\"), r#""C:\dir with space\\""#);
        assert_eq!(quote_arg(r#"x\"y"#), r#""x\\\"y""#);
    }

    #[test]
    fn params_keep_order_and_tray_flag() {
        assert_eq!(
            build_params(["--tray", r"C:\My Dir\cfg.json"]),
            r#"--tray "C:\My Dir\cfg.json""#
        );
        assert_eq!(build_params(Vec::<String>::new()), "");
    }

    #[test]
    fn elevated_runs_without_relaunching() {
        let o = resolve(true, false, || panic!("must not relaunch"));
        assert_eq!(o, Outcome::Run);
    }

    #[test]
    fn disabled_runs_without_relaunching() {
        let o = resolve(false, true, || panic!("must not relaunch"));
        assert_eq!(o, Outcome::Run);
    }

    #[test]
    fn not_elevated_relaunches_once() {
        let mut calls = 0;
        let o = resolve(false, false, || {
            calls += 1;
            true
        });
        assert_eq!((o, calls), (Outcome::Relaunched, 1));
    }

    #[test]
    fn declined_relaunch_runs_degraded() {
        assert_eq!(resolve(false, false, || false), Outcome::Degraded);
    }
}
