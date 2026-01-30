use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::models::sequence::SequenceConfigHandler;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkerConfig
where
    Self: SequenceConfigHandler,
{
    /// Percent of speed increase required to add an additional worker
    pub threshold:  u8,
    /// The maximum amount of RAM (in megabytes) allowed across all workers
    pub max_memory: Option<u32>,
    // pub scenes_directory: PathBuf,
    // pub input:            Option<InputModel>,
    // pub encoder:          Option<Encoder>,
}

impl Default for BenchmarkerConfig {
    #[inline]
    fn default() -> Self {
        Self {
            threshold:  5,
            max_memory: None,
        }
    }
}

impl SequenceConfigHandler for BenchmarkerConfig {
}

pub trait BenchmarkerConfigHandler
where
    Self: SequenceConfigHandler,
{
    fn benchmarker(&self) -> Result<&BenchmarkerConfig>;
    fn benchmarker_mut(&mut self) -> Result<&mut BenchmarkerConfig>;
}
