use anyhow::{Context, Result, bail};
use av_format::rational::Rational64;
use vapoursynth::{
    api::API,
    core::CoreRef,
    node::Node,
    prelude::{Environment, Property},
    video_info::VideoInfo,
};

use crate::core::input::{clip_info::ClipInfo, color_range::ColorRange, pixel_format::PixelFormat};

pub mod plugins;
pub mod script_builder;
pub mod vapoursynth_filters;

#[derive(Debug, Clone, thiserror::Error)]
pub enum VapourSynthError {
    #[error("VapourSynth internal error ({message})")]
    Internal { message: String },
    #[error("VapourSynth Plugin {plugin} not found")]
    PluginNotFound { plugin: String },
    #[error("VapourSynth Plugin {plugin} failed to load: {message}")]
    PluginLoadError { plugin: String, message: String },
    #[error("VapourSynth Plugin {plugin} function {function} not found")]
    PluginFunctionNotFound { plugin: String, function: String },
    #[error("VapourSynth Plugin {plugin} failed to set arguments: {message}")]
    PluginArgumentsError {
        plugin:   String,
        argument: String,
        message:  String,
    },
    #[error("VapourSynth Plugin {plugin} failed to execute function {function}: {message}")]
    PluginFunctionError {
        plugin:   String,
        function: String,
        message:  String,
    },
    #[error("VapourSynth Plugin {plugin} failed to import video: {message}")]
    VideoImportError { plugin: String, message: String },
}

#[inline]
pub fn get_api() -> anyhow::Result<API, VapourSynthError> {
    API::get().ok_or_else(|| VapourSynthError::Internal {
        message: "Failed to get VapourSynth API".to_owned(),
    })
}

#[inline]
pub fn get_environment() -> anyhow::Result<Environment, VapourSynthError> {
    let env = Environment::new().map_err(|e| VapourSynthError::Internal {
        message: format!("Failed to get VapourSynth Environment {e}"),
    })?;

    Ok(env)
}

#[inline]
pub fn get_core<'core>(
    environment: &'core Environment,
) -> anyhow::Result<CoreRef<'core>, VapourSynthError> {
    let core = environment.get_core().map_err(|e| VapourSynthError::Internal {
        message: format!("Failed to get VapourSynth Core {e}"),
    })?;

    Ok(core)
}

/// Reads one of the integer colour properties from a node's first frame.
fn get_color_prop(node: &Node, key: &str) -> Option<i64> {
    let first_frame = node.get_frame(0).ok()?;
    first_frame.props().get::<i64>(key).ok()
}

#[inline]
pub fn get_clip_info(node: &Node) -> Result<ClipInfo> {
    let info = node.info();

    Ok(ClipInfo {
        num_frames:                    get_num_frames(&info)?,
        format_info:                   PixelFormat::VapourSynth {
            bit_depth: get_bit_depth(&info)?,
        },
        frame_rate:                    get_frame_rate(&info)?,
        resolution:                    get_resolution(&info)?,
        color_range:                   get_color_range(node)?,
        color_primaries:               get_color_prop(node, "_Primaries")
            .and_then(|value| i32::try_from(value).ok())
            .and_then(av_decoders::ColorPrimaries::from_raw),
        color_transfer_characteristic: get_color_prop(node, "_Transfer")
            .and_then(|value| i32::try_from(value).ok())
            .and_then(av_decoders::TransferCharacteristic::from_raw),
        color_space:                   get_color_prop(node, "_Matrix")
            .and_then(|value| i32::try_from(value).ok())
            .and_then(av_decoders::ColorSpace::from_raw),
        chroma_location:               get_color_prop(node, "_ChromaLocation")
            .and_then(map_vapoursynth_chroma_location),
        // VapourSynth frame properties carry no mastering display or content
        // light level metadata.
        mastering_display:             None,
        content_light_level:           None,
    })
}

/// Get the number of frames from an environment that has already been
/// evaluated on a script.
fn get_num_frames(info: &VideoInfo) -> anyhow::Result<usize> {
    let num_frames = {
        if Property::Variable == info.resolution {
            bail!("Cannot output clips with varying dimensions");
        }
        if Property::Variable == info.framerate {
            bail!("Cannot output clips with varying framerate");
        }

        info.num_frames
    };

    assert!(num_frames != 0, "vapoursynth reported 0 frames");

    Ok(num_frames)
}

fn get_frame_rate(info: &VideoInfo) -> anyhow::Result<Rational64> {
    match info.framerate {
        Property::Variable => bail!("Cannot output clips with varying framerate"),
        Property::Constant(fps) => Ok(Rational64::new(
            fps.numerator as i64,
            fps.denominator as i64,
        )),
    }
}

/// Get the bit depth from an environment that has already been
/// evaluated on a script.
fn get_bit_depth(info: &VideoInfo) -> anyhow::Result<usize> {
    let bits_per_sample = info.format.bits_per_sample();

    Ok(bits_per_sample as usize)
}

/// Get the resolution from an environment that has already been
/// evaluated on a script.
fn get_resolution(info: &VideoInfo) -> anyhow::Result<(u32, u32)> {
    let resolution = {
        match info.resolution {
            Property::Variable => {
                bail!("Cannot output clips with variable resolution");
            },
            Property::Constant(x) => x,
        }
    };

    Ok((resolution.width as u32, resolution.height as u32))
}

/// Get the color range for a VideoNode.
fn get_color_range(node: &Node) -> anyhow::Result<Option<ColorRange>> {
    let frame = node.get_frame(0).context("get_color_range")?;
    let color_range = frame
        .props()
        .get::<i64>("_ColorRange")
        .ok()
        .and_then(map_vapoursynth_color_range);

    Ok(color_range)
}

#[inline]
const fn map_vapoursynth_chroma_location(
    chroma_location: i64,
) -> Option<av_decoders::ChromaLocation> {
    // RATIONALE: VapourSynth numbers `_ChromaLocation` from 0 (left) upwards,
    // while the FFmpeg/H.273 numbering av-decoders uses starts at 0 for
    // "unspecified", so every value is shifted by one.
    av_decoders::ChromaLocation::from_raw(match chroma_location {
        0..=5 => chroma_location as i32 + 1,
        _ => return None,
    })
}

#[inline]
const fn map_vapoursynth_color_range(color_range: i64) -> Option<ColorRange> {
    match color_range {
        // RATIONALE: VapourSynth frame props use the legacy convention where
        // 0 means full-range and 1 means limited-range.
        0 => Some(ColorRange::Full),
        1 => Some(ColorRange::Limited),
        _ => None,
    }
}
