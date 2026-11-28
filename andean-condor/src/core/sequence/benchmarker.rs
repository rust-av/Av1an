use crate::utils::cancellation::Cancellation;
use std::{
    collections::VecDeque,
    sync::{self, Arc, atomic::Ordering},
    thread,
    time::{Duration, Instant},
};

use anyhow::Result;
use thiserror::Error;
use tracing::{debug, trace};

use crate::{
    core::{
        Condor,
        encoder::{MIB, ResourceUsageTracker},
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
        let (threshold, max_memory) = {
            let config = condor.sequence_config.benchmarker()?;
            (config.threshold, config.max_memory)
        };
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

        let options = super::parallel_encoder::ParallelEncoderOptions {
            max_tries: parallel_encoder_config.max_tries,
            validate_frame_count: parallel_encoder_config.validate_frame_count,
            ..Default::default()
        };
        let mut previous_result = Self::benchmark_workers(
            input,
            1,
            tasks.as_slice(),
            &progress_tx,
            &cancelled,
            &options,
        )?;
        #[cfg(all(test, target_os = "linux"))]
        super::parallel_encoder::tests::artifact_validation::resume_policy::internal_integration::finish_fixed_batch(
            &cancelled,
        );

        if previous_result.exceeds_memory_limit(max_memory) {
            // One worker is the smallest configuration, so no trial would use less.
            condor.sequence_config.parallel_encoder_mut()?.workers = Some(previous_result.workers);
            progress_tx.send(SequenceStatus::Whole(Status::Failed {
                id: previous_result.workers.to_string(),
                error: BenchmarkerError::MemoryLimitExceeded {
                    peak_mib: previous_result.peak_memory_mib(),
                    limit_mib: max_memory.unwrap_or_default(),
                }
                .to_string(),
            }))?;
        } else {
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
                    &options,
                )?;
                cancelled.check_forced()?;
                if cancelled.is_cancelled() {
                    break;
                }

                match Self::decide_trial(&previous_result, &current_result, threshold, max_memory) {
                    TrialDecision::Continue => {
                        previous_result = current_result;
                        progress_tx.send(SequenceStatus::Whole(Status::Completed {
                            id: previous_result.workers.to_string(),
                        }))?;
                    },
                    decision @ (TrialDecision::ThresholdNotMet
                    | TrialDecision::MemoryLimitExceeded) => {
                        // Revert to the last worker count that met every limit.
                        condor.sequence_config.parallel_encoder_mut()?.workers =
                            Some(previous_result.workers);
                        let error = match decision {
                            TrialDecision::MemoryLimitExceeded => {
                                BenchmarkerError::MemoryLimitExceeded {
                                    peak_mib: current_result.peak_memory_mib(),
                                    limit_mib: max_memory.unwrap_or_default(),
                                }
                            },
                            _ => BenchmarkerError::ThresholdNotMet,
                        };
                        progress_tx.send(SequenceStatus::Whole(Status::Failed {
                            id: current_result.workers.to_string(),
                            error: error.to_string(),
                        }))?;
                        break;
                    },
                }
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

    /// Decides whether `current` justifies trying another worker, given `previous`.
    /// Memory is checked first, so a faster trial cannot exceed the limit.
    #[inline]
    pub fn decide_trial(
        previous: &BenchmarkResult,
        current: &BenchmarkResult,
        threshold: u8,
        max_memory: Option<u32>,
    ) -> TrialDecision {
        if current.exceeds_memory_limit(max_memory) {
            return TrialDecision::MemoryLimitExceeded;
        }
        let required = previous.frames_per_second() * (1.0 + (f64::from(threshold) / 100.0));
        if current.frames_per_second() < required {
            TrialDecision::ThresholdNotMet
        } else {
            TrialDecision::Continue
        }
    }

    /// A fixed trial with explicit per-scene total-attempt/count policy.
    #[inline]
    pub fn benchmark_workers(
        input: &mut Input,
        workers: u8,
        tasks: &[ParallelEncodeTask],
        progress_tx: &sync::mpsc::Sender<SequenceStatus>,
        cancelled: &crate::utils::cancellation::Cancellation,
        options: &super::parallel_encoder::ParallelEncoderOptions,
    ) -> Result<BenchmarkResult> {
        trace!(
            "Benchmarker: attempting {workers} workers on {} scenes",
            tasks.len()
        );
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

        let usage = Arc::new(ResourceUsageTracker::default());
        let encoded = ParallelEncoder::encode_tasks(
            input,
            workers,
            tasks,
            encode_progress_tx,
            cancelled.clone(),
            super::parallel_encoder::ParallelEncoderOptions {
                usage: Some(Arc::clone(&usage)),
                ..options.clone()
            },
        );
        let relayed =
            crate::core::encoder::pipes::join_worker(relay, "Joining worker-probe progress");
        encoded?;
        relayed?;

        let result = BenchmarkResult {
            workers,
            frames_encoded: frames_completed.load(Ordering::Relaxed),
            time: start_time.elapsed(),
            peak_memory: usage.peak_memory(),
            peak_cpu: usage.peak_cpu(),
        };
        debug!(
            "Benchmarker: {workers} workers encoded {} frames in {:.2}s ({:.2} FPS), peak CPU {:.1}%, peak memory {:.1} MiB",
            result.frames_encoded,
            result.time.as_secs_f64(),
            result.frames_per_second(),
            result.peak_cpu,
            result.peak_memory_mib()
        );

        progress_tx.send(Self::trial_usage_status(&result))?;

        Ok(result)
    }

    /// What the UI records for a finished trial, before its verdict is known.
    #[inline]
    fn trial_usage_status(result: &BenchmarkResult) -> SequenceStatus {
        SequenceStatus::Whole(Status::Processing {
            id: result.workers.to_string(),
            completion: SequenceCompletion::ResourceUsage {
                cpu: result.peak_cpu,
                memory: result.peak_memory,
            },
        })
    }
}

pub struct BenchmarkResult {
    pub workers: u8,
    pub frames_encoded: usize,
    pub time: Duration,
    /// Peak summed resident memory of the encoders active in the trial, in bytes.
    pub peak_memory: u64,
    /// Peak summed CPU usage reported by the encoders in the trial, in percent.
    pub peak_cpu: f32,
}

impl BenchmarkResult {
    #[inline]
    pub fn frames_per_second(&self) -> f64 {
        self.frames_encoded as f64 / self.time.as_secs_f64()
    }

    /// Peak summed encoder RSS in mebibytes, rounded down.
    #[inline]
    pub fn peak_memory_mib(&self) -> u64 {
        self.peak_memory / MIB
    }

    /// Whether the trial's peak encoder memory exceeds `max_memory` MiB.
    #[inline]
    fn exceeds_memory_limit(&self, max_memory: Option<u32>) -> bool {
        max_memory.is_some_and(|limit| self.peak_memory > u64::from(limit) * MIB)
    }
}

/// Why a benchmark trial did or did not justify trying another worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrialDecision {
    Continue,
    ThresholdNotMet,
    MemoryLimitExceeded,
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
    #[error("Exceeded maximum memory: peak {peak_mib} MiB > limit {limit_mib} MiB")]
    MemoryLimitExceeded { peak_mib: u64, limit_mib: u32 },
}

#[cfg(test)]
mod tests {
    use super::*;

    const THRESHOLD: u8 = 5;

    fn trial(workers: u8, time: Duration, peak_memory: u64) -> BenchmarkResult {
        BenchmarkResult {
            workers,
            frames_encoded: 100,
            time,
            peak_memory,
            peak_cpu: 0.0,
        }
    }

    fn baseline() -> BenchmarkResult {
        trial(2, Duration::from_secs(1), 0)
    }

    #[test]
    fn faster_trial_within_the_memory_limit_continues() {
        let current = trial(3, Duration::from_millis(900), 100 * MIB);
        assert_eq!(
            Benchmarker::decide_trial(&baseline(), &current, THRESHOLD, Some(200)),
            TrialDecision::Continue
        );
    }

    #[test]
    fn trial_exactly_at_the_memory_limit_is_allowed() {
        let current = trial(3, Duration::from_millis(900), 200 * MIB);
        assert_eq!(
            Benchmarker::decide_trial(&baseline(), &current, THRESHOLD, Some(200)),
            TrialDecision::Continue
        );
    }

    #[test]
    fn trial_above_the_memory_limit_stops_even_when_it_is_faster() {
        let current = trial(3, Duration::from_millis(500), 201 * MIB);
        assert_eq!(
            Benchmarker::decide_trial(&baseline(), &current, THRESHOLD, Some(200)),
            TrialDecision::MemoryLimitExceeded
        );
    }

    #[test]
    fn memory_limit_outranks_the_speed_threshold() {
        let current = trial(3, Duration::from_secs(2), 201 * MIB);
        assert_eq!(
            Benchmarker::decide_trial(&baseline(), &current, THRESHOLD, Some(200)),
            TrialDecision::MemoryLimitExceeded
        );
    }

    #[test]
    fn trial_below_the_speed_threshold_stops() {
        let current = trial(3, Duration::from_secs(1), 100 * MIB);
        assert_eq!(
            Benchmarker::decide_trial(&baseline(), &current, THRESHOLD, Some(200)),
            TrialDecision::ThresholdNotMet
        );
    }

    #[test]
    fn absent_memory_limit_ignores_resource_usage() {
        let current = trial(3, Duration::from_millis(500), u64::MAX);
        assert_eq!(
            Benchmarker::decide_trial(&baseline(), &current, THRESHOLD, None),
            TrialDecision::Continue
        );
    }

    /// The table shows each trial's peaks, so a finished trial announces them
    /// under its own worker count, in bytes and percent as measured.
    #[test]
    fn a_trial_reports_its_peak_usage_under_its_worker_count() {
        let mut result = trial(4, Duration::from_secs(1), 3 * 1024 * MIB);
        result.peak_cpu = 812.5;
        let SequenceStatus::Whole(Status::Processing { id, completion }) =
            Benchmarker::trial_usage_status(&result)
        else {
            panic!("trial usage is a processing status");
        };
        assert_eq!(id, "4", "the usage belongs to the trial's worker count");
        match completion {
            SequenceCompletion::ResourceUsage { cpu, memory } => {
                assert_eq!(cpu, 812.5);
                assert_eq!(memory, 3 * 1024 * MIB, "bytes, not mebibytes");
            },
            other => panic!("expected a usage completion, got {other:?}"),
        }
    }
}
