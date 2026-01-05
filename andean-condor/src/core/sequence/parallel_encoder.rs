use std::{
    collections::{BTreeMap, VecDeque},
    error::Error,
    fs,
    io::Cursor,
    path::{Path, PathBuf},
    sync::{
        self,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
        Condvar,
        Mutex,
    },
    thread::{self, available_parallelism},
};

use anyhow::{bail, Result};
use av1_grain::write_grain_table;
use thiserror::Error;
use tracing::{debug, error, trace};

use crate::{
    core::{
        encoder::{EncodeProgress, EncoderResult},
        input::Input,
        sequence::{Sequence, SequenceCompletion, SequenceDetails, SequenceStatus, Status},
        Condor,
    },
    models::{
        encoder::Encoder,
        scene::SubScene,
        sequence::{
            parallel_encode::ParallelEncodeDataHandler,
            scene_detect::SceneDetectDataHandler,
            SequenceConfigHandler,
            SequenceDataHandler,
        },
    },
};

static DETAILS: SequenceDetails = SequenceDetails {
    name:        "Parallel Encoder",
    description: "Encodes a set of scenes in parallel until all scenes are encoded.",
    version:     "0.0.1",
};

pub struct ParallelEncoder {
    pub workers:          u8,
    pub input:            Option<Input>,
    pub encoder:          Option<Encoder>,
    pub scenes_directory: PathBuf,
}

impl<DataHandler, ConfigHandler> Sequence<DataHandler, ConfigHandler> for ParallelEncoder
where
    DataHandler: SequenceDataHandler + SceneDetectDataHandler + ParallelEncodeDataHandler,
    ConfigHandler: SequenceConfigHandler,
{
    #[inline]
    fn details(&self) -> SequenceDetails {
        DETAILS
    }

    #[inline]
    fn validate(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
    ) -> Result<((), Vec<Box<dyn Error>>)> {
        let warnings: Vec<Box<dyn Error>> = vec![];

        if self.workers == 0 {
            bail!(ParallelEncoderError::NoWorkers);
        }

        if let Some(input) = &self.input {
            Input::validate(&input.as_data())?;
        }
        let encoder = self.encoder.as_ref().map_or(&condor.encoder, |e| e);

        encoder.validate()?;

        // Ensure all the scene encoders are validated
        for scene in &condor.scenes {
            scene.encoder.validate()?;
        }

        // TODO: Validate all scenes output the same codec

        Ok(((), warnings))
    }

    #[inline]
    fn initialize(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        progress_tx: sync::mpsc::Sender<SequenceStatus>,
    ) -> Result<((), Vec<Box<dyn Error>>)> {
        let mut warnings: Vec<Box<dyn Error>> = vec![];

        // Ensure scenes is not empty
        if condor.scenes.is_empty() {
            warnings.push(Box::new(ParallelEncoderError::ScenesEmpty));
        }
        // let encoder = self.encoder.as_ref().map_or(&condor.encoder, |e| e);
        if let Some(input) = &mut self.input {
            // Initialize input by getting clip_info. For VapourSynth inputs, this may begin
            // a lengthy caching process, hence the separation between validate and
            // initialize.
            progress_tx.send(SequenceStatus::Whole(Status::Processing {
                id:         DETAILS.name.to_owned(),
                completion: SequenceCompletion::Custom {
                    name:      DETAILS.name.to_owned(),
                    completed: 0.0,
                    total:     1.0,
                },
            }))?;
            input.clip_info()?;
            progress_tx.send(SequenceStatus::Whole(Status::Completed {
                id: DETAILS.name.to_owned(),
            }))?;
        }

        if !self.scenes_directory.exists() {
            std::fs::create_dir_all(&self.scenes_directory)?;
        }

        // Generate Photon Noise tables
        let input = self.input.as_mut().unwrap_or(&mut condor.input);
        // TODO: Get transfer functions more effectively
        // let transfer_function =
        // input.clip_info()?.transfer_function_params_adjusted(enc_params)
        let clip_info = input.clip_info()?;
        let transfer_function = clip_info.transfer_characteristics;
        for scene in &mut condor.scenes {
            let params = scene.encoder.generate_photon_noise_table(
                clip_info.resolution.0,
                clip_info.resolution.1,
                transfer_function,
            )?;

            if let Some((hashed_name, params)) = params {
                let output_directory = self.scenes_directory.join(DETAILS.name);
                let output = output_directory.join(format!("{}.tbl", hashed_name));
                let output_clone = output.clone();
                if !output.exists() {
                    if !output_directory.exists() {
                        std::fs::create_dir_all(&output_directory)?;
                    }
                    debug!("Writing a new photon noise table to {}", output.display());
                    write_grain_table(output, &[params])?;
                }
                scene.encoder.apply_photon_noise_parameters(&output_clone)?;
            }
        }

        Ok(((), warnings))
    }

    #[inline]
    fn execute(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        progress_tx: sync::mpsc::Sender<SequenceStatus>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<((), Vec<Box<dyn Error>>)> {
        // TODO:
        // handle subscenes

        let mut warnings: Vec<Box<dyn Error>> = vec![];
        let input = if let Some(input) = &mut self.input {
            input
        } else {
            &mut condor.input
        };
        if condor.scenes.is_empty() {
            warnings.push(Box::new(ParallelEncoderError::ScenesEmpty));
            return Ok(((), warnings));
        }

        let tasks = condor
            .scenes
            .iter()
            .enumerate()
            .filter(|(index, scene)| {
                let output = self.scenes_directory.join(format!(
                    "{}.{}",
                    Self::scene_id(*index),
                    scene.encoder.output_extension()
                ));
                !output.exists()
            })
            .enumerate()
            .map(|(index, (original_index, scene))| Task {
                original_index,
                index,
                start_frame: scene.start_frame,
                end_frame: scene.end_frame,
                sub_scenes: scene.sub_scenes.clone(),
                encoder: scene.encoder.clone(),
                output: self.scenes_directory.join(format!(
                    "{}.{}",
                    Self::scene_id(original_index),
                    scene.encoder.output_extension()
                )),
            })
            .collect::<VecDeque<Task>>();

        let encoder_thread = Self::encode_tasks(input, self.workers, tasks, progress_tx, cancelled);

        match encoder_thread {
            Ok(encoder_results) => {
                for encoder_result in encoder_results.into_iter().flatten() {
                    if let Some(scene) = condor.scenes.get_mut(encoder_result.scene)
                        && encoder_result.bytes != 0
                    {
                        let parallel_encode_data = scene.processing.get_parallel_encode_mut()?;
                        parallel_encode_data.bytes = Some(encoder_result.bytes);
                        parallel_encode_data.started_on = Some(
                            encoder_result
                                .started
                                .duration_since(std::time::UNIX_EPOCH)
                                .expect("Time is valid")
                                .as_millis(),
                        );
                        scene.processing.get_parallel_encode_mut()?.started_on = Some(
                            encoder_result
                                .ended
                                .duration_since(std::time::UNIX_EPOCH)
                                .expect("Time is valid")
                                .as_millis(),
                        );
                    }
                }
                condor.save()?;
            },
            Err(err) => bail!(err),
        }

        Ok(((), warnings))
    }
}

impl Default for ParallelEncoder {
    #[inline]
    fn default() -> Self {
        Self {
            workers:          1,
            input:            None,
            encoder:          None,
            scenes_directory: PathBuf::new(),
        }
    }
}

impl ParallelEncoder {
    pub const DETAILS: SequenceDetails = DETAILS;
    #[inline]
    pub fn new(workers: u8, scenes_directory: &Path) -> Self {
        ParallelEncoder {
            workers,
            input: None,
            encoder: None,
            scenes_directory: scenes_directory.to_path_buf(),
        }
    }

    #[inline]
    pub fn scene_id(index: usize) -> String {
        format!("{:>05}", index)
    }

    #[inline]
    pub fn encode_tasks(
        input: &mut Input,
        workers: u8,
        tasks: VecDeque<Task>,
        progress_tx: sync::mpsc::Sender<SequenceStatus>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Vec<Option<ParallelEncoderResult>>> {
        let (task_tx, task_rx) = crossbeam_channel::unbounded();
        let mut frames_senders = BTreeMap::new();
        let mut frames_receivers = BTreeMap::new();
        let mut encoder_semaphores = BTreeMap::new();
        let total_tasks = tasks.len();
        let total_process_frames =
            tasks.iter().fold(0, |acc, task| acc + (task.end_frame - task.start_frame));
        for task in tasks.iter() {
            let index = task.index;
            task_tx.send(task.clone())?;
            let (ftx, rtx) = crossbeam_channel::unbounded();
            frames_senders.insert(index, ftx);
            frames_receivers.insert(index, rtx);
            encoder_semaphores.insert(index, Arc::new(Semaphore::new(0)));
        }
        drop(task_tx);
        let encoder_errored = Arc::new(AtomicBool::new(false));

        thread::scope(|s| -> Result<_> {
            let total_final_pass_frames_encoded = Arc::new(AtomicUsize::new(0));
            let worker_semaphore = Arc::new(Semaphore::new(workers.into()));
            let decoder_semaphore = Arc::new(Semaphore::new((workers + 1).into()));
            let frame_receivers = Arc::new(Mutex::new(frames_receivers));
            // let progress_tx = progress_tx.clone();
            let mut encoder_threads = Vec::new();
            let cancelled = Arc::new(cancelled);

            for _ in 0..total_tasks {
                let total_final_pass_frames_encoded = Arc::clone(&total_final_pass_frames_encoded);
                let cancelled = Arc::clone(&cancelled);
                let task_rx = task_rx.clone();
                let frame_receivers = Arc::clone(&frame_receivers);
                let task_progress_tx = progress_tx.clone();
                let size_tx = progress_tx.clone();
                let task = task_rx.recv()?;
                let total_passes = task.encoder.total_passes();
                let total_scene_frames = task.end_frame - task.start_frame;
                let worker_semaphore = Arc::clone(&worker_semaphore);
                let decoder_semaphore_clone = Arc::clone(&decoder_semaphore);
                let encoder_semaphore =
                    encoder_semaphores.get(&task.index).expect("encoder_semaphore exists");
                let encoder_semaphore_clone: Arc<Semaphore> = Arc::clone(encoder_semaphore);
                let encoder_errored = Arc::clone(&encoder_errored);

                let encoder_thread = s.spawn(move || -> Result<Option<ParallelEncoderResult>> {
                    let mut fr_lock =
                        frame_receivers.lock().expect("frame_receivers mutex should acquire lock");
                    let frames_rx = fr_lock.remove(&task.index).expect("should have frames_rx)");
                    drop(fr_lock);
                    let (encode_progress_tx, encode_progress_rx) =
                        sync::mpsc::channel::<EncodeProgress>();

                    // Wait for encoder semaphore (unblocked when decoder starts)
                    // Prevents all encoders from starting at creation
                    encoder_semaphore_clone.acquire();
                    trace!(
                        "Scene {} Encoder waiting for a free Worker",
                        task.original_index
                    );
                    // Wait for worker semaphore (unblocked when worker finishes)
                    // Prevents active encoders exceeding worker limit
                    let worker_id = worker_semaphore.acquire();
                    if cancelled.load(Ordering::Relaxed) || encoder_errored.load(Ordering::Relaxed)
                    {
                        // Release decoder semaphore to allow decoder to exit
                        worker_semaphore.release();
                        decoder_semaphore_clone.release();
                        return Ok(None);
                    }
                    debug!(
                        "Encoding Scene {} with Worker {}",
                        task.original_index, worker_id
                    );
                    let started = std::time::SystemTime::now();
                    // Handle progress from Encoder
                    s.spawn(move || -> Result<()> {
                        for progress in encode_progress_rx {
                            if progress.pass.0 == total_passes {
                                let total_final_encoded =
                                    total_final_pass_frames_encoded.fetch_add(1, Ordering::Relaxed);
                                // Scene's final-pass frame completed
                                task_progress_tx.send(SequenceStatus::Subprocess {
                                    parent: Status::Processing {
                                        id:         DETAILS.name.to_string(),
                                        completion: SequenceCompletion::Frames {
                                            completed: total_final_encoded as u64,
                                            total:     total_process_frames as u64,
                                        },
                                    },
                                    child:  Status::Processing {
                                        id:         task.original_index.to_string(),
                                        completion: SequenceCompletion::Frames {
                                            completed: progress.frame as u64,
                                            total:     total_scene_frames as u64,
                                        },
                                    },
                                })?;
                                task_progress_tx.send(SequenceStatus::Whole(
                                    Status::Processing {
                                        id:         DETAILS.name.to_string(),
                                        completion: SequenceCompletion::Frames {
                                            completed: total_final_encoded as u64,
                                            total:     total_process_frames as u64,
                                        },
                                    },
                                ))?;
                                // Scene completed
                                if progress.frame == total_scene_frames {
                                    task_progress_tx.send(SequenceStatus::Subprocess {
                                        parent: Status::Processing {
                                            id:         DETAILS.name.to_string(),
                                            completion: SequenceCompletion::Frames {
                                                completed: total_final_encoded as u64,
                                                total:     total_process_frames as u64,
                                            },
                                        },
                                        child:  Status::Completed {
                                            id: task.original_index.to_string(),
                                        },
                                    })?;
                                }
                                // Process completed
                                if total_final_encoded == total_process_frames {
                                    task_progress_tx.send(SequenceStatus::Whole(
                                        Status::Completed {
                                            id: DETAILS.name.to_string(),
                                        },
                                    ))?;
                                }
                            }
                            // Scene's pass/frame completed
                            task_progress_tx.send(SequenceStatus::Subprocess {
                                parent: Status::Processing {
                                    id:         DETAILS.name.to_string(),
                                    completion: SequenceCompletion::Frames {
                                        completed: total_final_pass_frames_encoded
                                            .load(Ordering::Relaxed)
                                            as u64,
                                        total:     total_process_frames as u64,
                                    },
                                },
                                child:  Status::Processing {
                                    id:         task.original_index.to_string(),
                                    completion: SequenceCompletion::PassFrames {
                                        passes: progress.pass,
                                        frames: (progress.frame as u64, total_scene_frames as u64),
                                    },
                                },
                            })?;
                        }
                        Ok(())
                    });
                    // Encode to temporary file
                    let temp_output = task
                        .output
                        .with_extension(format!("temp.{}", task.encoder.output_extension()));
                    trace!(
                        "Encoding Scene {} to {}",
                        task.original_index,
                        temp_output.display()
                    );
                    let result = task.encoder.encode_with_stream(
                        frames_rx,
                        &temp_output,
                        encode_progress_tx,
                    )?;
                    let ended = std::time::SystemTime::now();
                    let bytes = temp_output.metadata().ok().map_or(0, |meta| meta.len());
                    if result.status.success() {
                        size_tx.send(SequenceStatus::Whole(Status::Processing {
                            id:         task.original_index.to_string(),
                            completion: SequenceCompletion::Custom {
                                name:      "size".to_owned(),
                                completed: bytes as f64,
                                total:     bytes as f64,
                            },
                        }))?;
                        // Rename to final output
                        fs::rename(temp_output, &task.output)?;
                    } else {
                        encoder_errored.store(true, Ordering::Relaxed);
                    }
                    debug!(
                        "Encoded Scene {} in {} seconds yielding {} bytes",
                        task.original_index,
                        ended.duration_since(started)?.as_secs(),
                        bytes
                    );
                    worker_semaphore.release(); // Release for next worker
                    decoder_semaphore_clone.release(); // Release for next decoder
                    let result = ParallelEncoderResult {
                        scene: task.original_index,
                        started,
                        ended,
                        bytes,
                        result,
                    };
                    Ok(Some(result))
                });

                encoder_threads.push(encoder_thread);
            }

            for task in tasks {
                // Wait for decoder semaphore (unblocked when encoder finishes)
                // Prevents decoder from filling up memory for upcoming tasks
                decoder_semaphore.acquire();
                if !cancelled.load(Ordering::Relaxed) && !encoder_errored.load(Ordering::Relaxed) {
                    debug!("Decoding Scene {}", task.original_index);
                    let frames_tx =
                        frames_senders.remove(&task.index).expect("should have frames_tx");
                    let y4m_header = input.y4m_header(Some(task.end_frame - task.start_frame))?;
                    frames_tx.send(Cursor::new(Vec::from(y4m_header.as_bytes())))?;

                    input.y4m_frames(frames_tx, task.start_frame, task.end_frame)?;
                }
                encoder_semaphores
                    .get(&task.index)
                    .expect("should have encoder_semaphore")
                    .release();
            }

            let encoder_results = encoder_threads
                .into_iter()
                .map(|result| {
                    result
                        .join()
                        .expect("should join encoder thread")
                        .expect("should join encoder_thread")
                })
                .collect::<Vec<_>>();
            drop(progress_tx);

            let first_error = encoder_results.iter().find(|maybe_result| {
                maybe_result.as_ref().is_some_and(|result| !result.result.status.success())
            });
            if let Some(Some(first_error)) = first_error {
                let err = ParallelEncoderError::EncoderFailed {
                    scene:  first_error.scene,
                    result: first_error.result.clone(),
                };
                error!("{}", err);
                bail!(err);
            }

            Ok(encoder_results)
        })
    }
}

#[derive(Debug, Clone)]
pub struct Task {
    original_index: usize,
    index:          usize,
    start_frame:    usize,
    end_frame:      usize,
    sub_scenes:     Option<Vec<SubScene>>,
    encoder:        Encoder,
    output:         PathBuf,
}

pub struct Semaphore {
    permit_count: AtomicUsize,
    signal:       Mutex<()>,
    condvar:      Condvar,
}

impl Semaphore {
    #[inline]
    pub fn new(initial_permits: usize) -> Self {
        Semaphore {
            permit_count: AtomicUsize::new(initial_permits),
            signal:       Mutex::new(()),
            condvar:      Condvar::new(),
        }
    }

    /// Acquire a permit and block until one is available.
    #[inline]
    pub fn acquire(&self) -> usize {
        loop {
            let current_count = self.permit_count.load(Ordering::SeqCst);
            if current_count > 0 {
                if let Ok(updated_count) = self.permit_count.compare_exchange(
                    current_count,
                    current_count - 1,
                    Ordering::SeqCst,
                    Ordering::Relaxed,
                ) {
                    // Semaphore acquired, return count as ID
                    return updated_count;
                }
            } else {
                // Block with Condvar
                let lock_guard = self.signal.lock().expect("Mutex poisoned");
                // In case count increases after lock
                if self.permit_count.load(Ordering::SeqCst) > 0 {
                    continue; // Loop back and try the atomic decrement path.
                }
                // Block until released and drop lock guard
                drop(self.condvar.wait(lock_guard).expect("Condvar poisoned"));
            }
        }
    }

    /// Releases a permit, allowing next acquire to succeed.
    #[inline]
    pub fn release(&self) {
        self.permit_count.fetch_add(1, Ordering::SeqCst);

        // Unblock Condvar
        drop(self.signal.lock().expect("Mutex poisoned"));
        self.condvar.notify_one();
    }
}

pub struct ParallelEncoderResult {
    pub scene:   usize,
    pub started: std::time::SystemTime,
    pub ended:   std::time::SystemTime,
    pub bytes:   u64,
    pub result:  EncoderResult,
}

#[derive(Debug, Clone, Error)]
pub enum ParallelEncoderError {
    #[error("Must have at least one worker")]
    NoWorkers,
    #[error("No Scenes found")]
    ScenesEmpty,
    #[error("Failed to encode Scene {scene}: {result}")]
    EncoderFailed {
        scene:  usize,
        result: EncoderResult,
    },
}
