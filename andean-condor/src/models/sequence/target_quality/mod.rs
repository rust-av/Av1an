use std::{collections::HashMap, path::PathBuf, thread::available_parallelism, time::SystemTime};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use strum::{Display, EnumString, IntoStaticStr};

use crate::{
    core::input::Input,
    models::{
        encoder::cli_parameter::CLIParameter,
        input::Input as InputModel,
        sequence::SequenceConfigHandler,
    },
};

pub static DEFAULT_MAXIMUM_PROBES: u8 = 4;
pub static DEFAULT_VMAF_TARGET_RANGE: (f64, f64) = (94.0, 96.0);

pub struct TargetQuality {
    pub metric:          QualityMetric,
    pub maximum_probes:  u8,
    pub quantizer_range: (u32, u32),
    pub interpolators:   (InterpolationMethod, InterpolationMethod),
    pub input:           Option<Input>,
    pub probing:         TargetQualityProbing,
    // Completion is determined by target and max probes, date from final quality pass
}

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

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString, IntoStaticStr,
)]
pub enum InterpolationMethod {
    #[strum(serialize = "linear")]
    Linear,
    #[strum(serialize = "quadratic")]
    Quadratic,
    #[strum(serialize = "natural")]
    Natural,
    #[strum(serialize = "pchip")]
    Pchip,
    #[strum(serialize = "catmull")]
    Catmull,
    #[strum(serialize = "akima")]
    Akima,
    #[strum(serialize = "cubicpolynomial")]
    CubicPolynomial,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, EnumString, IntoStaticStr, Display)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QualityMetric {
    VMAF {
        target_range: (f64, f64),
        resolution:   Option<(u32, u32)>,
        scaler:       String,
        filter:       Option<String>,
        threads:      usize,
        model:        Option<PathBuf>,
        features:     Vec<VmafFeature>,
    },
    SSIMULACRA2 {
        target_range: (f64, f64),
        resolution:   Option<(u32, u32)>,
        threads:      Option<u8>,
    },
    BUTTERAUGLI {
        target_range:         (f64, f64),
        resolution:           Option<(u32, u32)>,
        threads:              Option<u8>,
        intensity_multiplier: Option<f64>,
    },
    XPSNR {
        target_range: (f64, f64),
        resolution:   Option<(u32, u32)>,
        threads:      Option<u8>,
    },
}

impl Default for QualityMetric {
    #[inline]
    fn default() -> Self {
        QualityMetric::VMAF {
            target_range: DEFAULT_VMAF_TARGET_RANGE,
            resolution:   None,
            scaler:       String::from("bicubic"),
            filter:       None,
            threads:      available_parallelism()
                .expect("Unrecoverable. Failed to get thread count")
                .get(),
            model:        None,
            features:     vec![VmafFeature::Default],
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TargetQualityProbing {
    encoder_options: HashMap<String, CLIParameter>,
    strategy:        ProbeStategy,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub enum ProbeStategy {
    #[default]
    Whole,
    Skip {
        skip: u32,
    },
    Subset {
        position: SubsetProbePosition,
        strategy: SubsetProbeLength,
    },
    Exact {
        frames: Vec<u32>,
    },
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub enum SubsetProbePosition {
    Start,
    #[default]
    Middle,
    End,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityPass {
    quantizer:    f64,
    scores:       Vec<f64>,
    bitrate:      f64,
    start_time:   SystemTime,
    completed_on: SystemTime,
}

impl Default for QualityPass {
    #[inline]
    fn default() -> Self {
        Self {
            quantizer:    0.0,
            scores:       Vec::new(),
            bitrate:      0.0,
            start_time:   SystemTime::now(),
            completed_on: SystemTime::now(),
        }
    }
}

// pub struct QualityPassOptions {
//     metric: QualityMetric,
//     input:  Option<Input>,
// }

pub trait TargetQualityDataHandler {
    fn get_target_quality(&self) -> Result<&TargetQualityData>;
    fn get_target_quality_mut(&mut self) -> Result<&mut TargetQualityData>;
}
