#![forbid(unsafe_code)]

use std::mem::{offset_of, size_of};

/// Layout mirror of dav1d API 7's `Dav1dFilmGrainData` in `dav1d/headers.h`.
/// Only field values may be serialized: Rust moves need not preserve padding.
#[repr(C)]
#[derive(Clone)]
pub(crate) struct Dav1dFilmGrainDataRaw {
    pub(crate) seed: u32,
    pub(crate) num_y_points: i32,
    pub(crate) y_points: [[u8; 2]; 14],
    pub(crate) chroma_scaling_from_luma: i32,
    pub(crate) num_uv_points: [i32; 2],
    pub(crate) uv_points: [[[u8; 2]; 10]; 2],
    pub(crate) scaling_shift: i32,
    pub(crate) ar_coeff_lag: i32,
    pub(crate) ar_coeffs_y: [i8; 24],
    pub(crate) ar_coeffs_uv: [[i8; 28]; 2],
    pub(crate) ar_coeff_shift: u64,
    pub(crate) grain_scale_shift: i32,
    pub(crate) uv_mult: [i32; 2],
    pub(crate) uv_luma_mult: [i32; 2],
    pub(crate) uv_offset: [i32; 2],
    pub(crate) overlap_flag: i32,
    pub(crate) clip_to_restricted_range: i32,
}

const _: () = assert!(
    size_of::<std::ffi::c_int>() == 4 && size_of::<std::ffi::c_uint>() == 4,
    "unsupported dav1d ABI: C int and unsigned must be 32-bit"
);

impl Dav1dFilmGrainDataRaw {
    pub(crate) fn to_bytes(&self) -> Vec<u8> {
        // SECURITY: Never read the mirror's object representation. Only this
        // byte buffer owns initialized padding, including the ABI's tail.
        let mut bytes = vec![0; size_of::<Self>()];
        macro_rules! write {
            ($field:ident, $value:expr) => {{
                let start = offset_of!(Self, $field);
                bytes[start..start + std::mem::size_of_val(&self.$field)].copy_from_slice(&$value);
            }};
        }
        write!(seed, self.seed.to_ne_bytes());
        write!(num_y_points, self.num_y_points.to_ne_bytes());
        write!(y_points, self.y_points.as_flattened());
        write!(
            chroma_scaling_from_luma,
            self.chroma_scaling_from_luma.to_ne_bytes()
        );
        write!(
            num_uv_points,
            self.num_uv_points.map(i32::to_ne_bytes).as_flattened()
        );
        write!(uv_points, self.uv_points.as_flattened().as_flattened());
        write!(scaling_shift, self.scaling_shift.to_ne_bytes());
        write!(ar_coeff_lag, self.ar_coeff_lag.to_ne_bytes());
        write!(ar_coeffs_y, self.ar_coeffs_y.map(|value| value as u8));
        write!(
            ar_coeffs_uv,
            self.ar_coeffs_uv.map(|row| row.map(|value| value as u8)).as_flattened()
        );
        write!(ar_coeff_shift, self.ar_coeff_shift.to_ne_bytes());
        write!(grain_scale_shift, self.grain_scale_shift.to_ne_bytes());
        write!(uv_mult, self.uv_mult.map(i32::to_ne_bytes).as_flattened());
        write!(
            uv_luma_mult,
            self.uv_luma_mult.map(i32::to_ne_bytes).as_flattened()
        );
        write!(
            uv_offset,
            self.uv_offset.map(i32::to_ne_bytes).as_flattened()
        );
        write!(overlap_flag, self.overlap_flag.to_ne_bytes());
        write!(
            clip_to_restricted_range,
            self.clip_to_restricted_range.to_ne_bytes()
        );
        bytes
    }
}

#[cfg(test)]
#[path = "raw/tests.rs"]
mod tests;
