use std::{
    collections::BTreeMap,
    path::Path,
    sync::{atomic::AtomicBool, Arc},
    thread,
};

use anyhow::Result;
use av1an_core::{
    condor::core::{
        processors::{
            parallel_encoder::ParallelEncoder,
            scene_concatenator::SceneConcatenator,
            scene_detector::SceneDetector,
            Processor,
        },
        Condor,
    },
    vs::vapoursynth_filters::VapourSynthFilter,
};

use crate::{
    apps::{parallel_encoder::ParallelEncoderApp, scene_detection::SceneDetectionApp, TuiApp},
    configuration::{CliProcessData, CliProcessing, CliProcessorConfig, Configuration},
};

pub fn run_scene_detection_tui(
    condor: &mut Condor<CliProcessData, CliProcessorConfig>,
    scd_input_filters: &[VapourSynthFilter],
    cancelled: Arc<AtomicBool>,
) -> Result<()> {
    let initial_frames = condor.scenes.iter().fold(0, |acc, scene| {
        acc + (scene.end_frame - scene.start_frame) as u64
    });

    let (input, clip_info) = if let Some(input) = &condor.processor_config.scene_detection.input {
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

    let scd = SceneDetector {
        input,
        method: condor.processor_config.scene_detection.method,
    };

    let mut scene_detector =
        Box::new(scd) as Box<dyn Processor<CliProcessing, CliProcessData, CliProcessorConfig>>;

    // Validate - Input should be already validated
    let (_, _validation_warnings) = scene_detector.validate(condor)?;

    // Initialize - Input should be already indexed
    let (init_progress_tx, _init_progress_rx) = std::sync::mpsc::channel();
    let (_, initialization_warnings) = scene_detector.initialize(condor, init_progress_tx)?;

    for warning in initialization_warnings.iter() {
        println!("{}", warning);
    }

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
    let (_, processing_warnings) = scene_detector.process(condor, progress_tx, cancelled)?;

    for warning in processing_warnings.iter() {
        println!("{}", warning);
    }

    Ok(())
}

pub fn run_parallel_encoder_tui(
    condor: &mut Condor<CliProcessData, CliProcessorConfig>,
    input_filters: &[VapourSynthFilter],
    scenes_directory: &Path,
    cancelled: Arc<AtomicBool>,
) -> Result<()> {
    let (parallel_encoder_input, clip_info) = if let Some(input) =
        &condor.processor_config.parallel_encoder.input
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

    let workers = condor.processor_config.parallel_encoder.workers;
    let parallel_encoder = ParallelEncoder {
        input: parallel_encoder_input,
        encoder: condor.processor_config.parallel_encoder.encoder.clone(),
        scenes_directory: scenes_directory.to_path_buf(),
        workers,
    };
    let mut parallel_encoder = Box::new(parallel_encoder)
        as Box<dyn Processor<CliProcessing, CliProcessData, CliProcessorConfig>>;

    // Validate - Input should be already validated
    let (_, _validation_warnings) = parallel_encoder.validate(condor)?;

    // Initialize - Input should be already indexed
    let (init_progress_tx, _init_progress_rx) = std::sync::mpsc::channel();
    let (_, initialization_warnings) = parallel_encoder.initialize(condor, init_progress_tx)?;

    for warning in initialization_warnings.iter() {
        println!("{}", warning);
    }

    // Encode Scenes
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
    let (_, processing_warnings) = parallel_encoder.process(condor, progress_tx, cancelled)?;

    for warning in processing_warnings.iter() {
        println!("{}", warning);
    }

    Ok(())
}

pub fn run_scene_concatenator_tui(
    condor: &mut Condor<CliProcessData, CliProcessorConfig>,
    scenes_directory: &Path,
    cancelled: Arc<AtomicBool>,
) -> Result<()> {
    let concatenator = SceneConcatenator::new(scenes_directory);

    let mut scene_concatenator = Box::new(concatenator)
        as Box<dyn Processor<CliProcessing, CliProcessData, CliProcessorConfig>>;

    // Validate - Input should be already validated
    let (_, _validation_warnings) = scene_concatenator.validate(condor)?;

    // Initialize - Input should be already indexed
    let (init_progress_tx, _init_progress_rx) = std::sync::mpsc::channel();
    let (_, initialization_warnings) = scene_concatenator.initialize(condor, init_progress_tx)?;
    for warning in initialization_warnings.iter() {
        println!("{}", warning);
    }

    let (progress_tx, _progress_rx) = std::sync::mpsc::channel();
    let (_, processing_warnings) = scene_concatenator.process(condor, progress_tx, cancelled)?;
    for warning in processing_warnings.iter() {
        println!("{}", warning);
    }

    Ok(())
}
