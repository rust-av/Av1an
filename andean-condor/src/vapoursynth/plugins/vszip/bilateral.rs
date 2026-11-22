use std::fmt::Write;

use anyhow::Result;
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use vapoursynth::{core::CoreRef, map::ValueType, node::Node};

use crate::vapoursynth::{
    VapourSynthError,
    plugins::{
        Plugin, PluginFunction,
        vszip::{DOCS, ID, NAME},
    },
    script_builder::{
        NodeVariableName, VapourSynthPluginScript,
        script::{Imports, Line},
    },
};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Bilateral {
    /// Only used for script generation
    pub ref_name: Option<NodeVariableName>,
    /// sigma of Gaussian function to calculate spatial weight.
    /// The scale of this parameter is equivalent to pixel distance.
    /// Larger sigmaS results in larger filtering radius as well as stronger
    /// smoothing. Use an array to assign sigmaS for each plane. If sigmaS
    /// for the second plane is not specified, it will be set according to the
    /// sigmaS of first plane and sub-sampling.
    ///
    /// Defaults to `[3.0, 3.0, 3.0]`
    pub sigma_s: Option<Vec<f64>>,
    /// sigma of Gaussian function to calculate range weight.
    /// The scale of this parameter is the same as pixel values in `[0, 1]`.
    /// Smaller sigmaR preserves edges better, may also leads to weaker
    /// smoothing. Use an array to specify sigmaR for each plane, otherwise
    /// the same sigmaR is used for all the planes.
    ///
    /// Defaults to `[0.02, 0.02, 0.02]`
    pub sigma_r: Option<Vec<f64>>,
    /// An array to specify which planes to process.
    pub planes: Option<Vec<bool>>,
    /// 0 = Automatically determine the algorithm according to sigmaS, sigmaR
    /// and PBFICnum.
    ///
    /// 1 = O(1) Bilateral filter uses quantized PBFICs. (IMO it should be
    /// O(PBFICnum))
    ///
    /// 2 = Bilateral filter with truncated spatial window and sub-sampling.
    /// O(sigmaS^2)
    pub algorithm: Option<Vec<u32>>,
    /// Number of PBFICs used in algorithm=1.
    /// Default: 4 when sigmaR>=0.08. It will increase as sigmaR decreases, up
    /// to 32. For chroma plane default value will be odd to better preserve
    /// neutral value of chrominance. Available range is `[2, 256]`.
    /// Use an array to specify PBFICnum for each plane.
    pub pbficnum: Option<Vec<u32>>,
}

impl Plugin for Bilateral {
    const PLUGIN_NAME: &'static str = NAME;
    const PLUGIN_ID: &'static str = ID;
    const PLUGIN_DOCS: Option<&'static str> = Some(DOCS);
}

impl PluginFunction for Bilateral {
    const FUNCTION_NAME: &'static str = "Bilateral";
    const FUNCTION_DOCS: Option<&'static str> =
        Some("https://github.com/dnjulek/vapoursynth-zip/wiki/Bilateral");
    const REQUIRED_ARGUMENTS: &'static [(&'static str, &'static ValueType)] =
        &[("clip", &ValueType::VideoNode)];
    const OPTIONAL_ARGUMENTS: &'static [(&'static str, &'static ValueType)] = &[
        ("ref", &ValueType::VideoNode),
        ("sigmaS", &ValueType::Float),
        ("sigmaR", &ValueType::Float),
        ("planes", &ValueType::Int),
        ("algorithm", &ValueType::Int),
        ("PBFICnum", &ValueType::Int),
    ];
}

impl Bilateral {
    #[inline]
    pub fn invoke<'core>(
        self,
        core: CoreRef<'core>,
        node: &Node<'core>,
        ref_node: Option<&Node<'core>>,
    ) -> Result<Node<'core>, VapourSynthError> {
        let mut arguments = Self::arguments()?;
        arguments
            .set_node("clip", node)
            .map_err(|e| VapourSynthError::PluginArgumentsError {
                plugin: Self::PLUGIN_NAME.to_owned(),
                argument: "clip".to_owned(),
                message: e.to_string(),
            })?;
        if let Some(ref_node) = ref_node {
            arguments.set_node("ref", ref_node).map_err(|e| {
                VapourSynthError::PluginArgumentsError {
                    plugin: Self::PLUGIN_NAME.to_owned(),
                    argument: "ref".to_owned(),
                    message: e.to_string(),
                }
            })?;
        }
        Self::arguments_set_float_arrays(
            &mut arguments,
            vec![("sigmaS", self.sigma_s), ("sigmaR", self.sigma_r)],
        )?;
        Self::argument_set_int_arrays(
            &mut arguments,
            vec![
                (
                    "planes",
                    self.planes
                        .as_ref()
                        .map(|b| b.iter().map(|b| if *b { 1 } else { 0 }).collect()),
                ),
                ("algorithm", self.algorithm),
                ("PBFICnum", self.pbficnum),
            ],
        )?;
        let node = Self::invoke_and_get_node(core, arguments, Some("clip"))?;

        Ok(node)
    }
}

impl VapourSynthPluginScript for Bilateral {
    #[inline]
    fn generate_script(&self, node_name: NodeVariableName) -> Result<(Option<Imports>, Vec<Line>)> {
        let mut lines = vec![];

        let line = {
            let mut line = String::new();
            write!(&mut line, "core.vszip.Bilateral(clip = {}", node_name)?;
            if let Some(ref_name) = &self.ref_name {
                write!(&mut line, ", ref = {}", ref_name)?;
            }
            if let Some(sigma_s) = &self.sigma_s {
                write!(&mut line, ", sigmaS = [{}]", sigma_s.iter().join(", "))?;
            }
            if let Some(sigma_r) = &self.sigma_r {
                write!(&mut line, ", sigmaR = [{}]", sigma_r.iter().join(", "))?;
            }
            if let Some(planes) = &self.planes {
                write!(&mut line, ", planes = [{}]", planes.iter().join(", "))?;
            }
            if let Some(algorithm) = &self.algorithm {
                write!(&mut line, ", algorithm = [{}]", algorithm.iter().join(", "))?;
            }
            if let Some(pbficnum) = &self.pbficnum {
                write!(&mut line, ", PBFICnum = [{}]", pbficnum.iter().join(", "))?;
            }
            write!(&mut line, ")")?;
            line
        };

        lines.push(Line::Expression(node_name, line));

        Ok((None, lines))
    }
}
