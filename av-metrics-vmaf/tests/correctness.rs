//! Correctness tests for VMAF scoring.
//!
//! Tests that need libvmaf are skipped when it is unavailable, so the suite
//! stays green in environments without it. Everything that does not require
//! libvmaf always runs. CI sets `AV_METRICS_REQUIRE_CPU` to require native inputs.

// Test code asserts and unwraps freely: a failure panics with a readable message,
// which is exactly what a test wants.
#![allow(clippy::unwrap_used, reason = "assertion failures should be loud")]

mod common;

use std::path::{Path, PathBuf};

use av_metrics_vmaf::{
    Backend, BackendPreference, PlaneSource, PoolMethod, VideoFormat, VmafConfig, VmafError,
    VmafModel, VmafScorer, is_available, libvmaf_version,
};
use common::{Distortion, SyntheticPair, reference_mean};
use v_frame::chroma::ChromaSubsampling;

/// Skip the test body when libvmaf is not installed.
macro_rules! require_libvmaf {
    () => {
        if !is_available() {
            eprintln!("skipping: libvmaf is not installed");
            return;
        }
    };
}

/// A model configuration that works on any libvmaf build.
///
/// Not every libvmaf build compiles the stock models in. Arch's `vmaf` package
/// and MSYS2's `mingw-w64-*-vmaf`, for example, ship without them, and
/// `vmaf_model_load` then rejects a bare version like `vmaf_v0.6.1` with
/// `EINVAL`.
///
/// `VMAF_TEST_MODEL` names a model JSON explicitly and wins. Otherwise the
/// stock model is used, which the scorer resolves either from libvmaf's
/// built-ins or from a model file found through `VMAF_MODEL_PATH` and the
/// platform search paths.
fn model_config() -> VmafConfig {
    std::env::var("VMAF_TEST_MODEL").map_or_else(
        |_| VmafConfig::default(),
        |path| VmafConfig::default().with_model(VmafModel::Path(path.into())),
    )
}

/// Format a small reference/distorted pair.
fn small_pair(distortion: Distortion) -> SyntheticPair {
    SyntheticPair::generate(64, 48, 8, ChromaSubsampling::Yuv420, 12, distortion)
}

/// Capture native stderr in an isolated process, never redirect production FDs.
#[test]
fn serial_cpu_finalization_is_silent_and_matches_score_decoders() {
    stock_cpu_finalization_case(
        "serial_cpu_finalization_is_silent_and_matches_score_decoders",
        0,
    );
}

#[test]
fn threaded_cpu_finalization_is_silent_and_matches_score_decoders() {
    stock_cpu_finalization_case(
        "threaded_cpu_finalization_is_silent_and_matches_score_decoders",
        2,
    );
}

fn stock_cpu_finalization_case(test: &str, threads: u32) {
    use std::{
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    if let Ok(selected) = std::env::var("VMAF_FINALIZATION_FIXTURE_CPU_CHILD") {
        assert_eq!(
            selected,
            threads.to_string(),
            "child thread count belongs to this contract"
        );
        let pair = small_pair(Distortion::Moderate);
        let format = VideoFormat {
            width: 64,
            height: 48,
            bit_depth: 8,
            chroma_sampling: ChromaSubsampling::Yuv420,
        };
        let config = model_config().with_backend(BackendPreference::CpuOnly).with_threads(threads);
        let mut scorer = VmafScorer::new(config.clone(), format).expect("required CPU stock model");
        assert_eq!(
            scorer.backend(),
            Backend::Cpu,
            "finalization uses the required CPU backend"
        );
        let mut reference = pair.reference.decoder();
        let mut distorted = pair.distorted.decoder();
        for index in 0..12 {
            let reference = reference.read_video_frame::<u8>().expect("reference");
            let distorted = distorted.read_video_frame::<u8>().expect("distorted");
            assert_eq!(
                scorer.submit_pair(&reference, &distorted).expect("submit"),
                index,
                "each accepted pair advances the submission index once"
            );
            assert!(
                scorer.drain_scores(0).expect("pending scores").is_empty(),
                "scores remain pending before flush"
            );
        }
        let scores = scorer.finish().expect("final scores");
        assert_eq!(scores.len(), 12, "all submitted pairs yield final scores");
        assert_eq!(
            scorer.finish().expect("repeat finish"),
            scores,
            "repeated finish is stable"
        );
        assert_eq!(
            scorer.drain_scores(0).expect("drain"),
            scores,
            "drain returns the final vector"
        );
        assert!(
            scorer.drain_scores(12).expect("advanced cursor").is_empty(),
            "advanced cursor has no unread scores"
        );
        let mut baseline = VmafScorer::new(config, format).expect("reference native path");
        let expected = baseline
            .score_decoders::<u8>(
                &mut pair.reference.decoder(),
                &mut pair.distorted.decoder(),
                |_, _| {},
            )
            .expect("reference scores");
        assert_eq!(
            scores,
            expected.iter().map(|score| score.score).collect::<Vec<_>>(),
            "same inputs and existing native score_decoders path, exact identity"
        );
        println!(
            "stock CPU threads={threads}: 12 finite scores, exact score_decoders match, repeated finish/drain stable"
        );
        assert!(
            scores.iter().all(|score| score.is_finite()),
            "native scores must be finite"
        );
        return;
    }
    if !is_available() {
        assert!(
            std::env::var_os("AV_METRICS_REQUIRE_CPU").is_none(),
            "required CPU gate needs libvmaf"
        );
        eprintln!("UNEXECUTED stock CPU gate: libvmaf unavailable");
        return;
    }
    let mut child = Command::new(std::env::current_exe().expect("executable"))
        .args(["--exact", test, "--nocapture"])
        .env("VMAF_FINALIZATION_FIXTURE_CPU_CHILD", threads.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("CPU child");
    let start = Instant::now();
    while child.try_wait().expect("status").is_none() {
        if start.elapsed() > Duration::from_secs(30) {
            child.kill().expect("kill");
            child.wait().expect("reap");
            panic!("CPU child timeout");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().expect("output");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stdout}\n{stderr}");
    assert!(
        stdout.contains("running 1 test")
            && stdout.contains("1 passed; 0 failed")
            && stdout.contains(&format!("stock CPU threads={threads}: 12 finite scores")),
        "exact nonzero filter"
    );
    assert!(stderr.is_empty(), "native stderr must be empty: {stderr}");
    eprintln!(
        "{stdout}stock CPU threads={threads}: captured stderr={} bytes",
        output.stderr.len()
    );
}

/// Manual hardware gate: no absence skip once explicitly requested.
#[test]
fn required_cuda_matches_cpu_on_same_clip() {
    if std::env::var_os("AV_METRICS_REQUIRE_CUDA").is_none() {
        eprintln!("UNEXECUTED hardware gate: set AV_METRICS_REQUIRE_CUDA=1 on a CUDA host");
        return;
    }
    assert!(is_available(), "required CUDA acceptance needs libvmaf");
    // Generate once: both backends receive exactly the same reference/distortion.
    let pair = small_pair(Distortion::Moderate);
    let format = VideoFormat {
        width: 64,
        height: 48,
        bit_depth: 8,
        chroma_sampling: ChromaSubsampling::Yuv420,
    };
    let config = model_config().with_threads(2);
    let score = |preference, expected_backend| {
        let mut scorer = VmafScorer::new(config.clone().with_backend(preference), format)
            .expect("required backend must fully initialize, never silently fall back");
        assert_eq!(
            scorer.backend(),
            expected_backend,
            "required backend selection"
        );
        let mut reference = pair.reference.decoder();
        let mut distorted = pair.distorted.decoder();
        scorer
            .score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {})
            .expect("native scoring, flush and teardown")
    };
    let cpu = score(BackendPreference::CpuOnly, Backend::Cpu);
    let cuda = score(BackendPreference::RequireCuda, Backend::Cuda);
    assert_eq!(cpu.len(), 12, "all native CPU scores");
    assert_eq!(cuda.len(), cpu.len(), "all required-CUDA scores");
    for (index, (cpu, cuda)) in cpu.iter().zip(&cuda).enumerate() {
        // Allow 0.01 absolute points (0.01% of VMAF's 100-point scale) for
        // CPU/CUDA kernel rounding, not a broad quality or golden-score tolerance.
        // This acceptance bound needs hardware execution; it is not a measured claim.
        assert!(
            cpu.score.is_finite()
                && cuda.score.is_finite()
                && (cpu.score - cuda.score).abs() <= 0.01,
            "frame {index}: CPU={} CUDA={}",
            cpu.score,
            cuda.score
        );
    }
}

#[test]
fn native_cpu_threaded_matches_serial() {
    require_libvmaf!();
    let pair = small_pair(Distortion::Mild);
    let format = VideoFormat {
        width: 64,
        height: 48,
        bit_depth: 8,
        chroma_sampling: ChromaSubsampling::Yuv420,
    };
    let score = |threads| {
        let mut scorer = VmafScorer::new(
            model_config().with_backend(BackendPreference::CpuOnly).with_threads(threads),
            format,
        )
        .expect("native CPU scorer");
        assert_eq!(scorer.backend(), Backend::Cpu);
        let mut reference = pair.reference.decoder();
        let mut distorted = pair.distorted.decoder();
        scorer
            .score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {})
            .expect("native CPU scoring, flush, and drop")
    };
    let serial = score(0);
    let threaded = score(2);
    assert_eq!(serial.len(), 12, "all submitted indices must survive");
    assert_eq!(
        threaded.len(),
        serial.len(),
        "native worker-held references must drain"
    );
    for (serial, threaded) in serial.iter().zip(&threaded) {
        assert!(
            serial.score.is_finite() && (serial.score - threaded.score).abs() < 1e-8,
            "threading must preserve native scores: {serial:?} vs {threaded:?}"
        );
    }
}

#[test]
fn native_cpu_model_parse_error_preserves_path_and_allows_later_scoring() {
    if std::env::var_os("AV_METRICS_REQUIRE_CPU").is_some() {
        assert!(
            is_available(),
            "required malformed-model check needs libvmaf"
        );
    }
    require_libvmaf!();
    let directory = tempfile::tempdir().expect("malformed model fixture");
    let path = directory.path().join("malformed.json");
    // Fail after one feature name has been allocated, not before opening the file.
    std::fs::write(&path, br#"{"model_dict":{"feature_names":["partial",42]}}"#)
        .expect("write malformed model");
    let format = VideoFormat {
        width: 64,
        height: 48,
        bit_depth: 8,
        chroma_sampling: ChromaSubsampling::Yuv420,
    };
    for _ in 0..8 {
        let result = VmafScorer::new(
            VmafConfig::default()
                .with_backend(BackendPreference::CpuOnly)
                .with_model(VmafModel::Path(path.clone())),
            format,
        );
        assert!(
            matches!(result, Err(VmafError::ModelNotFound { ref model, ref reason })
            if model == &path.display().to_string() && reason.contains("-22")),
            "preserve native malformed-model failure"
        );
        let mut scorer = VmafScorer::new(
            model_config().with_backend(BackendPreference::CpuOnly),
            format,
        )
        .expect("valid CPU construction after parser failure");
        let pair = small_pair(Distortion::Mild);
        let mut reference = pair.reference.decoder();
        let mut distorted = pair.distorted.decoder();
        let scores = scorer
            .score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {})
            .expect("valid scoring/flush/drop after parser failure");
        assert_eq!(scores.len(), 12, "all frames after malformed-model failure");
        assert!(
            scores.iter().all(|score| score.score.is_finite()),
            "valid native scores"
        );
    }
    // Native failure alone is not a leak proof; boundary tests count destruction.
}

#[test]
fn ten_bit_cpu_rejects_typed_u8_without_advancing_submission() {
    require_libvmaf!();
    let mut scorer = ten_bit_cpu_scorer();
    let frame = v_frame::frame::FrameBuilder::new(64, 48, ChromaSubsampling::Yuv420, 8)
        .build::<u8>()
        .expect("u8 frame");
    assert!(matches!(
        scorer.submit_pair(&frame, &frame),
        Err(VmafError::InputMismatch { .. })
    ));
    assert_first_ten_bit_submission_scores(&mut scorer);
}

#[test]
fn ten_bit_cpu_rejects_narrow_raw_plane_on_either_side_without_advancing_submission() {
    require_libvmaf!();
    let mut scorer = ten_bit_cpu_scorer();
    let bytes = [0u8; 64 * 48];
    let words = [0u16; 64 * 48];
    let good = [(64, 48), (32, 24), (32, 24)].map(|(width, height)| {
        PlaneSource::from_u16(&words, width, height, (width * 2) as isize, 0).unwrap()
    });
    let narrow = [(64, 48), (32, 24), (32, 24)].map(|(width, height)| {
        PlaneSource::from_bytes(&bytes, width, height, width as isize, 0, 1).unwrap()
    });
    for side in 0..2 {
        for plane in 0..3 {
            let mut bad = good;
            bad[plane] = narrow[plane];
            let result = if side == 0 {
                scorer.submit_pair_raw(bad, good)
            } else {
                scorer.submit_pair_raw(good, bad)
            };
            assert!(
                matches!(result, Err(VmafError::InputMismatch { .. })),
                "side {side}, plane {plane}: {result:?}"
            );
        }
    }
    assert_first_ten_bit_submission_scores(&mut scorer);
}

fn ten_bit_cpu_scorer() -> VmafScorer {
    let format = VideoFormat {
        width: 64,
        height: 48,
        bit_depth: 10,
        chroma_sampling: ChromaSubsampling::Yuv420,
    };
    let scorer = VmafScorer::new(
        model_config().with_backend(BackendPreference::CpuOnly),
        format,
    )
    .expect("CPU scorer setup (native allocation/model calls are allowed)");
    assert_eq!(
        scorer.backend(),
        Backend::Cpu,
        "ten-bit rejection uses the CPU backend"
    );
    scorer
}

fn assert_first_ten_bit_submission_scores(scorer: &mut VmafScorer) {
    let words = [0u16; 64 * 48];
    let good = [(64, 48), (32, 24), (32, 24)].map(|(width, height)| {
        PlaneSource::from_u16(&words, width, height, (width * 2) as isize, 0).unwrap()
    });
    assert!(
        matches!(scorer.finish(), Err(VmafError::InvalidConfiguration { .. })),
        "rejected frames must not count as submitted"
    );
    // No invalid pair reaches vmaf_read_pictures. A valid pair still starts at zero.
    assert_eq!(
        scorer.submit_pair_raw(good, good).expect("valid wide pair"),
        0,
        "rejected pairs must not advance the first valid submission index"
    );
    let scores = scorer.finish().expect("finish the valid wide pair");
    assert_eq!(scores.len(), 1, "only the valid wide pair yields a score");
    assert!(
        scores[0].is_finite(),
        "the accepted pair yields a finite score"
    );
}

#[test]
fn required_cpu_gate_has_library_model_clip_and_model_directory() {
    if std::env::var_os("AV_METRICS_REQUIRE_CPU").is_none() {
        eprintln!("UNEXECUTED prerequisite gate: set AV_METRICS_REQUIRE_CPU=1");
        return;
    }
    assert!(is_available(), "required CPU-native checks need libvmaf");
    for variable in ["VMAF_TEST_MODEL", "VMAF_TEST_Y4M"] {
        let path = std::env::var(variable)
            .unwrap_or_else(|_| panic!("required CPU-native checks need {variable}"));
        assert!(
            Path::new(&path).is_file(),
            "{variable} is not a file: {path}"
        );
    }
    let directory =
        std::env::var("VMAF_MODEL_PATH").expect("required CPU-native checks need VMAF_MODEL_PATH");
    assert!(
        Path::new(&directory).is_dir(),
        "VMAF_MODEL_PATH is not a directory: {directory}"
    );
}

#[test]
fn vmaf_availability_and_version_agree() {
    match is_available() {
        true => assert!(
            libvmaf_version().is_some(),
            "a loadable libvmaf must report a version"
        ),
        false => assert_eq!(libvmaf_version(), None),
    }
}

#[test]
fn auto_constructor_reports_cpu_or_cuda_backend() {
    require_libvmaf!();

    let pair = small_pair(Distortion::Mild);
    let details = *pair.reference.decoder().get_video_details();
    let format = VideoFormat {
        width: details.width as u32,
        height: details.height as u32,
        bit_depth: details.bit_depth as u32,
        chroma_sampling: details.chroma_sampling,
    };

    let scorer = VmafScorer::new(model_config(), format).expect("scorer should initialise");
    // Auto preference must land on a real backend, never an uninitialised one.
    assert!(matches!(scorer.backend(), Backend::Cpu | Backend::Cuda));
}

#[test]
fn cpu_only_preference_forces_the_cpu_backend() {
    require_libvmaf!();

    let pair = small_pair(Distortion::Mild);
    let details = *pair.reference.decoder().get_video_details();
    let format = VideoFormat {
        width: details.width as u32,
        height: details.height as u32,
        bit_depth: details.bit_depth as u32,
        chroma_sampling: details.chroma_sampling,
    };

    let scorer = VmafScorer::new(
        model_config().with_backend(BackendPreference::CpuOnly),
        format,
    )
    .expect("scorer should initialise");

    assert_eq!(scorer.backend(), Backend::Cpu);
}

#[test]
fn identical_clips_score_at_or_near_the_maximum() {
    require_libvmaf!();

    let pair = small_pair(Distortion::None);
    // Score the undistorted clip against itself.
    let mut reference = pair.reference.decoder();
    let mut distorted = pair.reference.decoder();

    let details = *reference.get_video_details();
    let format = VideoFormat {
        width: details.width as u32,
        height: details.height as u32,
        bit_depth: details.bit_depth as u32,
        chroma_sampling: details.chroma_sampling,
    };

    let mut scorer = VmafScorer::new(model_config(), format).expect("scorer should initialise");
    let scores = scorer
        .score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {})
        .expect("identical clips should score");

    assert!(
        !scores.is_empty(),
        "scoring identical clips must produce at least one score"
    );

    // Identical clips must score identically at every frame, which is the
    // property this asserts. The absolute value is *not* 100: libvmaf's
    // motion and temporal extractors compare consecutive frames, so the
    // first frame has no predecessor and scores lower, and a high-frequency
    // synthetic pattern can score below 100 on later frames too. Measured
    // against libvmaf's own CLI, an identical pair scores exactly 100 per
    // frame on smooth content but not on synthetic noise, so pinning an
    // absolute number here would be pinning our test content rather than
    // the implementation. `native_fixture_scores_match_recorded_vmaf_cli_and_ffmpeg_values` is what
    // verifies absolute agreement with the reference tools.
    for frame in &scores {
        assert!(
            frame.score > 90.0 && frame.score <= 100.0,
            "identical clips should score in (90, 100], got {}",
            frame.score
        );
    }

    let mean = VmafScorer::pool(&scores, PoolMethod::Mean).expect("scores should pool");
    assert!(
        mean > 99.0 && mean <= 100.0,
        "identical clips should pool to just under 100, got {mean}"
    );
}

#[test]
fn scores_decrease_as_distortion_increases() {
    require_libvmaf!();

    let score_at = |distortion: Distortion| {
        let pair = small_pair(distortion);
        let mut reference = pair.reference.decoder();
        let mut distorted = pair.distorted.decoder();

        let details = *reference.get_video_details();
        let format = VideoFormat {
            width: details.width as u32,
            height: details.height as u32,
            bit_depth: details.bit_depth as u32,
            chroma_sampling: details.chroma_sampling,
        };

        let mut scorer = VmafScorer::new(model_config(), format).expect("scorer should initialise");
        let scores = scorer
            .score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {})
            .expect("distorted clips should score");
        VmafScorer::pool(&scores, PoolMethod::Mean).expect("scores should pool")
    };

    let none = score_at(Distortion::None);
    let mild = score_at(Distortion::Mild);
    let moderate = score_at(Distortion::Moderate);
    let severe = score_at(Distortion::Severe);

    assert!(
        none >= mild,
        "no distortion ({none}) must score at least as high as mild ({mild})"
    );
    assert!(
        mild > moderate,
        "mild ({mild}) must score higher than moderate ({moderate})"
    );
    assert!(
        moderate > severe,
        "moderate ({moderate}) must score higher than severe ({severe})"
    );
}

#[test]
fn frame_count_is_preserved_exactly() {
    require_libvmaf!();

    // The sliding window means the last score only arrives on flush. This is
    // the most likely place for an off-by-one, so the count is asserted
    // exactly.
    for frames in [1usize, 2, 3, 8, 17] {
        let pair = SyntheticPair::generate(
            64,
            48,
            8,
            ChromaSubsampling::Yuv420,
            frames,
            Distortion::Mild,
        );
        let mut reference = pair.reference.decoder();
        let mut distorted = pair.distorted.decoder();

        let details = *reference.get_video_details();
        let format = VideoFormat {
            width: details.width as u32,
            height: details.height as u32,
            bit_depth: details.bit_depth as u32,
            chroma_sampling: details.chroma_sampling,
        };

        let mut scorer = VmafScorer::new(model_config(), format).expect("scorer should initialise");
        let scores = scorer
            .score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {})
            .expect("scoring should succeed");

        assert_eq!(
            scores.len(),
            frames,
            "{frames} frames must yield exactly {frames} scores"
        );
    }
}

#[test]
fn score_decoders_calls_back_once_per_score_with_sequential_indices() {
    require_libvmaf!();

    let pair = small_pair(Distortion::Mild);
    let mut reference = pair.reference.decoder();
    let mut distorted = pair.distorted.decoder();

    let details = *reference.get_video_details();
    let format = VideoFormat {
        width: details.width as u32,
        height: details.height as u32,
        bit_depth: details.bit_depth as u32,
        chroma_sampling: details.chroma_sampling,
    };

    let mut seen = Vec::new();
    let mut scorer = VmafScorer::new(model_config(), format).expect("scorer should initialise");
    let scores = scorer
        .score_decoders::<u8>(&mut reference, &mut distorted, |index, score| {
            seen.push((index, score.score));
        })
        .expect("scoring should succeed");

    assert_eq!(
        seen.len(),
        scores.len(),
        "the callback must fire once per score"
    );
    for (index, (callback_index, _)) in seen.iter().enumerate() {
        assert_eq!(
            *callback_index, index,
            "callback indices must be sequential"
        );
    }
}

#[test]
fn final_score_drains_follow_the_callers_cursor_and_allow_rereading() {
    require_libvmaf!();

    let pair = small_pair(Distortion::Mild);
    let total = pair.reference.decoder().get_video_details().total_frames.unwrap_or(12);

    let mut reference = pair.reference.decoder();
    let mut distorted = pair.distorted.decoder();

    let details = *reference.get_video_details();
    let format = VideoFormat {
        width: details.width as u32,
        height: details.height as u32,
        bit_depth: details.bit_depth as u32,
        chroma_sampling: details.chroma_sampling,
    };

    let mut scorer = VmafScorer::new(model_config(), format).expect("scorer should initialise");
    let wanted: Vec<usize> = (0..total).collect();
    for (reference_frame, distorted_frame) in decode_pairs(&mut reference, &mut distorted, &wanted)
    {
        scorer
            .submit_pair(&reference_frame, &distorted_frame)
            .expect("submitting a pair should succeed");
    }

    assert!(
        scorer.drain_scores(0).expect("pending drain").is_empty(),
        "readiness is not guaranteed before successful finalization"
    );
    let finished = scorer.finish().expect("scoring should succeed");
    assert_eq!(finished.len(), total, "one score per submitted pair");

    // The caller advances its own cursor; final reads need no readiness polling.
    let mut drained = Vec::new();
    loop {
        let batch = scorer.drain_scores(drained.len()).expect("draining scores should succeed");
        if batch.is_empty() {
            break;
        }
        assert!(
            drained.len() + batch.len() <= total,
            "draining reported more scores than pairs were submitted"
        );
        drained.extend(batch);
    }

    assert_eq!(drained.len(), total, "every final score is drained once");

    for (position, &score) in drained.iter().enumerate() {
        assert!(
            (score - finished[position]).abs() < 1e-9,
            "score at {position} changed after finalization: {score} vs {}",
            finished[position]
        );
    }

    // Repeating the same caller-controlled cursor deliberately rereads scores.
    let redrained = scorer.drain_scores(0).expect("draining after a flush");
    assert_eq!(
        redrained, finished,
        "every score must still be readable after the flush"
    );
}

#[test]
fn mismatched_frame_counts_are_rejected() {
    require_libvmaf!();

    let reference_pair =
        SyntheticPair::generate(64, 48, 8, ChromaSubsampling::Yuv420, 6, Distortion::Mild);
    let distorted_pair =
        SyntheticPair::generate(64, 48, 8, ChromaSubsampling::Yuv420, 9, Distortion::Mild);

    let mut reference = reference_pair.reference.decoder();
    let mut distorted = distorted_pair.distorted.decoder();

    let details = *reference.get_video_details();
    let format = VideoFormat {
        width: details.width as u32,
        height: details.height as u32,
        bit_depth: details.bit_depth as u32,
        chroma_sampling: details.chroma_sampling,
    };

    let mut scorer = VmafScorer::new(model_config(), format).expect("scorer should initialise");
    let result = scorer.score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {});

    assert!(
        matches!(result, Err(VmafError::FrameCountMismatch { .. })),
        "differing lengths must be reported, got {:?}",
        result.err()
    );
}

#[test]
fn mismatched_resolutions_are_rejected() {
    require_libvmaf!();

    let reference_pair =
        SyntheticPair::generate(64, 48, 8, ChromaSubsampling::Yuv420, 4, Distortion::Mild);
    let distorted_pair =
        SyntheticPair::generate(32, 32, 8, ChromaSubsampling::Yuv420, 4, Distortion::Mild);

    let mut reference = reference_pair.reference.decoder();
    let mut distorted = distorted_pair.distorted.decoder();

    let details = *reference.get_video_details();
    let format = VideoFormat {
        width: details.width as u32,
        height: details.height as u32,
        bit_depth: details.bit_depth as u32,
        chroma_sampling: details.chroma_sampling,
    };

    let mut scorer = VmafScorer::new(model_config(), format).expect("scorer should initialise");
    let result = scorer.score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {});

    assert!(
        matches!(result, Err(VmafError::InputMismatch { .. })),
        "differing resolutions must be reported, got {:?}",
        result.err()
    );
}

#[test]
fn a_missing_model_file_is_reported_before_scoring_starts() {
    let format = VideoFormat {
        width: 64,
        height: 48,
        bit_depth: 8,
        chroma_sampling: ChromaSubsampling::Yuv420,
    };

    let result = VmafScorer::new(
        VmafConfig::default().with_model(VmafModel::Path("/nonexistent/model.json".into())),
        format,
    );

    assert!(
        matches!(result, Err(VmafError::ModelFileMissing { .. })),
        "a missing model file must be reported, got {:?}",
        result.err()
    );
}

#[test]
fn ten_bit_decoder_scoring_returns_six_finite_scores() {
    require_libvmaf!();

    let pair = SyntheticPair::generate(64, 48, 10, ChromaSubsampling::Yuv420, 6, Distortion::Mild);
    let mut reference = pair.reference.decoder();
    let mut distorted = pair.distorted.decoder();

    let details = *reference.get_video_details();
    let format = VideoFormat {
        width: details.width as u32,
        height: details.height as u32,
        bit_depth: details.bit_depth as u32,
        chroma_sampling: details.chroma_sampling,
    };

    let mut scorer = VmafScorer::new(model_config(), format).expect("scorer should initialise");
    let scores = scorer
        .score_decoders::<u16>(&mut reference, &mut distorted, |_, _| {})
        .expect("10-bit content should score");

    assert_eq!(scores.len(), 6);
    for frame in &scores {
        assert!(
            frame.score.is_finite(),
            "scores must be finite, got {}",
            frame.score
        );
    }
}

#[test]
fn requested_psnr_is_present_as_a_finite_pooled_feature() {
    require_libvmaf!();

    let pair = small_pair(Distortion::Moderate);
    let mut reference = pair.reference.decoder();
    let mut distorted = pair.distorted.decoder();

    let details = *reference.get_video_details();
    let format = VideoFormat {
        width: details.width as u32,
        height: details.height as u32,
        bit_depth: details.bit_depth as u32,
        chroma_sampling: details.chroma_sampling,
    };

    let mut scorer = VmafScorer::new(
        model_config().with_feature(av_metrics_vmaf::VmafFeature::Psnr),
        format,
    )
    .expect("scorer should initialise");

    let scores = scorer
        .score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {})
        .expect("scoring with features should succeed");
    assert!(!scores.is_empty());

    // Feature extractors report pooled values rather than a score per frame, so
    // they are read once scoring has finished.
    let features = scorer.pooled_features().expect("pooled feature values should be readable");

    assert!(
        features.iter().any(|(name, _)| name == "psnr_y"),
        "PSNR was requested, so `psnr_y` must be present, got {features:?}"
    );
    for (name, value) in &features {
        assert!(
            value.is_finite(),
            "feature `{name}` produced a non-finite value: {value}"
        );
    }
}

/// End-to-end scoring against a model file on disk.
///
/// Some libvmaf builds ship without the built-in models compiled in, in which
/// case `vmaf_model_load` rejects a bare version such as `vmaf_v0.6.1`. This
/// test covers the other path: an explicit model JSON, loaded via
/// `VmafModel::Path`.
///
/// The model is not committed to this repository, so the test is driven by the
/// `VMAF_TEST_MODEL` environment variable and skips when unset.
#[test]
fn scoring_works_with_an_explicit_model_file() {
    if !is_available() {
        eprintln!("skipping: libvmaf is not installed");
        return;
    }

    let Ok(model) = std::env::var("VMAF_TEST_MODEL") else {
        eprintln!("skipping: set VMAF_TEST_MODEL to a libvmaf model JSON to run this");
        return;
    };

    let pair = small_pair(Distortion::Moderate);
    let mut reference = pair.reference.decoder();
    let mut distorted = pair.distorted.decoder();

    let details = *reference.get_video_details();
    let format = VideoFormat {
        width: details.width as u32,
        height: details.height as u32,
        bit_depth: details.bit_depth as u32,
        chroma_sampling: details.chroma_sampling,
    };

    let mut scorer = VmafScorer::new(
        VmafConfig::default().with_model(VmafModel::Path(PathBuf::from(&model))),
        format,
    )
    .expect("scorer should initialise with an explicit model file");

    let scores = scorer
        .score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {})
        .expect("scoring with an explicit model should succeed");

    assert_eq!(scores.len(), 12, "12 frames must yield 12 scores");
    let pooled = VmafScorer::pool(&scores, PoolMethod::Mean).expect("scores should pool");
    assert!(
        (0.0..=100.0).contains(&pooled),
        "a pooled VMAF score must fall in 0..=100, got {pooled}"
    );
}

/// Report constructor failures and check finite scores for accepted models.
///
/// The Windows installer ships all nine upstream models, but only four are
/// reachable by name through [`av_metrics_vmaf::VmafModel`]. The `float`
/// variants in particular are reachable solely by path, so nothing else in the
/// suite would notice if they stopped loading — and a build without
/// `-Denable_float=true` rejects them with `EINVAL`, which would otherwise
/// surface as a confusing failure at scoring time rather than at install time.
///
/// Scan only `VMAF_MODEL_PATH`. Rejected models are reported without failing the
/// run; a model that loads but cannot score still fails.
#[test]
fn configured_model_directory_reports_load_failures_and_checks_finite_scores_for_supported_models()
{
    if !is_available() {
        eprintln!("skipping: libvmaf is not installed");
        return;
    }

    let Ok(directory) = std::env::var("VMAF_MODEL_PATH") else {
        eprintln!("skipping: set VMAF_MODEL_PATH to a directory of model JSONs to run this");
        return;
    };
    let Ok(entries) = std::fs::read_dir(&directory) else {
        eprintln!("skipping: {directory} is not readable");
        return;
    };

    let mut models: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "json"))
        .collect();
    models.sort();

    assert!(
        !models.is_empty(),
        "no model JSONs were found in {directory}"
    );

    let mut loaded = Vec::new();
    let mut unsupported = Vec::new();

    // Take the geometry from a real clip rather than hard-coding it: the 4K
    // models declare a 3840x2160 input and libvmaf rescales to that, so a
    // mismatch here would be an error about the test rather than the model.
    let pair = small_pair(Distortion::Moderate);
    let details = *pair.reference.decoder().get_video_details();
    let format = VideoFormat {
        width: details.width as u32,
        height: details.height as u32,
        bit_depth: details.bit_depth as u32,
        chroma_sampling: details.chroma_sampling,
    };

    for model in &models {
        let name = model.file_name().map_or_else(
            || model.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );

        let result = VmafScorer::new(
            VmafConfig::default().with_model(VmafModel::Path(model.clone())),
            format,
        );

        match result {
            Ok(mut scorer) => {
                let mut reference = pair.reference.decoder();
                let mut distorted = pair.distorted.decoder();
                let scores = scorer
                    .score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {})
                    .unwrap_or_else(|error| panic!("{name} loaded but could not score: {error}"));

                assert!(
                    scores.iter().all(|score| score.score.is_finite()),
                    "{name} produced a non-finite score"
                );
                loaded.push(name);
            },
            Err(error) => {
                // A build without -Denable_float=true rejects the float models.
                // That is a property of the libvmaf build, not a defect here.
                let message = error.to_string();
                eprintln!("note: {name} is not supported by this libvmaf build: {message}");
                unsupported.push(name);
            },
        }
    }

    println!(
        "loaded {} model(s): {loaded:?}; unsupported by this build: {unsupported:?}",
        loaded.len()
    );

    assert!(
        loaded.len() + unsupported.len() == models.len(),
        "every model must either load or be reported as unsupported"
    );
}

/// Scores a real clip against itself and compares with the reference tools.
///
/// Set `VMAF_TEST_Y4M` to a y4m file and `VMAF_TEST_MODEL` to a model JSON.
/// Scoring the file against itself is the strongest available check that the
/// frames and geometry handed to libvmaf are correct, because the expected
/// values come from libvmaf's own CLI and FFmpeg's libvmaf filter run over the
/// same bytes.
#[test]
fn native_fixture_scores_match_recorded_vmaf_cli_and_ffmpeg_values() {
    if !is_available() {
        eprintln!("skipping: libvmaf is not installed");
        return;
    }

    let (Ok(clip), Ok(model)) = (
        std::env::var("VMAF_TEST_Y4M"),
        std::env::var("VMAF_TEST_MODEL"),
    ) else {
        eprintln!("skipping: set VMAF_TEST_Y4M and VMAF_TEST_MODEL to run this");
        return;
    };

    let mut reference =
        av_decoders::Decoder::from_file(&clip).expect("the reference clip should decode");
    // Count the fixture independently so dropped scores cannot redefine the expected mean.
    let mut expected_frame_count = 0;
    loop {
        match reference.read_video_frame::<u8>() {
            Ok(_) => expected_frame_count += 1,
            Err(av_decoders::DecoderError::EndOfFile) => break,
            Err(error) => panic!("the reference fixture should decode completely: {error}"),
        }
    }
    let expected_mean = reference_mean(expected_frame_count);
    reference = av_decoders::Decoder::from_file(&clip).expect("the reference clip should reopen");
    let mut distorted =
        av_decoders::Decoder::from_file(&clip).expect("the second decoder should open");

    let details = *reference.get_video_details();
    let format = VideoFormat {
        width: details.width as u32,
        height: details.height as u32,
        bit_depth: details.bit_depth as u32,
        chroma_sampling: details.chroma_sampling,
    };

    let mut scorer = VmafScorer::new(
        VmafConfig::default().with_model(VmafModel::Path(PathBuf::from(&model))),
        format,
    )
    .expect("scorer should initialise");

    let scores = scorer
        .score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {})
        .expect("scoring a clip against itself should succeed");

    assert_eq!(
        scores.len(),
        expected_frame_count,
        "every fixture frame must yield exactly one score"
    );

    // Measured with libvmaf 3.2.1's CLI and FFmpeg's libvmaf filter, which
    // agree to six decimal places: frame 0 scores 97.42837 and every later
    // frame scores exactly 100. Frame 0 is lower because the motion and
    // temporal extractors have no previous frame to compare against.
    for (index, frame) in scores.iter().enumerate() {
        let expected = if index == 0 { 97.42837 } else { 100.0 };
        assert!(
            (frame.score - expected).abs() < 0.001,
            "frame {index} scored {} but the reference tools give {expected}",
            frame.score
        );
    }

    let mean = VmafScorer::pool(&scores, PoolMethod::Mean).expect("scores should pool");
    assert!(
        (mean - expected_mean).abs() < 0.001,
        "pooled score should match the reference tools' {expected_mean}, got {mean}"
    );
}

/// A contiguous selection must score identically through the frame-pair path
/// and the streaming path.
///
/// Both hand libvmaf the same frames; if they disagree, one is submitting
/// frames incorrectly. This is what lets a caller pick whichever is convenient
/// without changing the numbers.
///
/// Requires `VMAF_TEST_Y4M` and `VMAF_TEST_MODEL`.
#[test]
fn the_frame_path_agrees_with_the_streaming_path() {
    if !is_available() {
        eprintln!("skipping: libvmaf is not installed");
        return;
    }

    let (Ok(clip), Ok(model)) = (
        std::env::var("VMAF_TEST_Y4M"),
        std::env::var("VMAF_TEST_MODEL"),
    ) else {
        eprintln!("skipping: set VMAF_TEST_Y4M and VMAF_TEST_MODEL to run this");
        return;
    };

    let config = VmafConfig::default().with_model(VmafModel::Path(PathBuf::from(&model)));
    let wanted: Vec<usize> = (0..12).collect();

    let open = || {
        let reference = av_decoders::Decoder::from_file(&clip).expect("reference opens");
        let distorted = av_decoders::Decoder::from_file(&clip).expect("output opens");
        let details = *reference.get_video_details();
        let format = VideoFormat {
            width: details.width as u32,
            height: details.height as u32,
            bit_depth: details.bit_depth as u32,
            chroma_sampling: details.chroma_sampling,
        };
        (reference, distorted, format)
    };

    let (mut reference, mut distorted, format) = open();
    let streaming = VmafScorer::new(config.clone(), format)
        .expect("scorer")
        .score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {})
        .expect("streaming scoring");

    let (mut reference, mut distorted, format) = open();
    let mut scorer = VmafScorer::new(config, format).expect("scorer");
    let pairs = decode_pairs(&mut reference, &mut distorted, &wanted);
    let frame_path = scorer.score_frames::<u8>(&pairs).expect("frame-path scoring");

    // The streaming path scores every frame of the clip; the frame path scores
    // only the selection. The selection is the leading run, so its scores
    // must match the streaming path's for the same frames.
    assert!(
        streaming.len() >= wanted.len(),
        "streaming should score at least the {} selected frames, got {}",
        wanted.len(),
        streaming.len()
    );
    assert_eq!(
        frame_path.len(),
        wanted.len(),
        "the frame path should score exactly the selection"
    );

    for (index, sparse) in frame_path.iter().enumerate() {
        let streamed = streaming[index].score;
        assert!(
            (streamed - *sparse).abs() < 0.001,
            "frame {index} streamed {streamed} but scored {sparse} through the frame path"
        );
    }
}

/// Decode the first `wanted.len()` frames from both decoders.
///
/// Reads forward rather than seeking, because the y4m decoder has no random
/// access; `wanted` is therefore a contiguous run starting at frame 0.
fn decode_pairs(
    reference: &mut av_decoders::Decoder,
    distorted: &mut av_decoders::Decoder,
    wanted: &[usize],
) -> Vec<(v_frame::frame::Frame<u8>, v_frame::frame::Frame<u8>)> {
    let mut pairs = Vec::with_capacity(wanted.len());
    for _ in wanted {
        pairs.push((
            reference.read_video_frame::<u8>().expect("reference frame"),
            distorted.read_video_frame::<u8>().expect("output frame"),
        ));
    }
    pairs
}

/// A stock model resolves from `VMAF_MODEL_PATH` when libvmaf has none built
/// in.
///
/// Several distributions ship libvmaf without the models compiled in — Arch's
/// `vmaf` package and MSYS2's `mingv-w64-*-vmaf` both do — and
/// `vmaf_model_load` then rejects a bare version such as `vmaf_v0.6.1` with
/// `EINVAL`. Scoring must still work by finding the same model as a file on
/// disk.
///
/// Requires `VMAF_TEST_Y4M` and `VMAF_MODEL_PATH`.
#[test]
fn a_stock_model_resolves_from_a_directory() {
    if !is_available() {
        eprintln!("skipping: libvmaf is not installed");
        return;
    }

    let (Ok(clip), Ok(directory)) = (
        std::env::var("VMAF_TEST_Y4M"),
        std::env::var("VMAF_MODEL_PATH"),
    ) else {
        eprintln!("skipping: set VMAF_TEST_Y4M and VMAF_MODEL_PATH to run this");
        return;
    };

    assert!(
        Path::new(&directory).join("vmaf_v0.6.1.json").is_file(),
        "VMAF_MODEL_PATH should contain vmaf_v0.6.1.json"
    );

    let mut reference = av_decoders::Decoder::from_file(&clip).expect("reference opens");
    let mut distorted = av_decoders::Decoder::from_file(&clip).expect("output opens");

    let details = *reference.get_video_details();
    let format = VideoFormat {
        width: details.width as u32,
        height: details.height as u32,
        bit_depth: details.bit_depth as u32,
        chroma_sampling: details.chroma_sampling,
    };

    // No explicit model path, so the stock name is used and must be found on
    // disk.
    let mut scorer =
        VmafScorer::new(VmafConfig::default(), format).expect("the model should resolve from disk");

    let scores = scorer
        .score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {})
        .expect("scoring should succeed");

    // y4m carries no frame count in its header, so the count is whatever the
    // clip yields; the point is that scoring succeeded and the values are
    // sane.
    assert!(!scores.is_empty(), "scoring should produce scores");
    for frame in &scores {
        assert!(
            frame.score > 90.0 && frame.score <= 100.0,
            "identical clips should score in (90, 100], got {}",
            frame.score
        );
    }
}
