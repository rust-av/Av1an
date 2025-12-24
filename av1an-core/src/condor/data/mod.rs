use serde::{Deserialize, Serialize};

use crate::condor::data::{
    encoding::Encoder,
    input::Input,
    output::Output,
    processing::{BaseProcessDataTrait, BaseProcessorConfigTrait},
    scene::Scene,
};

pub mod encoding;
pub mod input;
pub mod output;
pub mod processing;
pub mod scene;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Condor<ProcessData, ProcessorConfig>
where
    ProcessData: BaseProcessDataTrait,
    ProcessorConfig: BaseProcessorConfigTrait,
{
    pub input:            Input,
    pub output:           Output,
    pub encoder:          Encoder,
    pub scenes:           Vec<Scene<ProcessData>>,
    pub processor_config: ProcessorConfig,
}
