//! Ordered parallel scanning: process chunks `0..n` on a small thread pool
//! and hand results to a consumer **in order**, as soon as each prefix of
//! chunks is complete.  This gives background jobs streaming, in-order
//! output with multi-core throughput.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use parking_lot::Mutex;

/// Number of worker threads to use for background scans.
pub fn worker_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2)
        .clamp(1, 8)
}

struct Emitter<T> {
    pending: BTreeMap<usize, T>,
    next: usize,
}

/// Run `work(i)` for every `i in 0..n` across `threads` workers and call
/// `emit(i, result)` strictly in ascending `i` order.  Returns early if
/// `cancel` is set or `emit` returns `false`.
pub fn ordered_scan<T, W, E>(n: usize, threads: usize, cancel: &AtomicBool, work: W, emit: E)
where
    T: Send,
    W: Fn(usize) -> T + Sync,
    E: FnMut(usize, T) -> bool + Send,
{
    if n == 0 {
        return;
    }
    let threads = threads.clamp(1, n);
    let counter = AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    let emitter = Mutex::new(Emitter {
        pending: BTreeMap::new(),
        next: 0,
    });
    let emit = Mutex::new(emit);
    // Limit how far ahead workers may run so memory stays bounded.
    let window = threads * 2;

    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| loop {
                if stop.load(Ordering::Relaxed) || cancel.load(Ordering::Relaxed) {
                    return;
                }
                let i = counter.fetch_add(1, Ordering::Relaxed);
                if i >= n {
                    return;
                }
                // Back-pressure: wait until the emitter has caught up.
                while emitter.lock().next + window < i {
                    if stop.load(Ordering::Relaxed) || cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_micros(200));
                }
                let result = work(i);
                let mut e = emitter.lock();
                e.pending.insert(i, result);
                // Flush the contiguous prefix.
                loop {
                    // Never hand out more results once the consumer stopped.
                    if stop.load(Ordering::Relaxed) || cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    let idx = e.next;
                    let Some(r) = e.pending.remove(&idx) else {
                        break;
                    };
                    e.next += 1;
                    // Call emit while holding the emitter lock so ordering
                    // is preserved; emit is short (appends + wakeups).
                    let keep_going = (emit.lock())(idx, r);
                    if !keep_going {
                        stop.store(true, Ordering::Relaxed);
                        return;
                    }
                }
            });
        }
    });
}
