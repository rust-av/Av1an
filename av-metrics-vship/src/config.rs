//! Configuration types for vship scoring.

use std::ffi::CString;

use av_decoders::{
    ChromaLocation, ColorPrimaries, ColorRange, ColorSpace, TransferCharacteristic, VideoDetails,
};
use v_frame::chroma::ChromaSubsampling;

use crate::{
    error::VshipError,
    ffi::{
        VshipChromaLocation, VshipChromaSubsample, VshipColorFamily, VshipColorspace,
        VshipCropRectangle, VshipPrimaries, VshipRange, VshipSample, VshipTransferFunction,
        VshipYuvMatrix,
    },
    scorer::GPU_UNSET,
};

/// Default legacy handler-thread request.
///
/// Retained for compatibility; the synchronous scorer always owns one handler.
pub const DEFAULT_HANDLER_THREADS: u32 = 4;

/// Butteraugli's default intensity target, in nits.
///
/// This is the SDR reference white value, and is what libvship's own CLI
/// defaults to.
pub const DEFAULT_INTENSITY_MULTIPLIER: f32 = 203.0;

/// CVVDP's default display model key.
pub const DEFAULT_DISPLAY_MODEL: &str = "standard_fhd";

/// The libvship metric to compute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VshipMetric {
    /// SSIMULACRA2: single-scale, per-pair, higher is better.
    #[default]
    Ssimulacra2,
    /// Butteraugli: a psychovisually tuned difference, lower is better.
    Butteraugli,
    /// CVVDP: temporally pooled contrast sensitivity, higher is better.
    ///
    /// This metric accumulates over the frames a handler has seen, so its
    /// scores are cumulative prefixes rather than independent per-frame values.
    /// Only the final prefix is the session result. With `disable_temporal`,
    /// frames are independent instead. See [`Self::is_temporal`].
    Cvvdp,
}

impl VshipMetric {
    /// This metric's name, as libvship spells it.
    #[inline]
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ssimulacra2 => "SSIMULACRA2",
            Self::Butteraugli => "Butteraugli",
            Self::Cvvdp => "CVVDP",
        }
    }

    /// Whether this metric accumulates state across frames.
    ///
    /// A temporal metric's handler must see every frame in order, so the scorer
    /// keeps exactly one such handler and calls `Vship_Reset` at a
    /// discontinuity.
    #[inline]
    #[must_use]
    pub const fn is_temporal(self) -> bool {
        matches!(self, Self::Cvvdp)
    }

    /// Whether higher values are better for this metric.
    #[inline]
    #[must_use]
    pub const fn higher_is_better(self) -> bool {
        !matches!(self, Self::Butteraugli)
    }

    /// Every metric, in declaration order.
    #[inline]
    #[must_use]
    pub const fn all() -> [Self; 3] {
        [Self::Ssimulacra2, Self::Butteraugli, Self::Cvvdp]
    }

    /// Parse a metric name, case-insensitively.
    ///
    /// Accepts the spellings libvship's CLI accepts, so a value configured for
    /// the standalone tool works here unchanged.
    #[inline]
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        // Reducing to lowercase alphanumeric makes one comparison serve every
        // case and punctuation variant a user might type.
        let normalised: String = name
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .map(|c| c.to_ascii_lowercase())
            .collect();

        match normalised.as_str() {
            "ssimulacra2" | "ssimulacra" | "ssim2" => Some(Self::Ssimulacra2),
            "butteraugli" => Some(Self::Butteraugli),
            "cvvdp" => Some(Self::Cvvdp),
            _ => None,
        }
    }
}

/// How to pool independent per-frame scores into one value.
///
/// Cumulative CVVDP already pools the session natively; [`crate::VshipScorer::pool`]
/// returns its final prefix without applying this method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PoolMethod {
    /// Arithmetic mean over frames.
    #[default]
    Mean,
    /// The best frame's score.
    Min,
    /// The worst frame's score.
    Max,
}

impl PoolMethod {
    /// Combine independent per-frame scores into a single value.
    ///
    /// For CVVDP with `disable_temporal=true`, pass the values extracted via
    /// [`crate::FrameScore::value`]. Do not apply this to cumulative prefixes.
    ///
    /// # Errors
    ///
    /// Returns [`VshipError::InvalidConfiguration`] if `scores` is empty, since
    /// the result would be undefined.
    #[inline]
    pub fn apply(self, scores: &[f64]) -> Result<f64, VshipError> {
        if scores.is_empty() {
            return Err(VshipError::InvalidConfiguration {
                reason: "cannot pool an empty score list".to_owned(),
            });
        }

        Ok(match self {
            Self::Mean => scores.iter().sum::<f64>() / scores.len() as f64,
            Self::Min => scores.iter().copied().fold(f64::INFINITY, f64::min),
            Self::Max => scores.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        })
    }
}

/// Butteraugli-specific parameters.
///
/// Only consulted when the metric is [`VshipMetric::Butteraugli`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ButteraugliParams {
    /// Exponent of the norm the metric minimises. 2 is the convention.
    pub q_norm: i32,
    /// Peak display brightness in nits, which scales intensity weighting.
    pub intensity_multiplier: f32,
}

impl Default for ButteraugliParams {
    #[inline]
    fn default() -> Self {
        Self {
            q_norm: 2,
            intensity_multiplier: DEFAULT_INTENSITY_MULTIPLIER,
        }
    }
}

/// CVVDP-specific parameters.
///
/// Only consulted when the metric is [`VshipMetric::Cvvdp`].
#[derive(Debug, Clone, PartialEq)]
pub struct CvvdpParams {
    /// Display model key, e.g. `standard_fhd`. Resolved by libvship.
    pub display_model: String,
    /// Path to a JSON display-configuration file, or empty for none.
    pub display_model_json: String,
    /// Whether to scale both inputs to the model's display resolution.
    pub resize_to_display: bool,
    /// Frame rate handed to libvship's temporal model.
    ///
    /// Defaults to 60 when not set. Requires a finite value in `0 < fps <= 100000`,
    /// even with temporal processing disabled. Explicit zero is invalid.
    pub fps: f32,
    /// Whether to clear the temporal history at a discontinuity in frame index.
    ///
    /// True by default. A scene cut is a discontinuity in the motion the
    /// temporal filter tracks, so carrying history across one would fold two
    /// unrelated sequences into a single score.
    pub reset_on_discontinuity: bool,
    /// Score each frame independently, clearing history AND accumulated score
    /// after each successful compute, like the VapourSynth plugin.
    pub disable_temporal: bool,
}

impl Default for CvvdpParams {
    #[inline]
    fn default() -> Self {
        Self {
            display_model: DEFAULT_DISPLAY_MODEL.to_owned(),
            display_model_json: String::new(),
            resize_to_display: false,
            fps: 60.0,
            reset_on_discontinuity: true,
            disable_temporal: false,
        }
    }
}

/// A vship scoring configuration.
///
/// Defaults to SSIMULACRA2 on the best available device and libvship's SDR
/// reference white point and display model. The legacy handler-thread request
/// defaults to four, but synchronous scoring always uses one handler.
#[derive(Debug, Clone, PartialEq)]
pub struct VshipConfig {
    metric: VshipMetric,
    /// The configured device, or [`GPU_UNSET`] when none was chosen.
    gpu_id: u32,
    handler_threads: u32,
    butteraugli: ButteraugliParams,
    cvvdp: CvvdpParams,
}

impl Default for VshipConfig {
    #[inline]
    fn default() -> Self {
        Self {
            metric: VshipMetric::default(),
            gpu_id: GPU_UNSET,
            handler_threads: DEFAULT_HANDLER_THREADS,
            butteraugli: ButteraugliParams::default(),
            cvvdp: CvvdpParams::default(),
        }
    }
}

impl VshipConfig {
    /// A configuration using the default metric, device and legacy thread request.
    #[inline]
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Select the metric.
    #[inline]
    #[must_use]
    pub const fn with_metric(mut self, metric: VshipMetric) -> Self {
        self.metric = metric;
        self
    }

    /// Select the compute device by index.
    #[inline]
    #[must_use]
    pub const fn with_gpu_id(mut self, gpu_id: u32) -> Self {
        self.gpu_id = gpu_id;
        self
    }

    /// Retain a legacy handler-thread request; synchronous scoring ignores it.
    ///
    /// Zero is still clamped to one for compatibility. This value is returned by
    /// [`Self::handler_threads`]; [`Self::handler_count`] always reports one.
    #[inline]
    #[must_use]
    pub const fn with_handler_threads(mut self, handler_threads: u32) -> Self {
        // `Ord::max` is not const, so the clamp is written out.
        self.handler_threads = if handler_threads == 0 {
            1
        } else {
            handler_threads
        };
        self
    }

    /// Set the Butteraugli norm exponent.
    #[inline]
    #[must_use]
    pub const fn with_q_norm(mut self, q_norm: i32) -> Self {
        self.butteraugli.q_norm = q_norm;
        self
    }

    /// Set the Butteraugli intensity target, in nits.
    #[inline]
    #[must_use]
    pub const fn with_intensity_multiplier(mut self, intensity_multiplier: f32) -> Self {
        self.butteraugli.intensity_multiplier = intensity_multiplier;
        self
    }

    /// Set the CVVDP display model key.
    ///
    /// # Errors
    ///
    /// Returns [`VshipError::InvalidConfiguration`] if `display_model` contains
    /// an interior NUL, which libvship cannot receive as a C string.
    #[inline]
    pub fn with_display_model(
        mut self,
        display_model: impl Into<String>,
    ) -> Result<Self, VshipError> {
        let display_model = display_model.into();
        to_c_string(&display_model, "display model")?;

        self.cvvdp.display_model = display_model;
        Ok(self)
    }

    /// Set a JSON display-configuration override for CVVDP.
    ///
    /// The path is passed to libvship verbatim; this crate neither reads nor
    /// validates it, because libvship merges its properties over the named
    /// display model and only a wholly new model name must be fully specified
    /// there.
    ///
    /// # Errors
    ///
    /// Returns [`VshipError::InvalidConfiguration`] if `path` contains an
    /// interior NUL.
    #[inline]
    pub fn with_display_model_json(mut self, path: impl Into<String>) -> Result<Self, VshipError> {
        let path = path.into();
        to_c_string(&path, "display model JSON path")?;

        self.cvvdp.display_model_json = path;
        Ok(self)
    }

    /// Whether CVVDP should scale its inputs to the model's display resolution.
    #[inline]
    #[must_use]
    pub const fn with_resize_to_display(mut self, resize_to_display: bool) -> Self {
        self.cvvdp.resize_to_display = resize_to_display;
        self
    }

    /// Set the frame rate handed to CVVDP's temporal model.
    ///
    /// Replaces the 60 fps default. CVVDP requires a finite value in
    /// `0 < fps <= 100000`; other metrics ignore this setting.
    #[inline]
    #[must_use]
    pub const fn with_fps(mut self, fps: f32) -> Self {
        self.cvvdp.fps = fps;
        self
    }

    /// Whether CVVDP clears its temporal history at a scene break.
    ///
    /// The default, `true`, resets the temporal filter at an index
    /// discontinuity, which stops a cut being scored as a motion event. Setting
    /// it to `false` keeps the history, so one score spans scenes.
    #[inline]
    #[must_use]
    pub const fn with_reset_on_discontinuity(mut self, reset_on_discontinuity: bool) -> Self {
        self.cvvdp.reset_on_discontinuity = reset_on_discontinuity;
        self
    }

    /// Score each CVVDP frame independently, resetting history then accumulated
    /// score after each compute. Does not change the source-gap policy.
    #[inline]
    #[must_use]
    pub const fn with_disable_temporal(mut self, disable_temporal: bool) -> Self {
        self.cvvdp.disable_temporal = disable_temporal;
        self
    }

    /// The configured metric.
    #[inline]
    #[must_use]
    pub const fn metric(&self) -> VshipMetric {
        self.metric
    }

    /// The configured device index, or `u32::MAX` when none was chosen.
    ///
    /// A default configuration leaves this unset so the scorer can pick a
    /// discrete device at run time; see [`Self::gpu_id_or_default`].
    #[inline]
    #[must_use]
    pub const fn gpu_id(&self) -> u32 {
        self.gpu_id
    }

    /// The device to score on: the configured one, or the chosen default.
    ///
    /// Falls back to device 0 when the default could not be resolved, because
    /// libvship's own entry points still require an index. Scoring then fails
    /// with libvship's own error rather than being pointed at a device chosen
    /// here.
    #[inline]
    #[must_use]
    pub fn gpu_id_or_default(&self) -> u32 {
        if self.gpu_id == GPU_UNSET {
            crate::scorer::default_gpu_id().unwrap_or(0)
        } else {
            self.gpu_id
        }
    }

    /// Whether a device was chosen explicitly rather than left to the default.
    #[inline]
    #[must_use]
    pub const fn has_explicit_gpu_id(&self) -> bool {
        self.gpu_id != GPU_UNSET
    }

    /// The stored legacy handler-thread request, not the actual handler count.
    #[inline]
    #[must_use]
    pub const fn handler_threads(&self) -> u32 {
        self.handler_threads
    }

    /// The configured Butteraugli parameters.
    #[inline]
    #[must_use]
    pub const fn butteraugli(&self) -> &ButteraugliParams {
        &self.butteraugli
    }

    /// The configured CVVDP parameters.
    #[inline]
    #[must_use]
    pub const fn cvvdp(&self) -> &CvvdpParams {
        &self.cvvdp
    }

    /// How many handlers the synchronous scorer builds: always one.
    ///
    /// Independent of the legacy [`Self::handler_threads`] request.
    #[inline]
    #[must_use]
    pub fn handler_count(&self) -> usize {
        1
    }

    /// Validate the configuration, without touching libvship.
    ///
    /// # Errors
    ///
    /// Returns [`VshipError::InvalidConfiguration`] when a parameter libvship
    /// would reject has been set, so the failure is reported before a device is
    /// claimed.
    #[inline]
    pub fn validate(&self) -> Result<(), VshipError> {
        if self.handler_threads == 0 {
            return Err(VshipError::InvalidConfiguration {
                reason: "handler_threads must be at least 1".to_owned(),
            });
        }

        if self.metric == VshipMetric::Butteraugli {
            // Butteraugli's norms are defined for positive integer exponents,
            // and a zero or negative `Qnorm` would make libvship's
            // `powq` call meaningless rather than fail cleanly.
            if self.butteraugli.q_norm <= 0 {
                return Err(VshipError::InvalidConfiguration {
                    reason: format!(
                        "q_norm must be positive for Butteraugli, got {}",
                        self.butteraugli.q_norm
                    ),
                });
            }
            if !self.butteraugli.intensity_multiplier.is_finite()
                || self.butteraugli.intensity_multiplier <= 0.0
            {
                return Err(VshipError::InvalidConfiguration {
                    reason: format!(
                        "intensity_multiplier must be a positive finite value, got {}",
                        self.butteraugli.intensity_multiplier
                    ),
                });
            }
        }

        if self.metric == VshipMetric::Cvvdp {
            if self.cvvdp.display_model.is_empty() {
                return Err(VshipError::InvalidConfiguration {
                    reason: "CVVDP display model must not be empty".to_owned(),
                });
            }
            to_c_string(&self.cvvdp.display_model, "display model")?;
            to_c_string(&self.cvvdp.display_model_json, "display model JSON path")?;

            // SECURITY: Bound FPS-driven temporal allocations; reject rather than sanitize.
            if !self.cvvdp.fps.is_finite() || self.cvvdp.fps <= 0.0 || self.cvvdp.fps > 100_000.0 {
                return Err(VshipError::InvalidConfiguration {
                    reason: format!(
                        "CVVDP frame rate must be finite and satisfy 0 < fps <= 100000, got {}",
                        self.cvvdp.fps
                    ),
                });
            }
        }

        Ok(())
    }
}

/// Copy a string into a NUL-terminated C string for libvship to read.
///
/// # Errors
///
/// Returns [`VshipError::InvalidConfiguration`] if `value` contains an interior
/// NUL, which cannot be represented in a C string.
#[inline]
pub fn to_c_string(value: &str, what: &str) -> Result<CString, VshipError> {
    CString::new(value).map_err(|_| VshipError::InvalidConfiguration {
        reason: format!("{what} contains an interior null byte"),
    })
}

/// The libvship sample type for a bit depth.
///
/// Returns `None` for depths libvship has no enum value for.
#[inline]
#[must_use]
pub const fn sample_for_bit_depth(bit_depth: u32) -> Option<VshipSample> {
    match bit_depth {
        8 => Some(VshipSample::Uint8),
        9 => Some(VshipSample::Uint9),
        10 => Some(VshipSample::Uint10),
        12 => Some(VshipSample::Uint12),
        14 => Some(VshipSample::Uint14),
        16 => Some(VshipSample::Uint16),
        _ => None,
    }
}

/// The libvship subsampling for a chroma sampling.
///
/// Returns `None` for monochrome, which has no chroma planes at all and so
/// cannot be expressed as a subsampling.
#[inline]
#[must_use]
pub const fn subsample_for_chroma(
    chroma_sampling: ChromaSubsampling,
) -> Option<VshipChromaSubsample> {
    match chroma_sampling {
        ChromaSubsampling::Yuv420 => Some(VshipChromaSubsample::yuv420()),
        ChromaSubsampling::Yuv422 => Some(VshipChromaSubsample::yuv422()),
        ChromaSubsampling::Yuv444 => Some(VshipChromaSubsample::yuv444()),
        ChromaSubsampling::Monochrome => None,
    }
}

/// Build a libvship colorspace from decoder-reported video details.
///
/// `target_width` and `target_height` come from `target_resolution`, which is
/// Av1an's encode resolution: libvship scales each input to the requested size
/// internally, so no plane is resized here. The reference and the encode may
/// name different targets, which is how a lower-resolution reference is scored
/// against a higher-resolution result.
///
/// Only absent/`Unspecified` metadata uses defaults: limited range, left chroma,
/// BT.709 transfer/primaries, and BT.709 above 650 lines (BT.601 otherwise).
///
/// # Errors
///
/// Returns [`VshipError::UnsupportedFormat`] if the bit depth or chroma
/// sampling or explicit color metadata has no libvship equivalent.
#[inline]
pub fn colorspace_from_details(
    details: &VideoDetails,
    target_resolution: Option<(u32, u32)>,
) -> Result<VshipColorspace, VshipError> {
    let sample = sample_for_bit_depth(details.bit_depth as u32).ok_or_else(|| {
        VshipError::UnsupportedFormat {
            reason: format!(
                "bit depth {} has no libvship sample type; libvship supports 8, 9, 10, 12, 14 and \
                 16",
                details.bit_depth
            ),
        }
    })?;

    let subsampling = subsample_for_chroma(details.chroma_sampling).ok_or_else(|| {
        VshipError::UnsupportedFormat {
            reason: "monochrome content has no chroma planes; libvship requires 4:2:0, 4:2:2 or \
                     4:4:4"
                .to_owned(),
        }
    })?;

    // -1 is libvship's "do not scale" sentinel, which is what an absent target
    // resolution means.
    let (target_width, target_height) = match target_resolution {
        Some((width, height)) => (i64::from(width), i64::from(height)),
        None => (-1, -1),
    };

    Ok(VshipColorspace {
        width: details.width as i64,
        height: details.height as i64,
        target_width,
        target_height,
        sample,
        range: match details.color_range {
            Some(ColorRange::Full) => VshipRange::Full,
            None | Some(ColorRange::Unspecified | ColorRange::Limited) => VshipRange::Limited,
        },
        subsampling,
        chroma_location: match details.chroma_location {
            None | Some(ChromaLocation::Unspecified | ChromaLocation::Left) => {
                VshipChromaLocation::Left
            },
            Some(ChromaLocation::Center) => VshipChromaLocation::Center,
            Some(ChromaLocation::TopLeft) => VshipChromaLocation::TopLeft,
            Some(ChromaLocation::Top) => VshipChromaLocation::Top,
            Some(location) => return Err(unsupported_metadata("chroma location", location)),
        },
        color_family: VshipColorFamily::YUV,
        yuv_matrix: match details.color_space {
            None | Some(ColorSpace::Unspecified) => matrix_for_resolution(details.height),
            Some(ColorSpace::BT709) => VshipYuvMatrix::Bt709,
            Some(ColorSpace::BT470BG) => VshipYuvMatrix::Bt470Bg,
            Some(ColorSpace::SMPTE170M) => VshipYuvMatrix::St170M,
            Some(ColorSpace::YCGCO) => VshipYuvMatrix::Ycgco,
            Some(ColorSpace::BT2020NCL) => VshipYuvMatrix::Bt2020Ncl,
            Some(ColorSpace::BT2020CL) => VshipYuvMatrix::Bt2020Cl,
            Some(ColorSpace::ICTCP) => VshipYuvMatrix::Bt2100Ictcp,
            Some(ColorSpace::YCGCORE) => VshipYuvMatrix::YcgcoRe,
            Some(ColorSpace::YCGCORO) => VshipYuvMatrix::YcgcoRo,
            // RGB planes are not supported by this planar-YUV adapter.
            Some(matrix) => return Err(unsupported_metadata("YUV matrix", matrix)),
        },
        transfer_function: match details.color_transfer_characteristic {
            None | Some(TransferCharacteristic::Unspecified | TransferCharacteristic::BT709) => {
                VshipTransferFunction::Bt709
            },
            Some(TransferCharacteristic::GAMMA22) => VshipTransferFunction::Bt470M,
            Some(TransferCharacteristic::GAMMA28) => VshipTransferFunction::Bt470Bg,
            Some(TransferCharacteristic::SMPTE170M) => VshipTransferFunction::Bt601,
            Some(TransferCharacteristic::SMPTE240M) => VshipTransferFunction::St240M,
            Some(TransferCharacteristic::Linear) => VshipTransferFunction::Linear,
            Some(TransferCharacteristic::IEC61966_2_1) => VshipTransferFunction::SRgb,
            // Match libvship's FFMS2 adapter: both BT.2020 SDR curves use BT.709.
            Some(TransferCharacteristic::BT2020_10 | TransferCharacteristic::BT2020_12) => {
                VshipTransferFunction::Bt709
            },
            Some(TransferCharacteristic::SMPTE2084) => VshipTransferFunction::Pq,
            Some(TransferCharacteristic::SMPTE428) => VshipTransferFunction::St428,
            Some(TransferCharacteristic::ARIB_STD_B67) => VshipTransferFunction::Hlg,
            Some(transfer) => return Err(unsupported_metadata("transfer", transfer)),
        },
        primaries: match details.color_primaries {
            None | Some(ColorPrimaries::Unspecified | ColorPrimaries::BT709) => {
                VshipPrimaries::Bt709
            },
            Some(ColorPrimaries::BT470M) => VshipPrimaries::Bt470M,
            Some(ColorPrimaries::BT470BG) => VshipPrimaries::Bt470Bg,
            Some(ColorPrimaries::SMPTE170M) => VshipPrimaries::St170M,
            Some(ColorPrimaries::SMPTE240M) => VshipPrimaries::St240M,
            Some(ColorPrimaries::BT2020) => VshipPrimaries::Bt2020,
            Some(ColorPrimaries::SMPTE432) => VshipPrimaries::DisplayP3,
            Some(primaries) => return Err(unsupported_metadata("primaries", primaries)),
        },
        crop: VshipCropRectangle::default(),
    })
}

fn unsupported_metadata(field: &str, value: impl std::fmt::Debug) -> VshipError {
    VshipError::UnsupportedFormat {
        reason: format!("{field} {value:?} has no libvship equivalent for planar YUV input"),
    }
}

/// The YUV matrix libvship should assume for a picture of this height.
///
/// Used only for absent/`Unspecified` matrix metadata, matching the 650-line
/// threshold libvship's own FFmpeg plugin uses.
#[inline]
#[must_use]
pub const fn matrix_for_resolution(height: usize) -> VshipYuvMatrix {
    if height > 650 {
        VshipYuvMatrix::Bt709
    } else {
        VshipYuvMatrix::Bt470Bg
    }
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "a failing assertion should panic loudly, which is what a unit test wants"
)]
mod tests {
    use av_decoders::Rational32;

    use super::*;

    #[test]
    fn cvvdp_default_frame_rate_is_usable() {
        let config = VshipConfig::new().with_metric(VshipMetric::Cvvdp);
        assert_eq!(config.cvvdp().fps, 60.0);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn cvvdp_invalid_fps_rejects_before_discovery_with_or_without_temporal_filtering() {
        let format =
            crate::VideoFormat::from_details(&details(32, 32, 8, ChromaSubsampling::Yuv420));
        for rate in [
            0.0,
            -0.0,
            -1.0,
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            100_000.01,
            f32::MAX,
        ] {
            for disabled in [false, true] {
                let config = VshipConfig::new()
                    .with_metric(VshipMetric::Cvvdp)
                    .with_fps(rate)
                    .with_disable_temporal(disabled);
                assert!(
                    matches!(
                        config.validate(),
                        Err(VshipError::InvalidConfiguration { .. })
                    ),
                    "{rate}"
                );
                assert!(
                    matches!(
                        crate::VshipScorer::new(config.clone(), format, format, None),
                        Err(VshipError::InvalidConfiguration { .. })
                    ),
                    "{rate} must fail before library discovery"
                );
            }
        }
    }

    #[test]
    fn cvvdp_accepts_positive_fps_through_one_hundred_thousand() {
        for rate in [f32::MIN_POSITIVE, 24.0, 23.976_025, 60.0, 100_000.0] {
            assert!(
                VshipConfig::new()
                    .with_metric(VshipMetric::Cvvdp)
                    .with_fps(rate)
                    .validate()
                    .is_ok(),
                "{rate}"
            );
        }
    }

    #[test]
    fn stateless_metrics_ignore_cvvdp_fps_bounds() {
        for rate in [
            0.0,
            -0.0,
            -1.0,
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            100_000.01,
            f32::MAX,
        ] {
            for disabled in [false, true] {
                let config = VshipConfig::new()
                    .with_metric(VshipMetric::Cvvdp)
                    .with_fps(rate)
                    .with_disable_temporal(disabled);
                for metric in [VshipMetric::Ssimulacra2, VshipMetric::Butteraugli] {
                    assert!(config.clone().with_metric(metric).validate().is_ok());
                }
            }
        }
    }

    /// Video details in the shape `av-decoders` reports them.
    fn details(
        width: usize,
        height: usize,
        bit_depth: usize,
        chroma_sampling: ChromaSubsampling,
    ) -> VideoDetails {
        VideoDetails {
            width,
            height,
            bit_depth,
            chroma_sampling,
            frame_rate: Rational32::new(24, 1),
            total_frames: None,
            color_range: None,
            color_primaries: None,
            color_transfer_characteristic: None,
            color_space: None,
            chroma_location: None,
            mastering_display: None,
            content_light_level: None,
        }
    }

    #[test]
    fn explicit_metadata_overrides_resolution_defaults() {
        use av_decoders::{
            ChromaLocation, ColorPrimaries, ColorRange, ColorSpace,
            TransferCharacteristic as Transfer,
        };
        let mut source = details(1920, 1080, 10, ChromaSubsampling::Yuv420);
        source.color_range = Some(ColorRange::Full);
        source.color_space = Some(ColorSpace::SMPTE170M);
        source.chroma_location = Some(ChromaLocation::Center);
        let color = colorspace_from_details(&source, None).unwrap();
        assert_eq!(color.range, VshipRange::Full);
        assert_eq!(color.yuv_matrix, VshipYuvMatrix::St170M);
        assert_eq!(color.chroma_location, VshipChromaLocation::Center);

        source.height = 480;
        source.color_range = Some(ColorRange::Limited);
        source.color_space = Some(ColorSpace::BT2020NCL);
        source.color_primaries = Some(ColorPrimaries::BT2020);
        for (transfer, expected) in [
            (Transfer::SMPTE2084, VshipTransferFunction::Pq),
            (Transfer::ARIB_STD_B67, VshipTransferFunction::Hlg),
            (Transfer::Linear, VshipTransferFunction::Linear),
            (Transfer::IEC61966_2_1, VshipTransferFunction::SRgb),
        ] {
            source.color_transfer_characteristic = Some(transfer);
            let color = colorspace_from_details(&source, None).unwrap();
            assert_eq!(color.range, VshipRange::Limited);
            assert_eq!(color.yuv_matrix, VshipYuvMatrix::Bt2020Ncl);
            assert_eq!(color.primaries, VshipPrimaries::Bt2020);
            assert_eq!(color.transfer_function, expected);
        }
    }

    #[test]
    fn unspecified_color_metadata_matches_absent_defaults() {
        use av_decoders::{
            ChromaLocation, ColorPrimaries, ColorRange, ColorSpace,
            TransferCharacteristic as Transfer,
        };
        let mut source = details(1920, 480, 8, ChromaSubsampling::Yuv420);
        let absent = colorspace_from_details(&source, None).unwrap();
        source.color_range = Some(ColorRange::Unspecified);
        source.color_space = Some(ColorSpace::Unspecified);
        source.color_primaries = Some(ColorPrimaries::Unspecified);
        source.color_transfer_characteristic = Some(Transfer::Unspecified);
        source.chroma_location = Some(ChromaLocation::Unspecified);
        assert_eq!(colorspace_from_details(&source, None).unwrap(), absent);
    }

    #[test]
    fn unsupported_transfers_matrix_primaries_and_chroma_location_are_rejected() {
        use av_decoders::{
            ChromaLocation, ColorPrimaries, ColorRange, ColorSpace,
            TransferCharacteristic as Transfer,
        };
        let mut source = details(1920, 480, 8, ChromaSubsampling::Yuv420);
        source.color_range = Some(ColorRange::Unspecified);
        source.color_space = Some(ColorSpace::Unspecified);
        source.color_primaries = Some(ColorPrimaries::Unspecified);
        source.chroma_location = Some(ChromaLocation::Unspecified);
        for transfer in [Transfer::Reserved, Transfer::VLog, Transfer::Log] {
            source.color_transfer_characteristic = Some(transfer);
            assert!(matches!(
                colorspace_from_details(&source, None),
                Err(VshipError::UnsupportedFormat { .. })
            ));
        }
        source.color_transfer_characteristic = None;
        source.color_space = Some(ColorSpace::Reserved);
        assert!(colorspace_from_details(&source, None).is_err());
        source.color_space = None;
        source.color_primaries = Some(ColorPrimaries::SMPTE431);
        assert!(colorspace_from_details(&source, None).is_err());
        source.color_primaries = None;
        source.chroma_location = Some(ChromaLocation::Bottom);
        assert!(colorspace_from_details(&source, None).is_err());
    }

    #[test]
    fn bit_depths_map_to_the_vship_sample_enum() {
        assert_eq!(sample_for_bit_depth(8), Some(VshipSample::Uint8));
        assert_eq!(sample_for_bit_depth(9), Some(VshipSample::Uint9));
        assert_eq!(sample_for_bit_depth(10), Some(VshipSample::Uint10));
        assert_eq!(sample_for_bit_depth(12), Some(VshipSample::Uint12));
        assert_eq!(sample_for_bit_depth(14), Some(VshipSample::Uint14));
        assert_eq!(sample_for_bit_depth(16), Some(VshipSample::Uint16));
        // The C enum has no value for these depths.
        assert_eq!(sample_for_bit_depth(11), None);
        assert_eq!(sample_for_bit_depth(13), None);
    }

    #[test]
    fn chroma_sampling_maps_to_the_vship_subsampling() {
        assert_eq!(
            subsample_for_chroma(ChromaSubsampling::Yuv420),
            Some(VshipChromaSubsample { subw: 1, subh: 1 })
        );
        assert_eq!(
            subsample_for_chroma(ChromaSubsampling::Yuv422),
            Some(VshipChromaSubsample { subw: 1, subh: 0 })
        );
        assert_eq!(
            subsample_for_chroma(ChromaSubsampling::Yuv444),
            Some(VshipChromaSubsample { subw: 0, subh: 0 })
        );
        assert_eq!(subsample_for_chroma(ChromaSubsampling::Monochrome), None);
    }

    #[test]
    fn colorspace_carries_geometry_and_target_resolution() {
        let details = details(1920, 1080, 10, ChromaSubsampling::Yuv420);
        let colorspace = colorspace_from_details(&details, Some((1280, 720)))
            .expect("10-bit 4:2:0 is supported");

        assert_eq!(colorspace.width, 1920);
        assert_eq!(colorspace.height, 1080);
        assert_eq!(colorspace.target_width, 1280);
        assert_eq!(colorspace.target_height, 720);
        assert_eq!(colorspace.sample, VshipSample::Uint10);
        assert_eq!(colorspace.subsampling.subw, 1);
        assert_eq!(colorspace.subsampling.subh, 1);
        assert_eq!(colorspace.range, VshipRange::Limited);
        assert_eq!(colorspace.yuv_matrix, VshipYuvMatrix::Bt709);
        assert_eq!(colorspace.transfer_function, VshipTransferFunction::Bt709);
        assert_eq!(colorspace.primaries, VshipPrimaries::Bt709);
    }

    #[test]
    fn absent_target_resolution_means_no_scaling() {
        let details = details(640, 480, 8, ChromaSubsampling::Yuv420);
        let colorspace = colorspace_from_details(&details, None).expect("8-bit 4:2:0 is supported");

        assert_eq!(colorspace.target_width, -1);
        assert_eq!(colorspace.target_height, -1);
    }

    /// The reference and the encode are allowed to disagree on size and depth;
    /// libvship converts each per its own colorspace.
    #[test]
    fn differing_inputs_produce_independent_colorspaces() {
        let reference = details(1920, 1080, 8, ChromaSubsampling::Yuv420);
        let encode = details(1280, 720, 10, ChromaSubsampling::Yuv422);

        let reference = colorspace_from_details(&reference, Some((1920, 1080))).unwrap();
        let encode = colorspace_from_details(&encode, Some((1280, 720))).unwrap();

        assert_eq!(reference.sample, VshipSample::Uint8);
        assert_eq!(encode.sample, VshipSample::Uint10);
        assert_eq!(reference.width, 1920);
        assert_eq!(encode.width, 1280);
    }

    #[test]
    fn matrix_follows_the_bt709_height_threshold() {
        assert_eq!(matrix_for_resolution(1080), VshipYuvMatrix::Bt709);
        assert_eq!(matrix_for_resolution(651), VshipYuvMatrix::Bt709);
        assert_eq!(matrix_for_resolution(650), VshipYuvMatrix::Bt470Bg);
        assert_eq!(matrix_for_resolution(480), VshipYuvMatrix::Bt470Bg);
    }

    #[test]
    fn colorspace_rejects_eleven_bit_and_monochrome_inputs() {
        let bad_depth = details(1920, 1080, 11, ChromaSubsampling::Yuv420);
        assert!(matches!(
            colorspace_from_details(&bad_depth, None),
            Err(VshipError::UnsupportedFormat { .. })
        ));

        let monochrome = details(1920, 1080, 8, ChromaSubsampling::Monochrome);
        assert!(matches!(
            colorspace_from_details(&monochrome, None),
            Err(VshipError::UnsupportedFormat { .. })
        ));
    }

    #[test]
    fn handler_count_is_independent_of_legacy_request() {
        let config = VshipConfig::new().with_handler_threads(8);

        assert_eq!(config.handler_threads(), 8);
        assert_eq!(config.handler_count(), 1);
        assert_eq!(
            config.clone().with_metric(VshipMetric::Butteraugli).handler_count(),
            1
        );
        assert_eq!(config.with_metric(VshipMetric::Cvvdp).handler_count(), 1);
    }

    #[test]
    fn zero_threads_is_clamped_to_one() {
        let config = VshipConfig::new().with_handler_threads(0);

        assert_eq!(config.handler_threads(), 1);
        assert_eq!(config.handler_count(), 1);
    }

    #[test]
    fn default_config_selects_ssimulacra2_and_documented_butteraugli_cvvdp_options() {
        let config = VshipConfig::new();

        assert_eq!(config.metric(), VshipMetric::Ssimulacra2);
        assert_eq!(config.handler_threads(), DEFAULT_HANDLER_THREADS);
        assert_eq!(config.butteraugli().q_norm, 2);
        assert_eq!(config.butteraugli().intensity_multiplier, 203.0);
        assert_eq!(config.cvvdp().display_model, "standard_fhd");
        assert!(!config.cvvdp().resize_to_display);
        assert!(config.cvvdp().reset_on_discontinuity);
        assert!(!config.cvvdp().disable_temporal);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn config_builders_retain_metric_device_threads_norm_intensity_and_temporal_options() {
        let config = VshipConfig::new()
            .with_metric(VshipMetric::Butteraugli)
            .with_gpu_id(2)
            .with_handler_threads(6)
            .with_q_norm(3)
            .with_intensity_multiplier(400.0)
            .with_resize_to_display(true)
            .with_fps(60.0)
            .with_disable_temporal(true);

        assert_eq!(config.metric(), VshipMetric::Butteraugli);
        assert_eq!(config.gpu_id(), 2);
        assert_eq!(config.handler_threads(), 6);
        assert_eq!(config.butteraugli().q_norm, 3);
        assert_eq!(config.butteraugli().intensity_multiplier, 400.0);
        assert!(config.cvvdp().resize_to_display);
        assert_eq!(config.cvvdp().fps, 60.0);
        assert!(config.cvvdp().reset_on_discontinuity);
        assert!(config.cvvdp().disable_temporal);
    }

    #[test]
    fn temporal_and_gap_policy_are_independent() {
        for disabled in [false, true] {
            for gap in [false, true] {
                let config = VshipConfig::new()
                    .with_disable_temporal(disabled)
                    .with_reset_on_discontinuity(gap);
                assert_eq!(config.cvvdp().disable_temporal, disabled);
                assert_eq!(config.cvvdp().reset_on_discontinuity, gap);
            }
        }
    }

    #[test]
    fn display_name_and_json_path_builders_reject_interior_nul() {
        // Each builder takes `self` by value, so a fresh config is used per
        // case rather than cloning one.
        assert!(
            VshipConfig::new()
                .with_display_model("standard\0_fhd")
                .is_err_and(|error| matches!(error, VshipError::InvalidConfiguration { .. }))
        );
        assert!(
            VshipConfig::new()
                .with_display_model_json("C:\\models\\my\0model.json")
                .is_err_and(|error| matches!(error, VshipError::InvalidConfiguration { .. }))
        );
    }

    #[test]
    fn display_model_builder_preserves_valid_name() {
        let config = VshipConfig::new().with_display_model("uhd_4k").unwrap();
        assert_eq!(config.cvvdp().display_model, "uhd_4k");
    }

    #[test]
    fn butteraugli_validation_rejects_zero_qnorm() {
        let bad_qnorm = VshipConfig::new().with_metric(VshipMetric::Butteraugli).with_q_norm(0);
        assert!(bad_qnorm.validate().is_err());
    }

    #[test]
    fn butteraugli_validation_rejects_negative_intensity() {
        let bad_intensity = VshipConfig::new()
            .with_metric(VshipMetric::Butteraugli)
            .with_intensity_multiplier(-1.0);
        assert!(bad_intensity.validate().is_err());
    }

    #[test]
    fn cvvdp_validation_rejects_nan_fps() {
        let bad_fps = VshipConfig::new().with_metric(VshipMetric::Cvvdp).with_fps(f32::NAN);
        assert!(bad_fps.validate().is_err());
    }

    #[test]
    fn cvvdp_validation_rejects_empty_display_model() {
        // An empty display model is only rejected when CVVDP is the metric.
        let empty = VshipConfig::new()
            .with_metric(VshipMetric::Cvvdp)
            .with_display_model("")
            .expect("an empty name is a valid C string");
        assert!(empty.validate().is_err());
    }

    #[test]
    fn metric_names_parse_case_insensitively() {
        assert_eq!(
            VshipMetric::from_name("ssimulacra2"),
            Some(VshipMetric::Ssimulacra2)
        );
        assert_eq!(
            VshipMetric::from_name("SSIMULACRA2"),
            Some(VshipMetric::Ssimulacra2)
        );
        assert_eq!(
            VshipMetric::from_name("SSIM2"),
            Some(VshipMetric::Ssimulacra2)
        );
        assert_eq!(
            VshipMetric::from_name("butteraugli"),
            Some(VshipMetric::Butteraugli)
        );
        assert_eq!(VshipMetric::from_name("cvvdp"), Some(VshipMetric::Cvvdp));
        assert_eq!(VshipMetric::from_name("vmaf"), None);
        assert_eq!(VshipMetric::from_name(""), None);
    }

    #[test]
    fn butteraugli_is_lower_better_and_other_metrics_higher_better() {
        assert!(VshipMetric::Ssimulacra2.higher_is_better());
        assert!(!VshipMetric::Butteraugli.higher_is_better());
        assert!(VshipMetric::Cvvdp.higher_is_better());
    }

    #[test]
    fn only_cvvdp_is_temporal() {
        assert!(!VshipMetric::Ssimulacra2.is_temporal());
        assert!(!VshipMetric::Butteraugli.is_temporal());
        assert!(VshipMetric::Cvvdp.is_temporal());
    }

    #[test]
    fn all_metrics_lists_three_variants() {
        assert_eq!(VshipMetric::all().len(), 3);
    }

    #[test]
    fn pool_method_returns_requested_mean_min_or_max() {
        let scores = [1.0, 5.0, 3.0];

        assert_eq!(PoolMethod::Mean.apply(&scores).unwrap(), 3.0);
        assert_eq!(PoolMethod::Min.apply(&scores).unwrap(), 1.0);
        assert_eq!(PoolMethod::Max.apply(&scores).unwrap(), 5.0);
    }

    #[test]
    fn pool_method_rejects_empty_scores() {
        assert!(PoolMethod::Mean.apply(&[]).is_err());
    }
}
