use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

use andean_condor::{
    models::{
        encoder::EncoderBase,
        sequence::scene_detector::{
            DEFAULT_MAX_SCENE_LENGTH_SECONDS,
            DEFAULT_MIN_SCENE_LENGTH_FRAMES,
            SceneDetectionMethod as CoreSCDMethod,
            ScenecutMethod,
        },
    },
    vapoursynth::vapoursynth_filters::VapourSynthFilter,
};
use clap::{Parser as ClapParser, Subcommand, value_parser};
use serde::{Deserialize, Serialize};
use strum::{Display as DisplayMacro, EnumString, IntoStaticStr};
use thiserror::Error;

use crate::commands::help_text::*;

pub mod config;
pub mod handlers;
pub mod help_text;
pub mod version;

#[derive(Debug, ClapParser)]
#[command(
    name = "condor",
    about = "A simple, extensible Commandline tool for the Condor chunked encoding framework.",
    version = "0.0.1",
    disable_version_flag = true
)]
pub struct CondorCli {
    #[command(subcommand)]
    pub command:            Option<Commands>,
    #[arg(long, global = true, value_name = "Config File", help = HELP_CONFIG_SHORT, long_help = HELP_CONFIG)]
    pub config_file:        Option<PathBuf>,
    #[arg(long, global = true, value_name = "Temporary Directory", help = HELP_TEMP_SHORT, long_help = HELP_TEMP)]
    pub temp:               Option<PathBuf>,
    #[arg(long, global = true, value_name = "Log File", help = HELP_LOGS_SHORT, long_help = HELP_LOGS)]
    pub logs:               Option<PathBuf>,
    /// Enable verbose output and logging.
    #[arg(long, global = true, default_value_t = false)]
    pub verbose:            bool,
    /// Display Encoder, VapourSynth and quality metric library installation
    /// information.
    #[arg(short('v'), long, global = true, default_value_t = false)]
    pub version:            bool,
    // Main command arguments
    #[arg(long, short('i'), value_name = "Input", help = HELP_INPUT_SHORT, long_help = HELP_INPUT)]
    pub input:              Option<PathBuf>,
    #[arg(long, short('o'), value_name = "Output", help = HELP_OUTPUT_SHORT, long_help = HELP_OUTPUT)]
    pub output:             Option<PathBuf>,
    #[arg(long, value_name = "Scene Detector Input", help = HELP_SCD_INPUT_SHORT, long_help = HELP_SCD_INPUT)]
    pub scd_input:          Option<PathBuf>,
    #[arg(long, value_name = "Target Quality Input", help = HELP_TQ_INPUT_SHORT, long_help = HELP_TQ_INPUT)]
    pub tq_input:           Option<PathBuf>,
    #[arg(long, value_name = "Decoder", help = HELP_DECODER_SHORT, long_help = HELP_DECODER)]
    pub decoder:            Option<DecoderMethod>,
    #[arg(long, value_name = "Decoder", help = HELP_SCD_DECODER_SHORT, long_help = HELP_SCD_DECODER)]
    pub scd_decoder:        Option<DecoderMethod>,
    #[arg(long, value_name = "Decoder", help = HELP_TQ_DECODER_SHORT, long_help = HELP_TQ_DECODER)]
    pub tq_decoder:         Option<DecoderMethod>,
    #[arg(long, value_name = "VapourSynth Filters", help = HELP_FILTERS_SHORT, long_help = HELP_FILTERS)]
    pub filters:            Option<Vec<VapourSynthFilter>>,
    #[arg(long, value_name = "VapourSynth Filters", help = HELP_SCD_FILTERS_SHORT, long_help = HELP_SCD_FILTERS)]
    pub scd_filters:        Option<Vec<VapourSynthFilter>>,
    #[arg(long, value_name = "VapourSynth Filters", help = HELP_TQ_FILTERS_SHORT, long_help = HELP_TQ_FILTERS)]
    pub tq_filters:         Option<Vec<VapourSynthFilter>>,
    #[arg(long, value_name = "VapourSynth Arguments", help = HELP_VS_ARGS_SHORT, long_help = HELP_VS_ARGS)]
    pub vs_args:            Option<Vec<String>>,
    #[arg(long, value_name = "VapourSynth Arguments", help = HELP_SCD_VS_ARGS_SHORT, long_help = HELP_SCD_VS_ARGS)]
    pub scd_vs_args:        Option<Vec<String>>,
    #[arg(long, value_name = "VapourSynth Arguments", help = HELP_TQ_VS_ARGS_SHORT, long_help = HELP_TQ_VS_ARGS)]
    pub tq_vs_args:         Option<Vec<String>>,
    #[arg(long, value_name = "Concatenation Method", help = HELP_CONCAT_SHORT, long_help = HELP_CONCAT)]
    pub concat:             Option<ConcatenationMethod>,
    #[arg(long, value_name = "Category", value_enum, value_delimiter = ',', help = HELP_DISCARD_SHORT, long_help = HELP_DISCARD)]
    pub discard:            Option<Vec<DiscardCategory>>,
    /// The amount of encoder processes to use at once
    #[arg(long, short('w'), value_name = "Workers", help = HELP_WORKERS_SHORT, long_help = HELP_WORKERS)]
    pub workers:            Option<u8>,
    #[arg(long, short('e'), value_name = "Encoder", help = HELP_ENCODER_SHORT, long_help = HELP_ENCODER)]
    pub encoder:            Option<EncoderMethod>,
    #[arg(long, value_name = "Passes", help = HELP_PASSES_SHORT, long_help = HELP_PASSES)]
    pub passes:             Option<u8>,
    #[arg(long, value_name = "Encoder Parameters", allow_hyphen_values = true, help = HELP_PARAMS_SHORT, long_help = HELP_PARAMS)]
    pub params:             Option<String>,
    #[arg(long, value_name = "Encoder Parameters", allow_hyphen_values = true, help = HELP_TQ_PARAMS_SHORT, long_help = HELP_TQ_PARAMS)]
    pub tq_params:          Option<String>,
    #[arg(long, value_name = "ISO", help = HELP_PHOTON_NOISE_SHORT, long_help = HELP_PHOTON_NOISE)]
    pub photon_noise:       Option<u32>,
    #[arg(long, value_name = "ISO", help = HELP_CHROMA_NOISE_SHORT, long_help = HELP_CHROMA_NOISE)]
    pub chroma_noise:       Option<u32>,
    #[arg(long, value_name = "FFmpeg Filter", allow_hyphen_values = true, help = HELP_FFMPEG_FILTER_SHORT, long_help = HELP_FFMPEG_FILTER)]
    pub ffmpeg_filter:      Option<String>,
    #[arg(long, default_value_t = false, help = HELP_DISABLE_COLOR_DATA_SHORT, long_help = HELP_DISABLE_COLOR_DATA)]
    pub disable_color_data: bool,
    #[arg(long, value_name = "Zone", allow_hyphen_values = true, help = HELP_ZONES_SHORT, long_help = HELP_ZONES)]
    pub zones:              Option<Vec<String>>,
    #[arg(long, value_name = "Metric", help = HELP_TARGET_METRIC_SHORT, long_help = HELP_TARGET_METRIC)]
    pub target_metric:      Option<QualityMetric>,
    #[arg(long, value_name = "Score", help = HELP_TARGET_SHORT, long_help = HELP_TARGET)]
    pub target:             Option<f64>,
    #[arg(long, value_name = "Quantizer", help = HELP_MINIMUM_QUANTIZER_SHORT, long_help = HELP_MINIMUM_QUANTIZER)]
    pub minimum_quantizer:  Option<u8>,
    #[arg(long, value_name = "Quantizer", help = HELP_MAXIMUM_QUANTIZER_SHORT, long_help = HELP_MAXIMUM_QUANTIZER)]
    pub maximum_quantizer:  Option<u8>,
    #[arg(long, value_name = "Profile", help = HELP_TARGET_QUALITY_PROFILE_SHORT, long_help = HELP_TARGET_QUALITY_PROFILE)]
    pub target_profile:     Option<QualityProfile>,
    /// Skip Scene Detection. Useful when encoding a subset of scenes.
    #[arg(long, default_value_t = false)]
    pub skip_scd:           bool,
}

impl CondorCli {
    /// Rejects `condor scenes INDEX add` as a usage error.
    ///
    /// `add` places the scene it inserts itself, so an index before it is not
    /// a form the command has; Clap cannot express an argument that conflicts
    /// with one subcommand, so the check runs after parsing and reports the
    /// conflict the way Clap would.
    pub(crate) fn reject_indexed_add(&self, cmd: &mut clap::Command) {
        if matches!(
            &self.command,
            Some(Commands::Scenes {
                index:  Some(_),
                action: Some(handlers::scenes::ScenesAction::Add { .. }),
            })
        ) {
            let scenes = cmd.find_subcommand_mut("scenes").expect("the scenes subcommand exists");
            scenes
                .error(
                    clap::error::ErrorKind::ArgumentConflict,
                    "`add` creates a new scene and takes no index: run `condor scenes add START \
                     END`",
                )
                .exit();
        }
    }

    /// Hyphen-accepting positionals make Clap capture flags after the first
    /// zone value. Recover only the reserved standalone dry-run flag.
    pub(crate) fn normalize_zone_flags(&mut self) {
        if let Some(Commands::Zone {
            zones,
            dry_run,
        }) = &mut self.command
        {
            zones.retain(|zone| {
                if zone == "--dry-run" {
                    *dry_run = true;
                    false
                } else {
                    true
                }
            });
        }
    }
}

/// The global `--verbose` flag, recorded once at startup so handlers that
/// print optional detail can consult it without threading it through every
/// call.
static VERBOSE: AtomicBool = AtomicBool::new(false);

/// Records the global `--verbose` flag; called once at startup.
pub fn set_verbose(verbose: bool) {
    VERBOSE.store(verbose, Ordering::Relaxed);
}

/// Whether `--verbose` was given. Output that only repeats what a status line
/// already says is printed only then.
pub fn is_verbose() -> bool {
    VERBOSE.load(Ordering::Relaxed)
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Initialize a new configuration.
    ///
    /// Creating a configuration takes INPUT and OUTPUT; both are required
    /// unless one of the configuration subcommands below is used.
    ///
    /// Also reads, changes or resets the top-level settings of an existing
    /// configuration without running anything: *condor init info*, *set* and
    /// *reset*.
    #[command(args_conflicts_with_subcommands = true)]
    Init {
        #[command(subcommand)]
        edit:               Option<handlers::init::InitAction>,
        #[arg(value_name = "Input", help = HELP_INPUT_SHORT, long_help = HELP_INPUT)]
        input:              Option<PathBuf>,
        #[arg(value_name = "Output", help = HELP_OUTPUT_SHORT, long_help = HELP_OUTPUT)]
        output:             Option<PathBuf>,
        #[arg(long, value_name = "Decoder", help = HELP_DECODER_SHORT, long_help = HELP_DECODER)]
        decoder:            Option<DecoderMethod>,
        #[arg(long, value_name = "VapourSynth Filters", help = HELP_FILTERS_SHORT, long_help = HELP_FILTERS)]
        filters:            Option<Vec<VapourSynthFilter>>,
        #[arg(long, value_name = "VapourSynth Arguments", help = HELP_VS_ARGS_SHORT, long_help = HELP_VS_ARGS)]
        vs_args:            Option<Vec<String>>,
        #[arg(long, value_name = "Concatenation Method", help = HELP_CONCAT_SHORT, long_help = HELP_CONCAT)]
        concat:             Option<ConcatenationMethod>,
        #[arg(long, value_name = "Category", value_enum, value_delimiter = ',', help = HELP_DISCARD_SHORT, long_help = HELP_DISCARD)]
        discard:            Option<Vec<DiscardCategory>>,
        /// The amount of encoder processes to use at once
        #[arg(long, short('w'), value_name = "Workers", help = HELP_WORKERS_SHORT, long_help = HELP_WORKERS)]
        workers:            Option<u8>,
        #[arg(long, short('e'), value_name = "Encoder", help = HELP_ENCODER_SHORT, long_help = HELP_ENCODER)]
        encoder:            Option<EncoderMethod>,
        #[arg(long, value_name = "Encoder Parameters", allow_hyphen_values = true, help = HELP_PARAMS_SHORT, long_help = HELP_PARAMS)]
        params:             Option<String>,
        #[arg(long, value_name = "ISO", help = HELP_PHOTON_NOISE_SHORT, long_help = HELP_PHOTON_NOISE)]
        photon_noise:       Option<u32>,
        /// Do not append the input's colour and HDR metadata to the encoder
        /// parameters.
        #[arg(long, default_value_t = false, help = HELP_DISABLE_COLOR_DATA_SHORT, long_help = HELP_DISABLE_COLOR_DATA)]
        disable_color_data: bool,
        #[arg(long, value_name = "Metric", help = HELP_TARGET_METRIC_SHORT, long_help = HELP_TARGET_METRIC)]
        target_metric:      Option<QualityMetric>,
        #[arg(long, value_name = "Score", help = HELP_TARGET_SHORT, long_help = HELP_TARGET)]
        target:             Option<f64>,
    },
    /// Detect scenes (Triggers TUI).
    ///
    /// Also reads, changes or resets its saved settings without running:
    /// *condor detect-scenes info*, *set* and *reset*.
    #[command(args_conflicts_with_subcommands = true)]
    DetectScenes {
        #[command(subcommand)]
        edit:              Option<handlers::detect_scenes::DetectScenesAction>,
        #[arg(long, short('i'), value_name = "Input", help = HELP_SCD_INPUT_SHORT, long_help = HELP_SCD_INPUT)]
        input:             Option<PathBuf>,
        #[arg(long, value_name = "Decoder", help = HELP_DECODER_SHORT, long_help = HELP_DECODER)]
        decoder:           Option<DecoderMethod>,
        #[arg(long, value_name = "VapourSynth Filters", help = HELP_SCD_FILTERS_SHORT, long_help = HELP_SCD_FILTERS)]
        filters:           Option<Vec<VapourSynthFilter>>,
        #[arg(long, value_name = "VapourSynth Arguments", help = HELP_SCD_VS_ARGS_SHORT, long_help = HELP_SCD_VS_ARGS)]
        vs_args:           Option<Vec<String>>,
        #[arg(long, value_name = "Scene Detection Method", help = HELP_SCD_METHOD_SHORT, long_help = HELP_SCD_METHOD)]
        method:            Option<SceneDetectionMethod>,
        #[arg(long, value_name = "Scene Duration", help = HELP_MIN_SCENE_SECONDS_SHORT, long_help = HELP_MIN_SCENE_SECONDS)]
        min_scene_seconds: Option<usize>,
        #[arg(long, value_name = "Scene Duration", help = HELP_MAX_SCENE_SECONDS_SHORT, long_help = HELP_MAX_SCENE_SECONDS)]
        max_scene_seconds: Option<usize>,
    },
    /// Apply zone overrides to the scenes inside their frame ranges.
    ///
    /// Zones mark the scenes that share a frame with them, merging their
    /// encoder settings into the scenes' own or replacing the encoder outright,
    /// and splitting scenes longer than the zone's maximum.
    Zone {
        /// Zone definitions, or paths to JSON, JSON lines, or text zone files.
        #[arg(value_name = "Zone", allow_hyphen_values = true)]
        zones:   Vec<String>,
        /// Report planned scene changes without saving configuration or
        /// changing outputs.
        #[arg(long)]
        dry_run: bool,
    },
    /// Inspect and edit the scenes saved in the configuration.
    ///
    /// The index comes first and selects the scenes the action addresses:
    /// `1`, `0-4`, `1,3`, `1-2,8-10,16`, or `-1`/`all`/`every` for the whole
    /// list. Scenes hold everything the pipeline saved per scene: frame
    /// ranges, sub-scenes, the per-scene encoder and the step results. *info*
    /// prints them as JSON, *set* changes them, *reset* restores them and
    /// *delete* removes them. Without an action the addressed scenes are
    /// printed.
    ///
    /// To add a scene, run `condor scenes add START END`: it places the new
    /// scene itself, so it takes no index.
    Scenes {
        /// Which scenes to address: `1`, `0-4`, `1,3`, `1-2,8-10,16`, or
        /// `-1`/`all`/`every` for every scene.
        #[arg(value_name = "Index", allow_hyphen_values = true)]
        index:  Option<crate::scenes::SceneSelection>,
        #[command(subcommand)]
        action: Option<handlers::scenes::ScenesAction>,
    },
    /// Detect noise per scene (Triggers TUI).
    ///
    /// Also reads, changes or resets its saved settings without running:
    /// *condor detect-noise info*, *set* and *reset*.
    #[command(args_conflicts_with_subcommands = true)]
    DetectNoise {
        #[command(subcommand)]
        edit:              Option<handlers::detect_noise::DetectNoiseAction>,
        #[arg(long, value_name = "Noise Detector Input", help = HELP_ND_INPUT_SHORT, long_help = HELP_ND_INPUT)]
        input:             Option<PathBuf>,
        #[arg(long, value_name = "Decoder", help = HELP_DECODER_SHORT, long_help = HELP_DECODER)]
        decoder:           Option<DecoderMethod>,
        #[arg(long, value_name = "VapourSynth Arguments", help = HELP_VS_ARGS_SHORT, long_help = HELP_VS_ARGS)]
        vs_args:           Option<Vec<String>>,
        #[arg(long, value_name = "Reference VapourSynth Filters", help = HELP_REFERENCE_FILTERS_SHORT, long_help = HELP_REFERENCE_FILTERS)]
        reference_filters: Option<Vec<VapourSynthFilter>>,
        #[arg(long, value_name = "Denoised VapourSynth Filters", help = HELP_DENOISED_FILTERS_SHORT, long_help = HELP_DENOISED_FILTERS)]
        denoised_filters:  Option<Vec<VapourSynthFilter>>,
    },
    /// Scale Photon Noise ISO per scene based on Noise Detector results.
    ///
    /// Scenes must have Photon Noise (*--photon-noise*/*--chroma-noise*)
    /// configured. Scenes below the threshold will not be scaled.
    ///
    /// Also reads, changes or resets its saved settings without running:
    /// *condor scale-noise info*, *set* and *reset*.
    #[command(args_conflicts_with_subcommands = true)]
    ScaleNoise {
        #[command(subcommand)]
        edit:           Option<handlers::scale_noise::ScaleNoiseAction>,
        /// Minimum noise value for a scene to scale Photon Noise ISO.
        ///
        /// Recommended: 0.002
        #[arg(long, value_name = "Threshold")]
        threshold:      Option<f64>,
        /// Minimum scale factor for Photon Noise ISO scaling.
        #[arg(long, value_name = "Scaler")]
        minimum_scaler: Option<f64>,
        /// Maximum scale factor for Photon Noise ISO scaling.
        #[arg(long, value_name = "Scaler")]
        maximum_scaler: Option<f64>,
        /// Scale Chroma ISO.
        #[arg(long)]
        scale_chroma:   bool,
    },
    /// Benchmark the optimum amount of workers (Triggers TUI).
    ///
    /// Also reads, changes or resets its saved settings without running:
    /// *condor benchmark info*, *set* and *reset*.
    #[command(args_conflicts_with_subcommands = true)]
    Benchmark {
        #[command(subcommand)]
        edit:       Option<handlers::benchmarker::BenchmarkAction>,
        /// The minimum speed increase (in percent) required to add an
        /// additional worker.
        ///
        /// Defaults to `5`.
        #[arg(long, value_name = "Percent", value_parser = value_parser!(u8).range(0..=100))]
        threshold:  Option<u8>,
        /// The maximum amount of RAM (in megabytes) allowed across all workers
        /// (unimplemented)
        #[arg(long, value_name = "Megabytes", hide = true)]
        max_memory: Option<u32>,
    },
    /// Calculate the optimum quantizer per scene for a given metric target
    /// (Triggers TUI).
    ///
    /// Also reads, changes or resets its saved settings without running:
    /// *condor target-quality info*, *set* and *reset*.
    #[command(args_conflicts_with_subcommands = true)]
    TargetQuality {
        #[command(subcommand)]
        edit:              Option<handlers::target_quality::TargetQualityAction>,
        #[arg(long, short('i'), value_name = "Input", help = HELP_INPUT_SHORT, long_help = HELP_INPUT)]
        input:             Option<PathBuf>,
        #[arg(long, value_name = "Decoder", help = HELP_DECODER_SHORT, long_help = HELP_DECODER)]
        decoder:           Option<DecoderMethod>,
        #[arg(long, value_name = "VapourSynth Filters")]
        filters:           Option<Vec<VapourSynthFilter>>,
        #[arg(long, value_name = "VapourSynth Arguments", help = HELP_VS_ARGS_SHORT, long_help = HELP_VS_ARGS)]
        vs_args:           Option<Vec<String>>,
        #[arg(long, value_name = "Encoder Parameters", allow_hyphen_values = true, help = HELP_PARAMS_SHORT, long_help = HELP_PARAMS)]
        params:            Option<String>,
        #[arg(long, value_name = "Metric", help = HELP_TARGET_METRIC_SHORT, long_help = HELP_TARGET_METRIC)]
        metric:            Option<QualityMetric>,
        #[arg(long, value_name = "Score", help = HELP_TARGET_SHORT, long_help = HELP_TARGET)]
        target:            Option<f64>,
        #[arg(long("min-q"), value_name = "Quantizer", help = HELP_MINIMUM_QUANTIZER_SHORT, long_help = HELP_MINIMUM_QUANTIZER)]
        minimum_quantizer: Option<u8>,
        #[arg(long("max-q"), value_name = "Quantizer", help = HELP_MAXIMUM_QUANTIZER_SHORT, long_help = HELP_MAXIMUM_QUANTIZER)]
        maximum_quantizer: Option<u8>,
        #[arg(long, value_name = "Profile", help = HELP_TARGET_QUALITY_PROFILE_SHORT, long_help = HELP_TARGET_QUALITY_PROFILE)]
        profile:           Option<QualityProfile>,
    },
    /// Optimize bitrate for scenes that exceed normal bitrate after Target
    /// Quality.
    ///
    /// Scenes with a bitrate above the normal threshold will be optimized by
    /// clamping the quantizer to the average quantizer value.
    ///
    /// Also reads, changes or resets its saved settings without running:
    /// *condor optimize-bitrate info*, *set* and *reset*.
    #[command(args_conflicts_with_subcommands = true)]
    OptimizeBitrate {
        #[command(subcommand)]
        edit:            Option<handlers::optimize_bitrate::OptimizeBitrateAction>,
        /// Minimum bitrate sigma (σ) threshold for a scene to be optimized.
        #[arg(long, value_name = "Sigma", value_parser = value_parser!(u8).range(1..=10))]
        sigma_threshold: Option<u8>,
    },
    /// Apply speed based on quantizer per scene using Convex Hull
    /// interpolation.
    ///
    /// Speeds for scenes with quantizers between points will be interpolated
    /// between the nearest points.
    ///
    /// Also reads, changes or resets its saved settings without running:
    /// *condor scale-speed info*, *set* and *reset*.
    #[command(args_conflicts_with_subcommands = true)]
    ScaleSpeed {
        #[command(subcommand)]
        edit:       Option<handlers::scale_speed::ScaleSpeedAction>,
        /// Quantizer values for speed-quantizer pairs. (must match number of
        /// speeds)
        ///
        /// Examples:
        ///
        /// - `--encoder svt-av1 --quantizers 20 --speeds 5 --quantizers 30
        ///   --speeds 4 --quantizers 55 --speeds 2`
        ///
        /// - `--encoder aom --quantizers 10 --speeds 6 --quantizers 25 --speeds
        ///   4 --quantizers 40 --speeds 3`
        #[arg(long, value_name = "Quantizer", num_args = 1.., requires = "speeds")]
        quantizers: Option<Vec<i8>>,
        /// Speed values for speed-quantizer pairs (must match number of
        /// quantizers)
        ///
        /// Examples:
        ///
        /// - `--encoder svt-av1 --quantizers 20 --speeds 5 --quantizers 30
        ///   --speeds 4 --quantizers 55 --speeds 2`
        ///
        /// - `--encoder aom --quantizers 10 --speeds 6 --quantizers 25 --speeds
        ///   4 --quantizers 40 --speeds 3`
        #[arg(long, value_name = "Speed", num_args = 1.., requires = "quantizers")]
        speeds:     Option<Vec<i8>>,
    },
    /// Encode scenes in parallel (Triggers TUI).
    ///
    /// Also reads, changes or resets its saved settings without running:
    /// *condor encode info*, *set* and *reset*.
    #[command(args_conflicts_with_subcommands = true)]
    Encode {
        #[command(subcommand)]
        edit:          Option<handlers::encode::EncodeAction>,
        #[arg(long, short('i'), value_name = "Input", help = HELP_INPUT_SHORT, long_help = HELP_INPUT)]
        input:         Option<PathBuf>,
        #[arg(long, value_name = "Decoder", help = HELP_DECODER_SHORT, long_help = HELP_DECODER)]
        decoder:       Option<DecoderMethod>,
        #[arg(long, value_name = "VapourSynth Filters", help = HELP_FILTERS_SHORT, long_help = HELP_FILTERS)]
        filters:       Option<Vec<VapourSynthFilter>>,
        #[arg(long, value_name = "VapourSynth Arguments", help = HELP_VS_ARGS_SHORT, long_help = HELP_VS_ARGS)]
        vs_args:       Option<Vec<String>>,
        /// The amount of encoder processes to use at once
        #[arg(long, short('w'), value_name = "Workers", help = HELP_WORKERS_SHORT, long_help = HELP_WORKERS)]
        workers:       Option<u8>,
        #[arg(long, short('e'), value_name = "Encoder", help = HELP_ENCODER_SHORT, long_help = HELP_ENCODER)]
        encoder:       Option<EncoderMethod>,
        #[arg(long, value_name = "Passes", help = HELP_PASSES_SHORT, long_help = HELP_PASSES)]
        passes:        Option<u8>,
        #[arg(long, value_name = "Encoder Parameters", allow_hyphen_values = true, help = HELP_PARAMS_SHORT, long_help = HELP_PARAMS)]
        params:        Option<String>,
        #[arg(long, value_name = "ISO", help = HELP_PHOTON_NOISE_SHORT, long_help = HELP_PHOTON_NOISE)]
        photon_noise:  Option<u32>,
        #[arg(long, value_name = "ISO", help = HELP_CHROMA_NOISE_SHORT, long_help = HELP_CHROMA_NOISE)]
        chroma_noise:  Option<u32>,
        #[arg(long, value_name = "FFmpeg Filter", allow_hyphen_values = true, help = HELP_FFMPEG_FILTER_SHORT, long_help = HELP_FFMPEG_FILTER)]
        ffmpeg_filter: Option<String>,
    },
    /// Concatenate encoded scenes into output video (Triggers TUI).
    ///
    /// Also reads, changes or resets its saved settings without running:
    /// *condor concatenate info*, *set* and *reset*.
    #[command(args_conflicts_with_subcommands = true)]
    Concatenate {
        #[command(subcommand)]
        edit:    Option<handlers::concatenate::ConcatenateAction>,
        #[arg(long, value_name = "Concatenation Method", help = HELP_CONCAT_SHORT, long_help = HELP_CONCAT)]
        method:  Option<ConcatenationMethod>,
        #[arg(long, value_name = "Category", value_enum, value_delimiter = ',', help = HELP_DISCARD_SHORT, long_help = HELP_DISCARD)]
        discard: Option<Vec<DiscardCategory>>,
    },
    /// Measure the quality of the concatenated output per scene (Triggers
    /// TUI).
    ///
    /// Also reads, changes or resets its saved settings without running:
    /// *condor quality-check info*, *set* and *reset*.
    #[command(args_conflicts_with_subcommands = true)]
    QualityCheck {
        #[command(subcommand)]
        edit:    Option<handlers::quality_check::QualityCheckAction>,
        #[arg(long, short('i'), value_name = "Input", help = HELP_INPUT_SHORT, long_help = HELP_INPUT)]
        input:   Option<PathBuf>,
        #[arg(long, value_name = "Decoder", help = HELP_DECODER_SHORT, long_help = HELP_DECODER)]
        decoder: Option<DecoderMethod>,
        #[arg(long, value_name = "VapourSynth Filters", help = HELP_FILTERS_SHORT, long_help = HELP_FILTERS)]
        filters: Option<Vec<VapourSynthFilter>>,
        #[arg(long, value_name = "VapourSynth Arguments", help = HELP_VS_ARGS_SHORT, long_help = HELP_VS_ARGS)]
        vs_args: Option<Vec<String>>,
        #[arg(long, value_name = "Metric", help = HELP_QUALITY_CHECK_METRIC_SHORT, long_help = HELP_QUALITY_CHECK_METRIC)]
        metric:  Option<QualityMetric>,
        #[arg(long, value_name = "Profile", help = HELP_QUALITY_CHECK_PROFILE_SHORT, long_help = HELP_QUALITY_CHECK_PROFILE)]
        profile: Option<QualityProfile>,
    },
    /// Clean temporary files.
    Clean {
        #[arg(long, value_name = "All")]
        all: bool,
    },
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    EnumString,
    IntoStaticStr,
    DisplayMacro,
    clap::ValueEnum,
)]
pub enum SceneDetectionMethod {
    /// No scene detection, chunks scenes by maximum length
    #[strum(serialize = "none")]
    #[value(name = "none")]
    None,
    /// Fast scene detection, uses av-scenechange with the fast algorithm
    #[strum(serialize = "fast")]
    #[value(name = "fast")]
    Fast,
    /// Standard scene detection, uses av-scenechange with the standard
    /// algorithm
    #[strum(serialize = "standard")]
    #[value(name = "standard")]
    Standard,
    /// Neural network scene detection, uses TransNetV2 via ONNX Runtime
    #[strum(serialize = "transnetv2")]
    #[value(name = "transnetv2")]
    TransNetV2,
}

impl SceneDetectionMethod {
    pub fn as_core_method(
        &self,
        minimum_length: Option<usize>,
        maximum_length: Option<usize>,
    ) -> CoreSCDMethod {
        let min_length = minimum_length.unwrap_or(DEFAULT_MIN_SCENE_LENGTH_FRAMES as usize);
        let max_length = maximum_length.unwrap_or(
            DEFAULT_MAX_SCENE_LENGTH_SECONDS as usize * DEFAULT_MIN_SCENE_LENGTH_FRAMES as usize,
        );
        match self {
            SceneDetectionMethod::None => CoreSCDMethod::None {
                minimum_length: min_length,
                maximum_length: max_length,
            },
            SceneDetectionMethod::Fast => CoreSCDMethod::AVSceneChange {
                method:         ScenecutMethod::Fast,
                minimum_length: min_length,
                maximum_length: max_length,
                save_scores:    false,
            },
            SceneDetectionMethod::Standard => CoreSCDMethod::AVSceneChange {
                method:         ScenecutMethod::Standard,
                minimum_length: min_length,
                maximum_length: max_length,
                save_scores:    false,
            },
            SceneDetectionMethod::TransNetV2 => CoreSCDMethod::TransNetV2 {
                minimum_length: min_length,
                maximum_length: max_length,
                model_path:     None,
                save_scores:    false,
            },
        }
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    EnumString,
    IntoStaticStr,
    DisplayMacro,
    clap::ValueEnum,
)]
pub enum DecoderMethod {
    /// BestSource VapourSynth plugin
    #[strum(serialize = "bestsource")]
    #[value(name = "bestsource")]
    BestSource,
    /// FFmpegSource VapourSynth plugin
    #[strum(serialize = "vs-ffms2")]
    #[value(name = "vs-ffms2")]
    VSFFMS2,
    /// LSMASH VapourSynth plugin
    #[strum(serialize = "lsmash")]
    #[value(name = "lsmash")]
    LSMASHWorks,
    /// DGDecodeNV VapourSynth plugin
    #[strum(serialize = "dgdecnv")]
    #[value(name = "dgdecnv")]
    DGDecodeNV,
    /// FFmpegSource (integrated)
    #[strum(serialize = "ffms2")]
    #[value(name = "ffms2")]
    FFMS2,
}

#[allow(clippy::upper_case_acronyms)]
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    EnumString,
    IntoStaticStr,
    DisplayMacro,
    clap::ValueEnum,
)]
pub enum EncoderMethod {
    /// Alliance for Open Media AV1 encoder
    #[strum(serialize = "aom")]
    #[value(name = "aom")]
    AOM,
    /// Rust AV1 encoder
    #[strum(serialize = "rav1e")]
    #[value(name = "rav1e")]
    RAV1E,
    /// Alliance for Open Media VP8/VP9
    #[strum(serialize = "vpx")]
    #[value(name = "vpx")]
    VPX,
    /// Scalable Video Technology for AV1
    #[strum(serialize = "svt-av1")]
    #[value(name = "svt-av1")]
    SVTAV1,
    /// Alliance for Open Media Video Model
    #[strum(serialize = "avm")]
    #[value(name = "avm")]
    AVM,
    /// VideoLAN x264
    #[strum(serialize = "x264")]
    #[value(name = "x264")]
    X264,
    /// MulticoreWare x265
    #[strum(serialize = "x265")]
    #[value(name = "x265")]
    X265,
    /// Fraunhofer Versatile Video Encoder
    #[strum(serialize = "vvenc")]
    #[value(name = "vvenc")]
    VVenC,
    /// FFmpeg
    #[strum(serialize = "ffmpeg")]
    #[value(name = "ffmpeg")]
    FFmpeg,
}

impl EncoderMethod {
    pub fn as_encoder_base(&self) -> EncoderBase {
        match self {
            EncoderMethod::AOM => EncoderBase::AOM,
            EncoderMethod::RAV1E => EncoderBase::RAV1E,
            EncoderMethod::VPX => EncoderBase::VPX,
            EncoderMethod::SVTAV1 => EncoderBase::SVTAV1,
            EncoderMethod::AVM => EncoderBase::AVM,
            EncoderMethod::X264 => EncoderBase::X264,
            EncoderMethod::X265 => EncoderBase::X265,
            EncoderMethod::VVenC => EncoderBase::VVenC,
            EncoderMethod::FFmpeg => EncoderBase::FFmpeg,
        }
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    EnumString,
    IntoStaticStr,
    DisplayMacro,
    clap::ValueEnum,
)]
pub enum ConcatenationMethod {
    /// MKVToolNix mkvmerge -- Merge multimedia streams into a Matroska™ file
    ///
    /// Generally the best concatenation method, but can only produce Matroska™
    /// (.mkv) files. Requires mkvmerge to be installed.
    #[strum(serialize = "mkvmerge")]
    #[value(name = "mkvmerge")]
    MkvMerge,
    /// FFmpeg command line tool
    ///
    /// Can mux into formats besides Matroska (.mkv) and Indeo Video Format
    /// (.ivf). Unfortunately, sometimes produces file with partially broken
    /// audio seeking, so mkvmerge should generally be preferred if available.
    /// Also produces broken files with the `--enable-keyframe filtering=2`
    /// option in aomenc. Requires FFmpeg to be installed.
    #[strum(serialize = "ffmpeg")]
    #[value(name = "ffmpeg")]
    FFmpeg,
    /// Indeo Video Format (.ivf)
    ///
    /// Only supports concatenation of IVF files and will not include other
    /// streams or metadata (audio, subtitle, chapters, etc.) from the input.
    #[strum(serialize = "ivf")]
    #[value(name = "ivf")]
    Ivf,
}

/// A kind of source content `--discard` can leave out of the output.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    EnumString,
    IntoStaticStr,
    DisplayMacro,
    clap::ValueEnum,
)]
pub enum DiscardCategory {
    /// Audio tracks.
    #[strum(serialize = "audio")]
    #[value(name = "audio")]
    Audio,
    /// Subtitle tracks.
    #[strum(serialize = "subtitle")]
    #[value(name = "subtitle")]
    Subtitle,
    /// Attachments, such as fonts.
    #[strum(serialize = "attachment")]
    #[value(name = "attachment")]
    Attachment,
    /// Global metadata/tags, including the title.
    #[strum(serialize = "metadata")]
    #[value(name = "metadata")]
    Metadata,
    /// Chapters.
    #[strum(serialize = "chapters")]
    #[value(name = "chapters")]
    Chapters,
    /// The encoded video, leaving an audio/subtitle-only output.
    #[strum(serialize = "video")]
    #[value(name = "video")]
    Video,
}

#[allow(clippy::upper_case_acronyms)]
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    EnumString,
    IntoStaticStr,
    DisplayMacro,
    clap::ValueEnum,
)]
pub enum QualityMetric {
    /// Video Multi-Method Assessment Fusion
    ///
    /// Requires libvmaf at runtime. Set `VMAF_LIB_DIR` if it is installed
    /// somewhere non-standard. GPU acceleration is used when libvmaf was
    /// built with CUDA; otherwise the CPU backend is selected
    /// automatically.
    #[strum(serialize = "vmaf")]
    #[value(name = "vmaf")]
    VMAF,
    /// Structural SIMilarity Unveiling Local And Compression Related Artifacts
    ///
    /// Runs through libvship (GPU) or libfmetrics (CPU), falling back to the
    /// Vapoursynth-HIP / vszip plugin only when neither is available.
    #[strum(serialize = "ssimulacra2")]
    #[value(name = "ssimulacra2")]
    SSIMULACRA2,
    /// butteraugli Infinite-Norm
    ///
    /// Runs through libvship (GPU) or libfmetrics (CPU), falling back to the
    /// Vapoursynth-HIP / julek plugin only when neither is available.
    #[strum(serialize = "butteraugli")]
    #[value(name = "butteraugli")]
    BUTTERAUGLI,
    /// butteraugli 3-Norm
    ///
    /// Runs through libvship (GPU) or libfmetrics (CPU), falling back to the
    /// Vapoursynth-HIP / julek plugin only when neither is available.
    #[strum(serialize = "butteraugli-3")]
    #[value(name = "butteraugli-3")]
    BUTTERAUGLI3Norm,
    /// Extended Perceptually Weighted Peak Signal-to-Noise Ratio
    ///
    /// Uses the minimum of the `Y`, `U`, and `V` scores.
    ///
    /// Requires the Vapoursynth-Zig Image Process plugin; no native
    /// implementation.
    #[strum(serialize = "xpsnr")]
    #[value(name = "xpsnr")]
    XPSNR,
    /// ColorVideoVDP
    ///
    /// Runs on the GPU through libvship, falling back to the Vapoursynth-HIP
    /// plugin only when libvship is unavailable.
    #[strum(serialize = "cvvdp")]
    CVVDP,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    EnumString,
    IntoStaticStr,
    DisplayMacro,
    clap::ValueEnum,
)]
pub enum QualityProfile {
    /// Fast
    ///
    /// Measures the average of the middle 11 frames.
    #[strum(serialize = "fast")]
    #[value(name = "fast")]
    Fast,
    /// Standard
    ///
    /// Measures the root-mean-square of the middle 25% of frames.
    #[strum(serialize = "standard")]
    #[value(name = "standard")]
    Standard,
    /// Slow
    ///
    /// Measures the 10th percentile of all frames.
    #[strum(serialize = "slow")]
    #[value(name = "slow")]
    Slow,
}

#[derive(Debug, Error)]
pub enum CondorCliError {
    #[error("Cannot initialize over an existing config file: {0}")]
    ConfigFileAlreadyExists(PathBuf),
    #[error("No config file found at: {0}")]
    ConfigFileNotFound(PathBuf),
    #[error("Failed to load config file: {0}")]
    ConfigLoadError(PathBuf),
    #[error("Failed to parse config file: {0}")]
    ConfigParseError(serde_json::Error),
    #[error("Failed to serialize config file: {0}")]
    ConfigSerializeError(serde_json::Error),
    #[error("Failed to save config file: {0}")]
    ConfigSaveError(std::io::Error),
    #[error("Cannot start without a config file or without input path")]
    NoConfigOrInput,
    #[error("Cannot start without a config file or without input and output paths")]
    NoConfigOrInputOrOutput,
    #[error("Cannot set Decoder without a valid Input path")]
    DecoderWithoutInput,
    #[error("No config file found. Run 'condor init' to create a configuration.")]
    NoConfig,
    #[error("No scenes found in the config. Run 'condor detect-scenes' to populate scenes")]
    NoScenes,
    #[error("Input {0} must be a VapourSynth script (.vpy or .py)")]
    InvalidVapourSynthScript(PathBuf),
}

#[cfg(test)]
mod tests {
    use std::assert_matches;

    use andean_condor::models::{
        encoder::EncoderBase,
        sequence::scene_detector::{SceneDetectionMethod as CoreSCDMethod, ScenecutMethod},
    };
    use clap::Parser;

    use super::{
        Commands,
        ConcatenationMethod,
        CondorCli,
        DecoderMethod,
        DiscardCategory,
        EncoderMethod,
        QualityMetric,
        QualityProfile,
        SceneDetectionMethod,
    };
    use crate::test_helpers::get_test_video;

    mod cli_parser {
        use super::*;

        mod init {
            use super::*;

            #[test]
            fn default() {
                let test_video = get_test_video();
                let temp = tempfile::tempdir().expect("temp directory");
                let output = temp.path().join("out.mkv");

                let result = CondorCli::try_parse_from([
                    "condor",
                    "init",
                    test_video.path.to_str().expect("test_video path is valid"),
                    output.to_str().expect("output path is valid"),
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Init { .. }),
                        ..
                    }),
                    "\"condor init INPUT OUTPUT\" parses"
                );
            }

            #[test]
            fn with_relative_paths() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "init",
                    "./relative-input.mov",
                    "./relative-output.ivf",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Init { .. }),
                        ..
                    }),
                    "\"condor init ./relative-input.mov ./relative-output.ivf\" parses"
                );
            }

            #[test]
            fn with_optional_flags() {
                let test_video = get_test_video();
                let temp = tempfile::tempdir().expect("temp directory");
                let output = temp.path().join("out.mkv");
                let config_path = temp.path().join("condor.json");
                let logs_path = temp.path().join("condor.log");

                let result = CondorCli::try_parse_from([
                    "condor",
                    "--config-file",
                    config_path.to_str().expect("config path is valid"),
                    "--temp",
                    temp.path().to_str().expect("temp path is valid"),
                    "--logs",
                    logs_path.to_str().expect("log path is valid"),
                    "init",
                    test_video.path.to_str().expect("test_video path is valid"),
                    output.to_str().expect("output path is valid"),
                    "--encoder",
                    "x264",
                    "--workers",
                    "4",
                    "--photon-noise",
                    "2400",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Init { .. }),
                        ..
                    }),
                    "\"condor init INPUT OUTPUT --encoder x264 --workers 4 --photon-noise 2400\" \
                     parses"
                );
            }

            /// INPUT and OUTPUT are optional at parse time because `init` also
            /// carries `info`/`set`/`reset`; the missing one is reported by the
            /// command itself (see `handlers::init::require_paths`).
            #[test]
            fn without_output() {
                let test_video = get_test_video();

                let result = CondorCli::try_parse_from([
                    "condor",
                    "init",
                    test_video.path.to_str().expect("test_video path is valid"),
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Init {
                            edit: None,
                            input: Some(_),
                            output: None,
                            ..
                        }),
                        ..
                    }),
                    "\"condor init input\" parses and leaves output unset"
                );
            }

            #[test]
            fn with_disable_color_data() {
                let test_video = get_test_video();
                let temp = tempfile::tempdir().expect("temp directory");
                let output = temp.path().join("out.mkv");

                let result = CondorCli::try_parse_from([
                    "condor",
                    "init",
                    test_video.path.to_str().expect("test_video path is valid"),
                    output.to_str().expect("output path is valid"),
                    "--disable-color-data",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Init {
                            disable_color_data: true,
                            ..
                        }),
                        ..
                    }),
                    "\"condor init INPUT OUTPUT --disable-color-data\" parses"
                );
            }

            #[test]
            fn with_invalid_optional_flags() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "init",
                    "./relative-input.mov",
                    "./relative-output.ivf",
                    "--encoder",
                    "x263",
                ]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor init INPUT OUTPUT --encoder x263\" does not parse"
                );
                let result = CondorCli::try_parse_from([
                    "condor",
                    "init",
                    "C:\\absolute-input.mov",
                    "C:\\output\\absolute-output.mkv",
                    "--workers",
                    "-2",
                ]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor init INPUT OUTPUT --workers -2\" does not parse"
                );
                let result = CondorCli::try_parse_from([
                    "condor",
                    "init",
                    "../relative-input.mov",
                    "../relative-output.ivf",
                    "--photon-noise",
                    "-24.8",
                ]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor init INPUT OUTPUT --photon-noise -24.8\" does not parse"
                );
            }

            #[test]
            fn with_discard() {
                let test_video = get_test_video();
                let temp = tempfile::tempdir().expect("temp directory");
                let output = temp.path().join("out.mkv");

                let result = CondorCli::try_parse_from([
                    "condor",
                    "init",
                    test_video.path.to_str().expect("test_video path is valid"),
                    output.to_str().expect("output path is valid"),
                    "--discard",
                    "audio,subtitle",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Init {
                            discard: Some(_),
                            ..
                        }),
                        ..
                    }),
                    "\"condor init INPUT OUTPUT --discard audio,subtitle\" parses"
                );
                if let Ok(CondorCli {
                    command:
                        Some(Commands::Init {
                            discard: Some(categories),
                            ..
                        }),
                    ..
                }) = CondorCli::try_parse_from([
                    "condor",
                    "init",
                    test_video.path.to_str().expect("test_video path is valid"),
                    output.to_str().expect("output path is valid"),
                    "--discard",
                    "audio,subtitle",
                ]) {
                    assert_eq!(categories, vec![
                        DiscardCategory::Audio,
                        DiscardCategory::Subtitle
                    ]);
                }
            }

            #[test]
            fn with_invalid_discard() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "init",
                    "./relative-input.mov",
                    "./relative-output.ivf",
                    "--discard",
                    "fonts",
                ]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor init INPUT OUTPUT --discard fonts\" does not parse"
                );
            }
        }

        mod start {
            use super::*;

            #[test]
            fn default() {
                let test_video = get_test_video();

                let result = CondorCli::try_parse_from([
                    "condor",
                    "--input",
                    test_video.path.to_str().expect("test_video path is valid"),
                    "--output",
                    "./out.mkv",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: None,
                        ..
                    }),
                    "\"condor --input INPUT --output OUTPUT\" parses"
                );
            }

            #[test]
            fn without_input() {
                let temp = tempfile::tempdir().expect("temp directory");
                let output = temp.path().join("out.mkv");

                let result = CondorCli::try_parse_from([
                    "condor",
                    "--output",
                    output.to_str().expect("output path is valid"),
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: None,
                        ..
                    }),
                    "\"condor --output OUTPUT\" without --input parses"
                );
            }

            #[test]
            fn without_output() {
                let test_video = get_test_video();

                let result = CondorCli::try_parse_from([
                    "condor",
                    "--input",
                    test_video.path.to_str().expect("test_video path exists"),
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: None,
                        ..
                    }),
                    "\"condor init --input INPUT\" without --output parses"
                );
            }

            #[test]
            fn without_parameters() {
                let result = CondorCli::try_parse_from(["condor"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: None,
                        ..
                    }),
                    "\"condor\" parses"
                );
            }

            #[test]
            fn with_ffmpeg_filter() {
                let test_video = get_test_video();

                let result = CondorCli::try_parse_from([
                    "condor",
                    "--input",
                    test_video.path.to_str().expect("test_video path is valid"),
                    "--output",
                    "./out.mkv",
                    "--ffmpeg-filter",
                    "crop=iw-16:ih-16",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: None,
                        ffmpeg_filter: Some(_),
                        ..
                    }),
                    "\"condor --input INPUT --output OUTPUT --ffmpeg-filter ...\" parses"
                );
            }

            #[test]
            fn with_disable_color_data() {
                let test_video = get_test_video();

                let result = CondorCli::try_parse_from([
                    "condor",
                    "--input",
                    test_video.path.to_str().expect("test_video path is valid"),
                    "--output",
                    "./out.mkv",
                    "--disable-color-data",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: None,
                        disable_color_data: true,
                        ..
                    }),
                    "\"condor --input INPUT --output OUTPUT --disable-color-data\" parses"
                );
            }

            #[test]
            fn with_invalid_optional_flags() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "--input",
                    "./relative-input.mp4",
                    "--output",
                    "/absolute/path/to/output.mkv",
                    "--encoder",
                    "x263",
                ]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor --input INPUT --output OUTPUT --encoder x263\" does not parse"
                );
                let result = CondorCli::try_parse_from([
                    "condor",
                    "--input",
                    "../relative-input.mp4",
                    "--output",
                    "/absolute/path/to/output.mkv",
                    "--workers",
                    "-2",
                ]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor --input INPUT --output OUTPUT --workers -2\" does not parse"
                );
                let result = CondorCli::try_parse_from([
                    "condor",
                    "--input",
                    "./relative-input.mp4",
                    "--output",
                    "/absolute/path/to/output.mkv",
                    "--photon-noise",
                    "-24.8",
                ]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor --input INPUT --output OUTPUT --photon-noise -24.8\" does not parse"
                );
            }

            #[test]
            fn with_zones() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "--input",
                    "./relative-input.mp4",
                    "--output",
                    "/absolute/path/to/output.mkv",
                    "--zones",
                    "-1 200 svt-av1 \"--preset 3\"",
                    "--zones",
                    "./zones.json",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: None,
                        zones: Some(ref zones),
                        ..
                    }) if zones.len() == 2 && zones[1] == "./zones.json",
                    "\"condor --zones ZONE --zones FILE\" parses"
                );
            }
        }

        mod zone {
            use super::*;

            #[test]
            fn dry_run_before_and_after_negative_bound_zones() {
                for args in [
                    vec!["condor", "zone", "--dry-run", "-1 200 svt-av1 \"--preset 3\""],
                    vec!["condor", "zone", "-1 200 svt-av1 \"--preset 3\"", "--dry-run"],
                    vec!["condor", "zone", "--dry-run"],
                ] {
                    let mut cli = CondorCli::try_parse_from(args).expect("dry-run flag parses");
                    cli.normalize_zone_flags();
                    assert_matches!(cli.command, Some(Commands::Zone { dry_run: true, ref zones })
                        if zones.len() <= 1 && zones.iter().all(|zone| zone.starts_with("-1 200")));
                }
            }

            #[test]
            fn default() {
                let result = CondorCli::try_parse_from(["condor", "zone"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Zone { ref zones, dry_run: false }),
                        ..
                    }) if zones.is_empty(),
                    "\"condor zone\" parses"
                );
            }

            #[test]
            fn with_zones() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "zone",
                    "-1 200 svt-av1 \"--preset 3\"",
                    "./zones.json",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Zone { ref zones, dry_run: false }),
                        ..
                    }) if zones.len() == 2 && zones[0].starts_with("-1 200"),
                    "\"condor zone ZONE FILE\" parses"
                );
            }
        }

        mod scenes {
            use super::*;
            use crate::{
                commands::handlers::scenes::{ScenesAction, ScenesKey},
                scenes::SceneSelection,
            };

            /// The index comes first and names what the action works on;
            /// without an action the addressed scenes are printed. The shapes
            /// the index binds are pinned here so the grammar cannot drift.
            #[test]
            fn the_index_comes_before_the_action() {
                let cli = CondorCli::try_parse_from([
                    "condor",
                    "scenes",
                    "1-2,8-10,16",
                    "info",
                    "quality-check",
                ])
                .expect("\"condor scenes 1-2,8-10,16 info quality-check\" parses");
                assert_matches!(
                    cli.command,
                    Some(Commands::Scenes {
                        index: Some(SceneSelection::Ranges(ref ranges)),
                        action: Some(ScenesAction::Info {
                            key: Some(ScenesKey::QualityCheck),
                        }),
                        ..
                    }) if ranges == &[(1, 2), (8, 10), (16, 16)]
                );

                let cli = CondorCli::try_parse_from(["condor", "scenes", "all", "reset"])
                    .expect("\"condor scenes all reset\" parses");
                assert_matches!(
                    cli.command,
                    Some(Commands::Scenes {
                        index: Some(SceneSelection::All),
                        action: Some(ScenesAction::Reset {
                            key: None
                        }),
                        ..
                    })
                );

                // Without an action, and without an index, the whole list is
                // what a bare `condor scenes` prints.
                let cli = CondorCli::try_parse_from(["condor", "scenes"])
                    .expect("\"condor scenes\" parses");
                assert_matches!(
                    cli.command,
                    Some(Commands::Scenes {
                        index: None,
                        action: None,
                        ..
                    })
                );
            }

            /// `add` places its own scene, so its placements are exclusive.
            #[test]
            fn add_placements_are_exclusive() {
                let cli =
                    CondorCli::try_parse_from(["condor", "scenes", "add", "0", "100", "--after=3"])
                        .expect("\"condor scenes add 0 100 --after=3\" parses");
                assert_matches!(
                    cli.command,
                    Some(Commands::Scenes {
                        index: None,
                        action: Some(ScenesAction::Add {
                            after: Some(3),
                            ..
                        }),
                        ..
                    })
                );

                let result = CondorCli::try_parse_from([
                    "condor",
                    "scenes",
                    "add",
                    "0",
                    "100",
                    "--append",
                    "--prepend",
                ]);
                assert!(result.is_err(), "the placements are exclusive");
            }
        }
        mod config {
            use super::*;
            use crate::commands::{
                config::{ConfigAction, Toggle},
                handlers::{
                    detect_noise::DetectNoiseSet,
                    detect_scenes::{DetectScenesKey, DetectScenesSet},
                    scale_noise::{ScaleNoiseKey, ScaleNoiseSet},
                    scale_speed::ScaleSpeedSet,
                },
            };

            /// The `info` / `set` / `reset` triple is one generic
            /// `ConfigAction`, so a single command pins the wiring every
            /// editor shares: a keyed read, a typed write and both reset
            /// forms.
            #[test]
            fn the_config_action_triple_binds() {
                let cli = CondorCli::try_parse_from(["condor", "detect-scenes", "info", "method"])
                    .expect("\"detect-scenes info method\" parses");
                assert_matches!(
                    cli.command,
                    Some(Commands::DetectScenes {
                        edit: Some(ConfigAction::Info {
                            key: Some(DetectScenesKey::Method),
                        }),
                        ..
                    })
                );

                let cli = CondorCli::try_parse_from([
                    "condor",
                    "init",
                    "set",
                    "input",
                    "--path",
                    "./alt.mkv",
                    "--decoder",
                    "ffms2",
                ])
                .expect("\"init set input ...\" parses");
                assert_matches!(
                    cli.command,
                    Some(Commands::Init {
                        edit: Some(ConfigAction::Set { .. }),
                        ..
                    })
                );

                let cli = CondorCli::try_parse_from(["condor", "detect-noise", "reset"])
                    .expect("\"detect-noise reset\" parses");
                assert_matches!(
                    cli.command,
                    Some(Commands::DetectNoise {
                        edit: Some(ConfigAction::Reset {
                            key: None
                        }),
                        ..
                    })
                );

                let cli =
                    CondorCli::try_parse_from(["condor", "scale-noise", "reset", "threshold"])
                        .expect("\"scale-noise reset threshold\" parses");
                assert_matches!(
                    cli.command,
                    Some(Commands::ScaleNoise {
                        edit: Some(ConfigAction::Reset {
                            key: Some(ScaleNoiseKey::Threshold),
                        }),
                        ..
                    })
                );

                // The redundant `section` key is gone; a bare `reset`
                // replaces it.
                assert!(
                    CondorCli::try_parse_from(["condor", "scale-noise", "reset", "section"])
                        .is_err(),
                    "\"reset section\" is no longer a property"
                );
            }

            /// Values that are never optional are positionals, and the value
            /// tables the parser owns — the switch spellings, the alias names —
            /// are pinned here rather than end to end.
            #[test]
            fn values_positionals_and_aliases_parse() {
                let cli = CondorCli::try_parse_from([
                    "condor",
                    "scale-speed",
                    "set",
                    "speed-scalers",
                    "20=6",
                    "35=4",
                ])
                .expect("\"scale-speed set speed-scalers 20=6 35=4\" parses");
                assert_matches!(
                    cli.command,
                    Some(Commands::ScaleSpeed {
                        edit: Some(ConfigAction::Set {
                            setting: ScaleSpeedSet::SpeedQuantizers {
                                points,
                            },
                        }),
                        ..
                    }) if points == vec![(20, 6), (35, 4)]
                );

                let cli = CondorCli::try_parse_from([
                    "condor",
                    "detect-noise",
                    "set",
                    "reference-filters",
                    "resize:width=960;",
                ])
                .expect("\"detect-noise set reference-filters resize:...\" parses");
                assert_matches!(
                    cli.command,
                    Some(Commands::DetectNoise {
                        edit: Some(ConfigAction::Set {
                            setting: DetectNoiseSet::ReferenceFilters {
                                ref filters,
                            },
                        }),
                        ..
                    }) if filters == &["resize:width=960;".to_owned()]
                );

                // One flag turns a setting both ways: every spelling of the
                // on/off value lands on the switch, and a following flag is
                // never swallowed as its value.
                for (value, expected) in [
                    ("on", Toggle::On),
                    ("enable", Toggle::On),
                    ("enabled", Toggle::On),
                    ("yes", Toggle::On),
                    ("true", Toggle::On),
                    ("1", Toggle::On),
                    ("off", Toggle::Off),
                    ("disable", Toggle::Off),
                    ("disabled", Toggle::Off),
                    ("no", Toggle::Off),
                    ("false", Toggle::Off),
                    ("0", Toggle::Off),
                ] {
                    let cli = CondorCli::try_parse_from([
                        "condor",
                        "detect-scenes",
                        "set",
                        "method",
                        "avscenechange",
                        "--save-scores",
                        value,
                    ])
                    .unwrap_or_else(|err| panic!("--save-scores {value} parses: {err}"));
                    assert_matches!(
                        cli.command,
                        Some(Commands::DetectScenes {
                            edit: Some(ConfigAction::Set {
                                setting: DetectScenesSet::Method {
                                    save_scores, ..
                                },
                            }),
                            ..
                        }) if save_scores == Some(expected)
                    );
                }

                let cli = CondorCli::try_parse_from([
                    "condor",
                    "detect-scenes",
                    "set",
                    "method",
                    "avscenechange",
                    "--save-scores",
                    "--minimum-frames",
                    "20",
                ])
                .expect("\"--save-scores --minimum-frames 20\" parses");
                assert_matches!(
                    cli.command,
                    Some(Commands::DetectScenes {
                        edit: Some(ConfigAction::Set {
                            setting: DetectScenesSet::Method {
                                save_scores: Some(Toggle::On),
                                minimum_frames: Some(20),
                                ..
                            },
                        }),
                        ..
                    })
                );

                // The switch value is optional and means on when omitted.
                let cli =
                    CondorCli::try_parse_from(["condor", "scale-noise", "set", "scale-chroma"])
                        .expect("\"scale-noise set scale-chroma\" parses");
                assert_matches!(
                    cli.command,
                    Some(Commands::ScaleNoise {
                        edit: Some(ConfigAction::Set {
                            setting: ScaleNoiseSet::ScaleChroma {
                                value: None
                            },
                        }),
                        ..
                    })
                );
            }

            /// Unknown properties and kinds are parse errors, and run flags
            /// never mix with an editing subcommand.
            #[test]
            fn invalid_edits_and_mixed_run_flags_are_rejected() {
                for args in [
                    vec!["condor", "detect-scenes", "set", "nonsense"],
                    vec!["condor", "detect-scenes", "set", "method", "not-an-algorithm"],
                    vec!["condor", "quality-check", "set", "strategy", "bogus"],
                    vec!["condor", "target-quality", "set", "metric", "bogus"],
                    vec!["condor", "concatenate", "set", "method", "bogus"],
                    vec!["condor", "scale-speed", "set", "speed-quantizers", "not-a-pair", "x=1"],
                    vec!["condor", "target-quality", "set", "quantizer-range", "30-10"],
                    vec!["condor", "target-quality", "set", "interpolators", "linear"],
                    vec!["condor", "target-quality", "set", "plot"],
                ] {
                    assert!(
                        CondorCli::try_parse_from(&args).is_err(),
                        "{args:?} is rejected"
                    );
                }

                // Run flags still parse on their own, but conflict with a
                // configuration subcommand.
                let cli = CondorCli::try_parse_from([
                    "condor",
                    "detect-scenes",
                    "--min-scene-seconds",
                    "3",
                ])
                .expect("run flags still parse");
                assert_matches!(
                    cli.command,
                    Some(Commands::DetectScenes {
                        edit: None,
                        min_scene_seconds: Some(3),
                        ..
                    })
                );

                assert!(
                    CondorCli::try_parse_from([
                        "condor",
                        "detect-scenes",
                        "--min-scene-seconds",
                        "3",
                        "info",
                    ])
                    .is_err(),
                    "flags and subcommands conflict"
                );
            }
        }
        mod detect_scenes {
            use super::*;

            #[test]
            fn default() {
                let result = CondorCli::try_parse_from(["condor", "detect-scenes"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::DetectScenes { .. }),
                        ..
                    }),
                    "\"condor detect-scenes\" parses"
                );
            }

            #[test]
            fn with_input() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "detect-scenes",
                    "--input",
                    "./input.mp4",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::DetectScenes {
                            input: Some(_),
                            ..
                        }),
                        ..
                    }),
                    "\"condor detect-scenes --input INPUT\" parses"
                );
            }

            #[test]
            fn with_options() {
                let temp = tempfile::tempdir().expect("temp directory");
                let config_path = temp.path().join("condor.json");
                let logs_path = temp.path().join("condor.log");

                let result = CondorCli::try_parse_from([
                    "condor",
                    "detect-scenes",
                    "--method",
                    "fast",
                    "--min-scene-seconds",
                    "2",
                    "--max-scene-seconds",
                    "5",
                    "--config-file",
                    config_path.to_str().expect("config path exists"),
                    "--temp",
                    temp.path().to_str().expect("temp path exists"),
                    "--logs",
                    logs_path.to_str().expect("log path exists"),
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::DetectScenes {
                            method: Some(SceneDetectionMethod::Fast),
                            min_scene_seconds: Some(2),
                            max_scene_seconds: Some(5),
                            ..
                        }),
                        ..
                    }),
                    "\"condor detect-scenes --method fast --min-scene-seconds 2 \
                     --max-scene-seconds 5\" parses"
                );
            }

            #[test]
            fn with_invalid_optional_flags() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "detect-scenes",
                    "--min-scene-seconds",
                    "pi",
                ]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor detect-scenes --min-scene-seconds pi\" does not parse"
                );
                let result = CondorCli::try_parse_from([
                    "condor",
                    "detect-scenes",
                    "--max-scene-seconds",
                    "-12",
                ]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor detect-scenes --max-scene-seconds -12\" does not parse"
                );
                let result = CondorCli::try_parse_from([
                    "condor",
                    "detect-scenes",
                    "--method",
                    "thefastestoneyoucangivemeplease",
                ]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor --input INPUT --output OUTPUT --method \
                     thefastestoneyoucangivemeplease\" does not parse"
                );
            }
        }

        mod detect_noise {
            use super::*;

            #[test]
            fn default() {
                let result = CondorCli::try_parse_from(["condor", "detect-noise"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::DetectNoise { .. }),
                        ..
                    }),
                    "\"condor detect-noise\" parses"
                );
            }

            #[test]
            fn with_input() {
                let result =
                    CondorCli::try_parse_from(["condor", "detect-noise", "--input", "./input.mp4"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::DetectNoise {
                            input: Some(_),
                            ..
                        }),
                        ..
                    }),
                    "\"condor detect-noise --input INPUT\" parses"
                );
            }

            #[test]
            fn with_vs_args() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "detect-noise",
                    "--input",
                    "./script.vpy",
                    "--vs-args",
                    "preset=slow",
                    "--vs-args",
                    "threads=4",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command:
                            Some(Commands::DetectNoise {
                                input: Some(_),
                                vs_args: Some(ref args),
                                ..
                            }),
                        ..
                    }) if args.len() == 2,
                    "\"condor detect-noise --input ./script.vpy --vs_args \"preset=slow\" --vs-args \"threads=4\" parses"
                );
            }
        }

        mod scale_noise {
            use super::*;

            #[test]
            fn default() {
                let result = CondorCli::try_parse_from(["condor", "scale-noise"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::ScaleNoise { .. }),
                        ..
                    }),
                    "\"condor scale-noise\" parses"
                );
            }

            #[test]
            fn with_threshold() {
                let result =
                    CondorCli::try_parse_from(["condor", "scale-noise", "--threshold", "0.002"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::ScaleNoise {
                            threshold: Some(0.002),
                            ..
                        }),
                        ..
                    }),
                    "\"condor scale-noise --threshold 0.002\" parses"
                );
            }

            #[test]
            fn with_min_max_scaler() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "scale-noise",
                    "--minimum-scaler",
                    "0.5",
                    "--maximum-scaler",
                    "2.0",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::ScaleNoise {
                            minimum_scaler: Some(0.5),
                            maximum_scaler: Some(2.0),
                            ..
                        }),
                        ..
                    }),
                    "\"condor scale-noise --minimum-scaler 0.5 --maximum-scaler 2.0\" parses"
                );
            }

            #[test]
            fn with_scale_chroma() {
                let result = CondorCli::try_parse_from(["condor", "scale-noise", "--scale-chroma"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::ScaleNoise {
                            scale_chroma: true,
                            ..
                        }),
                        ..
                    }),
                    "\"condor scale-noise --scale-chroma\" parses"
                );
            }

            #[test]
            fn with_all_options() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "scale-noise",
                    "--threshold",
                    "0.005",
                    "--minimum-scaler",
                    "0.3",
                    "--maximum-scaler",
                    "1.5",
                    "--scale-chroma",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::ScaleNoise {
                            threshold: Some(0.005),
                            minimum_scaler: Some(0.3),
                            maximum_scaler: Some(1.5),
                            scale_chroma: true,
                            ..
                        }),
                        ..
                    }),
                    "\"condor scale-noise\" with all options parses"
                );
            }
        }

        mod benchmark {
            use super::*;

            #[test]
            fn default() {
                let result = CondorCli::try_parse_from(["condor", "benchmark"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Benchmark { .. }),
                        ..
                    }),
                    "\"condor benchmark\" parses"
                );
            }

            #[test]
            fn with_threshold() {
                let result =
                    CondorCli::try_parse_from(["condor", "benchmark", "--threshold", "10"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Benchmark {
                            threshold: Some(10),
                            ..
                        }),
                        ..
                    }),
                    "\"condor benchmark --threshold 10\" parses"
                );
            }

            #[test]
            fn with_max_memory() {
                let result =
                    CondorCli::try_parse_from(["condor", "benchmark", "--max-memory", "4096"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Benchmark {
                            max_memory: Some(4096),
                            ..
                        }),
                        ..
                    }),
                    "\"condor benchmark --max-memory 4096\" parses"
                );
            }

            #[test]
            fn with_invalid_threshold_range() {
                let result =
                    CondorCli::try_parse_from(["condor", "benchmark", "--threshold", "101"]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor benchmark --threshold 101\" does not parse (range 0..=100)"
                );
            }

            #[test]
            fn with_invalid_threshold_negative() {
                let result =
                    CondorCli::try_parse_from(["condor", "benchmark", "--threshold", "-5"]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor benchmark --threshold -5\" does not parse"
                );
            }
        }

        mod target_quality {
            use super::*;

            #[test]
            fn default() {
                let result = CondorCli::try_parse_from(["condor", "target-quality"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::TargetQuality { .. }),
                        ..
                    }),
                    "\"condor target-quality\" parses"
                );
            }

            #[test]
            fn with_input() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "target-quality",
                    "--input",
                    "./input.mp4",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::TargetQuality {
                            input: Some(_),
                            ..
                        }),
                        ..
                    }),
                    "\"condor target-quality --input INPUT\" parses"
                );
            }

            #[test]
            fn with_metric_and_target() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "target-quality",
                    "--metric",
                    "ssimulacra2",
                    "--target",
                    "90",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::TargetQuality {
                            metric: Some(QualityMetric::SSIMULACRA2),
                            target: Some(90.0),
                            ..
                        }),
                        ..
                    }),
                    "\"condor target-quality --metric ssimulacra2 --target 90\" parses"
                );
            }

            #[test]
            fn with_profile() {
                let result =
                    CondorCli::try_parse_from(["condor", "target-quality", "--profile", "slow"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::TargetQuality {
                            profile: Some(QualityProfile::Slow),
                            ..
                        }),
                        ..
                    }),
                    "\"condor target-quality --profile slow\" parses"
                );
            }

            #[test]
            fn with_all_metric_variants() {
                for (name, expected) in [
                    ("vmaf", QualityMetric::VMAF),
                    ("ssimulacra2", QualityMetric::SSIMULACRA2),
                    ("butteraugli", QualityMetric::BUTTERAUGLI),
                    ("butteraugli-3", QualityMetric::BUTTERAUGLI3Norm),
                    ("xpsnr", QualityMetric::XPSNR),
                    ("cvvdp", QualityMetric::CVVDP),
                ] {
                    let result = CondorCli::try_parse_from([
                        "condor",
                        "target-quality",
                        "--metric",
                        name,
                        "--target",
                        "80",
                    ]);
                    assert_matches!(
                        result,
                        Ok(CondorCli {
                            command: Some(Commands::TargetQuality {
                                metric: Some(_),
                                ..
                            }),
                            ..
                        }),
                        "\"condor target-quality --metric {name}\" parses"
                    );
                    if let Ok(CondorCli {
                        command:
                            Some(Commands::TargetQuality {
                                metric: Some(ref m),
                                ..
                            }),
                        ..
                    }) = result
                    {
                        assert_eq!(m, &expected, "metric variant matches for {name}");
                    }
                }
            }

            #[test]
            fn with_all_profile_variants() {
                for (name, expected) in [
                    ("fast", QualityProfile::Fast),
                    ("standard", QualityProfile::Standard),
                    ("slow", QualityProfile::Slow),
                ] {
                    let result =
                        CondorCli::try_parse_from(["condor", "target-quality", "--profile", name]);
                    assert_matches!(
                        result,
                        Ok(CondorCli {
                            command: Some(Commands::TargetQuality {
                                profile: Some(_),
                                ..
                            }),
                            ..
                        }),
                        "\"condor target-quality --profile {name}\" parses"
                    );
                    if let Ok(CondorCli {
                        command:
                            Some(Commands::TargetQuality {
                                profile: Some(ref p),
                                ..
                            }),
                        ..
                    }) = result
                    {
                        assert_eq!(p, &expected, "profile variant matches for {name}");
                    }
                }
            }

            #[test]
            fn with_min_max_quantizer() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "target-quality",
                    "--min-q",
                    "10",
                    "--max-q",
                    "50",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::TargetQuality {
                            minimum_quantizer: Some(10),
                            maximum_quantizer: Some(50),
                            ..
                        }),
                        ..
                    }),
                    "\"condor target-quality --min-q 10 --max-q 50\" parses"
                );
            }

            #[test]
            fn with_params() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "target-quality",
                    "--params",
                    "--preset 2 --crf 24",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::TargetQuality {
                            params: Some(_),
                            ..
                        }),
                        ..
                    }),
                    "\"condor target-quality --params ...\" parses"
                );
            }

            #[test]
            fn with_invalid_metric() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "target-quality",
                    "--metric",
                    "nonexistent",
                ]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor target-quality --metric nonexistent\" does not parse"
                );
            }

            #[test]
            fn with_invalid_profile() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "target-quality",
                    "--profile",
                    "nonexistent",
                ]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor target-quality --profile nonexistent\" does not parse"
                );
            }
        }

        mod quality_check {
            use super::*;

            #[test]
            fn default() {
                let result = CondorCli::try_parse_from(["condor", "quality-check"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::QualityCheck { .. }),
                        ..
                    }),
                    "\"condor quality-check\" parses"
                );
            }

            #[test]
            fn with_metric() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "quality-check",
                    "--metric",
                    "ssimulacra2",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::QualityCheck {
                            metric: Some(QualityMetric::SSIMULACRA2),
                            ..
                        }),
                        ..
                    }),
                    "\"condor quality-check --metric ssimulacra2\" parses"
                );
            }

            #[test]
            fn with_profile() {
                let result =
                    CondorCli::try_parse_from(["condor", "quality-check", "--profile", "fast"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::QualityCheck {
                            profile: Some(QualityProfile::Fast),
                            ..
                        }),
                        ..
                    }),
                    "\"condor quality-check --profile fast\" parses"
                );
            }

            #[test]
            fn with_all_metric_variants() {
                for (name, expected) in [
                    ("vmaf", QualityMetric::VMAF),
                    ("ssimulacra2", QualityMetric::SSIMULACRA2),
                    ("butteraugli", QualityMetric::BUTTERAUGLI),
                    ("butteraugli-3", QualityMetric::BUTTERAUGLI3Norm),
                    ("xpsnr", QualityMetric::XPSNR),
                    ("cvvdp", QualityMetric::CVVDP),
                ] {
                    let result =
                        CondorCli::try_parse_from(["condor", "quality-check", "--metric", name]);
                    assert_matches!(
                        result,
                        Ok(CondorCli {
                            command: Some(Commands::QualityCheck {
                                metric: Some(_),
                                ..
                            }),
                            ..
                        }),
                        "\"condor quality-check --metric {name}\" parses"
                    );
                    if let Ok(CondorCli {
                        command:
                            Some(Commands::QualityCheck {
                                metric: Some(ref m),
                                ..
                            }),
                        ..
                    }) = result
                    {
                        assert_eq!(m, &expected, "metric variant matches for {name}");
                    }
                }
            }

            #[test]
            fn with_invalid_metric() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "quality-check",
                    "--metric",
                    "nonexistent",
                ]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor quality-check --metric nonexistent\" does not parse"
                );
            }
        }

        mod optimize_bitrate {
            use super::*;

            #[test]
            fn default() {
                let result = CondorCli::try_parse_from(["condor", "optimize-bitrate"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::OptimizeBitrate { .. }),
                        ..
                    }),
                    "\"condor optimize-bitrate\" parses"
                );
            }

            #[test]
            fn with_sigma_threshold() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "optimize-bitrate",
                    "--sigma-threshold",
                    "3",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::OptimizeBitrate {
                            sigma_threshold: Some(3),
                            ..
                        }),
                        ..
                    }),
                    "\"condor optimize-bitrate --sigma-threshold 3\" parses"
                );
            }

            #[test]
            fn with_invalid_sigma_below_range() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "optimize-bitrate",
                    "--sigma-threshold",
                    "0",
                ]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor optimize-bitrate --sigma-threshold 0\" does not parse (range 1..=10)"
                );
            }

            #[test]
            fn with_invalid_sigma_above_range() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "optimize-bitrate",
                    "--sigma-threshold",
                    "11",
                ]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor optimize-bitrate --sigma-threshold 11\" does not parse (range \
                     1..=10)"
                );
            }
        }

        mod scale_speed {
            use super::*;

            #[test]
            fn requires_both_quantizers_and_speeds() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "scale-speed",
                    "--quantizers",
                    "20",
                    "--speeds",
                    "5",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::ScaleSpeed { .. }),
                        ..
                    }),
                    "\"condor scale-speed --quantizers 20 --speeds 5\" parses"
                );
            }

            #[test]
            fn with_multiple_pairs() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "scale-speed",
                    "--quantizers",
                    "20",
                    "--speeds",
                    "5",
                    "--quantizers",
                    "30",
                    "--speeds",
                    "4",
                    "--quantizers",
                    "55",
                    "--speeds",
                    "2",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::ScaleSpeed {
                            quantizers: Some(_),
                            speeds: Some(_),
                            ..
                        }),
                        ..
                    }),
                    "\"condor scale-speed\" with three pairs parses"
                );
                if let Ok(CondorCli {
                    command:
                        Some(Commands::ScaleSpeed {
                            quantizers: Some(ref q),
                            speeds: Some(ref s),
                            ..
                        }),
                    ..
                }) = result
                {
                    assert_eq!(q.len(), 3, "three quantizer values");
                    assert_eq!(s.len(), 3, "three speed values");
                    assert_eq!(q[0], 20);
                    assert_eq!(s[0], 5);
                    assert_eq!(q[1], 30);
                    assert_eq!(s[1], 4);
                    assert_eq!(q[2], 55);
                    assert_eq!(s[2], 2);
                }
            }

            #[test]
            fn quantizers_without_speeds() {
                let result =
                    CondorCli::try_parse_from(["condor", "scale-speed", "--quantizers", "20"]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor scale-speed --quantizers 20\" without --speeds fails"
                );
            }

            #[test]
            fn speeds_without_quantizers() {
                let result = CondorCli::try_parse_from(["condor", "scale-speed", "--speeds", "5"]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor scale-speed --speeds 5\" without --quantizers fails"
                );
            }

            #[test]
            fn with_negative_quantizers() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "scale-speed",
                    "--quantizers",
                    "-10",
                    "--speeds",
                    "7",
                ]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor scale-speed --quantizers -10 ...\" fails (no allow_hyphen_values)"
                );
            }
        }

        mod encode {
            use super::*;

            #[test]
            fn default() {
                let result = CondorCli::try_parse_from(["condor", "encode"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Encode { .. }),
                        ..
                    }),
                    "\"condor encode\" parses"
                );
            }

            #[test]
            fn with_input() {
                let result =
                    CondorCli::try_parse_from(["condor", "encode", "--input", "./input.mp4"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Encode {
                            input: Some(_),
                            ..
                        }),
                        ..
                    }),
                    "\"condor encode --input INPUT\" parses"
                );
            }

            #[test]
            fn with_encoder_and_workers() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "encode",
                    "--encoder",
                    "x264",
                    "--workers",
                    "4",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Encode {
                            encoder: Some(EncoderMethod::X264),
                            workers: Some(4),
                            ..
                        }),
                        ..
                    }),
                    "\"condor encode --encoder x264 --workers 4\" parses"
                );
            }

            #[test]
            fn with_passes() {
                let result = CondorCli::try_parse_from(["condor", "encode", "--passes", "2"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Encode {
                            passes: Some(2),
                            ..
                        }),
                        ..
                    }),
                    "\"condor encode --passes 2\" parses"
                );
            }

            #[test]
            fn with_params() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "encode",
                    "--params",
                    "--preset 8 --crf 30",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Encode {
                            params: Some(_),
                            ..
                        }),
                        ..
                    }),
                    "\"condor encode --params ...\" parses"
                );
            }

            #[test]
            fn with_photon_noise() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "encode",
                    "--photon-noise",
                    "2400",
                    "--chroma-noise",
                    "1200",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Encode {
                            photon_noise: Some(2400),
                            chroma_noise: Some(1200),
                            ..
                        }),
                        ..
                    }),
                    "\"condor encode --photon-noise 2400 --chroma-noise 1200\" parses"
                );
            }

            #[test]
            fn with_decoder_and_filters() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "encode",
                    "--decoder",
                    "bestsource",
                    "--filters",
                    "resize:scaler=bilinear;width=1280;height=720;format=yuv420p;",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Encode {
                            decoder: Some(DecoderMethod::BestSource),
                            filters: Some(_),
                            ..
                        }),
                        ..
                    }),
                    "\"condor encode --decoder bestsource --filters ...\" parses"
                );
            }

            #[test]
            fn with_vs_args() {
                let result =
                    CondorCli::try_parse_from(["condor", "encode", "--vs-args", "threads=8"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Encode {
                            vs_args: Some(_),
                            ..
                        }),
                        ..
                    }),
                    "\"condor encode --vs-args threads=8\" parses"
                );
            }

            #[test]
            fn with_ffmpeg_filter() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "encode",
                    "--ffmpeg-filter",
                    "crop=iw-16:ih-16",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Encode {
                            ffmpeg_filter: Some(_),
                            ..
                        }),
                        ..
                    }),
                    "\"condor encode --ffmpeg-filter ...\" parses"
                );
            }

            #[test]
            fn with_invalid_encoder() {
                let result = CondorCli::try_parse_from(["condor", "encode", "--encoder", "x263"]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor encode --encoder x263\" does not parse"
                );
            }

            #[test]
            fn with_invalid_workers_negative() {
                let result = CondorCli::try_parse_from(["condor", "encode", "--workers", "-1"]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor encode --workers -1\" does not parse"
                );
            }
        }

        mod concatenate {
            use super::*;

            #[test]
            fn default() {
                let result = CondorCli::try_parse_from(["condor", "concatenate"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Concatenate { .. }),
                        ..
                    }),
                    "\"condor concatenate\" parses"
                );
            }

            #[test]
            fn with_mkvmerge() {
                let result =
                    CondorCli::try_parse_from(["condor", "concatenate", "--method", "mkvmerge"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Concatenate {
                            method: Some(ConcatenationMethod::MkvMerge),
                            ..
                        }),
                        ..
                    }),
                    "\"condor concatenate --method mkvmerge\" parses"
                );
            }

            #[test]
            fn with_ffmpeg() {
                let result =
                    CondorCli::try_parse_from(["condor", "concatenate", "--method", "ffmpeg"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Concatenate {
                            method: Some(ConcatenationMethod::FFmpeg),
                            ..
                        }),
                        ..
                    }),
                    "\"condor concatenate --method ffmpeg\" parses"
                );
            }

            #[test]
            fn with_ivf() {
                let result =
                    CondorCli::try_parse_from(["condor", "concatenate", "--method", "ivf"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Concatenate {
                            method: Some(ConcatenationMethod::Ivf),
                            ..
                        }),
                        ..
                    }),
                    "\"condor concatenate --method ivf\" parses"
                );
            }

            #[test]
            fn with_invalid_method() {
                let result =
                    CondorCli::try_parse_from(["condor", "concatenate", "--method", "invalid"]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor concatenate --method invalid\" does not parse"
                );
            }

            #[test]
            fn with_discard() {
                let result =
                    CondorCli::try_parse_from(["condor", "concatenate", "--discard", "audio"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Concatenate {
                            discard: Some(_),
                            ..
                        }),
                        ..
                    }),
                    "\"condor concatenate --discard audio\" parses"
                );
                if let Ok(CondorCli {
                    command:
                        Some(Commands::Concatenate {
                            discard: Some(categories),
                            ..
                        }),
                    ..
                }) = CondorCli::try_parse_from(["condor", "concatenate", "--discard", "audio"])
                {
                    assert_eq!(categories, vec![DiscardCategory::Audio]);
                }
            }

            #[test]
            fn with_repeated_discard() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "concatenate",
                    "--discard",
                    "audio",
                    "--discard",
                    "subtitle",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Concatenate {
                            discard: Some(_),
                            ..
                        }),
                        ..
                    }),
                    "\"condor concatenate --discard audio --discard subtitle\" parses"
                );
                if let Ok(CondorCli {
                    command:
                        Some(Commands::Concatenate {
                            discard: Some(categories),
                            ..
                        }),
                    ..
                }) = CondorCli::try_parse_from([
                    "condor",
                    "concatenate",
                    "--discard",
                    "audio",
                    "--discard",
                    "subtitle",
                ]) {
                    assert_eq!(categories, vec![
                        DiscardCategory::Audio,
                        DiscardCategory::Subtitle
                    ]);
                }
            }

            #[test]
            fn with_delimited_discard() {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "concatenate",
                    "--discard",
                    "audio,subtitle,chapters",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Concatenate {
                            discard: Some(_),
                            ..
                        }),
                        ..
                    }),
                    "\"condor concatenate --discard audio,subtitle,chapters\" parses"
                );
                if let Ok(CondorCli {
                    command:
                        Some(Commands::Concatenate {
                            discard: Some(categories),
                            ..
                        }),
                    ..
                }) = CondorCli::try_parse_from([
                    "condor",
                    "concatenate",
                    "--discard",
                    "audio,subtitle,chapters",
                ]) {
                    assert_eq!(categories, vec![
                        DiscardCategory::Audio,
                        DiscardCategory::Subtitle,
                        DiscardCategory::Chapters
                    ]);
                }
            }

            #[test]
            fn with_invalid_discard() {
                let result =
                    CondorCli::try_parse_from(["condor", "concatenate", "--discard", "font"]);
                assert_matches!(
                    result,
                    Err(_),
                    "\"condor concatenate --discard font\" does not parse"
                );
            }

            #[test]
            fn discard_defaults_to_none() {
                let result = CondorCli::try_parse_from(["condor", "concatenate"]);
                if let Ok(CondorCli {
                    command:
                        Some(Commands::Concatenate {
                            discard, ..
                        }),
                    ..
                }) = result
                {
                    assert!(discard.is_none(), "--discard defaults to unset");
                }
            }
        }

        mod clean {
            use super::*;

            #[test]
            fn default() {
                let result = CondorCli::try_parse_from(["condor", "clean"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Clean { .. }),
                        ..
                    }),
                    "\"condor clean\" parses"
                );
            }

            #[test]
            fn with_all() {
                let result = CondorCli::try_parse_from(["condor", "clean", "--all"]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Clean {
                            all: true,
                            ..
                        }),
                        ..
                    }),
                    "\"condor clean --all\" parses"
                );
            }

            #[test]
            fn all_defaults_to_false() {
                let result = CondorCli::try_parse_from(["condor", "clean"]);
                if let Ok(CondorCli {
                    command:
                        Some(Commands::Clean {
                            all, ..
                        }),
                    ..
                }) = result
                {
                    assert!(!all, "--all defaults to false");
                }
            }
        }
    }

    mod discard {
        use std::str::FromStr;

        use super::*;

        #[test]
        fn parses_every_category() {
            for name in ["audio", "subtitle", "attachment", "metadata", "chapters", "video"] {
                let result =
                    CondorCli::try_parse_from(["condor", "concatenate", "--discard", name]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::Concatenate {
                            discard: Some(_),
                            ..
                        }),
                        ..
                    }),
                    "\"--discard {name}\" parses"
                );
            }
        }

        #[test]
        fn category_names_round_trip() {
            assert_eq!(DiscardCategory::Audio.to_string(), "audio");
            assert_eq!(DiscardCategory::Subtitle.to_string(), "subtitle");
            assert_eq!(DiscardCategory::Attachment.to_string(), "attachment");
            assert_eq!(DiscardCategory::Metadata.to_string(), "metadata");
            assert_eq!(DiscardCategory::Chapters.to_string(), "chapters");
            assert_eq!(DiscardCategory::Video.to_string(), "video");
            for category in [
                DiscardCategory::Audio,
                DiscardCategory::Subtitle,
                DiscardCategory::Attachment,
                DiscardCategory::Metadata,
                DiscardCategory::Chapters,
                DiscardCategory::Video,
            ] {
                let name: &'static str = category.into();
                assert_eq!(DiscardCategory::from_str(name), Ok(category));
            }
        }

        /// The top-level flag applies to any command, so it is accepted before
        /// the subcommand as well as after it.
        #[test]
        fn the_top_level_flag_is_accepted() {
            let result =
                CondorCli::try_parse_from(["condor", "--discard", "audio", "detect-scenes"]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    discard: Some(_),
                    ..
                }),
                "\"condor --discard audio detect-scenes\" parses"
            );
        }

        /// The subcommand's own `--discard` is parsed independently of the
        /// top-level one, which belongs to the full run.
        #[test]
        fn the_subcommand_flag_is_parsed_separately() {
            let cli = CondorCli::try_parse_from([
                "condor",
                "--discard",
                "audio",
                "concatenate",
                "--discard",
                "subtitle",
            ])
            .expect("\"condor --discard audio concatenate --discard subtitle\" parses");
            assert_eq!(cli.discard, Some(vec![DiscardCategory::Audio]));
            let Commands::Concatenate {
                discard: Some(categories),
                ..
            } = cli.command.expect("the concatenate subcommand is set")
            else {
                panic!("the command is concatenate");
            };
            assert_eq!(categories, vec![DiscardCategory::Subtitle]);
        }
    }

    mod enum_mappings {
        use super::*;

        #[test]
        fn scene_detection_method_mapping() {
            // None -> CoreSCDMethod::None
            let result = SceneDetectionMethod::None.as_core_method(Some(120), Some(240));
            assert_matches!(result, CoreSCDMethod::None { .. });
            if let CoreSCDMethod::None {
                minimum_length,
                maximum_length,
            } = result
            {
                assert_eq!(minimum_length, 120);
                assert_eq!(maximum_length, 240);
            }

            // Fast -> CoreSCDMethod::AVSceneChange with Fast
            let result = SceneDetectionMethod::Fast.as_core_method(Some(60), Some(300));
            assert_matches!(result, CoreSCDMethod::AVSceneChange {
                method: ScenecutMethod::Fast,
                ..
            });
            if let CoreSCDMethod::AVSceneChange {
                minimum_length,
                maximum_length,
                ..
            } = result
            {
                assert_eq!(minimum_length, 60);
                assert_eq!(maximum_length, 300);
            }

            // Standard -> CoreSCDMethod::AVSceneChange with Standard
            let result = SceneDetectionMethod::Standard.as_core_method(Some(120), Some(600));
            assert_matches!(result, CoreSCDMethod::AVSceneChange {
                method: ScenecutMethod::Standard,
                ..
            });
            if let CoreSCDMethod::AVSceneChange {
                minimum_length,
                maximum_length,
                ..
            } = result
            {
                assert_eq!(minimum_length, 120);
                assert_eq!(maximum_length, 600);
            }
        }

        #[test]
        fn scene_detection_method_default_lengths() {
            // None with None lengths uses defaults
            let result = SceneDetectionMethod::None.as_core_method(None, None);
            assert_matches!(result, CoreSCDMethod::None { .. });
            if let CoreSCDMethod::None {
                minimum_length,
                maximum_length,
            } = result
            {
                assert_eq!(
                    minimum_length,
                    andean_condor::models::sequence::scene_detector::DEFAULT_MIN_SCENE_LENGTH_FRAMES
                        as usize
                );
                assert_eq!(
                    maximum_length,
                    andean_condor::models::sequence::scene_detector::
                        DEFAULT_MAX_SCENE_LENGTH_SECONDS as usize
                        * andean_condor::models::sequence::scene_detector::
                            DEFAULT_MIN_SCENE_LENGTH_FRAMES as usize
                );
            }
        }

        #[test]
        fn encoder_method_mapping() {
            assert_matches!(EncoderMethod::AOM.as_encoder_base(), EncoderBase::AOM);
            assert_matches!(EncoderMethod::RAV1E.as_encoder_base(), EncoderBase::RAV1E);
            assert_matches!(EncoderMethod::VPX.as_encoder_base(), EncoderBase::VPX);
            assert_matches!(EncoderMethod::SVTAV1.as_encoder_base(), EncoderBase::SVTAV1);
            assert_matches!(EncoderMethod::AVM.as_encoder_base(), EncoderBase::AVM);
            assert_matches!(EncoderMethod::X264.as_encoder_base(), EncoderBase::X264);
            assert_matches!(EncoderMethod::X265.as_encoder_base(), EncoderBase::X265);
            assert_matches!(EncoderMethod::VVenC.as_encoder_base(), EncoderBase::VVenC);
            assert_matches!(EncoderMethod::FFmpeg.as_encoder_base(), EncoderBase::FFmpeg);
        }

        #[test]
        fn decoder_method_strum_serialization() {
            use super::DecoderMethod;
            assert_eq!(<&str>::from(&DecoderMethod::BestSource), "bestsource");
            assert_eq!(<&str>::from(&DecoderMethod::VSFFMS2), "vs-ffms2");
            assert_eq!(<&str>::from(&DecoderMethod::LSMASHWorks), "lsmash");
            assert_eq!(<&str>::from(&DecoderMethod::DGDecodeNV), "dgdecnv");
            assert_eq!(<&str>::from(&DecoderMethod::FFMS2), "ffms2");
        }

        #[test]
        fn decoder_method_from_str() {
            use std::str::FromStr;
            assert_matches!(
                DecoderMethod::from_str("bestsource"),
                Ok(DecoderMethod::BestSource)
            );
            assert_matches!(
                DecoderMethod::from_str("vs-ffms2"),
                Ok(DecoderMethod::VSFFMS2)
            );
            assert_matches!(
                DecoderMethod::from_str("lsmash"),
                Ok(DecoderMethod::LSMASHWorks)
            );
            assert_matches!(
                DecoderMethod::from_str("dgdecnv"),
                Ok(DecoderMethod::DGDecodeNV)
            );
            assert_matches!(DecoderMethod::from_str("ffms2"), Ok(DecoderMethod::FFMS2));
            assert_matches!(DecoderMethod::from_str("nonexistent"), Err(_));
        }

        #[test]
        fn concatenation_method_strum_serialization() {
            assert_eq!(<&str>::from(&ConcatenationMethod::MkvMerge), "mkvmerge");
            assert_eq!(<&str>::from(&ConcatenationMethod::FFmpeg), "ffmpeg");
            assert_eq!(<&str>::from(&ConcatenationMethod::Ivf), "ivf");
        }

        #[test]
        fn encoder_method_strum_serialization() {
            assert_eq!(<&str>::from(&EncoderMethod::AOM), "aom");
            assert_eq!(<&str>::from(&EncoderMethod::RAV1E), "rav1e");
            assert_eq!(<&str>::from(&EncoderMethod::VPX), "vpx");
            assert_eq!(<&str>::from(&EncoderMethod::SVTAV1), "svt-av1");
            assert_eq!(<&str>::from(&EncoderMethod::AVM), "avm");
            assert_eq!(<&str>::from(&EncoderMethod::X264), "x264");
            assert_eq!(<&str>::from(&EncoderMethod::X265), "x265");
            assert_eq!(<&str>::from(&EncoderMethod::VVenC), "vvenc");
            assert_eq!(<&str>::from(&EncoderMethod::FFmpeg), "ffmpeg");
        }

        #[test]
        fn target_quality_metric_strum_serialization() {
            assert_eq!(<&str>::from(&QualityMetric::VMAF), "vmaf");
            assert_eq!(<&str>::from(&QualityMetric::SSIMULACRA2), "ssimulacra2");
            assert_eq!(<&str>::from(&QualityMetric::BUTTERAUGLI), "butteraugli");
            assert_eq!(
                <&str>::from(&QualityMetric::BUTTERAUGLI3Norm),
                "butteraugli-3"
            );
            assert_eq!(<&str>::from(&QualityMetric::XPSNR), "xpsnr");
            assert_eq!(<&str>::from(&QualityMetric::CVVDP), "cvvdp");
        }

        #[test]
        fn target_quality_profile_strum_serialization() {
            assert_eq!(<&str>::from(&QualityProfile::Fast), "fast");
            assert_eq!(<&str>::from(&QualityProfile::Standard), "standard");
            assert_eq!(<&str>::from(&QualityProfile::Slow), "slow");
        }

        #[test]
        fn scene_detection_method_strum_serialization() {
            assert_eq!(<&str>::from(&SceneDetectionMethod::None), "none");
            assert_eq!(<&str>::from(&SceneDetectionMethod::Fast), "fast");
            assert_eq!(<&str>::from(&SceneDetectionMethod::Standard), "standard");
        }
    }
}
