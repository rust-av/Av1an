//! Native VMAF scoring via [libvmaf], with opt-in CUDA acceleration.
//!
//! VMAF (Video Multi-Method Assessment Fusion) is Netflix's perceptual quality
//! metric. This crate binds [libvmaf] directly and scores frame pairs as they
//! are decoded, so no temporary files or JSON logs are involved.
//!
//! Frames come from [`av_decoders`], so a clip can be read from Y4M, FFMS2,
//! FFmpeg or a VapourSynth script without this crate knowing which.
//!
//! # Backends
//!
//! CPU is the default. With the Cargo `cuda` feature and libvmaf built with
//! `-Denable_cuda=true`, Auto attempts CUDA first and falls back to CPU on failure.
//! The choice is made by constructing a complete scorer rather than probing
//! devices: libvmaf rejects models whose feature extractors lack an implementation for the
//! requested backend. See [`BackendPreference`] to override.
//!
//! # Example
//!
//! ```no_run
//! use av_decoders::Decoder;
//! use av_metrics_vmaf::{PoolMethod, VideoFormat, VmafConfig, VmafScorer};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! // Requires libvmaf at runtime; see the crate README for install instructions.
//! let config = VmafConfig::default().with_threads(4);
//!
//! let reference = Decoder::from_file("reference.mkv")?;
//! let distorted = Decoder::from_file("distorted.mkv")?;
//!
//! let details = reference.get_video_details();
//! let format = VideoFormat {
//!     width:           details.width as u32,
//!     height:          details.height as u32,
//!     bit_depth:       details.bit_depth as u32,
//!     chroma_sampling: details.chroma_sampling,
//! };
//!
//! let mut scorer = VmafScorer::new(config, format)?;
//! println!("scoring on the {} backend", scorer.backend());
//!
//! let mut reference = reference;
//! let mut distorted = distorted;
//! let scores = scorer.score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {})?;
//!
//! let pooled = VmafScorer::pool(&scores, PoolMethod::Mean)?;
//! println!("VMAF: {pooled:.3}");
//! # Ok(())
//! # }
//! ```
//!
//! [libvmaf]: https://github.com/Netflix/vmaf
//! [`av_decoders`]: https://github.com/rust-av/av-decoders

mod backend;
mod config;
mod error;
mod ffi;
mod scorer;

pub use backend::Backend;
pub use config::{BackendPreference, PoolMethod, VmafConfig, VmafFeature, VmafModel};
pub use error::VmafError;
pub use scorer::{FrameScore, PlaneSet, PlaneSource, VideoFormat, VmafScorer};

/// The runtime libvmaf version string, if libvmaf could be loaded.
#[inline]
#[must_use]
pub fn libvmaf_version() -> Option<&'static str> {
    ffi::VmafApi::load().ok().map(|api| api.version)
}

/// Whether VMAF scoring is available on this system.
///
/// Cheap to call: libvmaf is resolved once and cached.
#[inline]
#[must_use]
pub fn is_available() -> bool {
    ffi::VmafApi::load().is_ok()
}

/// Whether the loaded libvmaf exposes a usable CUDA interface.
///
/// False without the Cargo `cuda` feature, without loading libvmaf. With the
/// feature enabled, requires all native CUDA entry points and verified allocator
/// cleanup support. This does not initialize CUDA or prove device/model support;
/// only a complete scorer attempt can do that.
#[inline]
#[must_use]
pub fn cuda_available() -> bool {
    cfg!(feature = "cuda") && ffi::VmafApi::load().is_ok_and(ffi::VmafApi::cuda_available)
}
