//! Configuration for fmetrics scoring: which metric, and how to read the input.
//!
//! Colour properties come from the caller's [`VideoDetails`] rather than being
//! hardcoded, because the YUV to RGB conversion in [`crate::convert`] is part
//! of the measurement: a wrong matrix silently changes every score.

use av_decoders::{ColorPrimaries, ColorRange, ColorSpace, TransferCharacteristic, VideoDetails};

use crate::error::FmetricsError;

/// The default worker count, matching the VapourSynth plugin path's four
/// handlers so the two agree when `threads` is unset.
pub const DEFAULT_THREADS: u32 = 4;

/// The finite Butteraugli exponent used when none is configured at this low level.
/// This is not the application's/plugin's default infinity norm.
pub const DEFAULT_PNORM: i32 = 3;

/// A metric fmetrics can compute.
///
/// IW-SSIM and MS-SSIM operate over five scales and score in the opposite
/// direction to the perceptual metrics, which is why
/// [`FmetricsMetric::prefers_lower_is_better`] exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FmetricsMetric {
    /// SSIMULACRA2.
    Ssimulacra2,
    /// Butteraugli.
    Butteraugli,
    /// CVVDP.
    Cvvdp,
    /// IW-SSIM, a scale-invariant structural similarity.
    Iwssim,
    /// MS-SSIM, multi-scale structural similarity.
    Msssim,
}

impl FmetricsMetric {
    /// A stable lowercase name, for logs and error messages.
    #[inline]
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ssimulacra2 => "ssimu2",
            Self::Butteraugli => "butteraugli",
            Self::Cvvdp => "cvvdp",
            Self::Iwssim => "iwssim",
            Self::Msssim => "msssim",
        }
    }

    /// Whether a lower score is better.
    ///
    /// Butteraugli is a distance; the rest are quality measures. Pooling a
    /// metric the wrong way round silently inverts a quality judgement.
    #[inline]
    #[must_use]
    pub const fn prefers_lower_is_better(self) -> bool {
        matches!(self, Self::Butteraugli)
    }

    /// The minimum image dimension the metric can score, or [`None`] if it has
    /// no floor.
    ///
    /// The two multi-scale metrics need an image reducible four times, and they
    /// do not agree on where that begins: IW-SSIM refuses a 15-pixel image with
    /// [`crate::FmetricsErr::IwssimImgTooSmall`] where MS-SSIM still scores it.
    /// The larger floor is given so a caller has one number to check before
    /// decoding; the library decides the exact one.
    #[inline]
    #[must_use]
    pub const fn minimum_dimension(self) -> Option<usize> {
        match self {
            Self::Iwssim | Self::Msssim => Some(16),
            Self::Ssimulacra2 | Self::Butteraugli | Self::Cvvdp => None,
        }
    }

    /// Whether the metric needs a frame rate.
    ///
    /// Only CVVDP, whose temporal filter is parameterised by the rate.
    #[inline]
    #[must_use]
    pub const fn is_temporal(self) -> bool {
        matches!(self, Self::Cvvdp)
    }
}

/// The YUV to RGB matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YuvMatrix {
    /// ITU-R BT.601.
    Bt601,
    /// ITU-R BT.709.
    Bt709,
    /// ITU-R BT.2020 non-constant luminance.
    Bt2020,
}

/// Luma or chroma sample range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleRange {
    /// Limited (studio) range: luma 16-235, chroma 16-240.
    Limited,
    /// Full range: 0-255 throughout.
    Full,
}

/// The colour properties fmetrics needs to read a frame.
///
/// fmetrics understands only sRGB and linear sRGB, so the *conversion* matrix
/// and range are ours to supply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorInfo {
    /// The YUV to RGB matrix.
    pub matrix: YuvMatrix,
    /// Sample range.
    pub range: SampleRange,
    /// Source bit depth.
    pub bit_depth: usize,
    /// Source transfer, resolved to sRGB when decoder metadata is unknown.
    pub transfer: TransferCharacteristic,
    /// Output RGB encoding: video-family transfers are converted to sRGB.
    pub colorspace: crate::ffi::FmetricsColorspace,
    /// Whether the source carries HDR values.
    ///
    /// This flag describes already-supported RGB values, not a PQ/HLG decoder.
    /// [`Self::from_details`] rejects unsupported transfer functions and gamuts.
    pub hdr: bool,
}

impl ColorInfo {
    /// Derive the colour properties from a clip's details.
    ///
    /// Explicit metadata wins. Only absent/`Unspecified` values fall back to
    /// limited range, BT.709 above 650 lines (BT.601 otherwise), and sRGB.
    /// Video-family SDR transfers are decoded using BT.1886 gamma 2.4 and encoded
    /// as sRGB. Existing sRGB/linear samples retain their encoding. Only BT.709
    /// primaries are supported; no gamut or HDR conversion is performed.
    ///
    /// # Errors
    ///
    /// Returns an error for unsupported explicit color metadata or bit depth. 8-bit maps
    /// to `RGB_UINT8`; 10- and 12-bit are up-converted to `RGB_UINT16`
    /// rather than truncated.
    #[inline]
    pub fn from_details(details: &VideoDetails) -> Result<Self, FmetricsError> {
        if !matches!(details.bit_depth, 8 | 10 | 12) {
            return Err(FmetricsError::UnsupportedFormat {
                reason: format!(
                    "{}-bit input cannot be represented: fmetrics accepts 8-bit directly and 10- \
                     or 12-bit up-converted to 16-bit",
                    details.bit_depth
                ),
            });
        }

        Ok(Self {
            matrix: match details.color_space {
                None | Some(ColorSpace::Unspecified) => matrix_for_resolution(details.height),
                Some(ColorSpace::BT709) => YuvMatrix::Bt709,
                Some(ColorSpace::BT470BG | ColorSpace::SMPTE170M) => YuvMatrix::Bt601,
                Some(ColorSpace::BT2020NCL) => YuvMatrix::Bt2020,
                Some(matrix) => {
                    return Err(FmetricsError::UnsupportedFormat {
                        reason: format!("matrix {matrix:?} has no fmetrics YUV conversion"),
                    });
                },
            },
            range: match details.color_range {
                Some(ColorRange::Full) => SampleRange::Full,
                None | Some(ColorRange::Unspecified | ColorRange::Limited) => SampleRange::Limited,
            },
            bit_depth: details.bit_depth,
            transfer: match details.color_transfer_characteristic {
                None | Some(TransferCharacteristic::Unspecified) => {
                    TransferCharacteristic::IEC61966_2_1
                },
                Some(transfer) => transfer,
            },
            colorspace: colorspace_from_details(details)?,
            hdr: false,
        })
    }

    /// Mark this source as carrying HDR values.
    #[inline]
    #[must_use]
    pub const fn with_hdr(mut self, hdr: bool) -> Self {
        self.hdr = hdr;
        self
    }

    /// Override the declared matrix.
    #[inline]
    #[must_use]
    pub const fn with_matrix(mut self, matrix: YuvMatrix) -> Self {
        self.matrix = matrix;
        self
    }

    /// Override the declared range.
    #[inline]
    #[must_use]
    pub const fn with_range(mut self, range: SampleRange) -> Self {
        self.range = range;
        self
    }

    /// The meaningful code peak for validating the conversion's depth agreement.
    /// yuvxyb derives normalization from depth/range; limited range uses shifted
    /// nominal endpoints rather than dividing all samples by this peak.
    ///
    /// # Errors
    ///
    /// Returns an error for a bit depth with no defined peak.
    #[inline]
    pub fn max_sample(&self) -> Result<u32, FmetricsError> {
        match self.bit_depth {
            8 => Ok(255),
            10 => Ok(1023),
            12 => Ok(4095),
            // Reaching this arm means `ColorInfo` was built by hand with an
            // unsupported depth.
            depth => Err(FmetricsError::UnsupportedFormat {
                reason: format!("no peak sample value is defined for {depth}-bit"),
            }),
        }
    }

    /// Whether samples must be handed over as 16-bit.
    ///
    /// Anything above 8 bits is up-converted rather than truncated.
    #[inline]
    #[must_use]
    pub const fn needs_wide_samples(&self) -> bool {
        self.bit_depth > 8
    }
}

/// The YUV matrix to assume for a picture of this height.
///
/// The 650-pixel threshold is the one libvship's own FFmpeg plugin uses, so the
/// same clip is declared identically on both engine paths.
#[inline]
#[must_use]
pub const fn matrix_for_resolution(height: usize) -> YuvMatrix {
    if height > 650 {
        YuvMatrix::Bt709
    } else {
        YuvMatrix::Bt601
    }
}

/// The fmetrics colorspace a clip should be submitted as.
///
/// Absent/`Unspecified` transfer and primaries retain the legacy sRGB assumption.
/// Video-family SDR transfers are converted to sRGB by the metric converter.
///
/// # Errors
///
/// Returns [`FmetricsError::UnsupportedFormat`] for other primaries or transfers,
/// including PQ and HLG. An HDR display model is not a decoder.
#[inline]
pub fn colorspace_from_details(
    details: &VideoDetails,
) -> Result<crate::ffi::FmetricsColorspace, FmetricsError> {
    if let Some(primaries) = details.color_primaries
        && !matches!(
            primaries,
            ColorPrimaries::Unspecified | ColorPrimaries::BT709
        )
    {
        return Err(FmetricsError::UnsupportedFormat {
            reason: format!(
                "primaries {primaries:?} cannot be represented by fmetrics sRGB inputs; no gamut conversion is available"
            ),
        });
    }
    let transfer = source_transfer(
        details
            .color_transfer_characteristic
            .unwrap_or(TransferCharacteristic::Unspecified),
    )?;
    Ok(if transfer == yuvxyb::TransferCharacteristic::Linear {
        crate::ffi::FmetricsColorspace::LinearSrgb
    } else {
        crate::ffi::FmetricsColorspace::Srgb
    })
}

pub(crate) fn source_transfer(
    transfer: TransferCharacteristic,
) -> Result<yuvxyb::TransferCharacteristic, FmetricsError> {
    use yuvxyb::TransferCharacteristic as Target;
    match transfer {
        TransferCharacteristic::Unspecified | TransferCharacteristic::IEC61966_2_1 => {
            Ok(Target::SRGB)
        },
        TransferCharacteristic::Linear => Ok(Target::Linear),
        TransferCharacteristic::BT709 => Ok(Target::BT1886),
        TransferCharacteristic::SMPTE170M => Ok(Target::ST170M),
        TransferCharacteristic::BT2020_10 => Ok(Target::BT2020Ten),
        TransferCharacteristic::BT2020_12 => Ok(Target::BT2020Twelve),
        transfer => Err(FmetricsError::UnsupportedFormat {
            reason: format!(
                "transfer {transfer:?} has no supported SDR conversion to fmetrics sRGB/linear sRGB inputs; an HDR flag or display model does not convert it"
            ),
        }),
    }
}

/// Configuration for a scoring run.
#[derive(Debug, Clone)]
pub struct FmetricsConfig {
    /// The metric to compute.
    pub metric: FmetricsMetric,
    /// Worker count for the per-pair metrics.
    pub threads: u32,
    /// CVVDP display model.
    pub display_model: crate::ffi::FmetricsCvvdpDisplayModel,
    /// CVVDP frame rate, in frames per second.
    ///
    /// Defaults to 60 when not set. CVVDP requires a finite value in
    /// `0 < frame_rate <= 100000`, even with temporal processing disabled.
    /// Explicit zero is invalid; other metrics ignore this field.
    pub frame_rate: f32,
    /// Butteraugli p-norm exponent.
    /// `None` means finite p=3, not infinity. Explicit exponents must be positive.
    pub butteraugli_pnorm: Option<i32>,
    /// Butteraugli intensity target.
    pub intensity_target: f32,
    /// Whether the selection owner requests history-only resets at source gaps.
    ///
    /// fmetrics 0.1.0 cannot reset history without clearing accumulated scores.
    /// Callers must reject noncontiguous, temporal-enabled CVVDP selections
    /// with this policy before scoring, or choose another backend.
    pub reset_on_discontinuity: bool,
    /// Score CVVDP frames independently, clearing history AND accumulated score
    /// after each successful frame, like the VapourSynth plugin.
    pub disable_temporal: bool,
}

impl FmetricsConfig {
    /// A configuration for `metric` with default options.
    #[inline]
    #[must_use]
    pub const fn new(metric: FmetricsMetric) -> Self {
        Self {
            metric,
            threads: DEFAULT_THREADS,
            display_model: crate::ffi::FmetricsCvvdpDisplayModel::StandardFhd,
            frame_rate: 60.0,
            butteraugli_pnorm: None,
            intensity_target: 203.0,
            reset_on_discontinuity: true,
            disable_temporal: false,
        }
    }

    /// Request history-only resets at source gaps (unsupported by fmetrics 0.1.0
    /// for temporal-enabled CVVDP; selection owners must preflight this policy).
    #[inline]
    #[must_use]
    pub const fn with_reset_on_discontinuity(mut self, reset_on_discontinuity: bool) -> Self {
        self.reset_on_discontinuity = reset_on_discontinuity;
        self
    }

    /// Score each CVVDP frame independently, resetting history and accumulated
    /// score after each compute. Does not change the source-gap policy.
    #[inline]
    #[must_use]
    pub const fn with_disable_temporal(mut self, disable_temporal: bool) -> Self {
        self.disable_temporal = disable_temporal;
        self
    }

    /// Set the Butteraugli intensity target. The default of 203 is the
    /// reference intensity the metric is defined against.
    #[inline]
    #[must_use]
    pub const fn with_intensity_target(mut self, intensity_target: f32) -> Self {
        self.intensity_target = intensity_target;
        self
    }

    /// Set the worker count.
    #[inline]
    #[must_use]
    pub const fn with_threads(mut self, threads: u32) -> Self {
        self.threads = threads;
        self
    }

    /// Set the CVVDP frame rate, replacing the 60 fps default.
    /// The scorer rejects nonfinite values and values outside `0 < fps <= 100000`.
    #[inline]
    #[must_use]
    pub const fn with_frame_rate(mut self, frame_rate: f32) -> Self {
        self.frame_rate = frame_rate;
        self
    }

    /// Set the CVVDP display model.
    #[inline]
    #[must_use]
    pub const fn with_display_model(
        mut self,
        display_model: crate::ffi::FmetricsCvvdpDisplayModel,
    ) -> Self {
        self.display_model = display_model;
        self
    }

    /// Set the finite Butteraugli exponent. The scorer rejects nonpositive values;
    /// large positive exponents may produce a checked nonfinite-result error.
    #[inline]
    #[must_use]
    pub const fn with_pnorm(mut self, pnorm: i32) -> Self {
        self.butteraugli_pnorm = Some(pnorm);
        self
    }

    /// The configured worker count, before [`Self::effective_threads`] clamps
    /// it.
    #[inline]
    #[must_use]
    pub const fn threads(&self) -> u32 {
        self.threads
    }

    /// The metric being computed.
    #[inline]
    #[must_use]
    pub const fn metric(&self) -> FmetricsMetric {
        self.metric
    }

    /// The CVVDP frame rate, defaulting to 60 when not set.
    #[inline]
    #[must_use]
    pub const fn frame_rate(&self) -> f32 {
        self.frame_rate
    }

    /// The CVVDP display model.
    #[inline]
    #[must_use]
    pub const fn display_model(&self) -> crate::ffi::FmetricsCvvdpDisplayModel {
        self.display_model
    }

    /// The effective worker count, with zero clamped to one.
    ///
    /// Zero means "all cores" to the library, which the scorer resolves itself;
    /// a pool of no workers is not a meaningful request.
    #[inline]
    #[must_use]
    pub const fn effective_threads(&self) -> u32 {
        if self.threads == 0 { 1 } else { self.threads }
    }

    /// Whether the configured metric is temporal and needs a frame rate.
    #[inline]
    #[must_use]
    pub const fn is_temporal(&self) -> bool {
        self.metric.is_temporal()
    }

    /// The Butteraugli options this configuration implies.
    ///
    /// A configured p-norm passes through; an absent one becomes finite
    /// [`DEFAULT_PNORM`]. The scorer rejects explicit nonpositive exponents before
    /// loading the library rather than letting native code silently default them.
    ///
    /// There is no infinity-norm sentinel: the library computes `pow(d,
    /// pnorm)`, so a large exponent may overflow. The scorer rejects nonfinite
    /// Butteraugli results even when the native status is OK.
    #[inline]
    #[must_use]
    pub const fn butteraugli_options(&self) -> crate::ffi::FmetricsButteraugliOptions {
        crate::ffi::FmetricsButteraugliOptions {
            intensity_target: self.intensity_target,
            pnorm: match self.butteraugli_pnorm {
                Some(pnorm) => pnorm,
                None => DEFAULT_PNORM,
            },
        }
    }
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "a failing assertion should panic loudly, which is what a unit test wants"
)]
mod tests {
    use av_decoders::Rational32;
    use v_frame::chroma::ChromaSubsampling;

    use super::*;

    /// Video details in the shape `av-decoders` reports them.
    fn details(width: usize, height: usize, bit_depth: usize) -> VideoDetails {
        VideoDetails {
            width,
            height,
            bit_depth,
            chroma_sampling: ChromaSubsampling::Yuv420,
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
    fn sdr_source_transfer_and_output_encoding_are_distinct() {
        use TransferCharacteristic as Source;
        use yuvxyb::TransferCharacteristic as Target;
        for (source, target, output) in [
            (
                Source::IEC61966_2_1,
                Target::SRGB,
                crate::FmetricsColorspace::Srgb,
            ),
            (
                Source::Linear,
                Target::Linear,
                crate::FmetricsColorspace::LinearSrgb,
            ),
            (
                Source::BT709,
                Target::BT1886,
                crate::FmetricsColorspace::Srgb,
            ),
            (
                Source::SMPTE170M,
                Target::ST170M,
                crate::FmetricsColorspace::Srgb,
            ),
            (
                Source::BT2020_10,
                Target::BT2020Ten,
                crate::FmetricsColorspace::Srgb,
            ),
            (
                Source::BT2020_12,
                Target::BT2020Twelve,
                crate::FmetricsColorspace::Srgb,
            ),
        ] {
            let mut details = details(1920, 1080, 8);
            details.color_transfer_characteristic = Some(source);
            let info = ColorInfo::from_details(&details).unwrap();
            assert_eq!(info.transfer, source);
            assert_eq!(source_transfer(info.transfer).unwrap(), target);
            assert_eq!(info.colorspace, output);
        }
        for source in [None, Some(Source::Unspecified)] {
            let mut details = details(1920, 1080, 8);
            details.color_transfer_characteristic = source;
            let info = ColorInfo::from_details(&details).unwrap();
            assert_eq!(info.transfer, Source::IEC61966_2_1);
            assert_eq!(info.colorspace, crate::FmetricsColorspace::Srgb);
        }
    }

    #[test]
    fn explicit_range_and_matrix_override_resolution() {
        let mut source = details(1920, 1080, 10);
        source.color_range = Some(av_decoders::ColorRange::Full);
        source.color_space = Some(av_decoders::ColorSpace::SMPTE170M);
        let color = ColorInfo::from_details(&source).unwrap();
        assert_eq!(color.range, SampleRange::Full);
        assert_eq!(color.matrix, YuvMatrix::Bt601);

        source.height = 480;
        source.color_range = Some(av_decoders::ColorRange::Limited);
        source.color_space = Some(av_decoders::ColorSpace::BT2020NCL);
        let color = ColorInfo::from_details(&source).unwrap();
        assert_eq!(color.range, SampleRange::Limited);
        assert_eq!(color.matrix, YuvMatrix::Bt2020);
    }

    #[test]
    fn only_unknown_metadata_uses_defaults() {
        let mut source = details(1920, 1080, 8);
        let absent = ColorInfo::from_details(&source).unwrap();
        source.color_range = Some(av_decoders::ColorRange::Unspecified);
        source.color_space = Some(av_decoders::ColorSpace::Unspecified);
        source.color_primaries = Some(av_decoders::ColorPrimaries::Unspecified);
        source.color_transfer_characteristic =
            Some(av_decoders::TransferCharacteristic::Unspecified);
        assert_eq!(ColorInfo::from_details(&source).unwrap(), absent);
        source.color_space = Some(av_decoders::ColorSpace::Reserved);
        assert!(matches!(
            ColorInfo::from_details(&source),
            Err(FmetricsError::UnsupportedFormat { .. })
        ));
    }

    #[test]
    fn unsupported_transfer_and_gamut_do_not_become_srgb() {
        use av_decoders::{ColorPrimaries, TransferCharacteristic as Transfer};
        for transfer in
            [Transfer::SMPTE2084, Transfer::ARIB_STD_B67, Transfer::Reserved, Transfer::VLog]
        {
            let mut source = details(1920, 1080, 10);
            source.color_transfer_characteristic = Some(transfer);
            let error = ColorInfo::from_details(&source).unwrap_err();
            assert!(matches!(error, FmetricsError::UnsupportedFormat { .. }));
            assert!(error.to_string().contains("transfer"), "{error}");
        }
        for primaries in
            [ColorPrimaries::BT2020, ColorPrimaries::SMPTE432, ColorPrimaries::Reserved]
        {
            let mut source = details(1920, 1080, 10);
            source.color_primaries = Some(primaries);
            let error = ColorInfo::from_details(&source).unwrap_err();
            assert!(error.to_string().contains("primaries"), "{error}");
        }
    }

    #[test]
    fn the_default_pnorm_stays_inside_the_range_the_library_can_compute() {
        // The library computes `pow(d, pnorm)` and reads `pnorm <= 0` as 3.0.
        // An earlier default of `i32::MAX` -- intended as an "infinity norm" --
        // overflowed that exponentiation and returned non-finite scores on
        // every frame. The infinity norm is simply not expressible
        // through this API.
        let options = FmetricsConfig::new(FmetricsMetric::Butteraugli).butteraugli_options();
        assert_eq!(options.pnorm, DEFAULT_PNORM);
        assert!(
            (1..=64).contains(&options.pnorm),
            "the default exponent must be one the library can evaluate, got {}",
            options.pnorm
        );

        // A configured norm is still passed through verbatim.
        let chosen = FmetricsConfig::new(FmetricsMetric::Butteraugli)
            .with_pnorm(2)
            .butteraugli_options();
        assert_eq!(chosen.pnorm, 2);
    }

    #[test]
    fn matrix_follows_the_same_height_threshold_as_the_gpu_path() {
        assert_eq!(matrix_for_resolution(1080), YuvMatrix::Bt709);
        assert_eq!(matrix_for_resolution(651), YuvMatrix::Bt709);
        assert_eq!(matrix_for_resolution(650), YuvMatrix::Bt601);
        assert_eq!(matrix_for_resolution(480), YuvMatrix::Bt601);
    }

    #[test]
    fn color_info_matches_what_the_gpu_path_declares() {
        // Both engines must declare the same clip identically, or their scores
        // differ for reasons that have nothing to do with the metric.
        let hd = ColorInfo::from_details(&details(1920, 1080, 8)).unwrap();
        assert_eq!(hd.matrix, YuvMatrix::Bt709);
        assert_eq!(hd.range, SampleRange::Limited);

        let sd = ColorInfo::from_details(&details(720, 480, 8)).unwrap();
        assert_eq!(sd.matrix, YuvMatrix::Bt601);
    }

    #[test]
    fn a_caller_can_override_unknown_metadata() {
        let info = ColorInfo::from_details(&details(1920, 1080, 10))
            .unwrap()
            .with_matrix(YuvMatrix::Bt2020)
            .with_range(SampleRange::Full)
            .with_hdr(true);

        assert_eq!(info.matrix, YuvMatrix::Bt2020);
        assert_eq!(info.range, SampleRange::Full);
        assert!(info.hdr);
    }

    #[test]
    fn peak_sample_values_match_the_bit_depths() {
        for (depth, peak) in [(8usize, 255u32), (10, 1023), (12, 4095)] {
            let info = ColorInfo::from_details(&details(1920, 1080, depth)).unwrap();
            assert_eq!(info.max_sample().unwrap(), peak, "{depth}-bit");
        }
    }

    #[test]
    fn eight_bit_needs_narrow_samples_and_higher_needs_wide() {
        let eight = ColorInfo::from_details(&details(1920, 1080, 8)).unwrap();
        assert!(!eight.needs_wide_samples());

        for depth in [10, 12] {
            let wide = ColorInfo::from_details(&details(1920, 1080, depth)).unwrap();
            assert!(wide.needs_wide_samples(), "{depth}-bit must up-convert");
        }
    }

    #[test]
    fn unsupported_bit_depth_is_rejected_rather_than_rescaled() {
        for depth in [9, 11, 14, 16] {
            assert!(
                matches!(
                    ColorInfo::from_details(&details(1920, 1080, depth)),
                    Err(FmetricsError::UnsupportedFormat { .. })
                ),
                "{depth}-bit must be rejected"
            );
        }
    }

    #[test]
    fn absent_norm_selects_the_library_default() {
        // Previously `i32::MAX`, on the theory that an infinity norm needs a
        // large exponent. The library evaluates `pow(d, pnorm)`, so
        // that overflowed and returned non-finite scores for every
        // frame -- this test asserted the sentinel without ever
        // checking that the library could compute it.
        let config = FmetricsConfig::new(FmetricsMetric::Butteraugli);
        assert_eq!(config.butteraugli_options().pnorm, DEFAULT_PNORM);
    }

    #[test]
    fn a_configured_norm_is_passed_through() {
        for pnorm in [2, 3, 5] {
            let config = FmetricsConfig::new(FmetricsMetric::Butteraugli).with_pnorm(pnorm);
            assert_eq!(config.butteraugli_options().pnorm, pnorm);
        }
    }

    #[test]
    fn zero_threads_is_clamped_to_one() {
        let config = FmetricsConfig::new(FmetricsMetric::Ssimulacra2).with_threads(0);
        assert_eq!(config.effective_threads(), 1);
    }

    #[test]
    fn default_threads_matches_the_plugin_path() {
        assert_eq!(DEFAULT_THREADS, 4);
        assert_eq!(FmetricsConfig::new(FmetricsMetric::Ssimulacra2).threads, 4);
    }

    #[test]
    fn only_cvvdp_is_temporal() {
        assert!(FmetricsConfig::new(FmetricsMetric::Cvvdp).is_temporal());
        assert!(!FmetricsConfig::new(FmetricsMetric::Ssimulacra2).is_temporal());
        assert!(!FmetricsConfig::new(FmetricsMetric::Butteraugli).is_temporal());
    }

    #[test]
    fn cvvdp_defaults_to_the_fhd_display_model() {
        // This matches the GPU path's default, so an unconfigured CVVDP pass
        // compares against the same target range.
        assert_eq!(
            FmetricsConfig::new(FmetricsMetric::Cvvdp).display_model,
            crate::ffi::FmetricsCvvdpDisplayModel::StandardFhd
        );
    }
}
