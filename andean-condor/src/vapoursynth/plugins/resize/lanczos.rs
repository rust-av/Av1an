use std::fmt::Write;

use anyhow::Result;
use vapoursynth::{core::CoreRef, map::ValueType, node::Node};

use crate::vapoursynth::{
    VapourSynthError,
    plugins::{
        Plugin, PluginFunction,
        resize::{
            ChromaLocation, ColorPrimaries, DOCS, DitherType, ID, MatrixCoefficients, NAME, Range,
            TransferCharacteristics,
        },
    },
    script_builder::{
        NodeVariableName, VapourSynthPluginScript,
        script::{Imports, Line, python_string_literal},
    },
};

#[derive(Debug, Clone, Default)]
pub struct Lanczos {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub format: Option<vapoursynth::format::PresetFormat>,
    pub matrix: Option<MatrixCoefficients>,
    pub matrix_in: Option<MatrixCoefficients>,
    pub transfer: Option<TransferCharacteristics>,
    pub transfer_in: Option<TransferCharacteristics>,
    pub primaries: Option<ColorPrimaries>,
    pub primaries_in: Option<ColorPrimaries>,
    pub range: Option<Range>,
    pub range_in: Option<Range>,
    pub chromaloc: Option<ChromaLocation>,
    pub chromaloc_in: Option<ChromaLocation>,
    pub dither_type: Option<DitherType>,
    pub taps: Option<u32>,
}

impl Plugin for Lanczos {
    const PLUGIN_NAME: &'static str = NAME;
    const PLUGIN_ID: &'static str = ID;
    const PLUGIN_DOCS: Option<&'static str> = Some(DOCS);
}

impl PluginFunction for Lanczos {
    const FUNCTION_NAME: &'static str = "Lanczos";
    const FUNCTION_DOCS: Option<&'static str> =
        Some("https://www.vapoursynth.com/doc/functions/video/resize.html#resize.Lanczos");
    const REQUIRED_ARGUMENTS: &'static [(&'static str, &'static ValueType)] =
        &[("clip", &ValueType::VideoNode)];
    const OPTIONAL_ARGUMENTS: &'static [(&'static str, &'static ValueType)] = &[
        ("width", &ValueType::Int),
        ("height", &ValueType::Int),
        ("format", &ValueType::Int),
        ("matrix", &ValueType::Int),
        ("matrix_in", &ValueType::Int),
        ("transfer", &ValueType::Int),
        ("transfer_in", &ValueType::Int),
        ("primaries", &ValueType::Int),
        ("primaries_in", &ValueType::Int),
        ("range", &ValueType::Int),
        ("range_in", &ValueType::Int),
        ("chromaloc", &ValueType::Int),
        ("chromaloc_in", &ValueType::Int),
        ("filter_param_a", &ValueType::Int),
        ("dither", &ValueType::Data),
    ];
}

impl Lanczos {
    #[inline]
    pub fn new(taps: Option<u32>) -> Self {
        Self {
            taps,
            ..Default::default()
        }
    }

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
                plugin: Self::PLUGIN_NAME.to_owned(),
                argument: "clip".to_owned(),
                message: e.to_string(),
            })?;
        Self::argument_set_ints(
            &mut arguments,
            vec![
                ("width", self.width.map(|i| i as i64)),
                ("height", self.height.map(|i| i as i64)),
                ("format", self.format.map(|e| e as i64)),
                ("matrix", self.matrix.map(|e| e as i64)),
                ("matrix_in", self.matrix_in.map(|e| e as i64)),
                ("transfer", self.transfer.map(|e| e as i64)),
                ("transfer_in", self.transfer_in.map(|e| e as i64)),
                ("primaries", self.primaries.map(|e| e as i64)),
                ("primaries_in", self.primaries_in.map(|e| e as i64)),
                ("range", self.range.map(|e| e as i64)),
                ("range_in", self.range_in.map(|e| e as i64)),
                ("chromaloc", self.chromaloc.map(|e| e as i64)),
                ("chromaloc_in", self.chromaloc_in.map(|e| e as i64)),
                ("filter_param_a", self.taps.map(|e| e as i64)),
            ],
        )?;
        // Self::arguments_set_floats(&mut arguments, vec![("filter_param_a",
        // self.taps)])?;
        Self::arguments_set(
            &mut arguments,
            vec![(
                "dither_type",
                self.dither_type.map(|dt| dt.to_string().into_bytes()).as_deref(),
            )],
        )?;
        let node = Self::invoke_and_get_node(core, arguments, Some("clip"))?;

        Ok(node)
    }
}

impl VapourSynthPluginScript for Lanczos {
    #[inline]
    fn generate_script(&self, node_name: NodeVariableName) -> Result<(Option<Imports>, Vec<Line>)> {
        let mut lines = vec![];

        let line = {
            let mut line = String::new();
            write!(&mut line, "core.resize.Lanczos(clip = {}", node_name)?;
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
            if let Some(matrix_in) = self.matrix_in {
                write!(&mut line, ", matrix_in = {}", matrix_in as i64)?;
            }
            if let Some(transfer) = self.transfer {
                write!(&mut line, ", transfer = {}", transfer as i64)?;
            }
            if let Some(transfer_in) = self.transfer_in {
                write!(&mut line, ", transfer_in = {}", transfer_in as i64)?;
            }
            if let Some(primaries) = self.primaries {
                write!(&mut line, ", primaries = {}", primaries as i64)?;
            }
            if let Some(primaries_in) = self.primaries_in {
                write!(&mut line, ", primaries_in = {}", primaries_in as i64)?;
            }
            if let Some(range) = self.range {
                write!(&mut line, ", range = {}", range as i64)?;
            }
            if let Some(range_in) = self.range_in {
                write!(&mut line, ", range_in = {}", range_in as i64)?;
            }
            if let Some(chromaloc) = self.chromaloc {
                write!(&mut line, ", chromaloc = {}", chromaloc as i64)?;
            }
            if let Some(chromaloc_in) = self.chromaloc_in {
                write!(&mut line, ", chromaloc_in = {}", chromaloc_in as i64)?;
            }
            if let Some(dither_type) = self.dither_type {
                write!(
                    &mut line,
                    ", dither_type = {}",
                    python_string_literal(&dither_type.to_string())
                )?;
            }
            if let Some(taps) = self.taps {
                write!(&mut line, ", filter_param_a = {}", taps)?;
            }
            write!(&mut line, ")")?;
            line
        };

        lines.push(Line::Expression(node_name, line));

        Ok((None, lines))
    }
}
