use anyhow::{Ok, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::models::sequence::{
    bitrate_optimizer::{BitrateOptimizerConfig, BitrateOptimizerConfigHandler},
    noise_detector::{
        NoiseDetectorConfig, NoiseDetectorConfigHandler, NoiseDetectorData,
        NoiseDetectorDataHandler,
    },
    noise_scaler::{
        NoiseScalerConfig, NoiseScalerConfigHandler, NoiseScalerData, NoiseScalerDataHandler,
    },
    parallel_encoder::{
        ParallelEncoderConfig, ParallelEncoderConfigHandler, ParallelEncoderData,
        ParallelEncoderDataHandler,
    },
    quality_check::{
        QualityCheckConfig, QualityCheckConfigHandler, QualityCheckData, QualityCheckDataHandler,
    },
    scene_concatenator::{SceneConcatenatorConfig, SceneConcatenatorConfigHandler},
    scene_detector::{SceneDetectorConfig, SceneDetectorData, SceneDetectorDataHandler},
    target_quality::{
        TargetQualityConfig, TargetQualityConfigHandler, TargetQualityData,
        TargetQualityDataHandler,
    },
};

pub mod benchmarker;
pub mod bitrate_optimizer;
pub mod noise_detector;
pub mod noise_scaler;
pub mod parallel_encoder;
pub mod provenance;
pub mod quality_check;
pub mod scene_concatenator;
pub mod scene_detector;
pub mod speed_scaler;
pub mod target_quality;

pub trait Sequence: Default {}

pub trait SequenceDataHandler: Default + Clone + Serialize + Send + Sync {}

pub trait SequenceConfigHandler: Default + Clone + Serialize {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DefaultSequenceConfig
where
    Self: SequenceConfigHandler
        + NoiseDetectorConfigHandler
        + NoiseScalerConfigHandler
        + TargetQualityConfigHandler
        + BitrateOptimizerConfigHandler
        + ParallelEncoderConfigHandler
        + SceneConcatenatorConfigHandler,
{
    pub scene_detector: SceneDetectorConfig,
    pub noise_detector: Option<NoiseDetectorConfig>,
    pub noise_scaler: Option<NoiseScalerConfig>,
    pub target_quality: Option<TargetQualityConfig>,
    pub quality_check: Option<QualityCheckConfig>,
    pub bitrate_optimizer: BitrateOptimizerConfig,
    pub parallel_encoder: ParallelEncoderConfig,
    pub scene_concatenator: SceneConcatenatorConfig,
}

impl Default for DefaultSequenceConfig {
    #[inline]
    fn default() -> Self {
        Self {
            scene_detector: SceneDetectorConfig::default(),
            noise_detector: None,
            noise_scaler: None,
            target_quality: None,
            quality_check: None,
            bitrate_optimizer: BitrateOptimizerConfig::default(),
            parallel_encoder: ParallelEncoderConfig::default(),
            scene_concatenator: SceneConcatenatorConfig::default(),
        }
    }
}

impl SequenceConfigHandler for DefaultSequenceConfig {}

impl NoiseDetectorConfigHandler for DefaultSequenceConfig {
    #[inline]
    fn noise_detector(&self) -> Result<&Option<NoiseDetectorConfig>> {
        Ok(&self.noise_detector)
    }

    #[inline]
    fn noise_detector_mut(&mut self) -> Result<&mut Option<NoiseDetectorConfig>> {
        Ok(&mut self.noise_detector)
    }
}

impl NoiseScalerConfigHandler for DefaultSequenceConfig {
    #[inline]
    fn noise_scaler(&self) -> Result<&Option<NoiseScalerConfig>> {
        Ok(&self.noise_scaler)
    }

    #[inline]
    fn noise_scaler_mut(&mut self) -> Result<&mut Option<NoiseScalerConfig>> {
        Ok(&mut self.noise_scaler)
    }
}

impl TargetQualityConfigHandler for DefaultSequenceConfig {
    #[inline]
    fn target_quality(&self) -> Result<&Option<TargetQualityConfig>> {
        Ok(&self.target_quality)
    }

    #[inline]
    fn target_quality_mut(&mut self) -> Result<&mut Option<TargetQualityConfig>> {
        Ok(&mut self.target_quality)
    }
}

impl QualityCheckConfigHandler for DefaultSequenceConfig {
    #[inline]
    fn quality_check(&self) -> Result<&Option<QualityCheckConfig>> {
        Ok(&self.quality_check)
    }

    #[inline]
    fn quality_check_mut(&mut self) -> Result<&mut Option<QualityCheckConfig>> {
        Ok(&mut self.quality_check)
    }
}

impl BitrateOptimizerConfigHandler for DefaultSequenceConfig {
    #[inline]
    fn bitrate_optimizer(&self) -> Result<&BitrateOptimizerConfig> {
        Ok(&self.bitrate_optimizer)
    }

    #[inline]
    fn bitrate_optimizer_mut(&mut self) -> Result<&mut BitrateOptimizerConfig> {
        Ok(&mut self.bitrate_optimizer)
    }
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

    #[inline]
    fn recovery_protected_paths(&self) -> Vec<std::path::PathBuf> {
        let mut paths = self.protected_sources();
        paths.extend(self.scene_concatenator.mux_sources());
        paths.extend(self.scene_concatenator.output.iter().cloned());
        paths
    }
}

impl SceneConcatenatorConfigHandler for DefaultSequenceConfig {
    #[inline]
    fn protected_sources(&self) -> Vec<std::path::PathBuf> {
        [
            self.scene_detector.input.as_ref(),
            self.parallel_encoder.input.as_ref(),
            self.noise_detector.as_ref().and_then(|config| config.input.as_ref()),
            self.target_quality.as_ref().and_then(|config| config.input.as_ref()),
            self.target_quality.as_ref().and_then(|config| config.metric_input.as_ref()),
            self.quality_check.as_ref().and_then(|config| config.input.as_ref()),
        ]
        .into_iter()
        .flatten()
        .filter_map(crate::models::input::Input::path)
        .map(std::path::Path::to_path_buf)
        .collect()
    }

    #[inline]
    fn scene_concatenator(&self) -> Result<&SceneConcatenatorConfig> {
        Ok(&self.scene_concatenator)
    }

    #[inline]
    fn scene_concatenator_mut(&mut self) -> Result<&mut SceneConcatenatorConfig> {
        Ok(&mut self.scene_concatenator)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DefaultSequenceData
where
    Self: SequenceDataHandler
        + SceneDetectorDataHandler
        + NoiseDetectorDataHandler
        + NoiseScalerDataHandler
        + TargetQualityDataHandler
        + ParallelEncoderDataHandler
        // + SceneConcatenateDataHandler
        + QualityCheckDataHandler,
{
    pub scene_detection: SceneDetectorData,
    pub noise_detection: Option<NoiseDetectorData>,
    pub noise_scaling: Option<NoiseScalerData>,
    pub target_quality: TargetQualityData,
    pub parallel_encoder: ParallelEncoderData,
    pub quality_check: QualityCheckData,
}

impl SequenceDataHandler for DefaultSequenceData {}

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

impl NoiseDetectorDataHandler for DefaultSequenceData {
    #[inline]
    fn get_noise_detection(&self) -> Result<&Option<NoiseDetectorData>> {
        Ok(&self.noise_detection)
    }

    #[inline]
    fn get_noise_detection_mut(&mut self) -> Result<&mut Option<NoiseDetectorData>> {
        Ok(&mut self.noise_detection)
    }
}

impl NoiseScalerDataHandler for DefaultSequenceData {
    #[inline]
    fn get_noise_scaling(&self) -> Result<&Option<NoiseScalerData>> {
        Ok(&self.noise_scaling)
    }

    #[inline]
    fn get_noise_scaling_mut(&mut self) -> Result<&mut Option<NoiseScalerData>> {
        Ok(&mut self.noise_scaling)
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
            scene_detection: SceneDetectorData::default(),
            noise_detection: None,
            noise_scaling: None,
            target_quality: TargetQualityData::default(),
            parallel_encoder: ParallelEncoderData::default(),
            quality_check: QualityCheckData::default(),
        }
    }
}
