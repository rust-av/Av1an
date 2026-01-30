use serde::{Deserialize, Serialize};

use crate::models::encoder::Encoder;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scene<SequenceData> {
    pub start_frame: usize, // Inclusive
    pub end_frame:   usize, // Exclusive
    pub sub_scenes:  Option<Vec<SubScene>>,
    pub encoder:     Encoder,
    pub sequence_data:  SequenceData,
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
pub struct SubScene {
    pub start_frame: usize, // Inclusive
    pub end_frame:   usize, // Exclusive
}
