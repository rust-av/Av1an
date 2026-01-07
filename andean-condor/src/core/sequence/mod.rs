use std::{
    error::Error,
    sync::{atomic::AtomicBool, Arc},
};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::{
    core::Condor,
    models::sequence::{SequenceConfigHandler, SequenceDataHandler},
};

pub mod benchmarker;
pub mod parallel_encoder;
pub mod quality_check;
pub mod scene_concatenator;
pub mod scene_detector;
pub mod serial_encoder;
pub mod target_quality;

pub trait Sequence<DataHandler, ConfigHandler>
where
    DataHandler: SequenceDataHandler,
    ConfigHandler: SequenceConfigHandler,
{
    fn details(&self) -> SequenceDetails;
    fn validate(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
    ) -> Result<((), Vec<Box<dyn Error>>)>;
    fn initialize(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        progress_tx: std::sync::mpsc::Sender<SequenceStatus>,
    ) -> Result<((), Vec<Box<dyn Error>>)>;
    fn execute(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        progress_tx: std::sync::mpsc::Sender<SequenceStatus>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<((), Vec<Box<dyn Error>>)>;
}

pub type Sequences<DataHandler, ConfigHandler>
where
    DataHandler: SequenceDataHandler,
    ConfigHandler: SequenceConfigHandler,
= Vec<Box<dyn Sequence<DataHandler, ConfigHandler>>>;

#[derive(Debug, Clone, Serialize, Deserialize, Copy)]
pub struct SequenceDetails {
    pub name:        &'static str,
    pub description: &'static str,
    pub version:     &'static str,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SequenceStatus {
    Whole(Status),
    Subprocess { parent: Status, child: Status },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Status {
    Processing {
        id:         String,
        completion: SequenceCompletion,
    },
    Completed {
        id: String,
    },
    Failed {
        id:    String,
        error: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SequenceCompletion {
    Percentage(f64),
    Scenes {
        completed: u64,
        total:     u64,
    },
    PassFrames {
        passes: (u8, u8),
        frames: (u64, u64),
    },
    Passes {
        completed: u8,
        total:     u8,
    },
    Frames {
        completed: u64,
        total:     u64,
    },
    Custom {
        name:      String,
        completed: f64,
        total:     f64,
    },
}
