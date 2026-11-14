//! Local admission shutdown; active finite encoders still finish and are joined.
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use crate::utils::cancellation::Cancellation;
use crate::utils::semaphore::Semaphore;
use anyhow::Result;

pub(super) struct Stop {
    pub(super) cancellation: Cancellation,
    pub(super) stopped: AtomicBool,
    pub(super) workers: Arc<Semaphore>,
    sender: Mutex<Option<crossbeam_channel::Sender<()>>>,
    pub(super) receiver: crossbeam_channel::Receiver<()>,
}

impl Stop {
    pub(super) fn new_cancel(workers: usize, cancellation: Cancellation) -> Self {
        let (sender, receiver) = crossbeam_channel::bounded(0);
        Self {
            cancellation,
            stopped: AtomicBool::new(false),
            workers: Arc::new(Semaphore::new(workers)),
            sender: Mutex::new(Some(sender)),
            receiver,
        }
    }

    pub(super) fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.workers.close();
        // Disconnect broadcasts to every idle worker and job handoff selector.
        self.sender.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
        #[cfg(all(test, target_os = "linux"))]
        super::tests::shutdown::internal_integration::stopped();
    }

    pub(super) fn stopped(&self, cancelled: &Cancellation) -> bool {
        self.stopped.load(Ordering::SeqCst) || cancelled.is_cancelled()
    }

    /// The guard stops admission before outer permits/receivers are dropped.
    pub(super) fn guard(&self) -> StopOnFailure<'_> {
        StopOnFailure {
            stop: self,
            successful: false,
        }
    }

    pub(super) fn catch<T>(&self, context: &str, f: impl FnOnce() -> Result<T>) -> Result<T> {
        self.catch_panic(context, || {
            f().map_err(|error| error.context(context.to_owned()))
        })
    }

    /// Scope boundaries already contain contextual worker errors; preserve
    /// them while converting scheduling/spawn panics after shutdown and joins.
    pub(super) fn catch_panic<T>(&self, context: &str, f: impl FnOnce() -> Result<T>) -> Result<T> {
        let result = self.boundary(context, f);
        if result.is_err() {
            self.stop();
        }
        result
    }

    /// An ordinary attempt failure is local; panics always close admission.
    pub(super) fn attempt<T>(&self, context: &str, f: impl FnOnce() -> Result<T>) -> Result<T> {
        self.boundary(context, || {
            f().map_err(|error| error.context(context.to_owned()))
        })
    }

    fn boundary<T>(&self, context: &str, f: impl FnOnce() -> Result<T>) -> Result<T> {
        match catch_unwind(AssertUnwindSafe(f)) {
            Ok(result) => result,
            Err(panic) => {
                self.stop();
                let message = panic
                    .downcast_ref::<&str>()
                    .copied()
                    .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
                    .unwrap_or("non-string panic");
                Err(crate::utils::CaughtPanic {
                    operation: context.into(),
                    message: message.into(),
                }
                .into())
            },
        }
    }
}

pub(super) struct StopOnFailure<'a> {
    stop: &'a Stop,
    successful: bool,
}

impl StopOnFailure<'_> {
    pub(super) fn success(&mut self) {
        self.successful = true;
    }
}

impl Drop for StopOnFailure<'_> {
    fn drop(&mut self) {
        if !self.successful {
            self.stop.stop();
        }
    }
}

/// Keep the original typed error; secondary failures remain visible in logs.
pub(super) fn retain_error(first: &mut Option<anyhow::Error>, next: anyhow::Error) {
    if first.is_some() {
        tracing::error!("Secondary parallel encoder failure: {next:#}");
    } else {
        *first = Some(next);
    }
}

pub(crate) struct TempOutput {
    pub(super) path: PathBuf,
    owned: bool,
}

impl TempOutput {
    pub(crate) fn new(path: PathBuf) -> Self {
        let owned = !path.exists();
        Self { path, owned }
    }
}

impl Drop for TempOutput {
    fn drop(&mut self) {
        if self.owned {
            match std::fs::remove_file(&self.path) {
                Ok(()) => {},
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {},
                Err(err) => tracing::error!("Could not remove {}: {err}", self.path.display()),
            }
        }
    }
}
