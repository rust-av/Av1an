use anyhow::{Ok, Result};
use serde::{Deserialize, Serialize};

use crate::models::sequence::{
    parallel_encoder::{
        ParallelEncoderConfig,
        ParallelEncoderConfigHandler,
        ParallelEncoderData,
        ParallelEncoderDataHandler,
    },
    quality_check::{QualityCheckData, QualityCheckDataHandler},
    scene_concatenator::{SceneConcatenatorConfig, SceneConcatenatorConfigHandler},
    scene_detector::{SceneDetectorConfig, SceneDetectorData, SceneDetectorDataHandler},
    target_quality::{TargetQualityConfig, TargetQualityData, TargetQualityDataHandler},
};

pub mod benchmarker;
pub mod parallel_encoder;
pub mod quality_check;
pub mod scene_concatenator;
pub mod scene_detector;
pub mod target_quality;

pub trait Sequence: Default {}

pub trait SequenceDataHandler: Default + Clone + Serialize + Send + Sync {}

pub trait SequenceConfigHandler: Default + Clone + Serialize {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefaultSequenceConfig
where
    Self: SequenceConfigHandler + ParallelEncoderConfigHandler + SceneConcatenatorConfigHandler,
{
    pub scene_detector:     SceneDetectorConfig,
    pub target_quality:     Option<TargetQualityConfig>,
    pub parallel_encoder:   ParallelEncoderConfig,
    pub scene_concatenator: SceneConcatenatorConfig,
}

impl Default for DefaultSequenceConfig {
    #[inline]
    fn default() -> Self {
        Self {
            scene_detector:     SceneDetectorConfig::default(),
            target_quality:     None,
            parallel_encoder:   ParallelEncoderConfig::default(),
            scene_concatenator: SceneConcatenatorConfig::default(),
        }
    }
}

impl SequenceConfigHandler for DefaultSequenceConfig {
}

impl ParallelEncoderConfigHandler for DefaultSequenceConfig {
    #[inline]
    fn parallel_encoder(&self) -> Result<&ParallelEncoderConfig> {
        Ok(&self.parallel_encoder)
    }

    #[inline]
    fn parallel_encoder_mut(&mut self) -> Result<&mut ParallelEncoderConfig> {
        Ok(&mut self.parallel_encoder)
    }
}

impl SceneConcatenatorConfigHandler for DefaultSequenceConfig {
    #[inline]
    fn scene_concatenator(&self) -> Result<&SceneConcatenatorConfig> {
        Ok(&self.scene_concatenator)
    }

    #[inline]
    fn scene_concatenator_mut(&mut self) -> Result<&mut SceneConcatenatorConfig> {
        Ok(&mut self.scene_concatenator)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefaultSequenceData
where
    Self: SequenceDataHandler
        + SceneDetectorDataHandler
        + TargetQualityDataHandler
        + ParallelEncoderDataHandler
        // + SceneConcatenateDataHandler
        + QualityCheckDataHandler,
{
    pub scene_detection:  SceneDetectorData,
    pub target_quality:   TargetQualityData,
    pub parallel_encoder: ParallelEncoderData,
    pub quality_check:    QualityCheckData,
}

impl SequenceDataHandler for DefaultSequenceData {
}

impl SceneDetectorDataHandler for DefaultSequenceData {
    #[inline]
    fn get_scene_detection(&self) -> Result<&SceneDetectorData> {
        Ok(&self.scene_detection)
    }

    #[inline]
    fn get_scene_detection_mut(&mut self) -> Result<&mut SceneDetectorData> {
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

impl ParallelEncoderDataHandler for DefaultSequenceData {
    #[inline]
    fn get_parallel_encoder(&self) -> Result<&ParallelEncoderData> {
        Ok(&self.parallel_encoder)
    }

    #[inline]
    fn get_parallel_encoder_mut(&mut self) -> Result<&mut ParallelEncoderData> {
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
            scene_detection:  SceneDetectorData::default(),
            target_quality:   TargetQualityData::default(),
            parallel_encoder: ParallelEncoderData::default(),
            quality_check:    QualityCheckData::default(),
        }
    }
}
