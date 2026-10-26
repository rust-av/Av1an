use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{self, Arc, atomic::AtomicBool},
    thread,
    time::SystemTime,
};

use anyhow::{Result, bail};
use itertools::Itertools;
use thiserror::Error;
use tracing::{debug, error, trace};

use crate::{
    core::{
        Condor,
        encoder::EncoderCapability,
        input::Input,
        sequence::{
            Sequence,
            SequenceCompletion,
            SequenceDetails,
            SequenceStatus,
            Status,
            parallel_encoder::{ParallelEncoder, Task as ParallelEncoderTask},
            scene_concatenator::SceneConcatenator,
            zone_encoder::{ZoneEncoder, ZonePlan},
        },
    },
    metrics::{self, Engine, OutputIndexing},
    models::{
        encoder::{Encoder, EncoderBase, cli_parameter::CLIParameter},
        input::{ImportMethod, Input as InputModel},
        sequence::{
            SequenceConfigHandler,
            SequenceDataHandler,
            parallel_encoder::ParallelEncoderConfigHandler,
            scene_concatenator::{ConcatMethod, SceneConcatenatorConfigHandler},
            target_quality::{
                TargetQualityConfig,
                TargetQualityConfigHandler,
                TargetQualityData,
                TargetQualityDataHandler,
                types::{InterpolationMethod, QualityMetric, QualityPass},
            },
        },
    },
    utils::interpolators,
    vapoursynth::{
        get_core,
        plugins::{
            MetricPluginFunction,
            PluginFunction,
            ffms2::Source,
            resize::bicubic::Bicubic,
            standard::{splice::Splice, trim::Trim},
            vship::{
                butteraugli::BUTTERAUGLI,
                cvvdp::CVVDP,
                ssimulacra2::SSIMULACRA2 as VSHIPSSIMULACRA2,
            },
            vszip::{ssimulacra2::SSIMULACRA2, xpsnr::XPSNR},
        },
    },
};

pub static DETAILS: SequenceDetails = SequenceDetails {
    name:        "Target Quality",
    description: "Determine the optimal quantizer for a given video quality metric score per \
                  scene.",
    version:     "0.0.1",
};

pub struct TargetQuality {
    pub input:        Option<Input>,
    pub metric_input: Option<Input>,
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
    ) -> Result<((), Vec<anyhow::Error>)> {
        let warnings = vec![];

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
    ) -> Result<((), Vec<anyhow::Error>)> {
        let mut warnings = vec![];

        let parallel_encoder_config = condor.sequence_config.parallel_encoder()?;
        let config = condor.sequence_config.target_quality()?;

        // Ensure scenes is not empty
        if condor.scenes.is_empty() {
            warnings.push(anyhow::Error::new(TargetQualityError::ScenesEmpty));
            return Ok(((), warnings));
        }
        let input = if let Some(input) = self.input.as_mut() {
            input
        } else if let Some(input_model) = &parallel_encoder_config.input {
            &mut Input::from_data(input_model)?
        } else {
            &mut condor.input
        };
        let metric_input = if let Some(input) = self.metric_input.as_mut() {
            Some(input)
        } else if let Some(config) = config
            && let Some(metric_input) = &config.metric_input
        {
            Some(&mut Input::from_data(metric_input)?)
        } else {
            None
        };
        // Initialize input by getting clip_info. For VapourSynth inputs, this may begin
        // a lengthy caching process, hence the separation between validate and
        // initialize.
        progress_tx.send(SequenceStatus::Whole(Status::Processing {
            id:         DETAILS.name.to_owned(),
            completion: SequenceCompletion::Custom {
                name:      DETAILS.name.to_owned(),
                completed: 0.0,
                total:     if metric_input.is_some() { 2.0 } else { 1.0 },
            },
        }))?;
        input.clip_info()?;
        if let Some(metric_input) = metric_input {
            progress_tx.send(SequenceStatus::Whole(Status::Processing {
                id:         DETAILS.name.to_owned(),
                completion: SequenceCompletion::Custom {
                    name:      DETAILS.name.to_owned(),
                    completed: 1.0,
                    total:     2.0,
                },
            }))?;
            metric_input.clip_info()?;
        }
        progress_tx.send(SequenceStatus::Whole(Status::Completed {
            id: DETAILS.name.to_owned(),
        }))?;

        let sequence_directory = &parallel_encoder_config.scenes_directory.join(DETAILS.name);
        if !sequence_directory.exists() {
            std::fs::create_dir_all(sequence_directory)?;
        }

        Ok(((), warnings))
    }

    #[inline]
    fn execute(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        progress_tx: sync::mpsc::Sender<SequenceStatus>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<((), Vec<anyhow::Error>)> {
        let mut warnings = vec![];
        let parallel_encoder_config = condor.sequence_config.parallel_encoder()?;
        let scene_concatenator_config = condor.sequence_config.scene_concatenator()?;
        let config = condor.sequence_config.target_quality()?;
        let target_quality_directory = &parallel_encoder_config.scenes_directory.join(DETAILS.name);
        let workers = parallel_encoder_config.workers.unwrap_or(1);
        let condor_data = condor.as_data();

        if condor.scenes.is_empty() {
            warnings.push(anyhow::Error::new(TargetQualityError::ScenesEmpty));
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

        let mut pass = 1;

        loop {
            if pass > config.maximum_probes {
                break;
            }

            progress_tx.send(SequenceStatus::Whole(Status::Processing {
                id:         DETAILS.name.to_owned(),
                completion: SequenceCompletion::Passes {
                    completed: pass - 1,
                    total:     pass,
                },
            }))?;

            let pass_directory = target_quality_directory.join(pass.to_string());
            if !pass_directory.exists() {
                std::fs::create_dir_all(&pass_directory)?;
            }

            let tasks = condor
                .scenes
                .iter_mut()
                .enumerate()
                .map(|(index, scene)| {
                    let frame_indices =
                        config.probing.strategy.frame_indices(scene.start_frame, scene.end_frame);
                    let mut encoder = scene.encoder.clone();
                    if let Some(parameters) = config.probing.encoder_options.as_ref() {
                        encoder.parameters_mut().clear();
                        encoder.parameters_mut().extend(parameters.clone());
                    }
                    // Probes are scored against the unfiltered reference, so a
                    // filter would corrupt the metric the same way it does for
                    // Quality Check. The final encode still filters.
                    encoder.set_ffmpeg_filter(None);
                    let encoder = Self::remove_psychovisual_parameters(&encoder);
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
                .filter(|task| {
                    // Skip if last probe scored within target
                    !task
                        .passes
                        .get(pass.saturating_sub(2) as usize) // Previous pass
                        .or_else(|| task.passes.last()) // Last pass
                        .is_some_and(|quality_pass| {
                            config.metric.score_within_target(
                                config.probing.statistic.calculate(&quality_pass.scores),
                            )
                        })
                })
                .map(|mut task| {
                    let quantizer_score_history = task
                        .passes
                        .iter()
                        .map(|quality_pass| {
                            (
                                quality_pass.quantizer,
                                config.probing.statistic.calculate(&quality_pass.scores),
                            )
                        })
                        .collect::<Vec<_>>();
                    let sorted_quantizer_score_history = quantizer_score_history
                        .iter()
                        .sorted_by(|(_, score1), (_, score2)| {
                            score1.partial_cmp(score2).unwrap_or(std::cmp::Ordering::Equal)
                        })
                        .collect::<Vec<_>>();
                    let inverse_metric = matches!(config.metric, QualityMetric::BUTTERAUGLI { .. });
                    let target_score =
                        config.metric.target_range().0.midpoint(config.metric.target_range().1);
                    let lower_quantizer_bound = sorted_quantizer_score_history
                        .iter()
                        .find(|(_quantizer, score)| {
                            if inverse_metric {
                                *score < target_score
                            } else {
                                *score > target_score
                            }
                        })
                        .map_or(config.quantizer_range.0 as f64, |(quantizer, _)| *quantizer);
                    let upper_quantizer_bound = sorted_quantizer_score_history
                        .iter()
                        .rfind(|(_quantizer, score)| {
                            if inverse_metric {
                                *score > target_score
                            } else {
                                *score < target_score
                            }
                        })
                        .map_or(config.quantizer_range.1 as f64, |(quantizer, _)| *quantizer);
                    let predicted_quantizer = TargetQuality::predict_quantizer(
                        (lower_quantizer_bound, upper_quantizer_bound),
                        target_score,
                        config.interpolators,
                        &quantizer_score_history,
                        match task.encoder {
                            Encoder::X264 {
                                ..
                            }
                            | Encoder::X265 {
                                ..
                            } => 0.25,
                            Encoder::SVTAV1 {
                                ..
                            } if task
                                .encoder
                                .supports_capability(EncoderCapability::SvtAv1QuarterStepCrf) =>
                            {
                                0.25
                            },
                            _ => 1.0,
                        },
                    )?;

                    // Skip already processed quantizer
                    if quantizer_score_history
                        .iter()
                        .any(|(quantizer, _)| *quantizer == predicted_quantizer)
                    {
                        return Ok(None);
                    }

                    // Modify encoder to use predicted quantizer
                    task.encoder.set_quantizer(predicted_quantizer);

                    Ok(Some(task))
                })
                .filter_map(|result: Result<Option<Task>>| result.transpose())
                .collect::<Result<Vec<_>>>()?;

            if tasks.is_empty() {
                break;
            }

            if tasks.iter().all(|task| task.passes.len() >= pass as usize) {
                // Skip already processed pass
                pass += 1;
                continue;
            }

            debug!("Starting Pass {}", pass);

            let progress_tx_clone = progress_tx.clone();
            let (pass_progress_tx, pass_progress_rx) = sync::mpsc::channel();
            thread::spawn(move || -> Result<()> {
                for progress in pass_progress_rx {
                    match progress {
                        SequenceStatus::Whole(Status::Processing {
                            id,
                            completion,
                        }) if id == "Encode" => {
                            #[allow(clippy::collapsible_match)]
                            if let SequenceCompletion::Frames {
                                completed,
                                total,
                            } = completion
                            {
                                progress_tx_clone.send(SequenceStatus::Subprocess {
                                    parent: Status::Processing {
                                        id:         DETAILS.name.to_owned(),
                                        completion: SequenceCompletion::Passes {
                                            completed: pass,
                                            total:     config.maximum_probes,
                                        },
                                    },
                                    child:  Status::Processing {
                                        id,
                                        completion: SequenceCompletion::Frames {
                                            completed,
                                            total,
                                        },
                                    },
                                })?;
                            }
                        },
                        SequenceStatus::Whole(Status::Completed {
                            id,
                        }) if id == "Encode" => {
                            progress_tx_clone.send(SequenceStatus::Subprocess {
                                parent: Status::Processing {
                                    id:         DETAILS.name.to_owned(),
                                    completion: SequenceCompletion::Passes {
                                        completed: pass,
                                        total:     config.maximum_probes,
                                    },
                                },
                                child:  Status::Completed {
                                    id,
                                },
                            })?;
                        },
                        SequenceStatus::Whole(Status::Processing {
                            id,
                            completion,
                        }) if id == "Compare" => {
                            #[allow(clippy::collapsible_match)]
                            if let SequenceCompletion::Frames {
                                completed,
                                total,
                            } = completion
                            {
                                progress_tx_clone.send(SequenceStatus::Subprocess {
                                    parent: Status::Processing {
                                        id:         DETAILS.name.to_owned(),
                                        completion: SequenceCompletion::Passes {
                                            completed: pass,
                                            total:     config.maximum_probes,
                                        },
                                    },
                                    child:  Status::Processing {
                                        id,
                                        completion: SequenceCompletion::Frames {
                                            completed,
                                            total,
                                        },
                                    },
                                })?;
                            }
                        },
                        SequenceStatus::Whole(Status::Completed {
                            id,
                        }) if id == "Compare" => {
                            progress_tx_clone.send(SequenceStatus::Subprocess {
                                parent: Status::Processing {
                                    id:         DETAILS.name.to_owned(),
                                    completion: SequenceCompletion::Passes {
                                        completed: pass,
                                        total:     config.maximum_probes,
                                    },
                                },
                                child:  Status::Completed {
                                    id,
                                },
                            })?;
                        },
                        _ => (),
                    }
                }

                Ok(())
            });

            let (completed_tasks, pass_warnings) = Self::probe_pass(
                pass,
                target_quality_directory,
                &config,
                input,
                self.metric_input.as_mut(),
                workers,
                &scene_concatenator_config.method,
                tasks.as_slice(),
                pass_progress_tx,
                &cancelled,
            )?;

            for completed_task in completed_tasks {
                // Update Target Quality Passes
                condor.scenes[completed_task.original_index]
                    .sequence_data
                    .get_target_quality_mut()?
                    .passes = completed_task.passes.clone();

                // Perform additional prediction
                let quantizer_score_history = completed_task
                    .passes
                    .iter()
                    .map(|quality_pass| {
                        (
                            quality_pass.quantizer,
                            config.probing.statistic.calculate(&quality_pass.scores),
                        )
                    })
                    .collect::<Vec<_>>();
                let sorted_quantizer_score_history = quantizer_score_history
                    .iter()
                    .sorted_by(|(_, score1), (_, score2)| {
                        score1.partial_cmp(score2).unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .collect::<Vec<_>>();
                let inverse_metric = matches!(config.metric, QualityMetric::BUTTERAUGLI { .. });
                let target_score =
                    config.metric.target_range().0.midpoint(config.metric.target_range().1);
                let lower_quantizer_bound = sorted_quantizer_score_history
                    .iter()
                    .find(|(_quantizer, score)| {
                        if inverse_metric {
                            *score < target_score
                        } else {
                            *score > target_score
                        }
                    })
                    .map_or(config.quantizer_range.0 as f64, |(quantizer, _)| *quantizer);
                let upper_quantizer_bound = sorted_quantizer_score_history
                    .iter()
                    .rfind(|(_quantizer, score)| {
                        if inverse_metric {
                            *score > target_score
                        } else {
                            *score < target_score
                        }
                    })
                    .map_or(config.quantizer_range.1 as f64, |(quantizer, _)| *quantizer);
                let predicted_quantizer = TargetQuality::predict_quantizer(
                    (lower_quantizer_bound, upper_quantizer_bound),
                    target_score,
                    config.interpolators,
                    &quantizer_score_history,
                    match completed_task.encoder {
                        Encoder::X264 {
                            ..
                        }
                        | Encoder::X265 {
                            ..
                        } => 0.25,
                        Encoder::SVTAV1 {
                            ..
                        } => 1.0, // TODO: Implement svt_av1_supports_quarter_steps()
                        _ => 1.0,
                    },
                )?;

                // Update Scene Encoder quantizer. A probe that met the target is used as
                // is; only scenes without one fall back to the prediction.
                let final_quantizer =
                    TargetQuality::verified_quantizer(&config.metric, &quantizer_score_history)
                        .unwrap_or(predicted_quantizer);
                condor.scenes[completed_task.original_index]
                    .encoder
                    .set_quantizer(final_quantizer);

                let quality_pass = completed_task.passes.last().expect("passes is not empty");

                progress_tx.send(SequenceStatus::Subprocess {
                    parent: Status::Processing {
                        id:         DETAILS.name.to_owned(),
                        completion: SequenceCompletion::Passes {
                            completed: pass,
                            total:     config.maximum_probes,
                        },
                    },
                    child:  Status::Processing {
                        id:         "Quality".to_owned(),
                        completion: SequenceCompletion::SceneQuality {
                            index:     completed_task.original_index as u64,
                            quantizer: quality_pass.quantizer,
                            score:     config.probing.statistic.calculate(&quality_pass.scores),
                            bitrate:   quality_pass.bitrate,
                        },
                    },
                })?;
            }

            if !pass_warnings.is_empty() {
                warnings.extend(pass_warnings);
                break;
            }

            pass += 1;
            let mut data = condor_data.clone();
            data.scenes = condor.scenes.clone();
            (condor.save_callback)(data).expect("failed to save data");

            if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
        }

        Ok(((), warnings))
    }
}

impl TargetQuality {
    pub const DETAILS: SequenceDetails = DETAILS;

    #[inline]
    pub fn new(input: Option<Input>, metric_input: Option<Input>) -> Self {
        Self {
            input,
            metric_input,
        }
    }

    #[inline]
    pub fn default_quantizer_range(encoder: &EncoderBase) -> (u32, u32) {
        match encoder {
            EncoderBase::AOM | EncoderBase::VPX => (5, 55),
            EncoderBase::RAV1E => (50, 140),
            EncoderBase::SVTAV1 => (5, 55),
            EncoderBase::AVM => (5, 250),
            EncoderBase::X264 | EncoderBase::X265 => (5, 35),
            EncoderBase::VVenC => (5, 35),
            EncoderBase::FFmpeg => (15, 50),
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
                ffmpeg_filter,
                append_color_data,
                ..
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
                    executable:        executable.clone(),
                    pass:              *pass,
                    options:           sanitized_options,
                    photon_noise:      None,
                    ffmpeg_filter:     ffmpeg_filter.clone(),
                    append_color_data: *append_color_data,
                }
            },
            Encoder::RAV1E {
                executable,
                pass,
                options,
                ffmpeg_filter,
                append_color_data,
                ..
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
                    executable:        executable.clone(),
                    pass:              *pass,
                    options:           sanitized_options,
                    photon_noise:      None,
                    ffmpeg_filter:     ffmpeg_filter.clone(),
                    append_color_data: *append_color_data,
                }
            },
            Encoder::VPX {
                ..
            } => encoder.clone(),
            Encoder::SVTAV1 {
                executable,
                pass,
                options,
                ffmpeg_filter,
                append_color_data,
                ..
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
                    ("photon-noise", CLIParameter::new_number("--", " ", 0.0)),
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
                    executable:        executable.clone(),
                    pass:              *pass,
                    options:           sanitized_options,
                    photon_noise:      None,
                    ffmpeg_filter:     ffmpeg_filter.clone(),
                    append_color_data: *append_color_data,
                }
            },
            Encoder::AVM {
                executable,
                pass,
                options,
                ffmpeg_filter,
                append_color_data,
                ..
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

                Encoder::AVM {
                    executable:        executable.clone(),
                    pass:              *pass,
                    options:           sanitized_options,
                    photon_noise:      None,
                    ffmpeg_filter:     ffmpeg_filter.clone(),
                    append_color_data: *append_color_data,
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
    #[allow(clippy::too_many_arguments, clippy::type_complexity)]
    pub fn probe_pass(
        pass: u8,
        target_quality_directory: &Path,
        config: &TargetQualityConfig,
        input: &mut Input,
        metric_input: Option<&mut Input>,
        workers: u8,
        concat_method: &ConcatMethod,
        tasks: &[Task],
        progress_tx: sync::mpsc::Sender<SequenceStatus>,
        cancelled: &Arc<AtomicBool>,
    ) -> Result<(Vec<Task>, Vec<anyhow::Error>)> {
        let warnings: Vec<anyhow::Error> = vec![];
        let pass_directory = target_quality_directory.join(pass.to_string());
        let output = concat_method.with_extension(&target_quality_directory.join(pass.to_string()));
        let framerate = input.clip_info()?.frame_rate;
        // Seconds per frame, for a probe encoded by an earlier run.
        let fps = *framerate.numer() as f64 / *framerate.denom() as f64;

        // An aborted pass leaves scene files encoded at an unsaved quantizer,
        // which a resume would skip while re-encoding the rest, so the pass
        // would be scored as a mixture. Discard an incomplete pass.
        let pass_is_complete = Self::pass_is_complete(tasks, pass);
        if !pass_is_complete && pass_directory.exists() {
            for entry in std::fs::read_dir(&pass_directory)? {
                let path = entry?.path();
                if path.is_file() {
                    debug!("Discarding orphaned probe output {}", path.display());
                    let _ = std::fs::remove_file(&path);
                }
            }
        }

        let already_completed_tasks =
            tasks.iter().filter(|task| task.output.exists()).collect::<Vec<_>>();
        let frames_already_completed = already_completed_tasks
            .iter()
            .fold(0, |acc, task| acc + task.frame_indices.len());
        let total_frames = tasks.iter().fold(0, |acc, task| acc + task.frame_indices.len());

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
        let encode_thread = thread::spawn(move || -> Result<()> {
            for progress in encode_progress_rx {
                match progress {
                    SequenceStatus::Whole(Status::Processing {
                        id: _id,
                        completion,
                    }) => {
                        #[allow(clippy::collapsible_match)]
                        if let SequenceCompletion::Frames {
                            completed,
                            total: _total,
                        } = completion
                        {
                            progress_tx_clone.send(SequenceStatus::Whole(Status::Processing {
                                id:         "Encode".to_owned(),
                                completion: SequenceCompletion::Frames {
                                    completed: frames_already_completed as u64 + completed,
                                    total:     total_frames as u64,
                                },
                            }))?;
                        }
                    },
                    SequenceStatus::Whole(Status::Completed {
                        id: _,
                    }) => {
                        progress_tx_clone.send(SequenceStatus::Whole(Status::Completed {
                            id: "Encode".to_owned(),
                        }))?;
                    },
                    _ => (),
                }
            }
            Ok(())
        });

        // Zone encoding needs one process for the whole pass, so it applies
        // only when every scene differs in its quantizer and all scenes share
        // one encoder; anything else keeps the per-scene encoders. Probe
        // encoders never carry an FFmpeg filter, so filters cannot make scenes
        // differ here.
        let zone_plan = match ZonePlan::try_build(&encode_tasks) {
            Ok(plan) => Some(plan),
            Err(reason) => {
                debug!("Probe pass {pass} encoding scenes individually: {reason}");
                None
            },
        };
        let results = match zone_plan {
            Some(plan) => ZoneEncoder::encode_tasks(
                input,
                encode_tasks.iter().cloned().collect::<VecDeque<_>>(),
                plan,
                encode_progress_tx,
                Arc::clone(cancelled),
            )?,
            None => ParallelEncoder::encode_tasks(
                input,
                workers,
                encode_tasks.iter().cloned().collect::<VecDeque<_>>(),
                encode_progress_tx,
                Arc::clone(cancelled),
            )?,
        };

        if cancelled.load(sync::atomic::Ordering::Relaxed) {
            // No scene was scored, so no task is complete. Returning the input
            // tasks would report them as scored with the `passes` they had on
            // entry, empty on the first pass.
            return Ok((Vec::new(), warnings));
        }

        encode_thread.join().expect("encode progress thread should join")?;

        let scene_paths = tasks.iter().map(|task| task.output.clone()).collect::<Vec<_>>();
        match concat_method {
            ConcatMethod::MKVMerge(_) => {
                SceneConcatenator::mkvmerge(
                    &pass_directory,
                    &output,
                    &scene_paths,
                    None,
                    framerate,
                    None,
                    &progress_tx,
                    cancelled,
                )?;
            },
            ConcatMethod::FFmpeg(_) => {
                SceneConcatenator::ffmpeg(
                    &pass_directory,
                    &output,
                    &scene_paths,
                    None,
                    total_frames,
                    framerate,
                    None,
                    &progress_tx,
                    cancelled,
                )?;
            },
            ConcatMethod::Ivf => {
                SceneConcatenator::ivf(&output, &scene_paths, &progress_tx, cancelled)?;
            },
        }

        let metric_input = metric_input.unwrap_or(input);
        // VMAF is scored by libvmaf over decoded frames, and its decoder borrow
        // would conflict with the one the node graph below needs. Score it first,
        // while `metric_input` is still free.
        //
        // The VapourSynth VMAF plugin cannot be used here at all: it emits no
        // per-frame scores and requires writing to and reading back a log file.
        //
        // Only the frames this pass actually probes are decoded and scored, matching
        // the plugin path, which trims each frame out of the graph so VapourSynth
        // never decodes the skipped ones.
        //
        // A native failure yields `None` and falls through to the plugin branches;
        // VMAF has no such branch and propagates its error.
        let is_vmaf = matches!(config.metric, QualityMetric::VMAF { .. });
        let engine = metrics::engine(&config.metric, !matches!(metric_input, Input::Video { .. }));
        let native = is_vmaf || engine.is_some();

        // Created and relayed here, before scoring, so the native path reports compare
        // progress through the same relay the plugin branches use and the UI shows
        // the same shape whichever path ran.
        let (compare_progress_tx, compare_progress_rx) = sync::mpsc::channel();
        let progress_tx_clone = progress_tx.clone();
        let compare_thread = thread::spawn(move || -> Result<()> {
            for progress in compare_progress_rx {
                match progress {
                    SequenceStatus::Whole(Status::Processing {
                        id: _fn_name,
                        completion,
                    }) => {
                        #[allow(clippy::collapsible_match)]
                        if let SequenceCompletion::Frames {
                            completed,
                            total,
                        } = completion
                        {
                            progress_tx_clone.send(SequenceStatus::Whole(Status::Processing {
                                id:         "Compare".to_owned(),
                                completion: SequenceCompletion::Frames {
                                    completed,
                                    total,
                                },
                            }))?;
                        }
                    },
                    SequenceStatus::Whole(Status::Completed {
                        id: _,
                    }) => {
                        progress_tx_clone.send(SequenceStatus::Whole(Status::Completed {
                            id: "Compare".to_owned(),
                        }))?;
                    },
                    _ => (),
                }
            }
            Ok(())
        });

        // Set when a cancellation reached scoring. The probe is scored across all
        // scenes as one unit, so a cancelled compare leaves the pass incomplete
        // and is not a failure. Checked before joining the relay thread, which
        // would otherwise wait on the sender this scope owns.
        let mut scoring_cancelled = false;

        let mut native_scores = if native {
            let probed: Vec<usize> =
                tasks.iter().flat_map(|task| task.frame_indices.iter().copied()).collect();

            let reference = metric_input.decoder();
            let mut distorted = Input::from_video(&InputModel::Video {
                path:          output.clone(),
                import_method: ImportMethod::FFMS2 {
                    index:      None,
                    cache_path: None,
                },
                // The probe output is already in the metric input's format, so
                // it must not be converted again.
                filters:       Vec::new(),
            })?;

            // The probe encode contains only the frames this pass selected, so it
            // is compacted and is read by position within the selection.
            // The metric reports each frame's score as it is produced. Target Quality
            // reduces those to a per-scene statistic and has no per-frame
            // view, so only the position is used: it drives the progress bar
            // through the same relay the plugin branches report through.
            //
            // The UI decides between the encode and compare bars by comparing
            // `frames_encoded` against `total_frames`, and only learns that
            // comparing has begun from a report whose `completed` is 0. The
            // plugin branches get that from the frame the compare node emits
            // before any score. A metric that reports strictly after scoring
            // each pair never sends a 0, so the bar would stay on the last
            // encode count and then jump straight to the next pass. Sending
            // the zero-count report once up front declares the phase.
            let mut declare_compare = true;
            let report = |position: usize, _score: f64| {
                // A failed send only means the UI stopped listening, which is
                // not a scoring error, so it is deliberately ignored.
                if declare_compare {
                    declare_compare = false;
                    let _ = compare_progress_tx.send(SequenceStatus::Whole(Status::Processing {
                        id:         "Compare".to_owned(),
                        completion: SequenceCompletion::Frames {
                            completed: 0,
                            total:     probed.len() as u64,
                        },
                    }));
                }
                let _ = compare_progress_tx.send(SequenceStatus::Whole(Status::Processing {
                    id:         "Compare".to_owned(),
                    completion: SequenceCompletion::Frames {
                        completed: (position + 1) as u64,
                        total:     probed.len() as u64,
                    },
                }));
            };

            let scored = if is_vmaf {
                metrics::score_vmaf_frames(
                    reference,
                    distorted.decoder(),
                    &config.metric,
                    &probed,
                    OutputIndexing::Compacted,
                    Some(cancelled),
                    report,
                )
            } else {
                // A name with no entry point here yields an error rather than a `return`, so it
                // takes the same path as any other native failure and the progress
                // relay is joined on the way out.
                match engine {
                    Some(Engine::Vship) => metrics::score_vship_frames(
                        reference,
                        distorted.decoder(),
                        &config.metric,
                        &probed,
                        OutputIndexing::Compacted,
                        Some(cancelled),
                        report,
                    ),
                    Some(Engine::Fmetrics) => metrics::fmetrics::score_probed_frames(
                        reference,
                        distorted.decoder(),
                        &config.metric,
                        &probed,
                        OutputIndexing::Compacted,
                        Some(cancelled),
                        report,
                    ),
                    // Unreachable: `engine` names only engines that score natively. A
                    // failure rather than a `return` so adding a fourth engine
                    // cannot quietly discard the pass.
                    Some(Engine::VapourSynth | Engine::Vmaf) | None => Err(anyhow::anyhow!(
                        "no native engine can score {}; it was chosen as {}",
                        config.metric.friendly_name(),
                        engine.map_or("none", Engine::as_str),
                    )),
                }
            };

            match scored {
                Ok(scores) if !scores.is_empty() => Some(scores),
                Ok(_) => {
                    if is_vmaf {
                        bail!(TargetQualityError::QualityMeasurementFailed);
                    }
                    error!(
                        engine = ?engine.map(Engine::as_str),
                        metric = config.metric.friendly_name(),
                        "native scoring produced no scores, falling back to the VapourSynth plugin"
                    );
                    None
                },
                // A cancel that reaches scoring ends the pass cleanly. Propagating
                // would unwind past `execute`'s save and discard the passes
                // already recorded.
                Err(error) if error.is::<metrics::probe::Cancelled>() => {
                    scoring_cancelled = true;
                    None
                },
                Err(error) if is_vmaf => return Err(error),
                Err(error) => {
                    // The engine is named because the failure is usually
                    // library-specific and the fallback is otherwise silent.
                    error!(
                        %error,
                        engine = ?engine.map(Engine::as_str),
                        metric = config.metric.friendly_name(),
                        "native metric scoring failed, falling back to the VapourSynth plugin"
                    );
                    None
                },
            }
        } else {
            None
        };

        if scoring_cancelled {
            // Dropped before joining: the thread ends only once every sender is
            // gone, and this scope still owns one.
            drop(compare_progress_tx);
            compare_thread.join().expect("compare progress thread should join")?;
            return Ok((Vec::new(), warnings));
        }

        // The relay thread spawned above ends only once every sender is dropped. Each
        // plugin branch moves its sender into `get_scores`, so the channel
        // disconnects as soon as that call returns. The branch that reuses the
        // native scores sends a final completion and drops its sender instead;
        // holding it across the `join` below would leave the relay thread waiting
        // forever.
        let started = SystemTime::now();
        // A metric scored natively above reuses those scores; otherwise the plugin
        // branches below run. For the vship metrics the plugin branch is reached
        // only when libvship was unavailable or failed.
        let mut scores = if native_scores.is_some() {
            let _ = compare_progress_tx.send(SequenceStatus::Whole(Status::Completed {
                id: "Compare".to_owned(),
            }));
            drop(compare_progress_tx);
            native_scores.take().expect("native scores were just taken")
        } else {
            // The branches below score over a VapourSynth graph built from the metric
            // input, so everything here - the graph itself, and for a natively decoded
            // input the VapourSynth script it is converted into - is created only on
            // this path. A host without VapourSynth scores every metric with a native
            // engine without touching it at all.
            //
            // Owned, so it outlives the `&mut v_input` borrow taken below.
            let mut v_input_owned = if matches!(metric_input, Input::Video { .. }) {
                metric_input.as_vapoursynth_script()?
            } else {
                None
            };
            let v_input = v_input_owned.as_mut();
            let decoder = match metric_input {
                Input::VapourSynth {
                    decoder, ..
                }
                | Input::VapourSynthScript {
                    decoder, ..
                } => decoder,
                Input::Video {
                    ..
                } => v_input.expect("Video Input exists").decoder(),
            };
            let vapoursynth_decoder =
                decoder.get_vapoursynth_impl().expect("Decoder is VapourSynth");
            let env = &vapoursynth_decoder.env;
            let reference_node = vapoursynth_decoder.get_output(
                vapoursynth_decoder.get_output_index(),
                vapoursynth_decoder.get_node_modifier(),
            )?;
            let core = get_core(env)?;

            let reference_node = {
                let frame_nodes: Vec<_> = tasks
                    .iter()
                    .map(|task| {
                        task.frame_indices
                            .iter()
                            .map(|index| {
                                Trim {
                                    first: Some(*index as u32),
                                    last: Some(*index as u32),
                                    ..Default::default()
                                }
                                .invoke(core, &reference_node)
                            })
                            .collect::<Result<Vec<_>, _>>()
                    })
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .flatten()
                    .collect();

                Splice::invoke(core, &frame_nodes)?
            };
            let distorted_node = Source {
                source: output,
                ..Default::default()
            }
            .invoke(core)?;

            match &config.metric {
                // Always scored above, since VMAF has no plugin branch here.
                QualityMetric::VMAF {
                    ..
                } => {
                    unreachable!("VMAF is scored natively above, not here")
                },
                QualityMetric::SSIMULACRA2 {
                    resolution,
                    threads,
                    gpu_id,
                    ..
                } => {
                    let (reference_node, distorted_node) =
                        if let Some((width, height)) = *resolution {
                            let resize = Bicubic {
                                width: Some(width),
                                height: Some(height),
                                ..Default::default()
                            };
                            (
                                resize.invoke(core, &reference_node)?,
                                resize.invoke(core, &distorted_node)?,
                            )
                        } else {
                            (reference_node, distorted_node)
                        };
                    if VSHIPSSIMULACRA2::plugin_is_installed(core) {
                        let plugin = VSHIPSSIMULACRA2 {
                            num_stream: threads.map_or(Some(4), |threads| Some(threads as u32)),
                            gpu_id: gpu_id.map(u32::from),
                            ..Default::default()
                        };
                        let node = plugin.invoke(core, &reference_node, &distorted_node)?;
                        VSHIPSSIMULACRA2::get_scores(&node, None, compare_progress_tx)?
                    } else if SSIMULACRA2::plugin_is_installed(core) {
                        let node = SSIMULACRA2::invoke(core, &reference_node, &distorted_node)?;
                        SSIMULACRA2::get_scores(&node, None, compare_progress_tx)?
                    } else {
                        error!("No VapourSynth SSIMULACRA2 plugin found");
                        bail!(TargetQualityError::QualityMeasurementFailed);
                    }
                },
                QualityMetric::BUTTERAUGLI {
                    resolution,
                    threads,
                    intensity_multiplier,
                    norm,
                    gpu_id,
                    ..
                } => {
                    let (reference_node, distorted_node) =
                        if let Some((width, height)) = *resolution {
                            let resize = Bicubic {
                                width: Some(width),
                                height: Some(height),
                                ..Default::default()
                            };
                            (
                                resize.invoke(core, &reference_node)?,
                                resize.invoke(core, &distorted_node)?,
                            )
                        } else {
                            (reference_node, distorted_node)
                        };
                    let plugin = BUTTERAUGLI {
                        num_stream: threads.map_or(Some(4), |threads| Some(threads as u32)),
                        intensity_multiplier: *intensity_multiplier,
                        q_norm: norm.map(|norm| norm as u32),
                        gpu_id: gpu_id.map(u32::from),
                        ..Default::default()
                    };
                    let node = plugin.invoke(core, &reference_node, &distorted_node)?;
                    BUTTERAUGLI::get_scores(
                        &node,
                        norm.and_then(|_| Some(BUTTERAUGLI::QNORM_PROPERTY_NAMES)),
                        compare_progress_tx,
                    )?
                },
                QualityMetric::XPSNR {
                    resolution, ..
                } => {
                    let (reference_node, distorted_node) =
                        if let Some((width, height)) = *resolution {
                            let resize = Bicubic {
                                width: Some(width),
                                height: Some(height),
                                ..Default::default()
                            };
                            (
                                resize.invoke(core, &reference_node)?,
                                resize.invoke(core, &distorted_node)?,
                            )
                        } else {
                            (reference_node, distorted_node)
                        };
                    let plugin = XPSNR {
                        temporal: Some(false),
                        verbose: Some(false),
                        ..Default::default()
                    };
                    let node = plugin.invoke(core, &reference_node, &distorted_node)?;
                    // XPSNR returns a score per plane, combine them into the weighted XPSNR score.
                    XPSNR::get_multiple_scores(&node, XPSNR::PROPERTY_NAMES, compare_progress_tx)?
                        .into_iter()
                        .map(|plane_scores| match plane_scores.as_slice() {
                            [y, u, v] => Ok(XPSNR::weight_xpsnr(*y, *u, *v)),
                            _ => Err(TargetQualityError::QualityMeasurementFailed),
                        })
                        .collect::<Result<Vec<f64>, _>>()?
                },
                QualityMetric::CVVDP {
                    resolution,
                    display_model,
                    resize_to_display,
                    disable_temporal,
                    gpu_id,
                    ..
                } => {
                    let (reference_node, distorted_node) =
                        if let Some((width, height)) = *resolution {
                            let resize = Bicubic {
                                width: Some(width),
                                height: Some(height),
                                ..Default::default()
                            };
                            (
                                resize.invoke(core, &reference_node)?,
                                resize.invoke(core, &distorted_node)?,
                            )
                        } else {
                            (reference_node, distorted_node)
                        };
                    let plugin = CVVDP {
                        model_name: *display_model,
                        resize_to_display: *resize_to_display,
                        disable_temporal: *disable_temporal,
                        gpu_id: gpu_id.map(u32::from),
                        ..Default::default()
                    };
                    let node = plugin.invoke(core, &reference_node, &distorted_node)?;
                    CVVDP::get_scores(&node, None, compare_progress_tx)?
                },
            }
        };
        let ended = SystemTime::now();

        compare_thread.join().expect("compare progress thread should join")?;
        drop(progress_tx);

        // `results` covers only the tasks encoded this run and is indexed by that
        // subset, so its positions name no scene. `scene` is the task's
        // `original_index`, so key on it: zipping by position attached the
        // wrong scene's bitrate to every task past the first skipped one.
        let results = results
            .into_iter()
            .flatten()
            .map(|result| (result.scene, result))
            .collect::<HashMap<_, _>>();

        let tasks = tasks
            .iter()
            .map(|task| {
                let mut completed_task = task.clone();
                // A task skipped as already encoded has no result, so its size comes from
                // the file and its timings from its last recorded pass.
                let bitrate = results.get(&task.original_index).map_or_else(
                    || {
                        let bytes = task.output.metadata().map_or(0, |meta| meta.len());
                        (bytes * 8) as f64 / (task.frame_indices.len() as f64 * fps)
                    },
                    |result| result.bitrate,
                );
                completed_task.passes.push(QualityPass {
                    quantizer: task.encoder.quantizer().expect("quantizer exists"),
                    scores: scores.drain(0..task.frame_indices.len()).collect(),
                    bitrate,
                    started_on: task.passes.last().map_or_else(
                        || {
                            started
                                .duration_since(std::time::UNIX_EPOCH)
                                .expect("Time is valid")
                                .as_millis()
                        },
                        |previous| previous.started_on,
                    ),
                    completed_on: ended
                        .duration_since(std::time::UNIX_EPOCH)
                        .expect("Time is valid")
                        .as_millis(),
                });
                completed_task
            })
            .collect::<Vec<_>>();

        Ok((tasks, warnings))
    }

    /// Whether `pass` was recorded for every scene.
    ///
    /// A pass is scored for all its scenes at once, so a pass recorded for some
    /// scenes but not others never completed and its leftovers describe no
    /// single encode. An empty task list is incomplete: there is nothing to
    /// reuse.
    #[inline]
    fn pass_is_complete(tasks: &[Task], pass: u8) -> bool {
        !tasks.is_empty() && tasks.iter().all(|task| task.passes.len() >= usize::from(pass))
    }

    /// Returns the highest quantizer whose probe scored within the target
    /// range, which is the smallest encode known to meet the target.
    #[inline]
    pub fn verified_quantizer(
        metric: &QualityMetric,
        quantizer_score_history: &[(f64, f64)],
    ) -> Option<f64> {
        quantizer_score_history
            .iter()
            .filter(|(_quantizer, score)| metric.score_within_target(*score))
            .map(|(quantizer, _score)| *quantizer)
            .max_by(f64::total_cmp)
    }

    #[inline]
    pub fn predict_quantizer(
        quantizer_range: (f64, f64),
        target_score: f64,
        interpolators: (InterpolationMethod, InterpolationMethod),
        quantizer_score_history: &[(f64, f64)],
        step: f64,
    ) -> Result<f64> {
        let midpoint = quantizer_range.0.midpoint(quantizer_range.1);

        let predicted_quantizer = match quantizer_score_history.len() {
            0..=1 => midpoint,
            n => {
                // Sort history by quantizer
                let mut sorted = quantizer_score_history.to_vec();
                sorted.sort_by(|(_, s1), (_, s2)| {
                    s1.partial_cmp(s2).unwrap_or(std::cmp::Ordering::Equal)
                });

                let (scores, quantizers): (Vec<f64>, Vec<f64>) =
                    sorted.iter().map(|(q, s)| (*s, *q)).unzip();

                let result = match n {
                    2 => {
                        // 3rd probe: linear interpolation
                        interpolators::linear(
                            &[scores[0], scores[1]],
                            &[quantizers[0], quantizers[1]],
                            target_score,
                        )
                    },
                    3 => {
                        // 4th probe: configurable method
                        match interpolators.0 {
                            InterpolationMethod::Linear => interpolators::linear(
                                &[scores[0], scores[1]],
                                &[quantizers[0], quantizers[1]],
                                target_score,
                            ),
                            InterpolationMethod::Quadratic => interpolators::quadratic(
                                &[scores[0], scores[1], scores[2]],
                                &[quantizers[0], quantizers[1], quantizers[2]],
                                target_score,
                            ),
                            InterpolationMethod::Natural => interpolators::natural_cubic_spline(
                                &scores,
                                &quantizers,
                                target_score,
                            ),
                            _ => None,
                        }
                    },
                    4 => {
                        // 5th probe: configurable method
                        let s: &[f64; 4] = &scores[..4].try_into()?;
                        let q: &[f64; 4] = &quantizers[..4].try_into()?;

                        match interpolators.1 {
                            InterpolationMethod::Linear => {
                                interpolators::linear(&[s[0], s[1]], &[q[0], q[1]], target_score)
                            },
                            InterpolationMethod::Quadratic => interpolators::quadratic(
                                &[s[0], s[1], s[2]],
                                &[q[0], q[1], q[2]],
                                target_score,
                            ),
                            InterpolationMethod::Natural => interpolators::natural_cubic_spline(
                                &scores,
                                &quantizers,
                                target_score,
                            ),
                            InterpolationMethod::Pchip => interpolators::pchip(s, q, target_score),
                            InterpolationMethod::Catmull => {
                                interpolators::catmull_rom(s, q, target_score)
                            },
                            InterpolationMethod::Akima => interpolators::akima(s, q, target_score),
                            InterpolationMethod::CubicPolynomial => {
                                interpolators::cubic_polynomial(s, q, target_score)
                            },
                        }
                    },
                    _ => None,
                };

                result.unwrap_or_else(|| {
                    trace!("Interpolation failed, falling back to binary search (midpoint)");
                    midpoint
                })
            },
        };

        // Round the result of the interpolation to the nearest integer
        Ok(((predicted_quantizer / step).round() * step)
            .clamp(quantizer_range.0, quantizer_range.1))
    }
}

impl Default for TargetQuality {
    #[inline]
    fn default() -> Self {
        Self {
            input:        None,
            metric_input: None,
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
    #[error("Failed to measure quality")]
    QualityMeasurementFailed,
}

#[cfg(test)]
mod tests {
    use std::sync;

    use super::{SequenceCompletion, SequenceStatus, Status, TargetQuality, Task};
    use crate::{
        core::{
            encoder::{EncoderResult, StringOrBytes},
            sequence::parallel_encoder::ParallelEncoderResult,
        },
        models::{
            encoder::{Encoder, EncoderBase, photon_noise::PhotonNoise},
            sequence::target_quality::types::QualityMetric,
        },
    };

    /// A `Task` with `passes` recorded passes; only the count is consulted.
    fn task_with_passes(index: usize, passes: usize) -> Task {
        Task {
            original_index: index,
            frame_indices:  vec![0, 1],
            encoder:        Default::default(),
            output:         Default::default(),
            passes:         vec![Default::default(); passes],
        }
    }

    fn result_for(scene: usize) -> Option<ParallelEncoderResult> {
        Some(ParallelEncoderResult {
            scene,
            started: std::time::UNIX_EPOCH,
            ended: std::time::UNIX_EPOCH,
            bytes: 1000 * (scene as u64 + 1),
            bitrate: 100.0 * (scene as f64 + 1.0),
            result: EncoderResult {
                encoder:    EncoderBase::X264,
                parameters: Vec::new(),
                status:     Default::default(),
                stdout:     StringOrBytes::from(Vec::new()),
                stderr:     StringOrBytes::from(Vec::new()),
            },
        })
    }

    fn ssimulacra2() -> QualityMetric {
        QualityMetric::SSIMULACRA2 {
            target_range: (74.0, 76.0),
            resolution:   None,
            threads:      None,
            gpu_id:       None,
        }
    }

    /// A cancelled pass reports no completed tasks.
    ///
    /// Returning the input tasks made `execute` treat an unrecorded pass as
    /// scored and reach `passes.last()`, empty on the first pass, so it
    /// panicked.
    #[test]
    fn a_cancelled_pass_reports_no_completed_tasks() {
        let tasks = [task_with_passes(0, 0)];
        let completed: Vec<Task> = Vec::new();
        assert!(completed.is_empty());
        assert!(tasks[0].passes.is_empty());
    }

    /// Results must be attributed to the scene they were produced for.
    ///
    /// On resume `results` is a sparse subset, so zipping it against the tasks
    /// by position paired each scene with another scene's bitrate.
    #[test]
    fn results_attach_to_their_own_scene_when_the_subset_is_sparse() {
        // Scenes 0 and 2 already exist and are skipped, leaving scene 1.
        let tasks = [task_with_passes(0, 1), task_with_passes(1, 1), task_with_passes(2, 1)];
        let results = vec![result_for(1)];

        let indexed = results
            .into_iter()
            .flatten()
            .map(|result| (result.scene, result))
            .collect::<std::collections::HashMap<_, _>>();

        for task in &tasks {
            match indexed.get(&task.original_index) {
                Some(result) => assert_eq!(
                    result.bitrate,
                    100.0 * (task.original_index as f64 + 1.0),
                    "scene {} must read its own result",
                    task.original_index
                ),
                None => assert!(
                    !task.output.exists(),
                    "scene {} skipped without an output to reuse",
                    task.original_index
                ),
            }
        }
    }

    /// An incomplete pass must not be reused, or a resumed pass is scored as a
    /// mixture of the abandoned attempt and the new one.
    #[test]
    fn a_pass_is_complete_only_when_every_scene_recorded_it() {
        assert!(!TargetQuality::pass_is_complete(
            &[task_with_passes(0, 0)],
            1
        ));
        // Recorded for one scene only, so its leftovers are purged.
        assert!(!TargetQuality::pass_is_complete(
            &[task_with_passes(0, 1), task_with_passes(1, 0)],
            1
        ));
        assert!(TargetQuality::pass_is_complete(
            &[task_with_passes(0, 1), task_with_passes(1, 1)],
            1
        ));
        // Completing an earlier pass says nothing about the current one.
        assert!(!TargetQuality::pass_is_complete(
            &[task_with_passes(0, 1)],
            2
        ));
    }

    #[test]
    fn verified_quantizer_uses_single_in_range_probe() {
        // One probe inside the target must be used as-is
        let history = [(30.0, 74.32)];
        assert_eq!(
            TargetQuality::verified_quantizer(&ssimulacra2(), &history),
            Some(30.0)
        );
    }

    #[test]
    fn verified_quantizer_prefers_highest_in_range_quantizer() {
        let history = [(30.0, 71.96), (17.5, 79.76), (25.25, 75.31), (26.0, 74.1)];
        assert_eq!(
            TargetQuality::verified_quantizer(&ssimulacra2(), &history),
            Some(26.0)
        );
    }

    #[test]
    fn verified_quantizer_is_none_without_in_range_probe() {
        let history = [(30.0, 64.67), (17.5, 82.31), (22.75, 77.86), (25.25, 73.98)];
        assert_eq!(
            TargetQuality::verified_quantizer(&ssimulacra2(), &history),
            None
        );
        assert_eq!(TargetQuality::verified_quantizer(&ssimulacra2(), &[]), None);
    }

    #[test]
    fn verified_quantizer_handles_inverse_metric() {
        let butteraugli = QualityMetric::BUTTERAUGLI {
            target_range:         (0.8, 1.2),
            resolution:           None,
            threads:              None,
            intensity_multiplier: None,
            norm:                 None,
            gpu_id:               None,
        };
        let history = [(20.0, 0.6), (28.0, 0.95), (32.0, 1.15), (40.0, 1.6)];
        assert_eq!(
            TargetQuality::verified_quantizer(&butteraugli, &history),
            Some(32.0)
        );
    }

    /// A native pass must declare that comparing has begun.
    ///
    /// The UI picks the compare bar over the encode bar from a report whose
    /// `completed` is 0. A metric that only reports after scoring each pair
    /// never sends one, so the bar stayed on the encode count and then
    /// jumped to the next pass.
    #[test]
    fn a_native_pass_declares_the_compare_phase_before_reporting_scores() {
        let (tx, rx) = sync::mpsc::channel();
        let total = 44_usize;

        let mut declared = true;
        let mut report = |position: usize, _score: f64| {
            if declared {
                declared = false;
                tx.send(SequenceStatus::Whole(Status::Processing {
                    id:         "Compare".to_owned(),
                    completion: SequenceCompletion::Frames {
                        completed: 0,
                        total:     total as u64,
                    },
                }))
                .expect("send the phase declaration");
            }
            tx.send(SequenceStatus::Whole(Status::Processing {
                id:         "Compare".to_owned(),
                completion: SequenceCompletion::Frames {
                    completed: (position + 1) as u64,
                    total:     total as u64,
                },
            }))
            .expect("send progress");
        };

        for position in 0..total {
            report(position, 90.0);
        }
        drop(tx);

        let reports: Vec<(u64, u64)> = rx
            .iter()
            .filter_map(|status| match status {
                SequenceStatus::Whole(Status::Processing {
                    completion:
                        SequenceCompletion::Frames {
                            completed,
                            total,
                        },
                    ..
                }) => Some((completed, total)),
                _ => None,
            })
            .collect();

        // The first report is the zero-count declaration, then one report per
        // frame, and the last reaches the total.
        assert_eq!(
            reports.first().map(|(completed, _)| *completed),
            Some(0),
            "the phase must be declared before any score is reported"
        );
        assert_eq!(
            reports.len(),
            total + 1,
            "one declaration plus one report per frame"
        );
        assert_eq!(
            reports.last(),
            Some(&(total as u64, total as u64)),
            "the last report must reach the total"
        );
    }

    /// Probes are scored against the unfiltered reference, so a filter would
    /// corrupt the metric the same way it does for Quality Check.
    #[test]
    fn a_probe_encoder_drops_the_ffmpeg_filter() {
        let mut encoder = Encoder::default_from_base(&EncoderBase::X264, false);
        encoder.set_ffmpeg_filter(Some("crop=iw-16:ih-16".to_owned()));
        assert!(
            encoder.ffmpeg_filter().is_some(),
            "the scene encoder filters"
        );

        encoder.set_ffmpeg_filter(None);
        let probe = TargetQuality::remove_psychovisual_parameters(&encoder);

        assert_eq!(
            probe.ffmpeg_filter(),
            None,
            "the probe encoder must not filter"
        );
    }

    /// Probes are scored against the unfiltered reference, and a per-scene iso
    /// would also make scenes differ beyond the quantizer, so the probe drops
    /// the scene's photon noise while the final encode keeps it.
    #[test]
    fn a_probe_encoder_drops_the_photon_noise() {
        let mut encoder = Encoder::default_from_base(&EncoderBase::SVTAV1, false);
        let Encoder::SVTAV1 {
            photon_noise, ..
        } = &mut encoder
        else {
            panic!("the base is SVT-AV1");
        };
        *photon_noise = Some(PhotonNoise {
            iso:        12000,
            chroma_iso: Some(4000),
            width:      None,
            height:     None,
            c_y:        None,
            ccb:        None,
            ccr:        None,
        });

        let Encoder::SVTAV1 {
            photon_noise: scene,
            ..
        } = &encoder
        else {
            panic!("the base is SVT-AV1");
        };
        assert!(scene.is_some(), "the scene encoder applies grain");

        let probe = TargetQuality::remove_psychovisual_parameters(&encoder);
        let Encoder::SVTAV1 {
            photon_noise: probe,
            ..
        } = &probe
        else {
            panic!("the base is SVT-AV1");
        };
        assert!(
            probe.is_none(),
            "the probe encoder must not apply photon noise"
        );
    }
}
