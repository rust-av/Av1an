use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::condor::{
    core::processors::parallel_encoder::ParallelEncoder,
    data::{encoding::Encoder, input::Input as InputData, processing::BaseProcessorConfigTrait},
};

pub static DEFAULT_MAX_SCENE_LENGTH_SECONDS: u8 = 10;
pub static DEFAULT_MIN_SCENE_LENGTH_FRAMES: u8 = 24;

pub trait ParallelEncoderProcessing {
    fn get_workers(&self) -> Result<u8>;
    fn get_workers_mut(&mut self) -> Result<&mut u8>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParallelEncoderConfig
where
    Self: BaseProcessorConfigTrait,
{
    pub workers:          u8,
    pub scenes_directory: PathBuf,
    pub input:            Option<InputData>,
    pub encoder:          Option<Encoder>,
}

impl Default for ParallelEncoderConfig {
    #[inline]
    fn default() -> Self {
        Self {
            workers:          1, // TODO: Default to heuristics
            scenes_directory: PathBuf::new(),
            input:            None,
            encoder:          None,
        }
    }
}

impl BaseProcessorConfigTrait for ParallelEncoderConfig {
}

impl ParallelEncoderConfig {
    #[inline]
    pub fn from_parallel_encoder(parallel_encoder: &ParallelEncoder) -> Self {
        Self {
            workers:          parallel_encoder.workers,
            scenes_directory: parallel_encoder.scenes_directory.clone(),
            input:            parallel_encoder.input.as_ref().map(|input| input.as_data()),
            encoder:          parallel_encoder.encoder.clone(),
        }
    }
}
