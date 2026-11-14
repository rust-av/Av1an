use std::fmt::Write;

use anyhow::Result;
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use vapoursynth::{
    core::CoreRef,
    format::{ColorFamily, Format, PresetFormat, SampleType},
    map::ValueType,
    node::Node,
};

use crate::vapoursynth::{
    VapourSynthError,
    plugins::{
        Plugin, PluginFunction,
        resize::bicubic::Bicubic,
        vszip::{DOCS, ID, NAME},
    },
    script_builder::{
        NodeVariableName, VapourSynthPluginScript,
        script::{Imports, Line},
    },
};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WNNM {
    /// Denoising strength per plane. A plane with sigma < FLT_EPSILON is copied
    /// through. Missing entries inherit the previous one.
    ///
    /// Defaults to `[3.0, 3.0, 3.0]`
    pub sigma: Option<Vec<f64>>,
    pub block_size: Option<u32>,
    pub block_step: Option<u32>,
    pub group_size: Option<u32>,
    pub bm_range: Option<u32>,
    pub radius: Option<u32>,
    pub ps_num: Option<u32>,
    pub ps_range: Option<u32>,
    pub residual: Option<bool>,
    pub adaptive_aggregation: Option<bool>,
    /// Only used for script generation
    pub rclip_name: Option<NodeVariableName>,
}

impl Plugin for WNNM {
    const PLUGIN_NAME: &'static str = NAME;
    const PLUGIN_ID: &'static str = ID;
    const PLUGIN_DOCS: Option<&'static str> = Some(DOCS);
}

impl PluginFunction for WNNM {
    const FUNCTION_NAME: &'static str = "WNNM";
    const FUNCTION_DOCS: Option<&'static str> =
        Some("https://github.com/dnjulek/vapoursynth-zip/wiki/WNNM");
    const REQUIRED_ARGUMENTS: &'static [(&'static str, &'static ValueType)] =
        &[("clip", &ValueType::VideoNode), ("planes", &ValueType::Int)];
    const OPTIONAL_ARGUMENTS: &'static [(&'static str, &'static ValueType)] = &[
        ("sigma", &ValueType::Float),
        ("block_size", &ValueType::Int),
        ("block_step", &ValueType::Int),
        ("group_size", &ValueType::Int),
        ("bm_range", &ValueType::Int),
        ("radius", &ValueType::Int),
        ("ps_num", &ValueType::Int),
        ("ps_range", &ValueType::Int),
        ("residual", &ValueType::Int),
        ("adaptive_aggregation", &ValueType::Int),
        ("rclip", &ValueType::VideoNode),
    ];
}

/// The source clip format captured from the node, sufficient to decide
/// whether the `com.julek.vszip` WNNM filter accepts the clip directly and
/// to reconstruct the original format for the round-trip back to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WnnmSourceFormat {
    color_family: ColorFamily,
    sample_type: SampleType,
    bits_per_sample: u8,
    sub_sampling_w: u8,
    sub_sampling_h: u8,
}

impl WnnmSourceFormat {
    /// Capture the format from a VapourSynth node's format.
    #[inline]
    fn from_format(format: Format<'_>) -> Self {
        Self {
            color_family: format.color_family(),
            sample_type: format.sample_type(),
            bits_per_sample: format.bits_per_sample(),
            sub_sampling_w: format.sub_sampling_w(),
            sub_sampling_h: format.sub_sampling_h(),
        }
    }

    /// `true` when the WNNM filter accepts the clip directly: 32-bit float
    /// samples in any color family (`Must be 32-bit float (any color
    /// family). Each plane is denoised separately.`).
    #[inline]
    fn wnnm_accepts_plain(self) -> bool {
        self.sample_type == SampleType::Float && self.bits_per_sample == 32
    }

    /// The 32-bit float [`PresetFormat`] the clip is converted to before
    /// calling WNNM, preserving the color family and chroma subsampling
    /// whenever a 32-bit float preset exists.
    ///
    /// `YUV410P8` (2, 2), `YUV411P8` (2, 0) and `YUV440P8` (0, 1) only exist
    /// at 8 bits in [`PresetFormat`], so they fall back to `YUV420PS`; the
    /// subsampling change is acceptable because WNNM denoises each plane
    /// independently.
    #[inline]
    fn wnnm_work_preset(self) -> Option<PresetFormat> {
        Some(match self.color_family {
            ColorFamily::Gray => PresetFormat::GrayS,
            ColorFamily::RGB => PresetFormat::RGBS,
            ColorFamily::YUV => match (self.sub_sampling_w, self.sub_sampling_h) {
                (1, 1) => PresetFormat::YUV420PS,
                (1, 0) => PresetFormat::YUV422PS,
                (0, 0) => PresetFormat::YUV444PS,
                _ => PresetFormat::YUV420PS,
            },
            ColorFamily::Undefined => return None,
        })
    }

    /// The original format the WNNM output is resized back to, mirroring the
    /// FGS reference's round-trip: `fgs_clip.resize.Bicubic(format=
    /// original_format.id, dither_type="none")`.
    ///
    /// Returns [`None`] for undefined formats and for depths without a
    /// [`PresetFormat`] (e.g. unusual float depths).
    #[inline]
    fn original_preset(self) -> Option<PresetFormat> {
        match self.color_family {
            ColorFamily::Undefined => None,
            ColorFamily::Gray => match self.sample_type {
                SampleType::Integer => match self.bits_per_sample {
                    8 => Some(PresetFormat::Gray8),
                    9 => Some(PresetFormat::Gray9),
                    10 => Some(PresetFormat::Gray10),
                    12 => Some(PresetFormat::Gray12),
                    14 => Some(PresetFormat::Gray14),
                    16 => Some(PresetFormat::Gray16),
                    32 => Some(PresetFormat::Gray32),
                    _ => None,
                },
                SampleType::Float => match self.bits_per_sample {
                    16 => Some(PresetFormat::GrayH),
                    32 => Some(PresetFormat::GrayS),
                    _ => None,
                },
            },
            ColorFamily::RGB => match self.sample_type {
                SampleType::Integer => match self.bits_per_sample {
                    8 => Some(PresetFormat::RGB24),
                    9 => Some(PresetFormat::RGB27),
                    10 => Some(PresetFormat::RGB30),
                    12 => Some(PresetFormat::RGB36),
                    14 => Some(PresetFormat::RGB42),
                    16 => Some(PresetFormat::RGB48),
                    _ => None,
                },
                SampleType::Float => match self.bits_per_sample {
                    16 => Some(PresetFormat::RGBH),
                    32 => Some(PresetFormat::RGBS),
                    _ => None,
                },
            },
            ColorFamily::YUV => match self.sample_type {
                SampleType::Integer => integer_yuv_preset(
                    self.sub_sampling_w,
                    self.sub_sampling_h,
                    self.bits_per_sample,
                ),
                SampleType::Float => float_yuv_preset(
                    self.sub_sampling_w,
                    self.sub_sampling_h,
                    self.bits_per_sample,
                ),
            },
        }
    }
}

/// Rounds a bit depth to the nearest depth that has an integer YUV
/// [`PresetFormat`] (8, 9, 10, 12, 14 or 16), rounding ties upward.
#[inline]
const fn nearest_integer_bits(bits_per_sample: u8) -> u8 {
    match bits_per_sample {
        8 | 9 | 10 | 12 | 14 | 16 => bits_per_sample,
        0..=7 => 8,
        11 => 12,
        13 => 14,
        15 => 16,
        _ => 16,
    }
}

/// The integer YUV [`PresetFormat`] with the given subsampling at the closest
/// integer bit depth, or [`None`] when the combination has no preset.
#[inline]
fn integer_yuv_preset(
    sub_sampling_w: u8,
    sub_sampling_h: u8,
    bits_per_sample: u8,
) -> Option<PresetFormat> {
    let bits = nearest_integer_bits(bits_per_sample);
    match (sub_sampling_w, sub_sampling_h, bits) {
        (1, 1, 8) => Some(PresetFormat::YUV420P8),
        (1, 1, 9) => Some(PresetFormat::YUV420P9),
        (1, 1, 10) => Some(PresetFormat::YUV420P10),
        (1, 1, 12) => Some(PresetFormat::YUV420P12),
        (1, 1, 14) => Some(PresetFormat::YUV420P14),
        (1, 1, 16) => Some(PresetFormat::YUV420P16),
        (1, 0, 8) => Some(PresetFormat::YUV422P8),
        (1, 0, 9) => Some(PresetFormat::YUV422P9),
        (1, 0, 10) => Some(PresetFormat::YUV422P10),
        (1, 0, 12) => Some(PresetFormat::YUV422P12),
        (1, 0, 14) => Some(PresetFormat::YUV422P14),
        (1, 0, 16) => Some(PresetFormat::YUV422P16),
        (0, 0, 8) => Some(PresetFormat::YUV444P8),
        (0, 0, 9) => Some(PresetFormat::YUV444P9),
        (0, 0, 10) => Some(PresetFormat::YUV444P10),
        (0, 0, 12) => Some(PresetFormat::YUV444P12),
        (0, 0, 14) => Some(PresetFormat::YUV444P14),
        (0, 0, 16) => Some(PresetFormat::YUV444P16),
        (2, 2, 8) => Some(PresetFormat::YUV410P8),
        (2, 0, 8) => Some(PresetFormat::YUV411P8),
        (0, 1, 8) => Some(PresetFormat::YUV440P8),
        _ => None,
    }
}

/// The YUV float [`PresetFormat`] with the given subsampling for 16-bit
/// (half) or 32-bit (single) float samples; [`None`] for other depths.
#[inline]
fn float_yuv_preset(
    sub_sampling_w: u8,
    sub_sampling_h: u8,
    bits_per_sample: u8,
) -> Option<PresetFormat> {
    Some(match (sub_sampling_w, sub_sampling_h, bits_per_sample) {
        (1, 1, 16) => PresetFormat::YUV420PH,
        (1, 1, 32) => PresetFormat::YUV420PS,
        (1, 0, 16) => PresetFormat::YUV422PH,
        (1, 0, 32) => PresetFormat::YUV422PS,
        (0, 0, 16) => PresetFormat::YUV444PH,
        (0, 0, 32) => PresetFormat::YUV444PS,
        _ => return None,
    })
}

impl WNNM {
    #[inline]
    pub fn invoke<'core>(
        self,
        core: CoreRef<'core>,
        node: &Node<'core>,
        rclip: Option<&Node<'core>>,
    ) -> Result<Node<'core>, VapourSynthError> {
        let source_format = WnnmSourceFormat::from_format(node.info().format);
        let needs_resample = !source_format.wnnm_accepts_plain();

        // Resample to a 32-bit float work clip when the source format isn't
        // directly supported. `resized_node` stays alive while `wnnm_input`
        // borrows it for the WNNM invocation below.
        let resized_node = if needs_resample {
            let work_preset = source_format
                .wnnm_work_preset()
                .expect("WNNM work preset exists for any defined color family");
            let resampler = Bicubic {
                format: Some(work_preset),
                ..Default::default()
            };
            Some(resampler.invoke(core, node)?)
        } else {
            None
        };
        let wnnm_input = resized_node.as_ref().unwrap_or(node);

        let mut arguments = Self::arguments()?;
        arguments.set_node("clip", wnnm_input).map_err(|e| {
            VapourSynthError::PluginArgumentsError {
                plugin: Self::PLUGIN_NAME.to_owned(),
                argument: "clip".to_owned(),
                message: e.to_string(),
            }
        })?;
        if let Some(rclip) = rclip {
            arguments.set_node("rclip", rclip).map_err(|e| {
                VapourSynthError::PluginArgumentsError {
                    plugin: Self::PLUGIN_NAME.to_owned(),
                    argument: "ref".to_owned(),
                    message: e.to_string(),
                }
            })?;
        }
        Self::arguments_set_float_arrays(&mut arguments, vec![("sigma", self.sigma)])?;
        Self::argument_set_ints(
            &mut arguments,
            vec![
                ("block_size", self.block_size),
                ("block_step", self.block_step),
                ("group_size", self.group_size),
                ("bm_range", self.bm_range),
                ("radius", self.radius),
                ("ps_num", self.ps_num),
                ("ps_range", self.ps_range),
                ("residual", self.residual.map(|b| if b { 1 } else { 0 })),
                (
                    "adaptive_aggregation",
                    self.adaptive_aggregation.map(|b| if b { 1 } else { 0 }),
                ),
            ],
        )?;
        let wnnm_node = Self::invoke_and_get_node(core, arguments, Some("clip"))?;

        // Convert the WNNM output back to the original format when a forward
        // resample was performed.
        let node = if needs_resample {
            match source_format.original_preset() {
                Some(original_preset) => {
                    let resampler = Bicubic {
                        format: Some(original_preset),
                        ..Default::default()
                    };
                    resampler.invoke(core, &wnnm_node)?
                },
                // No matching preset (e.g. an unusual float depth): keep the
                // 32-bit float WNNM output unchanged.
                None => wnnm_node,
            }
        } else {
            wnnm_node
        };

        Ok(node)
    }
}

impl VapourSynthPluginScript for WNNM {
    #[inline]
    fn generate_script(&self, node_name: NodeVariableName) -> Result<(Option<Imports>, Vec<Line>)> {
        let mut lines = vec![];

        let line = {
            let mut line = String::new();
            write!(&mut line, "core.vszip.WNNM(clip = {}", node_name)?;
            if let Some(sigma) = &self.sigma {
                write!(&mut line, ", sigma = [{}]", sigma.iter().join(", "))?;
            }
            if let Some(block_size) = self.block_size {
                write!(&mut line, ", block_size = {}", block_size as i64)?;
            }
            if let Some(block_step) = self.block_step {
                write!(&mut line, ", block_step = {}", block_step as i64)?;
            }
            if let Some(group_size) = self.group_size {
                write!(&mut line, ", group_size = {}", group_size as i64)?;
            }
            if let Some(bm_range) = self.bm_range {
                write!(&mut line, ", bm_range = {}", bm_range as i64)?;
            }
            if let Some(radius) = self.radius {
                write!(&mut line, ", radius = {}", radius as i64)?;
            }
            if let Some(ps_num) = self.ps_num {
                write!(&mut line, ", ps_num = {}", ps_num as i64)?;
            }
            if let Some(ps_range) = self.ps_range {
                write!(&mut line, ", ps_range = {}", ps_range as i64)?;
            }
            if let Some(residual) = self.residual {
                write!(&mut line, ", residual = {}", residual as i64)?;
            }
            if let Some(adaptive_aggregation) = self.adaptive_aggregation {
                write!(
                    &mut line,
                    ", adaptive_aggregation = {}",
                    adaptive_aggregation as i64
                )?;
            }
            if let Some(rclip_name) = &self.rclip_name {
                write!(&mut line, ", rclip = {}", rclip_name)?;
            }
            write!(&mut line, ")")?;
            line
        };

        lines.push(Line::Expression(node_name, line));

        Ok((None, lines))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a [`WnnmSourceFormat`] without a VapourSynth core.
    fn format(
        color_family: ColorFamily,
        sample_type: SampleType,
        bits_per_sample: u8,
        sub_sampling_w: u8,
        sub_sampling_h: u8,
    ) -> WnnmSourceFormat {
        WnnmSourceFormat {
            color_family,
            sample_type,
            bits_per_sample,
            sub_sampling_w,
            sub_sampling_h,
        }
    }

    #[test]
    fn wnnm_accepts_plain_only_for_32_bit_float() {
        // Any color family with 32-bit float samples is accepted directly.
        assert!(format(ColorFamily::YUV, SampleType::Float, 32, 1, 1).wnnm_accepts_plain());
        assert!(format(ColorFamily::Gray, SampleType::Float, 32, 0, 0).wnnm_accepts_plain());
        assert!(format(ColorFamily::RGB, SampleType::Float, 32, 0, 0).wnnm_accepts_plain());

        // Everything else must be resampled.
        assert!(!format(ColorFamily::YUV, SampleType::Integer, 16, 1, 1).wnnm_accepts_plain());
        assert!(!format(ColorFamily::YUV, SampleType::Integer, 12, 1, 1).wnnm_accepts_plain());
        assert!(!format(ColorFamily::YUV, SampleType::Float, 16, 1, 1).wnnm_accepts_plain());
        assert!(!format(ColorFamily::Gray, SampleType::Integer, 8, 0, 0).wnnm_accepts_plain());
    }

    #[test]
    fn wnnm_work_preset_maps_family_and_subsampling() {
        // YUV subsampling is preserved when a 32-bit float preset exists.
        let yuv420 = format(ColorFamily::YUV, SampleType::Integer, 12, 1, 1);
        assert_eq!(yuv420.wnnm_work_preset(), Some(PresetFormat::YUV420PS));

        let yuv422 = format(ColorFamily::YUV, SampleType::Integer, 10, 1, 0);
        assert_eq!(yuv422.wnnm_work_preset(), Some(PresetFormat::YUV422PS));

        let yuv444 = format(ColorFamily::YUV, SampleType::Integer, 16, 0, 0);
        assert_eq!(yuv444.wnnm_work_preset(), Some(PresetFormat::YUV444PS));

        // YUV410 (4:2:0 subsampling 2,2) has no 32-bit float preset: falls
        // back to 4:2:0.
        let yuv410 = format(ColorFamily::YUV, SampleType::Integer, 8, 2, 2);
        assert_eq!(yuv410.wnnm_work_preset(), Some(PresetFormat::YUV420PS));

        let gray = format(ColorFamily::Gray, SampleType::Integer, 8, 0, 0);
        assert_eq!(gray.wnnm_work_preset(), Some(PresetFormat::GrayS));

        let rgb = format(ColorFamily::RGB, SampleType::Integer, 8, 0, 0);
        assert_eq!(rgb.wnnm_work_preset(), Some(PresetFormat::RGBS));

        // Undefined has no work format.
        assert_eq!(
            format(ColorFamily::Undefined, SampleType::Integer, 8, 0, 0).wnnm_work_preset(),
            None,
        );
    }

    #[test]
    fn wnnm_original_preset_preserves_colour_family_depth_and_sample_type() {
        // 12-bit integer YUV source: back to the same 12-bit preset.
        let yuv420_p12 = format(ColorFamily::YUV, SampleType::Integer, 12, 1, 1);
        assert_eq!(yuv420_p12.original_preset(), Some(PresetFormat::YUV420P12));

        // 10-bit 4:2:2 integer source.
        let yuv422_p10 = format(ColorFamily::YUV, SampleType::Integer, 10, 1, 0);
        assert_eq!(yuv422_p10.original_preset(), Some(PresetFormat::YUV422P10));

        // 16-bit 4:4:4 integer source.
        let yuv444_p16 = format(ColorFamily::YUV, SampleType::Integer, 16, 0, 0);
        assert_eq!(yuv444_p16.original_preset(), Some(PresetFormat::YUV444P16));

        // Gray integer depths.
        let gray10 = format(ColorFamily::Gray, SampleType::Integer, 10, 0, 0);
        assert_eq!(gray10.original_preset(), Some(PresetFormat::Gray10));

        // RGB 8/10/16-bit integer depths.
        let rgb8 = format(ColorFamily::RGB, SampleType::Integer, 8, 0, 0);
        assert_eq!(rgb8.original_preset(), Some(PresetFormat::RGB24));

        let rgb16 = format(ColorFamily::RGB, SampleType::Integer, 16, 0, 0);
        assert_eq!(rgb16.original_preset(), Some(PresetFormat::RGB48));

        // Half-float sources round-trip to their half-float preset.
        let yuv420_ph = format(ColorFamily::YUV, SampleType::Float, 16, 1, 1);
        assert_eq!(yuv420_ph.original_preset(), Some(PresetFormat::YUV420PH));

        // Undefined formats have no original preset.
        assert_eq!(
            format(ColorFamily::Undefined, SampleType::Integer, 8, 0, 0).original_preset(),
            None,
        );

        // Unusual float depths without a preset map to None.
        let odd_float = format(ColorFamily::YUV, SampleType::Float, 24, 1, 1);
        assert_eq!(odd_float.original_preset(), None);
    }
}
