use std::path::{Path, PathBuf};

use andean_condor::models::sequence::speed_scaler::SpeedScalerConfig;
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

    configuration.admit_results()?;
    configuration.save(&config_path)?;

    Ok((configuration, config_path))
}

pub fn configure_scale_speed(
    configuration: &mut Configuration,
    quantizers: Option<&[i8]>,
    speeds: Option<&[i8]>,
) -> Result<()> {
    let (existing_quantizers, existing_speeds): (Vec<f64>, Vec<i8>) = configuration
        .condor
        .sequence_config
        .speed_scaler
        .speed_quantizers
        .iter()
        .map(|(s, q)| (*q, *s))
        .unzip();
    let quantizers =
        quantizers.map(|values| values.iter().map(|q| f64::from(*q)).collect::<Vec<_>>());
    let quantizers = quantizers.as_deref().unwrap_or(existing_quantizers.as_slice());
    let speeds = speeds.unwrap_or(existing_speeds.as_slice());
    if quantizers.len() != speeds.len() {
        let err = SpeedScalerError::MismatchedPairs {
            quantizers: quantizers.len(),
            speeds: speeds.len(),
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

    let candidate = SpeedScalerConfig {
        speed_quantizers: speeds.iter().zip(quantizers).map(|(s, q)| (*s, *q)).collect(),
    };
    candidate.validate(&configuration.condor.encoder)?;
    for scene in &configuration.condor.scenes {
        candidate.validate(&scene.encoder)?;
    }
    configuration.condor.sequence_config.speed_scaler = candidate;

    Ok(())
}

#[derive(Debug, Error)]
pub enum SpeedScalerError {
    #[error(
        "Mismatched speed-quantizer pairs: got {quantizers} quantizer(s) and {speeds} speed(s). \
         Each --quantizer must have a matching --speed value."
    )]
    MismatchedPairs { quantizers: usize, speeds: usize },
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

/// Settings `av1an scale-speed set` can change.
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

/// The action triple attached to `av1an scale-speed`.
pub type ScaleSpeedAction = ConfigAction<ScaleSpeedKey, ScaleSpeedSet>;

/// The `av1an scale-speed` configuration section.
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
            ScaleSpeedSet::SpeedQuantizers { points } => {
                let mut points = points.clone();
                points.sort_by_key(|(quantizer, _)| *quantizer);
                let (quantizers, speeds): (Vec<_>, Vec<_>) = points.into_iter().unzip();
                configure_scale_speed(configuration, Some(&quantizers), Some(&speeds))?;
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

/// `av1an scale-speed info|set|reset`.
pub fn config(config_path: Option<&Path>, action: &ScaleSpeedAction) -> Result<()> {
    edit::<ScaleSpeedCrud>(config_path, action)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::default_config;

    fn loaded_knots_configuration() -> Configuration {
        let temp = std::env::temp_dir().join("condor-speed-knots-model-only");
        let video = crate::test_helpers::TestVideo {
            path: temp.join("must-not-open.mkv"),
            width: 2,
            height: 2,
            frames: 2,
            fps_rational: (24, 1),
            format: andean_condor::ffmpeg::FFPixelFormat::YUV420P,
            scenes: vec![(0, 2)],
        };
        let mut configuration =
            default_config(&video, &temp.join("output.mkv"), &temp.join("scratch"));
        configuration.condor.encoder.set_executable(Some(temp.join("must-not-run")));
        configuration.condor.scenes = video.mock_scenes(&configuration.condor.encoder);
        configuration.condor.sequence_config.speed_scaler.speed_quantizers =
            vec![(4, 20.25), (8, 40.75)];
        configuration
    }

    fn check_loaded_knots(repair: bool) {
        let mut failures = vec![];
        for q in [20.25, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut config = loaded_knots_configuration();
            config.condor.sequence_config.speed_scaler.speed_quantizers[1].1 = q;
            let before = serde_json::to_vec(&config).expect("snapshot");
            let result = configure_scale_speed(&mut config, None, None);
            if result.is_ok() || serde_json::to_vec(&config).expect("config") != before {
                failures.push(format!("q={q}: {result:?}"));
            }
            if !repair {
                continue;
            }
            configure_scale_speed(&mut config, Some(&[20, 40]), Some(&[4, 8]))
                .expect("complete replacement repairs bad loaded knots");
            assert_eq!(
                config.condor.sequence_config.speed_scaler.speed_quantizers,
                vec![(4, 20.0), (8, 40.0)]
            );
        }
        assert!(
            failures.is_empty(),
            "loaded knots accepted or mutated: {failures:?}"
        );
    }

    #[test]
    fn duplicate_or_nonfinite_loaded_speed_knots_refuse_without_mutation() {
        check_loaded_knots(false);
    }

    #[test]
    fn complete_speed_knot_replacement_repairs_invalid_loaded_values() {
        check_loaded_knots(true);
    }
}
