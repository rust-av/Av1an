//! Deterministic in-memory synthetic clip generation for tests and benchmarks.
//!
//! No media files are read or written: content is generated as a y4m byte
//! stream in memory and fed to `av-decoders`' always-available Y4M path. That
//! keeps the test suite runnable anywhere, including environments without
//! FFMS2, FFmpeg or VapourSynth installed.
//!
//! Every value is derived from a fixed-seed linear congruential generator, so
//! the same parameters always produce byte-identical output on every platform.
//! That keeps Criterion's noise floor low and makes score assertions
//! reproducible.
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
/// y4m has no 16-bit colorspace, so 16-bit tests are limited to what the format
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

/// Bytes per sample for the y4m colorspace actually produced.
fn bytes_per_sample(bit_depth: u8) -> usize {
    if bit_depth > 8 { 2 } else { 1 }
}

/// How much distortion to apply to the generated clip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Distortion {
    /// No distortion. Scoring against an identical copy should yield the
    /// maximum score.
    None,
    /// Mild quantisation noise. Should score below the maximum, but high.
    Mild,
    /// Stronger noise plus block artefacts.
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
/// The content is a horizontal luma gradient, a vertically moving box and
/// per-frame noise from a fixed seed. The gradient and the moving box give the
/// motion and spatial extractors something to work with; the noise gives the
/// per-frame features variation. `distortion` perturbs the result, so the same
/// generator can produce both the reference and a degraded copy.
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
    let effective_bit_depth = match (bit_depth, chroma_sampling) {
        (16, _) => 8,
        (1..=8, _) => 8,
        (9 | 11, _) => 10,
        _ => bit_depth,
    };
    let sample_bytes = bytes_per_sample(effective_bit_depth);
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

        // Chroma: a simple two-dimensional ramp, enough for the extractors to
        // distinguish from luma.
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
        let data = synthetic_y4m(
            width,
            height,
            bit_depth,
            chroma_sampling,
            frames,
            0x5EED_1234_ABCD_0001,
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
    pub fn effective_bit_depth(&self) -> u8 {
        match self.chroma_sampling {
            _ if self.bit_depth == 16 => 8,
            _ => self.bit_depth.clamp(8, 12),
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

impl SyntheticClip {
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
}

pub(super) fn reference_mean(frame_count: usize) -> f64 {
    assert!(frame_count > 0, "the reference fixture must contain frames");
    // The independently measured first-frame deficit is spread over the whole fixture.
    100.0 - (100.0 - 97.42837) / frame_count as f64
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn generation_is_deterministic() {
        let first = synthetic_y4m(
            64,
            48,
            8,
            ChromaSubsampling::Yuv420,
            4,
            42,
            Distortion::Mild,
        );
        let second = synthetic_y4m(
            64,
            48,
            8,
            ChromaSubsampling::Yuv420,
            4,
            42,
            Distortion::Mild,
        );
        assert_eq!(
            first, second,
            "identical parameters must produce identical bytes"
        );
    }

    #[test]
    fn different_seeds_produce_different_content() {
        // The seed only affects the noise term, so a distortion level with a
        // non-zero amplitude is required for the seed to be observable.
        let first = SyntheticClip::generate_with_seed(
            64,
            48,
            8,
            ChromaSubsampling::Yuv420,
            3,
            1,
            Distortion::Mild,
        );
        let second = SyntheticClip::generate_with_seed(
            64,
            48,
            8,
            ChromaSubsampling::Yuv420,
            3,
            2,
            Distortion::Mild,
        );
        assert_ne!(first.data, second.data);
    }

    #[test]
    fn zero_distortion_ignores_the_seed() {
        // With no amplitude there is no noise term, so content is fully
        // determined by geometry. This is what lets a "distorted" clip
        // using `Distortion::None` be byte-identical to its reference
        // and score at the maximum.
        let first = SyntheticClip::generate_with_seed(
            64,
            48,
            8,
            ChromaSubsampling::Yuv420,
            3,
            1,
            Distortion::None,
        );
        let second = SyntheticClip::generate_with_seed(
            64,
            48,
            8,
            ChromaSubsampling::Yuv420,
            3,
            999,
            Distortion::None,
        );
        assert_eq!(first.data, second.data);
    }

    #[test]
    fn stronger_distortion_produces_more_variation() {
        let measure = |distortion: Distortion| {
            let clip = SyntheticClip::generate(64, 64, 8, ChromaSubsampling::Yuv420, 4, distortion);
            // Compare against the undistorted clip: more noise means a larger
            // byte-level difference.
            let clean =
                SyntheticClip::generate(64, 64, 8, ChromaSubsampling::Yuv420, 4, Distortion::None);
            clip.data
                .iter()
                .zip(clean.data.iter())
                .map(|(a, b)| u64::from(a.abs_diff(*b)))
                .sum::<u64>()
        };

        let mild = measure(Distortion::Mild);
        let moderate = measure(Distortion::Moderate);
        let severe = measure(Distortion::Severe);

        assert!(mild > 0, "mild distortion must change the data");
        assert!(moderate > mild, "moderate must exceed mild");
        assert!(severe > moderate, "severe must exceed moderate");
    }

    #[test]
    fn chroma_decimation_maps_420_422_and_444() {
        assert_eq!(chroma_decimation(ChromaSubsampling::Yuv420), (2, 2));
        assert_eq!(chroma_decimation(ChromaSubsampling::Yuv422), (2, 1));
        assert_eq!(chroma_decimation(ChromaSubsampling::Yuv444), (1, 1));
    }

    #[test]
    fn reference_mean_depends_on_the_fixture_frame_count() {
        for (frames, expected) in [(48, 99.946424375), (72, 99.96428291666667)] {
            assert!(
                (reference_mean(frames) - expected).abs() < 1e-12,
                "the {frames}-frame reference mean should be {expected}"
            );
        }
    }

    #[test]
    #[should_panic(expected = "the reference fixture must contain frames")]
    fn reference_mean_rejects_an_empty_fixture() {
        reference_mean(0);
    }
}

#[cfg(test)]
mod decoder_integration {
    use super::*;

    #[test]
    fn generated_stream_decodes_to_the_requested_geometry() {
        let clip =
            SyntheticClip::generate(64, 48, 8, ChromaSubsampling::Yuv420, 3, Distortion::None);
        let decoder = clip.decoder();
        let details = decoder.get_video_details();

        assert_eq!(details.width, clip.width);
        assert_eq!(details.height, clip.height);
        assert_eq!(details.bit_depth, usize::from(clip.effective_bit_depth()));
        assert_eq!(details.chroma_sampling, clip.chroma_sampling);
        // y4m carries no frame count in its header, so the decoder reports
        // `None`.
        assert_eq!(details.total_frames, None);
    }

    #[test]
    fn the_requested_number_of_frames_is_written() {
        for frames in [1usize, 3, 7] {
            let clip = SyntheticClip::generate(
                64,
                48,
                8,
                ChromaSubsampling::Yuv420,
                frames,
                Distortion::None,
            );
            let mut decoder = clip.decoder();

            let mut decoded = 0;
            while decoder.read_video_frame::<u8>().is_ok() {
                decoded += 1;
                assert!(
                    decoded <= frames,
                    "decoder produced more frames than written"
                );
            }

            assert_eq!(decoded, frames, "expected exactly {frames} frames");
        }
    }

    #[test]
    fn five_generated_ten_bit_frames_decode_as_u16_then_reach_end() {
        let clip =
            SyntheticClip::generate(32, 32, 10, ChromaSubsampling::Yuv420, 5, Distortion::None);
        let mut decoder = clip.decoder();

        for index in 0..5 {
            let frame = decoder
                .read_video_frame::<u16>()
                .unwrap_or_else(|error| panic!("frame {index} should decode: {error}"));
            assert_eq!(frame.y_plane.width(), 32);
            assert_eq!(frame.y_plane.height(), 32);
        }

        assert!(
            decoder.read_video_frame::<u16>().is_err(),
            "reading past the end must report an error"
        );
    }
}
