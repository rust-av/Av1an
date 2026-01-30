use std::sync::{self, atomic::AtomicBool, Arc};

use crate::{
    core::{
        input::Input,
        sequence::{Sequence, SequenceDetails, SequenceStatus},
        Condor,
    },
    models::sequence::{
        quality_check::QualityCheckDataHandler,
        target_quality::types::QualityMetric,
        SequenceConfigHandler,
        SequenceDataHandler,
    },
};

static DETAILS: SequenceDetails = SequenceDetails {
    name:        "Quality Check",
    description: "Measure the quality of the video per scene",
    version:     "0.0.1",
};

#[derive(Default)]
pub struct QualityCheck {
    pub metric: QualityMetric,
    pub input:  Option<Input>,
}

impl<Data, Config> Sequence<Data, Config> for QualityCheck
where
    Data: SequenceDataHandler + QualityCheckDataHandler,
    Config: SequenceConfigHandler,
{
    #[inline]
    fn details(&self) -> SequenceDetails {
        DETAILS
    }

    #[inline]
    fn validate(
        &mut self,
        condor: &mut Condor<Data, Config>,
    ) -> anyhow::Result<((), Vec<Box<dyn std::error::Error>>)> {
        let warnings = vec![];

        Ok(((), warnings))
    }

    #[inline]
    fn initialize(
        &mut self,
        condor: &mut Condor<Data, Config>,
        progress_tx: sync::mpsc::Sender<SequenceStatus>,
    ) -> anyhow::Result<((), Vec<Box<dyn std::error::Error>>)> {
        let warnings = vec![];

        Ok(((), warnings))
    }

    #[inline]
    fn execute(
        &mut self,
        condor: &mut Condor<Data, Config>,
        progress_tx: sync::mpsc::Sender<SequenceStatus>,
        cancelled: Arc<AtomicBool>,
    ) -> anyhow::Result<((), Vec<Box<dyn std::error::Error>>)> {
        let warnings = vec![];

        Ok(((), warnings))
    }
}

impl QualityCheck {
    pub const DETAILS: SequenceDetails = DETAILS;
}
