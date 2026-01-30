use std::fmt::Write;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use strum::{Display, EnumString, IntoStaticStr};
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
pub struct CVVDP {
    /// Only used for script generation
    pub reference_node_name: String,
    /// Only used for script generation
    pub distorted_node_name: String,
    pub gpu_id:              Option<u32>,
    pub model_name:          Option<DisplayModel>,
    pub model_config_json:   Option<String>,
    pub resize_to_display:   Option<bool>,
    pub distmap:             Option<bool>,
}

impl PluginFunction for CVVDP {
    const PLUGIN_NAME: &'static str = "VapourSynth-HIP";
    const PLUGIN_ID: &'static str = "com.lumen.vship";
    const FUNCTION_NAME: &'static str = "CVVDP";
    const REQUIRED_ARGUMENTS: &'static [(&'static str, &'static ValueType)] =
        &[("reference", &ValueType::VideoNode), ("distorted", &ValueType::VideoNode)];
    const OPTIONAL_ARGUMENTS: &'static [(&'static str, &'static ValueType)] = &[
        ("gpu_id", &ValueType::Int),
        ("distmap", &ValueType::Int),
        ("model_name", &ValueType::Data),
        ("model_config_json", &ValueType::Data),
        ("resizeToDisplay", &ValueType::Int),
    ];
}

impl CVVDP {
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
            ("gpu_id", self.gpu_id),
            (
                "resizeToDisplay",
                self.resize_to_display.map(|b| if b { 1 } else { 0 }),
            ),
            ("distmap", self.distmap.map(|b| if b { 1 } else { 0 })),
        ])?;
        Self::arguments_set(&mut arguments, vec![
            (
                "model_name",
                self.model_name
                    .clone()
                    .map(|model_name| model_name.to_string().into_bytes())
                    .as_deref(),
            ),
            (
                "model_config_json",
                self.model_config_json
                    .map(|model_config_json| model_config_json.into_bytes())
                    .as_deref(),
            ),
        ])?;
        let node = Self::invoke_and_get_node(core, arguments, Some("clip"))?;

        Ok(node)
    }
}

impl VapourSynthPluginScript for CVVDP {
    #[inline]
    fn generate_script(&self, node_name: NodeVariableName) -> Result<(Option<Imports>, Vec<Line>)> {
        let mut lines = vec![];

        let line = {
            let mut line = String::new();
            write!(
                &mut line,
                "core.vship.CVVDP(reference = {}, distorted = {}",
                self.reference_node_name, self.distorted_node_name
            )?;
            if let Some(gpu_id) = self.gpu_id {
                write!(&mut line, ", gpu_id = {}", gpu_id)?;
            }
            if let Some(model_name) = &self.model_name {
                write!(&mut line, ", model_name = {}", model_name)?;
            }
            if let Some(model_config_json) = &self.model_config_json {
                write!(&mut line, ", model_config_json = {}", model_config_json)?;
            }
            if let Some(resize_to_display) = self.resize_to_display {
                write!(
                    &mut line,
                    ", resizeToDisplay = {}",
                    resize_to_display as i64
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

#[derive(Debug, Clone, Serialize, Deserialize, Display, EnumString, IntoStaticStr)]
pub enum DisplayModel {
    #[strum(serialize = "standard_4k")]
    Standard4K,
    #[strum(serialize = "standard_hdr_pq")]
    StandardHDRPQ,
    #[strum(serialize = "standard_hdr_hlg")]
    StandardHDRHLG,
    #[strum(serialize = "standard_hdr_linear")]
    StandardHDRLinear,
    #[strum(serialize = "standard_hdr_dark")]
    StandardHDRDark,
    #[strum(serialize = "standard_hdr_linear_zoom")]
    StandardHDRLinearZoom,
    #[strum(serialize = "standard_fhd")]
    StandardFHD,
}
