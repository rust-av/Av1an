use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Condvar,
    Mutex,
};

pub struct Semaphore {
    permit_count: AtomicUsize,
    signal:       Mutex<()>,
    condvar:      Condvar,
}

impl Semaphore {
    #[inline]
    pub fn new(initial_permits: usize) -> Self {
        Semaphore {
            permit_count: AtomicUsize::new(initial_permits),
            signal:       Mutex::new(()),
            condvar:      Condvar::new(),
        }
    }

    /// Acquire a permit and block until one is available.
    #[inline]
    pub fn acquire(&self) -> usize {
        loop {
            let current_count = self.permit_count.load(Ordering::SeqCst);
            if current_count > 0 {
                if let Ok(updated_count) = self.permit_count.compare_exchange(
                    current_count,
                    current_count - 1,
                    Ordering::SeqCst,
                    Ordering::Relaxed,
                ) {
                    // Semaphore acquired, return count as ID
                    return updated_count;
                }
            } else {
                // Block with Condvar
                let lock_guard = self.signal.lock().expect("Mutex poisoned");
                // In case count increases after lock
                if self.permit_count.load(Ordering::SeqCst) > 0 {
                    continue; // Loop back and try the atomic decrement path.
                }
                // Block until released and drop lock guard
                drop(self.condvar.wait(lock_guard).expect("Condvar poisoned"));
            }
        }
    }

    /// Releases a permit, allowing next acquire to succeed.
    #[inline]
    pub fn release(&self) {
        self.permit_count.fetch_add(1, Ordering::SeqCst);

        // Unblock Condvar
        drop(self.signal.lock().expect("Mutex poisoned"));
        self.condvar.notify_one();
    }
}
