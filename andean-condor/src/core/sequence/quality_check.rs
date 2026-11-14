use std::{
    sync::{self, Arc, atomic::AtomicBool},
    thread,
    time::SystemTime,
};

use anyhow::{Context, Result, bail, ensure};
use thiserror::Error;
use tracing::error;

use crate::models::sequence::provenance::{self, MetricBackend, ResultProvenance};
use crate::{
    core::{
        Condor,
        input::Input,
        sequence::{Sequence, SequenceCompletion, SequenceDetails, SequenceStatus, Status},
    },
    metrics::{self, Engine, OutputIndexing},
    models::{
        input::{ImportMethod, Input as InputModel},
        sequence::{
            SequenceConfigHandler, SequenceDataHandler,
            quality_check::{
                QualityCheckConfig, QualityCheckConfigHandler, QualityCheckDataHandler,
            },
            scene_concatenator::SceneConcatenatorConfigHandler,
            target_quality::types::{
                MeasurementError, QualityMetric, QualityPass, validate_scores,
            },
        },
    },
    vapoursynth::{
        VapourSynthError, get_core,
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
    name: "Quality Check",
    description: "Measure the quality of the video per scene",
    version: "0.0.1",
};

/// Compare the reference against the effective final mux destination.
/// Sequence configs provide both QualityCheckConfigHandler and SceneConcatenatorConfigHandler.
pub struct QualityCheck {
    pub input: Option<Input>,
}

pub(super) fn log_native_fallback(error: &anyhow::Error, engine: Engine, metric: &QualityMetric) {
    // The unsupported field/value may be hidden behind several anyhow contexts.
    error!(
        error = %format_args!("{error:#}"),
        engine = engine.as_str(),
        metric = metric.friendly_name(),
        "native metric scoring failed, falling back to the VapourSynth plugin"
    );
}

/// Share the existing count relay; callers own disconnection and finalization.
pub(super) fn spawn_compare_progress(
    progress_tx: sync::mpsc::Sender<SequenceStatus>,
) -> (
    sync::mpsc::Sender<SequenceStatus>,
    thread::JoinHandle<Result<()>>,
) {
    let (compare_progress_tx, compare_progress_rx) = sync::mpsc::channel();
    let compare_thread = thread::spawn(move || -> Result<()> {
        for progress in compare_progress_rx {
            match progress {
                SequenceStatus::Whole(Status::Processing {
                    completion: SequenceCompletion::Frames { completed, total },
                    ..
                }) => {
                    progress_tx.send(SequenceStatus::Whole(Status::Processing {
                        id: "Compare".to_owned(),
                        completion: SequenceCompletion::Frames { completed, total },
                    }))?;
                },
                SequenceStatus::Whole(Status::Completed { id }) if id == "Compare" => {
                    progress_tx.send(SequenceStatus::Whole(Status::Completed {
                        id: "Compare".to_owned(),
                    }))?;
                },
                _ => (),
            }
        }
        Ok(())
    });
    (compare_progress_tx, compare_thread)
}

pub(super) fn finish_temporal_compare<T>(
    comparison: Result<T>,
    compare_thread: thread::JoinHandle<Result<()>>,
) -> Result<T> {
    let relay = compare_thread.join().expect("compare progress thread should join");
    // Join even on comparison failure, but a disconnected advisory UI must not
    // replace a score error, cancellation, or an otherwise successful result.
    let value = comparison?;
    if let Err(error) = relay
        && !error.is::<sync::mpsc::SendError<SequenceStatus>>()
    {
        return Err(error);
    }
    Ok(value)
}

impl<DataHandler, ConfigHandler> Sequence<DataHandler, ConfigHandler> for QualityCheck
where
    DataHandler: SequenceDataHandler + QualityCheckDataHandler,
    ConfigHandler:
        SequenceConfigHandler + QualityCheckConfigHandler + SceneConcatenatorConfigHandler,
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
        let mut warnings = vec![];

        if let Some(config) = condor.sequence_config.quality_check()? {
            for scene in &condor.scenes {
                config.strategy.validate_range(scene.start_frame, scene.end_frame)?;
            }
        }
        if condor.scenes.is_empty() {
            warnings.push(anyhow::Error::new(QualityCheckError::ScenesEmpty));
        }
        let output_path = condor
            .sequence_config
            .scene_concatenator()?
            .effective_output(&condor.output.path);
        if !output_path.exists() {
            warnings.push(anyhow::Error::new(QualityCheckError::OutputMissing {
                path: output_path.to_path_buf(),
            }));
        }
        // Quality Check compares the reference input against the encoded
        // output frame-for-frame, so a picture-altering filter makes the two
        // sides incomparable. Harmless filters still score fine, so this is a
        // warning rather than a failure.
        if condor.scenes.iter().any(|scene| scene.encoder.ffmpeg_filter().is_some()) {
            warnings.push(anyhow::Error::new(QualityCheckError::FilteredEncoderOutput));
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

        let Some(config) = condor.sequence_config.quality_check()? else {
            return Ok(((), warnings));
        };
        if condor.scenes.is_empty() {
            warnings.push(anyhow::Error::new(QualityCheckError::ScenesEmpty));
            return Ok(((), warnings));
        }
        for scene in &condor.scenes {
            config.strategy.validate_range(scene.start_frame, scene.end_frame)?;
        }
        let input = if let Some(input) = self.input.as_mut() {
            input
        } else if let Some(input_model) = &config.input {
            &mut Input::from_data(input_model)?
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

        Ok(((), warnings))
    }

    #[inline]
    fn execute(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        progress_tx: sync::mpsc::Sender<SequenceStatus>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<((), Vec<anyhow::Error>)> {
        crate::utils::cancellation::Cancellation::from(Arc::clone(&cancelled)).check()?;
        let mut warnings = vec![];

        let Some(config) = condor.sequence_config.quality_check()? else {
            return Ok(((), warnings));
        };

        if condor.scenes.is_empty() {
            warnings.push(anyhow::Error::new(QualityCheckError::ScenesEmpty));
            return Ok(((), warnings));
        }
        for scene in &condor.scenes {
            config.strategy.validate_range(scene.start_frame, scene.end_frame)?;
        }
        let output_path = condor
            .sequence_config
            .scene_concatenator()?
            .effective_output(&condor.output.path)
            .to_path_buf();
        let input_model = self
            .input
            .as_ref()
            .map(Input::as_data)
            .or_else(|| config.input.clone())
            .unwrap_or_else(|| condor.input.as_data());
        let mut admission = provenance::Admission::default();
        for (index, scene) in condor.scenes.iter().enumerate() {
            admission.admit_quality(
                &input_model,
                &output_path,
                scene,
                Some(config),
                &scene.sequence_data.get_quality_check()?.quality,
                index,
            )?;
        }
        if !output_path.exists() {
            warnings.push(anyhow::Error::new(QualityCheckError::OutputMissing {
                path: output_path,
            }));
            return Ok(((), warnings));
        }
        let scene_frame_indices = condor
            .scenes
            .iter()
            .enumerate()
            .map(|(index, scene)| {
                Ok((
                    index,
                    config.strategy.frame_indices(scene.start_frame, scene.end_frame)?,
                ))
            })
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .map(|(index, indices)| -> Result<Option<_>> {
                if let Ok(data) = condor.scenes[index].sequence_data.get_quality_check()
                    && data.quality.scores.len() == indices.len()
                    && !data.quality.is_unmeasured()
                {
                    Self::validate_scene_scores(config, index, indices.len(), &data.quality.scores)
                        .context("recorded Quality Check history")?;
                    return Ok(None);
                }
                Ok(Some((index, indices)))
            })
            .filter_map(Result::transpose)
            .collect::<Result<Vec<_>>>()?;
        let input = if let Some(input) = self.input.as_mut() {
            input
        } else if let Some(input_model) = &config.input {
            &mut Input::from_data(input_model)?
        } else {
            &mut condor.input
        };

        if scene_frame_indices.is_empty() {
            progress_tx.send(SequenceStatus::Whole(Status::Completed {
                id: DETAILS.name.to_owned(),
            }))?;
            return Ok(((), warnings));
        }

        // VMAF scores decoded frames through libvmaf rather than a VapourSynth
        // filter graph, so it takes a separate path. Every other metric runs as
        // a plugin over the nodes built by `measure`, unless a native
        // engine takes it first.
        let before = admission.measurement_inputs(&input_model, None, Some(&config.metric))?;
        let output_before = provenance::file_identity(&output_path)?;
        let planned = metrics::engine(&config.metric, Self::input_is_vapoursynth(input));
        let (measurements, actual) = if matches!(config.metric, QualityMetric::VMAF { .. }) {
            (
                Self::measure_vmaf(
                    input,
                    &output_path,
                    config,
                    &scene_frame_indices,
                    &progress_tx,
                    &cancelled,
                )?,
                MetricBackend::current(Engine::Vmaf),
            )
        } else if let Some(scores) = Self::measure_native(
            input,
            &output_path,
            config,
            &scene_frame_indices,
            &progress_tx,
            &cancelled,
        )? {
            (
                scores,
                MetricBackend::current(planned.expect("successful native scorer")),
            )
        } else {
            Self::measure(
                input,
                &output_path,
                config,
                &scene_frame_indices,
                &progress_tx,
                &cancelled,
            )?
        };

        let mut publication = provenance::Admission::default();
        ensure!(
            before == publication.measurement_inputs(&input_model, None, Some(&config.metric))?
                && output_before == provenance::file_identity(&output_path)?,
            "Quality Check inputs changed during measurement; recompute or use a new job"
        );
        let mut staged = condor.scenes.clone();
        Self::attach_measurements(&mut staged, config, &scene_frame_indices, measurements)?;
        for (index, _) in &scene_frame_indices {
            let backend = actual.clone();
            let context = publication.quality_context(
                &input_model,
                &output_path,
                &staged[*index],
                config,
                &backend,
            )?;
            let quality = &mut staged[*index].sequence_data.get_quality_check_mut()?.quality;
            quality.backend = Some(backend);
            quality.provenance = Some(ResultProvenance::new(context));
        }
        let mut data = condor.as_data();
        data.scenes = staged.clone();
        (condor.save_callback)(data).context("Saving measured Quality Check results")?;
        condor.scenes = staged;

        Ok(((), warnings))
    }
}

impl QualityCheck {
    fn validate_scene_scores(
        config: &QualityCheckConfig,
        index: usize,
        expected: usize,
        scores: &[f64],
    ) -> Result<()> {
        validate_scores(scores, Some(expected))
            .and_then(|()| config.metric.calculate_score(config.statistic, scores).map(|_| ()))
            .with_context(|| {
                format!(
                    "Quality Check {} scene {index} pass 1",
                    config.metric.friendly_name()
                )
            })
    }

    fn attach_measurements<Data: SequenceDataHandler + QualityCheckDataHandler>(
        scenes: &mut [crate::models::scene::Scene<Data>],
        config: &QualityCheckConfig,
        selection: &[(usize, Vec<usize>)],
        measurements: Vec<SceneMeasurement>,
    ) -> Result<()> {
        Self::validate_measurements(config, selection, &measurements)?;
        // Stage handler changes too: a failing mutable accessor cannot half-attach a batch.
        let mut staged = scenes.to_vec();
        for ((index, _), measurement) in selection.iter().zip(measurements) {
            let quality_pass = QualityPass {
                provenance: None,
                backend: None,
                quantizer: 0.0,
                scores: measurement.scores,
                bitrate: 0.0,
                started_on: measurement.started_on,
                completed_on: measurement.completed_on,
            };
            staged[*index].sequence_data.get_quality_check_mut()?.quality = quality_pass;
        }
        scenes.clone_from_slice(&staged);
        Ok(())
    }

    fn validate_measurements(
        config: &QualityCheckConfig,
        selection: &[(usize, Vec<usize>)],
        measurements: &[SceneMeasurement],
    ) -> Result<()> {
        ensure!(
            measurements.len() == selection.len(),
            MeasurementError::Count {
                expected: selection.len(),
                actual: measurements.len()
            }
        );
        for ((index, indices), measurement) in selection.iter().zip(measurements) {
            Self::validate_scene_scores(config, *index, indices.len(), &measurement.scores)?;
        }
        Ok(())
    }

    fn record_plugin_score(
        config: &QualityCheckConfig,
        selection: &[(usize, Vec<usize>)],
        state: &mut PluginSceneState,
        local_index: usize,
        score: f64,
        progress_tx: &sync::mpsc::Sender<SequenceStatus>,
    ) -> Result<()> {
        let total = selection.iter().map(|(_, indices)| indices.len()).sum();
        ensure!(
            local_index < total,
            MeasurementError::Position {
                position: local_index,
                expected: total
            }
        );
        let mut offset = 0;
        let (scene, (index, indices)) = selection
            .iter()
            .enumerate()
            .find(|(_, (_, indices))| {
                if local_index < offset + indices.len() {
                    true
                } else {
                    offset += indices.len();
                    false
                }
            })
            .expect("bounds checked selection position");
        crate::models::sequence::target_quality::types::validate_score(score, local_index)
            .with_context(|| {
                format!(
                    "vapoursynth Quality Check {} scene {index} pass 1",
                    config.metric.friendly_name()
                )
            })?;
        let (scores, received, started_on, completed_on) = state;
        let position = local_index - offset;
        ensure!(
            scores[scene][position].is_none(),
            MeasurementError::Duplicate {
                position: local_index
            }
        );
        if received[scene] + 1 == indices.len() {
            let proposed = scores[scene]
                .iter()
                .enumerate()
                .map(|(slot, value)| {
                    if slot == position {
                        score
                    } else {
                        value.expect("other slots filled")
                    }
                })
                .collect::<Vec<_>>();
            Self::validate_scene_scores(config, *index, indices.len(), &proposed)?;
        }
        let now = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("Time is valid")
            .as_millis();
        if received[scene] == 0 {
            started_on[scene] = now;
        }
        scores[scene][position] = Some(score);
        received[scene] += 1;
        if received[scene] == indices.len() {
            completed_on[scene] = now;
        }
        // Scene summaries wait for admission of the entire batch, not just this callback.
        Self::report_frame_score(
            &[indices[local_index - offset]],
            total,
            0,
            score,
            local_index + 1,
            progress_tx,
        );
        Ok(())
    }
}

impl QualityCheck {
    pub const DETAILS: SequenceDetails = DETAILS;

    #[inline]
    pub fn new(input: Option<Input>) -> Self {
        Self { input }
    }

    fn measure_vmaf(
        input: &mut Input,
        output_path: &std::path::Path,
        config: &QualityCheckConfig,
        scene_frame_indices: &[(usize, Vec<usize>)],
        progress_tx: &sync::mpsc::Sender<SequenceStatus>,
        cancelled: &AtomicBool,
    ) -> Result<Vec<SceneMeasurement>> {
        let (total_frames, selected) = Self::native_selection(scene_frame_indices);

        let reference = input.decoder();
        let mut distorted = Input::from_video(&InputModel::Video {
            path: output_path.to_path_buf(),
            import_method: ImportMethod::FFMS2 {
                index: None,
                cache_path: None,
            },
            // The encoded output is already in the input's format, so it must
            // not be converted again.
            filters: Vec::new(),
        })?;

        // Each score is reported as the metric produces it, mapped back to its
        // global frame number so the UI's per-frame report lines up with the
        // clip.
        let scores = metrics::score_vmaf_frames_with_progress(
            reference,
            distorted.decoder(),
            &config.metric,
            &selected,
            OutputIndexing::Aligned,
            Some(cancelled),
            |position| Self::report_vmaf_submission(total_frames, position, progress_tx),
            |position, score| {
                Self::report_frame_score(
                    &selected,
                    total_frames,
                    position,
                    score,
                    total_frames,
                    progress_tx,
                );
            },
        )?;

        let measurements = Self::group_native_scores(&scores, total_frames, scene_frame_indices)
            .context("vmaf VMAF Quality Check pass 1")?;
        Self::validate_measurements(config, scene_frame_indices, &measurements)
            .context("vmaf VMAF Quality Check pass 1")?;
        Ok(measurements)
    }

    /// Submission drives progress while native temporal scores are unavailable.
    fn report_vmaf_submission(
        total_frames: usize,
        position: usize,
        progress_tx: &sync::mpsc::Sender<SequenceStatus>,
    ) {
        let _ = progress_tx.send(SequenceStatus::Whole(Status::Processing {
            id: DETAILS.name.to_owned(),
            completion: SequenceCompletion::Custom {
                name: DETAILS.name.to_owned(),
                completed: (position + 1) as f64,
                total: total_frames as f64,
            },
        }));
    }

    /// Report one frame's score as it is produced.
    ///
    /// `position` indexes the selection, which is what the metric reports; the
    /// global frame number is what the UI displays.
    #[inline]
    fn report_frame_score(
        selected: &[usize],
        total_frames: usize,
        position: usize,
        score: f64,
        completed: usize,
        progress_tx: &sync::mpsc::Sender<SequenceStatus>,
    ) {
        // Progress is advisory: a closed receiver means the UI went away, which
        // is not a scoring failure.
        let _ = progress_tx.send(SequenceStatus::Subprocess {
            parent: Status::Processing {
                id: DETAILS.name.to_owned(),
                completion: SequenceCompletion::Custom {
                    name: DETAILS.name.to_owned(),
                    completed: completed as f64,
                    total: total_frames as f64,
                },
            },
            child: Status::Processing {
                id: "Quality".to_owned(),
                completion: SequenceCompletion::FrameScore {
                    frame: selected[position] as u64,
                    score,
                },
            },
        });
    }

    fn report_scene_score(
        config: &QualityCheckConfig,
        index: usize,
        scores: &[f64],
        completed: usize,
        total: usize,
        progress_tx: &sync::mpsc::Sender<SequenceStatus>,
    ) -> Result<()> {
        let _ = progress_tx.send(SequenceStatus::Subprocess {
            parent: Status::Processing {
                id: DETAILS.name.to_owned(),
                completion: SequenceCompletion::Custom {
                    name: DETAILS.name.to_owned(),
                    completed: completed as f64,
                    total: total as f64,
                },
            },
            child: Status::Processing {
                id: "Quality".to_owned(),
                completion: SequenceCompletion::SceneQuality {
                    index: index as u64,
                    quantizer: 0.0,
                    score: config.metric.calculate_score(config.statistic, scores)?,
                    bitrate: 0.0,
                },
            },
        });
        Ok(())
    }

    /// Group the per-frame scores back into scenes, recording when each span
    /// was measured so the UI can show per-scene progress.
    #[inline]
    fn group_native_scores(
        scores: &[f64],
        total_frames: usize,
        scene_frame_indices: &[(usize, Vec<usize>)],
    ) -> Result<Vec<SceneMeasurement>> {
        if total_frames == 0 && scores.is_empty() && scene_frame_indices.is_empty() {
            return Ok(Vec::new());
        }
        validate_scores(scores, Some(total_frames))
            .context("Quality Check flattened measurement")?;
        let expected = scene_frame_indices.iter().map(|(_, indices)| indices.len()).sum();
        ensure!(
            total_frames == expected,
            MeasurementError::Count {
                expected,
                actual: total_frames
            }
        );

        let now = || {
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("Time is valid")
                .as_millis()
        };

        let mut measurements = Vec::with_capacity(scene_frame_indices.len());
        let mut consumed = 0;
        let mut previous_end = now();
        for (_, indices) in scene_frame_indices {
            let end = consumed + indices.len();
            let completed_on = now();
            measurements.push(SceneMeasurement {
                scores: scores[consumed..end].to_vec(),
                started_on: previous_end,
                completed_on,
            });
            previous_end = completed_on;
            consumed = end;
        }

        Ok(measurements)
    }

    /// Score the selection with whichever native engine is preferred, or report
    /// that the native path does not apply.
    ///
    /// Returns `Ok(None)` when the caller should use its VapourSynth branch:
    /// the metric has no native engine, no engine's library is usable, or
    /// scoring failed before temporal frame progress. Failures after that
    /// progress propagate instead of duplicating callbacks through a fallback.
    /// The plugin checks its own enabled-temporal CVVDP gap capabilities.
    fn measure_native(
        input: &mut Input,
        output_path: &std::path::Path,
        config: &QualityCheckConfig,
        scene_frame_indices: &[(usize, Vec<usize>)],
        progress_tx: &sync::mpsc::Sender<SequenceStatus>,
        cancelled: &AtomicBool,
    ) -> Result<Option<Vec<SceneMeasurement>>> {
        let Some(engine) = metrics::engine(&config.metric, Self::input_is_vapoursynth(input))
        else {
            return Ok(None);
        };

        let scenes: Vec<_> =
            scene_frame_indices.iter().map(|(_, indices)| indices.as_slice()).collect();
        let total_frames = if metrics::scene_temporal(&config.metric) {
            metrics::probe::validate_scene_selection(&scenes)?
        } else {
            scenes.iter().map(|indices| indices.len()).sum()
        };
        if total_frames == 0 {
            return Self::group_native_scores(&[], 0, scene_frame_indices).map(Some);
        }

        let reference = input.decoder();
        let mut distorted = Input::from_video(&InputModel::Video {
            path: output_path.to_path_buf(),
            import_method: ImportMethod::FFMS2 {
                index: None,
                cache_path: None,
            },
            // The encoded output is already in the input's format, so it must
            // not be converted again.
            filters: Vec::new(),
        })?;

        // Each score is reported as the engine produces it.
        let scores = Self::score_native_scenes(
            engine,
            reference,
            distorted.decoder(),
            &config.metric,
            &scenes,
            progress_tx,
            cancelled,
        );

        let scores = match scores {
            Ok(scores) => scores,
            // A cancelled pass must not be retried through the plugin, which
            // would ignore the abort and score the whole selection again.
            Err(error)
                if error.is::<metrics::probe::Cancelled>()
                    || error.is::<metrics::SceneScoringStarted>()
                    || error.is::<MeasurementError>() =>
            {
                return Err(error);
            },
            Err(error) => {
                // The engine is named because the failure is usually
                // library-specific and the fallback is otherwise silent.
                log_native_fallback(&error, engine, &config.metric);
                return Ok(None);
            },
        };

        let measurements = Self::group_native_scores(&scores, total_frames, scene_frame_indices)
            .with_context(|| {
                format!(
                    "{engine} {} Quality Check pass 1",
                    config.metric.friendly_name()
                )
            })?;
        Self::validate_measurements(config, scene_frame_indices, &measurements).with_context(
            || {
                format!(
                    "{engine} {} Quality Check pass 1",
                    config.metric.friendly_name()
                )
            },
        )?;
        Ok(Some(measurements))
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
        let selected: Vec<_> = scenes.iter().flat_map(|scene| scene.iter().copied()).collect();
        metrics::score_scene_frames(
            engine,
            reference,
            distorted,
            metric,
            scenes,
            OutputIndexing::Aligned,
            Some(cancelled),
            |position, score| {
                Self::report_frame_score(
                    &selected,
                    selected.len(),
                    position,
                    score,
                    position + 1,
                    progress_tx,
                );
            },
        )
    }

    /// Whether the reference side is a VapourSynth graph rather than a native
    /// decode, which decides whether fmetrics outranks the plugin.
    #[inline]
    fn input_is_vapoursynth(input: &Input) -> bool {
        !matches!(input, Input::Video { .. })
    }

    /// The frames a native pass scores, and how many that is.
    ///
    /// The selection is flattened across scenes in ascending order, which is
    /// the order both the native driver and the VapourSynth trimming walk.
    #[inline]
    fn native_selection(scene_frame_indices: &[(usize, Vec<usize>)]) -> (usize, Vec<usize>) {
        let total_frames = scene_frame_indices.iter().map(|(_, indices)| indices.len()).sum();
        let selected = scene_frame_indices
            .iter()
            .flat_map(|(_, indices)| indices.iter().copied())
            .collect();

        (total_frames, selected)
    }

    #[inline]
    #[allow(clippy::too_many_arguments)]
    fn measure(
        input: &mut Input,
        output_path: &std::path::Path,
        config: &QualityCheckConfig,
        scene_frame_indices: &[(usize, Vec<usize>)],
        progress_tx: &sync::mpsc::Sender<SequenceStatus>,
        cancelled: &Arc<AtomicBool>,
    ) -> Result<(Vec<SceneMeasurement>, MetricBackend)> {
        let temporal = metrics::scene_temporal(&config.metric);
        let scenes: Vec<_> =
            scene_frame_indices.iter().map(|(_, indices)| indices.as_slice()).collect();
        if temporal && metrics::probe::validate_scene_selection(&scenes)? == 0 {
            return Self::group_native_scores(&[], 0, scene_frame_indices)
                .map(|measurements| (measurements, MetricBackend::current(Engine::VapourSynth)));
        }
        // Owned, so it outlives the `&mut v_input` borrow taken below.
        let mut v_input_owned = input.as_vapoursynth_script()?;
        let v_input = v_input_owned.as_mut();
        let decoder = match input {
            Input::VapourSynth { decoder, .. } | Input::VapourSynthScript { decoder, .. } => {
                decoder
            },
            Input::Video { .. } => v_input.expect("Video Input exists").decoder(),
        };
        let vapoursynth_decoder = decoder.get_vapoursynth_impl().expect("Decoder is VapourSynth");
        let env = &vapoursynth_decoder.env;
        let reference_node = vapoursynth_decoder.get_output(
            vapoursynth_decoder.get_output_index(),
            vapoursynth_decoder.get_node_modifier(),
        )?;
        let core = get_core(env)?;
        let backend = MetricBackend::plugin_for_metric(core, &config.metric)?;

        let reference_node = if temporal {
            reference_node
        } else {
            let frame_nodes: Vec<_> = scene_frame_indices
                .iter()
                .map(|(_, indices)| {
                    indices
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
        let distorted_node = {
            let source_node = Source {
                source: output_path.to_path_buf(),
                ..Default::default()
            }
            .invoke(core)?;
            if temporal {
                source_node
            } else {
                let frame_nodes: Vec<_> = scene_frame_indices
                    .iter()
                    .map(|(_, indices)| {
                        indices
                            .iter()
                            .map(|index| {
                                Trim {
                                    first: Some(*index as u32),
                                    last: Some(*index as u32),
                                    ..Default::default()
                                }
                                .invoke(core, &source_node)
                            })
                            .collect::<Result<Vec<_>, _>>()
                    })
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .flatten()
                    .collect();

                Splice::invoke(core, &frame_nodes)?
            }
        };

        let total_frames = scene_frame_indices.iter().map(|(_, indices)| indices.len()).sum();

        let (compare_progress_tx, compare_thread) = spawn_compare_progress(progress_tx.clone());

        // Live per-frame score events
        let scene_count = scene_frame_indices.len();
        let scene_state = Arc::new(sync::Mutex::new((
            scene_frame_indices
                .iter()
                .map(|(_, indices)| vec![None; indices.len()])
                .collect::<Vec<_>>(),
            vec![0; scene_count],
            vec![0u128; scene_count],
            vec![0u128; scene_count],
        )));
        let on_frame = {
            let progress_tx = progress_tx.clone();
            let selection = scene_frame_indices.to_vec();
            let config = config.clone();
            let scene_state_clone = Arc::clone(&scene_state);

            move |local_index: usize, score: &f64| -> Result<(), VapourSynthError> {
                let mut scene_state =
                    scene_state_clone.lock().expect("scene state mutex should acquire lock");
                Self::record_plugin_score(
                    &config,
                    &selection,
                    &mut scene_state,
                    local_index,
                    *score,
                    &progress_tx,
                )
                .map_err(|error| VapourSynthError::Measurement {
                    source: error.into_boxed_dyn_error().into(),
                })
            }
        };

        // The SSIMULACRA2, Butteraugli and CVVDP branches are the fallback for
        // the metrics libvship also implements: they are reached only when
        // `measure_vship` found no usable libvship, or its scoring failed.
        let comparison = (|| -> Result<()> {
            match &config.metric {
                // Handled by `measure_vmaf` before this graph is built: VMAF reads
                // decoded frames through libvmaf instead of a VapourSynth filter.
                QualityMetric::VMAF { .. } => {
                    unreachable!("VMAF is measured by `measure_vmaf`, not here");
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
                        VSHIPSSIMULACRA2::collect_frame_values(
                            &node,
                            compare_progress_tx,
                            on_frame,
                            |frame| {
                                VSHIPSSIMULACRA2::PROPERTY_NAMES
                                .iter()
                                .find_map(|property_name| {
                                    frame.props().get_float(property_name).ok()
                                })
                                .ok_or_else(|| {
                                    VSHIPSSIMULACRA2::new_error(format!(
                                        "Score not found on any of the following properties: {}",
                                        VSHIPSSIMULACRA2::PROPERTY_NAMES.join(", ")
                                    ))
                                })
                            },
                        )?
                    } else if SSIMULACRA2::plugin_is_installed(core) {
                        let node = SSIMULACRA2::invoke(core, &reference_node, &distorted_node)?;
                        SSIMULACRA2::collect_frame_values(
                            &node,
                            compare_progress_tx,
                            on_frame,
                            |frame| {
                                SSIMULACRA2::PROPERTY_NAMES
                                .iter()
                                .find_map(|property_name| {
                                    frame.props().get_float(property_name).ok()
                                })
                                .ok_or_else(|| {
                                    SSIMULACRA2::new_error(format!(
                                        "Score not found on any of the following properties: {}",
                                        SSIMULACRA2::PROPERTY_NAMES.join(", ")
                                    ))
                                })
                            },
                        )?
                    } else {
                        error!("No VapourSynth SSIMULACRA2 plugin found");
                        bail!(QualityCheckError::QualityMeasurementFailed);
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
                    let property_names = norm.and_then(|_| Some(BUTTERAUGLI::QNORM_PROPERTY_NAMES));
                    let property_names = property_names.unwrap_or(BUTTERAUGLI::PROPERTY_NAMES);
                    BUTTERAUGLI::collect_frame_values(
                        &node,
                        compare_progress_tx,
                        on_frame,
                        move |frame| {
                            property_names
                                .iter()
                                .find_map(|property_name| {
                                    frame.props().get_float(property_name).ok()
                                })
                                .ok_or_else(|| {
                                    BUTTERAUGLI::new_error(format!(
                                        "Score not found on any of the following properties: {}",
                                        property_names.join(", ")
                                    ))
                                })
                        },
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
                    XPSNR::collect_frame_values(&node, compare_progress_tx, on_frame, |frame| {
                        let plane_scores = XPSNR::PROPERTY_NAMES
                            .iter()
                            .map(|property_name| {
                                frame.props().get_float(property_name).map_err(|error| {
                                    XPSNR::new_error(format!(
                                        "Score not found on required property \"{property_name}\": \
                                     {error}"
                                    ))
                                })
                            })
                            .collect::<Result<Vec<f64>, _>>()?;
                        match plane_scores.as_slice() {
                            [y, u, v] => Ok(XPSNR::weight_xpsnr(*y, *u, *v)),
                            _ => Err(XPSNR::new_error(
                                "XPSNR should return three plane scores".to_owned(),
                            )),
                        }
                    })?
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
                            OutputIndexing::Aligned,
                            compare_progress_tx,
                            cancelled,
                            on_frame,
                        )?
                    } else {
                        let node = plugin.invoke(
                            core,
                            &reference_node,
                            &distorted_node,
                            // Preserve source gaps; node indices became contiguous when spliced.
                            scene_frame_indices
                                .iter()
                                .flat_map(|(_, indices)| indices.iter().copied()),
                        )?;
                        CVVDP::collect_frame_values(
                            &node,
                            compare_progress_tx,
                            on_frame,
                            |frame| {
                                CVVDP::PROPERTY_NAMES
                            .iter()
                            .find_map(|property_name| frame.props().get_float(property_name).ok())
                            .ok_or_else(|| {
                                CVVDP::new_error(format!(
                                    "Score not found on any of the following properties: {}",
                                    CVVDP::PROPERTY_NAMES.join(", ")
                                ))
                            })
                            },
                        )?
                    }
                },
            };
            Ok(())
        })();
        finish_temporal_compare(comparison, compare_thread)?;

        if cancelled.load(sync::atomic::Ordering::Relaxed) {
            bail!(QualityCheckError::Cancelled);
        }

        let scene_state = scene_state.lock().expect("scene state mutex should acquire lock");
        let (scene_scores, _scene_received, scene_started_on, scene_completed_on) = &*scene_state;
        let measurements = (0..scene_frame_indices.len())
            .map(|index| {
                Ok(SceneMeasurement {
                    scores: scene_scores[index]
                        .iter()
                        .enumerate()
                        .map(|(position, score)| {
                            score.ok_or(MeasurementError::Missing { position })
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                    started_on: scene_started_on[index],
                    completed_on: scene_completed_on[index],
                })
            })
            .collect::<Result<Vec<_>>>()?;

        Self::validate_measurements(config, scene_frame_indices, &measurements)?;
        for ((index, _), measurement) in scene_frame_indices.iter().zip(&measurements) {
            Self::report_scene_score(
                config,
                *index,
                &measurement.scores,
                total_frames,
                total_frames,
                progress_tx,
            )?;
        }
        let _ = progress_tx.send(SequenceStatus::Whole(Status::Completed {
            id: "Compare".to_owned(),
        }));

        Ok((measurements, backend))
    }
}

impl Default for QualityCheck {
    #[inline]
    fn default() -> Self {
        Self { input: None }
    }
}

struct SceneMeasurement {
    scores: Vec<f64>,
    started_on: u128,
    completed_on: u128,
}

type PluginSceneState = (Vec<Vec<Option<f64>>>, Vec<usize>, Vec<u128>, Vec<u128>);

#[derive(Debug, Clone, Error)]
pub enum QualityCheckError {
    #[error("No Scenes found")]
    ScenesEmpty,
    #[error("Output file not found: {path}")]
    OutputMissing { path: std::path::PathBuf },
    #[error(
        "Quality Check compares unfiltered reference frames against the Parallel Encoder's \
         filtered output; picture-altering filters (crop, invert, ...) make the comparison \
         unrepresentative"
    )]
    FilteredEncoderOutput,
    #[error("Failed to measure quality")]
    QualityMeasurementFailed,
    #[error("Cancelled")]
    Cancelled,
}

#[cfg(test)]
mod tests;
