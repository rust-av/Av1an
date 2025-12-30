use clap::Subcommand;

#[derive(Subcommand)]
pub enum ConfigEncoderSubcommand {
    Set {
        encoder: String,
        #[arg(long)]
        params:  Option<String>,
    },
}
