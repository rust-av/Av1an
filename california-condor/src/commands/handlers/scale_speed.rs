use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use clap::Subcommand;
use serde_json::Value;
use thiserror::Error;
use tracing::error;

use crate::{
    commands::{
        config::{ConfigAction, ConfigCommand, PropertyChange},
        handlers::{config::edit, load_configuration},
    },
    configuration::Configuration,
};

pub fn scale_speed_handler(
    config_path: Option<&Path>,
    quantizers: Option<&[i8]>,
    speeds: Option<&[i8]>,
) -> Result<(Configuration, PathBuf)> {
    let (mut configuration, config_path) = load_configuration(config_path)?;

    configure_scale_speed(&mut configuration, quantizers, speeds)?;

    configuration.save(&config_path)?;

    Ok((configuration, config_path))
}

pub fn configure_scale_speed(
    configuration: &mut Configuration,
    quantizers: Option<&[i8]>,
    speeds: Option<&[i8]>,
) -> Result<()> {
    let (existing_quantizers, existing_speeds): (Vec<i8>, Vec<i8>) = configuration
        .condor
        .sequence_config
        .speed_scaler
        .speed_quantizers
        .iter()
        .map(|(s, q)| (*q as i8, *s))
        .unzip();
    let quantizers = quantizers.unwrap_or(existing_quantizers.as_slice());
    let speeds = speeds.unwrap_or(existing_speeds.as_slice());
    if quantizers.len() != speeds.len() {
        let err = SpeedScalerError::MismatchedPairs {
            quantizers: quantizers.len(),
            speeds:     speeds.len(),
        };
        error!("{}", err);
        bail!(err);
    }

    // Ensure we have at least 2 pairs or none (for no scaling)
    if !quantizers.is_empty() && quantizers.len() < 2 {
        let err = SpeedScalerError::MinimumPairsRequired;
        error!("{}", err);
        bail!(err);
    }

    let speed_quantizers: Vec<(i8, f64)> =
        speeds.iter().zip(quantizers).map(|(s, q)| (*s, *q as f64)).collect();

    configuration.condor.sequence_config.speed_scaler.speed_quantizers = speed_quantizers;

    Ok(())
}

#[derive(Debug, Error)]
pub enum SpeedScalerError {
    #[error(
        "Mismatched speed-quantizer pairs: got {quantizers} quantizer(s) and {speeds} speed(s). \
         Each --quantizer must have a matching --speed value."
    )]
    MismatchedPairs {
        quantizers: usize,
        speeds:     usize,
    },
    #[error("At least 2 speed-quantizer pairs are required.")]
    MinimumPairsRequired,
}

/// Parses `QUANTIZER=SPEED` into one speed–quantizer pair, both as the
/// integers the run flags accept.
fn parse_point(value: &str) -> Result<(i8, i8), String> {
    let (quantizer, speed) = value
        .split_once('=')
        .ok_or_else(|| format!("expected QUANTIZER=SPEED, got {value}"))?;
    let quantizer: i8 = quantizer
        .trim()
        .parse()
        .map_err(|err| format!("invalid quantizer in {value}: {err}"))?;
    let speed: i8 =
        speed.trim().parse().map_err(|err| format!("invalid speed in {value}: {err}"))?;
    Ok((quantizer, speed))
}

/// `info` and `reset` keys of `sequence_config.speed_scaler`.
#[derive(Debug, Clone, Subcommand)]
pub enum ScaleSpeedKey {
    /// The speed–quantizer pairs Convex Hull interpolates between.
    #[command(alias = "speed-scalers")]
    SpeedQuantizers,
}

/// Settings `condor scale-speed set` can change.
#[derive(Debug, Clone, Subcommand)]
pub enum ScaleSpeedSet {
    /// The speed–quantizer pairs Convex Hull interpolates between.
    ///
    /// Give each pair as `QUANTIZER=SPEED`; at least two are required. Reset
    /// the property to stop scaling entirely.
    #[command(alias = "speed-scalers")]
    SpeedQuantizers {
        /// Two or more pairs, each written `QUANTIZER=SPEED`.
        #[arg(value_name = "QUANTIZER=SPEED", value_parser = parse_point, num_args = 2..)]
        points: Vec<(i8, i8)>,
    },
}

/// The action triple attached to `condor scale-speed`.
pub type ScaleSpeedAction = ConfigAction<ScaleSpeedKey, ScaleSpeedSet>;

/// Everything that can make a Speed Scaler setting unusable.
#[derive(Debug, Error)]
pub enum ScaleSpeedConfigError {
    #[error("each quantizer may appear only once")]
    DuplicateQuantizer,
}

/// The `condor scale-speed` configuration section.
pub struct ScaleSpeedCrud;

impl ConfigCommand for ScaleSpeedCrud {
    type Key = ScaleSpeedKey;
    type Set = ScaleSpeedSet;

    fn subject() -> &'static str {
        "Speed scaling settings"
    }

    fn render(configuration: &Configuration, key: Option<&Self::Key>) -> Result<Value> {
        let scaler = &configuration.condor.sequence_config.speed_scaler;
        Ok(match key {
            None => serde_json::to_value(scaler)?,
            Some(ScaleSpeedKey::SpeedQuantizers) => serde_json::to_value(&scaler.speed_quantizers)?,
        })
    }

    fn apply(configuration: &mut Configuration, set: &Self::Set) -> Result<PropertyChange> {
        match set {
            ScaleSpeedSet::SpeedQuantizers {
                points,
            } => {
                let mut points = points.clone();
                points.sort_by_key(|(quantizer, _)| *quantizer);
                if !points.windows(2).all(|pair| pair[0].0 < pair[1].0) {
                    return Err(ScaleSpeedConfigError::DuplicateQuantizer.into());
                }
                // The configuration stores speed first, quantizer second.
                let speed_quantizers: Vec<(i8, f64)> = points
                    .iter()
                    .map(|(quantizer, speed)| (*speed, f64::from(*quantizer)))
                    .collect();
                configuration.condor.sequence_config.speed_scaler.speed_quantizers =
                    speed_quantizers;
                PropertyChange::new(
                    "speed-quantizers",
                    &configuration.condor.sequence_config.speed_scaler.speed_quantizers,
                )
            },
        }
    }

    fn restore(
        configuration: &mut Configuration,
        key: Option<&Self::Key>,
    ) -> Result<Option<PropertyChange>> {
        let scaler = &mut configuration.condor.sequence_config.speed_scaler;
        let (property, value) = match key {
            None | Some(ScaleSpeedKey::SpeedQuantizers) => {
                scaler.speed_quantizers = Vec::new();
                (
                    "speed-quantizers",
                    serde_json::to_value(&scaler.speed_quantizers)?,
                )
            },
        };
        Ok(Some(PropertyChange {
            property: property.to_owned(),
            value,
        }))
    }
}

/// `condor scale-speed info|set|reset`.
pub fn config(config_path: Option<&Path>, action: &ScaleSpeedAction) -> Result<()> {
    edit::<ScaleSpeedCrud>(config_path, action)
}

#[cfg(test)]
mod tests {
    use std::assert_matches;

    use andean_condor::models::sequence::speed_scaler::SpeedScalerConfig;

    use super::*;
    use crate::{
        commands::handlers::{init::init_handler, scale_speed::SpeedScalerError},
        test_helpers::{check_basic_config, default_config, get_test_video},
        utils::hash_path::hash_path,
    };

    #[test]
    fn scale_speed_default_config() {
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
        let (config, found_config_path) = scale_speed_handler(Some(&config_path), None, None)
            .expect("scale_speed_handler should succeed");

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
    fn scale_speed_custom_config() {
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
        expected_config.condor.sequence_config.speed_scaler = SpeedScalerConfig {
            speed_quantizers: vec![(8, 10.0), (5, 25.0), (3, 35.0)],
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
        let (config, found_config_path) = scale_speed_handler(
            Some(&config_path),
            Some(&[10_i8, 25, 35]),
            Some(&[8_i8, 5, 3]),
        )
        .expect("scale_speed_handler should succeed");

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
    fn scale_speed_mismatched_lengths() {
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
        let result =
            scale_speed_handler(Some(&config_path), Some(&[8_i8, 5, 3]), Some(&[10_i8, 25]));

        let error = result
            .expect_err("scale_speed_handler should fail")
            .downcast::<SpeedScalerError>()
            .expect("scale_speed_handler error should be SpeedScalerError");
        assert_matches!(error, SpeedScalerError::MismatchedPairs {
            quantizers: 3,
            speeds:     2,
        });
        // Ensure config was not modified
        let (config, found_config_path) =
            load_configuration(Some(&config_path)).expect("load_configuration should succeed");
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
    fn scale_speed_insufficient_pairs() {
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
        let result = scale_speed_handler(Some(&config_path), Some(&[8_i8]), Some(&[10_i8]));

        let error = result
            .expect_err("scale_speed_handler should fail")
            .downcast::<SpeedScalerError>()
            .expect("scale_speed_handler error should be SpeedScalerError");
        assert_matches!(error, SpeedScalerError::MinimumPairsRequired);
        // Ensure config was not modified
        let (config, found_config_path) =
            load_configuration(Some(&config_path)).expect("load_configuration should succeed");
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
