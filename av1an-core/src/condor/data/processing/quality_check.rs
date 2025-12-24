use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::condor::{
    data::processing::target_quality::{QualityMetric, QualityPass},
    core::input::Input,
};

pub trait QualityCheckProcessing {
    fn metric(&self) -> Result<QualityMetric>;
    fn metric_mut(&mut self) -> Result<&mut QualityMetric>;
    fn input(&self) -> Result<Option<Input>>;
    fn input_mut(&mut self) -> Result<&mut Option<Input>>;
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QualityCheckData {
    pub quality: QualityPass,
}

pub trait QualityCheckDataHandler {
    fn quality(&self) -> Result<&QualityPass>;
    fn quality_mut(&mut self) -> Result<&mut QualityPass>;
}
