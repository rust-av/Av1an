//! Streaming scoring through fmetrics.
//!
//! The frame-independent metrics -- SSIMULACRA2, Butteraugli, IW-SSIM and
//! MS-SSIM -- score one pair per call, reusing a workspace for scratch. A
//! pair's score is final the moment the call returns.
//!
//! CVVDP accumulates, so one context sees every frame in order. The caller
//! detects a discontinuity in the selection and calls
//! [`FmetricsScorer::reset_temporal`]; whether a gap is a scene break is the
//! caller's knowledge, not the scorer's.
//! Its ordered JOD outputs are cumulative prefixes: the last is the whole
//! session result, not the mean of the prefixes. With temporal processing
//! disabled, outputs are independent and callers may aggregate per-frame values.
//!
//! # Sharing
//!
//! A per-pair scorer is [`Send`] and [`Sync`]. fmetrics' scratch arena is
//! per-workspace, so distinct workspaces are independent and concurrent
//! submissions overlap -- but sharing *one* workspace between two threads
//! corrupts it silently, so the pool hands each to one caller at a time.
//!
//! A temporal scorer is [`Send`] but not [`Sync`], since its frames must pass
//! through one accumulator in order.

use std::{ffi::c_int, sync::Mutex};

use crate::{
    config::{ColorInfo, FmetricsConfig, FmetricsMetric},
    convert::{PlaneSet, RgbImage, RgbSource, RgbView, yuv_to_rgb},
    error::FmetricsError,
    ffi::{self, FmetricsApi, FmetricsCvvdpCtx, FmetricsCvvdpResult, FmetricsWorkspace},
};

/// A score for one frame pair.
///
/// Only the field for the configured metric is populated, so a caller can tell
/// which metric produced a score without being told separately.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameScore {
    /// SSIMULACRA2, higher is better.
    pub ssimulacra2: Option<f64>,
    /// Butteraugli, a distance where lower is better.
    pub butteraugli: Option<f64>,
    /// CVVDP's JOD, the headline measure, where higher is better. Cumulative
    /// prefix by default; independent with temporal processing disabled.
    pub cvvdp_jod: Option<f64>,
    /// CVVDP's quality score, which fmetrics reports alongside JOD.
    pub cvvdp_quality: Option<f64>,
    /// IW-SSIM, higher is better.
    pub iwssim: Option<f64>,
    /// MS-SSIM, higher is better.
    pub msssim: Option<f64>,
}

impl FrameScore {
    /// A score holding only `value`, for `metric`.
    #[inline]
    #[must_use]
    pub const fn with(metric: FmetricsMetric, value: f64) -> Self {
        Self {
            ssimulacra2: matches_if(metric, FmetricsMetric::Ssimulacra2, value),
            butteraugli: matches_if(metric, FmetricsMetric::Butteraugli, value),
            cvvdp_jod: matches_if(metric, FmetricsMetric::Cvvdp, value),
            cvvdp_quality: None,
            iwssim: matches_if(metric, FmetricsMetric::Iwssim, value),
            msssim: matches_if(metric, FmetricsMetric::Msssim, value),
        }
    }

    /// A score with every field empty, for a metric whose result has more than
    /// one part.
    #[inline]
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            ssimulacra2: None,
            butteraugli: None,
            cvvdp_jod: None,
            cvvdp_quality: None,
            iwssim: None,
            msssim: None,
        }
    }

    /// The value for `metric`, or [`None`] if this score is for another one.
    #[inline]
    #[must_use]
    pub const fn value(&self, metric: FmetricsMetric) -> Option<f64> {
        match metric {
            FmetricsMetric::Ssimulacra2 => self.ssimulacra2,
            FmetricsMetric::Butteraugli => self.butteraugli,
            FmetricsMetric::Cvvdp => self.cvvdp_jod,
            FmetricsMetric::Iwssim => self.iwssim,
            FmetricsMetric::Msssim => self.msssim,
        }
    }

    /// The value for `metric`, or an error naming what this score holds.
    ///
    /// # Errors
    ///
    /// Returns an error if the score was produced for a different metric, which
    /// is a programming error at the call site rather than a runtime condition.
    #[inline]
    pub fn require(&self, metric: FmetricsMetric) -> Result<f64, FmetricsError> {
        self.value(metric).ok_or_else(|| FmetricsError::MetricUnavailable {
            metric: metric.as_str().to_owned(),
            reason: "the score was produced for a different metric".to_owned(),
        })
    }
}

/// `Some(value)` when `metric` is `wanted`, so a score can be built without a
/// chain of branches.
#[inline]
#[must_use]
const fn matches_if(metric: FmetricsMetric, wanted: FmetricsMetric, value: f64) -> Option<f64> {
    let equal = match metric {
        FmetricsMetric::Ssimulacra2 => matches!(wanted, FmetricsMetric::Ssimulacra2),
        FmetricsMetric::Butteraugli => matches!(wanted, FmetricsMetric::Butteraugli),
        FmetricsMetric::Cvvdp => matches!(wanted, FmetricsMetric::Cvvdp),
        FmetricsMetric::Iwssim => matches!(wanted, FmetricsMetric::Iwssim),
        FmetricsMetric::Msssim => matches!(wanted, FmetricsMetric::Msssim),
    };
    if equal { Some(value) } else { None }
}

/// A reusable scratch workspace.
///
/// The library keeps a bump arena behind this handle, so holding one across
/// frames avoids reallocating it per frame.
#[derive(Debug)]
struct Workspace {
    /// The library's handle.
    handle: FmetricsWorkspace,
}

impl Workspace {
    /// Create a workspace.
    ///
    /// # Errors
    ///
    /// Returns an error if the library returns a null handle.
    #[inline]
    fn new(api: &'static FmetricsApi) -> Result<Self, FmetricsError> {
        // SAFETY: `workspace_create` takes no arguments and returns a handle
        // the library guarantees to be non-null on success.
        let handle = unsafe { (api.workspace_create)() };
        if handle.is_null() {
            return Err(FmetricsError::CallFailed {
                function: "fmetrics_workspace_create",
                code: 0,
                message: "returned a null workspace".to_owned(),
            });
        }
        Ok(Self { handle })
    }
}

impl Drop for Workspace {
    #[inline]
    fn drop(&mut self) {
        // A load failure means the library was never resolved, so there is
        // nothing to destroy.
        if let Ok(api) = FmetricsApi::load() {
            // SAFETY: the handle came from `workspace_create` and is destroyed
            // once, here.
            unsafe { (api.workspace_destroy)(self.handle) };
        }
    }
}

/// Free indices only; native ownership stays with `WorkspacePool`.
#[derive(Debug)]
struct FreeSlots {
    free: Mutex<Vec<usize>>,
}

impl FreeSlots {
    fn new(count: usize) -> Self {
        Self {
            free: Mutex::new((0..count).rev().collect()),
        }
    }

    fn claim(&self) -> Option<SlotLease<'_>> {
        // Preserve the recorded free set on poison; rebuilding it would lend
        // indices that are still in use. No callbacks run under this lock.
        let index = self.free.lock().unwrap_or_else(|poison| poison.into_inner()).pop()?;
        Some(SlotLease { slots: self, index })
    }
}

/// A non-Copy claim that returns exactly its own index, including on unwind.
#[derive(Debug)]
struct SlotLease<'a> {
    slots: &'a FreeSlots,
    index: usize,
}

impl Drop for SlotLease<'_> {
    fn drop(&mut self) {
        self.slots
            .free
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(self.index);
    }
}

/// Owns each native workspace until all borrowing leases have ended.
#[derive(Debug)]
struct WorkspacePool {
    workspaces: Vec<Workspace>,
    slots: FreeSlots,
}

impl WorkspacePool {
    /// Claim an index without waiting for an in-use workspace to finish.
    /// The allocator lock is released before returning the lease.
    fn claim(&self) -> Option<SlotLease<'_>> {
        self.slots.claim()
    }

    /// How many workspaces are held.
    #[inline]
    #[must_use]
    fn len(&self) -> usize {
        self.workspaces.len()
    }
}

/// Scores frame pairs through fmetrics.
///
/// A per-pair metric keeps one workspace per worker; CVVDP keeps a single
/// context instead, because its frames must be seen in order by one
/// accumulator.
pub struct FmetricsScorer {
    /// The configuration this scorer was built from.
    config: FmetricsConfig,
    /// The reference's colour properties.
    color: ColorInfo,
    /// The distorted input's independent colour properties.
    distorted_color: ColorInfo,
    /// The peak sample value for the source depth.
    peak: u32,
    /// The distorted input's peak sample value.
    distorted_peak: u32,
    /// The per-pair workspaces or the CVVDP context.
    inner: Inner,
}

enum Inner {
    /// Workspaces for a frame-independent metric, behind a free list so several
    /// threads can be inside the library at once.
    PerPair {
        /// The workspaces.
        pool: WorkspacePool,
    },
    /// The single context a temporal metric accumulates into.
    Temporal {
        /// The CVVDP context, or null until the first frame supplies a
        /// geometry.
        context: FmetricsCvvdpCtx,
        /// Width the context was created for.
        width: c_int,
        /// Height the context was created for.
        height: c_int,
    },
}

// SAFETY: both handles are `Send + Sync` newtypes this crate never
// dereferences, so the scorer is already both automatically. What is *not*
// automatic is that a temporal scorer's frames stay in order -- that is
// enforced by `submit_pair_temporal` taking `&mut self`, not by the auto
// traits.
unsafe impl Sync for FmetricsScorer {}

impl std::fmt::Debug for FmetricsScorer {
    #[inline]
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FmetricsScorer")
            .field("metric", &self.config.metric)
            .field("bit_depth", &self.color.bit_depth)
            .field("hdr", &self.color.hdr)
            .field("worker_count", &self.worker_count())
            .finish_non_exhaustive()
    }
}

impl FmetricsScorer {
    /// Build a scorer for `config`, reading frames as `color` describes.
    ///
    /// # Errors
    ///
    /// Returns an error if fmetrics is unavailable, if the bit depth has no
    /// peak sample value, or if CVVDP is configured without a usable frame
    /// rate, or if Butteraugli's finite exponent is not positive.
    #[inline]
    pub fn new(config: FmetricsConfig, color: ColorInfo) -> Result<Self, FmetricsError> {
        Self::new_with_colors(config, color, color)
    }

    /// Build a scorer with independent reference and distorted color properties.
    /// Use [`Self::new`] only when both inputs are known to share color metadata.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid sample depths/frame rate/Butteraugli exponent
    /// or an unavailable library.
    #[inline]
    pub fn new_with_colors(
        config: FmetricsConfig,
        color: ColorInfo,
        distorted_color: ColorInfo,
    ) -> Result<Self, FmetricsError> {
        let peak = color.max_sample()?;
        let distorted_peak = distorted_color.max_sample()?;

        if config.is_temporal() {
            Self::validate_frame_rate(&config)?;
        }
        if config.metric == FmetricsMetric::Butteraugli && config.butteraugli_options().pnorm <= 0 {
            return Err(FmetricsError::InvalidConfiguration {
                reason: format!(
                    "Butteraugli finite norm must be positive, got {}",
                    config.butteraugli_options().pnorm
                ),
            });
        }
        let api = FmetricsApi::load()?;

        let inner = if config.is_temporal() {
            Inner::Temporal {
                context: FmetricsCvvdpCtx(std::ptr::null_mut()),
                width: 0,
                height: 0,
            }
        } else {
            // One workspace per thread that may submit at once, so a caller
            // never finds the pool exhausted by its own
            // concurrency.
            let count = config.effective_threads() as usize;
            let mut pool = Vec::with_capacity(count);
            for _ in 0..count {
                pool.push(Workspace::new(api)?);
            }

            Inner::PerPair {
                pool: WorkspacePool {
                    workspaces: pool,
                    slots: FreeSlots::new(count),
                },
            }
        };

        Ok(Self {
            config,
            color,
            distorted_color,
            peak,
            distorted_peak,
            inner,
        })
    }

    /// Bound FPS-driven native temporal allocations before loading the library.
    fn validate_frame_rate(config: &FmetricsConfig) -> Result<(), FmetricsError> {
        // SECURITY: Match the pinned Vship plugin's upper bound, not its sanitization.
        if config.frame_rate.is_finite()
            && config.frame_rate > 0.0
            && config.frame_rate <= 100_000.0
        {
            Ok(())
        } else {
            Err(FmetricsError::InvalidConfiguration {
                reason: format!(
                    "CVVDP frame rate must be finite and satisfy 0 < fps <= 100000, got {}",
                    config.frame_rate
                ),
            })
        }
    }

    /// The configuration this scorer was built from.
    #[inline]
    #[must_use]
    pub const fn config(&self) -> &FmetricsConfig {
        &self.config
    }

    /// The metric being computed.
    #[inline]
    #[must_use]
    pub const fn metric(&self) -> FmetricsMetric {
        self.config.metric
    }

    /// The number of per-pair workers.
    ///
    /// A temporal metric always reports one, because its frames must pass
    /// through a single accumulator in order.
    #[inline]
    #[must_use]
    pub fn worker_count(&self) -> usize {
        match &self.inner {
            Inner::PerPair { pool } => pool.len().max(1),
            Inner::Temporal { .. } => 1,
        }
    }

    /// Whether fmetrics is available on this system.
    #[inline]
    #[must_use]
    pub fn is_available() -> bool {
        FmetricsApi::load().is_ok()
    }

    /// The fmetrics version, if it is loaded.
    #[inline]
    #[must_use]
    pub fn fmetrics_version() -> Option<String> {
        FmetricsApi::load().ok().map(FmetricsApi::version_string)
    }

    /// Score one pair that is already interleaved RGB.
    ///
    /// The same measurement as [`FmetricsScorer::submit_pair`] with the
    /// conversion done by the caller, for a caller that already holds RGB.
    ///
    /// The pixel format must match the source depth: the library's metric is
    /// depth-dependent, so reinterpreting bytes would change the score.
    ///
    /// # Errors
    ///
    /// Returns an error if the dimensions differ, if the frames are too small
    /// for the metric, or if fmetrics rejects the pair.
    #[inline]
    pub fn submit_rgb(
        &self,
        reference: &RgbView<'_>,
        distorted: &RgbView<'_>,
    ) -> Result<FrameScore, FmetricsError> {
        if let Some(minimum) = self.config.metric.minimum_dimension()
            && reference.width().min(reference.height()) < minimum
        {
            return Err(FmetricsError::UnsupportedFormat {
                reason: format!(
                    "{} needs at least {minimum} pixels in each dimension to score five scales, \
                     but the frame is {}x{}",
                    self.config.metric.as_str(),
                    reference.width(),
                    reference.height()
                ),
            });
        }

        let Inner::PerPair { pool } = &self.inner else {
            return Err(FmetricsError::MetricUnavailable {
                metric: self.config.metric.as_str().to_owned(),
                reason: "a temporal metric needs `submit_pair_temporal`, which preserves frame \
                         order"
                    .to_owned(),
            });
        };

        Self::check_dimensions(reference, distorted)?;

        self.with_workspace(pool, |workspace| {
            self.score_per_pair(workspace, reference, distorted)
        })
    }

    /// Run `score` with a workspace held exclusively, returning it afterwards.
    ///
    /// The lease returns the index on success, error or unwind. Native scoring
    /// runs after the allocator lock has been released.
    ///
    /// # Errors
    ///
    /// Returns an error if every workspace is in use, meaning the caller is
    /// submitting from more threads at once than the pool has workspaces.
    #[inline]
    fn with_workspace<T>(
        &self,
        pool: &WorkspacePool,
        score: impl FnOnce(FmetricsWorkspace) -> Result<T, FmetricsError>,
    ) -> Result<T, FmetricsError> {
        let Some(lease) = pool.claim() else {
            return Err(FmetricsError::PoolExhausted {
                workspaces: pool.len(),
                reason: "every workspace is in use; the scorer needs one workspace per thread \
                             that submits at once"
                    .to_owned(),
            });
        };

        score(pool.workspaces[lease.index].handle)
    }

    /// Score one pair of planar frames.
    ///
    /// Takes `&self` and is safe to call from several threads; each call claims
    /// its own workspace, so the library calls overlap rather than serialising.
    /// A temporal metric needs [`FmetricsScorer::submit_pair_temporal`]
    /// instead, because one context accumulates across frames and its order
    /// is part of the measurement.
    ///
    /// # Errors
    ///
    /// Returns an error if the frames cannot be converted, or if fmetrics
    /// rejects the pair.
    #[inline]
    pub fn submit_pair(
        &self,
        reference: &PlaneSet<'_>,
        distorted: &PlaneSet<'_>,
    ) -> Result<FrameScore, FmetricsError> {
        Self::check_size(self, reference)?;

        let reference_image = yuv_to_rgb(reference, &self.color, self.peak)?;
        let distorted_image = yuv_to_rgb(distorted, &self.distorted_color, self.distorted_peak)?;

        Self::check_dimensions(&reference_image, &distorted_image)?;

        let Inner::PerPair { pool } = &self.inner else {
            return Err(FmetricsError::MetricUnavailable {
                metric: self.config.metric.as_str().to_owned(),
                reason: "a temporal metric needs `submit_pair_temporal`, which preserves frame \
                         order"
                    .to_owned(),
            });
        };

        self.with_workspace(pool, |workspace| {
            self.score_per_pair(workspace, &reference_image, &distorted_image)
        })
    }

    /// Reject a frame too small for the configured metric.
    fn check_size(&self, reference: &PlaneSet<'_>) -> Result<(), FmetricsError> {
        // Reported here rather than left to the library, whose error code does
        // not name the cause.
        if let Some(minimum) = self.config.metric.minimum_dimension()
            && reference[0].width().min(reference[0].height()) < minimum
        {
            return Err(FmetricsError::UnsupportedFormat {
                reason: format!(
                    "{} needs at least {minimum} pixels in each dimension to score five scales, \
                     but the frame is {}x{}",
                    self.config.metric.as_str(),
                    reference[0].width(),
                    reference[0].height()
                ),
            });
        }
        Ok(())
    }

    /// Reject a pair whose two sides differ in size.
    fn check_dimensions<R: RgbSource>(reference: &R, distorted: &R) -> Result<(), FmetricsError> {
        if reference.width() != distorted.width() || reference.height() != distorted.height() {
            return Err(FmetricsError::InputMismatch {
                reason: format!(
                    "reference is {}x{} but distorted is {}x{}",
                    reference.width(),
                    reference.height(),
                    distorted.width(),
                    distorted.height()
                ),
            });
        }
        Ok(())
    }

    /// Score one pair with a temporal metric, which must see frames in order.
    /// With `disable_temporal`, clear history and score after computing each
    /// pair; return its captured result only if that reset succeeds.
    ///
    /// # Errors
    ///
    /// Returns an error if the frames cannot be converted, or if fmetrics
    /// rejects the pair or its post-compute reset. A native failure may leave
    /// the context's progress invalid; callers must not treat it as a score.
    #[inline]
    pub fn submit_pair_temporal(
        &mut self,
        reference: &PlaneSet<'_>,
        distorted: &PlaneSet<'_>,
    ) -> Result<FrameScore, FmetricsError> {
        Self::check_size(self, reference)?;

        let reference_image = yuv_to_rgb(reference, &self.color, self.peak)?;
        let distorted_image = yuv_to_rgb(distorted, &self.distorted_color, self.distorted_peak)?;

        Self::check_dimensions(&reference_image, &distorted_image)?;

        let Inner::Temporal {
            context,
            width,
            height,
        } = &mut self.inner
        else {
            return Err(FmetricsError::MetricUnavailable {
                metric: self.config.metric.as_str().to_owned(),
                reason: "a frame-independent metric must be scored through a workspace".to_owned(),
            });
        };
        let mut context = *context;
        let (mut current_width, mut current_height) = (*width, *height);

        let result = self.score_cvvdp(
            &mut context,
            &mut current_width,
            &mut current_height,
            &reference_image,
            &distorted_image,
        );
        // The context is created lazily on the first frame, so its state is
        // written back.
        if let Inner::Temporal {
            context: slot,
            width,
            height,
        } = &mut self.inner
        {
            *slot = context;
            *width = current_width;
            *height = current_height;
        }

        result
    }

    /// Score one pair with the frame-independent metric.
    fn score_per_pair<R: RgbSource>(
        &self,
        workspace: FmetricsWorkspace,
        reference: &R,
        distorted: &R,
    ) -> Result<FrameScore, FmetricsError> {
        let api = FmetricsApi::load()?;
        let reference_img = reference.as_img(self.color.colorspace, self.color.hdr);
        let distorted_img =
            distorted.as_img(self.distorted_color.colorspace, self.distorted_color.hdr);

        let mut value = 0f64;
        let metric = self.config.metric;

        // The function called depends only on the metric; every signature takes
        // the same workspace and image pair.
        let (function, status) = match metric {
            FmetricsMetric::Ssimulacra2 => {
                // SAFETY: the pointers match the signatures transcribed from
                // `fmetrics.h`; the workspace is live, and both images outlive
                // the call because the caller owns them.
                ("fmetrics_ssimu2_cmp", unsafe {
                    (api.ssimu2_cmp)(
                        workspace,
                        reference_img.as_raw(),
                        distorted_img.as_raw(),
                        &mut value,
                    )
                })
            },
            FmetricsMetric::Butteraugli => {
                let options = self.config.butteraugli_options();
                // SAFETY: as above; `options` outlives the call.
                ("fmetrics_butteraugli_cmp", unsafe {
                    (api.butteraugli_cmp)(
                        workspace,
                        reference_img.as_raw(),
                        distorted_img.as_raw(),
                        &options,
                        &mut value,
                    )
                })
            },
            FmetricsMetric::Iwssim => (
                "fmetrics_iwssim_cmp",
                // SAFETY: as above.
                unsafe {
                    (api.iwssim_cmp)(
                        workspace,
                        reference_img.as_raw(),
                        distorted_img.as_raw(),
                        &mut value,
                    )
                },
            ),
            FmetricsMetric::Msssim => (
                "fmetrics_msssim_cmp",
                // SAFETY: as above.
                unsafe {
                    (api.msssim_cmp)(
                        workspace,
                        reference_img.as_raw(),
                        distorted_img.as_raw(),
                        &mut value,
                    )
                },
            ),
            // Unreachable: a CVVDP scorer holds `Inner::Temporal`.
            FmetricsMetric::Cvvdp => {
                return Err(FmetricsError::MetricUnavailable {
                    metric: metric.as_str().to_owned(),
                    reason: "a temporal metric cannot be scored through a workspace".to_owned(),
                });
            },
        };

        ffi::check(status, function)?;
        // Native power reduction may overflow yet return OK, especially for large p.
        if metric == FmetricsMetric::Butteraugli && !value.is_finite() {
            return Err(FmetricsError::NonFiniteButteraugli {
                value,
                pnorm: self.config.butteraugli_options().pnorm,
            });
        }
        Ok(FrameScore::with(metric, value))
    }

    /// Score one pair through the CVVDP context, creating it if this is the
    /// first frame or the geometry has changed.
    ///
    /// # Errors
    ///
    /// Returns an error if the context cannot be created, or if fmetrics
    /// rejects the frame.
    fn score_cvvdp(
        &self,
        context: &mut FmetricsCvvdpCtx,
        width: &mut c_int,
        height: &mut c_int,
        reference: &RgbImage,
        distorted: &RgbImage,
    ) -> Result<FrameScore, FmetricsError> {
        let api = FmetricsApi::load()?;
        let reference_img = reference.as_img(self.color.colorspace, self.color.hdr);
        let distorted_img =
            distorted.as_img(self.distorted_color.colorspace, self.distorted_color.hdr);

        let frame_width = c_int::try_from(reference_img.as_raw().width).map_err(|_| {
            FmetricsError::MalformedFrame {
                reason: "CVVDP width exceeds c_int".to_owned(),
            }
        })?;
        let frame_height = c_int::try_from(reference_img.as_raw().height).map_err(|_| {
            FmetricsError::MalformedFrame {
                reason: "CVVDP height exceeds c_int".to_owned(),
            }
        })?;

        // The context is bound to a geometry, so a change of size means
        // recreating it.
        if (*context).is_null() || *width != frame_width || *height != frame_height {
            self.recreate_cvvdp(context, frame_width, frame_height)?;
            *width = frame_width;
            *height = frame_height;
        }

        let mut result = FmetricsCvvdpResult::default();
        // SAFETY: `*context` is a live context created by `cvvdp_create` below
        // or by an earlier frame, and both images outlive the call.
        let status = unsafe {
            (api.cvvdp_process_frame)(
                *context,
                reference_img.as_raw(),
                distorted_img.as_raw(),
                &mut result,
            )
        };
        ffi::check(status, "fmetrics_cvvdp_process_frame")?;

        if self.config.disable_temporal {
            // COMPAT: fcvvdp's reset clears both history and accumulated score,
            // matching the plugin's post-compute disableTemporal resets.
            // SAFETY: the context remains live through this synchronous reset.
            let status = unsafe { (api.cvvdp_reset)(*context) };
            ffi::check(status, "fmetrics_cvvdp_reset")?;
        }

        let mut score = FrameScore::empty();
        score.cvvdp_jod = Some(result.jod);
        score.cvvdp_quality = Some(result.quality);
        Ok(score)
    }

    /// Destroy any existing context and create one for this geometry.
    ///
    /// # Errors
    ///
    /// Returns an error if fmetrics refuses to create the context.
    fn recreate_cvvdp(
        &self,
        context: &mut FmetricsCvvdpCtx,
        width: c_int,
        height: c_int,
    ) -> Result<(), FmetricsError> {
        let api = FmetricsApi::load()?;

        if !(*context).is_null() {
            // SAFETY: the context came from `cvvdp_create` and is released
            // once.
            unsafe { (api.cvvdp_destroy)(*context) };
            *context = FmetricsCvvdpCtx(std::ptr::null_mut());
        }

        let mut created = FmetricsCvvdpCtx(std::ptr::null_mut());
        // SAFETY: the arguments match the signature. A null `custom_params`
        // selects the display model's own parameters.
        let status = unsafe {
            (api.cvvdp_create)(
                width,
                height,
                self.config.frame_rate,
                self.config.display_model,
                self.config.effective_threads(),
                std::ptr::null(),
                &mut created,
            )
        };
        ffi::check(status, "fmetrics_cvvdp_create")?;

        if created.is_null() {
            return Err(FmetricsError::CallFailed {
                function: "fmetrics_cvvdp_create",
                code: 0,
                message: "returned a null context".to_owned(),
            });
        }

        *context = created;
        Ok(())
    }

    /// Clear CVVDP's temporal history AND accumulated score.
    ///
    /// fmetrics 0.1.0 has no history-only reset. This is not equivalent to
    /// Vship's history-only scene-break reset when scores must accumulate.
    ///
    /// A per-pair metric has no history to clear, so this succeeds without
    /// calling the library.
    ///
    /// # Errors
    ///
    /// Returns an error if fmetrics rejects the reset.
    #[inline]
    pub fn reset_temporal(&mut self) -> Result<(), FmetricsError> {
        let Inner::Temporal { context, .. } = &mut self.inner else {
            return Ok(());
        };

        // Nothing has been scored yet, so there is no context and no history.
        if context.is_null() {
            return Ok(());
        }

        let api = FmetricsApi::load()?;
        // SAFETY: the context is live and was created by `cvvdp_create`.
        let status = unsafe { (api.cvvdp_reset)(*context) };
        ffi::check(status, "fmetrics_cvvdp_reset")
    }
}

impl Drop for FmetricsScorer {
    #[inline]
    fn drop(&mut self) {
        if let Inner::Temporal { context, .. } = self.inner
            && !context.is_null()
            && let Ok(api) = FmetricsApi::load()
        {
            // SAFETY: the context was created by `cvvdp_create` and is
            // destroyed exactly once, here.
            unsafe { (api.cvvdp_destroy)(context) };
        }
    }
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "a failing assertion should panic loudly, which is what a unit test wants"
)]
mod tests {
    use av_decoders::{Rational32, VideoDetails};
    use v_frame::chroma::ChromaSubsampling;

    use super::*;

    fn details(bit_depth: usize) -> VideoDetails {
        VideoDetails {
            width: 64,
            height: 64,
            bit_depth,
            chroma_sampling: ChromaSubsampling::Yuv420,
            frame_rate: Rational32::new(24, 1),
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

    fn frame(width: usize, height: usize, fill: u8) -> v_frame::frame::Frame<u8> {
        let mut frame =
            v_frame::frame::FrameBuilder::new(width, height, ChromaSubsampling::Yuv444, 8)
                .build::<u8>()
                .unwrap();
        for index in 0..3 {
            for row in frame.plane_mut(index).unwrap().rows_mut() {
                row.fill(if index == 0 { fill } else { 128 });
            }
        }
        frame
    }

    fn color(bit_depth: usize) -> ColorInfo {
        ColorInfo::from_details(&details(bit_depth)).unwrap()
    }

    #[test]
    fn nonpositive_butteraugli_norm_rejects_before_library_loading() {
        for pnorm in [0, -1, i32::MIN] {
            let config = FmetricsConfig::new(FmetricsMetric::Butteraugli)
                .with_threads(1)
                .with_pnorm(pnorm);
            let error = FmetricsScorer::new(config, color(8)).unwrap_err();
            assert!(
                matches!(error, FmetricsError::InvalidConfiguration { .. }),
                "{error}"
            );
            assert!(error.to_string().contains("positive"), "{error}");
        }
    }

    #[test]
    fn cvvdp_default_frame_rate_is_usable() {
        let config = FmetricsConfig::new(FmetricsMetric::Cvvdp);
        assert_eq!(config.frame_rate(), 60.0);
        assert!(FmetricsScorer::validate_frame_rate(&config).is_ok());
    }

    #[test]
    fn cvvdp_invalid_fps_from_fields_or_builders_rejects_before_discovery() {
        for rate in [
            0.0,
            -0.0,
            -1.0,
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            100_000.01,
            f32::MAX,
        ] {
            for disabled in [false, true] {
                let mut direct =
                    FmetricsConfig::new(FmetricsMetric::Cvvdp).with_disable_temporal(disabled);
                direct.frame_rate = rate;
                for config in [
                    direct,
                    FmetricsConfig::new(FmetricsMetric::Cvvdp)
                        .with_frame_rate(rate)
                        .with_disable_temporal(disabled),
                ] {
                    assert!(
                        matches!(
                            FmetricsScorer::validate_frame_rate(&config),
                            Err(FmetricsError::InvalidConfiguration { .. })
                        ),
                        "{rate} must be rejected"
                    );
                    // Context creation is lazy; even a broken bound cannot allocate a ring here.
                    assert!(
                        matches!(
                            FmetricsScorer::new(config, color(8)),
                            Err(FmetricsError::InvalidConfiguration { .. })
                        ),
                        "{rate} must fail before library discovery"
                    );
                }
            }
        }
    }

    #[test]
    fn cvvdp_accepts_positive_fps_through_one_hundred_thousand() {
        for rate in [f32::MIN_POSITIVE, 24.0, 23.976_025, 60.0, 100_000.0] {
            let config = FmetricsConfig::new(FmetricsMetric::Cvvdp).with_frame_rate(rate);
            assert!(
                FmetricsScorer::validate_frame_rate(&config).is_ok(),
                "{rate}"
            );
        }
    }

    #[test]
    fn frame_rate_validation_rejects_zero_negative_nan_and_infinity_but_accepts_twenty_four() {
        for rate in [0.0f32, -1.0, f32::NAN, f32::INFINITY] {
            let config = FmetricsConfig::new(FmetricsMetric::Cvvdp).with_frame_rate(rate);
            assert!(
                FmetricsScorer::validate_frame_rate(&config).is_err(),
                "{rate} must be rejected"
            );
        }
        let good = FmetricsConfig::new(FmetricsMetric::Cvvdp).with_frame_rate(24.0);
        assert!(FmetricsScorer::validate_frame_rate(&good).is_ok());
    }

    #[test]
    fn frame_score_reports_only_the_metric_it_was_asked_for() {
        let score = FrameScore::with(FmetricsMetric::Ssimulacra2, 0.9);
        assert_eq!(score.value(FmetricsMetric::Ssimulacra2), Some(0.9));
        for other in [
            FmetricsMetric::Butteraugli,
            FmetricsMetric::Cvvdp,
            FmetricsMetric::Iwssim,
            FmetricsMetric::Msssim,
        ] {
            assert_eq!(score.value(other), None, "{other:?} must be unset");
            assert!(score.require(other).is_err());
        }
    }

    #[test]
    fn every_metric_has_a_name_and_a_direction() {
        let metrics = [
            FmetricsMetric::Ssimulacra2,
            FmetricsMetric::Butteraugli,
            FmetricsMetric::Cvvdp,
            FmetricsMetric::Iwssim,
            FmetricsMetric::Msssim,
        ];
        for metric in metrics {
            assert!(!metric.as_str().is_empty(), "{metric:?} needs a name");
        }

        // Butteraugli is a distance; everything else is a quality measure.
        assert!(FmetricsMetric::Butteraugli.prefers_lower_is_better());
        for metric in [
            FmetricsMetric::Ssimulacra2,
            FmetricsMetric::Cvvdp,
            FmetricsMetric::Iwssim,
            FmetricsMetric::Msssim,
        ] {
            assert!(
                !metric.prefers_lower_is_better(),
                "{metric:?} is higher-better"
            );
        }
    }

    #[test]
    fn only_cvvdp_is_temporal() {
        assert!(FmetricsMetric::Cvvdp.is_temporal());
        for metric in [
            FmetricsMetric::Ssimulacra2,
            FmetricsMetric::Butteraugli,
            FmetricsMetric::Iwssim,
            FmetricsMetric::Msssim,
        ] {
            assert!(!metric.is_temporal(), "{metric:?} must not be temporal");
        }
    }

    #[test]
    fn the_multi_scale_metrics_declare_a_minimum_size() {
        // They would otherwise fail inside the library with a code that does
        // not explain the cause.
        for metric in [FmetricsMetric::Iwssim, FmetricsMetric::Msssim] {
            let minimum = metric.minimum_dimension().expect("multi-scale needs a minimum");
            assert!(minimum >= 16, "{metric:?} minimum should be at least 16");
        }
        for metric in
            [FmetricsMetric::Ssimulacra2, FmetricsMetric::Butteraugli, FmetricsMetric::Cvvdp]
        {
            assert_eq!(
                metric.minimum_dimension(),
                None,
                "{metric:?} has no minimum"
            );
        }
    }

    #[test]
    fn a_frame_score_carries_each_metric_in_its_own_field() {
        for metric in [
            FmetricsMetric::Ssimulacra2,
            FmetricsMetric::Butteraugli,
            FmetricsMetric::Iwssim,
            FmetricsMetric::Msssim,
        ] {
            let score = FrameScore::with(metric, 1.25);
            assert_eq!(score.value(metric), Some(1.25), "{metric:?}");
            assert_eq!(FrameScore::empty().value(metric), None, "{metric:?}");
        }
    }

    #[test]
    fn cvvdp_reports_jod_as_the_headline() {
        // JOD is the headline measure, so it is what `value` returns.
        let score = FrameScore {
            ssimulacra2: None,
            butteraugli: None,
            cvvdp_jod: Some(9.5),
            cvvdp_quality: Some(120.0),
            iwssim: None,
            msssim: None,
        };
        assert_eq!(score.value(FmetricsMetric::Cvvdp), Some(9.5));
        assert_eq!(score.cvvdp_quality, Some(120.0));
    }

    #[test]
    fn peak_sample_values_match_the_bit_depths() {
        assert_eq!(color(8).max_sample().unwrap(), 255);
        assert_eq!(color(10).max_sample().unwrap(), 1023);
        assert_eq!(color(12).max_sample().unwrap(), 4095);
    }

    #[test]
    fn frame_plane_conversion_produces_expected_rgb_geometry_and_stride() {
        // Conversion is what every submission does first, so it must work on a
        // frame built exactly as the driver would supply one.
        let frame = frame(16, 16, 100);
        let frames = PlaneSet::from_frame(&frame).unwrap();
        let info = color(8);
        let image = yuv_to_rgb(&frames, &info, 255).unwrap();
        assert_eq!((image.width(), image.height()), (16, 16));
        assert_eq!(image.stride(), 48);
    }

    #[test]
    fn slots_release_exactly_one_and_restore_every_wave() {
        assert!(FreeSlots::new(0).claim().is_none());
        let slots = FreeSlots::new(3);
        for _ in 0..32 {
            let mut leases: Vec<_> = (0..3).map(|_| slots.claim().unwrap()).collect();
            let mut indices: Vec<_> = leases.iter().map(|lease| lease.index).collect();
            indices.sort_unstable();
            assert_eq!(indices, [0, 1, 2]);
            assert!(slots.claim().is_none(), "all slots are held");

            let released = leases.remove(1);
            let index = released.index;
            drop(released);
            let replacement = slots.claim().unwrap();
            assert_eq!(replacement.index, index, "return exactly the released slot");
            assert!(slots.claim().is_none(), "the other slots are still held");
        }
    }

    #[test]
    fn oversubscribed_claims_are_exclusive_across_release_waves() {
        use std::{
            collections::HashSet,
            sync::{Barrier, Mutex},
        };

        let slots = FreeSlots::new(3);
        let start = Barrier::new(8);
        let attempted = Barrier::new(8);
        let active = Mutex::new(HashSet::new());
        let results = std::thread::scope(|scope| {
            let threads: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        let mut waves = Vec::new();
                        for _ in 0..32 {
                            start.wait();
                            let lease = slots.claim();
                            let observation = lease.as_ref().map(|lease| {
                                (lease.index, active.lock().unwrap().insert(lease.index))
                            });
                            // Every contender must attempt before any slot is returned.
                            attempted.wait();
                            if let Some(lease) = &lease {
                                active.lock().unwrap().remove(&lease.index);
                            }
                            drop(lease);
                            waves.push(observation);
                        }
                        waves
                    })
                })
                .collect();
            threads.into_iter().map(|thread| thread.join().unwrap()).collect::<Vec<_>>()
        });

        for wave in 0..32 {
            let claims: Vec<_> = results.iter().filter_map(|thread| thread[wave]).collect();
            assert_eq!(
                claims.len(),
                3,
                "excess contenders must refuse, wave {wave}"
            );
            assert!(
                claims.iter().all(|(_, unique)| *unique),
                "active slot duplicated"
            );
            let mut indices: Vec<_> = claims.iter().map(|(index, _)| *index).collect();
            indices.sort_unstable();
            assert_eq!(
                indices,
                [0, 1, 2],
                "the original full set must return each wave"
            );
        }
    }

    #[test]
    fn free_slot_lease_returns_capacity_on_error() {
        let slots = FreeSlots::new(1);
        let score = || -> Result<(), &str> {
            let _lease = slots.claim().unwrap();
            Err("injected scoring failure")
        };
        assert_eq!(score(), Err("injected scoring failure"));
        let lease = slots.claim().expect("error returned the slot");
        assert_eq!(lease.index, 0);
        assert!(
            slots.claim().is_none(),
            "returning a slot must not duplicate it"
        );
    }

    #[test]
    fn free_slot_lease_returns_capacity_on_unwind_without_duplicate_free_slot() {
        let slots = FreeSlots::new(1);
        let panic = std::panic::catch_unwind(|| {
            let _lease = slots.claim().expect("available slot");
            panic!("injected scoring panic");
        });
        assert!(panic.is_err(), "the scoring panic must propagate");
        let lease = slots.claim().expect("unwind returned the slot");
        assert_eq!(lease.index, 0);
        assert!(
            slots.claim().is_none(),
            "returning a slot must not duplicate it"
        );
    }

    #[test]
    fn poison_recovery_does_not_free_a_held_slot() {
        let slots = FreeSlots::new(2);
        let held = slots.claim().unwrap();
        std::thread::scope(|scope| {
            let panic = scope
                .spawn(|| {
                    let _lock = slots.free.lock().unwrap();
                    panic!("inject allocator lock poisoning");
                })
                .join();
            assert!(panic.is_err());
        });
        let remaining = slots.claim().expect("preserve recorded free indices on poison");
        assert_ne!(remaining.index, held.index);
        assert!(
            slots.claim().is_none(),
            "poisoning must not invent free slots"
        );
        drop(held);
        drop(remaining);
        let leases = [slots.claim().unwrap(), slots.claim().unwrap()];
        assert_ne!(leases[0].index, leases[1].index);
        assert!(slots.claim().is_none());
    }

    #[cfg(unix)]
    mod internal_integration {
        use super::*;

        #[test]
        fn nested_workspace_closures_report_pool_exhaustion_without_losing_held_lease() {
            workspace_case(
                "nested_workspace_closures_report_pool_exhaustion_without_losing_held_lease",
                "nested",
            );
        }

        #[test]
        fn workspace_closure_error_returns_native_workspace_capacity() {
            workspace_case(
                "workspace_closure_error_returns_native_workspace_capacity",
                "error",
            );
        }

        #[test]
        fn workspace_closure_unwind_returns_native_workspace_capacity() {
            workspace_case(
                "workspace_closure_unwind_returns_native_workspace_capacity",
                "unwind",
            );
        }

        #[test]
        fn returned_native_workspace_leases_allow_two_overlapping_closures() {
            workspace_case(
                "returned_native_workspace_leases_allow_two_overlapping_closures",
                "overlap",
            );
        }

        fn workspace_case(test: &str, scenario: &str) {
            // Isolate the loader OnceLock and use genuinely allocated/freed handles,
            // even when a system fmetrics library is installed or unavailable.
            if std::env::var_os("FMETRICS_CALLBACK_UNWIND_CHILD").is_none() {
                let module = module_path!().split_once("::").expect("test crate prefix").1;
                let exact = format!("{module}::{test}");
                let directory = tempfile::tempdir().expect("temporary native ABI sink");
                let library = directory.path().join(ffi::library_names()[0]);
                let output = std::process::Command::new("cc")
                    .args(["-shared", "-fPIC", "-o"])
                    .arg(&library)
                    .arg(concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/../andean-condor/tests/fixtures/metadata.c"
                    ))
                    .output()
                    .expect("callback ownership check requires cc");
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                let output = std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", &exact, "--nocapture"])
                    .env("FMETRICS_CALLBACK_UNWIND_CHILD", scenario)
                    .env("FMETRICS_LIB_PATH", directory.path())
                    .output()
                    .expect("run isolated callback regression");
                assert!(
                    output.status.success(),
                    "{}\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                assert!(
                    String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed")
                        && String::from_utf8_lossy(&output.stdout)
                            .contains(&format!("workspace {scenario}: assertions completed")),
                    "callback child must execute exactly one test: {}",
                    String::from_utf8_lossy(&output.stdout)
                );
                return;
            }
            assert_eq!(
                std::env::var("FMETRICS_CALLBACK_UNWIND_CHILD").expect("scenario"),
                scenario,
                "case belongs to this contract"
            );
            assert_workspace_capacity(scenario);
            println!("workspace {scenario}: assertions completed");
        }

        fn assert_workspace_capacity(scenario: &str) {
            let scorer = FmetricsScorer::new(
                FmetricsConfig::new(FmetricsMetric::Ssimulacra2).with_threads(2),
                color(8),
            )
            .expect("the native ABI sink must load");
            let Inner::PerPair { pool } = &scorer.inner else {
                panic!("expected a per-pair pool");
            };
            let held = pool.claim().unwrap();
            assert!(matches!(
                scorer.with_workspace(pool, |_| scorer.with_workspace(pool, |_| Ok(()))),
                Err(FmetricsError::PoolExhausted { workspaces: 2, .. })
            ));
            if scenario == "nested" {
                let returned = pool.claim().expect("nested refusal returned its outer lease");
                assert_ne!(held.index, returned.index);
                assert!(
                    pool.claim().is_none(),
                    "held and returned leases remain exclusive"
                );
                return;
            }
            assert!(matches!(
                scorer.with_workspace(pool, |_| Err::<(), _>(FmetricsError::UnsupportedFormat {
                    reason: "injected scoring failure".to_owned(),
                })),
                Err(FmetricsError::UnsupportedFormat { .. })
            ));
            if scenario == "error" {
                assert!(
                    scorer.with_workspace(pool, |_| Ok(())).is_ok(),
                    "closure error returns its lease"
                );
                return;
            }
            let panic = std::panic::catch_unwind(|| {
                scorer.with_workspace(pool, |_| -> Result<(), FmetricsError> {
                    panic!("injected scoring panic");
                })
            });
            assert!(panic.is_err(), "the scoring panic must propagate");
            assert!(
                scorer.with_workspace(pool, |_| Ok(())).is_ok(),
                "a scoring panic must not permanently exhaust the pool"
            );
            if scenario == "unwind" {
                return;
            }
            drop(held);

            let overlap = std::sync::Barrier::new(2);
            std::thread::scope(|scope| {
                let threads: Vec<_> = (0..2)
                    .map(|_| {
                        scope.spawn(|| {
                            scorer.with_workspace(pool, |_| {
                                overlap.wait();
                                Ok(())
                            })
                        })
                    })
                    .collect();
                for thread in threads {
                    assert!(thread.join().unwrap().is_ok());
                }
            });
        }
    }
}
