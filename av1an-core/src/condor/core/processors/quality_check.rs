use std::sync::{self, atomic::AtomicBool, Arc};

use crate::condor::{
    data::processing::{
        quality_check::{QualityCheckDataHandler, QualityCheckProcessing},
        target_quality::QualityMetric,
        BaseProcessDataTrait,
        BaseProcessingTrait,
        BaseProcessorConfigTrait,
    },
    core::{
        input::Input,
        processors::{ProcessStatus, Processor, ProcessorDetails},
        Condor,
    },
};

static DETAILS: ProcessorDetails = ProcessorDetails {
    name:        "Quality Check",
    description: "Measure the quality of the video per scene",
    version:     "0.0.1",
};

#[derive(Default)]
pub struct QualityCheck {
    pub metric: QualityMetric,
    pub input:  Option<Input>,
}

impl<Processing, ProcessData, ProcessorConfig> Processor<Processing, ProcessData, ProcessorConfig>
    for QualityCheck
where
    Processing: BaseProcessingTrait + QualityCheckProcessing,
    ProcessData: BaseProcessDataTrait + QualityCheckDataHandler,
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
    ) -> anyhow::Result<((), Vec<Box<dyn std::error::Error>>)> {
        let warnings = vec![];

        Ok(((), warnings))
    }

    #[inline]
    fn initialize(
        &mut self,
        condor: &mut Condor<ProcessData, ProcessorConfig>,
        progress_tx: sync::mpsc::Sender<ProcessStatus>,
    ) -> anyhow::Result<((), Vec<Box<dyn std::error::Error>>)> {
        let warnings = vec![];

        Ok(((), warnings))
    }

    #[inline]
    fn process(
        &mut self,
        condor: &mut Condor<ProcessData, ProcessorConfig>,
        progress_tx: sync::mpsc::Sender<ProcessStatus>,
        cancelled: Arc<AtomicBool>,
    ) -> anyhow::Result<((), Vec<Box<dyn std::error::Error>>)> {
        let warnings = vec![];

        Ok(((), warnings))
    }
}

impl QualityCheck {
    pub const DETAILS: ProcessorDetails = DETAILS;
}
