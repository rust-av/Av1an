//! Streaming per-frame VMAF scoring.

use std::{ffi::CString, marker::PhantomData, mem::size_of, path::PathBuf};

use av_decoders::{Decoder, v_frame::pixel::Pixel};
use v_frame::chroma::ChromaSubsampling;

use crate::{
    backend::{self, Backend, Context},
    config::{PoolMethod, VmafConfig, VmafModel},
    error::VmafError,
    ffi::{
        self, VmafConfiguration, VmafModel as VmafModelHandle, VmafModelConfig, VmafPicture,
        VmafPixelFormat,
    },
};

/// Geometry and format of the clips being compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoFormat {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Bits per channel: 8, 10, 12 or 16.
    pub bit_depth: u32,
    /// Chroma subsampling.
    pub chroma_sampling: ChromaSubsampling,
}

/// A checked, borrowed plane with immutable geometry and a known backing span.
///
/// Width is in samples; stride, origin and span are in BYTES. Negative strides
/// describe bottom-up storage. Only visible bytes are read, not row padding.
/// Two-byte samples use native endian storage; byte alignment is sufficient.
/// Use slice constructors for Rust storage and [`Self::from_raw_parts`] for FFI.
///
/// ```compile_fail
/// use av_metrics_vmaf::PlaneSource;
/// let bytes = [0u8; 4];
/// let mut plane = PlaneSource::from_bytes(&bytes, 2, 2, 2, 0, 1).unwrap();
/// plane.width = 20;
/// ```
///
/// ```compile_fail
/// use av_metrics_vmaf::PlaneSource;
/// let bytes = [0u8; 4];
/// let mut plane = PlaneSource::from_bytes(&bytes, 2, 2, 2, 0, 1).unwrap();
/// plane.stride = 0;
/// ```
///
/// The raw constructor requires an explicit unsafe allocation/lifetime promise:
/// ```compile_fail
/// use av_metrics_vmaf::PlaneSource;
/// let bytes = [0u8; 4];
/// let plane = PlaneSource::from_raw_parts(bytes.as_ptr(), bytes.len(), 2, 2, 2, 0, 1);
/// ```
///
/// Views cannot outlive the frame they borrow:
/// ```compile_fail
/// use av_metrics_vmaf::PlaneSource;
/// use v_frame::{frame::FrameBuilder, chroma::ChromaSubsampling};
/// let plane;
/// {
///     let frame = FrameBuilder::new(2, 2, ChromaSubsampling::Yuv444, 8)
///         .build::<u8>().unwrap();
///     plane = PlaneSource::from_bytes(frame.y_plane.data(), 2, 2, 2, 0, 1).unwrap();
/// }
/// std::hint::black_box(plane);
/// ```
#[derive(Debug, Clone, Copy)]
pub struct PlaneSource<'a> {
    width: usize,
    height: usize,
    stride: isize,
    data_origin: usize,
    data: *const u8,
    bytes_per_sample: usize,
    readable_len: usize,
    _storage: PhantomData<&'a [u8]>,
}

impl<'a> PlaneSource<'a> {
    /// Borrow initialized bytes; stride and origin are in BYTES.
    ///
    /// # Errors
    /// Returns [`VmafError::MalformedFrame`] for zero/nonnative dimensions,
    /// sample sizes other than 1/2, short
    /// strides/spans, and overflowing or out-of-bounds signed byte offsets.
    #[inline]
    pub fn from_bytes(
        data: &'a [u8],
        width: usize,
        height: usize,
        stride: isize,
        data_origin: usize,
        bytes_per_sample: usize,
    ) -> Result<Self, VmafError> {
        // SAFETY: the slice proves the actual span, initialization and shared borrow.
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

    /// Borrow native-endian u16 samples; stride and origin still use BYTES.
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
    ) -> Result<Self, VmafError> {
        // SAFETY: initialized u16 has no padding; the slice supplies its real span.
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

    /// Describe foreign storage with an actual declared byte span.
    /// Stride and origin are in BYTES. No slice over padding is created.
    ///
    /// # Errors
    /// Returns layout errors as in [`Self::from_bytes`], also rejecting null
    /// pointers and spans larger than `isize::MAX`, without dereferencing.
    ///
    /// # Safety
    /// If validation succeeds, `data` must identify one live allocation covering
    /// at least `readable_len` bytes. Visible samples must be initialized and
    /// readable, without mutation for all of `'a`; padding may be uninitialized.
    /// Bind `'a` to the actual owner. A required extent inferred from geometry
    /// is not evidence of an allocation's length. Source storage must not overlap
    /// libvmaf's destination pictures.
    #[inline]
    pub unsafe fn from_raw_parts(
        data: *const u8,
        readable_len: usize,
        width: usize,
        height: usize,
        stride: isize,
        data_origin: usize,
        bytes_per_sample: usize,
    ) -> Result<Self, VmafError> {
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
        if width == 0
            || height == 0
            || !matches!(bytes_per_sample, 1 | 2)
            || readable_len > isize::MAX as usize
            || u32::try_from(width).is_err()
            || u32::try_from(height).is_err()
        {
            return Err(malformed("invalid plane dimensions, sample width or span"));
        }
        let row_bytes = width
            .checked_mul(bytes_per_sample)
            .and_then(|n| isize::try_from(n).ok())
            .ok_or_else(|| malformed("plane row size overflow"))?;
        let origin =
            isize::try_from(data_origin).map_err(|_| malformed("plane origin overflow"))?;
        let last = isize::try_from(height - 1)
            .ok()
            .and_then(|n| n.checked_mul(stride))
            .and_then(|n| origin.checked_add(n))
            .filter(|&n| n >= 0)
            .ok_or_else(|| malformed("plane row offset overflow or before allocation"))?;
        if data.is_null()
            || stride.checked_abs().is_none_or(|n| n < row_bytes)
            || origin
                .max(last)
                .checked_add(row_bytes)
                .is_none_or(|end| end as usize > readable_len)
        {
            return Err(malformed(
                "invalid plane pointer, stride or visible byte extent",
            ));
        }
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
    /// Sample storage width in bytes.
    #[inline]
    #[must_use]
    pub const fn bytes_per_sample(&self) -> usize {
        self.bytes_per_sample
    }
    /// Declared backing span in bytes; padding may be uninitialized.
    #[inline]
    #[must_use]
    pub const fn readable_len(&self) -> usize {
        self.readable_len
    }
    /// Start of backing storage. Dereferencing requires unsafe code.
    #[inline]
    #[must_use]
    pub const fn data_ptr(&self) -> *const u8 {
        self.data
    }

    /// Pointer to the first visible byte of `row`, or `None` when out of range.
    #[inline]
    fn row(&self, row: usize) -> Option<*const u8> {
        if row >= self.height {
            return None;
        }
        let offset = isize::try_from(row)
            .ok()?
            .checked_mul(self.stride)?
            .checked_add(isize::try_from(self.data_origin).ok()?)?;
        let offset = usize::try_from(offset).ok()?;
        // SAFETY: construction checks every visible row against the borrowed span.
        Some(unsafe { self.data.add(offset) })
    }
}

fn malformed(reason: &str) -> VmafError {
    VmafError::MalformedFrame {
        reason: reason.to_owned(),
    }
}

/// A frame as the three planes libvmaf scores.
///
/// All three representations a caller might have -- a VapourSynth `Frame`, a
/// `v_frame::frame::Frame`, or a decoded `Frame` from any backend -- reduce to
/// this, which is why the scorer needs no per-source special cases.
pub type PlaneSet<'a> = [PlaneSource<'a>; 3];

/// Describe a `v_frame` plane as a [`PlaneSource`].
///
/// This is the only adaptor `v_frame` needs: the plane already exposes a stride
/// and a data origin, so no copying or per-row walking happens here.
fn plane_source<T: Pixel>(plane: &v_frame::plane::Plane<T>) -> Result<PlaneSource<'_>, VmafError> {
    let geometry = plane.geometry();
    let bytes_per_sample = size_of::<T>();
    let stride = geometry
        .stride()
        .checked_mul(bytes_per_sample)
        .and_then(|n| isize::try_from(n).ok())
        .ok_or_else(|| malformed("frame stride overflow"))?;
    let origin = geometry
        .data_origin()
        .checked_mul(bytes_per_sample)
        .ok_or_else(|| malformed("frame origin overflow"))?;
    let readable_len = plane
        .data()
        .len()
        .checked_mul(bytes_per_sample)
        .ok_or_else(|| malformed("frame allocation byte length overflow"))?;
    // SAFETY: v_frame 0.7 Pixel is sealed to initialized, padding-free u8/u16.
    // Plane::data exposes the actual storage borrowed for the returned lifetime.
    unsafe {
        PlaneSource::from_raw_parts(
            plane.data().as_ptr().cast(),
            readable_len,
            geometry.width(),
            geometry.height(),
            stride,
            origin,
            bytes_per_sample,
        )
    }
}

impl VideoFormat {
    /// The libvmaf pixel format for this chroma sampling, or `None` if
    /// unsupported.
    ///
    /// libvmaf accepts only 4:2:0, 4:2:2 and 4:4:4 planar YUV.
    #[inline]
    #[must_use]
    pub const fn pixel_format(&self) -> Option<VmafPixelFormat> {
        match self.chroma_sampling {
            ChromaSubsampling::Yuv420 => Some(VmafPixelFormat::Yuv420p),
            ChromaSubsampling::Yuv422 => Some(VmafPixelFormat::Yuv422p),
            ChromaSubsampling::Yuv444 => Some(VmafPixelFormat::Yuv444p),
            _ => None,
        }
    }

    /// Validate that libvmaf can process this format.
    ///
    /// # Errors
    ///
    /// Returns [`VmafError::UnsupportedFormat`] if the bit depth or chroma
    /// sampling is outside libvmaf's supported set.
    #[inline]
    pub fn validate(&self) -> Result<VmafPixelFormat, VmafError> {
        if !matches!(self.bit_depth, 8 | 10 | 12 | 16) {
            return Err(VmafError::UnsupportedFormat {
                reason: format!(
                    "bit depth {} is not supported; libvmaf accepts 8, 10, 12 and 16",
                    self.bit_depth
                ),
            });
        }

        self.pixel_format().ok_or_else(|| VmafError::UnsupportedFormat {
            reason: format!(
                "chroma sampling {:?} is not supported; libvmaf accepts 4:2:0, 4:2:2 and 4:4:4",
                self.chroma_sampling
            ),
        })
    }
}

/// A single frame's scores.
#[derive(Debug, Clone, Default)]
pub struct FrameScore {
    /// The pooled model score for this frame.
    pub score: f64,
    /// Per-feature scores, keyed by libvmaf feature name.
    pub features: Vec<(String, f64)>,
}

/// A VMAF scoring session.
///
/// Scores are produced by streaming matching frame pairs from a reference and a
/// distorted [`Decoder`]. Temporal features and worker completion are only
/// guaranteed after a successful flush, so scores are read only then.
///
/// A `VmafScorer` is single-use and drives a single native `VmafContext`, which
/// libvmaf documents as not safe for concurrent use. It is [`Send`] so it can
/// be moved into a worker, but must not be shared.
pub struct VmafScorer {
    context: Context,
    config: VmafConfig,
    format: VideoFormat,
    model: *mut VmafModelHandle,
    /// Feature keys used to read pooled values back, e.g. `psnr_y`.
    feature_names: Vec<CString>,
    /// Checked-out pair. None after transfer until both replacements succeed;
    /// a failed replacement leaves the scorer terminal, never half-owned.
    pictures: Option<(backend::PooledPicture, backend::PooledPicture)>,
    /// Scratch buffers for the C strings handed to `vmaf_init`.
    _model_spec: CString,
    scored: usize,
    /// Native flush is single-use, even when final scores are read repeatedly.
    flushed: bool,
    /// Ensures `!Sync`, because a context is single-threaded.
    _not_sync: PhantomData<*mut ()>,
}

// Snapshot selection once: complete CUDA/CPU retries must not re-read the override.
struct ModelSelection {
    model: VmafModel,
    spec: CString,
    fallback: Option<PathBuf>,
    searched: Vec<PathBuf>,
}

impl Drop for VmafScorer {
    #[inline]
    fn drop(&mut self) {
        // Return the pooled pictures and release the model. This runs before
        // the struct's fields are dropped, so `self.context` is still
        // live here.
        drop(self.pictures.take());

        if !self.model.is_null() {
            // SAFETY: `model` came from `vmaf_model_load` or
            // `vmaf_model_load_from_path`, including initialized partials on
            // JSON errors, and is destroyed exactly once, here.
            unsafe { (self.context.api().model_destroy)(self.model) };
        }
    }
}

impl VmafScorer {
    /// Create a scorer for two clips of the given format.
    ///
    /// The reference and distorted clips must share this format exactly;
    /// mismatches are reported when scoring begins.
    ///
    /// # Errors
    ///
    /// Returns [`VmafError::LibraryLoadFailed`] if libvmaf could not be loaded,
    /// [`VmafError::UnsupportedFormat`] if the format is outside libvmaf's
    /// supported set, [`VmafError::ModelNotFound`] if an explicit stock-directory
    /// override is missing or a model fails to load, and [`VmafError::CallFailed`]
    /// if no requested backend can be initialised. A missing per-run file path
    /// returns [`VmafError::ModelFileMissing`] before native setup.
    /// Required CUDA without the Cargo `cuda` feature returns
    /// [`VmafError::UnsupportedFeature`] before loading libvmaf.
    #[expect(
        clippy::missing_inline_in_public_items,
        reason = "inlining the scorer would duplicate the whole body at every call site"
    )]
    pub fn new(config: VmafConfig, format: VideoFormat) -> Result<Self, VmafError> {
        Self::new_with_loader(config, format, ffi::VmafApi::load)
    }

    // Keep input validation before dynamic loading, including in boundary tests.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "each fallible backend attempt needs an owned copy of the requested configuration"
    )]
    fn new_with_loader(
        config: VmafConfig,
        format: VideoFormat,
        load_api: impl FnOnce() -> Result<&'static ffi::VmafApi, VmafError>,
    ) -> Result<Self, VmafError> {
        let pixel_format = format.validate()?;

        // Resolve the model first so a bad path fails before libvmaf is
        // touched.
        let selected_model = config
            .model
            .stock_model_override()?
            .map_or_else(|| config.model.clone(), VmafModel::Path);
        let model_spec = selected_model.as_libvmaf_model()?;
        let model_spec = CString::new(model_spec).map_err(|_| VmafError::InvalidConfiguration {
            reason: "model specification contains an interior null byte".to_owned(),
        })?;

        // A missing native library must not hide the required Cargo opt-in.
        backend::check_cuda_feature(config.backend)?;

        // Registration uses extractor names (`psnr`), while reading values back
        // uses feature keys (`psnr_y`). They differ for the per-plane
        // extractors, so both lists are kept.
        let extractor_names: Vec<CString> = config
            .extractor_names()
            .into_iter()
            .map(|name| CString::new(name).expect("static extractor names contain no nulls"))
            .collect();

        let feature_names: Vec<CString> = config
            .feature_names()
            .into_iter()
            .map(|name| CString::new(name).expect("static feature names contain no nulls"))
            .collect();

        let api = load_api()?;
        // Loaded-library inference is only available after discovery. Explicit
        // selections never consult these fallback sources, even on native failure.
        let searched = if selected_model.is_path() {
            Vec::new()
        } else {
            VmafModel::fallback_model_search_paths()
        };
        let selection = ModelSelection {
            fallback: selected_model.find_stock_model_file_in(&searched),
            model: selected_model,
            spec: model_spec,
            searched,
        };

        let configuration = VmafConfiguration {
            log_level: backend::LOG_LEVEL_NONE,
            n_threads: config.n_threads,
            // Always 1. libvmaf applies this as a gate on the *spatial*
            // extractors only, so a higher value would leave the frames in
            // between with no score at all. Subsample by submitting every n-th
            // frame pair instead.
            n_subsample: 1,
            cpumask: 0,
            gpumask: 0,
        };

        let geometry = backend::Geometry {
            width: format.width,
            height: format.height,
            bit_depth: format.bit_depth,
            pixel_format,
        };

        // Follow libvmaf's own initialisation order exactly: context, then its
        // picture pool, then the model, then the feature extractors that model
        // requires. `vmaf_read_pictures` assumes every registered extractor has
        // a live context, so ordering here is not incidental.
        backend::initialise(api, configuration, config.backend, geometry, |context| {
            Self::complete_attempt(
                context,
                config.clone(),
                format,
                &selection,
                feature_names.clone(),
                &extractor_names,
            )
        })
    }

    fn complete_attempt(
        context: Context,
        config: VmafConfig,
        format: VideoFormat,
        selection: &ModelSelection,
        feature_names: Vec<CString>,
        extractor_names: &[CString],
    ) -> Result<Self, VmafError> {
        // An incomplete scorer is the attempt's guard: pictures, then the model,
        // then the native context drop on every error before Auto retries.
        let mut scorer = Self {
            context,
            config,
            format,
            model: std::ptr::null_mut(),
            feature_names,
            pictures: None,
            _model_spec: selection.spec.clone(),
            scored: 0,
            flushed: false,
            _not_sync: PhantomData,
        };
        let api = scorer.context.api();
        let config = &scorer.config;

        // Built-in models and on-disk models use separate loaders; each rejects
        // the other's input with `EINVAL`.
        //
        // Not every libvmaf build compiles the models in. When the built-in
        // load fails for a stock model, look for the same model as a
        // JSON file on disk before giving up, so a distribution that
        // installs models separately works without the caller having to
        // say so.
        let mut model_config = VmafModelConfig::new();

        let load_from_path = selection.model.is_path();
        let mut load_spec = scorer._model_spec.clone();

        // Unlike failed context init/fetch outputs, a nonnull JSON model output
        // remains owned on error (libvmaf read_json_model.c). Write directly into
        // the staged scorer's guard before checking status or retrying.
        // SAFETY: `scorer.model` is a fresh null out-pointer, `model_config` a valid
        // mutable config, and `load_spec` a NUL-terminated string that
        // outlives the call.
        let mut status = unsafe {
            let loader = if load_from_path {
                api.model_load_from_path
            } else {
                api.model_load
            };
            loader(
                raw_mut_model(&mut scorer.model),
                raw_mut_config(&mut model_config),
                load_spec.as_ptr(),
            )
        };

        if status != 0
            && !load_from_path
            && let Some(path) = &selection.fallback
            && let Ok(spec) = CString::new(path.display().to_string())
        {
            tracing::info!(
                path = %path.display(),
                "the built-in VMAF model is unavailable; using the model file on disk"
            );
            load_spec = spec;
            if !scorer.model.is_null() {
                let partial = std::mem::replace(&mut scorer.model, std::ptr::null_mut());
                // SAFETY: the JSON loader leaves a destroyable initialized
                // partial on error. Release it before the file loader/retry.
                unsafe { (api.model_destroy)(partial) };
            }
            // SAFETY: as above, with `load_spec` now holding the file path and
            // the caller having supplied a configuration libvmaf
            // accepted.
            status = unsafe {
                (api.model_load_from_path)(
                    raw_mut_model(&mut scorer.model),
                    raw_mut_config(&mut model_config),
                    load_spec.as_ptr(),
                )
            };
        }

        if status != 0 {
            if load_from_path {
                return Err(VmafError::ModelNotFound {
                    model: load_spec.to_string_lossy().into_owned(),
                    reason: format!(
                        "{} failed in vmaf_model_load_from_path with status {status}; no other \
                         model will be tried",
                        if config.model.is_path() {
                            "explicit model path"
                        } else {
                            "VMAF_MODEL_PATH override"
                        },
                    ),
                });
            }
            let searched = selection
                .searched
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");

            return Err(VmafError::ModelNotFound {
                model: load_spec.to_string_lossy().into_owned(),
                reason: format!(
                    "loading the model returned status {status}. This libvmaf build appears to \
                     have no built-in models, and no matching model file was found. Pass an \
                     explicit model path, set VMAF_MODEL_PATH to the directory holding \
                     {version}.json, or search one of: {searched}",
                    version = config.model.stock_version().unwrap_or("the model"),
                ),
            });
        }

        if scorer.model.is_null() {
            return Err(VmafError::CallFailed {
                function: if load_from_path || load_spec != scorer._model_spec {
                    "vmaf_model_load_from_path"
                } else {
                    "vmaf_model_load"
                },
                status: 0,
            });
        }

        // SAFETY: both handles came from libvmaf and the context is live.
        let status = unsafe { (api.use_features_from_model)(scorer.context.raw, scorer.model) };
        if status != 0 {
            return Err(VmafError::CallFailed {
                function: "vmaf_use_features_from_model",
                status,
            });
        }

        // Register any additional feature extractors the caller asked for.
        // These are optional extras, so a rejection is logged rather
        // than fatal.
        for name in extractor_names {
            // SAFETY: `name` is a valid NUL-terminated string, and a null
            // options dictionary selects the extractor's defaults.
            let status = unsafe {
                (api.use_feature)(scorer.context.raw, name.as_ptr(), std::ptr::null_mut())
            };
            if status != 0 {
                tracing::warn!(
                    feature = name.to_string_lossy().into_owned(),
                    status,
                    "libvmaf rejected an additional feature extractor; ignoring it"
                );
            }
        }

        // Locals drop in reverse order on failure: any first checkout returns
        // before the context closes if the second fetch fails.
        let reference_picture = scorer.context.pooled_picture()?;
        let distorted_picture = scorer.context.pooled_picture()?;

        scorer.pictures = Some((reference_picture, distorted_picture));
        Ok(scorer)
    }

    /// The backend this scorer ended up on.
    #[inline]
    #[must_use]
    pub const fn backend(&self) -> Backend {
        self.context.backend
    }

    /// The configuration in use.
    #[inline]
    #[must_use]
    pub const fn config(&self) -> &VmafConfig {
        &self.config
    }

    /// The format being scored.
    #[inline]
    #[must_use]
    pub const fn format(&self) -> VideoFormat {
        self.format
    }

    /// Whether libvmaf is available on this system.
    ///
    /// Cheap to call: the library is resolved once and cached.
    #[inline]
    #[must_use]
    pub fn is_available() -> bool {
        ffi::VmafApi::load().is_ok()
    }

    /// The runtime libvmaf version string, if available.
    #[inline]
    #[must_use]
    pub fn libvmaf_version() -> Option<&'static str> {
        ffi::VmafApi::load().ok().map(|api| api.version)
    }

    /// Score every matching frame pair from the two decoders.
    ///
    /// Returns one [`FrameScore`] per frame. Scores and callbacks are delivered
    /// after successful finalization, not during submission.
    ///
    /// # Errors
    ///
    /// Returns [`VmafError::InputMismatch`] if the two clips disagree on
    /// format, [`VmafError::FrameCountMismatch`] if their lengths differ,
    /// and [`VmafError::Decode`], [`VmafError::CallFailed`] or
    /// [`VmafError::ScoreQueryFailed`] on downstream failures.
    #[expect(
        clippy::missing_inline_in_public_items,
        reason = "inlining the scoring loop would duplicate the whole body"
    )]
    pub fn score_decoders<T: Pixel>(
        &mut self,
        reference: &mut Decoder,
        distorted: &mut Decoder,
        mut on_score: impl FnMut(usize, &FrameScore),
    ) -> Result<Vec<FrameScore>, VmafError> {
        let reference_details = *reference.get_video_details();
        let distorted_details = *distorted.get_video_details();

        if reference_details.width != distorted_details.width
            || reference_details.height != distorted_details.height
        {
            return Err(VmafError::InputMismatch {
                reason: format!(
                    "resolution differs: {}x{} vs {}x{}",
                    reference_details.width,
                    reference_details.height,
                    distorted_details.width,
                    distorted_details.height
                ),
            });
        }

        if reference_details.bit_depth != distorted_details.bit_depth {
            return Err(VmafError::InputMismatch {
                reason: format!(
                    "bit depth differs: {} vs {}",
                    reference_details.bit_depth, distorted_details.bit_depth
                ),
            });
        }

        if reference_details.chroma_sampling != distorted_details.chroma_sampling {
            return Err(VmafError::InputMismatch {
                reason: format!(
                    "chroma sampling differs: {:?} vs {:?}",
                    reference_details.chroma_sampling, distorted_details.chroma_sampling
                ),
            });
        }

        let mut scores = Vec::new();

        loop {
            // `av-decoders` signals end of stream through `DecoderError`, not
            // an `Option`, so a short read is only distinguishable
            // via its error.
            let reference_frame = reference.read_video_frame::<T>();
            let distorted_frame = distorted.read_video_frame::<T>();

            let (reference_frame, distorted_frame) = match (reference_frame, distorted_frame) {
                (Ok(reference_frame), Ok(distorted_frame)) => (reference_frame, distorted_frame),
                (reference_result, distorted_result) => {
                    // `av-decoders` signals end of stream as an error variant.
                    let reference_done =
                        matches!(reference_result, Err(av_decoders::DecoderError::EndOfFile));
                    let distorted_done =
                        matches!(distorted_result, Err(av_decoders::DecoderError::EndOfFile));

                    if reference_done && distorted_done {
                        break;
                    }

                    if reference_done || distorted_done {
                        return Err(VmafError::FrameCountMismatch {
                            reference: self.scored + usize::from(!reference_done),
                            distorted: self.scored + usize::from(!distorted_done),
                        });
                    }

                    // A genuine decode failure on one or both sides; surface
                    // the reference's, falling back to the
                    // distorted's.
                    let error = match (reference_result, distorted_result) {
                        (Err(error), _) => error,
                        (_, Err(error)) => error,
                        // Unreachable: both arms above require an `Err`, and the
                        // Ok/Ok case was matched earlier.
                        (Ok(_), Ok(_)) => av_decoders::DecoderError::EndOfFile,
                    };
                    return Err(error.into());
                },
            };

            let index = self.scored as u32;
            self.submit(&reference_frame, &distorted_frame, index)?;
            self.scored += 1;
        }

        // Flushing runs the extractors to completion, after which every index
        // has a score. libvmaf extracts asynchronously, so scores are
        // not reliably available mid-stream: `vmaf_score_at_index`
        // reports an error until the features for that index have been
        // computed. Collecting only after the flush avoids depending on
        // how far the worker threads have progressed.
        self.flush()?;

        let total = self.scored;
        scores.reserve(total);
        for index in 0..total as u32 {
            scores.push(self.score_at(index)?);
        }
        for (index, score) in scores.iter().enumerate() {
            on_score(index, score);
        }

        Ok(scores)
    }

    /// Pool the per-frame scores into a single value.
    ///
    /// # Errors
    ///
    /// Returns [`VmafError::InvalidConfiguration`] if `scores` is empty.
    #[expect(
        clippy::missing_inline_in_public_items,
        reason = "inlining would duplicate the whole body for a cold path"
    )]
    pub fn pool(scores: &[FrameScore], method: PoolMethod) -> Result<f64, VmafError> {
        if scores.is_empty() {
            return Err(VmafError::InvalidConfiguration {
                reason: "cannot pool an empty score list".to_owned(),
            });
        }

        Ok(match method {
            PoolMethod::Mean => {
                scores.iter().map(|score| score.score).sum::<f64>() / scores.len() as f64
            },
            PoolMethod::Min => scores.iter().map(|score| score.score).fold(f64::INFINITY, f64::min),
            PoolMethod::Max => {
                scores.iter().map(|score| score.score).fold(f64::NEG_INFINITY, f64::max)
            },
            PoolMethod::HarmonicMean => {
                // n / sum(1/x_i), which for 10, 20, 30 is 3 / (0.1 + 0.05 +
                // 0.0333) = 18. Matches libvmaf's
                // VMAF_POOL_METHOD_HARMONIC_MEAN.
                let count = scores.len() as f64;
                count / scores.iter().map(|score| 1.0 / score.score).sum::<f64>()
            },
        })
    }

    /// Submit one frame pair to libvmaf.
    ///
    /// Each successful submission transfers its checked-out picture pair and
    /// acquires a new pair from the pool.
    fn submit<T: Pixel>(
        &mut self,
        reference: &v_frame::frame::Frame<T>,
        distorted: &v_frame::frame::Frame<T>,
        index: u32,
    ) -> Result<(), VmafError> {
        // `Frame` exposes chroma planes as `Option`, since monochrome frames
        // have none. libvmaf requires chroma, so this is a validation
        // failure.
        let (Some(u_plane), Some(v_plane)) = (&reference.u_plane, &reference.v_plane) else {
            return Err(VmafError::UnsupportedFormat {
                reason: "frame has no chroma planes; libvmaf requires 4:2:0, 4:2:2 or 4:4:4 \
                         content"
                    .to_owned(),
            });
        };
        let (Some(d_u), Some(d_v)) = (&distorted.u_plane, &distorted.v_plane) else {
            return Err(VmafError::UnsupportedFormat {
                reason: "frame has no chroma planes; libvmaf requires 4:2:0, 4:2:2 or 4:4:4 \
                         content"
                    .to_owned(),
            });
        };

        let reference_planes = [
            plane_source(&reference.y_plane)?,
            plane_source(u_plane)?,
            plane_source(v_plane)?,
        ];
        let distorted_planes =
            [plane_source(&distorted.y_plane)?, plane_source(d_u)?, plane_source(d_v)?];

        self.submit_planes(index, reference_planes, distorted_planes)
    }

    /// Copy plane sources into the pooled pictures and hand them to libvmaf.
    ///
    /// The single path every caller funnels through: both the `v_frame` and the
    /// raw-source entry points reduce to three [`PlaneSource`]s per side, so
    /// there is exactly one copy loop and one set of validation.
    fn submit_planes(
        &mut self,
        index: u32,
        reference: PlaneSet<'_>,
        distorted: PlaneSet<'_>,
    ) -> Result<(), VmafError> {
        self.ensure_active()?;
        if self.flushed {
            return Err(VmafError::InvalidConfiguration {
                reason: "scorer has been flushed; create a new scorer to submit more frames"
                    .to_owned(),
            });
        }
        let (reference_picture, distorted_picture) =
            self.pictures.as_mut().expect("active scorer owns a complete pair");
        Self::fill_pictures(
            &mut reference_picture.raw,
            &mut distorted_picture.raw,
            &reference,
            &distorted,
            self.format.pixel_format(),
            self.format.bit_depth,
        )?;

        // SAFETY: both pictures came from this context's pool and hold valid
        // buffers for the geometry configured at construction.
        let status = unsafe {
            (self.context.api().read_pictures)(
                self.context.raw,
                raw_mut(&mut reference_picture.raw),
                raw_mut(&mut distorted_picture.raw),
                index,
            )
        };

        // Audited v3.2.1 valid-pool-input errors (including enqueue rejection)
        // precede caller consumption. Preserve both guards and the read error.
        ffi::check(status, "vmaf_read_pictures")?;

        // Success consumes both refs. CUDA-compiled CPU extraction unrefs local
        // aliases, so caller descriptor zeroing/pointer presence proves nothing.
        let (reference, distorted) = self.pictures.take().expect("submitted pair");
        reference.transfer();
        distorted.transfer();

        // Publish only a complete pair. A failed second fetch drops the first
        // local guard; None keeps all subsequent calls off stale inputs/indices.
        let reference = self.context.pooled_picture()?;
        let distorted = self.context.pooled_picture()?;
        self.pictures = Some((reference, distorted));
        Ok(())
    }

    /// Replacement failure follows a native submission, so partial scores must
    /// not be presented as a successful run or retried at a duplicate index.
    fn ensure_active(&self) -> Result<(), VmafError> {
        if self.pictures.is_none() {
            return Err(VmafError::InvalidConfiguration {
                reason:
                    "scorer is unusable after a picture replacement failure; create a new scorer"
                        .to_owned(),
            });
        }
        Ok(())
    }

    /// Preflight the whole pair before either host picture is touched.
    fn fill_pictures(
        reference_picture: &mut VmafPicture,
        distorted_picture: &mut VmafPicture,
        reference: &PlaneSet<'_>,
        distorted: &PlaneSet<'_>,
        pixel_format: Option<VmafPixelFormat>,
        bit_depth: u32,
    ) -> Result<(), VmafError> {
        Self::validate_picture(reference_picture, reference, pixel_format, bit_depth)?;
        Self::validate_picture(distorted_picture, distorted, pixel_format, bit_depth)?;
        Self::fill_picture(reference_picture, reference, pixel_format, bit_depth)?;
        Self::fill_picture(distorted_picture, distorted, pixel_format, bit_depth)
    }

    /// Validate all planes before any row read or destination write.
    fn validate_picture(
        picture: &VmafPicture,
        planes: &PlaneSet<'_>,
        pixel_format: Option<VmafPixelFormat>,
        bit_depth: u32,
    ) -> Result<(), VmafError> {
        let expected = pixel_format.ok_or_else(|| VmafError::UnsupportedFormat {
            reason: "chroma sampling is outside libvmaf's supported set".to_owned(),
        })?;
        let actual = VmafPixelFormat::try_from(picture.pix_fmt)?;
        if actual != expected {
            return Err(VmafError::UnsupportedFormat {
                reason: format!(
                    "libvmaf pool pixel format {} differs from configured {expected:?}",
                    picture.pix_fmt
                ),
            });
        }
        let bytes_per_sample = match bit_depth {
            8 => 1,
            10 | 12 | 16 => 2,
            _ => {
                return Err(VmafError::UnsupportedFormat {
                    reason: format!("unsupported bit depth {bit_depth}"),
                });
            },
        };
        for (index, source) in planes.iter().enumerate() {
            if source.bytes_per_sample != bytes_per_sample {
                return Err(VmafError::InputMismatch {
                    reason: format!(
                        "plane {index} has {}-byte samples but {bit_depth}-bit scoring requires \
                         {bytes_per_sample}-byte storage",
                        source.bytes_per_sample
                    ),
                });
            }
            // The pooled picture is sized for the scorer's configured geometry,
            // so a mismatch means the frame disagrees with what the
            // scorer expects.
            let expected_width = picture.w[index] as usize;
            let expected_height = picture.h[index] as usize;
            if source.width != expected_width || source.height != expected_height {
                return Err(VmafError::InputMismatch {
                    reason: format!(
                        "plane {index} is {}x{} but the scorer was configured for \
                         {expected_width}x{expected_height}",
                        source.width, source.height
                    ),
                });
            }

            let row_bytes = expected_width.checked_mul(bytes_per_sample).ok_or_else(|| {
                VmafError::MalformedFrame {
                    reason: format!("plane {index} row length overflows"),
                }
            })?;

            let picture_stride = picture.stride[index];
            if picture_stride < 0 || (picture_stride as usize) < row_bytes {
                return Err(VmafError::MalformedFrame {
                    reason: format!(
                        "libvmaf pool stride {picture_stride} is too small for the {row_bytes} \
                         bytes a visible row requires"
                    ),
                });
            }

            if picture.data[index].is_null() {
                return Err(VmafError::MalformedFrame {
                    reason: format!("libvmaf returned a null buffer for plane {index}"),
                });
            }
            expected_height
                .checked_sub(1)
                .and_then(|n| n.checked_mul(picture_stride as usize))
                .and_then(|n| n.checked_add(row_bytes))
                .filter(|&n| isize::try_from(n).is_ok())
                .ok_or_else(|| malformed("libvmaf destination byte offset overflow"))?;
        }
        Ok(())
    }

    /// Copy checked sources into a host picture. Pool geometry is authoritative.
    fn fill_picture(
        picture: &mut VmafPicture,
        planes: &PlaneSet<'_>,
        pixel_format: Option<VmafPixelFormat>,
        bit_depth: u32,
    ) -> Result<(), VmafError> {
        Self::validate_picture(picture, planes, pixel_format, bit_depth)?;
        // libvmaf normalizes by bpc; don't rely on pooled metadata retaining it.
        picture.bpc = bit_depth;
        for (index, source) in planes.iter().enumerate() {
            let row_bytes = source.width * source.bytes_per_sample;
            let picture_stride = picture.stride[index] as usize;
            // This host-copy path relies on the pool supplying writable host
            // storage. Geometry validation is not proof of device-pointer safety.
            let destination = picture.data[index].cast::<u8>();
            for row in 0..source.height {
                let Some(source_row) = source.row(row) else {
                    return Err(VmafError::MalformedFrame {
                        reason: format!("plane {index} does not have row {row}"),
                    });
                };

                // SAFETY: the checks above established that the source has this
                // row, that it is `row_bytes` long, and that the destination
                // allocation is `picture_stride` bytes per row.
                unsafe {
                    std::ptr::copy_nonoverlapping::<u8>(
                        source_row,
                        destination.add(row * picture_stride),
                        row_bytes,
                    );
                };
            }
        }

        Ok(())
    }

    /// Finalize native extraction exactly once on success; failures remain errors.
    fn flush(&mut self) -> Result<(), VmafError> {
        self.ensure_active()?;
        if self.flushed {
            return Ok(());
        }
        // SAFETY: libvmaf documents null pictures as the flush signal.
        let status = unsafe {
            (self.context.api().read_pictures)(
                self.context.raw,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                self.scored as u32,
            )
        };
        ffi::check(status, "vmaf_read_pictures")?;
        self.flushed = true;
        Ok(())
    }

    /// Read a final score. Callers must successfully flush first.
    fn score_at(&self, index: u32) -> Result<FrameScore, VmafError> {
        let mut score = 0.0f64;
        // SAFETY: the context and its nonnull configured model are live, native
        // extraction has been flushed, and `score` is a valid out-pointer.
        let status = unsafe {
            (self.context.api().score_at_index)(self.context.raw, self.model, &mut score, index)
        };
        if status != 0 {
            return Err(VmafError::ScoreQueryFailed { index, status });
        }

        Ok(FrameScore {
            score,
            // Feature values are pooled over the clip rather than read per index,
            // because extractors do not expose a score for every frame. They are
            // available through `pooled_features` once scoring has finished.
            features: Vec::new(),
        })
    }

    /// Score an explicit sequence of frame pairs, taken in the order given.
    ///
    /// This is the primitive for scoring a sparse set of frames. The caller
    /// decodes each frame itself and hands over the pair, so unselected frames
    /// are never decoded and never reach libvmaf. libvmaf scores index *i*
    /// from the pair at position *i*, so the returned vector lines up with
    /// `pairs` one-for-one.
    ///
    /// `pairs` must be non-empty, and every frame must match the geometry this
    /// scorer was built for.
    ///
    /// # Errors
    ///
    /// Returns [`VmafError::MalformedFrame`] if a frame's planes do not match
    /// the scorer's geometry, and [`VmafError::InvalidConfiguration`] if
    /// `pairs` is empty.
    #[expect(
        clippy::missing_inline_in_public_items,
        reason = "inlining the scoring loop would duplicate the whole body"
    )]
    pub fn score_frames<T: Pixel>(
        &mut self,
        pairs: &[(v_frame::frame::Frame<T>, v_frame::frame::Frame<T>)],
    ) -> Result<Vec<f64>, VmafError> {
        if pairs.is_empty() {
            return Err(VmafError::InvalidConfiguration {
                reason: "no frame pairs were given to score".to_owned(),
            });
        }

        for (reference, distorted) in pairs {
            self.submit_pair(reference, distorted)?;
        }

        self.finish()
    }

    /// Submit a single frame pair and return its index.
    ///
    /// This is the low-level primitive behind [`Self::score_frames`] and
    /// [`Self::score_decoders`]. Prefer those unless the caller needs to
    /// interleave decoding with submission, which is what makes memory use
    /// independent of clip length: only libvmaf's picture pool and the one pair
    /// in flight are live, rather than every decoded frame.
    ///
    /// The index must be sequential from zero. libvmaf's temporal extractors
    /// compare each frame against its predecessor, so the order of calls is the
    /// order the model sees.
    ///
    /// # Errors
    ///
    /// Returns [`VmafError::InputMismatch`] for geometry or storage width that
    /// disagrees with the configuration (u8 for 8-bit; u16 for 10/12/16-bit),
    /// before copying/submitting either frame. Also reports malformed layouts
    /// and native errors. Failed submissions do not advance the scored count.
    /// If replacement acquisition fails after native acceptance, this scorer
    /// becomes unusable; create a new scorer instead of retrying or reading scores.
    /// Submissions after successful finalization are rejected before copying.
    #[inline]
    pub fn submit_pair<T: Pixel>(
        &mut self,
        reference: &v_frame::frame::Frame<T>,
        distorted: &v_frame::frame::Frame<T>,
    ) -> Result<u32, VmafError> {
        let index = u32::try_from(self.scored).map_err(|_| VmafError::InvalidConfiguration {
            reason: "too many frame pairs to index".to_owned(),
        })?;

        self.submit(reference, distorted, index)?;
        self.scored += 1;

        Ok(index)
    }

    /// Finish a [`Self::submit_pair`] sequence and collect every score.
    ///
    /// Flushing runs the extractors to completion. Repeated calls return the
    /// full final vector without flushing again; no more pairs may be submitted.
    ///
    /// # Errors
    ///
    /// Returns [`VmafError::CallFailed`] on flush failure, or
    /// [`VmafError::ScoreQueryFailed`] with the native status and index on a final
    /// query failure. No shortened vector is returned.
    ///
    /// [`VmafError::InvalidConfiguration`] if no pair was submitted.
    #[inline]
    pub fn finish(&mut self) -> Result<Vec<f64>, VmafError> {
        self.ensure_active()?;
        if self.scored == 0 {
            return Err(VmafError::InvalidConfiguration {
                reason: "no frame pairs were given to score".to_owned(),
            });
        }

        self.flush()?;

        let total = u32::try_from(self.scored).map_err(|_| VmafError::InvalidConfiguration {
            reason: "too many frame pairs to index".to_owned(),
        })?;

        let mut scores = Vec::with_capacity(self.scored);
        for index in 0..total {
            let frame = self.score_at(index)?;
            scores.push(frame.score);
        }

        Ok(scores)
    }

    /// Read final scores from `from` in index order, without flushing.
    ///
    /// Before successful [`Self::finish`], returns empty without querying native
    /// scores: interim readiness is not guaranteed for stock temporal models.
    /// Afterward, advance `from` by the returned length to avoid duplicate reads.
    /// The cursor belongs to the caller; repeating the same cursor rereads scores.
    ///
    /// # Errors
    ///
    /// Returns [`VmafError::ScoreQueryFailed`] if a final native query fails.
    #[inline]
    pub fn drain_scores(&mut self, from: usize) -> Result<Vec<f64>, VmafError> {
        self.ensure_active()?;
        if !self.flushed {
            return Ok(Vec::new());
        }
        let total = u32::try_from(self.scored).map_err(|_| VmafError::InvalidConfiguration {
            reason: "too many frame pairs to index".to_owned(),
        })?;

        let mut scores = Vec::new();
        for index in u32::try_from(from).unwrap_or(u32::MAX)..total {
            let frame = self.score_at(index)?;
            scores.push(frame.score);
        }

        Ok(scores)
    }

    /// Submit one frame pair from raw plane sources.
    ///
    /// The entry point for callers whose frames are not `v_frame::Frame`s. A
    /// VapourSynth `Frame` is the case that matters: its planes are already
    /// strided buffers, so pointing at them directly avoids the per-plane copy
    /// that `av_decoders` performs to produce a `v_frame::Frame` -- roughly
    /// 5.9 MB of copying per 1080p frame, per side.
    ///
    /// This is the same path `v_frame` callers take: both reduce to three
    /// [`PlaneSource`]s, so there is one copy loop and one set of validation.
    ///
    /// # Errors
    ///
    /// Borrows the validated planes only during this call; no source pointer
    /// is retained. Geometry and sample width on both sides must match the
    /// configuration (1 byte for 8-bit, 2 for 10/12/16-bit) before any copy.
    /// Returns [`VmafError::InputMismatch`] on disagreement, malformed-picture
    /// errors, or [`VmafError::CallFailed`] if libvmaf rejects the pair.
    /// Errors do not advance the scored count. Replacement acquisition failure
    /// after native acceptance makes the scorer unusable, including score reads.
    /// Submissions after successful finalization are rejected before copying.
    #[inline]
    pub fn submit_pair_raw(
        &mut self,
        reference: PlaneSet<'_>,
        distorted: PlaneSet<'_>,
    ) -> Result<u32, VmafError> {
        let index = u32::try_from(self.scored).map_err(|_| VmafError::InvalidConfiguration {
            reason: "too many frame pairs to index".to_owned(),
        })?;

        self.submit_planes(index, reference, distorted)?;
        self.scored += 1;

        Ok(index)
    }

    /// Pooled value of each requested feature over the whole clip.
    ///
    /// Only valid after scoring has finished, since the features are produced
    /// as frames are read. Each requested feature that libvmaf could
    /// compute appears in the result; unsupported ones are omitted.
    ///
    /// # Errors
    ///
    /// Returns [`VmafError::CallFailed`] if libvmaf rejects the query.
    #[expect(
        clippy::missing_inline_in_public_items,
        reason = "a cold reporting path that would bloat every call site"
    )]
    pub fn pooled_features(&self) -> Result<Vec<(String, f64)>, VmafError> {
        self.ensure_active()?;
        if self.scored == 0 {
            return Ok(Vec::new());
        }

        let last = (self.scored - 1) as u32;
        let mut features = Vec::with_capacity(self.feature_names.len());

        for name in &self.feature_names {
            let mut value = 0.0f64;
            // SAFETY: `name` is a valid NUL-terminated string, the pool method
            // is valid, the index range covers every submitted
            // frame, and `value` is a valid out-pointer.
            let status = unsafe {
                (self.context.api().feature_score_pooled)(
                    self.context.raw,
                    name.as_ptr(),
                    PoolMethod::Mean.as_libvmaf_pool(),
                    &mut value,
                    0,
                    last,
                )
            };
            ffi::check(status, "vmaf_feature_score_pooled")?;
            features.push((name.to_string_lossy().into_owned(), value));
        }

        Ok(features)
    }
}

/// libvmaf takes a mutable pointer to the picture even though it only reads it.
#[inline]
fn raw_mut(picture: &mut VmafPicture) -> *mut VmafPicture {
    picture as *mut VmafPicture
}

/// Out-pointer for a model handle.
#[inline]
fn raw_mut_model(model: &mut *mut VmafModelHandle) -> *mut *mut VmafModelHandle {
    std::ptr::from_mut(model)
}

/// Mutable pointer to a model configuration.
#[inline]
fn raw_mut_config(config: &mut VmafModelConfig) -> *mut VmafModelConfig {
    config as *mut VmafModelConfig
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "assertions read better with unwrap; a failure panics with a usable message"
)]
mod tests;
