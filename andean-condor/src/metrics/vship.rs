//! Native libvship scoring for SSIMULACRA2, Butteraugli and CVVDP.
//!
//! libvship implements the same three perceptual metrics Av1an exposes as
//! VapourSynth plugins, but behind a C API rather than a filter graph. Binding
//! it directly lets the pass decode only the frames it selected and hand each
//! pair straight to the device, with no temporary files and no distortion maps.
//!
//! Everything that is not metric-specific lives in [`super::probe`]: opening
//! the two clips, walking the selection, and keeping each frame's planes alive
//! across the submission. This module supplies only the mapping from Av1an's
//! [`QualityMetric`] to a [`VshipConfig`], the plane adaptor, and the driver
//! loop that submits each pair.
//!
//! # Availability and fallback
//!
//! libvship ships as a VapourSynth plugin and is opened with `dlopen` at
//! runtime, so it may be absent. [`av_metrics_vship::is_available`] reports
//! whether the library loaded and a device passed libvship's full check. This
//! path is preferred when usable. Callers can fall back to plugins on failure,
//! but do not retry a temporal scene pass after frame progress was reported.
//!
//! # Temporal metrics
//!
//! CVVDP accumulates across the frames a handler has seen, so
//! [`VshipScorer`] builds it exactly one handler and sees every frame in order.
//! This selection owner requests history-only resets at actual source gaps.
//! Disabled-temporal mode instead resets history and score after every frame.

use std::{sync::atomic::AtomicBool, time::Instant};

use anyhow::{Context as _, Result, bail};
use av_metrics_vship::{
    PlaneSet as MetricPlaneSet, PlaneSource as MetricPlaneSource, VideoFormat, VshipConfig,
    VshipMetric, VshipScorer,
};

use crate::{
    metrics::probe::{self, OutputIndexing, PlaneSet},
    models::sequence::target_quality::types::QualityMetric,
};

/// The libvship metric a configured [`QualityMetric`] maps to, or `None` when
/// the metric has no libvship equivalent.
///
/// VMAF and XPSNR are absent: VMAF is served by [`super::vmaf`]'s libvmaf
/// binding and XPSNR by the `vszip` plugin, neither of which libvship provides.
#[inline]
#[must_use]
pub const fn vship_metric_for(metric: &QualityMetric) -> Option<VshipMetric> {
    match metric {
        QualityMetric::SSIMULACRA2 { .. } => Some(VshipMetric::Ssimulacra2),
        QualityMetric::BUTTERAUGLI { .. } => Some(VshipMetric::Butteraugli),
        QualityMetric::CVVDP { .. } => Some(VshipMetric::Cvvdp),
        QualityMetric::VMAF { .. } | QualityMetric::XPSNR { .. } => None,
    }
}

/// Whether the native path should be attempted for `metric`.
///
/// True only for a metric libvship implements *and* when the library and a
/// device are usable. When this is false the caller keeps its VapourSynth
/// branch, so behaviour on a machine without libvship is exactly what it was
/// before this module existed.
#[inline]
#[must_use]
pub fn native_supported(metric: &QualityMetric) -> bool {
    vship_metric_for(metric).is_some() && av_metrics_vship::is_available()
}

/// Build a `VshipConfig` from Av1an's metric configuration.
///
/// `threads` is retained as a legacy request, defaulting to
/// [`av_metrics_vship::DEFAULT_HANDLER_THREADS`] and clamping zero to one.
/// Native synchronous scoring uses exactly one handler regardless of that request.
///
/// `resolution` is not applied here. It becomes libvship's `target_width` and
/// `target_height`, and libvship scales each input internally, so no plane is
/// resized before submission.
///
/// CVVDP's display model, resize-to-display and temporal options pass through
/// unchanged, as do Butteraugli's `norm` and `intensity_multiplier`.
///
/// `frame_rate` is an explicit source rate, passed unchanged for CVVDP.
/// The scorer rejects nonfinite values and values outside `0 < fps <= 100000`;
/// invalid metadata must not silently become the low-level 60 fps default.
///
/// # Errors
///
/// Returns an error for a metric libvship does not implement, or when the
/// display model carries an interior NUL, which cannot reach the C API.
/// Validating early keeps the failure beside the configuration that caused it.
#[inline]
pub fn scorer_config(metric: &QualityMetric, frame_rate: f32) -> Result<VshipConfig> {
    match metric {
        QualityMetric::SSIMULACRA2 {
            threads, gpu_id, ..
        } => Ok(with_gpu_id(
            VshipConfig::new()
                .with_metric(VshipMetric::Ssimulacra2)
                .with_handler_threads(handler_threads(*threads)),
            *gpu_id,
        )),
        QualityMetric::BUTTERAUGLI {
            threads,
            intensity_multiplier,
            norm,
            gpu_id,
            ..
        } => {
            let mut config = with_gpu_id(
                VshipConfig::new()
                    .with_metric(VshipMetric::Butteraugli)
                    .with_handler_threads(handler_threads(*threads)),
                *gpu_id,
            );
            if let Some(norm) = *norm {
                config = config.with_q_norm(i32::from(norm));
            }
            if let Some(intensity_multiplier) = *intensity_multiplier {
                // `intensity_multiplier` is an `f64` in Av1an's config and an
                // `f32` here; a value too large for `f32` is not a real
                // brightness target, and the library rejects it.
                config = config.with_intensity_multiplier(intensity_multiplier as f32);
            }
            Ok(config)
        },
        QualityMetric::CVVDP {
            display_model,
            resize_to_display,
            disable_temporal,
            gpu_id,
            ..
        } => {
            let mut config = with_gpu_id(
                VshipConfig::new()
                    .with_metric(VshipMetric::Cvvdp)
                    .with_handler_threads(av_metrics_vship::DEFAULT_HANDLER_THREADS)
                    .with_fps(frame_rate),
                *gpu_id,
            );
            if let Some(display_model) = display_model {
                config = config
                    .with_display_model(display_model.to_string())
                    .map_err(|error| anyhow::anyhow!(error))?;
            }
            if let Some(resize_to_display) = *resize_to_display {
                config = config.with_resize_to_display(resize_to_display);
            }
            if let Some(disable_temporal) = *disable_temporal {
                config = config.with_disable_temporal(disable_temporal);
            }
            Ok(config)
        },
        QualityMetric::VMAF { .. } | QualityMetric::XPSNR { .. } => {
            bail!("{} has no libvship equivalent", metric.friendly_name());
        },
    }
}

/// The resolution every variant of [`QualityMetric`] scores at.
///
/// It is an `Option` on all five variants alike, so it can be read without
/// matching, and it reaches libvship as the colorspace's target size rather
/// than as a resize of the decoded planes.
#[inline]
#[must_use]
const fn resolution(metric: &QualityMetric) -> Option<(u32, u32)> {
    match metric {
        QualityMetric::VMAF { resolution, .. }
        | QualityMetric::SSIMULACRA2 { resolution, .. }
        | QualityMetric::BUTTERAUGLI { resolution, .. }
        | QualityMetric::XPSNR { resolution, .. }
        | QualityMetric::CVVDP { resolution, .. } => *resolution,
    }
}

/// Whether Butteraugli should report the Q-norm rather than the infinity norm.
///
/// The VapourSynth branch this replaces selects the property to read as
/// `norm.and_then(|_| QNORM).unwrap_or(INF)`, so a requested norm means the
/// Q-norm and no norm means the infinity norm. `butteraugli-3` is the case that
/// distinguishes them: the plugin reads `_BUTTERAUGLI_QNorm` there, not the
/// distortion map's L3 norm, so this must not be inferred from the exponent.
#[inline]
#[must_use]
fn butteraugli_uses_q_norm(metric: &QualityMetric) -> bool {
    matches!(metric, QualityMetric::BUTTERAUGLI { norm: Some(_), .. })
}

/// Legacy libvship handler-thread request for a configured `threads` value.
///
/// Retain the default request of four and the crate's zero-to-one clamp for
/// compatibility. Native synchronous scoring always owns one handler.
#[inline]
fn handler_threads(threads: Option<u8>) -> u32 {
    threads.map_or(av_metrics_vship::DEFAULT_HANDLER_THREADS, u32::from)
}

/// Apply a configured `gpu_id` to a `VshipConfig`.
///
/// Leaving it unset is meaningful: libvship then chooses the device itself,
/// preferring a discrete GPU over an integrated one. Mapping `None` to `0`
/// would instead pin every unscored run to device 0, discarding that
/// preference, so the field is only written when a device was actually named.
#[inline]
fn with_gpu_id(mut config: VshipConfig, gpu_id: Option<u8>) -> VshipConfig {
    if let Some(gpu_id) = gpu_id {
        config = config.with_gpu_id(u32::from(gpu_id));
    }

    config
}

/// Score only the frames named by `selected`, returning one score per
/// selection.
///
/// The decode loop belongs to [`probe::probe_selected_frames`]; this wraps it
/// with the libvship scorer and the per-pair submission the library needs.
///
/// # Index spaces
///
/// The two decoders may use different index spaces, which is why `indexing` is
/// passed explicitly rather than inferred. See `probe::output_index`.
///
/// # Temporal handling
///
/// For CVVDP the scorer keeps a single handler and sees frames in submission
/// order. The probe driver visits `selected` in the order given, so a temporal
/// metric's history is reset at source-index gaps rather than carrying history
/// across frames it never saw. This clip-wide API has no scene boundaries;
/// QC/TQ scene dispatch instead starts a fresh scorer per temporal scene.
///
/// # Progress
///
/// `on_score` is called with `(position in the selection, score)` after each
/// pair is scored, in the order the pairs were visited. Scores are reported
/// here rather than only returned because the caller needs them as they arrive
/// to drive its own progress reporting; libvship yields each pair's score as
/// soon as that pair is computed, so nothing is withheld until the end.
///
/// # Errors
///
/// Returns an error when the two clips disagree on resolution or bit depth,
/// when libvship is unavailable or rejects the configuration, when seeking or
/// decoding fails, or when a selected frame cannot be scored.
#[inline]
pub fn score_probed_frames(
    reference: &mut av_decoders::Decoder,
    distorted: &mut av_decoders::Decoder,
    config: &QualityMetric,
    selected: &[usize],
    indexing: OutputIndexing,
    cancelled: Option<&AtomicBool>,
    on_score: impl FnMut(usize, f64),
) -> Result<Vec<f64>> {
    score_probed_frames_from(
        reference, distorted, config, selected, indexing, 0, 0, cancelled, on_score,
    )
}

/// A fresh scorer for a scene, with its actual compacted-output offset.
#[allow(clippy::too_many_arguments)]
pub(super) fn score_probed_frames_from(
    reference: &mut av_decoders::Decoder,
    distorted: &mut av_decoders::Decoder,
    config: &QualityMetric,
    selected: &[usize],
    indexing: OutputIndexing,
    output_start: usize,
    reference_next: usize,
    cancelled: Option<&AtomicBool>,
    mut on_score: impl FnMut(usize, f64),
) -> Result<Vec<f64>> {
    if selected.is_empty() {
        return Ok(Vec::new());
    }

    let reference_details = probe::video_details(reference).context("reference metadata")?;
    let distorted_details = probe::video_details(distorted).context("distorted metadata")?;

    // The scorer is created before any frame is read, so a bad configuration or
    // an unusable device is reported before spending time decoding.
    let opened = Instant::now();
    let frame_rate = {
        let rate = reference_details.frame_rate;
        (*rate.numer() as f32) / (*rate.denom() as f32)
    };
    let vship_config = scorer_config(config, frame_rate)?;
    let metric = vship_config.metric();

    // Which Butteraugli norm is the headline depends on the configuration, not
    // on the metric. The VapourSynth plugin reads `BUTTERAUGLI_INFNorm`
    // unless a norm is requested and `BUTTERAUGLI_QNorm` when one is, so
    // the native path has to follow the same rule or the two report
    // different quantities for the same frames -- and `target_range` is
    // calibrated against whichever the plugin reported.
    let butteraugli_uses_q_norm = butteraugli_uses_q_norm(config);

    // Independent frames already clear both history and score after compute.
    let reset_on_discontinuity =
        vship_config.cvvdp().reset_on_discontinuity && !vship_config.cvvdp().disable_temporal;

    let reference_format = VideoFormat::from_details(&reference_details);
    let distorted_format = VideoFormat::from_details(&distorted_details);

    // `resolution` becomes libvship's `target_width`/`target_height` rather
    // than a rescale: libvship scales each input to it internally.
    let target_resolution = resolution(config);

    let mut scorer = VshipScorer::new(
        vship_config,
        reference_format,
        distorted_format,
        target_resolution,
    )
    .map_err(describe_error)?;
    let setup = opened.elapsed();

    let total = selected.len();
    let mut scores = Vec::with_capacity(total);
    // `previous` tracks the source index the temporal handler last saw, so the
    // next submission can be classified as contiguous or a discontinuity.
    let mut previous: Option<usize> = None;

    let pass = Instant::now();
    probe::probe_selected_frames_from::<_>(
        reference,
        distorted,
        selected,
        indexing,
        output_start,
        reference_next,
        cancelled,
        probe::check_geometry,
        |position, reference_planes, distorted_planes| {
            let reference_planes = metric_planes(reference_planes)?;
            let distorted_planes = metric_planes(distorted_planes)?;
            let source_index = selected[position];

            // A temporal handler must see every frame in order. When the
            // selection skips frames -- `ProbeStrategy::Skip` or `Subset`, or a
            // scene boundary -- the handler would otherwise fold a
            // discontinuity into its motion history, so its history
            // is cleared first.
            let reset_temporal = metric.is_temporal()
                && reset_on_discontinuity
                && previous.is_some_and(|previous| previous.checked_add(1) != Some(source_index));

            let frame_score = scorer
                .submit_pair_raw_with_reset(reference_planes, distorted_planes, reset_temporal)
                .map_err(describe_error)?;

            let value = match metric {
                VshipMetric::Butteraugli => frame_score
                    .butteraugli_value(butteraugli_uses_q_norm)
                    .ok_or({
                        crate::models::sequence::target_quality::types::MeasurementError::Missing {
                            position,
                        }
                    })
                    .context("vship butteraugli result")?,
                _ => frame_score
                    .value(metric)
                    .ok_or({
                        crate::models::sequence::target_quality::types::MeasurementError::Missing {
                            position,
                        }
                    })
                    .with_context(|| format!("vship {} result", config.friendly_name()))?,
            };

            crate::models::sequence::target_quality::types::validate_score(value, position)
                .with_context(|| format!("vship {} result", config.friendly_name()))?;
            previous = Some(source_index);
            on_score(position, value);
            scores.push(value);
            Ok(())
        },
    )?;
    let exchanged = pass.elapsed();

    tracing::debug!(
        frames = selected.len(),
        ?setup,
        pass = ?exchanged,
        "vship pass"
    );

    crate::models::sequence::target_quality::types::validate_scores(&scores, Some(selected.len()))
        .with_context(|| format!("vship {} result", config.friendly_name()))?;

    Ok(scores)
}

/// Adapt the driver's plane set to the one `av-metrics-vship` submits.
///
/// The two descriptors carry the same fields, so this is a field-by-field copy
/// rather than a reinterpretation: the pointer still aliases the frame the
/// driver is holding, and libvship reads the planes before the frame is
/// dropped.
#[inline]
fn metric_planes(planes: PlaneSet<'_>) -> Result<MetricPlaneSet<'_>> {
    let adapt = |plane: probe::PlaneSource<'_>| {
        // SAFETY: every field is copied unchanged from a `PlaneSource` the
        // driver produced from a live frame. The driver consumes the set inside
        // the `submit` call that receives it, so the frame outlives this
        // reference, and libvship only reads the planes before it is dropped.
        unsafe {
            MetricPlaneSource::from_raw_parts(
                plane.data_ptr(),
                plane.readable_len(),
                plane.width(),
                plane.height(),
                plane.stride(),
                plane.data_origin(),
                plane.bytes_per_sample(),
            )
        }
    };
    let [y, u, v] = planes;
    Ok([adapt(y)?, adapt(u)?, adapt(v)?])
}

/// Turn a scorer error into a message that says what to do about it.
#[inline]
fn describe_error(error: av_metrics_vship::VshipError) -> anyhow::Error {
    anyhow::anyhow!(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vapoursynth::plugins::vship::cvvdp::DisplayModel;

    /// A SSIMULACRA2 metric configuration.
    fn ssimulacra2(threads: Option<u8>) -> QualityMetric {
        QualityMetric::SSIMULACRA2 {
            target_range: (74.0, 76.0),
            resolution: None,
            threads,
            gpu_id: None,
        }
    }

    #[test]
    fn vship_maps_ssimulacra2_butteraugli_and_cvvdp() {
        let butteraugli = QualityMetric::BUTTERAUGLI {
            target_range: (1.0, 3.0),
            resolution: None,
            threads: None,
            intensity_multiplier: None,
            norm: None,
            gpu_id: None,
        };
        let cvvdp = QualityMetric::CVVDP {
            target_range: (0.0, 1.0),
            resolution: None,
            display_model: None,
            resize_to_display: None,
            disable_temporal: None,
            gpu_id: None,
        };

        assert_eq!(
            vship_metric_for(&ssimulacra2(None)),
            Some(VshipMetric::Ssimulacra2)
        );
        assert_eq!(
            vship_metric_for(&butteraugli),
            Some(VshipMetric::Butteraugli)
        );
        assert_eq!(vship_metric_for(&cvvdp), Some(VshipMetric::Cvvdp));
    }

    #[test]
    fn metrics_libvship_does_not_implement_are_rejected() {
        let vmaf = QualityMetric::VMAF {
            target_range: (94.0, 96.0),
            resolution: None,
            scaler: String::new(),
            threads: 1,
            model: None,
            features: vec![],
        };
        let xpsnr = QualityMetric::XPSNR {
            target_range: (40.0, 45.0),
            resolution: None,
        };

        assert_eq!(vship_metric_for(&vmaf), None);
        assert_eq!(vship_metric_for(&xpsnr), None);
        assert!(scorer_config(&vmaf, 24.0).is_err());
        assert!(scorer_config(&xpsnr, 24.0).is_err());
    }

    #[test]
    fn an_absent_thread_count_takes_the_plugin_default_of_four() {
        let config = scorer_config(&ssimulacra2(None), 24.0).expect("config should build");
        assert_eq!(
            config.handler_threads(),
            av_metrics_vship::DEFAULT_HANDLER_THREADS
        );
        assert_eq!(config.handler_threads(), 4);
    }

    #[test]
    fn vship_config_preserves_explicit_handler_threads() {
        let config = scorer_config(&ssimulacra2(Some(2)), 24.0).expect("config should build");
        assert_eq!(config.handler_threads(), 2);
    }

    /// A named device must reach libvship, and an unnamed one must leave the
    /// choice to the library rather than defaulting to index 0 — that default
    /// would silently override the discrete-GPU preference.
    #[test]
    fn a_configured_gpu_id_is_passed_through() {
        for metric in [
            QualityMetric::SSIMULACRA2 {
                target_range: (74.0, 76.0),
                resolution: None,
                threads: None,
                gpu_id: Some(2),
            },
            QualityMetric::BUTTERAUGLI {
                target_range: (1.0, 3.0),
                resolution: None,
                threads: None,
                intensity_multiplier: None,
                norm: None,
                gpu_id: Some(2),
            },
            QualityMetric::CVVDP {
                target_range: (9.4, 9.6),
                resolution: None,
                display_model: None,
                resize_to_display: None,
                disable_temporal: None,
                gpu_id: Some(2),
            },
        ] {
            let config = scorer_config(&metric, 24.0).expect("config should build");

            assert!(
                config.has_explicit_gpu_id(),
                "{}: gpu_id should be set",
                metric.friendly_name()
            );
            assert_eq!(
                config.gpu_id(),
                2,
                "{}: gpu_id should be 2",
                metric.friendly_name()
            );
        }
    }

    #[test]
    fn an_unset_gpu_id_leaves_the_device_to_libvship() {
        for metric in [
            ssimulacra2(None),
            QualityMetric::BUTTERAUGLI {
                target_range: (1.0, 3.0),
                resolution: None,
                threads: None,
                intensity_multiplier: None,
                norm: None,
                gpu_id: None,
            },
            QualityMetric::CVVDP {
                target_range: (9.4, 9.6),
                resolution: None,
                display_model: None,
                resize_to_display: None,
                disable_temporal: None,
                gpu_id: None,
            },
        ] {
            let config = scorer_config(&metric, 24.0).expect("config should build");

            assert!(
                !config.has_explicit_gpu_id(),
                "{}: gpu_id should be left unset so libvship picks a discrete device",
                metric.friendly_name()
            );
        }
    }

    #[test]
    fn butteraugli_norm_and_intensity_pass_through() {
        let metric = QualityMetric::BUTTERAUGLI {
            target_range: (1.0, 3.0),
            resolution: None,
            threads: Some(3),
            intensity_multiplier: Some(250.0),
            norm: Some(3),
            gpu_id: None,
        };
        let config = scorer_config(&metric, 24.0).expect("config should build");

        assert_eq!(config.metric(), VshipMetric::Butteraugli);
        assert_eq!(config.handler_threads(), 3);
        assert_eq!(config.butteraugli().q_norm, 3);
        assert!((config.butteraugli().intensity_multiplier - 250.0).abs() < f32::EPSILON);
    }

    #[test]
    fn vship_butteraugli_uses_q_norm_only_when_explicitly_configured() {
        // No norm requested: the plugin reads `BUTTERAUGLI_INFNorm`.
        let default = QualityMetric::BUTTERAUGLI {
            target_range: (0.8, 1.2),
            resolution: None,
            threads: None,
            intensity_multiplier: None,
            norm: None,
            gpu_id: None,
        };
        assert!(
            !butteraugli_uses_q_norm(&default),
            "with no norm the plugin reports the infinity norm, so the native path must too"
        );

        // A norm requested: the plugin reads `BUTTERAUGLI_QNorm`. Note this is
        // the configured-norm value, not the distortion map's L3 norm,
        // so `butteraugli-3` selects `norm_q` rather than `norm_3`.
        for norm in [2, 3, 5] {
            let configured = QualityMetric::BUTTERAUGLI {
                target_range: default.target_range(),
                resolution: None,
                threads: None,
                intensity_multiplier: None,
                norm: Some(norm),
                gpu_id: None,
            };
            assert!(
                butteraugli_uses_q_norm(&configured),
                "with norm {norm} the plugin reports the Q-norm, so the native path must too"
            );
        }
    }

    #[test]
    fn cvvdp_display_model_and_temporal_options_pass_through() {
        let metric = QualityMetric::CVVDP {
            target_range: (0.0, 1.0),
            resolution: None,
            display_model: Some(DisplayModel::StandardFHD),
            resize_to_display: Some(true),
            disable_temporal: Some(true),
            gpu_id: None,
        };
        let config = scorer_config(&metric, 23.976).expect("config should build");

        assert_eq!(config.metric(), VshipMetric::Cvvdp);
        assert_eq!(config.cvvdp().display_model, "standard_fhd");
        assert!(config.cvvdp().resize_to_display);

        assert!(config.cvvdp().disable_temporal);
        assert!(config.cvvdp().reset_on_discontinuity);
    }

    #[test]
    fn explicit_source_frame_rate_is_never_replaced_by_default() {
        let metric = QualityMetric::CVVDP {
            target_range: (0.0, 1.0),
            resolution: None,
            display_model: None,
            resize_to_display: None,
            disable_temporal: None,
            gpu_id: None,
        };

        for rate in [
            0.0,
            -0.0,
            -1.0,
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            100_000.01,
            f32::MAX,
            24_000.0 / 1001.0,
        ] {
            assert_eq!(
                scorer_config(&metric, rate)
                    .expect("mapping preserves the explicit rate for constructor validation")
                    .cvvdp()
                    .fps
                    .to_bits(),
                rate.to_bits()
            );
        }
    }
}
