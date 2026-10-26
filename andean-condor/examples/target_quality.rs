use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool, mpsc},
    thread,
};

use andean_condor::{
    core::{
        Condor,
        SaveCallback,
        SequenceProgressEvent,
        SequenceType,
        input::Input,
        output::Output,
        sequence::{
            Sequence,
            SequenceStatus,
            Status,
            benchmarker::Benchmarker,
            bitrate_optimizer::BitrateOptimizer,
            parallel_encoder::ParallelEncoder,
            scene_concatenator::SceneConcatenator,
            scene_detector::SceneDetector,
            speed_scaler::SpeedScaler,
            target_quality::TargetQuality,
        },
    },
    ffmpeg::FFPixelFormat,
    models::{
        encoder::{Encoder, EncoderBase, EncoderPasses, cli_parameter::CLIParameter},
        input::{Input as InputModel, VapourSynthImportMethod, VapourSynthScriptSource},
        output::Output as OutputModel,
        sequence::{
            SequenceConfigHandler,
            SequenceDataHandler,
            benchmarker::{BenchmarkerConfig, BenchmarkerConfigHandler},
            bitrate_optimizer::{BitrateOptimizerConfig, BitrateOptimizerConfigHandler},
            noise_detector::{NoiseDetectorData, NoiseDetectorDataHandler},
            parallel_encoder::{
                ParallelEncoderConfig,
                ParallelEncoderConfigHandler,
                ParallelEncoderData,
                ParallelEncoderDataHandler,
            },
            scene_concatenator::{
                ConcatMethod,
                SceneConcatenatorConfig,
                SceneConcatenatorConfigHandler,
            },
            scene_detector::{
                SceneDetectionMethod,
                SceneDetectorConfig,
                SceneDetectorData,
                SceneDetectorDataHandler,
                ScenecutMethod,
            },
            speed_scaler::{SpeedScalerConfig, SpeedScalerConfigHandler},
            target_quality::{
                TargetQualityConfig,
                TargetQualityConfigHandler,
                TargetQualityData,
                TargetQualityDataHandler,
                types::{
                    DEFAULT_MAXIMUM_PROBES,
                    InterpolationMethod,
                    ProbeStatistic,
                    ProbeStrategy,
                    QualityMetric,
                    SubsetProbeLength,
                    SubsetProbePosition,
                    TargetQualityProbing,
                },
            },
        },
    },
    vapoursynth::{plugins::resize::Scaler, vapoursynth_filters::VapourSynthFilter},
};
use anyhow::Result;
use serde::{Deserialize, Serialize};

/// A full end-to-end example using Target Quality and the Benchmarker.
///
/// This example builds a complete encode pipeline:
///
/// 1. **Input** — `input.mp4` is opened with VapourSynth's FFMS2.
/// 2. **Output** — the final file is written to `output.mkv`.
/// 3. **Scene detection** — scenes are detected with AVSceneChange on
///    `script.vpy` (`mode = "scene detection"`) resized to 960x540 with no
///    format change, using 1 second of the input FPS as the minimum length and
///    5 seconds as the maximum length.
/// 4. **Benchmarker** — determines the number of workers for the Parallel
///    Encoder (`ParallelEncoderConfig.workers` starts as `None`).
/// 5. **Target Quality** — targets SSIMULACRA2 79.5-80.5, sampling 5 frames in
///    the middle of each scene aggregated with the mean, on `script.vpy` (`mode
///    = "target quality"`) resized to 1280x720 with YUV420P10LE and `--preset
///    fast` probe encoder params.
/// 6. **Bitrate optimizer** — runs with a 2 standard deviation distance
///    threshold.
/// 7. **Speed scaler** — maps crf 10 to fast, crf 15 to medium, and crf 30 to
///    veryslow (via their number equivalents).
/// 8. **Encoding** — each scene is encoded with x264 `--preset medium --crf 20`
///    using the benchmarked worker count, on `script.vpy` resized to 1280x720
///    with YUV420P10LE.
/// 9. **Concatenation** — the encoded scenes are muxed together with FFmpeg.
///
/// Note: The paths are hardcoded for demonstration purposes; the input files
/// do not need to exist to compile this example.
fn main() -> Result<()> {
    // Input: VapourSynth FFMS2
    let input_model = InputModel::VapourSynth {
        path:          PathBuf::from("input.mp4"),
        import_method: VapourSynthImportMethod::FFMS2 {
            index: None
        },
        cache_path:    None,
        filters:       Vec::new(),
    };
    let mut input = Input::from_vapoursynth(&input_model, None)?;

    // Derive scene lengths from the input frame rate: 1 second minimum, 5
    // seconds maximum.
    let clip_info = input.clip_info()?;
    let fps = *clip_info.frame_rate.numer() as f64 / *clip_info.frame_rate.denom() as f64;
    let minimum_length = fps.round() as usize;
    let maximum_length = 5 * fps.round() as usize;

    // Output: output.mkv
    let output_model = OutputModel {
        path:       PathBuf::from("output.mkv"),
        tags:       HashMap::new(),
        video_tags: HashMap::new(),
    };
    let output = Output::new(&output_model)?;

    // Encoder: x264 with --preset medium --crf 20
    let mut encoder_options = EncoderBase::X264.default_parameters();
    encoder_options.insert(
        "preset".to_owned(),
        CLIParameter::new_string("--", " ", "medium"),
    );
    encoder_options.insert("crf".to_owned(), CLIParameter::new_number("--", " ", 20.0));
    let encoder = Encoder::X264 {
        executable:        None,
        pass:              EncoderPasses::All(1),
        options:           encoder_options,
        ffmpeg_filter:     None,
        append_color_data: true,
    };

    // Per-sequence VapourSynth script inputs, all reading script.vpy.
    let scd_input_model = InputModel::VapourSynthScript {
        source:              VapourSynthScriptSource::Path(PathBuf::from("script.vpy")),
        variables:           HashMap::from([("mode".to_owned(), "scene detection".to_owned())]),
        index:               0,
        filters:             resize_filters(Some(960), Some(540), None),
        stream_concurrently: true,
    };
    let tq_input_model = InputModel::VapourSynthScript {
        source:              VapourSynthScriptSource::Path(PathBuf::from("script.vpy")),
        variables:           HashMap::from([("mode".to_owned(), "target quality".to_owned())]),
        index:               0,
        filters:             resize_filters(
            Some(1280),
            Some(720),
            Some(FFPixelFormat::YUV420P10LE),
        ),
        stream_concurrently: true,
    };
    let pe_input_model = InputModel::VapourSynthScript {
        source:              VapourSynthScriptSource::Path(PathBuf::from("script.vpy")),
        variables:           HashMap::new(),
        index:               0,
        filters:             resize_filters(
            Some(1280),
            Some(720),
            Some(FFPixelFormat::YUV420P10LE),
        ),
        stream_concurrently: true,
    };
    let scd_input = Input::from_vapoursynth(&scd_input_model, None)?;
    let tq_input = Input::from_vapoursynth(&tq_input_model, None)?;
    let pe_input = Input::from_vapoursynth(&pe_input_model, None)?;

    // Configure sequences.
    let scenes_directory = PathBuf::from("scenes");
    let scene_detection_method = SceneDetectionMethod::AVSceneChange {
        minimum_length,
        maximum_length,
        method: ScenecutMethod::Standard,
        save_scores: false,
    };

    // Target Quality probe encoder params: x264 defaults with --preset fast.
    let mut probe_options = EncoderBase::X264.default_parameters();
    probe_options.insert(
        "preset".to_owned(),
        CLIParameter::new_string("--", " ", "fast"),
    );

    let sequence_config = ExampleSequenceConfig {
        scene_detector:     SceneDetectorConfig {
            method: scene_detection_method.clone(),
            input:  Some(scd_input_model),
        },
        benchmarker:        BenchmarkerConfig::default(),
        target_quality:     Some(TargetQualityConfig {
            metric:          QualityMetric::SSIMULACRA2 {
                target_range: (79.5, 80.5),
                resolution:   None,
                threads:      None,
                gpu_id:       None,
            },
            maximum_probes:  DEFAULT_MAXIMUM_PROBES,
            quantizer_range: TargetQuality::default_quantizer_range(&EncoderBase::X264),
            interpolators:   (InterpolationMethod::Natural, InterpolationMethod::Pchip),
            input:           Some(tq_input_model),
            metric_input:    None,
            probing:         TargetQualityProbing {
                encoder_options: Some(probe_options),
                strategy:        ProbeStrategy::Subset {
                    position: SubsetProbePosition::Middle,
                    length:   SubsetProbeLength::Frames(5),
                },
                statistic:       ProbeStatistic::Mean,
            },
        }),
        bitrate_optimizer:  BitrateOptimizerConfig {
            bitrate_sigma_threshold: Some(2),
        },
        speed_scaler:       SpeedScalerConfig {
            // (speed, quantizer): 5 is fast, 4 is medium, 1 is veryslow for
            // x264. See `Encoder::set_speed`.
            speed_quantizers: vec![(5, 10.0), (4, 15.0), (1, 30.0)],
        },
        parallel_encoder:   ParallelEncoderConfig {
            workers: None,
            input: Some(pe_input_model),
            ..ParallelEncoderConfig::new(&scenes_directory)
        },
        scene_concatenator: SceneConcatenatorConfig {
            method: ConcatMethod::FFmpeg(Default::default()),
            ..SceneConcatenatorConfig::new(&scenes_directory)
        },
    };

    // Instantiate Condor.
    let save_callback: SaveCallback<ExampleSequenceData, ExampleSequenceConfig> =
        Box::new(|_data| Ok(()));
    let mut condor = Condor::new(
        input,
        output,
        encoder,
        Vec::new(), // scenes are populated by the SceneDetector
        Some(sequence_config),
        save_callback,
    );

    // Run the pipeline, printing progress events.
    let (progress_tx, progress_rx) = mpsc::channel::<SequenceProgressEvent>();
    let progress_thread = thread::spawn(move || {
        for event in progress_rx {
            print_progress(&event);
        }
    });
    let cancelled = Arc::new(AtomicBool::new(false));

    // Scene Detector: populate `condor.scenes` with AVSceneChange.
    let mut scene_detector = SceneDetector {
        input:  Some(scd_input),
        method: scene_detection_method,
    };
    run_sequence(
        &mut condor,
        &mut scene_detector,
        0,
        &progress_tx,
        &cancelled,
    )?;

    // Benchmarker: determine the Parallel Encoder worker count.
    let mut benchmarker = Benchmarker::default();
    run_sequence(&mut condor, &mut benchmarker, 1, &progress_tx, &cancelled)?;

    // Target Quality: find the optimal quantizer per scene.
    let mut target_quality = TargetQuality::new(Some(tq_input), None);
    run_sequence(
        &mut condor,
        &mut target_quality,
        2,
        &progress_tx,
        &cancelled,
    )?;

    // Bitrate Optimizer: re-quantize scenes with excessive bitrates.
    let mut bitrate_optimizer = BitrateOptimizer::default();
    run_sequence(
        &mut condor,
        &mut bitrate_optimizer,
        3,
        &progress_tx,
        &cancelled,
    )?;

    // Speed Scaler: adjust per-scene speed from the quantizer.
    let mut speed_scaler = SpeedScaler::default();
    run_sequence(&mut condor, &mut speed_scaler, 4, &progress_tx, &cancelled)?;

    // Parallel Encoder: encode every scene.
    let mut parallel_encoder = ParallelEncoder::new(Some(pe_input));
    run_sequence(
        &mut condor,
        &mut parallel_encoder,
        5,
        &progress_tx,
        &cancelled,
    )?;

    // Scene Concatenator: mux the encoded scenes into `output.mkv`.
    let mut scene_concatenator = SceneConcatenator::default();
    run_sequence(
        &mut condor,
        &mut scene_concatenator,
        6,
        &progress_tx,
        &cancelled,
    )?;

    drop(progress_tx);
    progress_thread.join().expect("progress thread should join");

    println!("Done! Output written to output.mkv");

    Ok(())
}

/// Run a single sequence with `condor`: validate, initialize, then execute.
///
/// The sequence is instantiated, used directly, and any warnings are printed
/// along the way.
fn run_sequence(
    condor: &mut Condor<ExampleSequenceData, ExampleSequenceConfig>,
    sequence: &mut impl Sequence<ExampleSequenceData, ExampleSequenceConfig>,
    index: usize,
    progress_tx: &mpsc::Sender<SequenceProgressEvent>,
    cancelled: &Arc<AtomicBool>,
) -> Result<()> {
    let details = sequence.details();

    let (_, validation_warnings) = sequence.validate(condor)?;
    for warning in validation_warnings {
        eprintln!("warning: {warning}");
    }

    let (init_tx, init_rx) = mpsc::channel();
    let event_tx = progress_tx.clone();
    let init_handle = thread::spawn(move || {
        for progress in init_rx {
            let event = SequenceProgressEvent {
                sequence_type: SequenceType::Initialization,
                index,
                details,
                progress,
            };
            let _ = event_tx.send(event);
        }
    });
    let (_, initialization_warnings) = sequence.initialize(condor, init_tx)?;
    let _ = init_handle.join();
    for warning in initialization_warnings {
        eprintln!("warning: {warning}");
    }

    let (execute_tx, execute_rx) = mpsc::channel();
    let event_tx = progress_tx.clone();
    let execute_handle = thread::spawn(move || {
        for progress in execute_rx {
            let event = SequenceProgressEvent {
                sequence_type: SequenceType::Processing,
                index,
                details,
                progress,
            };
            let _ = event_tx.send(event);
        }
    });
    let (_, execution_warnings) = sequence.execute(condor, execute_tx, Arc::clone(cancelled))?;
    let _ = execute_handle.join();
    for warning in execution_warnings {
        eprintln!("warning: {warning}");
    }

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

/// Sequence config for this example.
///
/// `DefaultSequenceConfig` cannot be used here because it does not implement
/// `BenchmarkerConfigHandler` or `SpeedScalerConfigHandler`, both of which are
/// required by the `Benchmarker` and `SpeedScaler` sequences.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExampleSequenceConfig {
    pub scene_detector:     SceneDetectorConfig,
    pub benchmarker:        BenchmarkerConfig,
    pub target_quality:     Option<TargetQualityConfig>,
    pub bitrate_optimizer:  BitrateOptimizerConfig,
    pub speed_scaler:       SpeedScalerConfig,
    pub parallel_encoder:   ParallelEncoderConfig,
    pub scene_concatenator: SceneConcatenatorConfig,
}

impl Default for ExampleSequenceConfig {
    #[inline]
    fn default() -> Self {
        Self {
            scene_detector:     SceneDetectorConfig::default(),
            benchmarker:        BenchmarkerConfig::default(),
            target_quality:     None,
            bitrate_optimizer:  BitrateOptimizerConfig::default(),
            speed_scaler:       SpeedScalerConfig::default(),
            parallel_encoder:   ParallelEncoderConfig::default(),
            scene_concatenator: SceneConcatenatorConfig::default(),
        }
    }
}

impl SequenceConfigHandler for ExampleSequenceConfig {
}

impl BenchmarkerConfigHandler for ExampleSequenceConfig {
    #[inline]
    fn benchmarker(&self) -> Result<&BenchmarkerConfig> {
        Ok(&self.benchmarker)
    }

    #[inline]
    fn benchmarker_mut(&mut self) -> Result<&mut BenchmarkerConfig> {
        Ok(&mut self.benchmarker)
    }
}

impl ParallelEncoderConfigHandler for ExampleSequenceConfig {
    #[inline]
    fn parallel_encoder(&self) -> Result<&ParallelEncoderConfig> {
        Ok(&self.parallel_encoder)
    }

    #[inline]
    fn parallel_encoder_mut(&mut self) -> Result<&mut ParallelEncoderConfig> {
        Ok(&mut self.parallel_encoder)
    }
}

impl SceneConcatenatorConfigHandler for ExampleSequenceConfig {
    #[inline]
    fn scene_concatenator(&self) -> Result<&SceneConcatenatorConfig> {
        Ok(&self.scene_concatenator)
    }

    #[inline]
    fn scene_concatenator_mut(&mut self) -> Result<&mut SceneConcatenatorConfig> {
        Ok(&mut self.scene_concatenator)
    }
}

impl TargetQualityConfigHandler for ExampleSequenceConfig {
    #[inline]
    fn target_quality(&self) -> Result<&Option<TargetQualityConfig>> {
        Ok(&self.target_quality)
    }

    #[inline]
    fn target_quality_mut(&mut self) -> Result<&mut Option<TargetQualityConfig>> {
        Ok(&mut self.target_quality)
    }
}

impl BitrateOptimizerConfigHandler for ExampleSequenceConfig {
    #[inline]
    fn bitrate_optimizer(&self) -> Result<&BitrateOptimizerConfig> {
        Ok(&self.bitrate_optimizer)
    }

    #[inline]
    fn bitrate_optimizer_mut(&mut self) -> Result<&mut BitrateOptimizerConfig> {
        Ok(&mut self.bitrate_optimizer)
    }
}

impl SpeedScalerConfigHandler for ExampleSequenceConfig {
    #[inline]
    fn speed_scaler(&self) -> Result<&SpeedScalerConfig> {
        Ok(&self.speed_scaler)
    }

    #[inline]
    fn speed_scaler_mut(&mut self) -> Result<&mut SpeedScalerConfig> {
        Ok(&mut self.speed_scaler)
    }
}

/// Sequence data for this example.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExampleSequenceData {
    pub scene_detection:  SceneDetectorData,
    pub noise_detection:  Option<NoiseDetectorData>,
    pub target_quality:   TargetQualityData,
    pub parallel_encoder: ParallelEncoderData,
}

impl SequenceDataHandler for ExampleSequenceData {
}

impl SceneDetectorDataHandler for ExampleSequenceData {
    #[inline]
    fn get_scene_detection(&self) -> Result<&SceneDetectorData> {
        Ok(&self.scene_detection)
    }

    #[inline]
    fn get_scene_detection_mut(&mut self) -> Result<&mut SceneDetectorData> {
        Ok(&mut self.scene_detection)
    }
}

impl NoiseDetectorDataHandler for ExampleSequenceData {
    #[inline]
    fn get_noise_detection(&self) -> Result<&Option<NoiseDetectorData>> {
        Ok(&self.noise_detection)
    }

    #[inline]
    fn get_noise_detection_mut(&mut self) -> Result<&mut Option<NoiseDetectorData>> {
        Ok(&mut self.noise_detection)
    }
}

impl TargetQualityDataHandler for ExampleSequenceData {
    #[inline]
    fn get_target_quality(&self) -> Result<&TargetQualityData> {
        Ok(&self.target_quality)
    }

    #[inline]
    fn get_target_quality_mut(&mut self) -> Result<&mut TargetQualityData> {
        Ok(&mut self.target_quality)
    }
}

impl ParallelEncoderDataHandler for ExampleSequenceData {
    #[inline]
    fn get_parallel_encoder(&self) -> Result<&ParallelEncoderData> {
        Ok(&self.parallel_encoder)
    }

    #[inline]
    fn get_parallel_encoder_mut(&mut self) -> Result<&mut ParallelEncoderData> {
        Ok(&mut self.parallel_encoder)
    }
}

/// Build the filter list that resizes a clip.
fn resize_filters(
    width: Option<usize>,
    height: Option<usize>,
    format: Option<FFPixelFormat>,
) -> Vec<VapourSynthFilter> {
    vec![VapourSynthFilter::Resize {
        scaler: Some(Scaler::Bicubic),
        width,
        height,
        format,
    }]
}
