//! Shared admission shutdown and forced teardown. No signal handlers are installed here.
use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicBool, AtomicU8, Ordering},
};

use super::process::ProcessState;
use anyhow::Result;

/// Monotonic cancellation state, also usable as a typed anyhow cause.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum Cancelled {
    #[error("graceful cancellation requested")]
    Graceful,
    #[error("forced cancellation requested")]
    Forced,
}

struct Shared {
    transition: Mutex<()>,
    state: AtomicU8,
    legacy: Arc<AtomicBool>,
    force_sender: Mutex<Option<crossbeam_channel::Sender<()>>>,
    force_receiver: crossbeam_channel::Receiver<()>,
    processes: Mutex<Vec<Weak<Mutex<ProcessState>>>>,
}

/// One job's shared cancellation. First request stops admission; subsequent
/// requests kill owned process trees and disconnect every force receiver.
#[derive(Clone)]
pub struct Cancellation(Arc<Shared>, bool);

impl Default for Cancellation {
    #[inline]
    fn default() -> Self {
        Self::from(Arc::new(AtomicBool::new(false)))
    }
}

impl From<Arc<AtomicBool>> for Cancellation {
    /// Legacy flag updates remain visible, but can request graceful shutdown only.
    #[inline]
    fn from(legacy: Arc<AtomicBool>) -> Self {
        let (tx, rx) = crossbeam_channel::bounded(0);
        Self(
            Arc::new(Shared {
                transition: Mutex::new(()),
                state: AtomicU8::new(0),
                legacy,
                force_sender: Mutex::new(Some(tx)),
                force_receiver: rx,
                processes: Mutex::new(Vec::new()),
            }),
            false,
        )
    }
}

impl Cancellation {
    /// Admit one scene/unit while Running. Keep this view across manual passes
    /// and filter setup; use the original root, not this view, for the next scene.
    /// The view shares state/wakeups and continues through Graceful, never Forced.
    #[inline]
    pub fn admit(&self) -> Result<Self> {
        self.check_admission()?;
        Ok(self.active())
    }
    // Only owning scene/unit boundaries create this view; the state/wakeups are shared.
    pub(crate) fn active(&self) -> Self {
        Self(Arc::clone(&self.0), true)
    }

    pub(crate) fn check_admission(&self) -> Result<()> {
        if self.1 {
            self.check_forced()
        } else {
            self.check()
        }
    }
    /// Escalates once, saturating at Forced. Kill errors are returned, never hidden;
    /// state and wakeups remain forced even if an OS operation fails.
    #[inline]
    pub fn request(&self) -> Result<()> {
        let transition =
            self.0.transition.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = self.0.state.try_update(Ordering::SeqCst, Ordering::SeqCst, |old| {
            Some((old.max(u8::from(self.0.legacy.load(Ordering::SeqCst))) + 1).min(2))
        });
        self.0.legacy.store(true, Ordering::SeqCst);
        drop(transition);
        if self.is_forced() {
            self.0
                .force_sender
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            let mut error = None;
            let mut processes =
                self.0.processes.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            processes.retain(|weak| {
                weak.upgrade().is_some_and(|process| {
                    let killed =
                        process.lock().unwrap_or_else(std::sync::PoisonError::into_inner).kill();
                    if let Err(next) = killed {
                        error = Some(error.take().map_or_else(
                            || next.to_string(),
                            |previous| format!("{previous}; {next}"),
                        ));
                    }
                    true
                })
            });
            if let Some(error) = error {
                anyhow::bail!("Killing cancelled process trees: {error}");
            }
        }
        Ok(())
    }

    // Linearize the short rename/result-enqueue commit against force. Never
    // include child waits, decoding, or caller callbacks in this critical section.
    pub(crate) fn publish<T>(&self, commit: impl FnOnce() -> Result<T>) -> Result<T> {
        let _transition =
            self.0.transition.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        self.check_forced()?;
        commit()
    }

    #[inline]
    pub fn is_cancelled(&self) -> bool {
        if self.0.legacy.load(Ordering::SeqCst) {
            self.0.state.fetch_max(1, Ordering::SeqCst);
        }
        self.0.state.load(Ordering::SeqCst) != 0
    }

    #[inline]
    pub fn is_forced(&self) -> bool {
        self.0.state.load(Ordering::SeqCst) == 2
    }

    /// Use at scene/sequence admission, not between an active scene's passes.
    #[inline]
    pub fn check(&self) -> Result<()> {
        self.check_forced()?;
        if self.is_cancelled() {
            return Err(Cancelled::Graceful.into());
        }
        Ok(())
    }

    /// Active work observes only force, including capture/replay and filter feeds.
    #[inline]
    pub fn check_forced(&self) -> Result<()> {
        if self.is_forced() {
            return Err(Cancelled::Forced.into());
        }
        Ok(())
    }

    /// Disconnect broadcasts to all clones, including future subscribers.
    #[inline]
    pub fn forced(&self) -> &crossbeam_channel::Receiver<()> {
        &self.0.force_receiver
    }

    /// Compatibility for metric callbacks; this bool does not provide force support.
    #[deprecated(
        note = "graceful-only compatibility flag; new sequences should use the token itself (`check`, `check_forced`, `is_cancelled`, `forced`)"
    )]
    #[inline]
    pub fn legacy_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.0.legacy)
    }

    pub(crate) fn register(&self, process: &Arc<Mutex<ProcessState>>) -> Result<()> {
        let mut processes =
            self.0.processes.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        processes.retain(|weak| weak.strong_count() != 0);
        processes.push(Arc::downgrade(process));
        if self.is_forced() {
            process.lock().unwrap_or_else(std::sync::PoisonError::into_inner).kill()?;
            return Err(Cancelled::Forced.into());
        }
        Ok(())
    }

    /// Sends active-scene data without waiting for a consumer after force.
    #[inline]
    pub fn send<T: Send + Sync + 'static>(
        &self,
        tx: &crossbeam_channel::Sender<T>,
        value: T,
    ) -> Result<()> {
        self.check_forced()?;
        crossbeam_channel::select_biased! {
            recv(self.forced()) -> _ => Err(Cancelled::Forced.into()),
            send(tx, value) -> result => result.map_err(anyhow::Error::from),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn graceful_cancellation_rejects_new_admission_but_keeps_active_work() {
        let token = Cancellation::default();
        assert!(!token.is_cancelled());
        let admitted = token.admit().expect("scene admitted while Running");
        token.request().expect("graceful");
        assert!(token.is_cancelled() && !token.is_forced());
        assert!(token.admit().is_err(), "root cannot admit another scene");
        admitted.check_admission().expect("active manual passes still admitted");
    }

    #[test]
    fn concurrent_force_requests_wake_every_cancellation_waiter() {
        let token = Cancellation::default();
        let admitted = token.admit().expect("scene admitted while Running");
        token.request().expect("graceful");
        std::thread::scope(|s| {
            let waiters: Vec<_> =
                std::iter::repeat_with(|| s.spawn(|| token.forced().recv())).take(8).collect();
            let requesters: Vec<_> = std::iter::repeat_with(|| {
                let token = token.clone();
                s.spawn(move || token.request())
            })
            .take(8)
            .collect();
            for request in requesters {
                request.join().expect("requester").expect("request");
            }
            for waiter in waiters {
                assert!(waiter.join().expect("waiter").is_err());
            }
        });
        assert!(token.check_forced().expect_err("forced").is::<Cancelled>());
        token.request().expect("saturated");
        assert!(token.is_forced());
        assert!(admitted.check_admission().expect_err("active view forced").is::<Cancelled>());
    }
    #[test]
    #[expect(deprecated, reason = "exercises the compatibility flag")]
    fn legacy_cancellation_flag_is_monotonic_and_never_forces() {
        let flag = Arc::new(AtomicBool::new(false));
        let token = Cancellation::from(Arc::clone(&flag));
        flag.store(true, Ordering::SeqCst);
        assert!(token.is_cancelled() && !token.is_forced());
        flag.store(false, Ordering::SeqCst);
        assert!(
            token.is_cancelled(),
            "observed graceful cancellation cannot reset"
        );
        assert!(token.forced().try_recv().is_err());
        let fresh = Cancellation::default();
        fresh.request().expect("request");
        assert!(fresh.legacy_flag().load(Ordering::SeqCst));
    }
}
