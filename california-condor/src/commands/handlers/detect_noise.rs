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
            configure_input, load_configuration,
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

    configuration.admit_results()?;
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

/// Settings `av1an detect-noise set` can change.
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

/// The action triple attached to `av1an detect-noise`.
pub type DetectNoiseAction = ConfigAction<DetectNoiseKey, DetectNoiseSet>;

/// Everything that can make a Noise Detector setting unusable.
#[derive(Debug, Error)]
pub enum DetectNoiseConfigError {
    #[error("invalid {which} filter `{value}`: {reason}")]
    InvalidFilter {
        which: &'static str,
        value: String,
        reason: String,
    },
}

/// The `av1an detect-noise` configuration section.
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
            DetectNoiseSet::Input { args } => {
                Self::apply_input(configuration, args)?;
                PropertyChange::new("input", &Self::section_mut(configuration).input)
            },
            DetectNoiseSet::ReferenceFilters { filters } => {
                let filters = Self::parse_filters("reference", filters)?;
                Self::section_mut(configuration).reference_filters = filters;
                PropertyChange::new(
                    "reference-filters",
                    &Self::section_mut(configuration).reference_filters,
                )
            },
            DetectNoiseSet::DenoisedFilters { filters } => {
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
                value: Value::Null,
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
                "To clear every reference filter instead, run `av1an detect-noise set \
                 reference-filters none`."
                    .to_owned(),
            ),
            Some(DetectNoiseKey::DenoisedFilters) => Some(
                "To clear every denoised filter instead, run `av1an detect-noise set \
                 denoised-filters none`."
                    .to_owned(),
            ),
            _ => None,
        }
    }
}

/// `av1an detect-noise info|set|reset`.
pub fn config(config_path: Option<&Path>, action: &DetectNoiseAction) -> Result<()> {
    edit::<DetectNoiseCrud>(config_path, action)
}
