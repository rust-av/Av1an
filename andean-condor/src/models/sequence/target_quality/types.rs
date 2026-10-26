use std::{collections::HashMap, path::PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use strum::{Display, EnumString, IntoStaticStr};

use crate::{
    models::encoder::cli_parameter::CLIParameter,
    vapoursynth::plugins::vship::cvvdp::DisplayModel,
};

pub static DEFAULT_MAXIMUM_PROBES: u8 = 4;
pub static DEFAULT_VMAF_TARGET_RANGE: (f64, f64) = (94.0, 96.0);
pub static DEFAULT_SSIMULACRA2_TARGET_RANGE: (f64, f64) = (74.0, 76.0);
pub static DEFAULT_BUTTERAUGLI_TARGET_RANGE: (f64, f64) = (0.8, 1.2);
pub static DEFAULT_XPSNR_TARGET_RANGE: (f64, f64) = (44.0, 46.0);
pub static DEFAULT_CVVDP_TARGET_RANGE: (f64, f64) = (9.4, 9.6);

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
        resolution:   Option<(u32, u32)>,
        scaler:       String,
        threads:      usize,
        model:        Option<PathBuf>,
        features:     Vec<VmafFeature>,
    },
    SSIMULACRA2 {
        target_range: (f64, f64),
        resolution:   Option<(u32, u32)>,
        threads:      Option<u8>,
        gpu_id:       Option<u8>,
    },
    BUTTERAUGLI {
        target_range:         (f64, f64),
        resolution:           Option<(u32, u32)>,
        threads:              Option<u8>,
        intensity_multiplier: Option<f64>,
        norm:                 Option<u8>,
        gpu_id:               Option<u8>,
    },
    XPSNR {
        target_range: (f64, f64),
        resolution:   Option<(u32, u32)>,
    },
    CVVDP {
        target_range:      (f64, f64),
        resolution:        Option<(u32, u32)>,
        display_model:     Option<DisplayModel>,
        resize_to_display: Option<bool>,
        disable_temporal:  Option<bool>,
        gpu_id:            Option<u8>,
    },
}

impl Default for QualityMetric {
    #[inline]
    fn default() -> Self {
        Self::SSIMULACRA2 {
            target_range: DEFAULT_SSIMULACRA2_TARGET_RANGE,
            resolution:   None,
            threads:      None,
            gpu_id:       None,
        }
    }
}

impl QualityMetric {
    #[inline]
    pub fn score_within_target(&self, score: f64) -> bool {
        match self {
            QualityMetric::VMAF {
                target_range, ..
            } => score >= target_range.0 && score <= target_range.1,
            QualityMetric::SSIMULACRA2 {
                target_range, ..
            } => score >= target_range.0 && score <= target_range.1,
            QualityMetric::BUTTERAUGLI {
                target_range, ..
            } => score >= target_range.0 && score <= target_range.1,
            QualityMetric::XPSNR {
                target_range, ..
            } => score >= target_range.0 && score <= target_range.1,
            QualityMetric::CVVDP {
                target_range, ..
            } => score >= target_range.0 && score <= target_range.1,
        }
    }

    #[inline]
    pub fn target_range(&self) -> (f64, f64) {
        match self {
            QualityMetric::VMAF {
                target_range, ..
            } => *target_range,
            QualityMetric::SSIMULACRA2 {
                target_range, ..
            } => *target_range,
            QualityMetric::BUTTERAUGLI {
                target_range, ..
            } => *target_range,
            QualityMetric::XPSNR {
                target_range, ..
            } => *target_range,
            QualityMetric::CVVDP {
                target_range, ..
            } => *target_range,
        }
    }

    #[inline]
    pub fn target_range_mut(&mut self) -> &mut (f64, f64) {
        match self {
            QualityMetric::VMAF {
                target_range, ..
            } => target_range,
            QualityMetric::SSIMULACRA2 {
                target_range, ..
            } => target_range,
            QualityMetric::BUTTERAUGLI {
                target_range, ..
            } => target_range,
            QualityMetric::XPSNR {
                target_range, ..
            } => target_range,
            QualityMetric::CVVDP {
                target_range, ..
            } => target_range,
        }
    }

    #[inline]
    pub fn is_inverse_metric(&self) -> bool {
        matches!(self, QualityMetric::BUTTERAUGLI { .. })
    }

    #[inline]
    pub fn friendly_name(&self) -> &'static str {
        match self {
            QualityMetric::VMAF {
                ..
            } => "VMAF",
            QualityMetric::SSIMULACRA2 {
                ..
            } => "SSIMULACRA2",
            QualityMetric::BUTTERAUGLI {
                ..
            } => "butteraugli",
            QualityMetric::XPSNR {
                ..
            } => "XPSNR",
            QualityMetric::CVVDP {
                ..
            } => "CVVDP",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct TargetQualityProbing {
    pub encoder_options: Option<HashMap<String, CLIParameter>>,
    pub strategy:        ProbeStrategy,
    pub statistic:       ProbeStatistic,
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
        length:   SubsetProbeLength,
    },
}

impl ProbeStrategy {
    #[inline]
    pub fn frame_indices(&self, start: usize, end: usize) -> Vec<usize> {
        match self {
            ProbeStrategy::Whole => (start..end).collect::<Vec<_>>(),
            ProbeStrategy::Skip {
                skip,
            } => (start..end)
                .filter(|index| {
                    (index - start) == 0 || (index - start).is_multiple_of(*skip as usize)
                })
                .collect::<Vec<_>>(),
            ProbeStrategy::Subset {
                position,
                length,
            } => {
                let max_length = end - start;
                let length = match length {
                    SubsetProbeLength::Percentage(percentage) => {
                        (percentage * max_length as f64).round() as usize
                    },
                    SubsetProbeLength::Frames(frames) => (*frames as usize).min(max_length),
                };
                match position {
                    SubsetProbePosition::Start => {
                        (start..(start + length).min(end)).collect::<Vec<_>>()
                    },
                    SubsetProbePosition::Middle => {
                        let middle = start + (max_length / 2);
                        let middle_start = (middle - (length / 2)).max(start);
                        let middle_end = (middle + (length as f64 / 2.0).ceil() as usize).min(end);
                        (middle_start..middle_end).collect::<Vec<_>>()
                    },
                    SubsetProbePosition::End => {
                        ((end - length).max(start)..end).collect::<Vec<_>>()
                    },
                }
            },
        }
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
    Percentage(f64),
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
    #[inline]
    pub fn calculate(&self, scores: &[f64]) -> f64 {
        let mut scores = scores.to_vec();
        scores.sort_by(f64::total_cmp);
        match self {
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
                let sum_reciprocals: f64 = scores.iter().map(|&x| 1.0 / x).sum();
                scores.len() as f64 / sum_reciprocals
            },
            ProbeStatistic::Percentile(index) => *scores
                .get((*index / 100.0 * scores.len() as f64) as usize)
                .unwrap_or(&scores[0]),
            ProbeStatistic::StandardDeviationDistance {
                sigma,
            } => {
                let average = ProbeStatistic::Mean.calculate(&scores);
                let minimum = ProbeStatistic::Minimum.calculate(&scores);
                let maximum = ProbeStatistic::Maximum.calculate(&scores);
                let variance = scores
                    .iter()
                    .map(|x| {
                        let diff = x - average;
                        diff * diff
                    })
                    .sum::<f64>()
                    / scores.len() as f64;
                (average + (sigma * variance.sqrt())).clamp(minimum, maximum)
            },
            ProbeStatistic::Mode => {
                let mut counts = HashMap::new();
                for score in &scores {
                    // Round to nearest integer for fewer unique buckets
                    let rounded_score = score.round() as i32;
                    *counts.entry(rounded_score).or_insert(0) += 1;
                }
                let max_count = counts.values().copied().max().unwrap_or(0);
                *scores
                    .iter()
                    .find(|score| counts[&(score.round() as i32)] == max_count)
                    .unwrap_or(&0.0)
            },
            ProbeStatistic::Minimum => *scores
                .iter()
                .min_by(|a, b| a.partial_cmp(b).expect("should not have NaN in scores set"))
                .unwrap_or(&0.0),
            ProbeStatistic::Maximum => *scores
                .iter()
                .max_by(|a, b| a.partial_cmp(b).expect("should not have NaN in scores set"))
                .unwrap_or(&0.0),
            ProbeStatistic::RootMeanSquare => {
                let sum_of_squares: f64 = scores.iter().map(|&x| x * x).sum();
                (sum_of_squares / scores.len() as f64).sqrt()
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct QualityPass {
    pub quantizer:    f64,
    pub scores:       Vec<f64>,
    pub bitrate:      f64,
    /// Must be in milliseconds since UNIX Epoch
    pub started_on:   u128,
    /// Must be in milliseconds since UNIX Epoch
    pub completed_on: u128,
}

impl Default for QualityPass {
    #[inline]
    fn default() -> Self {
        Self {
            quantizer:    0.0,
            scores:       Vec::new(),
            bitrate:      0.0,
            started_on:   0,
            completed_on: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::QualityMetric;

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
            matches!(metric, QualityMetric::SSIMULACRA2 {
                gpu_id: None,
                ..
            }),
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
