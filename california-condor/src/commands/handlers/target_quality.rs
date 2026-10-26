use std::path::{Path, PathBuf};

use andean_condor::{
    core::sequence::target_quality::TargetQuality,
    models::sequence::target_quality::{
        TargetQualityConfig,
        types::{
            DEFAULT_BUTTERAUGLI_TARGET_RANGE,
            DEFAULT_CVVDP_TARGET_RANGE,
            DEFAULT_SSIMULACRA2_TARGET_RANGE,
            DEFAULT_VMAF_TARGET_RANGE,
            DEFAULT_XPSNR_TARGET_RANGE,
            InterpolationMethod,
            ProbeStatistic,
            ProbeStrategy,
            QualityMetric,
            SubsetProbeLength,
            SubsetProbePosition,
        },
    },
    vapoursynth::vapoursynth_filters::VapourSynthFilter,
};
use anyhow::Result;
use clap::{Subcommand, ValueEnum};
use serde_json::Value;
use thiserror::Error;
use tracing::warn;

use crate::{
    commands::{
        DecoderMethod,
        QualityMetric as QualityMetricBase,
        QualityProfile,
        config::{
            ConfigAction,
            ConfigCommand,
            InputOverride,
            PropertyChange,
            StatisticArgs,
            StrategyArgs,
        },
        handlers::{
            apply_input_override,
            config::{edit, ensure_override_requested},
            configure_override_input,
            configure_temp,
            load_configuration,
        },
    },
    configuration::Configuration,
    utils::parameter_parser::EncoderParamsParser,
};

#[allow(clippy::too_many_arguments)]
pub fn target_quality_handler(
    config_path: Option<&Path>,
    temp_path: Option<&Path>,
    input_path: Option<&Path>,
    decoder: Option<&DecoderMethod>,
    filters: Option<&[VapourSynthFilter]>,
    vs_args: Option<&[String]>,
    params: Option<&str>,
    metric: Option<&QualityMetricBase>,
    target: Option<f64>,
    minimum_quantizer: Option<u8>,
    maximum_quantizer: Option<u8>,
    profile: Option<&QualityProfile>,
) -> Result<(Configuration, PathBuf)> {
    let (mut configuration, config_path) = load_configuration(config_path)?;

    configure_temp(&mut configuration, temp_path)?;
    configure_target_quality(
        &mut configuration,
        input_path,
        decoder,
        filters,
        vs_args,
        params,
        metric,
        target,
        minimum_quantizer,
        maximum_quantizer,
        profile,
    )?;

    configuration.save(&config_path)?;

    Ok((configuration, config_path))
}

#[allow(clippy::too_many_arguments)]
pub fn configure_target_quality(
    configuration: &mut Configuration,
    input_path: Option<&Path>,
    decoder: Option<&DecoderMethod>,
    filters: Option<&[VapourSynthFilter]>,
    vs_args: Option<&[String]>,
    params: Option<&str>,
    metric: Option<&QualityMetricBase>,
    target: Option<f64>,
    minimum_quantizer: Option<u8>,
    maximum_quantizer: Option<u8>,
    profile: Option<&QualityProfile>,
) -> Result<()> {
    // Initialize target quality if it doesn't exist yet
    if configuration.condor.sequence_config.target_quality.is_none() && target.is_some() {
        configuration.condor.sequence_config.target_quality = Some(TargetQualityConfig {
            quantizer_range: TargetQuality::default_quantizer_range(
                &configuration.condor.encoder.base(),
            ),
            ..Default::default()
        });
    }

    if configuration.condor.sequence_config.target_quality.is_none() {
        // No target was ever specified, skip configuration
        return Ok(());
    }

    let existing_input = configuration
        .condor
        .sequence_config
        .target_quality
        .as_ref()
        .and_then(|tq| tq.input.clone())
        .unwrap_or_else(|| configuration.condor.input.clone());
    let override_input = configure_override_input(
        configuration,
        &existing_input,
        input_path,
        decoder,
        filters,
        vs_args,
    )?;
    if let (Some(input), Some(target_quality)) = (
        override_input,
        &mut configuration.condor.sequence_config.target_quality,
    ) {
        target_quality.input = Some(input);
    }

    if let (Some(metric), Some(target_quality)) = (
        metric,
        &mut configuration.condor.sequence_config.target_quality,
    ) {
        target_quality.metric = match metric {
            QualityMetricBase::VMAF => QualityMetric::VMAF {
                target_range: DEFAULT_VMAF_TARGET_RANGE,
                resolution:   None,
                scaler:       String::new(),
                // 0 means one libvmaf worker per available core.
                threads:      0,
                model:        None,
                features:     vec![],
            },
            QualityMetricBase::SSIMULACRA2 => QualityMetric::SSIMULACRA2 {
                target_range: DEFAULT_SSIMULACRA2_TARGET_RANGE,
                resolution:   None,
                threads:      None,
                gpu_id:       None,
            },
            QualityMetricBase::BUTTERAUGLI => QualityMetric::BUTTERAUGLI {
                target_range:         DEFAULT_BUTTERAUGLI_TARGET_RANGE,
                resolution:           None,
                threads:              None,
                intensity_multiplier: None,
                norm:                 None,
                gpu_id:               None,
            },
            QualityMetricBase::BUTTERAUGLI3Norm => QualityMetric::BUTTERAUGLI {
                target_range:         DEFAULT_BUTTERAUGLI_TARGET_RANGE,
                resolution:           None,
                threads:              None,
                intensity_multiplier: None,
                norm:                 Some(3),
                gpu_id:               None,
            },
            QualityMetricBase::XPSNR => QualityMetric::XPSNR {
                target_range: DEFAULT_XPSNR_TARGET_RANGE,
                resolution:   None,
            },
            QualityMetricBase::CVVDP => QualityMetric::CVVDP {
                target_range:      DEFAULT_CVVDP_TARGET_RANGE,
                resolution:        None,
                display_model:     None,
                resize_to_display: None,
                disable_temporal:  None,
                gpu_id:            None,
            },
        };
    }
    if let (Some(target), Some(target_quality)) = (
        target,
        &mut configuration.condor.sequence_config.target_quality,
    ) {
        let target_range = match target_quality.metric {
            QualityMetric::BUTTERAUGLI {
                ..
            }
            | QualityMetric::CVVDP {
                ..
            } => (target - 0.1, target + 0.1),
            _ => (target - 1.0, target + 1.0),
        };
        target_quality.metric.target_range_mut().0 = target_range.0;
        target_quality.metric.target_range_mut().1 = target_range.1;
    }
    if let (Some(minimum_quantizer), Some(target_quality)) = (
        minimum_quantizer,
        &mut configuration.condor.sequence_config.target_quality,
    ) {
        target_quality.quantizer_range.0 = minimum_quantizer as u32;
    }
    if let (Some(maximum_quantizer), Some(target_quality)) = (
        maximum_quantizer,
        &mut configuration.condor.sequence_config.target_quality,
    ) {
        target_quality.quantizer_range.1 = maximum_quantizer as u32;
    }
    if let (Some(params), Some(target_quality)) = (
        params,
        &mut configuration.condor.sequence_config.target_quality,
    ) {
        let mut parameters = configuration.condor.encoder.base().default_parameters();
        parameters.extend(EncoderParamsParser::parse_string(params)?);
        target_quality.probing.encoder_options = Some(parameters);
    }
    if let (Some(profile), Some(target_quality)) = (
        profile,
        &mut configuration.condor.sequence_config.target_quality,
    ) {
        match profile {
            QualityProfile::Fast => {
                target_quality.probing.strategy = ProbeStrategy::Subset {
                    position: SubsetProbePosition::Middle,
                    length:   SubsetProbeLength::Frames(11),
                };
                target_quality.probing.statistic = ProbeStatistic::Mean;
            },
            QualityProfile::Standard => {
                target_quality.probing.strategy = ProbeStrategy::Subset {
                    position: SubsetProbePosition::Middle,
                    length:   SubsetProbeLength::Percentage(25.0),
                };
                target_quality.probing.statistic = ProbeStatistic::RootMeanSquare;
            },
            QualityProfile::Slow => {
                target_quality.probing.strategy = ProbeStrategy::Whole;
                if target_quality.metric.is_inverse_metric() {
                    target_quality.probing.statistic = ProbeStatistic::Percentile(90.0);
                } else {
                    target_quality.probing.statistic = ProbeStatistic::Percentile(10.0);
                }
            },
        }
    }

    Ok(())
}

/// The spline interpolators Target Quality offers, as parseable choices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Interpolator {
    Linear,
    Quadratic,
    Natural,
    Pchip,
    Catmull,
    Akima,
    CubicPolynomial,
}

impl From<Interpolator> for InterpolationMethod {
    #[inline]
    fn from(interpolator: Interpolator) -> Self {
        match interpolator {
            Interpolator::Linear => Self::Linear,
            Interpolator::Quadratic => Self::Quadratic,
            Interpolator::Natural => Self::Natural,
            Interpolator::Pchip => Self::Pchip,
            Interpolator::Catmull => Self::Catmull,
            Interpolator::Akima => Self::Akima,
            Interpolator::CubicPolynomial => Self::CubicPolynomial,
        }
    }
}

/// Parses `MIN-MAX` into the quantizer range the search may probe.
fn parse_quantizer_range(value: &str) -> Result<(u32, u32), String> {
    let (minimum, maximum) =
        value.split_once('-').ok_or_else(|| format!("expected MIN-MAX, got {value}"))?;
    let minimum: u32 = minimum
        .trim()
        .parse()
        .map_err(|err| format!("invalid minimum quantizer in {value}: {err}"))?;
    let maximum: u32 = maximum
        .trim()
        .parse()
        .map_err(|err| format!("invalid maximum quantizer in {value}: {err}"))?;
    if minimum > maximum {
        return Err(format!(
            "minimum quantizer {minimum} must not exceed maximum quantizer {maximum}"
        ));
    }
    Ok((minimum, maximum))
}

/// `info` and `reset` keys of `sequence_config.target_quality`.
#[derive(Debug, Clone, Subcommand)]
pub enum TargetQualityKey {
    /// The quality metric measured during the search.
    Metric,
    /// The score the search aims for.
    Target,
    /// How many probes a scene may take.
    MaximumProbes,
    /// The quantizers the search may probe.
    QuantizerRange,
    /// The interpolators used between probe results.
    Interpolators,
    /// The input Target Quality encodes and measures.
    Input,
    /// The input only the metric measures.
    MetricInput,
    /// Which frames of each probe the search samples.
    Strategy,
    /// How the probe scores become one result.
    Statistic,
    /// The encoder parameters the probes encode with.
    Params,
}

/// Settings `condor target-quality set` can change.
#[derive(Debug, Clone, Subcommand)]
pub enum TargetQualitySet {
    /// The quality metric measured during the search.
    ///
    /// Switching the metric resets its target range; use *set target* to aim
    /// for a score afterwards.
    Metric {
        #[arg(value_name = "Metric")]
        metric: QualityMetricBase,
    },
    /// The score the search aims for, stored in the metric's target range.
    Target {
        #[arg(value_name = "Score")]
        value: f64,
    },
    /// How many probes a scene may take before settling.
    MaximumProbes {
        #[arg(value_name = "Probes")]
        value: u8,
    },
    /// The quantizers the search may probe.
    QuantizerRange {
        /// Lowest and highest quantizer, as one `MIN-MAX` value (`18-30`).
        #[arg(value_name = "MIN-MAX", value_parser = parse_quantizer_range)]
        range: (u32, u32),
    },
    /// The interpolators used between probe results.
    Interpolators {
        /// Interpolator used below a probe's value.
        #[arg(value_name = "Interpolator")]
        first:  Interpolator,
        /// Interpolator used above a probe's value.
        #[arg(value_name = "Interpolator")]
        second: Interpolator,
    },
    /// The input Target Quality encodes and measures.
    Input {
        #[command(flatten)]
        args: InputOverride,
    },
    /// The input only the metric measures.
    MetricInput {
        #[command(flatten)]
        args: InputOverride,
    },
    /// Which frames of each probe the search samples.
    ///
    /// The probing strategy of Target Quality; takes its values exactly as
    /// *condor quality-check set strategy*.
    Strategy {
        #[command(flatten)]
        args: StrategyArgs,
    },
    /// How the probe scores become one result.
    ///
    /// The probe statistic of Target Quality; takes its values exactly as
    /// *condor quality-check set statistic*.
    Statistic {
        #[command(flatten)]
        args: StatisticArgs,
    },
    /// The encoder parameters the probes encode with.
    ///
    /// Space-separated `key=value` pairs (`--crf=16 --tune=0`), exactly like
    /// *--params*. Pairs you omit keep their current value; *reset params*
    /// falls back to the encoder defaults.
    Params {
        #[arg(value_name = "Parameters", allow_hyphen_values = true)]
        params: String,
    },
}

/// The action triple attached to `condor target-quality`.
pub type TargetQualityAction = ConfigAction<TargetQualityKey, TargetQualitySet>;

/// Everything that can make a Target Quality setting unusable.
#[derive(Debug, Error)]
pub enum TargetQualityConfigError {
    #[error("target must be a finite score")]
    TargetNotFinite,
    #[error("maximum probes must be positive")]
    MaximumProbesNotPositive,
    #[error("minimum quantizer must not exceed maximum quantizer")]
    QuantizerRangeInverted,
}

/// The `condor target-quality` configuration section.
pub struct TargetQualityCrud;

impl TargetQualityCrud {
    /// The section, materialized so a `set` always has somewhere to write.
    fn section_mut(configuration: &mut Configuration) -> &mut TargetQualityConfig {
        if configuration.condor.sequence_config.target_quality.is_none() {
            configuration.condor.sequence_config.target_quality = Some(TargetQualityConfig {
                quantizer_range: TargetQuality::default_quantizer_range(
                    &configuration.condor.encoder.base(),
                ),
                ..Default::default()
            });
        }
        configuration
            .condor
            .sequence_config
            .target_quality
            .as_mut()
            .expect("target quality was just created")
    }

    /// Rejects Target Quality settings the search could not honour, before
    /// anything is written.
    fn validate(quality: &TargetQualityConfig) -> Result<(), TargetQualityConfigError> {
        if quality.maximum_probes == 0 {
            return Err(TargetQualityConfigError::MaximumProbesNotPositive);
        }
        if quality.quantizer_range.0 > quality.quantizer_range.1 {
            return Err(TargetQualityConfigError::QuantizerRangeInverted);
        }
        Ok(())
    }

    /// Restores the metric's default acceptable range, undoing *set target*.
    pub(crate) fn reset_target_range(metric: &mut QualityMetric) {
        let (minimum, maximum) = default_target_range(metric);
        let range = metric.target_range_mut();
        range.0 = minimum;
        range.1 = maximum;
    }
}

/// The default acceptable score range of `metric`.
fn default_target_range(metric: &QualityMetric) -> (f64, f64) {
    match metric {
        QualityMetric::VMAF {
            ..
        } => DEFAULT_VMAF_TARGET_RANGE,
        QualityMetric::SSIMULACRA2 {
            ..
        } => DEFAULT_SSIMULACRA2_TARGET_RANGE,
        QualityMetric::BUTTERAUGLI {
            ..
        } => DEFAULT_BUTTERAUGLI_TARGET_RANGE,
        QualityMetric::XPSNR {
            ..
        } => DEFAULT_XPSNR_TARGET_RANGE,
        QualityMetric::CVVDP {
            ..
        } => DEFAULT_CVVDP_TARGET_RANGE,
    }
}

impl ConfigCommand for TargetQualityCrud {
    type Key = TargetQualityKey;
    type Set = TargetQualitySet;

    fn subject() -> &'static str {
        "Target Quality settings"
    }

    fn render(configuration: &Configuration, key: Option<&Self::Key>) -> Result<Value> {
        let quality = configuration.condor.sequence_config.target_quality.as_ref();
        Ok(match key {
            None => serde_json::to_value(&configuration.condor.sequence_config.target_quality)?,
            Some(TargetQualityKey::Metric) => {
                serde_json::to_value(quality.map(|quality| &quality.metric))?
            },
            Some(TargetQualityKey::Target) => {
                serde_json::to_value(quality.map(|quality| quality.metric.target_range()))?
            },
            Some(TargetQualityKey::MaximumProbes) => {
                serde_json::to_value(quality.map(|quality| quality.maximum_probes))?
            },
            Some(TargetQualityKey::QuantizerRange) => {
                serde_json::to_value(quality.map(|quality| quality.quantizer_range))?
            },
            Some(TargetQualityKey::Interpolators) => {
                serde_json::to_value(quality.map(|quality| quality.interpolators))?
            },
            Some(TargetQualityKey::Input) => {
                serde_json::to_value(quality.map(|quality| quality.input.as_ref()))?
            },
            Some(TargetQualityKey::MetricInput) => {
                serde_json::to_value(quality.map(|quality| quality.metric_input.as_ref()))?
            },
            Some(TargetQualityKey::Strategy) => {
                serde_json::to_value(quality.map(|quality| &quality.probing.strategy))?
            },
            Some(TargetQualityKey::Statistic) => {
                serde_json::to_value(quality.map(|quality| &quality.probing.statistic))?
            },
            Some(TargetQualityKey::Params) => {
                serde_json::to_value(quality.map(|quality| &quality.probing.encoder_options))?
            },
        })
    }

    fn apply(configuration: &mut Configuration, set: &Self::Set) -> Result<PropertyChange> {
        Self::section_mut(configuration);
        match set {
            TargetQualitySet::Metric {
                metric,
            } => {
                configure_target_quality(
                    configuration,
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some(metric),
                    None,
                    None,
                    None,
                    None,
                )?;
                Self::validate(Self::section_mut(configuration))?;
                PropertyChange::new("metric", &Self::section_mut(configuration).metric)
            },
            TargetQualitySet::Target {
                value,
            } => {
                if !value.is_finite() {
                    return Err(TargetQualityConfigError::TargetNotFinite.into());
                }
                configure_target_quality(
                    configuration,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some(*value),
                    None,
                    None,
                    None,
                )?;
                Self::validate(Self::section_mut(configuration))?;
                PropertyChange::new(
                    "target",
                    Self::section_mut(configuration).metric.target_range(),
                )
            },
            TargetQualitySet::MaximumProbes {
                value,
            } => {
                Self::section_mut(configuration).maximum_probes = *value;
                Self::validate(Self::section_mut(configuration))?;
                PropertyChange::new(
                    "maximum-probes",
                    Self::section_mut(configuration).maximum_probes,
                )
            },
            TargetQualitySet::QuantizerRange {
                range,
            } => {
                Self::section_mut(configuration).quantizer_range = *range;
                Self::validate(Self::section_mut(configuration))?;
                PropertyChange::new(
                    "quantizer-range",
                    Self::section_mut(configuration).quantizer_range,
                )
            },
            TargetQualitySet::Interpolators {
                first,
                second,
            } => {
                let quality = Self::section_mut(configuration);
                quality.interpolators = ((*first).into(), (*second).into());
                PropertyChange::new("interpolators", quality.interpolators)
            },
            TargetQualitySet::Input {
                args,
            } => {
                ensure_override_requested(args)?;
                let existing = configuration
                    .condor
                    .sequence_config
                    .target_quality
                    .as_ref()
                    .and_then(|quality| quality.input.clone())
                    .unwrap_or_else(|| configuration.condor.input.clone());
                Self::section_mut(configuration).input =
                    Some(apply_input_override(configuration, &existing, args)?);
                Self::validate(Self::section_mut(configuration))?;
                PropertyChange::new("input", &Self::section_mut(configuration).input)
            },
            TargetQualitySet::MetricInput {
                args,
            } => {
                ensure_override_requested(args)?;
                let existing = configuration
                    .condor
                    .sequence_config
                    .target_quality
                    .as_ref()
                    .and_then(|quality| quality.metric_input.clone())
                    .unwrap_or_else(|| configuration.condor.input.clone());
                Self::section_mut(configuration).metric_input =
                    Some(apply_input_override(configuration, &existing, args)?);
                PropertyChange::new(
                    "metric-input",
                    &Self::section_mut(configuration).metric_input,
                )
            },
            TargetQualitySet::Strategy {
                args,
            } => {
                let strategy = args.build()?;
                Self::section_mut(configuration).probing.strategy = strategy;
                PropertyChange::new(
                    "strategy",
                    &Self::section_mut(configuration).probing.strategy,
                )
            },
            TargetQualitySet::Statistic {
                args,
            } => {
                let statistic = args.build()?;
                Self::section_mut(configuration).probing.statistic = statistic;
                PropertyChange::new(
                    "statistic",
                    Self::section_mut(configuration).probing.statistic,
                )
            },
            TargetQualitySet::Params {
                params,
            } => {
                // Keep the options the user does not restate: the current map
                // (or the encoder defaults) first, then the pairs from the
                // command line.
                let mut parameters = configuration.condor.encoder.base().default_parameters();
                if let Some(existing) = configuration
                    .condor
                    .sequence_config
                    .target_quality
                    .as_ref()
                    .and_then(|quality| quality.probing.encoder_options.clone())
                {
                    parameters.extend(existing);
                }
                parameters.extend(EncoderParamsParser::parse_string(params)?);
                Self::section_mut(configuration).probing.encoder_options = Some(parameters);
                PropertyChange::new(
                    "params",
                    &Self::section_mut(configuration).probing.encoder_options,
                )
            },
        }
    }

    fn restore(
        configuration: &mut Configuration,
        key: Option<&Self::Key>,
    ) -> Result<Option<PropertyChange>> {
        if configuration.condor.sequence_config.target_quality.is_none() {
            warn!("Target Quality is not configured; nothing to reset.");
            return Ok(None);
        }
        if key.is_none() {
            configuration.condor.sequence_config.target_quality = None;
            return Ok(Some(PropertyChange {
                property: "section".to_owned(),
                value:    Value::Null,
            }));
        }
        let quantizer_range =
            TargetQuality::default_quantizer_range(&configuration.condor.encoder.base());
        let quality = Self::section_mut(configuration);
        let defaults = TargetQualityConfig::default();
        let (property, value) = match key {
            None => unreachable!("handled above"),
            Some(TargetQualityKey::Metric) => {
                quality.metric = defaults.metric;
                ("metric", serde_json::to_value(&quality.metric)?)
            },
            Some(TargetQualityKey::Target) => {
                Self::reset_target_range(&mut quality.metric);
                (
                    "target",
                    serde_json::to_value(quality.metric.target_range())?,
                )
            },
            Some(TargetQualityKey::MaximumProbes) => {
                quality.maximum_probes = defaults.maximum_probes;
                (
                    "maximum-probes",
                    serde_json::to_value(quality.maximum_probes)?,
                )
            },
            Some(TargetQualityKey::QuantizerRange) => {
                quality.quantizer_range = quantizer_range;
                (
                    "quantizer-range",
                    serde_json::to_value(quality.quantizer_range)?,
                )
            },
            Some(TargetQualityKey::Interpolators) => {
                quality.interpolators = defaults.interpolators;
                (
                    "interpolators",
                    serde_json::to_value(quality.interpolators)?,
                )
            },
            Some(TargetQualityKey::Input) => {
                quality.input = None;
                ("input", serde_json::to_value(&quality.input)?)
            },
            Some(TargetQualityKey::MetricInput) => {
                quality.metric_input = None;
                ("metric-input", serde_json::to_value(&quality.metric_input)?)
            },
            Some(TargetQualityKey::Strategy) => {
                quality.probing.strategy = ProbeStrategy::default();
                ("strategy", serde_json::to_value(&quality.probing.strategy)?)
            },
            Some(TargetQualityKey::Statistic) => {
                quality.probing.statistic = ProbeStatistic::default();
                (
                    "statistic",
                    serde_json::to_value(quality.probing.statistic)?,
                )
            },
            Some(TargetQualityKey::Params) => {
                quality.probing.encoder_options = None;
                (
                    "params",
                    serde_json::to_value(&quality.probing.encoder_options)?,
                )
            },
        };
        Ok(Some(PropertyChange {
            property: property.to_owned(),
            value,
        }))
    }
}

/// `condor target-quality info|set|reset`.
pub fn config(config_path: Option<&Path>, action: &TargetQualityAction) -> Result<()> {
    edit::<TargetQualityCrud>(config_path, action)
}

#[cfg(test)]
mod tests {
    use andean_condor::models::{
        encoder::{EncoderBase, cli_parameter::CLIParameter},
        input::{Input, VapourSynthImportMethod},
        sequence::target_quality::types::{InterpolationMethod, TargetQualityProbing},
    };

    use super::*;
    use crate::{
        commands::handlers::init::init_handler,
        test_helpers::{check_basic_config, default_config, get_test_video},
        utils::hash_path::hash_path,
    };

    #[test]
    fn target_quality_default_config() {
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
        let (config, found_config_path) = target_quality_handler(
            Some(&config_path),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("target_quality_handler should succeed");

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
    fn target_quality_custom_config() {
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
        let custom_filters = vec![VapourSynthFilter::Crop {
            top:    Some(140),
            bottom: Some(140),
            left:   None,
            right:  None,
        }];
        let custom_vs_args = vec!["method=target quality".to_string()];

        let mut expected_config = default_config(&test_video, &output, &temp_abs);
        let mut tq_encoder_parameters = EncoderBase::SVTAV1.default_parameters();
        tq_encoder_parameters.extend(CLIParameter::new_numbers("--", " ", &[
            ("preset", 6.0),
            ("tune", 3.0),
        ]));
        expected_config.condor.sequence_config.target_quality = Some(TargetQualityConfig {
            metric:          QualityMetric::BUTTERAUGLI {
                target_range:         (1.4, 1.6),
                resolution:           None,
                threads:              None,
                intensity_multiplier: None,
                norm:                 None,
                gpu_id:               None,
            },
            maximum_probes:  4,
            quantizer_range: (8, 40),
            interpolators:   (InterpolationMethod::Natural, InterpolationMethod::Pchip),
            input:           Some(Input::VapourSynth {
                path:          input_abs.clone(),
                import_method: VapourSynthImportMethod::LSMASHWorks {
                    index: None
                },
                cache_path:    None,
                filters:       custom_filters.clone(),
            }),
            metric_input:    None,
            probing:         TargetQualityProbing {
                encoder_options: Some(tq_encoder_parameters),
                statistic:       ProbeStatistic::Mean,
                strategy:        ProbeStrategy::Subset {
                    position: SubsetProbePosition::Middle,
                    length:   SubsetProbeLength::Frames(11),
                },
            },
        });
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
        let (config, found_config_path) = target_quality_handler(
            Some(&config_path),
            Some(&temp.path().join(hash_path(&input_abs))),
            Some(&test_video.path),
            Some(&DecoderMethod::LSMASHWorks),
            Some(&custom_filters),
            Some(&custom_vs_args),
            Some("--preset 6 --tune 3"),
            Some(&QualityMetricBase::BUTTERAUGLI),
            Some(1.5),
            Some(8),
            Some(40),
            Some(&QualityProfile::Fast),
        )
        .expect("target_quality_handler should succeed");

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
