//! Describes a decoded clip: its geometry, frame rate and colour metadata.
//!
//! The colour fields reuse [`av_decoders`]' types, which mirror the
//! FFmpeg/libavutil (H.273) numbering both decoders report.

pub use av_decoders::{
    ChromaLocation,
    ColorPrimaries,
    ColorSpace,
    ContentLightLevel,
    MasteringDisplayMetadata,
    TransferCharacteristic,
};
use av_format::rational::Rational64;

use crate::core::input::{color_range::ColorRange, pixel_format::PixelFormat};

type Width = u32;
type Height = u32;

pub use av1_grain::TransferFunction;

#[derive(Debug, Clone, Copy)]
pub struct ClipInfo {
    pub num_frames:                    usize,
    pub format_info:                   PixelFormat,
    pub frame_rate:                    Rational64,
    pub resolution:                    (Width, Height),
    pub color_range:                   Option<ColorRange>,
    /// The source's colour primaries (gamut), if the decoder reported them.
    pub color_primaries:               Option<ColorPrimaries>,
    /// The source's transfer characteristics, if the decoder reported them.
    pub color_transfer_characteristic: Option<TransferCharacteristic>,
    /// The source's matrix coefficients, if the decoder reported them.
    pub color_space:                   Option<ColorSpace>,
    /// The source's chroma sample location, if the decoder reported it.
    pub chroma_location:               Option<ChromaLocation>,
    /// SMPTE ST 2086 mastering display metadata, if the source carries it.
    pub mastering_display:             Option<MasteringDisplayMetadata>,
    /// CTA-861.3 content light level metadata, if the source carries it.
    pub content_light_level:           Option<ContentLightLevel>,
}

impl ClipInfo {
    /// The two-valued transfer function photon noise generation supports.
    #[inline]
    pub fn photon_noise_transfer_function(&self) -> TransferFunction {
        match self.color_transfer_characteristic {
            Some(TransferCharacteristic::SMPTE2084) => TransferFunction::SMPTE2084,
            _ => TransferFunction::BT1886,
        }
    }

    /// The transfer function photon noise generation should use, accounting for
    /// a transfer characteristic the user put in the encoder parameters.
    ///
    /// An explicit PQ transfer means the user wants HDR; an explicit SDR
    /// transfer means they want SDR.
    #[inline]
    pub fn transfer_function_params_adjusted(&self, enc_params: &[String]) -> TransferFunction {
        if enc_params.iter().any(|p| {
            let p = p.to_ascii_lowercase();
            p == "pq" || p.ends_with("=pq") || p.ends_with("smpte2084")
        }) {
            return TransferFunction::SMPTE2084;
        }
        if enc_params.iter().any(|p| {
            let p = p.to_ascii_lowercase();
            // If the user specified an SDR transfer characteristic, assume they want to
            // encode to SDR.
            p.ends_with("bt709")
                || p.ends_with("bt.709")
                || p.ends_with("bt601")
                || p.ends_with("bt.601")
                || p.contains("smpte240")
                || p.contains("smpte170")
        }) {
            return TransferFunction::BT1886;
        }
        self.photon_noise_transfer_function()
    }
}
