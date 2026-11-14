use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

use andean_condor::{
    models::{
        encoder::EncoderBase,
        sequence::scene_detector::{
            DEFAULT_MAX_SCENE_LENGTH_SECONDS, DEFAULT_MIN_SCENE_LENGTH_FRAMES,
            SceneDetectionMethod as CoreSCDMethod, ScenecutMethod,
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

#[cfg(test)]
mod tests;

#[derive(Debug, ClapParser)]
#[command(
    name = "av1an",
    about = "A simple, extensible Commandline tool for the Condor chunked encoding framework.",
    version,
    disable_version_flag = true,
    after_help = "Put execution options after the selected command (e.g. av1an encode -w 8). Root execution options require no subcommand. --config-file, --logs and --verbose work at either level. Target Quality run options need --target or an existing saved target."
)]
pub struct CondorCli {
    #[command(subcommand)]
    pub command: Option<Commands>,
    #[arg(long, global = true, value_name = "Config File", help = HELP_CONFIG_SHORT, long_help = HELP_CONFIG)]
    pub config_file: Option<PathBuf>,
    #[arg(long, global = true, value_name = "Temporary Directory", help = HELP_TEMP_SHORT, long_help = HELP_TEMP)]
    pub temp: Option<PathBuf>,
    #[arg(long, global = true, value_name = "Log File", help = HELP_LOGS_SHORT, long_help = HELP_LOGS)]
    pub logs: Option<PathBuf>,
    /// Enable verbose output and logging.
    #[arg(long, global = true, default_value_t = false)]
    pub verbose: bool,
    /// Display Encoder, VapourSynth and quality metric library installation
    /// information.
    #[arg(short('v'), long, global = true, default_value_t = false)]
    pub version: bool,
    // Main command arguments
    #[arg(long, short('i'), value_name = "Input", help = HELP_INPUT_SHORT, long_help = HELP_INPUT)]
    pub input: Option<PathBuf>,
    #[arg(long, short('o'), value_name = "Output", help = HELP_OUTPUT_SHORT, long_help = HELP_OUTPUT)]
    pub output: Option<PathBuf>,
    /// Save explicit consent to replace a distinct final output; never prompts.
    #[arg(long)]
    pub overwrite: bool,
    #[arg(long, value_name = "Scene Detector Input", help = HELP_SCD_INPUT_SHORT, long_help = HELP_SCD_INPUT)]
    pub scd_input: Option<PathBuf>,
    #[arg(long, value_name = "Target Quality Input", help = HELP_TQ_INPUT_SHORT, long_help = HELP_TQ_INPUT)]
    pub tq_input: Option<PathBuf>,
    #[arg(long, value_name = "Decoder", help = HELP_DECODER_SHORT, long_help = HELP_DECODER)]
    pub decoder: Option<DecoderMethod>,
    #[arg(long, value_name = "Decoder", help = HELP_SCD_DECODER_SHORT, long_help = HELP_SCD_DECODER)]
    pub scd_decoder: Option<DecoderMethod>,
    #[arg(long, value_name = "Decoder", help = HELP_TQ_DECODER_SHORT, long_help = HELP_TQ_DECODER)]
    pub tq_decoder: Option<DecoderMethod>,
    #[arg(long, value_name = "VapourSynth Filters", help = HELP_FILTERS_SHORT, long_help = HELP_FILTERS)]
    pub filters: Option<Vec<VapourSynthFilter>>,
    #[arg(long, value_name = "VapourSynth Filters", help = HELP_SCD_FILTERS_SHORT, long_help = HELP_SCD_FILTERS)]
    pub scd_filters: Option<Vec<VapourSynthFilter>>,
    #[arg(long, value_name = "VapourSynth Filters", help = HELP_TQ_FILTERS_SHORT, long_help = HELP_TQ_FILTERS)]
    pub tq_filters: Option<Vec<VapourSynthFilter>>,
    #[arg(long, value_name = "VapourSynth Arguments", help = HELP_VS_ARGS_SHORT, long_help = HELP_VS_ARGS)]
    pub vs_args: Option<Vec<String>>,
    #[arg(long, value_name = "VapourSynth Arguments", help = HELP_SCD_VS_ARGS_SHORT, long_help = HELP_SCD_VS_ARGS)]
    pub scd_vs_args: Option<Vec<String>>,
    #[arg(long, value_name = "VapourSynth Arguments", help = HELP_TQ_VS_ARGS_SHORT, long_help = HELP_TQ_VS_ARGS)]
    pub tq_vs_args: Option<Vec<String>>,
    #[arg(long, value_name = "Concatenation Method", help = HELP_CONCAT_SHORT, long_help = HELP_CONCAT)]
    pub concat: Option<ConcatenationMethod>,
    #[arg(long, value_name = "Category", value_enum, value_delimiter = ',', help = HELP_DISCARD_SHORT, long_help = HELP_DISCARD)]
    pub discard: Option<Vec<DiscardCategory>>,
    /// The amount of encoder processes to use at once
    #[arg(long, short('w'), value_name = "Workers", help = HELP_WORKERS_SHORT, long_help = HELP_WORKERS)]
    pub workers: Option<u8>,
    #[arg(long, value_parser = value_parser!(u32).range(1..), help = HELP_MAX_TRIES)]
    pub max_tries: Option<u32>,
    #[arg(long, short('e'), value_name = "Encoder", help = HELP_ENCODER_SHORT, long_help = HELP_ENCODER)]
    pub encoder: Option<EncoderMethod>,
    #[arg(long, value_name = "Passes", help = HELP_PASSES_SHORT, long_help = HELP_PASSES)]
    pub passes: Option<u8>,
    #[arg(long, value_name = "Encoder Parameters", allow_hyphen_values = true, help = HELP_PARAMS_SHORT, long_help = HELP_PARAMS)]
    pub params: Option<String>,
    #[arg(long, value_name = "Encoder Parameters", allow_hyphen_values = true, help = HELP_TQ_PARAMS_SHORT, long_help = HELP_TQ_PARAMS)]
    pub tq_params: Option<String>,
    #[arg(long, value_name = "ISO", help = HELP_PHOTON_NOISE_SHORT, long_help = HELP_PHOTON_NOISE)]
    pub photon_noise: Option<u32>,
    #[arg(long, value_name = "ISO", help = HELP_CHROMA_NOISE_SHORT, long_help = HELP_CHROMA_NOISE)]
    pub chroma_noise: Option<u32>,
    #[arg(long, value_name = "FFmpeg Filter", allow_hyphen_values = true, help = HELP_FFMPEG_FILTER_SHORT, long_help = HELP_FFMPEG_FILTER)]
    pub ffmpeg_filter: Option<String>,
    #[arg(long, default_value_t = false, help = HELP_DISABLE_COLOR_DATA_SHORT, long_help = HELP_DISABLE_COLOR_DATA)]
    pub disable_color_data: bool,
    #[arg(long, value_name = "Zone", allow_hyphen_values = true, help = HELP_ZONES_SHORT, long_help = HELP_ZONES)]
    pub zones: Option<Vec<String>>,
    #[arg(long, value_name = "Metric", help = HELP_TARGET_METRIC_SHORT, long_help = HELP_TARGET_METRIC)]
    pub target_metric: Option<QualityMetric>,
    #[arg(long, value_name = "Score", help = HELP_TARGET_SHORT, long_help = HELP_TARGET)]
    pub target: Option<f64>,
    #[arg(long, value_name = "Quantizer", help = HELP_MINIMUM_QUANTIZER_SHORT, long_help = HELP_MINIMUM_QUANTIZER)]
    pub minimum_quantizer: Option<u8>,
    #[arg(long, value_name = "Quantizer", help = HELP_MAXIMUM_QUANTIZER_SHORT, long_help = HELP_MAXIMUM_QUANTIZER)]
    pub maximum_quantizer: Option<u8>,
    #[arg(long, value_name = "Profile", help = HELP_TARGET_QUALITY_PROFILE_SHORT, long_help = HELP_TARGET_QUALITY_PROFILE)]
    pub target_profile: Option<QualityProfile>,
    /// Skip Scene Detection. Useful when encoding a subset of scenes.
    #[arg(long, default_value_t = false)]
    pub skip_scd: bool,
}

impl CondorCli {
    /// Admit only explicit globals before a subcommand; defaults are not supplied options.
    pub(crate) fn admit_root_arguments(
        matches: &clap::ArgMatches,
        cmd: &mut clap::Command,
    ) -> Result<(), clap::Error> {
        // COMPAT: Clap's args_conflicts_with_subcommands also rejects globals at the root.
        let conflicting_arg = cmd
            .get_arguments()
            .find(|arg| {
                !arg.is_global_set()
                    && matches.value_source(arg.get_id().as_str())
                        == Some(clap::parser::ValueSource::CommandLine)
            })
            .map(ToString::to_string);
        if let Some(command) = matches.subcommand_name()
            && let Some(arg) = conflicting_arg
        {
            let message = format!(
                "{arg} cannot be used with subcommand '{command}' at the root; put execution options after the selected command, or omit the subcommand for a root run"
            );
            return Err(cmd.error(clap::error::ErrorKind::ArgumentConflict, message));
        }
        Ok(())
    }

    /// Rejects the global temp flag on routes that never consume it, before logging or loading.
    pub(crate) fn reject_unused_temp(&self, cmd: &mut clap::Command) {
        if self.temp.is_some()
            && !matches!(
                &self.command,
                None | Some(Commands::Init { edit: None, .. })
                    | Some(Commands::Encode { edit: None, .. })
                    | Some(Commands::DetectScenes { edit: None, .. })
                    | Some(Commands::TargetQuality { edit: None, .. })
                    | Some(Commands::Concatenate { edit: None, .. })
                    | Some(Commands::QualityCheck { edit: None, .. })
                    | Some(Commands::Benchmark { edit: None, .. })
            )
        {
            cmd.error(
                clap::error::ErrorKind::ArgumentConflict,
                "this command does not use --temp; remove --temp. It is supported only for root, init-create, encode, detect-scenes, target-quality, concatenate, quality-check and benchmark runs, not configuration edits",
            )
            .exit();
        }
    }

    /// Rejects `av1an scenes INDEX add` as a usage error.
    ///
    /// `add` places the scene it inserts itself, so an index before it is not
    /// a form the command has; Clap cannot express an argument that conflicts
    /// with one subcommand, so the check runs after parsing and reports the
    /// conflict the way Clap would.
    pub(crate) fn reject_indexed_add(&self, cmd: &mut clap::Command) {
        if matches!(
            &self.command,
            Some(Commands::Scenes {
                index: Some(_),
                action: Some(handlers::scenes::ScenesAction::Add { .. }),
            })
        ) {
            let scenes = cmd.find_subcommand_mut("scenes").expect("the scenes subcommand exists");
            scenes
                .error(
                    clap::error::ErrorKind::ArgumentConflict,
                    "`add` creates a new scene and takes no index: run `av1an scenes add START \
                     END`",
                )
                .exit();
        }
    }

    /// Hyphen-accepting positionals make Clap capture flags after the first
    /// zone value. Recover only the reserved standalone dry-run flag.
    pub(crate) fn normalize_zone_flags(&mut self) {
        if let Some(Commands::Zone { zones, dry_run }) = &mut self.command {
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
    /// configuration without running anything: *av1an init info*, *set* and
    /// *reset*.
    #[command(args_conflicts_with_subcommands = true)]
    Init {
        #[command(subcommand)]
        edit: Option<handlers::init::InitAction>,
        #[arg(value_name = "Input", help = HELP_INPUT_SHORT, long_help = HELP_INPUT)]
        input: Option<PathBuf>,
        #[arg(value_name = "Output", help = HELP_OUTPUT_SHORT, long_help = HELP_OUTPUT)]
        output: Option<PathBuf>,
        /// Save explicit consent to replace a distinct final output; never prompts.
        #[arg(long)]
        overwrite: bool,
        #[arg(long, value_name = "Decoder", help = HELP_DECODER_SHORT, long_help = HELP_DECODER)]
        decoder: Option<DecoderMethod>,
        #[arg(long, value_name = "VapourSynth Filters", help = HELP_FILTERS_SHORT, long_help = HELP_FILTERS)]
        filters: Option<Vec<VapourSynthFilter>>,
        #[arg(long, value_name = "VapourSynth Arguments", help = HELP_VS_ARGS_SHORT, long_help = HELP_VS_ARGS)]
        vs_args: Option<Vec<String>>,
        #[arg(long, value_name = "Concatenation Method", help = HELP_CONCAT_SHORT, long_help = HELP_CONCAT)]
        concat: Option<ConcatenationMethod>,
        #[arg(long, value_name = "Category", value_enum, value_delimiter = ',', help = HELP_DISCARD_SHORT, long_help = HELP_DISCARD)]
        discard: Option<Vec<DiscardCategory>>,
        /// The amount of encoder processes to use at once
        #[arg(long, short('w'), value_name = "Workers", help = HELP_WORKERS_SHORT, long_help = HELP_WORKERS)]
        workers: Option<u8>,
        #[arg(long, value_parser = value_parser!(u32).range(1..), help = HELP_MAX_TRIES)]
        max_tries: Option<u32>,
        #[arg(long, short('e'), value_name = "Encoder", help = HELP_ENCODER_SHORT, long_help = HELP_ENCODER)]
        encoder: Option<EncoderMethod>,
        #[arg(long, value_name = "Encoder Parameters", allow_hyphen_values = true, help = HELP_PARAMS_SHORT, long_help = HELP_PARAMS)]
        params: Option<String>,
        #[arg(long, value_name = "ISO", help = HELP_PHOTON_NOISE_SHORT, long_help = HELP_PHOTON_NOISE)]
        photon_noise: Option<u32>,
        /// Do not append the input's colour and HDR metadata to the encoder
        /// parameters.
        #[arg(long, default_value_t = false, help = HELP_DISABLE_COLOR_DATA_SHORT, long_help = HELP_DISABLE_COLOR_DATA)]
        disable_color_data: bool,
        #[arg(long, value_name = "Metric", help = HELP_TARGET_METRIC_SHORT, long_help = HELP_TARGET_METRIC)]
        target_metric: Option<QualityMetric>,
        #[arg(long, value_name = "Score", help = HELP_TARGET_SHORT, long_help = HELP_TARGET)]
        target: Option<f64>,
    },
    /// Detect scenes (Triggers TUI).
    ///
    /// Also reads, changes or resets its saved settings without running:
    /// *av1an detect-scenes info*, *set* and *reset*.
    #[command(args_conflicts_with_subcommands = true)]
    DetectScenes {
        #[command(subcommand)]
        edit: Option<handlers::detect_scenes::DetectScenesAction>,
        #[arg(long, short('i'), value_name = "Input", help = HELP_SCD_INPUT_SHORT, long_help = HELP_SCD_INPUT)]
        input: Option<PathBuf>,
        #[arg(long, value_name = "Decoder", help = HELP_DECODER_SHORT, long_help = HELP_DECODER)]
        decoder: Option<DecoderMethod>,
        #[arg(long, value_name = "VapourSynth Filters", help = HELP_SCD_FILTERS_SHORT, long_help = HELP_SCD_FILTERS)]
        filters: Option<Vec<VapourSynthFilter>>,
        #[arg(long, value_name = "VapourSynth Arguments", help = HELP_SCD_VS_ARGS_SHORT, long_help = HELP_SCD_VS_ARGS)]
        vs_args: Option<Vec<String>>,
        #[arg(long, value_name = "Scene Detection Method", help = HELP_SCD_METHOD_SHORT, long_help = HELP_SCD_METHOD)]
        method: Option<SceneDetectionMethod>,
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
        zones: Vec<String>,
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
    /// To add a scene, run `av1an scenes add START END`: it places the new
    /// scene itself, so it takes no index.
    Scenes {
        /// Which scenes to address: `1`, `0-4`, `1,3`, `1-2,8-10,16`, or
        /// `-1`/`all`/`every` for every scene.
        #[arg(value_name = "Index", allow_hyphen_values = true)]
        index: Option<crate::scenes::SceneSelection>,
        #[command(subcommand)]
        action: Option<handlers::scenes::ScenesAction>,
    },
    /// Detect noise per scene (Triggers TUI).
    ///
    /// Also reads, changes or resets its saved settings without running:
    /// *av1an detect-noise info*, *set* and *reset*.
    #[command(args_conflicts_with_subcommands = true)]
    DetectNoise {
        #[command(subcommand)]
        edit: Option<handlers::detect_noise::DetectNoiseAction>,
        #[arg(long, value_name = "Noise Detector Input", help = HELP_ND_INPUT_SHORT, long_help = HELP_ND_INPUT)]
        input: Option<PathBuf>,
        #[arg(long, value_name = "Decoder", help = HELP_DECODER_SHORT, long_help = HELP_DECODER)]
        decoder: Option<DecoderMethod>,
        #[arg(long, value_name = "VapourSynth Arguments", help = HELP_VS_ARGS_SHORT, long_help = HELP_VS_ARGS)]
        vs_args: Option<Vec<String>>,
        #[arg(long, value_name = "Reference VapourSynth Filters", help = HELP_REFERENCE_FILTERS_SHORT, long_help = HELP_REFERENCE_FILTERS)]
        reference_filters: Option<Vec<VapourSynthFilter>>,
        #[arg(long, value_name = "Denoised VapourSynth Filters", help = HELP_DENOISED_FILTERS_SHORT, long_help = HELP_DENOISED_FILTERS)]
        denoised_filters: Option<Vec<VapourSynthFilter>>,
    },
    /// Scale Photon Noise ISO per scene based on Noise Detector results.
    ///
    /// Scenes must have Photon Noise (*--photon-noise*/*--chroma-noise*)
    /// configured. Scenes below the threshold will not be scaled.
    ///
    /// Also reads, changes or resets its saved settings without running:
    /// *av1an scale-noise info*, *set* and *reset*.
    #[command(args_conflicts_with_subcommands = true)]
    ScaleNoise {
        #[command(subcommand)]
        edit: Option<handlers::scale_noise::ScaleNoiseAction>,
        /// Minimum noise value for a scene to scale Photon Noise ISO.
        ///
        /// Recommended: 0.002
        #[arg(long, value_name = "Threshold")]
        threshold: Option<f64>,
        /// Minimum scale factor for Photon Noise ISO scaling.
        #[arg(long, value_name = "Scaler")]
        minimum_scaler: Option<f64>,
        /// Maximum scale factor for Photon Noise ISO scaling.
        #[arg(long, value_name = "Scaler")]
        maximum_scaler: Option<f64>,
        /// Scale Chroma ISO.
        #[arg(long)]
        scale_chroma: bool,
    },
    /// Benchmark the optimum amount of workers (Triggers TUI).
    ///
    /// Also reads, changes or resets its saved settings without running:
    /// *av1an benchmark info*, *set* and *reset*.
    #[command(args_conflicts_with_subcommands = true)]
    Benchmark {
        #[command(subcommand)]
        edit: Option<handlers::benchmarker::BenchmarkAction>,
        /// The minimum speed increase (in percent) required to add an
        /// additional worker.
        ///
        /// Defaults to `5`.
        #[arg(long, value_name = "Percent", value_parser = value_parser!(u8).range(0..=100))]
        threshold: Option<u8>,
        /// The maximum amount of RAM (in megabytes) allowed across all workers
        /// (unimplemented)
        #[arg(long, value_name = "Megabytes", hide = true)]
        max_memory: Option<u32>,
    },
    /// Calculate the optimum quantizer per scene for a given metric target
    /// (Triggers TUI).
    ///
    /// Also reads, changes or resets its saved settings without running:
    /// *av1an target-quality info*, *set* and *reset*.
    #[command(args_conflicts_with_subcommands = true)]
    TargetQuality {
        #[command(subcommand)]
        edit: Option<handlers::target_quality::TargetQualityAction>,
        #[arg(long, short('i'), value_name = "Input", help = HELP_INPUT_SHORT, long_help = HELP_INPUT)]
        input: Option<PathBuf>,
        #[arg(long, value_name = "Decoder", help = HELP_DECODER_SHORT, long_help = HELP_DECODER)]
        decoder: Option<DecoderMethod>,
        #[arg(long, value_name = "VapourSynth Filters")]
        filters: Option<Vec<VapourSynthFilter>>,
        #[arg(long, value_name = "VapourSynth Arguments", help = HELP_VS_ARGS_SHORT, long_help = HELP_VS_ARGS)]
        vs_args: Option<Vec<String>>,
        #[arg(long, value_name = "Encoder Parameters", allow_hyphen_values = true, help = HELP_PROBE_PARAMS_SHORT, long_help = HELP_PROBE_PARAMS)]
        params: Option<String>,
        #[arg(long, value_name = "Metric", help = HELP_TARGET_METRIC_SHORT, long_help = HELP_TARGET_METRIC)]
        metric: Option<QualityMetric>,
        #[arg(long, value_name = "Score", help = HELP_TARGET_SHORT, long_help = HELP_TARGET)]
        target: Option<f64>,
        #[arg(long("min-q"), value_name = "Quantizer", help = HELP_MINIMUM_QUANTIZER_SHORT, long_help = HELP_MINIMUM_QUANTIZER)]
        minimum_quantizer: Option<u8>,
        #[arg(long("max-q"), value_name = "Quantizer", help = HELP_MAXIMUM_QUANTIZER_SHORT, long_help = HELP_MAXIMUM_QUANTIZER)]
        maximum_quantizer: Option<u8>,
        #[arg(long, value_name = "Profile", help = HELP_TARGET_QUALITY_PROFILE_SHORT, long_help = HELP_TARGET_QUALITY_PROFILE)]
        profile: Option<QualityProfile>,
    },
    /// Optimize bitrate for scenes that exceed normal bitrate after Target
    /// Quality.
    ///
    /// Scenes with a bitrate above the normal threshold will be optimized by
    /// clamping the quantizer to the average quantizer value.
    ///
    /// Also reads, changes or resets its saved settings without running:
    /// *av1an optimize-bitrate info*, *set* and *reset*.
    #[command(args_conflicts_with_subcommands = true)]
    OptimizeBitrate {
        #[command(subcommand)]
        edit: Option<handlers::optimize_bitrate::OptimizeBitrateAction>,
        /// Minimum bitrate sigma (σ) threshold for a scene to be optimized.
        #[arg(long, value_name = "Sigma", value_parser = value_parser!(u8).range(1..=10))]
        sigma_threshold: Option<u8>,
    },
    /// Apply speed based on quantizer per scene using Convex Hull
    /// interpolation.
    ///
    /// Two points use linear interpolation; three or more use a natural cubic
    /// spline, bounded to the configured speed range. Quantizers must be unique.
    ///
    /// Also reads, changes or resets its saved settings without running:
    /// *av1an scale-speed info*, *set* and *reset*.
    #[command(args_conflicts_with_subcommands = true)]
    ScaleSpeed {
        #[command(subcommand)]
        edit: Option<handlers::scale_speed::ScaleSpeedAction>,
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
        speeds: Option<Vec<i8>>,
    },
    /// Encode scenes in parallel (Triggers TUI).
    ///
    /// Also reads, changes or resets its saved settings without running:
    /// *av1an encode info*, *set* and *reset*.
    #[command(args_conflicts_with_subcommands = true)]
    Encode {
        #[command(subcommand)]
        edit: Option<handlers::encode::EncodeAction>,
        #[arg(long, short('i'), value_name = "Input", help = HELP_INPUT_SHORT, long_help = HELP_INPUT)]
        input: Option<PathBuf>,
        #[arg(long, value_name = "Decoder", help = HELP_DECODER_SHORT, long_help = HELP_DECODER)]
        decoder: Option<DecoderMethod>,
        #[arg(long, value_name = "VapourSynth Filters", help = HELP_FILTERS_SHORT, long_help = HELP_FILTERS)]
        filters: Option<Vec<VapourSynthFilter>>,
        #[arg(long, value_name = "VapourSynth Arguments", help = HELP_VS_ARGS_SHORT, long_help = HELP_VS_ARGS)]
        vs_args: Option<Vec<String>>,
        /// The amount of encoder processes to use at once
        #[arg(long, short('w'), value_name = "Workers", help = HELP_WORKERS_SHORT, long_help = HELP_WORKERS)]
        workers: Option<u8>,
        #[arg(long, value_parser = value_parser!(u32).range(1..), help = HELP_MAX_TRIES)]
        max_tries: Option<u32>,
        #[arg(long, short('e'), value_name = "Encoder", help = HELP_ENCODER_SHORT, long_help = HELP_ENCODER)]
        encoder: Option<EncoderMethod>,
        #[arg(long, value_name = "Passes", help = HELP_PASSES_SHORT, long_help = HELP_PASSES)]
        passes: Option<u8>,
        #[arg(long, value_name = "Encoder Parameters", allow_hyphen_values = true, help = HELP_PARAMS_SHORT, long_help = HELP_PARAMS)]
        params: Option<String>,
        #[arg(long, value_name = "ISO", help = HELP_PHOTON_NOISE_SHORT, long_help = HELP_PHOTON_NOISE)]
        photon_noise: Option<u32>,
        #[arg(long, value_name = "ISO", help = HELP_CHROMA_NOISE_SHORT, long_help = HELP_CHROMA_NOISE)]
        chroma_noise: Option<u32>,
        #[arg(long, value_name = "FFmpeg Filter", allow_hyphen_values = true, help = HELP_FFMPEG_FILTER_SHORT, long_help = HELP_FFMPEG_FILTER)]
        ffmpeg_filter: Option<String>,
    },
    /// Concatenate encoded scenes into output video (Triggers TUI).
    ///
    /// Also reads, changes or resets its saved settings without running:
    /// *av1an concatenate info*, *set* and *reset*.
    #[command(args_conflicts_with_subcommands = true)]
    Concatenate {
        /// Save explicit consent to replace a distinct final output; never prompts.
        #[arg(long)]
        overwrite: bool,
        #[command(subcommand)]
        edit: Option<handlers::concatenate::ConcatenateAction>,
        #[arg(long, value_name = "Concatenation Method", help = HELP_CONCAT_SHORT, long_help = HELP_CONCAT)]
        method: Option<ConcatenationMethod>,
        #[arg(long, value_name = "Category", value_enum, value_delimiter = ',', help = HELP_DISCARD_SHORT, long_help = HELP_DISCARD)]
        discard: Option<Vec<DiscardCategory>>,
    },
    /// Measure the quality of the concatenated output per scene (Triggers
    /// TUI).
    ///
    /// Also reads, changes or resets its saved settings without running:
    /// *av1an quality-check info*, *set* and *reset*.
    #[command(args_conflicts_with_subcommands = true)]
    QualityCheck {
        #[command(subcommand)]
        edit: Option<handlers::quality_check::QualityCheckAction>,
        #[arg(long, short('i'), value_name = "Input", help = HELP_INPUT_SHORT, long_help = HELP_INPUT)]
        input: Option<PathBuf>,
        #[arg(long, value_name = "Decoder", help = HELP_DECODER_SHORT, long_help = HELP_DECODER)]
        decoder: Option<DecoderMethod>,
        #[arg(long, value_name = "VapourSynth Filters", help = HELP_FILTERS_SHORT, long_help = HELP_FILTERS)]
        filters: Option<Vec<VapourSynthFilter>>,
        #[arg(long, value_name = "VapourSynth Arguments", help = HELP_VS_ARGS_SHORT, long_help = HELP_VS_ARGS)]
        vs_args: Option<Vec<String>>,
        #[arg(long, value_name = "Metric", help = HELP_QUALITY_CHECK_METRIC_SHORT, long_help = HELP_QUALITY_CHECK_METRIC)]
        metric: Option<QualityMetric>,
        #[arg(long, value_name = "Profile", help = HELP_QUALITY_CHECK_PROFILE_SHORT, long_help = HELP_QUALITY_CHECK_PROFILE)]
        profile: Option<QualityProfile>,
    },
    /// Cleanup is unavailable; no files removed.
    ///
    /// Start a new job with a different --config-file, a distinct empty --temp directory,
    /// and a distinct output.
    Clean {
        /// Reserved; not implemented. No files removed.
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
                method: ScenecutMethod::Fast,
                minimum_length: min_length,
                maximum_length: max_length,
                save_scores: false,
            },
            SceneDetectionMethod::Standard => CoreSCDMethod::AVSceneChange {
                method: ScenecutMethod::Standard,
                minimum_length: min_length,
                maximum_length: max_length,
                save_scores: false,
            },
            SceneDetectionMethod::TransNetV2 => CoreSCDMethod::TransNetV2 {
                minimum_length: min_length,
                maximum_length: max_length,
                model_path: None,
                save_scores: false,
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
    #[error("No config file found. Run 'av1an init' to create a configuration.")]
    NoConfig,
    #[error("No scenes found in the config. Run 'av1an detect-scenes' to populate scenes")]
    NoScenes,
    #[error("Input {0} must be a VapourSynth script (.vpy or .py)")]
    InvalidVapourSynthScript(PathBuf),
}
