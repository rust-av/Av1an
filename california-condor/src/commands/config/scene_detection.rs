use clap::Subcommand;

use crate::commands::{config::input::ConfigInputSubcommand, SceneDetectionMethod};

#[derive(Subcommand)]
pub enum ConfigSceneDetectionSubcommand {
    Set {
        method: SceneDetectionMethod,
        #[arg(long)]
        min:    Option<usize>,
        #[arg(long)]
        max:    Option<usize>,
        #[command(subcommand)]
        input:  Option<ConfigInputSubcommand>,
    },
}
