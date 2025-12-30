use std::{
    collections::BTreeMap,
    io,
    path::Path,
    sync::{atomic::AtomicBool, Arc},
};

use anyhow::Result;
use av1an_core::{
    condor::core::{
        processors::{
            parallel_encoder::ParallelEncoder,
            scene_detector::SceneDetector,
            ProcessCompletion,
            ProcessStatus,
            Processor,
            Status,
        },
        Condor,
    },
    vs::vapoursynth_filters::VapourSynthFilter,
};
use ratatui::{backend::CrosstermBackend, crossterm, Terminal};

use crate::{
    components::layouts::{
        parallel_encoder::ParallelEncoderLayout,
        scene_detection::SceneDetectionLayout,
    },
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

    let (scene_detection_input, scene_detection_clip_info) =
        if let Some(input) = &condor.processor_config.scene_detection.input {
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

    let scd = SceneDetector {
        input:  scene_detection_input,
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

    // Detect Scenes
    let (_, screen_height) = ratatui::crossterm::terminal::size()?;
    for _ in 0..screen_height {
        println!();
    }
    crossterm::terminal::enable_raw_mode()?;
    let stdout = io::stdout();
    // crossterm::execute!(stdout, crossterm::terminal::EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let (progress_tx, progress_rx) = std::sync::mpsc::channel();
    let progress_thread = std::thread::spawn(move || -> Result<()> {
        let start_time = std::time::Instant::now();
        let mut layout = SceneDetectionLayout {
            started:          start_time,
            resolution:       scene_detection_clip_info.resolution,
            framerate:        (
                *scene_detection_clip_info.frame_rate.numer() as u64,
                *scene_detection_clip_info.frame_rate.denom() as u64,
            ),
            bit_depth:        scene_detection_clip_info.format_info.as_bit_depth()? as u32,
            hdr:              false, // TODO
            frames_processed: initial_frames,
            total_frames:     scene_detection_clip_info.num_frames as u64,
            scenes:           Vec::new(),
        };
        for progress in progress_rx {
            if let ProcessStatus::Whole(status) = progress {
                match status {
                    Status::Processing {
                        id: _id,
                        completion,
                    } => match completion {
                        ProcessCompletion::Frames {
                            completed,
                            total,
                        } => {
                            // println!("Frame: {}/{} - Scenes: {}", completed, total,
                            // scenes.len());
                            layout.frames_processed = completed;
                            terminal.draw(|f| {
                                layout.draw(f);
                            })?;
                        },
                        ProcessCompletion::Custom {
                            name,
                            completed: start_frame,
                            total: end_frame,
                        } => {
                            if name == "new-scene" {
                                layout.scenes.push((start_frame as u64, end_frame as u64));
                            }
                        },
                        _ => (),
                    },
                    Status::Completed {
                        ..
                    } => {
                        // crossterm::execute!(io::stdout(),
                        // crossterm::terminal::LeaveAlternateScreen)?;
                        println!("\n\n");
                        crossterm::terminal::disable_raw_mode()?;
                    },
                    _ => (),
                }
            }
        }
        Ok(())
    });

    let (_, processing_warnings) = scene_detector.process(condor, progress_tx, cancelled)?;
    let _ = progress_thread.join().expect("Progress thread should join");

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
        let mut scd_input =
            Configuration::instantiate_input_with_filters(&condor.input.as_data(), input_filters)?;
        let clip_info = scd_input.clip_info()?;
        (Some(scd_input), clip_info)
    };

    let parallel_encoder = ParallelEncoder {
        input:            parallel_encoder_input,
        encoder:          condor.processor_config.parallel_encoder.encoder.clone(),
        scenes_directory: scenes_directory.to_path_buf(),
        workers:          condor.processor_config.parallel_encoder.workers,
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
    let (_, screen_height) = ratatui::crossterm::terminal::size()?;
    for _ in 0..screen_height {
        println!();
    }
    crossterm::terminal::enable_raw_mode()?;
    let stdout = io::stdout();
    // crossterm::execute!(stdout, crossterm::terminal::EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

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
    let initial_frames = initial_scenes
        .iter()
        .filter(|(index, scene, already_encoded)| !*already_encoded)
        .map(|(_, scene, _)| (scene.end_frame - scene.start_frame) as u64)
        .sum::<u64>();
    let mut scenes_map = BTreeMap::new();
    for (index, scene, already_encoded) in initial_scenes.iter() {
        let scene_length = scene.end_frame - scene.start_frame;
        let scene_frames_encoded = if *already_encoded {
            scene_length as u64
        } else {
            0
        };
        scenes_map.insert(*index as u64, (scene_frames_encoded, scene_length as u64));
    }

    let (progress_tx, progress_rx) = std::sync::mpsc::channel();
    let progress_thread = std::thread::spawn(move || -> Result<()> {
        let start_time = std::time::Instant::now();
        let mut layout = ParallelEncoderLayout {
            started:          start_time,
            resolution:       clip_info.resolution,
            framerate:        (
                *clip_info.frame_rate.numer() as u64,
                *clip_info.frame_rate.denom() as u64,
            ),
            bit_depth:        clip_info.format_info.as_bit_depth()? as u32,
            hdr:              false, // TODO
            frames_processed: initial_frames,
            total_frames:     clip_info.num_frames as u64,
            scenes:           scenes_map,
        };
        for progress in progress_rx {
            if let ProcessStatus::Subprocess {
                parent,
                child,
            } = progress
            {
                match child {
                    Status::Processing {
                        id,
                        completion,
                    } => {
                        #[allow(clippy::collapsible_match)]
                        if let ProcessCompletion::PassFrames {
                            passes,
                            frames,
                        } = completion
                        {
                            let (current_pass, total_passes) = passes;
                            let (current_frame, total_frames) = frames;
                            println!(
                                "Scene {}: Pass {}/{} Frame {}/{}",
                                id, current_pass, total_passes, current_frame, total_frames
                            );
                            // layout.frames_processed = completed;
                            // terminal.draw(|f| {
                            //     layout.draw(f);
                            // })?;
                        }
                    },
                    Status::Completed {
                        id,
                    } => {
                        // Remove scene from active_scenes
                    },
                    _ => (),
                }
                if let Status::Completed {
                    id,
                } = parent
                {
                    // crossterm::execute!(io::stdout(),
                    // crossterm::terminal::LeaveAlternateScreen)?;
                    println!("\n\n");
                    crossterm::terminal::disable_raw_mode()?;
                }
            }
        }
        Ok(())
    });

    let (_, processing_warnings) = parallel_encoder.process(condor, progress_tx, cancelled)?;
    let _ = progress_thread.join().expect("Progress thread should join");

    for warning in processing_warnings.iter() {
        println!("{}", warning);
    }

    Ok(())
}
