//! Streaming parallel encoding with bounded memory.
//!
//! Each of `workers` threads reads from its own `FrameSource` and streams
//! frames to its encoder through a bounded channel, so resident raw frames stay
//! bounded by roughly 2 * window * workers regardless of scene length.

use std::{
    collections::VecDeque,
    fs,
    io::Cursor,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    thread,
};

use anyhow::{Result, bail};
use tracing::{debug, error};

use super::{
    ParallelEncoder,
    encode::{FilteredFrames, SceneOutcome, finalize_scene, spawn_filter},
    error::ParallelEncoderError,
    progress::relay_progress,
    task::{ParallelEncoderResult, ResultStream, Task},
};
use crate::core::{encoder::EncodeProgress, input::Input, sequence::SequenceStatus};

impl ParallelEncoder {
    #[allow(clippy::too_many_lines)]
    #[allow(clippy::too_many_arguments)]
    pub(super) fn encode_tasks_streaming(
        input: &mut Input,
        workers: u8,
        window: usize,
        tasks: VecDeque<Task>,
        progress_tx: &std::sync::mpsc::Sender<SequenceStatus>,
        cancelled: &AtomicBool,
        stream: &mut ResultStream<'_, impl FnMut(Vec<ParallelEncoderResult>) -> Result<()>>,
    ) -> Result<Vec<Option<ParallelEncoderResult>>> {
        let total_tasks = tasks.len();
        let workers = usize::from(workers.max(1)).min(total_tasks.max(1));
        let clip_info = input.clip_info()?;
        let framerate = *clip_info.frame_rate.numer() as f64 / *clip_info.frame_rate.denom() as f64;
        let total_process_frames: usize = tasks.iter().map(|t| t.frame_indices.len()).sum();
        let headers = tasks
            .iter()
            .map(|t| Ok((t.index, input.y4m_header(Some(t.frame_indices.len()))?)))
            .collect::<Result<std::collections::HashMap<_, _>>>()?;
        let sources = input.frame_sources(workers)?;

        let (task_tx, task_rx) = crossbeam_channel::unbounded::<Task>();
        for task in tasks {
            task_tx.send(task)?;
        }
        drop(task_tx);
        let errored = AtomicBool::new(false);
        let total_final = AtomicUsize::new(0);
        let results_tx = stream.results_tx.clone();

        let worker_outputs =
            thread::scope(|s| -> Result<Vec<Vec<(usize, ParallelEncoderResult)>>> {
                let mut handles = Vec::with_capacity(workers);
                for (worker_id, source) in sources.into_iter().enumerate() {
                    let task_rx = task_rx.clone();
                    let results_tx = results_tx.clone();
                    let progress_tx = progress_tx.clone();
                    let (headers, errored, cancelled, total_final, clip_info) =
                        (&headers, &errored, &cancelled, &total_final, &clip_info);
                    handles.push(s.spawn(
                        move || -> Result<Vec<(usize, ParallelEncoderResult)>> {
                            // Native decoders must be opened on the reading thread.
                            let mut reader = source.open()?;
                            let mut done = Vec::new();
                            while let Ok(task) = task_rx.recv() {
                                if cancelled.load(Ordering::Relaxed)
                                    || errored.load(Ordering::Relaxed)
                                {
                                    break;
                                }
                                debug!(
                                    "Worker {} encoding Scene {}",
                                    worker_id, task.original_index
                                );
                                let started = std::time::SystemTime::now();
                                let temp_output = task.output.with_extension(format!(
                                    "temp.{}",
                                    task.encoder.output_extension()
                                ));
                                let (ftx, frx) =
                                    crossbeam_channel::bounded::<Cursor<Vec<u8>>>(window);
                                ftx.send(Cursor::new(headers[&task.index].clone().into_bytes()))?;
                                // The encoder consumes FFmpeg's regenerated
                                // header, not the one just sent.
                                let (frx, filter_stage): FilteredFrames =
                                    spawn_filter(frx, &task, None, window)?;
                                let (encode_progress_tx, encode_progress_rx) =
                                    std::sync::mpsc::channel::<EncodeProgress>();
                                let (encode_result, produce_result) = thread::scope(|s2| {
                                    let relay_tx = progress_tx.clone();
                                    let total_passes = task.encoder.total_passes();
                                    let scene_frames = task.frame_indices.len();
                                    s2.spawn(move || {
                                        relay_progress(
                                            encode_progress_rx,
                                            &relay_tx,
                                            total_passes,
                                            task.original_index,
                                            scene_frames,
                                            total_process_frames,
                                            total_final,
                                        )
                                    });
                                    let (encoder, temp) = (&task.encoder, &temp_output);
                                    let encoder_thread = s2.spawn(move || {
                                        encoder.encode_with_stream(
                                            frx,
                                            temp,
                                            encode_progress_tx,
                                            clip_info,
                                        )
                                    });
                                    // Blocks when the encoder lags.
                                    let produced =
                                        reader.y4m_frames(&ftx, &task.frame_indices, window);
                                    drop(ftx); // EOF for the encoder
                                    (
                                        encoder_thread.join().expect("encoder thread panicked"),
                                        produced,
                                    )
                                });
                                // Prefer the filter's stderr over the encoder
                                // or producer fallout it causes.
                                if let Some(stage) = &filter_stage
                                    && let Err(err) = stage.check()
                                {
                                    let _ = fs::remove_file(&temp_output);
                                    errored.store(true, Ordering::Relaxed);
                                    bail!(
                                        "FFmpeg filter failed while encoding scene {}: {err}",
                                        task.original_index
                                    );
                                }
                                let result = encode_result?;
                                let ended = std::time::SystemTime::now();
                                let bytes = temp_output.metadata().ok().map_or(0, |m| m.len());
                                let seconds = task.frame_indices.len() as f64 / framerate;
                                if let Err(e) = produce_result {
                                    errored.store(true, Ordering::Relaxed);
                                    bail!(
                                        "Scene {} frame production failed: {e}",
                                        task.original_index
                                    );
                                }
                                finalize_scene(
                                    SceneOutcome {
                                        temp_output,
                                        bytes,
                                        usable: result.status.success(),
                                    },
                                    &task.output,
                                    task.original_index,
                                    &progress_tx,
                                    errored,
                                )?;
                                let parallel_result = ParallelEncoderResult {
                                    scene: task.original_index,
                                    started,
                                    ended,
                                    bytes,
                                    bitrate: (bytes * 8) as f64 / seconds,
                                    result,
                                };
                                let _ = results_tx.send(parallel_result.clone());
                                done.push((task.index, parallel_result));
                            }
                            Ok(done)
                        },
                    ));
                }
                drop(results_tx);

                // Persist results as they arrive.
                loop {
                    match stream.results_rx.recv_timeout(std::time::Duration::from_millis(200)) {
                        Ok(result) => (stream.on_results)(vec![result])?,
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                            if handles.iter().all(|h| h.is_finished()) {
                                while let Ok(result) = stream.results_rx.try_recv() {
                                    (stream.on_results)(vec![result])?;
                                }
                                break;
                            }
                        },
                        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                    }
                }
                handles
                    .into_iter()
                    .map(|h| h.join().map_err(|_| anyhow::anyhow!("encoder worker panicked"))?)
                    .collect()
            })?;

        let mut ordered: Vec<Option<ParallelEncoderResult>> = vec![None; total_tasks];
        for (index, result) in worker_outputs.into_iter().flatten() {
            if let Some(slot) = ordered.get_mut(index) {
                *slot = Some(result);
            }
        }
        if let Some(failed) = ordered.iter().flatten().find(|r| !r.result.status.success()) {
            let err = ParallelEncoderError::EncoderFailed {
                scene:  failed.scene,
                result: failed.result.clone(),
            };
            error!("{}", err);
            bail!(err);
        }
        Ok(ordered)
    }
}
