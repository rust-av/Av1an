use anyhow::{bail, Context, Result};
use av_format::rational::Rational64;
use vapoursynth::{api::API, node::Node, prelude::Property, video_info::VideoInfo};

use crate::{ClipInfo, InputPixelFormat};

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

/// Get the transfer characteristics for a VideoNode
#[inline]
pub fn get_transfer(node: &Node) -> Result<u8> {
    let first_frame = node.get_frame(0).context("get_transfer")?;
    let transfer = first_frame.props().get::<i64>("_Transfer").map(|val| val as u8).unwrap_or(2);

    Ok(transfer)
}

#[inline]
pub fn get_clip_info(node: &Node) -> Result<ClipInfo> {
    let info = node.info();

    Ok(ClipInfo {
        num_frames:               get_num_frames(&info)?,
        format_info:              InputPixelFormat::VapourSynth {
            bit_depth: get_bit_depth(&info)?,
        },
        frame_rate:               get_frame_rate(&info)?,
        resolution:               get_resolution(&info)?,
        transfer_characteristics: match get_transfer(node)? {
            16 => av1_grain::TransferFunction::SMPTE2084,
            _ => av1_grain::TransferFunction::BT1886,
        },
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
