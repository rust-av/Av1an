use crate::utils::cancellation::Cancellation;
use std::sync::Arc;

use anyhow::{Result, bail, ensure};
use av1_grain::write_grain_table;
use tracing::{debug, error};

use crate::models::sequence::{
    provenance::{self, ResultProvenance},
    target_quality::{TargetQualityConfigHandler, TargetQualityDataHandler},
};
use crate::{
    core::{
        Condor,
        input::Input,
        sequence::{Sequence, SequenceCompletion, SequenceDetails, SequenceStatus, Status},
    },
    models::sequence::{
        SequenceConfigHandler, SequenceDataHandler,
        parallel_encoder::{ParallelEncoderConfigHandler, ParallelEncoderDataHandler},
    },
};

pub(crate) mod artifact;
mod encode;
mod error;
mod lifecycle;
mod progress;
mod streaming;
mod task;

pub use error::ParallelEncoderError;
pub(super) use lifecycle::TempOutput;
pub use task::{ParallelEncoderResult, Task};

/// Per-scene dispatch policy. A try includes every pass and output validation.
#[derive(Clone, Copy, Debug)]
pub struct ParallelEncoderOptions {
    pub max_tries: u32,
    pub validate_frame_count: bool,
}

impl Default for ParallelEncoderOptions {
    #[inline]
    fn default() -> Self {
        Self {
            max_tries: 3,
            validate_frame_count: true,
        }
    }
}

/// Frames each streaming worker keeps requested ahead and queued for its
/// encoder. 4-8 measured as fast as larger windows with less memory.
pub(super) const STREAM_WINDOW: usize = 8;

pub static DETAILS: SequenceDetails = SequenceDetails {
    name: "Parallel Encoder",
    description: "Encodes a set of scenes in parallel until all scenes are encoded.",
    version: "0.0.1",
};

pub struct ParallelEncoder {
    pub input: Option<Input>,
}

/// Bits per second for the selected frames, not the encoder's elapsed wall time.
pub(super) fn checked_bitrate(bytes: u64, frames: usize, fps: f64) -> Result<f64> {
    ensure!(
        frames > 0,
        "Cannot calculate bitrate with zero selected frames"
    );
    ensure!(
        fps.is_finite() && fps > 0.0,
        "Cannot calculate bitrate with invalid FPS {fps}"
    );
    let seconds = frames as f64 / fps;
    ensure!(
        seconds.is_finite() && seconds > 0.0,
        "Invalid bitrate duration {seconds}"
    );
    let bitrate = bytes as f64 * 8.0 / seconds;
    ensure!(bitrate.is_finite(), "Non-finite bitrate {bitrate}");
    Ok(bitrate)
}

impl<DataHandler, ConfigHandler> Sequence<DataHandler, ConfigHandler> for ParallelEncoder
where
    DataHandler: SequenceDataHandler + ParallelEncoderDataHandler + TargetQualityDataHandler,
    ConfigHandler:
        SequenceConfigHandler + ParallelEncoderConfigHandler + TargetQualityConfigHandler,
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

        for (index, scene) in condor.scenes.iter().enumerate() {
            scene.sequence_data.get_parallel_encoder()?.admit_scene(index)?;
        }

        condor.sequence_config.parallel_encoder()?.validate()?;

        if condor.sequence_config.parallel_encoder()?.workers.is_some_and(|w| w == 0) {
            bail!(ParallelEncoderError::NoWorkers);
        }

        // Fail early instead of at the first scene's filter spawn.
        if condor.scenes.iter().any(|scene| scene.encoder.ffmpeg_filter().is_some())
            && which::which("ffmpeg").is_err()
        {
            bail!(ParallelEncoderError::FfmpegNotFound);
        }

        if let Some(input) = &self.input {
            Input::validate(&input.as_data())?;
        }

        // Ensure all the scene encoders are validated
        for scene in &condor.scenes {
            scene.encoder.validate()?;
        }

        artifact::preflight(condor.sequence_config.parallel_encoder()?.validate_frame_count)?;

        Ok(((), warnings))
    }

    #[inline]
    fn initialize(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        progress_tx: std::sync::mpsc::Sender<SequenceStatus>,
        cancellation: &Cancellation,
    ) -> Result<((), Vec<anyhow::Error>)> {
        super::check_sequence(cancellation, || {
            for (index, scene) in condor.scenes.iter().enumerate() {
                scene.sequence_data.get_parallel_encoder()?.admit_scene(index)?;
            }
            condor.sequence_config.parallel_encoder()?.validate()?;
            let mut warnings = vec![];

            // Ensure scenes is not empty
            if condor.scenes.is_empty() {
                warnings.push(anyhow::Error::new(ParallelEncoderError::ScenesEmpty));
            }
            if let Some(input) = &mut self.input {
                // VapourSynth inputs may cache here, which is why this is separate
                // from validate.
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
            }

            let scenes_directory = &condor.sequence_config.parallel_encoder()?.scenes_directory;
            if !scenes_directory.exists() {
                std::fs::create_dir_all(scenes_directory)?;
            }

            // Generate Photon Noise tables
            let input = self.input.as_mut().unwrap_or(&mut condor.input);
            let clip_info = input.clip_info()?;
            let transfer_function = clip_info.photon_noise_transfer_function();
            for scene in &mut condor.scenes {
                let params = scene.encoder.generate_photon_noise_table(
                    clip_info.resolution.0,
                    clip_info.resolution.1,
                    transfer_function,
                    clip_info.color_range,
                )?;

                if let Some((hashed_name, params)) = params {
                    let output_directory = scenes_directory.join(DETAILS.name);
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
        })
    }

    #[inline]
    fn execute(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        progress_tx: std::sync::mpsc::Sender<SequenceStatus>,
        cancelled: crate::utils::cancellation::Cancellation,
    ) -> Result<((), Vec<anyhow::Error>)> {
        for (index, scene) in condor.scenes.iter().enumerate() {
            scene.sequence_data.get_parallel_encoder()?.admit_scene(index)?;
        }
        condor.sequence_config.parallel_encoder()?.validate()?;
        cancelled.check_forced()?;
        if cancelled.is_cancelled() {
            return Ok(((), Vec::new()));
        }
        // TODO:
        // handle subscenes

        let mut warnings = vec![];
        let config = condor.sequence_config.parallel_encoder()?.clone();
        let tq = condor.sequence_config.target_quality()?.clone();
        let root_model = condor.input.as_data();
        let input_model = self
            .input
            .as_ref()
            .map(Input::as_data)
            .or_else(|| config.input.clone())
            .unwrap_or_else(|| root_model.clone());
        let probe_model = provenance::probe_input(&root_model, &config, tq.as_ref()).clone();
        let metric_model = tq.as_ref().and_then(|c| c.metric_input.as_ref());
        // Admit the entire batch before metadata withdrawal, saves, or resumed events.
        let mut admission = provenance::Admission::default();
        for (index, scene) in condor.scenes.iter().enumerate() {
            admission.admit_target(&probe_model, metric_model, scene, tq.as_ref(), index)?;
            provenance::admit_selection(scene, tq.as_ref(), index)?;
            let output = config.scenes_directory.join(format!(
                "{}.{}",
                Self::scene_id(index),
                scene.encoder.base().output_extension()
            ));
            let data = scene.sequence_data.get_parallel_encoder()?;
            if data.bytes.is_some()
                || data.started_on.is_some()
                || data.completed_on.is_some()
                || output.symlink_metadata().is_ok()
            {
                admission
                    .admit_encode(
                        &input_model,
                        &probe_model,
                        metric_model,
                        scene,
                        tq.as_ref(),
                        &output,
                        index,
                    )
                    .map_err(|e| e.context(format!("saved artifact {}", output.display())))?;
            }
        }
        let workers = config.workers.unwrap_or(1);
        let scenes_directory = &config.scenes_directory;
        if condor.scenes.is_empty() {
            warnings.push(anyhow::Error::new(ParallelEncoderError::ScenesEmpty));
            return Ok(((), warnings));
        }

        let mut tasks = std::collections::VecDeque::new();
        let mut resumed = Vec::new();
        let mut resume_error = None;
        let mut metadata_changed = false;
        let scene_count = condor.scenes.len();
        for (original_index, scene) in condor.scenes.iter_mut().enumerate() {
            cancelled.check_forced()?;
            // Finish an active finite probe during Graceful, but admit neither
            // another candidate nor a new encode after it returns.
            if cancelled.is_cancelled() {
                break;
            }
            let output = scenes_directory.join(format!(
                "{}.{}",
                Self::scene_id(original_index),
                scene.encoder.base().output_extension()
            ));
            let data = scene.sequence_data.get_parallel_encoder_mut()?;
            let recorded =
                data.bytes.is_some() || data.started_on.is_some() || data.completed_on.is_some();
            let present = match output.symlink_metadata() {
                Ok(_) => true,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                // Let the validator reject unreadable candidates through the same
                // metadata-withdrawal/save path as corrupt and missing chunks.
                Err(_) => true,
            };
            if present || recorded {
                let validation_progress = |completed: usize| {
                    let _ = progress_tx.send(SequenceStatus::Whole(Status::Processing {
                        id: DETAILS.name.to_owned(),
                        completion: SequenceCompletion::Custom {
                            name: "validating".to_owned(),
                            completed: completed as f64,
                            total: scene_count as f64,
                        },
                    }));
                };
                validation_progress(original_index);
                let eligible = (|| -> Result<u64> {
                    ensure!(
                        data.bytes.is_some_and(|bytes| bytes > 0)
                            && data
                                .started_on
                                .zip(data.completed_on)
                                .is_some_and(|(start, end)| start <= end),
                        "artifact has no usable saved completed encoding record"
                    );
                    artifact::validate(
                        &output,
                        (scene.start_frame..scene.end_frame).len(),
                        config.validate_frame_count,
                        &cancelled,
                    )
                })();
                match eligible {
                    Ok(bytes) => {
                        metadata_changed |= data.bytes != Some(bytes);
                        data.bytes = Some(bytes);
                        resumed.push((original_index, bytes));
                    },
                    Err(error) if cancelled.is_forced() => return Err(error),
                    Err(error) => {
                        let conclusively_unusable = match output.metadata() {
                            Ok(metadata) => !metadata.is_file() || metadata.len() == 0,
                            Err(error) => error.kind() == std::io::ErrorKind::NotFound,
                        };
                        // Probe/count/read failures withhold eligibility, not the
                        // historical record needed to retry after policy/tool repair.
                        if conclusively_unusable {
                            *data = Default::default();
                            metadata_changed |= recorded;
                        }
                        lifecycle::retain_error(&mut resume_error, error.context(format!("Scene {original_index} cannot resume {}; file preserved. Repair the probe or intentional count policy and retry; otherwise recompute in a new job or move the chunk aside and clear its saved completion record", output.display())));
                    },
                }
                validation_progress(original_index + 1);
                continue;
            }
            tasks.push_back(Task {
                original_index,
                index: tasks.len(),
                frame_indices: (scene.start_frame..scene.end_frame).collect::<Vec<_>>(),
                sub_scenes: scene.sub_scenes.clone(),
                encoder: scene.encoder.clone(),
                output,
            });
        }
        let mut configured_input =
            if self.input.is_none() && !tasks.is_empty() && !cancelled.is_cancelled() {
                config.input.as_ref().map(Input::from_data).transpose()?
            } else {
                None
            };
        if !tasks.is_empty() && !cancelled.is_cancelled() {
            let clip_info = self
                .input
                .as_mut()
                .or(configured_input.as_mut())
                .unwrap_or(&mut condor.input)
                .clip_info()?;
            let fps = *clip_info.frame_rate.numer() as f64 / *clip_info.frame_rate.denom() as f64;
            for task in &tasks {
                checked_bitrate(0, task.frame_indices.len(), fps).map_err(|error| {
                    error.context(format!("Scene {} bitrate preflight", task.original_index))
                })?;
            }
        }
        if metadata_changed {
            let saved = (condor.save_callback)(condor.as_data());
            if let Err(error) = saved {
                lifecycle::retain_error(
                    &mut resume_error,
                    error.context("Saving artifact eligibility metadata"),
                );
            }
        }
        if let Some(error) = resume_error {
            return Err(error);
        }
        let initial_frames = resumed
            .iter()
            .map(|(index, _)| condor.scenes[*index].end_frame - condor.scenes[*index].start_frame)
            .sum::<usize>();
        let total_frames =
            tasks.iter().map(|task| task.frame_indices.len()).sum::<usize>() + initial_frames;
        for (scene, bytes) in resumed {
            cancelled.publish(|| {
                let _ = progress_tx.send(SequenceStatus::Whole(Status::Processing {
                    id: scene.to_string(),
                    completion: SequenceCompletion::Custom {
                        // Private parallel protocol: only a currently validated
                        // resume contributes to the initial-completed FPS baseline.
                        name: "resumed-size".to_owned(),
                        completed: bytes as f64,
                        total: bytes as f64,
                    },
                }));
                Ok(())
            })?;
        }
        let _ = progress_tx.send(SequenceStatus::Whole(Status::Processing {
            id: DETAILS.name.to_owned(),
            completion: SequenceCompletion::Frames {
                completed: initial_frames as u64,
                total: total_frames as u64,
            },
        }));
        if tasks.is_empty() || cancelled.is_cancelled() {
            if tasks.is_empty() && !cancelled.is_cancelled() {
                cancelled.publish(|| {
                    let _ = progress_tx.send(SequenceStatus::Whole(Status::Completed {
                        id: DETAILS.name.to_owned(),
                    }));
                    Ok(())
                })?;
            }
            return Ok(((), warnings));
        }

        let Condor {
            input: condor_input,
            output,
            encoder,
            scenes,
            sequence_config,
            save_callback,
        } = condor;
        let input_data = condor_input.as_data();
        let before =
            provenance::Admission::default().measurement_inputs(&input_model, None, None)?;
        let input = self.input.as_mut().or(configured_input.as_mut()).unwrap_or(condor_input);

        let (results_tx, results_rx) = crossbeam_channel::unbounded();
        let finished_scenes = Arc::new(crate::utils::semaphore::Semaphore::new(0));
        let mut on_results = |results: Vec<ParallelEncoderResult>| -> Result<()> {
            ensure!(
                before
                    == provenance::Admission::default().measurement_inputs(
                        &input_model,
                        None,
                        None
                    )?,
                "Encoding input changed during processing; recompute or use a new job"
            );
            let mut staged = scenes.clone();
            let mut publication = provenance::Admission::default();
            let mut published = Vec::new();
            for encoder_result in results {
                if let Some(scene) = staged.get_mut(encoder_result.scene)
                    && encoder_result.result.status.success()
                    && encoder_result.bytes != 0
                {
                    let path = config.scenes_directory.join(format!(
                        "{}.{}",
                        Self::scene_id(encoder_result.scene),
                        scene.encoder.base().output_extension()
                    ));
                    let context = publication.encode_context(
                        &input_model,
                        &probe_model,
                        metric_model,
                        scene,
                        tq.as_ref(),
                        &path,
                        encoder_result.scene,
                    )?;
                    let parallel_encode_data = scene.sequence_data.get_parallel_encoder_mut()?;
                    parallel_encode_data.provenance = Some(ResultProvenance::new(context));
                    parallel_encode_data.bytes = Some(encoder_result.bytes);
                    parallel_encode_data.started_on = Some(
                        encoder_result
                            .started
                            .duration_since(std::time::UNIX_EPOCH)
                            .expect("Time is valid")
                            .as_millis(),
                    );
                    parallel_encode_data.completed_on = Some(
                        encoder_result
                            .ended
                            .duration_since(std::time::UNIX_EPOCH)
                            .expect("Time is valid")
                            .as_millis(),
                    );
                    published.push(encoder_result.scene);
                }
            }

            let data = crate::models::Condor {
                input: input_data.clone(),
                output: output.as_data(),
                encoder: encoder.clone(),
                scenes: staged.clone(),
                sequence_config: sequence_config.clone(),
            };
            *scenes = staged;
            if let Err(error) = (save_callback)(data) {
                // Retain observations for diagnostics, but a failed publication
                // callback cannot authorize a later saved-job resume.
                for index in published {
                    scenes[index].sequence_data.get_parallel_encoder_mut()?.provenance = None;
                }
                return Err(error);
            }
            Ok(())
        };
        let mut stream = task::ResultStream {
            results_tx,
            results_rx: &results_rx,
            finished_scenes,
            on_results: &mut on_results,
        };

        let encoder_thread = ParallelEncoder::encode_tasks_with_baseline(
            input,
            workers,
            tasks,
            progress_tx,
            cancelled,
            &mut stream,
            ParallelEncoderOptions {
                max_tries: config.max_tries,
                validate_frame_count: config.validate_frame_count,
            },
            initial_frames,
        );

        if let Err(e) = encoder_thread {
            error!("{}", e);
            return Err(e);
        }

        progress::drain_finished_results(&mut stream)?;

        Ok(((), warnings))
    }
}

impl Default for ParallelEncoder {
    #[inline]
    fn default() -> Self {
        Self { input: None }
    }
}

impl ParallelEncoder {
    pub const DETAILS: SequenceDetails = DETAILS;

    #[inline]
    pub fn new(input: Option<Input>) -> Self {
        ParallelEncoder { input }
    }

    #[inline]
    pub fn scene_id(index: usize) -> String {
        format!("{:>05}", index)
    }
}

#[cfg(test)]
mod bitrate_tests {
    use super::checked_bitrate;

    #[test]
    fn checked_bitrate_converts_bytes_and_frame_duration_to_bits_per_second() {
        for (bytes, frames, fps, expected) in [
            (1000, 240, 24.0, 800.0),
            (1000, 240, 24000.0 / 1001.0, 800000.0 / 1001.0),
            (0, 240, 24.0, 0.0),
            (u64::MAX, 8, 1.0, 18446744073709551616.0),
            (1, 1, 1e100, 8e100),
        ] {
            let actual = checked_bitrate(bytes, frames, fps).expect("valid bps");
            assert!(
                (actual - expected).abs() <= expected.abs() * 1e-14,
                "{actual} != {expected}"
            );
        }
    }

    #[test]
    fn checked_bitrate_rejects_invalid_duration_and_nonfinite_results() {
        for (bytes, frames, fps) in [
            (1000, 0, 24.0),
            (1000, 240, 0.0),
            (1000, 240, -24.0),
            (1000, 240, f64::NAN),
            (1000, 240, f64::INFINITY),
            (1000, 240, f64::NEG_INFINITY),
            (1000, 240, f64::from_bits(1)), // Duration overflows despite finite positive FPS.
            (1000, 1, f64::MAX),            // Duration is positive, but bps overflows.
        ] {
            assert!(
                checked_bitrate(bytes, frames, fps).is_err(),
                "accepted {bytes}/{frames}/{fps}"
            );
        }
    }
}

#[cfg(test)]
#[cfg(target_os = "linux")]
pub(in crate::core::sequence) mod tests;
