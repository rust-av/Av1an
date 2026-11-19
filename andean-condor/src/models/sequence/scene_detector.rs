use std::{collections::BTreeMap, path::PathBuf, time::SystemTime};

use anyhow::{Result, bail};
use av_scenechange::ScenecutResult;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use strum::{Display, EnumString, IntoStaticStr};

use crate::models::{input::Input as InputModel, sequence::SequenceConfigHandler};

pub static DEFAULT_MAX_SCENE_LENGTH_SECONDS: u8 = 10;
pub static DEFAULT_MIN_SCENE_LENGTH_FRAMES: u8 = 24;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SceneDetectorData
where
    Self: Default,
{
    pub scenecut_scores: Option<BTreeMap<usize, ScenecutScore>>,
    /// Per-frame TransNetV2 transition probabilities, absolute frame index.
    pub transnetv2_scores: Option<BTreeMap<usize, f32>>,
    pub created_on: SystemTime,
}

impl Default for SceneDetectorData {
    #[inline]
    fn default() -> Self {
        Self {
            scenecut_scores: None,
            transnetv2_scores: None,
            created_on: SystemTime::now(),
        }
    }
}

pub trait SceneDetectorDataHandler {
    fn get_scene_detection(&self) -> Result<&SceneDetectorData>;
    fn get_scene_detection_mut(&mut self) -> Result<&mut SceneDetectorData>;
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct SceneDetectorConfig
where
    Self: SequenceConfigHandler,
{
    pub method: SceneDetectionMethod,
    pub input: Option<InputModel>,
}

impl SequenceConfigHandler for SceneDetectorConfig {}

// Not `Copy`: the TransNetV2 variant carries a `PathBuf` model override.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub enum SceneDetectionMethod {
    None {
        minimum_length: usize,
        maximum_length: usize,
    },
    AVSceneChange {
        minimum_length: usize,
        maximum_length: usize,
        method: ScenecutMethod,
        /// Keep the per-frame scenecut scores in `SceneDetectorData`. Off by
        /// default: the scores flood the JSON config and nothing reads them.
        save_scores: bool,
    },
    /// TransNetV2 neural network via ONNX Runtime. `model_path` pins the
    /// model file; when unset it is resolved from the standard locations.
    TransNetV2 {
        minimum_length: usize,
        maximum_length: usize,
        model_path: Option<PathBuf>,
        /// Keep the per-frame transition probabilities in `SceneDetectorData`.
        /// Off by default: they flood the JSON config and nothing reads them.
        save_scores: bool,
    },
}

impl Default for SceneDetectionMethod {
    #[inline]
    fn default() -> Self {
        Self::AVSceneChange {
            minimum_length: DEFAULT_MIN_SCENE_LENGTH_FRAMES as usize,
            maximum_length: (DEFAULT_MIN_SCENE_LENGTH_FRAMES as usize
                * DEFAULT_MAX_SCENE_LENGTH_SECONDS as usize),
            method: ScenecutMethod::Standard,
            save_scores: false,
        }
    }
}

impl SceneDetectionMethod {
    #[inline]
    pub fn minimum_length(&self) -> usize {
        match self {
            SceneDetectionMethod::AVSceneChange {
                minimum_length,
                maximum_length: _,
                method: _,
                save_scores: _,
            }
            | SceneDetectionMethod::TransNetV2 {
                minimum_length,
                maximum_length: _,
                model_path: _,
                save_scores: _,
            }
            | SceneDetectionMethod::None {
                minimum_length,
                maximum_length: _,
            } => *minimum_length,
        }
    }

    #[inline]
    pub fn set_minimum_length(&mut self, length: usize) -> Result<()> {
        match self {
            SceneDetectionMethod::AVSceneChange {
                minimum_length,
                maximum_length: _,
                method: _,
                save_scores: _,
            }
            | SceneDetectionMethod::TransNetV2 {
                minimum_length,
                maximum_length: _,
                model_path: _,
                save_scores: _,
            }
            | SceneDetectionMethod::None {
                minimum_length,
                maximum_length: _,
            } => {
                *minimum_length = length;
                Ok(())
            },
        }
    }

    #[inline]
    pub fn maximum_length(&self) -> usize {
        match self {
            SceneDetectionMethod::AVSceneChange {
                minimum_length: _,
                maximum_length,
                method: _,
                save_scores: _,
            }
            | SceneDetectionMethod::TransNetV2 {
                minimum_length: _,
                maximum_length,
                model_path: _,
                save_scores: _,
            }
            | SceneDetectionMethod::None {
                minimum_length: _,
                maximum_length,
            } => *maximum_length,
        }
    }

    /// Rejects scene length settings that cannot produce progress.
    ///
    /// A zero maximum length would leave the splitter without a chunk size,
    /// and an inverted range can never be satisfied.
    #[inline]
    pub fn validate_lengths(&self) -> Result<()> {
        let minimum_length = self.minimum_length();
        let maximum_length = self.maximum_length();
        if maximum_length == 0 {
            bail!("scene detection maximum length must be at least one frame");
        }
        if minimum_length > maximum_length {
            bail!(
                "scene detection minimum length ({minimum_length}) exceeds maximum length \
                 ({maximum_length})"
            );
        }
        Ok(())
    }

    #[inline]
    pub fn set_maximum_length(&mut self, length: usize) -> Result<()> {
        match self {
            SceneDetectionMethod::AVSceneChange {
                minimum_length: _,
                maximum_length,
                method: _,
                save_scores: _,
            }
            | SceneDetectionMethod::TransNetV2 {
                minimum_length: _,
                maximum_length,
                model_path: _,
                save_scores: _,
            }
            | SceneDetectionMethod::None {
                minimum_length: _,
                maximum_length,
            } => {
                *maximum_length = length;
                Ok(())
            },
        }
    }

    #[inline]
    pub fn method(&self) -> Result<ScenecutMethod> {
        match self {
            SceneDetectionMethod::AVSceneChange {
                minimum_length: _,
                maximum_length: _,
                method,
                save_scores: _,
            } => Ok(*method),
            _ => {
                bail!("Cannot get method for None method");
            },
        }
    }

    #[inline]
    pub fn set_method(&mut self, scenecut_method: ScenecutMethod) -> Result<()> {
        match self {
            SceneDetectionMethod::AVSceneChange {
                minimum_length: _,
                maximum_length: _,
                method,
                save_scores: _,
            } => {
                *method = scenecut_method;
                Ok(())
            },
            _ => {
                bail!("Cannot set method for None method");
            },
        }
    }
}

#[derive(
    Serialize,
    Deserialize,
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    EnumString,
    IntoStaticStr,
    Display,
    JsonSchema,
)]
pub enum ScenecutMethod {
    #[strum(serialize = "fast")]
    Fast,
    #[strum(serialize = "standard")]
    Standard,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ScenecutScore {
    pub inter_cost: f64,
    pub imp_block_cost: f64,
    pub backward_adjusted_cost: f64,
    pub forward_adjusted_cost: f64,
    pub threshold: f64,
}

impl ScenecutScore {
    #[inline]
    pub fn from_scenecutresult(result: &ScenecutResult) -> Self {
        Self {
            inter_cost: result.inter_cost,
            imp_block_cost: result.imp_block_cost,
            backward_adjusted_cost: result.backward_adjusted_cost,
            forward_adjusted_cost: result.forward_adjusted_cost,
            threshold: result.threshold,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zero_maximum_length_is_rejected() {
        let method = SceneDetectionMethod::None {
            minimum_length: 24,
            maximum_length: 0,
        };
        assert!(method.validate_lengths().is_err());
    }

    #[test]
    fn an_inverted_length_range_is_rejected() {
        let method = SceneDetectionMethod::AVSceneChange {
            minimum_length: 48,
            maximum_length: 24,
            method: ScenecutMethod::Standard,
            save_scores: false,
        };
        assert!(method.validate_lengths().is_err());
    }

    #[test]
    fn usable_lengths_are_accepted() {
        assert!(SceneDetectionMethod::default().validate_lengths().is_ok());
        assert!(
            SceneDetectionMethod::None {
                minimum_length: 0,
                maximum_length: 1,
            }
            .validate_lengths()
            .is_ok()
        );
    }
}
