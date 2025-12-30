use std::path::PathBuf;

use av1an_core::ConcatMethod;
use clap::Subcommand;

#[derive(Subcommand)]
pub enum ConfigOutputSubcommand {
    Set {
        path:   PathBuf,
        #[arg(long)]
        concat: Option<ConcatMethod>,
    },
}
