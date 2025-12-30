use anyhow::{Ok, Result};
use serde::{Deserialize, Serialize};

use crate::condor::{
    data::processing::{
        quality_check::{QualityCheckData, QualityCheckDataHandler, QualityCheckProcessing},
        scene_detection::{
            SceneDetectionData,
            SceneDetectionDataHandler,
            SceneDetectionMethod,
            SceneDetectionProcessing,
            SceneDetectorConfig,
        },
        target_quality::{
            QualityPass,
            TargetQuality,
            TargetQualityConfig,
            TargetQualityData,
            TargetQualityDataHandler,
            TargetQualityProcessing,
        },
    },
    core::processors::scene_detector::SceneDetector,
};

pub mod quality_check;
pub mod scene_detection;
pub mod target_quality;
pub mod parallel_encoder;

pub trait BaseProcessingTrait: Default {}

pub trait BaseProcessDataTrait: Default + Clone + Serialize + Send + Sync {}

pub trait BaseProcessorConfigTrait: Default + Clone + Serialize {}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct BaseProcessorConfig {}

pub struct DefaultProcessing
where
    Self: BaseProcessingTrait
        + SceneDetectionProcessing
        + TargetQualityProcessing
        + QualityCheckProcessing,
{
    pub scene_detection: SceneDetector,
    pub target_quality:  Option<TargetQuality>,
}

impl Default for DefaultProcessing {
    #[inline]
    fn default() -> Self {
        Self {
            scene_detection: SceneDetector::default(),
            target_quality:  None,
        }
    }
}

impl BaseProcessingTrait for DefaultProcessing {
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefaultProcessorConfig
where
    Self: BaseProcessorConfigTrait,
{
    pub scene_detection: SceneDetectorConfig,
    pub target_quality:  Option<TargetQualityConfig>,
}

impl Default for DefaultProcessorConfig {
    #[inline]
    fn default() -> Self {
        Self {
            scene_detection: SceneDetectorConfig::default(),
            target_quality:  None,
        }
    }
}

impl BaseProcessorConfigTrait for DefaultProcessorConfig {
}

impl SceneDetectionProcessing for DefaultProcessing {
    #[inline]
    fn get_scene_detection_method(&self) -> Result<SceneDetectionMethod> {
        Ok(self.scene_detection.method)
    }

    #[inline]
    fn get_scene_detection_method_mut(&mut self) -> Result<&mut SceneDetectionMethod> {
        Ok(&mut self.scene_detection.method)
    }

    #[inline]
    fn get_scene_detection_input_mut(
        &mut self,
    ) -> Result<&mut Option<crate::condor::core::input::Input>> {
        Ok(&mut self.scene_detection.input)
    }

    #[inline]
    fn set_scene_detection_input(
        &mut self,
        input: crate::condor::core::input::Input,
    ) -> Result<()> {
        self.scene_detection.input = Some(input);
        Ok(())
    }
}

impl TargetQualityProcessing for DefaultProcessing {
    #[inline]
    fn target_quality_mut(&mut self) -> Result<&mut Option<TargetQuality>> {
        Ok(&mut self.target_quality)
    }
}

impl QualityCheckProcessing for DefaultProcessing {
    #[inline]
    fn metric(&self) -> Result<target_quality::QualityMetric> {
        todo!()
    }

    #[inline]
    fn metric_mut(&mut self) -> Result<&mut target_quality::QualityMetric> {
        todo!()
    }

    #[inline]
    fn input(&self) -> Result<Option<crate::condor::core::input::Input>> {
        todo!()
    }

    #[inline]
    fn input_mut(&mut self) -> Result<&mut Option<crate::condor::core::input::Input>> {
        todo!()
    }
}

pub struct DefaultProcessingData {
    pub scene_detection: SceneDetectorConfig,
    pub target_quality:  TargetQualityData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefaultProcessData
where
    Self: BaseProcessDataTrait
        + SceneDetectionDataHandler
        + TargetQualityDataHandler
        + QualityCheckDataHandler,
{
    pub scene_detection: SceneDetectionData,
    pub target_quality:  TargetQualityData,
    pub quality_check:   QualityCheckData,
}

impl BaseProcessDataTrait for DefaultProcessData {
}

impl Default for DefaultProcessData {
    #[inline]
    fn default() -> Self {
        Self {
            scene_detection: SceneDetectionData::default(),
            target_quality:  TargetQualityData::default(),
            quality_check:   QualityCheckData::default(),
        }
    }
}

impl SceneDetectionDataHandler for DefaultProcessData {
    #[inline]
    fn get_scene_detection(&self) -> Result<SceneDetectionData> {
        Ok(self.scene_detection.clone())
    }

    #[inline]
    fn get_scene_detection_mut(&mut self) -> Result<&mut SceneDetectionData> {
        Ok(&mut self.scene_detection)
    }
}

impl TargetQualityDataHandler for DefaultProcessData {
    #[inline]
    fn get_passes(&self) -> Result<&Vec<QualityPass>> {
        Ok(&self.target_quality.passes)
    }

    #[inline]
    fn get_passes_mut(&mut self) -> Result<&mut Vec<QualityPass>> {
        Ok(&mut self.target_quality.passes)
    }
}

impl QualityCheckDataHandler for DefaultProcessData {
    #[inline]
    fn quality(&self) -> Result<&QualityPass> {
        Ok(&self.quality_check.quality)
    }

    #[inline]
    fn quality_mut(&mut self) -> Result<&mut QualityPass> {
        Ok(&mut self.quality_check.quality)
    }
}
