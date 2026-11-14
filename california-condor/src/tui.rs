use std::{
    io::{IsTerminal, Write},
    sync::{Arc, atomic::AtomicBool},
    thread,
    time::{Duration, Instant},
};

use andean_condor::{
    core::{
        Condor,
        input::{Input, OpenProgress, clip_info::ClipInfo},
        sequence::{
            Sequence, benchmarker::Benchmarker, bitrate_optimizer::BitrateOptimizer,
            noise_detector::NoiseDetector, noise_scaler::NoiseScaler,
            parallel_encoder::ParallelEncoder, quality_check::QualityCheck,
            scene_concatenator::SceneConcatenator, scene_detector::SceneDetector,
            speed_scaler::SpeedScaler, target_quality::TargetQuality,
        },
    },
    models::{
        encoder::Encoder, input::Input as InputModel, scene::Scene,
        sequence::target_quality::TargetQualityConfig,
    },
};
use anyhow::{Context, Result, bail};
use thiserror::Error as ThisError;
use tracing::{debug, error, info, trace, warn};

use crate::{
    apps::{
        TuiApp,
        benchmarker::BenchmarkerApp,
        noise_detection::NoiseDetectionApp,
        parallel_encoder::{ParallelEncoderApp, SceneDisplay},
        quality_check::QualityCheckApp,
        scene_concatenator::SceneConcatenatorApp,
        scene_detection::SceneDetectionApp,
        target_quality::TargetQualityApp,
    },
    configuration::{CliSequenceConfig, CliSequenceData},
    interrupt::Interrupt,
};

#[derive(Debug, ThisError)]
#[error("{primary}; additionally, {display:#}")]
pub(crate) struct SecondaryDisplayFailure {
    primary: String,
    pub(crate) display: anyhow::Error,
}

// Execute must consume/drop its progress sender before finalization so the
// display can exit on disconnect. Join before warnings or either error escapes:
// a running display still owns the terminal's panic-restoration registration.
pub(crate) fn finish_stage(
    stage: &str,
    executed: Result<((), Vec<anyhow::Error>)>,
    join_display: impl FnOnce() -> thread::Result<Result<()>>,
) -> Result<()> {
    let displayed = join_display()
        .map_err(|payload| {
            let message = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .unwrap_or("non-string panic payload");
            andean_condor::utils::CaughtPanic {
                operation: format!("{stage} TUI thread"),
                message: message.into(),
            }
            .into()
        })
        .and_then(|result| result.with_context(|| format!("{stage} TUI thread failed")));
    match executed {
        Ok(((), processing_warnings)) => {
            for warning in processing_warnings {
                warn!("{}", warning);
            }
            displayed
        },
        Err(primary) => match displayed {
            Ok(()) => Err(primary),
            Err(display) => {
                // Context retains anyhow's typed primary downcast/cause chain;
                // its diagnostic leads with that primary, not the display error.
                let context = SecondaryDisplayFailure {
                    primary: primary.to_string(),
                    display,
                };
                Err(primary.context(context))
            },
        },
    }
}

/// Opens `data`, logging its progress, if available.
///
/// Only FFMS2 and DGDecNV report real progress. A VapourSynth input indexes
/// inside its source plugin, where no callback can reach it, so it only gets
/// the single `Opening input...` line.
///
/// Returns the input together with its probed clip info, so callers do not have
/// to reopen it just to read the header.
pub fn open_input(data: &InputModel, cancelled: &Arc<AtomicBool>) -> Result<(Input, ClipInfo)> {
    // Legacy flags are graceful-only; retain one explicit token for force support.
    open_input_cancel(
        data,
        &andean_condor::utils::cancellation::Cancellation::from(Arc::clone(cancelled)),
    )
}

/// Library callers can retain this token across opening and sequence execution.
pub fn open_input_cancel(
    data: &InputModel,
    cancellation: &andean_condor::utils::cancellation::Cancellation,
) -> Result<(Input, ClipInfo)> {
    info!("Opening input...");
    // FFMS2 reports every whole percent, so only the tens are worth a line.
    let mut last_decile = 0;
    let mut input = Input::ensure_indexed_cancel(
        data,
        |progress| {
            let OpenProgress::Indexing { current, total } = progress else {
                return;
            };
            if total == 0 {
                return;
            }
            let decile = (current * 100 / total).min(100) / 10;
            if decile > last_decile {
                last_decile = decile;
                trace!("Indexing input: {}%", decile * 10);
            }
        },
        cancellation,
    )?;
    let clip_info = input.clip_info()?;
    Ok((input, clip_info))
}

#[tracing::instrument(skip_all)]
pub fn run_scene_detector_tui(
    condor: &mut Condor<CliSequenceData, CliSequenceConfig>,
    cancelled: impl Into<Interrupt>,
) -> Result<()> {
    let cancelled = cancelled.into();
    cancelled.token.check()?;
    let initial_frames = condor.scenes.iter().fold(0, |acc, scene| {
        acc + (scene.end_frame - scene.start_frame) as u64
    });
    let initial_scenes = condor
        .scenes
        .iter()
        .map(|scene| (scene.start_frame as u64, scene.end_frame as u64))
        .collect::<Vec<_>>();

    debug!("Instantiating Scene Detector Input");
    // Without a `scene_detector.input` override there is nothing to open:
    // `SceneDetector` reads `condor.input` itself when it holds no input of
    // its own, so re-opening the same model here would only index it twice.
    let (input, clip_info) = if let Some(configured) = &condor.sequence_config.scene_detector.input
    {
        let (input, clip_info) = open_input_cancel(configured, &cancelled.token)?;
        (Some(input), clip_info)
    } else {
        (None, condor.input.clip_info()?)
    };

    if initial_frames as usize == clip_info.num_frames {
        return Ok(());
    }

    let (input, clip_info) = if clip_info.num_frames == condor.input.clip_info()?.num_frames {
        (input, clip_info)
    } else {
        let input_frames = condor.input.clip_info()?.num_frames;
        let scd_input_frames = clip_info.num_frames;
        let main_input_data = condor.input.as_data();
        let scd_input_data = condor
            .sequence_config
            .scene_detector
            .input
            .clone()
            .unwrap_or_else(|| main_input_data.clone());

        if scd_input_data.has_time_altering_filters()
            || !main_input_data.has_time_altering_filters()
        {
            bail!(TUIError::FramesMismatchWithInputFilters(
                input_frames as u64,
                scd_input_frames as u64
            ));
        }

        // Try to recover by appending the time altering filters from the input
        // to the scd input
        warn!(
            "Input and Scene Detector Input have mismatched frames and Scene Detector Input is \
             missing time altering filters such as trim or splice."
        );
        info!("Adding known time altering filters from Input to Scene Detector Input");
        let start = Instant::now();
        loop {
            let elapsed = start.elapsed();
            if elapsed >= Duration::from_secs(5) {
                break;
            }
            if std::io::stdout().is_terminal() {
                print!("\rResuming in {} seconds...", 5 - elapsed.as_secs());
                std::io::stdout().flush()?;
            } else {
                eprint!("\rResuming in {} seconds...", 5 - elapsed.as_secs());
                std::io::stderr().flush()?;
            }
            thread::sleep(Duration::from_secs(1));
        }

        let mut combined = scd_input_data;
        combined.append_input_filters(&main_input_data)?;
        let (scd_input, clip_info) = open_input_cancel(&combined, &cancelled.token)?;

        // Double check that the frames match
        if clip_info.num_frames != condor.input.clip_info()?.num_frames {
            error!("Failed to recover with time altering filters");
            bail!(TUIError::FramesMismatchWithInputFilters(
                input_frames as u64,
                scd_input_frames as u64
            ));
        }

        (Some(scd_input), clip_info)
    };

    let mut scene_detector = SceneDetector {
        input,
        method: condor.sequence_config.scene_detector.method.clone(),
    };

    debug!("Validating Scene Detector"); // Input should alrady be validated but just in case
    let (_, validation_warnings) = scene_detector.validate(condor)?;

    for warning in validation_warnings.iter() {
        warn!("{}", warning);
    }

    debug!("Initializing Scene Detector"); // Input should already be indexed but just in case
    let (init_progress_tx, _init_progress_rx) = std::sync::mpsc::channel();
    let (_, initialization_warnings) =
        scene_detector.initialize_cancel(condor, init_progress_tx, &cancelled.token)?;

    for warning in initialization_warnings.iter() {
        warn!("{}", warning);
    }

    debug!("Running Scene Detector");
    let ctrlc_cancelled = cancelled.clone();
    let (progress_tx, progress_rx) = std::sync::mpsc::channel();
    let tui_handle = thread::spawn(move || -> Result<()> {
        let mut scd_app = SceneDetectionApp::new(
            initial_frames,
            clip_info.num_frames as u64,
            initial_scenes,
            clip_info,
        );
        scd_app.run(progress_rx, ctrlc_cancelled)?;
        Ok(())
    });
    let executed = scene_detector.execute_cancel(condor, progress_tx, cancelled.token);
    finish_stage("Scene Detection", executed, || tui_handle.join())
}

#[tracing::instrument(skip_all)]
pub fn run_noise_detector_tui(
    condor: &mut Condor<CliSequenceData, CliSequenceConfig>,
    cancelled: impl Into<Interrupt>,
) -> Result<()> {
    let cancelled = cancelled.into();
    cancelled.token.check()?;
    debug!("Instantiating Noise Detector Input");
    let Some(noise_detector_config) = &condor.sequence_config.noise_detector else {
        return Ok(());
    };

    let (input, clip_info) = if let Some(input) = &noise_detector_config.input {
        let (nd_input, clip_info) = open_input_cancel(input, &cancelled.token)?;
        (Some(nd_input), clip_info)
    } else {
        (None, condor.input.clip_info()?)
    };

    let total_scenes = condor.scenes.len();
    if total_scenes == 0
        || condor.scenes.iter().all(|scene| scene.sequence_data.noise_detection.is_some())
    {
        return Ok(());
    }

    let mut noise_detector = NoiseDetector { input };

    debug!("Validating Noise Detector"); // Input should alrady be validated but just in case
    let (_, validation_warnings) = noise_detector.validate(condor)?;

    for warning in validation_warnings.iter() {
        warn!("{}", warning);
    }

    debug!("Initializing Noise Detector"); // Input should already be indexed but just in case
    let (init_progress_tx, _init_progress_rx) = std::sync::mpsc::channel();
    let (_, initialization_warnings) =
        noise_detector.initialize_cancel(condor, init_progress_tx, &cancelled.token)?;

    for warning in initialization_warnings.iter() {
        warn!("{}", warning);
    }

    debug!("Running Noise Detector");
    let mut nd_app = NoiseDetectionApp::new(&condor.scenes, clip_info);
    let ctrlc_cancelled = cancelled.clone();
    let (progress_tx, progress_rx) = std::sync::mpsc::channel();
    let tui_handle = thread::spawn(move || -> Result<()> {
        nd_app.run(progress_rx, ctrlc_cancelled)?;
        Ok(())
    });
    let executed = noise_detector.execute_cancel(condor, progress_tx, cancelled.token);
    finish_stage("Noise Detection", executed, || tui_handle.join())
}

#[tracing::instrument(skip_all)]
pub fn run_noise_scaler_tui(
    condor: &mut Condor<CliSequenceData, CliSequenceConfig>,
    cancelled: impl Into<Interrupt>,
) -> Result<()> {
    let cancelled = cancelled.into();
    cancelled.token.check()?;
    let mut noise_scaler = NoiseScaler::default();
    debug!("Validating Noise Scaler");
    let (_, validation_warnings) = noise_scaler.validate(condor)?;

    for warning in validation_warnings.iter() {
        warn!("{}", warning);
    }

    debug!("Initializing Noise Scaler");
    let (init_progress_tx, _init_progress_rx) = std::sync::mpsc::channel();
    let (_, initialization_warnings) =
        noise_scaler.initialize_cancel(condor, init_progress_tx, &cancelled.token)?;

    for warning in initialization_warnings.iter() {
        warn!("{}", warning);
    }

    debug!("Running Noise Scaler");
    let (progress_tx, _progress_rx) = std::sync::mpsc::channel();
    let (_, processing_warnings) =
        noise_scaler.execute_cancel(condor, progress_tx, cancelled.token)?;

    for warning in processing_warnings.iter() {
        warn!("{}", warning);
    }

    Ok(())
}

#[tracing::instrument(skip_all)]
pub fn run_benchmarker_tui(
    condor: &mut Condor<CliSequenceData, CliSequenceConfig>,
    cancelled: impl Into<Interrupt>,
) -> Result<()> {
    let cancelled = cancelled.into();
    cancelled.token.check()?;
    let mut benchmarker = Benchmarker::default();

    debug!("Validating Benchmarker"); // Input should alrady be validated but just in case
    let (_, validation_warnings) = benchmarker.validate(condor)?;

    for warning in validation_warnings.iter() {
        warn!("{}", warning);
    }

    debug!("Initializing Benchmarker"); // Input should already be indexed but just in case
    let (init_progress_tx, _init_progress_rx) = std::sync::mpsc::channel();
    let (_, initialization_warnings) =
        benchmarker.initialize_cancel(condor, init_progress_tx, &cancelled.token)?;

    for warning in initialization_warnings.iter() {
        warn!("{}", warning);
    }

    debug!("Running Benchmarker");
    let encoder = condor.encoder.clone();
    let clip_info = condor.input.clip_info()?;

    let ctrlc_cancelled = cancelled.clone();
    let (progress_tx, progress_rx) = std::sync::mpsc::channel();
    let tui_handle = thread::spawn(move || -> Result<()> {
        let mut benchmarker_app = BenchmarkerApp::new(encoder, clip_info);
        benchmarker_app.run(progress_rx, ctrlc_cancelled)?;
        Ok(())
    });
    let executed = benchmarker.execute_cancel(condor, progress_tx, cancelled.token);
    finish_stage("Benchmarker", executed, || tui_handle.join())
}

fn target_quality_app(
    clip_info: ClipInfo,
    scenes: &[Scene<CliSequenceData>],
    encoder: &Encoder,
    config: &TargetQualityConfig,
) -> Result<TargetQualityApp> {
    // Resolve only encoder settings, not full scenes and their measurement histories.
    let encoders: Vec<_> =
        scenes.iter().map(|scene| config.probe_encoder(&scene.encoder)).collect();
    let encoder = config.probe_encoder(encoder);
    let summary =
        crate::components::encoder_info::representative_encoder(encoders.iter(), &encoder).cloned();
    let mut app = TargetQualityApp::new(
        clip_info,
        scenes,
        encoder,
        config.probing.statistic,
        &config.metric,
    )?;
    app.encoder_summary = summary;
    Ok(app)
}

#[tracing::instrument(skip_all)]
pub fn run_target_quality_tui(
    condor: &mut Condor<CliSequenceData, CliSequenceConfig>,
    cancelled: impl Into<Interrupt>,
) -> Result<()> {
    let cancelled = cancelled.into();
    cancelled.token.check()?;
    if condor.scenes.is_empty() || condor.sequence_config.target_quality.is_none() {
        return Ok(());
    }
    TargetQuality::validate_config(condor)?;
    debug!("Instantiating Target Quality Input");
    let (target_quality_input, clip_info) = if let Some(Some(input)) =
        &condor.sequence_config.target_quality.as_ref().map(|tq| tq.input.clone())
    {
        let (tq_input, clip_info) = open_input_cancel(input, &cancelled.token)?;
        (Some(tq_input), clip_info)
    } else if let Some(pe_input) = &condor.sequence_config.parallel_encoder.input {
        debug!("Falling back to Parallel Encoder input");
        let (tq_input, clip_info) = open_input_cancel(pe_input, &cancelled.token)?;
        (Some(tq_input), clip_info)
    } else {
        (None, condor.input.clip_info()?)
    };
    let target_quality_metric_input = match condor
        .sequence_config
        .target_quality
        .as_ref()
        .and_then(|tq| tq.metric_input.clone())
    {
        Some(input) => Some(open_input_cancel(&input, &cancelled.token)?.0),
        None => None,
    };

    let mut target_quality = TargetQuality::new(target_quality_input, target_quality_metric_input);

    debug!("Validating Target Quality"); // Input should alrady be validated but just in case
    let (_, validation_warnings) = target_quality.validate(condor)?;

    for warning in validation_warnings.iter() {
        warn!("{}", warning);
    }

    debug!("Initializing Target Quality"); // Input should already be indexed but just in case
    let (init_progress_tx, _init_progress_rx) = std::sync::mpsc::channel();
    let (_, initialization_warnings) =
        target_quality.initialize_cancel(condor, init_progress_tx, &cancelled.token)?;

    for warning in initialization_warnings.iter() {
        warn!("{}", warning);
    }

    debug!("Running Target Quality");
    let target_quality_config = condor
        .sequence_config
        .target_quality
        .as_ref()
        .expect("target_quality is required");
    let mut tq_app = target_quality_app(
        clip_info,
        &condor.scenes,
        &condor.encoder,
        target_quality_config,
    )?;
    let ctrlc_cancelled = cancelled.clone();
    let (progress_tx, progress_rx) = std::sync::mpsc::channel();
    let tui_handle = thread::spawn(move || -> Result<()> {
        tq_app.run(progress_rx, ctrlc_cancelled)?;
        Ok(())
    });
    let executed = target_quality.execute_cancel(condor, progress_tx, cancelled.token);
    finish_stage("Target Quality", executed, || tui_handle.join())
}

#[tracing::instrument(skip_all)]
pub fn run_quality_check_tui(
    condor: &mut Condor<CliSequenceData, CliSequenceConfig>,
    cancelled: impl Into<Interrupt>,
) -> Result<()> {
    let cancelled = cancelled.into();
    cancelled.token.check()?;
    debug!("Instantiating Quality Check Input");
    let root = condor.input.as_data();
    let reference = andean_condor::models::sequence::provenance::quality_input(
        &root,
        &condor.sequence_config.parallel_encoder,
        condor.sequence_config.quality_check.as_ref(),
    );
    let (quality_check_input, clip_info) = if !std::ptr::eq(reference, &root) {
        let input = reference;
        let (qc_input, clip_info) = open_input_cancel(input, &cancelled.token)?;
        (Some(qc_input), clip_info)
    } else {
        (None, condor.input.clip_info()?)
    };

    let mut quality_check = QualityCheck::new(quality_check_input);

    debug!("Validating Quality Check");
    let (_, validation_warnings) = quality_check.validate(condor)?;

    for warning in validation_warnings.iter() {
        warn!("{}", warning);
    }

    debug!("Initializing Quality Check");
    let (init_progress_tx, _init_progress_rx) = std::sync::mpsc::channel();
    let (_, initialization_warnings) =
        quality_check.initialize_cancel(condor, init_progress_tx, &cancelled.token)?;

    for warning in initialization_warnings.iter() {
        warn!("{}", warning);
    }

    debug!("Running Quality Check");
    let quality_check_config = condor
        .sequence_config
        .quality_check
        .as_ref()
        .expect("quality_check is required");
    let metric = quality_check_config.metric.clone();
    let statistic = quality_check_config.statistic;
    let mut qc_app = QualityCheckApp::new(
        clip_info,
        &condor.scenes,
        metric,
        statistic,
        &quality_check_config.strategy,
    )?;
    let ctrlc_cancelled = cancelled.clone();
    let (progress_tx, progress_rx) = std::sync::mpsc::channel();
    let tui_handle = thread::spawn(move || -> Result<()> {
        qc_app.run(progress_rx, ctrlc_cancelled)?;
        Ok(())
    });
    let executed = quality_check.execute_cancel(condor, progress_tx, cancelled.token);
    finish_stage("Quality Check", executed, || tui_handle.join())
}

#[tracing::instrument(skip_all)]
pub fn run_bitrate_optimizer_tui(
    condor: &mut Condor<CliSequenceData, CliSequenceConfig>,
    cancelled: impl Into<Interrupt>,
) -> Result<()> {
    let cancelled = cancelled.into();
    cancelled.token.check()?;
    let mut bitrate_optimizer = BitrateOptimizer::default();
    debug!("Validating Bitrate Optimizer");
    let (_, validation_warnings) = bitrate_optimizer.validate(condor)?;

    for warning in validation_warnings.iter() {
        warn!("{}", warning);
    }

    debug!("Initializing Bitrate Optimizer");
    let (init_progress_tx, _init_progress_rx) = std::sync::mpsc::channel();
    let (_, initialization_warnings) =
        bitrate_optimizer.initialize_cancel(condor, init_progress_tx, &cancelled.token)?;

    for warning in initialization_warnings.iter() {
        warn!("{}", warning);
    }

    debug!("Running Bitrate Optimizer");
    let (progress_tx, _progress_rx) = std::sync::mpsc::channel();
    let (_, processing_warnings) =
        bitrate_optimizer.execute_cancel(condor, progress_tx, cancelled.token)?;

    for warning in processing_warnings.iter() {
        warn!("{}", warning);
    }

    Ok(())
}

#[tracing::instrument(skip_all)]
pub fn run_speed_scaler_tui(
    condor: &mut Condor<CliSequenceData, CliSequenceConfig>,
    cancelled: impl Into<Interrupt>,
) -> Result<()> {
    let cancelled = cancelled.into();
    cancelled.token.check()?;
    let mut convex_hull = SpeedScaler::default();
    debug!("Validating Convex Hull");
    let (_, validation_warnings) = convex_hull.validate(condor)?;

    for warning in validation_warnings.iter() {
        warn!("{}", warning);
    }

    debug!("Initializing Convex Hull");
    let (init_progress_tx, _init_progress_rx) = std::sync::mpsc::channel();
    let (_, initialization_warnings) =
        convex_hull.initialize_cancel(condor, init_progress_tx, &cancelled.token)?;

    for warning in initialization_warnings.iter() {
        warn!("{}", warning);
    }

    debug!("Running Convex Hull");
    let (progress_tx, _progress_rx) = std::sync::mpsc::channel();
    let (_, processing_warnings) =
        convex_hull.execute_cancel(condor, progress_tx, cancelled.token)?;

    for warning in processing_warnings.iter() {
        warn!("{}", warning);
    }

    Ok(())
}

#[tracing::instrument(skip_all)]
pub fn run_parallel_encoder_tui(
    condor: &mut Condor<CliSequenceData, CliSequenceConfig>,
    cancelled: impl Into<Interrupt>,
) -> Result<()> {
    let cancelled = cancelled.into();
    cancelled.token.check()?;
    debug!("Instantiating Parallel Encoder Input");
    let (parallel_encoder_input, clip_info) =
        if let Some(input) = &condor.sequence_config.parallel_encoder.input {
            let (pe_input, clip_info) = open_input_cancel(input, &cancelled.token)?;
            (Some(pe_input), clip_info)
        } else {
            (None, condor.input.clip_info()?)
        };

    let workers = condor.sequence_config.parallel_encoder.workers.unwrap_or(1);
    let mut parallel_encoder = ParallelEncoder::new(parallel_encoder_input);

    debug!("Validating Parallel Encoder"); // Input should alrady be validated but just in case
    let (_, validation_warnings) = parallel_encoder.validate(condor)?;

    for warning in validation_warnings.iter() {
        warn!("{}", warning);
    }

    debug!("Initializing Parallel Encoder"); // Input should already be indexed but just in case
    let (init_progress_tx, _init_progress_rx) = std::sync::mpsc::channel();
    let (_, initialization_warnings) =
        parallel_encoder.initialize_cancel(condor, init_progress_tx, &cancelled.token)?;

    for warning in initialization_warnings.iter() {
        warn!("{}", warning);
    }

    debug!("Running Parallel Encoder");
    let encoder = condor.encoder.clone();
    let scenes_map = condor
        .scenes
        .iter()
        .enumerate()
        .map(|(index, scene)| (index as u64, (0, SceneDisplay::from(scene))))
        .collect();

    let ctrlc_cancelled = cancelled.clone();
    let (progress_tx, progress_rx) = std::sync::mpsc::channel();
    let tui_handle = thread::spawn(move || -> Result<()> {
        let mut pe_app = ParallelEncoderApp::new(workers, encoder, scenes_map, clip_info);
        pe_app.run(progress_rx, ctrlc_cancelled)?;
        Ok(())
    });
    let executed = parallel_encoder.execute_cancel(condor, progress_tx, cancelled.token);
    finish_stage("Parallel Encoder", executed, || tui_handle.join())
}

#[tracing::instrument(skip_all)]
pub fn run_scene_concatenator_tui(
    condor: &mut Condor<CliSequenceData, CliSequenceConfig>,
    cancelled: impl Into<Interrupt>,
) -> Result<()> {
    let cancelled = cancelled.into();
    cancelled.token.check()?;
    let scenes_len = condor.scenes.len();
    let clip_info = condor.input.clip_info()?;
    let method = condor.sequence_config.scene_concatenator.method.clone();

    let mut scene_concatenator = SceneConcatenator::default();

    // Validate - Input should be already validated
    let (_, _validation_warnings) = scene_concatenator.validate(condor)?;

    // Initialize - Input should be already indexed
    let (init_progress_tx, _init_progress_rx) = std::sync::mpsc::channel();
    let (_, initialization_warnings) =
        scene_concatenator.initialize_cancel(condor, init_progress_tx, &cancelled.token)?;
    for warning in initialization_warnings.iter() {
        warn!("{}", warning);
    }

    debug!("Running Scene Concatenator");
    let ctrlc_cancelled = cancelled.clone();
    let (progress_tx, progress_rx) = std::sync::mpsc::channel();
    let tui_handle = thread::spawn(move || -> Result<()> {
        let mut sc_app = SceneConcatenatorApp::new(clip_info, scenes_len, method);
        sc_app.run(progress_rx, ctrlc_cancelled)?;
        Ok(())
    });
    let executed = scene_concatenator.execute_cancel(condor, progress_tx, cancelled.token);
    finish_stage("Scene Concatenator", executed, || tui_handle.join())
}

#[derive(Debug, ThisError)]
pub enum TUIError {
    #[error("Input and Scene Detector Input have a different number of frames: {0} != {1}")]
    FramesMismatch(u64, u64),
    #[error(
        "Input and Scene Detector Input have a different number of frames: {0} != {1}. Check the \
         input filters."
    )]
    FramesMismatchWithInputFilters(u64, u64),
}

#[cfg(test)]
mod tests {
    use std::{
        io::Seek,
        process::{Command, Stdio},
        sync::{Mutex, mpsc},
    };

    use tracing_subscriber::{Layer, layer::SubscriberExt};

    use super::*;

    #[test]
    fn frame_mismatch_displays_both_counts_and_filter_hint() {
        for (error, suffix) in [
            (TUIError::FramesMismatch(100, 120), "100 != 120"),
            (
                TUIError::FramesMismatchWithInputFilters(100, 120),
                "Check the input filters.",
            ),
        ] {
            let message = error.to_string();
            assert!(message.contains("100 != 120"), "{message}");
            assert!(message.ends_with(suffix), "{message}");
        }
    }

    #[test]
    fn stage_tq_summary_uses_resolved_scene_probe_encoders() {
        use andean_condor::models::encoder::{
            EncoderBase, EncoderPasses, cli_parameter::CLIParameter,
        };
        use ratatui::{Terminal, backend::TestBackend};

        let (clip, mut scenes) = crate::test_helpers::quality_scene_fixture();
        let global = Encoder::default_from_base(&EncoderBase::AOM, false);
        let mut actual = Encoder::default_from_base(&EncoderBase::SVTAV1, false);
        actual.set_ffmpeg_filter(Some("hflip".into()));
        actual
            .parameters_mut()
            .insert("psy-rd".into(), CLIParameter::new_number("--", " ", 1.0));
        for scene in &mut scenes {
            scene.encoder = actual.clone();
        }
        scenes[1]
            .encoder
            .parameters_mut()
            .insert("psy-rd".into(), CLIParameter::new_number("--", " ", 3.0));
        let before = serde_json::to_value(&scenes).unwrap();
        for replace in [false, true] {
            let mut config = TargetQualityConfig::default();
            if replace {
                config.probing.encoder_options = Some(
                    [
                        ("preset".into(), CLIParameter::new_number("--", " ", 9.0)),
                        ("psy-rd".into(), CLIParameter::new_number("--", " ", 2.0)),
                    ]
                    .into_iter()
                    .collect(),
                );
            }
            let app = target_quality_app(clip, &scenes, &global, &config).unwrap();
            let mut terminal = Terminal::new(TestBackend::new(160, 40)).unwrap();
            terminal.draw(|frame| app.render(frame)).unwrap();
            let buf = terminal.backend().buffer();
            let header = (0..8)
                .map(|y| (0..160).map(|x| buf[(x, y)].symbol()).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                header.contains("Scalable Video Technology for AV1"),
                "{header}"
            );
            assert!(
                !header.contains("Mixed encoders"),
                "resolved settings are uniform: {header}"
            );
            assert!(!header.contains("cpu-used"), "{header}");
            assert!(
                !header.contains("psy-rd"),
                "must strip psychovisual options: {header}"
            );
            assert!(
                header.contains(if replace { "--preset 9" } else { "--preset 4" }),
                "{header}"
            );
            assert_eq!(
                header.contains("--crf"),
                !replace,
                "options replace rather than merge: {header}"
            );
            let expected = config.probe_encoder(&actual);
            assert_eq!(expected.ffmpeg_filter(), None);
            assert_eq!(expected.passes(), EncoderPasses::All(1));
            assert_eq!(
                serde_json::to_value(&scenes).unwrap(),
                before,
                "display must not mutate saved scenes"
            );
        }
    }

    #[derive(Clone, Copy, Debug)]
    enum StageOutcome {
        Success,
        ImmediateFailure,
        AfterProgressFailure,
    }

    #[derive(Clone, Copy, Debug)]
    enum DisplayOutcome {
        Success,
        IoFailure,
        PanicStr,
        PanicString,
        PanicOpaque,
    }

    #[derive(Debug, ThisError)]
    #[error("typed primary stage failure")]
    struct StageFailure;

    type Events = Arc<Mutex<Vec<&'static str>>>;

    struct Warnings(Events);

    impl<S: tracing::Subscriber> Layer<S> for Warnings {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _context: tracing_subscriber::layer::Context<'_, S>,
        ) {
            if *event.metadata().level() == tracing::Level::WARN {
                self.0.lock().unwrap().push("warning");
            }
        }
    }

    // Removing the production join (or moving warnings before it) breaks this
    // ordering check. Only the stage/display work is fake, not finalization.
    fn lifecycle_case(
        stage: &'static str,
        stage_outcome: StageOutcome,
        display_outcome: DisplayOutcome,
    ) -> Result<()> {
        let events = Events::default();
        let caller_events = Arc::clone(&events);
        let (release_tx, release_rx) = mpsc::channel();
        let (control_tx, control_rx) = mpsc::channel();
        let caller = thread::spawn(move || {
            let (progress_tx, progress_rx) = mpsc::channel();
            let (started_tx, started_rx) = mpsc::channel();
            let display_events = Arc::clone(&caller_events);
            let mut display = Some(thread::spawn(move || -> Result<()> {
                display_events.lock().unwrap().push("display_started");
                started_tx.send(()).unwrap();
                for () in progress_rx {
                    display_events.lock().unwrap().push("progress");
                }
                display_events.lock().unwrap().push("progress_closed");
                release_rx.recv().unwrap();
                display_events.lock().unwrap().push("display_restored");
                display_events.lock().unwrap().push("display_finished");
                match display_outcome {
                    DisplayOutcome::Success => Ok(()),
                    DisplayOutcome::IoFailure => {
                        Err(std::io::Error::other("injected display I/O failure").into())
                    },
                    DisplayOutcome::PanicStr => panic!("display panic payload"),
                    DisplayOutcome::PanicString => {
                        std::panic::panic_any(String::from("owned display panic payload"));
                    },
                    DisplayOutcome::PanicOpaque => std::panic::panic_any(123_u32),
                }
            }));
            started_rx.recv().unwrap();
            // Like Sequence::execute, the fake stage consumes its only sender
            // on both success and error; no test-held clone can mask a hang.
            let executed = {
                if matches!(stage_outcome, StageOutcome::AfterProgressFailure) {
                    progress_tx.send(()).unwrap();
                }
                caller_events.lock().unwrap().push("stage_end");
                drop(progress_tx);
                if matches!(stage_outcome, StageOutcome::Success) {
                    Ok(((), vec![anyhow::anyhow!("processing warning")]))
                } else {
                    Err(anyhow::Error::new(StageFailure).context("stage execution failed"))
                }
            };
            let subscriber =
                tracing_subscriber::registry().with(Warnings(Arc::clone(&caller_events)));
            let result = tracing::subscriber::with_default(subscriber, || {
                finish_stage(stage, executed, || {
                    caller_events.lock().unwrap().push("join_entered");
                    control_tx.send("join_entered").unwrap();
                    let joined = display.take().unwrap().join();
                    caller_events.lock().unwrap().push("display_joined");
                    joined
                })
            });
            caller_events.lock().unwrap().push("caller_returned");
            control_tx.send("caller_returned").unwrap();
            // RED must clean up too if the old helper returns without joining.
            if let Some(display) = display {
                let _ = display.join();
            }
            result
        });
        // The first signal proves either entry into the real join callback or
        // an early return. Never infer ordering from a scheduling-dependent poll.
        let first = control_rx.recv()?;
        let released = release_tx.send(());
        let result = caller.join().expect("fake caller must not panic");
        released?;
        let events = events.lock().unwrap();
        anyhow::ensure!(
            first == "join_entered",
            "returned before joining: {events:?}"
        );
        let position = |name| -> Result<usize> {
            events
                .iter()
                .position(|event| *event == name)
                .ok_or_else(|| anyhow::anyhow!("missing {name}: {events:?}"))
        };
        anyhow::ensure!(
            position("display_started")? < position("stage_end")?
                && position("stage_end")? < position("progress_closed")?
                && position("progress_closed")? < position("display_restored")?
                && position("display_restored")? < position("display_finished")?
                && position("display_finished")? < position("display_joined")?
                && position("display_joined")? < position("caller_returned")?,
            "out of order: {events:?}"
        );
        anyhow::ensure!(
            events.contains(&"progress")
                == matches!(stage_outcome, StageOutcome::AfterProgressFailure),
            "wrong progress: {events:?}"
        );
        if matches!(stage_outcome, StageOutcome::Success) {
            anyhow::ensure!(
                events.iter().filter(|event| **event == "warning").count() == 1
                    && position("display_joined")? < position("warning")?
                    && position("warning")? < position("caller_returned")?,
                "warning missing or emitted before join: {events:?}"
            );
        } else {
            anyhow::ensure!(!events.contains(&"warning"));
        }
        if matches!(stage_outcome, StageOutcome::Success)
            && matches!(display_outcome, DisplayOutcome::Success)
        {
            return result;
        }
        let error = result.expect_err("injected failure must reach the caller");
        let diagnostic = format!("{error:#}");
        if !matches!(stage_outcome, StageOutcome::Success) {
            anyhow::ensure!(
                error.downcast_ref::<StageFailure>().is_some(),
                "{diagnostic}"
            );
            anyhow::ensure!(
                error.to_string().starts_with("stage execution failed"),
                "{diagnostic}"
            );
            anyhow::ensure!(
                diagnostic.contains("typed primary stage failure"),
                "{diagnostic}"
            );
            if matches!(display_outcome, DisplayOutcome::Success) {
                anyhow::ensure!(
                    diagnostic == "stage execution failed: typed primary stage failure"
                );
            }
        }
        let secondary = match display_outcome {
            DisplayOutcome::Success => return Ok(()),
            DisplayOutcome::IoFailure => "injected display I/O failure",
            DisplayOutcome::PanicStr => "display panic payload",
            DisplayOutcome::PanicString => "owned display panic payload",
            DisplayOutcome::PanicOpaque => "non-string panic payload",
        };
        anyhow::ensure!(
            diagnostic.contains(stage) && diagnostic.contains(secondary),
            "{diagnostic}"
        );
        if matches!(stage_outcome, StageOutcome::Success)
            && matches!(display_outcome, DisplayOutcome::IoFailure)
        {
            anyhow::ensure!(
                error.downcast_ref::<std::io::Error>().is_some(),
                "{diagnostic}"
            );
        }
        Ok(())
    }

    #[test]
    fn tui_stages_share_cancellation_and_restore_terminal() -> Result<()> {
        // File-backed output cannot fill a pipe if a regressed join hangs. The
        // watchdog only bounds failures; timing is not the ordering assertion.
        let mut output = tempfile::tempfile()?;
        let mut child = Command::new(std::env::current_exe()?)
            .args(["--exact", "tui::tests::tui_stage_lifecycle_child", "--ignored", "--nocapture"])
            .stdout(Stdio::from(output.try_clone()?))
            .stderr(Stdio::from(output.try_clone()?))
            .spawn()?;
        let watched: Result<()> = (|| {
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                if child.try_wait()?.is_some() {
                    return Ok(());
                }
                anyhow::ensure!(
                    Instant::now() < deadline,
                    "lifecycle child exceeded 30s watchdog"
                );
                thread::sleep(Duration::from_millis(10));
            }
        })();
        if watched.is_err() {
            let _ = child.kill();
        }
        let status = child.wait()?;
        output.rewind()?;
        let text = std::io::read_to_string(output)?;
        anyhow::ensure!(watched.is_ok() && status.success(), "{watched:?}\n{text}");
        anyhow::ensure!(
            text.contains("1 passed; 0 failed"),
            "child did not run: {text}"
        );
        Ok(())
    }

    #[test]
    #[ignore = "bounded child of tui_stages_share_cancellation_and_restore_terminal, not a standalone test"]
    fn tui_stage_lifecycle_child() -> Result<()> {
        let mut failures = Vec::new();
        // These are the seven production finish_stage call-site identities.
        // Runtime coverage is the common lifecycle, not native encoder jobs.
        for stage in [
            "Scene Detection",
            "Noise Detection",
            "Benchmarker",
            "Target Quality",
            "Quality Check",
            "Parallel Encoder",
            "Scene Concatenator",
        ] {
            for stage_outcome in [
                StageOutcome::Success,
                StageOutcome::ImmediateFailure,
                StageOutcome::AfterProgressFailure,
            ] {
                for display_outcome in [
                    DisplayOutcome::Success,
                    DisplayOutcome::IoFailure,
                    DisplayOutcome::PanicStr,
                    DisplayOutcome::PanicString,
                    DisplayOutcome::PanicOpaque,
                ] {
                    if let Err(error) = lifecycle_case(stage, stage_outcome, display_outcome) {
                        failures.push(format!(
                            "{stage}/{stage_outcome:?}/{display_outcome:?}: {error:#}"
                        ));
                    }
                }
            }
        }
        anyhow::ensure!(failures.is_empty(), "{}", failures.join("\n"));
        Ok(())
    }
}
