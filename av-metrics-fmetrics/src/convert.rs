//! Planar YUV to interleaved RGB, which fmetrics requires.
//!
//! [`FmetricsImg`] takes one packed, interleaved buffer with a single stride,
//! while decoders hand out three planar planes.
//!
//! This conversion is part of the measurement rather than a convenience: a
//! wrong matrix or range silently changes every score. The tests check
//! known-value behaviour rather than only that the conversion runs.
//!
//! 8-bit input becomes `RGB_UINT8`; 10- and 12-bit are up-converted to
//! `RGB_UINT16` rather than truncated. Range and matrix come from [`ColorInfo`]
//! rather than a default.
//! yuvxyb supplies matrix/range/transfer conversion; this module validates and
//! copies descriptors, pads/crops valid odd edges, and packs integer output.
//! Its published math dependency retains default fastmath powers even with
//! yuvxyb's root default features disabled; numeric regressions are independent.

use std::{marker::PhantomData, mem::size_of};

use crate::{
    config::{ColorInfo, SampleRange, YuvMatrix, source_transfer},
    error::FmetricsError,
    ffi::{FmetricsColorspace, FmetricsImg},
};

impl RgbSource for RgbImage {
    #[inline]
    fn width(&self) -> usize {
        self.width
    }

    #[inline]
    fn height(&self) -> usize {
        self.height
    }

    #[inline]
    fn stride(&self) -> usize {
        self.stride()
    }

    #[inline]
    fn as_bytes(&self) -> &[u8] {
        self.as_bytes()
    }

    #[inline]
    fn is_wide(&self) -> bool {
        self.is_wide()
    }

    #[inline]
    fn as_img(&self, colorspace: FmetricsColorspace, hdr: bool) -> BorrowedImg<'_> {
        rgb_img(
            self.as_bytes(),
            self.width,
            self.height,
            self.stride(),
            self.is_wide(),
            colorspace,
            hdr,
        )
    }
}

/// A borrowed interleaved RGB image, for a caller that already has the bytes.
///
/// No ownership or allocation, so scoring a caller-supplied buffer costs
/// nothing beyond the library call.
#[derive(Debug, Clone, Copy)]
pub struct RgbView<'a> {
    data: &'a [u8],
    width: usize,
    height: usize,
    stride: usize,
    wide: bool,
}

impl<'a> RgbView<'a> {
    /// Borrow `bytes` as a tightly packed image of `width` by `height`.
    ///
    /// `stride` is `width * 3` for 8-bit samples and `width * 6` for 16-bit
    /// ones. A `wide` view uses native-endian u16 samples and requires a
    /// u16-aligned starting address, as required by the native readers.
    ///
    /// # Errors
    ///
    /// Rejects zero dimensions, arithmetic overflow, geometry outside native
    /// `c_uint` limits, buffers shorter than the complete packed image, and
    /// misaligned wide buffers.
    #[inline]
    pub fn new(
        bytes: &'a [u8],
        width: usize,
        height: usize,
        wide: bool,
    ) -> Result<Self, FmetricsError> {
        let channels = if wide { 6 } else { 3 };
        let stride = width.checked_mul(channels).ok_or_else(|| malformed("RGB stride overflow"))?;
        for value in [width, height, stride] {
            if value == 0 || std::ffi::c_uint::try_from(value).is_err() {
                return Err(malformed("RGB geometry exceeds native representation"));
            }
        }
        let extent = stride
            .checked_mul(height)
            .filter(|&n| isize::try_from(n).is_ok())
            .ok_or_else(|| malformed("RGB extent overflow"))?;
        if extent > bytes.len() {
            return Err(malformed("RGB buffer is too short"));
        }
        // CVVDP indexes const uint16_t* rows; packed wide strides preserve base alignment.
        if wide && !bytes.as_ptr().cast::<u16>().is_aligned() {
            return Err(malformed("wide RGB buffer must be u16-aligned"));
        }
        Ok(Self {
            data: bytes,
            width,
            height,
            stride,
            wide,
        })
    }
}

impl RgbSource for RgbView<'_> {
    #[inline]
    fn width(&self) -> usize {
        self.width
    }

    #[inline]
    fn height(&self) -> usize {
        self.height
    }

    #[inline]
    fn stride(&self) -> usize {
        self.stride
    }

    #[inline]
    fn as_bytes(&self) -> &[u8] {
        self.data
    }

    #[inline]
    fn is_wide(&self) -> bool {
        self.wide
    }

    #[inline]
    fn as_img(&self, colorspace: FmetricsColorspace, hdr: bool) -> BorrowedImg<'_> {
        rgb_img(
            self.data,
            self.width,
            self.height,
            self.stride,
            self.wide,
            colorspace,
            hdr,
        )
    }
}

/// Describe a packed interleaved buffer as a [`FmetricsImg`].
///
/// 8-bit samples become `RGB_UINT8` with the sRGB colorspace and 16-bit samples
/// become `RGB_UINT16`; the library has no other integer format, so the bit
/// depth maps one to one.
#[inline]
#[must_use]
fn rgb_img(
    data: &[u8],
    width: usize,
    height: usize,
    stride: usize,
    wide: bool,
    colorspace: FmetricsColorspace,
    hdr: bool,
) -> BorrowedImg<'_> {
    let width = width as std::ffi::c_uint;
    let height = height as std::ffi::c_uint;
    let stride = stride as std::ffi::c_uint;

    let raw = if wide {
        FmetricsImg::rgb16(data.as_ptr().cast(), width, height, stride, colorspace, hdr)
    } else {
        FmetricsImg::rgb8(data.as_ptr().cast(), width, height, stride, colorspace, hdr)
    };
    BorrowedImg {
        raw,
        _storage: PhantomData,
    }
}

/// A validated ABI image borrowing live RGB storage. Raw C calls remain unsafe.
#[derive(Debug)]
pub struct BorrowedImg<'a> {
    raw: FmetricsImg,
    _storage: PhantomData<&'a [u8]>,
}

impl BorrowedImg<'_> {
    /// Inspect the unchanged raw C descriptor; it is not a safe scoring input.
    #[inline]
    #[must_use]
    pub const fn as_raw(&self) -> &FmetricsImg {
        &self.raw
    }
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for super::RgbImage {}
    impl Sealed for super::RgbView<'_> {}
}

/// Anything that can be described to fmetrics as one interleaved RGB image.
///
/// Implemented by [`RgbImage`], which owns its buffer, and [`RgbView`], which
/// borrows one, so the scorer is written once against both.
/// Sealed: custom implementations cannot bypass checked geometry and storage.
pub trait RgbSource: sealed::Sealed {
    /// Width in pixels.
    fn width(&self) -> usize;
    /// Height in pixels.
    fn height(&self) -> usize;
    /// Bytes from one row to the next.
    fn stride(&self) -> usize;
    /// The interleaved samples.
    fn as_bytes(&self) -> &[u8];
    /// Whether the samples are 16-bit.
    fn is_wide(&self) -> bool;
    /// A borrowed [`FmetricsImg`] describing these samples.
    fn as_img(&self, colorspace: FmetricsColorspace, hdr: bool) -> BorrowedImg<'_>;
}

/// One planar source plane, describing a decoder's plane without owning it.
#[derive(Debug, Clone, Copy)]
pub struct PlaneSource<'a> {
    /// Width in samples, not bytes.
    width: usize,
    /// Height in samples.
    height: usize,
    /// Bytes from one row to the next. May be negative for bottom-up images.
    stride: isize,
    /// Byte offset of the first visible sample from `data`.
    data_origin: usize,
    /// Pointer to the start of the underlying allocation.
    data: *const u8,
    /// Bytes per sample.
    bytes_per_sample: usize,
    readable_len: usize,
    _storage: PhantomData<&'a [u8]>,
}

impl<'a> PlaneSource<'a> {
    /// Borrow a plane from initialized bytes. Stride and origin are in BYTES.
    /// Two-byte samples use native endian storage (unaligned origins are supported).
    ///
    /// # Errors
    /// Rejects zero geometry, sample sizes other than 1/2, short strides/spans,
    /// overflowing/out-of-bounds signed byte offsets, and dimensions beyond c_uint.
    #[inline]
    pub fn from_bytes(
        data: &'a [u8],
        width: usize,
        height: usize,
        stride: isize,
        data_origin: usize,
        bytes_per_sample: usize,
    ) -> Result<Self, FmetricsError> {
        // SAFETY: the slice proves allocation length, initialization and shared lifetime.
        unsafe {
            Self::from_raw_parts(
                data.as_ptr(),
                data.len(),
                width,
                height,
                stride,
                data_origin,
                bytes_per_sample,
            )
        }
    }

    /// Borrow native-endian u16 storage; stride and origin still use BYTES.
    ///
    /// # Errors
    /// Returns the same layout errors as [`Self::from_bytes`].
    #[inline]
    pub fn from_u16(
        data: &'a [u16],
        width: usize,
        height: usize,
        stride: isize,
        data_origin: usize,
    ) -> Result<Self, FmetricsError> {
        // SAFETY: u16 has no padding/uninitialized bytes; the slice supplies its actual span.
        unsafe {
            Self::from_raw_parts(
                data.as_ptr().cast(),
                std::mem::size_of_val(data),
                width,
                height,
                stride,
                data_origin,
                2,
            )
        }
    }

    /// Describe borrowed foreign storage with an explicitly declared byte span.
    /// Stride and origin use BYTES. No reference to row padding is created.
    ///
    /// # Errors
    /// Validates the layout and span as in [`Self::from_bytes`], also rejecting
    /// null pointers and spans larger than `isize::MAX`, without dereferencing.
    ///
    /// # Safety
    ///
    /// If layout validation succeeds, `data` must identify one live allocation
    /// covering at least `readable_len` bytes. Every visible sample must be
    /// initialized and readable, with no mutation for all of `'a`. Padding need
    /// not be initialized and is never read. The caller must bind `'a` to the
    /// allocation's actual owner; validation cannot prove allocation existence.
    #[inline]
    pub unsafe fn from_raw_parts(
        data: *const u8,
        readable_len: usize,
        width: usize,
        height: usize,
        stride: isize,
        data_origin: usize,
        bytes_per_sample: usize,
    ) -> Result<Self, FmetricsError> {
        let plane = Self {
            width,
            height,
            stride,
            data_origin,
            data,
            bytes_per_sample,
            readable_len,
            _storage: PhantomData,
        };
        validate_plane_layout(&plane)?;
        Ok(plane)
    }

    /// Width in samples.
    #[inline]
    #[must_use]
    pub const fn width(&self) -> usize {
        self.width
    }
    /// Height in rows.
    #[inline]
    #[must_use]
    pub const fn height(&self) -> usize {
        self.height
    }
    /// Signed row spacing in bytes.
    #[inline]
    #[must_use]
    pub const fn stride(&self) -> isize {
        self.stride
    }
    /// First visible sample's byte offset.
    #[inline]
    #[must_use]
    pub const fn data_origin(&self) -> usize {
        self.data_origin
    }
    /// Size of one sample in bytes.
    #[inline]
    #[must_use]
    pub const fn bytes_per_sample(&self) -> usize {
        self.bytes_per_sample
    }
    /// Start of the backing span. Dereferencing this pointer requires unsafe code.
    #[inline]
    #[must_use]
    pub const fn data_ptr(&self) -> *const u8 {
        self.data
    }
    /// Declared backing span in bytes; padding may be uninitialized.
    #[inline]
    #[must_use]
    pub const fn readable_len(&self) -> usize {
        self.readable_len
    }
}

/// The three planes of a planar YUV frame, in Y, U, V order.
#[derive(Debug, Clone, Copy)]
pub struct PlaneSet<'a> {
    /// The three planes, Y first.
    planes: [PlaneSource<'a>; 3],
}

impl<'a> PlaneSet<'a> {
    /// Group three checked, borrowed planes in Y, U, V order.
    #[inline]
    #[must_use]
    pub const fn new(planes: [PlaneSource<'a>; 3]) -> Self {
        Self { planes }
    }
    /// Describe the planes of a decoded frame.
    ///
    /// The stride and origin conversion lives here rather than in the caller,
    /// so it happens once.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid byte geometry or a frame with no chroma
    /// planes. fmetrics measures colour, so monochrome is rejected.
    #[inline]
    pub fn from_frame<T>(frame: &'a v_frame::frame::Frame<T>) -> Result<Self, FmetricsError>
    where
        T: v_frame::pixel::Pixel,
    {
        let (Some(u_plane), Some(v_plane)) = (&frame.u_plane, &frame.v_plane) else {
            return Err(FmetricsError::UnsupportedFormat {
                reason: "frame has no chroma planes; fmetrics measures colour and requires 4:2:0, \
                         4:2:2 or 4:4:4 content"
                    .to_owned(),
            });
        };

        let bytes_per_sample = std::mem::size_of::<T>();

        Ok(Self {
            planes: [
                Self::describe(&frame.y_plane, bytes_per_sample)?,
                Self::describe(u_plane, bytes_per_sample)?,
                Self::describe(v_plane, bytes_per_sample)?,
            ],
        })
    }

    /// Describe one plane, scaling `v_frame`'s sample-based geometry to bytes.
    #[inline]
    fn describe<T: v_frame::pixel::Pixel>(
        plane: &'a v_frame::plane::Plane<T>,
        bytes_per_sample: usize,
    ) -> Result<PlaneSource<'a>, FmetricsError> {
        let geometry = plane.geometry();
        let stride = geometry
            .stride()
            .checked_mul(bytes_per_sample)
            .and_then(|n| isize::try_from(n).ok())
            .ok_or_else(|| malformed("frame stride overflow"))?;
        let origin = geometry
            .data_origin()
            .checked_mul(bytes_per_sample)
            .ok_or_else(|| malformed("frame origin overflow"))?;
        // SAFETY: v_frame 0.7's Pixel is sealed to u8/u16, both fully initialized
        // without padding. Plane::data exposes the actual allocation, borrowed for 'a.
        unsafe {
            PlaneSource::from_raw_parts(
                plane.data().as_ptr().cast(),
                std::mem::size_of_val(plane.data()),
                geometry.width(),
                geometry.height(),
                stride,
                origin,
                bytes_per_sample,
            )
        }
    }

    /// The planes in Y, U, V order.
    #[inline]
    #[must_use]
    pub const fn as_array(&self) -> &[PlaneSource<'a>; 3] {
        &self.planes
    }
}

impl<'a> std::ops::Deref for PlaneSet<'a> {
    type Target = [PlaneSource<'a>; 3];

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.planes
    }
}

/// A packed, interleaved RGB image owned by this crate.
///
/// The buffer outlives the [`FmetricsImg`] referencing it, which is why an
/// image is borrowed rather than passed as a bare pointer.
#[derive(Debug)]
pub struct RgbImage {
    /// Interleaved samples, `u8` or `u16` according to [`RgbImage::is_wide`].
    data: RgbStorage,
    width: usize,
    height: usize,
}

// Native RGB_UINT16 readers dereference uint16_t*, so allocation alignment must
// follow the sample type, not incidental allocator behavior for Vec<u8>.
#[derive(Debug)]
enum RgbStorage {
    Narrow(Vec<u8>),
    Wide(Vec<u16>),
}

impl RgbImage {
    /// Bytes per row, which is the stride fmetrics is given.
    #[inline]
    #[must_use]
    pub const fn stride(&self) -> usize {
        if self.is_wide() {
            self.width * 6
        } else {
            self.width * 3
        }
    }

    /// Width in pixels.
    #[inline]
    #[must_use]
    pub const fn width(&self) -> usize {
        self.width
    }

    /// Height in pixels.
    #[inline]
    #[must_use]
    pub const fn height(&self) -> usize {
        self.height
    }

    /// Whether the samples are 16-bit.
    #[inline]
    #[must_use]
    pub const fn is_wide(&self) -> bool {
        matches!(self.data, RgbStorage::Wide(_))
    }

    /// The interleaved samples, as bytes.
    ///
    /// 16-bit samples use native byte order, matching the C uint16_t ABI;
    /// these bytes are not an endian-independent serialization.
    #[inline]
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        match &self.data {
            RgbStorage::Narrow(data) => data,
            RgbStorage::Wide(data) => {
                // SAFETY: initialized u16 samples have no padding; the byte view
                // covers exactly those samples and shares this image's borrow.
                unsafe {
                    std::slice::from_raw_parts(
                        data.as_ptr().cast(),
                        std::mem::size_of_val(data.as_slice()),
                    )
                }
            },
        }
    }

    /// Read one pixel as `(r, g, b)`.
    ///
    /// # Panics
    ///
    /// Panics if `(x, y)` is outside the image.
    #[inline]
    #[must_use]
    pub fn pixel(&self, x: usize, y: usize) -> (u32, u32, u32) {
        assert!(
            x < self.width && y < self.height,
            "{x},{y} is outside {}x{}",
            self.width,
            self.height
        );

        let offset = (y * self.width + x) * 3;
        match &self.data {
            RgbStorage::Wide(data) => (
                u32::from(data[offset]),
                u32::from(data[offset + 1]),
                u32::from(data[offset + 2]),
            ),
            RgbStorage::Narrow(data) => (
                u32::from(data[offset]),
                u32::from(data[offset + 1]),
                u32::from(data[offset + 2]),
            ),
        }
    }

    /// A borrowed [`FmetricsImg`] describing this image.
    #[inline]
    #[must_use]
    pub fn as_img(&self, colorspace: FmetricsColorspace, hdr: bool) -> BorrowedImg<'_> {
        rgb_img(
            self.as_bytes(),
            self.width,
            self.height,
            self.stride(),
            self.is_wide(),
            colorspace,
            hdr,
        )
    }
}

/// Convert a planar YUV frame to interleaved RGB.
///
/// `max_sample` is the source's peak: 255 for 8-bit, 1023 for 10-bit, 4095 for
/// 12-bit. It must agree with `info.bit_depth`; yuvxyb derives normalization
/// from depth and range (limited range uses nominal shifted endpoints).
/// yuvxyb clips normalized Y to `[0,1]` and C to `[-0.5,0.5]` before the matrix.
/// Video-family SDR is converted through gamma-2.4 linear light to sRGB; existing
/// sRGB/linear data is not transfer-converted. Odd subsampled edges are replicated
/// for conversion and cropped back, never resized.
///
/// # Errors
///
/// Returns `MalformedFrame` for inconsistent storage widths/chroma dimensions
/// or overflowing output geometry, and `UnsupportedFormat` for unsupported
/// depth/peak/color metadata or a rejected yuvxyb conversion. Inputs borrow live
/// checked storage; the returned RGB owns its bytes independently.
///
#[inline]
pub fn yuv_to_rgb(
    planes: &PlaneSet<'_>,
    info: &ColorInfo,
    max_sample: u32,
) -> Result<RgbImage, FmetricsError> {
    let expected_peak = info.max_sample()?;
    if max_sample != expected_peak {
        return Err(FmetricsError::UnsupportedFormat {
            reason: format!(
                "{}-bit input requires peak {expected_peak}, got {max_sample}",
                info.bit_depth
            ),
        });
    }
    let expected_bytes = if info.needs_wide_samples() { 2 } else { 1 };
    for (index, plane) in planes.iter().enumerate() {
        if plane.bytes_per_sample != expected_bytes {
            return Err(FmetricsError::MalformedFrame {
                reason: format!(
                    "plane {index} has {}-byte samples; {}-bit input requires {expected_bytes}",
                    plane.bytes_per_sample, info.bit_depth
                ),
            });
        }
    }

    let luma = &planes[0];
    let width = luma.width;
    let height = luma.height;

    if width == 0 || height == 0 {
        return Err(FmetricsError::MalformedFrame {
            reason: format!("frame is {width}x{height}"),
        });
    }

    let u = &planes[1];
    let v = &planes[2];
    if (u.width, u.height) != (v.width, v.height) {
        return Err(FmetricsError::MalformedFrame {
            reason: format!(
                "chroma planes disagree: U is {}x{}, V is {}x{}",
                u.width, u.height, v.width, v.height
            ),
        });
    }
    // Prefer 4:4:4 when one-pixel axes make layouts indistinguishable;
    // every matching layout addresses the same valid cells on those axes.
    let subsampling = [[1, 1], [2, 1], [2, 2]]
        .into_iter()
        .find(|&[sx, sy]| u.width == width.div_ceil(sx) && u.height == height.div_ceil(sy))
        .ok_or_else(|| FmetricsError::MalformedFrame {
            reason: format!(
                "chroma is {}x{} for {width}x{height} luma; expected 4:2:0, 4:2:2 or 4:4:4 ceil dimensions",
                u.width, u.height
            ),
        })?;

    let transfer = source_transfer(info.transfer)?;
    let expected_colorspace = if transfer == yuvxyb::TransferCharacteristic::Linear {
        FmetricsColorspace::LinearSrgb
    } else {
        FmetricsColorspace::Srgb
    };
    if info.colorspace != expected_colorspace {
        return Err(FmetricsError::UnsupportedFormat {
            reason: format!(
                "source transfer {:?} requires output {expected_colorspace:?}, got {:?}",
                info.transfer, info.colorspace
            ),
        });
    }

    let padded_width = width
        .checked_add(width % subsampling[0])
        .ok_or_else(|| malformed("padded width overflow"))?;
    let padded_height = height
        .checked_add(height % subsampling[1])
        .ok_or_else(|| malformed("padded height overflow"))?;
    // Bound the largest intermediate (float RGB), before frame allocation or reads.
    padded_width
        .checked_mul(padded_height)
        .and_then(|n| n.checked_mul(12))
        .filter(|&n| isize::try_from(n).is_ok())
        .ok_or_else(|| malformed("RGB allocation dimensions overflow"))?;
    let stride = width
        .checked_mul(if info.needs_wide_samples() { 6 } else { 3 })
        .filter(|&n| u32::try_from(n).is_ok())
        .ok_or_else(|| malformed("RGB stride exceeds native representation"))?;
    if u32::try_from(height).is_err() {
        return Err(malformed("RGB height exceeds native representation"));
    }
    for plane in &planes.planes {
        validate_plane_layout(plane)?;
    }
    let config = yuvxyb::YuvConfig {
        bit_depth: info.bit_depth as u8,
        subsampling_x: (subsampling[0] - 1) as u8,
        subsampling_y: (subsampling[1] - 1) as u8,
        full_range: info.range == SampleRange::Full,
        matrix_coefficients: match info.matrix {
            YuvMatrix::Bt601 => yuvxyb::MatrixCoefficients::ST170M,
            YuvMatrix::Bt709 => yuvxyb::MatrixCoefficients::BT709,
            YuvMatrix::Bt2020 => yuvxyb::MatrixCoefficients::BT2020NonConstantLuminance,
        },
        transfer_characteristics: transfer,
        // Resolve explicitly; do not let the library guess a gamut from the matrix.
        color_primaries: yuvxyb::ColorPrimaries::BT709,
    };
    // SAFETY: immutable borrowed descriptors enforce spans, layout and lifetime;
    // configured sample storage width is checked above before the typed reads.
    let rgb = unsafe {
        if info.needs_wide_samples() {
            convert_owned::<u16>(planes, config, padded_width, padded_height)?
        } else {
            convert_owned::<u8>(planes, config, padded_width, padded_height)?
        }
    };
    let rgb = if matches!(
        transfer,
        yuvxyb::TransferCharacteristic::SRGB | yuvxyb::TransferCharacteristic::Linear
    ) {
        rgb
    } else {
        let linear = yuvxyb::LinearRgb::try_from(rgb).map_err(conversion_error)?;
        yuvxyb::Rgb::try_from((
            linear,
            yuvxyb::TransferCharacteristic::SRGB,
            yuvxyb::ColorPrimaries::BT709,
        ))
        .map_err(conversion_error)?
    };
    let mut data = if info.needs_wide_samples() {
        RgbStorage::Wide(Vec::with_capacity(stride / 2 * height))
    } else {
        RgbStorage::Narrow(Vec::with_capacity(stride * height))
    };
    for row in rgb.into_data().chunks_exact(padded_width).take(height) {
        for pixel in &row[..width] {
            for &value in pixel {
                match &mut data {
                    RgbStorage::Wide(data) => {
                        data.push((value * 65535.0).round().clamp(0.0, 65535.0) as u16);
                    },
                    RgbStorage::Narrow(data) => {
                        data.push((value * 255.0).round().clamp(0.0, 255.0) as u8);
                    },
                }
            }
        }
    }
    Ok(RgbImage {
        data,
        width,
        height,
    })
}

fn malformed(reason: impl Into<String>) -> FmetricsError {
    FmetricsError::MalformedFrame {
        reason: reason.into(),
    }
}

fn conversion_error(error: yuvxyb::ConversionError) -> FmetricsError {
    FmetricsError::UnsupportedFormat {
        reason: format!("yuvxyb conversion: {error}"),
    }
}

/// Check visible byte offsets against the actual borrowed/declared backing span.
fn validate_plane_layout(plane: &PlaneSource<'_>) -> Result<(), FmetricsError> {
    if plane.width == 0
        || plane.height == 0
        || !matches!(plane.bytes_per_sample, 1 | 2)
        || plane.readable_len > isize::MAX as usize
        || std::ffi::c_uint::try_from(plane.width).is_err()
        || std::ffi::c_uint::try_from(plane.height).is_err()
    {
        return Err(malformed("invalid plane geometry, sample width or span"));
    }
    let row_bytes = plane
        .width
        .checked_mul(plane.bytes_per_sample)
        .and_then(|n| isize::try_from(n).ok())
        .ok_or_else(|| malformed("plane row size overflow"))?;
    let origin =
        isize::try_from(plane.data_origin).map_err(|_| malformed("plane origin overflow"))?;
    let last = isize::try_from(plane.height - 1)
        .ok()
        .and_then(|n| n.checked_mul(plane.stride))
        .and_then(|n| origin.checked_add(n))
        .filter(|&n| n >= 0)
        .ok_or_else(|| malformed("plane row offset overflow"))?;
    if plane.data.is_null()
        || plane.stride.checked_abs().is_none_or(|stride| stride < row_bytes)
        || origin
            .max(last)
            .checked_add(row_bytes)
            .is_none_or(|end| end as usize > plane.readable_len)
    {
        return Err(malformed("invalid plane pointer, stride or byte extent"));
    }
    Ok(())
}

/// Copy visible source codes into owned storage; only valid odd luma edges pad.
///
/// # Safety
/// Source layouts must be validated and all described samples must remain readable.
unsafe fn convert_owned<T: v_frame::pixel::Pixel>(
    planes: &PlaneSet<'_>,
    config: yuvxyb::YuvConfig,
    width: usize,
    height: usize,
) -> Result<yuvxyb::Rgb, FmetricsError> {
    let sampling = match (config.subsampling_x, config.subsampling_y) {
        (0, 0) => v_frame::chroma::ChromaSubsampling::Yuv444,
        (1, 0) => v_frame::chroma::ChromaSubsampling::Yuv422,
        (1, 1) => v_frame::chroma::ChromaSubsampling::Yuv420,
        _ => unreachable!("subsampling was validated"),
    };
    let mut frame = v_frame::frame::FrameBuilder::new(width, height, sampling, config.bit_depth)
        .build::<T>()
        .map_err(|error| malformed(format!("owned YUV frame: {error}")))?;
    for (index, source) in planes.iter().enumerate() {
        let target = frame.plane_mut(index).expect("validated three-plane frame");
        for (y, row) in target.rows_mut().enumerate() {
            let source_y = y.min(source.height - 1);
            let offset = (source.data_origin as isize + source_y as isize * source.stride) as usize;
            for (x, sample) in row.iter_mut().enumerate() {
                // SAFETY: checked row/column offsets address visible samples;
                // read_unaligned supports byte origins and preserves meaningful depth.
                *sample = unsafe {
                    source
                        .data
                        .add(offset + x.min(source.width - 1) * size_of::<T>())
                        .cast::<T>()
                        .read_unaligned()
                };
            }
        }
    }
    let yuv = yuvxyb::Yuv::new(frame, config)
        .map_err(|error| malformed(format!("yuvxyb input: {error}")))?;
    yuvxyb::Rgb::try_from(&yuv).map_err(conversion_error)
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "a failing assertion should panic loudly, which is what a unit test wants"
)]
mod tests {
    use super::*;

    #[repr(align(2))]
    struct AlignedRgbBytes([u8; 24]);

    #[test]
    fn wide_rgb_view_rejects_misaligned_storage() {
        let storage = AlignedRgbBytes([0; 24]);
        let odd = &storage.0[1..13];
        assert!(!odd.as_ptr().cast::<u16>().is_aligned());
        assert!(matches!(
            RgbView::new(odd, 2, 1, true),
            Err(FmetricsError::MalformedFrame { .. })
        ));
        // Byte RGB has no u16 alignment requirement.
        assert!(RgbView::new(odd, 2, 2, false).is_ok());
        let aligned = RgbView::new(&storage.0[..12], 2, 1, true).unwrap();
        assert!(
            aligned
                .as_img(FmetricsColorspace::Srgb, false)
                .as_raw()
                .data
                .cast::<u16>()
                .is_aligned()
        );
    }

    #[test]
    fn owned_wide_rgb_alignment_and_native_byte_roundtrip() {
        for (depth, midpoint, gray) in [(10, 512u16, 32800u16), (12, 2048, 32776)] {
            let image =
                convert_integer_pixel([midpoint; 3], depth, YuvMatrix::Bt709, SampleRange::Full);
            let descriptor = image.as_img(FmetricsColorspace::Srgb, false);
            let raw = descriptor.as_raw();
            assert!(raw.data.cast::<u16>().is_aligned());
            assert_eq!(raw.stride, 6);
            assert_eq!(raw.format, crate::ffi::FmetricsPixFmt::RgbUint16);
            assert_eq!(
                image.pixel(0, 0),
                (u32::from(gray), u32::from(gray), u32::from(gray))
            );
            let expected: Vec<_> = [gray; 3].into_iter().flat_map(u16::to_ne_bytes).collect();
            assert_eq!(image.as_bytes(), expected);
            assert_eq!(raw.data.cast::<u8>(), image.as_bytes().as_ptr());
            let view = RgbView::new(image.as_bytes(), 1, 1, true).unwrap();
            assert_eq!(view.as_bytes(), expected);
        }
    }

    #[test]
    fn rgb_invalid_geometry_returns_errors_not_panics() {
        for (width, height) in [(usize::MAX, 2), (2, usize::MAX), (2, 2)] {
            assert!(
                matches!(
                    std::panic::catch_unwind(|| RgbView::new(&[], width, height, false)),
                    Ok(Err(FmetricsError::MalformedFrame { .. }))
                ),
                "invalid RGB geometry must return an error, not panic"
            );
        }
    }

    #[test]
    fn plane_layout_rejects_invalid_readable_spans() {
        let bytes = [0; 16];
        for (len, width, height, stride, origin, sample) in [
            (16, 0, 2, 2, 0, 1),
            (16, 2, 0, 2, 0, 1),
            (16, 2, 2, 1, 0, 1),
            (3, 2, 2, 2, 0, 1),
            (5, 2, 2, -4, 4, 1),
            (11, 2, 2, -8, 8, 2),
            (16, 2, 2, -4, 0, 1),
            (16, 2, 2, 4, 15, 1),
            (16, 1, 1, isize::MIN, 0, 1),
            (16, 1, 3, isize::MAX, 0, 1),
            (16, usize::MAX, 1, 1, 0, 2),
            (16, 1, usize::MAX, 1, 0, 1),
            (16, 1, 1, 1, usize::MAX, 1),
            (16, 1, 1, 1, isize::MAX as usize, 1),
            (16, 1, 1, 1, 0, 0),
            (16, 1, 1, 4, 0, 4),
        ] {
            assert!(
                matches!(
                    PlaneSource::from_bytes(&bytes[..len], width, height, stride, origin, sample),
                    Err(FmetricsError::MalformedFrame { .. })
                ),
                "{len}/{width}/{height}/{stride}/{origin}/{sample}"
            );
        }
        assert!(
            // SAFETY: null is an invalid layout, rejected without accessing storage.
            unsafe { PlaneSource::from_raw_parts(std::ptr::null(), 16, 2, 2, 2, 0, 1) }.is_err()
        );
        assert!(
            // SAFETY: an unrepresentable span is rejected without accessing storage.
            unsafe { PlaneSource::from_raw_parts(bytes.as_ptr(), usize::MAX, 1, 1, 1, 0, 1) }
                .is_err()
        );
    }

    #[test]
    fn padded_bottom_up_matches_top_down_conversion() {
        for (depth, bytes, bottom, top) in [
            (
                8,
                1,
                vec![128, 255, 93, 94, 0, 64, 95, 96],
                vec![0, 64, 128, 255],
            ),
            (
                10,
                2,
                [512u16, 1023, 93, 94, 0, 256, 95, 96]
                    .into_iter()
                    .flat_map(u16::to_ne_bytes)
                    .collect(),
                [0u16, 256, 512, 1023].into_iter().flat_map(u16::to_ne_bytes).collect(),
            ),
        ] {
            let neutral: Vec<_> = if bytes == 1 {
                vec![128; 4]
            } else {
                [512u16; 4].into_iter().flat_map(u16::to_ne_bytes).collect()
            };
            let c =
                PlaneSource::from_bytes(&neutral, 2, 2, (2 * bytes) as isize, 0, bytes).unwrap();
            let bottom =
                PlaneSource::from_bytes(&bottom, 2, 2, -((4 * bytes) as isize), 4 * bytes, bytes)
                    .unwrap();
            let top = PlaneSource::from_bytes(&top, 2, 2, (2 * bytes) as isize, 0, bytes).unwrap();
            let info = ColorInfo {
                bit_depth: depth,
                ..full_range(YuvMatrix::Bt709)
            };
            let a = yuv_to_rgb(
                &PlaneSet::new([bottom, c, c]),
                &info,
                info.max_sample().unwrap(),
            )
            .unwrap();
            let b = yuv_to_rgb(
                &PlaneSet::new([top, c, c]),
                &info,
                info.max_sample().unwrap(),
            )
            .unwrap();
            assert_eq!(a.as_bytes(), b.as_bytes());
        }
        let words = [512u16; 8];
        assert!(PlaneSource::from_u16(&words, 2, 2, -8, 8).is_ok());
    }

    #[test]
    fn rgb_checked_native_geometry_and_depth() {
        for (width, height, wide) in [
            (0, 2, false),
            (2, 0, true),
            (2, 2, false),
            (usize::MAX, 1, true),
            (1, usize::MAX, false),
            (u32::MAX as usize, 1, true),
        ] {
            assert!(matches!(
                RgbView::new(&[], width, height, wide),
                Err(FmetricsError::MalformedFrame { .. })
            ));
        }
        let aligned = AlignedRgbBytes([0; 24]);
        assert!(RgbView::new(&aligned.0, 2, 2, true).is_ok());
        // A short slice must not hide native narrowing behind the buffer check.
        if usize::BITS > 32 {
            assert!(matches!(RgbView::new(&[], u32::MAX as usize, 1, false),
                Err(FmetricsError::MalformedFrame { reason }) if reason.contains("native")));
        }
        let plane = PlaneSource::from_bytes(&[0; 4], 2, 2, 2, 0, 1).unwrap();
        let info = ColorInfo {
            bit_depth: 10,
            ..full_range(YuvMatrix::Bt709)
        };
        assert!(matches!(
            yuv_to_rgb(&PlaneSet::new([plane; 3]), &info, 1023),
            Err(FmetricsError::MalformedFrame { .. })
        ));
    }

    #[test]
    #[expect(
        clippy::suboptimal_flops,
        reason = "keep the independent standard gamma/sRGB reference readable"
    )]
    fn tagged_sdr_high_depth_gray_matches_independent_gamma() {
        for (depth, samples) in [
            (10, [0u16, 16, 64, 92, 256, 512, 1023]),
            (12, [0, 64, 256, 368, 1024, 2048, 4095]),
        ] {
            let neutral = [1u16 << (depth - 1); 7];
            let y = PlaneSource::from_u16(&samples, 7, 1, 14, 0).unwrap();
            let c = PlaneSource::from_u16(&neutral, 7, 1, 14, 0).unwrap();
            let info = ColorInfo {
                bit_depth: depth,
                transfer: av_decoders::TransferCharacteristic::BT709,
                ..full_range(YuvMatrix::Bt709)
            };
            let image =
                yuv_to_rgb(&PlaneSet::new([y, c, c]), &info, info.max_sample().unwrap()).unwrap();
            for (x, code) in samples.into_iter().enumerate() {
                let linear = (f64::from(code) / f64::from((1u16 << depth) - 1)).powf(2.4);
                let srgb = if linear <= 0.0031308 {
                    12.92 * linear
                } else {
                    1.055 * linear.powf(1.0 / 2.4) - 0.055
                };
                let expected = (srgb * 65535.0).round() as u32;
                let (r, g, b) = image.pixel(x, 0);
                // yuvxyb 0.6 uses alpha=1.0550107, beta=.0030412825 rather than
                // standard 1.055/.0031308. Include the knee (92/368); allow four
                // u16 codes (<.007% full scale) for those constants + fastmath/FMA.
                for actual in [r, g, b] {
                    assert!(
                        actual.abs_diff(expected) <= 4,
                        "{depth}-bit code {code}: {actual} != {expected}"
                    );
                }
            }
        }
    }

    #[test]
    fn sdr_video_transfers_convert_gray_to_srgb() {
        use av_decoders::TransferCharacteristic as Transfer;
        // Independent: L=(Y/255)^2.4, piecewise sRGB encoding, round once.
        for transfer in
            [Transfer::BT709, Transfer::SMPTE170M, Transfer::BT2020_10, Transfer::BT2020_12]
        {
            let details = av_decoders::VideoDetails {
                width: 5,
                height: 1,
                bit_depth: 8,
                chroma_sampling: v_frame::chroma::ChromaSubsampling::Yuv444,
                frame_rate: av_decoders::Rational32::new(24, 1),
                total_frames: None,
                color_range: Some(av_decoders::ColorRange::Full),
                color_space: Some(av_decoders::ColorSpace::BT709),
                color_primaries: Some(av_decoders::ColorPrimaries::BT709),
                color_transfer_characteristic: Some(transfer),
                chroma_location: None,
                mastering_display: None,
                content_light_level: None,
            };
            let info = ColorInfo::from_details(&details).expect("ordinary tagged SDR is supported");
            let frame = frame444(5, 1, vec![0, 16, 64, 128, 255], vec![128; 5], vec![128; 5]);
            let image = convert_frame(&frame, &info, 255).unwrap();
            let actual: Vec<_> = (0..5).map(|x| image.pixel(x, 0)).collect();
            assert_eq!(
                actual,
                [0, 4, 53, 121, 255].map(|v| (v, v, v)),
                "{transfer:?}"
            );
            assert_eq!(info.colorspace, FmetricsColorspace::Srgb);
        }
    }

    #[test]
    fn sdr_srgb_and_linear_preserve_encoded_values() {
        for (transfer, colorspace) in [
            (
                av_decoders::TransferCharacteristic::IEC61966_2_1,
                FmetricsColorspace::Srgb,
            ),
            (
                av_decoders::TransferCharacteristic::Linear,
                FmetricsColorspace::LinearSrgb,
            ),
        ] {
            let info = ColorInfo {
                transfer,
                colorspace,
                ..full_range(YuvMatrix::Bt709)
            };
            let frame = frame444(5, 1, vec![0, 16, 64, 128, 255], vec![128; 5], vec![128; 5]);
            let image = convert_frame(&frame, &info, 255).unwrap();
            for (x, gray) in [0, 16, 64, 128, 255].into_iter().enumerate() {
                assert_eq!(image.pixel(x, 0), (gray, gray, gray));
            }
        }
    }

    #[test]
    fn sdr_clips_nominal_excursions_before_matrix() {
        // Clamp Y to [0,1], C to [-.5,.5], then independent BT.709 inverse luma.
        assert_eq!(
            convert_u8_pixel([0, 0, 128], YuvMatrix::Bt709, SampleRange::Limited),
            (0, 24, 0)
        );
        assert_eq!(
            convert_u8_pixel([255, 128, 255], YuvMatrix::Bt709, SampleRange::Limited),
            (255, 195, 255)
        );
    }

    #[test]
    fn sdr_invalid_stored_codes_are_rejected() {
        for depth in [10, 12] {
            for invalid_plane in 0..3 {
                let mut samples = [1u16 << (depth - 1); 3];
                samples[invalid_plane] = 1 << depth;
                let info = ColorInfo {
                    bit_depth: depth,
                    ..full_range(YuvMatrix::Bt709)
                };
                let planes = PlaneSet::new(samples.each_ref().map(|sample| {
                    PlaneSource::from_u16(std::slice::from_ref(sample), 1, 1, 2, 0).unwrap()
                }));
                let result = yuv_to_rgb(&planes, &info, info.max_sample().unwrap());
                assert!(matches!(result, Err(FmetricsError::MalformedFrame { .. })));
            }
        }
    }

    #[test]
    fn sdr_even_and_odd_frames_preserve_strides_origins_and_crop() {
        for depth in [8, 10, 12] {
            let bytes = if depth == 8 { 1 } else { 2 };
            let peak = (1u16 << depth) - 1;
            let midpoint = 1u16 << (depth - 1);
            let codes = [0, 1 << (depth - 2), midpoint, peak];
            for (width, height) in [(5usize, 3usize), (6, 4)] {
                for (sx, sy) in [(1usize, 1usize), (2, 1), (2, 2)] {
                    let geometry = [
                        (width, height),
                        (width.div_ceil(sx), height.div_ceil(sy)),
                        (width.div_ceil(sx), height.div_ceil(sy)),
                    ];
                    let storage: [Vec<u8>; 3] = std::array::from_fn(|index| {
                        let (w, h) = geometry[index];
                        let stride = w * bytes + 3;
                        let mut data = vec![0; 1 + stride * h];
                        for y in 0..h {
                            for x in 0..w {
                                let code = if index == 0 {
                                    codes[(y * w + x) % 4]
                                } else {
                                    midpoint
                                };
                                let offset = 1 + (h - 1 - y) * stride + x * bytes;
                                if bytes == 1 {
                                    data[offset] = code as u8;
                                } else {
                                    data[offset..offset + 2].copy_from_slice(&code.to_ne_bytes());
                                }
                            }
                        }
                        data
                    });
                    let planes = PlaneSet {
                        planes: std::array::from_fn(|index| {
                            let (w, h) = geometry[index];
                            let stride = w * bytes + 3;
                            PlaneSource::from_bytes(
                                &storage[index],
                                w,
                                h,
                                -(stride as isize),
                                1 + (h - 1) * stride,
                                bytes,
                            )
                            .unwrap()
                        }),
                    };
                    let info = ColorInfo {
                        bit_depth: depth,
                        ..full_range(YuvMatrix::Bt709)
                    };
                    let image = yuv_to_rgb(&planes, &info, u32::from(peak)).unwrap();
                    assert_eq!((image.width(), image.height()), (width, height));
                    let expected = if depth == 8 {
                        [0, 64, 128, 255]
                    } else if depth == 10 {
                        [0, 16400, 32800, 65535]
                    } else {
                        [0, 16388, 32776, 65535]
                    };
                    for index in 0..width * height {
                        let gray = expected[index % 4];
                        assert_eq!(
                            image.pixel(index % width, index / width),
                            (gray, gray, gray)
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn sdr_invalid_descriptor_arithmetic_fails_before_reads() {
        for (width, stride, origin) in [(1, 1, usize::MAX), (1, isize::MIN, 0), (usize::MAX, 1, 0)]
        {
            let result = PlaneSource::from_bytes(&[128], width, 1, stride, origin, 1);
            assert!(matches!(result, Err(FmetricsError::MalformedFrame { .. })));
        }
    }

    /// A 4:4:4 frame.
    fn frame444(
        width: usize,
        height: usize,
        y: Vec<u8>,
        u: Vec<u8>,
        v: Vec<u8>,
    ) -> v_frame::frame::Frame<u8> {
        let mut frame = v_frame::frame::FrameBuilder::new(
            width,
            height,
            v_frame::chroma::ChromaSubsampling::Yuv444,
            8,
        )
        .build::<u8>()
        .unwrap();
        for (index, samples) in [y, u, v].iter().enumerate() {
            for (row, data) in
                frame.plane_mut(index).unwrap().rows_mut().zip(samples.chunks_exact(width))
            {
                row.copy_from_slice(data);
            }
        }
        frame
    }

    fn convert_frame(
        frame: &v_frame::frame::Frame<u8>,
        info: &ColorInfo,
        peak: u32,
    ) -> Result<RgbImage, FmetricsError> {
        yuv_to_rgb(&PlaneSet::from_frame(frame)?, info, peak)
    }

    fn full_range(matrix: YuvMatrix) -> ColorInfo {
        ColorInfo {
            matrix,
            range: SampleRange::Full,
            bit_depth: 8,
            hdr: false,
            colorspace: FmetricsColorspace::Srgb,
            transfer: av_decoders::TransferCharacteristic::IEC61966_2_1,
        }
    }

    /// Read one pixel out of a converted image.
    fn pixel(image: &RgbImage, x: usize, y: usize) -> (u32, u32, u32) {
        let channels = if image.is_wide() { 6 } else { 3 };
        let offset = y * image.stride() + x * channels;
        if image.is_wide() {
            let mut out = [0u32; 3];
            for (index, channel) in out.iter_mut().enumerate() {
                let start = offset + index * 2;
                *channel =
                    u16::from_ne_bytes([image.as_bytes()[start], image.as_bytes()[start + 1]])
                        as u32;
            }
            (out[0], out[1], out[2])
        } else {
            (
                image.as_bytes()[offset] as u32,
                image.as_bytes()[offset + 1] as u32,
                image.as_bytes()[offset + 2] as u32,
            )
        }
    }

    fn convert_u8_pixel(
        samples: [u8; 3],
        matrix: YuvMatrix,
        range: SampleRange,
    ) -> (u32, u32, u32) {
        let planes = PlaneSet::new(samples.each_ref().map(|sample| {
            PlaneSource::from_bytes(std::slice::from_ref(sample), 1, 1, 1, 0, 1).unwrap()
        }));
        let info = ColorInfo {
            matrix,
            range,
            bit_depth: 8,
            hdr: false,
            colorspace: FmetricsColorspace::Srgb,
            transfer: av_decoders::TransferCharacteristic::IEC61966_2_1,
        };
        let image = yuv_to_rgb(&planes, &info, 255).unwrap();
        pixel(&image, 0, 0)
    }

    #[test]
    fn matrix_reference_vectors() {
        let samples = [[128, 144, 128], [128, 128, 144], [96, 112, 152], [160, 152, 112]];
        // Independent inverse-luma reference: R=Y+2(1-Kr)Cr, B=Y+2(1-Kb)Cb,
        // G=(Y-Kr*R-Kb*B)/(1-Kr-Kb), rounded once. Limited Y uses 255/219,
        // Cb/Cr use (sample-128)*255/224. All reference channels are unclipped.
        let expected = [
            (
                YuvMatrix::Bt601,
                SampleRange::Full,
                [(128, 122, 156), (150, 117, 128), (130, 84, 68), (138, 163, 203)],
            ),
            (
                YuvMatrix::Bt601,
                SampleRange::Limited,
                [(130, 124, 163), (156, 117, 130), (131, 80, 61), (142, 171, 216)],
            ),
            (
                YuvMatrix::Bt709,
                SampleRange::Full,
                [(128, 125, 158), (153, 121, 128), (134, 88, 66), (135, 163, 205)],
            ),
            (
                YuvMatrix::Bt709,
                SampleRange::Limited,
                [(130, 127, 164), (159, 122, 130), (136, 84, 59), (139, 171, 218)],
            ),
            (
                YuvMatrix::Bt2020,
                SampleRange::Full,
                [(128, 125, 158), (152, 119, 128), (131, 85, 66), (136, 165, 205)],
            ),
            (
                YuvMatrix::Bt2020,
                SampleRange::Limited,
                [(130, 127, 165), (157, 120, 130), (133, 81, 59), (141, 174, 219)],
            ),
        ];
        let actual = expected.map(|(matrix, range, _)| {
            (
                matrix,
                range,
                samples.map(|samples| convert_u8_pixel(samples, matrix, range)),
            )
        });
        assert_eq!(actual, expected, "matrix/range reference RGB values");
    }

    #[test]
    fn neutral_chroma_preserves_gray_and_endpoints() {
        for matrix in [YuvMatrix::Bt601, YuvMatrix::Bt709, YuvMatrix::Bt2020] {
            for (range, cases) in [
                (SampleRange::Full, [(0, 0), (128, 128), (255, 255)]),
                (SampleRange::Limited, [(16, 0), (128, 130), (235, 255)]),
            ] {
                for (y, gray) in cases {
                    assert_eq!(
                        convert_u8_pixel([y, 128, 128], matrix, range),
                        (gray, gray, gray),
                        "{matrix:?} {range:?} neutral Y={y}"
                    );
                }
            }
        }
    }

    fn convert_integer_pixel<T: v_frame::pixel::Pixel>(
        samples: [T; 3],
        bit_depth: usize,
        matrix: YuvMatrix,
        range: SampleRange,
    ) -> RgbImage {
        let bytes = std::mem::size_of::<T>();
        let planes = PlaneSet {
            planes: samples.each_ref().map(|sample| {
                // SAFETY: Pixel is sealed to initialized u8/u16; this sample is live.
                unsafe {
                    PlaneSource::from_raw_parts(
                        std::ptr::from_ref(sample).cast(),
                        bytes,
                        1,
                        1,
                        bytes as isize,
                        0,
                        bytes,
                    )
                    .unwrap()
                }
            }),
        };
        let info = ColorInfo {
            matrix,
            range,
            bit_depth,
            hdr: false,
            colorspace: FmetricsColorspace::Srgb,
            transfer: av_decoders::TransferCharacteristic::IEC61966_2_1,
        };
        yuv_to_rgb(&planes, &info, info.max_sample().unwrap()).unwrap()
    }

    #[test]
    fn full_range_neutral_uses_meaningful_midpoint_and_peak() {
        for (depth, midpoint, cases) in [
            (
                10,
                512u16,
                [(0, 0), (256, 16400), (512, 32800), (1023, 65535)],
            ),
            (
                12,
                2048,
                [(0, 0), (1024, 16388), (2048, 32776), (4095, 65535)],
            ),
        ] {
            for matrix in [YuvMatrix::Bt601, YuvMatrix::Bt709, YuvMatrix::Bt2020] {
                for (y, gray) in cases {
                    let image = convert_integer_pixel(
                        [y, midpoint, midpoint],
                        depth,
                        matrix,
                        SampleRange::Full,
                    );
                    assert!(image.is_wide(), "{depth}-bit input needs wide output");
                    assert_eq!(image.stride(), 6);
                    // Independent gray reference: round(Y * 65535 / source peak).
                    assert_eq!(
                        image.pixel(0, 0),
                        (gray, gray, gray),
                        "{depth}-bit {matrix:?} Y={y}"
                    );
                }
            }
        }
    }

    #[test]
    fn limited_range_nominal_endpoints_shift_exactly() {
        for (depth, midpoint, cases) in [
            (
                10,
                512u16,
                [(64, 0), (502, 32768), (512, 33516), (940, 65535)],
            ),
            (
                12,
                2048,
                [(256, 0), (2008, 32768), (2048, 33516), (3760, 65535)],
            ),
        ] {
            for matrix in [YuvMatrix::Bt601, YuvMatrix::Bt709, YuvMatrix::Bt2020] {
                for (y, gray) in cases {
                    let image = convert_integer_pixel(
                        [y, midpoint, midpoint],
                        depth,
                        matrix,
                        SampleRange::Limited,
                    );
                    // Nominal Y=16/235 and neutral C=128 are shifted, not peak-scaled.
                    let actual = image.pixel(0, 0);
                    for value in [actual.0, actual.1, actual.2] {
                        // Exactly half light is 32767.5 in u16; yuvxyb's f32
                        // matrix can land one ULP below that tie (BT.601 R/B).
                        let tolerance = u32::from(gray == 32768);
                        assert!(
                            value.abs_diff(gray) <= tolerance,
                            "{depth}-bit {matrix:?} Y={y}: {actual:?} != {gray}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    #[expect(
        clippy::suboptimal_flops,
        reason = "keep the independent inverse-luma reference equation directly readable"
    )]
    fn chroma_endpoints_match_independent_inverse_luma_reference() {
        for (depth, range, black, white, low, midpoint, high) in [
            (10, SampleRange::Full, 0u16, 1023, 0, 512, 1023),
            (12, SampleRange::Full, 0, 4095, 0, 2048, 4095),
            (10, SampleRange::Limited, 64, 940, 64, 512, 960),
            (12, SampleRange::Limited, 256, 3760, 256, 2048, 3840),
        ] {
            for (matrix, kr, kb) in [
                (YuvMatrix::Bt601, 0.299, 0.114),
                (YuvMatrix::Bt709, 0.2126, 0.0722),
                (YuvMatrix::Bt2020, 0.2627, 0.0593),
            ] {
                for (u, v) in [(low, midpoint), (high, midpoint), (midpoint, low), (midpoint, high)]
                {
                    let image = convert_integer_pixel([midpoint, u, v], depth, matrix, range);
                    // Reference works in unit RGB, reconstructing G from the luma identity
                    // rather than reusing the production matrix or normalization helpers.
                    let y = f64::from(midpoint - black) / f64::from(white - black);
                    // Approved clipping precedes the matrix, including full-range
                    // code zero, which lies just below -0.5 about the integer center.
                    let cb = ((f64::from(u) - f64::from(midpoint)) / f64::from(high - low))
                        .clamp(-0.5, 0.5);
                    let cr = ((f64::from(v) - f64::from(midpoint)) / f64::from(high - low))
                        .clamp(-0.5, 0.5);
                    let r = y + 2.0 * (1.0 - kr) * cr;
                    let b = y + 2.0 * (1.0 - kb) * cb;
                    let g = (y - kr * r - kb * b) / (1.0 - kr - kb);
                    let expected =
                        [r, g, b].map(|c| (c * 65535.0).round().clamp(0.0, 65535.0) as u32);
                    let actual = image.pixel(0, 0);
                    for (actual, expected) in
                        [actual.0, actual.1, actual.2].into_iter().zip(expected)
                    {
                        assert!(
                            actual.abs_diff(expected) <= 1,
                            "{depth}-bit {range:?} {matrix:?} U={u} V={v}: {actual} != {expected}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn equivalent_colors_agree_across_depths() {
        // Full-range Y follows the peak ratio; chroma differences follow that
        // ratio around the integer midpoint. These are rounded source codes.
        let full = [
            ([0u8, 128, 128], [0u16, 512, 512], [0u16, 2048, 2048]),
            ([128, 128, 128], [514, 512, 512], [2056, 2048, 2048]),
            ([255, 128, 128], [1023, 512, 512], [4095, 2048, 2048]),
            ([96, 112, 152], [385, 448, 608], [1542, 1791, 2433]),
            ([160, 152, 112], [642, 608, 448], [2569, 2433, 1791]),
        ];
        for range in [SampleRange::Full, SampleRange::Limited] {
            for matrix in [YuvMatrix::Bt601, YuvMatrix::Bt709, YuvMatrix::Bt2020] {
                for (narrow, ten, twelve) in full {
                    let baseline = convert_integer_pixel(narrow, 8, matrix, range).pixel(0, 0);
                    for (depth, samples) in [(10, ten), (12, twelve)] {
                        let samples = if range == SampleRange::Limited {
                            narrow.map(|s| u16::from(s) << (depth - 8))
                        } else {
                            samples
                        };
                        let actual =
                            convert_integer_pixel(samples, depth, matrix, range).pixel(0, 0);
                        for (actual, expected) in [actual.0, actual.1, actual.2]
                            .into_iter()
                            .zip([baseline.0, baseline.1, baseline.2])
                        {
                            assert!(
                                actual.abs_diff(expected * 257) <= 257,
                                "{depth}-bit {range:?} {matrix:?}: {actual} != {expected} * 257 (one 8-bit code tolerance)"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn adjacent_high_depth_codes_retain_sub_eight_bit_precision() {
        for (depth, midpoint, expected) in
            [(10, 512u16, [32800, 32864]), (12, 2048u16, [32776, 32792])]
        {
            let actual = [midpoint, midpoint + 1].map(|y| {
                convert_integer_pixel(
                    [y, midpoint, midpoint],
                    depth,
                    YuvMatrix::Bt709,
                    SampleRange::Full,
                )
                .pixel(0, 0)
            });
            assert_eq!(
                actual,
                expected.map(|gray| (gray, gray, gray)),
                "adjacent {depth}-bit images must not collapse or quantize to 8-bit"
            );
        }
    }

    #[test]
    fn inconsistent_sample_widths_are_rejected() {
        let samples = [512u16; 3];
        for depth in [8, 10, 12] {
            let expected_bytes = if depth == 8 { 1 } else { 2 };
            for plane in 0..3 {
                for bytes in [0, 1, 2, 3, 4].into_iter().filter(|&bytes| bytes != expected_bytes) {
                    let sources = samples
                        .each_ref()
                        .into_iter()
                        .enumerate()
                        .map(|(index, sample)| {
                            // SAFETY: a live u16 supplies two initialized bytes; invalid
                            // widths must be rejected before any sample access.
                            unsafe {
                                PlaneSource::from_raw_parts(
                                    std::ptr::from_ref(sample).cast(),
                                    2,
                                    1,
                                    1,
                                    2,
                                    0,
                                    if index == plane {
                                        bytes
                                    } else {
                                        expected_bytes
                                    },
                                )
                            }
                        })
                        .collect::<Result<Vec<_>, _>>();
                    let info = ColorInfo {
                        bit_depth: depth,
                        ..full_range(YuvMatrix::Bt709)
                    };
                    let result = sources.and_then(|sources| {
                        yuv_to_rgb(
                            &PlaneSet::new(sources.try_into().unwrap()),
                            &info,
                            info.max_sample().unwrap(),
                        )
                    });
                    assert!(
                        matches!(result, Err(FmetricsError::MalformedFrame { .. })),
                        "{depth}-bit plane {plane} with {bytes}-byte samples must fail: {result:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn inconsistent_peak_and_unsupported_depth_are_rejected() {
        let samples = [512u16; 3];
        for (depth, peak) in [(8, 1023), (10, 255), (10, 4095), (12, 1023), (16, 65535)] {
            let planes = PlaneSet {
                planes: samples.each_ref().map(|sample| {
                    // SAFETY: a live u16 supplies both initialized bytes.
                    unsafe {
                        PlaneSource::from_raw_parts(
                            std::ptr::from_ref(sample).cast(),
                            2,
                            1,
                            1,
                            2,
                            0,
                            if depth == 8 { 1 } else { 2 },
                        )
                        .unwrap()
                    }
                }),
            };
            let info = ColorInfo {
                bit_depth: depth,
                ..full_range(YuvMatrix::Bt709)
            };
            let result = yuv_to_rgb(&planes, &info, peak);
            assert!(
                matches!(result, Err(FmetricsError::UnsupportedFormat { .. })),
                "{depth}-bit peak {peak} must fail: {result:?}"
            );
        }
    }

    #[test]
    fn neutral_chroma_yields_grey() {
        // With U and V centred the output must be achromatic; any colour cast
        // here means the chroma offsets are wrong.
        for matrix in [YuvMatrix::Bt601, YuvMatrix::Bt709, YuvMatrix::Bt2020] {
            let neutral = vec![128u8; 4];
            let image = convert_frame(
                &frame444(2, 2, vec![128u8; 4], neutral.clone(), neutral),
                &full_range(matrix),
                255,
            )
            .unwrap();
            let (r, g, b) = pixel(&image, 0, 0);
            assert_eq!((r, g, b), (r, r, r), "{matrix:?} must be achromatic");
        }
    }

    #[test]
    fn extreme_chroma_clamps_instead_of_wrapping() {
        // A wrapped value would appear as a small number where saturation is
        // expected, so this catches a missing clamp.
        //
        // Green stays in range: the independent BT.709 inverse-luma equation
        // gives 164.129724, even though red and blue need clamping.
        let image = convert_frame(
            &frame444(2, 2, vec![128u8; 4], vec![255u8; 4], vec![0u8; 4]),
            &full_range(YuvMatrix::Bt709),
            255,
        )
        .unwrap();
        let (r, g, b) = pixel(&image, 0, 0);
        assert_eq!(b, 255, "blue must saturate high, not wrap");
        assert_eq!(r, 0, "red must saturate low, not wrap");
        assert_eq!(g, 164, "green must retain its unclipped BT.709 value");
    }

    #[test]
    fn full_range_black_and_white_map_to_extremes() {
        let neutral = vec![128u8; 4];
        let black = convert_frame(
            &frame444(2, 2, vec![0u8; 4], neutral.clone(), neutral.clone()),
            &full_range(YuvMatrix::Bt709),
            255,
        )
        .unwrap();
        assert_eq!(pixel(&black, 0, 0), (0, 0, 0), "black must stay black");

        let white = convert_frame(
            &frame444(2, 2, vec![255u8; 4], neutral.clone(), neutral),
            &full_range(YuvMatrix::Bt709),
            255,
        )
        .unwrap();
        assert_eq!(
            pixel(&white, 0, 0),
            (255, 255, 255),
            "white must stay white"
        );
    }

    #[test]
    fn limited_range_expands_before_converting() {
        // Limited-range black is Y=16 and white is Y=235. Treating 16 as black
        // only works if the expansion happens; skipping it would lift the
        // shadows of every limited-range clip.
        let limited = ColorInfo {
            matrix: YuvMatrix::Bt709,
            range: SampleRange::Limited,
            bit_depth: 8,
            hdr: false,
            colorspace: FmetricsColorspace::Srgb,
            transfer: av_decoders::TransferCharacteristic::IEC61966_2_1,
        };
        let neutral = vec![128u8; 4];
        let image = convert_frame(
            &frame444(2, 2, vec![16u8, 235, 16, 16], neutral.clone(), neutral),
            &limited,
            255,
        )
        .unwrap();
        assert_eq!(pixel(&image, 0, 0).0, 0, "limited-range 16 is black");
        assert!(
            pixel(&image, 1, 0).0 >= 254,
            "limited-range 235 is white, got {}",
            pixel(&image, 1, 0).0
        );
    }

    #[test]
    fn matrix_changes_the_result_for_saturated_colour() {
        // BT.601 and BT.709 differ in chroma gain. A fully saturated sample
        // would clamp to 255 under both and hide the difference, so the
        // chroma is set far enough off neutral to stay in range.
        let neutral = vec![128u8; 4];
        let convert = |matrix| {
            convert_frame(
                &frame444(2, 2, vec![96u8; 4], neutral.clone(), vec![176u8; 4]),
                &full_range(matrix),
                255,
            )
            .unwrap()
        };

        let bt601 = pixel(&convert(YuvMatrix::Bt601), 0, 0);
        let bt709 = pixel(&convert(YuvMatrix::Bt709), 0, 0);

        // The matrix must actually be applied, and must not be ignored.
        assert_ne!(bt601, bt709, "the matrix must affect the output");
        // Sanity: a mid-luma reddish sample stays mid-range rather than
        // clamping.
        assert!(
            bt601.0 < 255 && bt601.2 < 255,
            "the probe must not clamp, got {bt601:?}"
        );
    }

    // Hand-written covering-cell maps for 5x3 luma, including its final row/column.
    const CHROMA_LAYOUTS: [([usize; 2], [usize; 15]); 3] = [
        ([3, 2], [0, 0, 1, 1, 2, 0, 0, 1, 1, 2, 3, 3, 4, 4, 5]),
        ([3, 3], [0, 0, 1, 1, 2, 3, 3, 4, 4, 5, 6, 6, 7, 7, 8]),
        ([5, 3], [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14]),
    ];
    const CHROMA_Y: [u8; 15] =
        [100, 104, 108, 112, 116, 120, 124, 128, 132, 136, 140, 144, 148, 152, 156];
    const CHROMA_U: [u8; 15] =
        [104, 120, 136, 148, 112, 140, 124, 144, 108, 116, 132, 152, 100, 128, 156];
    const CHROMA_V: [u8; 15] =
        [148, 112, 140, 116, 136, 104, 152, 124, 144, 108, 128, 156, 120, 132, 100];

    #[expect(
        clippy::suboptimal_flops,
        reason = "keep the independent inverse-luma reference equation directly readable"
    )]
    fn assert_cell_map<T: v_frame::pixel::Pixel + Into<u16>>(
        geometry: [[usize; 2]; 2],
        samples: [&[T]; 3],
        cells: &[usize],
        bit_depth: usize,
        range: SampleRange,
    ) {
        let [[width, height], _] = geometry;
        let bytes = std::mem::size_of::<T>();
        let planes = PlaneSet {
            planes: std::array::from_fn(|index| {
                let [width, height] = geometry[usize::from(index != 0)];
                assert!(samples[index].len() >= width * height);
                // SAFETY: Pixel is sealed to initialized u8/u16; actual slices
                // remain live through conversion, with their full known lengths.
                unsafe {
                    PlaneSource::from_raw_parts(
                        samples[index].as_ptr().cast(),
                        std::mem::size_of_val(samples[index]),
                        width,
                        height,
                        (width * bytes) as isize,
                        0,
                        bytes,
                    )
                    .unwrap()
                }
            }),
        };
        let info = ColorInfo {
            range,
            bit_depth,
            ..full_range(YuvMatrix::Bt709)
        };
        let image = yuv_to_rgb(&planes, &info, info.max_sample().unwrap()).unwrap();
        assert_eq!((image.width(), image.height()), (width, height));
        assert_eq!(image.is_wide(), bytes == 2);
        assert_eq!(image.stride(), width * 3 * bytes);
        assert_eq!(image.as_bytes().len(), width * height * 3 * bytes);
        assert_eq!(cells.len(), width * height);

        let shift = f64::from(1u32 << (bit_depth - 8));
        let peak = f64::from(info.max_sample().unwrap());
        let center = f64::from(1u32 << (bit_depth - 1));
        let (black, white, chroma_span) = match range {
            SampleRange::Full => (0.0, peak, peak),
            SampleRange::Limited => (16.0 * shift, 235.0 * shift, 224.0 * shift),
        };
        for (index, &cell) in cells.iter().enumerate() {
            // Only the literal map associates pixels with chroma; this f64 reference
            // reconstructs green from the BT.709 luma identity, independently of production.
            let y = (f64::from(samples[0][index].into()) - black) / (white - black);
            let cb = (f64::from(samples[1][cell].into()) - center) / chroma_span;
            let cr = (f64::from(samples[2][cell].into()) - center) / chroma_span;
            let r = y + 2.0 * (1.0 - 0.2126) * cr;
            let b = y + 2.0 * (1.0 - 0.0722) * cb;
            let g = (y - 0.2126 * r - 0.0722 * b) / (1.0 - 0.2126 - 0.0722);
            let output_peak = if bytes == 2 { 65535.0 } else { 255.0 };
            let expected =
                [r, g, b].map(|c| (c * output_peak).round().clamp(0.0, output_peak) as u32);
            let actual = image.pixel(index % width, index / width);
            for (actual, expected) in [actual.0, actual.1, actual.2].into_iter().zip(expected) {
                assert!(
                    actual.abs_diff(expected) <= u32::from(bytes == 2),
                    "{geometry:?} {bit_depth}-bit {range:?} pixel {index}, cell {cell}: {actual} != {expected}"
                );
            }
        }
    }

    #[test]
    fn odd_420_422_444_use_explicit_covering_cells() {
        for (chroma, cells) in CHROMA_LAYOUTS {
            assert_cell_map(
                [[5, 3], chroma],
                [&CHROMA_Y, &CHROMA_U, &CHROMA_V],
                &cells,
                8,
                SampleRange::Full,
            );
        }
    }

    #[test]
    fn multi_pixel_u16_preserves_byte_strides_and_cell_precision() {
        for depth in [10, 12] {
            let samples = [CHROMA_Y, CHROMA_U, CHROMA_V].map(|plane| {
                std::array::from_fn::<_, 15, _>(|index| {
                    (u16::from(plane[index]) << (depth - 8)) + (index % 3) as u16
                })
            });
            for range in [SampleRange::Full, SampleRange::Limited] {
                for (chroma, cells) in CHROMA_LAYOUTS {
                    assert_cell_map(
                        [[5, 3], chroma],
                        samples.each_ref().map(|plane| plane.as_slice()),
                        &cells,
                        depth,
                        range,
                    );
                }
            }
        }
    }

    #[test]
    fn single_pixel_axes_accept_ambiguous_valid_layouts() {
        for (luma, chroma, cells) in [
            ([1, 1], [1, 1], &[0][..]),
            ([1, 3], [1, 2], &[0, 0, 1][..]),
            ([1, 3], [1, 3], &[0, 1, 2][..]),
            ([3, 1], [2, 1], &[0, 0, 1][..]),
            ([3, 1], [3, 1], &[0, 1, 2][..]),
        ] {
            assert_cell_map(
                [luma, chroma],
                [&CHROMA_Y, &CHROMA_U, &CHROMA_V],
                cells,
                8,
                SampleRange::Full,
            );
        }
    }

    #[test]
    fn inconsistent_or_unsupported_chroma_geometry_is_rejected() {
        let samples = [[128u8; 64]; 3];
        for (u, v) in [
            ([3, 2], [3, 3]),
            ([3, 3], [5, 3]),
            ([5, 3], [3, 3]),
            ([3, 2], [2, 2]),
            ([2, 2], [3, 2]),
            ([2, 1], [2, 1]),
            ([2, 2], [2, 2]),
            ([3, 1], [3, 1]),
            ([4, 2], [4, 2]),
            ([6, 3], [6, 3]),
            ([5, 4], [5, 4]),
            ([5, 2], [5, 2]),
            ([0, 0], [0, 0]),
            ([0, 2], [0, 2]),
            ([3, 0], [3, 0]),
        ] {
            let geometry = [[5, 3], u, v];
            let planes = (0..3)
                .map(|index| {
                    let [width, height] = geometry[index];
                    PlaneSource::from_bytes(&samples[index], width, height, width as isize, 0, 1)
                })
                .collect::<Result<Vec<_>, _>>();
            let result = planes.and_then(|planes| {
                yuv_to_rgb(
                    &PlaneSet::new(planes.try_into().unwrap()),
                    &full_range(YuvMatrix::Bt709),
                    255,
                )
            });
            assert!(
                matches!(result, Err(FmetricsError::MalformedFrame { .. })),
                "U={u:?}, V={v:?} must fail: {result:?}"
            );
        }
    }

    #[test]
    fn eight_bit_input_produces_narrow_samples() {
        let image = convert_frame(
            &frame444(2, 2, vec![128u8; 4], vec![128u8; 4], vec![128u8; 4]),
            &full_range(YuvMatrix::Bt709),
            255,
        )
        .unwrap();
        assert!(!image.is_wide());
        assert_eq!(image.stride(), 6);
    }

    #[test]
    fn stride_matches_width_times_channels() {
        for (width, height) in [(1usize, 1usize), (3, 2), (16, 9)] {
            let size = width * height;
            let image = convert_frame(
                &frame444(
                    width,
                    height,
                    vec![128u8; size],
                    vec![128u8; size],
                    vec![128u8; size],
                ),
                &full_range(YuvMatrix::Bt709),
                255,
            )
            .unwrap();
            assert_eq!(image.stride(), width * 3, "{width}x{height}");
            assert_eq!(
                image.as_bytes().len(),
                width * 3 * height,
                "{width}x{height}"
            );
        }
    }

    #[test]
    fn hdr_flag_is_carried_rather_than_linearised() {
        // The flag does not decode transfer functions; from_details rejects PQ/HLG.
        let info = ColorInfo {
            matrix: YuvMatrix::Bt2020,
            range: SampleRange::Limited,
            bit_depth: 10,
            hdr: true,
            colorspace: FmetricsColorspace::Srgb,
            transfer: av_decoders::TransferCharacteristic::IEC61966_2_1,
        };
        let samples = [512u16; 3];
        let planes = PlaneSet {
            planes: samples.each_ref().map(|sample| {
                PlaneSource::from_u16(std::slice::from_ref(sample), 1, 1, 2, 0).unwrap()
            }),
        };
        let image = yuv_to_rgb(&planes, &info, 1023).unwrap();
        let img = image.as_img(FmetricsColorspace::Srgb, true);
        assert!(img.as_raw().hdr, "the hdr flag must survive");
        assert_eq!(img.as_raw().colorspace, FmetricsColorspace::Srgb);
    }

    #[test]
    fn a_degenerate_frame_is_rejected() {
        let result = PlaneSource::from_bytes(&[], 0, 0, 0, 0, 1);
        assert!(matches!(result, Err(FmetricsError::MalformedFrame { .. })));
    }
}
