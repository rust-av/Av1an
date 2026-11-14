use anyhow::{Context, Result, ensure};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::models::{
    encoder::Encoder,
    input::Input as InputModel,
    sequence::{
        SequenceConfigHandler,
        target_quality::types::{
            DEFAULT_MAXIMUM_PROBES, InterpolationMethod, QualityMetric, QualityPass,
            TargetQualityProbing,
        },
    },
};

pub mod types;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TargetQualityConfig
where
    Self: SequenceConfigHandler,
{
    pub metric: QualityMetric,
    pub maximum_probes: u8,
    pub quantizer_range: (u32, u32),
    /// Methods for 3 and 4 recorded probes, respectively (not score directions).
    /// With 0/1 records use midpoint, 2 use linear, and 5+ use whole-history PCHIP.
    /// Linear/PCHIP need 2 points; quadratic/Fritsch–Carlson/natural need 3;
    /// Catmull/Akima/cubic polynomial need 4 (Catmull covers only the inner interval).
    /// Insufficient points, rejection or nonfinite output use bracketing linear,
    /// then midpoint. Duplicate scores or an out-of-observed-range target use midpoint.
    pub interpolators: (InterpolationMethod, InterpolationMethod),
    pub input: Option<InputModel>,
    pub metric_input: Option<InputModel>,
    pub probing: TargetQualityProbing,
    // Completion is determined by target and max probes, date from final quality pass
}

impl Default for TargetQualityConfig {
    #[inline]
    fn default() -> Self {
        Self {
            metric: QualityMetric::default(),
            maximum_probes: DEFAULT_MAXIMUM_PROBES,
            quantizer_range: (0, 51),
            interpolators: (
                InterpolationMethod::FritschCarlson,
                InterpolationMethod::Pchip,
            ),
            input: None,
            metric_input: None,
            probing: TargetQualityProbing::default(),
        }
    }
}

impl SequenceConfigHandler for TargetQualityConfig {}

impl TargetQualityConfig {
    /// Checks a target and both quantizer endpoints against the effective probe encoder.
    /// Equal target/quantizer bounds are valid. This is model-only: native bit-depth,
    /// capability and arbitrary FFmpeg codec constraints still require runtime checks.
    #[inline]
    pub fn validate(&self, encoder: &Encoder) -> Result<()> {
        let (minimum, maximum) = self.metric.target_range();
        ensure!(
            minimum.is_finite() && maximum.is_finite(),
            "target endpoints must be finite"
        );
        ensure!(minimum <= maximum, "target minimum must not exceed maximum");
        ensure!(
            self.quantizer_range.0 <= self.quantizer_range.1,
            "minimum quantizer must not exceed maximum quantizer"
        );
        ensure!(self.maximum_probes > 0, "maximum probes must be positive");
        self.probing.strategy.validate()?;
        let mut probe = self.probe_encoder(encoder);
        for quantizer in [self.quantizer_range.0, self.quantizer_range.1] {
            probe.set_quantizer(f64::from(quantizer));
            probe
                .validate_settings()
                .with_context(|| format!("quantizer {quantizer} for {}", encoder.base()))?;
        }
        Ok(())
    }

    /// Builds the same option replacement and metric-safe encoder used by execution.
    /// Pass/rate-control settings are retained; only the existing probe sanitization applies.
    #[inline]
    pub fn probe_encoder(&self, encoder: &Encoder) -> Encoder {
        let mut encoder = encoder.clone();
        if let Some(parameters) = &self.probing.encoder_options {
            *encoder.parameters_mut() = parameters.clone();
        }
        // Probes measure against the unfiltered reference; final encodes still filter.
        encoder.set_ffmpeg_filter(None);
        crate::core::sequence::target_quality::TargetQuality::remove_psychovisual_parameters(
            &encoder,
        )
    }
}

pub trait TargetQualityConfigHandler {
    fn target_quality(&self) -> Result<&Option<TargetQualityConfig>>;
    fn target_quality_mut(&mut self) -> Result<&mut Option<TargetQualityConfig>>;
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TargetQualityData
where
    Self: Default,
{
    pub passes: Vec<QualityPass>,
    /// Selection over certified raw measurements, before later bitrate/speed edits.
    #[serde(default)]
    pub selection: Option<super::provenance::ResultProvenance>,
    /// Owning Speed Scaler transition; it does not certify measurements by itself.
    #[serde(default)]
    pub speed_transition: Option<ProbeEncoderTransition>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ProbeEncoderTransition {
    /// Raw scene settings, so removing a probe override restores inheritance.
    pub before: Encoder,
    pub after: Encoder,
}

impl Default for TargetQualityData {
    #[inline]
    fn default() -> Self {
        Self {
            passes: Vec::new(),
            selection: None,
            speed_transition: None,
        }
    }
}

pub trait TargetQualityDataHandler {
    fn get_target_quality(&self) -> Result<&TargetQualityData>;
    fn get_target_quality_mut(&mut self) -> Result<&mut TargetQualityData>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::encoder::{EncoderBase, EncoderPasses, cli_parameter::CLIParameter};
    use types::ProbeStrategy;

    #[test]
    fn target_quality_quantizer_endpoints_match_each_encoder_range() {
        for (base, maximum) in [
            (EncoderBase::AOM, 63),
            (EncoderBase::VPX, 63),
            (EncoderBase::RAV1E, 255),
            (EncoderBase::SVTAV1, 70),
            (EncoderBase::AVM, 255),
            (EncoderBase::X264, 51),
            (EncoderBase::X265, 51),
            (EncoderBase::VVenC, 63),
        ] {
            let mut encoder = Encoder::default_from_base(&base, false);
            encoder.set_executable(Some("must-not-run".into()));
            let before = serde_json::to_value(&encoder).expect("snapshot");
            for range in [(0, maximum), (maximum, maximum), (0, 0)] {
                let config = TargetQualityConfig {
                    quantizer_range: range,
                    maximum_probes: 255,
                    ..Default::default()
                };
                config
                    .validate(&encoder)
                    .unwrap_or_else(|error| panic!("{base}/{range:?}: {error:#}"));
            }
            for range in [(0, maximum + 1), (maximum + 1, maximum + 1)] {
                assert!(
                    TargetQualityConfig {
                        quantizer_range: range,
                        ..Default::default()
                    }
                    .validate(&encoder)
                    .is_err(),
                    "{base}/{range:?}"
                );
            }
            assert_eq!(serde_json::to_value(&encoder).expect("unchanged"), before);
        }
        // FFmpeg's native codec has no universal CRF envelope in the pure checks.
        TargetQualityConfig {
            quantizer_range: (0, u32::MAX),
            ..Default::default()
        }
        .validate(&Encoder::default_from_base(&EncoderBase::FFmpeg, false))
        .expect("native ceiling deferred");
    }

    #[test]
    fn target_quality_rejects_invalid_score_ranges() {
        let encoder = Encoder::default();
        for range in [
            (f64::NAN, 75.0),
            (74.0, f64::NAN),
            (f64::NEG_INFINITY, 75.0),
            (74.0, f64::INFINITY),
            (76.0, 74.0),
        ] {
            let mut config = TargetQualityConfig::default();
            *config.metric.target_range_mut() = range;
            assert!(config.validate(&encoder).is_err(), "{range:?}");
        }
        let mut config = TargetQualityConfig::default();
        *config.metric.target_range_mut() = (75.0, 75.0);
        config.quantizer_range = (20, 20);
        config.validate(&encoder).expect("equal bounds");
    }

    #[test]
    fn target_quality_rejects_too_few_probes() {
        let encoder = Encoder::default();
        let mut config = TargetQualityConfig::default();
        *config.metric.target_range_mut() = (75.0, 75.0);
        config.quantizer_range = (20, 20);
        config.maximum_probes = 0;
        assert!(config.validate(&encoder).is_err());
    }

    #[test]
    fn target_quality_rejects_invalid_frame_selection() {
        let encoder = Encoder::default();
        let mut config = TargetQualityConfig::default();
        *config.metric.target_range_mut() = (75.0, 75.0);
        config.quantizer_range = (20, 20);
        config.maximum_probes = 1;
        config.probing.strategy = ProbeStrategy::Skip { skip: 0 };
        assert!(config.validate(&encoder).is_err());
    }

    #[test]
    fn effective_options_replace_scene_options_without_hiding_pass_or_rc_conflicts() {
        let mut encoder = Encoder::default_from_base(&EncoderBase::SVTAV1, true);
        *encoder.parameters_mut() = CLIParameter::new_numbers("--", " ", &[("preset", 99.0)]);
        let mut config = TargetQualityConfig::default();
        assert!(config.validate(&encoder).is_err());
        config.probing.encoder_options = Some(EncoderBase::SVTAV1.default_parameters());
        config.validate(&encoder).expect("probe options replace bad scene options");
        assert_eq!(
            config
                .probe_encoder(&encoder)
                .parameters()
                .get("preset")
                .expect("preset")
                .to_string_value(),
            "4"
        );
        config
            .probing
            .encoder_options
            .as_mut()
            .expect("options")
            .insert("rc".into(), CLIParameter::new_number("--", " ", 1.0));
        assert!(config.validate(&encoder).is_err(), "retain rc conflict");
        config.probing.encoder_options = Some(EncoderBase::SVTAV1.default_parameters());
        if let Encoder::SVTAV1 { pass, .. } = &mut encoder {
            *pass = EncoderPasses::All(0);
        }
        assert!(
            config.validate(&encoder).is_err(),
            "retain stored pass tuple"
        );
        let mut encoder = Encoder::default_from_base(&EncoderBase::X264, false);
        encoder
            .parameters_mut()
            .insert("qp".into(), CLIParameter::new_number("--", " ", 82.0));
        config.probing.encoder_options = None;
        assert!(
            config.validate(&encoder).is_err(),
            "unrelated native numeric checks remain"
        );
    }
}
