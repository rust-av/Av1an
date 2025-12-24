use std::{
    sync::{self, atomic::AtomicBool, Arc},
    time::SystemTime,
};

use anyhow::Result;

use crate::condor::{
    data::{
        encoding::{Encoder, EncoderBase},
        processing::{
            target_quality::{TargetQualityDataHandler, TargetQualityProcessing},
            BaseProcessDataTrait,
            BaseProcessingTrait,
            BaseProcessorConfigTrait,
        },
    },
    core::{
        input::Input,
        processors::{ProcessStatus, Processor, ProcessorDetails},
        Condor,
    },
};

static DETAILS: ProcessorDetails = ProcessorDetails {
    name:        "Target Quality",
    description: "Determine the optimal quantizer for a given video quality metric score per \
                  scene.",
    version:     "0.0.1",
};

pub struct TargetQuality {
    pub input:      Option<Input>,
    pub encoder:    Encoder,
    pub started_at: Option<SystemTime>,
}

impl<Processing, ProcessData, ProcessorConfig> Processor<Processing, ProcessData, ProcessorConfig>
    for TargetQuality
where
    Processing: BaseProcessingTrait + TargetQualityProcessing,
    ProcessData: BaseProcessDataTrait + TargetQualityDataHandler,
    ProcessorConfig: BaseProcessorConfigTrait,
{
    #[inline]
    fn details(&self) -> ProcessorDetails {
        DETAILS
    }

    #[inline]
    fn validate(
        &mut self,
        condor: &mut Condor<ProcessData, ProcessorConfig>,
    ) -> Result<((), Vec<Box<dyn std::error::Error>>)> {
        let warnings = vec![];

        Ok(((), warnings))
    }

    #[inline]
    fn initialize(
        &mut self,
        condor: &mut Condor<ProcessData, ProcessorConfig>,
        progress_tx: sync::mpsc::Sender<ProcessStatus>,
    ) -> Result<((), Vec<Box<dyn std::error::Error>>)> {
        let warnings = vec![];

        Ok(((), warnings))
    }

    #[inline]
    fn process(
        &mut self,
        condor: &mut Condor<ProcessData, ProcessorConfig>,
        progress_tx: sync::mpsc::Sender<ProcessStatus>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<((), Vec<Box<dyn std::error::Error>>)> {
        let warnings = vec![];

        Ok(((), warnings))
    }
}

impl TargetQuality {
    pub const DETAILS: ProcessorDetails = DETAILS;
    #[inline]
    pub fn new(encoder: Encoder, input: Option<Input>) -> Self {
        Self {
            input,
            encoder,
            started_at: None,
        }
    }

    #[inline]
    pub fn get_default_cq_range(&self) -> (u32, u32) {
        default_quantizer_range(&self.encoder.base())
    }
}

impl Default for TargetQuality {
    #[inline]
    fn default() -> Self {
        Self {
            input:      None,
            encoder:    Encoder::default(),
            started_at: None,
        }
    }
}

#[inline]
pub fn default_quantizer_range(encoder: &EncoderBase) -> (u32, u32) {
    match encoder {
        EncoderBase::AOM | EncoderBase::VPX => (15, 55),
        EncoderBase::RAV1E => (50, 140),
        EncoderBase::SVTAV1 => (15, 50),
        EncoderBase::X264 | EncoderBase::X265 => (15, 35),
        EncoderBase::VVenC => todo!(),
        EncoderBase::FFmpeg => todo!(),
    }
}
