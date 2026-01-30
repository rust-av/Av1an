use std::fmt::Write;

use anyhow::Result;
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

pub struct AssumeFPS {
    /// Only used for script generation
    pub source_node_name: Option<String>,
    /// Only used for script generation
    pub fps:              Option<(u32, u32)>,
}

impl PluginFunction for AssumeFPS {
    const PLUGIN_NAME: &'static str = "std";
    const PLUGIN_ID: &'static str = "com.vapoursynth.std";
    const FUNCTION_NAME: &'static str = "AssumeFPS";
    const REQUIRED_ARGUMENTS: &'static [(&'static str, &'static ValueType)] =
        &[("clip", &ValueType::VideoNode)];
    const OPTIONAL_ARGUMENTS: &'static [(&'static str, &'static ValueType)] = &[
        ("first", &ValueType::Int),
        ("last", &ValueType::Int),
        ("length", &ValueType::Int),
    ];
}

impl AssumeFPS {
    #[inline]
    pub fn invoke<'core>(
        core: CoreRef<'core>,
        node: &Node<'core>,
        options: &AssumeFPSOptions<'core>,
    ) -> Result<Node<'core>, VapourSynthError> {
        let mut arguments = Self::arguments()?;
        arguments
            .set_node("clip", node)
            .map_err(|e| VapourSynthError::PluginArgumentsError {
                plugin:   Self::PLUGIN_NAME.to_owned(),
                argument: "clip".to_owned(),
                message:  e.to_string(),
            })?;

        match options {
            AssumeFPSOptions::Source(node) => {
                arguments.set_node("src", node).map_err(|e| {
                    VapourSynthError::PluginArgumentsError {
                        plugin:   Self::PLUGIN_NAME.to_owned(),
                        argument: "src".to_owned(),
                        message:  e.to_string(),
                    }
                })?;
            },
            AssumeFPSOptions::FPS {
                numerator,
                denominator,
            } => {
                Self::argument_set_ints(&mut arguments, vec![
                    ("numerator", Some(*numerator)),
                    ("denominator", Some(*denominator)),
                ])?;
            },
        }

        let node = Self::invoke_and_get_node(core, arguments, Some("clip"))?;

        Ok(node)
    }
}

// #[derive(Serialize, Deserialize, Debug, Clone)]
pub enum AssumeFPSOptions<'core> {
    Source(Node<'core>),
    FPS { numerator: u32, denominator: u32 },
}

impl VapourSynthPluginScript for AssumeFPS {
    #[inline]
    fn generate_script(&self, node_name: NodeVariableName) -> Result<(Option<Imports>, Vec<Line>)> {
        let mut lines = vec![];

        let line = {
            let mut line = String::new();
            write!(&mut line, "core.std.AssumeFPS(clip = {}", node_name)?;
            if let Some(source_node_name) = &self.source_node_name {
                write!(&mut line, ", src = {}", source_node_name)?;
            }
            if let Some((numerator, denominator)) = self.fps {
                write!(
                    &mut line,
                    ", numerator = {}, denominator = {}",
                    numerator, denominator
                )?;
            }
            write!(&mut line, ")")?;
            line
        };

        lines.push(Line::Expression(node_name, line));

        Ok((None, lines))
    }
}
