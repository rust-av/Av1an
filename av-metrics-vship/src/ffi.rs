//! Hand-written declarations for the libvship C API.
//!
//! libvship is opened with [`libloading`] rather than linked, so the crate has
//! no build-time dependency on it and compiles on machines where it is absent.
//! No `bindgen` is used: the surface needed here is small and stable, and
//! declaring it by hand keeps the build free of a libclang dependency.
//!
//! Every struct layout, enum discriminant and function signature below was
//! transcribed from `src/VshipAPI.h` and `src/VshipColor.h` in the Vship
//! repository at tag v5.1.2, not inferred.
//!
//! # Loading strategy
//!
//! The [`Library`] is deliberately leaked by [`VshipApi::load`], so the code
//! its function pointers refer to stays mapped for the life of the process.
//! Only the pointers are kept in [`VshipApi`]; since raw `fn` pointers are
//! `Send + Sync`, the resolved API can then be cached in a `OnceLock` and
//! shared freely.
//!
//! # Backends
//!
//! libvship is compiled per-backend. Since v5.1.2, Vulkan initialization is
//! lazy and a missing driver is reported through the library's API; other
//! backends may still fail to load when their runtime libraries are missing.

use std::{
    ffi::{c_char, c_int, c_void},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use libloading::Library;

use crate::error::VshipError;

// ponytail: serialize this binding's error-writing native calls across all scorers.
// Relax only after upstream's shared lastError is verified thread-local or synchronized.
static NATIVE_CALLS: Mutex<()> = Mutex::new(());

/// A libvship metric handler: an opaque pointer allocated by
/// `Vship_InitHandler`.
pub type VshipHandle = *mut c_void;

/// Pixel sample type.
///
/// The discriminants are **non-contiguous**: 9, 10, 12, 14 and 16 bits occupy
/// values 3, 5, 7, 9 and 11, leaving 4, 6, 8 and 10 for formats the C header
/// does not name. Every variant therefore carries an explicit value, and none
/// of these types may be assigned a `#[repr(C)]` sequential conversion.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VshipSample {
    /// 32-bit float samples.
    Float = 0,
    /// 16-bit half-float samples.
    Half = 1,
    /// 8-bit unsigned integer samples.
    Uint8 = 2,
    /// 9-bit unsigned integer samples.
    Uint9 = 3,
    /// 10-bit unsigned integer samples.
    Uint10 = 5,
    /// 12-bit unsigned integer samples.
    Uint12 = 7,
    /// 14-bit unsigned integer samples.
    Uint14 = 9,
    /// 16-bit unsigned integer samples.
    Uint16 = 11,
}

/// Luma/chroma sample range.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VshipRange {
    /// Limited (studio) range.
    Limited = 0,
    /// Full range.
    Full = 1,
}

/// Horizontal and vertical chroma decimation, as exponents.
///
/// `{1, 1}` is 4:2:0, `{1, 0}` is 4:2:2 and `{0, 0}` is 4:4:4.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VshipChromaSubsample {
    /// Horizontal decimation exponent: chroma width is `width >> subw`.
    pub subw: c_int,
    /// Vertical decimation exponent: chroma height is `height >> subh`.
    pub subh: c_int,
}

impl VshipChromaSubsample {
    /// 4:2:0 chroma.
    #[inline]
    #[must_use]
    pub const fn yuv420() -> Self {
        Self { subw: 1, subh: 1 }
    }

    /// 4:2:2 chroma.
    #[inline]
    #[must_use]
    pub const fn yuv422() -> Self {
        Self { subw: 1, subh: 0 }
    }

    /// 4:4:4 chroma.
    #[inline]
    #[must_use]
    pub const fn yuv444() -> Self {
        Self { subw: 0, subh: 0 }
    }
}

/// Chroma siting relative to the luma samples.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VshipChromaLocation {
    /// Left-sited, as MPEG-2 specifies.
    Left = 0,
    /// Centred.
    Center = 1,
    /// Top-left.
    TopLeft = 2,
    /// Top, as JPEG/MPEG-1 specifies.
    Top = 3,
}

/// Colour family.
///
/// The C header marks this deprecated and states that `YUVMatrix` alone decides
/// the family, with matrix 0 meaning RGB. It is still present in the struct, so
/// it is declared here and always set to [`Self::YUV`].
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VshipColorFamily {
    /// YUV content, which is all this crate produces.
    YUV = 0,
    /// RGB content.
    RGB = 1,
}

/// YUV to RGB matrix coefficients.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code, reason = "variants mirror the C enum for FFI fidelity")]
pub enum VshipYuvMatrix {
    /// Identity: the samples are already RGB.
    Rgb = 0,
    /// BT.709, correct for HD and above.
    Bt709 = 1,
    /// BT.470 system B/G, a.k.a. BT.601.
    Bt470Bg = 5,
    /// SMPTE 170M, identical coefficients to [`Self::Bt470Bg`].
    St170M = 6,
    /// YCgCo, as used by HEVC RExt.
    Ycgco = 8,
    /// BT.2020 non-constant luminance.
    Bt2020Ncl = 9,
    /// BT.2020 constant luminance.
    Bt2020Cl = 10,
    /// BT.2100 ICTCP.
    Bt2100Ictcp = 14,
    /// YCgCo-Re, as used by Dolby Vision RPU level 6.
    YcgcoRe = 16,
    /// YCgCo-Ro, as used by Dolby Vision RPU level 7.
    YcgcoRo = 17,
}

/// Electro-optical transfer function.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code, reason = "variants mirror the C enum for FFI fidelity")]
pub enum VshipTransferFunction {
    /// BT.709, whose curve also serves BT.601 and BT.2020 at 8 bits.
    Bt709 = 1,
    /// BT.470 system M (NTSC).
    Bt470M = 4,
    /// BT.470 system B/G.
    Bt470Bg = 5,
    /// BT.601.
    Bt601 = 6,
    /// SMPTE ST 240M.
    St240M = 7,
    /// Linear light.
    Linear = 8,
    /// sRGB.
    SRgb = 13,
    /// SMPTE ST 2084 (PQ).
    Pq = 16,
    /// SMPTE ST 428.
    St428 = 17,
    /// ARIB STD-B67 (HLG).
    Hlg = 18,
}

/// Colorimetry primaries.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VshipPrimaries {
    /// XYZ, libvship's internal working space.
    Internal = -1,
    /// BT.709, a.k.a. sRGB and HDTV.
    Bt709 = 1,
    /// BT.470 system M (NTSC).
    Bt470M = 4,
    /// BT.470 system B/G.
    Bt470Bg = 5,
    /// SMPTE 170M (NTSC).
    St170M = 6,
    /// SMPTE ST 240M, identical coefficients to [`Self::St170M`].
    St240M = 7,
    /// BT.2020.
    Bt2020 = 9,
    /// DCI-P3 with D65 white point.
    DisplayP3 = 12,
}

/// Region excluded from processing, in luma rows and columns.
///
/// Applied *after* any scaling, so a non-zero crop reduces the final picture
/// dimensions.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VshipCropRectangle {
    /// Rows removed from the top.
    pub top: c_int,
    /// Rows removed from the bottom.
    pub bottom: c_int,
    /// Columns removed from the left.
    pub left: c_int,
    /// Columns removed from the right.
    pub right: c_int,
}

/// A complete description of one input picture.
///
/// Passed by value across the FFI boundary and holding only integers and enums,
/// so `Copy` is sound.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VshipColorspace {
    /// Luma width in pixels.
    pub width: i64,
    /// Luma height in pixels.
    pub height: i64,
    /// Width to scale the picture to before cropping, or -1 for no scaling.
    pub target_width: i64,
    /// Height to scale the picture to before cropping, or -1 for no scaling.
    pub target_height: i64,
    /// Sample type.
    pub sample: VshipSample,
    /// Luma/chroma range.
    pub range: VshipRange,
    /// Chroma subsampling.
    pub subsampling: VshipChromaSubsample,
    /// Chroma siting.
    pub chroma_location: VshipChromaLocation,
    /// Colour family. Deprecated in the C header; [`Self::yuv_matrix`] decides.
    pub color_family: VshipColorFamily,
    /// YUV to RGB matrix.
    pub yuv_matrix: VshipYuvMatrix,
    /// Transfer function.
    pub transfer_function: VshipTransferFunction,
    /// Primaries.
    pub primaries: VshipPrimaries,
    /// Cropping applied after any scaling.
    pub crop: VshipCropRectangle,
}

impl VshipColorspace {
    /// A BT.709 limited-range 8-bit 4:2:0 space of the given size, which is the
    /// C header's own documented default.
    #[inline]
    #[must_use]
    pub const fn bt709_default(width: i64, height: i64) -> Self {
        Self {
            width,
            height,
            target_width: -1,
            target_height: -1,
            sample: VshipSample::Uint8,
            range: VshipRange::Limited,
            subsampling: VshipChromaSubsample { subw: 1, subh: 1 },
            chroma_location: VshipChromaLocation::Left,
            color_family: VshipColorFamily::YUV,
            yuv_matrix: VshipYuvMatrix::Bt709,
            transfer_function: VshipTransferFunction::Bt709,
            primaries: VshipPrimaries::Bt709,
            crop: VshipCropRectangle {
                top: 0,
                bottom: 0,
                left: 0,
                right: 0,
            },
        }
    }
}

/// The compute backend libvship was compiled for.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VshipBackend {
    /// AMD ROCm.
    Hip = 0,
    /// NVIDIA CUDA.
    Cuda = 1,
    /// Vulkan.
    Vulkan = 2,
    /// CPU only.
    Cpu = 3,
}

impl VshipBackend {
    /// The backend's name.
    #[inline]
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hip => "HIP",
            Self::Cuda => "CUDA",
            Self::Vulkan => "Vulkan",
            Self::Cpu => "CPU",
        }
    }
}

impl TryFrom<c_int> for VshipBackend {
    type Error = VshipError;

    #[inline]
    fn try_from(raw: c_int) -> Result<Self, Self::Error> {
        match raw {
            0 => Ok(Self::Hip),
            1 => Ok(Self::Cuda),
            2 => Ok(Self::Vulkan),
            3 => Ok(Self::Cpu),
            _ => Err(VshipError::IncompatibleLibrary {
                reason: format!("unknown backend {raw}"),
            }),
        }
    }
}

/// The libvship version and the backend it was built against.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VshipVersion {
    /// Major version.
    pub major: c_int,
    /// Minor version.
    pub minor: c_int,
    /// Patch version.
    pub minor_minor: c_int,
    /// Raw native backend; validate with [`VshipBackend::try_from`] before use.
    pub backend: c_int,
}

/// Known libvship status codes. Native entry points return [`c_int`], not this enum.
///
/// The C enum is grouped by cause but its numeric values are scattered, so
/// every variant carries an explicit value.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code, reason = "variants mirror the C enum for FFI fidelity")]
pub enum VshipException {
    /// Success.
    NoError = 0,
    /// A VRAM allocation failed.
    OutOfVram = 1,
    /// A RAM allocation failed.
    OutOfRam = 2,
    /// The input colorspace names an unknown display model.
    BadDisplayModel = 3,
    /// The two input colorspaces are incompatible with each other.
    DifferingInputType = 4,
    /// A non-RGBS input survived conversion, which should be impossible.
    NonRgbsInput = 5,
    /// Enumerating devices failed.
    DeviceCountError = 6,
    /// No usable device was found.
    NoDeviceDetected = 7,
    /// The device index is out of range.
    BadDeviceArgument = 8,
    /// The device is of an unusable kind.
    BadDeviceCode = 9,
    /// The handler is invalid.
    BadHandler = 10,
    /// A required pointer was null.
    BadPointer = 11,
    /// A ROCm error occurred.
    HipError = 12,
    /// A filesystem path was invalid.
    BadPath = 13,
    /// A JSON document was invalid.
    BadJson = 14,
    /// The operation is unsupported on this backend.
    NotSupported = 15,
    /// A struct's discriminator did not match the function that received it.
    BadInputStructType = 16,
    /// An error value outside the documented range was passed in.
    BadErrorType = 17,
}

impl VshipException {
    /// Whether this status indicates success.
    #[inline]
    #[must_use]
    pub const fn is_ok(self) -> bool {
        matches!(self, Self::NoError)
    }
}

/// Discriminator selecting which init or score struct is being passed.
///
/// `Vship_InitHandler` and `Vship_ComputeHandler` are metric-agnostic: the
/// `structType` field of the struct behind their `void *` selects the variant.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code, reason = "variants mirror the C enum for FFI fidelity")]
pub enum VshipStructType {
    /// Reserved, never passed.
    Null = 0,
    /// `Vship_InitSSIMULACRA2_1`.
    InitSsimulacra2 = 1,
    /// `Vship_InitButteraugli_1`.
    InitButteraugli = 2,
    /// `Vship_InitCVVDP_1`.
    InitCvvdp = 3,
    /// `Vship_ScoreSSIMULACRA2`.
    ScoreSsimulacra2 = 4,
    /// `Vship_ScoreButteraugli`.
    ScoreButteraugli = 5,
    /// `Vship_ScoreCVVDP`.
    ScoreCvvdp = 6,
}

/// Initialisation arguments for SSIMULACRA2.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VshipInitSsimulacra2_1 {
    /// Must be [`VshipStructType::InitSsimulacra2`].
    pub struct_type: VshipStructType,
    /// Colorspace of the reference frames.
    pub src_colorspace: VshipColorspace,
    /// Colorspace of the distorted frames.
    pub dis_colorspace: VshipColorspace,
    /// Device index, as accepted by `Vship_GPUFullCheck`.
    pub gpu_id: c_int,
}

/// Initialisation arguments for Butteraugli.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VshipInitButteraugli_1 {
    /// Must be [`VshipStructType::InitButteraugli`].
    pub struct_type: VshipStructType,
    /// Colorspace of the reference frames.
    pub src_colorspace: VshipColorspace,
    /// Colorspace of the distorted frames.
    pub dis_colorspace: VshipColorspace,
    /// Exponent of the norm to minimise. 2 by convention.
    pub q_norm: c_int,
    /// Peak display brightness in nits, which scales the metric's intensity
    /// weighting.
    pub intensity_multiplier: f32,
    /// Device index, as accepted by `Vship_GPUFullCheck`.
    pub gpu_id: c_int,
}

/// Initialisation arguments for CVVDP.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VshipInitCvvdp_1 {
    /// Must be [`VshipStructType::InitCvvdp`].
    pub struct_type: VshipStructType,
    /// Colorspace of the reference frames.
    pub src_colorspace: VshipColorspace,
    /// Colorspace of the distorted frames.
    pub dis_colorspace: VshipColorspace,
    /// Frame rate in frames per second, which sets the motion speed the
    /// temporal model assumes.
    pub fps: f32,
    /// Whether to scale to the display resolution named by the model.
    pub resize_to_display: bool,
    /// NUL-terminated model name, e.g. `standard_fhd`.
    pub model_key_cstr: *const c_char,
    /// NUL-terminated path to a JSON display-configuration override, or null
    /// for none.
    pub model_config_json_cstr: *const c_char,
    /// Device index, as accepted by `Vship_GPUFullCheck`.
    pub gpu_id: c_int,
}

/// Output of `Vship_ComputeHandler` for SSIMULACRA2.
///
/// Populated by the library.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VshipScoreSsimulacra2 {
    /// Raw tag, checked against [`VshipStructType::ScoreSsimulacra2`] after compute.
    pub struct_type: c_int,
    /// Written by libvship. Higher is better, and 1.0 is the maximum for
    /// identical inputs.
    pub score: f64,
}

impl Default for VshipScoreSsimulacra2 {
    #[inline]
    fn default() -> Self {
        Self {
            struct_type: VshipStructType::ScoreSsimulacra2 as c_int,
            score: 0.0,
        }
    }
}

/// Output of `Vship_ComputeHandler` for Butteraugli.
///
/// The distortion map is never retrieved: `dstp` stays null, which the C header
/// defines as meaning the map is not copied back from the device.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VshipScoreButteraugli {
    /// Raw tag, checked against [`VshipStructType::ScoreButteraugli`] after compute.
    pub struct_type: c_int,
    /// Written by libvship. The score for the configured `Qnorm`.
    pub norm_q: f64,
    /// Written by libvship. The L3 norm of the difference.
    pub norm_3: f64,
    /// Written by libvship. The Linfinity norm of the difference.
    pub norm_inf: f64,
    /// Always null: this crate does not retrieve distortion maps.
    pub dstp: *const u8,
    /// Unused while `dstp` is null.
    pub dst_stride: i64,
}

impl Default for VshipScoreButteraugli {
    #[inline]
    fn default() -> Self {
        Self {
            struct_type: VshipStructType::ScoreButteraugli as c_int,
            norm_q: 0.0,
            norm_3: 0.0,
            norm_inf: 0.0,
            dstp: std::ptr::null(),
            dst_stride: 0,
        }
    }
}

/// Output of `Vship_ComputeHandler` for CVVDP.
///
/// The CVVDP score accumulates over every frame the handler has seen, so it is
/// a running mean rather than a per-frame value. The distortion map is never
/// retrieved: `dstp` stays null.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VshipScoreCvvdp {
    /// Raw tag, checked against [`VshipStructType::ScoreCvvdp`] after compute.
    pub struct_type: c_int,
    /// Written by libvship. Higher is better.
    pub score: f64,
    /// Always null: this crate does not retrieve distortion maps.
    pub dstp: *const u8,
    /// Unused while `dstp` is null.
    pub dst_stride: i64,
}

impl Default for VshipScoreCvvdp {
    #[inline]
    fn default() -> Self {
        Self {
            struct_type: VshipStructType::ScoreCvvdp as c_int,
            score: 0.0,
            dstp: std::ptr::null(),
            dst_stride: 0,
        }
    }
}

type GetVersion = unsafe extern "C" fn() -> VshipVersion;
type GetDeviceCount = unsafe extern "C" fn(*mut c_int) -> c_int;
type GetDeviceInfo = unsafe extern "C" fn(*mut VshipDeviceInfo, c_int) -> c_int;
type GpuFullCheck = unsafe extern "C" fn(c_int) -> c_int;
type GetErrorMessage = unsafe extern "C" fn(c_int, *mut c_char, c_int) -> c_int;
type GetDetailedLastError = unsafe extern "C" fn(*mut c_char, c_int) -> c_int;
type GetDetailedLastErrorHandler = unsafe extern "C" fn(VshipHandle, *mut c_char, c_int) -> c_int;
type InitHandler = unsafe extern "C" fn(*mut VshipHandle, *mut c_void) -> c_int;
type FreeHandler = unsafe extern "C" fn(VshipHandle) -> c_int;
type ComputeHandler = unsafe extern "C" fn(
    VshipHandle,
    *mut c_void,
    *const *const u8,
    *const *const u8,
    *const i64,
    *const i64,
) -> c_int;
type Reset = unsafe extern "C" fn(VshipHandle) -> c_int;
type ResetScore = unsafe extern "C" fn(VshipHandle) -> c_int;

/// Allocates host-pinned memory for plane uploads.
///
/// A buffer allocated this way is only worth it when it is reused across
/// frames, since the pinning itself is the expensive part.
type PinnedMalloc2 = unsafe extern "C" fn(*mut *mut c_void, u64, c_int) -> c_int;

/// Releases memory from [`PinnedMalloc2`].
type PinnedFree2 = unsafe extern "C" fn(*mut c_void, c_int) -> c_int;

/// Vulkan-specific device capabilities.
///
/// Declared because it is embedded by value in [`VshipDeviceInfo`], so the
/// trailing padding it induces is part of that struct's size.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
#[allow(
    dead_code,
    reason = "only the layout matters; the C struct carries reserved fields"
)]
pub struct VshipVulkanFeatureMatrix {
    /// 16-bit storage buffer access.
    pub storage_buffer_16_bit_access: bool,
    /// 16-bit uniform and storage buffer access.
    pub uniform_and_storage_buffer_16_bit_access: bool,
    /// Variable pointers in storage buffers.
    pub variable_pointers_storage_buffer: bool,
    /// Variable pointers in shaders.
    pub variable_pointers: bool,
    /// 8-bit storage push constants.
    pub storage_push_constant_8: bool,
    /// 8-bit storage buffer access.
    pub storage_buffer_8_bit_access: bool,
    /// 8-bit uniform and storage buffer access.
    pub uniform_and_storage_buffer_8_bit_access: bool,
    /// Float16 shaders.
    pub shader_float_16: bool,
    /// Float64 shaders.
    pub shader_float_64: bool,
    /// Int8 shaders.
    pub shader_int_8: bool,
    /// Int16 shaders.
    pub shader_int_16: bool,
    /// Int64 shaders.
    pub shader_int_64: bool,
    /// Buffer device address.
    pub buffer_device_address: bool,
    /// Dynamic indexing of shader storage buffer arrays.
    pub shader_storage_buffer_array_dynamic_indexing: bool,
    /// Synchronization2.
    pub synchronization_2: bool,
    /// LogicOp.
    pub logic_op: bool,
    /// Reserved for future use.
    pub future_features: [bool; 10],
}

impl Default for VshipVulkanFeatureMatrix {
    #[inline]
    fn default() -> Self {
        Self {
            storage_buffer_16_bit_access: false,
            uniform_and_storage_buffer_16_bit_access: false,
            variable_pointers_storage_buffer: false,
            variable_pointers: false,
            storage_push_constant_8: false,
            storage_buffer_8_bit_access: false,
            uniform_and_storage_buffer_8_bit_access: false,
            shader_float_16: false,
            shader_float_64: false,
            shader_int_8: false,
            shader_int_16: false,
            shader_int_64: false,
            buffer_device_address: false,
            shader_storage_buffer_array_dynamic_indexing: false,
            synchronization_2: false,
            logic_op: false,
            future_features: [false; 10],
        }
    }
}

/// What libvship reports about a compute device.
///
/// `Default` is implemented by hand because `[c_char; 256]` exceeds the array
/// length `Default` covers for arbitrary `T`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VshipDeviceInfo {
    /// Device name, NUL-padded.
    pub name: [c_char; 256],
    /// VRAM size in bytes.
    pub vram_size: u64,
    /// Non-zero when the device is integrated.
    pub integrated: c_int,
    /// Streaming multiprocessor count.
    pub multi_processor_count: c_int,
    /// Warp size.
    pub warp_size: c_int,
    /// Only populated for the Vulkan backend.
    pub vulkan_feature_matrix: VshipVulkanFeatureMatrix,
}

impl VshipDeviceInfo {
    /// The device name with its NUL padding trimmed.
    ///
    /// The C header declares `name` as `char`, so each element is a byte in
    /// practice; a name that is not valid UTF-8 is lossily converted rather
    /// than treated as an error, since it is diagnostic output only.
    #[inline]
    #[must_use]
    pub fn name(&self) -> String {
        let end = self.name.iter().position(|&byte| byte == 0).unwrap_or(self.name.len());

        // The C `char` is signed, so reinterpreting each element as a byte is
        // required rather than optional; a name byte above 0x7F is negative as
        // `c_char`.
        let bytes: Vec<u8> = self.name[..end].iter().map(|&byte| byte as u8).collect();

        String::from_utf8_lossy(&bytes).into_owned()
    }
}

/// A `VshipDeviceInfo` before libvship has filled one in.
///
/// Hand-written because `[c_char; 256]` exceeds the array length `Default`
/// covers for an arbitrary `T`.
impl Default for VshipDeviceInfo {
    #[inline]
    fn default() -> Self {
        Self {
            // NUL-filled, which `name` trims to an empty string.
            name: [0; 256],
            vram_size: 0,
            integrated: 0,
            multi_processor_count: 0,
            warp_size: 0,
            vulkan_feature_matrix: VshipVulkanFeatureMatrix::default(),
        }
    }
}

/// Resolved libvship entry points.
///
/// Contains only function pointers and copyable data, so it is `Send + Sync`
/// and can be cached process-wide. The [`Library`] backing these pointers is
/// leaked by [`VshipApi::load`] and never unloaded.
///
/// # Raw-call safety
///
/// Public function pointers bypass the safe wrapper's shared native-call lock.
/// Callers must prevent raw error-state readers/writers from overlapping safe
/// APIs or other raw calls. This binding cannot coordinate separate bindings,
/// plugins or duplicate crate versions that do not share its lock.
#[derive(Clone, Copy)]
pub struct VshipApi {
    /// Checked once at admission, before any device or handler call.
    backend: VshipBackend,
    /// Reports the library version and backend.
    pub get_version: GetVersion,
    /// Counts the usable compute devices.
    pub get_device_count: GetDeviceCount,
    /// Describes one compute device.
    pub get_device_info: GetDeviceInfo,
    /// Verifies a device can actually run libvship.
    pub gpu_full_check: GpuFullCheck,
    /// Expands a status code into its message.
    pub get_error_message: GetErrorMessage,
    /// Retrieves the process-wide last error.
    pub get_detailed_last_error: GetDetailedLastError,
    /// Retrieves one handler's last error.
    pub get_detailed_last_error_handler: GetDetailedLastErrorHandler,
    /// Creates a metric handler for any metric.
    pub init_handler: InitHandler,
    /// Destroys a metric handler.
    pub free_handler: FreeHandler,
    /// Scores one reference/distorted pair.
    pub compute_handler: ComputeHandler,
    /// Clears a temporal metric's frame history.
    pub reset: Reset,
    /// Clears a temporal metric's accumulated score.
    pub reset_score: ResetScore,
    /// Allocates host-pinned memory for plane uploads.
    pub pinned_malloc2: Option<PinnedMalloc2>,
    /// Releases memory from `pinned_malloc2`.
    pub pinned_free2: Option<PinnedFree2>,
}

impl VshipApi {
    /// The libvship version, formatted as `major.minor.minorMinor`.
    #[inline]
    #[must_use]
    pub fn version_string(&self) -> String {
        // SAFETY: `get_version` takes no arguments and only reads the library's
        // own constant version data.
        let version = unsafe { (self.get_version)() };
        format!(
            "{}.{}.{}",
            version.major, version.minor, version.minor_minor
        )
    }

    /// The backend this libvship build targets.
    #[inline]
    #[must_use]
    pub fn backend(&self) -> VshipBackend {
        self.backend
    }

    /// The number of compute devices libvship can see.
    ///
    /// # Errors
    ///
    /// Returns [`VshipError::CallFailed`] if enumeration fails, which libvship
    /// reports rather than leaving the count meaningful.
    #[inline]
    pub fn device_count(&self) -> Result<u32, VshipError> {
        let mut count: c_int = 0;
        // SAFETY: `count` is a writable out-pointer for one `c_int`, which is
        // exactly what `Vship_GetDeviceCount` writes.
        self.call("Vship_GetDeviceCount", || unsafe {
            (self.get_device_count)(&raw mut count)
        })?;

        u32::try_from(count).map_err(|_| VshipError::DeviceCount { count })
    }

    /// What libvship reports about one device.
    ///
    /// # Errors
    ///
    /// Returns [`VshipError::CallFailed`] if `gpu_id` is not a valid index.
    #[inline]
    pub fn device_info(&self, gpu_id: u32) -> Result<VshipDeviceInfo, VshipError> {
        let mut info = VshipDeviceInfo::default();
        // SAFETY: `info` is a writable `VshipDeviceInfo` the library fills in
        // place, and `gpu_id` is narrowed from a value libvship enumerated.
        self.call("Vship_GetDeviceInfo", || unsafe {
            (self.get_device_info)(&raw mut info, gpu_id as c_int)
        })?;

        Ok(info)
    }

    /// Whether a device can actually run libvship.
    ///
    /// This is the C API's own combined check: the header describes it as the
    /// useful function for seeing whether vship will work at all, with several
    /// distinct failures possible. It confirms the device after the library
    /// loads.
    ///
    /// # Errors
    ///
    /// Returns [`VshipError::CallFailed`] carrying libvship's own message when
    /// the device is unusable.
    #[inline]
    pub fn gpu_full_check(&self, gpu_id: u32) -> Result<(), VshipError> {
        // SAFETY: `gpu_id` was narrowed from an enumerated index, and the
        // function takes no pointers.
        self.call("Vship_GPUFullCheck", || unsafe {
            (self.gpu_full_check)(gpu_id as c_int)
        })
    }

    /// Run one error-writing native call and capture its short diagnostic.
    /// Resolve the API, default-device caches and arguments before entering;
    /// the closure must not reenter this lock or invoke user callbacks.
    pub(crate) fn call(
        &self,
        function: &'static str,
        call: impl FnOnce() -> c_int,
    ) -> Result<(), VshipError> {
        // No Rust logical state is protected: unwinding cannot invalidate it.
        // Recover poison so Drop still releases an already-owned native handle.
        let _guard = NATIVE_CALLS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let status = call();
        if status == 0 {
            Ok(())
        } else {
            Err(VshipError::CallFailed {
                function,
                status,
                // Use the resolved API, never load()/check() under this lock.
                message: short_error_message(self, status),
            })
        }
    }

    /// Load libvship and resolve the entry points the scorer needs.
    ///
    /// The `Library` is intentionally leaked: libvship has no meaningful global
    /// teardown, and the pointers stored in the returned [`VshipApi`] must
    /// remain valid for the life of the process.
    ///
    /// The result is cached, so repeated calls are cheap.
    ///
    /// # Errors
    ///
    /// Returns [`VshipError::LibraryNotFound`] when no candidate opens or a
    /// required entry point is missing, and [`VshipError::IncompatibleLibrary`]
    /// when the opened binary reports an unverified version or backend.
    #[inline]
    pub fn load() -> Result<&'static Self, VshipError> {
        static API: OnceLock<Result<VshipApi, std::io::Error>> = OnceLock::new();

        match API.get_or_init(resolve) {
            Ok(api) => Ok(api),
            Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
                Err(VshipError::IncompatibleLibrary {
                    reason: error.to_string(),
                })
            },
            Err(error) => Err(VshipError::LibraryNotFound {
                reason: format!(
                    "libvship could not be loaded ({}); tried {}",
                    error,
                    library_candidates()
                        .iter()
                        .map(|candidate| candidate.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            }),
        }
    }
}

/// Check a libvship status code, mapping a failure to
/// [`VshipError::CallFailed`].
#[inline]
pub fn check(status: c_int, function: &'static str) -> Result<(), VshipError> {
    if status == 0 {
        Ok(())
    } else {
        Err(VshipError::CallFailed {
            function,
            status,
            message: error_message(status),
        })
    }
}

/// The short message libvship associates with a status code.
///
/// The header's two-buffer protocol, where a zero `len` reports the required
/// size, exists to size a caller-owned buffer. A fixed one is used here, so a
/// message longer than the buffer is simply truncated.
#[inline]
#[must_use]
pub fn error_message(exception: c_int) -> String {
    // The native helper casts to an internal C++ enum; do not feed it unknown
    // codes. Numeric diagnostics are meaningful without native interpretation.
    if !(0..=17).contains(&exception) {
        return fallback_message(exception);
    }

    // A loaded library is required to translate the code, so an unloadable one
    // falls back to the numeric status.
    let Ok(api) = VshipApi::load() else {
        return fallback_message(exception);
    };

    short_error_message(api, exception)
}

fn short_error_message(api: &VshipApi, exception: c_int) -> String {
    const CAPACITY: usize = 1024;

    // The pinned short helper constructs only a local string from the status;
    // unlike GetDetailedLastError it never reads native global lastError.
    if !(0..=17).contains(&exception) {
        return fallback_message(exception);
    }

    let mut buffer = vec![0 as c_char; CAPACITY];
    // SAFETY: `exception` is a valid status code, and `buffer` is a writable
    // `CAPACITY`-element array matching the length passed alongside it.
    let written =
        unsafe { (api.get_error_message)(exception, buffer.as_mut_ptr(), CAPACITY as c_int) };
    if written <= 0 {
        return fallback_message(exception);
    }

    // SAFETY: a non-positive return means libvship wrote nothing, so the buffer
    // is untouched; otherwise it wrote a NUL-terminated string into the
    // capacity it was given.
    let text = unsafe { std::ffi::CStr::from_ptr(buffer.as_ptr()) }
        .to_string_lossy()
        .into_owned();

    if text.is_empty() {
        fallback_message(exception)
    } else {
        text
    }
}

/// The message used when libvship cannot be asked about a status code.
#[inline]
#[must_use]
fn fallback_message(exception: c_int) -> String {
    format!("libvship error {exception}")
}

/// Resolve one of the candidate libvship libraries.
fn resolve() -> Result<VshipApi, std::io::Error> {
    let library = open_library()?;
    let get_version: GetVersion = required(&library, b"Vship_GetVersion\0")?;
    // SAFETY: the pinned version record contains only C integers, including
    // backend. Admission precedes all layout-sensitive/device calls.
    let version = unsafe { get_version() };
    if (version.major, version.minor, version.minor_minor) != (5, 1, 2) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "unsupported version {}.{}.{}; expected 5.1.2",
                version.major, version.minor, version.minor_minor
            ),
        ));
    }
    // Do not call check/error_message: this OnceLock is still initializing.
    let backend = VshipBackend::try_from(version.backend)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string()))?;
    // Keep the mapping alive for the life of the process. The pointers resolved
    // below are only valid while it is loaded.
    let library = Box::leak(Box::new(library));

    // Each `required` call checks for the symbol's presence before its result
    // is dereferenced, and `T` is written to match the C signature of
    // `name`. The leaked `library` above keeps the code these point at
    // mapped.
    Ok(VshipApi {
        backend,
        get_version,
        get_device_count: required(library, b"Vship_GetDeviceCount\0")?,
        get_device_info: required(library, b"Vship_GetDeviceInfo\0")?,
        gpu_full_check: required(library, b"Vship_GPUFullCheck\0")?,
        get_error_message: required(library, b"Vship_GetErrorMessage\0")?,
        get_detailed_last_error: required(library, b"Vship_GetDetailedLastError\0")?,
        get_detailed_last_error_handler: required(library, b"Vship_GetDetailedLastErrorHandler\0")?,
        init_handler: required(library, b"Vship_InitHandler\0")?,
        free_handler: required(library, b"Vship_FreeHandler\0")?,
        compute_handler: required(library, b"Vship_ComputeHandler\0")?,
        reset: required(library, b"Vship_Reset\0")?,
        reset_score: required(library, b"Vship_ResetScore\0")?,
        // Optional: an older or trimmed build may not export the pinned-memory
        // entry points, in which case planes are uploaded from decoder memory.
        pinned_malloc2: optional(library, b"Vship_PinnedMalloc2\0"),
        pinned_free2: optional(library, b"Vship_PinnedFree2\0"),
    })
}

/// Resolve a required symbol.
fn required<T: Copy>(library: &Library, name: &[u8]) -> Result<T, std::io::Error> {
    // SAFETY: the caller guarantees `T` matches the C signature for `name`.
    unsafe { library.get(name) }
        .map(|symbol| *symbol)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::NotFound, error.to_string()))
}

/// Resolve an optional symbol, yielding `None` when the build omits it.
fn optional<T: Copy>(library: &Library, name: &[u8]) -> Option<T> {
    // SAFETY: the caller guarantees `T` matches the C signature for `name`.
    unsafe { library.get(name) }.ok().map(|symbol| *symbol)
}

/// Open the first libvship candidate that loads.
fn open_library() -> Result<Library, std::io::Error> {
    let mut failures = Vec::new();

    for candidate in library_candidates() {
        // SAFETY: loading an arbitrary shared library executes its
        // initialisers. This is the intended way to use libvship, and
        // the candidate paths are constrained to libvship file names.
        match unsafe { load_library(&candidate) } {
            Ok(library) => return Ok(library),
            Err(error) => failures.push(format!("{}: {error}", candidate.display())),
        }
    }

    tracing::debug!(?failures, "no libvship candidate could be loaded");
    Err(library_load_error(&failures))
}

fn library_load_error(failures: &[String]) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::NotFound, failures.join("; "))
}

#[cfg(any(target_os = "windows", test))]
fn windows_load_policy(candidate: &Path) -> std::io::Result<(PathBuf, bool)> {
    if candidate.as_os_str().is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "empty DLL path",
        ));
    }
    if candidate.file_name().is_some_and(|name| name == candidate.as_os_str()) {
        Ok((candidate.to_path_buf(), false))
    } else {
        std::path::absolute(candidate).map(|path| (path, true))
    }
}

#[cfg(any(target_os = "windows", test))]
fn windows_directory_override(value: &std::ffi::OsStr) -> std::io::Result<Option<PathBuf>> {
    if value.is_empty() {
        return Ok(None);
    }
    let directory = std::path::absolute(Path::new(value))?;
    if !directory.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "{} is not an existing library directory",
                directory.display()
            ),
        ));
    }
    Ok(Some(directory))
}

#[cfg(any(target_os = "windows", test))]
fn windows_load_error(error: impl std::error::Error + Send + Sync + 'static) -> std::io::Error {
    std::io::Error::other(WindowsLoadError(error))
}

// libloading's Windows Display omits the OS cause; retain and render its chain.
#[cfg(any(target_os = "windows", test))]
#[derive(Debug)]
struct WindowsLoadError<E>(E);

#[cfg(any(target_os = "windows", test))]
impl<E: std::error::Error> std::fmt::Display for WindowsLoadError<E> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)?;
        let mut source = self.0.source();
        while let Some(cause) = source {
            write!(formatter, ": {cause}")?;
            source = cause.source();
        }
        Ok(())
    }
}

#[cfg(any(target_os = "windows", test))]
impl<E: std::error::Error + 'static> std::error::Error for WindowsLoadError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

/// # Safety
/// The caller must trust native initializers and terminators in the selected DLL.
#[cfg(target_os = "windows")]
unsafe fn load_library(candidate: &Path) -> std::io::Result<Library> {
    use libloading::os::windows::{
        LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
        Library as WindowsLibrary,
    };
    let (path, siblings) = windows_load_policy(candidate)?;
    let flags = LOAD_LIBRARY_SEARCH_DEFAULT_DIRS
        | if siblings {
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR
        } else {
            0
        };
    // SECURITY: per-load search only; never retry with flags 0 or mutate global policy.
    // SAFETY: delegated to the caller, including native initializers/terminators.
    unsafe { WindowsLibrary::load_with_flags(&path, flags) }
        .map(Library::from)
        .map_err(windows_load_error)
}

#[cfg(not(target_os = "windows"))]
unsafe fn load_library(candidate: &Path) -> std::io::Result<Library> {
    // SAFETY: delegated to the caller, including native initializers/terminators.
    unsafe { Library::new(candidate) }.map_err(std::io::Error::other)
}

#[cfg(any(target_os = "windows", test))]
fn windows_script_directory(script: &Path) -> std::io::Result<Option<PathBuf>> {
    script_parent(script).map(std::path::absolute).transpose()
}

fn script_parent(script: &Path) -> Option<&Path> {
    // A bare filename selects no directory, not the media CWD's plugins.
    script.parent().filter(|parent| !parent.as_os_str().is_empty())
}

#[cfg(test)]
#[path = "../../test-support/loader_policy.rs"]
mod loader_policy_tests;

/// Candidate libvship paths, most specific first.
///
/// The order is deliberate. Where libvship works it is the VapourSynth plugin
/// `vapoursynth/plugins/libvship.dll`, a sibling of `vsscript.dll` and on no
/// loader search path, so every environment-derived directory must be offered
/// before the bare names that let the platform loader search its own paths.
/// That is what makes "the VS plugin works" imply "the C API loads".
#[inline]
#[must_use]
pub fn library_candidates() -> Vec<PathBuf> {
    let directories = CandidateDirectories {
        overrides: [env_directory("VSHIP_PLUGIN_PATH"), env_directory("VSHIP_LIB_DIR")],
        runtime: std::env::var_os("VSSCRIPT_PATH").as_deref().and_then(script_runtime_directory),
        extra: std::env::var_os("VAPOURSYNTH_EXTRA_PLUGIN_PATH")
            .as_deref()
            .map(extra_plugin_directories)
            .unwrap_or_default(),
        system: system_directories(),
    };
    library_candidates_from_directories(&directories)
}

/// Resolved at the environment/OS boundary; candidate ordering needs no ambient state.
#[derive(Default)]
struct CandidateDirectories {
    overrides: [Option<PathBuf>; 2],
    runtime: Option<PathBuf>,
    extra: Vec<PathBuf>,
    system: Vec<PathBuf>,
}

fn library_candidates_from_directories(directories: &CandidateDirectories) -> Vec<PathBuf> {
    let names = library_names();
    let mut candidates = Vec::with_capacity(names.len() * 8);

    // 1. The user's explicit overrides.
    for directory in directories.overrides.iter().flatten() {
        candidates.extend(names.iter().map(|name| directory.join(name)));
    }

    // 2. The VapourSynth runtime that owns the plugin. `VSSCRIPT_PATH` names
    //    the script DLL, so the plugin directory is its sibling `plugins`.
    if let Some(runtime) = &directories.runtime {
        candidates.extend(names.iter().map(|name| runtime.join("plugins").join(name)));
        candidates.extend(names.iter().map(|name| runtime.join(name)));
    }

    // 3. VapourSynth's own extra-plugin path list, in supplied order.
    for directory in &directories.extra {
        candidates.extend(names.iter().map(|name| directory.join(name)));
    }

    // 4. Platform system directories.
    for directory in &directories.system {
        candidates.extend(names.iter().map(|name| directory.join(name)));
    }

    // 5. Bare names let the platform loader search its own paths last.
    candidates.extend(names.iter().map(PathBuf::from));
    candidates
}

fn script_runtime_directory(script: &std::ffi::OsStr) -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    match windows_script_directory(Path::new(script)) {
        Ok(directory) => directory,
        Err(error) => {
            tracing::warn!(%error, "ignoring invalid VSSCRIPT_PATH");
            None
        },
    }
    #[cfg(not(target_os = "windows"))]
    script
        .to_str()
        .filter(|script| !script.is_empty())
        .and_then(|script| script_parent(Path::new(script)))
        .map(Path::to_path_buf)
}

fn extra_plugin_directories(extra: &std::ffi::OsStr) -> Vec<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let mut directories = Vec::new();
        for entry in std::env::split_paths(extra).filter(|entry| !entry.as_os_str().is_empty()) {
            match windows_directory_override(entry.as_os_str()) {
                Ok(Some(directory)) => directories.push(directory),
                Ok(None) => {},
                Err(error) => {
                    tracing::warn!(%error, "ignoring invalid VAPOURSYNTH_EXTRA_PLUGIN_PATH entry")
                },
            }
        }
        directories
    }
    #[cfg(not(target_os = "windows"))]
    extra
        .to_str()
        .into_iter()
        .flat_map(|extra| extra.split(':'))
        .filter(|entry| !entry.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// Directories a system-wide install would use on this host.
///
/// On Windows these are asked of the OS rather than written down. `C:\Windows`
/// is not a safe constant: Windows can be installed to another drive or
/// relocated, and a hardcoded path then names a directory that does not exist.
/// `GetSystemDirectoryW` and its WOW64 counterpart report what this process can
/// actually load from, which also gets the WoW64 redirection right without
/// reasoning about the process bitness here.
///
/// Nothing is returned on other platforms, where the bare names already reach
/// every directory the loader searches.
#[inline]
#[must_use]
fn system_directories() -> Vec<PathBuf> {
    if !cfg!(target_os = "windows") {
        return Vec::new();
    }

    // Native System32 first, then the WOW64 directory. A 32-bit process asking
    // for System32 is redirected to SysWOW64, which is the directory it can
    // really load from, so both entry points are worth asking.
    let mut directories = Vec::with_capacity(2);
    directories.extend(windows_system_directory(true));
    directories.extend(windows_system_directory(false));
    directories.extend(windows_directory("SysWOW64"));
    directories.dedup();

    directories
}

/// One of Windows' system directories, as the OS reports it.
///
/// `native_system` selects `GetSystemDirectoryW` over
/// `GetSystemWow64DirectoryW`.
#[cfg(target_os = "windows")]
fn windows_system_directory(native_system: bool) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;

    unsafe extern "system" {
        fn GetSystemDirectoryW(buffer: *mut u16, size: u32) -> u32;
        fn GetSystemWow64DirectoryW(buffer: *mut u16, size: u32) -> u32;
    }

    let entry: unsafe extern "system" fn(*mut u16, u32) -> u32 = if native_system {
        GetSystemDirectoryW
    } else {
        GetSystemWow64DirectoryW
    };

    // A return equal to the buffer size means the path was truncated, so grow
    // and ask again rather than using a partial directory.
    let mut buffer = vec![0u16; 260];
    loop {
        let capacity = buffer.len();
        // SAFETY: `buffer` is writable for `capacity` units, which is the size
        // passed alongside it.
        let written = unsafe { entry(buffer.as_mut_ptr(), capacity as u32) };
        if written == 0 {
            return None;
        }
        if (written as usize) < capacity {
            buffer.truncate(written as usize);
            break;
        }
        buffer.resize(capacity * 2, 0);
    }

    Some(PathBuf::from(std::ffi::OsString::from_wide(&buffer)))
}

/// Nothing to ask off Windows.
#[cfg(not(target_os = "windows"))]
#[inline]
#[must_use]
const fn windows_system_directory(_native_system: bool) -> Option<PathBuf> {
    None
}

/// A directory beside `%SystemRoot%`.
#[cfg(target_os = "windows")]
fn windows_directory(subdirectory: &str) -> Option<PathBuf> {
    env_directory("SystemRoot").map(|root| root.join(subdirectory))
}

/// Nothing to ask off Windows.
#[cfg(not(target_os = "windows"))]
#[inline]
#[must_use]
fn windows_directory(_subdirectory: &str) -> Option<PathBuf> {
    None
}

/// Candidate library file names for the host platform.
#[inline]
fn library_names() -> &'static [&'static str] {
    if cfg!(target_os = "windows") {
        &["libvship.dll", "vship.dll"]
    } else if cfg!(target_os = "macos") {
        &["libvship.dylib", "libvship.2.dylib"]
    } else {
        &["libvship.so", "libvship.so.2"]
    }
}

/// A directory named by an environment variable, if it is set and non-empty.
#[inline]
#[must_use]
fn env_directory(variable: &str) -> Option<PathBuf> {
    let value = std::env::var_os(variable)?;
    match directory_override(&value) {
        Ok(directory) => directory,
        Err(error) => {
            tracing::warn!(%error, variable, "ignoring invalid library directory");
            None
        },
    }
}

fn directory_override(value: &std::ffi::OsStr) -> std::io::Result<Option<PathBuf>> {
    #[cfg(target_os = "windows")]
    {
        windows_directory_override(value)
    }
    #[cfg(not(target_os = "windows"))]
    Ok(value.to_str().filter(|value| !value.is_empty()).map(PathBuf::from))
}

/// The directory the loaded libvship module was mapped from.
///
/// Asks the operating system rather than inferring it from the candidate list,
/// so it is correct however the library was found: an explicit
/// `VSHIP_PLUGIN_PATH`, the VapourSynth plugin directory, the platform loader's
/// search path, or an already-mapped module.
#[inline]
#[must_use]
pub fn loaded_library_directory() -> Option<&'static Path> {
    static MODULE_DIRECTORY: OnceLock<Option<PathBuf>> = OnceLock::new();

    MODULE_DIRECTORY
        .get_or_init(|| {
            let Ok(api) = VshipApi::load() else {
                return None;
            };

            // `get_version` is a valid function pointer once the library is
            // loaded, and its address identifies the module holding it.
            windows_module_directory_from(api.get_version as *const c_void)
        })
        .as_deref()
}

/// The directory the OS reports for the mapped libvship module.
#[cfg(target_os = "windows")]
fn windows_module_directory_from(address: *const c_void) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;

    unsafe extern "system" {
        fn GetModuleHandleExW(
            flags: u32,
            module: *const c_void,
            module_handle: *mut *mut c_void,
        ) -> i32;
        fn GetModuleFileNameW(module: *mut c_void, buffer: *mut u16, size: u32) -> u32;
    }

    /// Ask for the module containing an address.
    const FROM_ADDRESS: u32 = 0x0000_0004;
    /// Report the module without changing its reference count, so the caller
    /// does not have to free it.
    const UNCHANGED_REFCOUNT: u32 = 0x0000_0002;

    let mut handle: *mut c_void = std::ptr::null_mut();
    // SAFETY: `address` is a valid code address inside a loaded module, and
    // `handle` is a writable out-pointer.
    if unsafe { GetModuleHandleExW(FROM_ADDRESS | UNCHANGED_REFCOUNT, address, &raw mut handle) }
        == 0
    {
        return None;
    }

    // A return equal to the buffer size means the name was truncated, so grow
    // the buffer and ask again.
    let mut buffer = vec![0u16; 512];
    loop {
        let capacity = buffer.len();
        // SAFETY: `buffer` is writable for `buffer.len()` units, which is the
        // capacity passed alongside it.
        let written = unsafe { GetModuleFileNameW(handle, buffer.as_mut_ptr(), capacity as u32) };
        if written == 0 {
            return None;
        }
        if written < capacity as u32 {
            buffer.truncate(written as usize);
            break;
        }
        buffer.resize(capacity * 2, 0);
    }

    let path =
        std::ffi::OsString::from_wide(buffer.split(|&unit| unit == 0).next().unwrap_or_default());

    // The API answers with a file name; callers want the directory it sits in.
    let path = PathBuf::from(path);
    Some(path.parent()?.to_path_buf())
}

/// No module introspection is available off Windows.
#[cfg(not(target_os = "windows"))]
fn windows_module_directory_from(_address: *const c_void) -> Option<PathBuf> {
    None
}

#[cfg(test)]
mod tests;
