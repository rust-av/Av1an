use std::path::PathBuf;

use andean_condor::{
    models::{
        encoder::EncoderBase,
        sequence::{
            parallel_encode::{DEFAULT_MAX_SCENE_LENGTH_SECONDS, DEFAULT_MIN_SCENE_LENGTH_FRAMES},
            scene_concatenate::ConcatMethod,
            scene_detect::{SceneDetectionMethod as CoreSCDMethod, ScenecutMethod},
        },
    },
    vapoursynth::vapoursynth_filters::VapourSynthFilter,
};
use clap::{Parser as ClapParser, Subcommand};
use serde::{Deserialize, Serialize};
use strum::{Display as DisplayMacro, EnumString, IntoStaticStr};

use crate::commands::config::ConfigSubcommand;

pub mod config;
pub mod detect_scenes;
pub mod init;
pub mod start;

#[derive(ClapParser)]
#[command(
    name = "condor",
    about = "A simple, extensible Commandline tool for the Condor chunked encoding framework.",
    version = "0.0.1"
)]
pub struct CondorCli {
    #[command(subcommand)]
    pub command:     Commands,
    /// Specify the location of the config file. Defaults to `./condor.json`.
    #[arg(long)]
    pub config_file: Option<PathBuf>,
    #[arg(long)]
    pub logs:        Option<PathBuf>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize a new configuration.
    Init {
        input:   PathBuf,
        output:  PathBuf,
        #[arg(long)]
        temp:    Option<PathBuf>,
        #[arg(long, default_value_t = DecoderMethod::BestSource)]
        decoder: DecoderMethod,
        #[arg(long, default_value_t = ConcatMethod::MKVMerge)]
        concat:  ConcatMethod,
        #[arg(long, short('w'))]
        workers: Option<u8>,
    },
    /// Configuration management
    Config {
        #[command(subcommand)]
        subcommand: ConfigSubcommand,
    },
    /// Detect scenes (Triggers TUI)
    DetectScenes {
        #[arg(long)]
        method:            Option<SceneDetectionMethod>,
        #[arg(long)]
        min_scene_seconds: Option<usize>,
        #[arg(long)]
        max_scene_seconds: Option<usize>,
    },
    /// Benchmark the optimum amount of workers
    Benchmark {
        /// The minimum speed increase (in percent) required to add a worker.
        /// Defaults to 5%.
        #[arg(long)]
        threshold:  Option<u8>,
        /// The maximum amount of RAM (in megabytes) allowed across all workers
        #[arg(long)]
        max_memory: Option<u32>,
    },
    /// Start encoding (Triggers TUI)
    Start {
        #[arg(long)]
        temp:           Option<PathBuf>,
        #[arg(long)]
        input:          Option<PathBuf>,
        #[arg(long, short('i'))]
        decoder:        Option<DecoderMethod>,
        #[arg(long)]
        filters:        Option<Vec<VapourSynthFilter>>,
        #[arg(long, short('o'))]
        output:         Option<PathBuf>,
        #[arg(long)]
        concat:         Option<ConcatMethod>,
        #[arg(long, short('w'))]
        workers:        Option<u8>,
        #[arg(long, short('e'))]
        encoder:        Option<EncoderBase>,
        #[arg(long)]
        passes:         Option<u8>,
        #[arg(long, allow_hyphen_values = true)]
        params:         Option<String>,
        #[arg(long)]
        photon_noise:   Option<u32>,
        #[arg(long)]
        skip_benchmark: bool,
    },
    /// Clean temporary files
    Clean {
        #[arg(long)]
        all: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, EnumString, IntoStaticStr, DisplayMacro)]
pub enum SceneDetectionMethod {
    None,
    Fast,
    Standard,
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
            },
            SceneDetectionMethod::Standard => CoreSCDMethod::AVSceneChange {
                method:         ScenecutMethod::Standard,
                minimum_length: min_length,
                maximum_length: max_length,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, EnumString, IntoStaticStr, DisplayMacro)]
pub enum DecoderMethod {
    #[strum(serialize = "bestsource")]
    BestSource,
    #[strum(serialize = "vs-ffms2")]
    VSFFMS2,
    #[strum(serialize = "lsmash")]
    LSMASHWorks,
    #[strum(serialize = "dgdecnv")]
    DGDecodeNV,
    #[strum(serialize = "ffms2")]
    FFMS2,
}
