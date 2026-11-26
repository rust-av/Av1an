//! Native CPU perceptual quality scoring via the fmetrics C API.
//!
//! Binds fmetrics' C API and scores frame pairs as they are decoded, so no
//! temporary files and no distortion maps are involved. Frames come from
//! [`av_decoders`], so a clip can be read from Y4M, FFMS2, FFmpeg or a
//! VapourSynth script without this crate knowing which.
//!
//! # Availability
//!
//! fmetrics is distributed as source rather than as a prebuilt install, so it
//! is opened with `dlopen` at runtime: the crate compiles without it,
//! [`is_available`] reports `false`, and scoring reports unavailability rather
//! than failing to build.
//!
//! # Metrics
//!
//! [`FmetricsMetric::Ssimulacra2`], [`FmetricsMetric::Butteraugli`],
//! [`FmetricsMetric::Iwssim`] and [`FmetricsMetric::Msssim`] are per-pair: each
//! score is final the moment the call returns. [`FmetricsMetric::Cvvdp`] is
//! temporal, so it keeps a single context, sees frames in order, and resets on
//! a scene break.
//!
//! # Threading
//!
//! A per-pair [`FmetricsScorer`] is `Send + Sync`. The scratch arena is
//! per-workspace, so distinct workspaces are independent and concurrent
//! submissions overlap -- but sharing *one* workspace corrupts it silently, so
//! the pool hands each to one caller at a time and refuses when empty.
//!
//! What that is worth depends on frame size, since the arena is ~72 bytes per
//! pixel per workspace: on 6 cores, throughput scales ~3.3x at 192x108 and
//! ~1.2x at 1920x1080, where it no longer fits in cache and memory bandwidth is
//! the limit. See `benches/README.md`.
//!
//! # Colour
//!
//! fmetrics takes interleaved RGB rather than planar YUV, so each pair is
//! converted before submission. That conversion is part of the measurement, so
//! the matrix and range come from the clip's own details rather than a default,
//! and the conversion module carries its own tests.
//! yuvxyb converts tagged SDR video through BT.1886 gamma 2.4 to sRGB, while
//! existing sRGB/linear data retains its encoding. CPU input remains BT.709-gamut
//! only; unsupported HDR transfers/gamuts return [`FmetricsError::UnsupportedFormat`].
//!
//! # Scores are engine-specific
//!
//! These are CPU implementations, so a score is not interchangeable with one
//! from a GPU metric implementing the same algorithm. A caller comparing across
//! engines must record which produced a number.
//!
//! # Borrowed storage contracts
//!
//! Plane descriptors cannot be forged from arbitrary pointers in safe code:
//! ```compile_fail
//! use av_metrics_fmetrics::PlaneSource;
//! let plane = PlaneSource {
//!     width: 1, height: 1, stride: 1, data_origin: 0,
//!     data: 1usize as *const u8, bytes_per_sample: 1,
//! };
//! ```
//!
//! A plane set cannot outlive its decoded frame:
//! ```compile_fail
//! use av_metrics_fmetrics::PlaneSet;
//! use av_decoders::v_frame::{frame::FrameBuilder, chroma::ChromaSubsampling};
//! let planes;
//! {
//!     let frame = FrameBuilder::new(2, 2, ChromaSubsampling::Yuv444, 8)
//!         .build::<u8>().unwrap();
//!     planes = PlaneSet::from_frame(&frame).unwrap();
//! }
//! println!("{:?}", planes.as_array());
//! ```
//!
//! An ABI image borrowed from owned RGB cannot outlive that RGB:
//! ```compile_fail
//! use av_metrics_fmetrics::{PlaneSet, ColorInfo, FmetricsColorspace, yuv_to_rgb};
//! # fn demo(frame: &av_decoders::v_frame::frame::Frame<u8>, info: &ColorInfo) {
//! let descriptor;
//! {
//!     let planes = PlaneSet::from_frame(frame).unwrap();
//!     let image = yuv_to_rgb(&planes, info, 255).unwrap();
//!     descriptor = image.as_img(FmetricsColorspace::Srgb, false);
//! }
//! println!("{descriptor:?}");
//! # }
//! ```
//!
//! A view's ABI image also keeps its backing vector borrowed:
//! ```compile_fail
//! use av_metrics_fmetrics::{RgbView, RgbSource, FmetricsColorspace};
//! let descriptor;
//! {
//!     let bytes = vec![0; 12];
//!     let view = RgbView::new(&bytes, 2, 2, false).unwrap();
//!     descriptor = view.as_img(FmetricsColorspace::Srgb, false);
//! }
//! println!("{descriptor:?}");
//! ```
//!
//! Raw plane construction requires an explicit unsafe allocation contract:
//! ```compile_fail,E0133
//! use av_metrics_fmetrics::PlaneSource;
//! let bytes = [0u8; 4];
//! let plane = PlaneSource::from_raw_parts(bytes.as_ptr(), bytes.len(), 2, 2, 2, 0, 1);
//! ```
//!
//! # Example
//!
//! ```no_run
//! use av_decoders::Decoder;
//! use av_metrics_fmetrics::{ColorInfo, FmetricsConfig, FmetricsMetric, FmetricsScorer};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! // Requires fmetrics at runtime; see the crate README for how to build it.
//! let config = FmetricsConfig::new(FmetricsMetric::Ssimulacra2);
//!
//! let mut reference = Decoder::from_file("reference.mkv")?;
//! let mut distorted = Decoder::from_file("distorted.mkv")?;
//!
//! let reference_color = ColorInfo::from_details(reference.get_video_details())?;
//! let distorted_color = ColorInfo::from_details(distorted.get_video_details())?;
//! let scorer = FmetricsScorer::new(config, reference_color, distorted_color)?;
//!
//! // Submit pairs as they are decoded; the driver owns frame lifetime.
//! # Ok(())
//! # }
//! ```

mod config;
mod convert;
mod error;
pub mod ffi;
mod scorer;

pub use config::{
    ColorInfo, DEFAULT_PNORM, DEFAULT_THREADS, FmetricsConfig, FmetricsMetric, SampleRange,
    YuvMatrix, colorspace_from_details, matrix_for_resolution,
};
pub use convert::{BorrowedImg, PlaneSet, PlaneSource, RgbImage, RgbSource, RgbView, yuv_to_rgb};
pub use error::FmetricsError;
pub use ffi::{
    FmetricsApi, FmetricsButteraugliOptions, FmetricsColorspace, FmetricsCvvdpCtx,
    FmetricsCvvdpDisplayModel, FmetricsCvvdpDisplayParams, FmetricsCvvdpResult, FmetricsErr,
    FmetricsImg, FmetricsPixFmt, FmetricsWorkspace,
};
pub use scorer::{FmetricsScorer, FrameScore};

/// The runtime fmetrics version string, if fmetrics is loaded.
#[inline]
#[must_use]
pub fn fmetrics_version() -> Option<String> {
    FmetricsApi::load().ok().map(FmetricsApi::version_string)
}

/// The CVVDP implementation version string, if fmetrics is loaded.
#[inline]
#[must_use]
pub fn cvvdp_version() -> Option<String> {
    FmetricsApi::load().ok().map(FmetricsApi::cvvdp_version_string)
}

/// Whether fmetrics scoring is available on this system.
///
/// Cheap to call: the library is resolved once and cached.
#[inline]
#[must_use]
pub fn is_available() -> bool {
    FmetricsApi::load().is_ok()
}
