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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RFS {
    /// Only used for script generation
    pub clip_a_name: String,
    /// Only used for script generation
    pub frames:      Vec<u32>,
    pub clip_b_name: String,
    pub planes:      Option<Vec<u32>>,
    pub mismatch:    Option<bool>,
}

impl PluginFunction for RFS {
    const PLUGIN_NAME: &'static str = "VapourSynth Zig Image Process";
    const PLUGIN_ID: &'static str = "com.julek.vszip";
    const FUNCTION_NAME: &'static str = "RFS";
    const REQUIRED_ARGUMENTS: &'static [(&'static str, &'static ValueType)] =
        &[("clip", &ValueType::VideoNode), ("planes", &ValueType::Int)];
    const OPTIONAL_ARGUMENTS: &'static [(&'static str, &'static ValueType)] = &[
        ("hradius", &ValueType::Int),
        ("hpasses", &ValueType::Int),
        ("vradius", &ValueType::Int),
        ("vpasses", &ValueType::Int),
    ];
}

impl RFS {
    #[inline]
    pub fn invoke<'core>(
        self,
        core: CoreRef<'core>,
        clip_a: &Node<'core>,
        clip_b: &Node<'core>,
    ) -> Result<Node<'core>, VapourSynthError> {
        let mut arguments = Self::arguments()?;
        arguments.set_node("clipa", clip_a).map_err(|e| {
            VapourSynthError::PluginArgumentsError {
                plugin:   Self::PLUGIN_NAME.to_owned(),
                argument: "clipa".to_owned(),
                message:  e.to_string(),
            }
        })?;
        arguments.set_node("clipb", clip_b).map_err(|e| {
            VapourSynthError::PluginArgumentsError {
                plugin:   Self::PLUGIN_NAME.to_owned(),
                argument: "clipb".to_owned(),
                message:  e.to_string(),
            }
        })?;
        Self::argument_set_ints(&mut arguments, vec![(
            "mismatch",
            self.mismatch.map(|b| if b { 1 } else { 0 }),
        )])?;
        Self::argument_set_int_arrays(&mut arguments, vec![
            ("frames", Some(self.frames)),
            ("planes", self.planes),
        ])?;
        let node = Self::invoke_and_get_node(core, arguments, Some("clip"))?;

        Ok(node)
    }
}

impl VapourSynthPluginScript for RFS {
    #[inline]
    fn generate_script(&self, node_name: NodeVariableName) -> Result<(Option<Imports>, Vec<Line>)> {
        let mut lines = vec![];

        let line = {
            let mut line = String::new();
            write!(
                &mut line,
                "core.vszip.RFS(clipa = {}, clipb = {}",
                self.clip_a_name, self.clip_b_name
            )?;
            write!(&mut line, ", frames = [{}]", self.frames.iter().join(", "))?;
            if let Some(planes) = &self.planes {
                write!(&mut line, ", planes = [{}]", planes.iter().join(", "))?;
            }
            if let Some(mismatch) = self.mismatch {
                write!(&mut line, ", mismatch = {}", mismatch as i64)?;
            }
            write!(&mut line, ")")?;
            line
        };

        lines.push(Line::Expression(node_name, line));

        Ok((None, lines))
    }
}
