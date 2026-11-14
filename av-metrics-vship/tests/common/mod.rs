//! Deterministic in-memory synthetic clip generation for tests and benchmarks.
//!
//! No media files are read or written: content is generated as a y4m byte
//! stream in memory and fed to `av-decoders`' always-available Y4M path. That
//! keeps the suite runnable anywhere, including CI with no GPU, no libvship and
//! no FFMS2.
//!
//! Every value comes from a fixed-seed linear congruential generator, so the
//! same parameters always produce byte-identical output on every platform.
//!
//! This module is compiled into both the `correctness` test target and the
//! `scoring` benchmark. Some helpers exist for only one of the two, so the
//! "unused" lints are relaxed here rather than duplicating the generator.

#![allow(
    dead_code,
    unused_imports,
    reason = "shared by the test and benchmark targets, which use different subsets"
)]

use std::io::Cursor;

use av_decoders::{Decoder, DecoderImpl, Y4mDecoder};
use v_frame::chroma::ChromaSubsampling;
use y4m::{Colorspace, EncoderBuilder, Frame, Ratio};

/// A fixed-seed linear congruential generator.
///
/// Chosen over a hash so the sequence is trivially reproducible and independent
/// of any standard library implementation detail.
struct Lcg(u64);

impl Lcg {
    /// Parameters from Numerical Recipes; `modulus` is 2^64.
    const MULTIPLIER: u64 = 6_364_136_223_846_793_005;
    const INCREMENT: u64 = 1_442_695_040_888_963_407;

    /// Create a generator with the given seed.
    const fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// Produce the next value in the sequence.
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(Self::MULTIPLIER).wrapping_add(Self::INCREMENT);
        self.0
    }

    /// Produce a value in `[0, bound)`.
    fn below(&mut self, bound: u64) -> u64 {
        if bound == 0 {
            return 0;
        }
        self.next_u64() % bound
    }
}

/// The y4m colorspace matching a bit depth and chroma sampling.
///
/// y4m has no 16-bit colorspace, so 16-bit tests fall back to what the format
/// can express.
fn colorspace(bit_depth: u8, chroma_sampling: ChromaSubsampling) -> Colorspace {
    match (bit_depth, chroma_sampling) {
        (8, ChromaSubsampling::Yuv422) => Colorspace::C422,
        (8, ChromaSubsampling::Yuv444) => Colorspace::C444,
        (8, _) => Colorspace::C420,
        (10, ChromaSubsampling::Yuv422) => Colorspace::C422p10,
        (10, ChromaSubsampling::Yuv444) => Colorspace::C444p10,
        (10, _) => Colorspace::C420p10,
        (12, ChromaSubsampling::Yuv422) => Colorspace::C422p12,
        (12, ChromaSubsampling::Yuv444) => Colorspace::C444p12,
        (12, _) => Colorspace::C420p12,
        // Monochrome and 16-bit have no faithful y4m equivalent; fall back to
        // 8-bit 4:2:0, which every decoder accepts.
        _ => Colorspace::C420,
    }
}

/// How much distortion to apply to the generated clip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Distortion {
    /// No distortion. Scoring against an identical copy should yield the
    /// metric's ideal value.
    None,
    /// Mild noise. Should score near the ideal.
    Mild,
    /// Stronger noise.
    Moderate,
    /// Heaviest perturbation. Should score lowest of the three.
    Severe,
}

impl Distortion {
    /// Peak perturbation amplitude for this level.
    const fn amplitude(self, max_value: u32) -> u32 {
        match self {
            Self::None => 0,
            Self::Mild => max_value / 64,
            Self::Moderate => max_value / 12,
            Self::Severe => max_value / 3,
        }
    }
}

/// Generate a deterministic y4m stream in memory.
///
/// The content is a horizontal luma gradient, a box that sweeps across the
/// frame and per-frame noise from a fixed seed. The gradient and the box give a
/// temporal metric something to track, and the noise gives each frame
/// variation. `distortion` perturbs the result, so one generator produces both
/// a reference and a degraded copy.
#[must_use]
pub fn synthetic_y4m(
    width: usize,
    height: usize,
    bit_depth: u8,
    chroma_sampling: ChromaSubsampling,
    frames: usize,
    seed: u64,
    distortion: Distortion,
) -> Vec<u8> {
    let effective_bit_depth = match bit_depth {
        0..=8 => 8,
        9 | 11 => 10,
        16 => 8,
        _ => bit_depth,
    };
    let sample_bytes = if effective_bit_depth > 8 { 2 } else { 1 };
    let max_value = (1u32 << effective_bit_depth) - 1;
    let amplitude = distortion.amplitude(max_value);

    let (chroma_h, chroma_v) = chroma_decimation(chroma_sampling);
    let chroma_width = (width / chroma_h).max(1);
    let chroma_height = (height / chroma_v).max(1);

    let mut buffer = Vec::with_capacity(
        frames * (width * height + 2 * chroma_width * chroma_height) * sample_bytes + 64,
    );

    let mut encoder = EncoderBuilder::new(width, height, Ratio::new(24, 1))
        .with_colorspace(colorspace(effective_bit_depth, chroma_sampling))
        .write_header(Cursor::new(&mut buffer))
        .expect("y4m header is well-formed for supported parameters");

    let mut luma = vec![0u8; width * height * sample_bytes];
    let mut chroma_u = vec![0u8; chroma_width * chroma_height * sample_bytes];
    let mut chroma_v_plane = vec![0u8; chroma_width * chroma_height * sample_bytes];

    for frame_index in 0..frames {
        let mut random = Lcg::new(seed ^ (frame_index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));

        // Luma: horizontal gradient plus a box that sweeps across the frame.
        let box_left = (frame_index * width / frames.max(1)) % width;
        let box_width = (width / 8).max(1);

        for y in 0..height {
            for x in 0..width {
                let gradient = (x as u32 * max_value) / width.max(1) as u32;
                let in_box = x >= box_left && x < box_left + box_width;
                let base = if in_box {
                    gradient / 2 + max_value / 4
                } else {
                    gradient
                };

                let noise = if amplitude > 0 {
                    random.below(u64::from(amplitude) * 2 + 1) as i64 - i64::from(amplitude)
                } else {
                    0
                };

                write_sample(
                    &mut luma,
                    (y * width + x) * sample_bytes,
                    sample_bytes,
                    clamp_sample(i64::from(base) + noise, max_value),
                );
            }
        }

        // Chroma: a two-dimensional ramp, which the metrics treat differently
        // from luma.
        for y in 0..chroma_height {
            for x in 0..chroma_width {
                let u = (x as u32 * 3 + y as u32) % (max_value + 1);
                let v = (y as u32 * 3 + x as u32) % (max_value + 1);
                let noise = if amplitude > 0 {
                    random.below(u64::from(amplitude) * 2 + 1) as i64 - i64::from(amplitude)
                } else {
                    0
                };

                write_sample(
                    &mut chroma_u,
                    (y * chroma_width + x) * sample_bytes,
                    sample_bytes,
                    clamp_sample(i64::from(u) + noise, max_value),
                );
                write_sample(
                    &mut chroma_v_plane,
                    (y * chroma_width + x) * sample_bytes,
                    sample_bytes,
                    clamp_sample(i64::from(v) + noise, max_value),
                );
            }
        }

        let frame = Frame::new([&luma, &chroma_u, &chroma_v_plane], None);
        encoder
            .write_frame(&frame)
            .expect("generated frame dimensions match the header");
    }

    buffer
}

/// Horizontal and vertical chroma decimation for a subsampling.
const fn chroma_decimation(sampling: ChromaSubsampling) -> (usize, usize) {
    match sampling {
        ChromaSubsampling::Yuv420 => (2, 2),
        ChromaSubsampling::Yuv422 => (2, 1),
        ChromaSubsampling::Yuv444 => (1, 1),
        _ => (1, 1),
    }
}

/// Clamp a possibly negative sample into the valid range.
#[inline]
fn clamp_sample(value: i64, max_value: u32) -> u32 {
    if value < 0 {
        0
    } else if value > i64::from(max_value) {
        max_value
    } else {
        value as u32
    }
}

/// Write one sample in little-endian order.
#[inline]
fn write_sample(buffer: &mut [u8], offset: usize, sample_bytes: usize, value: u32) {
    if sample_bytes == 1 {
        buffer[offset] = value as u8;
    } else {
        buffer[offset] = (value & 0xFF) as u8;
        buffer[offset + 1] = ((value >> 8) & 0xFF) as u8;
    }
}

/// A generated clip, ready to be decoded.
pub struct SyntheticClip {
    /// The raw y4m bytes.
    pub data: Vec<u8>,
    /// Clip width in pixels.
    pub width: usize,
    /// Clip height in pixels.
    pub height: usize,
    /// Bits per channel actually used.
    pub bit_depth: u8,
    /// Chroma subsampling actually used.
    pub chroma_sampling: ChromaSubsampling,
}

impl SyntheticClip {
    /// Generate a clip with the given parameters.
    #[must_use]
    pub fn generate(
        width: usize,
        height: usize,
        bit_depth: u8,
        chroma_sampling: ChromaSubsampling,
        frames: usize,
        distortion: Distortion,
    ) -> Self {
        Self::generate_with_seed(
            width,
            height,
            bit_depth,
            chroma_sampling,
            frames,
            0x5EED_1234_ABCD_0001,
            distortion,
        )
    }

    /// Generate a clip with an explicit seed.
    #[must_use]
    pub fn generate_with_seed(
        width: usize,
        height: usize,
        bit_depth: u8,
        chroma_sampling: ChromaSubsampling,
        frames: usize,
        seed: u64,
        distortion: Distortion,
    ) -> Self {
        let data = synthetic_y4m(
            width,
            height,
            bit_depth,
            chroma_sampling,
            frames,
            seed,
            distortion,
        );

        Self {
            data,
            width,
            height,
            bit_depth,
            chroma_sampling,
        }
    }

    /// The bit depth actually written, which may differ from the request
    /// because y4m cannot express every depth.
    #[must_use]
    pub fn effective_bit_depth(&self) -> u8 {
        match self.bit_depth {
            0..=8 => 8,
            9 | 11 => 10,
            16 => 8,
            _ => self.bit_depth,
        }
    }

    /// Build a decoder over this clip, entirely in memory.
    ///
    /// # Panics
    ///
    /// Panics if the generated data is somehow malformed, which would indicate
    /// a bug in the generator rather than a test condition.
    #[must_use]
    pub fn decoder(&self) -> Decoder {
        // `DecoderImpl::Y4m` is boxed, so the reader must be boxed to match.
        let reader: Box<dyn std::io::Read> = Box::new(Cursor::new(self.data.clone()));
        let decoder = Y4mDecoder::new(reader).expect("generated y4m header is valid");
        Decoder::from_decoder_impl(DecoderImpl::Y4m(decoder))
            .expect("y4m decoder reports video details")
    }
}

/// A pair of clips: a clean reference and a distorted version to score against.
pub struct SyntheticPair {
    /// The reference clip.
    pub reference: SyntheticClip,
    /// The distorted clip, sharing the reference's geometry.
    pub distorted: SyntheticClip,
}

impl SyntheticPair {
    /// Generate a reference and a distorted clip with matching geometry.
    #[must_use]
    pub fn generate(
        width: usize,
        height: usize,
        bit_depth: u8,
        chroma_sampling: ChromaSubsampling,
        frames: usize,
        distortion: Distortion,
    ) -> Self {
        let reference = SyntheticClip::generate(
            width,
            height,
            bit_depth,
            chroma_sampling,
            frames,
            Distortion::None,
        );
        // A different seed for the noise keeps the two clips independent while
        // remaining fully deterministic.
        let distorted = SyntheticClip::generate_with_seed(
            width,
            height,
            bit_depth,
            chroma_sampling,
            frames,
            0x5EED_1234_ABCD_0002,
            distortion,
        );

        Self {
            reference,
            distorted,
        }
    }

    /// Decoders for both clips.
    #[must_use]
    pub fn decoders(&self) -> (Decoder, Decoder) {
        (self.reference.decoder(), self.distorted.decoder())
    }
}
