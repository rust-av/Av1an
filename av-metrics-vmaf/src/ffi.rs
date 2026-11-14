//! Hand-written declarations for the libvmaf C API.
//!
//! libvmaf is opened with [`libloading`] rather than linked, so the crate has
//! no build-time dependency on it and compiles on machines where it is absent.
//! No `bindgen` is used: the surface needed here is small and stable, and
//! declaring it by hand keeps the build free of a libclang dependency.
//!
//! Declarations target release v3.2.1 (f85a853692a8c730d0270cd733c8bb30b5b93b7c),
//! whose source archive reports runtime `3.2.0`. The loader rejects unverified
//! versions and known newer picture-ABI exports before any layout-sensitive call.
//! This bounded guard is not attestation of arbitrary modified binaries. Tests
//! compare layouts with real C headers; ordinary builds need neither headers nor C.
//!
//! # Loading strategy
//!
//! The [`Library`] is deliberately leaked by [`VmafApi::load`], so the code its
//! function pointers refer to stays mapped for the life of the process. Only
//! the pointers are kept in [`VmafApi`]; since raw `fn` pointers are `Send +
//! Sync`, the resolved API can then be cached in a `OnceLock` and shared
//! freely.

use std::{
    ffi::{c_char, c_int, c_uint, c_void},
    path::{Path, PathBuf},
    sync::OnceLock,
};

use libloading::Library;

use crate::error::VmafError;

/// Silence native stderr; checked native statuses still surface as Rust errors.
pub const LOG_LEVEL_NONE: c_int = 0;

/// Opaque libvmaf scoring context, allocated by `vmaf_init`.
#[repr(C)]
pub struct VmafContext {
    _opaque: [u8; 0],
}

/// Opaque handle to a loaded VMAF model.
#[repr(C)]
pub struct VmafModel {
    _opaque: [u8; 0],
}

/// Feature options dictionary, used to overload extractor parameters.
#[repr(C)]
pub struct VmafFeatureDictionary {
    _opaque: [u8; 0],
}

/// Opaque reference-picture handle referenced by [`VmafPicture::ref_`].
#[repr(C)]
pub struct VmafRef {
    _opaque: [u8; 0],
}

/// Pixel formats accepted by libvmaf.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmafPixelFormat {
    /// Unknown or unsupported.
    Unknown = 0,
    /// Planar YUV 4:2:0.
    Yuv420p = 1,
    /// Planar YUV 4:2:2.
    Yuv422p = 2,
    /// Planar YUV 4:4:4.
    Yuv444p = 3,
    /// Planar YUV 4:0:0, monochrome.
    Yuv400p = 4,
}

impl TryFrom<c_int> for VmafPixelFormat {
    type Error = VmafError;

    #[inline]
    fn try_from(raw: c_int) -> Result<Self, Self::Error> {
        match raw {
            0 => Ok(Self::Unknown),
            1 => Ok(Self::Yuv420p),
            2 => Ok(Self::Yuv422p),
            3 => Ok(Self::Yuv444p),
            4 => Ok(Self::Yuv400p),
            _ => Err(VmafError::UnsupportedFormat {
                reason: format!("libvmaf returned unknown pool pixel format {raw}"),
            }),
        }
    }
}

/// A single picture handed to libvmaf.
///
/// Pool-allocated `data`, `ref_` and `priv_` belong to a native refcounted
/// allocation, not borrowed caller planes. A successful `vmaf_read_pictures`
/// consumes the caller's reference even when it leaves this descriptor nonzero.
///
/// Layout mirrors `libvmaf/include/libvmaf/picture.h`. Note that `w` and `h`
/// are per-plane arrays rather than single scalars, `stride` is signed, `data`
/// is `void *`, and the trailing `priv` pointer is populated by native allocation.
#[repr(C)]
pub struct VmafPicture {
    /// Raw native pixel format; validate before any copy or submission.
    pub pix_fmt: c_int,
    /// Bits per channel: 8, 10, 12 or 16.
    pub bpc: c_uint,
    /// Width of each plane in pixels.
    pub w: [c_uint; 3],
    /// Height of each plane in pixels.
    pub h: [c_uint; 3],
    /// Byte stride of each plane.
    pub stride: [isize; 3],
    /// Pointers to the Y, U and V plane data.
    pub data: [*mut c_void; 3],
    /// Native reference-count handle for this allocation.
    pub ref_: *mut VmafRef,
    /// Native private allocation/release metadata; never infer ownership from it.
    pub priv_: *mut c_void,
}

impl VmafPicture {
    /// An all-zero picture, ready for libvmaf to populate.
    #[inline]
    #[must_use]
    pub const fn zeroed() -> Self {
        Self {
            pix_fmt: VmafPixelFormat::Unknown as c_int,
            bpc: 0,
            w: [0; 3],
            h: [0; 3],
            stride: [0; 3],
            data: [std::ptr::null_mut(); 3],
            ref_: std::ptr::null_mut(),
            priv_: std::ptr::null_mut(),
        }
    }
}

/// Global libvmaf configuration passed to `vmaf_init`.
///
/// Passed by value across the FFI boundary, so `Copy` is derived: the struct
/// holds only integers, which makes copying sound.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VmafConfiguration {
    /// Log verbosity.
    pub log_level: c_int,
    /// Worker thread count. 0 lets libvmaf choose.
    pub n_threads: c_uint,
    /// Score every n-th frame. 1 scores every frame.
    pub n_subsample: c_uint,
    /// CPU feature mask to disable.
    pub cpumask: u64,
    /// GPU feature mask to disable.
    pub gpumask: u64,
}

impl Default for VmafConfiguration {
    #[inline]
    fn default() -> Self {
        Self {
            log_level: LOG_LEVEL_NONE,
            n_threads: 0,
            n_subsample: 1,
            cpumask: 0,
            gpumask: 0,
        }
    }
}

/// A model and its configuration, passed to `vmaf_model_load`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VmafModelConfig {
    /// Model name override, NUL-terminated. Null when loading by version or
    /// path.
    pub name: *const c_char,
    /// Bitfield of `VmafModelFlags`.
    pub flags: u64,
}

impl VmafModelConfig {
    /// A default configuration with no name override.
    #[inline]
    #[must_use]
    pub const fn new() -> Self {
        Self {
            name: std::ptr::null(),
            flags: 0,
        }
    }
}

impl Default for VmafModelConfig {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

/// Opaque CUDA state, allocated by `vmaf_cuda_state_init`.
#[repr(C)]
pub struct VmafCudaState {
    _opaque: [u8; 0],
}

/// CUDA state configuration.
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct VmafCudaConfiguration {
    /// A `CUcontext` to adopt, or null to let libvmaf create one.
    pub cu_ctx: *mut c_void,
}

/// How libvmaf should preallocate picture buffers.
///
/// Only [`Self::Device`] is used today; the rest mirror the C enum so the
/// configuration can express any strategy libvmaf supports.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code, reason = "variants mirror the C enum for FFI fidelity")]
pub enum VmafCudaPicturePreallocationMethod {
    /// Do not preallocate.
    None = 0,
    /// Preallocate on the device.
    Device = 1,
    /// Preallocate in pageable host memory.
    Host = 2,
    /// Preallocate in pinned host memory.
    HostPinned = 3,
}

/// Picture geometry for CUDA preallocation.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VmafCudaPictureConfigPictureParams {
    /// Width in pixels.
    pub w: c_uint,
    /// Height in pixels.
    pub h: c_uint,
    /// Bits per channel.
    pub bpc: c_uint,
    /// Pixel format.
    pub pix_fmt: VmafPixelFormat,
}

/// CUDA picture preallocation configuration.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VmafCudaPictureConfiguration {
    /// Geometry to preallocate for.
    pub pic_params: VmafCudaPictureConfigPictureParams,
    /// Allocation strategy.
    pub pic_prealloc_method: VmafCudaPicturePreallocationMethod,
}

type VmafInit = unsafe extern "C" fn(*mut *mut VmafContext, VmafConfiguration) -> c_int;
type VmafClose = unsafe extern "C" fn(*mut VmafContext) -> c_int;
type VmafReadPictures =
    unsafe extern "C" fn(*mut VmafContext, *mut VmafPicture, *mut VmafPicture, c_uint) -> c_int;
type VmafScoreAtIndex =
    unsafe extern "C" fn(*mut VmafContext, *mut VmafModel, *mut f64, c_uint) -> c_int;
type VmafVersion = unsafe extern "C" fn() -> *const c_char;
type VmafModelLoad =
    unsafe extern "C" fn(*mut *mut VmafModel, *mut VmafModelConfig, *const c_char) -> c_int;
type VmafModelDestroy = unsafe extern "C" fn(*mut VmafModel);
type VmafUseFeaturesFromModel = unsafe extern "C" fn(*mut VmafContext, *mut VmafModel) -> c_int;
type VmafUseFeature =
    unsafe extern "C" fn(*mut VmafContext, *const c_char, *mut VmafFeatureDictionary) -> c_int;
type CudaStateInit = unsafe extern "C" fn(*mut *mut VmafCudaState, VmafCudaConfiguration) -> c_int;
pub(crate) type CudaRelease = unsafe extern "C" fn(*mut VmafCudaState) -> c_int;
pub(crate) type CudaStateFree = unsafe extern "C" fn(*mut c_void);
type CudaImportState = unsafe extern "C" fn(*mut VmafContext, *mut VmafCudaState) -> c_int;
type CudaPreallocatePictures =
    unsafe extern "C" fn(*mut VmafContext, VmafCudaPictureConfiguration) -> c_int;

/// The set of CUDA entry points, each optional.
#[derive(Clone, Copy, Default)]
pub(crate) struct CudaSymbols {
    pub(crate) state_init: Option<CudaStateInit>,
    pub(crate) release: Option<CudaRelease>,
    /// Frees the original malloc wrapper, NOT its copied CUDA resources.
    pub(crate) free_state: Option<CudaStateFree>,
    pub(crate) import_state: Option<CudaImportState>,
    pub(crate) preallocate_pictures: Option<CudaPreallocatePictures>,
}

/// Resolved libvmaf entry points.
///
/// Contains only function pointers and a version string, so it is `Send + Sync`
/// and can be cached process-wide. The [`Library`] backing these pointers is
/// leaked by [`VmafApi::load`] and never unloaded.
#[derive(Clone, Copy)]
pub struct VmafApi {
    /// Runtime libvmaf version string.
    pub version: &'static str,

    /// Allocates and initialises a scoring context.
    pub init: VmafInit,
    /// Releases a scoring context.
    pub close: VmafClose,
    /// Submits and consumes a reference/distorted pair on success, or nulls to
    /// flush. Caller descriptors need not be zeroed after consumption.
    pub read_pictures: VmafReadPictures,
    /// Reads the model score for a frame index.
    pub score_at_index: VmafScoreAtIndex,
    /// Reads a named feature score pooled over a frame range.
    ///
    /// Feature extractors do not reliably expose a per-index score for every
    /// frame, so pooling is the dependable way to obtain one. See
    /// `vmaf_feature_score_pooled` in libvmaf.h.
    pub feature_score_pooled: unsafe extern "C" fn(
        *mut VmafContext,
        *const c_char,
        c_int,
        *mut f64,
        c_uint,
        c_uint,
    ) -> c_int,
    /// Loads a built-in model by version string, e.g. `vmaf_v0.6.1`.
    ///
    /// libvmaf rejects a filesystem path here with `EINVAL`; use
    /// [`Self::model_load_from_path`] for custom models.
    pub model_load: VmafModelLoad,
    /// Loads a model from a JSON file on disk.
    pub model_load_from_path: VmafModelLoad,
    /// Releases a model.
    pub model_destroy: VmafModelDestroy,
    /// Registers the feature extractors a model requires.
    pub use_features_from_model: VmafUseFeaturesFromModel,
    /// Registers an additional feature extractor by name.
    pub use_feature: VmafUseFeature,

    /// Releases a picture's buffers.
    pub picture_unref: unsafe extern "C" fn(*mut VmafPicture) -> c_int,
    /// Fetches a picture whose buffers libvmaf has sized for the context.
    pub fetch_preallocated_picture:
        unsafe extern "C" fn(*mut VmafContext, *mut VmafPicture) -> c_int,
    /// Configures the context's picture pool for a given geometry.
    pub preallocate_pictures:
        unsafe extern "C" fn(*mut VmafContext, VmafPictureConfiguration) -> c_int,

    /// The optional CUDA entry points.
    pub(crate) cuda: CudaSymbols,
}

/// Per-plane geometry for the picture pool.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VmafPictureConfigPictureParams {
    /// Width in pixels.
    pub w: c_uint,
    /// Height in pixels.
    pub h: c_uint,
    /// Bits per channel.
    pub bpc: c_uint,
    /// Pixel format.
    pub pix_fmt: VmafPixelFormat,
}

/// Picture pool configuration, passed to `vmaf_preallocate_pictures`.
///
/// Note the nesting: `libvmaf.c` reads `cfg.pic_params.w` and `cfg.pic_cnt`, so
/// this must not be flattened.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VmafPictureConfiguration {
    /// Geometry to allocate for.
    pub pic_params: VmafPictureConfigPictureParams,
    /// Number of pictures to keep in the pool.
    pub pic_cnt: c_uint,
}

impl VmafApi {
    /// Whether the loaded libvmaf exposes a usable CUDA interface.
    ///
    /// True only with the Cargo `cuda` feature and every required entry point.
    /// A libvmaf built without `-Denable_cuda=true` exports none of them.
    /// Cleanup also needs the process-default Linux allocator; other platforms
    /// remain unavailable until allocator-matched cleanup is verified.
    #[inline]
    #[must_use]
    pub const fn cuda_available(&self) -> bool {
        cfg!(feature = "cuda")
            && self.cuda.state_init.is_some()
            && self.cuda.release.is_some()
            && self.cuda.free_state.is_some()
            && self.cuda.import_state.is_some()
            && self.cuda.preallocate_pictures.is_some()
    }

    /// Load libvmaf and resolve the entry points the scorer needs.
    ///
    /// The `Library` is intentionally leaked: libvmaf has no meaningful global
    /// teardown, and the pointers stored in the returned [`VmafApi`] must
    /// remain valid for the life of the process.
    ///
    /// The result is cached, so repeated calls are cheap.
    ///
    /// # Errors
    ///
    /// Returns [`VmafError::LibraryLoadFailed`] when no candidate library could
    /// be opened, [`VmafError::IncompatibleLibrary`] for an unverified ABI, or
    /// [`VmafError::SymbolNotFound`] when a required entry point is missing.
    pub fn load() -> Result<&'static Self, VmafError> {
        static API: OnceLock<Result<VmafApi, VmafError>> = OnceLock::new();

        match API.get_or_init(resolve) {
            Ok(api) => Ok(api),
            Err(error) => Err(clone_error(error)),
        }
    }
}

/// Errors are cached, so hand back a fresh value rather than a shared
/// reference.
fn clone_error(error: &VmafError) -> VmafError {
    match error {
        VmafError::LibraryNotFound => VmafError::LibraryNotFound,
        VmafError::LibraryLoadFailed { attempts } => VmafError::LibraryLoadFailed {
            attempts: attempts.clone(),
        },
        VmafError::IncompatibleLibrary { version, reason } => VmafError::IncompatibleLibrary {
            version: version.clone(),
            reason: reason.clone(),
        },
        VmafError::SymbolNotFound { symbol, message } => VmafError::SymbolNotFound {
            symbol: symbol.clone(),
            message: message.clone(),
        },
        VmafError::CallFailed { function, status } => VmafError::CallFailed {
            function,
            status: *status,
        },
        VmafError::ScoreQueryFailed { index, status } => VmafError::ScoreQueryFailed {
            index: *index,
            status: *status,
        },
        VmafError::InvalidConfiguration { reason } => VmafError::InvalidConfiguration {
            reason: reason.clone(),
        },
        VmafError::ModelNotFound { model, reason } => VmafError::ModelNotFound {
            model: model.clone(),
            reason: reason.clone(),
        },
        VmafError::InputMismatch { reason } => VmafError::InputMismatch {
            reason: reason.clone(),
        },
        VmafError::UnsupportedFormat { reason } => VmafError::UnsupportedFormat {
            reason: reason.clone(),
        },
        VmafError::MalformedFrame { reason } => VmafError::MalformedFrame {
            reason: reason.clone(),
        },
        VmafError::FrameCountMismatch {
            reference,
            distorted,
        } => VmafError::FrameCountMismatch {
            reference: *reference,
            distorted: *distorted,
        },
        VmafError::UnsupportedFeature(feature) => VmafError::UnsupportedFeature(feature.clone()),
        VmafError::MissingScore { index } => VmafError::MissingScore { index: *index },
        VmafError::ModelFileMissing { path } => VmafError::ModelFileMissing { path: path.clone() },
        VmafError::Decode { message } => VmafError::Decode {
            message: message.clone(),
        },
    }
}

/// Check a libvmaf status code, mapping a failure to [`VmafError::CallFailed`].
#[inline]
pub fn check(status: c_int, function: &'static str) -> Result<(), VmafError> {
    if status == 0 {
        Ok(())
    } else {
        Err(VmafError::CallFailed { function, status })
    }
}

/// Resolve one of the candidate libvmaf libraries.
fn resolve() -> Result<VmafApi, VmafError> {
    let library = open_library()?;
    // Keep the mapping alive for the life of the process. The pointers resolved
    // below are only valid while it is loaded.
    let library = Box::leak(Box::new(library));

    // SAFETY: each `library.get` is checked for presence before its result is
    // dereferenced, and `T` is written to match the C signature of `name`. The
    // leaked `library` above keeps the code these point at mapped.
    unsafe {
        let version = {
            let vmaf_version: VmafVersion = required(library, b"vmaf_version\0")?;
            // Record where the module came from, so assets shipped beside the
            // library can be found without assuming an install prefix. The
            // symbol address is enough to identify the module.
            let _ =
                MODULE_DIRECTORY.set(windows_module_directory_from(vmaf_version as *const c_void));
            // SAFETY: `vmaf_version` returns a static NUL-terminated string
            // that lives as long as the library.
            let raw = vmaf_version();
            if raw.is_null() {
                return Err(VmafError::IncompatibleLibrary {
                    version: "<null>".to_owned(),
                    reason: "vmaf_version returned a null pointer".to_owned(),
                });
            }
            let bytes = std::ffi::CStr::from_ptr(raw);
            bytes.to_str().map_err(|_| VmafError::IncompatibleLibrary {
                version: format!("<non-UTF8 {:?}>", bytes.to_bytes()),
                reason: "vmaf_version is not UTF-8".to_owned(),
            })?
        };

        if version != "3.2.0" {
            return Err(VmafError::IncompatibleLibrary {
                version: version.to_owned(),
                reason: "version has no verified header mapping".to_owned(),
            });
        }
        // SECURITY: newer color layouts can also report 3.2.0. These exports
        // arrived with/after that layout (0497a0f2, 700124a4); probe presence
        // only, NEVER call an entry point with an unverified signature.
        for marker in [
            b"vmaf_picture_convert_context_init\0".as_slice(),
            b"vmaf_picture_convert\0",
            b"vmaf_picture_convert_context_close\0",
            b"vmaf_picture_wrap\0",
        ] {
            if library.get::<*const c_void>(marker).is_ok() {
                return Err(VmafError::IncompatibleLibrary {
                    version: version.to_owned(),
                    reason: format!(
                        "known incompatible picture ABI export `{}`",
                        symbol_name(marker)
                    ),
                });
            }
        }

        Ok(VmafApi {
            version,
            init: required(library, b"vmaf_init\0")?,
            close: required(library, b"vmaf_close\0")?,
            read_pictures: required(library, b"vmaf_read_pictures\0")?,
            score_at_index: required(library, b"vmaf_score_at_index\0")?,
            feature_score_pooled: required(library, b"vmaf_feature_score_pooled\0")?,
            model_load: required(library, b"vmaf_model_load\0")?,
            model_load_from_path: required(library, b"vmaf_model_load_from_path\0")?,
            model_destroy: required(library, b"vmaf_model_destroy\0")?,
            use_features_from_model: required(library, b"vmaf_use_features_from_model\0")?,
            use_feature: required(library, b"vmaf_use_feature\0")?,
            picture_unref: required(library, b"vmaf_picture_unref\0")?,
            fetch_preallocated_picture: required(library, b"vmaf_fetch_preallocated_picture\0")?,
            preallocate_pictures: required(library, b"vmaf_preallocate_pictures\0")?,
            cuda: CudaSymbols {
                state_init: optional(library, b"vmaf_cuda_state_init\0"),
                release: optional(library, b"vmaf_cuda_release\0"),
                free_state: cuda_state_free(),
                import_state: optional(library, b"vmaf_cuda_import_state\0"),
                preallocate_pictures: optional(library, b"vmaf_cuda_preallocate_pictures\0"),
            },
        })
    }
}

/// Ordinary Linux relocations bind process-global allocators first, including
/// LD_PRELOAD overrides. Handle-local lookup can select a different free.
/// Assumes no DEEPBIND or symbolic/private allocator binding in libvmaf.
#[cfg(target_os = "linux")]
fn cuda_state_free() -> Option<CudaStateFree> {
    static PROCESS: OnceLock<libloading::os::unix::Library> = OnceLock::new();
    let process = PROCESS.get_or_init(libloading::os::unix::Library::this);
    // SAFETY: free has this C signature; keep the process mapping for the
    // callback's lifetime, just as resolve retains the native library mapping.
    unsafe { process.get::<CudaStateFree>(b"free\0").ok().map(|symbol| *symbol) }
}

/// No allocator-matched deallocator has been verified on these platforms.
#[cfg(not(target_os = "linux"))]
fn cuda_state_free() -> Option<CudaStateFree> {
    None
}

/// Resolve a required symbol.
fn required<T: Copy>(library: &Library, name: &[u8]) -> Result<T, VmafError> {
    // SAFETY: the caller guarantees `T` matches the C signature for `name`.
    unsafe { library.get(name) }
        .map(|symbol| *symbol)
        .map_err(|error| VmafError::SymbolNotFound {
            symbol: symbol_name(name),
            message: error.to_string(),
        })
}

/// Resolve an optional symbol, yielding `None` when the build omits it.
fn optional<T: Copy>(library: &Library, name: &[u8]) -> Option<T> {
    // SAFETY: as `required`, but absence is tolerated.
    unsafe { library.get(name).ok().map(|symbol| *symbol) }
}

/// Decode a symbol name for diagnostics.
fn symbol_name(name: &[u8]) -> String {
    std::str::from_utf8(name).unwrap_or("unknown").trim_end_matches('\0').to_owned()
}

/// The directory the loaded libvmaf module was mapped from.
///
/// Asks the operating system rather than inferring it from the candidate list,
/// so it is correct however the library was found: an explicit `VMAF_LIB_DIR`,
/// the platform loader's search path, or an already-mapped module. That matters
/// on Windows, where a prefix such as `C:\msys64` must not be assumed.
///
/// Returns the recorded candidate directory on other platforms, where the
/// loader does not expose this.
#[inline]
#[must_use]
pub fn loaded_library_directory() -> Option<&'static Path> {
    MODULE_DIRECTORY
        .get()
        .and_then(|path| path.as_deref())
        .or_else(recorded_candidate_directory)
}

/// The directory a load candidate named, when the path was explicit.
static RECORDED_CANDIDATE_DIRECTORY: OnceLock<Option<PathBuf>> = OnceLock::new();

/// The directory the OS reports for the mapped libvmaf module.
static MODULE_DIRECTORY: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Record the candidate directory so a later lookup can fall back to it.
fn remember_candidate_directory(directory: Option<PathBuf>) {
    let _ = RECORDED_CANDIDATE_DIRECTORY.set(directory);
}

/// The explicit directory a load candidate named, if it named one.
fn recorded_candidate_directory() -> Option<&'static Path> {
    RECORDED_CANDIDATE_DIRECTORY.get().and_then(|path| path.as_deref())
}

/// The directory the module containing `address` was mapped from.
///
/// `address` is any code address inside libvmaf; the answer describes the
/// *loaded* module even when it was mapped under a different name or found on
/// the loader's search path.
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
    // and ask again.
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

/// Open the first libvmaf candidate that loads.
///
/// Tries `VMAF_LIB_DIR` first, then the bare file name so the platform loader
/// searches its own paths.
fn open_library() -> Result<Library, VmafError> {
    let mut failures = Vec::new();

    for candidate in library_candidates() {
        // SAFETY: loading an arbitrary shared library executes its
        // initialisers. This is the intended way to use libvmaf, and
        // the candidate paths are constrained to libvmaf file names.
        match unsafe { load_library(&candidate) } {
            Ok(library) => {
                // A bare file name says nothing about where the loader found
                // it, so only an explicit candidate directory is worth keeping.
                remember_candidate_directory(
                    candidate
                        .parent()
                        .filter(|parent| !parent.as_os_str().is_empty())
                        .map(Path::to_path_buf),
                );
                return Ok(library);
            },
            Err(error) => failures.push(format!("{}: {error}", candidate.display())),
        }
    }

    tracing::debug!(?failures, "no libvmaf candidate could be loaded");
    Err(library_load_error(&failures))
}

fn library_load_error(failures: &[String]) -> VmafError {
    VmafError::LibraryLoadFailed {
        attempts: failures.join("; "),
    }
}

/// Qualify deliberate paths, leaving only actual bare names to safe OS search.
/// The boolean selects sibling-dependency search, which requires a full path.
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

#[cfg(all(test, target_os = "windows"))]
#[path = "../../test-support/windows_loader.rs"]
mod windows_loader_tests;

/// Candidate libvmaf paths, most specific first.
fn library_candidates() -> Vec<PathBuf> {
    let names: &[&str] = if cfg!(target_os = "windows") {
        &["vmaf.dll", "libvmaf.dll"]
    } else if cfg!(target_os = "macos") {
        &["libvmaf.dylib", "libvmaf.3.dylib"]
    } else {
        &["libvmaf.so", "libvmaf.so.3"]
    };

    let mut candidates = Vec::with_capacity(names.len() * 2);

    #[cfg(target_os = "windows")]
    if let Some(value) = std::env::var_os("VMAF_LIB_DIR") {
        match windows_directory_override(&value) {
            Ok(Some(directory)) => candidates.extend(names.iter().map(|name| directory.join(name))),
            Ok(None) => {},
            Err(error) => tracing::warn!(%error, "ignoring invalid VMAF_LIB_DIR"),
        }
    }
    #[cfg(not(target_os = "windows"))]
    if let Ok(directory) = std::env::var("VMAF_LIB_DIR")
        && !directory.is_empty()
    {
        let directory = PathBuf::from(directory);
        candidates.extend(names.iter().map(|name| directory.join(name)));
    }

    candidates.extend(names.iter().map(PathBuf::from));

    candidates
}

#[cfg(test)]
mod tests;
