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

pub struct SSIMULACRA2 {
    /// Only used for script generation
    pub reference_node_name: String,
    /// Only used for script generation
    pub distorted_node_name: String,
}

impl PluginFunction for SSIMULACRA2 {
    const PLUGIN_NAME: &'static str = "VapourSynth Zig Image Process";
    const PLUGIN_ID: &'static str = "com.julek.vszip";
    const FUNCTION_NAME: &'static str = "SSIMULACRA2";
    const REQUIRED_ARGUMENTS: &'static [(&'static str, &'static ValueType)] =
        &[("reference", &ValueType::VideoNode), ("distorted", &ValueType::VideoNode)];
    const OPTIONAL_ARGUMENTS: &'static [(&'static str, &'static ValueType)] = &[];
}

impl SSIMULACRA2 {
    #[inline]
    pub fn invoke<'core>(
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
        let node = Self::invoke_and_get_node(core, arguments, Some("clip"))?;

        Ok(node)
    }
}

impl VapourSynthPluginScript for SSIMULACRA2 {
    #[inline]
    fn generate_script(&self, node_name: NodeVariableName) -> Result<(Option<Imports>, Vec<Line>)> {
        let mut lines = vec![];

        let line = {
            let mut line = String::new();
            write!(
                &mut line,
                "core.vszip.SSIMULACRA2(reference = {}, distorted = {}",
                self.reference_node_name, self.distorted_node_name
            )?;
            write!(&mut line, ")")?;
            line
        };

        lines.push(Line::Expression(node_name, line));

        Ok((None, lines))
    }
}
