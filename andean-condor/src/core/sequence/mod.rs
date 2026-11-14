use std::sync::{Arc, atomic::AtomicBool};

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
    fn initialize(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        progress_tx: std::sync::mpsc::Sender<SequenceStatus>,
    ) -> Result<((), Vec<anyhow::Error>)>;
    fn execute(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        progress_tx: std::sync::mpsc::Sender<SequenceStatus>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<((), Vec<anyhow::Error>)>;

    /// Explicit shared cancellation. Legacy implementations are graceful-only;
    /// child-owning implementations override this entry point.
    #[inline]
    fn execute_cancel(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        progress_tx: std::sync::mpsc::Sender<SequenceStatus>,
        cancellation: Cancellation,
    ) -> Result<((), Vec<anyhow::Error>)> {
        cancellation.check()?;
        let result = self.execute(condor, progress_tx, cancellation.legacy_flag());
        cancellation.check_forced()?;
        result
    }

    #[inline]
    fn initialize_cancel(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        progress_tx: std::sync::mpsc::Sender<SequenceStatus>,
        cancellation: &Cancellation,
    ) -> Result<((), Vec<anyhow::Error>)> {
        cancellation.check()?;
        let result = self.initialize(condor, progress_tx);
        cancellation.check_forced()?;
        result
    }
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
