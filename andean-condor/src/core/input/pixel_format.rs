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
                format,
            } => match format {
                FFPixelFormat::GBRP
                | FFPixelFormat::YUV440P
                | FFPixelFormat::YUV444P
                | FFPixelFormat::YUVA420P
                | FFPixelFormat::YUVJ420P
                | FFPixelFormat::YUVJ422P
                | FFPixelFormat::YUVJ444P
                | FFPixelFormat::YUV422P
                | FFPixelFormat::GRAY8
                | FFPixelFormat::NV12
                | FFPixelFormat::NV16
                | FFPixelFormat::NV21
                | FFPixelFormat::YUV420P => Ok(8),
                FFPixelFormat::GRAY10LE
                | FFPixelFormat::NV20LE
                | FFPixelFormat::YUV420P10LE
                | FFPixelFormat::GBRP10LE
                | FFPixelFormat::YUV422P10LE
                | FFPixelFormat::YUV440P10LE
                | FFPixelFormat::YUV444P10LE => Ok(10),
                FFPixelFormat::YUV420P12LE
                | FFPixelFormat::GRAY12L
                | FFPixelFormat::GRAY12LE
                | FFPixelFormat::GBRP12L
                | FFPixelFormat::GBRP12LE
                | FFPixelFormat::YUV422P12LE
                | FFPixelFormat::YUV440P12LE
                | FFPixelFormat::YUV444P12LE => Ok(12),
            },
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
