//! Native CPU scoring for SSIMULACRA2, Butteraugli and CVVDP via fmetrics.
//!
//! fmetrics is the CPU implementation of the three metrics libvship runs on a
//! GPU, so it can be the fallback for a machine with no usable device. Butteraugli
//! requires an explicit finite norm: the application's default infinity norm has
//! no fmetrics equivalent. It does not
//! agree with libvship to the last digit, so a score records which engine
//! produced it -- see [`super::Engine`].
//!
//! Decoding and frame lifetime belong to [`super::probe`]; this module supplies
//! the metric mapping, the plane adaptor, and the submit loop. Where this
//! engine sits relative to the plugin branches is decided once in
//! [`super::engine`].

use std::{sync::atomic::AtomicBool, time::Instant};

use anyhow::{Context as _, Result, bail};
use av_metrics_fmetrics::{
    ColorInfo, FmetricsConfig, FmetricsCvvdpDisplayModel, FmetricsMetric, FmetricsScorer,
    PlaneSet as MetricPlaneSet, PlaneSource as MetricPlaneSource,
};

use crate::{
    metrics::probe::{self, OutputIndexing, PlaneSet},
    models::sequence::target_quality::types::QualityMetric,
    vapoursynth::plugins::vship::cvvdp::DisplayModel,
};

/// The fmetrics metric a configured [`QualityMetric`] maps to, or `None` when
/// the metric has no fmetrics equivalent.
///
/// VMAF, XPSNR and Butteraugli's default infinity norm have no fmetrics equivalent.
#[inline]
#[must_use]
pub const fn fmetrics_metric_for(metric: &QualityMetric) -> Option<FmetricsMetric> {
    match metric {
        QualityMetric::SSIMULACRA2 { .. } => Some(FmetricsMetric::Ssimulacra2),
        QualityMetric::BUTTERAUGLI { norm: Some(_), .. } => Some(FmetricsMetric::Butteraugli),
        QualityMetric::CVVDP { .. } => Some(FmetricsMetric::Cvvdp),
        QualityMetric::BUTTERAUGLI { norm: None, .. }
        | QualityMetric::VMAF { .. }
        | QualityMetric::XPSNR { .. } => None,
    }
}

/// Whether the fmetrics path should be attempted for `metric`.
///
/// An absent library leaves the caller's existing branch in place.
#[inline]
#[must_use]
pub fn native_supported(metric: &QualityMetric) -> bool {
    fmetrics_metric_for(metric).is_some() && av_metrics_fmetrics::is_available()
}

/// Build an [`FmetricsConfig`] from Av1an's metric configuration.
///
/// `threads` becomes the number of workspaces, so it bounds how many
/// submissions can be inside the library at once.
///
/// `frame_rate` is an explicit source rate, passed unchanged for CVVDP.
/// The scorer rejects nonfinite values and values outside `0 < fps <= 100000`;
/// invalid metadata must not silently become the low-level 60 fps default.
///
/// `resolution` is **not** applied: fmetrics scores at the source resolution
/// and cannot scale, so a configured target resolution is honoured here only by
/// coincidence.
///
/// # Errors
///
/// Returns an error for a metric fmetrics does not implement.
#[inline]
pub fn scorer_config(metric: &QualityMetric, frame_rate: f32) -> Result<FmetricsConfig> {
    match metric {
        QualityMetric::SSIMULACRA2 { threads, .. } => {
            Ok(FmetricsConfig::new(FmetricsMetric::Ssimulacra2)
                .with_threads(worker_threads(*threads)))
        },
        QualityMetric::BUTTERAUGLI {
            threads,
            intensity_multiplier,
            norm,
            ..
        } => {
            let Some(norm) = *norm else {
                bail!(
                    "Butteraugli infinity norm unavailable in fmetrics 0.1.0; use libvship/VS plugin \
                     or explicitly request a finite norm"
                );
            };
            let mut config = FmetricsConfig::new(FmetricsMetric::Butteraugli)
                .with_threads(worker_threads(*threads))
                .with_pnorm(i32::from(norm));
            if let Some(intensity_multiplier) = *intensity_multiplier {
                config = config.with_intensity_target(intensity_multiplier as f32);
            }
            Ok(config)
        },
        QualityMetric::CVVDP {
            display_model,
            disable_temporal,
            ..
        } => {
            let mut config = FmetricsConfig::new(FmetricsMetric::Cvvdp)
                .with_threads(av_metrics_fmetrics::DEFAULT_THREADS)
                .with_frame_rate(frame_rate);
            if let Some(display_model) = display_model {
                config = config.with_display_model(display_model_for(*display_model));
            }
            if let Some(disable_temporal) = *disable_temporal {
                config = config.with_disable_temporal(disable_temporal);
            }
            Ok(config)
        },
        QualityMetric::VMAF { .. } | QualityMetric::XPSNR { .. } => {
            bail!("{} has no fmetrics equivalent", metric.friendly_name());
        },
    }
}

/// fmetrics worker count for a configured `threads` value.
#[inline]
fn worker_threads(threads: Option<u8>) -> u32 {
    threads.map_or(av_metrics_fmetrics::DEFAULT_THREADS, u32::from)
}

/// Map Av1an's display model onto fmetrics'.
///
/// One-to-one: both enums carry the same five HDR variants.
#[inline]
#[must_use]
const fn display_model_for(model: DisplayModel) -> FmetricsCvvdpDisplayModel {
    match model {
        DisplayModel::Standard4K => FmetricsCvvdpDisplayModel::Standard4K,
        DisplayModel::StandardFHD => FmetricsCvvdpDisplayModel::StandardFhd,
        DisplayModel::StandardHDRPQ => FmetricsCvvdpDisplayModel::StandardHdrPq,
        DisplayModel::StandardHDRHLG => FmetricsCvvdpDisplayModel::StandardHdrHlg,
        DisplayModel::StandardHDRLinear => FmetricsCvvdpDisplayModel::StandardHdrLinear,
        DisplayModel::StandardHDRDark => FmetricsCvvdpDisplayModel::StandardHdrDark,
        DisplayModel::StandardHDRLinearZoom => FmetricsCvvdpDisplayModel::StandardHdrLinearZoom,
    }
}

/// Score only the frames named by `selected`, returning one score per
/// selection.
///
/// `indexing` states each side's index space; see `probe::output_index`.
///
/// Disabled-temporal CVVDP frames are independent. Enabled CVVDP with source
/// gaps is rejected before scoring: fmetrics cannot clear history while keeping
/// accumulated scores, so the caller must use its existing backend fallback.
/// `on_score` is called per successfully scored pair.
///
/// # Errors
///
/// Returns an error when the clips disagree, when fmetrics is unavailable or
/// rejects the configuration, or when a frame cannot be decoded or scored.
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

    // Built before any frame is read, so a bad configuration costs nothing.
    let opened = Instant::now();
    let frame_rate = {
        let rate = reference_details.frame_rate;
        (*rate.numer() as f32) / (*rate.denom() as f32)
    };
    let fmetrics_config = scorer_config(config, frame_rate)?;
    let metric = fmetrics_config.metric();

    if metric.is_temporal()
        && !fmetrics_config.disable_temporal
        && fmetrics_config.reset_on_discontinuity
        && selected.windows(2).any(|pair| pair[0].checked_add(1) != Some(pair[1]))
    {
        bail!(
            "fmetrics 0.1.0 cannot perform history-only CVVDP resets at selected source gaps; \
             use disable_temporal=true for independent frames or another backend"
        );
    }

    // Which Butteraugli norm is the headline depends on the configuration, not
    // the metric, and `target_range` is calibrated against the plugin's
    // choice.
    let butteraugli_uses_q_norm =
        matches!(config, QualityMetric::BUTTERAUGLI { norm: Some(_), .. });

    let reference_color =
        ColorInfo::from_details(&reference_details).context("reference color metadata")?;
    let distorted_color =
        ColorInfo::from_details(&distorted_details).context("distorted color metadata")?;

    let mut scorer =
        FmetricsScorer::new_with_colors(fmetrics_config, reference_color, distorted_color)
            .map_err(describe_error)?;
    let setup = opened.elapsed();

    let total = selected.len();
    let mut scores = Vec::with_capacity(total);

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

            // `submit_pair` refuses a context-backed scorer, so CVVDP must go
            // through the entry point that owns its accumulating context.
            let frame_score = if metric.is_temporal() {
                scorer.submit_pair_temporal(&reference_planes, &distorted_planes)
            } else {
                scorer.submit_pair(&reference_planes, &distorted_planes)
            }
            .map_err(describe_error)?;

            let value = frame_score
                .value(metric)
                .ok_or({
                    crate::models::sequence::target_quality::types::MeasurementError::Missing {
                        position,
                    }
                })
                .with_context(|| format!("fmetrics {} result", config.friendly_name()))?;

            crate::models::sequence::target_quality::types::validate_score(value, position)
                .with_context(|| format!("fmetrics {} result", config.friendly_name()))?;

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
        ?butteraugli_uses_q_norm,
        "fmetrics pass"
    );

    crate::models::sequence::target_quality::types::validate_scores(&scores, Some(selected.len()))
        .with_context(|| format!("fmetrics {} result", config.friendly_name()))?;

    Ok(scores)
}

/// Adapt the driver's plane set to the one `av-metrics-fmetrics` submits.
///
/// Retains the frame borrow and its known span; does not infer allocation length
/// from geometry. Only visible samples (not padding) are initialized by VS.
#[inline]
fn metric_planes(planes: PlaneSet<'_>) -> Result<MetricPlaneSet<'_>> {
    let [y, u, v] = planes.map(|plane| {
        // SAFETY: the driver's private descriptors carry an actual frame
        // borrow, known span and immutable geometry. The returned lifetime
        // is that same borrow, and only initialized visible samples are read.
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
    });
    Ok(MetricPlaneSet::new([y?, u?, v?]))
}

/// Turn a scorer error into a message that says what to do about it.
#[inline]
fn describe_error(error: av_metrics_fmetrics::FmetricsError) -> anyhow::Error {
    let corrupt = matches!(
        error,
        av_metrics_fmetrics::FmetricsError::NonFiniteButteraugli { .. }
    );
    let error = anyhow::anyhow!(error);
    if corrupt {
        error.context(
            crate::models::sequence::target_quality::types::MeasurementError::Statistic(
                "backend reported a non-finite Butteraugli result",
            ),
        )
    } else {
        error
    }
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "a failing assertion should panic loudly, which is what a unit test wants"
)]
mod tests {
    use super::*;

    #[test]
    fn infinity_has_no_fmetrics_mapping_or_native_support() {
        let metric = QualityMetric::BUTTERAUGLI {
            target_range: (1.0, 3.0),
            resolution: None,
            threads: Some(2),
            intensity_multiplier: Some(150.0),
            norm: None,
            gpu_id: None,
        };
        assert!(fmetrics_metric_for(&metric).is_none());
        assert!(!native_supported(&metric));
    }

    #[test]
    fn fmetrics_infinite_butteraugli_norm_reports_the_finite_plugin_alternative() {
        let metric = QualityMetric::BUTTERAUGLI {
            target_range: (1.0, 3.0),
            resolution: None,
            threads: Some(2),
            intensity_multiplier: Some(150.0),
            norm: None,
            gpu_id: None,
        };
        let error = scorer_config(&metric, 24.0).unwrap_err().to_string();
        assert!(
            error.contains("infinity")
                && error.contains("fmetrics 0.1.0")
                && error.contains("finite")
                && error.contains("plugin"),
            "{error}"
        );
    }

    #[test]
    fn explicit_finite_config_preserves_threads_intensity_and_exponent() {
        for norm in [3, 7, u8::MAX] {
            let metric = QualityMetric::BUTTERAUGLI {
                target_range: (1.0, 3.0),
                resolution: None,
                threads: Some(2),
                intensity_multiplier: Some(150.0),
                norm: Some(norm),
                gpu_id: None,
            };
            assert_eq!(
                fmetrics_metric_for(&metric),
                Some(FmetricsMetric::Butteraugli)
            );
            let config = scorer_config(&metric, 24.0).unwrap();
            assert_eq!(config.threads(), 2);
            assert_eq!(config.butteraugli_options().pnorm, i32::from(norm));
            assert_eq!(config.butteraugli_options().intensity_target, 150.0);
        }
    }

    #[test]
    fn driver_rejects_hdr_on_either_side_before_native_scoring() {
        for hdr_side in [0, 1] {
            let mut decoders: Vec<_> = (0..2).map(|side| {
                let transfer = if side == hdr_side { 16 } else { 13 };
                av_decoders::Decoder::from_script(&format!(
                    "import vapoursynth as vs\nvs.core.std.BlankClip(width=32, height=32, length=1, format=vs.YUV420P8).std.SetFrameProps(_Primaries=1, _Transfer={transfer}).set_output()"
                ), Default::default()).expect("open metadata fixture")
            }).collect();
            let (reference, distorted) = decoders.split_at_mut(1);
            let error = score_probed_frames(
                &mut reference[0],
                &mut distorted[0],
                &ssimulacra2(None),
                &[0],
                OutputIndexing::Aligned,
                None,
                |_, _| panic!("unsupported HDR must not score"),
            )
            .unwrap_err();
            let error = format!("{error:#}");
            assert!(
                error.contains("transfer")
                    && error.contains(if hdr_side == 0 {
                        "reference"
                    } else {
                        "distorted"
                    }),
                "{error}"
            );
        }
    }

    /// A SSIMULACRA2 metric configuration.
    fn ssimulacra2(threads: Option<u8>) -> QualityMetric {
        QualityMetric::SSIMULACRA2 {
            target_range: (74.0, 76.0),
            resolution: None,
            threads,
            gpu_id: None,
        }
    }

    /// Fixture width and height.
    ///
    /// Below ~32 pixels SSIMULACRA2's scale pyramid has nothing to compare and
    /// returns 100.0 for any input. Measured on a 30-level shift: 8x8 gives
    /// 89.2, 16x16 gives 86.0, 32x32 gives 57.1.
    const FIXTURE_SIZE: usize = 32;

    /// A y4m with a diagonal luma ramp, optionally shifted.
    ///
    /// Not flat: SSIMULACRA2 is scale invariant, so a uniform offset on a
    /// constant picture is legitimately scored as no distortion.
    fn y4m_gradient(shift: u8) -> Vec<u8> {
        let size = FIXTURE_SIZE;
        let mut bytes = format!("YUV4MPEG2 W{size} H{size} F24:1 Ip A1:1 C420mpeg2\n").into_bytes();

        for frame in 0..4usize {
            bytes.extend_from_slice(b"FRAME\n");
            for row in 0..size {
                let ramp: Vec<u8> = (0..size)
                    .map(|column| {
                        let base = (row * 4 + column * 3 + frame * 5) as u16;
                        (base + u16::from(shift)).clamp(16, 235) as u8
                    })
                    .collect();
                bytes.extend_from_slice(&ramp);
            }
            // Neutral chroma, one plane per 2x2 cell.
            let chroma_len = (size / 2) * (size / 2);
            bytes.extend(std::iter::repeat_n(128u8, chroma_len));
            bytes.extend(std::iter::repeat_n(128u8, chroma_len));
        }
        bytes
    }

    /// Write `bytes` to a y4m fixture that is removed when it goes out of
    /// scope.
    fn y4m_fixture(bytes: &[u8]) -> tempfile::TempPath {
        let file = tempfile::Builder::new()
            .suffix(".y4m")
            .tempfile()
            .expect("create a y4m fixture");
        let path = file.into_temp_path();
        std::fs::write(&path, bytes).expect("write the y4m fixture");
        path
    }

    /// Score a gradient clip pair differing by `shift` levels of luma.
    ///
    /// Drives the real submit loop, so a metric routed to the wrong entry point
    /// fails here rather than passing every mapping test.
    fn score_pair(metric: &QualityMetric, shift: u8) -> Result<Vec<f64>> {
        let reference = y4m_fixture(&y4m_gradient(0));
        let distorted = y4m_fixture(&y4m_gradient(shift));
        let selected: Vec<usize> = (0..4).collect();

        let mut reference =
            av_decoders::Decoder::from_file(&reference).expect("the reference fixture decodes");
        let mut distorted =
            av_decoders::Decoder::from_file(&distorted).expect("the distorted fixture decodes");

        score_probed_frames(
            &mut reference,
            &mut distorted,
            metric,
            &selected,
            OutputIndexing::Aligned,
            None,
            |_, _| {},
        )
    }

    #[test]
    fn every_supported_metric_scores_a_pair_through_the_driver() {
        if !av_metrics_fmetrics::is_available() {
            return;
        }

        let butteraugli = QualityMetric::BUTTERAUGLI {
            target_range: (1.0, 3.0),
            resolution: None,
            threads: None,
            intensity_multiplier: None,
            norm: Some(3),
            gpu_id: None,
        };

        // Each metric must produce one score per selected frame. CVVDP is the
        // case that distinguishes the two submit entry points.
        for metric in [ssimulacra2(None), butteraugli, cvvdp()] {
            let scores = score_pair(&metric, 30).unwrap_or_else(|error| {
                panic!("{} failed to score: {error:#}", metric.friendly_name())
            });

            assert_eq!(
                scores.len(),
                4,
                "{} should score every selected frame",
                metric.friendly_name()
            );
            assert!(
                scores.iter().all(|score| score.is_finite()),
                "{} produced a non-finite score",
                metric.friendly_name()
            );
        }
    }

    /// A temporal filter accumulates, so its per-frame scores cannot all be
    /// equal on content that differs frame to frame. Equal values would mean
    /// each frame was scored independently, which is what the per-pair path
    /// does.
    #[test]
    fn cvvdp_accumulates_rather_than_scoring_each_frame_alone() {
        if !av_metrics_fmetrics::is_available() {
            return;
        }

        let scores = score_pair(&cvvdp(), 30).expect("CVVDP scores");

        assert!(
            scores.windows(2).any(|pair| pair[0] != pair[1]),
            "CVVDP scores should change as the temporal filter accumulates, got {scores:?}"
        );
    }

    /// Confirms the fixture pair really differs, in both metric conventions:
    /// SSIMULACRA2 is a score below its ceiling, Butteraugli a distance above
    /// zero. Without this every other assertion here also holds for an
    /// identical pair.
    #[test]
    fn a_distorted_gradient_scores_below_the_ceiling() {
        if !av_metrics_fmetrics::is_available() {
            return;
        }

        let scores = score_pair(&ssimulacra2(None), 30).expect("SSIMULACRA2 scores");
        assert!(
            scores.iter().all(|score| *score < 100.0),
            "a shifted gradient should not score at the ceiling, got {scores:?}"
        );

        let butteraugli = QualityMetric::BUTTERAUGLI {
            target_range: (1.0, 3.0),
            resolution: None,
            threads: None,
            intensity_multiplier: None,
            norm: Some(3),
            gpu_id: None,
        };
        let distances = score_pair(&butteraugli, 30).expect("Butteraugli scores");
        assert!(
            distances.iter().all(|distance| *distance > 0.0),
            "a shifted gradient should register a non-zero distance, got {distances:?}"
        );
    }

    /// A CVVDP metric configuration with no display model.
    fn cvvdp() -> QualityMetric {
        cvvdp_with(None)
    }

    /// A CVVDP configuration with the given `disable_temporal`.
    fn cvvdp_with(disable_temporal: Option<bool>) -> QualityMetric {
        QualityMetric::CVVDP {
            target_range: (40.0, 50.0),
            resolution: None,
            display_model: None,
            resize_to_display: None,
            disable_temporal,
            gpu_id: None,
        }
    }

    #[test]
    fn fmetrics_maps_ssimulacra2_butteraugli_and_cvvdp_only() {
        assert_eq!(
            fmetrics_metric_for(&ssimulacra2(None)),
            Some(FmetricsMetric::Ssimulacra2)
        );
        assert_eq!(
            fmetrics_metric_for(&QualityMetric::BUTTERAUGLI {
                target_range: (1.0, 3.0),
                resolution: None,
                threads: None,
                intensity_multiplier: None,
                norm: Some(3),
                gpu_id: None,
            }),
            Some(FmetricsMetric::Butteraugli)
        );
        assert_eq!(fmetrics_metric_for(&cvvdp()), Some(FmetricsMetric::Cvvdp));

        // Neither VMAF nor XPSNR has an fmetrics implementation, so neither may
        // claim one.
        assert!(
            fmetrics_metric_for(&QualityMetric::XPSNR {
                target_range: (45.0, 55.0),
                resolution: None,
            })
            .is_none()
        );
        assert!(
            fmetrics_metric_for(&QualityMetric::VMAF {
                target_range: (85.0, 95.0),
                resolution: None,
                scaler: "area".to_owned(),
                threads: 4,
                model: None,
                features: Vec::new(),
            })
            .is_none()
        );
    }

    #[test]
    fn availability_follows_the_metric_and_the_library() {
        // Availability follows the library, so the assertion mirrors it rather
        // than assuming either answer.
        assert_eq!(
            native_supported(&ssimulacra2(None)),
            av_metrics_fmetrics::is_available(),
            "SSIMULACRA2 is supported exactly when the library loaded"
        );
        assert!(
            !native_supported(&QualityMetric::XPSNR {
                target_range: (45.0, 55.0),
                resolution: None,
            }),
            "a metric fmetrics cannot score is never supported, whatever the library state"
        );
    }

    #[test]
    fn fmetrics_config_preserves_explicit_threads_or_uses_the_default() {
        assert_eq!(
            scorer_config(&ssimulacra2(Some(3)), 0.0).unwrap().threads(),
            3
        );
        // An absent value takes the crate default, not zero.
        assert_eq!(
            scorer_config(&ssimulacra2(None), 0.0).unwrap().threads(),
            av_metrics_fmetrics::DEFAULT_THREADS
        );
    }

    #[test]
    fn explicit_source_frame_rate_is_never_replaced_by_default() {
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
                scorer_config(&cvvdp(), rate).unwrap().frame_rate().to_bits(),
                rate.to_bits()
            );
        }
    }

    #[test]
    fn every_display_model_maps_to_a_distinct_fmetrics_model() {
        // A silent fall-through would score HDR content against an SDR model,
        // so the mapping must be total and injective.
        let models = [
            DisplayModel::Standard4K,
            DisplayModel::StandardFHD,
            DisplayModel::StandardHDRPQ,
            DisplayModel::StandardHDRHLG,
            DisplayModel::StandardHDRLinear,
            DisplayModel::StandardHDRDark,
            DisplayModel::StandardHDRLinearZoom,
        ];
        let mapped: std::collections::HashSet<_> =
            models.iter().copied().map(display_model_for).collect();
        assert_eq!(mapped.len(), models.len(), "two models collided");

        // Spot-check one HDR model, which has no default.
        assert_eq!(
            display_model_for(DisplayModel::StandardHDRPQ),
            FmetricsCvvdpDisplayModel::StandardHdrPq
        );
    }

    #[test]
    fn fmetrics_config_preserves_the_hdr_pq_display_model() {
        let config = scorer_config(
            &QualityMetric::CVVDP {
                target_range: (40.0, 50.0),
                resolution: None,
                display_model: Some(DisplayModel::StandardHDRPQ),
                resize_to_display: None,
                disable_temporal: None,
                gpu_id: None,
            },
            24.0,
        )
        .unwrap();
        assert_eq!(
            config.display_model(),
            FmetricsCvvdpDisplayModel::StandardHdrPq
        );
    }

    #[test]
    fn disable_temporal_maps_directly_without_changing_gap_policy() {
        for flag in [None, Some(false), Some(true)] {
            let config = scorer_config(&cvvdp_with(flag), 24.0).unwrap();
            assert_eq!(config.disable_temporal, flag.unwrap_or(false));
            assert!(config.reset_on_discontinuity);
        }
    }

    #[test]
    fn fmetrics_cvvdp_config_is_temporal_while_pair_metrics_are_not() {
        // The submit loop branches on this, and the branch is otherwise
        // unreachable from a unit test.
        assert!(FmetricsMetric::Cvvdp.is_temporal());
        for metric in [
            FmetricsMetric::Ssimulacra2,
            FmetricsMetric::Butteraugli,
            FmetricsMetric::Iwssim,
            FmetricsMetric::Msssim,
        ] {
            assert!(!metric.is_temporal(), "{metric:?} is per-pair");
        }

        let config = scorer_config(&cvvdp(), 24.0).unwrap();
        assert!(config.is_temporal(), "CVVDP must build a temporal config");
    }

    #[test]
    fn unsupported_metrics_are_rejected_rather_than_silently_substituted() {
        // Scoring XPSNR here would produce a number of the wrong kind.
        let error = scorer_config(
            &QualityMetric::XPSNR {
                target_range: (45.0, 55.0),
                resolution: None,
            },
            24.0,
        )
        .unwrap_err();
        assert!(error.to_string().contains("XPSNR"), "{error}");
    }
}
