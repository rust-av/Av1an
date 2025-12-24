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
pub struct SelectEvery {
    pub cycle:           u32,
    pub offsets:         Vec<u32>,
    pub modify_duration: Option<bool>,
}

impl PluginFunction for SelectEvery {
    const PLUGIN_NAME: &'static str = "std";
    const PLUGIN_ID: &'static str = "com.vapoursynth.std";
    const FUNCTION_NAME: &'static str = "SelectEvery";
    const REQUIRED_ARGUMENTS: &'static [(&'static str, &'static ValueType)] = &[
        ("clip", &ValueType::Node),
        ("cycle", &ValueType::Int),
        ("offsets", &ValueType::Int),
    ];
    const OPTIONAL_ARGUMENTS: &'static [(&'static str, &'static ValueType)] =
        &[("modify_duration", &ValueType::Int)];
}

impl SelectEvery {
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
            ("cycle", Some(self.cycle as i64)),
            (
                "modify_duration",
                self.modify_duration.map(|b| if b { 1 } else { 0 }),
            ),
        ])?;
        Self::argument_set_int_arrays(&mut arguments, vec![("offsets", Some(self.offsets))])?;
        let node = Self::invoke_and_get_node(core, arguments, Some("clip"))?;

        Ok(node)
    }
}

impl VapourSynthPluginScript for SelectEvery {
    #[inline]
    fn generate_script(&self, node_name: NodeVariableName) -> Result<(Option<Imports>, Vec<Line>)> {
        let mut lines = vec![];

        let line = {
            let mut line = String::new();
            write!(&mut line, "core.std.SelectEvery(clip = {}", node_name)?;
            write!(&mut line, ", cycle = {}", self.cycle)?;
            write!(
                &mut line,
                ", offsets = [{}]",
                self.offsets.iter().join(", ")
            )?;
            if let Some(modify_duration) = self.modify_duration {
                write!(
                    &mut line,
                    ", modify_duration = {}",
                    if modify_duration { 1 } else { 0 }
                )?;
            }
            write!(&mut line, ")")?;
            line
        };

        lines.push(Line::Expression(node_name, line));

        Ok((None, lines))
    }
}
