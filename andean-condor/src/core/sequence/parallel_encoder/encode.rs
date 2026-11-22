use std::{
    collections::{BTreeMap, VecDeque},
    fs,
    io::Cursor,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use crate::utils::cancellation::Cancellation;
use anyhow::Result;
use crossbeam_channel::Receiver;
use tracing::debug;

use super::{
    ParallelEncoder, ParallelEncoderOptions, STREAM_WINDOW,
    error::ParallelEncoderError,
    lifecycle::{Stop, retain_error},
    progress::{AttemptProgress, Progress, drain_finished_results, relay_progress},
    task::{ParallelEncoderResult, ResultStream, Task},
};
use crate::{
    core::{
        encoder::{EncodeProgress, EncoderResult},
        input::{Input, clip_info::ClipInfo},
        sequence::{SequenceStatus, Status, ffmpeg_filter::FfmpegFilter},
    },
    utils::semaphore::Semaphore,
};

/// The frame channel an encoder reads, plus the filter stage feeding it.
pub(super) type FilteredFrames = (Receiver<Cursor<Vec<u8>>>, Option<FfmpegFilter>);

/// Inserts the scene's FFmpeg filter between the frame producer and the
/// encoder. The encoder consumes the Y4M header FFmpeg regenerates.
///
/// `filter_failures` is `None` for callers that report failures through
/// [`FfmpegFilter::check`] instead of a shared cell.
pub(super) fn spawn_filter(
    frames_rx: Receiver<Cursor<Vec<u8>>>,
    task: &Task,
    filter_failures: Option<&BTreeMap<usize, Arc<Mutex<Option<String>>>>>,
    window: usize,
    cancellation: &Cancellation,
) -> Result<FilteredFrames> {
    match task.encoder.ffmpeg_filter() {
        Some(graph) => {
            let failure = filter_failures
                .and_then(|cells| cells.get(&task.original_index))
                .cloned()
                .unwrap_or_else(|| Arc::new(Mutex::new(None)));
            let mut stage =
                FfmpegFilter::spawn_cancel(frames_rx, window, graph, failure, cancellation)?;
            Ok((stage.take_receiver()?, Some(stage)))
        },
        None => Ok((frames_rx, None)),
    }
}

/// A finished scene encode, and whether it is usable.
pub(super) struct SceneOutcome {
    pub(super) temp_output: std::path::PathBuf,
    pub(super) bytes: u64,
    pub(super) usable: bool,
}

/// Renames a usable encode to its final path, or discards it and flags the
/// error. A failed filter truncates the stream, so a short encode must not
/// survive as the scene's output.
pub(super) fn finalize_scene(
    outcome: SceneOutcome,
    output: &std::path::Path,
    progress: &mut AttemptProgress<'_>,
    encoder_errored: &AtomicBool,
) -> Result<()> {
    if outcome.usable {
        #[cfg(all(test, target_os = "linux"))]
        super::tests::shutdown::internal_integration::fault("size", progress.scene())?;
        super::artifact::protect_existing(output)?;
        fs::rename(outcome.temp_output, output)?;
        #[cfg(all(test, target_os = "linux"))]
        super::tests::artifact_validation::resume_policy::internal_integration::after_rename(
            output,
        );
        progress.commit(outcome.bytes);
    } else {
        let _ = fs::remove_file(&outcome.temp_output);
        encoder_errored.store(true, Ordering::Relaxed);
    }
    Ok(())
}

/// The consumer owns filter cleanup and joins the progress relay even when
/// encoding fails. A producer may still be blocked on the filter's input.
#[allow(clippy::too_many_arguments)]
pub(super) fn consume_frames(
    task: &Task,
    frames: Receiver<Cursor<Vec<u8>>>,
    temp: &std::path::Path,
    clip_info: &ClipInfo,
    progress: &AttemptProgress<'_>,
    stop: &Stop,
    window: usize,
) -> Result<EncoderResult> {
    #[cfg(all(test, target_os = "linux"))]
    if let Some(result) =
        super::tests::dispatch::internal_integration::consume(task, &frames, temp, stop)
    {
        return result;
    }
    let cancellation = &stop.cancellation.active();
    let (frames, mut filter) = spawn_filter(frames, task, None, window, cancellation)?;
    thread::scope(|scope| {
        let (tx, rx) = std::sync::mpsc::channel::<EncodeProgress>();
        let relay = scope.spawn(move || {
            stop.catch(
                &format!("Scene {} progress relay", task.original_index),
                || {
                    #[cfg(all(test, target_os = "linux"))]
                    super::tests::shutdown::internal_integration::fault(
                        "relay",
                        task.original_index,
                    )?;
                    relay_progress(rx, progress)
                },
            )
        });
        let encoded = stop.attempt(&format!("Scene {} encoder", task.original_index), || {
            #[cfg(all(test, target_os = "linux"))]
            super::tests::shutdown::internal_integration::fault("worker", task.original_index)?;
            let result = task
                .encoder
                // encode_attempts retains the exclusive TempDir through this call.
                .encode_owned_with_stream_cancel(frames, temp, tx, clip_info, cancellation)
                .or_else(|error| {
                    if error.is::<crate::utils::cancellation::Cancelled>() {
                        Err(error)
                    } else {
                        error.downcast::<EncoderResult>()
                    }
                });
            #[cfg(all(test, target_os = "linux"))]
            super::tests::dispatch::internal_integration::consumer_closed("encoder");
            result
        });
        let filtered = filter.as_mut().map_or(Ok(()), |stage| stage.check()).map_err(|err| {
            err.context(RetryableAttempt).context(format!(
                "FFmpeg filter failed while encoding scene {}",
                task.original_index
            ))
        });
        #[cfg(all(test, target_os = "linux"))]
        super::tests::dispatch::internal_integration::consumer_closed("filter");
        let relayed = relay.join().unwrap_or_else(|_| {
            Err(anyhow::anyhow!(
                "Scene {} progress relay panicked",
                task.original_index
            ))
        });
        let filter_error = if cancellation.is_forced() {
            None
        } else {
            filtered.err()
        };
        let mut error = None;
        let result = match encoded {
            Ok(result) if result.status.success() => Some(result),
            Ok(result) => {
                retain_error(
                    &mut error,
                    ParallelEncoderError::EncoderFailed {
                        scene: task.original_index,
                        result,
                    }
                    .into(),
                );
                None
            },
            Err(err) => {
                retain_error(&mut error, err);
                None
            },
        };
        if let Some(filtered) = filter_error {
            // Explain truncated-input fallout without discarding typed codec/raw diagnostics.
            // A filter error must not turn a configuration or worker fault into a retry.
            error = Some(match error {
                Some(encoded) => encoded.context(format!("{filtered:#}")),
                None => filtered,
            });
        }
        if let Err(err) = relayed {
            retain_error(&mut error, err);
        }
        if let Some(err) = error {
            return Err(err);
        }
        Ok(result.expect("successful encode retained its result"))
    })
}

#[derive(Debug, thiserror::Error)]
#[error("Scene attempt failed")]
struct RetryableAttempt;

/// All attempt owners join inside `encode`; only the successful private output escapes.
pub(super) struct EncodedAttempt<'a> {
    _directory: tempfile::TempDir,
    pub(super) path: std::path::PathBuf,
    pub(super) result: EncoderResult,
    pub(super) bytes: u64,
    pub(super) progress: AttemptProgress<'a>,
}

fn retryable(error: &anyhow::Error) -> bool {
    error.is::<RetryableAttempt>()
        || matches!(
            error.downcast_ref::<ParallelEncoderError>(),
            Some(ParallelEncoderError::EncoderFailed { .. })
        )
        || error.is::<EncoderResult>()
        || error
            .downcast_ref::<crate::core::encoder::EncoderProcessError>()
            .is_some_and(|error| {
                // A pipe/process IO failure can retry, not an arbitrary worker panic/cause.
                error.failures.iter().all(|cause| {
                    cause.is::<std::io::Error>() || cause.is::<crossbeam_channel::RecvError>()
                })
            })
}

pub(super) fn encode_attempts<'a>(
    task: &Task,
    options: ParallelEncoderOptions,
    stop: &Stop,
    progress: &'a Progress,
    mut encode: impl FnMut(&std::path::Path, &AttemptProgress<'_>) -> Result<EncoderResult>,
) -> Result<EncodedAttempt<'a>> {
    let terminal_error = |error: anyhow::Error| {
        if stop.cancellation.is_cancelled() && !stop.cancellation.is_forced() {
            error.context(crate::utils::cancellation::Cancelled::Graceful)
        } else {
            error
        }
    };
    let mut last = None;
    for attempt in 1..=options.max_tries {
        if attempt > 1 && stop.stopped(&stop.cancellation) {
            return Err(terminal_error(
                last.expect("replacement attempts retain their preceding failure"),
            ));
        }
        stop.cancellation.check_forced()?;
        let directory = tempfile::Builder::new()
            .prefix(".av1an-attempt-")
            .tempdir_in(task.output.parent().unwrap_or_else(|| std::path::Path::new(".")))?;
        let path = directory
            .path()
            .join(format!("scene.{}", task.encoder.base().output_extension()));
        let attempt_progress = AttemptProgress::new(
            progress,
            task.original_index,
            task.frame_indices.len(),
            task.encoder.passes(),
        );
        let encoded = encode(&path, &attempt_progress).and_then(|result| {
            stop.cancellation.check_forced()?;
            let bytes = super::artifact::validate(
                &path,
                task.frame_indices.len(),
                options.validate_frame_count,
                &stop.cancellation,
            )
            .map_err(|error| error.context(RetryableAttempt))?;
            Ok((result, bytes))
        });
        match encoded {
            Ok((result, bytes)) => {
                return Ok(EncodedAttempt {
                    _directory: directory,
                    path,
                    result,
                    bytes,
                    progress: attempt_progress,
                });
            },
            Err(error) => {
                // Cleanup follows joins, and precedes even the retry-admission check.
                drop(directory);
                drop(attempt_progress);
                #[cfg(all(test, target_os = "linux"))]
                super::tests::retry_progress::internal_integration::after_failure(
                    &stop.cancellation,
                );
                let again = retryable(&error)
                    && attempt < options.max_tries
                    && !stop.stopped(&stop.cancellation);
                if again {
                    tracing::warn!(
                        "Scene {} attempt {attempt}/{} failed; retrying: {error:#}",
                        task.original_index,
                        options.max_tries
                    );
                }
                last = Some(error.context(format!(
                    "Scene {} attempt {attempt}/{}",
                    task.original_index, options.max_tries
                )));
                if !again {
                    return Err(terminal_error(last.take().expect("last failed attempt")));
                }
            },
        }
    }
    unreachable!("dispatch validates a positive total attempt budget")
}

/// Sends a scene's result to the persistence channel and returns it.
fn report_result(
    results_tx: &crossbeam_channel::Sender<ParallelEncoderResult>,
    finished_scenes: &Semaphore,
    result: ParallelEncoderResult,
) -> Result<ParallelEncoderResult> {
    results_tx.send(result.clone())?;
    finished_scenes.release();
    Ok(result)
}

impl ParallelEncoder {
    #[inline]
    pub fn encode_tasks(
        input: &mut Input,
        workers: u8,
        tasks: VecDeque<Task>,
        progress_tx: std::sync::mpsc::Sender<SequenceStatus>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Vec<Option<ParallelEncoderResult>>> {
        Self::encode_tasks_cancel(input, workers, tasks, progress_tx, cancelled.into())
    }

    /// Force-capable dispatcher, defaulting to three total attempts per scene.
    /// All active worker owners join before return.
    #[inline]
    pub fn encode_tasks_cancel(
        input: &mut Input,
        workers: u8,
        tasks: VecDeque<Task>,
        progress_tx: std::sync::mpsc::Sender<SequenceStatus>,
        cancelled: Cancellation,
    ) -> Result<Vec<Option<ParallelEncoderResult>>> {
        Self::encode_tasks_with_validation(input, workers, tasks, progress_tx, cancelled, true)
    }

    /// Explicit count-only policy for intentional frame-changing filters.
    /// Successful encoder status and nonempty regular output remain mandatory.
    /// Retains the default of three total attempts per scene.
    #[inline]
    pub fn encode_tasks_with_validation(
        input: &mut Input,
        workers: u8,
        tasks: VecDeque<Task>,
        progress_tx: std::sync::mpsc::Sender<SequenceStatus>,
        cancelled: Cancellation,
        validate_frame_count: bool,
    ) -> Result<Vec<Option<ParallelEncoderResult>>> {
        Self::encode_tasks_with_options(
            input,
            workers,
            tasks,
            progress_tx,
            cancelled,
            ParallelEncoderOptions {
                validate_frame_count,
                ..Default::default()
            },
        )
    }

    /// Explicit attempt/count policy. Non-replayable scenes with more than one
    /// attempt capture one anonymous Y4M disk spool before the first encode.
    #[allow(clippy::too_many_arguments)]
    #[inline]
    pub fn encode_tasks_with_options(
        input: &mut Input,
        workers: u8,
        tasks: VecDeque<Task>,
        progress_tx: std::sync::mpsc::Sender<SequenceStatus>,
        cancelled: Cancellation,
        options: ParallelEncoderOptions,
    ) -> Result<Vec<Option<ParallelEncoderResult>>> {
        if workers == 0 {
            anyhow::bail!(ParallelEncoderError::NoWorkers);
        }
        let (results_tx, results_rx) = crossbeam_channel::unbounded();
        let mut on_results = |_results: Vec<ParallelEncoderResult>| -> Result<()> { Ok(()) };
        let mut stream = ResultStream {
            results_tx,
            results_rx: &results_rx,
            finished_scenes: Arc::new(Semaphore::new(0)),
            on_results: &mut on_results,
        };
        let results = Self::encode_tasks_with_sender(
            input,
            workers,
            tasks,
            progress_tx,
            cancelled,
            &mut stream,
            options,
        )?;
        Ok(results)
    }

    #[inline]
    #[allow(
        clippy::needless_pass_by_value,
        reason = "Own the shared token through all worker joins"
    )]
    #[allow(clippy::too_many_arguments)]
    pub(super) fn encode_tasks_with_sender(
        input: &mut Input,
        workers: u8,
        tasks: VecDeque<Task>,
        progress_tx: std::sync::mpsc::Sender<SequenceStatus>,
        cancelled: Cancellation,
        stream: &mut ResultStream<'_, impl FnMut(Vec<ParallelEncoderResult>) -> Result<()>>,
        options: ParallelEncoderOptions,
    ) -> Result<Vec<Option<ParallelEncoderResult>>> {
        Self::encode_tasks_with_baseline(
            input,
            workers,
            tasks,
            progress_tx,
            cancelled,
            stream,
            options,
            0,
        )
    }

    #[allow(clippy::too_many_arguments)]
    #[allow(
        clippy::needless_pass_by_value,
        reason = "Own the shared token through all worker joins"
    )]
    pub(super) fn encode_tasks_with_baseline(
        input: &mut Input,
        workers: u8,
        tasks: VecDeque<Task>,
        progress_tx: std::sync::mpsc::Sender<SequenceStatus>,
        cancelled: Cancellation,
        stream: &mut ResultStream<'_, impl FnMut(Vec<ParallelEncoderResult>) -> Result<()>>,
        options: ParallelEncoderOptions,
        initial_frames: usize,
    ) -> Result<Vec<Option<ParallelEncoderResult>>> {
        if workers == 0 {
            anyhow::bail!(ParallelEncoderError::NoWorkers);
        }
        anyhow::ensure!(
            options.max_tries >= 1,
            "max_tries must be at least 1 total attempt"
        );
        cancelled.check_forced()?;
        if cancelled.is_cancelled() {
            return Ok(vec![None; tasks.len()]);
        }
        if tasks.is_empty() {
            cancelled.publish(|| {
                let _ = progress_tx.send(SequenceStatus::Whole(Status::Completed {
                    id: super::DETAILS.name.to_owned(),
                }));
                Ok(())
            })?;
            return Ok(Vec::new());
        }
        let clip_info = input.clip_info()?;
        let framerate = *clip_info.frame_rate.numer() as f64 / *clip_info.frame_rate.denom() as f64;
        for task in &tasks {
            super::checked_bitrate(0, task.frame_indices.len(), framerate).map_err(|error| {
                error.context(format!("Scene {} bitrate preflight", task.original_index))
            })?;
            task.encoder.validate().map_err(|error| {
                error.context(format!("Scene {} encoder preflight", task.original_index))
            })?;
        }
        if !tasks.is_empty() {
            super::artifact::preflight(options.validate_frame_count)?;
            if tasks.iter().any(|task| task.encoder.ffmpeg_filter().is_some())
                && which::which("ffmpeg").is_err()
            {
                anyhow::bail!(ParallelEncoderError::FfmpegNotFound);
            }
        }
        let total_frames =
            tasks.iter().map(|task| task.frame_indices.len()).sum::<usize>() + initial_frames;
        let progress = Progress::new(progress_tx.clone(), total_frames, initial_frames);
        // Nonstream VS decoding stays on this thread; both paths bound workers.
        if input.streams_concurrently() {
            let result = Self::encode_tasks_streaming(
                input,
                workers,
                STREAM_WINDOW,
                tasks,
                &cancelled,
                stream,
                options,
                &progress,
            );
            #[cfg(all(test, target_os = "linux"))]
            crate::core::encoder::process_lifecycle::before_stream_return(&cancelled);
            let result = crate::core::encoder::cancellation_result(result, &cancelled);
            if result.as_ref().is_ok_and(|results| results.iter().all(Option::is_some)) {
                cancelled.publish(|| {
                    let _ = progress_tx.send(SequenceStatus::Whole(Status::Completed {
                        id: super::DETAILS.name.to_owned(),
                    }));
                    Ok(())
                })?;
            }
            return result;
        }
        if workers == 0 {
            anyhow::bail!(ParallelEncoderError::NoWorkers);
        }
        let stop = Stop::new_cancel(workers.into(), cancelled.clone());
        let total_tasks = tasks.len();
        let results = stop.catch_panic("Parallel scheduling", || thread::scope(|scope| {
            // Also covers a thread-spawn panic before the scheduling loop:
            // close admission before std::thread::scope implicitly joins.
            let mut scheduling = stop.guard();
            // Rendezvous hands an admitted scene directly to an idle worker.
            // Only admitted scenes own frame channels; queued tasks remain metadata.
            let (jobs_tx, jobs_rx) = crossbeam_channel::bounded::<(
                &Task, Receiver<Cursor<Vec<u8>>>, crate::utils::semaphore::Permit, Arc<AtomicBool>
            )>(0);
            let mut handles = Vec::new();
            let setup = stop.catch("Parallel worker setup", || {
            for worker in 0..usize::from(workers).min(total_tasks) {
                let (stop, progress, clip_info, cancelled) =
                    (&stop, &progress, &clip_info, &cancelled);
                let jobs_rx = jobs_rx.clone();
                let results_tx = stream.results_tx.clone();
                let finished = Arc::clone(&stream.finished_scenes);
                handles.push(scope.spawn(move || stop.catch_panic(&format!("Parallel worker {worker}"), || {
                    #[cfg(all(test, target_os = "linux"))]
                    let _worker_count = super::tests::dispatch::internal_integration::worker_started();
                    let mut done = Vec::new();
                    loop {
                        let (task, frames_rx, _permit, decoded) = crossbeam_channel::select! {
                            recv(stop.receiver) -> _ => break,
                            recv(jobs_rx) -> job => match job { Ok(job) => job, Err(_) => break },
                        };
                        #[cfg(all(test, target_os = "linux"))]
                        super::tests::dispatch::internal_integration::worker_received();
                        if stop.stopped(cancelled) { break; }
                        #[cfg(all(test, target_os = "linux"))]
                        super::tests::dispatch::internal_integration::worker_admitted();
                        let result = stop.catch(&format!("Scene {} worker", task.original_index), || {
                            let mut guard = stop.guard();
                            #[cfg(all(test, target_os = "linux"))]
                            super::tests::shutdown::internal_integration::fault("gate", task.original_index)?;
                            debug!("Encoding Scene {}", task.original_index);
                            let started = std::time::SystemTime::now();
                             let mut direct = Some(frames_rx);
                             let mut spool = if options.max_tries > 1 {
                                 let mut file = crate::core::encoder::spool::create(task.output.parent().unwrap_or_else(|| std::path::Path::new(".")), &task.output)?;
                                 crate::core::encoder::spool::capture(&mut file, direct.take().expect("initial scene feed"), &task.output, cancelled)?;
                                 if !decoded.load(Ordering::Acquire) {
                                     guard.success();
                                     return Ok(None);
                                 }
                                 Some(file)
                             } else { None };
                              let mut attempt = encode_attempts(task, options, stop, progress, |path, attempt_progress| {
                                  let consume = |frames| consume_frames(task, frames, path, clip_info, attempt_progress,
                                      stop, STREAM_WINDOW);
                                 spool.as_mut().map_or_else(
                                     || consume(direct.take().expect("one direct attempt")),
                                     |file| crate::core::encoder::spool::replay_cancel(file, &task.output, cancelled, consume),
                                 )
                             })?;
                            // EOF alone is not success: a failed producer can leave
                            // a codec happily encoding a truncated scene. This is
                            // scene-local so successful active peers still persist.
                            if !decoded.load(Ordering::Acquire) {
                                guard.success();
                                return Ok(None);
                            }
                              cancelled.check_forced()?;
                              let bytes = attempt.bytes;
                             let ended = std::time::SystemTime::now();
                             let bitrate = super::checked_bitrate(bytes, task.frame_indices.len(), framerate)
                                 .map_err(|error| error.context(format!("Scene {} bitrate", task.original_index)))?;
                             let result = cancelled.publish(|| {
                              finalize_scene(SceneOutcome { temp_output: attempt.path.clone(), bytes, usable: true },
                                  &task.output, &mut attempt.progress, &stop.stopped)?;
                             let result = report_result(&results_tx, &finished, ParallelEncoderResult {
                                  scene: task.original_index, started, ended, bytes, bitrate, result: attempt.result.clone(),
                             })?;
                             Ok(result)
                             })?;
                            guard.success();
                            Ok(Some(result))
                        });
                        #[cfg(all(test, target_os = "linux"))]
                        if result.is_err() { super::tests::shutdown::internal_integration::collected_error(); }
                        if let Some(result) = result? { done.push((task.index, result)); }
                    }
                    Ok(done)
                })));
            }
            #[cfg(all(test, target_os = "linux"))]
            if tasks.len() > 1 { super::tests::shutdown::internal_integration::fault("schedule", 7)?; }
            Ok(())
            });
            // Callback errors stop admission immediately, but remain secondary
            // to a worker's original typed failure collected at join below.
            let mut persistence_error = None;
            let mut persist = || {
                match stop.catch("Scene persistence", || drain_finished_results(stream)) {
                    Ok(()) => true,
                    Err(err) => {
                        retain_error(&mut persistence_error, err);
                        false
                    },
                }
            };
            let scheduled = stop.catch("Scene decoding/persistence", || {
                for task in &tasks {
                    #[cfg(all(test, target_os = "linux"))]
                    super::tests::dispatch::internal_integration::before_admission(&stop, task);
                    // Reserve a real worker before creating/decoding its input.
                    // The worker owns this permit through completion, never reacquires it.
                    let Some(permit) = stop.workers.acquire_owned_cancel(&cancelled) else {
                        break;
                    };
                    if stop.stopped(&cancelled) {
                        break;
                    }
                    let mut guard = stop.guard();
                    if !persist() { guard.success(); break; }
                    let (frames_tx, frames_rx) = crossbeam_channel::unbounded();
                    let decoded = Arc::new(AtomicBool::new(false));
                    crossbeam_channel::select! {
                        recv(stop.receiver) -> _ => { guard.success(); break; },
                        send(jobs_tx, (task, frames_rx, permit, Arc::clone(&decoded))) -> sent => {
                            if sent.is_err() {
                                if stop.stopped(&cancelled) { guard.success(); break; }
                                anyhow::bail!("Scene {} worker handoff disconnected", task.original_index);
                            }
                        },
                    }
                    #[cfg(all(test, target_os = "linux"))]
                    super::tests::dispatch::internal_integration::after_handoff(&cancelled.legacy_flag());
                    // The worker owns the stop check after handoff. If it has
                    // started, graceful cancellation must still feed its input.
                    #[cfg(all(test, target_os = "linux"))]
                    super::tests::shutdown::internal_integration::fault("header", task.original_index)?;
                    #[cfg(all(test, target_os = "linux"))]
                    super::tests::dispatch::internal_integration::header(task);
                    #[cfg(all(test, target_os = "linux"))]
                    super::tests::dispatch::internal_integration::before_header();
                    let header = input.y4m_header(Some(task.frame_indices.len()))?;
                    if let Err(err) = frames_tx.send(Cursor::new(header.into_bytes())) {
                        if stop.stopped(&cancelled) {
                            guard.success();
                            break;
                        }
                        return Err(err.into());
                    }
                    #[cfg(all(test, target_os = "linux"))]
                    super::tests::shutdown::internal_integration::fault("decode", task.original_index)?;
                    #[cfg(all(test, target_os = "linux"))]
                    super::tests::dispatch::internal_integration::before_frames(&stop);
                    let produced = input.y4m_frames_cancel(frames_tx.clone(), &task.frame_indices, &cancelled);
                    #[cfg(all(test, target_os = "linux"))]
                    super::tests::dispatch::internal_integration::producer_returned(&produced);
                    if let Err(err) = produced {
                        if stop.stopped(&cancelled)
                            && err.is::<crossbeam_channel::SendError<Cursor<Vec<u8>>>>()
                        {
                            tracing::error!(
                                "Frame production stopped after worker failure: {err:#}"
                            );
                            guard.success();
                            break;
                        }
                        return Err(
                            err.context(format!("Scene {} frame production", task.original_index))
                        );
                    }
                    // Publish full production before EOF can reach the consumer.
                    decoded.store(true, Ordering::Release);
                    drop(frames_tx);
                    if !persist() { guard.success(); break; }
                    guard.success();
                }
                Ok(())
            });
            #[cfg(all(test, target_os = "linux"))]
            super::tests::dispatch::internal_integration::scheduled_finished();
            // EOF and admission closure precede every join, including decode or
            // callback failure. No pool-owned sender can keep a consumer alive.
            drop(jobs_tx);
            #[cfg(all(test, target_os = "linux"))]
            super::tests::dispatch::internal_integration::before_joins();
            // The final batch can finish after decoding ends. Observe callback
            // failures before blocking on any still-active peer (as streaming does).
            while !stop.stopped(&cancelled)
                && handles.iter().any(|handle| !handle.is_finished())
            {
                if !persist() { break; }
                thread::park_timeout(std::time::Duration::from_millis(10));
            }
            if stop.stopped(&cancelled) {
                stop.stop();
            }
            let mut error = setup.err();
            // A consumer can close its receiver before reporting its diagnostic.
            // Defer typed consumer disconnect/cancellation until worker diagnostics
            // arrive. Real header/decode errors still stay primary (E28).
            let mut frame_send_error = None;
            if let Err(err) = scheduled {
                if err.is::<crossbeam_channel::SendError<Cursor<Vec<u8>>>>()
                    || err.is::<crate::utils::cancellation::Cancelled>()
                {
                    frame_send_error = Some(err);
                } else {
                    retain_error(&mut error, err);
                }
            }
            let mut results = vec![None; total_tasks];
            for handle in handles {
                match handle.join() {
                    Ok(Ok(done)) => {
                        for (index, result) in done { results[index] = Some(result); }
                    },
                    Ok(Err(err)) => {
                        stop.stop();
                        retain_error(&mut error, err);
                    },
                    Err(_) => {
                        stop.stop();
                        retain_error(&mut error, anyhow::anyhow!("encoder worker panicked"));
                    },
                }
            }
            if let Some(err) = persistence_error { retain_error(&mut error, err); }
            // Successful active jobs may report after a peer stops scheduling.
            // Persist those results without replacing the original failure.
            if let Err(err) = stop.catch("Scene persistence", || drain_finished_results(stream)) {
                stop.stop();
                retain_error(&mut error, err);
            }
            if let Some(err) = frame_send_error { retain_error(&mut error, err); }
            #[cfg(all(test, target_os = "linux"))]
            super::tests::shutdown::internal_integration::verify_pools(&stop, workers.into());
            scheduling.success();
            error.map_or_else(|| Ok(results), Err)
        }));
        let results = crate::core::encoder::cancellation_result(results, &cancelled);
        if results.as_ref().is_ok_and(|results| results.iter().all(Option::is_some)) {
            cancelled.publish(|| {
                let _ = progress_tx.send(SequenceStatus::Whole(Status::Completed {
                    id: super::DETAILS.name.to_owned(),
                }));
                Ok(())
            })?;
        }
        drop(progress_tx);
        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod internal_integration {
        use super::*;

        /// A usable scene reports its size on the progress channel — the event the
        /// interface turns into the finished-scene counter and the size estimate —
        /// and is renamed to its final output.
        #[test]
        fn finalizing_a_usable_scene_publishes_output_and_reports_its_size() {
            let directory = tempfile::tempdir().expect("scene finalization fixture");
            let dir = directory.path();
            let temp_output = dir.join("0.temp.ivf");
            let output = dir.join("0.ivf");
            std::fs::write(&temp_output, vec![7_u8; 42]).expect("temp file should be written");
            let (tx, rx) = std::sync::mpsc::channel();
            let errored = AtomicBool::new(false);
            let progress = Progress::new(tx, 3, 0);
            let mut attempt = AttemptProgress::new(
                &progress,
                3,
                3,
                crate::models::encoder::EncoderPasses::All(1),
            );

            finalize_scene(
                SceneOutcome {
                    temp_output: temp_output.clone(),
                    bytes: 42,
                    usable: true,
                },
                &output,
                &mut attempt,
                &errored,
            )
            .expect("finalize should succeed");

            match rx.try_iter().find(|event| matches!(event, SequenceStatus::Whole(Status::Processing { completion: crate::core::sequence::SequenceCompletion::Custom { name, .. }, .. }) if name == "size")).expect("the size event is sent") {
            SequenceStatus::Whole(Status::Processing {
                id,
                completion:
                    crate::core::sequence::SequenceCompletion::Custom {
                        name,
                        completed,
                        total,
                    },
            }) => {
                assert_eq!(id, "3");
                assert_eq!(name, "size");
                assert_eq!(completed, 42.0);
                assert_eq!(total, 42.0);
            },
            other => panic!("unexpected status: {other:?}"),
        }
            assert!(output.exists(), "the encode is renamed to its final output");
            assert!(!temp_output.exists(), "the temp file is gone");
            assert!(!errored.load(Ordering::Relaxed));
        }

        /// An unusable encode is discarded without reporting a size — an estimate
        /// built on it would claim bytes that do not exist — and flags the error.
        #[test]
        fn an_unusable_scene_is_discarded_without_a_size() {
            let directory = tempfile::tempdir().expect("scene finalization fixture");
            let dir = directory.path();
            let temp_output = dir.join("1.temp.ivf");
            let output = dir.join("1.ivf");
            std::fs::write(&temp_output, vec![7_u8; 42]).expect("temp file should be written");
            let (tx, rx) = std::sync::mpsc::channel();
            let errored = AtomicBool::new(false);
            let progress = Progress::new(tx, 1, 0);
            let mut attempt = AttemptProgress::new(
                &progress,
                1,
                1,
                crate::models::encoder::EncoderPasses::All(1),
            );
            let _ = rx.try_iter().count();

            finalize_scene(
                SceneOutcome {
                    temp_output: temp_output.clone(),
                    bytes: 42,
                    usable: false,
                },
                &output,
                &mut attempt,
                &errored,
            )
            .expect("finalize should succeed");

            assert!(rx.try_recv().is_err(), "a discarded scene reports no size");
            assert!(!output.exists(), "a discarded scene produces no output");
            assert!(!temp_output.exists(), "the temp output is removed");
            assert!(errored.load(Ordering::Relaxed), "the error is flagged");
        }
    }
}
