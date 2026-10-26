use std::path::{Path, PathBuf};

use andean_condor::{
    models::sequence::{
        quality_check::QualityCheckConfig,
        target_quality::types::{
            DEFAULT_BUTTERAUGLI_TARGET_RANGE,
            DEFAULT_CVVDP_TARGET_RANGE,
            DEFAULT_SSIMULACRA2_TARGET_RANGE,
            DEFAULT_VMAF_TARGET_RANGE,
            DEFAULT_XPSNR_TARGET_RANGE,
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
use clap::Subcommand;
use serde_json::Value;
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
};

#[allow(clippy::too_many_arguments)]
pub fn quality_check_handler(
    config_path: Option<&Path>,
    temp_path: Option<&Path>,
    input_path: Option<&Path>,
    decoder: Option<&DecoderMethod>,
    filters: Option<&[VapourSynthFilter]>,
    vs_args: Option<&[String]>,
    metric: Option<&QualityMetricBase>,
    profile: Option<&QualityProfile>,
) -> Result<(Configuration, PathBuf)> {
    let (mut configuration, config_path) = load_configuration(config_path)?;

    configure_temp(&mut configuration, temp_path)?;
    configure_quality_check(
        &mut configuration,
        input_path,
        decoder,
        filters,
        vs_args,
        metric,
        profile,
    )?;

    configuration.save(&config_path)?;

    Ok((configuration, config_path))
}

#[allow(clippy::too_many_arguments)]
pub fn configure_quality_check(
    configuration: &mut Configuration,
    input_path: Option<&Path>,
    decoder: Option<&DecoderMethod>,
    filters: Option<&[VapourSynthFilter]>,
    vs_args: Option<&[String]>,
    metric: Option<&QualityMetricBase>,
    profile: Option<&QualityProfile>,
) -> Result<()> {
    // Initialize quality check if it doesn't exist yet
    if configuration.condor.sequence_config.quality_check.is_none() {
        configuration.condor.sequence_config.quality_check = Some(Default::default());
    }

    let existing_input = configuration
        .condor
        .sequence_config
        .quality_check
        .as_ref()
        .and_then(|qc| qc.input.clone())
        .unwrap_or_else(|| configuration.condor.input.clone());
    let override_input = configure_override_input(
        configuration,
        &existing_input,
        input_path,
        decoder,
        filters,
        vs_args,
    )?;
    if let (Some(input), Some(quality_check)) = (
        override_input,
        &mut configuration.condor.sequence_config.quality_check,
    ) {
        quality_check.input = Some(input);
    }

    if let (Some(metric), Some(quality_check)) = (
        metric,
        &mut configuration.condor.sequence_config.quality_check,
    ) {
        quality_check.metric = match metric {
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
    if let (Some(profile), Some(quality_check)) = (
        profile,
        &mut configuration.condor.sequence_config.quality_check,
    ) {
        match profile {
            QualityProfile::Fast => {
                quality_check.strategy = ProbeStrategy::Subset {
                    position: SubsetProbePosition::Middle,
                    length:   SubsetProbeLength::Frames(11),
                };
                quality_check.statistic = ProbeStatistic::Mean;
            },
            QualityProfile::Standard => {
                quality_check.strategy = ProbeStrategy::Subset {
                    position: SubsetProbePosition::Middle,
                    length:   SubsetProbeLength::Percentage(25.0),
                };
                quality_check.statistic = ProbeStatistic::RootMeanSquare;
            },
            QualityProfile::Slow => {
                quality_check.strategy = ProbeStrategy::Whole;
                if quality_check.metric.is_inverse_metric() {
                    quality_check.statistic = ProbeStatistic::Percentile(90.0);
                } else {
                    quality_check.statistic = ProbeStatistic::Percentile(10.0);
                }
            },
        }
    }

    Ok(())
}

/// `info` and `reset` keys of `sequence_config.quality_check`.
#[derive(Debug, Clone, Subcommand)]
pub enum QualityCheckKey {
    /// The quality metric measured per scene.
    Metric,
    /// Which frames of each scene are probed.
    Strategy,
    /// How the probe scores become one result.
    Statistic,
    /// The input Quality Check measures.
    Input,
}

/// Settings `condor quality-check set` can change.
#[derive(Debug, Clone, Subcommand)]
pub enum QualityCheckSet {
    /// The quality metric measured per scene.
    Metric {
        #[arg(value_name = "Metric")]
        metric: QualityMetricBase,
    },
    /// Which frames of each scene are probed.
    Strategy {
        #[command(flatten)]
        args: StrategyArgs,
    },
    /// How the probe scores become one result.
    Statistic {
        #[command(flatten)]
        args: StatisticArgs,
    },
    /// The input Quality Check measures.
    Input {
        #[command(flatten)]
        args: InputOverride,
    },
}

/// The action triple attached to `condor quality-check`.
pub type QualityCheckAction = ConfigAction<QualityCheckKey, QualityCheckSet>;

/// The `condor quality-check` configuration section.
pub struct QualityCheckCrud;

impl QualityCheckCrud {
    /// The section, materialized so a `set` always has somewhere to write.
    fn section_mut(configuration: &mut Configuration) -> &mut QualityCheckConfig {
        configuration
            .condor
            .sequence_config
            .quality_check
            .get_or_insert_with(QualityCheckConfig::default)
    }
}

impl ConfigCommand for QualityCheckCrud {
    type Key = QualityCheckKey;
    type Set = QualityCheckSet;

    fn subject() -> &'static str {
        "Quality check settings"
    }

    fn render(configuration: &Configuration, key: Option<&Self::Key>) -> Result<Value> {
        let check = configuration.condor.sequence_config.quality_check.as_ref();
        Ok(match key {
            None => serde_json::to_value(&configuration.condor.sequence_config.quality_check)?,
            Some(QualityCheckKey::Metric) => serde_json::to_value(check.map(|c| &c.metric))?,
            Some(QualityCheckKey::Strategy) => serde_json::to_value(check.map(|c| &c.strategy))?,
            Some(QualityCheckKey::Statistic) => serde_json::to_value(check.map(|c| &c.statistic))?,
            Some(QualityCheckKey::Input) => serde_json::to_value(check.map(|c| c.input.as_ref()))?,
        })
    }

    fn apply(configuration: &mut Configuration, set: &Self::Set) -> Result<PropertyChange> {
        Self::section_mut(configuration);
        match set {
            QualityCheckSet::Metric {
                metric,
            } => {
                configure_quality_check(configuration, None, None, None, None, Some(metric), None)?;
                PropertyChange::new("metric", &Self::section_mut(configuration).metric)
            },
            QualityCheckSet::Strategy {
                args,
            } => {
                let strategy = args.build()?;
                Self::section_mut(configuration).strategy = strategy;
                PropertyChange::new("strategy", &Self::section_mut(configuration).strategy)
            },
            QualityCheckSet::Statistic {
                args,
            } => {
                let statistic = args.build()?;
                Self::section_mut(configuration).statistic = statistic;
                PropertyChange::new("statistic", Self::section_mut(configuration).statistic)
            },
            QualityCheckSet::Input {
                args,
            } => {
                ensure_override_requested(args)?;
                let existing = configuration
                    .condor
                    .sequence_config
                    .quality_check
                    .as_ref()
                    .and_then(|check| check.input.clone())
                    .unwrap_or_else(|| configuration.condor.input.clone());
                Self::section_mut(configuration).input =
                    Some(apply_input_override(configuration, &existing, args)?);
                PropertyChange::new("input", &Self::section_mut(configuration).input)
            },
        }
    }

    fn restore(
        configuration: &mut Configuration,
        key: Option<&Self::Key>,
    ) -> Result<Option<PropertyChange>> {
        if configuration.condor.sequence_config.quality_check.is_none() {
            warn!("Quality check is not configured; nothing to reset.");
            return Ok(None);
        }
        if key.is_none() {
            configuration.condor.sequence_config.quality_check = None;
            return Ok(Some(PropertyChange {
                property: "section".to_owned(),
                value:    Value::Null,
            }));
        }
        let check = Self::section_mut(configuration);
        let defaults = QualityCheckConfig::default();
        let (property, value) = match key {
            None => unreachable!("handled above"),
            Some(QualityCheckKey::Metric) => {
                check.metric = defaults.metric;
                ("metric", serde_json::to_value(&check.metric)?)
            },
            Some(QualityCheckKey::Strategy) => {
                check.strategy = defaults.strategy;
                ("strategy", serde_json::to_value(&check.strategy)?)
            },
            Some(QualityCheckKey::Statistic) => {
                check.statistic = defaults.statistic;
                ("statistic", serde_json::to_value(check.statistic)?)
            },
            Some(QualityCheckKey::Input) => {
                check.input = None;
                ("input", serde_json::to_value(&check.input)?)
            },
        };
        Ok(Some(PropertyChange {
            property: property.to_owned(),
            value,
        }))
    }
}

/// `condor quality-check info|set|reset`.
pub fn config(config_path: Option<&Path>, action: &QualityCheckAction) -> Result<()> {
    edit::<QualityCheckCrud>(config_path, action)
}

#[cfg(test)]
mod tests {
    use andean_condor::models::{
        input::{Input, VapourSynthImportMethod},
        sequence::quality_check::QualityCheckConfig,
    };

    use super::*;
    use crate::{
        commands::handlers::init::init_handler,
        test_helpers::{check_basic_config, default_config, get_test_video},
        utils::hash_path::hash_path,
    };

    #[test]
    fn quality_check_default_config() {
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
        let (config, found_config_path) =
            quality_check_handler(Some(&config_path), None, None, None, None, None, None, None)
                .expect("quality_check_handler should succeed");

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
    fn quality_check_custom_config() {
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
        let custom_vs_args = vec!["method=quality check".to_string()];

        let mut expected_config = default_config(&test_video, &output, &temp_abs);
        expected_config.condor.sequence_config.quality_check = Some(QualityCheckConfig {
            input:     Some(Input::VapourSynth {
                path:          input_abs.clone(),
                import_method: VapourSynthImportMethod::LSMASHWorks {
                    index: None
                },
                cache_path:    None,
                filters:       custom_filters.clone(),
            }),
            metric:    QualityMetric::BUTTERAUGLI {
                target_range:         DEFAULT_BUTTERAUGLI_TARGET_RANGE,
                resolution:           None,
                threads:              None,
                intensity_multiplier: None,
                norm:                 None,
                gpu_id:               None,
            },
            statistic: ProbeStatistic::Mean,
            strategy:  ProbeStrategy::Subset {
                position: SubsetProbePosition::Middle,
                length:   SubsetProbeLength::Frames(11),
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
        let (config, found_config_path) = quality_check_handler(
            Some(&config_path),
            Some(&temp.path().join(hash_path(&input_abs))),
            Some(&test_video.path),
            Some(&DecoderMethod::LSMASHWorks),
            Some(&custom_filters),
            Some(&custom_vs_args),
            Some(&QualityMetricBase::BUTTERAUGLI),
            Some(&QualityProfile::Fast),
        )
        .expect("quality_check_handler should succeed");

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
