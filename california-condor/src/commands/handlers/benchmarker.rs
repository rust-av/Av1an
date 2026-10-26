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
    configuration.condor.sequence_config.benchmarker.max_memory = max_memory;

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

/// Settings `condor benchmark set` can change.
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

/// The action triple attached to `condor benchmark`.
pub type BenchmarkAction = ConfigAction<BenchmarkKey, BenchmarkSet>;

/// The `condor benchmark` configuration section.
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
            BenchmarkSet::Threshold {
                value,
            } => {
                benchmarker.threshold = *value;
                PropertyChange::new("threshold", value)
            },
            BenchmarkSet::MaxMemory {
                value,
            } => {
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
                    value:    serde_json::to_value(&*benchmarker)?,
                }
            },
            Some(BenchmarkKey::Threshold) => {
                benchmarker.threshold = defaults.threshold;
                PropertyChange {
                    property: "threshold".to_owned(),
                    value:    serde_json::to_value(benchmarker.threshold)?,
                }
            },
            Some(BenchmarkKey::MaxMemory) => {
                benchmarker.max_memory = None;
                PropertyChange {
                    property: "max-memory".to_owned(),
                    value:    serde_json::to_value(benchmarker.max_memory)?,
                }
            },
        }))
    }
}

/// `condor benchmark info|set|reset`.
pub fn config(config_path: Option<&Path>, action: &BenchmarkAction) -> Result<()> {
    edit::<BenchmarkCrud>(config_path, action)
}

#[cfg(test)]
mod tests {
    use andean_condor::models::sequence::benchmarker::BenchmarkerConfig;

    use super::*;
    use crate::{
        commands::handlers::init::init_handler,
        test_helpers::{check_basic_config, default_config, get_test_video},
        utils::hash_path::hash_path,
    };

    #[test]
    fn benchmarker_default_config() {
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
        let (config, found_config_path) = benchmarker_handler(Some(&config_path), None, None, None)
            .expect("detect_noise_handler should succeed");

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
    fn benchmarker_custom_config() {
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
        expected_config.condor.sequence_config.benchmarker = BenchmarkerConfig {
            threshold:  1,
            max_memory: Some(4096),
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
        let (config, found_config_path) = benchmarker_handler(
            Some(&config_path),
            Some(&temp.path().join(hash_path(&input_abs))),
            Some(1),
            Some(4096),
        )
        .expect("benchmarker_handler should succeed");

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
