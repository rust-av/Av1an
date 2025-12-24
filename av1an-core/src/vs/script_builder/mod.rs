use anyhow::Result;

use crate::vs::script_builder::script::{Imports, Line};

pub mod script;

pub type NodeVariableName = String;

pub trait VapourSynthPluginScript {
    fn generate_script(&self, node_name: NodeVariableName) -> Result<(Option<Imports>, Vec<Line>)>;
}
