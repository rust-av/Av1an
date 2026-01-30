use clap::Subcommand;

#[derive(Subcommand)]
pub enum ConfigBenchmarkerSubcommand {
    Enable,
    Disable,
    Set {
        #[arg(long)]
        threshold: Option<f64>,
        #[arg(long)]
        max_mem:   Option<u32>,
    },
}
