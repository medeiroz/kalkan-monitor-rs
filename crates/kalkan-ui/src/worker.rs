//! Device worker: owns the display on its own thread, so the blocking port
//! open/write never runs inside the UI's timer.
//!
//! The UI hands over the latest [`Fields`] through a one-slot "latest value
//! wins" mailbox; the worker opens the device (retrying while it is absent),
//! sends, and reports a [`Status`] back through a callback. Nothing here knows
//! about Slint: the callback is where the caller hops to the UI thread.

use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use anyhow::Result;
use kalkan_core::{Display, Fields};

/// Minimum time between attempts to open an absent display (the UI pushes one
/// value per second, so this means one attempt per tick, as before).
const OPEN_RETRY: Duration = Duration::from_millis(900);

/// Longest wait between attempts while a firmware 1.2 handshake keeps failing.
/// Each failed handshake can block this thread for up to 2 s, so repeated
/// failures back off (0.9, 1.8, 3.6, 5 s…). Other open errors (display absent,
/// permission denied) keep the plain [`OPEN_RETRY`] cadence, as before.
const MAX_BACKOFF: Duration = Duration::from_secs(5);

/// Wait before the next open attempt after `handshake_fails` consecutive
/// handshake failures.
fn retry_delay(handshake_fails: u32) -> Duration {
    match handshake_fails {
        0 => OPEN_RETRY,
        n => OPEN_RETRY.saturating_mul(1 << (n - 1).min(8)).min(MAX_BACKOFF),
    }
}

/// Connection state reported to the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Conn {
    Connected,
    Disconnected,
    Error,
}

/// What the UI should show after a worker step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub conn: Conn,
    /// New error text (`""` clears it); `None` leaves the current one.
    pub error: Option<String>,
}

/// The worker's decision logic, free of threads and clocks.
#[derive(Default)]
struct Link {
    device: Option<Box<dyn Display>>,
    last_open: Option<Instant>,
    /// Consecutive failed 1.2 handshakes (drives the backoff).
    handshake_fails: u32,
}

impl Link {
    /// Handles one value: (re)opens the device when needed, then sends.
    ///
    /// With no open device it tries to open on every value (like the original,
    /// which reconnects by itself when the cooler is plugged in), at most once
    /// per [`OPEN_RETRY`]. A write error drops the device; the next value tries
    /// again. Returns `None` when nothing changed (open attempt skipped).
    fn step(
        &mut self,
        f: &Fields,
        now: Instant,
        open: &mut impl FnMut() -> Result<Box<dyn Display>>,
    ) -> Option<Status> {
        let mut opened = false;
        if self.device.is_none() {
            let wait = retry_delay(self.handshake_fails);
            if self.last_open.is_some_and(|t| now.saturating_duration_since(t) < wait) {
                return None;
            }
            self.last_open = Some(now);
            match open() {
                Ok(d) => {
                    self.device = Some(d);
                    self.handshake_fails = 0;
                    opened = true;
                }
                Err(e) => {
                    if e.downcast_ref::<kalkan_core::HandshakeError>().is_some() {
                        self.handshake_fails = self.handshake_fails.saturating_add(1);
                    } else {
                        self.handshake_fails = 0;
                    }
                    return Some(Status { conn: Conn::Disconnected, error: Some(format!("{e}")) });
                }
            }
        }
        let device = self.device.as_mut()?;
        match device.send(f) {
            Ok(_) => Some(Status { conn: Conn::Connected, error: opened.then(String::new) }),
            Err(e) => {
                self.device = None;
                Some(Status { conn: Conn::Error, error: Some(format!("Failed to send: {e}")) })
            }
        }
    }
}

struct Slot {
    latest: Option<Fields>,
    closed: bool,
}

/// One-slot mailbox: a new value replaces an unread one.
struct Mailbox {
    slot: Mutex<Slot>,
    ready: Condvar,
}

impl Mailbox {
    fn lock(&self) -> MutexGuard<'_, Slot> {
        self.slot.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn put(&self, f: Fields) {
        self.lock().latest = Some(f);
        self.ready.notify_one();
    }

    fn close(&self) {
        self.lock().closed = true;
        self.ready.notify_one();
    }

    /// Blocks until a value arrives; `None` once closed.
    fn take(&self) -> Option<Fields> {
        let guard = self.lock();
        let mut guard = self
            .ready
            .wait_while(guard, |s| s.latest.is_none() && !s.closed)
            .unwrap_or_else(|e| e.into_inner());
        if guard.closed {
            return None;
        }
        guard.latest.take()
    }
}

/// Handle to the worker thread. Dropping it stops the thread.
pub struct Worker {
    mailbox: Arc<Mailbox>,
}

impl Worker {
    /// Starts the thread. `report` runs on the worker thread after each step
    /// that has something to show.
    pub fn spawn(report: impl Fn(Status) + Send + 'static) -> Self {
        Self::spawn_with(report, kalkan_core::open)
    }

    fn spawn_with(
        report: impl Fn(Status) + Send + 'static,
        mut open: impl FnMut() -> Result<Box<dyn Display>> + Send + 'static,
    ) -> Self {
        let mailbox = Arc::new(Mailbox {
            slot: Mutex::new(Slot { latest: None, closed: false }),
            ready: Condvar::new(),
        });
        let rx = mailbox.clone();
        let spawned = std::thread::Builder::new().name("kalkan-device".into()).spawn(move || {
            let mut link = Link::default();
            while let Some(f) = rx.take() {
                if let Some(status) = link.step(&f, Instant::now(), &mut open) {
                    report(status);
                }
            }
        });
        if let Err(e) = spawned {
            eprintln!("could not start the device worker: {e}");
        }
        Self { mailbox }
    }

    /// Hands the worker the latest values; an unsent older one is dropped.
    pub fn push(&self, f: Fields) {
        self.mailbox.put(f);
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.mailbox.close();
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use kalkan_core::{Firmware, Sent};
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::SeqCst};

    /// Display that records what it is sent and can fail on demand.
    struct Fake {
        sent: Arc<Mutex<Vec<Fields>>>,
        fail: Arc<AtomicBool>,
    }

    impl Display for Fake {
        fn firmware(&self) -> Firmware {
            Firmware::V1_3
        }
        fn description(&self) -> String {
            "fake".into()
        }
        fn send(&mut self, f: &Fields) -> Result<Sent> {
            if self.fail.load(SeqCst) {
                return Err(anyhow!("gone"));
            }
            self.sent.lock().unwrap().push(*f);
            Ok(Sent { raw: vec![], wire: vec![], index: None })
        }
    }

    /// Controls for a fake display and its opener.
    #[derive(Clone)]
    struct Rig {
        sent: Arc<Mutex<Vec<Fields>>>,
        fail: Arc<AtomicBool>,
        present: Arc<AtomicBool>,
        opens: Arc<AtomicU32>,
    }

    impl Rig {
        fn new() -> Self {
            Self {
                sent: Default::default(),
                fail: Default::default(),
                present: Arc::new(AtomicBool::new(true)),
                opens: Default::default(),
            }
        }

        fn open(&self) -> Result<Box<dyn Display>> {
            self.opens.fetch_add(1, SeqCst);
            if self.present.load(SeqCst) {
                Ok(Box::new(Fake { sent: self.sent.clone(), fail: self.fail.clone() }))
            } else {
                Err(anyhow!("no display"))
            }
        }

        fn sent(&self) -> Vec<Fields> {
            self.sent.lock().unwrap().clone()
        }
    }

    fn f(temp: u8) -> Fields {
        Fields { temp, fan: 1000, load: 0 }
    }

    #[test]
    fn opens_sends_and_clears_the_error_once() {
        let rig = Rig::new();
        let mut open = || rig.open();
        let mut link = Link::default();
        let t0 = Instant::now();
        let s = link.step(&f(40), t0, &mut open).unwrap();
        assert_eq!(s, Status { conn: Conn::Connected, error: Some(String::new()) });
        // Already open: no error change, no new open.
        let s = link.step(&f(41), t0 + Duration::from_secs(1), &mut open).unwrap();
        assert_eq!(s, Status { conn: Conn::Connected, error: None });
        assert_eq!(rig.opens.load(SeqCst), 1);
        assert_eq!(rig.sent(), vec![f(40), f(41)]);
    }

    #[test]
    fn absent_device_is_retried_at_most_once_per_interval() {
        let rig = Rig::new();
        rig.present.store(false, SeqCst);
        let mut open = || rig.open();
        let mut link = Link::default();
        let t0 = Instant::now();
        let s = link.step(&f(1), t0, &mut open).unwrap();
        assert_eq!(s, Status { conn: Conn::Disconnected, error: Some("no display".into()) });
        // Too soon: skipped, nothing reported.
        assert_eq!(link.step(&f(2), t0 + Duration::from_millis(300), &mut open), None);
        assert_eq!(rig.opens.load(SeqCst), 1);
        // A tick later it tries again, and connects once plugged in.
        rig.present.store(true, SeqCst);
        let s = link.step(&f(3), t0 + Duration::from_secs(1), &mut open).unwrap();
        assert_eq!(s.conn, Conn::Connected);
        assert_eq!(rig.opens.load(SeqCst), 2);
        assert_eq!(rig.sent(), vec![f(3)]);
    }

    #[test]
    fn write_error_drops_the_device_and_reopens_next_time() {
        let rig = Rig::new();
        let mut open = || rig.open();
        let mut link = Link::default();
        let t0 = Instant::now();
        link.step(&f(1), t0, &mut open).unwrap();
        rig.fail.store(true, SeqCst);
        let s = link.step(&f(2), t0 + Duration::from_secs(1), &mut open).unwrap();
        assert_eq!(s, Status { conn: Conn::Error, error: Some("Failed to send: gone".into()) });
        rig.fail.store(false, SeqCst);
        let s = link.step(&f(3), t0 + Duration::from_secs(2), &mut open).unwrap();
        assert_eq!(s, Status { conn: Conn::Connected, error: Some(String::new()) });
        assert_eq!(rig.opens.load(SeqCst), 2);
    }

    #[test]
    fn retry_delay_backs_off_up_to_the_cap() {
        assert_eq!(retry_delay(0), OPEN_RETRY);
        assert_eq!(retry_delay(1), OPEN_RETRY);
        assert_eq!(retry_delay(2), OPEN_RETRY * 2);
        assert_eq!(retry_delay(3), OPEN_RETRY * 4);
        assert_eq!(retry_delay(4), MAX_BACKOFF);
        assert_eq!(retry_delay(u32::MAX), MAX_BACKOFF);
    }

    #[test]
    fn failing_handshakes_back_off_and_success_resets() {
        let opens = AtomicU32::new(0);
        let ok = AtomicBool::new(false);
        let rig = Rig::new();
        let mut open = || -> Result<Box<dyn Display>> {
            opens.fetch_add(1, SeqCst);
            if ok.load(SeqCst) {
                rig.open()
            } else {
                Err(kalkan_core::HandshakeError("no reply".into()).into())
            }
        };
        let mut link = Link::default();
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let s = link.step(&f(1), at(0), &mut open).unwrap();
        assert_eq!(s.conn, Conn::Disconnected);
        assert!(s.error.unwrap().contains("handshake"));
        link.step(&f(1), at(1000), &mut open).unwrap(); // 1st retry after 0.9 s
        assert_eq!(opens.load(SeqCst), 2);
        // Now 2 failures: the next try waits 1.8 s.
        assert_eq!(link.step(&f(1), at(2000), &mut open), None);
        link.step(&f(1), at(2900), &mut open).unwrap();
        assert_eq!(opens.load(SeqCst), 3);
        // 3 failures: 3.6 s.
        assert_eq!(link.step(&f(1), at(6000), &mut open), None);
        ok.store(true, SeqCst);
        let s = link.step(&f(1), at(6500), &mut open).unwrap();
        assert_eq!(s.conn, Conn::Connected);
        assert_eq!(link.handshake_fails, 0);
    }

    #[test]
    fn other_open_errors_keep_the_one_second_cadence() {
        let rig = Rig::new();
        rig.present.store(false, SeqCst);
        let mut open = || rig.open();
        let mut link = Link::default();
        let t0 = Instant::now();
        for i in 0..5u64 {
            assert!(link.step(&f(1), t0 + Duration::from_secs(i), &mut open).is_some());
        }
        assert_eq!(rig.opens.load(SeqCst), 5);
    }

    #[test]
    fn mailbox_keeps_only_the_latest_value() {
        let m = Mailbox {
            slot: Mutex::new(Slot { latest: None, closed: false }),
            ready: Condvar::new(),
        };
        m.put(f(1));
        m.put(f(2));
        assert_eq!(m.take(), Some(f(2)));
        m.close();
        assert_eq!(m.take(), None);
    }

    #[test]
    fn worker_thread_reports_and_stops_on_drop() {
        use std::sync::mpsc;
        let (tx, rx) = mpsc::channel();
        let rig = Rig::new();
        let r = rig.clone();
        let w = Worker::spawn_with(
            move |s| {
                let _ = tx.send(s);
            },
            move || r.open(),
        );
        w.push(f(10));
        let s = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(s, Status { conn: Conn::Connected, error: Some(String::new()) });
        assert_eq!(rig.sent(), vec![f(10)]);
        drop(w);
        // The thread ends and drops the sender.
        assert!(rx.recv_timeout(Duration::from_secs(5)).is_err());
    }
}
