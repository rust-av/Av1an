use std::{
    error::Error,
    sync::{atomic::AtomicBool, Arc},
};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::condor::{
    data::processing::{
        BaseProcessDataTrait,
        BaseProcessingTrait,
        BaseProcessorConfigTrait,
        DefaultProcessData,
        DefaultProcessing,
        DefaultProcessorConfig,
    },
    core::{
        processors::{
            quality_check::QualityCheck,
            scene_detector::SceneDetector,
            target_quality::TargetQuality,
        },
        Condor,
    },
};

pub mod parallel_encoder;
pub mod quality_check;
pub mod scene_concatenator;
pub mod scene_detector;
pub mod serial_encoder;
pub mod target_quality;

pub trait Processor<Processing, ProcessData, ProcessorConfig>
where
    Processing: BaseProcessingTrait,
    ProcessData: BaseProcessDataTrait,
    ProcessorConfig: BaseProcessorConfigTrait,
{
    fn details(&self) -> ProcessorDetails;
    fn validate(
        &mut self,
        condor: &mut Condor<ProcessData, ProcessorConfig>,
    ) -> Result<((), Vec<Box<dyn Error>>)>;
    fn initialize(
        &mut self,
        condor: &mut Condor<ProcessData, ProcessorConfig>,
        progress_tx: std::sync::mpsc::Sender<ProcessStatus>,
    ) -> Result<((), Vec<Box<dyn Error>>)>;
    fn process(
        &mut self,
        condor: &mut Condor<ProcessData, ProcessorConfig>,
        progress_tx: std::sync::mpsc::Sender<ProcessStatus>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<((), Vec<Box<dyn Error>>)>;
}

pub type Processors<Processing, ProcessData, ProcessorConfig>
where
    Processing: BaseProcessingTrait,
    ProcessData: BaseProcessDataTrait,
    ProcessorConfig: BaseProcessorConfigTrait,
= Vec<Box<dyn Processor<Processing, ProcessData, ProcessorConfig>>>;

#[derive(Debug, Clone, Serialize, Deserialize, Copy)]
pub struct ProcessorDetails {
    pub name:        &'static str,
    pub description: &'static str,
    pub version:     &'static str,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProcessStatus {
    Whole(Status),
    Subprocess { parent: Status, child: Status },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Status {
    Processing {
        id:         String,
        completion: ProcessCompletion,
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
pub enum ProcessCompletion {
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

#[inline]
pub fn default_preprocessors(
) -> Processors<DefaultProcessing, DefaultProcessData, DefaultProcessorConfig> {
    vec![
        Box::new(SceneDetector::default())
            as Box<dyn Processor<DefaultProcessing, DefaultProcessData, DefaultProcessorConfig>>,
        Box::new(TargetQuality::default())
            as Box<dyn Processor<DefaultProcessing, DefaultProcessData, DefaultProcessorConfig>>,
        Box::new(QualityCheck::default())
            as Box<dyn Processor<DefaultProcessing, DefaultProcessData, DefaultProcessorConfig>>,
    ]
}
