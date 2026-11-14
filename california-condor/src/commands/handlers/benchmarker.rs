use std::path::{Path, PathBuf};

use andean_condor::models::sequence::benchmarker::BenchmarkerConfig;
use anyhow::Result;
use clap::{Subcommand, value_parser};
use serde_json::Value;

use crate::{
    commands::{
        config::{ConfigAction, ConfigCommand, PropertyChange},
        handlers::{config::edit, configure_temp, load_configuration},
    },
    configuration::Configuration,
};

#[allow(clippy::too_many_arguments)]
pub fn benchmarker_handler(
    config_path: Option<&Path>,
    temp_path: Option<&Path>,
    threshold: Option<u8>,
    max_memory: Option<u32>,
) -> Result<(Configuration, PathBuf)> {
    let (mut configuration, config_path) = load_configuration(config_path)?;

    configure_temp(&mut configuration, temp_path)?;
    configure_benchmarker(&mut configuration, threshold, max_memory)?;

    configuration.admit_results()?;
    configuration.save(&config_path)?;

    Ok((configuration, config_path))
}

#[allow(clippy::too_many_arguments)]
pub fn configure_benchmarker(
    configuration: &mut Configuration,
    threshold: Option<u8>,
    max_memory: Option<u32>,
) -> Result<()> {
    if let Some(threshold) = threshold {
        configuration.condor.sequence_config.benchmarker.threshold = threshold;
    }
    if let Some(max_memory) = max_memory {
        configuration.condor.sequence_config.benchmarker.max_memory = Some(max_memory);
    }

    Ok(())
}

/// `info` and `reset` keys of `sequence_config.benchmarker`.
#[derive(Debug, Clone, Subcommand)]
pub enum BenchmarkKey {
    /// Speed increase required to add another worker.
    Threshold,
    /// RAM allowed across all workers.
    MaxMemory,
}

/// Settings `av1an benchmark set` can change.
#[derive(Debug, Clone, Subcommand)]
pub enum BenchmarkSet {
    /// Speed increase (in percent) required to add an additional worker.
    Threshold {
        #[arg(value_parser = value_parser!(u8).range(0..=100), value_name = "Percent")]
        value: u8,
    },
    /// Maximum RAM (in megabytes) allowed across all workers.
    MaxMemory {
        #[arg(value_name = "Megabytes")]
        value: u32,
    },
}

/// The action triple attached to `av1an benchmark`.
pub type BenchmarkAction = ConfigAction<BenchmarkKey, BenchmarkSet>;

/// The `av1an benchmark` configuration section.
pub struct BenchmarkCrud;

impl ConfigCommand for BenchmarkCrud {
    type Key = BenchmarkKey;
    type Set = BenchmarkSet;

    fn subject() -> &'static str {
        "Benchmark settings"
    }

    fn render(configuration: &Configuration, key: Option<&Self::Key>) -> Result<Value> {
        let benchmarker = &configuration.condor.sequence_config.benchmarker;
        Ok(match key {
            None => serde_json::to_value(benchmarker)?,
            Some(BenchmarkKey::Threshold) => serde_json::to_value(benchmarker.threshold)?,
            Some(BenchmarkKey::MaxMemory) => serde_json::to_value(benchmarker.max_memory)?,
        })
    }

    fn apply(configuration: &mut Configuration, set: &Self::Set) -> Result<PropertyChange> {
        let benchmarker = &mut configuration.condor.sequence_config.benchmarker;
        match set {
            BenchmarkSet::Threshold { value } => {
                benchmarker.threshold = *value;
                PropertyChange::new("threshold", value)
            },
            BenchmarkSet::MaxMemory { value } => {
                benchmarker.max_memory = Some(*value);
                PropertyChange::new("max-memory", value)
            },
        }
    }

    fn restore(
        configuration: &mut Configuration,
        key: Option<&Self::Key>,
    ) -> Result<Option<PropertyChange>> {
        let benchmarker = &mut configuration.condor.sequence_config.benchmarker;
        let defaults = BenchmarkerConfig::default();
        Ok(Some(match key {
            None => {
                *benchmarker = defaults;
                PropertyChange {
                    property: "section".to_owned(),
                    value: serde_json::to_value(&*benchmarker)?,
                }
            },
            Some(BenchmarkKey::Threshold) => {
                benchmarker.threshold = defaults.threshold;
                PropertyChange {
                    property: "threshold".to_owned(),
                    value: serde_json::to_value(benchmarker.threshold)?,
                }
            },
            Some(BenchmarkKey::MaxMemory) => {
                benchmarker.max_memory = None;
                PropertyChange {
                    property: "max-memory".to_owned(),
                    value: serde_json::to_value(benchmarker.max_memory)?,
                }
            },
        }))
    }
}

/// `av1an benchmark info|set|reset`.
pub fn config(config_path: Option<&Path>, action: &BenchmarkAction) -> Result<()> {
    edit::<BenchmarkCrud>(config_path, action)
}
