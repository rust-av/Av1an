use std::fmt::Write;

use anyhow::Result;
use vapoursynth::{core::CoreRef, map::ValueType, node::Node};

use crate::vs::{
    plugins::{
        resize::{
            ChromaLocation,
            ColorPrimaries,
            DitherType,
            MatrixCoefficients,
            Range,
            TransferCharacteristics,
        },
        PluginFunction,
    },
    script_builder::{
        script::{Imports, Line},
        NodeVariableName,
        VapourSynthPluginScript,
    },
    VapourSynthError,
};

#[derive(Debug, Clone, Default)]
pub struct Spline16 {
    pub width:       Option<u32>,
    pub height:      Option<u32>,
    pub format:      Option<vapoursynth::format::PresetFormat>,
    pub matrix:      Option<MatrixCoefficients>,
    pub transfer:    Option<TransferCharacteristics>,
    pub primaries:   Option<ColorPrimaries>,
    pub range:       Option<Range>,
    pub chromaloc:   Option<ChromaLocation>,
    pub dither_type: Option<DitherType>,
}

impl PluginFunction for Spline16 {
    const PLUGIN_NAME: &'static str = "resize";
    const PLUGIN_ID: &'static str = "com.vapoursynth.resize";
    const FUNCTION_NAME: &'static str = "Spline16";
    const REQUIRED_ARGUMENTS: &'static [(&'static str, &'static ValueType)] =
        &[("clip", &ValueType::VideoNode)];
    const OPTIONAL_ARGUMENTS: &'static [(&'static str, &'static ValueType)] = &[
        ("width", &ValueType::Int),
        ("height", &ValueType::Int),
        ("format", &ValueType::Int),
        ("matrix", &ValueType::Int),
        ("transfer", &ValueType::Int),
        ("primaries", &ValueType::Int),
        ("range", &ValueType::Int),
        ("chromaloc", &ValueType::Int),
        ("dither", &ValueType::Data),
    ];
}

impl Spline16 {
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
            ("width", self.width.map(|i| i as i64)),
            ("height", self.height.map(|i| i as i64)),
            ("format", self.format.map(|e| e as i64)),
            ("matrix", self.matrix.map(|e| e as i64)),
            ("transfer", self.transfer.map(|e| e as i64)),
            ("primaries", self.primaries.map(|e| e as i64)),
            ("range", self.range.map(|e| e as i64)),
            ("chromaloc", self.chromaloc.map(|e| e as i64)),
        ])?;
        Self::arguments_set(&mut arguments, vec![(
            "dither_type",
            self.dither_type.map(|dt| dt.to_string().into_bytes()).as_deref(),
        )])?;
        let node = Self::invoke_and_get_node(core, arguments, Some("clip"))?;

        Ok(node)
    }
}

impl VapourSynthPluginScript for Spline16 {
    #[inline]
    fn generate_script(&self, node_name: NodeVariableName) -> Result<(Option<Imports>, Vec<Line>)> {
        let mut lines = vec![];

        let line = {
            let mut line = String::new();
            write!(&mut line, "core.resize.Spline16(clip = {}", node_name)?;
            if let Some(width) = self.width {
                write!(&mut line, ", width = {}", width)?;
            }
            if let Some(height) = self.height {
                write!(&mut line, ", height = {}", height)?;
            }
            if let Some(format) = self.format {
                write!(&mut line, ", format = {}", format as i64)?;
            }
            if let Some(matrix) = self.matrix {
                write!(&mut line, ", matrix = {}", matrix as i64)?;
            }
            if let Some(transfer) = self.transfer {
                write!(&mut line, ", transfer = {}", transfer as i64)?;
            }
            if let Some(primaries) = self.primaries {
                write!(&mut line, ", primaries = {}", primaries as i64)?;
            }
            if let Some(range) = self.range {
                write!(&mut line, ", range = {}", range as i64)?;
            }
            if let Some(chromaloc) = self.chromaloc {
                write!(&mut line, ", chromaloc = {}", chromaloc as i64)?;
            }
            if let Some(dither_type) = self.dither_type {
                write!(&mut line, ", dither_type = {}", dither_type as i64)?;
            }
            write!(&mut line, ")")?;
            line
        };

        lines.push(Line::Expression(node_name, line));

        Ok((None, lines))
    }
}
