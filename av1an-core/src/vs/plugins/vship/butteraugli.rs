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
pub struct BUTTERAUGLI {
    /// Only used for script generation
    pub reference_node_name:  String,
    /// Only used for script generation
    pub distorted_node_name:  String,
    pub num_stream:           Option<u32>,
    pub gpu_id:               Option<u32>,
    pub q_norm:               Option<u32>,
    pub intensity_multiplier: Option<f64>,
    pub distmap:              Option<bool>,
}

impl PluginFunction for BUTTERAUGLI {
    const PLUGIN_NAME: &'static str = "VapourSynth-HIP";
    const PLUGIN_ID: &'static str = "com.lumen.vship";
    const FUNCTION_NAME: &'static str = "BUTTERAUGLI";
    const REQUIRED_ARGUMENTS: &'static [(&'static str, &'static ValueType)] =
        &[("reference", &ValueType::Node), ("distorted", &ValueType::Node)];
    const OPTIONAL_ARGUMENTS: &'static [(&'static str, &'static ValueType)] = &[
        ("numStream", &ValueType::Int),
        ("gpu_id", &ValueType::Int),
        ("qnorm", &ValueType::Int),
        ("intensity_multiplier", &ValueType::Float),
        ("distmap", &ValueType::Int),
    ];
}

impl BUTTERAUGLI {
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
            ("numStream", self.num_stream),
            ("gpu_id", self.gpu_id),
            ("qnorm", self.q_norm),
            ("distmap", self.distmap.map(|b| if b { 1 } else { 0 })),
        ])?;
        Self::arguments_set_floats(&mut arguments, vec![(
            "intensity_multiplier",
            self.intensity_multiplier,
        )])?;
        let node = Self::invoke_and_get_node(core, arguments, Some("clip"))?;

        Ok(node)
    }
}

impl VapourSynthPluginScript for BUTTERAUGLI {
    #[inline]
    fn generate_script(&self, node_name: NodeVariableName) -> Result<(Option<Imports>, Vec<Line>)> {
        let mut lines = vec![];

        let line = {
            let mut line = String::new();
            write!(
                &mut line,
                "core.vship.BUTTERAUGLI(reference = {}, distorted = {}",
                self.reference_node_name, self.distorted_node_name
            )?;
            if let Some(num_stream) = self.num_stream {
                write!(&mut line, ", numStream = {}", num_stream)?;
            }
            if let Some(gpu_id) = self.gpu_id {
                write!(&mut line, ", gpu_id = {}", gpu_id)?;
            }
            if let Some(q_norm) = self.q_norm {
                write!(&mut line, ", qnorm = {}", q_norm)?;
            }
            if let Some(intensity_multiplier) = self.intensity_multiplier {
                write!(
                    &mut line,
                    ", intensity_multiplier = {}",
                    intensity_multiplier
                )?;
            }
            if let Some(distmap) = self.distmap {
                write!(&mut line, ", distmap = {}", distmap as i64)?;
            }
            write!(&mut line, ")")?;
            line
        };

        lines.push(Line::Expression(node_name, line));

        Ok((None, lines))
    }
}
