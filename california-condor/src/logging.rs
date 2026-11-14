use std::{
    collections::HashMap,
    env,
    fmt::{Debug, Write},
    io::{self, IsTerminal, Write as _},
    path::Path,
    sync::{Mutex, MutexGuard},
};

use anyhow::Context;
use tracing::{Level, Metadata};
use tracing_appender::{
    non_blocking::WorkerGuard,
    rolling::{RollingFileAppender, Rotation},
};
use tracing_subscriber::{EnvFilter, filter::LevelFilter, fmt, fmt::MakeWriter, prelude::*};

pub const DEFAULT_CONSOLE_LEVEL: LevelFilter = LevelFilter::INFO;
pub const DEFAULT_LOG_LEVEL: LevelFilter = LevelFilter::DEBUG;

const DEFERRED_EVENTS: usize = 8;
const DEFERRED_BYTES: usize = 1024;
static CONSOLE: Mutex<ConsoleState> = Mutex::new(ConsoleState {
    owners: 0,
    events: [[0; DEFERRED_BYTES]; DEFERRED_EVENTS],
    lengths: [0; DEFERRED_EVENTS],
    count: 0,
    dropped: 0,
    truncated: 0,
});

struct ConsoleState {
    owners: usize,
    events: [[u8; DEFERRED_BYTES]; DEFERRED_EVENTS],
    lengths: [usize; DEFERRED_EVENTS],
    count: usize,
    dropped: usize,
    truncated: usize,
}

/// Silences only console events until the terminal owner's final flush.
/// Lock order: terminal owner -> console -> stderr; console writers never
/// acquire terminal/registry locks. One event holds the console lock throughout
/// output, so a writer already in flight finishes before acquisition.
pub(crate) struct ConsolePause;

impl ConsolePause {
    pub(crate) fn new() -> Self {
        CONSOLE.lock().unwrap_or_else(|e| e.into_inner()).owners += 1;
        Self
    }
}

impl Drop for ConsolePause {
    fn drop(&mut self) {
        let mut state = CONSOLE.lock().unwrap_or_else(|e| e.into_inner());
        state.owners -= 1;
        if state.owners != 0 {
            return;
        }
        let mut stderr = io::stderr().lock();
        for index in 0..state.count {
            let bytes = &state.events[index][..state.lengths[index]];
            let _ = stderr.write_all(bytes);
            // A bounded prefix may end before the formatter's newline/reset.
            if !bytes.ends_with(b"\n") {
                if stderr.is_terminal() {
                    let _ = stderr.write_all(b"\x1b[0m");
                }
                let _ = stderr.write_all(b"\n");
            }
        }
        if state.dropped != 0 || state.truncated != 0 {
            let _ = writeln!(
                stderr,
                "WARN deferred console diagnostics: {} dropped, {} truncated (limit: {DEFERRED_EVENTS} events, {DEFERRED_BYTES} bytes each)",
                state.dropped, state.truncated
            );
        }
        let _ = stderr.flush();
        state.count = 0;
        state.dropped = 0;
        state.truncated = 0;
    }
}

struct Console;

struct ConsoleWriter<'a> {
    state: MutexGuard<'a, ConsoleState>,
    retain: bool,
    bytes: [u8; DEFERRED_BYTES],
    len: usize,
    truncated: bool,
}

impl<'a> MakeWriter<'a> for Console {
    type Writer = ConsoleWriter<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        ConsoleWriter {
            state: CONSOLE.lock().unwrap_or_else(|e| e.into_inner()),
            retain: false,
            bytes: [0; DEFERRED_BYTES],
            len: 0,
            truncated: false,
        }
    }

    fn make_writer_for(&'a self, metadata: &Metadata<'_>) -> Self::Writer {
        let mut writer = self.make_writer();
        writer.retain = *metadata.level() <= Level::WARN;
        writer
    }
}

impl io::Write for ConsoleWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.state.owners == 0 {
            return io::stderr().write(bytes);
        }
        // ponytail: retain the first eight warning/error prefixes only. File
        // logging is independent; a larger UI message subsystem is not needed.
        if self.retain {
            let len = bytes.len().min(DEFERRED_BYTES - self.len);
            self.bytes[self.len..self.len + len].copy_from_slice(&bytes[..len]);
            self.len += len;
            self.truncated |= len != bytes.len();
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.state.owners == 0 {
            io::stderr().flush()
        } else {
            Ok(())
        }
    }
}

impl Drop for ConsoleWriter<'_> {
    fn drop(&mut self) {
        if self.state.owners == 0 || !self.retain {
            return;
        }
        if self.state.count == DEFERRED_EVENTS {
            self.state.dropped = self.state.dropped.saturating_add(1);
            return;
        }
        // Don't split a UTF-8 code point at the byte ceiling.
        let len = std::str::from_utf8(&self.bytes[..self.len])
            .map_or_else(|error| error.valid_up_to(), |text| text.len());
        let index = self.state.count;
        self.state.events[index][..len].copy_from_slice(&self.bytes[..len]);
        self.state.lengths[index] = len;
        self.state.count += 1;
        self.state.truncated += usize::from(self.truncated);
    }
}

// Define our module configuration structure
#[derive(Debug, Clone)]
struct ModuleConfig {
    console_level: LevelFilter,
    file_level: LevelFilter,
    console_enabled: bool,
    file_enabled: bool,
}

impl Default for ModuleConfig {
    fn default() -> Self {
        Self {
            console_level: DEFAULT_CONSOLE_LEVEL,
            file_level: DEFAULT_LOG_LEVEL,
            console_enabled: true,
            file_enabled: true,
        }
    }
}

/// Initialize logging with per-module configuration. The caller must retain the
/// returned guard through all workers; dropping it flushes the file on normal
/// return, early errors and unwind. Subscriber setup errors also drop the guard.
pub fn init_logging(
    console_level: LevelFilter,
    log_path: &Path,
    file_level: LevelFilter,
) -> anyhow::Result<WorkerGuard> {
    // Set up our module configurations
    let mut module_configs = HashMap::new();

    // Configure condor CLI module
    module_configs.insert(
        "california_condor",
        ModuleConfig {
            console_level,
            file_level,
            console_enabled: true,
            file_enabled: true,
        },
    );

    // Configure core module
    module_configs.insert(
        "andean_condor",
        ModuleConfig {
            console_level,
            file_level,
            console_enabled: true,
            file_enabled: true,
        },
    );

    // Allow override through environment variables
    if let Ok(rust_log) = env::var("RUST_LOG") {
        for directive in rust_log.split(',') {
            if let Some((module, level)) = directive.split_once('=')
                && let (Some(config), Ok(level)) =
                    (module_configs.get_mut(module), level.parse::<LevelFilter>())
            {
                config.console_level = level;
                config.file_level = level;
            }
        }
    }

    // Create our filters
    let console_filter = {
        let mut filter = String::new();
        for (module, config) in &module_configs {
            if config.console_enabled {
                if !filter.is_empty() {
                    filter.push(',');
                }
                write!(&mut filter, "{}={}", module, config.console_level)
                    .expect("write to string should work");
            }
        }
        EnvFilter::try_new(&filter).unwrap()
    };

    let file_filter = {
        let mut filter = String::new();
        for (module, config) in &module_configs {
            if config.file_enabled {
                if !filter.is_empty() {
                    filter.push(',');
                }
                write!(&mut filter, "{}={}", module, config.file_level)
                    .expect("write to string should work");
            }
        }
        EnvFilter::try_new(&filter).unwrap()
    };

    // Set up file appender
    let file_appender = RollingFileAppender::new(
        Rotation::NEVER,
        log_path.parent().expect("Log path should have a parent"),
        log_path.file_name().unwrap_or_else(|| std::ffi::OsStr::new("condor.log")),
    );

    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let file_layer = fmt::layer()
    .with_ansi(false)
    .with_target(true)
    .with_thread_ids(false)
    .with_thread_names(false)
    .with_file(false)
    .with_line_number(false)
    .with_level(true)
    .with_writer(non_blocking)
    // Apply the filter last
    .with_filter(file_filter);
    let console_layer = fmt::layer()
    .compact()
    .with_ansi(std::io::stderr().is_terminal())
    .with_ansi_sanitization(false)
    .with_target(false)
    .with_file(false)
    .with_thread_ids(false)
    .with_thread_names(false)
    .with_file(false)
    .with_line_number(false)
    .without_time()
    .with_writer(Console)
    // Apply the filter last
    .with_filter(console_filter);

    // Create our subscriber with correctly ordered layers
    let subscriber = tracing_subscriber::registry().with(file_layer).with(console_layer);

    // Set as global default
    tracing::subscriber::set_global_default(subscriber)
        .context("Failed to set global default subscriber")?;

    // Log initialization
    tracing::debug!("Logging system initialized");
    tracing::debug!("Module-specific logging enabled");

    Ok(guard)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logging_filters_preserve_console_policy_and_flush_file_guard() -> anyhow::Result<()> {
        if let Some(root) = env::var_os("LOGGING_FILTER_CHILD") {
            let root = std::path::PathBuf::from(root);
            let _guard = init_logging(
                LevelFilter::INFO,
                &root.join("events.log"),
                LevelFilter::DEBUG,
            )?;
            tracing::debug!(target: "california_condor", "TUI_LOGGING-debug-policy");
            tracing::info!(target: "andean_condor", "TUI_LOGGING-info-policy");
            tracing::warn!(target: "andean_condor", "TUI_LOGGING-warn-policy");
            tracing::info!(target: "foreign_target", "TUI_LOGGING-foreign-policy");
            #[cfg(target_os = "linux")]
            let threads = std::fs::read_dir("/proc/self/task")?.count();
            for _ in 0..3 {
                let error = init_logging(
                    LevelFilter::INFO,
                    &root.join("rejected.log"),
                    LevelFilter::DEBUG,
                )
                .expect_err("duplicate subscriber must return an error, not leak or panic");
                assert!(error.to_string().contains("global default subscriber"));
            }
            #[cfg(target_os = "linux")]
            {
                // WorkerGuard acknowledges shutdown but does not join its OS
                // thread (tracing-appender 0.2.5). Check eventual teardown, not
                // an instantaneous thread count. The final log is emitted only
                // AFTER this check, so this cannot hide a missing final flush.
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                while std::fs::read_dir("/proc/self/task")?.count() != threads {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "rejected setup leaked a logging worker"
                    );
                    std::thread::yield_now();
                }
            }
            tracing::error!(target: "california_condor", "TUI_LOGGING-policy-final");
            return Ok(());
        }
        for overrides in [false, true] {
            let root = tempfile::tempdir()?;
            let mut child = std::process::Command::new(env::current_exe()?);
            child
                .args([
                    "--exact",
                    "logging::tests::logging_filters_preserve_console_policy_and_flush_file_guard",
                    "--nocapture",
                    "--quiet",
                ])
                .env("LOGGING_FILTER_CHILD", root.path())
                .env_remove("RUST_LOG");
            if overrides {
                child.env("RUST_LOG", "andean_condor=warn,california_condor=debug");
            }
            let output = child.output()?;
            assert!(output.status.success(), "{output:?}");
            let console = String::from_utf8(output.stderr)?;
            let stdout = String::from_utf8(output.stdout)?;
            let file = std::fs::read_to_string(root.path().join("events.log"))?;
            assert!(
                !stdout.contains("TUI_LOGGING-"),
                "logs must never use stdout"
            );
            assert!(!console.contains('\x1b') && !file.contains('\x1b'));
            assert_eq!(console.contains("TUI_LOGGING-debug-policy"), overrides);
            assert_eq!(console.contains("TUI_LOGGING-info-policy"), !overrides);
            assert_eq!(file.contains("TUI_LOGGING-info-policy"), !overrides);
            assert!(
                file.contains("TUI_LOGGING-debug-policy") && file.contains("california_condor")
            );
            for log in [&console, &file] {
                assert!(
                    log.contains("TUI_LOGGING-warn-policy")
                        && log.contains("TUI_LOGGING-policy-final")
                );
                assert!(!log.contains("TUI_LOGGING-foreign-policy"));
            }
        }
        Ok(())
    }
}
