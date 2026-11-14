//! VMAF scoring on top of the shared decode/probe driver.
//!
//! Av1an's `QualityMetric::VMAF` predates the `av-metrics-vmaf` crate and
//! describes the metric in terms of the old VapourSynth plugin, where
//! `features` doubled as a model selector (`neg`, `uhd`, `weighted`). libvmaf
//! instead selects the model directly and registers extra feature extractors by
//! name, so the two vocabularies are reconciled here rather than in the metric
//! crate.
//!
//! Everything that is not specific to VMAF -- reading the two clips, pairing
//! the selected frames, handing each pair to a callback -- belongs to
//! [`super::probe`]. This module supplies the scorer, the per-pair submission
//! libvmaf needs, and the mapping from scorer errors to advice.

use std::{
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use av_metrics_vmaf::{
    BackendPreference, PlaneSet as MetricPlaneSet, PlaneSource as MetricPlaneSource, VideoFormat,
    VmafConfig as ScorerConfig, VmafError, VmafModel, VmafScorer,
};

use crate::{
    metrics::probe::{self, OutputIndexing, PlaneSet},
    models::sequence::target_quality::types::{QualityMetric, VmafFeature},
};

/// Build a `VmafConfig` from Av1an's metric configuration.
///
/// The `resolution` pre-scale is not applied here; VMAF scores at the
/// resolution it is given, so callers that need one must rescale the decoded
/// frames first. A `threads` value of 0 expands to one worker per available
/// core rather than being passed to libvmaf raw (see `vmaf_thread_count`).
///
/// # Errors
///
/// Returns an error when a feature has no libvmaf equivalent, or when an
/// explicit model path is missing. Validating early keeps the failure next to
/// the configuration that caused it.
#[inline]
pub fn scorer_config(metric: &QualityMetric) -> Result<ScorerConfig> {
    let QualityMetric::VMAF {
        threads,
        model,
        features,
        ..
    } = metric
    else {
        bail!("expected a VMAF metric configuration");
    };

    if features.contains(&VmafFeature::Motionless) {
        bail!("the `motionless` VMAF feature has no libvmaf equivalent");
    }

    // An explicit path wins; otherwise the feature list selects a stock model.
    let model = model.as_ref().map_or_else(
        || stock_model(features),
        |path| VmafModel::Path(path.clone()),
    );

    Ok(ScorerConfig::new()
        .with_model(model)
        .with_threads(vmaf_thread_count(*threads))
        // Try CUDA when libvmaf was built with it, and fall back to CPU
        // otherwise.
        .with_backend(BackendPreference::Auto))
}

/// libvmaf worker threads for a configured `threads` value.
///
/// Zero means "one worker per available core". Passing 0 through to libvmaf
/// leaves it with no worker pool rather than one per core, so the request is
/// expanded to the machine's parallelism here instead. A positive value is
/// honoured exactly, which matters for capping the thread pool on a shared
/// machine.
#[inline]
fn vmaf_thread_count(configured: usize) -> u32 {
    if configured == 0 {
        std::thread::available_parallelism()
            .map_or(1, |parallelism| parallelism.get())
            .try_into()
            .unwrap_or(u32::MAX)
    } else {
        u32::try_from(configured).unwrap_or(u32::MAX)
    }
}

/// Pick a stock libvmaf model from the feature selectors.
///
/// The features are a set, so a combination can be requested that names its own
/// stock model: `uhd` together with `neg` selects `vmaf_4k_v0.6.1neg`, the
/// 4K model trained for negative content. Checking `uhd` first would silently
/// drop `neg`, so the combined case is matched before either alone.
#[inline]
fn stock_model(features: &[VmafFeature]) -> VmafModel {
    let has = |feature: VmafFeature| features.contains(&feature);

    match (has(VmafFeature::Uhd), has(VmafFeature::Neg)) {
        (true, true) => VmafModel::UhdNeg,
        (true, false) => VmafModel::Uhd,
        (false, true) => VmafModel::Neg,
        (false, false) if has(VmafFeature::Weighted) => VmafModel::Weighted,
        (false, false) => VmafModel::Default,
    }
}

/// Score only the frames named by `selected`, returning one score per
/// selection.
///
/// The decode loop itself belongs to [`probe::probe_selected_frames`]; this
/// wraps it with the scorer and the per-pair submission libvmaf needs.
///
/// # Index spaces
///
/// The two decoders may use different index spaces, which is why `indexing` is
/// passed explicitly rather than inferred. See `probe::output_index`.
///
/// # Cost
///
/// Only the selected frames are decoded when a side can seek.
///
/// A caveat specific to VMAF: its motion and temporal extractors compare
/// consecutive frames, so a score is only meaningful within a contiguous run.
/// When the selection skips frames — `ProbeStrategy::Skip` or `Subset` — each
/// wanted frame is scored against whichever selected frame precedes it rather
/// than its true predecessor. Absolute numbers therefore differ slightly from a
/// continuous pass, though they stay directly comparable across encodes of the
/// same frames, which is what target-quality probing needs.
///
/// `selected` need not be sorted; frames are scored in the order given.
///
/// # Progress
///
/// `on_score` is called with `(position in the selection, score)` for each
/// scored frame after successful finalization, in selection order. For progress
/// during decoding/submission, use [`score_probed_frames_with_progress`].
///
/// # Errors
///
/// Returns an error when the two clips disagree on resolution or bit depth,
/// when libvmaf is unavailable, when seeking or decoding fails, or when a
/// selected frame lies past the end of the encoded output.
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
    score_probed_frames_with_progress(
        reference,
        distorted,
        config,
        selected,
        indexing,
        cancelled,
        |_| {},
        on_score,
    )
}

/// Score selected frames, reporting submission independently of final scores.
///
/// `on_submitted(position)` runs only after successful acceptance of a pair,
/// before finalization. `on_score(position, score)` runs only after successful
/// finalization and collection of every score. Positions index `selected`, not
/// source frames. Empty selections invoke neither native code nor callbacks.
///
/// # Errors
///
/// Reports configuration, decode, cancellation and native errors as in
/// [`score_probed_frames`]. Failed/rejected pairs have no submission callback;
/// finalization/query failures have no score callbacks or partial result vector.
/// Incomplete/non-finite native results are measurement errors; all values are
/// validated before the first score callback.
#[inline]
#[expect(
    clippy::too_many_arguments,
    reason = "preserve the score-only API while separating submitted progress"
)]
pub fn score_probed_frames_with_progress(
    reference: &mut av_decoders::Decoder,
    distorted: &mut av_decoders::Decoder,
    config: &QualityMetric,
    selected: &[usize],
    indexing: OutputIndexing,
    cancelled: Option<&AtomicBool>,
    mut on_submitted: impl FnMut(usize),
    mut on_score: impl FnMut(usize, f64),
) -> Result<Vec<f64>> {
    if selected.is_empty() {
        return Ok(Vec::new());
    }

    // A scorer is created before any frame is read, so a bad configuration is
    // reported before spending time decoding.
    let opened = Instant::now();
    let details = *reference.get_video_details();
    let format = VideoFormat {
        width: details.width as u32,
        height: details.height as u32,
        bit_depth: details.bit_depth as u32,
        chroma_sampling: details.chroma_sampling,
    };
    let mut scorer = VmafScorer::new(scorer_config(config)?, format).map_err(describe_error)?;
    let setup = opened.elapsed();

    // Decoding is hard to separate from libvmaf's extraction from the outside,
    // so the split is recorded where the calls happen: `exchanged` is the whole
    // probe pass, `submitted` only the libvmaf call, so the fetch cost is
    // roughly their difference.
    let mut submitted = Duration::ZERO;

    // libvmaf's temporal extractors depend on the *index sequence* given to
    // `vmaf_read_pictures`, not on frames arriving in one batch, so
    // interleaving decoding with submission produces identical scores.
    //
    // `submit_pair_raw` already returns the index it scored, so the driver's
    // per-pair results are collected but discarded: the scores that matter come
    // from `finish`, one per submitted pair, in submission order.
    let pass = Instant::now();
    probe::probe_selected_frames::<(), _>(
        reference,
        distorted,
        selected,
        indexing,
        cancelled,
        probe::check_geometry,
        |position, reference_planes, distorted_planes| {
            let submitted_at = Instant::now();
            let result = scorer
                .submit_pair_raw(
                    metric_planes(reference_planes)?,
                    metric_planes(distorted_planes)?,
                )
                .map(|_| ())
                .map_err(describe_error);
            submitted += submitted_at.elapsed();
            result?;
            on_submitted(position);
            Ok(())
        },
    )?;
    let exchanged = pass.elapsed();

    let finished = Instant::now();
    let scores = scorer.finish().map_err(describe_error)?;
    let finished = finished.elapsed();

    tracing::debug!(
        frames = selected.len(),
        ?setup,
        fetch = ?exchanged.saturating_sub(submitted),
        submit = ?submitted,
        finish = ?finished,
        "vmaf pass phases"
    );

    crate::models::sequence::target_quality::types::validate_scores(&scores, Some(selected.len()))
        .context("vmaf VMAF result")?;

    for (position, score) in scores.iter().enumerate() {
        on_score(position, *score);
    }

    Ok(scores)
}

/// Adapt the driver's plane set to the one `av-metrics-vmaf` submits.
///
/// Retains the driver's frame borrow and actual span, including when native
/// padding is uninitialized. Geometry is checked again at the metric boundary.
#[inline]
fn metric_planes(planes: PlaneSet<'_>) -> Result<MetricPlaneSet<'_>> {
    let [y, u, v] = planes.map(|plane| {
        // SAFETY: private driver descriptors carry the actual frame borrow,
        // known span, immutable geometry and initialized visible samples. The
        // returned lifetime is the same borrow, not a pointer-derived lifetime.
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
    Ok([y?, u?, v?])
}

/// Turn a scorer error into a message that says what to do about it.
#[inline]
fn describe_error(error: VmafError) -> anyhow::Error {
    match error {
        VmafError::LibraryNotFound => anyhow::anyhow!(
            "libvmaf is not installed. Install it, or set VMAF_LIB_DIR to the directory \
             containing it. See the av-metrics-vmaf README for per-distro instructions."
        ),
        other => anyhow::anyhow!(other),
    }
}

#[cfg(test)]
mod tests;
