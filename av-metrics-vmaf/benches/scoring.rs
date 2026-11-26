//! Criterion benchmarks for VMAF scoring.
//!
//! Run with `cargo bench -p av-metrics-vmaf`.
//!
//! These exist to guide local optimisation work and are deliberately **not**
//! run in CI: Criterion adds minutes per run and its output is noisy on shared
//! runners, so enforcing a regression threshold would produce flaky failures.
//!
//! All input is generated in memory by the shared test helpers, so no media
//! files are required. Content is deterministic, which keeps the noise floor
//! low.
//!
//! Benchmarks that need libvmaf are skipped when it is unavailable.

#[path = "../tests/common/mod.rs"]
mod common;

use std::hint::black_box;

use av_metrics_vmaf::{
    BackendPreference, PoolMethod, VideoFormat, VmafConfig, VmafScorer, is_available,
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
        eprintln!("skipping scoring benchmarks: libvmaf is not installed");
        return false;
    }
    true
}

/// Build a scorer for a generated pair, alongside fresh decoders.
fn scorer_for(
    pair: &SyntheticPair,
    config: VmafConfig,
) -> Option<(VmafScorer, av_decoders::Decoder, av_decoders::Decoder)> {
    let (reference, distorted) = pair.decoders();
    let details = *reference.get_video_details();
    let format = VideoFormat {
        width: details.width as u32,
        height: details.height as u32,
        bit_depth: details.bit_depth as u32,
        chroma_sampling: details.chroma_sampling,
    };

    let scorer = VmafScorer::new(config, format).ok()?;
    Some((scorer, reference, distorted))
}

/// Score a generated pair once, returning the pooled mean.
fn score_once(pair: &SyntheticPair, config: VmafConfig) -> Option<f64> {
    let (mut scorer, mut reference, mut distorted) = scorer_for(pair, config)?;
    let scores = scorer.score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {}).ok()?;
    VmafScorer::pool(&scores, PoolMethod::Mean).ok()
}

/// Throughput across the resolutions and bit depths that matter in practice.
fn throughput(criterion: &mut Criterion) {
    if !scoring_available() {
        return;
    }

    let mut group = criterion.benchmark_group("vmaf/throughput");

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
            bencher.iter(|| black_box(score_once(&pair, VmafConfig::default())));
        });
    }

    group.finish();
}

/// How scoring cost scales with libvmaf's worker thread count.
fn thread_scaling(criterion: &mut Criterion) {
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

    let mut group = criterion.benchmark_group("vmaf/threads");

    for threads in [1u32, 2, 4, 8] {
        let config = VmafConfig::default().with_threads(threads);
        group.throughput(Throughput::Elements(BENCH_FRAMES as u64));
        group.bench_function(format!("{threads}-threads"), |bencher| {
            bencher.iter(|| black_box(score_once(&pair, config.clone())));
        });
    }

    group.finish();
}

/// CUDA against CPU.
///
/// The selected backend is printed so a silent fallback to CPU can never be
/// mistaken for a CUDA result. This is also the experiment that settles whether
/// CUDA actually supports the stock VMAF models, since libvmaf's CUDA
/// extractors cover only integer ADM, motion and VIF.
fn cuda_vs_cpu(criterion: &mut Criterion) {
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

    for (label, preference) in
        [("cpu", BackendPreference::CpuOnly), ("auto", BackendPreference::Auto)]
    {
        let config = VmafConfig::default().with_backend(preference).with_threads(1);

        // Report the backend outside the measured loop so printing does not
        // pollute the numbers.
        if let Some((scorer, _, _)) = scorer_for(&pair, config.clone()) {
            eprintln!("backend for preference `{label}`: {}", scorer.backend());
        }

        let mut group = criterion.benchmark_group(format!("vmaf/backend-{label}"));
        group.throughput(Throughput::Elements(BENCH_FRAMES as u64));
        group.bench_function("score", |bencher| {
            bencher.iter(|| black_box(score_once(&pair, config.clone())));
        });
        group.finish();
    }
}

/// The cost of decoding alone, so we can tell whether optimisation effort
/// belongs in this crate or in libvmaf.
fn decode_only(criterion: &mut Criterion) {
    let pair = SyntheticPair::generate(
        1920,
        1080,
        8,
        ChromaSubsampling::Yuv420,
        BENCH_FRAMES,
        Distortion::Mild,
    );

    let mut group = criterion.benchmark_group("vmaf/decode-only");
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

criterion_group!(
    benches,
    throughput,
    thread_scaling,
    cuda_vs_cpu,
    decode_only
);
criterion_main!(benches);
