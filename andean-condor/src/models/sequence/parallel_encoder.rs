use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::models::{
    input::Input as InputModel,
    sequence::{SequenceConfigHandler, SequenceDataHandler},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParallelEncoderConfig
where
    Self: SequenceConfigHandler,
{
    pub workers:          Option<u8>,
    pub buffer_strategy:  BufferStrategy,
    pub scenes_directory: PathBuf,
    pub input:            Option<InputModel>,
}

impl Default for ParallelEncoderConfig {
    #[inline]
    fn default() -> Self {
        Self {
            workers:          None,
            buffer_strategy:  BufferStrategy::Workers(1),
            scenes_directory: PathBuf::new(),
            input:            None,
        }
    }
}

impl ParallelEncoderConfig {
    #[inline]
    pub fn new(scenes_directory: &Path) -> Self {
        Self {
            scenes_directory: scenes_directory.to_path_buf(),
            ..Default::default()
        }
    }
}

impl SequenceConfigHandler for ParallelEncoderConfig {
}

pub trait ParallelEncoderConfigHandler
where
    Self: SequenceConfigHandler,
{
    fn parallel_encoder(&self) -> Result<&ParallelEncoderConfig>;
    fn parallel_encoder_mut(&mut self) -> Result<&mut ParallelEncoderConfig>;
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ParallelEncoderData {
    /// Must be milliseconds since UNIX Epoch
    pub started_on:   Option<u128>,
    /// Must be milliseconds since UNIX Epoch
    pub completed_on: Option<u128>,
    pub bytes:        Option<u64>,
}

pub trait ParallelEncoderDataHandler
where
    Self: SequenceDataHandler,
{
    fn get_parallel_encoder(&self) -> Result<&ParallelEncoderData>;
    fn get_parallel_encoder_mut(&mut self) -> Result<&mut ParallelEncoderData>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BufferStrategy {
    None,
    Workers(u8),
    Maximum,
}

impl BufferStrategy {
    #[inline]
    pub fn workers(&self, workers: u8) -> u8 {
        match self {
            BufferStrategy::None => workers,
            BufferStrategy::Workers(buffer) => workers + *buffer,
            BufferStrategy::Maximum => workers * 2,
        }
    }
}