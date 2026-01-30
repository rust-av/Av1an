use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::models::{
    input::Input as InputModel,
    sequence::{
        target_quality::types::{
            InterpolationMethod,
            QualityMetric,
            QualityPass,
            TargetQualityProbing,
            DEFAULT_MAXIMUM_PROBES,
        },
        SequenceConfigHandler,
    },
};

pub mod types;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetQualityConfig
where
    Self: SequenceConfigHandler,
{
    pub metric:          QualityMetric,
    pub maximum_probes:  u8,
    pub quantizer_range: (u32, u32),
    pub interpolators:   (InterpolationMethod, InterpolationMethod),
    pub input:           Option<InputModel>,
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
            interpolators:   (InterpolationMethod::Natural, InterpolationMethod::Pchip),
            input:           None,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
