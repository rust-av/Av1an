use anyhow::{Ok, Result};
use serde::{Deserialize, Serialize};

use crate::models::sequence::{
    parallel_encode::{ParallelEncodeData, ParallelEncodeDataHandler},
    quality_check::{QualityCheckData, QualityCheckDataHandler},
    scene_concatenate::SceneConcatenateConfig,
    scene_detect::{SceneDetectConfig, SceneDetectData, SceneDetectDataHandler},
    target_quality::{TargetQualityConfig, TargetQualityData, TargetQualityDataHandler},
};

pub mod parallel_encode;
pub mod quality_check;
pub mod scene_concatenate;
pub mod scene_detect;
pub mod target_quality;

pub trait Sequence: Default {}

pub trait SequenceDataHandler: Default + Clone + Serialize + Send + Sync {}

pub trait SequenceConfigHandler: Default + Clone + Serialize {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefaultSequenceConfig
where
    Self: SequenceConfigHandler,
{
    pub scene_detection:     SceneDetectConfig,
    pub target_quality:      Option<TargetQualityConfig>,
    pub scene_concatenation: SceneConcatenateConfig,
}

impl Default for DefaultSequenceConfig {
    #[inline]
    fn default() -> Self {
        Self {
            scene_detection:     SceneDetectConfig::default(),
            target_quality:      None,
            scene_concatenation: SceneConcatenateConfig::default(),
        }
    }
}

impl SequenceConfigHandler for DefaultSequenceConfig {
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefaultSequenceData
where
    Self: SequenceDataHandler
        + SceneDetectDataHandler
        + TargetQualityDataHandler
        + ParallelEncodeDataHandler
        // + SceneConcatenateDataHandler
        + QualityCheckDataHandler,
{
    pub scene_detection:  SceneDetectData,
    pub target_quality:   TargetQualityData,
    pub parallel_encoder: ParallelEncodeData,
    pub quality_check:    QualityCheckData,
}

impl SequenceDataHandler for DefaultSequenceData {
}

impl SceneDetectDataHandler for DefaultSequenceData {
    #[inline]
    fn get_scene_detection(&self) -> Result<&SceneDetectData> {
        Ok(&self.scene_detection)
    }

    #[inline]
    fn get_scene_detection_mut(&mut self) -> Result<&mut SceneDetectData> {
        Ok(&mut self.scene_detection)
    }
}

impl TargetQualityDataHandler for DefaultSequenceData {
    #[inline]
    fn get_target_quality(&self) -> Result<&TargetQualityData> {
        Ok(&self.target_quality)
    }

    #[inline]
    fn get_target_quality_mut(&mut self) -> Result<&mut TargetQualityData> {
        Ok(&mut self.target_quality)
    }
}

impl ParallelEncodeDataHandler for DefaultSequenceData {
    #[inline]
    fn get_parallel_encode(&self) -> Result<&ParallelEncodeData> {
        Ok(&self.parallel_encoder)
    }

    #[inline]
    fn get_parallel_encode_mut(&mut self) -> Result<&mut ParallelEncodeData> {
        Ok(&mut self.parallel_encoder)
    }
}

impl QualityCheckDataHandler for DefaultSequenceData {
    #[inline]
    fn get_quality_check(&self) -> Result<&QualityCheckData> {
        Ok(&self.quality_check)
    }

    #[inline]
    fn get_quality_check_mut(&mut self) -> Result<&mut QualityCheckData> {
        Ok(&mut self.quality_check)
    }
}

impl Default for DefaultSequenceData {
    #[inline]
    fn default() -> Self {
        Self {
            scene_detection:  SceneDetectData::default(),
            target_quality:   TargetQualityData::default(),
            parallel_encoder: ParallelEncodeData::default(),
            quality_check:    QualityCheckData::default(),
        }
    }
}
