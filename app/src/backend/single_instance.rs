//! Single instance: a second launch tells the running Lounge to come forward, then exits.
//!
//! The first instance takes an exclusive advisory lock on `<data dir>/instance.lock` (released by the
//! OS when the process dies, so a crash never leaves a stale lock; race-free even when two copies start
//! at the same moment), binds a TCP listener on 127.0.0.1 with an OS-chosen port and writes that port
//! to `<data dir>/instance.port`. A second instance fails to get the lock, reads the port, connects,
//! sends `show\n` and reports "not the first" so the caller can exit.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const LOCK_FILE: &str = "instance.lock";
const PORT_FILE: &str = "instance.port";
/// How long a second instance waits for a starting first instance to publish its port.
const NOTIFY_ATTEMPTS: u32 = 30;
const NOTIFY_RETRY: Duration = Duration::from_millis(100);

/// The running instance's claim. Dropping it releases the lock and stops the listener.
pub(super) struct Guard {
    _lock: Option<File>,
    port: u16,
    port_file: PathBuf,
    stop: Arc<AtomicBool>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = std::fs::remove_file(&self.port_file);
        // Wake the listener thread so it notices the stop flag.
        let _ = TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, self.port)), Duration::from_millis(200));
    }
}

/// Claim the single-instance slot for this user. Returns `true` if this is the only Lounge (and calls
/// `on_second` from a background thread whenever another launch asks it to come forward); returns
/// `false` if another instance is running, after telling it to come forward. The claim lasts for the
/// life of the process.
pub fn acquire(on_second: impl Fn() + Send + 'static) -> bool {
    match acquire_in(&super::paths::data_dir(), on_second) {
        Some(guard) => {
            // Held until the process exits.
            static HELD: Mutex<Vec<Guard>> = Mutex::new(Vec::new());
            HELD.lock().unwrap().push(guard);
            true
        }
        None => false,
    }
}

pub(super) fn acquire_in(dir: &Path, on_second: impl Fn() + Send + 'static) -> Option<Guard> {
    let _ = std::fs::create_dir_all(dir);
    let lock = match OpenOptions::new().create(true).truncate(false).write(true).open(dir.join(LOCK_FILE)) {
        Ok(f) => f,
        // Can't even open the lock file (read-only profile?): don't refuse to start over it.
        Err(_) => return bind_listener(None, dir, on_second),
    };
    match lock.try_lock() {
        Ok(()) => bind_listener(Some(lock), dir, on_second),
        Err(TryLockError::WouldBlock) => {
            notify_existing(dir);
            None
        }
        // Locking unsupported here: behave as the first instance.
        Err(TryLockError::Error(_)) => bind_listener(Some(lock), dir, on_second),
    }
}

fn bind_listener(lock: Option<File>, dir: &Path, on_second: impl Fn() + Send + 'static) -> Option<Guard> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).ok()?;
    let port = listener.local_addr().ok()?.port();
    let port_file = dir.join(PORT_FILE);
    let tmp = dir.join(format!("{PORT_FILE}.tmp"));
    if std::fs::write(&tmp, port.to_string()).and_then(|_| std::fs::rename(&tmp, &port_file)).is_err() {
        // Nobody could reach us, but we're still the only instance.
        let _ = std::fs::remove_file(&tmp);
    }
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            if thread_stop.load(Ordering::SeqCst) {
                break;
            }
            let Ok(stream) = stream else { continue };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
            let mut line = String::new();
            // Bounded: a stray connection can't make us buffer forever.
            if BufReader::new(stream.take(64)).read_line(&mut line).is_ok() && line.trim() == "show" {
                on_second();
            }
        }
    });
    Some(Guard { _lock: lock, port, port_file, stop })
}

/// Tell the instance that holds the lock to come forward. The first instance may still be starting up
/// (lock taken, port not yet published), so retry for a few seconds.
fn notify_existing(dir: &Path) -> bool {
    for _ in 0..NOTIFY_ATTEMPTS {
        if let Some(port) = std::fs::read_to_string(dir.join(PORT_FILE)).ok().and_then(|s| s.trim().parse::<u16>().ok()) {
            if let Ok(mut s) = TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)), Duration::from_millis(500)) {
                if s.write_all(b"show\n").is_ok() {
                    let _ = s.flush();
                    return true;
                }
            }
        }
        std::thread::sleep(NOTIFY_RETRY);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    #[test]
    fn second_instance_is_refused_and_wakes_the_first() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = channel();
        let first = acquire_in(dir.path(), move || {
            let _ = tx.send(());
        });
        assert!(first.is_some(), "the first instance gets the slot");
        assert!(acquire_in(dir.path(), || panic!("only the first instance is notified")).is_none(), "the second is refused");
        rx.recv_timeout(Duration::from_secs(5)).expect("first instance was told to show");
        assert!(acquire_in(dir.path(), || {}).is_none(), "still refused while the first lives");
        rx.recv_timeout(Duration::from_secs(5)).expect("and told again");

        drop(first);
        assert!(!dir.path().join(PORT_FILE).exists(), "port file removed on release");
        let again = acquire_in(dir.path(), || {});
        assert!(again.is_some(), "the slot is free again after the first exits");
    }

    #[test]
    fn stray_connections_do_not_wake_the_first() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = channel();
        let first = acquire_in(dir.path(), move || {
            let _ = tx.send(());
        })
        .unwrap();
        let mut s = TcpStream::connect((Ipv4Addr::LOCALHOST, first.port)).unwrap();
        s.write_all(b"hello\n").unwrap();
        drop(s);
        assert!(rx.recv_timeout(Duration::from_millis(500)).is_err());
    }
}
