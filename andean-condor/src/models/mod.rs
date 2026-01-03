use serde::{Deserialize, Serialize};

use crate::models::{
    encoder::Encoder,
    input::Input,
    output::Output,
    scene::Scene,
    sequence::{SequenceConfigHandler, SequenceDataHandler},
};

pub mod encoder;
pub mod input;
pub mod output;
pub mod scene;
pub mod sequence;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Condor<SequenceData, SequenceConfig>
where
    SequenceData: SequenceDataHandler,
    SequenceConfig: SequenceConfigHandler,
{
    pub input:           Input,
    pub output:          Output,
    pub encoder:         Encoder,
    pub scenes:          Vec<Scene<SequenceData>>,
    pub sequence_config: SequenceConfig,
}
