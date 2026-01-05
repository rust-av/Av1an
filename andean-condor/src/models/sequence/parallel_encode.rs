use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::models::{
    encoder::Encoder,
    input::Input as InputModel,
    sequence::{SequenceConfigHandler, SequenceDataHandler},
};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ParallelEncodeData {
    /// Must be milliseconds since UNIX Epoch
    pub started_on:   Option<u128>,
    /// Must be milliseconds since UNIX Epoch
    pub completed_on: Option<u128>,
    pub bytes:        Option<u64>,
}

pub trait ParallelEncodeDataHandler
where
    Self: SequenceDataHandler,
{
    fn get_parallel_encode(&self) -> Result<&ParallelEncodeData>;
    fn get_parallel_encode_mut(&mut self) -> Result<&mut ParallelEncodeData>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParallelEncodeConfig
where
    Self: SequenceConfigHandler,
{
    pub workers:          u8,
    pub scenes_directory: PathBuf,
    pub input:            Option<InputModel>,
    pub encoder:          Option<Encoder>,
}

impl Default for ParallelEncodeConfig {
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

impl SequenceConfigHandler for ParallelEncodeConfig {
}
