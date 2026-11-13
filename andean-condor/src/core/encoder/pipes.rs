use std::{
    io::{self, BufRead, BufReader, Read},
    panic::{AssertUnwindSafe, catch_unwind},
    process::Output,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use super::*;

/// How often the encoder process is sampled for CPU and memory usage.
///
/// Refreshing a process walks the system process table, so sampling it for
/// every progress line costs more than the encoding it is reporting on.
const RESOURCE_SAMPLE_INTERVAL: Duration = Duration::from_secs(1);

/// Samples one encoder process at most once per `RESOURCE_SAMPLE_INTERVAL`.
struct ResourceSampler {
    pid: sysinfo::Pid,
    system: sysinfo::System,
    last: EncodeResourceUsage,
    sampled_at: Option<Instant>,
}

impl ResourceSampler {
    fn new(pid: u32) -> Self {
        Self {
            pid: sysinfo::Pid::from_u32(pid),
            system: sysinfo::System::new(),
            last: EncodeResourceUsage::default(),
            sampled_at: None,
        }
    }

    fn usage(&mut self) -> EncodeResourceUsage {
        let due = self
            .sampled_at
            .is_none_or(|sampled_at| sampled_at.elapsed() >= RESOURCE_SAMPLE_INTERVAL);
        if due {
            self.system.refresh_processes_specifics(
                sysinfo::ProcessesToUpdate::Some(&[self.pid]),
                true,
                sysinfo::ProcessRefreshKind::nothing()
                    .with_cpu()
                    .with_memory(),
            );
            self.last = self.system.process(self.pid).map_or_else(
                EncodeResourceUsage::default,
                |process| EncodeResourceUsage {
                    cpu: process.cpu_usage(),
                    memory: process.memory(),
                },
            );
            self.sampled_at = Some(Instant::now());
        }
        self.last
    }
}

#[derive(Default)]
pub(super) struct PipeOutput {
    bytes: Vec<u8>,
    failure: Option<anyhow::Error>,
}

fn panic_error(payload: &(dyn std::any::Any + Send)) -> anyhow::Error {
    let message = payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("non-string panic");
    crate::utils::CaughtPanic {
        operation: "worker".into(),
        message: message.into(),
    }
    .into()
}

/// Join an owned worker, retaining its error or panic payload and operation context.
#[inline]
pub fn join_worker<T>(handle: JoinHandle<Result<T>>, operation: &str) -> Result<T> {
    handle
        .join()
        .map_err(|payload| panic_error(payload.as_ref()))
        .and_then(|result| result)
        .with_context(|| operation.to_owned())
}

// Capture before parsing: malformed progress and observer failure cannot lose diagnostics.
fn read_pipe(mut reader: impl BufRead, mut report: impl FnMut(&str)) -> PipeOutput {
    let mut output = PipeOutput::default();
    let result = catch_unwind(AssertUnwindSafe(|| -> io::Result<()> {
        let mut record = Vec::new();
        loop {
            let available = match reader.fill_buf() {
                Ok(available) => available,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            };
            if available.is_empty() {
                if let Ok(line) = simdutf8::basic::from_utf8(&record) {
                    report(line);
                }
                return Ok(());
            }
            let delimiter = available.iter().position(|b| matches!(b, b'\r' | b'\n'));
            let count = delimiter.map_or(available.len(), |index| index + 1);
            output.bytes.extend_from_slice(&available[..count]);
            record.extend_from_slice(&available[..count]);
            reader.consume(count);
            if delimiter.is_some() {
                if let Ok(line) = simdutf8::basic::from_utf8(&record) {
                    report(line);
                }
                record.clear();
            }
        }
    }));
    output.failure = match result {
        Ok(result) => result.err().map(Into::into),
        Err(payload) => Some(panic_error(payload.as_ref())),
    };
    output
}

pub(super) fn spawn_reader(
    reader: impl Read + Send + 'static,
    report: impl FnMut(&str) + Send + 'static,
) -> JoinHandle<Result<PipeOutput>> {
    thread::spawn(move || Ok(read_pipe(BufReader::new(reader), report)))
}

pub(super) fn progress_reader(
    base: EncoderBase,
    pass: (u8, u8),
    progress: mpsc::Sender<EncodeProgress>,
    stdout: bool,
    frames: Arc<AtomicU64>,
    pid: Option<u32>,
) -> impl FnMut(&str) + Send {
    let mut progress = Some(progress);
    let mut count = 0;
    let mut sampler = pid.map(ResourceSampler::new);
    move |line| {
        let latest = if stdout && base == EncoderBase::AVM {
            if !line.contains("POC:") {
                return;
            }
            count += 1;
            count
        } else if stdout != (base == EncoderBase::VVenC) {
            // VVenC stats use stdout; other keyed progress stays on stderr.
            return;
        } else if let Some(latest) = base.parse_encoded_frames(line) {
            if latest <= frames.load(Ordering::Relaxed) {
                return;
            }
            latest
        } else {
            return;
        };
        frames.store(latest, Ordering::Relaxed);
        if let Some(sender) = &progress {
            let usage = sampler
                .as_mut()
                .map_or_else(EncodeResourceUsage::default, ResourceSampler::usage);
            if sender
                .send(EncodeProgress {
                    pass,
                    frame: latest as usize,
                    usage,
                })
                .is_err()
            {
                progress = None;
            }
        }
    }
}

pub(super) struct PassWorkers {
    pub stdin: JoinHandle<Result<()>>,
    pub stdout: JoinHandle<Result<PipeOutput>>,
    pub stderr: JoinHandle<Result<PipeOutput>>,
    pub wait: JoinHandle<Result<Output>>,
    pub reap: Option<mpsc::Sender<()>>,
}

impl PassWorkers {
    pub fn finish(
        self,
        encoder: EncoderBase,
        parameters: Vec<String>,
        feed: Result<()>,
    ) -> Result<EncoderResult> {
        // No propagation until every handle has joined, including on feed/write/panic errors.
        let stdin = join_worker(self.stdin, "Writing encoder STDIN");
        let stdout = join_worker(self.stdout, "Reading encoder STDOUT");
        let stderr = join_worker(self.stderr, "Reading encoder STDERR");
        if let Some(reap) = self.reap {
            let _ = reap.send(());
        }
        let wait = join_worker(self.wait, "Waiting for encoder child");
        let mut failures = Vec::new();
        for result in [feed, stdin] {
            if let Err(error) = result {
                failures.push(error);
            }
        }
        let mut capture = |result: Result<PipeOutput>, operation: &str| {
            let output = match result {
                Ok(output) => output,
                Err(error) => {
                    failures.push(error);
                    PipeOutput::default()
                },
            };
            if let Some(error) = output.failure {
                failures.push(error.context(operation.to_owned()));
            }
            StringOrBytes::from(output.bytes)
        };
        let stdout = capture(stdout, "Reading encoder STDOUT");
        let stderr = capture(stderr, "Reading encoder STDERR");
        let status = match wait {
            Ok(output) => Some(output.status),
            Err(error) => {
                failures.push(error);
                None
            },
        };
        if !failures.is_empty() {
            let result = status.map(|status| EncoderResult {
                encoder,
                parameters,
                status,
                stdout: stdout.clone(),
                stderr: stderr.clone(),
            });
            return Err(EncoderProcessError {
                result,
                stdout,
                stderr,
                failures,
            }
            .into());
        }
        Ok(EncoderResult {
            encoder,
            parameters,
            status: status.context("Encoder wait returned no status")?,
            stdout,
            stderr,
        })
    }
}

/// Worker failures after all pipes and the child have been joined. Diagnostics are
/// exact bytes; `result` is absent only when waiting failed (no invented exit status).
#[derive(Debug)]
pub struct EncoderProcessError {
    pub result: Option<EncoderResult>,
    pub stdout: StringOrBytes,
    pub stderr: StringOrBytes,
    pub failures: Vec<anyhow::Error>,
}

impl Display for EncoderProcessError {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // A rejected invocation closes stdin; report the codec, not its broken pipe, first.
        let failed = self.result.as_ref().filter(|result| !result.status.success());
        if let Some(result) = failed {
            writeln!(f, "{result}")?;
        }
        for error in &self.failures {
            writeln!(f, "{error:#}")?;
        }
        if failed.is_some() {
            return Ok(());
        }
        if let Some(result) = &self.result {
            Display::fmt(result, f)
        } else {
            write!(
                f,
                "Encoder status unavailable\nSTDOUT:\n{}\nSTDERR:\n{}",
                self.stdout, self.stderr
            )
        }
    }
}

impl std::error::Error for EncoderProcessError {
    #[inline]
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.result.as_ref().map(|result| result as &dyn std::error::Error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Completion(Arc<AtomicU64>);
    impl Drop for Completion {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct FaultReader {
        prefix: Option<&'static [u8]>,
        panic: bool,
        _completion: Completion,
    }

    impl Read for FaultReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if let Some(prefix) = self.prefix.take() {
                buf[..prefix.len()].copy_from_slice(prefix);
                return Ok(prefix.len());
            }
            assert!(!self.panic, "injected reader panic");
            Err(io::Error::other("injected reader error"))
        }
    }

    #[cfg(unix)]
    #[test]
    fn process_display_prioritizes_failed_codec_but_retains_io_and_raw_bytes() {
        use std::os::unix::process::ExitStatusExt;
        for status in [Some(42), Some(0), None] {
            let result = status.map(|code| EncoderResult {
                encoder: EncoderBase::X264,
                parameters: vec!["--bad-flag".into()],
                status: std::process::ExitStatus::from_raw(code << 8),
                stdout: b"stdout\xff".to_vec().into(),
                stderr: b"codec rejected flag\xfe".to_vec().into(),
            });
            let error = EncoderProcessError {
                result,
                stdout: b"stdout\xff".to_vec().into(),
                stderr: b"codec rejected flag\xfe".to_vec().into(),
                failures: vec![
                    anyhow::Error::from(io::Error::from(io::ErrorKind::BrokenPipe))
                        .context("Writing encoder STDIN"),
                ],
            };
            let text = error.to_string();
            if status == Some(42) {
                assert!(text.starts_with("Encoder ("), "{text}");
                assert!(
                    text.find("42").expect("status")
                        < text.find("Writing encoder STDIN").expect("worker failure")
                );
            } else {
                assert!(
                    text.starts_with("Writing encoder STDIN"),
                    "IO is primary without a failed child: {text}"
                );
            }
            assert!(text.contains("broken pipe"), "{text}");
            assert!(
                matches!(&error.stderr, StringOrBytes::Bytes(bytes) if bytes == b"codec rejected flag\xfe")
            );
            let source = std::error::Error::source(&error);
            if let Some(code) = status {
                assert_eq!(
                    source
                        .expect("typed result source")
                        .downcast_ref::<EncoderResult>()
                        .expect("result")
                        .status
                        .code(),
                    Some(code)
                );
            } else {
                assert!(source.is_none());
                assert!(text.contains("Encoder status unavailable"));
                assert!(!text.contains("exit status:"), "no fabricated status");
            }
        }
    }

    #[test]
    fn interrupted_reads_retain_fragments_bytes_and_progress() {
        struct InterruptedReader(usize);
        impl Read for InterruptedReader {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                let step = self.0;
                self.0 += 1;
                let data: &[u8] = match step {
                    0 | 2 | 4 => return Err(io::ErrorKind::Interrupted.into()),
                    1 => b"1/4 fra",
                    3 => b"mes\r\nbad\xff\n3/4 fra",
                    5 => b"mes",
                    _ => b"",
                };
                buf[..data.len()].copy_from_slice(data);
                Ok(data.len())
            }
        }

        let (tx, rx) = mpsc::channel();
        let reader = spawn_reader(
            InterruptedReader(0),
            progress_reader(
                EncoderBase::X264,
                (1, 1),
                tx,
                false,
                Arc::new(AtomicU64::new(0)),
                None,
            ),
        );
        let captured = join_worker(reader, "interrupted pipe regression").expect("reader joined");
        assert!(captured.failure.is_none(), "{:?}", captured.failure);
        assert_eq!(captured.bytes, b"1/4 frames\r\nbad\xff\n3/4 frames");
        assert_eq!(rx.into_iter().map(|p| p.frame).collect::<Vec<_>>(), [1, 3]);
    }

    #[test]
    fn invalid_utf8_progress_records_preserve_bytes_and_later_counts() {
        for (base, stdout, data) in [
            (
                EncoderBase::X264,
                false,
                b"1/4 frames\r\nbad\xff\n3/4 frames".as_slice(),
            ),
            (
                EncoderBase::AVM,
                true,
                b"POC: 0\r\nbad\xff\nPOC: 1".as_slice(),
            ),
        ] {
            let (tx, rx) = mpsc::channel();
            let reader = spawn_reader(
                Cursor::new(data.to_vec()),
                progress_reader(base, (1, 1), tx, stdout, Arc::new(AtomicU64::new(0)), None),
            );
            let captured = join_worker(reader, "regression pipe reader").expect("reader joined");
            assert!(captured.failure.is_none());
            assert_eq!(captured.bytes, data);
            assert_eq!(
                rx.into_iter().map(|p| p.frame).collect::<Vec<_>>(),
                if stdout { vec![1, 2] } else { vec![1, 3] }
            );
        }
    }

    #[test]
    fn reader_writer_wait_errors_and_panics_are_all_joined_with_context() {
        for panic in [false, true] {
            let completed = Arc::new(AtomicU64::new(0));
            let stdout = spawn_reader(
                FaultReader {
                    prefix: Some(b"stdout\xff\r"),
                    panic,
                    _completion: Completion(Arc::clone(&completed)),
                },
                |_| {},
            );
            let stderr = spawn_reader(
                FaultReader {
                    prefix: Some(b"stderr\xfe\n"),
                    panic,
                    _completion: Completion(Arc::clone(&completed)),
                },
                |_| {},
            );
            let writer_done = Completion(Arc::clone(&completed));
            let wait_done = Completion(Arc::clone(&completed));
            let workers = PassWorkers {
                reap: None,
                stdin: thread::spawn(move || {
                    let _done = writer_done;
                    assert!(!panic, "injected writer panic");
                    Err(io::Error::other("injected writer error").into())
                }),
                stdout,
                stderr,
                wait: thread::spawn(move || {
                    let _done = wait_done;
                    assert!(!panic, "injected wait panic");
                    Err(io::Error::other("injected wait error").into())
                }),
            };
            let error = workers
                .finish(
                    EncoderBase::X264,
                    vec![],
                    Err(anyhow::anyhow!("injected decode error")),
                )
                .expect_err("every worker failure is retained");
            let error =
                error.downcast_ref::<EncoderProcessError>().expect("structured diagnostics");
            assert_eq!(error.failures.len(), 5);
            assert!(
                error.result.is_none(),
                "wait failure must not invent a status"
            );
            assert!(matches!(&error.stdout, StringOrBytes::Bytes(b) if b == b"stdout\xff\r"));
            assert!(matches!(&error.stderr, StringOrBytes::Bytes(b) if b == b"stderr\xfe\n"));
            let text = error.to_string();
            for context in
                ["STDIN", "STDOUT", "STDERR", "Waiting for encoder child", "decode error"]
            {
                assert!(text.contains(context), "{text}");
            }
            assert_eq!(
                completed.load(Ordering::SeqCst),
                4,
                "all worker owners dropped before return"
            );
        }
    }
}
