//! Background threads that fail loudly instead of hanging hem.
//!
//! Every long-running job (indexing, view building, search, template
//! mining, merging) runs on its own thread, and its consumers wait for a
//! "complete" flag.  A panic in one of them used to kill the thread before
//! the flag was set, so plain mode waited forever and the TUI showed a view
//! that never finished.  Jobs are now started through [`spawn`]: a panic is
//! caught, its message is recorded here for the front end to report, and
//! the job's `on_panic` hook marks it finished so nothing waits on it.

use std::any::Any;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Mutex;
use std::thread::JoinHandle;

static FAILURE: Mutex<Option<String>> = Mutex::new(None);

/// Record a background failure.  The first one wins: later failures are
/// usually consequences of it.
pub fn record(message: impl Into<String>) {
    let mut f = FAILURE.lock().unwrap_or_else(|e| e.into_inner());
    if f.is_none() {
        *f = Some(message.into());
    }
}

/// The first background failure, if any thread has failed.
pub fn failure() -> Option<String> {
    FAILURE.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The text of a panic payload (`panic!("...")` or a formatted message).
pub fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

/// Spawn a named background thread.  If `f` panics, the failure is
/// recorded and `on_panic` runs on the same thread, so the job can mark
/// itself complete and wake whoever waits on it.
pub fn spawn<F, P>(name: &str, f: F, on_panic: P) -> std::io::Result<JoinHandle<()>>
where
    F: FnOnce() + Send + 'static,
    P: FnOnce() + Send + 'static,
{
    let thread_name = name.to_string();
    std::thread::Builder::new()
        .name(thread_name.clone())
        .spawn(move || {
            if let Err(p) = catch_unwind(AssertUnwindSafe(f)) {
                record(format!("{thread_name}: {}", panic_message(&*p)));
                on_panic();
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    #[test]
    fn panics_are_recorded_and_finish_the_job() {
        let done = Arc::new(AtomicBool::new(false));
        let d2 = done.clone();
        let h = spawn(
            "hem-test-crash",
            || panic!("boom at byte 15"),
            move || d2.store(true, Ordering::SeqCst),
        )
        .unwrap();
        h.join().unwrap();
        assert!(done.load(Ordering::SeqCst));
        assert!(failure().unwrap().contains("boom at byte 15"));
    }
}
