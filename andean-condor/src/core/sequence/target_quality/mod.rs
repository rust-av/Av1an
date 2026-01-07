use std::{
    sync::{self, atomic::AtomicBool, Arc},
    time::SystemTime,
};

use anyhow::Result;

use crate::{
    core::{
        input::Input,
        sequence::{Sequence, SequenceDetails, SequenceStatus},
        Condor,
    },
    models::{
        encoder::{Encoder, EncoderBase},
        sequence::{
            target_quality::TargetQualityDataHandler,
            SequenceConfigHandler,
            SequenceDataHandler,
        },
    },
};

static DETAILS: SequenceDetails = SequenceDetails {
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

impl<DataHandler, ConfigHandler> Sequence<DataHandler, ConfigHandler> for TargetQuality
where
    DataHandler: SequenceDataHandler + TargetQualityDataHandler,
    ConfigHandler: SequenceConfigHandler,
{
    #[inline]
    fn details(&self) -> SequenceDetails {
        DETAILS
    }

    #[inline]
    fn validate(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
    ) -> Result<((), Vec<Box<dyn std::error::Error>>)> {
        let warnings = vec![];

        Ok(((), warnings))
    }

    #[inline]
    fn initialize(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        progress_tx: sync::mpsc::Sender<SequenceStatus>,
    ) -> Result<((), Vec<Box<dyn std::error::Error>>)> {
        let warnings = vec![];

        Ok(((), warnings))
    }

    #[inline]
    fn execute(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        progress_tx: sync::mpsc::Sender<SequenceStatus>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<((), Vec<Box<dyn std::error::Error>>)> {
        let warnings = vec![];

        Ok(((), warnings))
    }
}

impl TargetQuality {
    pub const DETAILS: SequenceDetails = DETAILS;

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
