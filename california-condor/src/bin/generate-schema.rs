//! Generates the JSON schema for the Condor configuration file.
//!
//! Run with `cargo run --release --bin generate-schema` and redirect stdout
//! to `configuration.schema.json`. The schema is published as a GitHub
//! release asset alongside the `av1an` executable.

use california_condor::configuration::{CONFIGURATION_SCHEMA_URL, Configuration};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut schema = schemars::schema_for!(Configuration);
    let schema_url = option_env!("CONDOR_SCHEMA_URL").unwrap_or(CONFIGURATION_SCHEMA_URL);
    schema.insert("$id".to_owned(), schema_url.into());
    println!("{}", serde_json::to_string_pretty(&schema)?);
    Ok(())
}
