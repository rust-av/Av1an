use crate::utils::cancellation::Cancellation;
use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::{
    core::Condor,
    models::sequence::{SequenceConfigHandler, SequenceDataHandler},
};

pub mod benchmarker;
pub mod bitrate_optimizer;
pub mod ffmpeg_filter;
pub mod noise_detector;
pub mod noise_scaler;
pub mod parallel_encoder;
pub mod quality_check;
pub mod scene_concatenator;
pub mod scene_detector;
pub mod serial_encoder;
pub mod speed_scaler;
pub mod target_quality;
pub mod zone_encoder;

pub trait Sequence<DataHandler, ConfigHandler>
where
    DataHandler: SequenceDataHandler,
    ConfigHandler: SequenceConfigHandler,
{
    fn details(&self) -> SequenceDetails;
    fn validate(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
    ) -> Result<((), Vec<anyhow::Error>)>;
    /// Prepare a sequence. Check the job token before opening inputs or creating
    /// outputs, and report forced cancellation even if preparation succeeded.
    fn initialize(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        progress_tx: std::sync::mpsc::Sender<SequenceStatus>,
        cancellation: &Cancellation,
    ) -> Result<((), Vec<anyhow::Error>)>;

    /// Run a sequence using the job token. Stop admitting work on graceful
    /// cancellation. On force, join workers, reap owned children, and return
    /// [`Cancelled::Forced`](crate::utils::cancellation::Cancelled::Forced).
    fn execute(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        progress_tx: std::sync::mpsc::Sender<SequenceStatus>,
        cancellation: Cancellation,
    ) -> Result<((), Vec<anyhow::Error>)>;
}

/// Guard a cooperative stage without replacing its result on a graceful request.
fn check_sequence<T>(cancellation: &Cancellation, run: impl FnOnce() -> Result<T>) -> Result<T> {
    cancellation.check()?;
    let result = run();
    cancellation.check_forced()?;
    result
}

#[allow(type_alias_bounds)]
pub type Sequences<DataHandler, ConfigHandler>
where
    DataHandler: SequenceDataHandler,
    ConfigHandler: SequenceConfigHandler,
= Vec<Box<dyn Sequence<DataHandler, ConfigHandler>>>;

#[derive(Debug, Clone, Serialize, Deserialize, Copy)]
pub struct SequenceDetails {
    pub name: &'static str,
    pub description: &'static str,
    pub version: &'static str,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SequenceStatus {
    Whole(Status),
    Subprocess { parent: Status, child: Status },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Status {
    Processing {
        id: String,
        completion: SequenceCompletion,
    },
    Completed {
        id: String,
    },
    Failed {
        id: String,
        error: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SequenceCompletion {
    Percentage(f64),
    Scenes {
        completed: u64,
        total: u64,
    },
    PassFrames {
        passes: (u8, u8),
        frames: (u64, u64),
    },
    Passes {
        completed: u8,
        total: u8,
    },
    Frames {
        completed: u64,
        total: u64,
    },
    SceneQuality {
        index: u64,
        quantizer: f64,
        score: f64,
        bitrate: f64,
    },
    FrameScore {
        frame: u64,
        score: f64,
    },
    Custom {
        name: String,
        completed: f64,
        total: f64,
    },
}

#[cfg(test)]
mod tests;
