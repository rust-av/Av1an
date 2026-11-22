use std::fmt::Write;

use anyhow::Result;
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use vapoursynth::{core::CoreRef, map::ValueType, node::Node};

use crate::vapoursynth::{
    VapourSynthError,
    plugins::{
        Plugin, PluginFunction,
        mvutensils::{DOCS, ID, NAME},
    },
    script_builder::{
        NodeVariableName, VapourSynthPluginScript,
        script::{Imports, Line, python_string_literal},
    },
};

/// MVUtensils Recalculate plugin - re-estimates an existing vector field
/// at a finer block size.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Recalculate {
    /// Only used for script generation
    pub super_name: Option<NodeVariableName>,
    /// Only used for script generation
    pub vectors_name: Option<NodeVariableName>,
    /// Blocks whose SAD is below this keep their vector. Default 200.
    pub thsad: Option<u32>,
    /// Interpolate new vector field from neighbours. Default true.
    pub smooth: Option<bool>,
    /// Finer block size [h, v].
    pub blksize: Option<Vec<u32>>,
    /// Finer overlap [h, v].
    pub overlap: Option<Vec<u32>>,
    /// Search algorithm: 0=log/diamond, 1=exhaustive, 2=hex, 3=UMH, 4=horiz,
    /// 5=vert.
    pub search: Option<u32>,
    /// Search radius/step for chosen search.
    pub searchparam: Option<u32>,
    /// Weight of motion-coherence penalty.
    pub mvlambda: Option<u32>,
    /// Include chroma planes in SAD/SATD metric.
    pub chroma: Option<bool>,
    /// Extra penalty for accepting a freshly searched vector.
    pub pnew: Option<u32>,
    /// Scan rows alternately for better predictor reuse.
    pub meander: Option<bool>,
    /// Use SATD instead of SAD.
    pub satd: Option<bool>,
    /// Prefix for frame properties.
    pub prefix: Option<String>,
}

impl Plugin for Recalculate {
    const PLUGIN_NAME: &'static str = NAME;
    const PLUGIN_ID: &'static str = ID;
    const PLUGIN_DOCS: Option<&'static str> = Some(DOCS);
}

impl PluginFunction for Recalculate {
    const FUNCTION_NAME: &'static str = "Recalculate";
    const FUNCTION_DOCS: Option<&'static str> =
        Some("https://github.com/myrsloik/mvutensils#recalculate");
    const REQUIRED_ARGUMENTS: &'static [(&'static str, &'static ValueType)] =
        &[("super", &ValueType::VideoNode), ("vectors", &ValueType::VideoNode)];
    const OPTIONAL_ARGUMENTS: &'static [(&'static str, &'static ValueType)] = &[
        ("thsad", &ValueType::Int),
        ("smooth", &ValueType::Int),
        ("blksize", &ValueType::Int),
        ("overlap", &ValueType::Int),
        ("search", &ValueType::Int),
        ("searchparam", &ValueType::Int),
        ("mvlambda", &ValueType::Int),
        ("chroma", &ValueType::Int),
        ("pnew", &ValueType::Int),
        ("meander", &ValueType::Int),
        ("satd", &ValueType::Int),
        ("prefix", &ValueType::Data),
    ];
}

impl Recalculate {
    #[inline]
    pub fn invoke<'core>(
        self,
        core: CoreRef<'core>,
        super_node: &Node<'core>,
        vectors: &[Node<'core>],
    ) -> Result<Vec<Node<'core>>, VapourSynthError> {
        let mut arguments = Self::arguments()?;
        arguments.set_node("super", super_node).map_err(|e| {
            VapourSynthError::PluginArgumentsError {
                plugin: Self::PLUGIN_NAME.to_owned(),
                argument: "super".to_owned(),
                message: e.to_string(),
            }
        })?;
        for vector_node in vectors.iter() {
            arguments.append_node("vectors", vector_node).map_err(|e| {
                VapourSynthError::PluginArgumentsError {
                    plugin: Self::PLUGIN_NAME.to_owned(),
                    argument: "vectors".to_owned(),
                    message: e.to_string(),
                }
            })?;
        }
        Self::argument_set_int_arrays(
            &mut arguments,
            vec![("blksize", self.blksize), ("overlap", self.overlap)],
        )?;
        Self::argument_set_ints(
            &mut arguments,
            vec![
                ("thsad", self.thsad),
                ("smooth", self.smooth.map(|b| if b { 1 } else { 0 })),
                ("search", self.search),
                ("searchparam", self.searchparam),
                ("mvlambda", self.mvlambda),
                ("chroma", self.chroma.map(|b| if b { 1 } else { 0 })),
                ("pnew", self.pnew),
                ("meander", self.meander.map(|b| if b { 1 } else { 0 })),
                ("satd", self.satd.map(|b| if b { 1 } else { 0 })),
            ],
        )?;
        Self::arguments_set(&mut arguments, vec![("prefix", self.prefix)])?;
        Self::invoke_and_get_node_array(core, arguments, "clip")
    }
}

impl VapourSynthPluginScript for Recalculate {
    #[inline]
    fn generate_script(&self, node_name: NodeVariableName) -> Result<(Option<Imports>, Vec<Line>)> {
        let mut lines = vec![];

        let line = {
            let mut line = String::new();
            match (&self.super_name, &self.vectors_name) {
                (Some(super_name), Some(vectors_name)) => {
                    write!(
                        &mut line,
                        "core.mvu.Recalculate(super = {}, vectors = {}",
                        super_name, vectors_name
                    )?;
                },
                (Some(super_name), None) => {
                    write!(
                        &mut line,
                        "core.mvu.Recalculate(super = {}, vectors = {}",
                        super_name, node_name
                    )?;
                },
                (None, Some(vectors_name)) => {
                    write!(
                        &mut line,
                        "core.mvu.Recalculate(super = {}, vectors = {}",
                        node_name, vectors_name
                    )?;
                },
                (None, None) => {
                    write!(
                        &mut line,
                        "core.mvu.Recalculate(super = {}, vectors = {}",
                        node_name, node_name
                    )?;
                },
            }
            if let Some(thsad) = self.thsad {
                write!(&mut line, ", thsad = {}", thsad as i64)?;
            }
            if let Some(smooth) = self.smooth {
                write!(&mut line, ", smooth = {}", smooth as i64)?;
            }
            if let Some(blksize) = &self.blksize {
                write!(&mut line, ", blksize = [{}]", blksize.iter().join(", "))?;
            }
            if let Some(overlap) = &self.overlap {
                write!(&mut line, ", overlap = [{}]", overlap.iter().join(", "))?;
            }
            if let Some(search) = self.search {
                write!(&mut line, ", search = {}", search as i64)?;
            }
            if let Some(searchparam) = self.searchparam {
                write!(&mut line, ", searchparam = {}", searchparam as i64)?;
            }
            if let Some(mvlambda) = self.mvlambda {
                write!(&mut line, ", mvlambda = {}", mvlambda as i64)?;
            }
            if let Some(chroma) = self.chroma {
                write!(&mut line, ", chroma = {}", chroma as i64)?;
            }
            if let Some(pnew) = self.pnew {
                write!(&mut line, ", pnew = {}", pnew as i64)?;
            }
            if let Some(meander) = self.meander {
                write!(&mut line, ", meander = {}", meander as i64)?;
            }
            if let Some(satd) = self.satd {
                write!(&mut line, ", satd = {}", satd as i64)?;
            }
            if let Some(prefix) = &self.prefix {
                write!(&mut line, ", prefix = {}", python_string_literal(prefix))?;
            }
            write!(&mut line, ")")?;
            line
        };

        lines.push(Line::Expression(node_name, line));

        Ok((None, lines))
    }
}
