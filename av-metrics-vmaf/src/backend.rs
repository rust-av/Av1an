//! Backend selection: opt-in CUDA when it works, CPU otherwise.

use std::{
    ffi::c_int,
    fmt::{self, Display},
};

use crate::{
    config::BackendPreference,
    error::VmafError,
    ffi::{VmafApi, VmafCudaConfiguration, VmafCudaState},
};

/// The compute backend a scorer is using.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Backend {
    /// libvmaf's scalar and SIMD CPU extractors.
    #[default]
    Cpu,
    /// libvmaf's CUDA extractors.
    Cuda,
}

impl Display for Backend {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Cpu => "cpu",
            Self::Cuda => "cuda",
        })
    }
}

/// Geometry of the pictures a context will be fed.
///
/// Carried separately from [`VideoFormat`](crate::VideoFormat) so this module
/// does not depend on the scorer's types.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Geometry {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Bits per channel.
    pub bit_depth: u32,
    /// Pixel format.
    pub pixel_format: crate::ffi::VmafPixelFormat,
}

/// A libvmaf scoring context, tagged with the backend it was initialised for.
pub(crate) struct Context {
    /// The context pointer owned by libvmaf.
    pub(crate) raw: *mut crate::ffi::VmafContext,
    api: &'static VmafApi,
    /// The backend this context was created on.
    pub(crate) backend: Backend,
}

/// Owns the C allocation independently of the CUDA resources it describes.
/// Import transfers only resource cleanup to the native context's copied state.
struct CudaState {
    raw: *mut VmafCudaState,
    release: Option<crate::ffi::CudaRelease>,
    free: crate::ffi::CudaStateFree,
}

impl Drop for CudaState {
    fn drop(&mut self) {
        if !self.raw.is_null() {
            // SAFETY: state_init returns an owned, initialized allocation even
            // on returned errors. Only unimported states retain release duty.
            unsafe {
                if let Some(release) = self.release {
                    release(self.raw);
                }
                (self.free)(self.raw.cast());
            }
        }
    }
}

/// One successful pool checkout, until returned or transferred to libvmaf.
/// The enclosing scorer must drop this before closing its context/pool.
pub(crate) struct PooledPicture {
    pub(crate) raw: crate::ffi::VmafPicture,
    api: &'static VmafApi,
}

impl PooledPicture {
    /// Disarm after a successful `vmaf_read_pictures`, even if C left `raw` nonzero.
    pub(crate) fn transfer(self) {
        // Only the native reference needs cleanup; libvmaf now owns that cleanup.
        std::mem::forget(self);
    }
}

impl Drop for PooledPicture {
    fn drop(&mut self) {
        // SAFETY: only successful checkouts get guards, and transfer disarms
        // them. The enclosing owner keeps the API and context/pool alive.
        unsafe { (self.api.picture_unref)(raw_mut_picture(&mut self.raw)) };
    }
}

impl Drop for Context {
    #[inline]
    fn drop(&mut self) {
        if !self.raw.is_null() {
            // SAFETY: `raw` came from `vmaf_init` and is closed exactly once,
            // here, when the owning handle drops.
            unsafe { (self.api.close)(self.raw) };
        }
    }
}

impl Context {
    /// The resolved libvmaf API backing this context.
    #[inline]
    pub(crate) const fn api(&self) -> &'static VmafApi {
        self.api
    }

    /// Fetch a picture from libvmaf's pool for the configured geometry.
    ///
    /// The pool is sized during construction, so the returned picture's `data`,
    /// `w`, `h` and `stride` are authoritative and match the scorer's format.
    /// The returned guard releases it unless submission transfers ownership.
    /// Callers must drop all guards before closing this context: pool close can
    /// block until every outstanding owner returns its picture.
    ///
    /// # Errors
    ///
    /// Pool exhaustion can block until a picture returns. Allocation/setup
    /// failures return [`VmafError::CallFailed`].
    pub(crate) fn pooled_picture(&self) -> Result<PooledPicture, VmafError> {
        let mut picture = crate::ffi::VmafPicture::zeroed();
        // SAFETY: `picture` is a valid out-parameter and `raw` is a live
        // context whose pool was sized during construction.
        let status = unsafe {
            (self.api.fetch_preallocated_picture)(self.raw, raw_mut_picture(&mut picture))
        };
        // A failed fetch rolls its slot back but may leave dangling fields in
        // the output. Only successful fetches acquire an owning guard.
        crate::ffi::check(status, "vmaf_fetch_preallocated_picture")?;
        let picture = PooledPicture {
            raw: picture,
            api: self.api,
        };
        // Success owns a checkout even when its metadata is invalid. Adopt the
        // guard before validating so an error returns it before context close.
        let format = crate::ffi::VmafPixelFormat::try_from(picture.raw.pix_fmt)?;
        if !matches!(
            format,
            crate::ffi::VmafPixelFormat::Yuv420p
                | crate::ffi::VmafPixelFormat::Yuv422p
                | crate::ffi::VmafPixelFormat::Yuv444p
        ) {
            return Err(VmafError::UnsupportedFormat {
                reason: format!(
                    "unsupported libvmaf pool pixel format {}",
                    picture.raw.pix_fmt
                ),
            });
        }
        Ok(picture)
    }
}

/// Mutable pointer to a picture, for the C entry points that require one.
#[inline]
fn raw_mut_picture(picture: &mut crate::ffi::VmafPicture) -> *mut crate::ffi::VmafPicture {
    picture as *mut crate::ffi::VmafPicture
}

// SAFETY: `VmafContext` is only ever used from the single worker that owns the
// `Context`. libvmaf documents a context as not safe for concurrent use, and
// the scorer never shares one across threads.
unsafe impl Send for Context {}

impl Context {
    /// Create a CPU scoring context.
    ///
    /// # Safety
    ///
    /// The caller must ensure the `configuration`'s pointed-to strings and
    /// arrays stay valid for the duration of the call. libvmaf copies what
    /// it needs during `init`.
    /// Create a CPU scoring context and size its picture pool.
    fn new_cpu(
        api: &'static VmafApi,
        configuration: crate::ffi::VmafConfiguration,
        geometry: Geometry,
    ) -> Result<Self, VmafError> {
        let mut raw: *mut crate::ffi::VmafContext = std::ptr::null_mut();
        // SAFETY: `&mut raw` is a valid out-pointer of the expected type, and
        // `configuration` is passed by value with no borrowed pointers.
        let status = unsafe { (api.init)(&mut raw, configuration) };
        crate::ffi::check(status, "vmaf_init")?;

        if raw.is_null() {
            return Err(VmafError::CallFailed {
                function: "vmaf_init",
                status: 0,
            });
        }

        let context = Self {
            raw,
            api,
            backend: Backend::Cpu,
        };
        context.preallocate_host(geometry, configuration.n_threads)?;
        Ok(context)
    }

    /// The generic fetch/copy path requires a HOST pool, even with CUDA enabled.
    fn preallocate_host(&self, geometry: Geometry, n_threads: u32) -> Result<(), VmafError> {
        // The picture pool must be sized before
        // `vmaf_fetch_preallocated_picture` can hand out buffers;
        // without it the fetch fails. The size matters as much as the
        // existence: libvmaf's own auto-sizing (`n_threads * 2 + 2`,
        // see [`picture_pool_size`]) is skipped when a pool already exists
        // here, so undersizing it would starve the thread pool and
        // serialise extraction regardless of `n_threads`.
        // SAFETY: `raw` is a live context and `geometry` describes the frames
        // that will be submitted.
        let status = unsafe {
            (self.api.preallocate_pictures)(
                self.raw,
                crate::ffi::VmafPictureConfiguration {
                    pic_params: crate::ffi::VmafPictureConfigPictureParams {
                        w: geometry.width,
                        h: geometry.height,
                        bpc: geometry.bit_depth,
                        pix_fmt: geometry.pixel_format,
                    },
                    pic_cnt: picture_pool_size(n_threads),
                },
            )
        };
        crate::ffi::check(status, "vmaf_preallocate_pictures")
    }

    /// Create a CUDA scoring context.
    ///
    /// Import CUDA state, then configure separate DEVICE and HOST pools.
    /// Model/feature compatibility is checked later, within the same attempt.
    fn new_cuda(
        api: &'static VmafApi,
        mut configuration: crate::ffi::VmafConfiguration,
        geometry: Geometry,
    ) -> Result<Self, VmafError> {
        let Some(state_init) = api.cuda.state_init else {
            return Err(VmafError::CallFailed {
                function: "vmaf_cuda_state_init",
                status: -1,
            });
        };
        let release = api.cuda.release.ok_or_else(|| VmafError::SymbolNotFound {
            symbol: "vmaf_cuda_release".to_owned(),
            message: "CUDA requires vmaf_cuda_release before allocating state; use CpuOnly or a compatible libvmaf build".to_owned(),
        })?;
        let free = api.cuda.free_state.ok_or_else(|| VmafError::SymbolNotFound {
            symbol: "free".to_owned(),
            message: "CUDA requires allocator-matched native free (currently verified on Linux only); use CpuOnly or Auto".to_owned(),
        })?;
        let Some(import_state) = api.cuda.import_state else {
            return Err(VmafError::CallFailed {
                function: "vmaf_cuda_import_state",
                status: -1,
            });
        };
        let Some(preallocate) = api.cuda.preallocate_pictures else {
            return Err(VmafError::CallFailed {
                function: "vmaf_cuda_preallocate_pictures",
                status: -1,
            });
        };

        // Install cleanup before the call: returned errors can populate `raw`.
        let mut state = CudaState {
            raw: std::ptr::null_mut(),
            release: Some(release),
            free,
        };
        // SAFETY: `raw` is a valid out-pointer; a null cu_ctx asks libvmaf to
        // retain its primary context, not reset or own any external GPU user.
        let status = unsafe { state_init(&mut state.raw, VmafCudaConfiguration::default()) };
        crate::ffi::check(status, "vmaf_cuda_state_init")?;

        if state.raw.is_null() {
            return Err(VmafError::CallFailed {
                function: "vmaf_cuda_state_init",
                status: 0,
            });
        }

        // WORKAROUND: v3.2.1 libvmaf.c:898–936 returns from the threaded branch
        // before unref'ing translated device pictures. Use its serial path for
        // CUDA only, until a native fix is verified. CPU retries keep the request.
        configuration.n_threads = 0;
        let mut raw: *mut crate::ffi::VmafContext = std::ptr::null_mut();
        // SAFETY: as `new_cpu`, plus `state` which was just initialised.
        let status = unsafe { (api.init)(&mut raw, configuration) };
        crate::ffi::check(status, "vmaf_init")?;
        if raw.is_null() {
            return Err(VmafError::CallFailed {
                function: "vmaf_init",
                status: 0,
            });
        }
        let context = Self {
            raw,
            api,
            backend: Backend::Cuda,
        };

        // SAFETY: both pointers are valid and non-null; `import_state` copies
        // the state only on success. Failure leaves resource ownership here.
        let status = unsafe { import_state(raw, state.raw) };
        crate::ffi::check(status, "vmaf_cuda_import_state")?;
        state.release = None;

        // SAFETY: the live context now owns the imported CUDA resources.
        let status = unsafe {
            preallocate(
                raw,
                crate::ffi::VmafCudaPictureConfiguration {
                    pic_params: crate::ffi::VmafCudaPictureConfigPictureParams {
                        w: geometry.width,
                        h: geometry.height,
                        bpc: geometry.bit_depth,
                        pix_fmt: geometry.pixel_format,
                    },
                    pic_prealloc_method: crate::ffi::VmafCudaPicturePreallocationMethod::Device,
                },
            )
        };

        crate::ffi::check(status, "vmaf_cuda_preallocate_pictures")?;
        // CUDA preallocation sizes only its upload ring. Generic picture fetch
        // must still return host-writable buffers; libvmaf handles device upload.
        context.preallocate_host(geometry, configuration.n_threads)?;
        // The original wrapper is freed here; close owns only the copied state.
        Ok(context)
    }
}

/// Reject a required, disabled backend before API loading or native allocation.
pub(crate) fn check_cuda_feature(preference: BackendPreference) -> Result<(), VmafError> {
    if !cfg!(feature = "cuda") && preference == BackendPreference::RequireCuda {
        return Err(VmafError::UnsupportedFeature(
            "RequireCuda needs the Cargo `cuda` feature; rebuild av-metrics-vmaf with --features cuda"
                .to_owned(),
        ));
    }
    Ok(())
}

/// Choose a backend only after its entire scorer attempt succeeds.
/// `complete` owns the context and must release all attempt resources on error,
/// before a fresh CPU attempt starts. Required model features decide compatibility.
///
/// # Errors
///
/// Returns the last error seen when the requested backend cannot be
/// initialised. With [`BackendPreference::RequireCuda`] no fallback is
/// attempted; a disabled Cargo `cuda` feature returns [`VmafError::UnsupportedFeature`].
pub(crate) fn initialise<T>(
    api: &'static VmafApi,
    configuration: crate::ffi::VmafConfiguration,
    preference: BackendPreference,
    geometry: Geometry,
    mut complete: impl FnMut(Context) -> Result<T, VmafError>,
) -> Result<T, VmafError> {
    check_cuda_feature(preference)?;
    let try_cuda = match preference {
        BackendPreference::CpuOnly => false,
        BackendPreference::Auto => api.cuda_available(),
        // Resolve required cleanup explicitly so missing allocator support is
        // actionable, without ever calling state_init on unsupported platforms.
        BackendPreference::RequireCuda => true,
    };

    if try_cuda {
        match Context::new_cuda(api, configuration, geometry).and_then(&mut complete) {
            Ok(scorer) => {
                tracing::info!("VMAF using the CUDA backend");
                return Ok(scorer);
            },
            Err(error) => {
                if preference == BackendPreference::RequireCuda {
                    return Err(error);
                }
                tracing::warn!(
                    %error,
                    "VMAF CUDA initialisation failed; falling back to the CPU backend"
                );
            },
        }
    }

    let scorer = complete(Context::new_cpu(api, configuration, geometry)?)?;
    tracing::info!("VMAF using the CPU backend");
    Ok(scorer)
}

/// Picture-pool size for a context with `n_threads` workers.
///
/// Mirrors libvmaf's own sizing in `check_picture_pool` (`n_threads * 2 + 2`)
/// for the same reason: each in-flight batch holds references on its pictures,
/// so a pool smaller than that caps how many batches the thread pool can run
/// concurrently and serialises extraction no matter what `n_threads` requests.
/// A fixed ring of four pictures left one frame in flight once libvmaf's
/// previous-reference chain is accounted for, which measured as barely more
/// than one worker no matter how many were configured — while FFmpeg, which
/// never preallocates and gets the full formula, extracted the same frames
/// about three times faster at six threads. libvmaf skips its own auto-sizing
/// when a pool already exists, so the formula has to be applied here.
///
/// The floor of four covers a serial context (`n_threads = 0`), which still
/// needs a picture for the previous-reference chain alongside the pair being
/// filled and its replacement.
#[inline]
const fn picture_pool_size(n_threads: u32) -> u32 {
    let sized = n_threads.saturating_mul(2).saturating_add(2);
    if sized < 4 { 4 } else { sized }
}

/// libvmaf log levels, used to keep the C library quiet by default.
pub(crate) const LOG_LEVEL_NONE: c_int = 0;

#[cfg(test)]
mod tests;
