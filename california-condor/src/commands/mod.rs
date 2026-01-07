use std::path::PathBuf;

use andean_condor::{
    models::{
        encoder::EncoderBase,
        sequence::{
            scene_concatenator::ConcatMethod,
            scene_detector::{
                SceneDetectionMethod as CoreSCDMethod,
                ScenecutMethod,
                DEFAULT_MAX_SCENE_LENGTH_SECONDS,
                DEFAULT_MIN_SCENE_LENGTH_FRAMES,
            },
        },
    },
    vapoursynth::vapoursynth_filters::VapourSynthFilter,
};
use clap::{Parser as ClapParser, Subcommand};
use serde::{Deserialize, Serialize};
use strum::{Display as DisplayMacro, EnumString, IntoStaticStr};

use crate::commands::config::ConfigSubcommand;

pub mod benchmarker;
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
        #[arg(long)]
        vs_args: Option<Vec<String>>,
    },
    /// Configuration management
    Config {
        #[command(subcommand)]
        subcommand: ConfigSubcommand,
    },
    /// Detect scenes (Triggers TUI)
    DetectScenes {
        #[arg(long, short('i'))]
        input:             Option<PathBuf>,
        #[arg(long)]
        decoder:           Option<DecoderMethod>,
        #[arg(long)]
        filters:           Option<Vec<VapourSynthFilter>>,
        #[arg(long)]
        vs_args:           Option<Vec<String>>,
        #[arg(long)]
        method:            Option<SceneDetectionMethod>,
        #[arg(long)]
        min_scene_seconds: Option<usize>,
        #[arg(long)]
        max_scene_seconds: Option<usize>,
    },
    /// Benchmark the optimum amount of workers (Triggers TUI)
    Benchmark {
        #[arg(long)]
        temp:       Option<PathBuf>,
        #[arg(long, short('i'))]
        input:      Option<PathBuf>,
        #[arg(long)]
        decoder:    Option<DecoderMethod>,
        #[arg(long)]
        filters:    Option<Vec<VapourSynthFilter>>,
        #[arg(long)]
        vs_args:    Option<Vec<String>>,
        encoder:    Option<EncoderBase>,
        #[arg(long)]
        passes:     Option<u8>,
        #[arg(long, allow_hyphen_values = true)]
        params:     Option<String>,
        /// The minimum speed increase (in percent) required to add an
        /// additional worker. Defaults to 5%.
        #[arg(long, value_parser = clap::value_parser!(u8).range(0..=100))]
        threshold:  Option<u8>,
        /// The maximum amount of RAM (in megabytes) allowed across all workers
        #[arg(long)]
        max_memory: Option<u32>,
    },
    /// Start encoding (Triggers TUI)
    Start {
        #[arg(long)]
        temp:         Option<PathBuf>,
        #[arg(long, short('i'))]
        input:        Option<PathBuf>,
        #[arg(long)]
        scd_input:    Option<PathBuf>,
        #[arg(long)]
        decoder:      Option<DecoderMethod>,
        #[arg(long)]
        filters:      Option<Vec<VapourSynthFilter>>,
        #[arg(long)]
        scd_filters:  Option<Vec<VapourSynthFilter>>,
        #[arg(long)]
        vs_args:      Option<Vec<String>>,
        #[arg(long)]
        scd_vs_args:  Option<Vec<String>>,
        #[arg(long, short('o'))]
        output:       Option<PathBuf>,
        #[arg(long)]
        concat:       Option<ConcatMethod>,
        /// The amount of encoder processes to use at once
        #[arg(long, short('w'))]
        workers:      Option<u8>,
        #[arg(long, short('e'))]
        encoder:      Option<EncoderBase>,
        #[arg(long)]
        passes:       Option<u8>,
        #[arg(long, allow_hyphen_values = true)]
        params:       Option<String>,
        #[arg(long)]
        photon_noise: Option<u32>,
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
