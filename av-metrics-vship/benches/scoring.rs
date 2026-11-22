//! Criterion benchmarks for vship scoring.
//!
//! Run with `cargo bench -p av-metrics-vship`.
//!
//! These exist to guide local optimisation work and are deliberately **not**
//! run in CI: Criterion adds minutes per run and its output is noisy on shared
//! runners, so enforcing a regression threshold would produce flaky failures.
//!
//! All input is generated in memory by the shared test helpers, so no media
//! files are required. Content is deterministic, which keeps the noise floor
//! low.
//!
//! Benchmarks that need libvship are skipped when it is unavailable, which on a
//! machine without a GPU means the scoring benchmarks do nothing.

#[path = "../tests/common/mod.rs"]
mod common;

use std::hint::black_box;

use av_metrics_vship::{
    PoolMethod, VideoFormat, VshipConfig, VshipMetric, VshipScorer, is_available,
};
use common::{Distortion, SyntheticPair};
use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use v_frame::chroma::ChromaSubsampling;

/// Frame count used by every benchmark. Large enough to amortise setup, small
/// enough to keep a full run to a few seconds.
const BENCH_FRAMES: usize = 30;

/// Whether scoring benchmarks can run.
fn scoring_available() -> bool {
    if !is_available() {
        eprintln!("skipping scoring benchmarks: libvship is not installed or no device is usable");
        return false;
    }
    true
}

/// Build a scorer for a generated pair, alongside fresh decoders.
fn scorer_for(
    pair: &SyntheticPair,
    config: VshipConfig,
) -> Option<(VshipScorer, av_decoders::Decoder, av_decoders::Decoder)> {
    let (reference, distorted) = pair.decoders();
    let format = VideoFormat::from_details(reference.get_video_details());

    let scorer = VshipScorer::new(config, format, format, None).ok()?;
    Some((scorer, reference, distorted))
}

/// Score a generated pair once, returning the pooled mean.
fn score_once(pair: &SyntheticPair, config: VshipConfig) -> Option<f64> {
    let (mut scorer, mut reference, mut distorted) = scorer_for(pair, config)?;
    let scores = scorer.score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {}).ok()?;

    VshipScorer::pool(&scores, VshipMetric::Ssimulacra2, PoolMethod::Mean, true).ok()
}

/// Throughput across the resolutions and bit depths that matter in practice.
fn throughput(criterion: &mut Criterion) {
    if !scoring_available() {
        return;
    }

    let mut group = criterion.benchmark_group("vship/throughput");

    for (label, (width, height, bit_depth, chroma)) in [
        (
            "720p-8bit",
            (1280usize, 720usize, 8u8, ChromaSubsampling::Yuv420),
        ),
        ("1080p-8bit", (1920, 1080, 8, ChromaSubsampling::Yuv420)),
        ("1080p-10bit", (1920, 1080, 10, ChromaSubsampling::Yuv420)),
    ] {
        let pair = SyntheticPair::generate(
            width,
            height,
            bit_depth,
            chroma,
            BENCH_FRAMES,
            Distortion::Mild,
        );

        group.throughput(Throughput::Elements(BENCH_FRAMES as u64));
        group.bench_function(label, |bencher| {
            bencher.iter(|| black_box(score_once(&pair, VshipConfig::default())));
        });
    }

    group.finish();
}

/// Cost per metric, so an encoder's default choice can be weighed against the
/// alternatives.
fn per_metric(criterion: &mut Criterion) {
    if !scoring_available() {
        return;
    }

    let pair = SyntheticPair::generate(
        1920,
        1080,
        8,
        ChromaSubsampling::Yuv420,
        BENCH_FRAMES,
        Distortion::Mild,
    );

    for metric in VshipMetric::all() {
        // All metrics use one synchronous handler.
        let config = VshipConfig::default().with_metric(metric).with_handler_threads(1);
        let label = metric.as_str().to_lowercase();

        let mut group = criterion.benchmark_group(format!("vship/metric-{label}"));
        group.throughput(Throughput::Elements(BENCH_FRAMES as u64));
        group.bench_function("score", |bencher| {
            bencher.iter(|| {
                // A machine whose device cannot build this metric contributes
                // no measurement rather than a failing
                // benchmark.
                let Some((mut scorer, mut reference, mut distorted)) =
                    scorer_for(&pair, config.clone())
                else {
                    return black_box(0.0);
                };
                let Ok(scores) =
                    scorer.score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {})
                else {
                    return black_box(0.0);
                };

                black_box(VshipScorer::pool(&scores, metric, PoolMethod::Mean, true).unwrap_or(0.0))
            });
        });
        group.finish();
    }
}

/// The cost of decoding alone, so it is clear whether optimisation effort
/// belongs in this crate or in the decoder.
fn decode_only(criterion: &mut Criterion) {
    let pair = SyntheticPair::generate(
        1920,
        1080,
        8,
        ChromaSubsampling::Yuv420,
        BENCH_FRAMES,
        Distortion::Mild,
    );

    let mut group = criterion.benchmark_group("vship/decode-only");
    group.throughput(Throughput::Elements(BENCH_FRAMES as u64));
    group.bench_function("y4m-decode", |bencher| {
        bencher.iter(|| {
            let mut decoder = pair.reference.decoder();
            let mut frames = 0;
            while decoder.read_video_frame::<u8>().is_ok() {
                frames += 1;
            }
            black_box(frames);
        });
    });
    group.finish();
}

criterion_group!(benches, throughput, per_metric, decode_only);
criterion_main!(benches);
