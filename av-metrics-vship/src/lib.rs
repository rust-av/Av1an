//! Native vship perceptual quality scoring via the libvship C API.
//!
//! libvship implements SSIMULACRA2, Butteraugli and CVVDP. This crate binds its
//! C API directly and scores frame pairs as they are decoded, so no temporary
//! files and no distortion maps are involved.
//!
//! Frames come from [`av_decoders`], so a clip can be read from Y4M, FFMS2,
//! FFmpeg or a VapourSynth script without this crate knowing which.
//!
//! # Availability
//!
//! libvship is a GPU metric with no CPU fallback in the shipped builds, and it
//! is distributed as a VapourSynth plugin rather than a standalone library. It
//! is opened with `dlopen` at runtime, so the crate compiles on machines
//! without it and [`is_available`] reports `false`. Since libvship v5.1.2,
//! Vulkan initialization is lazy and a missing driver is reported normally.
//! [`Vship_GPUFullCheck`](ffi::VshipApi::gpu_full_check) confirms the device is
//! usable after loading.
//!
//! # Metrics
//!
//! [`VshipMetric::Ssimulacra2`] and [`VshipMetric::Butteraugli`] are per-pair:
//! each score is final the moment `Vship_ComputeHandler` returns, and the
//! scorer uses one synchronous handler for every metric. [`VshipMetric::Cvvdp`]
//! is temporal: submit frames in order and request history resets at scene breaks.
//!
//! Distinct thread-local scorers share a native-call lock: pinned libvship writes
//! global error state even across different handles. Device probes, init, compute,
//! reset and free are serialized through short-diagnostic capture; user callbacks
//! run outside the lock. Raw [`ffi::VshipApi`] pointers and independent native
//! bindings/plugins do not participate in this synchronization.
//!
//! # Example
//!
//! ```no_run
//! use av_decoders::Decoder;
//! use av_metrics_vship::{PoolMethod, VideoFormat, VshipConfig, VshipMetric, VshipScorer};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! // Requires libvship and a usable GPU at runtime; see the crate README.
//! let config = VshipConfig::default();
//!
//! let mut reference = Decoder::from_file("reference.mkv")?;
//! let mut distorted = Decoder::from_file("distorted.mkv")?;
//!
//! let reference_format = VideoFormat::from_details(reference.get_video_details());
//! let distorted_format = VideoFormat::from_details(distorted.get_video_details());
//! let mut scorer = VshipScorer::new(config, reference_format, distorted_format, None)?;
//!
//! let scores = scorer.score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {})?;
//! let pooled = VshipScorer::pool(&scores, VshipMetric::Ssimulacra2, PoolMethod::Mean, true)?;
//! println!("SSIMULACRA2: {pooled:.4}");
//! # Ok(())
//! # }
//! ```

mod config;
mod error;
pub mod ffi;
mod scorer;

pub use config::{
    ButteraugliParams, CvvdpParams, DEFAULT_HANDLER_THREADS, PoolMethod, VshipConfig, VshipMetric,
    colorspace_from_details, sample_for_bit_depth, subsample_for_chroma,
};
pub use error::VshipError;
pub use ffi::{
    VshipBackend, VshipChromaLocation, VshipChromaSubsample, VshipColorFamily, VshipColorspace,
    VshipCropRectangle, VshipDeviceInfo, VshipPrimaries, VshipRange, VshipSample,
    VshipTransferFunction, VshipYuvMatrix,
};
pub use scorer::{
    FrameScore, PlaneSet, PlaneSource, VideoFormat, VshipDevice, VshipScorer, default_gpu_id,
    devices,
};

/// The runtime libvship version string, if libvship is loaded and usable.
/// Shares the successful default-device capability check with [`is_available`].
#[inline]
#[must_use]
pub fn libvship_version() -> Option<&'static str> {
    default_gpu_id().and_then(|gpu_id| scorer::availability(gpu_id).ok())
}

/// Whether vship scoring is available on this system.
///
/// Requires a loadable library and a default device passing `Vship_GPUFullCheck`.
/// Successful capability checks are cached and shared with version requests and
/// default constructors; failed checks may retry. Runtime initialization and
/// scoring can still fail independently.
#[inline]
#[must_use]
pub fn is_available() -> bool {
    default_gpu_id().is_some_and(|gpu_id| scorer::availability(gpu_id).is_ok())
}

/// The name of the device libvship would use, if it is available.
#[inline]
#[must_use]
pub fn device_name() -> Option<String> {
    let api = ffi::VshipApi::load().ok()?;
    api.device_info(default_gpu_id()?).ok().map(|info| info.name())
}

/// The compute backend the loaded libvship was compiled for, if it is loaded.
///
/// Distinct from [`is_available`]: a library can load and still fail
/// `Vship_GPUFullCheck`, so the backend may be reported for a system where
/// scoring cannot actually run.
#[inline]
#[must_use]
pub fn backend() -> Option<VshipBackend> {
    ffi::VshipApi::load().ok().map(ffi::VshipApi::backend)
}
