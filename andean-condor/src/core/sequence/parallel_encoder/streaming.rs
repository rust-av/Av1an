//! Bounded frame streaming; active finite jobs finish on admission shutdown.
use std::{collections::VecDeque, io::Cursor, thread};

use crate::utils::cancellation::{Cancellation, Cancelled};
use anyhow::Result;

use super::{
    ParallelEncoder, ParallelEncoderOptions,
    encode::{SceneOutcome, consume_frames, encode_attempts, finalize_scene, report_result},
    error::ParallelEncoderError,
    lifecycle::{Stop, retain_error},
    progress::Progress,
    task::{ParallelEncoderResult, PublishedResult, ResultStream, Task},
};
use crate::core::{encoder::EncoderResult, input::Input};

/// Merge only after the consumer (including filter/relays) and producer have joined.
pub(super) fn joined_scene_result(
    encoded: Result<EncoderResult>,
    produced: Result<()>,
) -> Result<EncoderResult> {
    match (encoded, produced) {
        (Ok(result), Ok(())) => Ok(result),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(encoded), Err(produced)) => {
            // A closed receiver is fallout, not an independent source failure.
            if produced.is::<crossbeam_channel::SendError<Cursor<Vec<u8>>>>()
                || produced.is::<Cancelled>()
            {
                return Err(encoded);
            }
            let (primary, secondary) =
                if encoded.downcast_ref::<Cancelled>() == Some(&Cancelled::Forced) {
                    (encoded, produced)
                } else {
                    (produced, encoded)
                };
            // anyhow renders the primary chain itself; only repeat its outer label here.
            let context = format!("{primary}\nSecondary scene failure: {secondary:#}");
            Err(primary.context(context))
        },
    }
}

impl ParallelEncoder {
    #[allow(clippy::too_many_lines, clippy::too_many_arguments)]
    pub(super) fn encode_tasks_streaming(
        input: &mut Input,
        workers: u8,
        window: usize,
        tasks: VecDeque<Task>,
        cancelled: &Cancellation,
        stream: &mut ResultStream<'_, impl FnMut(Vec<PublishedResult>) -> Result<()>>,
        options: ParallelEncoderOptions,
        progress: &Progress,
    ) -> Result<Vec<Option<ParallelEncoderResult>>> {
        if workers == 0 {
            anyhow::bail!(ParallelEncoderError::NoWorkers);
        }
        let total_tasks = tasks.len();
        let workers = usize::from(workers).min(total_tasks.max(1));
        let window = window.max(1);
        let clip_info = input.clip_info()?;
        let framerate = *clip_info.frame_rate.numer() as f64 / *clip_info.frame_rate.denom() as f64;
        for task in &tasks {
            super::checked_bitrate(0, task.frame_indices.len(), framerate).map_err(|error| {
                error.context(format!("Scene {} bitrate preflight", task.original_index))
            })?;
        }
        let headers = tasks
            .iter()
            .map(|task| {
                Ok((
                    task.index,
                    input.y4m_header(Some(task.frame_indices.len()))?,
                ))
            })
            .collect::<Result<std::collections::HashMap<_, _>>>()?;
        let sources = input.frame_sources(workers)?;
        let (task_tx, task_rx) = crossbeam_channel::unbounded();
        for task in tasks {
            task_tx.send(task)?;
        }
        drop(task_tx);
        let stop = Stop::new_cancel(workers, cancelled.clone());
        stop.catch_panic("Streaming scheduling", || {
            thread::scope(|scope| {
                let mut scheduling = stop.guard();
                let mut handles = Vec::new();
                let setup = stop.catch("Streaming worker setup", || {
                    for (worker, source) in sources.into_iter().enumerate() {
                        let task_rx = task_rx.clone();
                        let results_tx = stream.results_tx.clone();
                        let finished = std::sync::Arc::clone(&stream.finished_scenes);
                        let (stop, headers, clip_info) = (&stop, &headers, &clip_info);
                        handles.push(scope.spawn(move || {
                            let mut scene = None;
                            let result = stop.catch(&format!("Streaming worker {worker}"), || {
                                let mut guard = stop.guard();
                                #[cfg(all(test, target_os = "linux"))]
                                super::tests::shutdown::internal_integration::fault("open", 7)?;
                                let mut reader = source.open()?;
                                let mut done = Vec::new();
                                while !stop.stopped(cancelled) {
                                    let Ok(task) = task_rx.recv() else { break };
                                    if stop.stopped(cancelled) {
                                        break;
                                    }
                                    scene = Some(task.original_index);
                                    let task_result = stop.catch(
                                        &format!("Scene {} worker", task.original_index),
                                        || {
                                            let started = std::time::SystemTime::now();
                                            let mut attempt = encode_attempts(
                                                &task,
                                                options,
                                                stop,
                                                progress,
                                                |temp, attempt_progress| {
                                                    let (tx, rx) = crossbeam_channel::bounded::<
                                                        Cursor<Vec<u8>>,
                                                    >(
                                                        window
                                                    );
                                                    tx.send(Cursor::new(
                                                        headers[&task.index].clone().into_bytes(),
                                                    ))?;
                                                    let (encoded, produced) =
                                                        thread::scope(|inner| {
                                                            let task = &task;
                                                            let encoder = inner.spawn(move || {
                                                                stop.attempt(
                                                                    &format!(
                                                                        "Scene {} consumer",
                                                                        task.original_index
                                                                    ),
                                                                    || {
                                                                        consume_frames(
                                                                            task,
                                                                            rx,
                                                                            temp,
                                                                            clip_info,
                                                                            attempt_progress,
                                                                            stop,
                                                                            window,
                                                                        )
                                                                    },
                                                                )
                                                            });
                                                            let produced = stop.attempt(
                                                                &format!(
                                                                    "Scene {} frame production",
                                                                    task.original_index
                                                                ),
                                                                || {
                                                                    #[cfg(all(
                                                                        test,
                                                                        target_os = "linux"
                                                                    ))]
                                                                    super::tests::shutdown::internal_integration::fault(
                                                                        "produce",
                                                                        task.original_index,
                                                                    )?;
                                                                    reader.y4m_frames_cancel(
                                                                        &tx,
                                                                        &task.frame_indices,
                                                                        window,
                                                                        cancelled,
                                                                    )
                                                                },
                                                            );
                                                            drop(tx); // EOF before joining the consumer and its filter/relays.
                                                            let encoded = encoder
                                                                .join()
                                                                .unwrap_or_else(|_| {
                                                                    Err(anyhow::anyhow!(
                                                                        "Scene {} encoder panicked",
                                                                        task.original_index
                                                                    ))
                                                                });
                                                            (encoded, produced)
                                                        });
                                                    joined_scene_result(encoded, produced)
                                                },
                                            )?;
                                            let bytes = attempt.bytes;
                                            let bitrate = super::checked_bitrate(
                                                bytes,
                                                task.frame_indices.len(),
                                                framerate,
                                            )
                                            .map_err(|error| {
                                                error.context(format!(
                                                    "Scene {} bitrate",
                                                    task.original_index
                                                ))
                                            })?;
                                            let ended = std::time::SystemTime::now();
                                            let result = cancelled.publish(|| {
                                                 let identity = finalize_scene(
                                                    SceneOutcome {
                                                        temp_output: attempt.path.clone(),
                                                        bytes,
                                                         usable: true,
                                                         identity: attempt.identity,
                                                    },
                                                    &task.output,
                                                    &mut attempt.progress,
                                                    &stop.stopped,
                                                )?;
                                                let result = ParallelEncoderResult {
                                                    scene: task.original_index,
                                                    started,
                                                    ended,
                                                    bytes,
                                                    bitrate,
                                                    result: attempt.result.clone(),
                                                };
                                                 report_result(&results_tx, &finished, result, identity)
                                            })?;
                                            Ok(result)
                                        },
                                    )?;
                                    done.push((task.index, task_result));
                                }
                                guard.success();
                                Ok(done)
                            });
                            result.map_err(|err| {
                                err.context(scene.map_or_else(
                                    || format!("Streaming worker {worker}"),
                                    |scene| format!("Scene {scene} on streaming worker {worker}"),
                                ))
                            })
                        }));
                    }
                    Ok(())
                });
                // The public stream retains a sender: completion is determined by
                // handles, not channel disconnection. Callback failure stops NOW,
                // before any blocking join, rather than letting workers drain tasks.
                let persisted = stop.catch("Scene persistence", || {
                    loop {
                        super::progress::drain_finished_results(stream)?;
                        if handles.iter().all(thread::ScopedJoinHandle::is_finished) {
                            break;
                        }
                        thread::park_timeout(std::time::Duration::from_millis(10));
                    }
                    super::progress::drain_finished_results(stream)
                });
                let mut error = setup.err();
                if let Err(err) = persisted {
                    retain_error(&mut error, err);
                }
                if stop.stopped(cancelled) {
                    stop.stop();
                }
                let mut ordered = vec![None; total_tasks];
                for handle in handles {
                    match handle.join() {
                        Ok(Ok(results)) => {
                            for (index, result) in results {
                                ordered[index] = Some(result);
                            }
                        },
                        Ok(Err(err)) => {
                            stop.stop();
                            retain_error(&mut error, err);
                        },
                        Err(_) => {
                            stop.stop();
                            retain_error(&mut error, anyhow::anyhow!("Streaming worker panicked"));
                        },
                    }
                }
                #[cfg(all(test, target_os = "linux"))]
                super::tests::shutdown::internal_integration::verify_pools(&stop, workers);
                scheduling.success();
                error.map_or_else(|| Ok(ordered), Err)
            })
        })
    }
}
