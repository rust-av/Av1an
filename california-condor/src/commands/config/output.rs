use std::path::PathBuf;

use andean_condor::models::sequence::scene_concatenator::ConcatMethod;
use clap::Subcommand;

#[derive(Subcommand)]
pub enum ConfigOutputSubcommand {
    Set {
        path:   PathBuf,
        #[arg(long)]
        concat: Option<ConcatMethod>,
    },
}
