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

/// MVUtensils Analyse plugin - estimates motion vectors for one temporal
/// direction/distance.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Analyse {
    /// Only used for script generation
    pub super_name: Option<NodeVariableName>,
    /// Block size [h, v]. Defaults to super's value.
    pub blksize: Option<Vec<u32>>,
    /// Block overlap [h, v]. Defaults to super's value.
    pub overlap: Option<Vec<u32>>,
    /// Number of hierarchical levels to use. 0 = all.
    pub levels: Option<u32>,
    /// Search algorithm: 0=log/diamond, 1=exhaustive, 2=hex, 3=UMH, 4=horiz,
    /// 5=vert.
    pub search: Option<u32>,
    /// Search radius/step for chosen search.
    pub searchparam: Option<u32>,
    /// Refinement search radius at finest level.
    pub pelsearch: Option<u32>,
    /// Weight of motion-coherence penalty (per 8x8 block).
    pub mvlambda: Option<u32>,
    /// Include chroma planes in SAD/SATD metric.
    pub chroma: Option<bool>,
    /// Temporal distance and direction. Positive=backward, negative=forward.
    pub delta: Option<i32>,
    /// SAD "knee" that throttles mvlambda per block.
    pub lsad: Option<u32>,
    /// How mvlambda scales across pyramid levels: 0=const, 1=linear,
    /// 2=quadratic.
    pub plevel: Option<u32>,
    /// Estimate global (pan) motion vector.
    pub globalmv: Option<bool>,
    /// Extra penalty for accepting a freshly searched vector.
    pub pnew: Option<u32>,
    /// Extra penalty for accepting the zero vector. Defaults to pnew.
    pub pzero: Option<u32>,
    /// Penalty for the global-motion predictor.
    pub pglobal: Option<u32>,
    /// SAD above which a block gets a wider search.
    pub badsad: Option<u32>,
    /// Radius of the wider search.
    pub badrange: Option<u32>,
    /// Scan rows alternately for better predictor reuse.
    pub meander: Option<bool>,
    /// Try multiple motion-vector candidates per block: 0=off, 1=except finest,
    /// 2=all.
    pub trymany: Option<u32>,
    /// Use SATD instead of SAD.
    pub satd: Option<bool>,
    /// Prefix for frame properties.
    pub prefix: Option<String>,
}

impl Plugin for Analyse {
    const PLUGIN_NAME: &'static str = NAME;
    const PLUGIN_ID: &'static str = ID;
    const PLUGIN_DOCS: Option<&'static str> = Some(DOCS);
}

impl PluginFunction for Analyse {
    const FUNCTION_NAME: &'static str = "Analyse";
    const FUNCTION_DOCS: Option<&'static str> =
        Some("https://github.com/myrsloik/mvutensils#analyse");
    const REQUIRED_ARGUMENTS: &'static [(&'static str, &'static ValueType)] =
        &[("super", &ValueType::VideoNode)];
    const OPTIONAL_ARGUMENTS: &'static [(&'static str, &'static ValueType)] = &[
        ("blksize", &ValueType::Int),
        ("overlap", &ValueType::Int),
        ("levels", &ValueType::Int),
        ("search", &ValueType::Int),
        ("searchparam", &ValueType::Int),
        ("pelsearch", &ValueType::Int),
        ("mvlambda", &ValueType::Int),
        ("chroma", &ValueType::Int),
        ("delta", &ValueType::Int),
        ("lsad", &ValueType::Int),
        ("plevel", &ValueType::Int),
        ("globalmv", &ValueType::Int),
        ("pnew", &ValueType::Int),
        ("pzero", &ValueType::Int),
        ("pglobal", &ValueType::Int),
        ("badsad", &ValueType::Int),
        ("badrange", &ValueType::Int),
        ("meander", &ValueType::Int),
        ("trymany", &ValueType::Int),
        ("satd", &ValueType::Int),
        ("prefix", &ValueType::Data),
    ];
}

impl Analyse {
    #[inline]
    pub fn invoke<'core>(
        self,
        core: CoreRef<'core>,
        super_node: &Node<'core>,
    ) -> Result<Node<'core>, VapourSynthError> {
        let mut arguments = Self::arguments()?;
        arguments.set_node("super", super_node).map_err(|e| {
            VapourSynthError::PluginArgumentsError {
                plugin: Self::PLUGIN_NAME.to_owned(),
                argument: "super".to_owned(),
                message: e.to_string(),
            }
        })?;
        Self::argument_set_int_arrays(
            &mut arguments,
            vec![("blksize", self.blksize), ("overlap", self.overlap)],
        )?;
        Self::argument_set_ints(
            &mut arguments,
            vec![
                ("levels", self.levels),
                ("search", self.search),
                ("searchparam", self.searchparam),
                ("pelsearch", self.pelsearch),
                ("mvlambda", self.mvlambda),
                ("chroma", self.chroma.map(|b| if b { 1 } else { 0 })),
                ("lsad", self.lsad),
                ("plevel", self.plevel),
                ("globalmv", self.globalmv.map(|b| if b { 1 } else { 0 })),
                ("pnew", self.pnew),
                ("pzero", self.pzero),
                ("pglobal", self.pglobal),
                ("badsad", self.badsad),
                ("badrange", self.badrange),
                ("meander", self.meander.map(|b| if b { 1 } else { 0 })),
                ("trymany", self.trymany),
                ("satd", self.satd.map(|b| if b { 1 } else { 0 })),
            ],
        )?;
        // delta is signed (negative = forward, positive = backward)
        Self::argument_set_int(&mut arguments, "delta", self.delta.map(|d| d as i64))?;
        Self::arguments_set(&mut arguments, vec![("prefix", self.prefix)])?;
        let node = Self::invoke_and_get_node(core, arguments, Some("clip"))?;
        Ok(node)
    }
}

impl VapourSynthPluginScript for Analyse {
    #[inline]
    fn generate_script(&self, node_name: NodeVariableName) -> Result<(Option<Imports>, Vec<Line>)> {
        let mut lines = vec![];

        let line = {
            let mut line = String::new();
            match &self.super_name {
                Some(super_name) => {
                    write!(&mut line, "core.mvu.Analyse(super = {}", super_name)?;
                },
                None => {
                    write!(&mut line, "core.mvu.Analyse(super = {}", node_name)?;
                },
            }
            if let Some(blksize) = &self.blksize {
                write!(&mut line, ", blksize = [{}]", blksize.iter().join(", "))?;
            }
            if let Some(overlap) = &self.overlap {
                write!(&mut line, ", overlap = [{}]", overlap.iter().join(", "))?;
            }
            if let Some(levels) = self.levels {
                write!(&mut line, ", levels = {}", levels as i64)?;
            }
            if let Some(search) = self.search {
                write!(&mut line, ", search = {}", search as i64)?;
            }
            if let Some(searchparam) = self.searchparam {
                write!(&mut line, ", searchparam = {}", searchparam as i64)?;
            }
            if let Some(pelsearch) = self.pelsearch {
                write!(&mut line, ", pelsearch = {}", pelsearch as i64)?;
            }
            if let Some(mvlambda) = self.mvlambda {
                write!(&mut line, ", mvlambda = {}", mvlambda as i64)?;
            }
            if let Some(chroma) = self.chroma {
                write!(&mut line, ", chroma = {}", chroma as i64)?;
            }
            if let Some(delta) = self.delta {
                write!(&mut line, ", delta = {}", delta)?;
            }
            if let Some(lsad) = self.lsad {
                write!(&mut line, ", lsad = {}", lsad as i64)?;
            }
            if let Some(plevel) = self.plevel {
                write!(&mut line, ", plevel = {}", plevel as i64)?;
            }
            if let Some(globalmv) = self.globalmv {
                write!(&mut line, ", globalmv = {}", globalmv as i64)?;
            }
            if let Some(pnew) = self.pnew {
                write!(&mut line, ", pnew = {}", pnew as i64)?;
            }
            if let Some(pzero) = self.pzero {
                write!(&mut line, ", pzero = {}", pzero as i64)?;
            }
            if let Some(pglobal) = self.pglobal {
                write!(&mut line, ", pglobal = {}", pglobal as i64)?;
            }
            if let Some(badsad) = self.badsad {
                write!(&mut line, ", badsad = {}", badsad as i64)?;
            }
            if let Some(badrange) = self.badrange {
                write!(&mut line, ", badrange = {}", badrange as i64)?;
            }
            if let Some(meander) = self.meander {
                write!(&mut line, ", meander = {}", meander as i64)?;
            }
            if let Some(trymany) = self.trymany {
                write!(&mut line, ", trymany = {}", trymany as i64)?;
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
