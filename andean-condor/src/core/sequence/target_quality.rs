use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{self, Arc, atomic::AtomicBool},
    thread,
};

use anyhow::{Context, Result, bail, ensure};
use itertools::Itertools;
use thiserror::Error;
use tracing::{debug, error, trace};

use crate::models::sequence::provenance::{self, MetricBackend, ResultProvenance};
use crate::{
    core::{
        Condor,
        encoder::EncoderCapability,
        input::Input,
        sequence::{
            Sequence, SequenceCompletion, SequenceDetails, SequenceStatus, Status,
            parallel_encoder::{ParallelEncoder, Task as ParallelEncoderTask},
            quality_check::{finish_temporal_compare, spawn_compare_progress},
            scene_concatenator::SceneConcatenator,
            zone_encoder::{ZoneEncoder, ZonePlan},
        },
    },
    metrics::{self, Engine, OutputIndexing},
    models::{
        encoder::{Encoder, EncoderBase, cli_parameter::CLIParameter},
        input::{ImportMethod, Input as InputModel},
        sequence::{
            SequenceConfigHandler, SequenceDataHandler,
            parallel_encoder::ParallelEncoderConfigHandler,
            scene_concatenator::{ConcatMethod, SceneConcatenatorConfigHandler},
            target_quality::{
                TargetQualityConfig, TargetQualityConfigHandler, TargetQualityData,
                TargetQualityDataHandler,
                types::{
                    InterpolationMethod, MeasurementError, QualityMetric, QualityPass,
                    validate_scores,
                },
            },
        },
    },
    utils::interpolators,
    vapoursynth::{
        get_core,
        plugins::{
            MetricPluginFunction, PluginFunction,
            ffms2::Source,
            resize::bicubic::Bicubic,
            standard::{splice::Splice, trim::Trim},
            vship::{
                butteraugli::BUTTERAUGLI, cvvdp::CVVDP,
                ssimulacra2::SSIMULACRA2 as VSHIPSSIMULACRA2,
            },
            vszip::{ssimulacra2::SSIMULACRA2, xpsnr::XPSNR},
        },
    },
};

pub static DETAILS: SequenceDetails = SequenceDetails {
    name: "Target Quality",
    description: "Determine the optimal quantizer for a given video quality metric score per \
                  scene.",
    version: "0.0.1",
};

pub struct TargetQuality {
    pub input: Option<Input>,
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

        Self::validate_config(condor)?;
        // Ensure all the scene encoders are validated
        for scene in &condor.scenes {
            scene.encoder.validate()?;
        }
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
    ) -> Result<((), Vec<anyhow::Error>)> {
        let mut warnings = vec![];

        let parallel_encoder_config = condor.sequence_config.parallel_encoder()?;
        let config = condor.sequence_config.target_quality()?;

        // Ensure scenes is not empty
        if condor.scenes.is_empty() {
            warnings.push(anyhow::Error::new(TargetQualityError::ScenesEmpty));
            return Ok(((), warnings));
        }
        let Some(config) = config else {
            return Ok(((), warnings));
        };
        Self::validate_config(condor)?;
        let input = if let Some(input) = self.input.as_mut() {
            input
        } else if let Some(input_model) = &parallel_encoder_config.input {
            &mut Input::from_data(input_model)?
        } else {
            &mut condor.input
        };
        let metric_input = if let Some(input) = self.metric_input.as_mut() {
            Some(input)
        } else if let Some(metric_input) = &config.metric_input {
            Some(&mut Input::from_data(metric_input)?)
        } else {
            None
        };
        // Initialize input by getting clip_info. For VapourSynth inputs, this
        // may begin a lengthy caching process, hence the separation
        // between validate and initialize.
        progress_tx.send(SequenceStatus::Whole(Status::Processing {
            id: DETAILS.name.to_owned(),
            completion: SequenceCompletion::Custom {
                name: DETAILS.name.to_owned(),
                completed: 0.0,
                total: if metric_input.is_some() { 2.0 } else { 1.0 },
            },
        }))?;
        input.clip_info()?;
        if let Some(metric_input) = metric_input {
            progress_tx.send(SequenceStatus::Whole(Status::Processing {
                id: DETAILS.name.to_owned(),
                completion: SequenceCompletion::Custom {
                    name: DETAILS.name.to_owned(),
                    completed: 1.0,
                    total: 2.0,
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
        self.execute_cancel(condor, progress_tx, cancelled.into())
    }

    #[inline]
    fn execute_cancel(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        progress_tx: sync::mpsc::Sender<SequenceStatus>,
        cancelled: crate::utils::cancellation::Cancellation,
    ) -> Result<((), Vec<anyhow::Error>)> {
        self.execute_with_probe(
            condor,
            &progress_tx,
            &cancelled,
            Self::probe_pass_with_options,
        )
    }
}

impl TargetQuality {
    fn execute_with_probe<DataHandler, ConfigHandler>(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        progress_tx: &sync::mpsc::Sender<SequenceStatus>,
        cancelled: &crate::utils::cancellation::Cancellation,
        mut probe: impl FnMut(
            u8,
            &Path,
            &TargetQualityConfig,
            &mut Input,
            Option<&mut Input>,
            u8,
            &ConcatMethod,
            &[Task],
            sync::mpsc::Sender<SequenceStatus>,
            &crate::utils::cancellation::Cancellation,
            super::parallel_encoder::ParallelEncoderOptions,
        ) -> Result<(Vec<Task>, Vec<anyhow::Error>)>,
    ) -> Result<((), Vec<anyhow::Error>)>
    where
        DataHandler: SequenceDataHandler + TargetQualityDataHandler,
        ConfigHandler: SequenceConfigHandler
            + ParallelEncoderConfigHandler
            + SceneConcatenatorConfigHandler
            + TargetQualityConfigHandler,
    {
        cancelled.check()?;
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
        Self::validate_config(condor)?;
        let root_model = condor.input.as_data();
        let input_model = self.input.as_ref().map_or_else(
            || provenance::probe_input(&root_model, parallel_encoder_config, Some(&config)).clone(),
            Input::as_data,
        );
        let metric_model = self
            .metric_input
            .as_ref()
            .map(Input::as_data)
            .or_else(|| config.metric_input.clone());
        let mut admission = provenance::Admission::default();
        for (index, scene) in condor.scenes.iter().enumerate() {
            admission.admit_target(
                &input_model,
                metric_model.as_ref(),
                scene,
                Some(&config),
                index,
            )?;
        }
        let scene_frame_indices = condor
            .scenes
            .iter()
            .map(|scene| config.probing.strategy.frame_indices(scene.start_frame, scene.end_frame))
            .collect::<Result<Vec<_>>>()?;
        // Validate every claimed history before restoring even the first scene.
        for (index, (scene, indices)) in condor.scenes.iter().zip(&scene_frame_indices).enumerate()
        {
            if let Ok(data) = scene.sequence_data.get_target_quality() {
                Self::validate_history(&config, index, indices.len(), &data.passes)?;
            }
        }
        let input = if let Some(input) = self.input.as_mut() {
            input
        } else if let Some(input_model) = &config.input {
            &mut Input::from_data(input_model)?
        } else if let Some(input_model) = &parallel_encoder_config.input {
            &mut Input::from_data(input_model)?
        } else {
            &mut condor.input
        };

        // The post-pass ordinal must represent 256 even though public passes stop at 255.
        let mut ordinal = 1_u16;

        // A resumed/exhausted search may schedule no tasks, but its final q must
        // still come from recorded measurements. Empty histories leave scenes untouched.
        let mut restored_quantizers = false;
        for scene in &mut condor.scenes {
            let Ok(data) = scene.sequence_data.get_target_quality() else {
                continue;
            };
            let history = Self::quantizer_score_history(&config, &data.passes)?;
            if let Some(q) = Self::measured_quantizer(&config.metric, &history)
                && scene.encoder.quantizer() != Some(q)
            {
                scene.encoder.set_quantizer(q);
                restored_quantizers = true;
            }
            if !history.is_empty() {
                let selection =
                    ResultProvenance::new(provenance::selection_context(scene, &config)?);
                let data = scene.sequence_data.get_target_quality_mut()?;
                if data.selection.as_ref() != Some(&selection) {
                    data.selection = Some(selection);
                    restored_quantizers = true;
                }
            }
        }

        loop {
            if ordinal > u16::from(config.maximum_probes) {
                break;
            }
            let pass = u8::try_from(ordinal).expect("ordinal is within the configured u8 budget");

            let pass_directory = target_quality_directory.join(pass.to_string());
            let tasks = condor
                .scenes
                .iter()
                .zip(&scene_frame_indices)
                .enumerate()
                .map(|(index, (scene, frame_indices))| -> Result<Option<Task>> {
                    let passes = scene
                        .sequence_data
                        .get_target_quality()
                        .map_or_else(|_| TargetQualityData::default(), |tq| tq.clone())
                        .passes;
                    // Admit only this scene's next ordinal, before capability probes or scratch.
                    // More-complete peers must never enter a lower whole-pass replacement.
                    if passes.len() != usize::from(pass - 1)
                        || passes.len() >= usize::from(config.maximum_probes)
                    {
                        return Ok(None);
                    }
                    let quantizer_score_history = Self::quantizer_score_history(&config, &passes)?;
                    if quantizer_score_history
                        .iter()
                        .any(|(_, score)| config.metric.score_within_target(*score))
                    {
                        return Ok(None);
                    }
                    let mut encoder = provenance::probe_encoder(scene, &config)?;
                    let Some(predicted_quantizer) = Self::next_probe_quantizer(
                        &config,
                        &encoder,
                        &quantizer_score_history,
                        cancelled,
                    )?
                    else {
                        return Ok(None);
                    };
                    encoder.set_quantizer(predicted_quantizer);
                    let output = pass_directory.join(format!(
                        "{}.{}",
                        ParallelEncoder::scene_id(index),
                        encoder.base().output_extension()
                    ));
                    Ok(Some(Task {
                        original_index: index,
                        frame_indices: frame_indices.clone(),
                        encoder,
                        output,
                        passes,
                    }))
                })
                .filter_map(|result: Result<Option<Task>>| result.transpose())
                .collect::<Result<Vec<_>>>()?;

            if tasks.is_empty() {
                // Empty early ordinals can precede resumable scenes with longer prefixes.
                if !condor.scenes.iter().any(|scene| {
                    scene.sequence_data.get_target_quality().is_ok_and(|data| {
                        data.passes.len() >= usize::from(pass)
                            && data.passes.len() < usize::from(config.maximum_probes)
                    })
                }) {
                    break;
                }
                ordinal += 1;
                continue;
            }

            progress_tx.send(SequenceStatus::Whole(Status::Processing {
                id: DETAILS.name.to_owned(),
                completion: SequenceCompletion::Passes {
                    completed: pass - 1,
                    total: pass,
                },
            }))?;
            if !pass_directory.exists() {
                std::fs::create_dir_all(&pass_directory)?;
            }

            debug!("Starting Pass {}", pass);

            let progress_tx_clone = progress_tx.clone();
            let (pass_progress_tx, pass_progress_rx) = sync::mpsc::channel();
            let relay = thread::spawn(move || -> Result<()> {
                for progress in pass_progress_rx {
                    match progress {
                        SequenceStatus::Whole(Status::Processing { id, completion })
                            if id == "Encode" =>
                        {
                            #[allow(clippy::collapsible_match)]
                            if let SequenceCompletion::Frames { completed, total } = completion {
                                progress_tx_clone.send(SequenceStatus::Subprocess {
                                    parent: Status::Processing {
                                        id: DETAILS.name.to_owned(),
                                        completion: SequenceCompletion::Passes {
                                            completed: pass,
                                            total: config.maximum_probes,
                                        },
                                    },
                                    child: Status::Processing {
                                        id,
                                        completion: SequenceCompletion::Frames { completed, total },
                                    },
                                })?;
                            }
                        },
                        SequenceStatus::Whole(Status::Completed { id }) if id == "Encode" => {
                            progress_tx_clone.send(SequenceStatus::Subprocess {
                                parent: Status::Processing {
                                    id: DETAILS.name.to_owned(),
                                    completion: SequenceCompletion::Passes {
                                        completed: pass,
                                        total: config.maximum_probes,
                                    },
                                },
                                child: Status::Completed { id },
                            })?;
                        },
                        SequenceStatus::Whole(Status::Processing { id, completion })
                            if id == "Compare" =>
                        {
                            #[allow(clippy::collapsible_match)]
                            if let SequenceCompletion::Frames { completed, total } = completion {
                                progress_tx_clone.send(SequenceStatus::Subprocess {
                                    parent: Status::Processing {
                                        id: DETAILS.name.to_owned(),
                                        completion: SequenceCompletion::Passes {
                                            completed: pass,
                                            total: config.maximum_probes,
                                        },
                                    },
                                    child: Status::Processing {
                                        id,
                                        completion: SequenceCompletion::Frames { completed, total },
                                    },
                                })?;
                            }
                        },
                        SequenceStatus::Whole(Status::Completed { id }) if id == "Compare" => {
                            progress_tx_clone.send(SequenceStatus::Subprocess {
                                parent: Status::Processing {
                                    id: DETAILS.name.to_owned(),
                                    completion: SequenceCompletion::Passes {
                                        completed: pass,
                                        total: config.maximum_probes,
                                    },
                                },
                                child: Status::Completed { id },
                            })?;
                        },
                        _ => (),
                    }
                }

                Ok(())
            });

            let before = admission.measurement_inputs(
                &input_model,
                metric_model.as_ref(),
                Some(&config.metric),
            )?;
            let mut configured_metric_input = if self.metric_input.is_none() {
                config.metric_input.as_ref().map(Input::from_data).transpose()?
            } else {
                None
            };
            let probed = probe(
                pass,
                target_quality_directory,
                &config,
                input,
                self.metric_input.as_mut().or(configured_metric_input.as_mut()),
                workers,
                &scene_concatenator_config.method,
                tasks.as_slice(),
                pass_progress_tx,
                cancelled,
                super::parallel_encoder::ParallelEncoderOptions {
                    max_tries: parallel_encoder_config.max_tries,
                    validate_frame_count: parallel_encoder_config.validate_frame_count,
                },
            );
            let relayed = crate::core::encoder::pipes::join_worker(
                relay,
                "Joining target-quality pass progress",
            );
            let (completed_tasks, pass_warnings) = probed?;
            relayed?;
            let mut publication = provenance::Admission::default();
            ensure!(
                before
                    == publication.measurement_inputs(
                        &input_model,
                        metric_model.as_ref(),
                        Some(&config.metric)
                    )?,
                "Target Quality inputs changed during measurement; recompute or use a new job"
            );
            if completed_tasks.is_empty() {
                warnings.extend(pass_warnings);
                break;
            }

            let previous_lengths = condor
                .scenes
                .iter()
                .map(|scene| scene.sequence_data.get_target_quality().map(|data| data.passes.len()))
                .collect::<Result<Vec<_>>>()?;
            let mut staged = condor.scenes.clone();
            let mut completions = Vec::with_capacity(completed_tasks.len());
            for mut completed_task in completed_tasks {
                let index = completed_task.original_index;
                publication.admit_target(
                    &input_model,
                    metric_model.as_ref(),
                    &staged[index],
                    Some(&config),
                    index,
                )?;
                let previous = staged[index].sequence_data.get_target_quality()?.passes.len();
                Self::validate_history(
                    &config,
                    index,
                    scene_frame_indices[index].len(),
                    &completed_task.passes,
                )?;
                // Only newly successful measurements are stamped; never certify a prefix.
                for quality in completed_task.passes.iter_mut().skip(previous) {
                    quality.provenance = Some(ResultProvenance::new(publication.target_context(
                        &input_model,
                        metric_model.as_ref(),
                        &staged[index],
                        &config,
                        quality,
                    )?));
                }
                if let Some(previous_backend) = staged[index]
                    .sequence_data
                    .get_target_quality()?
                    .passes
                    .first()
                    .and_then(|p| p.backend.as_ref())
                {
                    ensure!(
                        completed_task
                            .passes
                            .iter()
                            .all(|p| p.backend.as_ref() == Some(previous_backend)),
                        "scene {index} TQ backend changed; explicitly recompute history or use a new job"
                    );
                }
                // Update Target Quality Passes
                staged[completed_task.original_index]
                    .sequence_data
                    .get_target_quality_mut()?
                    .passes = completed_task.passes.clone();

                let quantizer_score_history =
                    Self::quantizer_score_history(&config, &completed_task.passes)?;
                let final_quantizer =
                    Self::measured_quantizer(&config.metric, &quantizer_score_history)
                        .ok_or(TargetQualityError::QualityMeasurementFailed)?;
                staged[completed_task.original_index].encoder.set_quantizer(final_quantizer);
                let selection =
                    ResultProvenance::new(provenance::selection_context(&staged[index], &config)?);
                staged[index].sequence_data.get_target_quality_mut()?.selection = Some(selection);

                let quality_pass = completed_task.passes.last().expect("passes is not empty");

                completions.push(SequenceStatus::Subprocess {
                    parent: Status::Processing {
                        id: DETAILS.name.to_owned(),
                        completion: SequenceCompletion::Passes {
                            completed: pass,
                            total: config.maximum_probes,
                        },
                    },
                    child: Status::Processing {
                        id: "Quality".to_owned(),
                        completion: Self::quality_completion(
                            &config,
                            completed_task.original_index,
                            quality_pass,
                        )?,
                    },
                });
            }
            admission = publication;
            condor.scenes = staged;
            for completion in completions {
                progress_tx.send(completion)?;
            }

            if !pass_warnings.is_empty() {
                warnings.extend(pass_warnings);
                break;
            }

            ordinal += 1;
            let mut data = condor_data.clone();
            data.scenes = condor.scenes.clone();
            if let Err(error) = (condor.save_callback)(data) {
                for (scene, previous) in condor.scenes.iter_mut().zip(previous_lengths) {
                    for quality in scene
                        .sequence_data
                        .get_target_quality_mut()?
                        .passes
                        .iter_mut()
                        .skip(previous)
                    {
                        quality.provenance = None;
                    }
                    scene.sequence_data.get_target_quality_mut()?.selection = None;
                }
                return Err(error).context("Saving measured Target Quality results");
            }
            restored_quantizers = false;

            cancelled.check_forced()?;
            if cancelled.is_cancelled() {
                break;
            }
        }

        cancelled.check_forced()?;
        if restored_quantizers && !cancelled.is_cancelled() {
            let mut data = condor_data;
            data.scenes = condor.scenes.clone();
            (condor.save_callback)(data)
                .context("Saving restored measured Target Quality quantizers")?;
        }

        Ok(((), warnings))
    }
}

impl TargetQuality {
    pub const DETAILS: SequenceDetails = DETAILS;

    fn next_probe_quantizer(
        config: &TargetQualityConfig,
        encoder: &Encoder,
        history: &[(f64, f64)],
        cancelled: &crate::utils::cancellation::Cancellation,
    ) -> Result<Option<f64>> {
        cancelled.check_forced()?;
        let target = config.metric.target_range().0.midpoint(config.metric.target_range().1);
        let bracket = Self::quantizer_bounds(
            config.quantizer_range,
            target,
            history,
            config.metric.is_inverse_metric(),
        );
        let bounds = (
            bracket.0.max(config.quantizer_range.0 as f64),
            bracket.1.min(config.quantizer_range.1 as f64),
        );
        let step = match encoder {
            Encoder::X264 { .. } | Encoder::X265 { .. } => 0.25,
            Encoder::SVTAV1 { .. }
                if encoder.supports_capability_cancel(
                    EncoderCapability::SvtAv1QuarterStepCrf,
                    cancelled,
                )? =>
            {
                0.25
            },
            _ => 1.0,
        };
        let predicted =
            Self::predict_quantizer(bounds, target, config.interpolators, history, step)?;
        Ok(Self::untested_quantizer(bounds, predicted, history, step))
    }

    fn untested_quantizer(
        bounds: (f64, f64),
        predicted: f64,
        history: &[(f64, f64)],
        step: f64,
    ) -> Option<f64> {
        // predict_quantizer validates these bounds and step before any tick arithmetic.
        let lower = bounds.0 / step;
        let upper = bounds.1 / step;
        let available = |tick: i64| {
            let q = tick as f64 * step;
            (bounds.0..=bounds.1).contains(&q) && !history.iter().any(|(tested, _)| *tested == q)
        };
        let interior = (lower.floor() as i64 + 1, upper.ceil() as i64 - 1);
        let candidate = (predicted / step).round();
        if candidate.is_finite()
            && (interior.0 as f64..=interior.1 as f64).contains(&candidate)
            && available(candidate as i64)
        {
            return Some(candidate * step);
        }
        let midpoint = lower.midpoint(upper);
        let nearest = |first: i64, last: i64| {
            let mut left = (midpoint.floor() as i64).min(last);
            let mut right = (left + 1).max(first);
            // ponytail: O(history²) exclusion checks, sorted ticks if histories grow;
            // never enumerate the grid, even for the full u32 configured range.
            for _ in 0..=history.len() {
                let tick = if left >= first
                    && (right > last || midpoint - left as f64 <= right as f64 - midpoint)
                {
                    let tick = left;
                    left -= 1;
                    tick
                } else if right <= last {
                    let tick = right;
                    right += 1;
                    tick
                } else {
                    return None;
                };
                if available(tick) {
                    return Some(tick as f64 * step);
                }
            }
            None
        };
        nearest(interior.0, interior.1).or_else(|| {
            // Only aligned endpoints can remain after interior exhaustion;
            // both are equally far from the midpoint, so try the lower first.
            [lower.ceil() as i64, upper.floor() as i64]
                .into_iter()
                .find(|tick| (*tick < interior.0 || *tick > interior.1) && available(*tick))
                .map(|tick| tick as f64 * step)
        })
    }

    fn validate_quantizer_grid(range: (f64, f64), step: f64) -> Result<()> {
        anyhow::ensure!(
            range.0.is_finite() && range.1.is_finite() && range.0 <= range.1,
            "Target Quality quantizer range must be finite and ordered: {range:?}"
        );
        anyhow::ensure!(
            step.is_finite() && step > 0.0,
            "Target Quality quantizer step must be positive and finite: {step}"
        );
        // Keep ticks and their immediate neighbors exactly representable, including
        // public callers with much larger ranges or smaller steps than encoder CRFs.
        let max_tick = ((1_u64 << 52) - 1) as f64;
        anyhow::ensure!(
            (range.0 / step).abs() <= max_tick && (range.1 / step).abs() <= max_tick,
            "Target Quality quantizer grid exceeds exact tick arithmetic: range {range:?}, step {step}"
        );
        Ok(())
    }

    /// Pure admission check shared with the TUI before it opens override inputs.
    /// Disabled/empty stages do no probing; cancellation is checked by their callers first.
    #[inline]
    pub fn validate_config<DataHandler, ConfigHandler>(
        condor: &Condor<DataHandler, ConfigHandler>,
    ) -> Result<()>
    where
        DataHandler: SequenceDataHandler,
        ConfigHandler: SequenceConfigHandler + TargetQualityConfigHandler,
    {
        if let Some(config) = condor.sequence_config.target_quality()? {
            config.validate(&condor.encoder)?;
            for (index, scene) in condor.scenes.iter().enumerate() {
                config
                    .validate(&scene.encoder)
                    .map_err(|error| anyhow::anyhow!("Target Quality scene {index}: {error:#}"))?;
                config.probing.strategy.validate_range(scene.start_frame, scene.end_frame)?;
            }
        }
        Ok(())
    }

    pub(crate) fn quantizer_score_history(
        config: &TargetQualityConfig,
        passes: &[QualityPass],
    ) -> Result<Vec<(f64, f64)>> {
        passes
            .iter()
            .enumerate()
            .map(|(index, pass)| {
                ensure!(
                    pass.quantizer.is_finite(),
                    MeasurementError::Record("quantizer")
                );
                Ok((
                    pass.quantizer,
                    config
                        .metric
                        .calculate_score(config.probing.statistic, &pass.scores)
                        .with_context(|| {
                            format!(
                                "Target Quality {} history pass {}",
                                config.metric.friendly_name(),
                                index + 1
                            )
                        })?,
                ))
            })
            .collect()
    }

    pub(crate) fn validate_history(
        config: &TargetQualityConfig,
        scene: usize,
        expected: usize,
        passes: &[QualityPass],
    ) -> Result<()> {
        for (index, pass) in passes.iter().enumerate() {
            validate_scores(&pass.scores, Some(expected))
                .and_then(|()| {
                    ensure!(
                        pass.quantizer.is_finite(),
                        MeasurementError::Record("quantizer")
                    );
                    ensure!(
                        pass.bitrate.is_finite(),
                        MeasurementError::Record("bitrate")
                    );
                    config
                        .metric
                        .calculate_score(config.probing.statistic, &pass.scores)
                        .map(|_| ())
                })
                .with_context(|| {
                    format!(
                        "Target Quality {} scene {scene} history pass {}",
                        config.metric.friendly_name(),
                        index + 1
                    )
                })?;
        }
        Ok(())
    }

    fn quantizer_bounds(
        quantizer_range: (u32, u32),
        target_score: f64,
        quantizer_score_history: &[(f64, f64)],
        inverse_metric: bool,
    ) -> (f64, f64) {
        let sorted = quantizer_score_history
            .iter()
            .sorted_by(|(_, score1), (_, score2)| {
                score1.partial_cmp(score2).unwrap_or(std::cmp::Ordering::Equal)
            })
            .collect::<Vec<_>>();
        // Exact target probes stay in the verified-quantizer flow, not the strict bracket.
        let above = sorted.iter().find(|(_, score)| *score > target_score);
        let below = sorted.iter().rfind(|(_, score)| *score < target_score);
        let (lower, upper) = if inverse_metric {
            (below, above)
        } else {
            (above, below)
        };
        let bounds = (
            lower.map_or(quantizer_range.0 as f64, |(quantizer, _)| *quantizer),
            upper.map_or(quantizer_range.1 as f64, |(quantizer, _)| *quantizer),
        );
        if bounds.0 > bounds.1 {
            // Either metric direction can cross under noise; use the safe configured range.
            (quantizer_range.0 as f64, quantizer_range.1 as f64)
        } else {
            bounds
        }
    }

    fn quality_completion(
        config: &TargetQualityConfig,
        index: usize,
        pass: &QualityPass,
    ) -> Result<SequenceCompletion> {
        Ok(SequenceCompletion::SceneQuality {
            index: index as u64,
            quantizer: pass.quantizer,
            score: config.metric.calculate_score(config.probing.statistic, &pass.scores)?,
            bitrate: pass.bitrate,
        })
    }

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
                    executable: executable.clone(),
                    pass: *pass,
                    options: sanitized_options,
                    photon_noise: None,
                    ffmpeg_filter: ffmpeg_filter.clone(),
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
                    executable: executable.clone(),
                    pass: *pass,
                    options: sanitized_options,
                    photon_noise: None,
                    ffmpeg_filter: ffmpeg_filter.clone(),
                    append_color_data: *append_color_data,
                }
            },
            Encoder::VPX { .. } => encoder.clone(),
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
                    executable: executable.clone(),
                    pass: *pass,
                    options: sanitized_options,
                    photon_noise: None,
                    ffmpeg_filter: ffmpeg_filter.clone(),
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
                    executable: executable.clone(),
                    pass: *pass,
                    options: sanitized_options,
                    photon_noise: None,
                    ffmpeg_filter: ffmpeg_filter.clone(),
                    append_color_data: *append_color_data,
                }
            },
            Encoder::X264 { .. } => encoder.clone(),
            Encoder::X265 { .. } => encoder.clone(),
            Encoder::VVenC { .. } => encoder.clone(),
            Encoder::FFmpeg { .. } => encoder.clone(),
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
        Self::probe_pass_cancel(
            pass,
            target_quality_directory,
            config,
            input,
            metric_input,
            workers,
            concat_method,
            tasks,
            progress_tx,
            &crate::utils::cancellation::Cancellation::from(Arc::clone(cancelled)),
        )
    }

    #[inline]
    #[allow(clippy::too_many_arguments, clippy::type_complexity)]
    pub fn probe_pass_cancel(
        pass: u8,
        target_quality_directory: &Path,
        config: &TargetQualityConfig,
        input: &mut Input,
        metric_input: Option<&mut Input>,
        workers: u8,
        concat_method: &ConcatMethod,
        tasks: &[Task],
        progress_tx: sync::mpsc::Sender<SequenceStatus>,
        cancellation: &crate::utils::cancellation::Cancellation,
    ) -> Result<(Vec<Task>, Vec<anyhow::Error>)> {
        Self::probe_pass_with_validation(
            pass,
            target_quality_directory,
            config,
            input,
            metric_input,
            workers,
            concat_method,
            tasks,
            progress_tx,
            cancellation,
            true,
        )
    }

    /// Probe pass with the owning job's count-only policy; old standalone APIs are strict.
    #[inline]
    #[allow(clippy::too_many_arguments, clippy::type_complexity)]
    pub fn probe_pass_with_validation(
        pass: u8,
        target_quality_directory: &Path,
        config: &TargetQualityConfig,
        input: &mut Input,
        metric_input: Option<&mut Input>,
        workers: u8,
        concat_method: &ConcatMethod,
        tasks: &[Task],
        progress_tx: sync::mpsc::Sender<SequenceStatus>,
        cancellation: &crate::utils::cancellation::Cancellation,
        validate_frame_count: bool,
    ) -> Result<(Vec<Task>, Vec<anyhow::Error>)> {
        Self::probe_pass_with_options(
            pass,
            target_quality_directory,
            config,
            input,
            metric_input,
            workers,
            concat_method,
            tasks,
            progress_tx,
            cancellation,
            super::parallel_encoder::ParallelEncoderOptions {
                validate_frame_count,
                ..Default::default()
            },
        )
    }

    /// Explicit per-scene policy; whole-zone batches still make one attempt.
    /// Complete validated pass records return unchanged without native work.
    /// Incomplete passes require all earlier records and outputs directly under
    /// `<target_quality_directory>/<pass>`; stale outputs must unlink successfully
    /// before the whole batch is encoded and admitted. No per-file reuse occurs.
    #[allow(clippy::too_many_arguments, clippy::type_complexity)]
    #[inline]
    pub fn probe_pass_with_options(
        pass: u8,
        target_quality_directory: &Path,
        config: &TargetQualityConfig,
        input: &mut Input,
        metric_input: Option<&mut Input>,
        workers: u8,
        concat_method: &ConcatMethod,
        tasks: &[Task],
        progress_tx: sync::mpsc::Sender<SequenceStatus>,
        cancellation: &crate::utils::cancellation::Cancellation,
        options: super::parallel_encoder::ParallelEncoderOptions,
    ) -> Result<(Vec<Task>, Vec<anyhow::Error>)> {
        anyhow::ensure!(
            options.max_tries >= 1,
            "max_tries must be at least 1 total attempt"
        );
        cancellation.check_forced()?;
        if cancellation.is_cancelled() {
            return Ok((Vec::new(), Vec::new()));
        }
        ensure!(pass > 0, "Target Quality pass must be at least 1");
        if tasks.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        for task in tasks {
            config.validate(&task.encoder)?;
            task.encoder.validate_settings()?;
            ensure!(
                !task.frame_indices.is_empty(),
                "Target Quality scene {} has no selected frames",
                task.original_index
            );
            Self::validate_history(
                config,
                task.original_index,
                task.frame_indices.len(),
                &task.passes,
            )?;
        }
        let temporal = metrics::scene_temporal(&config.metric);
        let scenes: Vec<_> = tasks.iter().map(|task| task.frame_indices.as_slice()).collect();
        if temporal {
            metrics::probe::validate_scene_selection(&scenes)?;
        }
        // Records, not scratch artifacts or native availability, establish reuse.
        if Self::pass_is_complete(tasks, pass) {
            return Ok((tasks.to_vec(), Vec::new()));
        }
        let previous_passes = usize::from(pass - 1);
        for task in tasks {
            ensure!(
                task.passes.len() >= previous_passes,
                "Target Quality scene {} pass {pass} is missing an earlier recorded pass",
                task.original_index
            );
            task.encoder.validate()?;
        }
        // Preserve early prerequisite diagnostics, but only for passes that
        // actually need work. These lookups launch no encoder or metric.
        super::parallel_encoder::artifact::preflight(options.validate_frame_count)?;
        let warnings: Vec<anyhow::Error> = vec![];
        let pass_directory = target_quality_directory.join(pass.to_string());
        let output = concat_method.with_extension(&target_quality_directory.join(pass.to_string()));
        // SECURITY: Only declared outputs in this pass belong to the replay. Do
        // not scan neighbors, recurse into directories, or follow a pass-dir link.
        match pass_directory.symlink_metadata() {
            Ok(metadata) => ensure!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "Target Quality pass {pass} scratch directory is not a directory: {}",
                pass_directory.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "Inspecting Target Quality pass {pass} scratch {}",
                        pass_directory.display()
                    )
                });
            },
        }
        for task in tasks {
            ensure!(
                task.output.parent() == Some(pass_directory.as_path()),
                "Target Quality scene {} output is outside pass {pass} scratch: {}",
                task.original_index,
                task.output.display()
            );
        }
        for source in std::iter::once(&*input).chain(metric_input.as_deref()) {
            let path = match source {
                Input::Video { path, .. } | Input::VapourSynth { path, .. } => Some(path),
                Input::VapourSynthScript {
                    source: crate::models::input::VapourSynthScriptSource::Path(path),
                    ..
                } => Some(path),
                Input::VapourSynthScript { .. } => None,
            };
            if let Some(source) = path {
                let source = source.canonicalize().with_context(|| {
                    format!(
                        "Resolving Target Quality input before pass {pass} cleanup: {}",
                        source.display()
                    )
                })?;
                for candidate in
                    tasks.iter().map(|task| &task.output).chain(std::iter::once(&output))
                {
                    // Unlinking an output symlink is safe; only a regular entry
                    // could remove the input itself through an aliased parent.
                    if candidate.symlink_metadata().is_ok_and(|metadata| metadata.is_file()) {
                        ensure!(
                            candidate.canonicalize().with_context(|| format!(
                                "Resolving stale output {}",
                                candidate.display()
                            ))? != source,
                            "Target Quality pass {pass} output overlaps input {}; refusing stale cleanup",
                            candidate.display()
                        );
                    }
                }
            }
        }
        // An incomplete pass describes no single measurement. Remove every
        // candidate output before any native work, even if some have records.
        for path in tasks.iter().map(|task| &task.output).chain(std::iter::once(&output)) {
            match std::fs::remove_file(path) {
                Ok(()) => debug!("Discarding orphaned probe output {}", path.display()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!(
                            "Discarding stale Target Quality pass {pass} output {}",
                            path.display()
                        )
                    });
                },
            }
        }
        let framerate = input.clip_info()?.frame_rate;
        let total_frames = tasks.iter().fold(0, |acc, task| acc + task.frame_indices.len());

        let encode_tasks = tasks
            .iter()
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
                                id: "Encode".to_owned(),
                                completion: SequenceCompletion::Frames {
                                    completed,
                                    total: total_frames as u64,
                                },
                            }))?;
                        }
                    },
                    SequenceStatus::Whole(Status::Completed { id: _ }) => {
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
        // ponytail: lose H.26x grouping for final-scene parity and crash-safe probes;
        // reenable only with native safety and probe/final parity proof.
        let zone_plan = if encode_tasks
            .iter()
            .any(|task| matches!(task.encoder.base(), EncoderBase::X264 | EncoderBase::X265))
        {
            None
        } else {
            match ZonePlan::try_build_cancel(&encode_tasks, cancellation) {
                Ok(plan) => Some(plan),
                Err(reason) => {
                    if cancellation.is_forced() {
                        drop(encode_progress_tx);
                        let relayed = crate::core::encoder::pipes::join_worker(
                            encode_thread,
                            "Joining cancelled probe progress",
                        );
                        relayed?;
                        return Err(reason);
                    }
                    debug!("Probe pass {pass} encoding scenes individually: {reason}");
                    None
                },
            }
        };
        let results = match zone_plan {
            Some(plan) => ZoneEncoder::encode_tasks_with_validation(
                input,
                encode_tasks.iter().cloned().collect::<VecDeque<_>>(),
                plan,
                encode_progress_tx,
                cancellation.clone(),
                options.validate_frame_count,
            ),
            None => ParallelEncoder::encode_tasks_with_options(
                input,
                workers,
                encode_tasks.iter().cloned().collect::<VecDeque<_>>(),
                encode_progress_tx,
                cancellation.clone(),
                options,
            ),
        };
        let relayed = crate::core::encoder::pipes::join_worker(
            encode_thread,
            "Joining probe encode progress",
        );
        let results = results?;
        relayed?;

        cancellation.check_forced()?;
        if cancellation.is_cancelled() {
            // No scene was scored, so no task is complete. Returning the input
            // tasks would report them as scored with the `passes` they had on
            // entry, empty on the first pass.
            return Ok((Vec::new(), warnings));
        }

        let scene_paths = tasks.iter().map(|task| task.output.clone()).collect::<Vec<_>>();
        match concat_method {
            ConcatMethod::MKVMerge(_) => {
                SceneConcatenator::mkvmerge_cancel(
                    &pass_directory,
                    &output,
                    &scene_paths,
                    None,
                    framerate,
                    None,
                    &progress_tx,
                    cancellation,
                )?;
            },
            ConcatMethod::FFmpeg(_) => {
                SceneConcatenator::ffmpeg_cancel(
                    &pass_directory,
                    &output,
                    &scene_paths,
                    None,
                    total_frames,
                    framerate,
                    None,
                    &progress_tx,
                    cancellation,
                )?;
            },
            ConcatMethod::Ivf => {
                SceneConcatenator::ivf_cancel(&output, &scene_paths, &progress_tx, cancellation)?;
            },
        }

        let cancelled = &cancellation.legacy_flag();
        let metric_input = metric_input.unwrap_or(input);
        // VMAF is scored by libvmaf over decoded frames, and its decoder borrow
        // would conflict with the one the node graph below needs. Score it
        // first, while `metric_input` is still free.
        //
        // The VapourSynth VMAF plugin cannot be used here at all: it emits no
        // per-frame scores and requires writing to and reading back a log file.
        //
        // Only the frames this pass actually probes are decoded and scored,
        // matching the plugin path, which trims each frame out of the
        // graph so VapourSynth never decodes the skipped ones.
        //
        // A native failure yields `None` and falls through to the plugin
        // branches; VMAF has no such branch and propagates its error.
        let is_vmaf = matches!(config.metric, QualityMetric::VMAF { .. });
        let engine = metrics::engine(&config.metric, !matches!(metric_input, Input::Video { .. }));
        let native = is_vmaf || engine.is_some();

        // Created and relayed here, before scoring, so the native path reports
        // compare progress through the same relay the plugin branches
        // use and the UI shows the same shape whichever path ran.
        let (compare_progress_tx, compare_thread) = spawn_compare_progress(progress_tx.clone());

        let native_scores = if native {
            let probed: Vec<usize> =
                tasks.iter().flat_map(|task| task.frame_indices.iter().copied()).collect();

            let reference = metric_input.decoder();
            let mut distorted = Input::from_video(&InputModel::Video {
                path: output.clone(),
                import_method: ImportMethod::FFMS2 {
                    index: None,
                    cache_path: None,
                },
                // The probe output is already in the metric input's format, so
                // it must not be converted again.
                filters: Vec::new(),
            })?;

            // The probe encode contains only the frames this pass selected, so
            // it is compacted and is read by position within the
            // selection. VMAF submission drives the same progress relay as the
            // other engines' score callbacks; final scores must not count twice.
            let scored = if is_vmaf {
                metrics::vmaf::score_probed_frames(
                    reference,
                    distorted.decoder(),
                    &config.metric,
                    &probed,
                    OutputIndexing::Compacted,
                    Some(cancelled),
                    |position| {
                        report_compare_progress(&compare_progress_tx, probed.len(), position + 1);
                    },
                    |_, _| {},
                )
            } else {
                // A name with no entry point here yields an error rather than a
                // `return`, so it takes the same path as any
                // other native failure and the progress
                // relay is joined on the way out.
                match engine {
                    Some(engine @ (Engine::Vship | Engine::Fmetrics)) => Self::score_native_scenes(
                        engine,
                        reference,
                        distorted.decoder(),
                        &config.metric,
                        &scenes,
                        &compare_progress_tx,
                        cancelled,
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

            Some(scored)
        } else {
            None
        };

        // The relay thread spawned above ends only once every sender is
        // dropped. Each plugin branch moves its sender into
        // `get_scores`, so the channel disconnects as soon as that call
        // returns. The branch that reuses the native scores sends a
        // final completion and drops its sender instead; holding it
        // across the `join` below would leave the relay thread waiting
        // forever.
        // A metric scored natively above reuses those scores; otherwise the
        // plugin branches below run. For the vship metrics the plugin
        // branch is reached only when libvship was unavailable or
        // failed.
        let mut plugin_backend = None;
        let plugin = |compare_progress_tx| -> Result<Vec<f64>> {
            // The branches below score over a VapourSynth graph built from the
            // metric input, so everything here - the graph itself,
            // and for a natively decoded input the VapourSynth
            // script it is converted into - is created only on this
            // path. A host without VapourSynth scores every metric with a
            // native engine without touching it at all.
            //
            // Owned, so it outlives the `&mut v_input` borrow taken below.
            let mut v_input_owned = if matches!(metric_input, Input::Video { .. }) {
                metric_input.as_vapoursynth_script()?
            } else {
                None
            };
            let v_input = v_input_owned.as_mut();
            let decoder = match metric_input {
                Input::VapourSynth { decoder, .. } | Input::VapourSynthScript { decoder, .. } => {
                    decoder
                },
                Input::Video { .. } => v_input.expect("Video Input exists").decoder(),
            };
            let vapoursynth_decoder =
                decoder.get_vapoursynth_impl().expect("Decoder is VapourSynth");
            let env = &vapoursynth_decoder.env;
            let reference_node = vapoursynth_decoder.get_output(
                vapoursynth_decoder.get_output_index(),
                vapoursynth_decoder.get_node_modifier(),
            )?;
            let core = get_core(env)?;
            plugin_backend = Some(MetricBackend::plugin_for_metric(core, &config.metric)?);

            let reference_node = if temporal {
                reference_node
            } else {
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

            Ok(match &config.metric {
                // Always scored above, since VMAF has no plugin branch here.
                QualityMetric::VMAF { .. } => {
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
                QualityMetric::XPSNR { resolution, .. } => {
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
                    // XPSNR returns a score per plane, combine them into the
                    // weighted XPSNR score.
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
                    if temporal {
                        plugin.collect_scene_values(
                            core,
                            &reference_node,
                            &distorted_node,
                            &scenes,
                            OutputIndexing::Compacted,
                            compare_progress_tx,
                            cancelled,
                            |_, _| Ok(()),
                        )?
                    } else {
                        let node = plugin.invoke(
                            core,
                            &reference_node,
                            &distorted_node,
                            // The compacted encode/node indices cannot describe source gaps.
                            tasks.iter().flat_map(|task| task.frame_indices.iter().copied()),
                        )?;
                        CVVDP::get_scores(&node, None, compare_progress_tx)?
                    }
                },
            })
        };
        let Some((scores, actual)) = Self::route_compare_scores(
            native_scores,
            engine,
            &config.metric,
            compare_progress_tx,
            compare_thread,
            plugin,
            |scores| Self::validate_task_scores(config, tasks, scores),
        )?
        else {
            return Ok((Vec::new(), warnings));
        };
        let actual = if actual == Engine::VapourSynth {
            plugin_backend.context("successful plugin branch has no admission identity")?
        } else {
            MetricBackend::current(actual)
        };

        drop(progress_tx);

        // Zone and per-scene results both identify the original scene, not
        // its position in this candidate batch.
        let results = results
            .into_iter()
            .flatten()
            .map(|result| (result.scene, result))
            .collect::<HashMap<_, _>>();

        let grouped = Self::group_task_scores(config, tasks, scores)?;
        let tasks = tasks
            .iter()
            .zip(grouped)
            .map(|(task, scores)| -> Result<Task> {
                let mut completed_task = task.clone();
                let result = results.get(&task.original_index).with_context(|| {
                    format!(
                        "Target Quality scene {} pass {pass} has no completed encode",
                        task.original_index
                    )
                })?;
                // Keep the verified prefix; replace any partially recorded
                // current pass and its dependent suffix only after full admission.
                completed_task.passes.truncate(previous_passes);
                completed_task.passes.push(QualityPass {
                    provenance: None,
                    backend: Some(actual.clone()),
                    quantizer: task.encoder.quantizer().expect("quantizer exists"),
                    scores,
                    bitrate: result.bitrate,
                    started_on: result
                        .started
                        .duration_since(std::time::UNIX_EPOCH)
                        .context("Probe encode start precedes UNIX epoch")?
                        .as_millis(),
                    completed_on: result
                        .ended
                        .duration_since(std::time::UNIX_EPOCH)
                        .context("Probe encode completion precedes UNIX epoch")?
                        .as_millis(),
                });
                Self::validate_history(
                    config,
                    task.original_index,
                    task.frame_indices.len(),
                    &completed_task.passes,
                )?;
                Ok(completed_task)
            })
            .collect::<Result<Vec<_>>>()?;

        Ok((tasks, warnings))
    }

    fn group_task_scores(
        config: &TargetQualityConfig,
        tasks: &[Task],
        mut scores: Vec<f64>,
    ) -> Result<Vec<Vec<f64>>> {
        Self::validate_task_scores(config, tasks, &scores)?;
        Ok(tasks
            .iter()
            .map(|task| scores.drain(..task.frame_indices.len()).collect())
            .collect())
    }

    fn validate_task_scores(
        config: &TargetQualityConfig,
        tasks: &[Task],
        scores: &[f64],
    ) -> Result<()> {
        let expected = tasks.iter().map(|task| task.frame_indices.len()).sum();
        validate_scores(scores, Some(expected)).with_context(|| {
            format!(
                "Target Quality {} flattened measurement",
                config.metric.friendly_name()
            )
        })?;
        let mut offset = 0;
        for task in tasks {
            let end = offset + task.frame_indices.len();
            config
                .metric
                .calculate_score(config.probing.statistic, &scores[offset..end])
                .with_context(|| {
                    format!(
                        "Target Quality {} scene {} pass {}",
                        config.metric.friendly_name(),
                        task.original_index,
                        task.passes.len() + 1
                    )
                })?;
            offset = end;
        }
        Ok(())
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

    /// Own fallback, incomplete-pass routing and relay cleanup after scoring.
    pub(super) fn route_compare_scores(
        native: Option<Result<Vec<f64>>>,
        engine: Option<Engine>,
        metric: &QualityMetric,
        compare_progress_tx: sync::mpsc::Sender<SequenceStatus>,
        compare_thread: thread::JoinHandle<Result<()>>,
        plugin: impl FnOnce(sync::mpsc::Sender<SequenceStatus>) -> Result<Vec<f64>>,
        admit: impl Fn(&[f64]) -> Result<()>,
    ) -> Result<Option<(Vec<f64>, Engine)>> {
        let is_vmaf = matches!(metric, QualityMetric::VMAF { .. });
        let temporal = metrics::scene_temporal(metric);
        let mut terminal_error = None;
        let scores = match native {
            Some(Ok(scores)) => Some(scores),
            Some(Err(error)) if is_vmaf => {
                terminal_error = Some(error);
                None
            },
            Some(Err(error))
                if error.is::<metrics::probe::Cancelled>()
                    || error.is::<metrics::SceneScoringStarted>()
                    || error.is::<MeasurementError>() =>
            {
                terminal_error = Some(error);
                None
            },
            Some(Err(error)) => {
                log_native_fallback(&error, engine, metric);
                None
            },
            None => None,
        };
        let terminal = terminal_error.is_some();
        let actual = if scores.is_some() {
            if is_vmaf {
                Engine::Vmaf
            } else {
                engine.context("native success has no engine")?
            }
        } else {
            Engine::VapourSynth
        };
        let source = if scores.is_some() || terminal {
            engine.map_or("native", Engine::as_str)
        } else {
            "vapoursynth"
        };
        let completion_tx = compare_progress_tx.clone();
        let comparison = if let Some(error) = terminal_error {
            // Disconnect before joining; retaining this sender would deadlock.
            drop(compare_progress_tx);
            Err(error)
        } else if let Some(scores) = scores {
            drop(compare_progress_tx);
            Ok(scores)
        } else {
            plugin(compare_progress_tx)
        };
        let comparison = comparison
            .and_then(|scores| {
                validate_scores(&scores, None)?;
                admit(&scores)?;
                Ok(scores)
            })
            .with_context(|| format!("{source} {} measurement", metric.friendly_name()));
        if comparison.is_ok() {
            let _ = completion_tx.send(SequenceStatus::Whole(Status::Completed {
                id: "Compare".to_owned(),
            }));
        }
        drop(completion_tx);
        let comparison = if temporal || comparison.is_err() {
            finish_temporal_compare(comparison, compare_thread)
        } else {
            // Preserve the successful non-temporal pass's fatal relay-send policy.
            compare_thread.join().expect("compare progress thread should join")?;
            comparison
        };
        match comparison {
            Err(error) if (temporal || terminal) && error.is::<metrics::probe::Cancelled>() => {
                Ok(None)
            },
            result => result.map(|scores| Some((scores, actual))),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn score_native_scenes(
        engine: Engine,
        reference: &mut av_decoders::Decoder,
        distorted: &mut av_decoders::Decoder,
        metric: &QualityMetric,
        scenes: &[&[usize]],
        progress_tx: &sync::mpsc::Sender<SequenceStatus>,
        cancelled: &AtomicBool,
    ) -> Result<Vec<f64>> {
        let total = scenes.iter().map(|scene| scene.len()).sum();
        metrics::score_scene_frames(
            engine,
            reference,
            distorted,
            metric,
            scenes,
            OutputIndexing::Compacted,
            Some(cancelled),
            |position, _| {
                report_compare_progress(progress_tx, total, position + 1);
            },
        )
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

    // A closest measurement is a fallback, not evidence that the target was met.
    pub(crate) fn measured_quantizer(
        metric: &QualityMetric,
        history: &[(f64, f64)],
    ) -> Option<f64> {
        Self::verified_quantizer(metric, history).or_else(|| {
            let target = metric.target_range().0.midpoint(metric.target_range().1);
            history
                .iter()
                .min_by(|(qa, sa), (qb, sb)| {
                    (sa - target)
                        .abs()
                        .total_cmp(&(sb - target).abs())
                        .then_with(|| qb.total_cmp(qa))
                })
                .map(|(q, _)| *q)
        })
    }

    /// Interpolates and rounds a prediction; it may already have been measured.
    /// Task admission separately selects an untested interior grid point.
    ///
    /// With 0/1 records use the configured midpoint; with 2 use linear; with 3/4
    /// use the first/second configured method; with 5+ use PCHIP over all records.
    /// Linear needs 2 points, quadratic/Fritsch–Carlson/natural need 3, PCHIP
    /// needs 2, and Catmull/Akima/cubic polynomial need 4. Catmull only covers
    /// the inner interval. Insufficient points, rejection, or nonfinite results
    /// fall back to the nearest score-bracketing linear pair, then midpoint.
    /// Duplicate scores (including flat histories) or targets outside the observed
    /// score range use midpoint without pruning records or extrapolating.
    /// Finite estimates are rounded to the step and clamped to the requested range.
    ///
    /// # Errors
    /// Rejects nonfinite targets or history quantizers/scores before sorting or
    /// prediction, including empty/single histories. Also rejects nonfinite/reversed
    /// ranges, nonpositive/nonfinite steps, and grids whose tick magnitudes exceed
    /// 2^52 - 1, before rounding or clamping.
    #[inline]
    pub fn predict_quantizer(
        quantizer_range: (f64, f64),
        target_score: f64,
        interpolators: (InterpolationMethod, InterpolationMethod),
        quantizer_score_history: &[(f64, f64)],
        step: f64,
    ) -> Result<f64> {
        Self::validate_quantizer_grid(quantizer_range, step)?;
        ensure!(target_score.is_finite(), "target score must be finite");
        for (index, (quantizer, score)) in quantizer_score_history.iter().enumerate() {
            ensure!(
                quantizer.is_finite(),
                "history[{index}].quantizer must be finite"
            );
            ensure!(score.is_finite(), "history[{index}].score must be finite");
        }
        let midpoint = quantizer_range.0.midpoint(quantizer_range.1);

        let predicted_quantizer = match quantizer_score_history.len() {
            0..=1 => midpoint,
            n => {
                // Sort history by score.
                let mut sorted = quantizer_score_history.to_vec();
                sorted.sort_by(|(_, s1), (_, s2)| s1.total_cmp(s2));

                let (scores, quantizers): (Vec<f64>, Vec<f64>) =
                    sorted.iter().map(|(q, s)| (*s, *q)).unzip();

                if scores.windows(2).any(|pair| pair[0] == pair[1])
                    || !(scores[0]..=scores[n - 1]).contains(&target_score)
                {
                    midpoint
                } else {
                    let linear = || {
                        let k = scores
                            .windows(2)
                            .position(|pair| pair[0] <= target_score && target_score <= pair[1])?;
                        interpolators::linear(
                            scores[k..k + 2].try_into().ok()?,
                            quantizers[k..k + 2].try_into().ok()?,
                            target_score,
                        )
                    };
                    let method = match n {
                        2 => InterpolationMethod::Linear,
                        3 => interpolators.0,
                        4 => interpolators.1,
                        _ => InterpolationMethod::Pchip,
                    };
                    let result = match method {
                        InterpolationMethod::Linear => linear(),
                        InterpolationMethod::Quadratic | InterpolationMethod::FritschCarlson => {
                            // Use a contiguous triple containing the target, not always the first.
                            let k = usize::from(n == 4 && target_score > scores[2]);
                            let s = scores[k..k + 3].try_into()?;
                            let q = quantizers[k..k + 3].try_into()?;
                            if method == InterpolationMethod::Quadratic {
                                interpolators::quadratic(s, q, target_score)
                            } else {
                                interpolators::fritsch_carlson(s, q, target_score)
                            }
                        },
                        InterpolationMethod::Natural => {
                            interpolators::natural_cubic_spline(&scores, &quantizers, target_score)
                        },
                        InterpolationMethod::Pchip => {
                            interpolators::pchip(&scores, &quantizers, target_score)
                        },
                        InterpolationMethod::Catmull
                        | InterpolationMethod::Akima
                        | InterpolationMethod::CubicPolynomial
                            if n < 4 =>
                        {
                            None
                        },
                        InterpolationMethod::Catmull => interpolators::catmull_rom(
                            scores[..4].try_into()?,
                            quantizers[..4].try_into()?,
                            target_score,
                        ),
                        InterpolationMethod::Akima => interpolators::akima(
                            scores[..4].try_into()?,
                            quantizers[..4].try_into()?,
                            target_score,
                        ),
                        InterpolationMethod::CubicPolynomial => interpolators::cubic_polynomial(
                            scores[..4].try_into()?,
                            quantizers[..4].try_into()?,
                            target_score,
                        ),
                    };

                    result
                        .filter(|q| q.is_finite())
                        .or_else(|| linear().filter(|q| q.is_finite()))
                        .unwrap_or_else(|| {
                            trace!("Interpolation failed, falling back to configured midpoint");
                            midpoint
                        })
                }
            },
        };

        // Round the result of the interpolation to the nearest step.
        Ok(((predicted_quantizer / step).round() * step)
            .clamp(quantizer_range.0, quantizer_range.1))
    }
}

impl Default for TargetQuality {
    #[inline]
    fn default() -> Self {
        Self {
            input: None,
            metric_input: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Task {
    pub original_index: usize,
    pub frame_indices: Vec<usize>,
    pub encoder: Encoder,
    pub output: PathBuf,
    pub passes: Vec<QualityPass>,
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

/// The UI needs a zero report to switch from encoding to comparing. Emit it
/// with the first accepted frame; a closed UI receiver is not a scoring error.
fn report_compare_progress(
    progress_tx: &sync::mpsc::Sender<SequenceStatus>,
    total: usize,
    completed: usize,
) {
    if completed == 1 {
        let _ = progress_tx.send(SequenceStatus::Whole(Status::Processing {
            id: "Compare".to_owned(),
            completion: SequenceCompletion::Frames {
                completed: 0,
                total: total as u64,
            },
        }));
    }
    let _ = progress_tx.send(SequenceStatus::Whole(Status::Processing {
        id: "Compare".to_owned(),
        completion: SequenceCompletion::Frames {
            completed: completed as u64,
            total: total as u64,
        },
    }));
}

pub(super) fn log_native_fallback(
    error: &anyhow::Error,
    engine: Option<Engine>,
    metric: &QualityMetric,
) {
    // The unsupported field/value may be hidden behind several anyhow contexts.
    error!(
        error = %format_args!("{error:#}"),
        engine = ?engine.map(Engine::as_str),
        metric = metric.friendly_name(),
        "native metric scoring failed, falling back to the VapourSynth plugin"
    );
}

#[cfg(test)]
mod tests;
