pub mod dav1d_fg;
pub mod format;

use std::fmt::Write;

use anyhow::Result;
use av1_grain::TransferFunction;
use serde::{Deserialize, Serialize};
use vapoursynth::{core::CoreRef, format::ColorFamily, map::ValueType, node::Node};

use self::format::FgsFormat;
use crate::{
    models::encoder::photon_noise::PhotonNoise,
    vapoursynth::{
        VapourSynthError,
        get_clip_info,
        plugins::{Plugin, PluginFunction, resize::bicubic::Bicubic},
        script_builder::{
            NodeVariableName,
            VapourSynthPluginScript,
            script::{Imports, Line},
        },
    },
};

/// VapourSynth FGS (Film Grain Synthesis) plugin using dav1d's film grain
/// engine.
///
/// Generates a binary film-grain data blob from [`PhotonNoise`] parameters
/// and clip metadata, then invokes the `com.vs.fgs` plugin's `FGS` function.
///
/// The binary grain data is produced internally (see [`dav1d_fg`]) so
/// callers work with the same [`PhotonNoise`] model used elsewhere in Andean
/// Condor, rather than with raw grain-table text files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FGS {
    /// Photon noise parameters used to generate the film grain data.
    pub photon_noise: PhotonNoise,

    /// When `true`, the random seed is overridden per frame using a curated
    /// list of cherry-picked seeds, producing dynamic grain variation even
    /// with a single grain data entry.
    ///
    /// Defaults to `false` (use the seed from the generated grain data).
    pub dynamic_seed: Option<bool>,

    /// SIMD instruction-set mask passed to `dav1d_set_cpu_flags_mask`.
    /// `0` disables all SIMD, `~0u` (default) enables all available features.
    pub simd_mask: Option<u32>,
}

impl Plugin for FGS {
    const PLUGIN_NAME: &'static str = "vs-fgs";
    const PLUGIN_ID: &'static str = "com.vs.fgs";
    const PLUGIN_DOCS: Option<&'static str> = Some("https://pypi.org/project/vsfgs");
}

impl PluginFunction for FGS {
    const FUNCTION_NAME: &'static str = "FGS";
    const FUNCTION_DOCS: Option<&'static str> = Some("https://github.com/PingWer/vs-fgs");
    const REQUIRED_ARGUMENTS: &'static [(&'static str, &'static ValueType)] =
        &[("clip", &ValueType::VideoNode), ("fgs_data", &ValueType::Data)];
    const OPTIONAL_ARGUMENTS: &'static [(&'static str, &'static ValueType)] =
        &[("dynamic_seed", &ValueType::Int), ("simd_mask", &ValueType::Int)];
}

impl FGS {
    /// Invoke the FGS filter on the given clip, applying film-grain data
    /// generated from the stored [`PhotonNoise`] parameters and the clip's
    /// metadata.
    ///
    /// Only accepts integer YUV formats at 8, 10 or 12 bits per sample. When
    /// the source is anything else (for example the 32-bit float `YUV420PS`
    /// noise-detector source, or a 16-bit integer clip), the clip is
    /// resized to the matching 12-bit integer YUV preset (`resize.Bicubic`,
    /// no dithering), FGS is applied, and then the result is resized back
    /// to the original format.
    #[inline]
    pub(crate) fn invoke<'core>(
        self,
        core: CoreRef<'core>,
        node: &Node<'core>,
    ) -> Result<Node<'core>, VapourSynthError> {
        let clip_info = get_clip_info(node).map_err(|e| VapourSynthError::PluginFunctionError {
            plugin:   Self::PLUGIN_NAME.to_owned(),
            function: Self::FUNCTION_NAME.to_owned(),
            message:  e.to_string(),
        })?;

        let (width, height) = clip_info.resolution;
        let fgs_data = self.build_grain_binary(
            Some(width),
            Some(height),
            clip_info.photon_noise_transfer_function(),
            clip_info.color_range,
        )?;

        let source_format = FgsFormat::from_format(node.info().format);
        let needs_resample =
            !source_format.fgs_accepts_plain() && source_format.color_family == ColorFamily::YUV;

        // Resample to a 12-bit integer work clip when the source format isn't
        // directly supported. `resized_node` stays alive while `fgs_input`
        // borrows it for the FGS invocation below.
        let resized_node = if needs_resample {
            let work_preset = source_format
                .fgs_work_preset()
                .expect("FGS work preset exists for any YUV source");
            let resampler = Bicubic {
                format: Some(work_preset),
                ..Default::default()
            };
            Some(resampler.invoke(core, node)?)
        } else {
            None
        };
        let fgs_input = resized_node.as_ref().unwrap_or(node);

        let mut arguments = Self::arguments()?;

        arguments.set_node("clip", fgs_input).map_err(|e| {
            VapourSynthError::PluginArgumentsError {
                plugin:   Self::PLUGIN_NAME.to_owned(),
                argument: "clip".to_owned(),
                message:  e.to_string(),
            }
        })?;

        Self::arguments_set(&mut arguments, vec![("fgs_data", Some(fgs_data))])?;

        // The vs-fgs plugin reads its optional arguments with null error
        // pointers, so omitted arguments produce "Property read unsuccessful
        // due to missing key but no error output" errors and are treated as
        // 0. Get the plugin's registered arguments list and set exactly
        // the arguments it accepts so we stay compatible with every plugin
        // version (e.g. `seed_list` added in vs-fgs v0.7.0+).
        let supported_arguments = Self::plugin_function_arguments(core)?;
        let is_supported =
            |name: &str| supported_arguments.iter().any(|argument| argument.name == name);
        Self::argument_set_ints(&mut arguments, vec![
            (
                "dynamic_seed",
                is_supported("dynamic_seed").then_some(self.dynamic_seed.unwrap_or(false) as i64),
            ),
            (
                "simd_mask",
                is_supported("simd_mask").then_some(self.simd_mask.unwrap_or(!0) as i64),
            ),
            ("seed_list", is_supported("seed_list").then_some(0)),
        ])?;

        let fgs_node = Self::invoke_and_get_node(core, arguments, Some("clip"))?;

        // Convert the FGS output back to the original format when a forward
        // resample was performed.
        let node = if needs_resample {
            match source_format.original_preset() {
                Some(original_preset) => {
                    let resampler = Bicubic {
                        format: Some(original_preset),
                        ..Default::default()
                    };
                    resampler.invoke(core, &fgs_node)?
                },
                // No matching preset (e.g. an unusual float depth): keep the
                // 12-bit FGS output unchanged.
                None => fgs_node,
            }
        } else {
            fgs_node
        };

        Ok(node)
    }
}

impl VapourSynthPluginScript for FGS {
    #[inline]
    fn generate_script(&self, node_name: NodeVariableName) -> Result<(Option<Imports>, Vec<Line>)> {
        let mut lines = vec![];

        // Build the grain binary with sensible defaults for script-gen.
        // Script-gen has no access to clip metadata, so we use the
        // PhotonNoise-provided values or standard SDR defaults.
        //
        //   width / height   – from PhotonNoise or 1920×1080
        //   transfer         – BT.1886 (SDR, the most common case)
        //   color_range      – limited range (same reasoning)
        let fgs_data = self.build_grain_binary(None, None, TransferFunction::BT1886, None)?;

        // Format the binary as a hex literal.
        let mut hex_string = String::with_capacity(fgs_data.len() * 2);
        for byte in &fgs_data {
            write!(&mut hex_string, "{byte:02x}")?;
        }

        let mut line = format!(
            "core.fgs.FGS(clip = {}, fgs_data = bytes.fromhex(\"{}\")",
            node_name, hex_string,
        );

        // Always pass dynamic_seed and simd_mask on every call (matching the
        // Python reference); the vs-fgs plugin reads them with null error
        // pointers and warns when they are missing.
        write!(
            line,
            ", dynamic_seed = {}",
            self.dynamic_seed.unwrap_or(false) as i64
        )?;
        write!(
            line,
            ", simd_mask = {}",
            self.simd_mask.unwrap_or(!0) as i64
        )?;
        // Errors without setting seed_list to 0
        write!(line, ", seed_list = 0")?;

        line.push(')');
        lines.push(Line::Expression(node_name, line));

        Ok((None, lines))
    }
}
