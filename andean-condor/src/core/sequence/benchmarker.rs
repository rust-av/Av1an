use crate::utils::cancellation::Cancellation;
use std::{
    collections::VecDeque,
    sync::{self, Arc, atomic::Ordering},
    thread,
    time::{Duration, Instant},
};

use anyhow::Result;
use thiserror::Error;

use crate::{
    core::{
        Condor,
        input::Input,
        sequence::{
            Sequence, SequenceCompletion, SequenceDetails, SequenceStatus, Status,
            parallel_encoder::{ParallelEncoder, Task as ParallelEncodeTask},
        },
    },
    models::sequence::{
        SequenceConfigHandler, SequenceDataHandler, benchmarker::BenchmarkerConfigHandler,
        parallel_encoder::ParallelEncoderConfigHandler, scene_detector::SceneDetectorDataHandler,
    },
};

pub static DETAILS: SequenceDetails = SequenceDetails {
    name: "Benchmarker",
    description: "Measures how fast scenes can be encoded in parallel before no meaningful \
                  improvement is seen by adding more workers.",
    version: "0.0.1",
};

pub struct Benchmarker {
    /// Should be identical to the Input used for Parallel Encoder
    pub input: Option<Input>,
}

impl<DataHandler, ConfigHandler> Sequence<DataHandler, ConfigHandler> for Benchmarker
where
    DataHandler: SequenceDataHandler + SceneDetectorDataHandler,
    ConfigHandler: SequenceConfigHandler + ParallelEncoderConfigHandler + BenchmarkerConfigHandler,
{
    #[inline]
    fn details(&self) -> SequenceDetails {
        DETAILS
    }

    #[inline]
    fn validate(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
    ) -> Result<((), Vec<anyhow::Error>)> {
        let warnings = vec![];

        // Ensure all the scene encoders are validated
        for scene in &condor.scenes {
            scene.encoder.validate()?;
        }
        condor.encoder.validate()?;
        super::parallel_encoder::artifact::preflight(
            condor.sequence_config.parallel_encoder()?.validate_frame_count,
        )?;

        Ok(((), warnings))
    }

    #[inline]
    fn initialize(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        progress_tx: sync::mpsc::Sender<SequenceStatus>,
        cancellation: &Cancellation,
    ) -> Result<((), Vec<anyhow::Error>)> {
        super::check_sequence(cancellation, || {
            let mut warnings = vec![];

            let parallel_encoder_config = condor.sequence_config.parallel_encoder()?;

            // Ensure scenes is not empty
            if condor.scenes.is_empty() {
                warnings.push(anyhow::Error::new(BenchmarkerError::ScenesEmpty));
                return Ok(((), warnings));
            }
            let input = if let Some(input) = self.input.as_mut() {
                input
            } else if let Some(input_model) = &parallel_encoder_config.input {
                &mut Input::ensure_indexed(input_model, |_| {}, cancellation)?
            } else {
                &mut condor.input
            };
            // Initialize input by getting clip_info. For VapourSynth inputs, this
            // may begin a lengthy caching process, hence the separation
            // between validate and initialize.
            progress_tx.send(SequenceStatus::Whole(Status::Processing {
                id: DETAILS.name.to_owned(),
                completion: SequenceCompletion::Custom {
                    name: DETAILS.name.to_owned(),
                    completed: 0.0,
                    total: 1.0,
                },
            }))?;
            input.clip_info()?;
            progress_tx.send(SequenceStatus::Whole(Status::Completed {
                id: DETAILS.name.to_owned(),
            }))?;

            let benchmarker_directory =
                &parallel_encoder_config.scenes_directory.join(DETAILS.name);
            if !benchmarker_directory.exists() {
                std::fs::create_dir_all(benchmarker_directory)?;
            }

            Ok(((), warnings))
        })
    }

    #[inline]
    fn execute(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        progress_tx: sync::mpsc::Sender<SequenceStatus>,
        cancelled: crate::utils::cancellation::Cancellation,
    ) -> Result<((), Vec<anyhow::Error>)> {
        cancelled.check()?;
        const BENCHMARK_SCENES: usize = 16;
        const BENCHMARK_SCENE_FRAMES: usize = 24;
        let (batch_scenes, batch_frames) = (BENCHMARK_SCENES, BENCHMARK_SCENE_FRAMES);
        #[cfg(all(test, target_os = "linux"))]
        let (batch_scenes, batch_frames) =
            super::parallel_encoder::tests::artifact_validation::resume_policy::internal_integration::fixed_batch()
                .unwrap_or((batch_scenes, batch_frames));
        let mut warnings = vec![];
        let parallel_encoder_config = condor.sequence_config.parallel_encoder()?;
        let config = condor.sequence_config.benchmarker()?;
        let input = if let Some(input) = self.input.as_mut() {
            input
        } else if let Some(input_model) = &parallel_encoder_config.input {
            &mut Input::ensure_indexed(input_model, |_| {}, &cancelled)?
        } else {
            &mut condor.input
        };
        let benchmarker_directory = &parallel_encoder_config.scenes_directory.join(DETAILS.name);

        if condor.scenes.is_empty() {
            warnings.push(anyhow::Error::new(BenchmarkerError::ScenesEmpty));
            return Ok(((), warnings));
        }

        if parallel_encoder_config.workers.is_some_and(|w| w > 0) {
            warnings.push(anyhow::Error::new(
                BenchmarkerError::WorkersAlreadyConfigured,
            ));
            return Ok(((), warnings));
        }

        let total_frames = input.clip_info()?.num_frames;
        let scene_frames = batch_frames.min(total_frames).max(1);
        let tasks: Vec<ParallelEncodeTask> = (0..batch_scenes)
            .map(|index| ParallelEncodeTask {
                index,
                original_index: index,
                frame_indices: (0..scene_frames).collect::<Vec<_>>(),
                sub_scenes: None,
                encoder: condor.encoder.clone(),
                output: benchmarker_directory.join(format!(
                    "{}.{}",
                    index,
                    condor.encoder.base().output_extension()
                )),
            })
            .collect();

        let mut previous_result = Self::benchmark_workers(
            input,
            1,
            tasks.as_slice(),
            &progress_tx,
            &cancelled,
            super::parallel_encoder::ParallelEncoderOptions {
                max_tries: parallel_encoder_config.max_tries,
                validate_frame_count: parallel_encoder_config.validate_frame_count,
            },
        )?;
        #[cfg(all(test, target_os = "linux"))]
        super::parallel_encoder::tests::artifact_validation::resume_policy::internal_integration::finish_fixed_batch(
            &cancelled,
        );

        loop {
            cancelled.check_forced()?;
            if cancelled.is_cancelled() {
                break;
            }
            let current_result = Self::benchmark_workers(
                input,
                previous_result.workers + 1,
                tasks.as_slice(),
                &progress_tx,
                &cancelled,
                super::parallel_encoder::ParallelEncoderOptions {
                    max_tries: parallel_encoder_config.max_tries,
                    validate_frame_count: parallel_encoder_config.validate_frame_count,
                },
            )?;
            cancelled.check_forced()?;
            if cancelled.is_cancelled() {
                break;
            }

            // Threshold is in percent
            if current_result.frames_per_second()
                < (previous_result.frames_per_second() * (1.0 + (config.threshold as f64 / 100.0)))
            {
                // Speed did not increase by threshold, revert to previous
                // amount of workers
                condor.sequence_config.parallel_encoder_mut()?.workers =
                    Some(previous_result.workers);

                progress_tx.send(SequenceStatus::Whole(Status::Failed {
                    id: current_result.workers.to_string(),
                    error: BenchmarkerError::ThresholdNotMet.to_string(),
                }))?;
                break;
            } else {
                previous_result = current_result;
                progress_tx.send(SequenceStatus::Whole(Status::Completed {
                    id: previous_result.workers.to_string(),
                }))?;
            }
        }

        condor.save()?;
        // Trial directories clean only their own outputs; preexisting scratch
        // contents do not belong to this invocation.

        progress_tx.send(SequenceStatus::Whole(Status::Completed {
            id: DETAILS.name.to_string(),
        }))?;

        Ok(((), warnings))
    }
}

impl Default for Benchmarker {
    #[inline]
    fn default() -> Self {
        Self { input: None }
    }
}

impl Benchmarker {
    pub const DETAILS: SequenceDetails = DETAILS;

    /// A fixed trial with explicit per-scene total-attempt/count policy.
    #[inline]
    pub fn benchmark_workers(
        input: &mut Input,
        workers: u8,
        tasks: &[ParallelEncodeTask],
        progress_tx: &sync::mpsc::Sender<SequenceStatus>,
        cancelled: &crate::utils::cancellation::Cancellation,
        options: super::parallel_encoder::ParallelEncoderOptions,
    ) -> Result<BenchmarkResult> {
        anyhow::ensure!(
            options.max_tries >= 1,
            "max_tries must be at least 1 total attempt"
        );
        for task in tasks {
            task.encoder.validate()?;
        }
        super::parallel_encoder::artifact::preflight(options.validate_frame_count)?;
        // Each worker-count trial owns fresh outputs, not the previous trial's
        // files. The parallel dispatcher deliberately refuses existing targets.
        let directory = tasks
            .first()
            .and_then(|task| task.output.parent())
            .ok_or(BenchmarkerError::ScenesEmpty)?;
        let attempt = tempfile::tempdir_in(directory)?;
        let tasks = tasks
            .iter()
            .cloned()
            .map(|mut task| {
                task.output = attempt.path().join(format!(
                    "{}.{}",
                    task.index,
                    task.encoder.base().output_extension()
                ));
                task
            })
            .collect::<VecDeque<_>>();
        let start_time = Instant::now();
        let frames_completed = sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let frames_completed_clone = Arc::clone(&frames_completed);

        let progress_tx_clone = progress_tx.clone();
        let (encode_progress_tx, encode_progress_rx) = sync::mpsc::channel();
        let relay = thread::spawn(move || -> Result<()> {
            for progress in encode_progress_rx {
                match progress {
                    SequenceStatus::Whole(Status::Processing {
                        id: _id,
                        completion,
                    }) =>
                    {
                        #[allow(clippy::collapsible_match)]
                        if let SequenceCompletion::Frames { completed, total } = completion {
                            frames_completed_clone.store(completed as usize, Ordering::SeqCst);
                            progress_tx_clone.send(SequenceStatus::Subprocess {
                                parent: Status::Processing {
                                    id: DETAILS.name.to_string(),
                                    completion: SequenceCompletion::Passes {
                                        completed: workers,
                                        total: workers,
                                    },
                                },
                                child: Status::Processing {
                                    id: workers.to_string(),
                                    completion: SequenceCompletion::Frames { completed, total },
                                },
                            })?;
                        }
                    },
                    SequenceStatus::Whole(Status::Completed { id: _ }) => {
                        progress_tx_clone.send(SequenceStatus::Subprocess {
                            parent: Status::Processing {
                                id: DETAILS.name.to_string(),
                                completion: SequenceCompletion::Passes {
                                    completed: workers,
                                    total: workers,
                                },
                            },
                            child: Status::Completed {
                                id: workers.to_string(),
                            },
                        })?;
                    },
                    _ => (),
                }
            }

            Ok(())
        });

        let encoded = ParallelEncoder::encode_tasks(
            input,
            workers,
            tasks,
            encode_progress_tx,
            cancelled.clone(),
            options,
        );
        let relayed =
            crate::core::encoder::pipes::join_worker(relay, "Joining worker-probe progress");
        encoded?;
        relayed?;

        let result = BenchmarkResult {
            workers,
            frames_encoded: frames_completed.load(Ordering::Relaxed),
            time: start_time.elapsed(),
        };

        Ok(result)
    }
}

pub struct BenchmarkResult {
    pub workers: u8,
    pub frames_encoded: usize,
    pub time: Duration,
}

impl BenchmarkResult {
    #[inline]
    pub fn frames_per_second(&self) -> f64 {
        self.frames_encoded as f64 / self.time.as_secs_f64()
    }
}

#[derive(Debug, Clone, Error)]
pub enum BenchmarkerError {
    #[error("No Scenes found")]
    ScenesEmpty,
    #[error("Parallel Encoder workers already configured")]
    WorkersAlreadyConfigured,
    #[error("Failed to encode")]
    EncoderFailed,
    #[error("Threshold not met")]
    ThresholdNotMet,
}
