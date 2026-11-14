use std::{collections::HashMap, path::PathBuf};

use anyhow::{Context, Result, ensure};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use strum::{Display, EnumString, IntoStaticStr};

use crate::{
    models::encoder::cli_parameter::CLIParameter, vapoursynth::plugins::vship::cvvdp::DisplayModel,
};

pub static DEFAULT_MAXIMUM_PROBES: u8 = 4;
pub static DEFAULT_VMAF_TARGET_RANGE: (f64, f64) = (94.0, 96.0);
pub static DEFAULT_SSIMULACRA2_TARGET_RANGE: (f64, f64) = (74.0, 76.0);
pub static DEFAULT_BUTTERAUGLI_TARGET_RANGE: (f64, f64) = (0.8, 1.2);
pub static DEFAULT_XPSNR_TARGET_RANGE: (f64, f64) = (44.0, 46.0);
pub static DEFAULT_CVVDP_TARGET_RANGE: (f64, f64) = (9.4, 9.6);

/// Invalid measurement data, distinct from an unavailable or failed backend.
#[derive(Debug, thiserror::Error)]
pub enum MeasurementError {
    #[error("empty score set")]
    Empty,
    #[error("expected {expected} scores, got {actual}")]
    Count { expected: usize, actual: usize },
    #[error("non-finite score at position {position}: {value}")]
    NonFinite { position: usize, value: f64 },
    #[error("score position {position} is outside the {expected} selected frames")]
    Position { position: usize, expected: usize },
    #[error("duplicate score at position {position}")]
    Duplicate { position: usize },
    #[error("no metric value at position {position}")]
    Missing { position: usize },
    #[error("invalid statistic: {0}")]
    Statistic(&'static str),
    #[error("non-finite recorded {0}")]
    Record(&'static str),
}

pub(crate) fn validate_score(value: f64, position: usize) -> Result<()> {
    ensure!(
        value.is_finite(),
        MeasurementError::NonFinite { position, value }
    );
    Ok(())
}

/// Admit a nonempty finite set, optionally requiring an exact selection count.
pub(crate) fn validate_scores(scores: &[f64], expected: Option<usize>) -> Result<()> {
    if let Some(expected) = expected {
        ensure!(
            scores.len() == expected,
            MeasurementError::Count {
                expected,
                actual: scores.len()
            }
        );
    }
    ensure!(!scores.is_empty(), MeasurementError::Empty);
    for (position, &value) in scores.iter().enumerate() {
        validate_score(value, position)?;
    }
    Ok(())
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    Display,
    EnumString,
    IntoStaticStr,
    JsonSchema,
)]
pub enum InterpolationMethod {
    #[strum(serialize = "linear")]
    Linear,
    #[strum(serialize = "quadratic")]
    Quadratic,
    #[strum(serialize = "natural")]
    Natural,
    #[strum(serialize = "fritschcarlson")]
    FritschCarlson,
    #[strum(serialize = "pchip")]
    Pchip,
    #[strum(serialize = "catmull")]
    Catmull,
    #[strum(serialize = "akima")]
    Akima,
    #[strum(serialize = "cubicpolynomial")]
    CubicPolynomial,
}

#[derive(
    Debug,
    Clone,
    Copy,
    Eq,
    PartialEq,
    Serialize,
    Deserialize,
    EnumString,
    IntoStaticStr,
    Display,
    JsonSchema,
)]
pub enum VmafFeature {
    #[strum(serialize = "default")]
    Default,
    #[strum(serialize = "weighted")]
    Weighted,
    #[strum(serialize = "neg")]
    Neg,
    #[strum(serialize = "motionless")]
    Motionless,
    #[strum(serialize = "uhd")]
    Uhd,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub enum QualityMetric {
    VMAF {
        target_range: (f64, f64),
        resolution: Option<(u32, u32)>,
        scaler: String,
        threads: usize,
        model: Option<PathBuf>,
        features: Vec<VmafFeature>,
    },
    SSIMULACRA2 {
        target_range: (f64, f64),
        resolution: Option<(u32, u32)>,
        threads: Option<u8>,
        gpu_id: Option<u8>,
    },
    BUTTERAUGLI {
        target_range: (f64, f64),
        resolution: Option<(u32, u32)>,
        threads: Option<u8>,
        intensity_multiplier: Option<f64>,
        norm: Option<u8>,
        gpu_id: Option<u8>,
    },
    XPSNR {
        target_range: (f64, f64),
        resolution: Option<(u32, u32)>,
    },
    CVVDP {
        target_range: (f64, f64),
        resolution: Option<(u32, u32)>,
        display_model: Option<DisplayModel>,
        resize_to_display: Option<bool>,
        disable_temporal: Option<bool>,
        gpu_id: Option<u8>,
    },
}

impl Default for QualityMetric {
    #[inline]
    fn default() -> Self {
        Self::SSIMULACRA2 {
            target_range: DEFAULT_SSIMULACRA2_TARGET_RANGE,
            resolution: None,
            threads: None,
            gpu_id: None,
        }
    }
}

impl QualityMetric {
    /// Whether ordered scores are cumulative prefixes rather than independent frames.
    #[inline]
    pub const fn is_cumulative(&self) -> bool {
        matches!(
            self,
            Self::CVVDP {
                disable_temporal: None | Some(false),
                ..
            }
        )
    }

    /// Reduce one ordered session: cumulative CVVDP uses its final prefix, all
    /// other modes use the configured statistic. Rejects empty/non-finite input
    /// (including every cumulative prefix), invalid configuration and undefined
    /// or non-finite reductions. Cumulative prefixes are never sorted.
    #[inline]
    pub fn calculate_score(&self, statistic: ProbeStatistic, scores: &[f64]) -> Result<f64> {
        validate_scores(scores, None)
            .with_context(|| format!("{} measurement", self.friendly_name()))?;
        statistic.validate()?;
        if self.is_cumulative() {
            Ok(scores[scores.len() - 1])
        } else {
            statistic
                .calculate(scores)
                .with_context(|| format!("{} statistic", self.friendly_name()))
        }
    }

    #[inline]
    pub fn score_within_target(&self, score: f64) -> bool {
        match self {
            QualityMetric::VMAF { target_range, .. } => {
                score >= target_range.0 && score <= target_range.1
            },
            QualityMetric::SSIMULACRA2 { target_range, .. } => {
                score >= target_range.0 && score <= target_range.1
            },
            QualityMetric::BUTTERAUGLI { target_range, .. } => {
                score >= target_range.0 && score <= target_range.1
            },
            QualityMetric::XPSNR { target_range, .. } => {
                score >= target_range.0 && score <= target_range.1
            },
            QualityMetric::CVVDP { target_range, .. } => {
                score >= target_range.0 && score <= target_range.1
            },
        }
    }

    #[inline]
    pub fn target_range(&self) -> (f64, f64) {
        match self {
            QualityMetric::VMAF { target_range, .. } => *target_range,
            QualityMetric::SSIMULACRA2 { target_range, .. } => *target_range,
            QualityMetric::BUTTERAUGLI { target_range, .. } => *target_range,
            QualityMetric::XPSNR { target_range, .. } => *target_range,
            QualityMetric::CVVDP { target_range, .. } => *target_range,
        }
    }

    #[inline]
    pub fn target_range_mut(&mut self) -> &mut (f64, f64) {
        match self {
            QualityMetric::VMAF { target_range, .. } => target_range,
            QualityMetric::SSIMULACRA2 { target_range, .. } => target_range,
            QualityMetric::BUTTERAUGLI { target_range, .. } => target_range,
            QualityMetric::XPSNR { target_range, .. } => target_range,
            QualityMetric::CVVDP { target_range, .. } => target_range,
        }
    }

    #[inline]
    pub fn is_inverse_metric(&self) -> bool {
        matches!(self, QualityMetric::BUTTERAUGLI { .. })
    }

    #[inline]
    pub fn friendly_name(&self) -> &'static str {
        match self {
            QualityMetric::VMAF { .. } => "VMAF",
            QualityMetric::SSIMULACRA2 { .. } => "SSIMULACRA2",
            QualityMetric::BUTTERAUGLI { .. } => "butteraugli",
            QualityMetric::XPSNR { .. } => "XPSNR",
            QualityMetric::CVVDP { .. } => "CVVDP",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct TargetQualityProbing {
    pub encoder_options: Option<HashMap<String, CLIParameter>>,
    pub strategy: ProbeStrategy,
    pub statistic: ProbeStatistic,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
pub enum ProbeStrategy {
    #[default]
    Whole,
    Skip {
        skip: u32,
    },
    Subset {
        position: SubsetProbePosition,
        length: SubsetProbeLength,
    },
}

impl ProbeStrategy {
    /// Reject zero frame/skip counts and non-finite percentages outside `0 < p <= 100`.
    #[inline]
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Skip { skip } => ensure!(*skip > 0, "probe skip must be positive, got {skip}"),
            Self::Subset {
                length: SubsetProbeLength::Frames(frames),
                ..
            } => {
                ensure!(*frames > 0, "probe frames must be positive, got {frames}");
            },
            Self::Subset {
                length: SubsetProbeLength::Percentage(percent),
                ..
            } => {
                ensure!(
                    percent.is_finite() && *percent > 0.0 && *percent <= 100.0,
                    "probe percent must be finite and within 0 < percent <= 100, got {percent}"
                );
            },
            Self::Whole => (),
        }
        Ok(())
    }

    /// Validate selection without allocating indices; ranges are nonempty and half-open.
    #[inline]
    pub fn validate_range(&self, start: usize, end: usize) -> Result<()> {
        self.validate()?;
        ensure!(
            start < end,
            "invalid probe frame range {start}..{end}: start must be less than end"
        );
        Ok(())
    }

    /// Select source indices in `[start, end)`, or reject an invalid strategy/range.
    /// Percentages use units of 0..100, rounded and clamped to at least one frame.
    #[inline]
    pub fn frame_indices(&self, start: usize, end: usize) -> Result<Vec<usize>> {
        self.validate_range(start, end)?;
        Ok(match self {
            ProbeStrategy::Whole => (start..end).collect::<Vec<_>>(),
            ProbeStrategy::Skip { skip } => (start..end).step_by(*skip as usize).collect(),
            ProbeStrategy::Subset { position, length } => {
                let max_length = end - start;
                let length = match length {
                    SubsetProbeLength::Percentage(percentage) => {
                        ((percentage / 100.0 * max_length as f64).round() as usize)
                            .clamp(1, max_length)
                    },
                    SubsetProbeLength::Frames(frames) => (*frames as usize).min(max_length),
                };
                let offset = match position {
                    SubsetProbePosition::Start => 0,
                    SubsetProbePosition::Middle => (max_length - length).div_ceil(2),
                    SubsetProbePosition::End => max_length - length,
                };
                // INVARIANT: offset + length <= max_length, including near usize::MAX.
                let subset_start = start.saturating_add(offset);
                (subset_start..subset_start.saturating_add(length).min(end)).collect()
            },
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub enum SubsetProbePosition {
    Start,
    #[default]
    Middle,
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum SubsetProbeLength {
    /// Finite percentage in `0 < p <= 100`; round to a count with a minimum of one.
    Percentage(f64),
    /// Positive frame count, clamped to the nonempty scene length.
    Frames(u32),
}

impl Default for SubsetProbeLength {
    #[inline]
    fn default() -> Self {
        SubsetProbeLength::Frames(11)
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum ProbeStatistic {
    #[default]
    Mean,
    Median,
    Harmonic,
    Percentile(f64),
    StandardDeviationDistance {
        sigma: f64,
    },
    Mode,
    Minimum,
    Maximum,
    RootMeanSquare,
}

impl ProbeStatistic {
    pub(crate) fn validate(&self) -> Result<()> {
        match self {
            Self::Percentile(p) => ensure!(
                p.is_finite() && (0.0..=100.0).contains(p),
                MeasurementError::Statistic("percentile must be finite and within 0..=100")
            ),
            Self::StandardDeviationDistance { sigma } => ensure!(
                sigma.is_finite(),
                MeasurementError::Statistic("sigma must be finite")
            ),
            _ => (),
        }
        Ok(())
    }

    /// Reduce a nonempty set of finite scores, preserving signed values.
    ///
    /// # Errors
    /// Rejects invalid percentile/sigma configuration, undefined harmonic means
    /// and non-finite arithmetic. Any zero sample makes the harmonic mean zero;
    /// otherwise its reciprocal sum must be finite and nonzero.
    #[inline]
    pub fn calculate(&self, scores: &[f64]) -> Result<f64> {
        self.validate()?;
        validate_scores(scores, None)?;
        let mut scores = scores.to_vec();
        scores.sort_by(f64::total_cmp);
        let result = match self {
            ProbeStatistic::Mean => {
                scores.iter().fold(0.0, |acc, score| acc + score) / scores.len() as f64
            },
            ProbeStatistic::Median => {
                let mid = scores.len() / 2;
                if scores.len().is_multiple_of(2) {
                    f64::midpoint(scores[mid - 1], scores[mid])
                } else {
                    scores[mid]
                }
            },
            ProbeStatistic::Harmonic => {
                if scores.contains(&0.0) {
                    0.0
                } else {
                    let sum_reciprocals: f64 = scores.iter().map(|&x| 1.0 / x).sum();
                    ensure!(
                        sum_reciprocals.is_finite() && sum_reciprocals != 0.0,
                        MeasurementError::Statistic(
                            "harmonic reciprocal sum must be finite and nonzero"
                        )
                    );
                    scores.len() as f64 / sum_reciprocals
                }
            },
            ProbeStatistic::Percentile(index) => {
                let index = ((*index / 100.0 * scores.len() as f64) as usize).min(scores.len() - 1);
                scores[index]
            },
            ProbeStatistic::StandardDeviationDistance { sigma } => {
                let average = ProbeStatistic::Mean.calculate(&scores)?;
                let minimum = ProbeStatistic::Minimum.calculate(&scores)?;
                let maximum = ProbeStatistic::Maximum.calculate(&scores)?;
                let variance = scores
                    .iter()
                    .map(|x| {
                        let diff = x - average;
                        diff * diff
                    })
                    .sum::<f64>()
                    / scores.len() as f64;
                let distance = average + (sigma * variance.sqrt());
                ensure!(
                    variance.is_finite() && distance.is_finite(),
                    MeasurementError::Statistic("non-finite variance or sigma distance")
                );
                distance.clamp(minimum, maximum)
            },
            ProbeStatistic::Mode => {
                let mut counts = HashMap::new();
                for score in &scores {
                    // Round to nearest integer for fewer unique buckets
                    let rounded_score = score.round() as i32;
                    *counts.entry(rounded_score).or_insert(0) += 1;
                }
                let max_count = counts.values().copied().max().ok_or(MeasurementError::Empty)?;
                *scores
                    .iter()
                    .find(|score| counts[&(score.round() as i32)] == max_count)
                    .ok_or(MeasurementError::Empty)?
            },
            ProbeStatistic::Minimum => scores[0],
            ProbeStatistic::Maximum => scores[scores.len() - 1],
            ProbeStatistic::RootMeanSquare => {
                let sum_of_squares: f64 = scores.iter().map(|&x| x * x).sum();
                (sum_of_squares / scores.len() as f64).sqrt()
            },
        };
        ensure!(
            result.is_finite(),
            MeasurementError::Statistic("non-finite reduction result")
        );
        Ok(result)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct QualityPass {
    #[serde(default)]
    pub provenance: Option<super::super::provenance::ResultProvenance>,
    #[serde(default)]
    pub backend: Option<super::super::provenance::MetricBackend>,
    pub quantizer: f64,
    pub scores: Vec<f64>,
    pub bitrate: f64,
    /// Must be in milliseconds since UNIX Epoch
    pub started_on: u128,
    /// Must be in milliseconds since UNIX Epoch
    pub completed_on: u128,
}

impl QualityPass {
    /// QC's initial record is not a claimed measurement. TQ history is always claimed.
    #[inline]
    pub fn is_unmeasured(&self) -> bool {
        self.scores.is_empty() && self.started_on == 0 && self.completed_on == 0
    }
}

impl Default for QualityPass {
    #[inline]
    fn default() -> Self {
        Self {
            provenance: None,
            backend: None,
            quantizer: 0.0,
            scores: Vec::new(),
            bitrate: 0.0,
            started_on: 0,
            completed_on: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ProbeStatistic, ProbeStrategy, QualityMetric, SubsetProbeLength, SubsetProbePosition,
    };

    #[test]
    fn statistics_reject_empty_and_nonfinite() {
        for statistic in [
            ProbeStatistic::Mean,
            ProbeStatistic::Median,
            ProbeStatistic::Harmonic,
            ProbeStatistic::Percentile(50.0),
            ProbeStatistic::StandardDeviationDistance { sigma: 1.0 },
            ProbeStatistic::Mode,
            ProbeStatistic::Minimum,
            ProbeStatistic::Maximum,
            ProbeStatistic::RootMeanSquare,
        ] {
            for scores in
                [vec![], vec![1.0, f64::NAN], vec![f64::INFINITY], vec![f64::NEG_INFINITY]]
            {
                let result = std::panic::catch_unwind(|| statistic.calculate(&scores));
                assert!(
                    matches!(result, Ok(Err(_))),
                    "{statistic:?}/{scores:?}: {result:?}"
                );
            }
        }
    }

    #[test]
    fn undefined_and_overflow_statistics_are_errors() {
        for (statistic, scores) in [
            (ProbeStatistic::Harmonic, vec![1.0, -1.0]),
            (ProbeStatistic::Harmonic, vec![f64::from_bits(1)]),
            (ProbeStatistic::Mean, vec![f64::MAX, f64::MAX]),
            (ProbeStatistic::RootMeanSquare, vec![f64::MAX]),
            (
                ProbeStatistic::StandardDeviationDistance { sigma: 1.0 },
                vec![-f64::MAX, f64::MAX],
            ),
            (
                ProbeStatistic::StandardDeviationDistance { sigma: f64::MAX },
                vec![-2.0, 2.0],
            ),
        ] {
            assert!(
                statistic.calculate(&scores).is_err(),
                "{statistic:?}/{scores:?}"
            );
        }
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0, 101.0] {
            assert!(ProbeStatistic::Percentile(value).calculate(&[1.0]).is_err());
        }
        for sigma in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(ProbeStatistic::StandardDeviationDistance { sigma }.calculate(&[1.0]).is_err());
        }
    }

    #[test]
    fn finite_statistics_preserve_conventions() {
        for (statistic, expected) in [
            (ProbeStatistic::Mean, 0.5),
            (ProbeStatistic::Median, 0.5),
            (ProbeStatistic::Harmonic, -4.0),
            (ProbeStatistic::Percentile(50.0), 2.0),
            (
                ProbeStatistic::StandardDeviationDistance { sigma: -1.0 },
                -1.0,
            ),
            (ProbeStatistic::Mode, -1.0),
            (ProbeStatistic::Minimum, -1.0),
            (ProbeStatistic::Maximum, 2.0),
            (ProbeStatistic::RootMeanSquare, 2.5_f64.sqrt()),
        ] {
            assert_eq!(
                statistic.calculate(&[-1.0, 2.0]).expect("signed finite input"),
                expected
            );
        }
        for statistic in [
            ProbeStatistic::Mean,
            ProbeStatistic::Median,
            ProbeStatistic::Harmonic,
            ProbeStatistic::Percentile(50.0),
            ProbeStatistic::StandardDeviationDistance { sigma: -1.0 },
            ProbeStatistic::Mode,
            ProbeStatistic::Minimum,
            ProbeStatistic::Maximum,
        ] {
            assert_eq!(statistic.calculate(&[-2.0]).expect("singleton"), -2.0);
        }
        assert_eq!(
            ProbeStatistic::RootMeanSquare.calculate(&[-2.0]).expect("RMS"),
            2.0
        );
        assert_eq!(
            ProbeStatistic::Mean.calculate(&[-1.0, 2.0]).expect("mean"),
            0.5
        );
        assert_eq!(
            ProbeStatistic::Harmonic.calculate(&[-1.0, 2.0]).expect("signed harmonic"),
            -4.0
        );
        assert_eq!(
            ProbeStatistic::Harmonic.calculate(&[0.0, 1.0, -1.0]).expect("zero wins"),
            0.0
        );
    }

    #[test]
    fn percentile_caps_at_maximum_without_changing_lower_ranks() {
        let scores = [4.0, 1.0, 3.0, 2.0];
        for (percentile, expected) in
            [(0.0, 1.0), (25.0, 2.0), (50.0, 3.0), (75.0, 4.0), (99.0, 4.0), (100.0, 4.0)]
        {
            let statistic = ProbeStatistic::Percentile(percentile);
            assert_eq!(statistic.calculate(&scores).expect("percentile"), expected);
            assert_eq!(
                QualityMetric::default()
                    .calculate_score(statistic, &scores)
                    .expect("independent"),
                expected
            );
        }
        for percentile in [0.0, 100.0] {
            let statistic = ProbeStatistic::Percentile(percentile);
            assert_eq!(statistic.calculate(&[-2.0]).expect("singleton"), -2.0);
            assert!(statistic.calculate(&[]).is_err());
        }
        for percentile in [-1.0, 101.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(ProbeStatistic::Percentile(percentile).calculate(&scores).is_err());
        }
        let temporal = QualityMetric::CVVDP {
            target_range: (0.0, 10.0),
            resolution: None,
            display_model: None,
            resize_to_display: None,
            disable_temporal: None,
            gpu_id: None,
        };
        assert_eq!(
            temporal
                .calculate_score(ProbeStatistic::Percentile(100.0), &scores)
                .expect("final prefix"),
            2.0
        );
        assert_eq!(scores, [4.0, 1.0, 3.0, 2.0]);
    }

    #[test]
    fn temporal_admission_checks_every_prefix_without_sorting() {
        let metric = QualityMetric::CVVDP {
            target_range: (0.0, 10.0),
            resolution: None,
            display_model: None,
            resize_to_display: None,
            disable_temporal: None,
            gpu_id: None,
        };
        for scores in [
            vec![],
            vec![f64::NAN, 4.0],
            vec![f64::INFINITY, 4.0],
            vec![f64::NEG_INFINITY, 4.0],
        ] {
            assert!(
                metric.calculate_score(ProbeStatistic::Mean, &scores).is_err(),
                "{scores:?}"
            );
        }
        let scores = [2.0, 9.0, 4.0];
        assert_eq!(
            metric.calculate_score(ProbeStatistic::Maximum, &scores).expect("final"),
            4.0
        );
        assert_eq!(scores, [2.0, 9.0, 4.0]);
    }

    #[test]
    fn percentage_selects_scene_fraction() {
        let strategy = ProbeStrategy::Subset {
            position: SubsetProbePosition::Start,
            length: SubsetProbeLength::Percentage(25.0),
        };
        assert_eq!(
            strategy.frame_indices(0, 100).expect("selection"),
            (0..25).collect::<Vec<_>>()
        );
    }

    #[test]
    fn small_numeric_and_boundary_matrix() {
        use SubsetProbeLength::{Frames, Percentage};
        use SubsetProbePosition::{End, Middle, Start};
        for (position, length, start, end, expected) in [
            (End, Percentage(25.0), 0, 100, (75..100).collect::<Vec<_>>()),
            (Middle, Percentage(25.0), 100, 200, (138..163).collect()),
            (Start, Percentage(100.0), 7, 12, vec![7, 8, 9, 10, 11]),
            (Middle, Percentage(100.0), 7, 12, vec![7, 8, 9, 10, 11]),
            (End, Percentage(100.0), 7, 12, vec![7, 8, 9, 10, 11]),
            (Start, Percentage(f64::MIN_POSITIVE), 7, 12, vec![7]),
            (Middle, Percentage(f64::MIN_POSITIVE), 7, 12, vec![9]),
            (End, Percentage(f64::MIN_POSITIVE), 7, 12, vec![11]),
            (Middle, Percentage(25.0), 9, 10, vec![9]),
            (Middle, Percentage(25.0), 7, 13, vec![9, 10]),
            (Middle, Percentage(f64::from_bits(1)), 7, 12, vec![9]),
            (Start, Frames(100), 7, 12, vec![7, 8, 9, 10, 11]),
            (Middle, Frames(100), 7, 12, vec![7, 8, 9, 10, 11]),
            (End, Frames(100), 7, 12, vec![7, 8, 9, 10, 11]),
            (
                Middle,
                Frames(1),
                usize::MAX - 100,
                usize::MAX,
                vec![usize::MAX - 50],
            ),
            (Middle, Frames(1), 0, usize::MAX, vec![usize::MAX / 2]),
            (End, Frames(1), 0, usize::MAX, vec![usize::MAX - 1]),
        ] {
            let strategy = ProbeStrategy::Subset { position, length };
            assert_eq!(
                strategy.frame_indices(start, end).expect("selection"),
                expected,
                "{strategy:?}/{start}..{end}"
            );
        }
        assert_eq!(
            ProbeStrategy::Skip { skip: 3 }.frame_indices(7, 15).expect("skip"),
            vec![7, 10, 13]
        );
        assert_eq!(
            ProbeStrategy::Skip { skip: 20 }.frame_indices(7, 15).expect("skip"),
            vec![7]
        );
        assert_eq!(
            ProbeStrategy::Whole.frame_indices(7, 10).expect("whole"),
            vec![7, 8, 9]
        );
    }

    #[test]
    fn checked_selection_rejects_invalid_values_and_ranges() {
        for strategy in [
            ProbeStrategy::Skip { skip: 0 },
            ProbeStrategy::Subset {
                position: SubsetProbePosition::Middle,
                length: SubsetProbeLength::Frames(0),
            },
        ]
        .into_iter()
        .chain(
            [0.0, -0.0, -1.0, 101.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY].map(|percent| {
                ProbeStrategy::Subset {
                    position: SubsetProbePosition::Middle,
                    length: SubsetProbeLength::Percentage(percent),
                }
            }),
        ) {
            assert!(strategy.validate().is_err(), "{strategy:?}");
            assert!(strategy.frame_indices(7, 12).is_err(), "{strategy:?}");
        }
        for strategy in [
            ProbeStrategy::Whole,
            ProbeStrategy::Skip { skip: 2 },
            ProbeStrategy::Subset {
                position: SubsetProbePosition::Middle,
                length: SubsetProbeLength::Frames(1),
            },
        ] {
            for (start, end) in [(0, 0), (10, 10), (10, 7), (usize::MAX, usize::MAX - 1)] {
                let error = strategy.frame_indices(start, end).expect_err("invalid range");
                assert!(error.to_string().contains(&format!("{start}..{end}")));
            }
        }
    }

    #[test]
    fn metric_reduction_keeps_prefix_order_and_independent_statistics() {
        let scores = [2.0, 9.0, 4.0];
        for disabled in [None, Some(false), Some(true)] {
            let metric = QualityMetric::CVVDP {
                target_range: (3.9, 4.1),
                resolution: None,
                display_model: None,
                resize_to_display: None,
                disable_temporal: disabled,
                gpu_id: None,
            };
            for (statistic, independent) in [
                (ProbeStatistic::Mean, 5.0),
                (ProbeStatistic::Minimum, 2.0),
                (ProbeStatistic::Maximum, 9.0),
            ] {
                let score = metric.calculate_score(statistic, &scores).expect("score");
                assert_eq!(
                    score,
                    if disabled == Some(true) {
                        independent
                    } else {
                        4.0
                    }
                );
                assert_eq!(metric.score_within_target(score), disabled != Some(true));
                assert_eq!(
                    QualityMetric::default().calculate_score(statistic, &scores).expect("score"),
                    independent
                );
                if disabled != Some(true) {
                    assert!(metric.calculate_score(statistic, &[]).is_err());
                }
            }
        }
        assert_eq!(scores, [2.0, 9.0, 4.0]);
    }

    /// A `condor.json` written before `gpu_id` existed must still load, or
    /// adding the field would invalidate every configuration in the wild.
    ///
    /// `Option` fields are not implicitly defaulted by serde: a missing key is
    /// an error unless the field is absent from the struct, so this asserts the
    /// omission is genuinely tolerated rather than assumed to be.
    #[test]
    fn a_metric_without_a_gpu_id_still_deserializes() {
        let legacy =
            r#"{"SSIMULACRA2":{"target_range":[74.0,76.0],"resolution":null,"threads":null}}"#;

        let metric: QualityMetric =
            serde_json::from_str(legacy).expect("a config without gpu_id should load");

        assert!(
            matches!(metric, QualityMetric::SSIMULACRA2 { gpu_id: None, .. }),
            "an omitted gpu_id should read as unset, got {metric:?}"
        );
    }

    /// The other two GPU metrics carry the same field, and a config naming a
    /// device must keep that value through a deserialize/serialize round trip.
    #[test]
    fn a_configured_gpu_id_survives_a_round_trip() {
        for json in [
            r#"{"SSIMULACRA2":{"target_range":[74.0,76.0],"resolution":null,"threads":null,"gpu_id":1}}"#,
            r#"{"BUTTERAUGLI":{"target_range":[0.8,1.2],"resolution":null,"threads":null,"intensity_multiplier":null,"norm":null,"gpu_id":2}}"#,
            r#"{"CVVDP":{"target_range":[9.4,9.6],"resolution":null,"display_model":null,"resize_to_display":null,"disable_temporal":null,"gpu_id":3}}"#,
        ] {
            let metric: QualityMetric =
                serde_json::from_str(json).expect("a config naming a device should load");
            let round_tripped =
                serde_json::to_string(&metric).expect("the metric should serialize");

            assert_eq!(
                round_tripped, json,
                "the gpu_id must survive a round trip unchanged"
            );
        }
    }
}
