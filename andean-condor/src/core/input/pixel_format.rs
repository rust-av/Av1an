use serde::{Deserialize, Serialize};

use crate::ffmpeg::FFPixelFormat;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum PixelFormat {
    VapourSynth { bit_depth: usize },
    FFmpeg { format: FFPixelFormat },
}

impl PixelFormat {
    #[inline]
    pub fn as_bit_depth(&self) -> anyhow::Result<usize> {
        match self {
            PixelFormat::VapourSynth {
                bit_depth,
            } => Ok(*bit_depth),
            PixelFormat::FFmpeg {
                ..
            } => Err(anyhow::anyhow!("failed to get bit depth; wrong input type")),
        }
    }

    #[inline]
    pub fn as_pixel_format(&self) -> anyhow::Result<FFPixelFormat> {
        match self {
            PixelFormat::VapourSynth {
                ..
            } => Err(anyhow::anyhow!("failed to get bit depth; wrong input type")),
            PixelFormat::FFmpeg {
                format,
            } => Ok(*format),
        }
    }
}
