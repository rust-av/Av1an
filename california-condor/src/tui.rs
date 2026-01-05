use std::{
    collections::BTreeMap,
    io::{IsTerminal, Write},
    path::Path,
    sync::{atomic::AtomicBool, Arc},
    thread,
    time::{Duration, Instant},
};

use andean_condor::{
    core::{
        sequence::{
            parallel_encoder::ParallelEncoder,
            scene_concatenator::SceneConcatenator,
            scene_detector::SceneDetector,
            Sequence,
        },
        Condor,
    },
    vapoursynth::vapoursynth_filters::VapourSynthFilter,
};
use anyhow::{bail, Result};
use thiserror::Error as ThisError;
use tracing::{debug, error, info, warn};

use crate::{
    apps::{parallel_encoder::ParallelEncoderApp, scene_detection::SceneDetectionApp, TuiApp},
    configuration::{CliSequenceConfig, CliSequenceData, Configuration},
};

#[tracing::instrument(skip_all)]
pub fn run_scene_detector_tui(
    condor: &mut Condor<CliSequenceData, CliSequenceConfig>,
    input_filters: &[VapourSynthFilter],
    scd_input_filters: &[VapourSynthFilter],
    cancelled: Arc<AtomicBool>,
) -> Result<()> {
    let initial_frames = condor.scenes.iter().fold(0, |acc, scene| {
        acc + (scene.end_frame - scene.start_frame) as u64
    });

    debug!("Instantiating Scene Detector Input");
    let (input, clip_info) = if let Some(input) = &condor.sequence_config.scene_detection.input {
        let mut scd_input =
            Configuration::instantiate_input_with_filters(input, scd_input_filters)?;
        let clip_info = scd_input.clip_info()?;
        (Some(scd_input), clip_info)
    } else {
        let mut scd_input = Configuration::instantiate_input_with_filters(
            &condor.input.as_data(),
            scd_input_filters,
        )?;
        let clip_info = scd_input.clip_info()?;
        (Some(scd_input), clip_info)
    };

    if initial_frames as usize == clip_info.num_frames {
        return Ok(());
    }

    let (input, clip_info) = if clip_info.num_frames == condor.input.clip_info()?.num_frames {
        (input, clip_info)
    } else {
        let input_frames = condor.input.clip_info()?.num_frames;
        let scd_input_frames = clip_info.num_frames;
        if input_filters.is_empty() && scd_input_filters.is_empty() {
            bail!(TUIError::FramesMismatch(
                input_frames as u64,
                scd_input_frames as u64
            ));
        }
        let input_time_altering_filters = input_filters
            .iter()
            .filter(|vs_filter| vs_filter.can_alter_time())
            .collect::<Vec<_>>();
        let scd_input_time_altering_filters = scd_input_filters
            .iter()
            .filter(|vs_filter| vs_filter.can_alter_time())
            .collect::<Vec<_>>();
        if input_time_altering_filters.is_empty() && scd_input_time_altering_filters.is_empty()
            || !scd_input_time_altering_filters.is_empty()
        {
            bail!(TUIError::FramesMismatchWithInputFilters(
                input_frames as u64,
                scd_input_frames as u64
            ));
        }

        // Try to recover by appending the time altering filters from the input to the
        // scd input
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
        let combined_filters = scd_input_filters
            .iter()
            .cloned()
            .chain(input_time_altering_filters.into_iter().cloned())
            .collect::<Vec<_>>();
        let mut scd_input = Configuration::instantiate_input_with_filters(
            &condor.input.as_data(),
            &combined_filters,
        )?;
        let clip_info = scd_input.clip_info()?;

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
        method: condor.sequence_config.scene_detection.method,
    };

    debug!("Validating Scene Detector"); // Input should alrady be validated but just in case
    let (_, validation_warnings) = scene_detector.validate(condor)?;

    for warning in validation_warnings.iter() {
        warn!("{}", warning);
    }

    debug!("Initializing Scene Detector"); // Input should already be indexed but just in case
    let (init_progress_tx, _init_progress_rx) = std::sync::mpsc::channel();
    let (_, initialization_warnings) = scene_detector.initialize(condor, init_progress_tx)?;

    for warning in initialization_warnings.iter() {
        warn!("{}", warning);
    }

    debug!("Running Scene Detector");
    let ctrlc_cancelled = Arc::clone(&cancelled);
    let (progress_tx, progress_rx) = std::sync::mpsc::channel();
    thread::spawn(move || -> Result<()> {
        let mut scd_app = SceneDetectionApp::new(
            // initial_frames, // SCD always starts from the beginning
            0,
            clip_info.num_frames as u64,
            Vec::new(),
            clip_info,
        );
        scd_app.run(progress_rx, ctrlc_cancelled)?;
        Ok(())
    });
    let (_, processing_warnings) = scene_detector.execute(condor, progress_tx, cancelled)?;

    for warning in processing_warnings.iter() {
        warn!("{}", warning);
    }

    Ok(())
}

#[tracing::instrument(skip_all)]
pub fn run_parallel_encoder_tui(
    condor: &mut Condor<CliSequenceData, CliSequenceConfig>,
    input_filters: &[VapourSynthFilter],
    scenes_directory: &Path,
    cancelled: Arc<AtomicBool>,
) -> Result<()> {
    debug!("Instantiating Parallel Encoder Input");
    let (parallel_encoder_input, clip_info) = if let Some(input) =
        &condor.sequence_config.parallel_encoder.input
    {
        let mut pe_input = Configuration::instantiate_input_with_filters(input, input_filters)?;
        let clip_info = pe_input.clip_info()?;
        (Some(pe_input), clip_info)
    } else {
        let mut pe_input =
            Configuration::instantiate_input_with_filters(&condor.input.as_data(), input_filters)?;
        let clip_info = pe_input.clip_info()?;
        (Some(pe_input), clip_info)
    };

    let workers = condor.sequence_config.parallel_encoder.workers;
    let mut parallel_encoder = ParallelEncoder {
        input: parallel_encoder_input,
        encoder: condor.sequence_config.parallel_encoder.encoder.clone(),
        scenes_directory: scenes_directory.to_path_buf(),
        workers,
    };

    debug!("Validating Parallel Encoder"); // Input should alrady be validated but just in case
    let (_, validation_warnings) = parallel_encoder.validate(condor)?;

    for warning in validation_warnings.iter() {
        warn!("{}", warning);
    }

    debug!("Initializing Parallel Encoder"); // Input should already be indexed but just in case
    let (init_progress_tx, _init_progress_rx) = std::sync::mpsc::channel();
    let (_, initialization_warnings) = parallel_encoder.initialize(condor, init_progress_tx)?;

    for warning in initialization_warnings.iter() {
        warn!("{}", warning);
    }

    debug!("Running Parallel Encoder");
    let encoder = condor.encoder.clone();
    let initial_scenes = condor
        .scenes
        .iter()
        .enumerate()
        .map(|(index, scene)| {
            (
                index,
                scene,
                scenes_directory
                    .join(format!(
                        "{}.{}",
                        ParallelEncoder::scene_id(index),
                        scene.encoder.output_extension()
                    ))
                    .exists(),
            )
        })
        .collect::<Vec<_>>();
    let mut scenes_map = BTreeMap::new();
    for (index, scene, already_encoded) in initial_scenes {
        let scene_frames_encoded = if already_encoded {
            (scene.end_frame - scene.start_frame) as u64
        } else {
            0
        };
        scenes_map.insert(index as u64, (scene_frames_encoded, scene.clone()));
    }

    let ctrlc_cancelled = Arc::clone(&cancelled);
    let (progress_tx, progress_rx) = std::sync::mpsc::channel();
    thread::spawn(move || -> Result<()> {
        let mut pe_app = ParallelEncoderApp::new(workers, encoder, scenes_map, clip_info);
        pe_app.run(progress_rx, ctrlc_cancelled)?;
        Ok(())
    });
    let (_, processing_warnings) = parallel_encoder.execute(condor, progress_tx, cancelled)?;

    for warning in processing_warnings.iter() {
        warn!("{}", warning);
    }

    Ok(())
}

pub fn run_scene_concatenator_tui(
    condor: &mut Condor<CliSequenceData, CliSequenceConfig>,
    scenes_directory: &Path,
    cancelled: Arc<AtomicBool>,
) -> Result<()> {
    let mut scene_concatenator = SceneConcatenator::new(
        scenes_directory,
        condor.sequence_config.scene_concatenation.method,
    );

    // Validate - Input should be already validated
    let (_, _validation_warnings) = scene_concatenator.validate(condor)?;

    // Initialize - Input should be already indexed
    let (init_progress_tx, _init_progress_rx) = std::sync::mpsc::channel();
    let (_, initialization_warnings) = scene_concatenator.initialize(condor, init_progress_tx)?;
    for warning in initialization_warnings.iter() {
        println!("{}", warning);
    }

    let (progress_tx, _progress_rx) = std::sync::mpsc::channel();
    let (_, processing_warnings) = scene_concatenator.execute(condor, progress_tx, cancelled)?;
    for warning in processing_warnings.iter() {
        println!("{}", warning);
    }

    Ok(())
}

#[derive(Debug, ThisError)]
pub enum TUIError {
    #[error("Input and Scene Detector Input have a different number of frames: {0} != {0}")]
    FramesMismatch(u64, u64),
    #[error(
        "Input and Scene Detector Input have a different number of frames: {0} != {0}. Check the \
         input filters."
    )]
    FramesMismatchWithInputFilters(u64, u64),
}
