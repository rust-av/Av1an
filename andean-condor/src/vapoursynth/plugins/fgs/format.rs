use vapoursynth::format::{ColorFamily, Format, PresetFormat, SampleType};

/// The source clip format captured from the node, sufficient to decide
/// whether the `vs-fgs` `FGS` filter accepts the clip directly and to
/// reconstruct the original format for the round-trip back to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FgsFormat {
    pub(crate) color_family: ColorFamily,
    pub(crate) sample_type: SampleType,
    pub(crate) bits_per_sample: u8,
    pub(crate) sub_sampling_w: u8,
    pub(crate) sub_sampling_h: u8,
}

impl FgsFormat {
    /// Capture the format from a VapourSynth node's format.
    #[inline]
    pub(crate) fn from_format(format: Format<'_>) -> Self {
        Self {
            color_family: format.color_family(),
            sample_type: format.sample_type(),
            bits_per_sample: format.bits_per_sample(),
            sub_sampling_w: format.sub_sampling_w(),
            sub_sampling_h: format.sub_sampling_h(),
        }
    }

    /// `true` when the `vs-fgs` FGS engine accepts the clip directly: integer
    /// YUV with exactly 8, 10 or 12 bits per sample (`vsfgs: only constant
    /// format 8, 10, or 12-bit YUV is supported`).
    #[inline]
    pub(crate) fn fgs_accepts_plain(self) -> bool {
        self.color_family == ColorFamily::YUV
            && self.sample_type == SampleType::Integer
            && matches!(self.bits_per_sample, 8 | 10 | 12)
    }

    /// The 12-bit integer YUV [`PresetFormat`] the clip is converted to
    /// before calling FGS, matching the Python reference's
    /// `original_format.replace(bits_per_sample=12, sample_type=INTEGER)`:
    /// the color family and chroma subsampling are preserved whenever a
    /// 12-bit preset exists.
    ///
    /// `YUV410P8` (2, 2), `YUV411P8` (2, 0) and `YUV440P8` (0, 1) only exist
    /// at 8 bits in [`PresetFormat`], so they fall back to `YUV420P12`; the
    /// subsampling change is acceptable because FGS is measured and rendered
    /// per-plane.
    #[inline]
    pub(crate) fn fgs_work_preset(self) -> Option<PresetFormat> {
        if self.color_family != ColorFamily::YUV {
            return None;
        }
        Some(match (self.sub_sampling_w, self.sub_sampling_h) {
            (1, 1) => PresetFormat::YUV420P12,
            (1, 0) => PresetFormat::YUV422P12,
            (0, 0) => PresetFormat::YUV444P12,
            // YUV410 (2, 2), YUV411 (2, 0) and YUV440 (0, 1) only exist at
            // 8 bits; fall back to 4:2:0 12-bit.
            _ => PresetFormat::YUV420P12,
        })
    }

    /// The original format the FGS output is resized back to, mirroring the
    /// reference implementation's non-`MakeDiff`/`MergeDiff` branch:
    /// `fgs_clip.resize.Bicubic(format=original_format.id,
    /// dither_type="none")`.
    ///
    /// Only YUV is mapped (FGS is YUV-only); returns [`None`] for other
    /// families and for float depths without a [`PresetFormat`] (only 16- and
    /// 32-bit float presets exist).
    #[inline]
    pub(crate) fn original_preset(self) -> Option<PresetFormat> {
        if self.color_family != ColorFamily::YUV {
            return None;
        }
        match self.sample_type {
            SampleType::Integer => integer_yuv_preset(
                self.sub_sampling_w,
                self.sub_sampling_h,
                self.bits_per_sample,
            ),
            SampleType::Float => float_yuv_preset(
                self.sub_sampling_w,
                self.sub_sampling_h,
                self.bits_per_sample,
            ),
        }
    }
}

/// Rounds a bit depth to the nearest depth that has an integer YUV
/// [`PresetFormat`] (8, 9, 10, 12, 14 or 16), rounding ties upward.
#[inline]
pub(crate) const fn nearest_integer_bits(bits_per_sample: u8) -> u8 {
    match bits_per_sample {
        8 | 9 | 10 | 12 | 14 | 16 => bits_per_sample,
        0..=7 => 8,
        11 => 12,
        13 => 14,
        15 => 16,
        _ => 16,
    }
}

/// The integer YUV [`PresetFormat`] with the given subsampling at the closest
/// integer bit depth, or [`None`] when the combination has no preset.
#[inline]
pub(crate) fn integer_yuv_preset(
    sub_sampling_w: u8,
    sub_sampling_h: u8,
    bits_per_sample: u8,
) -> Option<PresetFormat> {
    let bits = nearest_integer_bits(bits_per_sample);
    match (sub_sampling_w, sub_sampling_h, bits) {
        (1, 1, 8) => Some(PresetFormat::YUV420P8),
        (1, 1, 9) => Some(PresetFormat::YUV420P9),
        (1, 1, 10) => Some(PresetFormat::YUV420P10),
        (1, 1, 12) => Some(PresetFormat::YUV420P12),
        (1, 1, 14) => Some(PresetFormat::YUV420P14),
        (1, 1, 16) => Some(PresetFormat::YUV420P16),
        (1, 0, 8) => Some(PresetFormat::YUV422P8),
        (1, 0, 9) => Some(PresetFormat::YUV422P9),
        (1, 0, 10) => Some(PresetFormat::YUV422P10),
        (1, 0, 12) => Some(PresetFormat::YUV422P12),
        (1, 0, 14) => Some(PresetFormat::YUV422P14),
        (1, 0, 16) => Some(PresetFormat::YUV422P16),
        (0, 0, 8) => Some(PresetFormat::YUV444P8),
        (0, 0, 9) => Some(PresetFormat::YUV444P9),
        (0, 0, 10) => Some(PresetFormat::YUV444P10),
        (0, 0, 12) => Some(PresetFormat::YUV444P12),
        (0, 0, 14) => Some(PresetFormat::YUV444P14),
        (0, 0, 16) => Some(PresetFormat::YUV444P16),
        (2, 2, 8) => Some(PresetFormat::YUV410P8),
        (2, 0, 8) => Some(PresetFormat::YUV411P8),
        (0, 1, 8) => Some(PresetFormat::YUV440P8),
        _ => None,
    }
}

/// The YUV float [`PresetFormat`] with the given subsampling for 16-bit
/// (half) or 32-bit (single) float samples; [`None`] for other depths.
#[inline]
pub(crate) fn float_yuv_preset(
    sub_sampling_w: u8,
    sub_sampling_h: u8,
    bits_per_sample: u8,
) -> Option<PresetFormat> {
    Some(match (sub_sampling_w, sub_sampling_h, bits_per_sample) {
        (1, 1, 16) => PresetFormat::YUV420PH,
        (1, 1, 32) => PresetFormat::YUV420PS,
        (1, 0, 16) => PresetFormat::YUV422PH,
        (1, 0, 32) => PresetFormat::YUV422PS,
        (0, 0, 16) => PresetFormat::YUV444PH,
        (0, 0, 32) => PresetFormat::YUV444PS,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an [`FgsFormat`] without a VapourSynth core.
    fn format(
        color_family: ColorFamily,
        sample_type: SampleType,
        bits_per_sample: u8,
        sub_sampling_w: u8,
        sub_sampling_h: u8,
    ) -> FgsFormat {
        FgsFormat {
            color_family,
            sample_type,
            bits_per_sample,
            sub_sampling_w,
            sub_sampling_h,
        }
    }

    #[test]
    fn fgs_accepts_plain_only_for_integer_yuv_8_10_12() {
        for bits in [8u8, 10, 12].into_iter() {
            assert!(format(ColorFamily::YUV, SampleType::Integer, bits, 1, 1).fgs_accepts_plain());
        }

        // Unsupported depths, float sample types and non-YUV families.
        assert!(!format(ColorFamily::YUV, SampleType::Integer, 16, 1, 1).fgs_accepts_plain());
        assert!(!format(ColorFamily::YUV, SampleType::Float, 32, 1, 1).fgs_accepts_plain());
        assert!(!format(ColorFamily::Gray, SampleType::Integer, 8, 0, 0).fgs_accepts_plain());
    }

    #[test]
    fn fgs_work_preset_maps_subsampling_and_falls_back_to_420() {
        let yuv420 = format(ColorFamily::YUV, SampleType::Float, 32, 1, 1);
        assert_eq!(yuv420.fgs_work_preset(), Some(PresetFormat::YUV420P12));

        let yuv422 = format(ColorFamily::YUV, SampleType::Integer, 16, 1, 0);
        assert_eq!(yuv422.fgs_work_preset(), Some(PresetFormat::YUV422P12));

        let yuv444 = format(ColorFamily::YUV, SampleType::Integer, 8, 0, 0);
        assert_eq!(yuv444.fgs_work_preset(), Some(PresetFormat::YUV444P12));

        // YUV410 (4:2:0 subsampling 2,2) has no 12-bit preset: falls back to
        // 4:2:0 12-bit.
        let yuv410 = format(ColorFamily::YUV, SampleType::Integer, 8, 2, 2);
        assert_eq!(yuv410.fgs_work_preset(), Some(PresetFormat::YUV420P12));

        // Non-YUV families have no work format.
        assert_eq!(
            format(ColorFamily::Gray, SampleType::Integer, 8, 0, 0).fgs_work_preset(),
            None,
        );
    }

    #[test]
    fn fgs_original_preset_preserves_yuv_depth_and_sample_type() {
        // The noise generator's 32-bit float source: 12-bit work clip back to
        // the original float preset.
        let yuv420ps = format(ColorFamily::YUV, SampleType::Float, 32, 1, 1);
        assert_eq!(yuv420ps.original_preset(), Some(PresetFormat::YUV420PS));

        // 16-bit integer source: back to the same 16-bit subsampling preset.
        let yuv420_p16 = format(ColorFamily::YUV, SampleType::Integer, 16, 1, 1);
        assert_eq!(yuv420_p16.original_preset(), Some(PresetFormat::YUV420P16));

        // Exact 8/10/12-bit presets are preserved with their subsampling.
        let yuv422_p8 = format(ColorFamily::YUV, SampleType::Integer, 8, 1, 0);
        assert_eq!(yuv422_p8.original_preset(), Some(PresetFormat::YUV422P8));

        let yuv444_p10 = format(ColorFamily::YUV, SampleType::Integer, 10, 0, 0);
        assert_eq!(yuv444_p10.original_preset(), Some(PresetFormat::YUV444P10));

        // Half-precision float 4:4:4.
        let yuv444_ph = format(ColorFamily::YUV, SampleType::Float, 16, 0, 0);
        assert_eq!(yuv444_ph.original_preset(), Some(PresetFormat::YUV444PH));

        // Non-YUV families and float depths without a preset have no
        // round-trip format.
        assert_eq!(
            format(ColorFamily::Gray, SampleType::Integer, 8, 0, 0).original_preset(),
            None,
        );
        let odd_float = format(ColorFamily::YUV, SampleType::Float, 24, 1, 1);
        assert_eq!(odd_float.original_preset(), None);
    }

    #[test]
    fn fgs_integer_preset_rounds_depth_up_and_rejects_unsupported_subsampling() {
        assert_eq!(integer_yuv_preset(1, 1, 9), Some(PresetFormat::YUV420P9));
        assert_eq!(integer_yuv_preset(1, 1, 11), Some(PresetFormat::YUV420P12));
        assert_eq!(integer_yuv_preset(1, 1, 13), Some(PresetFormat::YUV420P14));
        assert_eq!(integer_yuv_preset(1, 1, 15), Some(PresetFormat::YUV420P16));
        assert_eq!(integer_yuv_preset(1, 1, 17), Some(PresetFormat::YUV420P16));

        // 4:1:1 (2, 0) at 10 bits has no preset.
        assert_eq!(integer_yuv_preset(2, 0, 10), None);
    }
}
