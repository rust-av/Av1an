//! The pixel component trait.

#[cfg(feature = "f16-pixel-type")]
use half::f16;

use crate::format::{Format, SampleType};

/// A trait for possible pixel components.
///
/// # Safety
/// Implementing this trait allows retrieving slices of pixel data from the frame for the target
/// type, so the target type must be valid for the given format.
pub unsafe trait Component {
    /// Returns whether this component is valid for this format.
    fn is_valid(format: Format) -> bool;
}

unsafe impl Component for u8 {
    #[inline]
    fn is_valid(format: Format) -> bool {
        format.sample_type() == SampleType::Integer && format.bytes_per_sample() == 1
    }
}

unsafe impl Component for u16 {
    #[inline]
    fn is_valid(format: Format) -> bool {
        format.sample_type() == SampleType::Integer && format.bytes_per_sample() == 2
    }
}

unsafe impl Component for u32 {
    #[inline]
    fn is_valid(format: Format) -> bool {
        format.sample_type() == SampleType::Integer && format.bytes_per_sample() == 4
    }
}

#[cfg(feature = "f16-pixel-type")]
unsafe impl Component for f16 {
    #[inline]
    fn is_valid(format: Format) -> bool {
        format.sample_type() == SampleType::Float && format.bytes_per_sample() == 2
    }
}

unsafe impl Component for f32 {
    #[inline]
    fn is_valid(format: Format) -> bool {
        format.sample_type() == SampleType::Float && format.bytes_per_sample() == 4
    }
}
