//! Streaming per-pair vship scoring.
//!
//! libvship scores a reference/distorted pair through `Vship_ComputeHandler`
//! and writes the result into the struct it is handed. There is no sliding
//! window and no flush: the score for a pair is final the moment the call
//! returns. That is the central difference from the libvmaf binding, and it is
//! why no finish or batch step appears here.

use std::{
    ffi::{CString, c_char, c_int, c_void},
    marker::PhantomData,
    mem::size_of,
    ptr,
    sync::{Mutex, OnceLock},
};

use av_decoders::{
    ChromaLocation, ColorPrimaries, ColorRange, ColorSpace, Decoder, TransferCharacteristic,
    VideoDetails, v_frame::pixel::Pixel,
};
use v_frame::chroma::ChromaSubsampling;

use crate::{
    config::{
        ButteraugliParams, PoolMethod, VshipConfig, VshipMetric, colorspace_from_details,
        to_c_string,
    },
    error::VshipError,
    ffi::{
        self, VshipApi, VshipBackend, VshipColorspace, VshipDeviceInfo, VshipHandle,
        VshipInitButteraugli_1, VshipInitCvvdp_1, VshipInitSsimulacra2_1, VshipScoreButteraugli,
        VshipScoreCvvdp, VshipScoreSsimulacra2, VshipStructType,
    },
};

/// Sentinel for "no device was configured", so the default can be chosen rather
/// than assumed.
///
/// [`VshipConfig::gpu_id`] returns this until [`default_gpu_id`] has run, and
/// [`VshipConfig::gpu_id_or_default`] resolves it.
pub(crate) const GPU_UNSET: u32 = u32::MAX;

/// The device libvship would use when none is configured.
///
/// A machine with more than one GPU usually has a discrete part plus an
/// integrated one, and the discrete part is the one that can actually run these
/// metrics at a useful rate. libvship reports `integrated` for every backend,
/// so the highest-indexed non-integrated device is chosen, falling back to the
/// last device enumerated when every one reports integrated.
///
/// Returns `None` when libvship cannot be loaded or reports no devices at all,
/// rather than substituting an index. Every index this can return was read back
/// from `device_count`, so a zero here means device 0 does not exist, and
/// handing it to `gpu_full_check` or reporting it as the default would name a
/// device that is not there.
///
/// The result is cached in a `OnceLock`: the device configuration cannot change
/// while the process runs, so re-enumerating on each request would be waste.
#[inline]
#[must_use]
pub fn default_gpu_id() -> Option<u32> {
    static CACHE: OnceLock<Option<u32>> = OnceLock::new();
    *CACHE.get_or_init(choose_gpu_id)
}

/// Enumerate devices and pick a discrete one, or the last one available.
fn choose_gpu_id() -> Option<u32> {
    let api = VshipApi::load().ok()?;
    let count = api.device_count().ok()?;

    (0..count)
        .rev()
        .find(|&id| api.device_info(id).is_ok_and(|info| info.integrated == 0))
        .or_else(|| count.checked_sub(1))
}

/// Every compute device libvship can see, in libvship's own order.
///
/// The index of each device is the `gpu_id` that
/// [`VshipConfig::with_gpu_id`](crate::VshipConfig::with_gpu_id) takes, so this
/// is what a caller needs in order to select one by hand.
///
/// A device whose properties cannot be read is omitted rather than reported
/// with blanks. Each entry keeps the index it was enumerated at, so omitting
/// one never renumbers the others: a caller holding `gpu_id` 2 still finds
/// device 2 in this list.
///
/// # Errors
///
/// Returns [`VshipError::LibraryNotFound`] if libvship could not be loaded, and
/// [`VshipError::CallFailed`] if the device count could not be read.
#[inline]
pub fn devices() -> Result<Vec<VshipDevice>, VshipError> {
    let api = VshipApi::load()?;
    let count = api.device_count()?;

    Ok((0..count)
        .filter_map(|id| api.device_info(id).ok().map(|info| VshipDevice { id, info }))
        .collect())
}

/// One compute device libvship enumerated.
///
/// No `PartialEq`: [`VshipDeviceInfo`] is a raw `#[repr(C)]` mirror of a C
/// struct and does not derive it. Compare `id` and the fields read off `info`
/// instead.
#[derive(Debug, Clone, Copy)]
pub struct VshipDevice {
    /// The device's index, which is the `gpu_id` that selects it.
    pub id: u32,
    /// What libvship reports about it.
    pub info: VshipDeviceInfo,
}

impl VshipDevice {
    /// Whether libvship classifies this device as integrated.
    #[inline]
    #[must_use]
    pub fn is_integrated(&self) -> bool {
        self.info.integrated != 0
    }
}

/// Geometry and format of one input.
///
/// The reference and the encode are described separately, because libvship
/// takes a colorspace per side and converts each independently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoFormat {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Bits per channel: 8, 9, 10, 12, 14 or 16.
    pub bit_depth: u32,
    /// Chroma subsampling.
    pub chroma_sampling: ChromaSubsampling,
    /// Decoder-reported range; only unknown values use the limited-range default.
    pub color_range: Option<ColorRange>,
    /// Decoder-reported primaries.
    pub color_primaries: Option<ColorPrimaries>,
    /// Decoder-reported transfer function.
    pub color_transfer_characteristic: Option<TransferCharacteristic>,
    /// Decoder-reported YUV matrix.
    pub color_space: Option<ColorSpace>,
    /// Decoder-reported chroma siting.
    pub chroma_location: Option<ChromaLocation>,
}

impl VideoFormat {
    /// Build a format from a decoder's reported details.
    #[inline]
    #[must_use]
    pub fn from_details(details: &VideoDetails) -> Self {
        Self {
            width: details.width as u32,
            height: details.height as u32,
            bit_depth: details.bit_depth as u32,
            chroma_sampling: details.chroma_sampling,
            color_range: details.color_range,
            color_primaries: details.color_primaries,
            color_transfer_characteristic: details.color_transfer_characteristic,
            color_space: details.color_space,
            chroma_location: details.chroma_location,
        }
    }

    /// The libvship colorspace for this format, optionally scaled to a target.
    ///
    /// # Errors
    ///
    /// Returns [`VshipError::UnsupportedFormat`] if the bit depth, chroma
    /// sampling or explicit color metadata has no libvship equivalent.
    #[inline]
    pub fn colorspace(
        &self,
        target_resolution: Option<(u32, u32)>,
    ) -> Result<VshipColorspace, VshipError> {
        colorspace_from_details(
            &VideoDetails {
                width: self.width as usize,
                height: self.height as usize,
                bit_depth: self.bit_depth as usize,
                chroma_sampling: self.chroma_sampling,
                frame_rate: av_decoders::Rational32::new(0, 1),
                total_frames: None,
                color_range: self.color_range,
                color_primaries: self.color_primaries,
                color_transfer_characteristic: self.color_transfer_characteristic,
                color_space: self.color_space,
                chroma_location: self.chroma_location,
                mastering_display: None,
                content_light_level: None,
            },
            target_resolution,
        )
    }
}

/// A borrowed plane with a checked contiguous native-upload span.
///
/// Dimensions count samples; stride, origin and readable length count bytes.
/// libvship uploads inter-row gaps but not the final row's trailing padding.
/// Foreign padding need not be an initialized Rust slice.
/// Positive row pitch must be sample-aligned; the host byte origin need not be.
///
/// A safe frame view cannot escape its owner:
/// ```compile_fail,E0597
/// use av_metrics_vship::PlaneSource;
/// use v_frame::{frame::FrameBuilder, chroma::ChromaSubsampling};
/// let plane;
/// {
///     let frame = FrameBuilder::new(2, 2, ChromaSubsampling::Yuv444, 8)
///         .build::<u8>().unwrap();
///     plane = PlaneSource::from_plane(&frame.y_plane).unwrap();
/// }
/// std::hint::black_box(plane);
/// ```
/// Validated geometry cannot be mutated:
/// ```compile_fail,E0616
/// use av_metrics_vship::PlaneSource;
/// let data = [0u8; 4];
/// let mut plane = PlaneSource::from_bytes(&data, 2, 2, 2, 0, 1).unwrap();
/// plane.stride = -2;
/// ```
/// Raw storage requires an explicit unsafe obligation:
/// ```compile_fail,E0133
/// use av_metrics_vship::PlaneSource;
/// let data = [0u8; 4];
/// let plane = PlaneSource::from_raw_parts(data.as_ptr(), data.len(), 2, 2, 2, 0, 1);
/// ```
#[derive(Debug, Clone, Copy)]
pub struct PlaneSource<'a> {
    /// Width in pixels, that is samples per row rather than bytes.
    width: usize,
    /// Height in pixels.
    height: usize,
    /// Positive byte spacing between rows; native upload cannot walk backwards.
    stride: isize,
    /// Offset of the first visible sample of row 0 from `data`.
    data_origin: usize,
    /// Bytes per sample, as libvship sees them.
    bytes_per_sample: usize,
    /// Pointer to the start of the underlying allocation.
    data: *const u8,
    readable_len: usize,
    _storage: PhantomData<&'a [u8]>,
}

impl<'a> PlaneSource<'a> {
    /// Borrow initialized bytes, with byte stride/origin and sample dimensions.
    ///
    /// # Errors
    /// Rejects zero/nonnative dimensions, sample sizes other than 1/2, nonpositive
    /// or short/sample-unaligned strides, short spans, and overflowing signed/native offsets.
    #[inline]
    pub fn from_bytes(
        data: &'a [u8],
        width: usize,
        height: usize,
        stride: isize,
        data_origin: usize,
        bytes_per_sample: usize,
    ) -> Result<Self, VshipError> {
        // SAFETY: the slice supplies the actual readable allocation and borrow.
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

    /// Borrow native-endian u16 storage; stride and origin still count BYTES.
    ///
    /// # Errors
    /// Returns the layout errors described by [`Self::from_bytes`].
    #[inline]
    pub fn from_u16(
        data: &'a [u16],
        width: usize,
        height: usize,
        stride: isize,
        data_origin: usize,
    ) -> Result<Self, VshipError> {
        // SAFETY: initialized u16 has no padding; the slice supplies its actual span.
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

    /// Borrow a frame plane without manufacturing a byte slice over its padding.
    ///
    /// # Errors
    /// Returns layout errors as in [`Self::from_bytes`], including byte-scaling overflow.
    #[inline]
    pub fn from_plane<T: Pixel>(plane: &'a v_frame::plane::Plane<T>) -> Result<Self, VshipError> {
        let geometry = plane.geometry();
        let bytes = size_of::<T>();
        let stride = geometry
            .stride()
            .checked_mul(bytes)
            .and_then(|n| isize::try_from(n).ok())
            .ok_or_else(|| malformed("frame stride overflow"))?;
        let origin = geometry
            .data_origin()
            .checked_mul(bytes)
            .ok_or_else(|| malformed("frame origin overflow"))?;
        // SAFETY: Plane owns the allocation and exposes its actual sample length.
        // The borrow, not a guessed extent, keeps it live through native compute.
        unsafe {
            Self::from_raw_parts(
                plane.data().as_ptr().cast(),
                std::mem::size_of_val(plane.data()),
                geometry.width(),
                geometry.height(),
                stride,
                origin,
                bytes,
            )
        }
    }

    /// Describe foreign storage with its actual byte span, without reading it.
    ///
    /// # Errors
    /// Returns the layout errors described by [`Self::from_bytes`], also rejecting
    /// null pointers and spans beyond `isize::MAX`.
    ///
    /// # Safety
    /// On success, bind `'a` to one live readable allocation covering `readable_len`
    /// bytes from `data`, without mutation throughout `'a` and the synchronous native
    /// compute/upload. The entire prefix from visible origin through the last visible
    /// byte, including inter-row gaps, must be readable. Visible samples must be
    /// initialized; foreign padding need not form an initialized Rust byte slice.
    /// Geometry alone is not evidence of an allocation's actual length.
    #[inline]
    pub unsafe fn from_raw_parts(
        data: *const u8,
        readable_len: usize,
        width: usize,
        height: usize,
        stride: isize,
        data_origin: usize,
        bytes_per_sample: usize,
    ) -> Result<Self, VshipError> {
        if width == 0
            || height == 0
            || !matches!(bytes_per_sample, 1 | 2)
            || data.is_null()
            || readable_len > isize::MAX as usize
            || c_int::try_from(width).is_err()
            || c_int::try_from(height).is_err()
            || stride <= 0
            || c_int::try_from(stride).is_err()
        {
            return Err(malformed(
                "invalid native plane dimensions, sample width, pointer, stride or span",
            ));
        }
        // WORKAROUND: Pinned Vulkan dispatch inverts the shader's byte-copy flag:
        // https://codeberg.org/Line-fr/Vship/src/commit/5a627933274196219f782b3b74e13fad47cd6370/src/Vulkan/gpuColorToLinear/anyDepthToFloat.hpp#L26
        // https://codeberg.org/Line-fr/Vship/src/commit/5a627933274196219f782b3b74e13fad47cd6370/src/Vulkan/gpuColorToLinear/shaders/anyDepthToFloat.slang#L48-L78
        // Require aligned pitch on all backends until corrected Vulkan dispatch
        // is the supported minimum and odd-pitch uploads pass the real GPU gate.
        if stride % bytes_per_sample as isize != 0 {
            return Err(malformed("native upload pitch must be sample-aligned"));
        }
        // HIP's int launch expression is (width*height + 255)/256.
        if width.checked_mul(height).is_none_or(|n| n > (c_int::MAX - 255) as usize) {
            return Err(malformed("native plane pixel count overflow"));
        }
        let row_bytes = width
            .checked_mul(bytes_per_sample)
            .and_then(|n| isize::try_from(n).ok())
            .ok_or_else(|| malformed("plane row size overflow"))?;
        let origin =
            isize::try_from(data_origin).map_err(|_| malformed("plane origin overflow"))?;
        let end = isize::try_from(height - 1)
            .ok()
            .and_then(|n| n.checked_mul(stride))
            .and_then(|n| origin.checked_add(n))
            .and_then(|n| n.checked_add(row_bytes))
            .ok_or_else(|| malformed("native upload extent overflow"))?;
        if stride < row_bytes || end as usize > readable_len {
            return Err(malformed("short plane stride or native upload span"));
        }
        Ok(Self {
            width,
            height,
            stride,
            data_origin,
            bytes_per_sample,
            data,
            readable_len,
            _storage: PhantomData,
        })
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

    /// Bytes per sample.
    #[inline]
    #[must_use]
    pub const fn bytes_per_sample(&self) -> usize {
        self.bytes_per_sample
    }

    /// Row spacing in bytes.
    #[inline]
    #[must_use]
    pub const fn stride(&self) -> isize {
        self.stride
    }
    /// Visible origin in bytes.
    #[inline]
    #[must_use]
    pub const fn data_origin(&self) -> usize {
        self.data_origin
    }
    /// Actual readable allocation span; foreign padding may be uninitialized.
    #[inline]
    #[must_use]
    pub const fn readable_len(&self) -> usize {
        self.readable_len
    }
    /// Allocation start. Dereferencing requires unsafe code.
    #[inline]
    #[must_use]
    pub const fn data_ptr(&self) -> *const u8 {
        self.data
    }
}

fn malformed(reason: &str) -> VshipError {
    VshipError::MalformedFrame {
        reason: reason.to_owned(),
    }
}

/// The validated pointers and byte strides for one synchronous native call.
struct PlaneArguments {
    pointers: [*const u8; 3],
    strides: [i64; 3],
}

/// Keep validation and native dispatch private; callers only supply borrowed planes.
fn plane_arguments(planes: &PlaneSet<'_>) -> Result<PlaneArguments, VshipError> {
    let mut arguments = PlaneArguments {
        pointers: [ptr::null(); 3],
        strides: [0; 3],
    };
    for (index, plane) in planes.iter().enumerate() {
        arguments.strides[index] =
            i64::try_from(plane.stride).map_err(|_| malformed("native stride overflow"))?;
        // SAFETY: the immutable constructor validated the visible origin and entire
        // upload prefix within the borrowed allocation. No slice over padding exists.
        arguments.pointers[index] = unsafe { plane.data.add(plane.data_origin) };
    }
    Ok(arguments)
}

/// A frame as the three planes libvship scores.
///
/// Every representation a caller might have reduces to this, which is why the
/// scorer needs no per-source special cases.
pub type PlaneSet<'a> = [PlaneSource<'a>; 3];

/// A single pair's score.
///
/// The fields present depend on the metric; the unused ones are `None` rather
/// than a zero, since a zero would be indistinguishable from a real score.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FrameScore {
    /// SSIMULACRA2's value, or CVVDP's cumulative prefix (independent when
    /// temporal processing is disabled). `None` for Butteraugli.
    pub score: Option<f64>,
    /// Butteraugli's configured-norm value.
    pub norm_q: Option<f64>,
    /// Butteraugli's L3 norm.
    pub norm_3: Option<f64>,
    /// Butteraugli's Linfinity norm.
    pub norm_inf: Option<f64>,
}

impl FrameScore {
    /// The value to pool or report for this pair.
    ///
    /// For Butteraugli this is [`Self::butteraugli_value`] rather than a fixed
    /// field, because which norm is the headline depends on the configuration
    /// and not on the metric: `q_norm` sets the exponent libvship
    /// minimises, while the VapourSynth plugin reports `norm_inf` unless a
    /// norm was requested explicitly.
    #[inline]
    #[must_use]
    pub const fn value(&self, metric: VshipMetric) -> Option<f64> {
        match metric {
            VshipMetric::Butteraugli => self.butteraugli_value(true),
            VshipMetric::Ssimulacra2 | VshipMetric::Cvvdp => self.score,
        }
    }

    /// The value to pool or report for Butteraugli.
    ///
    /// With `q_norm` set, libvship's Q-norm is the quantity the metric
    /// minimises and is what should be pooled. Without it, the reported
    /// value is the infinity norm: that is what the VapourSynth plugin
    /// exposes as `BUTTERAUGLI_INFNorm` when no norm is requested, and it
    /// is what `av-metrics-vship`'s caller is expected to pool, so that
    /// both paths agree.
    #[inline]
    #[must_use]
    pub const fn butteraugli_value(&self, use_q_norm: bool) -> Option<f64> {
        if use_q_norm {
            self.norm_q
        } else {
            self.norm_inf
        }
    }

    /// The headline value, or `None` when the metric produced none.
    #[inline]
    #[must_use]
    pub fn primary(&self) -> Option<f64> {
        // `Option::or` is not const, so the first present value is found by
        // hand.
        if self.score.is_some() {
            self.score
        } else {
            self.norm_q
        }
    }
}

/// The NUL-terminated strings CVVDP's init struct points at.
///
/// Held beside the pool so the pointers libvship is given outlive the calls
/// that use them.
struct CvvdpStrings {
    /// Display model key, always present.
    model_key: CString,
    /// Display model JSON path, or null when none is configured.
    model_config_json: *const c_char,
    /// Backing storage for `model_config_json`, kept alive beside the pointer.
    _model_config_json_storage: Option<CString>,
}

impl CvvdpStrings {
    /// Build the strings from a configuration.
    fn new(config: &VshipConfig) -> Result<Self, VshipError> {
        let model_key = to_c_string(&config.cvvdp().display_model, "display model")?;
        let json = config.cvvdp().display_model_json.as_str();

        let (model_config_json, storage) = if json.is_empty() {
            (ptr::null(), None)
        } else {
            let storage = to_c_string(json, "display model JSON path")?;
            (storage.as_ptr(), Some(storage))
        };

        Ok(Self {
            model_key,
            model_config_json,
            _model_config_json_storage: storage,
        })
    }
}

/// One libvship handler plus the colorspaces it was built for.
///
/// The handler pointer is owned here and released exactly once by [`Drop`]. It
/// and its scorer remain `!Send` and `!Sync`: native handle ownership is
/// restricted to the creating thread.
struct MetricHandler {
    handle: VshipHandle,
    src_colorspace: VshipColorspace,
    dis_colorspace: VshipColorspace,
}

impl MetricHandler {
    /// Create a handler for `metric` and the given colorspaces.
    fn new(
        api: &'static VshipApi,
        metric: VshipMetric,
        config: &VshipConfig,
        src_colorspace: VshipColorspace,
        dis_colorspace: VshipColorspace,
        cvvdp_strings: &CvvdpStrings,
    ) -> Result<Self, VshipError> {
        let gpu_id = config.gpu_id_or_default() as c_int;
        let mut handle: VshipHandle = ptr::null_mut();

        // The init struct is metric-agnostic on the C side: `Vship_InitHandler`
        // reads its `struct_type` discriminator to learn which layout it was
        // handed, so all three variants are built here and one is selected.
        let result = api.call("Vship_InitHandler", || match metric {
            VshipMetric::Ssimulacra2 => {
                let init = VshipInitSsimulacra2_1 {
                    struct_type: VshipStructType::InitSsimulacra2,
                    src_colorspace,
                    dis_colorspace,
                    gpu_id,
                };
                // SAFETY: `init` is a live `VshipInitSsimulacra2_1` whose
                // discriminator matches the metric, and `handle` is a writable
                // out-pointer for one `Vship_Handle`.
                unsafe {
                    (api.init_handler)(
                        &raw mut handle,
                        std::ptr::from_ref(&init).cast_mut().cast::<c_void>(),
                    )
                }
            },
            VshipMetric::Butteraugli => {
                let ButteraugliParams {
                    q_norm,
                    intensity_multiplier,
                } = config.butteraugli();

                let init = VshipInitButteraugli_1 {
                    struct_type: VshipStructType::InitButteraugli,
                    src_colorspace,
                    dis_colorspace,
                    q_norm: *q_norm,
                    intensity_multiplier: *intensity_multiplier,
                    gpu_id,
                };
                // SAFETY: as above, for the Butteraugli layout.
                unsafe {
                    (api.init_handler)(
                        &raw mut handle,
                        std::ptr::from_ref(&init).cast_mut().cast::<c_void>(),
                    )
                }
            },
            VshipMetric::Cvvdp => {
                let init = VshipInitCvvdp_1 {
                    struct_type: VshipStructType::InitCvvdp,
                    src_colorspace,
                    dis_colorspace,
                    fps: config.cvvdp().fps,
                    resize_to_display: config.cvvdp().resize_to_display,
                    model_key_cstr: cvvdp_strings.model_key.as_ptr(),
                    model_config_json_cstr: cvvdp_strings.model_config_json,
                    gpu_id,
                };
                // SAFETY: as above, for the CVVDP layout. Both string pointers
                // are NUL-terminated and outlive the call through
                // `CvvdpStrings`.
                unsafe {
                    (api.init_handler)(
                        &raw mut handle,
                        std::ptr::from_ref(&init).cast_mut().cast::<c_void>(),
                    )
                }
            },
        });

        if let Err(error) = result {
            return Err(VshipError::MetricUnavailable {
                metric: metric.as_str().to_owned(),
                reason: error.to_string(),
            });
        }

        Ok(Self {
            handle,
            src_colorspace,
            dis_colorspace,
        })
    }

    /// Score one pair and return the value libvship wrote.
    fn compute(
        &mut self,
        api: &'static VshipApi,
        metric: VshipMetric,
        reference: &PlaneArguments,
        distorted: &PlaneArguments,
    ) -> Result<FrameScore, VshipError> {
        // libvship reads three plane pointers and two line-size arrays per
        // side, all of which must stay live for the call.
        let PlaneArguments {
            pointers: srcp1,
            strides: line_size,
        } = reference;
        let PlaneArguments {
            pointers: srcp2,
            strides: line_size2,
        } = distorted;

        let mut score = FrameScore::default();

        match metric {
            VshipMetric::Ssimulacra2 => {
                let mut out = VshipScoreSsimulacra2::default();
                // SAFETY: the argument arrays are live locals of exactly the
                // pointer and `i64` types the signature names, `out` is a
                // writable score struct whose discriminator
                // matches the metric, and the handler is owned
                // by `self`.
                api.call("Vship_ComputeHandler", || unsafe {
                    (api.compute_handler)(
                        self.handle,
                        std::ptr::from_mut(&mut out).cast::<c_void>(),
                        srcp1.as_ptr(),
                        srcp2.as_ptr(),
                        line_size.as_ptr(),
                        line_size2.as_ptr(),
                    )
                })?;
                check_score_tag(out.struct_type, VshipStructType::ScoreSsimulacra2)?;
                score.score = Some(out.score);
            },
            VshipMetric::Butteraugli => {
                // `dstp` stays null, which the C header defines as never
                // copying the distortion map back from the
                // device.
                let mut out = VshipScoreButteraugli::default();
                // SAFETY: as above, for the Butteraugli score layout.
                api.call("Vship_ComputeHandler", || unsafe {
                    (api.compute_handler)(
                        self.handle,
                        std::ptr::from_mut(&mut out).cast::<c_void>(),
                        srcp1.as_ptr(),
                        srcp2.as_ptr(),
                        line_size.as_ptr(),
                        line_size2.as_ptr(),
                    )
                })?;
                check_score_tag(out.struct_type, VshipStructType::ScoreButteraugli)?;
                score.norm_q = Some(out.norm_q);
                score.norm_3 = Some(out.norm_3);
                score.norm_inf = Some(out.norm_inf);
            },
            VshipMetric::Cvvdp => {
                let mut out = VshipScoreCvvdp::default();
                // SAFETY: as above, for the CVVDP score layout.
                api.call("Vship_ComputeHandler", || unsafe {
                    (api.compute_handler)(
                        self.handle,
                        std::ptr::from_mut(&mut out).cast::<c_void>(),
                        srcp1.as_ptr(),
                        srcp2.as_ptr(),
                        line_size.as_ptr(),
                        line_size2.as_ptr(),
                    )
                })?;
                check_score_tag(out.struct_type, VshipStructType::ScoreCvvdp)?;
                score.score = Some(out.score);
            },
        }

        Ok(score)
    }

    /// Clear this handler's temporal frame history.
    fn reset(&self, api: &'static VshipApi) -> Result<(), VshipError> {
        // SAFETY: `handle` is owned by `self` and `Vship_Reset` takes no other
        // arguments.
        api.call("Vship_Reset", || unsafe { (api.reset)(self.handle) })
    }

    /// Clear this handler's accumulated score, leaving its frame history.
    fn reset_score(&self, api: &'static VshipApi) -> Result<(), VshipError> {
        // SAFETY: as `reset`.
        api.call("Vship_ResetScore", || unsafe {
            (api.reset_score)(self.handle)
        })
    }
}

fn check_score_tag(actual: c_int, expected: VshipStructType) -> Result<(), VshipError> {
    if actual != expected as c_int {
        return Err(VshipError::InvalidScoreTag {
            actual,
            expected: expected as c_int,
        });
    }
    Ok(())
}

impl Drop for MetricHandler {
    #[inline]
    fn drop(&mut self) {
        // libvship rejects freeing a null handle, so one that never initialised
        // is skipped rather than passed on.
        if self.handle.is_null() {
            return;
        }

        if let Ok(api) = VshipApi::load() {
            // SAFETY: `handle` came from `Vship_InitHandler` and is released
            // exactly once, here.
            let _ = api.call("Vship_FreeHandler", || unsafe {
                (api.free_handler)(self.handle)
            });
        }
    }
}

/// Check configured geometry/storage before exposing native arguments.
fn validate_planes(
    planes: &PlaneSet<'_>,
    colorspace: &VshipColorspace,
    side: &str,
) -> Result<PlaneArguments, VshipError> {
    let (chroma_width, chroma_height) = chroma_dimensions(colorspace)?;
    let expected_bytes = match colorspace.sample {
        ffi::VshipSample::Uint8 => 1,
        ffi::VshipSample::Uint9
        | ffi::VshipSample::Uint10
        | ffi::VshipSample::Uint12
        | ffi::VshipSample::Uint14
        | ffi::VshipSample::Uint16 => 2,
        sample => {
            return Err(VshipError::UnsupportedFormat {
                reason: format!("{side}: unsupported plane sample type {sample:?}"),
            });
        },
    };

    for (index, plane) in planes.iter().enumerate() {
        let (expected_width, expected_height) = if index == 0 {
            (colorspace.width, colorspace.height)
        } else {
            (chroma_width, chroma_height)
        };

        if i64::try_from(plane.width).unwrap_or(-1) != expected_width
            || i64::try_from(plane.height).unwrap_or(-1) != expected_height
            || plane.bytes_per_sample != expected_bytes
        {
            return Err(VshipError::InputMismatch {
                reason: format!(
                    "{side} plane {index} is {}x{} with {} bytes/sample but its colorspace declares \
                     {expected_width}x{expected_height} with {expected_bytes} bytes/sample",
                    plane.width, plane.height, plane.bytes_per_sample
                ),
            });
        }
    }

    plane_arguments(planes)
}

/// The chroma plane dimensions implied by a colorspace.
#[inline]
fn chroma_dimensions(colorspace: &VshipColorspace) -> Result<(i64, i64), VshipError> {
    if colorspace.width <= 0
        || colorspace.height <= 0
        || c_int::try_from(colorspace.width).is_err()
        || c_int::try_from(colorspace.height).is_err()
    {
        return Err(malformed("invalid native colorspace dimensions"));
    }
    if colorspace
        .width
        .checked_mul(colorspace.height)
        .is_none_or(|n| n > i64::from(c_int::MAX - 255))
    {
        return Err(malformed("native colorspace pixel count overflow"));
    }
    let shift = |value: c_int| {
        u32::try_from(value).map_err(|_| VshipError::MalformedFrame {
            reason: format!("negative chroma subsampling exponent {value}"),
        })
    };

    let subw = shift(colorspace.subsampling.subw)?;
    let subh = shift(colorspace.subsampling.subh)?;

    // Preserve the supported 4:2:0/4:2:2/4:4:4 set before any shift.
    if !matches!((subw, subh), (1, 1) | (1, 0) | (0, 0)) {
        return Err(VshipError::MalformedFrame {
            reason: format!("unsupported chroma subsampling exponent {subw}x{subh}"),
        });
    }

    Ok((
        ((colorspace.width - 1) >> subw) + 1,
        ((colorspace.height - 1) >> subh) + 1,
    ))
}

/// A vship scoring session.
///
/// Scores are produced by streaming matching frame pairs from a reference and a
/// distorted [`Decoder`]. libvship scores a pair as it arrives, so the value
/// returned by [`Self::submit_pair`] is final immediately: there is no flush
/// and no sliding window.
///
/// # Threading
///
/// Every metric owns one handler. Submission is synchronous through `&mut self`;
/// [`VshipConfig::handler_threads`] retains only a legacy request, not a worker
/// count. CVVDP sees frames in submission order; the selection owner requests
/// history resets at source-index discontinuities.
///
/// A scorer is neither [`Send`] nor [`Sync`]. Construct, use and drop it on the
/// same thread rather than moving or sharing its opaque native handle.
///
/// ```compile_fail
/// use av_metrics_vship::VshipScorer;
/// fn require_send<T: Send>() {}
/// require_send::<VshipScorer>();
/// ```
///
/// ```compile_fail
/// use av_metrics_vship::VshipScorer;
/// fn require_sync<T: Sync>() {}
/// require_sync::<VshipScorer>();
/// ```
pub struct VshipScorer {
    api: &'static VshipApi,
    config: VshipConfig,
    metric: VshipMetric,
    handler: MetricHandler,
    /// Successful submissions, not source-frame indices.
    submitted: u32,
}

fn input_format_error(input: &str, error: VshipError) -> VshipError {
    match error {
        VshipError::UnsupportedFormat { reason } => VshipError::UnsupportedFormat {
            reason: format!("{input}: {reason}"),
        },
        error => error,
    }
}

impl VshipScorer {
    /// Create a scorer for the given reference and encode formats.
    ///
    /// `target_resolution` is Av1an's encode resolution, which becomes
    /// libvship's `target_width` and `target_height`.
    ///
    /// The two sides need not share a format, but they must end up the same
    /// size: libvship does not resize implicitly and fails
    /// `Vship_InitHandler` with `DifferingInputType` when the two
    /// colorspaces resolve to different dimensions. When
    /// `target_resolution` is `None` and the formats differ in
    /// size, the reference size is used as the target, so a 1080p reference can
    /// be scored against a 720p encode without the caller having to say so.
    /// Pass an explicit target to scale to some third size instead.
    ///
    /// Default-device construction shares a successful capability probe with
    /// availability checks. An explicitly configured device is checked afresh,
    /// even when it is the default index. Every scorer creates its own handler;
    /// initialization and scoring can fail independently of cached availability.
    ///
    /// # Errors
    ///
    /// Returns [`VshipError::LibraryNotFound`] if libvship could not be loaded,
    /// [`VshipError::CallFailed`] if the device capability check fails,
    /// [`VshipError::InvalidConfiguration`] if the configuration is unusable,
    /// [`VshipError::UnsupportedFormat`] if either format is outside libvship's
    /// supported set, and [`VshipError::MetricUnavailable`] if a handler cannot
    /// be created.
    #[expect(
        clippy::missing_inline_in_public_items,
        reason = "inlining the scorer would duplicate the whole body at every call site"
    )]
    pub fn new(
        config: VshipConfig,
        reference: VideoFormat,
        encode: VideoFormat,
        target_resolution: Option<(u32, u32)>,
    ) -> Result<Self, VshipError> {
        config.validate()?;
        // libvship performs no geometry-equality check between the two sides:
        // it converts each per its own colorspace and then rejects the
        // pair with `DifferingInputType` if the results differ in size.
        // Scaling is therefore explicit, and an explicit target scales
        // both sides to that size. With no target, a mismatched pair
        // would fail, so the reference size is adopted --
        // the same choice FFVship makes, which always tells libvship to scale
        // the encode to the source. Pairs that already match are left
        // untouched.
        let target_resolution = target_resolution.or_else(|| {
            (reference.width != encode.width || reference.height != encode.height)
                .then_some((reference.width, reference.height))
        });

        let src_colorspace = reference
            .colorspace(target_resolution)
            .map_err(|error| input_format_error("reference", error))?;
        let dis_colorspace = encode
            .colorspace(target_resolution)
            .map_err(|error| input_format_error("distorted", error))?;
        // These descriptors also reach InitHandler, before any frame exists.
        chroma_dimensions(&src_colorspace)?;
        chroma_dimensions(&dis_colorspace)?;
        let cvvdp_strings = CvvdpStrings::new(&config)?;

        let api = VshipApi::load()?;
        let gpu_id = config.gpu_id_or_default();
        if !config.has_explicit_gpu_id() && default_gpu_id() == Some(gpu_id) {
            default_gpu_full_check(api, gpu_id)?;
        } else {
            api.gpu_full_check(gpu_id)?;
        }

        let handler = MetricHandler::new(
            api,
            config.metric(),
            &config,
            src_colorspace,
            dis_colorspace,
            &cvvdp_strings,
        )?;

        let metric = config.metric();

        Ok(Self {
            api,
            config,
            metric,
            handler,
            submitted: 0,
        })
    }

    /// The backend this libvship build targets.
    #[inline]
    #[must_use]
    pub fn backend(&self) -> VshipBackend {
        self.api.backend()
    }

    /// The device this scorer was built for.
    #[inline]
    #[must_use]
    pub fn gpu_id(&self) -> u32 {
        self.config.gpu_id_or_default()
    }

    /// The configuration in use.
    #[inline]
    #[must_use]
    pub const fn config(&self) -> &VshipConfig {
        &self.config
    }

    /// The metric being computed.
    #[inline]
    #[must_use]
    pub const fn metric(&self) -> VshipMetric {
        self.metric
    }

    /// How many handlers this synchronous scorer owns: always one.
    #[inline]
    #[must_use]
    pub fn handler_count(&self) -> usize {
        1
    }

    /// Whether libvship is available on this system.
    ///
    /// This needs more than a successful `dlopen`: `Vship_GetVersion` must
    /// answer and `Vship_GPUFullCheck` must pass on a device. Any of those
    /// failing reports unavailability rather than panicking.
    ///
    /// The resolved default device's successful capability check is shared with
    /// version requests and default constructors. Failed checks may retry;
    /// runtime initialization or scoring errors do not invalidate that success.
    #[inline]
    #[must_use]
    pub fn is_available() -> bool {
        default_gpu_id().is_some_and(|gpu_id| availability(gpu_id).is_ok())
    }

    /// The runtime libvship version string, if available.
    ///
    /// Borrows process-lifetime storage and shares the default capability cache
    /// with [`Self::is_available`].
    #[inline]
    #[must_use]
    pub fn libvship_version() -> Option<&'static str> {
        default_gpu_id().and_then(|gpu_id| availability(gpu_id).ok())
    }

    /// The name of the device libvship would use, if it is available.
    #[inline]
    #[must_use]
    pub fn device_name() -> Option<String> {
        let api = VshipApi::load().ok()?;
        api.device_info(default_gpu_id()?).ok().map(|info| info.name())
    }

    /// What libvship reports about the device it would use.
    ///
    /// # Errors
    ///
    /// Returns [`VshipError::LibraryNotFound`] if libvship could not be loaded,
    /// and [`VshipError::CallFailed`] if no device is available or the device
    /// cannot be queried.
    #[inline]
    pub fn device_info() -> Result<VshipDeviceInfo, VshipError> {
        let api = VshipApi::load()?;
        let gpu_id = default_gpu_id().ok_or_else(|| VshipError::CallFailed {
            function: "Vship_GetDeviceCount",
            status: 0,
            message: "libvship reported no compute devices".to_owned(),
        })?;

        api.device_info(gpu_id)
    }

    /// Score every matching frame pair from the two decoders.
    ///
    /// Returns one [`FrameScore`] per pair, in submission order. Each is
    /// complete the moment `Vship_ComputeHandler` returns, so the callback
    /// fires per pair and the final score needs no flush.
    ///
    /// # Errors
    ///
    /// Returns [`VshipError::FrameCountMismatch`] if the clips have different
    /// lengths, and [`VshipError::Decode`] or [`VshipError::CallFailed`] on
    /// downstream failures. Returns [`VshipError::InputMismatch`] when the decoded
    /// sample storage does not match either configured colorspace.
    #[expect(
        clippy::missing_inline_in_public_items,
        reason = "inlining the scoring loop would duplicate the whole body"
    )]
    pub fn score_decoders<T: Pixel>(
        &mut self,
        reference: &mut Decoder,
        distorted: &mut Decoder,
        mut on_score: impl FnMut(usize, &FrameScore),
    ) -> Result<Vec<FrameScore>, VshipError> {
        let mut scores = Vec::new();

        loop {
            let reference_frame = reference.read_video_frame::<T>();
            let distorted_frame = distorted.read_video_frame::<T>();

            let (reference_frame, distorted_frame) = match (reference_frame, distorted_frame) {
                (Ok(reference_frame), Ok(distorted_frame)) => (reference_frame, distorted_frame),
                (reference_result, distorted_result) => {
                    // `av-decoders` signals end of stream through
                    // `DecoderError`, not an `Option`, so a short read is only
                    // distinguishable via its error.
                    let reference_done =
                        matches!(reference_result, Err(av_decoders::DecoderError::EndOfFile));
                    let distorted_done =
                        matches!(distorted_result, Err(av_decoders::DecoderError::EndOfFile));

                    if reference_done && distorted_done {
                        break;
                    }

                    if reference_done || distorted_done {
                        let scored = scores.len();
                        return Err(VshipError::FrameCountMismatch {
                            reference: scored + usize::from(!reference_done),
                            distorted: scored + usize::from(!distorted_done),
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

            let frame_score = self.submit_pair(&reference_frame, &distorted_frame)?;
            on_score(scores.len(), &frame_score);
            scores.push(frame_score);
        }

        Ok(scores)
    }

    /// Reduce a scorer's ordered scores to one value.
    ///
    /// CVVDP is assumed cumulative (the native default): return the final
    /// score regardless of `method`. For `disable_temporal=true`, extract
    /// [`FrameScore::value`] for each frame and call [`PoolMethod::apply`].
    /// Other metrics retain their configured per-frame pooling.
    ///
    /// # Errors
    ///
    /// Returns [`VshipError::InvalidConfiguration`] if no values are available,
    /// or if the final cumulative CVVDP entry has no value. An earlier prefix
    /// cannot stand in for the whole session.
    #[inline]
    pub fn pool(
        scores: &[FrameScore],
        metric: VshipMetric,
        method: PoolMethod,
    ) -> Result<f64, VshipError> {
        if metric == VshipMetric::Cvvdp {
            return scores.last().and_then(|score| score.value(metric)).ok_or_else(|| {
                VshipError::InvalidConfiguration {
                    reason: "final cumulative CVVDP score is missing".to_owned(),
                }
            });
        }
        let values: Vec<f64> = scores.iter().filter_map(|score| score.value(metric)).collect();

        method.apply(&values)
    }

    /// Pool Butteraugli using an explicitly chosen norm.
    ///
    /// [`Self::pool`] always takes the Q-norm for Butteraugli, which is right
    /// when `q_norm` is configured. A caller that leaves the norm unset
    /// needs [`FrameScore::norm_inf`] instead, to match what the
    /// VapourSynth plugin reports, and this is the entry point that selects
    /// it.
    ///
    /// # Errors
    ///
    /// Returns [`VshipError::InvalidConfiguration`] if no score produced a
    /// value for `method`.
    #[inline]
    pub fn pool_butteraugli(
        scores: &[FrameScore],
        use_q_norm: bool,
        method: PoolMethod,
    ) -> Result<f64, VshipError> {
        let values: Vec<f64> =
            scores.iter().filter_map(|score| score.butteraugli_value(use_q_norm)).collect();

        method.apply(&values)
    }

    /// Submit one frame pair and return its score.
    ///
    /// This is the low-level primitive behind [`Self::score_decoders`]. Prefer
    /// that unless the caller needs to interleave decoding with submission,
    /// which is what makes memory use independent of clip length: only the
    /// handler and the one pair in flight are live.
    ///
    /// Temporal pairs must be submitted in order. Source indices belong to the
    /// selection owner; this scorer cannot infer gaps from submission counts.
    /// With `disable_temporal`, clear history and score after computing each
    /// pair; return its captured result only if both resets succeed.
    ///
    /// # Errors
    ///
    /// Returns [`VshipError::InputMismatch`] for configured geometry/storage
    /// mismatches, [`VshipError::MalformedFrame`] for invalid upload layouts,
    /// and [`VshipError::CallFailed`] if libvship rejects the pair or a required
    /// reset. A native failure may invalidate the handler's progress.
    #[expect(
        clippy::missing_inline_in_public_items,
        reason = "the plane preparation and temporal bookkeeping belong in one body"
    )]
    pub fn submit_pair<T: Pixel>(
        &mut self,
        reference: &v_frame::frame::Frame<T>,
        distorted: &v_frame::frame::Frame<T>,
    ) -> Result<FrameScore, VshipError> {
        // `Frame` exposes chroma planes as `Option`, since monochrome frames
        // have none. libvship requires chroma, so this is a validation
        // failure.
        let (Some(u_plane), Some(v_plane)) = (&reference.u_plane, &reference.v_plane) else {
            return Err(VshipError::UnsupportedFormat {
                reason: "reference frame has no chroma planes; libvship requires 4:2:0, 4:2:2 or \
                         4:4:4 content"
                    .to_owned(),
            });
        };
        let (Some(d_u), Some(d_v)) = (&distorted.u_plane, &distorted.v_plane) else {
            return Err(VshipError::UnsupportedFormat {
                reason: "distorted frame has no chroma planes; libvship requires 4:2:0, 4:2:2 or \
                         4:4:4 content"
                    .to_owned(),
            });
        };

        let reference_planes = [
            PlaneSource::from_plane(&reference.y_plane)?,
            PlaneSource::from_plane(u_plane)?,
            PlaneSource::from_plane(v_plane)?,
        ];
        let distorted_planes = [
            PlaneSource::from_plane(&distorted.y_plane)?,
            PlaneSource::from_plane(d_u)?,
            PlaneSource::from_plane(d_v)?,
        ];

        self.submit_planes(reference_planes, distorted_planes, false)
    }

    /// Submit one frame pair from raw plane sources.
    ///
    /// The entry point for callers whose frames are not `v_frame::Frame`s. A
    /// VapourSynth `Frame` is the case that matters: its planes are already
    /// strided buffers, so pointing at them directly avoids the per-plane copy
    /// that `av_decoders` performs to produce a `v_frame::Frame`.
    ///
    /// # Errors
    ///
    /// Returns [`VshipError::InputMismatch`] for configured geometry/storage
    /// mismatches and [`VshipError::CallFailed`] if libvship rejects the pair.
    #[inline]
    pub fn submit_pair_raw(
        &mut self,
        reference: PlaneSet<'_>,
        distorted: PlaneSet<'_>,
    ) -> Result<FrameScore, VshipError> {
        self.submit_pair_raw_with_reset(reference, distorted, false)
    }

    /// Submit borrowed planes, optionally clearing history for an external index gap.
    ///
    /// Like [`Self::reset_temporal`] followed by [`Self::submit_pair_raw`], but
    /// validates BOTH sides before resetting any native history. The caller still
    /// decides when its external frame selection has a discontinuity.
    ///
    /// # Errors
    /// Returns the validation/compute errors of [`Self::submit_pair_raw`] or a
    /// reset error, without resetting history for an invalid pair.
    #[inline]
    pub fn submit_pair_raw_with_reset(
        &mut self,
        reference: PlaneSet<'_>,
        distorted: PlaneSet<'_>,
        reset_temporal: bool,
    ) -> Result<FrameScore, VshipError> {
        self.submit_planes(reference, distorted, reset_temporal)
    }

    /// How many pairs have completed successfully, including required resets.
    ///
    /// # Errors
    ///
    /// Currently always succeeds; the `Result` is retained for compatibility.
    #[inline]
    pub fn submitted(&self) -> Result<u32, VshipError> {
        Ok(self.submitted)
    }

    /// Clear the temporal handler's frame history, as a scene break would
    /// require.
    ///
    /// The accumulated score survives: the C header defines `Vship_Reset` as
    /// emptying "temporal filter history of temporal metrics (but not score
    /// accumulation)", so the running mean continues across the break. Use
    /// [`Self::reset_score`] as well when a scene's score must cover only
    /// itself.
    ///
    /// Meaningful only for a temporal metric; for the others libvship holds no
    /// frame state to clear, so this is a no-op rather than an error.
    ///
    /// # Errors
    ///
    /// Returns [`VshipError::CallFailed`] if libvship rejects the reset.
    #[inline]
    pub fn reset_temporal(&self) -> Result<(), VshipError> {
        if !self.metric.is_temporal() {
            return Ok(());
        }
        self.handler.reset(self.api)
    }

    /// Clear the temporal handler's accumulated score, leaving its frame
    /// history.
    ///
    /// For non-temporal metrics this is a no-op rather than an error.
    ///
    /// # Errors
    ///
    /// Returns [`VshipError::CallFailed`] if libvship rejects the reset.
    #[inline]
    pub fn reset_score(&self) -> Result<(), VshipError> {
        if !self.metric.is_temporal() {
            return Ok(());
        }
        self.handler.reset_score(self.api)
    }

    /// The single path every submitter funnels through.
    fn submit_planes(
        &mut self,
        reference: PlaneSet<'_>,
        distorted: PlaneSet<'_>,
        reset_temporal: bool,
    ) -> Result<FrameScore, VshipError> {
        // Preflight BOTH sides before any native state or counter mutation.
        let handler = &mut self.handler;
        let reference_arguments =
            validate_planes(&reference, &handler.src_colorspace, "reference")?;
        let distorted_arguments =
            validate_planes(&distorted, &handler.dis_colorspace, "distorted")?;
        if reset_temporal && self.metric.is_temporal() {
            handler.reset(self.api)?;
        }

        let score = handler.compute(
            self.api,
            self.metric,
            &reference_arguments,
            &distorted_arguments,
        )?;
        if self.metric.is_temporal() && self.config.cvvdp().disable_temporal {
            // COMPAT: the plugin captures the value, then clears history and
            // accumulated score in this order before exposing that value.
            handler.reset(self.api)?;
            handler.reset_score(self.api)?;
        }
        self.submitted = self.submitted.wrapping_add(1);
        Ok(score)
    }
}

/// Verify that libvship is loadable and that a device can actually run it.
///
/// # Errors
///
/// Returns [`VshipError::LibraryNotFound`] when the library cannot be loaded,
/// and [`VshipError::CallFailed`] when `Vship_GPUFullCheck` fails.
#[inline]
fn availability(gpu_id: u32) -> Result<&'static str, VshipError> {
    static CACHED_VERSION: OnceLock<String> = OnceLock::new();

    let api = VshipApi::load()?;
    // The version is queried before the device check, so a library that loaded
    // but cannot run reports as unavailable rather than as a working scorer.
    let version = CACHED_VERSION.get_or_init(|| api.version_string());
    default_gpu_full_check(api, gpu_id)?;

    // The static owns the string for the process lifetime, so callers can borrow it.
    Ok(version.as_str())
}

/// Cache only successful capability checks for the resolved default device.
fn default_gpu_full_check(api: &VshipApi, gpu_id: u32) -> Result<(), VshipError> {
    static CHECKED: Mutex<Option<u32>> = Mutex::new(None);

    // Serialize the first success, not just its publication. Failed probes retry;
    // explicit devices and runtime init/compute never use or invalidate this cache.
    // Lock order: this capability cache -> ffi's native-call lock, never the reverse.
    let mut checked = CHECKED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if *checked != Some(gpu_id) {
        api.gpu_full_check(gpu_id)?;
        *checked = Some(gpu_id);
    }
    Ok(())
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "a failing assertion should panic loudly, which is what a unit test wants"
)]
mod tests {
    use super::*;

    #[test]
    fn cvvdp_pool_uses_last_score_independent_of_pool_method() {
        let scores = [2.0, 9.0, 4.0].map(|value| FrameScore {
            score: Some(value),
            norm_q: Some(value * 2.0),
            norm_inf: Some(value * 3.0),
            ..Default::default()
        });
        for method in [PoolMethod::Mean, PoolMethod::Min, PoolMethod::Max] {
            assert_eq!(
                VshipScorer::pool(&scores, VshipMetric::Cvvdp, method).unwrap(),
                4.0
            );
        }
    }

    #[test]
    fn stateless_pooling_applies_requested_method_to_each_metrics_value() {
        let scores = [2.0, 9.0, 4.0].map(|value| FrameScore {
            score: Some(value),
            norm_q: Some(value * 2.0),
            norm_inf: Some(value * 3.0),
            ..Default::default()
        });
        for (method, independent) in
            [(PoolMethod::Mean, 5.0), (PoolMethod::Min, 2.0), (PoolMethod::Max, 9.0)]
        {
            // The explicit independent-frame route uses the same real FrameScores.
            let values: Vec<_> =
                scores.iter().map(|s| s.value(VshipMetric::Cvvdp).unwrap()).collect();
            assert_eq!(method.apply(&values).unwrap(), independent);
            assert_eq!(
                VshipScorer::pool(&scores, VshipMetric::Ssimulacra2, method).unwrap(),
                independent
            );
            assert_eq!(
                VshipScorer::pool(&scores, VshipMetric::Butteraugli, method).unwrap(),
                independent * 2.0
            );
            assert_eq!(
                VshipScorer::pool_butteraugli(&scores, false, method).unwrap(),
                independent * 3.0
            );
        }
    }

    #[test]
    fn cumulative_pool_requires_the_final_metric_value() {
        for scores in [
            vec![],
            vec![
                FrameScore {
                    score: Some(2.0),
                    ..Default::default()
                },
                FrameScore::default(),
            ],
        ] {
            assert!(matches!(
                VshipScorer::pool(&scores, VshipMetric::Cvvdp, PoolMethod::Mean),
                Err(VshipError::InvalidConfiguration { .. })
            ));
        }
    }

    #[test]
    fn plane_validation_rejects_wrong_storage_width_on_every_plane_and_side() {
        let storage = [0u8; 32];
        for sample in [
            ffi::VshipSample::Uint8,
            ffi::VshipSample::Uint9,
            ffi::VshipSample::Uint10,
            ffi::VshipSample::Uint12,
            ffi::VshipSample::Uint14,
            ffi::VshipSample::Uint16,
        ] {
            let mut colorspace = VshipColorspace::bt709_default(2, 2);
            colorspace.sample = sample;
            let expected = if sample == ffi::VshipSample::Uint8 {
                1
            } else {
                2
            };
            for side in ["reference", "distorted"] {
                for index in 0..3 {
                    let mut planes = [
                        PlaneSource::from_bytes(&storage, 2, 2, 4, 0, expected).unwrap(),
                        PlaneSource::from_bytes(&storage, 1, 1, 2, 0, expected).unwrap(),
                        PlaneSource::from_bytes(&storage, 1, 1, 2, 0, expected).unwrap(),
                    ];
                    let plane = planes[index];
                    planes[index] = PlaneSource::from_bytes(
                        &storage,
                        plane.width(),
                        plane.height(),
                        plane.stride(),
                        0,
                        3 - expected,
                    )
                    .unwrap();
                    assert!(
                        validate_planes(&planes, &colorspace, side).is_err(),
                        "{sample:?} {side} {index}"
                    );
                }
            }
        }
    }

    #[test]
    fn safe_u8_frame_cannot_supply_ten_bit_storage() {
        let frame = v_frame::frame::FrameBuilder::new(2, 2, ChromaSubsampling::Yuv420, 8)
            .build::<u8>()
            .unwrap();
        let planes = [
            PlaneSource::from_plane(&frame.y_plane).unwrap(),
            PlaneSource::from_plane(frame.u_plane.as_ref().unwrap()).unwrap(),
            PlaneSource::from_plane(frame.v_plane.as_ref().unwrap()).unwrap(),
        ];
        let mut colorspace = VshipColorspace::bt709_default(2, 2);
        colorspace.sample = ffi::VshipSample::Uint10;
        assert!(validate_planes(&planes, &colorspace, "reference").is_err());
    }

    #[test]
    fn odd_chroma_and_one_pixel_axes_round_up() {
        let mut colorspace = VshipColorspace::bt709_default(1919, 1079);
        assert_eq!(chroma_dimensions(&colorspace).unwrap(), (960, 540));
        colorspace.subsampling = ffi::VshipChromaSubsample::yuv422();
        assert_eq!(chroma_dimensions(&colorspace).unwrap(), (960, 1079));
        colorspace.subsampling = ffi::VshipChromaSubsample::yuv444();
        assert_eq!(chroma_dimensions(&colorspace).unwrap(), (1919, 1079));
        colorspace = VshipColorspace::bt709_default(1, 1);
        assert_eq!(chroma_dimensions(&colorspace).unwrap(), (1, 1));
    }

    #[test]
    fn plane_constructor_rejects_negative_zero_and_short_native_upload_pitch() {
        let storage = [0u8; 32];
        for stride in [-4, isize::MIN, 0, 1] {
            assert!(
                PlaneSource::from_bytes(&storage, 2, 2, stride, 4, 1).is_err(),
                "{stride}"
            );
        }
    }

    #[test]
    fn chroma_dimensions_rejects_zero_negative_and_unrepresentable_geometry() {
        for (width, height) in [(0, 1), (1, 0), (-1, 1), (i64::MAX, 1)] {
            let colorspace = VshipColorspace::bt709_default(width, height);
            assert!(chroma_dimensions(&colorspace).is_err());
        }
    }

    #[test]
    fn chroma_dimensions_rejects_invalid_subsampling_exponents() {
        for exponent in [-1, 2, 9, i32::MAX] {
            let mut colorspace = VshipColorspace::bt709_default(2, 2);
            colorspace.subsampling.subw = exponent;
            assert!(chroma_dimensions(&colorspace).is_err());
            colorspace.subsampling.subw = 1;
            colorspace.subsampling.subh = exponent;
            assert!(chroma_dimensions(&colorspace).is_err());
        }
    }

    #[test]
    fn nonnative_formats_fail_before_library_discovery() {
        for (width, height) in [(u32::MAX as usize, 1), (65536, 65536)] {
            let format =
                VideoFormat::from_details(&details(width, height, 8, ChromaSubsampling::Yuv420));
            assert!(matches!(
                VshipScorer::new(VshipConfig::new(), format, format, None),
                Err(VshipError::MalformedFrame { .. })
            ));
        }
    }

    #[test]
    fn wide_pitch_must_be_sample_aligned_but_byte_pitch_may_be_odd() {
        let storage = [0u8; 32];
        let words = [0u16; 16];
        for stride in [3, 7, 9] {
            assert!(matches!(
                PlaneSource::from_bytes(&storage, 1, 2, stride, 1, 2),
                Err(VshipError::MalformedFrame { .. })
            ));
            assert!(matches!(
                PlaneSource::from_u16(&words, 1, 2, stride, 1),
                Err(VshipError::MalformedFrame { .. })
            ));
            assert!(matches!(
                // SAFETY: actual live allocation; a rejected layout is never uploaded.
                unsafe {
                    PlaneSource::from_raw_parts(storage.as_ptr(), storage.len(), 1, 2, stride, 1, 2)
                },
                Err(VshipError::MalformedFrame { .. })
            ));
        }
        assert!(PlaneSource::from_bytes(&storage, 2, 2, 3, 1, 1).is_ok());
    }

    #[test]
    fn exact_last_visible_prefix_and_unaligned_origin_are_valid() {
        // origin 1 + two row gaps of 10 + six final visible bytes = 27.
        let storage = [0u8; 27];
        let plane = PlaneSource::from_bytes(&storage, 3, 3, 10, 1, 2).unwrap();
        let arguments = plane_arguments(&[plane; 3]).unwrap();
        assert_eq!(arguments.pointers, [storage[1..].as_ptr(); 3]);
        assert_eq!(arguments.strides, [10; 3]);
        assert!(PlaneSource::from_bytes(&storage[..26], 3, 3, 10, 1, 2).is_err());
        let samples = [512u16; 12];
        assert!(PlaneSource::from_u16(&samples, 3, 3, 8, 0).is_ok());
    }

    #[test]
    fn origins_extents_and_native_narrowing_are_checked_without_reads() {
        let data = [0u8; 32];
        // Invalid declarations are rejected before any read or pointer addition.
        for (len, width, height, stride, origin, bytes) in [
            (32, 0, 1, 1, 0, 1),
            (32, 1, 0, 1, 0, 1),
            (32, 1, 1, 1, 0, 0),
            (32, 1, 1, 4, 0, 4),
            (32, 1, 1, 1, usize::MAX, 1),
            (32, 2, 2, 2, isize::MAX as usize, 1),
            (usize::MAX, 1, 1, 1, 0, 1),
            (32, c_int::MAX as usize + 1, 1, 1, 0, 1),
            (32, 1, c_int::MAX as usize + 1, 1, 0, 1),
            (32, 1, 2, (c_int::MAX as isize).saturating_add(1), 0, 1),
            (32, 65536, 65536, 65536, 0, 1),
        ] {
            assert!(
                // SAFETY: all declarations fail validation; no asserted span is used.
                unsafe {
                    PlaneSource::from_raw_parts(
                        data.as_ptr(),
                        len,
                        width,
                        height,
                        stride,
                        origin,
                        bytes,
                    )
                }
                .is_err()
            );
        }
        // SAFETY: null is rejected before use.
        assert!(unsafe { PlaneSource::from_raw_parts(ptr::null(), 1, 1, 1, 1, 0, 1) }.is_err());
    }

    #[test]
    fn integer_plane_storage_must_match_configured_sample_width() {
        let data = [0u8; 8];
        let mut colorspace = VshipColorspace::bt709_default(2, 2);
        let bytes = [
            PlaneSource::from_bytes(&data, 2, 2, 2, 0, 1).unwrap(),
            PlaneSource::from_bytes(&data, 1, 1, 1, 0, 1).unwrap(),
            PlaneSource::from_bytes(&data, 1, 1, 1, 0, 1).unwrap(),
        ];
        assert!(validate_planes(&bytes, &colorspace, "reference").is_ok());
        let words = [512u16; 4];
        let planes = [
            PlaneSource::from_u16(&words, 2, 2, 4, 0).unwrap(),
            PlaneSource::from_u16(&words, 1, 1, 2, 0).unwrap(),
            PlaneSource::from_u16(&words, 1, 1, 2, 0).unwrap(),
        ];
        assert!(validate_planes(&planes, &colorspace, "distorted").is_err());
        for depth in [9, 10, 12, 14, 16] {
            colorspace.sample = crate::sample_for_bit_depth(depth).unwrap();
            assert!(validate_planes(&planes, &colorspace, "distorted").is_ok());
        }
    }

    #[test]
    fn half_and_float_native_sample_formats_are_unsupported() {
        let words = [512u16; 4];
        let planes = [
            PlaneSource::from_u16(&words, 2, 2, 4, 0).unwrap(),
            PlaneSource::from_u16(&words, 1, 1, 2, 0).unwrap(),
            PlaneSource::from_u16(&words, 1, 1, 2, 0).unwrap(),
        ];
        let mut colorspace = VshipColorspace::bt709_default(2, 2);
        for sample in [ffi::VshipSample::Half, ffi::VshipSample::Float] {
            colorspace.sample = sample;
            assert!(matches!(
                validate_planes(&planes, &colorspace, "reference"),
                Err(VshipError::UnsupportedFormat { .. })
            ));
        }
    }

    /// Video details in the shape `av-decoders` reports them.
    fn details(
        width: usize,
        height: usize,
        bit_depth: usize,
        chroma_sampling: ChromaSubsampling,
    ) -> VideoDetails {
        VideoDetails {
            width,
            height,
            bit_depth,
            chroma_sampling,
            frame_rate: av_decoders::Rational32::new(24, 1),
            total_frames: None,
            color_range: None,
            color_primaries: None,
            color_transfer_characteristic: None,
            color_space: None,
            chroma_location: None,
            mastering_display: None,
            content_light_level: None,
        }
    }

    #[test]
    fn video_format_preserves_each_inputs_metadata() {
        use av_decoders::{ColorPrimaries, ColorRange, ColorSpace, TransferCharacteristic};
        let mut reference = details(1920, 1080, 10, ChromaSubsampling::Yuv420);
        reference.color_range = Some(ColorRange::Full);
        reference.color_space = Some(ColorSpace::BT470BG);
        let mut distorted = reference;
        distorted.color_range = Some(ColorRange::Limited);
        distorted.color_space = Some(ColorSpace::BT2020NCL);
        distorted.color_primaries = Some(ColorPrimaries::BT2020);
        distorted.color_transfer_characteristic = Some(TransferCharacteristic::SMPTE2084);
        let reference = VideoFormat::from_details(&reference).colorspace(None).unwrap();
        let distorted = VideoFormat::from_details(&distorted).colorspace(None).unwrap();
        assert_eq!(reference.range, ffi::VshipRange::Full);
        assert_eq!(reference.yuv_matrix, ffi::VshipYuvMatrix::Bt470Bg);
        assert_eq!(distorted.range, ffi::VshipRange::Limited);
        assert_eq!(distorted.yuv_matrix, ffi::VshipYuvMatrix::Bt2020Ncl);
        assert_eq!(distorted.primaries, ffi::VshipPrimaries::Bt2020);
        assert_eq!(distorted.transfer_function, ffi::VshipTransferFunction::Pq);
    }

    #[test]
    fn video_format_reads_decoder_details() {
        let format = VideoFormat::from_details(&details(1920, 1080, 10, ChromaSubsampling::Yuv420));

        assert_eq!(format.width, 1920);
        assert_eq!(format.height, 1080);
        assert_eq!(format.bit_depth, 10);
        assert_eq!(format.chroma_sampling, ChromaSubsampling::Yuv420);
    }

    #[test]
    fn video_format_maps_to_a_colorspace() {
        let format = VideoFormat::from_details(&details(64, 48, 8, ChromaSubsampling::Yuv420));
        let colorspace = format.colorspace(Some((32, 24))).expect("8-bit 4:2:0 is supported");

        assert_eq!(colorspace.width, 64);
        assert_eq!(colorspace.target_width, 32);
        assert_eq!(colorspace.target_height, 24);
    }

    #[test]
    fn plane_arguments_report_the_origin_and_stride() {
        let luma = [0u8; 16];
        let chroma = [0u8; 4];

        let planes = [
            PlaneSource::from_bytes(&luma, 4, 4, 4, 0, 1).unwrap(),
            PlaneSource::from_bytes(&chroma, 2, 2, 2, 0, 1).unwrap(),
            PlaneSource::from_bytes(&chroma, 2, 2, 2, 0, 1).unwrap(),
        ];

        let PlaneArguments { pointers, strides } = plane_arguments(&planes).unwrap();
        assert_eq!(pointers[0], luma.as_ptr());
        assert_eq!(pointers[1], chroma.as_ptr());
        assert_eq!(strides, [4, 2, 2]);
    }

    /// A plane whose data starts partway into its allocation must have that
    /// origin folded into the pointer libvship receives, since libvship walks
    /// the rows itself from a base address.
    #[test]
    fn plane_arguments_fold_in_the_data_origin() {
        let luma = [0u8; 16];
        let chroma = [0u8; 4];

        let planes = [
            PlaneSource::from_bytes(&luma, 2, 2, 2, 1, 1).unwrap(),
            PlaneSource::from_bytes(&chroma, 2, 2, 2, 0, 1).unwrap(),
            PlaneSource::from_bytes(&chroma, 2, 2, 2, 0, 1).unwrap(),
        ];

        let pointers = plane_arguments(&planes).unwrap().pointers;
        // SAFETY: `luma.as_ptr()` plus one byte is still inside `luma`.
        assert_eq!(pointers[0], unsafe { luma.as_ptr().add(1) });
    }

    #[test]
    fn validation_matches_the_declared_chroma_geometry() {
        let colorspace = VshipColorspace::bt709_default(16, 16);
        let luma = [0u8; 16 * 16];
        let chroma = [0u8; 8 * 8];

        let correct = [
            PlaneSource::from_bytes(&luma, 16, 16, 16, 0, 1).unwrap(),
            PlaneSource::from_bytes(&chroma, 8, 8, 8, 0, 1).unwrap(),
            PlaneSource::from_bytes(&chroma, 8, 8, 8, 0, 1).unwrap(),
        ];
        assert!(validate_planes(&correct, &colorspace, "reference").is_ok());

        let mut wrong = correct;
        wrong[0] = PlaneSource::from_bytes(&luma, 8, 16, 16, 0, 1).unwrap();
        assert!(matches!(
            validate_planes(&wrong, &colorspace, "reference"),
            Err(VshipError::InputMismatch { .. })
        ));
    }

    #[test]
    fn even_chroma_dimensions_follow_420_422_and_444() {
        let mut colorspace = VshipColorspace::bt709_default(64, 32);

        assert_eq!(chroma_dimensions(&colorspace).unwrap(), (32, 16));
        colorspace.subsampling = crate::ffi::VshipChromaSubsample::yuv422();
        assert_eq!(chroma_dimensions(&colorspace).unwrap(), (32, 32));
        colorspace.subsampling = crate::ffi::VshipChromaSubsample::yuv444();
        assert_eq!(chroma_dimensions(&colorspace).unwrap(), (64, 32));
    }

    #[test]
    fn negative_chroma_exponent_is_rejected() {
        let mut colorspace = VshipColorspace::bt709_default(64, 32);
        colorspace.subsampling = crate::ffi::VshipChromaSubsample::yuv444();
        colorspace.subsampling.subw = -1;
        assert!(chroma_dimensions(&colorspace).is_err());
    }

    #[test]
    fn frame_score_picks_the_metric_specific_value() {
        let score = FrameScore {
            score: Some(0.9),
            norm_q: Some(1.5),
            norm_3: Some(2.0),
            norm_inf: Some(3.0),
        };

        assert_eq!(score.value(VshipMetric::Ssimulacra2), Some(0.9));
        assert_eq!(score.value(VshipMetric::Cvvdp), Some(0.9));
        assert_eq!(score.value(VshipMetric::Butteraugli), Some(1.5));
        assert_eq!(score.primary(), Some(0.9));
        assert_eq!(FrameScore::default().primary(), None);
    }

    #[test]
    fn stateless_pooling_uses_primary_or_butteraugli_q_norm() {
        let scores = [
            FrameScore {
                score: Some(0.9),
                norm_q: Some(1.0),
                ..FrameScore::default()
            },
            FrameScore {
                score: Some(0.7),
                norm_q: Some(3.0),
                ..FrameScore::default()
            },
        ];

        let mean = VshipScorer::pool(&scores, VshipMetric::Ssimulacra2, PoolMethod::Mean).unwrap();
        // Compared with a tolerance, since summing binary fractions rarely
        // lands exactly on 0.8.
        assert!((mean - 0.8).abs() < 1e-12, "mean was {mean}, expected 0.8");

        let mean = VshipScorer::pool(&scores, VshipMetric::Butteraugli, PoolMethod::Mean).unwrap();
        assert!((mean - 2.0).abs() < 1e-12, "mean was {mean}, expected 2.0");
    }

    #[test]
    fn cvvdp_pooling_rejects_empty_scores() {
        assert!(VshipScorer::pool(&[], VshipMetric::Cvvdp, PoolMethod::Mean).is_err());
    }
}
