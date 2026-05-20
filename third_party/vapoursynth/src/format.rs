//! VapourSynth frame formats.

use std::ffi::{CStr, c_char};
use std::fmt::{self, Display};
use std::ops::Deref;
use std::ptr;
use vapoursynth_sys as ffi;

/// Contains information about a video format.
#[derive(Debug, Clone, Copy)]
pub struct Format<'core> {
    handle: &'core ffi::VSVideoFormat,
}

/// Preset VapourSynth formats.
///
/// The presets suffixed with H and S have floating point sample type. The H and S suffixes stand
/// for half precision and single precision, respectively.
///
/// Format IDs in VapourSynth v4 are computed using the formula:
/// `(colorFamily << 28) | (sampleType << 24) | (bitsPerSample << 16) | (subSamplingW << 8) | (subSamplingH << 0)`
#[repr(i32)]
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum PresetFormat {
    None = 0,

    Gray8 = make_video_id(ColorFamily::Gray, SampleType::Integer, 8, 0, 0),
    Gray9 = make_video_id(ColorFamily::Gray, SampleType::Integer, 9, 0, 0),
    Gray10 = make_video_id(ColorFamily::Gray, SampleType::Integer, 10, 0, 0),
    Gray12 = make_video_id(ColorFamily::Gray, SampleType::Integer, 12, 0, 0),
    Gray14 = make_video_id(ColorFamily::Gray, SampleType::Integer, 14, 0, 0),
    Gray16 = make_video_id(ColorFamily::Gray, SampleType::Integer, 16, 0, 0),
    Gray32 = make_video_id(ColorFamily::Gray, SampleType::Integer, 32, 0, 0),

    GrayH = make_video_id(ColorFamily::Gray, SampleType::Float, 16, 0, 0),
    GrayS = make_video_id(ColorFamily::Gray, SampleType::Float, 32, 0, 0),

    YUV410P8 = make_video_id(ColorFamily::YUV, SampleType::Integer, 8, 2, 2),
    YUV411P8 = make_video_id(ColorFamily::YUV, SampleType::Integer, 8, 2, 0),
    YUV440P8 = make_video_id(ColorFamily::YUV, SampleType::Integer, 8, 0, 1),

    YUV420P8 = make_video_id(ColorFamily::YUV, SampleType::Integer, 8, 1, 1),
    YUV422P8 = make_video_id(ColorFamily::YUV, SampleType::Integer, 8, 1, 0),
    YUV444P8 = make_video_id(ColorFamily::YUV, SampleType::Integer, 8, 0, 0),

    YUV420P9 = make_video_id(ColorFamily::YUV, SampleType::Integer, 9, 1, 1),
    YUV422P9 = make_video_id(ColorFamily::YUV, SampleType::Integer, 9, 1, 0),
    YUV444P9 = make_video_id(ColorFamily::YUV, SampleType::Integer, 9, 0, 0),

    YUV420P10 = make_video_id(ColorFamily::YUV, SampleType::Integer, 10, 1, 1),
    YUV422P10 = make_video_id(ColorFamily::YUV, SampleType::Integer, 10, 1, 0),
    YUV444P10 = make_video_id(ColorFamily::YUV, SampleType::Integer, 10, 0, 0),

    YUV420P12 = make_video_id(ColorFamily::YUV, SampleType::Integer, 12, 1, 1),
    YUV422P12 = make_video_id(ColorFamily::YUV, SampleType::Integer, 12, 1, 0),
    YUV444P12 = make_video_id(ColorFamily::YUV, SampleType::Integer, 12, 0, 0),

    YUV420P14 = make_video_id(ColorFamily::YUV, SampleType::Integer, 14, 1, 1),
    YUV422P14 = make_video_id(ColorFamily::YUV, SampleType::Integer, 14, 1, 0),
    YUV444P14 = make_video_id(ColorFamily::YUV, SampleType::Integer, 14, 0, 0),

    YUV420P16 = make_video_id(ColorFamily::YUV, SampleType::Integer, 16, 1, 1),
    YUV422P16 = make_video_id(ColorFamily::YUV, SampleType::Integer, 16, 1, 0),
    YUV444P16 = make_video_id(ColorFamily::YUV, SampleType::Integer, 16, 0, 0),

    YUV420PH = make_video_id(ColorFamily::YUV, SampleType::Float, 16, 1, 1),
    YUV420PS = make_video_id(ColorFamily::YUV, SampleType::Float, 32, 1, 1),
    YUV422PH = make_video_id(ColorFamily::YUV, SampleType::Float, 16, 1, 0),
    YUV422PS = make_video_id(ColorFamily::YUV, SampleType::Float, 32, 1, 0),
    YUV444PH = make_video_id(ColorFamily::YUV, SampleType::Float, 16, 0, 0),
    YUV444PS = make_video_id(ColorFamily::YUV, SampleType::Float, 32, 0, 0),

    RGB24 = make_video_id(ColorFamily::RGB, SampleType::Integer, 8, 0, 0),
    RGB27 = make_video_id(ColorFamily::RGB, SampleType::Integer, 9, 0, 0),
    RGB30 = make_video_id(ColorFamily::RGB, SampleType::Integer, 10, 0, 0),
    RGB36 = make_video_id(ColorFamily::RGB, SampleType::Integer, 12, 0, 0),
    RGB42 = make_video_id(ColorFamily::RGB, SampleType::Integer, 14, 0, 0),
    RGB48 = make_video_id(ColorFamily::RGB, SampleType::Integer, 16, 0, 0),

    RGBH = make_video_id(ColorFamily::RGB, SampleType::Float, 16, 0, 0),
    RGBS = make_video_id(ColorFamily::RGB, SampleType::Float, 32, 0, 0),
}

/// Format color families.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum ColorFamily {
    Undefined = 0,
    Gray = 1,
    RGB = 2,
    YUV = 3,
}

/// Format sample types.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum SampleType {
    Integer = 0,
    Float = 1,
}

/// Computes a VapourSynth video format ID from its components.
///
/// This is equivalent to the C macro:
/// `VS_MAKE_VIDEO_ID(colorFamily, sampleType, bitsPerSample, subSamplingW, subSamplingH)`
const fn make_video_id(
    color_family: ColorFamily,
    sample_type: SampleType,
    bits_per_sample: i32,
    sub_sampling_w: i32,
    sub_sampling_h: i32,
) -> i32 {
    ((color_family as i32) << 28)
        | ((sample_type as i32) << 24)
        | (bits_per_sample << 16)
        | (sub_sampling_w << 8)
        | sub_sampling_h
}

/// A unique format identifier.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct FormatID(pub(crate) i32);

impl<'core> PartialEq for Format<'core> {
    #[inline]
    fn eq(&self, other: &Format<'core>) -> bool {
        self.id() == other.id()
    }
}

impl<'core> Eq for Format<'core> {}

#[doc(hidden)]
impl<'core> Deref for Format<'core> {
    type Target = ffi::VSVideoFormat;

    // Technically this should return `&'core`.
    #[inline]
    fn deref(&self) -> &Self::Target {
        self.handle
    }
}

impl<'core> Format<'core> {
    /// Wraps a raw pointer in a `Format`.
    ///
    /// # Safety
    /// The caller must ensure `ptr` and the lifetime is valid.
    #[inline]
    pub(crate) unsafe fn from_ptr(ptr: *const ffi::VSVideoFormat) -> Self {
        Self { handle: &*ptr }
    }

    /// Gets the unique identifier of this format.
    ///
    /// In VapourSynth v4, format IDs are computed from format properties.
    #[inline]
    pub fn id(self) -> FormatID {
        use crate::api::API;

        // In v4, we compute the format ID from the properties
        unsafe {
            let api = API::get_cached();
            let id = api.query_video_format_id(
                self.handle.colorFamily,
                self.handle.sampleType,
                self.handle.bitsPerSample,
                self.handle.subSamplingW,
                self.handle.subSamplingH,
                ptr::null_mut(), // core parameter not needed for ID query
            );
            FormatID(id as i32)
        }
    }

    /// Gets the printable name of this format.
    ///
    /// In VapourSynth v4, format names are generated on-demand.
    #[inline]
    pub fn name(self) -> &'core str {
        use crate::api::API;

        // V4 requires a buffer to write the name into
        // Format names are typically short (e.g., "YUV420P8")
        const NAME_BUF_SIZE: usize = 64;
        let mut buf = [0 as c_char; NAME_BUF_SIZE];

        unsafe {
            let api = API::get_cached();
            api.get_video_format_name(self.handle as *const _, buf.as_mut_ptr());

            // Convert to Rust string
            // Note: This creates a temporary string. In v3, this returned a reference
            // to a static string. In v4, we need to handle this differently.
            // For now, we leak the string to maintain the 'core lifetime.
            let cstr = CStr::from_ptr(buf.as_ptr());
            let string = cstr.to_str().unwrap().to_owned();
            Box::leak(string.into_boxed_str())
        }
    }

    /// Gets the number of planes of this format.
    #[inline]
    pub fn plane_count(self) -> usize {
        let plane_count = self.handle.numPlanes;
        debug_assert!(plane_count >= 0);
        plane_count as usize
    }

    /// Gets the color family of this format.
    #[inline]
    pub fn color_family(self) -> ColorFamily {
        match self.handle.colorFamily {
            x if x == ffi::VSColorFamily_cfGray as i32 => ColorFamily::Gray,
            x if x == ffi::VSColorFamily_cfRGB as i32 => ColorFamily::RGB,
            x if x == ffi::VSColorFamily_cfYUV as i32 => ColorFamily::YUV,
            _ => unreachable!(),
        }
    }

    /// Gets the sample type of this format.
    #[inline]
    pub fn sample_type(self) -> SampleType {
        match self.handle.sampleType {
            x if x == ffi::VSSampleType_stInteger as i32 => SampleType::Integer,
            x if x == ffi::VSSampleType_stFloat as i32 => SampleType::Float,
            _ => unreachable!(),
        }
    }

    /// Gets the number of significant bits per sample.
    #[inline]
    pub fn bits_per_sample(self) -> u8 {
        let rv = self.handle.bitsPerSample;
        debug_assert!(rv >= 0 && rv <= i32::from(u8::MAX));
        rv as u8
    }

    /// Gets the number of bytes needed for a sample. This is always a power of 2 and the smallest
    /// possible that can fit the number of bits used per sample.
    #[inline]
    pub fn bytes_per_sample(self) -> u8 {
        let rv = self.handle.bytesPerSample;
        debug_assert!(rv >= 0 && rv <= i32::from(u8::MAX));
        rv as u8
    }

    /// log2 subsampling factor, applied to second and third plane.
    #[inline]
    pub fn sub_sampling_w(self) -> u8 {
        let rv = self.handle.subSamplingW;
        debug_assert!(rv >= 0 && rv <= i32::from(u8::MAX));
        rv as u8
    }

    /// log2 subsampling factor, applied to second and third plane.
    #[inline]
    pub fn sub_sampling_h(self) -> u8 {
        let rv = self.handle.subSamplingH;
        debug_assert!(rv >= 0 && rv <= i32::from(u8::MAX));
        rv as u8
    }
}

impl From<PresetFormat> for FormatID {
    fn from(x: PresetFormat) -> Self {
        FormatID(x as i32)
    }
}

#[doc(hidden)]
impl From<ColorFamily> for ffi::VSColorFamily {
    #[inline]
    fn from(x: ColorFamily) -> Self {
        match x {
            ColorFamily::Gray => ffi::VSColorFamily_cfGray,
            ColorFamily::RGB => ffi::VSColorFamily_cfRGB,
            ColorFamily::YUV => ffi::VSColorFamily_cfYUV,
            ColorFamily::Undefined => ffi::VSColorFamily_cfUndefined,
        }
    }
}

#[doc(hidden)]
impl From<SampleType> for ffi::VSSampleType {
    #[inline]
    fn from(x: SampleType) -> Self {
        match x {
            SampleType::Integer => ffi::VSSampleType_stInteger,
            SampleType::Float => ffi::VSSampleType_stFloat,
        }
    }
}

impl Display for ColorFamily {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(
            f,
            "{}",
            match *self {
                ColorFamily::Gray => "Gray",
                ColorFamily::RGB => "RGB",
                ColorFamily::YUV => "YUV",
                ColorFamily::Undefined => "Undefined",
            }
        )
    }
}

impl Display for SampleType {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(
            f,
            "{}",
            match *self {
                SampleType::Integer => "Integer",
                SampleType::Float => "Float",
            }
        )
    }
}

impl From<i32> for FormatID {
    fn from(x: i32) -> Self {
        FormatID(x)
    }
}

impl From<FormatID> for i32 {
    fn from(x: FormatID) -> Self {
        x.0
    }
}
