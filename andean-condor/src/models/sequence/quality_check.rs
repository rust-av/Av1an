use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::{
    core::input::Input,
    models::sequence::target_quality::types::{QualityMetric, QualityPass},
};

pub trait QualityCheckSequence {
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
    fn get_quality_check(&self) -> Result<&QualityCheckData>;
    fn get_quality_check_mut(&mut self) -> Result<&mut QualityCheckData>;
}
