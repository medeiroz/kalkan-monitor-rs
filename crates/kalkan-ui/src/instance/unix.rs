//! Single instance (Linux and macOS): a Unix domain socket.
//!
//! The running instance listens on `$XDG_RUNTIME_DIR/kalkan-monitor.sock` on
//! Linux. Elsewhere (macOS, where `$TMPDIR` is already a per-user directory)
//! and when there is no runtime dir, it uses a per-user file name in the temp
//! dir. A second launch connects to it, which makes the first instance show
//! its window, and exits. A socket left behind by a crash refuses
//! connections, so it is detected and replaced.

use std::io::{self, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Numeric user id: the owner of `/proc/self` on Linux.
#[cfg(target_os = "linux")]
fn uid() -> Option<u32> {
    std::fs::metadata("/proc/self").ok().map(|m| m.uid())
}

/// Numeric user id (macOS has no `/proc`).
#[cfg(not(target_os = "linux"))]
fn uid() -> Option<u32> {
    extern "C" {
        fn getuid() -> u32;
    }
    // SAFETY: `getuid` takes no arguments, cannot fail and touches no memory.
    Some(unsafe { getuid() })
}

/// Socket location: the per-user runtime dir when there is one, else a
/// per-user file name in the temp dir.
fn socket_path_in(runtime_dir: Option<PathBuf>, temp_dir: PathBuf, uid: Option<u32>) -> PathBuf {
    match runtime_dir {
        Some(dir) => dir.join("kalkan-monitor.sock"),
        None => temp_dir.join(format!("kalkan-monitor-{}.sock", uid.unwrap_or(0))),
    }
}

fn socket_path() -> PathBuf {
    // Only Linux has a runtime dir; on macOS the temp dir (`$TMPDIR`) is private to the user.
    #[cfg(target_os = "linux")]
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|p| p.is_dir());
    #[cfg(not(target_os = "linux"))]
    let runtime = None;
    socket_path_in(runtime, std::env::temp_dir(), uid())
}

/// Asks the instance listening on `path` to show its window.
fn signal_at(path: &Path) -> bool {
    // A socket in a shared directory must be ours, not planted by someone else.
    if let (Ok(meta), Some(me)) = (std::fs::metadata(path), uid()) {
        if meta.uid() != me {
            return false;
        }
    }
    match UnixStream::connect(path) {
        Ok(mut s) => {
            let _ = s.set_write_timeout(Some(Duration::from_secs(1)));
            let _ = s.write_all(b"show\n");
            true
        }
        Err(_) => false,
    }
}

/// Binds the socket, replacing a stale one. `Ok(None)` means another live
/// instance owns it.
fn bind_at(path: &Path) -> io::Result<Option<UnixListener>> {
    match UnixListener::bind(path) {
        Ok(l) => Ok(Some(l)),
        Err(e) if e.kind() == io::ErrorKind::AddrInUse => {
            if UnixStream::connect(path).is_ok() {
                return Ok(None); // someone else is listening
            }
            std::fs::remove_file(path)?; // leftover of a crashed instance
            UnixListener::bind(path).map(Some)
        }
        Err(e) => Err(e),
    }
}

fn listen_at(path: &Path, on_show: impl Fn() + Send + 'static) {
    let Ok(Some(listener)) = bind_at(path) else { return };
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            if conn.is_ok() {
                on_show();
            }
        }
    });
}

/// If another instance is running, asks it to show its window and returns true.
pub fn signal_existing() -> bool {
    signal_at(&socket_path())
}

/// Claims the instance and calls `on_show` (from a background thread) each
/// time another instance asks for the window.
pub fn listen(on_show: impl Fn() + Send + 'static) {
    listen_at(&socket_path(), on_show);
}

/// Removes the socket on a clean exit (a crash leaves it; see [`bind_at`]).
pub fn release() {
    let _ = std::fs::remove_file(socket_path());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn temp_socket(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("kalkan-test-{}-{name}.sock", std::process::id()))
    }

    #[test]
    fn path_prefers_the_runtime_dir() {
        let p = socket_path_in(Some("/run/user/1000".into()), "/tmp".into(), Some(1000));
        assert_eq!(p, PathBuf::from("/run/user/1000/kalkan-monitor.sock"));
        let p = socket_path_in(None, "/tmp".into(), Some(1000));
        assert_eq!(p, PathBuf::from("/tmp/kalkan-monitor-1000.sock"));
    }

    #[test]
    fn second_launch_reaches_the_first() {
        let path = temp_socket("live");
        let _ = std::fs::remove_file(&path);
        assert!(!signal_at(&path)); // nobody there yet
        let (tx, rx) = mpsc::channel();
        let tx = std::sync::Mutex::new(tx);
        listen_at(&path, move || {
            let _ = tx.lock().unwrap().send(());
        });
        assert!(signal_at(&path));
        rx.recv_timeout(Duration::from_secs(2)).expect("on_show was not called");
        // A third instance sees the socket as taken and does not replace it.
        assert!(bind_at(&path).unwrap().is_none());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn stale_socket_is_replaced() {
        let path = temp_socket("stale");
        let _ = std::fs::remove_file(&path);
        drop(UnixListener::bind(&path).unwrap()); // the file stays, nobody listens
        assert!(path.exists());
        assert!(!signal_at(&path));
        let (tx, rx) = mpsc::channel();
        let tx = std::sync::Mutex::new(tx);
        listen_at(&path, move || {
            let _ = tx.lock().unwrap().send(());
        });
        assert!(signal_at(&path));
        rx.recv_timeout(Duration::from_secs(2)).expect("on_show was not called");
        let _ = std::fs::remove_file(&path);
    }
}
