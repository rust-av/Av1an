//! Hand-written declarations for the fmetrics C API.
//!
//! fmetrics is opened with [`libloading`] rather than linked, so the crate has
//! no build-time dependency on it. No `bindgen`: the surface is small and
//! stable, so declaring it by hand keeps libclang out of the build.
//!
//! Every layout, discriminant and signature below was transcribed from
//! `src/fmetrics.h` at commit `ae87c8e5607063f0dcc8241b52cb143f6c9ad4ac`.
//!
//! [`FmetricsApi::load`] deliberately leaks the [`Library`], so the code its
//! pointers refer to stays mapped. Only the pointers are kept, and since raw
//! `fn` pointers are `Send + Sync` the resolved API caches in a [`OnceLock`].
//!
//! fmetrics takes **interleaved RGB** in a single buffer with a single stride,
//! not planar YUV, so the conversion module produces one packed image per pair.

use std::{
    ffi::{c_char, c_int, c_uint, c_void},
    path::{Path, PathBuf},
    sync::OnceLock,
};

use libloading::Library;

use crate::error::FmetricsError;

/// Reusable scratch allocations for the frame-independent metrics.
///
/// Handed to each `*_cmp` call and reused across pairs so the working buffers
/// are not reallocated per frame.
///
/// A newtype rather than a `*mut c_void` alias so the handle can be `Send +
/// Sync`, which a raw pointer is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct FmetricsWorkspace(pub *mut c_void);

impl FmetricsWorkspace {
    /// Whether this is the null handle, meaning nothing was created.
    #[inline]
    #[must_use]
    pub fn is_null(self) -> bool {
        self.0.is_null()
    }
}

// SAFETY: the handle is opaque, the library owns the memory, and this crate
// never dereferences it -- it only passes it back. Concurrent *use* is what
// would be unsafe, and the caller prevents it by handing each workspace to one
// thread at a time.
unsafe impl Send for FmetricsWorkspace {}
// SAFETY: as above.
unsafe impl Sync for FmetricsWorkspace {}

/// An opaque CVVDP context.
///
/// One context accumulates across every frame it sees, so a clip must be scored
/// through a single context in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct FmetricsCvvdpCtx(pub *mut c_void);

impl FmetricsCvvdpCtx {
    /// Whether this is the null handle, meaning no context exists yet.
    #[inline]
    #[must_use]
    pub fn is_null(self) -> bool {
        self.0.is_null()
    }
}

// SAFETY: as for `FmetricsWorkspace`. Moving a context moves exclusive
// ownership of it, so no second thread observes the same accumulator.
unsafe impl Send for FmetricsCvvdpCtx {}
// SAFETY: as above. Nothing shares a context; `submit_pair_temporal` takes
// `&mut`.
unsafe impl Sync for FmetricsCvvdpCtx {}

/// Result code returned by every fallible fmetrics entry point.
/// Every integer is valid here: C may return an undocumented status.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct FmetricsErr(pub c_int);

#[allow(non_upper_case_globals, reason = "retain the documented status names")]
impl FmetricsErr {
    /// Success.
    pub const Ok: Self = Self(0);
    /// A caller-supplied argument was invalid.
    pub const InvalidArgument: Self = Self(1);
    /// The pixel format or colorspace is not supported.
    pub const UnsupportedFormat: Self = Self(2);
    /// Reference and distorted dimensions differ.
    pub const DimensionMismatch: Self = Self(3);
    /// An allocation failed.
    pub const OutOfMemory: Self = Self(4);
    /// The image is too small for five-scale IW-SSIM.
    pub const IwssimImgTooSmall: Self = Self(5);
    /// An internal error.
    pub const Internal: Self = Self(6);
}

impl FmetricsErr {
    /// Whether the call succeeded.
    #[inline]
    #[must_use]
    pub const fn is_ok(self) -> bool {
        matches!(self, Self::Ok)
    }

    /// Preserve an arbitrary native status without normalizing unknown codes.
    #[inline]
    #[must_use]
    pub const fn from_raw(value: c_int) -> Self {
        Self(value)
    }
}

impl std::fmt::Debug for FmetricsErr {
    #[inline]
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            0 => formatter.write_str("Ok"),
            1 => formatter.write_str("InvalidArgument"),
            2 => formatter.write_str("UnsupportedFormat"),
            3 => formatter.write_str("DimensionMismatch"),
            4 => formatter.write_str("OutOfMemory"),
            5 => formatter.write_str("IwssimImgTooSmall"),
            6 => formatter.write_str("Internal"),
            raw => write!(formatter, "Unknown({raw})"),
        }
    }
}

/// Pixel format of an [`FmetricsImg`].
///
/// The discriminants are **not** contiguous: `RGB_UINT8` is 1 and
/// `RGB_FLOAT` is 2, while `RGB_UINT16` is 3.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FmetricsPixFmt {
    /// 8-bit unsigned integer samples.
    RgbUint8 = 1,
    /// 32-bit float samples.
    RgbFloat = 2,
    /// Native-endian 16-bit unsigned integer samples, aligned for uint16_t reads.
    RgbUint16 = 3,
}

/// Colorspace of an [`FmetricsImg`].
///
/// There is no wide-gamut or HDR transfer function here: only sRGB and its
/// linearised form, plus the separate [`FmetricsImg::hdr`] flag.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FmetricsColorspace {
    /// sRGB, with its transfer function applied.
    Srgb = 1,
    /// Linear sRGB, with the transfer function removed.
    LinearSrgb = 2,
}

/// One image: interleaved RGB in a single buffer.
///
/// An unvalidated raw ABI record, not a lifetime-enforced safe image. The unsafe
/// C caller must ensure readable live storage for the entire call. Safe Rust
/// images return [`crate::BorrowedImg`] instead; the C layout is unchanged.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FmetricsImg {
    /// Pointer to the first sample. Interleaved, so three consecutive samples
    /// are R, G and B.
    pub data: *const c_void,
    /// Width in pixels, not bytes.
    pub width: c_uint,
    /// Height in pixels.
    pub height: c_uint,
    /// Bytes from one row to the next.
    pub stride: c_uint,
    /// Sample format.
    pub format: FmetricsPixFmt,
    /// Colorspace the samples are in.
    pub colorspace: FmetricsColorspace,
    /// Whether the samples carry HDR values.
    pub hdr: bool,
}

// SAFETY: moving/sharing this raw record never dereferences its pointer. Native
// reads are unsafe and require separate storage/lifetime/aliasing guarantees.
unsafe impl Sync for FmetricsImg {}
// SAFETY: as above.
unsafe impl Send for FmetricsImg {}

impl FmetricsImg {
    /// An 8-bit sRGB image.
    #[inline]
    #[must_use]
    pub const fn rgb8(
        data: *const c_void,
        width: c_uint,
        height: c_uint,
        stride: c_uint,
        colorspace: FmetricsColorspace,
        hdr: bool,
    ) -> Self {
        Self {
            data,
            width,
            height,
            stride,
            format: FmetricsPixFmt::RgbUint8,
            colorspace,
            hdr,
        }
    }

    /// A 16-bit sRGB image.
    ///
    /// 10- and 12-bit sources are up-converted to this rather than truncated
    /// to 8 bits, which would discard precision the source actually has.
    /// The unsafe C caller must provide native-endian u16 storage with aligned rows;
    /// this raw descriptor constructor does not validate the storage.
    #[inline]
    #[must_use]
    pub const fn rgb16(
        data: *const c_void,
        width: c_uint,
        height: c_uint,
        stride: c_uint,
        colorspace: FmetricsColorspace,
        hdr: bool,
    ) -> Self {
        Self {
            data,
            width,
            height,
            stride,
            format: FmetricsPixFmt::RgbUint16,
            colorspace,
            hdr,
        }
    }
}

/// Options for the Butteraugli metric.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FmetricsButteraugliOptions {
    /// Target intensity, in unspecified units.
    pub intensity_target: f32,
    /// The finite p-norm exponent. No value represents infinity.
    pub pnorm: c_int,
}

impl Default for FmetricsButteraugliOptions {
    /// Finite p=3 at intensity 203, not the application's/plugin's infinity norm.
    #[inline]
    fn default() -> Self {
        Self {
            intensity_target: 203.0,
            pnorm: crate::config::DEFAULT_PNORM,
        }
    }
}

/// Display model for CVVDP.
///
/// HDR display variants describe the display, not an input PQ/HLG decoder.
/// Input samples must still use a supported sRGB/linear-sRGB encoding and gamut.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FmetricsCvvdpDisplayModel {
    /// Standard 1080p SDR.
    StandardFhd = 0,
    /// Standard 4K SDR.
    Standard4K = 1,
    /// Standard HDR PQ.
    StandardHdrPq = 2,
    /// Standard HDR HLG.
    StandardHdrHlg = 3,
    /// Standard HDR linear.
    StandardHdrLinear = 4,
    /// Standard HDR dark environment.
    StandardHdrDark = 5,
    /// Standard HDR linear, zoomed.
    StandardHdrLinearZoom = 6,
    /// Head-mounted display.
    Hmd = 7,
    /// Phone.
    Phone = 8,
    /// SDR 4K at 30 fps.
    Sdr4K30 = 9,
    /// SDR 1080p at 24 fps.
    SdrFhd24 = 10,
    /// HTC Vive Pro.
    HtcVivePro = 11,
    /// iPhone 12 Pro.
    Iphone12Pro = 12,
    /// iPhone 14 Pro.
    Iphone14Pro = 13,
    /// iPhone 14 Pro, vertical.
    Iphone14ProVert = 14,
    /// iPhone 14 Pro HDR.
    Iphone14ProHdr = 15,
    /// iPhone 14 Pro HDR, vertical.
    Iphone14ProHdrVert = 16,
    /// iPad Pro 12.9.
    IpadPro129 = 17,
    /// MacBook Pro 16.
    MacbookPro16 = 18,
    /// LG OLED 2017, SDR.
    LgOled2017Sdr = 19,
    /// LG OLED 2017, HDR.
    LgOled2017Hdr = 20,
    /// EIZO CG3146.
    EizoCg3146 = 21,
    /// 65-inch HDR PQ 4K nit.
    Display65InchHdrPq4KNit = 22,
    /// 65-inch HDR PQ 2K nit.
    Display65InchHdrPq2KNit = 23,
    /// 65-inch HDR PQ 1K nit.
    Display65InchHdrPq1KNit = 24,
    /// LG OLED 2026, HDR PQ.
    LgOled2026HdrPq = 25,
    /// CID22 mCOS.
    Cid22Mcos = 26,
    /// A caller-supplied parameter set.
    Custom = 27,
}

/// Physical display parameters, for [`FmetricsCvvdpDisplayModel::Custom`].
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FmetricsCvvdpDisplayParams {
    /// Panel resolution width.
    pub resolution_width: c_int,
    /// Panel resolution height.
    pub resolution_height: c_int,
    /// Viewing distance in metres.
    pub viewing_distance_meters: f32,
    /// Diagonal size in inches.
    pub diagonal_size_inches: f32,
    /// Peak luminance.
    pub max_luminance: f32,
    /// Contrast ratio.
    pub contrast: f32,
    /// Ambient light.
    pub ambient_light: f32,
    /// Reflectivity.
    pub reflectivity: f32,
    /// Whether the display is HDR.
    pub is_hdr: bool,
}

/// A CVVDP score.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct FmetricsCvvdpResult {
    /// Just-objective-difference score, on a JOD-like scale.
    pub jod: f64,
    /// A secondary quality measure on a different scale.
    ///
    /// This is not interchangeable with [`Self::jod`]; the two are reported
    /// separately so a caller can choose, rather than silently substituting
    /// one.
    pub quality: f64,
}

/// Reports the library version, for example `"0.1.0"`.
pub type VersionStr = unsafe extern "C" fn() -> *const c_char;

/// Expands an error code into its message.
pub type ErrorStr = unsafe extern "C" fn(error: FmetricsErr) -> *const c_char;

/// Allocates a reusable workspace.
pub type WorkspaceCreate = unsafe extern "C" fn() -> FmetricsWorkspace;

/// Releases a workspace.
pub type WorkspaceDestroy = unsafe extern "C" fn(workspace: FmetricsWorkspace);

/// Scores one pair with IW-SSIM.
pub type IwssimCmp = unsafe extern "C" fn(
    workspace: FmetricsWorkspace,
    reference: *const FmetricsImg,
    distorted: *const FmetricsImg,
    result: *mut f64,
) -> FmetricsErr;

/// Scores one pair with MS-SSIM.
pub type MsssimCmp = unsafe extern "C" fn(
    workspace: FmetricsWorkspace,
    reference: *const FmetricsImg,
    distorted: *const FmetricsImg,
    result: *mut f64,
) -> FmetricsErr;

/// Scores one pair with SSIMULACRA2.
pub type Ssimu2Cmp = unsafe extern "C" fn(
    workspace: FmetricsWorkspace,
    reference: *const FmetricsImg,
    distorted: *const FmetricsImg,
    result: *mut f64,
) -> FmetricsErr;

/// Scores one pair with SSIMULACRA2, writing a per-pixel error map.
pub type Ssimu2CmpMap = unsafe extern "C" fn(
    workspace: FmetricsWorkspace,
    reference: *const FmetricsImg,
    distorted: *const FmetricsImg,
    result: *mut f64,
    error_map: *mut c_uint,
) -> FmetricsErr;

/// Scores one pair with Butteraugli.
pub type ButteraugliCmp = unsafe extern "C" fn(
    workspace: FmetricsWorkspace,
    reference: *const FmetricsImg,
    distorted: *const FmetricsImg,
    options: *const FmetricsButteraugliOptions,
    result: *mut f64,
) -> FmetricsErr;

/// Scores one pair with Butteraugli, writing a per-pixel error map.
pub type ButteraugliCmpMap = unsafe extern "C" fn(
    workspace: FmetricsWorkspace,
    reference: *const FmetricsImg,
    distorted: *const FmetricsImg,
    options: *const FmetricsButteraugliOptions,
    result: *mut f64,
    error_map: *mut c_uint,
) -> FmetricsErr;

/// Creates a stateful CVVDP context.
///
/// `fps` is **required** and must be the true frame rate: CVVDP's temporal
/// filter is parameterised by it, and a rate of zero silently accumulates
/// error rather than failing.
pub type CvvdpCreate = unsafe extern "C" fn(
    width: c_int,
    height: c_int,
    fps: f32,
    display_model: FmetricsCvvdpDisplayModel,
    threads: c_uint,
    custom_params: *const FmetricsCvvdpDisplayParams,
    out_context: *mut FmetricsCvvdpCtx,
) -> FmetricsErr;

/// Destroys a CVVDP context.
pub type CvvdpDestroy = unsafe extern "C" fn(context: FmetricsCvvdpCtx);

/// Scores one frame pair through a CVVDP context.
pub type CvvdpProcessFrame = unsafe extern "C" fn(
    context: FmetricsCvvdpCtx,
    reference: *const FmetricsImg,
    distorted: *const FmetricsImg,
    result: *mut FmetricsCvvdpResult,
) -> FmetricsErr;

/// Clears CVVDP temporal history AND accumulated score (no history-only API).
pub type CvvdpReset = unsafe extern "C" fn(context: FmetricsCvvdpCtx) -> FmetricsErr;

/// Scores one pair with CVVDP in a single call.
///
/// Convenient for tests; it creates and destroys a context internally, so it
/// carries no history between calls.
pub type CvvdpCmp = unsafe extern "C" fn(
    reference: *const FmetricsImg,
    distorted: *const FmetricsImg,
    display_model: FmetricsCvvdpDisplayModel,
    threads: c_uint,
    custom_params: *const FmetricsCvvdpDisplayParams,
    result: *mut FmetricsCvvdpResult,
) -> FmetricsErr;

/// Reads a display model's physical parameters.
pub type CvvdpGetDisplayParams = unsafe extern "C" fn(
    model: FmetricsCvvdpDisplayModel,
    out_params: *mut FmetricsCvvdpDisplayParams,
) -> FmetricsErr;

/// Reports the CVVDP implementation version.
pub type CvvdpVersionStr = unsafe extern "C" fn() -> *const c_char;

/// The resolved fmetrics entry points.
///
/// This is a table of function pointers and nothing else, so it is `Copy` and
/// can be shared freely. The `Library` it was resolved from is leaked by
/// [`FmetricsApi::load`], so the pointers stay valid for the process lifetime.
#[derive(Clone, Copy)]
pub struct FmetricsApi {
    /// Reports the library version.
    pub version_str: VersionStr,
    /// Expands an error code into its message.
    pub error_str: ErrorStr,
    /// Allocates a reusable workspace.
    pub workspace_create: WorkspaceCreate,
    /// Releases a workspace.
    pub workspace_destroy: WorkspaceDestroy,
    /// Scores one pair with IW-SSIM.
    pub iwssim_cmp: IwssimCmp,
    /// Scores one pair with MS-SSIM.
    pub msssim_cmp: MsssimCmp,
    /// Scores one pair with SSIMULACRA2.
    pub ssimu2_cmp: Ssimu2Cmp,
    /// Scores one pair with SSIMULACRA2, writing an error map.
    pub ssimu2_cmp_map: Ssimu2CmpMap,
    /// Scores one pair with Butteraugli.
    pub butteraugli_cmp: ButteraugliCmp,
    /// Scores one pair with Butteraugli, writing an error map.
    pub butteraugli_cmp_map: ButteraugliCmpMap,
    /// Creates a stateful CVVDP context.
    pub cvvdp_create: CvvdpCreate,
    /// Destroys a CVVDP context.
    pub cvvdp_destroy: CvvdpDestroy,
    /// Scores one frame pair through a CVVDP context.
    pub cvvdp_process_frame: CvvdpProcessFrame,
    /// Clears a CVVDP context's temporal history AND accumulated score.
    pub cvvdp_reset: CvvdpReset,
    /// Scores one pair with CVVDP in a single call.
    pub cvvdp_cmp: CvvdpCmp,
    /// Reads a display model's parameters.
    pub cvvdp_get_display_params: CvvdpGetDisplayParams,
    /// Reports the CVVDP implementation version.
    pub cvvdp_version_str: CvvdpVersionStr,
}

impl FmetricsApi {
    /// The fmetrics version string.
    #[inline]
    #[must_use]
    pub fn version_string(&self) -> String {
        // SAFETY: `version_str` takes no arguments and returns a pointer to a
        // static string literal owned by the library, which stays mapped for
        // the life of the process.
        unsafe { cstr_to_string((self.version_str)()) }
    }

    /// The CVVDP implementation version string.
    ///
    /// This is the version of the underlying `fcvvdp`, which is not necessarily
    /// the same as the library version.
    #[inline]
    #[must_use]
    pub fn cvvdp_version_string(&self) -> String {
        // SAFETY: as `version_string`.
        unsafe { cstr_to_string((self.cvvdp_version_str)()) }
    }

    /// Resolves the fmetrics entry points, or reports why it could not.
    ///
    /// The result is cached: the library is opened at most once per process.
    #[inline]
    pub fn load() -> Result<&'static Self, FmetricsError> {
        static API: OnceLock<Result<FmetricsApi, std::io::Error>> = OnceLock::new();

        match API.get_or_init(resolve) {
            Ok(api) => Ok(api),
            // `error` is a plain data value here, not a borrowed one.
            Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
                Err(FmetricsError::IncompatibleLibrary {
                    reason: error.to_string(),
                })
            },
            Err(error) => Err(FmetricsError::LibraryNotFound {
                reason: format!(
                    "{error}; tried {}",
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

/// Copy a C string the library owns into an owned [`String`].
///
/// # Safety
///
/// `pointer` must be null or a valid NUL-terminated string that stays valid
/// for the duration of the call.
#[inline]
unsafe fn cstr_to_string(pointer: *const c_char) -> String {
    if pointer.is_null() {
        return String::new();
    }
    // SAFETY: delegated to the caller; `CStr` handles the length and
    // UTF-8 validation, falling back lossily rather than panicking.
    unsafe { std::ffi::CStr::from_ptr(pointer) }.to_string_lossy().into_owned()
}

/// Check an [`FmetricsErr`], mapping a failure to
/// [`FmetricsError::CallFailed`].
#[inline]
pub fn check(error: FmetricsErr, function: &'static str) -> Result<(), FmetricsError> {
    if error.is_ok() {
        Ok(())
    } else {
        Err(FmetricsError::CallFailed {
            function,
            code: error.0,
            message: error_message(error),
        })
    }
}

/// Candidate library file names for the host platform.
#[inline]
#[must_use]
pub fn library_names() -> &'static [&'static str] {
    if cfg!(target_os = "windows") {
        &["fmetrics.dll", "libfmetrics.dll"]
    } else if cfg!(target_os = "macos") {
        &["libfmetrics.dylib", "libfmetrics.0.dylib"]
    } else {
        &["libfmetrics.so", "libfmetrics.so.0"]
    }
}

/// Read an environment variable as a non-empty path.
#[inline]
fn env_directory(variable: &str) -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let value = std::env::var_os(variable)?;
        match windows_directory_override(&value) {
            Ok(directory) => directory,
            Err(error) => {
                tracing::warn!(%error, variable, "ignoring invalid library directory");
                None
            },
        }
    }
    #[cfg(not(target_os = "windows"))]
    std::env::var(variable)
        .ok()
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// The message fmetrics associates with an error code.
///
/// The library is needed to expand a code, so this returns a placeholder when
/// it is unavailable rather than failing: this is used to build an error
/// message, and recursing back into a load failure here would be unhelpful.
#[inline]
#[must_use]
pub fn error_message(error: FmetricsErr) -> String {
    // Unknown codes need no native interpretation (and retain their raw value).
    if !(0..=6).contains(&error.0) {
        return format!("unknown fmetrics error {}", error.0);
    }
    FmetricsApi::load().map_or_else(
        |_| format!("(fmetrics error {error:?}; the library is not loaded)"),
        |api| {
            // SAFETY: `error_str` returns a pointer to a static string literal
            // owned by the library, which stays mapped for the life of the
            // process because the `Library` is leaked on load.
            unsafe { cstr_to_string((api.error_str)(error)) }
        },
    )
}

/// Candidate paths to load fmetrics from, most specific first.
///
/// The order is: explicit environment overrides, the executable's own
/// directory, then platform system directories, then bare names so the loader's
/// own search path applies.
#[inline]
#[must_use]
pub fn library_candidates() -> Vec<PathBuf> {
    let names = library_names();

    let mut candidates = Vec::with_capacity(names.len() * 6);

    // 1. The user's explicit overrides.
    for variable in ["FMETRICS_LIB_PATH", "FMETRICS_LIB_DIR"] {
        if let Some(directory) = env_directory(variable) {
            candidates.extend(names.iter().map(|name| directory.join(name)));
        }
    }

    // 2. Beside the running executable, where a staged release puts it and
    //    which is on no loader search path.
    if let Ok(exe) = std::env::current_exe()
        && let Some(directory) = exe.parent()
    {
        candidates.extend(names.iter().map(|name| directory.join(name)));
    }

    // 3. Platform system directories.
    for directory in system_directories() {
        candidates.extend(names.iter().map(|name| directory.join(name)));
    }

    // 4. Bare names, so the platform loader's own search path applies. A
    //    parentless candidate is the signal that the loader, not this crate,
    //    decides where the library comes from.
    candidates.extend(names.iter().map(PathBuf::from));

    candidates
}

/// Directories a system-wide install would use on this host.
///
/// On Windows these are asked of the OS rather than written down. `C:\Windows`
/// is not a safe constant: Windows can be installed to another drive or
/// relocated, and a hardcoded path then names a directory that does not exist.
/// `GetSystemDirectoryW` and its WOW64 counterpart report what this process can
/// actually load from, which also gets the WoW64 redirection right without
/// reasoning about the process bitness here.
#[inline]
#[must_use]
fn system_directories() -> Vec<PathBuf> {
    if cfg!(target_os = "windows") {
        // Native System32 first, then the WOW64 directory. A 32-bit process
        // asking for System32 is redirected to SysWOW64, which is the
        // directory it can really load from, so both entry points are
        // worth asking.
        let mut directories = Vec::with_capacity(2);
        directories.extend(windows_system_directory(true));
        directories.extend(windows_system_directory(false));
        directories.extend(windows_directory("SysWOW64"));
        directories.dedup();

        return directories;
    }

    if cfg!(target_os = "macos") {
        return ["/opt/homebrew", "/usr/local", "/opt/local"]
            .into_iter()
            .map(|prefix| PathBuf::from(prefix).join("lib"))
            .collect();
    }

    ["/usr/lib", "/usr/lib64", "/usr/local/lib", "/lib", "/lib64"]
        .into_iter()
        .map(PathBuf::from)
        .collect()
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

/// Nothing to ask on other platforms.
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

/// Nothing to ask on other platforms.
#[cfg(not(target_os = "windows"))]
#[inline]
#[must_use]
fn windows_directory(_subdirectory: &str) -> Option<PathBuf> {
    None
}

/// Resolve every entry point, leaking the [`Library`] so the code it points at
/// stays mapped.
fn resolve() -> Result<FmetricsApi, std::io::Error> {
    let mut failures = Vec::new();

    for candidate in library_candidates() {
        // SAFETY: every path handled here is either an explicit user override
        // or a conventional library location; loading a shared object
        // from a trusted path is the intended use of this crate.
        match unsafe { load_library(&candidate) } {
            Ok(library) => {
                // Leaked deliberately: dropping it would `dlclose` the library
                // and leave every pointer below dangling, so
                // the fault would appear at the first call
                // rather than here. There is nothing to reclaim at
                // exit anyway, since fmetrics is resolved once per process.
                // SAFETY: each `get` is checked against the signature
                // transcribed from `fmetrics.h`, and a missing symbol is an
                // error rather than a null pointer.
                let api: Result<FmetricsApi, std::io::Error> = unsafe {
                    Ok(FmetricsApi {
                        version_str: symbol(&library, b"fmetrics_version_str\0")?,
                        error_str: symbol(&library, b"fmetrics_error_str\0")?,
                        workspace_create: symbol(&library, b"fmetrics_workspace_create\0")?,
                        workspace_destroy: symbol(&library, b"fmetrics_workspace_destroy\0")?,
                        iwssim_cmp: symbol(&library, b"fmetrics_iwssim_cmp\0")?,
                        msssim_cmp: symbol(&library, b"fmetrics_msssim_cmp\0")?,
                        ssimu2_cmp: symbol(&library, b"fmetrics_ssimu2_cmp\0")?,
                        ssimu2_cmp_map: symbol(&library, b"fmetrics_ssimu2_cmp_map\0")?,
                        butteraugli_cmp: symbol(&library, b"fmetrics_butteraugli_cmp\0")?,
                        butteraugli_cmp_map: symbol(&library, b"fmetrics_butteraugli_cmp_map\0")?,
                        cvvdp_create: symbol(&library, b"fmetrics_cvvdp_create\0")?,
                        cvvdp_destroy: symbol(&library, b"fmetrics_cvvdp_destroy\0")?,
                        cvvdp_process_frame: symbol(&library, b"fmetrics_cvvdp_process_frame\0")?,
                        cvvdp_reset: symbol(&library, b"fmetrics_cvvdp_reset\0")?,
                        cvvdp_cmp: symbol(&library, b"fmetrics_cvvdp_cmp\0")?,
                        cvvdp_get_display_params: symbol(
                            &library,
                            b"fmetrics_cvvdp_get_display_params\0",
                        )?,
                        cvvdp_version_str: symbol(&library, b"fmetrics_cvvdp_version_str\0")?,
                    })
                };

                // A missing symbol means this is not an fmetrics build, so try
                // the next candidate rather than reporting a partial API.
                match api {
                    Ok(api) => {
                        // Admission must not recurse into check/error_message while
                        // initializing the same OnceLock. No workspace exists yet.
                        // SAFETY: the version entry point returns a static C string.
                        let pointer = unsafe { (api.version_str)() };
                        let version = if pointer.is_null() {
                            "<null>".to_owned()
                        } else {
                            // SAFETY: pointer is non-null, static and NUL-terminated.
                            unsafe { std::ffi::CStr::from_ptr(pointer) }
                                .to_str()
                                .map_or_else(|_| "<non-UTF8>".to_owned(), str::to_owned)
                        };
                        if version != "0.1.0" {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                format!(
                                    "unsupported fmetrics version {version:?}; rebuild pinned 0.1.0 (ae87c8e5607063f0dcc8241b52cb143f6c9ad4ac)"
                                ),
                            ));
                        }
                        std::mem::forget(library);
                        return Ok(api);
                    },
                    Err(error) => {
                        // `library` drops here, unloading the bad candidate.
                        failures.push(format!("{}: {error}", candidate.display()));
                    },
                }
            },
            Err(error) => failures.push(format!("{}: {error}", candidate.display())),
        }
    }

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

#[cfg(test)]
#[path = "../../test-support/loader_policy.rs"]
mod loader_policy_tests;

/// Resolve one symbol, naming it in the error if it is missing.
///
/// # Errors
///
/// Returns an error naming `symbol` when the loaded library does not export it,
/// which means the library is not the fmetrics build this crate was written
/// against.
///
/// # Safety
///
/// `T` must be the function-pointer type `fmetrics.h` declares for `symbol`.
#[inline]
unsafe fn symbol<T: Copy>(library: &Library, name: &'static [u8]) -> Result<T, std::io::Error> {
    // The names are NUL-terminated for the loader, but the NUL must not
    // appear in the error message.
    let printable = String::from_utf8_lossy(match name.split_last() {
        Some((last, rest)) if *last == 0 => rest,
        _ => name,
    });

    // SAFETY: delegated to the caller, who names the correct type per symbol.
    unsafe { library.get(name) }.map(|pointer| *pointer).map_err(|error| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("{error} (looking for {printable}; this may not be an fmetrics build)"),
        )
    })
}

/// Whether a path names an existing file.
#[inline]
#[must_use]
pub fn is_library_file(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_status_is_not_normalized_to_internal() {
        for raw in [777, -123] {
            assert_ne!(FmetricsErr::from_raw(raw), FmetricsErr::Internal);
            assert!(!FmetricsErr::from_raw(raw).is_ok());
            assert_eq!(FmetricsErr::from_raw(raw).0, raw);
        }
    }

    #[test]
    fn known_status_debug_names_remain_unchanged() {
        for (raw, name) in [
            "Ok",
            "InvalidArgument",
            "UnsupportedFormat",
            "DimensionMismatch",
            "OutOfMemory",
            "IwssimImgTooSmall",
            "Internal",
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(format!("{:?}", FmetricsErr::from_raw(raw as c_int)), name);
        }
    }

    #[test]
    fn error_codes_round_trip_through_raw() {
        for error in [
            FmetricsErr::Ok,
            FmetricsErr::InvalidArgument,
            FmetricsErr::UnsupportedFormat,
            FmetricsErr::DimensionMismatch,
            FmetricsErr::OutOfMemory,
            FmetricsErr::IwssimImgTooSmall,
            FmetricsErr::Internal,
        ] {
            assert_eq!(FmetricsErr::from_raw(error.0), error);
            assert_eq!(error.is_ok(), error == FmetricsErr::Ok);
        }
    }

    #[test]
    fn unknown_error_code_is_a_failure_not_success() {
        // Undocumented native failures retain their codes and never become success.
        assert_eq!(FmetricsErr::from_raw(99).0, 99);
        assert!(!FmetricsErr::from_raw(99).is_ok());
        assert!(!FmetricsErr::from_raw(-1).is_ok());
    }

    #[test]
    fn pixel_format_discriminants_match_documented_literal_values() {
        // Non-contiguous by design: RgbFloat is 2, RgbUint16 is 3.
        assert_eq!(FmetricsPixFmt::RgbUint8 as c_int, 1);
        assert_eq!(FmetricsPixFmt::RgbFloat as c_int, 2);
        assert_eq!(FmetricsPixFmt::RgbUint16 as c_int, 3);
    }

    #[test]
    fn display_model_discriminants_match_documented_literal_values() {
        assert_eq!(FmetricsCvvdpDisplayModel::StandardFhd as c_int, 0);
        assert_eq!(FmetricsCvvdpDisplayModel::Standard4K as c_int, 1);
        assert_eq!(FmetricsCvvdpDisplayModel::StandardHdrPq as c_int, 2);
        assert_eq!(FmetricsCvvdpDisplayModel::StandardHdrHlg as c_int, 3);
        assert_eq!(FmetricsCvvdpDisplayModel::StandardHdrLinear as c_int, 4);
        assert_eq!(FmetricsCvvdpDisplayModel::StandardHdrDark as c_int, 5);
        assert_eq!(FmetricsCvvdpDisplayModel::StandardHdrLinearZoom as c_int, 6);
    }

    #[test]
    fn hdr_display_models_are_representable() {
        // Display models have counterparts, but do not expand input transfer support.
        for model in [
            FmetricsCvvdpDisplayModel::StandardHdrPq,
            FmetricsCvvdpDisplayModel::StandardHdrHlg,
            FmetricsCvvdpDisplayModel::StandardHdrLinear,
            FmetricsCvvdpDisplayModel::StandardHdrDark,
            FmetricsCvvdpDisplayModel::StandardHdrLinearZoom,
        ] {
            assert!(matches!(model as c_int, 2..=6));
        }
    }

    #[test]
    fn image_constructors_set_the_expected_format() {
        let data = std::ptr::null();
        let eight = FmetricsImg::rgb8(data, 4, 4, 12, FmetricsColorspace::Srgb, false);
        assert_eq!(eight.format, FmetricsPixFmt::RgbUint8);
        assert_eq!(eight.stride, 12);

        let sixteen = FmetricsImg::rgb16(data, 4, 4, 24, FmetricsColorspace::Srgb, true);
        assert_eq!(sixteen.format, FmetricsPixFmt::RgbUint16);
        assert_eq!(sixteen.stride, 24);
        assert!(sixteen.hdr);
    }

    #[test]
    fn raw_and_config_butteraugli_defaults_use_pnorm_three_and_intensity_two_hundred_three() {
        let options = FmetricsButteraugliOptions::default();
        assert_eq!(options.pnorm, 3);
        assert!((options.intensity_target - 203.0).abs() < f32::EPSILON);
        let configured =
            crate::FmetricsConfig::new(crate::FmetricsMetric::Butteraugli).butteraugli_options();
        assert_eq!(configured.pnorm, 3);
        assert!((configured.intensity_target - 203.0).abs() < f32::EPSILON);
    }

    mod internal_integration {
        use super::*;

        #[test]
        fn reported_system_paths_are_absolute_and_windows_system_directory_exists() {
            // Windows can be installed to another drive or relocated, so the system
            // directory has to come from the OS. A hardcoded `C:\Windows` silently
            // names a directory that does not exist there, and the failure looks
            // like "fmetrics is not installed" rather than "the search path
            // was wrong".
            let directories = system_directories();

            if cfg!(target_os = "windows") {
                let system =
                    windows_system_directory(true).expect("Windows reports a system directory");
                assert!(system.is_absolute(), "{system:?} should be absolute");
                assert!(
                    system.components().count() > 1,
                    "{system:?} should name a drive and a directory, not a bare name"
                );

                // Whatever it is, it must exist: a path the OS invented would be
                // worse than the hardcoded one it replaced.
                assert!(
                    system.is_dir(),
                    "the reported system directory {system:?} does not exist"
                );
            }

            // No platform should contribute a path that does not exist, since every
            // entry here costs a failed `dlopen` attempt.
            for directory in &directories {
                assert!(directory.is_absolute(), "{directory:?} should be absolute");
            }
        }

        #[test]
        fn the_wow64_directory_is_offered_when_it_exists() {
            if !cfg!(target_os = "windows") {
                return;
            }

            // A 64-bit process cannot load from SysWOW64, so this only matters on a
            // 32-bit one -- but asking costs nothing and the answer is
            // authoritative.
            let directories = system_directories();
            if let Some(wow64) = windows_directory("SysWOW64")
                && wow64.is_dir()
            {
                assert!(
                    directories.contains(&wow64),
                    "an existing WOW64 directory must be among the candidates"
                );
            }
        }
    }

    #[test]
    fn library_names_offers_two_platform_aliases() {
        // A build may emit either `fmetrics.dll` or `libfmetrics.dll` on
        // Windows, depending on how the linker was invoked.
        let names = library_names();
        assert_eq!(names.len(), 2);
    }
}
