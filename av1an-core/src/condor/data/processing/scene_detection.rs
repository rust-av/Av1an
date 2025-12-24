use std::{collections::BTreeMap, time::SystemTime};

use anyhow::Result;
use av_scenechange::ScenecutResult;
use serde::{Deserialize, Serialize};

use crate::{
    condor::{
        data::{input::Input as InputData, processing::BaseProcessorConfigTrait},
        core::{input::Input, processors::scene_detector::SceneDetector},
    },
    ScenecutMethod,
};

pub static DEFAULT_MAX_SCENE_LENGTH_SECONDS: u8 = 10;
pub static DEFAULT_MIN_SCENE_LENGTH_FRAMES: u8 = 24;

pub trait SceneDetectionProcessing {
    fn get_scene_detection_method(&self) -> Result<SceneDetectionMethod>;
    fn get_scene_detection_method_mut(&mut self) -> Result<&mut SceneDetectionMethod>;
    fn get_scene_detection_input_mut(&mut self) -> Result<&mut Option<Input>>;
    fn set_scene_detection_input(&mut self, input: Input) -> Result<()>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneDetectionData
where
    Self: Default,
{
    // pub scenecut_scores: Option<BTreeMap<usize, ScenecutResult>>,
    pub scenecut_scores: Option<BTreeMap<usize, ScenecutScore>>,
    pub created_on:      SystemTime,
}

impl Default for SceneDetectionData {
    #[inline]
    fn default() -> Self {
        Self {
            scenecut_scores: None,
            created_on:      SystemTime::now(),
        }
    }
}

pub trait SceneDetectionDataHandler {
    fn get_scene_detection(&self) -> Result<SceneDetectionData>;
    fn get_scene_detection_mut(&mut self) -> Result<&mut SceneDetectionData>;
    // fn set_scene_detection(&mut self, scene_detection: SceneDetectionData) ->
    // Result<()>;
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SceneDetectorConfig
where
    Self: BaseProcessorConfigTrait,
{
    pub method: SceneDetectionMethod,
    pub input:  Option<InputData>,
}

impl BaseProcessorConfigTrait for SceneDetectorConfig {
}

impl SceneDetectorConfig {
    #[inline]
    pub fn from_scene_detector(scene_detector: &SceneDetector) -> Self {
        Self {
            method: scene_detector.method,
            input:  scene_detector.input.as_ref().map(|input| input.as_data()),
        }
    }
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub enum SceneDetectionMethod {
    None {
        minimum_length: usize,
        maximum_length: usize,
    },
    AVSceneChange {
        minimum_length: usize,
        maximum_length: usize,
        method:         ScenecutMethod,
    },
}

impl Default for SceneDetectionMethod {
    #[inline]
    fn default() -> Self {
        Self::AVSceneChange {
            minimum_length: DEFAULT_MIN_SCENE_LENGTH_FRAMES as usize,
            maximum_length: (DEFAULT_MIN_SCENE_LENGTH_FRAMES as usize
                * DEFAULT_MAX_SCENE_LENGTH_SECONDS as usize),
            method:         ScenecutMethod::Standard,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenecutScore {
    pub inter_cost:             f64,
    pub imp_block_cost:         f64,
    pub backward_adjusted_cost: f64,
    pub forward_adjusted_cost:  f64,
    pub threshold:              f64,
}

impl ScenecutScore {
    #[inline]
    pub fn from_scenecutresult(result: &ScenecutResult) -> Self {
        Self {
            inter_cost:             result.inter_cost,
            imp_block_cost:         result.imp_block_cost,
            backward_adjusted_cost: result.backward_adjusted_cost,
            forward_adjusted_cost:  result.forward_adjusted_cost,
            threshold:              result.threshold,
        }
    }
}
