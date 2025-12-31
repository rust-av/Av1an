use clap::Subcommand;

#[derive(Subcommand)]
pub enum ConfigEncoderSubcommand {
    Set {
        encoder: String,
        #[arg(long, allow_hyphen_values = true)]
        params:  Option<String>,
    },
}
