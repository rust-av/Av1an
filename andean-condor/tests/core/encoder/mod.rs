mod cli;
#[cfg(target_os = "linux")]
mod public_encoding;

#[path = "../../../src/core/encoder/tests/support.rs"]
mod support;

use andean_condor::{
    core::{
        encoder::{PassStatistics, parse::valid_params},
        input::clip_info::ClipInfo,
    },
    models::encoder::{
        Encoder, EncoderBase, EncoderPasses, cli_parameter::CLIParameter, photon_noise::PhotonNoise,
    },
};
use av1_grain::TransferFunction;
use std::{
    path::Path,
    process::{Command, Stdio},
    thread,
    time::Instant,
};

fn clip_info() -> ClipInfo {
    use av_decoders::{ColorPrimaries, ColorSpace, TransferCharacteristic};
    ClipInfo {
        num_frames: 24,
        format_info: andean_condor::core::input::pixel_format::PixelFormat::VapourSynth {
            bit_depth: 10,
        },
        frame_rate: av_format::rational::Rational64::new(24, 1),
        resolution: (1920, 1080),
        color_range: None,
        color_primaries: Some(ColorPrimaries::BT2020),
        color_transfer_characteristic: Some(TransferCharacteristic::SMPTE2084),
        color_space: Some(ColorSpace::BT2020NCL),
        chroma_location: None,
        mastering_display: None,
        content_light_level: None,
    }
}
