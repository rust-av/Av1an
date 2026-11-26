//! CLI interrupt ownership. Libraries opt into tokens, never process handlers.
use andean_condor::utils::cancellation::Cancellation;
use anyhow::{Context, Result};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

/// A shared E12 root plus the first interrupt's exit classification.
/// Converting an old bool retains graceful compatibility, not an existing registry.
#[derive(Clone, Default)]
pub struct Interrupt {
    pub(crate) token: Cancellation,
    cause: Arc<Mutex<u8>>,
}

impl From<Cancellation> for Interrupt {
    fn from(token: Cancellation) -> Self {
        Self {
            token,
            cause: Arc::default(),
        }
    }
}
impl From<Arc<AtomicBool>> for Interrupt {
    fn from(flag: Arc<AtomicBool>) -> Self {
        Cancellation::from(flag).into()
    }
}

impl Interrupt {
    /// Ordinary-thread only. First request wins (keyboard/INT=130, TERM=143);
    /// all later requests escalate the same process registry, regardless of cause.
    pub(crate) fn request(&self, code: u8) -> Result<()> {
        let mut cause = self.cause.lock().unwrap_or_else(|e| e.into_inner());
        if *cause == 0 {
            *cause = code;
        }
        self.token.request()
    }

    pub(crate) fn finish(&self, result: Result<()>) -> Result<()> {
        if !self.token.is_cancelled() {
            return result;
        }
        if result.as_ref().is_err_and(|error| error.is::<Interrupted>()) {
            return result;
        }
        let cause = *self.cause.lock().unwrap_or_else(|e| e.into_inner());
        // Terminal/panic failures also stop admission through the legacy flag.
        // Do not mislabel their typed errors as a user/OS interrupt.
        if cause == 0
            && result
                .as_ref()
                .is_err_and(|error| !error.is::<andean_condor::utils::cancellation::Cancelled>())
        {
            return result;
        }
        let interrupted = Interrupted(if cause == 0 { 130 } else { cause });
        // Keep typed errors/cleanup context, but cancellation leads the diagnostic.
        match result {
            Ok(()) => Err(interrupted.into()),
            Err(error) => Err(error.context(interrupted)),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("interrupted: cancellation requested (exit {0})")]
pub(crate) struct Interrupted(pub(crate) u8);

/// One owned listener per CLI invocation, never per stage. Drop joins before
/// the token/error leaves the process frontend. The process registration keeps
/// no job/token reference between invocations.
pub(crate) struct Listener {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<Result<()>>>,
}

static ACTIVE: AtomicBool = AtomicBool::new(false);

impl Listener {
    pub(crate) fn start(interrupt: Interrupt) -> Result<Self> {
        anyhow::ensure!(
            !ACTIVE.swap(true, Ordering::SeqCst),
            "CLI interrupt listener already active"
        );
        let result = Self::start_inner(interrupt);
        if result.is_err() {
            ACTIVE.store(false, Ordering::SeqCst);
        }
        result
    }

    fn start_inner(interrupt: Interrupt) -> Result<Self> {
        let mut receive = platform::receiver()?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("condor-interrupts".into())
            .spawn(move || {
                let mut failure = None;
                loop {
                    // Drain the final handoff before joining; a short job must
                    // not return success just because the listener was parked.
                    let stopping = stopped.load(Ordering::SeqCst);
                    for code in receive() {
                        if let Err(error) = interrupt.request(code) {
                            // Continue listening after a kill failure, so another request
                            // can retry cleanup. Never drop the owning error.
                            failure = Some(match failure {
                                None => error,
                                Some(prior) => {
                                    error.context(format!("earlier interrupt cleanup: {prior:#}"))
                                },
                            });
                        }
                    }
                    if stopping {
                        break;
                    }
                    std::thread::park_timeout(std::time::Duration::from_millis(5));
                }
                failure.map_or(Ok(()), Err)
            })
            .context("starting interrupt listener")?;
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }

    pub(crate) fn join(&mut self) -> Result<()> {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            let result = thread.join().map_err(|payload| andean_condor::utils::CaughtPanic {
                operation: "interrupt listener".into(),
                message: payload
                    .downcast_ref::<&str>()
                    .copied()
                    .unwrap_or("non-string panic payload")
                    .into(),
            });
            ACTIVE.store(false, Ordering::SeqCst);
            result??;
        }
        Ok(())
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        let _ = self.join();
    }
}

#[cfg(unix)]
mod platform {
    use super::*;
    use signal_hook::{
        consts::{SIGINT, SIGTERM},
        iterator::Signals,
    };
    use std::sync::OnceLock;

    static SIGNALS: OnceLock<std::io::Result<Mutex<Signals>>> = OnceLock::new();

    pub(super) fn receiver() -> Result<impl FnMut() -> Vec<u8> + Send> {
        // signal-hook's self-pipe/atomic handoff is async-signal-safe. Only the
        // ordinary listener calls request(), locks, tracing or process operations.
        let signals = SIGNALS
            .get_or_init(|| Signals::new([SIGINT, SIGTERM]).map(Mutex::new))
            .as_ref()
            .map_err(|e| std::io::Error::new(e.kind(), e.to_string()))?;
        signals.lock().unwrap_or_else(|e| e.into_inner()).pending().for_each(drop);
        Ok(move || {
            signals
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .pending()
                .map(|signal| if signal == SIGTERM { 143 } else { 130 })
                .collect()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    pub(super) mod windows;

    #[test]
    fn internal_stop_is_not_user_interrupt() {
        let cancellation = Interrupt::default();
        #[expect(
            deprecated,
            reason = "simulates an internal stop without recording a user interrupt"
        )]
        cancellation.token.legacy_flag().store(true, Ordering::SeqCst);
        let error = cancellation
            .finish(Err(
                std::io::Error::other("terminal failure sentinel").into()
            ))
            .unwrap_err();
        assert!(error.downcast_ref::<std::io::Error>().is_some());
        assert!(error.downcast_ref::<Interrupted>().is_none());
    }

    fn check_interrupt_finish(typed_cleanup: bool) {
        let cancellation = Interrupt::default();
        cancellation.request(143).unwrap();
        assert!(!cancellation.token.is_forced());
        for code in [130, 143, 130] {
            cancellation.request(code).unwrap();
        }
        assert!(cancellation.token.is_forced());
        let failure = std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "cleanup failure sentinel",
        );
        let error = cancellation.finish(Err(failure.into())).unwrap_err();
        assert_eq!(error.downcast_ref::<Interrupted>().unwrap().0, 143);
        if !typed_cleanup {
            return;
        }
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert!(format!("{error:#}").contains("cleanup failure sentinel"));
    }

    #[test]
    fn repeated_interrupt_requests_escalate_but_preserve_the_first_exit_code() {
        check_interrupt_finish(false);
    }

    #[test]
    fn interrupt_finish_preserves_typed_cleanup_error_and_diagnostic() {
        check_interrupt_finish(true);
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::sync::OnceLock;
    static EVENTS: Mutex<Option<std::sync::mpsc::Sender<u8>>> = Mutex::new(None);
    static REGISTERED: OnceLock<std::result::Result<(), ctrlc::Error>> = OnceLock::new();

    pub(super) fn receiver() -> Result<impl FnMut() -> Vec<u8> + Send> {
        REGISTERED
            .get_or_init(|| {
                ctrlc::set_handler(|| {
                    // ctrlc hands console events to its normal thread, not this job's
                    // callback. It retains only a sender, never a stale job token.
                    if let Some(tx) = EVENTS.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                        let _ = tx.send(130);
                    }
                })
            })
            .as_ref()
            .map_err(|e| anyhow::anyhow!("registering console handler: {e}"))?;
        let (tx, rx) = std::sync::mpsc::channel();
        *EVENTS.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
        Ok(move || {
            let events = rx.try_iter().collect::<Vec<_>>();
            #[cfg(test)]
            super::tests::windows::observe_received(events.len());
            events
        })
    }
}
