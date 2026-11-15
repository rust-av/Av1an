use std::{
    io::{self, BufWriter, IsTerminal, Write, stderr, stdout},
    sync::{
        Arc, Mutex, Once, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError},
    },
    thread,
    time::Duration,
};

use andean_condor::core::sequence::SequenceStatus;
use anyhow::Result;
use ratatui::{
    Frame, Terminal,
    crossterm::{
        self,
        event::{self, Event as TermEvent, KeyCode, KeyModifiers},
        execute, queue,
        terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
    },
    prelude::CrosstermBackend,
};
use tracing::debug;

pub mod benchmarker;
pub mod noise_detection;
pub mod parallel_encoder;
pub mod quality_check;
pub mod scene_concatenator;
pub mod scene_detection;
pub mod target_quality;

pub type PanicHook = Box<dyn Fn(&std::panic::PanicHookInfo<'_>) + Sync + Send + 'static>;
type OwnedTerminal = Terminal<CrosstermBackend<SelectedWriter>>;
type TerminalBuffer = BufWriter<Box<dyn Write + Send>>;

static PANIC_HOOK: Once = Once::new();
static PANIC_TERMINAL: Mutex<Weak<Mutex<TerminalOwner>>> = Mutex::new(Weak::new());
static PANIC_STAGE: Mutex<Weak<PanicStage>> = Mutex::new(Weak::new());
static PANIC_REPORTS: AtomicU64 = AtomicU64::new(0);

pub(crate) fn panic_reports() -> u64 {
    PANIC_REPORTS.load(Ordering::SeqCst)
}

/// Install once, retaining the host's diagnostic hook. Never exit here: process
/// owners must unwind, kill their trees and reap before the caller fails.
pub(crate) fn install_panic_hook() {
    PANIC_HOOK.call_once(|| {
        let prior: PanicHook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let stage = PANIC_STAGE.lock().unwrap_or_else(|e| e.into_inner()).upgrade();
            if let Some(stage) = stage {
                stage.panicked.store(true, Ordering::SeqCst);
                stage.cancelled.store(true, Ordering::SeqCst);
            }
            let terminal = PANIC_TERMINAL.lock().unwrap_or_else(|e| e.into_inner()).upgrade();
            if let Some(terminal) = terminal {
                let mut owner = terminal.lock().unwrap_or_else(|e| e.into_inner());
                owner.cancelled.store(true, Ordering::SeqCst);
                let _ = owner.cleanup();
            }
            // No owner/registry lock spans an arbitrary host hook callback.
            PANIC_REPORTS.fetch_add(1, Ordering::SeqCst);
            prior(info);
        }));
    });
}

struct PanicStage {
    panicked: AtomicBool,
    cancelled: Arc<AtomicBool>,
}

impl PanicStage {
    fn check(&self) -> Result<()> {
        anyhow::ensure!(
            !self.panicked.load(Ordering::SeqCst),
            "stage thread panicked"
        );
        Ok(())
    }
}

// The progress sender can outlive a failed display. Stop receiving and join on
// every return/unwind instead of detaching a worker or waiting for disconnect.
struct ProgressThread {
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Drop for ProgressThread {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Owns terminal resources from before initialization through the last flush.
pub struct StdOutOrErrTerminal {
    terminal: OwnedTerminal,
    owner: Arc<Mutex<TerminalOwner>>,
    restored: bool,
}

impl StdOutOrErrTerminal {
    fn cleanup(&mut self) -> io::Result<()> {
        if std::mem::replace(&mut self.restored, true) {
            return Ok(());
        }
        // RATIONALE: ratatui's destructor can show the cursor again (and log on
        // failure). Retire its backend before our sole, best-effort cleanup;
        // only that destructor uses the inert sink, never real terminal I/O.
        let _backend = std::mem::replace(
            self.terminal.backend_mut(),
            CrosstermBackend::new(SelectedWriter::new(
                Box::new(io::sink()),
                Arc::new(AtomicBool::new(false)),
            )),
        );
        self.owner.lock().unwrap_or_else(|e| e.into_inner()).cleanup()
    }
}

impl Drop for StdOutOrErrTerminal {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

struct TerminalOwner {
    writer: SelectedWriter,
    cancelled: Arc<AtomicBool>,
    console_pause: Option<crate::logging::ConsolePause>,
    raw: bool,
    screen: bool,
    cursor: bool,
    // A concrete function pointer keeps OS failures injectable without a driver trait.
    raw_mode: fn(bool) -> io::Result<()>,
}

fn set_raw_mode(enabled: bool) -> io::Result<()> {
    if enabled {
        enable_raw_mode()
    } else {
        disable_raw_mode()
    }
}

impl TerminalOwner {
    fn new(writer: Box<dyn Write + Send>, cancelled: Arc<AtomicBool>) -> Self {
        Self {
            writer: SelectedWriter::new(writer, Arc::clone(&cancelled)),
            cancelled,
            console_pause: None,
            raw: false,
            screen: false,
            cursor: false,
            raw_mode: set_raw_mode,
        }
    }

    fn cancel_on_error<T>(&self, result: io::Result<T>) -> io::Result<T> {
        if result.is_err() {
            self.cancelled.store(true, Ordering::SeqCst);
        }
        result
    }

    fn disable_raw(&mut self) -> io::Result<()> {
        let result = if std::mem::take(&mut self.raw) {
            (self.raw_mode)(false)
        } else {
            Ok(())
        };
        self.cancel_on_error(result)
    }

    fn leave(&mut self, rows: u16) -> io::Result<()> {
        let raw = self.disable_raw();
        let screen = if std::mem::take(&mut self.screen) {
            leave_alternate_screen(&mut self.writer, rows)
        } else {
            Ok(())
        };
        raw.and(screen)
    }

    fn cleanup(&mut self) -> io::Result<()> {
        // BufWriter::drop retries buffered writes. Discard pending data so an
        // unsuccessful Enter/Hide cannot be replayed after Leave/Show.
        let buffered = self.writer.inner.lock().unwrap_or_else(|e| e.into_inner()).take();
        let active = self.raw || self.screen || self.cursor;
        // Raw ownership is independent of whether a panic already retired the writer.
        let mut result = self.disable_raw();
        if let Some(buffered) = buffered {
            let (mut writer, _) = buffered.into_parts();
            if std::mem::take(&mut self.screen) {
                let leave = queue!(writer, LeaveAlternateScreen);
                result = result.and(leave);
            }
            if std::mem::take(&mut self.cursor) {
                let finish = finish_restore(&mut writer);
                result = result.and(finish);
            } else if active {
                let flush = writer.flush();
                result = result.and(flush);
            }
        }
        let result = self.cancel_on_error(result);
        // Retire the writer and attempt every final control/flush before any
        // deferred console event (including the host panic hook) can print.
        drop(self.console_pause.take());
        result
    }
}

#[derive(Clone)]
struct SelectedWriter {
    inner: Arc<Mutex<Option<TerminalBuffer>>>,
    cancelled: Arc<AtomicBool>,
}

impl SelectedWriter {
    fn new(writer: Box<dyn Write + Send>, cancelled: Arc<AtomicBool>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Some(BufWriter::new(writer)))),
            cancelled,
        }
    }
    fn cancel_on_error<T>(&self, result: io::Result<T>) -> io::Result<T> {
        if result.is_err() {
            self.cancelled.store(true, Ordering::SeqCst);
        }
        result
    }
}

impl Write for SelectedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let result = self
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
            .ok_or_else(|| io::Error::other("terminal retired"))?
            .write(bytes);
        self.cancel_on_error(result)
    }
    fn flush(&mut self) -> io::Result<()> {
        let result = self
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
            .ok_or_else(|| io::Error::other("terminal retired"))?
            .flush();
        self.cancel_on_error(result)
    }
}

impl Drop for TerminalOwner {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

fn init_terminal(
    mut owner: TerminalOwner,
    terminal_ops: bool,
    construct: impl FnOnce(CrosstermBackend<SelectedWriter>) -> io::Result<OwnedTerminal>,
) -> Result<StdOutOrErrTerminal> {
    install_panic_hook();
    if terminal_ops && stderr().is_terminal() {
        owner.console_pause = Some(crate::logging::ConsolePause::new());
    }
    let cancelled = Arc::clone(&owner.cancelled);
    let owner = Arc::new(Mutex::new(owner));
    *PANIC_TERMINAL.lock().unwrap_or_else(|e| e.into_inner()) = Arc::downgrade(&owner);
    #[cfg(test)]
    if let Some(before_raw) = tests::BEFORE_RAW.with_borrow_mut(Option::take) {
        before_raw();
    }
    let result = (|| {
        let writer = owner.lock().unwrap_or_else(|e| e.into_inner()).writer.clone();
        if terminal_ops {
            let mut resources = owner.lock().unwrap_or_else(|e| e.into_inner());
            // A hook can retire this registration before raw acquisition starts.
            if resources.writer.inner.lock().unwrap_or_else(|e| e.into_inner()).is_none() {
                return Err(io::Error::other("terminal retired").into());
            }
            (resources.raw_mode)(true)?;
            resources.raw = true;
            // Even a failed write/flush may already have entered the screen.
            resources.screen = true;
            resources.cursor = true;
            drop(resources);
            enter_alternate_screen(&mut writer.clone())?;
        }
        let terminal = construct(CrosstermBackend::new(writer))?;
        Ok(StdOutOrErrTerminal {
            terminal,
            owner,
            restored: false,
        })
    })();
    if result.is_err() {
        cancelled.store(true, Ordering::SeqCst);
    }
    result
}

/// When `CONDOR_TEST_MODE=1` is set (testing / automation), skip
/// terminal-specific operations (raw mode, alternate screen, keyboard input)
/// and use JSON progress output instead.
pub fn is_test_mode() -> bool {
    std::env::var("CONDOR_TEST_MODE").is_ok_and(|v| v == "1")
}

/// Whether the app should drive the TUI or wait headlessly instead.
///
/// The TUI renders to stdout when it is a terminal and to stderr otherwise,
/// and it needs raw mode plus the alternate screen on whichever it picks.
/// With *neither* stream on a terminal — output piped into a file, a CI job,
/// or a container started without `-t` — enabling raw mode fails with
/// `ENXIO` (`No such device or address`) because there is no controlling
/// terminal to take over.
fn run_headless(stdout_is_terminal: bool, stderr_is_terminal: bool, test_mode: bool) -> bool {
    test_mode || !(stdout_is_terminal || stderr_is_terminal)
}

fn selected_writer(use_stdout: bool) -> Box<dyn Write + Send> {
    #[cfg(test)]
    if let Some(writer) = tests::TEST_WRITER.with_borrow_mut(Option::take) {
        return writer;
    }
    if is_test_mode() {
        Box::new(io::sink())
    } else if use_stdout {
        Box::new(stdout())
    } else {
        Box::new(stderr())
    }
}

fn enter_alternate_screen(writer: &mut impl Write) -> io::Result<()> {
    execute!(writer, EnterAlternateScreen)
}

fn leave_alternate_screen(writer: &mut impl Write, rows: u16) -> io::Result<()> {
    execute!(writer, LeaveAlternateScreen)?;
    for _ in 0..rows {
        writeln!(writer)?;
    }
    Ok(())
}

fn finish_restore(writer: &mut impl Write) -> io::Result<()> {
    let show = queue!(writer, crossterm::cursor::Show);
    let flush = writer.flush();
    show.and(flush)
}

/// Thread-safe container for sharing mutable progress state between a
/// producer thread (progress receiver) and a consumer thread (UI event loop).
///
/// The producer calls [`SharedProgress::apply`] to update state. The consumer
/// calls [`SharedProgress::read_if_dirty`] once per tick to retrieve the
/// latest snapshot.
pub struct SharedProgress<State: Clone + Send + 'static> {
    state: Arc<Mutex<State>>,
    dirty: Arc<AtomicBool>,
}

impl<State: Clone + Send + 'static> SharedProgress<State> {
    pub fn new(initial: State) -> Self {
        Self {
            state: Arc::new(Mutex::new(initial)),
            dirty: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Apply an update function to the shared state.
    ///
    /// The `update` closure receives a mutable reference to the inner state and
    /// returns `true` if the state was meaningfully changed (setting the dirty
    /// flag so the consumer will pick it up on the next tick).
    pub fn apply(&self, update: impl FnOnce(&mut State) -> bool) {
        let mut guard = self.state.lock().expect("SharedProgress lock");
        if update(&mut *guard) {
            self.dirty.store(true, Ordering::Release);
        }
    }

    /// Read-and-clear the dirty flag.
    ///
    /// Returns `Some(state)` if a new snapshot is available since the last
    /// call, or `None` if nothing has changed.
    pub fn read_if_dirty(&self) -> Option<State> {
        if self.dirty.swap(false, Ordering::Acquire) {
            let guard = self.state.lock().expect("SharedProgress lock");
            return Some(guard.clone());
        }
        None
    }

    /// Force-read the current state, ignoring the dirty flag.
    pub fn read(&self) -> State {
        let guard = self.state.lock().expect("SharedProgress lock");
        guard.clone()
    }
}

// Every clone shares the same `Arc`-backed state.
impl<S: Clone + Send + 'static> Clone for SharedProgress<S> {
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
            dirty: Arc::clone(&self.dirty),
        }
    }
}

enum AppEvent {
    Quit,
}

pub trait TuiApp: Send + Sync + 'static {
    /// The progress snapshot shared between the producer and consumer threads.
    type State: Clone + Send + 'static;

    /// The shared progress container backing this app.
    fn shared_progress(&self) -> &SharedProgress<Self::State>;

    /// The latest cached snapshot, updated by the event loop.
    fn cached_state(&self) -> &Self::State;

    /// Mutable access to the cached snapshot (used by the event loop).
    fn cached_state_mut(&mut self) -> &mut Self::State;

    /// The `attempted_cancel` flag (set on first Ctrl-C).
    fn attempted_cancel(&self) -> bool;

    /// Mutable access to the `attempted_cancel` flag.
    fn attempted_cancel_mut(&mut self) -> &mut bool;

    /// The notice sent to stderr/logging on the first Ctrl-C.
    fn cancel_message(&self) -> &'static str;

    /// Map a single [`SequenceStatus`] onto the shared state, returning `true`
    /// if the state changed. Console (non-TTY) emission is handled here too.
    ///
    /// This is a static method (no `self`) so it can run on the spawned
    /// progress thread. Producer-only context can be captured by `progress_mapper`.
    fn map_progress(status: SequenceStatus, state: &mut Self::State) -> bool;

    /// Capture producer-only report storage without putting it in render snapshots.
    fn progress_mapper(
        &self,
    ) -> impl FnMut(SequenceStatus, &mut Self::State) -> bool + Send + 'static {
        Self::map_progress
    }

    /// Optional hook invoked with each fresh snapshot read from the shared
    /// state. The default implementation caches it. Apps with UI-local state
    /// (e.g. scene tables) or transition reactions (e.g. resetting a pass
    /// timer) override this to sync before caching.
    fn on_snapshot(&mut self, snapshot: Self::State) {
        *self.cached_state_mut() = snapshot;
    }

    /// Optional hook invoked once after the loop terminates (both TTY and
    /// headless paths). Used for post-run reports. Defaults to a no-op.
    fn after_loop(&mut self) {}

    fn init(&mut self, cancelled: Arc<AtomicBool>) -> Result<StdOutOrErrTerminal> {
        let use_stdout = stdout().is_terminal();
        init_terminal(
            TerminalOwner::new(selected_writer(use_stdout), cancelled),
            !is_test_mode(),
            Terminal::new,
        )
    }

    fn restore(&mut self, terminal: &mut StdOutOrErrTerminal) -> Result<()> {
        if terminal.restored {
            return Ok(());
        }
        // A worker's panic hook may already have retired this backend. Never
        // render, hide the cursor or replay buffers after that restoration.
        if terminal
            .owner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .writer
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_none()
        {
            return terminal.cleanup().map_err(Into::into);
        }
        let result = (|| -> Result<()> {
            if !is_test_mode() {
                let (_columns, rows) = crossterm::terminal::size().unwrap_or((80, 24));
                terminal.owner.lock().unwrap_or_else(|e| e.into_inner()).leave(rows)?;
                // RATIONALE: clear() queries the cursor through crossterm's stdout,
                // even with a stderr backend. Fullscreen resize clears and resets
                // the render buffers without sending that query to piped stdout.
                terminal.terminal.resize(terminal.terminal.size()?.into())?;
                terminal.terminal.draw(|f| self.render(f))?;
            }
            Ok(())
        })();
        if result.is_err() {
            terminal
                .owner
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .cancelled
                .store(true, Ordering::SeqCst);
        }
        let cleanup = terminal.cleanup().map_err(Into::into);
        result.and(cleanup)
    }

    /// Legacy graceful bool API. A `true` result indicates a repeated request,
    /// never permission to exit before joining the runtime's process owners.
    fn handle_ctrl_c(
        &mut self,
        key: event::KeyEvent,
        cancelled: &Arc<AtomicBool>,
        stdout_is_terminal: bool,
    ) -> bool {
        if key.code == KeyCode::Char('c')
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && key.is_press()
        {
            *self.attempted_cancel_mut() = true;
            let already_cancelled = cancelled.swap(true, Ordering::SeqCst);
            if already_cancelled {
                debug!("Force quit Condor");
                return true;
            } else if !stdout_is_terminal {
                eprintln!("{}", self.cancel_message());
            }
        }
        false
    }

    /// Keyboard and OS events advance the same E12 root. Keep rendering until
    /// the producer disconnects, including after force; never detach its owners.
    fn handle_ctrl_c_cancel(
        &mut self,
        key: event::KeyEvent,
        cancelled: &crate::interrupt::Interrupt,
    ) -> Result<()> {
        if key.code == KeyCode::Char('c')
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && key.is_press()
        {
            *self.attempted_cancel_mut() = true;
            let first = !cancelled.token.is_cancelled();
            cancelled.request(130)?;
            if first {
                tracing::info!("{}", self.cancel_message());
            }
        }
        Ok(())
    }

    /// The full runtime: spawns the progress thread, waits headlessly in
    /// test mode, and drives the terminal event loop otherwise.
    fn run(
        &mut self,
        progress_rx: Receiver<SequenceStatus>,
        cancellation: impl Into<crate::interrupt::Interrupt>,
    ) -> Result<()> {
        let cancellation = cancellation.into();
        let cancelled = cancellation.token.legacy_flag();
        install_panic_hook();
        let panic_stage = Arc::new(PanicStage {
            panicked: AtomicBool::new(false),
            cancelled: Arc::clone(&cancelled),
        });
        *PANIC_STAGE.lock().unwrap_or_else(|e| e.into_inner()) = Arc::downgrade(&panic_stage);
        // Install the render theme for the whole run. Reading an env var keeps
        // the plumbing out of every `new()`; the default (dark) is the
        // historical look. Headless runs never render, so this is a no-op
        // there.
        let _theme_guard = crate::theme::Theme::set_mode(crate::theme::ThemeMode::parse(
            std::env::var("CONDOR_THEME").ok().as_deref(),
        ));
        let (event_tx, event_rx) = mpsc::channel();

        let shared_progress = self.shared_progress().clone();
        let mut map_progress = self.progress_mapper();
        let terminal = if run_headless(
            stdout().is_terminal(),
            stderr().is_terminal(),
            is_test_mode(),
        ) {
            None
        } else {
            Some(self.init(Arc::clone(&cancelled))?)
        };
        let quit = Arc::new(AtomicBool::new(false));
        let quit_flag = Arc::clone(&quit);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::clone(&stop);
        let worker_stage = Arc::clone(&panic_stage);
        let progress_thread = ProgressThread {
            stop,
            handle: Some(thread::spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    while !stop_flag.load(Ordering::SeqCst) && worker_stage.check().is_ok() {
                        match progress_rx.recv_timeout(Duration::from_millis(10)) {
                            Ok(progress) => {
                                shared_progress.apply(|state| map_progress(progress, state));
                            },
                            Err(RecvTimeoutError::Timeout) => (),
                            Err(RecvTimeoutError::Disconnected) => break,
                        }
                    }
                }));
                if result.is_err() {
                    worker_stage.panicked.store(true, Ordering::SeqCst);
                    worker_stage.cancelled.store(true, Ordering::SeqCst);
                }
                let _ = event_tx.send(AppEvent::Quit);
                quit_flag.store(true, Ordering::Release);
            })),
        };

        let Some(mut terminal) = terminal else {
            // No terminal to take over (or test mode): avoid any terminal
            // operations - headless wait.
            while !quit.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(10));
                panic_stage.check()?;
                #[cfg(test)]
                tests::HEADLESS_TICK.with_borrow_mut(|hook| {
                    if let Some(hook) = hook {
                        hook();
                    }
                });
                while event_rx.try_recv().is_ok() {}
            }
            drop(progress_thread);
            panic_stage.check()?;
            let snapshot = self.shared_progress().read();
            self.on_snapshot(snapshot);
            self.after_loop();
            return Ok(());
        };

        let result = (|| -> Result<()> {
            loop {
                panic_stage.check()?;
                // Keyboard input is read here, never on a helper thread:
                // crossterm's event reader is process-wide, so no other thread may
                // hold it. Teardown uses fullscreen resize, not a cursor query.
                // The drain also ends when
                // input is exhausted or no terminal can be read (`Err`).
                while matches!(event::poll(Duration::ZERO), Ok(true)) {
                    if let Ok(TermEvent::Key(key)) = event::read() {
                        self.handle_ctrl_c_cancel(key, &cancellation)?;
                    }
                }

                if let Some(snapshot) = self.shared_progress().read_if_dirty() {
                    self.on_snapshot(snapshot);
                }

                panic_stage.check()?;
                if quit.load(Ordering::Acquire) {
                    let snapshot = self.shared_progress().read();
                    self.on_snapshot(snapshot);
                    terminal.terminal.draw(|f| self.render(f))?;
                    self.restore(&mut terminal)?;
                    break;
                }

                match event_rx.recv_timeout(Duration::from_millis(33)) {
                    Ok(AppEvent::Quit) => {
                        panic_stage.check()?;
                        if let Some(snapshot) = self.shared_progress().read_if_dirty() {
                            self.on_snapshot(snapshot);
                        }
                        terminal.terminal.draw(|f| self.render(f))?;
                        self.restore(&mut terminal)?;
                        break;
                    },
                    Err(RecvTimeoutError::Timeout) => {
                        panic_stage.check()?;
                        terminal.terminal.draw(|f| self.render(f))?;
                    },
                    Err(RecvTimeoutError::Disconnected) => {
                        panic_stage.check()?;
                        if let Some(snapshot) = self.shared_progress().read_if_dirty() {
                            self.on_snapshot(snapshot);
                        }
                        terminal.terminal.draw(|f| self.render(f))?;
                        self.restore(&mut terminal)?;
                        break;
                    },
                }
            }
            Ok(())
        })();
        if result.is_err() {
            cancelled.store(true, Ordering::SeqCst);
        }
        let cleanup = terminal.cleanup().map_err(Into::into);
        result.and(cleanup)?;

        drop(progress_thread);
        panic_stage.check()?;
        self.after_loop();
        Ok(())
    }

    fn render(&self, frame: &mut Frame);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{TerminalOptions, Viewport, layout::Rect};

    thread_local! {
        pub(super) static TEST_WRITER: std::cell::RefCell<Option<Box<dyn Write + Send>>> = const { std::cell::RefCell::new(None) };
        pub(super) static BEFORE_RAW: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
        pub(super) static HEADLESS_TICK: std::cell::RefCell<Option<Box<dyn FnMut()>>> = const { std::cell::RefCell::new(None) };
    }

    mod internal_integration {
        use super::*;

        #[test]
        fn headless_applies_every_event_and_clones_only_the_final_state() {
            const TEST: &str = "apps::tests::internal_integration::headless_applies_every_event_and_clones_only_the_final_state";
            if std::env::var_os("HEADLESS_EVENT_MAPPING_CHILD").is_none() {
                let output = std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", TEST, "--nocapture"])
                    .env("HEADLESS_EVENT_MAPPING_CHILD", TEST)
                    .env("CONDOR_TEST_MODE", "1")
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                assert!(
                    String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed"),
                    "headless event-mapping child did not execute: {output:?}"
                );
                assert!(
                    String::from_utf8_lossy(&output.stdout)
                        .contains("HEADLESS_EVENT_MAPPING_ASSERTIONS_COMPLETED")
                );
                eprintln!("{}", String::from_utf8_lossy(&output.stdout));
                return;
            }
            assert_eq!(std::env::var("HEADLESS_EVENT_MAPPING_CHILD").unwrap(), TEST);
            use std::sync::atomic::AtomicUsize;
            struct Counted {
                clones: Arc<AtomicUsize>,
                events: Vec<String>,
                ack: mpsc::SyncSender<()>,
            }
            impl Clone for Counted {
                fn clone(&self) -> Self {
                    self.clones.fetch_add(1, Ordering::SeqCst);
                    Self {
                        clones: Arc::clone(&self.clones),
                        events: self.events.clone(),
                        ack: self.ack.clone(),
                    }
                }
            }
            struct App {
                progress: SharedProgress<Counted>,
                cached: Counted,
                cancel: bool,
                reported: bool,
            }
            impl TuiApp for App {
                type State = Counted;
                fn shared_progress(&self) -> &SharedProgress<Counted> {
                    &self.progress
                }
                fn cached_state(&self) -> &Counted {
                    &self.cached
                }
                fn cached_state_mut(&mut self) -> &mut Counted {
                    &mut self.cached
                }
                fn attempted_cancel(&self) -> bool {
                    self.cancel
                }
                fn attempted_cancel_mut(&mut self) -> &mut bool {
                    &mut self.cancel
                }
                fn cancel_message(&self) -> &'static str {
                    "cancel"
                }
                fn map_progress(status: SequenceStatus, state: &mut Counted) -> bool {
                    let SequenceStatus::Whole(andean_condor::core::sequence::Status::Completed {
                        id,
                    }) = status
                    else {
                        panic!("event")
                    };
                    state.events.push(id);
                    state.ack.send(()).unwrap();
                    true
                }
                fn after_loop(&mut self) {
                    self.reported = true;
                    assert_eq!(self.cached.events, ["0", "1", "2", "3"]);
                }
                fn render(&self, _: &mut Frame) {
                    panic!("headless render");
                }
            }
            let clones = Arc::new(AtomicUsize::new(0));
            let (ack, ack_rx) = mpsc::sync_channel(0);
            let state = Counted {
                clones: Arc::clone(&clones),
                events: vec![],
                ack,
            };
            let mut app = App {
                progress: SharedProgress::new(state.clone()),
                cached: state,
                cancel: false,
                reported: false,
            };
            clones.store(0, Ordering::SeqCst);
            let (tx, rx) = mpsc::channel();
            let mut tx = Some(tx);
            let mut index = 0;
            HEADLESS_TICK.with_borrow_mut(|slot| {
                *slot = Some(Box::new(move || {
                    if let Some(sender) = tx.as_ref() {
                        sender
                            .send(SequenceStatus::Whole(
                                andean_condor::core::sequence::Status::Completed {
                                    id: index.to_string(),
                                },
                            ))
                            .unwrap();
                        ack_rx.recv_timeout(Duration::from_secs(10)).unwrap();
                        index += 1;
                        if index == 4 {
                            tx.take();
                        }
                    }
                }));
            });
            let result = app.run(rx, Arc::new(AtomicBool::new(false)));
            HEADLESS_TICK.with_borrow_mut(|slot| slot.take());
            result.unwrap();
            assert!(app.reported);
            assert_eq!(
                clones.load(Ordering::SeqCst),
                1,
                "only the final complete state is needed headlessly"
            );
            println!("HEADLESS_EVENT_MAPPING_ASSERTIONS_COMPLETED");
        }

        #[cfg(target_os = "linux")]
        fn check_interrupt_listener(phase: u8, test: &str) -> Result<()> {
            assert!((1..=3).contains(&phase));
            let Some(root) = std::env::var_os("INTERRUPT_LISTENER_ROOT") else {
                let root = tempfile::tempdir()?;
                let output = std::process::Command::new("python3")
                    .arg(concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/tests/interrupt/pty.py"
                    ))
                    .arg(std::env::current_exe()?)
                    .arg(root.path())
                    .arg("listener")
                    .arg(phase.to_string())
                    .output()?;
                anyhow::ensure!(
                    output.status.success(),
                    "{}\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                let stdout = String::from_utf8_lossy(&output.stdout);
                assert!(
                    stdout.contains(&format!("listener phase {phase}: selected child completed"))
                );
                eprintln!("{stdout}");
                return Ok(());
            };
            assert_eq!(std::env::var("INTERRUPT_LISTENER_TEST")?, test);
            let root = std::path::PathBuf::from(root);
            let mut prior: Option<crate::interrupt::Interrupt> = None;
            for invocation in 0..if phase == 1 { 1 } else { 2 } {
                let cancellation = crate::interrupt::Interrupt::default();
                let mut listener = crate::interrupt::Listener::start(cancellation.clone())?;
                assert!(crate::interrupt::Listener::start(cancellation.clone()).is_err());
                for stage in 0..2 {
                    let (tx, rx) = mpsc::channel();
                    let display_cancel = cancellation.clone();
                    let display = thread::spawn(move || TestApp::new().run(rx, display_cancel));
                    if stage == 0 {
                        tx.send(SequenceStatus::Whole(
                            andean_condor::core::sequence::Status::Completed {
                                id: "interrupt-listener".into(),
                            },
                        ))?;
                    } else {
                        assert!(
                            !cancellation.token.is_cancelled(),
                            "stale job or stage cancellation"
                        );
                        std::fs::write(root.join(format!("ready-{invocation}")), b"ready")?;
                        let deadline = std::time::Instant::now() + Duration::from_secs(10);
                        while !cancellation.token.is_cancelled() {
                            anyhow::ensure!(
                                std::time::Instant::now() < deadline,
                                "first request watchdog"
                            );
                            thread::sleep(Duration::from_millis(5));
                        }
                        assert!(
                            !cancellation.token.is_forced(),
                            "duplicate handler escalated first event"
                        );
                        if let Some(prior) = &prior {
                            assert!(prior.token.is_forced());
                        }
                        std::fs::write(root.join(format!("graceful-{invocation}")), b"graceful")?;
                        while !cancellation.token.is_forced() {
                            anyhow::ensure!(
                                std::time::Instant::now() < deadline,
                                "force request watchdog"
                            );
                            thread::sleep(Duration::from_millis(5));
                        }
                    }
                    drop(tx);
                    display.join().expect("display")?;
                }
                listener.join()?;
                let error = cancellation.finish(Ok(())).unwrap_err();
                assert_eq!(
                    error.downcast_ref::<crate::interrupt::Interrupted>().unwrap().0,
                    if invocation == 0 { 143 } else { 130 }
                );
                // Refuse the same forced root before any source owner can open.
                let input = andean_condor::models::input::Input::Video {
                    path: root.join("must-not-open.mkv"),
                    import_method: andean_condor::models::input::ImportMethod::FFMS2 {
                        index: None,
                        cache_path: None,
                    },
                    filters: vec![],
                };
                let error = crate::tui::open_input_cancel(&input, &cancellation.token)
                    .err()
                    .expect("forced opening");
                assert!(error.is::<andean_condor::utils::cancellation::Cancelled>());
                cancellation.request(130)?;
                cancellation.request(143)?;
                prior = Some(cancellation);
            }
            if phase == 3 {
                // Join must drain an already handed-off OS event even before its thread starts.
                let cancellation = crate::interrupt::Interrupt::default();
                let mut listener = crate::interrupt::Listener::start(cancellation.clone())?;
                signal_hook::low_level::raise(signal_hook::consts::SIGTERM)?;
                listener.join()?;
                assert!(cancellation.token.is_cancelled());
                assert!(!cancellation.token.is_forced());
            }
            println!("INTERRUPT_LISTENER_PHASE_{phase}_ASSERTIONS_COMPLETED");
            Ok(())
        }

        #[cfg(target_os = "linux")]
        #[test]
        fn interrupt_listener_across_stages_preserves_first_signal_without_duplicate_delivery()
        -> Result<()> {
            check_interrupt_listener(
                1,
                "apps::tests::internal_integration::interrupt_listener_across_stages_preserves_first_signal_without_duplicate_delivery",
            )
        }

        #[cfg(target_os = "linux")]
        #[test]
        fn interrupt_listener_after_join_can_register_a_fresh_job() -> Result<()> {
            check_interrupt_listener(
                2,
                "apps::tests::internal_integration::interrupt_listener_after_join_can_register_a_fresh_job",
            )
        }

        #[cfg(target_os = "linux")]
        #[test]
        fn interrupt_listener_join_drains_an_already_received_sigterm() -> Result<()> {
            check_interrupt_listener(
                3,
                "apps::tests::internal_integration::interrupt_listener_join_drains_an_already_received_sigterm",
            )
        }

        #[cfg(unix)]
        fn check_terminal_ownership_in_pty(group: &str) {
            let output = std::process::Command::new("python3")
                .arg(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/src/apps/pty_test_support.py"
                ))
                .arg(std::env::current_exe().unwrap())
                .arg(format!("matrix-{group}"))
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            let stdout = String::from_utf8_lossy(&output.stdout);
            for stream in ["stderr", "stdout"] {
                assert!(
                    stdout.contains(&format!(
                        "{stream}/{group}: ownership cases passed; termios restored"
                    )),
                    "{stdout}"
                );
            }
            eprintln!("{stdout}");
        }

        #[cfg(unix)]
        macro_rules! terminal_ownership_cases {
            ($($parent:ident, $child:ident, $group:literal, [$($case:literal),+];)+) => {
                $(
                    #[test]
                    fn $parent() {
                        check_terminal_ownership_in_pty($group);
                    }

                    #[test]
                    #[ignore = "requires the paired parent's isolated controlling PTY"]
                    fn $child() -> Result<()> {
                        assert!(std::env::var_os("TUI_PTY_STREAM").is_some());
                        assert_eq!(
                            std::env::var("TUI_OWNERSHIP_TEST")?,
                            concat!("apps::tests::internal_integration::", stringify!($child))
                        );
                        check_terminal_ownership(&[$($case),+])
                    }
                )+
            };
        }

        #[cfg(unix)]
        terminal_ownership_cases! {
            terminal_raw_enable_failure_does_not_clean_unacquired_screen,
            terminal_raw_enable_failure_does_not_clean_unacquired_screen_in_controlled_pty,
            "raw", ["raw"];
            terminal_entry_or_construction_failure_cleans_acquired_resources_once,
            terminal_entry_or_construction_failure_cleans_acquired_resources_once_in_controlled_pty,
            "initialization", ["enter_write", "enter_flush", "construct"];
            terminal_restore_error_retains_typed_cause_and_cancels,
            terminal_restore_error_retains_typed_cause_and_cancels_in_controlled_pty,
            "restore", ["resize", "final_draw", "raw_cleanup", "restore_flush"];
            terminal_cleanup_error_attempts_remaining_resources_once_and_keeps_first_cause,
            terminal_cleanup_error_attempts_remaining_resources_once_and_keeps_first_cause_in_controlled_pty,
            "cleanup", ["leave_cleanup", "show_cleanup", "flush_cleanup", "broken"];
            terminal_successful_restore_and_drop_are_idempotent_and_restore_prior_hook,
            terminal_successful_restore_and_drop_are_idempotent_and_restore_prior_hook_in_controlled_pty,
            "normal", ["normal"];
            inactive_terminal_has_no_raw_or_screen_effects,
            inactive_terminal_has_no_raw_or_screen_effects_in_controlled_pty,
            "inactive", ["inactive"];
            terminal_unwind_cancels_releases_resources_and_restores_prior_hook,
            terminal_unwind_cancels_releases_resources_and_restores_prior_hook_in_controlled_pty,
            "unwind", ["unwind"];
        }

        #[cfg(unix)]
        #[test]
        fn tui_selected_pty_restore_renders_final_frame_and_keeps_redirected_stream_clean() {
            check_selected_stream_pty();
        }

        #[cfg(unix)]
        #[test]
        #[ignore = "requires the paired parent's isolated controlling PTY"]
        fn tui_selected_pty_restore_renders_final_frame_and_keeps_redirected_stream_clean_in_controlled_pty()
        -> Result<()> {
            assert!(std::env::var_os("TUI_PTY_STREAM").is_some());
            assert_eq!(
                std::env::var("TUI_SELECTED_TEST")?,
                "apps::tests::internal_integration::tui_selected_pty_restore_renders_final_frame_and_keeps_redirected_stream_clean_in_controlled_pty"
            );
            run_selected_stream_pty_child()?;
            println!("SELECTED_TERMINAL_ASSERTIONS_COMPLETED");
            Ok(())
        }

        #[cfg(unix)]
        #[test]
        fn tui_render_write_failure_restores_terminal_and_returns_broken_pipe_with_cancellation() {
            check_render_failure_pty();
        }

        #[cfg(unix)]
        #[test]
        #[ignore = "requires the paired parent's isolated controlling PTY"]
        fn tui_render_write_failure_restores_terminal_and_returns_broken_pipe_with_cancellation_in_controlled_pty()
        -> Result<()> {
            assert!(std::env::var_os("TUI_PTY_STREAM").is_some());
            assert_eq!(
                std::env::var("TUI_SELECTED_TEST")?,
                "apps::tests::internal_integration::tui_render_write_failure_restores_terminal_and_returns_broken_pipe_with_cancellation_in_controlled_pty"
            );
            run_render_failure_pty_child()?;
            println!("SELECTED_TERMINAL_ASSERTIONS_COMPLETED");
            Ok(())
        }
    }

    struct RenderFailureWriter {
        writer: Box<dyn Write + Send>,
        armed: Arc<AtomicBool>,
        failed: bool,
    }

    impl Write for RenderFailureWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.armed.swap(false, Ordering::SeqCst) {
                self.failed = true;
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "TERMINAL_OWNERSHIP render failure",
                ));
            }
            self.writer.write(bytes)
        }
        fn flush(&mut self) -> io::Result<()> {
            if self.failed {
                return Err(io::Error::other(
                    "TERMINAL_OWNERSHIP secondary cleanup failure",
                ));
            }
            self.writer.flush()
        }
    }

    #[derive(Default)]
    struct Recording {
        bytes: Vec<u8>,
        flushes: Vec<usize>,
        fail_write: bool,
        fail_flush: bool,
        writes: usize,
        fail_write_at: Option<usize>,
        fail_flush_at: Option<usize>,
        fail_bytes: Option<Vec<u8>>,
        attempts: Vec<Vec<u8>>,
        fail_show_flush: bool,
        last_operation: Option<&'static str>,
    }

    #[derive(Clone, Default)]
    struct Recorder(Arc<Mutex<Recording>>);

    impl Write for Recorder {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let mut recording = self.0.lock().unwrap();
            recording.last_operation = Some("write");
            recording.writes += 1;
            recording.attempts.push(bytes.to_vec());
            let matches = recording
                .fail_bytes
                .as_ref()
                .is_some_and(|needle| bytes.windows(needle.len()).any(|part| part == needle));
            if matches {
                recording.fail_bytes = None;
            }
            if matches || recording.fail_write || recording.fail_write_at == Some(recording.writes)
            {
                return Err(io::Error::other("selected writer write failure"));
            }
            recording.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            let mut recording = self.0.lock().unwrap();
            recording.last_operation = Some("flush");
            let len = recording.bytes.len();
            recording.flushes.push(len);
            if recording.fail_flush
                || recording.fail_flush_at == Some(recording.flushes.len())
                || (recording.fail_show_flush && recording.bytes.ends_with(b"\x1b[?25h"))
            {
                return Err(io::Error::other("selected writer flush failure"));
            }
            Ok(())
        }
    }

    struct TestApp {
        progress: SharedProgress<()>,
        state: (),
        cancelled: bool,
        render_failure: Option<Arc<AtomicBool>>,
        panic_case: Option<String>,
    }

    impl TestApp {
        fn new() -> Self {
            Self {
                progress: SharedProgress::new(()),
                state: (),
                cancelled: false,
                render_failure: None,
                panic_case: None,
            }
        }
    }

    impl TuiApp for TestApp {
        type State = ();

        fn shared_progress(&self) -> &SharedProgress<()> {
            &self.progress
        }
        fn cached_state(&self) -> &() {
            &self.state
        }
        fn cached_state_mut(&mut self) -> &mut () {
            &mut self.state
        }
        fn attempted_cancel(&self) -> bool {
            self.cancelled
        }
        fn attempted_cancel_mut(&mut self) -> &mut bool {
            &mut self.cancelled
        }
        fn cancel_message(&self) -> &'static str {
            "cancelled"
        }
        fn map_progress(_: SequenceStatus, _: &mut ()) -> bool {
            if std::env::var_os("TUI_LOGGING_ROOT").is_some() {
                emit_console_diagnostics();
            }
            assert!(
                !std::env::var("TUI_PANIC_CASE").is_ok_and(|case| case == "progress"),
                "TUI_PANIC-progress-payload"
            );
            false
        }
        fn render(&self, frame: &mut Frame) {
            assert!(
                self.panic_case.as_deref() != Some("ui"),
                "TUI_PANIC-ui-payload"
            );
            if let Some(armed) = &self.render_failure {
                armed.store(true, Ordering::SeqCst);
            }
            frame.render_widget("SELECTED_STREAM-final-render", frame.area());
        }
    }

    #[cfg(unix)]
    #[test]
    fn console_logging_preserves_terminal_and_output_lifecycle() {
        let output = std::process::Command::new("python3")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/src/apps/pty_test_support.py"
            ))
            .arg(std::env::current_exe().unwrap())
            .arg("logging")
            .output()
            .expect("python3 is required for the Unix PTY regression test");
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn emit_console_diagnostics() {
        static STAGE: AtomicU64 = AtomicU64::new(0);
        let stage = STAGE.fetch_add(1, Ordering::SeqCst) + 1;
        tracing::info!(target: "andean_condor::core::sequence::scene_detector", "TUI_LOGGING-scene-model-{stage}");
        tracing::error!(target: "andean_condor::core::sequence::quality_check", "TUI_LOGGING-metric-fallback-{stage}");
        tracing::warn!(target: "andean_condor::core::encoder", "TUI_LOGGING-retry-{stage}");
        tracing::error!(target: "andean_condor::core::encoder", "TUI_LOGGING-encoder-failure-{stage}");
        if std::env::var("TUI_LOGGING_CASE").unwrap() == "overflow" {
            let long = "🦜".repeat(3000);
            for index in 0..20 {
                tracing::warn!(target: "andean_condor::core::encoder", "TUI_LOGGING-overflow-{index:02}-{long}");
            }
        }
        let root = std::path::PathBuf::from(std::env::var_os("TUI_LOGGING_ROOT").unwrap());
        std::fs::write(root.join(format!("produced-{stage}")), b"ready").unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !root.join(format!("release-{stage}")).exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "TUI_LOGGING producer not released"
            );
            thread::yield_now();
        }
    }

    struct ConsoleFailureWriter {
        writer: Box<dyn Write + Send>,
        fail_write: Arc<AtomicBool>,
        fail_flush: Arc<AtomicBool>,
    }

    impl Write for ConsoleFailureWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.fail_write.swap(false, Ordering::SeqCst) {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "TUI_LOGGING draw failure",
                ));
            }
            if bytes.windows(6).any(|part| part == b"\x1b[?25h") {
                tracing::warn!(target: "california_condor", "TUI_LOGGING-final-control-warning");
            }
            self.writer.write(bytes)
        }
        fn flush(&mut self) -> io::Result<()> {
            self.writer.flush()?;
            if self.fail_flush.swap(false, Ordering::SeqCst) {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "TUI_LOGGING restore failure",
                ));
            }
            Ok(())
        }
    }

    // Isolated child only: actual global subscriber and terminal owner, never
    // a test-only pause switch. Removing owner integration must fail the driver.
    #[cfg(unix)]
    #[test]
    fn console_logging_child() -> Result<()> {
        let Ok(case) = std::env::var("TUI_LOGGING_CASE") else {
            return Ok(());
        };
        let root = std::path::PathBuf::from(std::env::var_os("TUI_LOGGING_ROOT").unwrap());
        let log = root.join("events.log");
        println!("TUI_LOGGING_BEGIN\nsource-output-sentinel\nTUI_LOGGING_END");
        stdout().flush()?;
        std::panic::set_hook(Box::new(|_| {
            assert!(!crossterm::terminal::is_raw_mode_enabled().unwrap());
            tracing::warn!(target: "california_condor", "TUI_LOGGING-prior-hook");
        }));
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<()> {
            let _logging_guard = crate::logging::init_logging(
                tracing_subscriber::filter::LevelFilter::INFO,
                &log,
                tracing_subscriber::filter::LevelFilter::DEBUG,
            )?;
            if matches!(
                case.as_str(),
                "normal" | "overflow" | "headless" | "testmode"
            ) {
                for stage in 1..=2 {
                    let (tx, rx) = mpsc::channel();
                    tx.send(SequenceStatus::Whole(
                        andean_condor::core::sequence::Status::Completed {
                            id: "logging".into(),
                        },
                    ))?;
                    drop(tx);
                    TestApp::new().run(rx, Arc::new(AtomicBool::new(false)))?;
                    tracing::info!(target: "california_condor", "TUI_LOGGING-between-stages-{stage}");
                    tracing::warn!(target: "california_condor", "TUI_LOGGING-returned-stage-warning-{stage}");
                }
            } else {
                let cancelled = Arc::new(AtomicBool::new(false));
                let fail_write = Arc::new(AtomicBool::new(false));
                let fail_flush = Arc::new(AtomicBool::new(false));
                let mut owner = TerminalOwner::new(
                    Box::new(ConsoleFailureWriter {
                        writer: selected_writer(stdout().is_terminal()),
                        fail_write: Arc::clone(&fail_write),
                        fail_flush: Arc::clone(&fail_flush),
                    }),
                    Arc::clone(&cancelled),
                );
                if case == "raw" {
                    owner.raw_mode = |enabled| {
                        assert!(enabled);
                        thread::spawn(emit_console_diagnostics).join().unwrap();
                        Err(io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            "TUI_LOGGING raw failure",
                        ))
                    };
                }
                if case == "startup" {
                    BEFORE_RAW.with_borrow_mut(|slot| {
                        *slot = Some(Box::new(|| {
                            assert!(
                                thread::spawn(|| {
                                    emit_console_diagnostics();
                                    panic!("TUI_LOGGING startup panic");
                                })
                                .join()
                                .is_err()
                            );
                        }));
                    });
                }
                let initialized = init_terminal(owner, true, |backend| {
                    if case == "construct" {
                        thread::spawn(emit_console_diagnostics).join().unwrap();
                        return Err(io::Error::new(
                            io::ErrorKind::NotFound,
                            "TUI_LOGGING construct failure",
                        ));
                    }
                    Terminal::new(backend)
                });
                let error = match initialized {
                    Err(error) => error,
                    Ok(mut terminal) => {
                        thread::spawn(emit_console_diagnostics).join().unwrap();
                        if case == "panic" {
                            tracing::debug!(target: "california_condor", "TUI_LOGGING-final-flush-marker");
                            panic!("TUI_LOGGING owner unwind");
                        }
                        let mut app = TestApp::new();
                        if case == "draw" {
                            fail_write.store(true, Ordering::SeqCst);
                            let error =
                                terminal.terminal.draw(|frame| app.render(frame)).unwrap_err();
                            terminal.cleanup()?;
                            error.into()
                        } else {
                            fail_flush.store(true, Ordering::SeqCst);
                            app.restore(&mut terminal).unwrap_err()
                        }
                    },
                };
                let expected = match case.as_str() {
                    "raw" => io::ErrorKind::PermissionDenied,
                    "draw" => io::ErrorKind::BrokenPipe,
                    "startup" => io::ErrorKind::Other,
                    _ => io::ErrorKind::NotFound,
                };
                assert_eq!(error.downcast_ref::<io::Error>().unwrap().kind(), expected);
                assert!(cancelled.load(Ordering::SeqCst));
                tracing::warn!(target: "california_condor", "TUI_LOGGING-after-error");
            }
            // Backlog plus a final marker, checked immediately after this owner
            // returns. No polling/sleep may hide a missing WorkerGuard drop.
            for index in 0..1024 {
                tracing::debug!(target: "california_condor", "TUI_LOGGING-shutdown-{index:04}");
            }
            tracing::debug!(target: "california_condor", "TUI_LOGGING-final-flush-marker");
            if case == "restore" {
                anyhow::bail!("TUI_LOGGING early error return");
            }
            Ok(())
        }));
        if case == "panic" {
            assert!(outcome.is_err());
        } else if case == "restore" {
            assert_eq!(
                outcome.unwrap().unwrap_err().to_string(),
                "TUI_LOGGING early error return"
            );
        } else {
            outcome.unwrap()?;
        }
        assert!(
            std::fs::read_to_string(&log)?.contains("TUI_LOGGING-final-flush-marker"),
            "logging owner did not flush"
        );
        assert!(
            crate::logging::init_logging(
                tracing_subscriber::filter::LevelFilter::INFO,
                &root.join("rejected.log"),
                tracing_subscriber::filter::LevelFilter::DEBUG,
            )
            .is_err(),
            "duplicate subscriber setup must return an error"
        );
        Ok(())
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn application_panics_restore_terminal_and_report_once() {
        let output = std::process::Command::new("python3")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/src/apps/pty_test_support.py"
            ))
            .arg(std::env::current_exe().unwrap())
            .arg("panic")
            .output()
            .expect("python3 is required for the Unix PTY regression test");
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[cfg(target_os = "linux")]
    fn owned_fixture(root: &std::path::Path) -> Result<andean_condor::core::process::OwnedProcess> {
        use andean_condor::{core::process::OwnedProcess, utils::cancellation::Cancellation};
        use std::{
            fmt::Write as _,
            io::BufRead,
            process::{Command, Stdio},
        };
        let mut command = Command::new("sh");
        command
            .args(["-c", "sleep 300 & echo $$ $!; wait"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut owned = OwnedProcess::spawn(&mut command, &Cancellation::default())?;
        let mut pids = String::new();
        io::BufReader::new(owned.stdout.take().unwrap()).read_line(&mut pids)?;
        let mut identities = String::new();
        for pid in pids.split_whitespace() {
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
            let fields = stat.rsplit_once(") ").unwrap().1.split_whitespace().collect::<Vec<_>>();
            writeln!(identities, "{pid} {} {}", fields[19], fields[2])?;
        }
        std::fs::write(root.join("pids"), identities)?;
        Ok(owned)
    }

    #[cfg(target_os = "linux")]
    fn record_reaped(root: &std::path::Path) -> Result<()> {
        let pids = std::fs::read_to_string(root.join("pids"))?;
        let leader = pids.split_whitespace().next().unwrap();
        assert!(
            !std::path::Path::new(&format!("/proc/{leader}")).exists(),
            "direct child must be reaped before outcome"
        );
        std::fs::write(root.join("unwound"), b"owned child reaped")?;
        Ok(())
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn startup_panic_child() -> Result<()> {
        use std::sync::atomic::AtomicUsize;
        let Some(root) = std::env::var_os("TUI_PANIC_ROOT") else {
            return Ok(());
        };
        static RAW_ON: AtomicUsize = AtomicUsize::new(0);
        static RAW_OFF: AtomicUsize = AtomicUsize::new(0);
        fn raw_mode(enabled: bool) -> io::Result<()> {
            if enabled {
                RAW_ON.fetch_add(1, Ordering::SeqCst);
            } else {
                RAW_OFF.fetch_add(1, Ordering::SeqCst);
            }
            set_raw_mode(enabled)
        }
        let root = std::path::PathBuf::from(root);
        println!("TUI_PANIC_BEGIN\nsource-output-sentinel\nTUI_PANIC_END");
        let owned = owned_fixture(&root)?;
        BEFORE_RAW.with_borrow_mut(|slot| {
            *slot = Some(Box::new(|| {
                assert!(thread::spawn(|| panic!("TUI_PANIC-startup-payload")).join().is_err());
            }));
        });
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut owner = TerminalOwner::new(
            selected_writer(stdout().is_terminal()),
            Arc::clone(&cancelled),
        );
        owner.raw_mode = raw_mode;
        let error = init_terminal(owner, true, Terminal::new)
            .err()
            .expect("retired initialization must fail");
        assert_eq!(error.to_string(), "terminal retired");
        assert_eq!(
            (
                RAW_ON.load(Ordering::SeqCst),
                RAW_OFF.load(Ordering::SeqCst)
            ),
            (0, 0)
        );
        assert!(!crossterm::terminal::is_raw_mode_enabled()?);
        assert!(cancelled.load(Ordering::SeqCst));
        // A separately acquired raw resource must also be cleaned without a writer.
        let mut owner = TerminalOwner::new(Box::new(io::sink()), cancelled);
        owner.raw_mode = raw_mode;
        raw_mode(true)?;
        owner.raw = true;
        owner.writer.inner.lock().unwrap().take();
        owner.cleanup()?;
        owner.cleanup()?;
        assert_eq!(
            (
                RAW_ON.load(Ordering::SeqCst),
                RAW_OFF.load(Ordering::SeqCst)
            ),
            (1, 1)
        );
        assert!(!crossterm::terminal::is_raw_mode_enabled()?);
        drop(owned);
        record_reaped(&root)?;
        println!("TUI_PANIC_STARTUP_RETIRED_RAW_OFF");
        Err(error)
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cli_panic_child() -> std::process::ExitCode {
        let Ok(case) = std::env::var("TUI_PANIC_CASE") else {
            return std::process::ExitCode::SUCCESS;
        };
        crate::TEST_ENTRY_RUN.with_borrow_mut(|slot| {
            *slot = Some(Box::new(move || {
                let root = std::path::PathBuf::from(std::env::var_os("TUI_PANIC_ROOT").unwrap());
                println!("TUI_PANIC_BEGIN\nsource-output-sentinel\nTUI_PANIC_END");
                let mut owned = Some(owned_fixture(&root)?);
                let cancelled = Arc::new(AtomicBool::new(false));
                let mut app = TestApp::new();
                if matches!(case.as_str(), "cli-ui" | "cli-mixed") {
                    app.panic_case = Some("ui".into());
                }
                let (tx, rx) = mpsc::channel();
                let display_cancelled = Arc::clone(&cancelled);
                let display = thread::spawn(move || app.run(rx, display_cancelled));
                let executed = if matches!(case.as_str(), "cli-worker" | "cli-aggregate") {
                    let owned = owned.take();
                    andean_condor::core::encoder::join_worker(
                        thread::spawn(move || -> Result<()> {
                            let _owned = owned;
                            let deadline = std::time::Instant::now() + Duration::from_secs(5);
                            while PANIC_STAGE.lock().unwrap().upgrade().is_none() {
                                assert!(
                                    std::time::Instant::now() < deadline,
                                    "stage never registered"
                                );
                                thread::yield_now();
                            }
                            if stdout().is_terminal() || stderr().is_terminal() {
                                while !crossterm::terminal::is_raw_mode_enabled().unwrap() {
                                    assert!(
                                        std::time::Instant::now() < deadline,
                                        "terminal never acquired"
                                    );
                                    thread::yield_now();
                                }
                            }
                            panic!("TUI_PANIC-worker-payload");
                        }),
                        "TUI_PANIC library worker join",
                    )
                    .map(|()| ((), Vec::new()))
                } else if case == "cli-unreported" {
                    Err(andean_condor::utils::CaughtPanic {
                        operation: "unreported fixture".into(),
                        message: "TUI_PANIC-unreported-payload".into(),
                    }
                    .into())
                } else if matches!(case.as_str(), "cli-mixed" | "cli-error") {
                    Err(anyhow::Error::from(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "TUI_PANIC-nonpanic-error",
                    ))
                    .context("TUI_PANIC primary context"))
                } else {
                    Ok(((), Vec::new()))
                };
                let executed = if case == "cli-aggregate" {
                    let panic = executed.expect_err("library worker panicked");
                    assert!(panic.downcast_ref::<andean_condor::utils::CaughtPanic>().is_some());
                    Err(andean_condor::core::encoder::EncoderProcessError {
                        result: None,
                        stdout: String::from("codec-stdout-sentinel").into(),
                        stderr: String::from("codec-stderr-sentinel").into(),
                        failures: vec![
                            panic,
                            io::Error::new(
                                io::ErrorKind::PermissionDenied,
                                "TUI_PANIC-nonpanic-error",
                            )
                            .into(),
                        ],
                    }
                    .into())
                } else {
                    executed
                };
                if matches!(case.as_str(), "cli-error" | "cli-unreported") {
                    drop(tx);
                }
                let result = crate::tui::finish_stage("TUI_PANIC", executed, || display.join());
                let error = result.expect_err("fixture stage must fail");
                if matches!(case.as_str(), "cli-ui" | "cli-worker") {
                    let payload = if case == "cli-ui" {
                        "TUI_PANIC-ui-payload"
                    } else {
                        "TUI_PANIC-worker-payload"
                    };
                    assert!(
                        format!("{error:#}").contains(payload),
                        "library panic cause lost"
                    );
                    assert!(error.downcast_ref::<andean_condor::utils::CaughtPanic>().is_some());
                }
                if matches!(case.as_str(), "cli-mixed" | "cli-error") {
                    assert_eq!(
                        error.downcast_ref::<io::Error>().unwrap().kind(),
                        io::ErrorKind::PermissionDenied
                    );
                    assert!(format!("{error:#}").contains("TUI_PANIC-nonpanic-error"));
                }
                if case == "cli-aggregate" {
                    let aggregate = error
                        .downcast_ref::<andean_condor::core::encoder::EncoderProcessError>()
                        .unwrap();
                    assert!(
                        aggregate.failures[0]
                            .downcast_ref::<andean_condor::utils::CaughtPanic>()
                            .is_some()
                    );
                    assert_eq!(
                        aggregate.failures[1].downcast_ref::<io::Error>().unwrap().kind(),
                        io::ErrorKind::PermissionDenied
                    );
                    assert!(format!("{error:#}").contains("TUI_PANIC-worker-payload"));
                }
                std::fs::write(
                    root.join("library-error"),
                    b"original library error retained",
                )?;
                drop(owned);
                record_reaped(&root)?;
                Err(error)
            }));
        });
        crate::cli_entry_point()
    }

    // Exact-name subprocess protocol: no hook mutation in the parent libtest.
    #[cfg(target_os = "linux")]
    #[test]
    fn application_panic_child() -> Result<()> {
        use std::sync::atomic::AtomicUsize;
        let Ok(case) = std::env::var("TUI_PANIC_CASE") else {
            return Ok(());
        };
        let root = std::path::PathBuf::from(std::env::var_os("TUI_PANIC_ROOT").unwrap());
        let custom = case == "subsequent";
        let calls = Arc::new(AtomicUsize::new(0));
        if custom {
            let calls = Arc::clone(&calls);
            std::panic::set_hook(Box::new(move |info| {
                calls.fetch_add(1, Ordering::SeqCst);
                assert!(!crossterm::terminal::is_raw_mode_enabled().unwrap());
                eprintln!("TUI_PANIC_PRIOR: {info}");
            }));
        }
        println!("TUI_PANIC_BEGIN\nsource-output-sentinel\nTUI_PANIC_END");
        stdout().flush()?;
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<()> {
            let mut owned = Some(owned_fixture(&root)?);
            let cancelled = Arc::new(AtomicBool::new(false));
            let mut app = TestApp::new();
            app.panic_case = Some(case.clone());
            if matches!(case.as_str(), "main" | "subsequent") {
                crate::TEST_ENTRY_RUN.with_borrow_mut(|slot| {
                    *slot = Some(Box::new(move || {
                        let _owned = owned;
                        let _terminal = if stdout().is_terminal() || stderr().is_terminal() {
                            Some(app.init(Arc::clone(&cancelled))?)
                        } else {
                            None
                        };
                        panic!("TUI_PANIC-main-payload");
                    }));
                });
                return crate::entry_point();
            }
            let (tx, rx) = mpsc::channel();
            let worker = (case == "worker").then(|| {
                let owned = owned.take();
                thread::spawn(move || {
                    let _owned = owned;
                    // Wait for the actual selected stream to enter raw mode.
                    let deadline = std::time::Instant::now() + Duration::from_secs(5);
                    while !crossterm::terminal::is_raw_mode_enabled().unwrap() {
                        assert!(std::time::Instant::now() < deadline, "UI never initialized");
                        thread::yield_now();
                    }
                    panic!("TUI_PANIC-worker-payload");
                })
            });
            if case == "progress" {
                tx.send(SequenceStatus::Whole(
                    andean_condor::core::sequence::Status::Completed { id: "panic".into() },
                ))?;
            }
            let display_cancelled = Arc::clone(&cancelled);
            let displayed = thread::spawn(move || app.run(rx, display_cancelled)).join();
            drop(tx);
            if let Some(worker) = worker {
                assert!(worker.join().is_err());
            }
            assert!(cancelled.load(Ordering::SeqCst), "panic must cancel stage");
            if case == "ui" {
                assert!(displayed.is_err(), "UI panic must escape display thread");
            } else {
                assert!(
                    displayed.unwrap().is_err(),
                    "worker panic must fail application"
                );
            }
            Ok(())
        }));
        if matches!(case.as_str(), "main" | "subsequent") {
            assert!(
                outcome.is_err(),
                "main panic must unwind through entry_point"
            );
        } else {
            outcome.expect("test boundary must not panic")?;
        }
        record_reaped(&root)?;
        if custom {
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            let mut app = TestApp::new();
            let mut terminal = app.init(Arc::new(AtomicBool::new(false)))?;
            assert!(std::panic::catch_unwind(|| panic!("TUI_PANIC-second-payload")).is_err());
            app.restore(&mut terminal)?;
            drop(terminal);
            assert!(std::panic::catch_unwind(|| panic!("TUI_PANIC-third-payload")).is_err());
            assert_eq!(calls.load(Ordering::SeqCst), 3);
            eprintln!("TUI_PANIC_PRIOR_COUNT=3");
        }
        // Return an error only after real unwind/join cleanup; no exit simulation.
        anyhow::bail!("TUI_PANIC failed application outcome")
    }

    #[test]
    fn selected_writer_receives_commands_render_and_flush() {
        let recorder = Recorder::default();
        let writer: Box<dyn Write + Send> = Box::new(recorder.clone());
        let mut writer = BufWriter::new(writer);
        enter_alternate_screen(&mut writer).unwrap();
        {
            let recording = recorder.0.lock().unwrap();
            assert_eq!(recording.bytes, b"\x1b[?1049h");
            assert_eq!(recording.flushes, [8]);
        }
        let mut terminal = Terminal::with_options(
            CrosstermBackend::new(writer),
            TerminalOptions {
                viewport: Viewport::Fixed(Rect::new(0, 0, 80, 3)),
            },
        )
        .unwrap();
        leave_alternate_screen(terminal.backend_mut(), 3).unwrap();
        terminal.draw(|frame| TestApp::new().render(frame)).unwrap();
        finish_restore(terminal.backend_mut()).unwrap();

        let (recorded_bytes, flushes) = {
            let recording = recorder.0.lock().unwrap();
            (recording.bytes.clone(), recording.flushes.clone())
        };
        let bytes = String::from_utf8_lossy(&recorded_bytes);
        assert!(
            bytes.starts_with("\x1b[?1049h\x1b[?1049l\n\n\n"),
            "{bytes:?}"
        );
        assert!(bytes.contains("SELECTED_STREAM-final-render"), "{bytes:?}");
        assert!(bytes.ends_with("\x1b[?25h"), "{bytes:?}");
        assert_eq!(flushes.last(), Some(&recorded_bytes.len()));
    }

    #[test]
    fn restore_commands_return_the_selected_writer_write_and_flush_errors() {
        for finish in [false, true] {
            for fail_flush in [false, true] {
                let mut recorder = Recorder::default();
                {
                    let mut recording = recorder.0.lock().unwrap();
                    recording.fail_write = !fail_flush;
                    recording.fail_flush = fail_flush;
                }
                let result = if finish {
                    finish_restore(&mut recorder)
                } else {
                    leave_alternate_screen(&mut recorder, 3)
                };
                let error = result.expect_err("must return the selected writer's error");
                assert_eq!(error.kind(), io::ErrorKind::Other);
                assert_eq!(
                    error.to_string(),
                    if fail_flush {
                        "selected writer flush failure"
                    } else {
                        "selected writer write failure"
                    }
                );
            }
        }
    }

    #[cfg(unix)]
    fn check_selected_stream_pty() {
        let output = std::process::Command::new("python3")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/src/apps/pty_test_support.py"
            ))
            .arg(std::env::current_exe().unwrap())
            .output()
            .expect("python3 is required for the Unix PTY regression test");
        assert!(
            output.status.success(),
            "PTY regression failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(
            stdout.matches("SELECTED_TERMINAL_ASSERTIONS_COMPLETED").count(),
            2
        );
        eprintln!("{stdout}");
    }

    #[cfg(unix)]
    fn run_selected_stream_pty_child() -> Result<()> {
        if std::env::var_os("TUI_PTY_STREAM").is_none() {
            return Ok(());
        }
        assert!(std::env::var_os("CONDOR_TEST_MODE").is_none());
        let use_stdout = std::env::var("TUI_PTY_STREAM")? == "stdout";
        assert_eq!(stdout().is_terminal(), use_stdout);
        assert_eq!(stderr().is_terminal(), !use_stdout);
        println!("SELECTED_STREAM_BEGIN\npayload");
        let mut app = TestApp::new();
        let mut terminal = app.init(Arc::new(AtomicBool::new(false)))?;
        terminal.terminal.draw(|frame| app.render(frame))?;
        app.restore(&mut terminal)?;
        println!("SELECTED_STREAM_END");
        Ok(())
    }

    #[cfg(unix)]
    fn check_render_failure_pty() {
        let output = std::process::Command::new("python3")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/src/apps/pty_test_support.py"
            ))
            .arg(std::env::current_exe().unwrap())
            .arg("render-failure")
            .output()
            .expect("python3 is required for the Unix PTY regression test");
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(
            stdout.matches("SELECTED_TERMINAL_ASSERTIONS_COMPLETED").count(),
            2
        );
        eprintln!("{stdout}");
    }

    #[cfg(unix)]
    fn check_terminal_ownership(cases: &[&str]) -> Result<()> {
        if std::env::var_os("TUI_PTY_STREAM").is_none() {
            return Ok(());
        }
        use std::sync::atomic::AtomicUsize;
        static RAW_ON: AtomicUsize = AtomicUsize::new(0);
        static RAW_OFF: AtomicUsize = AtomicUsize::new(0);
        static RAW_FAIL_ON: AtomicBool = AtomicBool::new(false);
        static RAW_FAIL_OFF: AtomicBool = AtomicBool::new(false);
        fn raw_mode(enabled: bool) -> io::Result<()> {
            if enabled {
                RAW_ON.fetch_add(1, Ordering::SeqCst);
                if RAW_FAIL_ON.load(Ordering::SeqCst) {
                    return Err(io::Error::other("TERMINAL_OWNERSHIP raw enable failure"));
                }
                enable_raw_mode()
            } else {
                RAW_OFF.fetch_add(1, Ordering::SeqCst);
                disable_raw_mode()?;
                if RAW_FAIL_OFF.load(Ordering::SeqCst) {
                    return Err(io::Error::other("TERMINAL_OWNERSHIP raw disable failure"));
                }
                Ok(())
            }
        }
        let previous = std::panic::take_hook();
        let hook_calls = Arc::new(AtomicUsize::new(0));
        let calls = Arc::clone(&hook_calls);
        std::panic::set_hook(Box::new(move |_| {
            calls.fetch_add(1, Ordering::SeqCst);
        }));
        for &case in cases {
            let recorder = Recorder::default();
            let cancelled = Arc::new(AtomicBool::new(false));
            RAW_ON.store(0, Ordering::SeqCst);
            RAW_OFF.store(0, Ordering::SeqCst);
            RAW_FAIL_ON.store(case == "raw", Ordering::SeqCst);
            RAW_FAIL_OFF.store(case == "raw_cleanup", Ordering::SeqCst);
            {
                let mut recording = recorder.0.lock().unwrap();
                if case == "enter_flush" {
                    recording.fail_flush_at = Some(1);
                }
                if case == "enter_write" {
                    recording.fail_write_at = Some(1);
                }
            }
            let mut owner = TerminalOwner::new(Box::new(recorder.clone()), Arc::clone(&cancelled));
            owner.raw_mode = raw_mode;
            let initialized = init_terminal(owner, case != "inactive", |backend| {
                if case == "construct" {
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        "TERMINAL_OWNERSHIP terminal size failure",
                    ));
                }
                Terminal::new(backend)
            });
            let mut app = TestApp::new();
            let result = match initialized {
                Err(error) => Err(error),
                Ok(mut terminal) => {
                    {
                        let mut recording = recorder.0.lock().unwrap();
                        recording.fail_bytes = match case {
                            "resize" => Some(b"\x1b[2J".to_vec()),
                            "final_draw" => Some(b"SELECTED_STREAM-final-render".to_vec()),
                            "leave_cleanup" => Some(b"\x1b[?1049l".to_vec()),
                            "show_cleanup" => Some(b"\x1b[?25h".to_vec()),
                            _ => None,
                        };
                        recording.fail_flush = case == "flush_cleanup";
                        recording.fail_show_flush = case == "restore_flush";
                        recording.fail_write = case == "broken";
                    }
                    if case == "unwind" {
                        assert!(
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                let _terminal = terminal;
                                panic!("TERMINAL_OWNERSHIP unwind");
                            }))
                            .is_err()
                        );
                        Ok(())
                    } else {
                        let result = if matches!(
                            case,
                            "resize" | "final_draw" | "raw_cleanup" | "restore_flush" | "normal"
                        ) {
                            app.restore(&mut terminal)
                        } else {
                            terminal.cleanup().map_err(Into::into)
                        };
                        let snapshot = {
                            let r = recorder.0.lock().unwrap();
                            (r.bytes.clone(), r.flushes.clone(), r.writes)
                        };
                        app.restore(&mut terminal)?;
                        terminal.cleanup()?;
                        drop(terminal);
                        let r = recorder.0.lock().unwrap();
                        assert_eq!(
                            snapshot,
                            (r.bytes.clone(), r.flushes.clone(), r.writes),
                            "{case}: restore/Drop not idempotent"
                        );
                        result
                    }
                },
            };
            assert!(
                !crossterm::terminal::is_raw_mode_enabled()?,
                "{case}: raw leaked"
            );
            assert_eq!(
                RAW_ON.load(Ordering::SeqCst),
                usize::from(case != "inactive"),
                "{case}"
            );
            assert_eq!(
                RAW_OFF.load(Ordering::SeqCst),
                usize::from(!matches!(case, "raw" | "inactive")),
                "{case}"
            );
            let success = matches!(case, "normal" | "inactive" | "unwind");
            assert_eq!(result.is_ok(), success, "{case}: {result:?}");
            assert_eq!(
                cancelled.load(Ordering::SeqCst),
                !success || case == "unwind",
                "{case}: cancellation"
            );
            if let Err(error) = result {
                let error = error.downcast_ref::<io::Error>().expect("typed I/O cause retained");
                let (kind, message) = match case {
                    "raw" => (
                        io::ErrorKind::Other,
                        "TERMINAL_OWNERSHIP raw enable failure",
                    ),
                    "construct" => (
                        io::ErrorKind::NotFound,
                        "TERMINAL_OWNERSHIP terminal size failure",
                    ),
                    "raw_cleanup" => (
                        io::ErrorKind::Other,
                        "TERMINAL_OWNERSHIP raw disable failure",
                    ),
                    "enter_flush" | "flush_cleanup" | "restore_flush" => {
                        (io::ErrorKind::Other, "selected writer flush failure")
                    },
                    _ => (io::ErrorKind::Other, "selected writer write failure"),
                };
                assert_eq!(
                    (error.kind(), error.to_string()),
                    (kind, message.to_string()),
                    "{case}"
                );
            }
            let recording = recorder.0.lock().unwrap();
            let bytes = String::from_utf8_lossy(&recording.bytes);
            if matches!(case, "raw" | "inactive") {
                assert!(
                    recording.attempts.is_empty() && recording.flushes.is_empty(),
                    "{case}"
                );
            } else {
                for command in [b"\x1b[?1049l".as_slice(), b"\x1b[?25h".as_slice()] {
                    let attempts = recording
                        .attempts
                        .iter()
                        .filter(|bytes| bytes.windows(command.len()).any(|part| part == command))
                        .count();
                    assert_eq!(attempts, 1, "{case}: {command:?}: {bytes:?}");
                }
                assert_eq!(
                    recording.last_operation,
                    Some("flush"),
                    "{case}: cleanup flush skipped"
                );
                if case == "enter_flush" {
                    assert_eq!(
                        bytes.matches("\x1b[?1049h").count(),
                        1,
                        "failed entry replayed after cleanup: {bytes:?}"
                    );
                }
            }
            drop(recording);
            let before = hook_calls.load(Ordering::SeqCst);
            assert!(
                std::panic::catch_unwind(|| panic!("TERMINAL_OWNERSHIP hook sentinel")).is_err()
            );
            assert_eq!(
                hook_calls.load(Ordering::SeqCst),
                before + 1,
                "{case}: hook not restored"
            );
            println!("TERMINAL_OWNERSHIP_CASE_{case}_PASSED");
        }
        std::panic::set_hook(previous);
        println!("TERMINAL_OWNERSHIP_SELECTED_CASES_PASSED");
        Ok(())
    }

    #[cfg(unix)]
    fn run_render_failure_pty_child() -> Result<()> {
        let Ok(stream) = std::env::var("TUI_PTY_STREAM") else {
            return Ok(());
        };
        assert!(std::env::var_os("CONDOR_TEST_MODE").is_none());
        let use_stdout = stream == "stdout";
        let armed = Arc::new(AtomicBool::new(false));
        let writer: Box<dyn Write + Send> = if use_stdout {
            Box::new(stdout())
        } else {
            Box::new(stderr())
        };
        TEST_WRITER.with_borrow_mut(|slot| {
            *slot = Some(Box::new(RenderFailureWriter {
                writer,
                armed: Arc::clone(&armed),
                failed: false,
            }));
        });
        let mut app = TestApp::new();
        app.render_failure = Some(armed);
        let cancelled = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let error = app.run(rx, Arc::clone(&cancelled)).expect_err("forced render must fail");
        drop(tx);
        assert_eq!(
            error.downcast_ref::<io::Error>().unwrap().kind(),
            io::ErrorKind::BrokenPipe
        );
        assert_eq!(error.to_string(), "TERMINAL_OWNERSHIP render failure");
        assert!(
            cancelled.load(Ordering::SeqCst),
            "display failure must cancel before returning"
        );
        println!("TERMINAL_OWNERSHIP_ERROR_RETURNED_CANCEL_TRUE");
        Ok(())
    }

    #[test]
    fn always_run_headless_in_test_mode() {
        assert!(run_headless(true, true, true));
        assert!(run_headless(false, false, true));
    }

    #[test]
    fn a_terminal_on_either_stream_runs_the_tui() {
        assert!(!run_headless(true, false, false));
        assert!(!run_headless(false, true, false));
        assert!(!run_headless(true, true, false));
    }

    #[test]
    fn no_terminal_at_all_runs_headless() {
        // The `docker run` without `-t` case: raw mode would fail with ENXIO.
        assert!(run_headless(false, false, false));
    }
}
