use std::path::{Path, PathBuf};

use andean_condor::{
    models::sequence::noise_detector::NoiseDetectorConfig,
    vapoursynth::vapoursynth_filters::VapourSynthFilter,
};
use anyhow::Result;
use clap::Subcommand;
use serde_json::Value;
use thiserror::Error;
use tracing::warn;

use crate::{
    commands::{
        DecoderMethod,
        config::{ConfigAction, ConfigCommand, InputOverride, PropertyChange},
        handlers::{
            apply_input_override,
            config::{edit, ensure_override_requested},
            configure_input,
            load_configuration,
        },
    },
    configuration::Configuration,
};

pub fn detect_noise_handler(
    config_path: Option<&Path>,
    input_path: Option<&Path>,
    decoder: Option<&DecoderMethod>,
    vs_args: Option<&[String]>,
    reference_filters: Option<&[VapourSynthFilter]>,
    denoised_filters: Option<&[VapourSynthFilter]>,
) -> Result<(Configuration, PathBuf)> {
    let (mut configuration, config_path) = load_configuration(config_path)?;

    if configuration.condor.sequence_config.noise_detector.is_none() {
        configuration.condor.sequence_config.noise_detector = Some(NoiseDetectorConfig::default());
    }

    configure_noise_detector(
        &mut configuration,
        input_path,
        decoder,
        vs_args,
        reference_filters,
        denoised_filters,
    )?;

    configuration.save(&config_path)?;

    Ok((configuration, config_path))
}

pub fn configure_noise_detector(
    configuration: &mut Configuration,
    input_path: Option<&Path>,
    decoder: Option<&DecoderMethod>,
    vs_args: Option<&[String]>,
    reference_filters: Option<&[VapourSynthFilter]>,
    denoised_filters: Option<&[VapourSynthFilter]>,
) -> Result<()> {
    // Initialize noise_detector if it doesn't exist yet
    if configuration.condor.sequence_config.noise_detector.is_none()
        && (input_path.is_some()
            || decoder.is_some()
            || vs_args.is_some()
            || reference_filters.is_some()
            || denoised_filters.is_some())
    {
        configuration.condor.sequence_config.noise_detector = Some(NoiseDetectorConfig::default());
    }

    if input_path.is_some() || decoder.is_some() || vs_args.is_some() {
        let existing_input = if let Some(Some(input)) = configuration
            .condor
            .sequence_config
            .noise_detector
            .as_ref()
            .map(|nd| nd.input.clone())
        {
            input
        } else {
            configuration.condor.input.clone()
        };
        let input = configure_input(
            configuration,
            &existing_input,
            input_path,
            decoder,
            vs_args,
            None,
            None,
        )?;

        if let Some(noise_detector) = configuration.condor.sequence_config.noise_detector.as_mut() {
            noise_detector.input = Some(input);
        } else {
            configuration.condor.sequence_config.noise_detector = Some(NoiseDetectorConfig {
                input: Some(input),
                ..Default::default()
            });
        }
    };

    if let (Some(filters), Some(noise_detector)) = (
        reference_filters,
        &mut configuration.condor.sequence_config.noise_detector,
    ) {
        noise_detector.reference_filters = filters.to_vec();
    }

    if let (Some(filters), Some(noise_detector)) = (
        denoised_filters,
        &mut configuration.condor.sequence_config.noise_detector,
    ) {
        noise_detector.denoised_filters = filters.to_vec();
    }

    Ok(())
}

/// The words that empty a filter list instead of naming a filter.
const CLEAR_FILTER_WORDS: [&str; 4] = ["none", "empty", "clear", "remove"];

/// `info` and `reset` keys of `sequence_config.noise_detector`.
#[derive(Debug, Clone, Subcommand)]
pub enum DetectNoiseKey {
    /// The input the Noise Detector reads instead of the main input.
    Input,
    /// VapourSynth filters that build the reference (noisy) video.
    ReferenceFilters,
    /// VapourSynth filters that build the denoised video.
    DenoisedFilters,
}

/// Settings `condor detect-noise set` can change.
#[derive(Debug, Clone, Subcommand)]
pub enum DetectNoiseSet {
    /// Read from this input instead of the main input.
    Input {
        #[command(flatten)]
        args: InputOverride,
    },
    /// Replace the reference filters, in order.
    ///
    /// Give the filters themselves (`resize:width=960;`,
    /// `wnnm:sigma=3.0,0.0,0.0;`). No filters at all, or one of `none`,
    /// `empty`, `clear` or `remove`, empties the list; `reset
    /// reference-filters` restores the defaults instead.
    ReferenceFilters {
        /// VapourSynth filters that build the reference video, in order.
        #[arg(value_name = "Filter", num_args = 0..)]
        filters: Vec<String>,
    },
    /// Replace the denoised filters, in order.
    ///
    /// Same syntax as *reference-filters*.
    DenoisedFilters {
        /// VapourSynth filters that build the denoised video, in order.
        #[arg(value_name = "Filter", num_args = 0..)]
        filters: Vec<String>,
    },
}

/// The action triple attached to `condor detect-noise`.
pub type DetectNoiseAction = ConfigAction<DetectNoiseKey, DetectNoiseSet>;

/// Everything that can make a Noise Detector setting unusable.
#[derive(Debug, Error)]
pub enum DetectNoiseConfigError {
    #[error("invalid {which} filter `{value}`: {reason}")]
    InvalidFilter {
        which:  &'static str,
        value:  String,
        reason: String,
    },
}

/// The `condor detect-noise` configuration section.
pub struct DetectNoiseCrud;

impl DetectNoiseCrud {
    /// The section, materialized so a `set` always has somewhere to write.
    fn section_mut(configuration: &mut Configuration) -> &mut NoiseDetectorConfig {
        configuration
            .condor
            .sequence_config
            .noise_detector
            .get_or_insert_with(NoiseDetectorConfig::default)
    }

    /// Parses a filter list, treating an empty list and the clear words as
    /// "no filters".
    fn parse_filters(
        which: &'static str,
        values: &[String],
    ) -> Result<Vec<VapourSynthFilter>, DetectNoiseConfigError> {
        let cleared = values.is_empty()
            || (values.len() == 1
                && CLEAR_FILTER_WORDS.iter().any(|word| values[0].eq_ignore_ascii_case(word)));
        if cleared {
            return Ok(Vec::new());
        }
        values
            .iter()
            .map(|value| {
                value.parse::<VapourSynthFilter>().map_err(|err| {
                    DetectNoiseConfigError::InvalidFilter {
                        which,
                        value: value.clone(),
                        reason: err.to_string(),
                    }
                })
            })
            .collect()
    }

    /// Applies one `set input` override.
    fn apply_input(configuration: &mut Configuration, args: &InputOverride) -> Result<()> {
        ensure_override_requested(args)?;
        let existing = configuration
            .condor
            .sequence_config
            .noise_detector
            .as_ref()
            .and_then(|detector| detector.input.clone())
            .unwrap_or_else(|| configuration.condor.input.clone());
        Self::section_mut(configuration).input =
            Some(apply_input_override(configuration, &existing, args)?);
        Ok(())
    }
}

impl ConfigCommand for DetectNoiseCrud {
    type Key = DetectNoiseKey;
    type Set = DetectNoiseSet;

    fn subject() -> &'static str {
        "Noise detection settings"
    }

    fn render(configuration: &Configuration, key: Option<&Self::Key>) -> Result<Value> {
        let detector = configuration.condor.sequence_config.noise_detector.as_ref();
        Ok(match key {
            None => serde_json::to_value(&configuration.condor.sequence_config.noise_detector)?,
            Some(DetectNoiseKey::Input) => {
                serde_json::to_value(detector.map(|detector| detector.input.as_ref()))?
            },
            Some(DetectNoiseKey::ReferenceFilters) => {
                serde_json::to_value(detector.map(|detector| &detector.reference_filters))?
            },
            Some(DetectNoiseKey::DenoisedFilters) => {
                serde_json::to_value(detector.map(|detector| &detector.denoised_filters))?
            },
        })
    }

    fn apply(configuration: &mut Configuration, set: &Self::Set) -> Result<PropertyChange> {
        match set {
            DetectNoiseSet::Input {
                args,
            } => {
                Self::apply_input(configuration, args)?;
                PropertyChange::new("input", &Self::section_mut(configuration).input)
            },
            DetectNoiseSet::ReferenceFilters {
                filters,
            } => {
                let filters = Self::parse_filters("reference", filters)?;
                Self::section_mut(configuration).reference_filters = filters;
                PropertyChange::new(
                    "reference-filters",
                    &Self::section_mut(configuration).reference_filters,
                )
            },
            DetectNoiseSet::DenoisedFilters {
                filters,
            } => {
                let filters = Self::parse_filters("denoised", filters)?;
                Self::section_mut(configuration).denoised_filters = filters;
                PropertyChange::new(
                    "denoised-filters",
                    &Self::section_mut(configuration).denoised_filters,
                )
            },
        }
    }

    fn restore(
        configuration: &mut Configuration,
        key: Option<&Self::Key>,
    ) -> Result<Option<PropertyChange>> {
        if configuration.condor.sequence_config.noise_detector.is_none() {
            warn!("Noise detection is not configured; nothing to reset.");
            return Ok(None);
        }
        if key.is_none() {
            configuration.condor.sequence_config.noise_detector = None;
            return Ok(Some(PropertyChange {
                property: "section".to_owned(),
                value:    Value::Null,
            }));
        }
        let detector = Self::section_mut(configuration);
        let defaults = NoiseDetectorConfig::default();
        let (property, value) = match key {
            None => unreachable!("handled above"),
            Some(DetectNoiseKey::Input) => {
                detector.input = None;
                ("input", serde_json::to_value(&detector.input)?)
            },
            Some(DetectNoiseKey::ReferenceFilters) => {
                detector.reference_filters = defaults.reference_filters;
                (
                    "reference-filters",
                    serde_json::to_value(&detector.reference_filters)?,
                )
            },
            Some(DetectNoiseKey::DenoisedFilters) => {
                detector.denoised_filters = defaults.denoised_filters;
                (
                    "denoised-filters",
                    serde_json::to_value(&detector.denoised_filters)?,
                )
            },
        };
        Ok(Some(PropertyChange {
            property: property.to_owned(),
            value,
        }))
    }

    fn reset_hint(key: Option<&Self::Key>) -> Option<String> {
        match key {
            Some(DetectNoiseKey::ReferenceFilters) => Some(
                "To clear every reference filter instead, run `condor detect-noise set \
                 reference-filters none`."
                    .to_owned(),
            ),
            Some(DetectNoiseKey::DenoisedFilters) => Some(
                "To clear every denoised filter instead, run `condor detect-noise set \
                 denoised-filters none`."
                    .to_owned(),
            ),
            _ => None,
        }
    }
}

/// `condor detect-noise info|set|reset`.
pub fn config(config_path: Option<&Path>, action: &DetectNoiseAction) -> Result<()> {
    edit::<DetectNoiseCrud>(config_path, action)
}

#[cfg(test)]
mod tests {
    use andean_condor::models::input::{Input as InputModel, VapourSynthImportMethod};

    use super::*;
    use crate::{
        commands::handlers::init::init_handler,
        test_helpers::{check_basic_config, default_config, get_test_video},
        utils::hash_path::hash_path,
    };

    #[test]
    fn detect_noise_default_config() {
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
        expected_config.condor.sequence_config.noise_detector =
            Some(NoiseDetectorConfig::default());
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
        let (config, found_config_path) =
            detect_noise_handler(Some(&config_path), None, None, None, None, None)
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
        let custom_vs_args = vec!["noise_level=2".to_string()];
        let custom_reference_filters = vec![VapourSynthFilter::WNNM {
            sigma:                Some(vec![2.0, 0.0, 0.0]),
            block_size:           None,
            block_step:           None,
            group_size:           None,
            bm_range:             None,
            radius:               None,
            ps_num:               None,
            ps_range:             None,
            residual:             None,
            adaptive_aggregation: None,
        }];
        let custom_denoised_filters = vec![VapourSynthFilter::WNNM {
            sigma:                Some(vec![4.0, 0.0, 0.0]),
            block_size:           None,
            block_step:           None,
            group_size:           None,
            bm_range:             None,
            radius:               None,
            ps_num:               None,
            ps_range:             None,
            residual:             None,
            adaptive_aggregation: None,
        }];

        let mut expected_config = default_config(&test_video, &output, &temp_abs);
        let input_filters = expected_config.condor.input.vapoursynth_filters().to_vec();
        expected_config.condor.sequence_config.noise_detector = Some(NoiseDetectorConfig {
            input:             Some(InputModel::VapourSynth {
                path:          input_abs.clone(),
                import_method: VapourSynthImportMethod::FFMS2 {
                    index: None
                },
                cache_path:    None,
                filters:       input_filters,
            }),
            reference_filters: custom_reference_filters.clone(),
            denoised_filters:  custom_denoised_filters.clone(),
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
        let (config, found_config_path) = detect_noise_handler(
            Some(&config_path),
            Some(&test_video.path),
            Some(&DecoderMethod::VSFFMS2),
            Some(&custom_vs_args),
            Some(&custom_reference_filters),
            Some(&custom_denoised_filters),
        )
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
}
