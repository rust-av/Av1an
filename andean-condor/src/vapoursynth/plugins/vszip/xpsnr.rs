use std::fmt::Write;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use vapoursynth::{core::CoreRef, map::ValueType, node::Node};

use crate::vapoursynth::{
    plugins::PluginFunction,
    script_builder::{
        script::{Imports, Line},
        NodeVariableName,
        VapourSynthPluginScript,
    },
    VapourSynthError,
};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct XPSNR {
    /// Only used for script generation
    pub reference_node_name: String,
    /// Only used for script generation
    pub distorted_node_name: String,
    pub temporal:            Option<bool>,
    pub verbose:             Option<bool>,
}

impl PluginFunction for XPSNR {
    const PLUGIN_NAME: &'static str = "VapourSynth Zig Image Process";
    const PLUGIN_ID: &'static str = "com.julek.vszip";
    const FUNCTION_NAME: &'static str = "XPSNR";
    const REQUIRED_ARGUMENTS: &'static [(&'static str, &'static ValueType)] =
        &[("reference", &ValueType::VideoNode), ("distorted", &ValueType::VideoNode)];
    const OPTIONAL_ARGUMENTS: &'static [(&'static str, &'static ValueType)] =
        &[("temporal", &ValueType::Int), ("verbose", &ValueType::Int)];
}

impl XPSNR {
    #[inline]
    pub fn invoke<'core>(
        self,
        core: CoreRef<'core>,
        reference: &Node<'core>,
        distorted: &Node<'core>,
    ) -> Result<Node<'core>, VapourSynthError> {
        let mut arguments = Self::arguments()?;
        arguments.set_node("reference", reference).map_err(|e| {
            VapourSynthError::PluginArgumentsError {
                plugin:   Self::PLUGIN_NAME.to_owned(),
                argument: "reference".to_owned(),
                message:  e.to_string(),
            }
        })?;
        arguments.set_node("distorted", distorted).map_err(|e| {
            VapourSynthError::PluginArgumentsError {
                plugin:   Self::PLUGIN_NAME.to_owned(),
                argument: "distorted".to_owned(),
                message:  e.to_string(),
            }
        })?;
        Self::argument_set_ints(&mut arguments, vec![
            ("temporal", self.temporal.map(|b| if b { 1 } else { 0 })),
            ("verbose", self.verbose.map(|b| if b { 1 } else { 0 })),
        ])?;
        let node = Self::invoke_and_get_node(core, arguments, Some("clip"))?;

        Ok(node)
    }
}

impl VapourSynthPluginScript for XPSNR {
    #[inline]
    fn generate_script(&self, node_name: NodeVariableName) -> Result<(Option<Imports>, Vec<Line>)> {
        let mut lines = vec![];

        let line = {
            let mut line = String::new();
            write!(
                &mut line,
                "core.vszip.XPSNR(reference = {}, distorted = {}",
                self.reference_node_name, self.distorted_node_name
            )?;
            if let Some(temporal) = self.temporal {
                write!(&mut line, ", temporal = {}", temporal as i64)?;
            }
            if let Some(verbose) = self.verbose {
                write!(&mut line, ", verbose = {}", verbose as i64)?;
            }
            write!(&mut line, ")")?;
            line
        };

        lines.push(Line::Expression(node_name, line));

        Ok((None, lines))
    }
}
