use std::sync::Arc;

use anyhow::{Result, bail};
use av1_grain::write_grain_table;
use tracing::{debug, error};

use crate::{
    core::{
        Condor,
        input::Input,
        sequence::{Sequence, SequenceCompletion, SequenceDetails, SequenceStatus, Status},
    },
    models::sequence::{
        SequenceConfigHandler,
        SequenceDataHandler,
        parallel_encoder::{ParallelEncoderConfigHandler, ParallelEncoderDataHandler},
    },
};

mod encode;
mod error;
mod progress;
mod streaming;
mod task;

pub use error::ParallelEncoderError;
pub use task::{ParallelEncoderResult, Task};

/// Frames each streaming worker keeps requested ahead and queued for its
/// encoder. 4-8 measured as fast as larger windows with less memory.
pub(super) const STREAM_WINDOW: usize = 8;

pub static DETAILS: SequenceDetails = SequenceDetails {
    name:        "Parallel Encoder",
    description: "Encodes a set of scenes in parallel until all scenes are encoded.",
    version:     "0.0.1",
};

pub struct ParallelEncoder {
    pub input: Option<Input>,
}

impl<DataHandler, ConfigHandler> Sequence<DataHandler, ConfigHandler> for ParallelEncoder
where
    DataHandler: SequenceDataHandler + ParallelEncoderDataHandler,
    ConfigHandler: SequenceConfigHandler + ParallelEncoderConfigHandler,
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

        Ok(((), warnings))
    }

    #[inline]
    fn initialize(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        progress_tx: std::sync::mpsc::Sender<SequenceStatus>,
    ) -> Result<((), Vec<anyhow::Error>)> {
        let mut warnings = vec![];

        // Ensure scenes is not empty
        if condor.scenes.is_empty() {
            warnings.push(anyhow::Error::new(ParallelEncoderError::ScenesEmpty));
        }
        if let Some(input) = &mut self.input {
            // VapourSynth inputs may cache here, which is why this is separate
            // from validate.
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
    }

    #[inline]
    fn execute(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        progress_tx: std::sync::mpsc::Sender<SequenceStatus>,
        cancelled: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<((), Vec<anyhow::Error>)> {
        // TODO:
        // handle subscenes

        let mut warnings = vec![];
        let config = condor.sequence_config.parallel_encoder()?.clone();
        let workers = config.workers.unwrap_or(1);
        let scenes_directory = &config.scenes_directory;
        if condor.scenes.is_empty() {
            warnings.push(anyhow::Error::new(ParallelEncoderError::ScenesEmpty));
            return Ok(((), warnings));
        }

        let tasks = condor
            .scenes
            .iter()
            .enumerate()
            .filter(|(index, scene)| {
                let output = scenes_directory.join(format!(
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
                frame_indices: (scene.start_frame..scene.end_frame).collect::<Vec<_>>(),
                sub_scenes: scene.sub_scenes.clone(),
                encoder: scene.encoder.clone(),
                output: scenes_directory.join(format!(
                    "{}.{}",
                    Self::scene_id(original_index),
                    scene.encoder.output_extension()
                )),
            })
            .collect::<std::collections::VecDeque<_>>();

        let Condor {
            input: condor_input,
            output,
            encoder,
            scenes,
            sequence_config,
            save_callback,
        } = condor;
        let input_data = condor_input.as_data();
        let input = self.input.as_mut().unwrap_or(condor_input);

        let (results_tx, results_rx) = crossbeam_channel::unbounded();
        let finished_scenes = Arc::new(crate::utils::semaphore::Semaphore::new(0));
        let mut on_results = |results: Vec<ParallelEncoderResult>| -> Result<()> {
            for encoder_result in results {
                if let Some(scene) = scenes.get_mut(encoder_result.scene)
                    && encoder_result.bytes != 0
                {
                    let parallel_encode_data = scene.sequence_data.get_parallel_encoder_mut()?;
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
                }
            }

            let data = crate::models::Condor {
                input:           input_data.clone(),
                output:          output.as_data(),
                encoder:         encoder.clone(),
                scenes:          scenes.clone(),
                sequence_config: sequence_config.clone(),
            };
            (save_callback)(data)?;
            Ok(())
        };
        let mut stream = task::ResultStream {
            results_tx,
            results_rx: &results_rx,
            finished_scenes,
            on_results: &mut on_results,
        };

        let encoder_thread = ParallelEncoder::encode_tasks_with_sender(
            input,
            workers,
            tasks,
            progress_tx,
            cancelled,
            &mut stream,
        );

        progress::drain_finished_results(&mut stream)?;
        on_results(Vec::new())?;

        if let Err(e) = encoder_thread {
            error!("{}", e);
            return Err(e);
        }

        Ok(((), warnings))
    }
}

impl Default for ParallelEncoder {
    #[inline]
    fn default() -> Self {
        Self {
            input: None
        }
    }
}

impl ParallelEncoder {
    pub const DETAILS: SequenceDetails = DETAILS;

    #[inline]
    pub fn new(input: Option<Input>) -> Self {
        ParallelEncoder {
            input,
        }
    }

    #[inline]
    pub fn scene_id(index: usize) -> String {
        format!("{:>05}", index)
    }
}
