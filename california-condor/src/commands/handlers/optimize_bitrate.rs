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

// --- `condor optimize-bitrate info|set|reset` -----------------------------

/// `info` and `reset` keys of `sequence_config.bitrate_optimizer`.
#[derive(Debug, Clone, Subcommand)]
pub enum OptimizeBitrateKey {
    /// Bitrate sigma (σ) threshold for a scene to be optimized.
    SigmaThreshold,
}

/// Settings `condor optimize-bitrate set` can change.
#[derive(Debug, Clone, Subcommand)]
pub enum OptimizeBitrateSet {
    /// Bitrate sigma (σ) distance from the average before a scene is clamped.
    SigmaThreshold {
        #[arg(value_parser = value_parser!(u8).range(1..=10), value_name = "Sigma")]
        value: u8,
    },
}

/// The action triple attached to `condor optimize-bitrate`.
pub type OptimizeBitrateAction = ConfigAction<OptimizeBitrateKey, OptimizeBitrateSet>;

/// The `condor optimize-bitrate` configuration section.
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
            OptimizeBitrateSet::SigmaThreshold {
                value,
            } => {
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
                    value:    serde_json::to_value(&*optimizer)?,
                }))
            },
            Some(OptimizeBitrateKey::SigmaThreshold) => {
                optimizer.bitrate_sigma_threshold = None;
                Ok(Some(PropertyChange {
                    property: "sigma-threshold".to_owned(),
                    value:    serde_json::to_value(optimizer.bitrate_sigma_threshold)?,
                }))
            },
        }
    }
}

/// `condor optimize-bitrate info|set|reset`.
pub fn config(config_path: Option<&Path>, action: &OptimizeBitrateAction) -> Result<()> {
    edit::<OptimizeBitrateCrud>(config_path, action)
}

#[cfg(test)]
mod tests {
    use andean_condor::models::sequence::bitrate_optimizer::BitrateOptimizerConfig;

    use super::*;
    use crate::{
        commands::handlers::init::init_handler,
        test_helpers::{check_basic_config, default_config, get_test_video},
        utils::hash_path::hash_path,
    };

    #[test]
    fn encode_default_config() {
        let test_video = get_test_video();
        let input_abs = path_abs::PathAbs::new(&test_video.path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let temp = tempfile::tempdir().expect("temp directory");
        let temp_abs = path_abs::PathAbs::new(temp.path().join(hash_path(&input_abs)))
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let output = temp.path().join("out.mkv");
        let config_path = temp.path().join("condor.json");
        let config_path_abs = path_abs::PathAbs::new(&config_path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();

        let expected_config = default_config(&test_video, &output, &temp_abs);

        init_handler(
            // Simulate default directory to avoid changing CWD
            Some(&config_path),
            Some(&temp.path().join(hash_path(&input_abs))),
            &test_video.path,
            &output,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            true,
            None,
            None,
        )
        .expect("init_handler should succeed");
        let (config, found_config_path) = optimize_bitrate_handler(Some(&config_path), None)
            .expect("optimize_bitrate_handler should succeed");

        assert_eq!(
            found_config_path,
            config_path_abs,
            "config path is {}",
            config_path_abs.display()
        );

        check_basic_config(&config, &expected_config);
        assert!(config.condor.scenes.is_empty(), "scenes is empty");
    }

    #[test]
    fn optimize_bitrate_custom_config() {
        let test_video = get_test_video();
        let input_abs = path_abs::PathAbs::new(&test_video.path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let temp = tempfile::tempdir().expect("temp directory");
        let temp_abs = path_abs::PathAbs::new(temp.path().join(hash_path(&input_abs)))
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let output = temp.path().join("out.mkv");
        let config_path = temp.path().join("condor.json");
        let config_path_abs = path_abs::PathAbs::new(&config_path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();

        let mut expected_config = default_config(&test_video, &output, &temp_abs);
        expected_config.condor.sequence_config.bitrate_optimizer = BitrateOptimizerConfig {
            bitrate_sigma_threshold: Some(3),
        };
        // immutable shadow
        let expected_config = expected_config;

        init_handler(
            // Simulate default directory to avoid changing CWD
            Some(&config_path),
            Some(&temp.path().join(hash_path(&input_abs))),
            &test_video.path,
            &output,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            true,
            None,
            None,
        )
        .expect("init_handler should succeed");
        let (config, found_config_path) = optimize_bitrate_handler(Some(&config_path), Some(3))
            .expect("optimize_bitrate_handler should succeed");

        assert_eq!(
            found_config_path,
            config_path_abs,
            "config path is {}",
            config_path_abs.display()
        );
        check_basic_config(&config, &expected_config);
        assert!(config.condor.scenes.is_empty(), "scenes is empty");
    }
}
