//! Single instance: a second launch must not fight the first one for the
//! display's serial port. It asks the running instance to show its window and
//! exits. Windows uses a named event, Linux and macOS a Unix domain socket.

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{listen, signal_existing};

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod unix;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use unix::{listen, release, signal_existing};
