//! Coalesced, single-writer persistence of the iCloud session.
//!
//! The iCloud client reports a changed session on every response that rotates
//! a cookie — a burst of a dozen within two seconds during sign-in. The latest
//! snapshot is parked here and one background thread writes it after a quiet
//! moment, then keeps draining until nothing new arrived.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use wattdrive_infrastructure::icloud::SavedSession;
use wattdrive_infrastructure::session_store::SessionStore;

const QUIET_PERIOD: Duration = Duration::from_millis(1500);

static STORE: OnceLock<Arc<SessionStore>> = OnceLock::new();
static PENDING: Mutex<Option<SavedSession>> = Mutex::new(None);
static WRITER_RUNNING: AtomicBool = AtomicBool::new(false);

/// Where sessions get written. Called once at startup.
pub fn init(store: Arc<SessionStore>) {
    let _ = STORE.set(store);
}

/// Remember `session` as the version to persist; starts the writer if idle.
pub fn queue(session: SavedSession) {
    if let Ok(mut slot) = PENDING.lock() {
        *slot = Some(session);
    }
    if WRITER_RUNNING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        std::thread::spawn(drain);
    }
}

fn take_pending() -> Option<SavedSession> {
    PENDING.lock().ok().and_then(|mut slot| slot.take())
}

fn drain() {
    loop {
        std::thread::sleep(QUIET_PERIOD);
        let Some(session) = take_pending() else {
            break;
        };
        match STORE.get() {
            Some(store) => {
                if let Err(e) = store.save_session(&session) {
                    tracing::warn!("could not save session: {e}");
                }
            }
            None => tracing::warn!("session saver used before init"),
        }
    }
    WRITER_RUNNING.store(false, Ordering::SeqCst);
    // A snapshot queued between the last take and the flag reset would be
    // stranded; hand it to a fresh writer.
    if PENDING.lock().ok().is_some_and(|s| s.is_some())
        && WRITER_RUNNING
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    {
        std::thread::spawn(drain);
    }
}
