//! Streams Y4M frame data through an FFmpeg filtergraph subprocess.
//!
//! FFmpeg reads `yuv4mpegpipe` on stdin and writes the filtered stream on
//! stdout, regenerating the Y4M header (resolution, pixel format, frame
//! rate), so downstream encoders consume FFmpeg's header rather than the
//! input's.
//!
//! Backpressure keeps window-streaming bounded: a slow encoder blocks the
//! bounded output channel, which propagates back to the producer's window.
//! Memory stays O(window) per worker plus FFmpeg's own buffers.

use std::{
    io::{self, Cursor, Read, Write},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};

use crate::{core::process::OwnedProcess, utils::cancellation::Cancellation};
use anyhow::{Context, Result, bail};
use crossbeam_channel::Receiver;

/// Tail of FFmpeg's stderr retained for failure messages.
const STDERR_TAIL_LIMIT: usize = 8 * 1024;
/// Read size for FFmpeg's stdout. Chunk boundaries are irrelevant: the
/// encoder's stdin thread concatenates chunks into one byte stream.
const STDOUT_READ_SIZE: usize = 64 * 1024;

/// Handle to a running FFmpeg filter subprocess bridging a frame producer to
/// a frame consumer.
///
/// Keep this handle with the consumer and call [`Self::check`] when it exits,
/// not after joining a potentially blocked producer. Dropping it also stops
/// and joins the stage. Only the owned FFmpeg process group/job is stopped.
pub struct FfmpegFilter {
    receiver: Option<Receiver<Cursor<Vec<u8>>>>,
    failure: Arc<Mutex<Option<String>>>,
    child: OwnedProcess,
    cancellation: Cancellation,
    stop: Option<crossbeam_channel::Sender<()>>,
    input_thread: Option<JoinHandle<io::Result<()>>>,
    output_thread: Option<JoinHandle<io::Result<()>>>,
    stderr_thread: Option<JoinHandle<Result<Vec<u8>>>>,
    output_eof: Arc<AtomicBool>,
    completed: bool,
}

impl FfmpegFilter {
    /// Spawns `ffmpeg` applying `graph` to the Y4M stream arriving on
    /// `input`, forwarding the filtered stream through the returned handle's
    /// receiver.
    ///
    /// `failure` is shared with the caller so producers can report FFmpeg
    /// errors that would otherwise surface as opaque channel disconnects. It
    /// is filled by [`FfmpegFilter::check`] once FFmpeg exits.
    #[inline]
    pub fn spawn(
        input: Receiver<Cursor<Vec<u8>>>,
        output_bound: usize,
        graph: &str,
        failure: Arc<Mutex<Option<String>>>,
    ) -> Result<Self> {
        Self::spawn_cancel(
            input,
            output_bound,
            graph,
            failure,
            &Cancellation::default(),
        )
    }

    /// Shares the owning scene's forced teardown; local consumer stop remains independent.
    #[inline]
    pub fn spawn_cancel(
        input: Receiver<Cursor<Vec<u8>>>,
        output_bound: usize,
        graph: &str,
        failure: Arc<Mutex<Option<String>>>,
        cancellation: &Cancellation,
    ) -> Result<Self> {
        cancellation.check_admission()?;
        let ffmpeg = which::which("ffmpeg").map_err(|err| {
            anyhow::anyhow!(
                "FFmpeg is required to apply the filtergraph but was not found in PATH: {err}"
            )
        })?;
        let mut child = OwnedProcess::spawn(
            Command::new(ffmpeg)
            .arg("-hide_banner")
            .arg("-v")
            .arg("error")
            .arg("-nostats")
            .arg("-i")
            .arg("pipe:0")
            .arg("-vf")
            .arg(graph)
            .arg("-fps_mode")
            .arg("passthrough")
            // Y4M is nominally 8-bit only; `-strict -1` lets the muxer write
            // high-bit-depth tags (C420p10, ...) that encoders accept.
            .arg("-strict")
            .arg("-1")
            .arg("-f")
            .arg("yuv4mpegpipe")
            .arg("pipe:1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
            cancellation,
        )?;
        let mut stdin = child.stdin.take().expect("FFmpeg should have STDIN");
        let mut stdout = child.stdout.take().expect("FFmpeg should have STDOUT");
        let stderr = child.stderr.take().expect("FFmpeg should have STDERR");

        let (output_tx, output_rx) = crossbeam_channel::bounded::<Cursor<Vec<u8>>>(output_bound);

        // Disconnect wakes both relays even before the first input/output.
        // Killing the owned child also releases blocked pipe I/O.
        let (stop_tx, stop_rx) = crossbeam_channel::bounded::<()>(0);
        let input_stop = stop_rx.clone();
        let input_cancellation = cancellation.clone();
        let input_thread = thread::spawn(move || {
            loop {
                crossbeam_channel::select_biased! {
                    recv(input_cancellation.forced()) -> _ => return Ok(()),
                    recv(input_stop) -> _ => return Ok(()),
                    recv(input) -> chunk => match chunk {
                        Ok(chunk) => stdin.write_all(chunk.get_ref())?,
                        Err(_) => return Ok(()),
                    },
                }
            }
        });

        // FFmpeg can block on a full stderr pipe, so drain it concurrently.
        let stderr_thread = spawn_stderr_drain(stderr);
        let output_eof = Arc::new(AtomicBool::new(false));
        let relay_eof = Arc::clone(&output_eof);
        let output_cancellation = cancellation.clone();
        let output_thread = thread::spawn(move || {
            let mut buffer = vec![0_u8; STDOUT_READ_SIZE];
            loop {
                match stdout.read(&mut buffer) {
                    Ok(0) => {
                        // Publish before dropping the sender: consumer EOF
                        // distinguishes natural completion from early abort.
                        relay_eof.store(true, Ordering::Release);
                        return Ok(());
                    },
                    Ok(read) => crossbeam_channel::select_biased! {
                        recv(output_cancellation.forced()) -> _ => return Ok(()),
                        recv(stop_rx) -> _ => return Ok(()),
                        send(output_tx, Cursor::new(buffer[..read].to_vec())) -> sent => {
                            if sent.is_err() { return Ok(()); }
                        },
                    },
                    Err(err) if err.kind() == io::ErrorKind::Interrupted => {},
                    Err(err) => return Err(err),
                }
            }
        });

        Ok(Self {
            receiver: Some(output_rx),
            failure,
            child,
            cancellation: cancellation.clone(),
            stop: Some(stop_tx),
            input_thread: Some(input_thread),
            output_thread: Some(output_thread),
            stderr_thread: Some(stderr_thread),
            output_eof,
            completed: false,
        })
    }

    /// Transfers the sole output receiver to the consumer, once. The first
    /// chunk carries FFmpeg's regenerated Y4M header. A second take is an error.
    #[inline]
    pub fn take_receiver(&mut self) -> Result<Receiver<Cursor<Vec<u8>>>> {
        self.receiver.take().context("FFmpeg filter output receiver was already taken")
    }

    /// Finishes after the consumer exits, stopping remaining input and joining
    /// every relay. Early consumer exit intentionally aborts FFmpeg; natural
    /// failure retains its status and bounded stderr tail.
    #[inline]
    pub fn check(&mut self) -> Result<()> {
        if !self.completed {
            self.finish();
        }
        self.cancellation.check_forced()?;
        if let Some(message) = self.is_failed() {
            bail!(message);
        }
        Ok(())
    }

    /// The failure message, if FFmpeg has failed.
    #[inline]
    pub fn is_failed(&self) -> Option<String> {
        self.failure.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    fn finish(&mut self) {
        self.receiver.take();
        self.stop.take();
        let mut errors = Vec::new();
        // A disconnected consumer may already have caused FFmpeg's SIGPIPE
        // before cleanup runs. Exit status alone cannot identify a real failure.
        let aborted = !self.output_eof.load(Ordering::Acquire);
        if aborted && let Err(err) = self.child.kill() {
            errors.push(format!("stopping FFmpeg failed: {err}"));
        }
        // This stage alone owns/reaps Child. No mutex or completion worker can
        // prevent stop/kill or race take/drop against a blocking wait.
        match self.child.wait() {
            Ok(status) if !status.success() && !aborted => {
                errors.insert(0, format!("FFmpeg exited with {status}"));
            },
            Ok(_) => {},
            Err(err) => errors.push(format!("waiting on FFmpeg failed: {err}")),
        }
        if let Some(worker) = self.input_thread.take() {
            match worker.join() {
                Ok(Ok(())) => {},
                Ok(Err(err)) if aborted && err.kind() == io::ErrorKind::BrokenPipe => {},
                Ok(Err(err)) => errors.push(format!("writing FFmpeg input failed: {err}")),
                Err(_) => errors.push("FFmpeg input forwarder panicked".to_owned()),
            }
        }
        if let Some(worker) = self.output_thread.take() {
            match worker.join() {
                Ok(Ok(())) => {},
                Ok(Err(err)) => errors.push(format!("reading FFmpeg output failed: {err}")),
                Err(_) => errors.push("FFmpeg output relay panicked".to_owned()),
            }
        }
        let tail = match self.stderr_thread.take().map(JoinHandle::join) {
            Some(Ok(Ok(tail))) => tail,
            Some(Ok(Err(err))) => {
                errors.push(format!("reading FFmpeg stderr failed: {err:#}"));
                Vec::new()
            },
            Some(Err(_)) => {
                errors.push("FFmpeg stderr drain panicked".to_owned());
                Vec::new()
            },
            None => Vec::new(),
        };
        if !errors.is_empty() {
            record_failure(
                &self.failure,
                format!("{}{}", errors.join("; "), format_stderr(&tail)),
            );
        }
        self.completed = true;
    }
}

impl Drop for FfmpegFilter {
    #[inline]
    fn drop(&mut self) {
        if !self.completed
            && let Err(err) = self.check()
        {
            tracing::warn!(%err, "FFmpeg filter cleanup failed");
        }
    }
}

/// Drains stderr into a bounded tail, keeping only the last
/// [`STDERR_TAIL_LIMIT`] bytes so a noisy filter cannot grow memory without
/// bound.
fn spawn_stderr_drain(mut stderr: impl Read + Send + 'static) -> JoinHandle<Result<Vec<u8>>> {
    thread::spawn(move || {
        let mut tail: Vec<u8> = Vec::new();
        let mut buffer = [0_u8; 1024];
        loop {
            match stderr.read(&mut buffer) {
                Ok(0) => break,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {},
                Err(err) => {
                    return Err(err)
                        .with_context(|| format!("FFmpeg stderr tail{}", format_stderr(&tail)));
                },
                Ok(read) => {
                    tail.extend_from_slice(&buffer[..read]);
                    if tail.len() > STDERR_TAIL_LIMIT {
                        let excess = tail.len() - STDERR_TAIL_LIMIT;
                        tail.drain(..excess);
                    }
                },
            }
        }
        Ok(tail)
    })
}

/// Records `message` as the stage's failure, overwriting any earlier one.
fn record_failure(failure: &Arc<Mutex<Option<String>>>, message: String) {
    *failure.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(message);
}

/// Formats a stderr tail for appending to a failure message. Returns an empty
/// string when there is nothing to show.
fn format_stderr(tail: &[u8]) -> String {
    let tail = String::from_utf8_lossy(tail);
    let tail = tail.trim();
    if tail.is_empty() {
        String::new()
    } else {
        format!(":\n{tail}")
    }
}

#[cfg(test)]
mod tests;
