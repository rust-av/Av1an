use std::fmt::Write;

use anyhow::Result;
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Trim {
    pub first:  Option<u32>,
    pub last:   Option<u32>,
    pub length: Option<u32>,
}

impl PluginFunction for Trim {
    const PLUGIN_NAME: &'static str = "std";
    const PLUGIN_ID: &'static str = "com.vapoursynth.std";
    const FUNCTION_NAME: &'static str = "Trim";
    const REQUIRED_ARGUMENTS: &'static [(&'static str, &'static ValueType)] =
        &[("clip", &ValueType::Node)];
    const OPTIONAL_ARGUMENTS: &'static [(&'static str, &'static ValueType)] = &[
        ("first", &ValueType::Int),
        ("last", &ValueType::Int),
        ("length", &ValueType::Int),
    ];
}

impl Trim {
    #[inline]
    pub fn invoke<'core>(
        self,
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
        Self::argument_set_ints(&mut arguments, vec![
            ("first", self.first),
            ("last", self.last),
            ("length", self.length),
        ])?;
        let node = Self::invoke_and_get_node(core, arguments, Some("clip"))?;

        Ok(node)
    }
}

impl VapourSynthPluginScript for Trim {
    #[inline]
    fn generate_script(&self, node_name: NodeVariableName) -> Result<(Option<Imports>, Vec<Line>)> {
        let mut lines = vec![];

        let line = {
            let mut line = String::new();
            write!(&mut line, "core.std.Trim(clip = {}", node_name)?;
            if let Some(first) = self.first {
                write!(&mut line, ", first = {}", first)?;
            }
            if let Some(last) = self.last {
                write!(&mut line, ", last = {}", last)?;
            }
            if let Some(length) = self.length {
                write!(&mut line, ", length = {}", length)?;
            }
            write!(&mut line, ")")?;
            line
        };

        lines.push(Line::Expression(node_name, line));

        Ok((None, lines))
    }
}
