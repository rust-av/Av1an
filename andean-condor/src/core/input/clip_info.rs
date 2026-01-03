pub use av1_grain::TransferFunction;
use av_format::rational::Rational64;

use crate::core::input::pixel_format::PixelFormat;

type Width = u32;
type Height = u32;

#[derive(Debug, Clone, Copy)]
pub struct ClipInfo {
    pub num_frames:               usize,
    pub format_info:              PixelFormat,
    pub frame_rate:               Rational64,
    pub resolution:               (Width, Height),
    /// This is overly simplified because we currently only use it for photon
    /// noise gen, which only supports two transfer functions
    pub transfer_characteristics: TransferFunction,
}

impl ClipInfo {
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
        self.transfer_characteristics
    }
}
