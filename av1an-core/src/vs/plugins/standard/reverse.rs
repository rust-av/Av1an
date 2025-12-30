use std::fmt::Write;

use anyhow::Result;
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

#[derive(Debug, Clone, Default)]
pub struct Reverse {}

impl PluginFunction for Reverse {
    const PLUGIN_NAME: &'static str = "std";
    const PLUGIN_ID: &'static str = "com.vapoursynth.std";
    const FUNCTION_NAME: &'static str = "Reverse";
    const REQUIRED_ARGUMENTS: &'static [(&'static str, &'static ValueType)] =
        &[("clip", &ValueType::VideoNode)];
    const OPTIONAL_ARGUMENTS: &'static [(&'static str, &'static ValueType)] = &[];
}

impl Reverse {
    #[inline]
    pub fn invoke<'core>(
        core: CoreRef<'core>,
        node: &Node<'core>,
    ) -> Result<Node<'core>, VapourSynthError> {
        let mut arguments = Self::arguments()?;
        arguments
            .set_node("clip", node)
            .map_err(|e| VapourSynthError::PluginArgumentsError {
                plugin:   Self::PLUGIN_NAME.to_owned(),
                argument: "clip".to_owned(),
                message:  e.to_string(),
            })?;
        let node = Self::invoke_and_get_node(core, arguments, Some("clip"))?;

        Ok(node)
    }
}

impl VapourSynthPluginScript for Reverse {
    #[inline]
    fn generate_script(&self, node_name: NodeVariableName) -> Result<(Option<Imports>, Vec<Line>)> {
        let mut lines = vec![];

        let line = {
            let mut line = String::new();
            write!(&mut line, "core.std.Reverse(clip = {}", node_name)?;
            write!(&mut line, ")")?;
            line
        };

        lines.push(Line::Expression(node_name, line));

        Ok((None, lines))
    }
}
