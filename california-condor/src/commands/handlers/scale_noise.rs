use std::path::{Path, PathBuf};

use andean_condor::models::sequence::noise_scaler::NoiseScalerConfig;
pub use andean_condor::models::sequence::noise_scaler::NoiseScalerConfigError as ScaleNoiseConfigError;
use anyhow::Result;
use clap::Subcommand;
use serde_json::Value;
use tracing::warn;

use crate::{
    commands::{
        config::{ConfigAction, ConfigCommand, PropertyChange, Toggle},
        handlers::{config::edit, load_configuration},
    },
    configuration::Configuration,
};

pub fn scale_noise_handler(
    config_path: Option<&Path>,
    threshold: Option<f64>,
    minimum_scaler: Option<f64>,
    maximum_scaler: Option<f64>,
    scale_chroma: bool,
) -> Result<(Configuration, PathBuf)> {
    let (mut configuration, config_path) = load_configuration(config_path)?;

    configure_noise_scaler(
        &mut configuration,
        threshold,
        minimum_scaler,
        maximum_scaler,
        scale_chroma,
    )?;

    configuration.admit_results()?;
    configuration.save(&config_path)?;

    Ok((configuration, config_path))
}

pub fn configure_noise_scaler(
    configuration: &mut Configuration,
    threshold: Option<f64>,
    minimum_scaler: Option<f64>,
    maximum_scaler: Option<f64>,
    scale_chroma: bool,
) -> Result<()> {
    let mut candidate =
        configuration.condor.sequence_config.noise_scaler.clone().unwrap_or_default();
    if let Some(t) = threshold {
        candidate.threshold = t;
    }
    if let Some(min) = minimum_scaler {
        candidate.minimum_scaler = min;
    }
    if let Some(max) = maximum_scaler {
        candidate.maximum_scaler = max;
    }
    if scale_chroma {
        candidate.scale_chroma = true;
    }
    candidate.validate()?;
    configuration.condor.sequence_config.noise_scaler = Some(candidate);

    Ok(())
}

/// `info` and `reset` keys of `sequence_config.noise_scaler`.
#[derive(Debug, Clone, Subcommand)]
pub enum ScaleNoiseKey {
    /// Noise below which a scene is not scaled.
    Threshold,
    /// Smallest ISO scale factor.
    MinimumScaler,
    /// Largest ISO scale factor.
    MaximumScaler,
    /// Whether the chroma ISO is scaled too.
    ScaleChroma,
}

/// Settings `av1an scale-noise set` can change.
#[derive(Debug, Clone, Subcommand)]
pub enum ScaleNoiseSet {
    /// Noise below which a scene's Photon Noise is left untouched.
    Threshold {
        /// The noise level; recommended `0.002`.
        #[arg(value_name = "Noise")]
        value: f64,
    },
    /// Smallest factor Photon Noise ISO may be scaled by.
    MinimumScaler {
        #[arg(value_name = "Factor")]
        value: f64,
    },
    /// Largest factor Photon Noise ISO may be scaled by.
    MaximumScaler {
        #[arg(value_name = "Factor")]
        value: f64,
    },
    /// Whether chroma Photon Noise ISO is scaled too.
    ///
    /// Bare `on` is the default when the value is omitted.
    ScaleChroma {
        #[arg(value_name = "on|off")]
        value: Option<Toggle>,
    },
}

/// The action triple attached to `av1an scale-noise`.
pub type ScaleNoiseAction = ConfigAction<ScaleNoiseKey, ScaleNoiseSet>;

/// The `av1an scale-noise` configuration section.
pub struct ScaleNoiseCrud;

impl ConfigCommand for ScaleNoiseCrud {
    type Key = ScaleNoiseKey;
    type Set = ScaleNoiseSet;

    fn subject() -> &'static str {
        "Noise scaling settings"
    }

    fn render(configuration: &Configuration, key: Option<&Self::Key>) -> Result<Value> {
        let scaler = configuration.condor.sequence_config.noise_scaler.as_ref();
        Ok(match key {
            None => serde_json::to_value(&configuration.condor.sequence_config.noise_scaler)?,
            Some(ScaleNoiseKey::Threshold) => serde_json::to_value(scaler.map(|s| s.threshold))?,
            Some(ScaleNoiseKey::MinimumScaler) => {
                serde_json::to_value(scaler.map(|s| s.minimum_scaler))?
            },
            Some(ScaleNoiseKey::MaximumScaler) => {
                serde_json::to_value(scaler.map(|s| s.maximum_scaler))?
            },
            Some(ScaleNoiseKey::ScaleChroma) => {
                serde_json::to_value(scaler.map(|s| s.scale_chroma))?
            },
        })
    }

    fn apply(configuration: &mut Configuration, set: &Self::Set) -> Result<PropertyChange> {
        let mut scaler =
            configuration.condor.sequence_config.noise_scaler.clone().unwrap_or_default();
        let (property, value) = match set {
            ScaleNoiseSet::Threshold { value } => {
                scaler.threshold = *value;
                ("threshold", serde_json::to_value(value)?)
            },
            ScaleNoiseSet::MinimumScaler { value } => {
                scaler.minimum_scaler = *value;
                ("minimum-scaler", serde_json::to_value(value)?)
            },
            ScaleNoiseSet::MaximumScaler { value } => {
                scaler.maximum_scaler = *value;
                ("maximum-scaler", serde_json::to_value(value)?)
            },
            ScaleNoiseSet::ScaleChroma { value } => {
                let enabled = value.unwrap_or(Toggle::On).enabled();
                scaler.scale_chroma = enabled;
                ("scale-chroma", serde_json::to_value(enabled)?)
            },
        };
        scaler.validate()?;
        configuration.condor.sequence_config.noise_scaler = Some(scaler);
        Ok(PropertyChange {
            property: property.to_owned(),
            value,
        })
    }

    fn restore(
        configuration: &mut Configuration,
        key: Option<&Self::Key>,
    ) -> Result<Option<PropertyChange>> {
        // Resetting a section that was never enabled must not enable it, and
        // resetting a key of one has nothing to restore.
        if configuration.condor.sequence_config.noise_scaler.is_none() {
            warn!("Noise scaling is not configured; nothing to reset.");
            return Ok(None);
        }
        if key.is_none() {
            configuration.condor.sequence_config.noise_scaler = None;
            return Ok(Some(PropertyChange {
                property: "section".to_owned(),
                value: Value::Null,
            }));
        }
        let scaler = configuration
            .condor
            .sequence_config
            .noise_scaler
            .as_ref()
            .expect("noise scaler was just checked to exist");
        let mut scaler = scaler.clone();
        let defaults = NoiseScalerConfig::default();
        let (property, value) = match key {
            None => unreachable!("handled above"),
            Some(ScaleNoiseKey::Threshold) => {
                scaler.threshold = defaults.threshold;
                ("threshold", serde_json::to_value(scaler.threshold)?)
            },
            Some(ScaleNoiseKey::MinimumScaler) => {
                scaler.minimum_scaler = defaults.minimum_scaler;
                (
                    "minimum-scaler",
                    serde_json::to_value(scaler.minimum_scaler)?,
                )
            },
            Some(ScaleNoiseKey::MaximumScaler) => {
                scaler.maximum_scaler = defaults.maximum_scaler;
                (
                    "maximum-scaler",
                    serde_json::to_value(scaler.maximum_scaler)?,
                )
            },
            Some(ScaleNoiseKey::ScaleChroma) => {
                scaler.scale_chroma = defaults.scale_chroma;
                ("scale-chroma", serde_json::to_value(scaler.scale_chroma)?)
            },
        };
        scaler.validate()?;
        configuration.condor.sequence_config.noise_scaler = Some(scaler);
        Ok(Some(PropertyChange {
            property: property.to_owned(),
            value,
        }))
    }
}

/// `av1an scale-noise info|set|reset`.
pub fn config(config_path: Option<&Path>, action: &ScaleNoiseAction) -> Result<()> {
    edit::<ScaleNoiseCrud>(config_path, action)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        commands::handlers::init::init_handler,
        test_helpers::{check_basic_config, default_config, get_test_video},
        utils::hash_path::hash_path,
    };

    #[test]
    fn detect_noise_custom_config() {
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
        expected_config.condor.scenes = test_video.mock_scenes(&expected_config.condor.encoder);
        expected_config.condor.sequence_config.noise_scaler = Some(NoiseScalerConfig {
            threshold: 0.002,
            minimum_scaler: 4.0,
            maximum_scaler: 12.0,
            scale_chroma: false,
        });
        // immutable shadow
        let expected_config = expected_config;

        init_handler(
            Some(&config_path),
            Some(&temp.path().join(hash_path(&input_abs))), /* Simulate default directory to
                                                             * avoid changing CWD */
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
            None,
            false,
        )
        .expect("init_handler should succeed");
        // Mock scenes and save to config file (simulates Scene Detector)
        let (mut config, _) =
            load_configuration(Some(&config_path)).expect("load_config should succeed");
        config.condor.scenes = test_video.mock_scenes(&config.condor.encoder);
        config.save(&config_path).expect("save should succeed");
        let (config, found_config_path) = scale_noise_handler(
            Some(&config_path),
            Some(0.002),
            Some(4.0),
            Some(12.0),
            false,
        )
        .expect("scale_scenes_handler should succeed");

        assert_eq!(
            found_config_path,
            config_path_abs,
            "config path is {}",
            config_path_abs.display()
        );
        check_basic_config(&config, &expected_config);
        assert!(!config.condor.scenes.is_empty(), "scenes is not empty");
    }
}
