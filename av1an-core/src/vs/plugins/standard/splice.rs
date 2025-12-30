use std::fmt::Write;

use anyhow::Result;
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use vapoursynth::{core::CoreRef, map::ValueType, node::Node};

use crate::vs::{
    plugins::PluginFunction,
    script_builder::{
        script::{Imports, Line},
        NodeVariableName,
        VapourSynthPluginScript,
    },
    VapourSynthError,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Splice {
    /// Only used for script generation
    pub node_names: Vec<NodeVariableName>,
}

impl PluginFunction for Splice {
    const PLUGIN_NAME: &'static str = "std";
    const PLUGIN_ID: &'static str = "com.vapoursynth.std";
    const FUNCTION_NAME: &'static str = "Splice";
    const REQUIRED_ARGUMENTS: &'static [(&'static str, &'static ValueType)] =
        &[("clips", &ValueType::VideoNode)];
    const OPTIONAL_ARGUMENTS: &'static [(&'static str, &'static ValueType)] = &[];
}

impl Splice {
    #[inline]
    pub fn invoke<'core>(
        core: CoreRef<'core>,
        nodes: &[Node<'core>],
    ) -> Result<Node<'core>, VapourSynthError> {
        let mut arguments = Self::arguments()?;
        for node in nodes.iter() {
            arguments.append_node("clips", node).map_err(|e| {
                VapourSynthError::PluginArgumentsError {
                    plugin:   Self::PLUGIN_NAME.to_owned(),
                    argument: "clips".to_owned(),
                    message:  e.to_string(),
                }
            })?;
        }

        let node = Self::invoke_and_get_node(core, arguments, Some("clip"))?;

        Ok(node)
    }
}

impl VapourSynthPluginScript for Splice {
    #[inline]
    fn generate_script(&self, node_name: NodeVariableName) -> Result<(Option<Imports>, Vec<Line>)> {
        let mut lines = vec![];

        let line = {
            let mut line = String::new();
            write!(
                &mut line,
                "core.std.Splice(clips = {}",
                self.node_names.iter().join(", ")
            )?;

            line
        };

        lines.push(Line::Expression(node_name, line));

        Ok((None, lines))
    }
}
