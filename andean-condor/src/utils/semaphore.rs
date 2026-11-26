use std::sync::{Arc, Condvar, Mutex};

struct State {
    permits: usize,
    closed: bool,
    #[cfg(test)]
    waiters: usize,
}

pub struct Semaphore {
    state: Mutex<State>,
    condvar: Condvar,
    #[cfg(test)]
    waiting: Condvar,
}

impl Semaphore {
    #[inline]
    pub fn new(initial_permits: usize) -> Self {
        Semaphore {
            state: Mutex::new(State {
                permits: initial_permits,
                closed: false,
                #[cfg(test)]
                waiters: 0,
            }),
            condvar: Condvar::new(),
            #[cfg(test)]
            waiting: Condvar::new(),
        }
    }

    /// Acquire a permit and block until one is available.
    #[inline]
    pub fn acquire(&self) -> usize {
        let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        while state.permits == 0 {
            state = self.condvar.wait(state).unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        let id = state.permits;
        state.permits -= 1;
        id
    }

    /// Attempts to acquire a permit without blocking.
    ///
    /// Returns `Some(id)` if a permit was available and acquired,
    /// or `None` if no permits are currently available.
    #[inline]
    pub fn try_acquire(&self) -> Option<usize> {
        let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        (state.permits > 0).then(|| {
            let id = state.permits;
            state.permits -= 1;
            id
        })
    }

    /// Releases a permit, allowing next acquire to succeed.
    #[inline]
    pub fn release(&self) {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).permits += 1;
        self.condvar.notify_one();
    }

    /// Waits for a permit, returning `None` if closed or cancelled. Dropping
    /// the permit releases it, including during unwinding.
    #[inline]
    pub fn acquire_owned(
        self: &Arc<Self>,
        cancellation: &super::cancellation::Cancellation,
    ) -> Option<Permit> {
        let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        while state.permits == 0 && !state.closed && !cancellation.is_cancelled() {
            #[cfg(test)]
            {
                state.waiters += 1;
                self.waiting.notify_all();
            }
            state = self
                .condvar
                .wait_timeout(state, std::time::Duration::from_millis(10))
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
            #[cfg(test)]
            {
                state.waiters -= 1;
            }
        }
        if state.closed || cancellation.is_cancelled() {
            return None;
        }
        state.permits -= 1;
        Some(Permit {
            semaphore: Arc::clone(self),
        })
    }

    /// Stop pool admission and wake every owned acquirer. Raw notification
    /// acquire/try_acquire/release retain their consuming counting semantics.
    #[inline]
    pub fn close(&self) {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).closed = true;
        self.condvar.notify_all();
        #[cfg(test)]
        self.waiting.notify_all();
    }

    /// Test handshake: observe a real blocked owned acquirer, not elapsed time.
    #[cfg(test)]
    pub(crate) fn wait_for_waiter(&self) -> bool {
        let state = self.state.lock().expect("pool state");
        let state =
            self.waiting.wait_while(state, |s| s.waiters == 0 && !s.closed).expect("waiter");
        state.waiters > 0
    }
}

/// One pool admission, returned automatically when its owner exits.
pub struct Permit {
    semaphore: Arc<Semaphore>,
}

impl Drop for Permit {
    #[inline]
    fn drop(&mut self) {
        self.semaphore.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_permits_return_on_error_and_unwind_without_inflation() {
        let pool = Arc::new(Semaphore::new(1));
        let failed = || -> Result<(), ()> {
            let _permit = pool
                .acquire_owned(&crate::utils::cancellation::Cancellation::default())
                .expect("open pool");
            Err(())
        };
        assert!(failed().is_err());
        let panic = std::panic::catch_unwind(|| {
            let _permit = pool
                .acquire_owned(&crate::utils::cancellation::Cancellation::default())
                .expect("error returned its permit");
            panic!("permit owner panic");
        });
        assert!(panic.is_err());
        assert_eq!(pool.try_acquire(), Some(1));
        assert_eq!(pool.try_acquire(), None);
        pool.release();
        drop(
            pool.acquire_owned(&crate::utils::cancellation::Cancellation::default())
                .expect("success"),
        );
        assert_eq!(pool.try_acquire(), Some(1));
        assert_eq!(pool.try_acquire(), None);
    }

    #[test]
    fn semaphore_close_wakes_all_owned_permit_waiters() {
        let pool = Arc::new(Semaphore::new(0));
        std::thread::scope(|s| {
            let handles: Vec<_> = std::iter::repeat_with(|| {
                s.spawn(|| {
                    pool.acquire_owned(&crate::utils::cancellation::Cancellation::default())
                        .is_none()
                })
            })
            .take(8)
            .collect();
            let state = pool.state.lock().expect("state");
            let state = pool
                .waiting
                .wait_while(state, |state| state.waiters < 8)
                .expect("all eight really waiting");
            drop(state);
            pool.close();
            for handle in handles {
                assert!(handle.join().expect("waiter"));
            }
        });
        assert!(
            pool.acquire_owned(&crate::utils::cancellation::Cancellation::default())
                .is_none()
        );
    }

    #[test]
    fn semaphore_raw_notifications_drain_without_creating_extra_permits() {
        let pool = Arc::new(Semaphore::new(0));
        pool.close();
        for _ in 0..3 {
            pool.release();
        }
        assert_eq!(
            (0..4).filter_map(|_| pool.try_acquire()).collect::<Vec<_>>(),
            [3, 2, 1]
        );
        pool.release();
        assert_eq!(pool.acquire(), 1);
        assert_eq!(pool.try_acquire(), None);
    }
}
