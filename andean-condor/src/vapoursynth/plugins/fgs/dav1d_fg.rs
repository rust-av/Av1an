use anyhow::Result;
use av1_grain::{GrainTableSegment, TransferFunction, generate_photon_noise_params};

use self::raw::Dav1dFilmGrainDataRaw;
use super::FGS;
use crate::{core::input::color_range::ColorRange, vapoursynth::VapourSynthError};

mod raw;

/// Convert an `av1_grain::GrainTableSegment` (produced by
/// `generate_photon_noise_params`) into the C-compatible binary blob.
impl From<&GrainTableSegment> for Dav1dFilmGrainDataRaw {
    #[inline]
    fn from(segment: &GrainTableSegment) -> Self {
        let mut y_points = [[0u8; 2]; 14];
        for (i, pt) in segment.scaling_points_y.iter().enumerate() {
            y_points[i] = *pt;
        }

        let mut uv_points = [[[0u8; 2]; 10]; 2];
        for (i, pt) in segment.scaling_points_cb.iter().enumerate() {
            uv_points[0][i] = *pt;
        }
        for (i, pt) in segment.scaling_points_cr.iter().enumerate() {
            uv_points[1][i] = *pt;
        }

        let mut ar_coeffs_y = [0i8; 24];
        for (i, c) in segment.ar_coeffs_y.iter().enumerate() {
            ar_coeffs_y[i] = *c;
        }

        let mut ar_coeffs_uv = [[0i8; 28]; 2];
        for (i, c) in segment.ar_coeffs_cb.iter().enumerate() {
            ar_coeffs_uv[0][i] = *c;
        }
        for (i, c) in segment.ar_coeffs_cr.iter().enumerate() {
            ar_coeffs_uv[1][i] = *c;
        }

        Self {
            seed: segment.random_seed as u32,
            num_y_points: segment.scaling_points_y.len() as i32,
            y_points,
            chroma_scaling_from_luma: segment.chroma_scaling_from_luma as i32,
            num_uv_points: [
                segment.scaling_points_cb.len() as i32,
                segment.scaling_points_cr.len() as i32,
            ],
            uv_points,
            scaling_shift: segment.scaling_shift as i32,
            ar_coeff_lag: segment.ar_coeff_lag as i32,
            ar_coeffs_y,
            ar_coeffs_uv,
            ar_coeff_shift: segment.ar_coeff_shift as u64,
            grain_scale_shift: segment.grain_scale_shift as i32,
            uv_mult: [segment.cb_mult as i32, segment.cr_mult as i32],
            uv_luma_mult: [segment.cb_luma_mult as i32, segment.cr_luma_mult as i32],
            uv_offset: [segment.cb_offset as i32, segment.cr_offset as i32],
            overlap_flag: segment.overlap_flag as i32,
            clip_to_restricted_range: 1,
        }
    }
}

impl FGS {
    /// Build the `Dav1dFilmGrainData` binary blob from the stored
    /// [`PhotonNoise`] and the given clip metadata.
    ///
    /// If `width` or `height` are [`None`], falls back to
    /// [`PhotonNoise::width`] / [`PhotonNoise::height`], then to the
    /// hard-coded defaults (1920×1080).
    pub(super) fn build_grain_binary(
        &self,
        width: Option<u32>,
        height: Option<u32>,
        transfer_function: TransferFunction,
        color_range: Option<ColorRange>,
    ) -> Result<Vec<u8>, VapourSynthError> {
        self.photon_noise.validate_settings()?;
        let width = self.photon_noise.width.or(width).unwrap_or(1920);
        let height = self.photon_noise.height.or(height).unwrap_or(1080);

        let mut params = generate_photon_noise_params(
            0,
            u64::MAX,
            av1_grain::NoiseGenArgs {
                iso_setting: self.photon_noise.iso,
                width,
                height,
                transfer_function,
                chroma_grain: self
                    .photon_noise
                    .chroma_iso
                    .is_some_and(|c_iso| c_iso == self.photon_noise.iso),
                random_seed: None,
                full_range: matches!(color_range, Some(ColorRange::Full)),
            },
        );

        self.photon_noise.apply_coefficients(&mut params)?;

        // Convert to binary
        let raw: Dav1dFilmGrainDataRaw = (&params).into();

        // The vs-fgs plugin indexes `fgs_data` entries with
        // `std::min((size_t)n, fg_data_array.size() - 1)`, so a single
        // entry applies to every frame.
        Ok(raw.to_bytes())
    }
}

#[cfg(test)]
mod tests;
