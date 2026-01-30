use std::{
    collections::{HashMap, VecDeque},
    error::Error,
    path::{Path, PathBuf},
    sync::{self, atomic::AtomicBool, Arc},
    thread,
};

use anyhow::Result;
use thiserror::Error;
use tracing::debug;

use crate::{
    core::{
        input::Input,
        sequence::{
            parallel_encoder::{ParallelEncoder, Task as ParallelEncoderTask},
            scene_concatenator::SceneConcatenator,
            Sequence,
            SequenceCompletion,
            SequenceDetails,
            SequenceStatus,
            Status,
        },
        Condor,
    },
    models::{
        encoder::{cli_parameter::CLIParameter, Encoder, EncoderBase},
        sequence::{
            parallel_encoder::{BufferStrategy, ParallelEncoderConfigHandler},
            scene_concatenator::{ConcatMethod, SceneConcatenatorConfigHandler},
            target_quality::{
                types::{QualityMetric, QualityPass},
                TargetQualityConfig,
                TargetQualityConfigHandler,
                TargetQualityData,
                TargetQualityDataHandler,
            },
            SequenceConfigHandler,
            SequenceDataHandler,
        },
    },
};

static DETAILS: SequenceDetails = SequenceDetails {
    name:        "Target Quality",
    description: "Determine the optimal quantizer for a given video quality metric score per \
                  scene.",
    version:     "0.0.1",
};

pub struct TargetQuality {
    pub input: Option<Input>,
}

impl<DataHandler, ConfigHandler> Sequence<DataHandler, ConfigHandler> for TargetQuality
where
    DataHandler: SequenceDataHandler + TargetQualityDataHandler,
    ConfigHandler: SequenceConfigHandler
        + ParallelEncoderConfigHandler
        + SceneConcatenatorConfigHandler
        + TargetQualityConfigHandler,
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
        progress_tx: sync::mpsc::Sender<SequenceStatus>,
    ) -> Result<((), Vec<Box<dyn Error>>)> {
        let mut warnings: Vec<Box<dyn Error>> = vec![];

        let parallel_encoder_config = condor.sequence_config.parallel_encoder()?;

        // Ensure scenes is not empty
        if condor.scenes.is_empty() {
            warnings.push(Box::new(TargetQualityError::ScenesEmpty));
            return Ok(((), warnings));
        }
        let input = if let Some(input) = self.input.as_mut() {
            input
        } else if let Some(input_model) = &parallel_encoder_config.input {
            &mut Input::from_data(input_model)?
        } else {
            &mut condor.input
        };
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

        let benchmarker_directory = &parallel_encoder_config.scenes_directory.join(DETAILS.name);
        if !benchmarker_directory.exists() {
            std::fs::create_dir_all(benchmarker_directory)?;
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
        let mut warnings: Vec<Box<dyn Error>> = vec![];
        let parallel_encoder_config = condor.sequence_config.parallel_encoder()?;
        let scene_concatenator_config = condor.sequence_config.scene_concatenator()?;
        let config = condor.sequence_config.target_quality()?;
        let target_quality_directory = &parallel_encoder_config.scenes_directory.join(DETAILS.name);
        let workers = parallel_encoder_config.workers.unwrap_or(1);
        let buffer_strategy = &parallel_encoder_config.buffer_strategy;

        if condor.scenes.is_empty() {
            warnings.push(Box::new(TargetQualityError::ScenesEmpty));
            return Ok(((), warnings));
        }

        if config.is_none() {
            return Ok(((), warnings));
        }
        let config = config.clone().expect("TargetQualityConfig is Some");
        let input = if let Some(input) = self.input.as_mut() {
            input
        } else if let Some(input_model) = &config.input {
            &mut Input::from_data(input_model)?
        } else if let Some(input_model) = &parallel_encoder_config.input {
            &mut Input::from_data(input_model)?
        } else {
            &mut condor.input
        };

        if parallel_encoder_config.workers.is_some_and(|w| w > 0) {
            warnings.push(Box::new(TargetQualityError::WorkersAlreadyConfigured));
            return Ok(((), warnings));
        }

        let pass = 1;

        loop {
            if pass == config.maximum_probes {
                break;
            }

            let pass_directory = target_quality_directory.join(pass.to_string());
            if !pass_directory.exists() {
                std::fs::create_dir_all(&pass_directory)?;
            }

            let tasks = condor
                .scenes
                .iter()
                .enumerate()
                .map(|(index, scene)| {
                    let frame_indices =
                        config.probing.strategy.frame_indices(scene.start_frame, scene.end_frame);
                    let encoder = Self::remove_psychovisual_parameters(&scene.encoder);
                    let output = pass_directory.join(format!(
                        "{}.{}",
                        ParallelEncoder::scene_id(index),
                        encoder.output_extension()
                    ));
                    let passes = scene
                        .sequence_data
                        .get_target_quality()
                        .map_or_else(|_| TargetQualityData::default(), |tq| tq.clone())
                        .passes;

                    Task {
                        original_index: index,
                        frame_indices,
                        encoder,
                        output,
                        passes,
                    }
                })
                .map(|task| {
                    if task.passes.get((pass - 1) as usize).is_some() {
                        return Ok(None);
                    }
                    let previous_pass = task
                        .passes
                        .get((pass - 2) as usize)
                        .ok_or(TargetQualityError::PreviousPassDataNotFound)?;
                    let average_score =
                        previous_pass.scores.iter().fold(0.0, |total, score| total + score)
                            / previous_pass.scores.len() as f64;
                    if config.metric.score_within_target(average_score) {
                        return Ok(None);
                    }

                    Ok(Some(task))
                })
                .filter_map(|result: Result<Option<Task>>| result.transpose())
                .collect::<Result<Vec<_>>>()?;

            debug!("Starting Pass {}", pass);

            let (pass_progress_tx, pass_progress_rx) = sync::mpsc::channel();

            thread::spawn(move || -> Result<()> {
                for progress in pass_progress_rx {}

                Ok(())
            });

            let ((), pass_warnings) = Self::probe_pass(
                pass,
                &target_quality_directory,
                &config,
                input,
                workers,
                buffer_strategy,
                &scene_concatenator_config.method,
                tasks.as_slice(),
                &pass_progress_tx,
                &cancelled,
            )?;
            if pass_warnings.is_empty() {
                break;
            }
        }

        Ok(((), warnings))
    }
}

impl TargetQuality {
    pub const DETAILS: SequenceDetails = DETAILS;

    #[inline]
    pub fn new(input: Option<Input>) -> Self {
        Self {
            input,
        }
    }

    #[inline]
    pub fn default_quantizer_range(encoder: &EncoderBase) -> (u32, u32) {
        match encoder {
            EncoderBase::AOM | EncoderBase::VPX => (15, 55),
            EncoderBase::RAV1E => (50, 140),
            EncoderBase::SVTAV1 => (15, 50),
            EncoderBase::X264 | EncoderBase::X265 => (15, 35),
            EncoderBase::VVenC => todo!(),
            EncoderBase::FFmpeg => todo!(),
        }
    }

    /// Remove known psychovisual parameters that reduce metric accuracy.
    #[inline]
    pub fn remove_psychovisual_parameters(encoder: &Encoder) -> Encoder {
        match encoder {
            Encoder::AOM {
                executable,
                pass,
                options,
                photon_noise,
            } => {
                let psychovisual_parameters: HashMap<String, CLIParameter> =
                    std::iter::once(("film-grain-table", CLIParameter::new_string("--", "=", "")))
                        .map(|(key, value)| (key.to_owned(), value))
                        .collect();

                let mut sanitized_options = options.clone();
                for (key, value) in psychovisual_parameters {
                    if let Some((_key, unsanitized_value)) = sanitized_options.get_key_value(&key)
                        && unsanitized_value.matches(&value)
                    {
                        sanitized_options.remove(&key);
                    }
                }

                Encoder::AOM {
                    executable:   executable.clone(),
                    pass:         *pass,
                    options:      sanitized_options,
                    photon_noise: photon_noise.clone(),
                }
            },
            Encoder::RAV1E {
                executable,
                pass,
                options,
                photon_noise,
            } => {
                let psychovisual_parameters: HashMap<String, CLIParameter> = std::iter::once((
                    "photon-noise-table",
                    CLIParameter::new_string("--", "=", ""),
                ))
                .map(|(key, value)| (key.to_owned(), value))
                .collect();

                let mut sanitized_options = options.clone();
                for (key, value) in psychovisual_parameters {
                    if let Some((_key, unsanitized_value)) = sanitized_options.get_key_value(&key)
                        && unsanitized_value.matches(&value)
                    {
                        sanitized_options.remove(&key);
                    }
                }

                Encoder::RAV1E {
                    executable:   executable.clone(),
                    pass:         *pass,
                    options:      sanitized_options,
                    photon_noise: photon_noise.clone(),
                }
            },
            Encoder::VPX {
                executable,
                pass,
                options,
            } => encoder.clone(),
            Encoder::SVTAV1 {
                executable,
                pass,
                options,
                photon_noise,
            } => {
                let psychovisual_parameters: HashMap<String, CLIParameter> = [
                    ("fgs-table", CLIParameter::new_string("--", " ", "")),
                    ("film-grain", CLIParameter::new_number("--", " ", 0.0)),
                    (
                        "film-grain-denoise",
                        CLIParameter::new_number("--", " ", 0.0),
                    ),
                    ("psy-rd", CLIParameter::new_number("--", " ", 0.0)),
                    ("ac-bias", CLIParameter::new_number("--", " ", 0.0)),
                ]
                .into_iter()
                .map(|(key, value)| (key.to_owned(), value))
                .collect();

                let mut sanitized_options = options.clone();
                for (key, value) in psychovisual_parameters {
                    if let Some((_key, unsanitized_value)) = sanitized_options.get_key_value(&key)
                        && unsanitized_value.matches(&value)
                    {
                        sanitized_options.remove(&key);
                    }
                }

                Encoder::SVTAV1 {
                    executable:   executable.clone(),
                    pass:         *pass,
                    options:      sanitized_options,
                    photon_noise: photon_noise.clone(),
                }
            },
            Encoder::X264 {
                ..
            } => encoder.clone(),
            Encoder::X265 {
                ..
            } => encoder.clone(),
            Encoder::VVenC {
                ..
            } => encoder.clone(),
            Encoder::FFmpeg {
                ..
            } => encoder.clone(),
        }
    }

    #[inline]
    #[allow(clippy::too_many_arguments)]
    pub fn probe_pass(
        pass: u8,
        target_quality_directory: &Path,
        config: &TargetQualityConfig,
        input: &mut Input,
        workers: u8,
        buffer_strategy: &BufferStrategy,
        concat_method: &ConcatMethod,
        tasks: &[Task],
        progress_tx: &sync::mpsc::Sender<SequenceStatus>,
        cancelled: &Arc<AtomicBool>,
    ) -> Result<((), Vec<Box<dyn Error>>)> {
        let warnings: Vec<Box<dyn Error>> = vec![];
        let pass_directory = target_quality_directory.join(pass.to_string());
        let output =
            concat_method.with_extension(&target_quality_directory.join(format!("pass-{}", pass)));
        let framerate = input.clip_info()?.frame_rate;

        let encode_tasks = tasks
            .iter()
            .filter(|task| !task.output.exists())
            .enumerate()
            .map(|(index, task)| ParallelEncoderTask {
                original_index: task.original_index,
                index,
                frame_indices: task.frame_indices.clone(),
                sub_scenes: None,
                encoder: task.encoder.clone(),
                output: task.output.clone(),
            })
            .collect::<Vec<_>>();

        let progress_tx_clone = progress_tx.clone();
        let (encode_progress_tx, encode_progress_rx) = sync::mpsc::channel();
        thread::spawn(move || -> Result<()> {
            for progress in encode_progress_rx {
                match progress {
                    SequenceStatus::Whole(Status::Processing {
                        id: _id,
                        completion,
                    }) => {
                        #[allow(clippy::collapsible_match)]
                        if let SequenceCompletion::Frames {
                            completed,
                            total,
                        } = completion
                        {
                            progress_tx_clone.send(SequenceStatus::Subprocess {
                                parent: Status::Processing {
                                    id:         DETAILS.name.to_string(),
                                    completion: SequenceCompletion::Passes {
                                        completed: workers,
                                        total:     workers,
                                    },
                                },
                                child:  Status::Processing {
                                    id:         workers.to_string(),
                                    completion: SequenceCompletion::Frames {
                                        completed,
                                        total,
                                    },
                                },
                            })?;
                        }
                    },
                    SequenceStatus::Whole(Status::Completed {
                        id: _,
                    }) => {
                        progress_tx_clone.send(SequenceStatus::Subprocess {
                            parent: Status::Processing {
                                id:         DETAILS.name.to_string(),
                                completion: SequenceCompletion::Passes {
                                    completed: workers,
                                    total:     workers,
                                },
                            },
                            child:  Status::Completed {
                                id: workers.to_string(),
                            },
                        })?;
                    },
                    _ => (),
                }
            }

            Ok(())
        });

        let results = ParallelEncoder::encode_tasks(
            input,
            workers,
            buffer_strategy,
            encode_tasks.iter().cloned().collect::<VecDeque<_>>(),
            encode_progress_tx,
            Arc::clone(cancelled),
        )?;

        let scene_paths = tasks.iter().map(|task| task.output.clone()).collect::<Vec<_>>();
        match concat_method {
            ConcatMethod::MKVMerge => {
                SceneConcatenator::mkvmerge(
                    &pass_directory,
                    &output,
                    &scene_paths,
                    None,
                    framerate,
                )?;
            },
            ConcatMethod::FFmpeg => {
                SceneConcatenator::ffmpeg(&pass_directory, &output, &scene_paths)?;
            },
            ConcatMethod::Ivf => SceneConcatenator::ivf(&output, &scene_paths)?,
        }

        match &config.metric {
            QualityMetric::VMAF {
                target_range,
                resolution,
                scaler,
                filter,
                threads,
                model,
                features,
            } => todo!(),
            QualityMetric::SSIMULACRA2 {
                target_range,
                resolution,
                threads,
            } => todo!(),
            QualityMetric::BUTTERAUGLI {
                target_range,
                resolution,
                threads,
                intensity_multiplier,
            } => todo!(),
            QualityMetric::XPSNR {
                target_range,
                resolution,
                threads,
            } => todo!(),
        }

        Ok(((), warnings))
    }
}

impl Default for TargetQuality {
    #[inline]
    fn default() -> Self {
        Self {
            input: None
        }
    }
}

#[derive(Debug, Clone)]
pub struct Task {
    pub original_index: usize,
    pub frame_indices:  Vec<usize>,
    pub encoder:        Encoder,
    pub output:         PathBuf,
    pub passes:         Vec<QualityPass>,
}

#[derive(Debug, Clone, Error)]
pub enum TargetQualityError {
    #[error("No Scenes found")]
    ScenesEmpty,
    #[error("Parallel Encoder workers already configured")]
    WorkersAlreadyConfigured,
    #[error("Failed to encode")]
    EncoderFailed,
    #[error("Previous Pass data not found")]
    PreviousPassDataNotFound,
}
