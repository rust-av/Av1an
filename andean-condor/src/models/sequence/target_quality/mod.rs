use anyhow::Result;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::models::{
    input::Input as InputModel,
    sequence::{
        SequenceConfigHandler,
        target_quality::types::{
            DEFAULT_MAXIMUM_PROBES,
            InterpolationMethod,
            QualityMetric,
            QualityPass,
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
    pub metric:          QualityMetric,
    pub maximum_probes:  u8,
    pub quantizer_range: (u32, u32),
    pub interpolators:   (InterpolationMethod, InterpolationMethod),
    pub input:           Option<InputModel>,
    pub metric_input:    Option<InputModel>,
    pub probing:         TargetQualityProbing,
    // Completion is determined by target and max probes, date from final quality pass
}

impl Default for TargetQualityConfig {
    #[inline]
    fn default() -> Self {
        Self {
            metric:          QualityMetric::default(),
            maximum_probes:  DEFAULT_MAXIMUM_PROBES,
            quantizer_range: (0, 51),
            interpolators:   (
                InterpolationMethod::FritschCarlson,
                InterpolationMethod::Pchip,
            ),
            input:           None,
            metric_input:    None,
            probing:         TargetQualityProbing::default(),
        }
    }
}

impl SequenceConfigHandler for TargetQualityConfig {
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
}

impl Default for TargetQualityData {
    #[inline]
    fn default() -> Self {
        Self {
            passes: Vec::new()
        }
    }
}

pub trait TargetQualityDataHandler {
    fn get_target_quality(&self) -> Result<&TargetQualityData>;
    fn get_target_quality_mut(&mut self) -> Result<&mut TargetQualityData>;
}
