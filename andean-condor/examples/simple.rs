use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool, mpsc},
    thread,
};

use andean_condor::{
    core::{
        AndeanCondor,
        Condor,
        DefaultAndeanCondor,
        SequenceProgressEvent,
        SequenceType,
        input::Input,
        output::Output,
        sequence::{
            SequenceStatus,
            Status,
            parallel_encoder::ParallelEncoder,
            scene_concatenator::SceneConcatenator,
            scene_detector::SceneDetector,
        },
    },
    ffmpeg::FFPixelFormat,
    models::{
        encoder::{Encoder, EncoderBase, EncoderPasses, cli_parameter::CLIParameter},
        input::{Input as InputModel, VapourSynthImportMethod},
        output::Output as OutputModel,
        sequence::{
            DefaultSequenceConfig,
            DefaultSequenceData,
            scene_concatenator::SceneConcatenatorConfig,
            scene_detector::{
                DEFAULT_MAX_SCENE_LENGTH_SECONDS,
                DEFAULT_MIN_SCENE_LENGTH_FRAMES,
                SceneDetectionMethod,
                SceneDetectorConfig,
                ScenecutMethod,
            },
        },
    },
    vapoursynth::{
        plugins::{resize::Scaler, standard::select_every::SelectEvery},
        vapoursynth_filters::VapourSynthFilter,
    },
};
use anyhow::Result;

/// A simple end-to-end example using Andean Condor.
///
/// This example builds a complete encode pipeline:
///
/// 1. **Input** — `input.mp4` is opened with VapourSynth's FFMS2 and resized to
///    `YUV420P10LE`.
/// 2. **Output** — the final file is written to `output.mkv`.
/// 3. **Scene detection** — scenes are detected with AVSceneChange.
/// 4. **Encoding** — each scene is encoded with SVT-AV1 with `--preset 6 --crf
///    30` using 4 workers.
/// 5. **Concatenation** — the encoded scenes are muxed together with
///    `mkvmerge`.
///
/// Note: The paths are hardcoded for demonstration purposes; the input file
/// does not need to exist to compile this example.
fn main() -> Result<()> {
    // Input: VapourSynth FFMS2 + resize to YUV420P10LE
    let input_model = InputModel::VapourSynth {
        path:          PathBuf::from("input.mp4"),
        import_method: VapourSynthImportMethod::FFMS2 {
            index: None
        },
        cache_path:    None,
        filters:       vec![VapourSynthFilter::Resize {
            scaler: Some(Scaler::Bicubic),
            width:  None,
            height: None,
            format: Some(FFPixelFormat::YUV420P10LE),
        }],
    };
    let node_modifier: andean_condor::core::input::ModifyNode = Box::new(move |core, node| {
        let select_every = SelectEvery {
            cycle:           2,
            offsets:         vec![1],
            modify_duration: None,
        };
        let node = node.expect("node exists");
        select_every.invoke(core, &node).map_err(|e| {
            andean_condor::core::input::DecoderError::VapoursynthScriptError {
                cause: e.to_string(),
            }
        })
    });

    // Instantiate Input. `ModifyNode` is applied first, then the filters.
    let input = Input::from_vapoursynth(&input_model, Some(node_modifier))?;

    // Output: output.mkv
    let output_model = OutputModel {
        path:       PathBuf::from("output.mkv"),
        tags:       HashMap::new(),
        video_tags: HashMap::new(),
    };
    // Instantiate Output
    let output = Output::new(&output_model)?;

    // Encoder: SVT-AV1 with --preset 6 --crf 30
    let mut options = EncoderBase::SVTAV1.default_parameters();
    options.insert(
        "preset".to_owned(),
        CLIParameter::new_number("--", " ", 6.0),
    );
    options.insert("crf".to_owned(), CLIParameter::new_number("--", " ", 30.0));

    let encoder = Encoder::SVTAV1 {
        executable: None,
        pass: EncoderPasses::All(1),
        options,
        photon_noise: None,
        ffmpeg_filter: None,
        append_color_data: true,
    };

    // Configure Sequences SceneDetector, ParallelEncoder, and SceneConcatenator
    let scenes_directory = PathBuf::from("scenes");

    let sequence_config = DefaultSequenceConfig {
        scene_detector: SceneDetectorConfig {
            method: SceneDetectionMethod::AVSceneChange {
                minimum_length: DEFAULT_MIN_SCENE_LENGTH_FRAMES as usize,
                maximum_length: DEFAULT_MIN_SCENE_LENGTH_FRAMES as usize
                    * DEFAULT_MAX_SCENE_LENGTH_SECONDS as usize,
                method:         ScenecutMethod::Standard,
                save_scores:    false,
            },
            input:  None,
        },
        parallel_encoder:
            andean_condor::models::sequence::parallel_encoder::ParallelEncoderConfig {
                scenes_directory: scenes_directory.clone(),
                workers: Some(4),
                ..Default::default()
            },
        scene_concatenator: SceneConcatenatorConfig::new(&scenes_directory),
        ..Default::default()
    };

    // Assemble Condor Instance
    let save_callback: andean_condor::core::SaveCallback<
        DefaultSequenceData,
        DefaultSequenceConfig,
    > = Box::new(|_data| Ok(()));

    let condor = Condor::new(
        input,
        output,
        encoder,
        Vec::new(), // scenes are populated by the SceneDetector
        Some(sequence_config),
        save_callback,
    );

    // Set up DefaultAndeanCondor pipeline with SceneDetector, ParallelEncoder, and
    // SceneConcatenator
    let mut andean_condor = DefaultAndeanCondor {
        condor,
        sequences: vec![
            Box::new(SceneDetector {
                input:  None, // use Condor's own input
                method: SceneDetectionMethod::AVSceneChange {
                    minimum_length: DEFAULT_MIN_SCENE_LENGTH_FRAMES as usize,
                    maximum_length: DEFAULT_MIN_SCENE_LENGTH_FRAMES as usize
                        * DEFAULT_MAX_SCENE_LENGTH_SECONDS as usize,
                    method:         ScenecutMethod::Standard,
                    save_scores:    false,
                },
            }),
            Box::new(ParallelEncoder::new(None)), // use Condor's own input
            Box::new(SceneConcatenator::default()),
        ],
    };

    // Run the DefaultAndeanCondor pipeline, printing progress events
    let (progress_tx, progress_rx) = mpsc::channel::<SequenceProgressEvent>();
    let progress_thread = thread::spawn(move || {
        for event in progress_rx {
            print_progress(&event);
        }
    });

    let cancelled = Arc::new(AtomicBool::new(false));
    let warnings = andean_condor.process_all(progress_tx, cancelled)?;

    progress_thread.join().expect("progress thread should join");

    // Print any warnings that were collected during processing.
    for (_, (validation, initialization, processing)) in warnings {
        for warning in validation.into_iter().chain(initialization).chain(processing) {
            eprintln!("warning: {warning}");
        }
    }

    println!("Done! Output written to output.mkv");

    Ok(())
}

/// Print a human-readable summary of a progress event.
fn print_progress(event: &SequenceProgressEvent) {
    let phase = match event.sequence_type {
        SequenceType::Validation => "validate",
        SequenceType::Initialization => "initialize",
        SequenceType::Processing => "process",
    };

    match &event.progress {
        SequenceStatus::Whole(status) => print_status(phase, event.details.name, status),
        SequenceStatus::Subprocess {
            parent,
            child,
        } => {
            print_status(phase, event.details.name, parent);
            print_status(phase, event.details.name, child);
        },
    }
}

fn print_status(phase: &str, name: &str, status: &Status) {
    match status {
        Status::Processing {
            id,
            completion,
        } => {
            println!("[{phase}] {name} ({id}): {completion:?}");
        },
        Status::Completed {
            id,
        } => {
            println!("[{phase}] {name} ({id}): completed");
        },
        Status::Failed {
            id,
            error,
        } => {
            eprintln!("[{phase}] {name} ({id}): failed: {error}");
        },
    }
}
