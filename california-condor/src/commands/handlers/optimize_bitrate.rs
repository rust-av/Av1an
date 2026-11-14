use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::{Subcommand, value_parser};
use serde_json::Value;

use crate::{
    commands::{
        config::{ConfigAction, ConfigCommand, PropertyChange},
        handlers::{config::edit, load_configuration},
    },
    configuration::Configuration,
};

pub fn optimize_bitrate_handler(
    config_path: Option<&Path>,
    sigma_threshold: Option<u8>,
) -> Result<(Configuration, PathBuf)> {
    let (mut configuration, config_path) = load_configuration(config_path)?;

    configure_bitrate_optimizer(&mut configuration, sigma_threshold)?;

    configuration.admit_results()?;
    configuration.save(&config_path)?;

    Ok((configuration, config_path))
}

pub fn configure_bitrate_optimizer(
    configuration: &mut Configuration,
    sigma_threshold: Option<u8>,
) -> Result<()> {
    if let Some(threshold) = sigma_threshold {
        configuration.condor.sequence_config.bitrate_optimizer.bitrate_sigma_threshold =
            Some(threshold);
    }

    Ok(())
}

// --- `av1an optimize-bitrate info|set|reset` ------------------------------

/// `info` and `reset` keys of `sequence_config.bitrate_optimizer`.
#[derive(Debug, Clone, Subcommand)]
pub enum OptimizeBitrateKey {
    /// Bitrate sigma (σ) threshold for a scene to be optimized.
    SigmaThreshold,
}

/// Settings `av1an optimize-bitrate set` can change.
#[derive(Debug, Clone, Subcommand)]
pub enum OptimizeBitrateSet {
    /// Bitrate sigma (σ) distance from the average before a scene is clamped.
    SigmaThreshold {
        #[arg(value_parser = value_parser!(u8).range(1..=10), value_name = "Sigma")]
        value: u8,
    },
}

/// The action triple attached to `av1an optimize-bitrate`.
pub type OptimizeBitrateAction = ConfigAction<OptimizeBitrateKey, OptimizeBitrateSet>;

/// The `av1an optimize-bitrate` configuration section.
pub struct OptimizeBitrateCrud;

impl ConfigCommand for OptimizeBitrateCrud {
    type Key = OptimizeBitrateKey;
    type Set = OptimizeBitrateSet;

    fn subject() -> &'static str {
        "Bitrate optimization settings"
    }

    fn render(configuration: &Configuration, key: Option<&Self::Key>) -> Result<Value> {
        let optimizer = &configuration.condor.sequence_config.bitrate_optimizer;
        Ok(match key {
            None => serde_json::to_value(optimizer)?,
            Some(OptimizeBitrateKey::SigmaThreshold) => {
                serde_json::to_value(optimizer.bitrate_sigma_threshold)?
            },
        })
    }

    fn apply(configuration: &mut Configuration, set: &Self::Set) -> Result<PropertyChange> {
        match set {
            OptimizeBitrateSet::SigmaThreshold { value } => {
                configuration.condor.sequence_config.bitrate_optimizer.bitrate_sigma_threshold =
                    Some(*value);
                PropertyChange::new("sigma-threshold", value)
            },
        }
    }

    fn restore(
        configuration: &mut Configuration,
        key: Option<&Self::Key>,
    ) -> Result<Option<PropertyChange>> {
        let optimizer = &mut configuration.condor.sequence_config.bitrate_optimizer;
        match key {
            None => {
                *optimizer = Default::default();
                Ok(Some(PropertyChange {
                    property: "section".to_owned(),
                    value: serde_json::to_value(&*optimizer)?,
                }))
            },
            Some(OptimizeBitrateKey::SigmaThreshold) => {
                optimizer.bitrate_sigma_threshold = None;
                Ok(Some(PropertyChange {
                    property: "sigma-threshold".to_owned(),
                    value: serde_json::to_value(optimizer.bitrate_sigma_threshold)?,
                }))
            },
        }
    }
}

/// `av1an optimize-bitrate info|set|reset`.
pub fn config(config_path: Option<&Path>, action: &OptimizeBitrateAction) -> Result<()> {
    edit::<OptimizeBitrateCrud>(config_path, action)
}
