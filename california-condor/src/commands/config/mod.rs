use std::path::PathBuf;

use anyhow::Result;
use clap::Subcommand;

use crate::commands::config::{
    benchmarker::ConfigBenchmarkerSubcommand,
    encoder::ConfigEncoderSubcommand,
    input::ConfigInputSubcommand,
    output::ConfigOutputSubcommand,
    scene_detection::ConfigSceneDetectionSubcommand,
};

pub mod benchmarker;
pub mod encoder;
pub mod input;
pub mod output;
pub mod scene_detection;
// pub mod scenes;

#[derive(Subcommand)]
pub enum ConfigSubcommand {
    Input {
        #[command(subcommand)]
        action: ConfigInputSubcommand,
    },
    Output {
        #[command(subcommand)]
        action: ConfigOutputSubcommand,
    },
    SceneDetection {
        #[command(subcommand)]
        action: ConfigSceneDetectionSubcommand,
    },
    Benchmarker {
        #[command(subcommand)]
        action: ConfigBenchmarkerSubcommand,
    },
    Encoder {
        #[command(subcommand)]
        action: ConfigEncoderSubcommand,
    },
    Scenes {
        index:   Option<usize>,
        #[arg(long)]
        encoder: Option<String>,
        #[arg(long)]
        params:  Option<String>,
    },
}

pub fn config_sub_handler(
    config_path: Option<PathBuf>,
    subcommand: ConfigSubcommand,
) -> Result<()> {
    match subcommand {
        ConfigSubcommand::Input {
            action,
        } => todo!(),
        ConfigSubcommand::Output {
            action,
        } => todo!(),
        ConfigSubcommand::SceneDetection {
            action,
        } => todo!(),
        ConfigSubcommand::Benchmarker {
            action,
        } => todo!(),
        ConfigSubcommand::Encoder {
            action,
        } => todo!(),
        ConfigSubcommand::Scenes {
            index,
            encoder,
            params,
        } => todo!(),
    }

    Ok(())
}
