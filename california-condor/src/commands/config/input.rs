use std::path::PathBuf;

use andean_condor::vapoursynth::vapoursynth_filters::VapourSynthFilter;
use clap::Subcommand;

use crate::commands::DecoderMethod;

#[derive(Subcommand)]
pub enum ConfigInputSubcommand {
    Set {
        path:    PathBuf,
        #[arg(long)]
        decoder: Option<DecoderMethod>,
        #[arg(long)]
        filters: Option<Vec<VapourSynthFilter>>,
    },
}
