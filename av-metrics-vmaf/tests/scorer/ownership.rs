//! Production constructor/submit/drop ownership at the C boundary. The double
//! models CPU and CUDA-compiled CPU semantics, not real CUDA or native workers.
#![cfg(unix)]

use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use av_metrics_vmaf::{
    BackendPreference, PlaneSource, VideoFormat, VmafConfig, VmafError, VmafScorer,
};
use v_frame::{chroma::ChromaSubsampling, frame::FrameBuilder};

const FETCH: u32 = 0;
const CHECKOUT: u32 = 1;
const READ: u32 = 2;
const TRANSFER: u32 = 3;
const RUST_RETURN: u32 = 4;
const WORKER_REF: u32 = 5;
const WORKER_RELEASE: u32 = 6;
const NATIVE_RELEASE: u32 = 7;
const DUPLICATE: u32 = 8;
const ZERO_UNREF: u32 = 9;
const CLOSE_OWNED: u32 = 10;
const CLOSE: u32 = 11;
const CLOSE_REFS: u32 = 12;
const BAD_INDEX: u32 = 13;
const RUST_UNREF: u32 = 15;
const ALLOC: u32 = 16;
const FREE: u32 = 17;

#[test]
fn finalization_boundary() {
    if let Ok(scenario) = std::env::var("VMAF_FINALIZATION_FIXTURE_CHILD") {
        check_finalization(&scenario);
        return;
    }
    let directory = tempfile::tempdir().expect("finalization fixture");
    let library = directory.path().join(if cfg!(target_os = "macos") {
        "libvmaf.dylib"
    } else {
        "libvmaf.so"
    });
    let output = Command::new("cc")
        .args(["-shared", "-fPIC", "-std=c99", "-Wall", "-Wextra", "-Werror", "-o"])
        .arg(&library)
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/ownership.c"
        ))
        .output()
        .expect("finalization fixture requires cc");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut failed = Vec::new();
    for scenario in ["early", "repeat", "query_error", "flush_error", "postflush", "logging"] {
        let mut child = Command::new(std::env::current_exe().expect("test executable"))
            .args(["--exact", "scorer::ownership::finalization_boundary", "--nocapture"])
            .env("VMAF_FINALIZATION_FIXTURE_CHILD", scenario)
            .env(
                "VMAF_FINALIZATION_FIXTURE_MODE",
                if scenario == "query_error" { "2" } else { "1" },
            )
            .env(
                "VMAF_OWNERSHIP_FIXTURE_FAIL_FLUSH",
                if scenario == "flush_error" { "1" } else { "0" },
            )
            .env("VMAF_OWNERSHIP_FIXTURE_THREADS", "2")
            .env("VMAF_OWNERSHIP_FIXTURE_NONZERO", "1")
            .env("VMAF_FINALIZATION_FIXTURE_LIBRARY", &library)
            .env("VMAF_LIB_DIR", directory.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("finalization child");
        let start = Instant::now();
        while child.try_wait().expect("status").is_none() {
            if start.elapsed() > Duration::from_secs(30) {
                child.kill().expect("kill");
                child.wait().expect("reap");
                panic!("finalization {scenario} timeout");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = child.wait_with_output().expect("output");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("finalization {scenario}: {stdout}\n{stderr}");
        assert!(stdout.contains("running 1 test"), "nonzero exact filter");
        if !output.status.success() {
            failed.push(scenario);
        }
    }
    assert!(
        failed.is_empty(),
        "failed finalization scenarios: {failed:?}"
    );
}

fn check_finalization(scenario: &str) {
    // SAFETY: fixture stays loaded while its documented counter function is used.
    let library = unsafe {
        libloading::Library::new(
            std::env::var_os("VMAF_FINALIZATION_FIXTURE_LIBRARY").expect("fixture"),
        )
    }
    .expect("library");
    // SAFETY: exact signature defined in ownership.c, indices 0..=23.
    let counter = unsafe { library.get::<unsafe extern "C" fn(u32) -> u32>(b"ownership_count\0") }
        .expect("counter");
    // SAFETY: all calls use defined indices while fixture is loaded.
    let count = |index| unsafe { counter(index) };
    let mut scorer = VmafScorer::new(
        VmafConfig::new().with_backend(BackendPreference::CpuOnly).with_threads(2),
        VideoFormat {
            width: 8,
            height: 8,
            bit_depth: 8,
            chroma_sampling: ChromaSubsampling::Yuv420,
        },
    )
    .expect("constructor");
    let bytes = [128u8; 64];
    let planes = [(8, 8), (4, 4), (4, 4)]
        .map(|(w, h)| PlaneSource::from_bytes(&bytes, w, h, w as isize, 0, 1).expect("planes"));
    for index in 0..3 {
        assert_eq!(
            scorer.submit_pair_raw(planes, planes).expect("submission"),
            index,
            "sequential accepted index"
        );
        if scenario == "early" {
            assert!(
                scorer.drain_scores(0).expect("pending").is_empty(),
                "no interim scores"
            );
            assert_eq!(count(19), 0, "no native score queries before flush");
        }
    }
    if scenario == "flush_error" {
        for attempt in 1..=2 {
            assert_call(
                &scorer.finish().expect_err("flush failure"),
                "vmaf_read_pictures",
                -73,
            );
            assert_eq!(count(18), attempt, "failed flush must not mark finalized");
            assert!(
                scorer.drain_scores(0).expect("not finalized").is_empty(),
                "failed flush leaves scores unavailable"
            );
            assert_eq!(count(19), 0, "failed flush must not query scores");
        }
    } else if scenario == "query_error" {
        for error in [
            scorer.finish().expect_err("final query"),
            scorer.drain_scores(1).expect_err("final drain"),
        ] {
            assert!(
                matches!(
                    error,
                    VmafError::ScoreQueryFailed {
                        index: 1,
                        status: -74
                    }
                ),
                "exact query error: {error:?}"
            );
            let message = error.to_string();
            assert!(
                message.contains("vmaf_score_at_index")
                    && message.contains("-74")
                    && message.contains("1"),
                "raw function/status/index: {error:?}"
            );
        }
        assert_eq!(
            count(18),
            1,
            "final query errors do not require a second flush"
        );
    } else {
        assert_eq!(
            scorer.finish().expect("all final scores"),
            [90.0, 91.0, 92.0],
            "all final scores retain index order"
        );
        let mut cursor = 0;
        let drained = scorer.drain_scores(cursor).expect("final drain");
        cursor += drained.len();
        assert_eq!(
            drained,
            [90.0, 91.0, 92.0],
            "drain matches the full final vector"
        );
        assert!(
            scorer.drain_scores(cursor).expect("advanced cursor").is_empty(),
            "caller-advanced drain has no duplicates"
        );
        assert_eq!(
            scorer.drain_scores(1).expect("suffix cursor"),
            [91.0, 92.0],
            "caller selects the starting index"
        );
        if scenario == "repeat" {
            assert_eq!(
                scorer.finish().expect("stable repeated finish"),
                drained,
                "finish always returns the full vector"
            );
        }
        if scenario == "postflush" {
            let sum = count(23);
            let changed = [7u8; 64];
            let changed = [(8, 8), (4, 4), (4, 4)].map(|(w, h)| {
                PlaneSource::from_bytes(&changed, w, h, w as isize, 0, 1).expect("changed")
            });
            assert!(
                scorer.submit_pair_raw(changed, changed).is_err(),
                "reject submissions after successful flush"
            );
            assert_eq!(
                count(23),
                sum,
                "post-flush rejection before either host copy"
            );
            assert_eq!(count(READ), 3, "no post-flush native submission");
            assert_eq!(count(FETCH), 8, "no post-flush replacement fetches");
        }
        if scenario == "logging" {
            assert_eq!(count(22), 0, "native diagnostics disabled");
        }
        assert_eq!(count(18), 1, "exactly one native flush");
        assert_eq!(count(21), 0, "no native calls into a flushed context");
    }
    drop(scorer);
    assert_eq!(
        count(CHECKOUT),
        count(TRANSFER) + count(RUST_RETURN),
        "one disposition per checkout"
    );
    assert_eq!(count(TRANSFER), 6, "three transferred picture pairs");
    assert_eq!(
        count(WORKER_REF),
        count(WORKER_RELEASE),
        "all worker refs released"
    );
    for index in [DUPLICATE, ZERO_UNREF, CLOSE_OWNED, CLOSE_REFS, BAD_INDEX] {
        assert_eq!(
            count(index),
            0,
            "no ownership/index violation at counter {index}"
        );
    }
    assert_eq!(count(CLOSE), 1, "context closes once");
    assert_eq!(count(ALLOC), count(FREE), "all fixture allocations freed");
    eprintln!(
        "finalization {scenario} counters {:?}",
        (0..23).map(count).collect::<Vec<_>>()
    );
}

#[test]
fn picture_ownership() {
    if let Ok(scenario) = std::env::var("VMAF_OWNERSHIP_FIXTURE_CHILD") {
        check_scenario(&scenario);
        return;
    }
    let directory = tempfile::tempdir().expect("temporary ABI fixture");
    let library = directory.path().join(if cfg!(target_os = "macos") {
        "libvmaf.dylib"
    } else {
        "libvmaf.so"
    });
    let output = Command::new("cc")
        .args(["-shared", "-fPIC", "-std=c99", "-Wall", "-Wextra", "-Werror", "-o"])
        .arg(&library)
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/ownership.c"
        ))
        .output()
        .expect("ownership ABI tests require cc");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    for (scenario, threads, nonzero, fail_fetch, reject, flush) in [
        ("cuda_compiled_cpu", 0, 1, 0, 0, 0),
        ("cpu", 0, 0, 0, 0, 0),
        ("threaded_cpu", 2, 0, 0, 0, 0),
        ("threaded_cuda_compiled_cpu", 2, 1, 0, 0, 0),
        ("reject_cpu", 0, 0, 3, 1, 0),
        ("reject_nonzero", 0, 1, 3, 1, 0),
        ("reject_threaded", 2, 0, 3, 1, 0),
        ("reject_threaded_nonzero", 2, 1, 3, 1, 0),
        ("replace_first", 2, 1, 3, 0, 0),
        ("replace_second", 2, 1, 4, 0, 0),
        ("later_replace_first", 2, 1, 5, 0, 0),
        ("later_replace_second", 2, 1, 6, 0, 0),
        ("initial_first", 0, 1, 1, 0, 0),
        ("initial_second", 0, 1, 2, 0, 0),
        ("flush_error", 2, 1, 0, 0, 1),
    ] {
        let mut child = Command::new(std::env::current_exe().expect("test executable"))
            .args(["--exact", "scorer::ownership::picture_ownership", "--nocapture"])
            .env("VMAF_OWNERSHIP_FIXTURE_CHILD", scenario)
            .env("VMAF_OWNERSHIP_FIXTURE_THREADS", threads.to_string())
            .env("VMAF_OWNERSHIP_FIXTURE_NONZERO", nonzero.to_string())
            .env("VMAF_OWNERSHIP_FIXTURE_FAIL_FETCH", fail_fetch.to_string())
            .env("VMAF_OWNERSHIP_FIXTURE_REJECT_READ", reject.to_string())
            .env("VMAF_OWNERSHIP_FIXTURE_FAIL_FLUSH", flush.to_string())
            .env("VMAF_OWNERSHIP_FIXTURE_LIBRARY", &library)
            .env("VMAF_LIB_DIR", directory.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("isolated ABI child");
        let start = Instant::now();
        while child.try_wait().expect("child status").is_none() {
            if start.elapsed() > Duration::from_secs(30) {
                child.kill().expect("kill timed out ABI child");
                child.wait().expect("reap timed out ABI child");
                panic!("{scenario}: ABI child exceeded 30s");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = child.wait_with_output().expect("ABI child output");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{scenario}: {stdout}\n{stderr}");
        assert!(
            stdout.contains("running 1 test") && stdout.contains("1 passed; 0 failed"),
            "{scenario}: exact child must execute one test, not silently match zero: {stdout}"
        );
        eprintln!("{scenario}: {stdout}\n{stderr}");
    }
}

fn assert_call(error: &VmafError, expected_function: &str, expected_status: i32) {
    assert!(
        matches!(error, VmafError::CallFailed { function, status }
            if *function == expected_function && *status == expected_status),
        "expected {expected_function}/{expected_status}, got {error:?}"
    );
}

fn check_scenario(scenario: &str) {
    let library_path = std::env::var_os("VMAF_OWNERSHIP_FIXTURE_LIBRARY").expect("fixture library");
    // SAFETY: this is our compiled fixture, kept mapped throughout counter reads.
    let library = unsafe { libloading::Library::new(library_path) }.expect("load fixture counters");
    // SAFETY: the fixture exports this exact signature, with indices 0..18.
    let counter = unsafe { library.get::<unsafe extern "C" fn(u32) -> u32>(b"ownership_count\0") }
        .expect("fixture counter symbol");
    // SAFETY: every call below uses a defined counter index while the library is live.
    let count = |index| unsafe { counter(index) };
    let threads: u32 = std::env::var("VMAF_OWNERSHIP_FIXTURE_THREADS")
        .expect("threads")
        .parse()
        .expect("thread count");
    let format = VideoFormat {
        width: 8,
        height: 8,
        bit_depth: 8,
        chroma_sampling: ChromaSubsampling::Yuv420,
    };
    let result = VmafScorer::new(
        VmafConfig::new().with_backend(BackendPreference::CpuOnly).with_threads(threads),
        format,
    );
    let expected_fetch;
    let expected_checkout;
    let expected_read;
    let expected_transfer;
    let expected_return;
    if scenario.starts_with("initial_") {
        assert_call(
            &result.err().expect("initial fetch must fail"),
            "vmaf_fetch_preallocated_picture",
            -72,
        );
        expected_fetch = if scenario == "initial_first" { 1 } else { 2 };
        expected_checkout = expected_fetch - 1;
        expected_read = 0;
        expected_transfer = 0;
        expected_return = expected_checkout;
        // M11's whole-attempt guard also releases the model on initial-fetch failure.
    } else {
        let mut scorer = result.expect("production constructor");
        let bytes = [128u8; 64];
        let planes = [(8, 8), (4, 4), (4, 4)]
            .map(|(w, h)| PlaneSource::from_bytes(&bytes, w, h, w as isize, 0, 1).expect("planes"));
        if scenario.starts_with("reject_") {
            assert_call(
                &scorer.submit_pair_raw(planes, planes).expect_err("read rejection"),
                "vmaf_read_pictures",
                -71,
            );
            assert_eq!(
                count(FETCH),
                2,
                "rejected read must not fetch replacements or mask error"
            );
            expected_fetch = 2;
            expected_checkout = 2;
            expected_read = 1;
            expected_transfer = 0;
            expected_return = 2;
        } else if scenario.contains("replace_") {
            let later = scenario.starts_with("later_");
            if later {
                assert_eq!(
                    scorer.submit_pair_raw(planes, planes).expect("first pair"),
                    0,
                    "first accepted index"
                );
            }
            assert_call(
                &scorer.submit_pair_raw(planes, planes).expect_err("replacement failure"),
                "vmaf_fetch_preallocated_picture",
                -72,
            );
            let reads = count(READ);
            assert!(
                scorer.submit_pair_raw(planes, planes).is_err(),
                "terminal scorer must not reuse a partial pair or index"
            );
            assert!(
                scorer.finish().is_err(),
                "terminal scorer must not return partial scores"
            );
            assert!(
                scorer.drain_scores(0).is_err(),
                "terminal scorer must not silently drain partial scores"
            );
            assert!(
                scorer.pooled_features().is_err(),
                "terminal scorer must not return partial features"
            );
            assert_eq!(
                count(READ),
                reads,
                "terminal scorer must not reach read/flush"
            );
            let second = scenario.ends_with("second");
            expected_fetch = if later { 5 } else { 3 } + u32::from(second);
            expected_checkout = expected_fetch - 1;
            expected_read = if later { 2 } else { 1 };
            expected_transfer = expected_read * 2;
            expected_return = u32::from(second);
        } else {
            let mut bad = planes;
            bad[2] = PlaneSource::from_bytes(&bytes, 3, 4, 4, 0, 1).expect("malformed geometry");
            assert!(
                scorer.submit_pair_raw(planes, bad).is_err(),
                "distorted-side preflight"
            );
            assert_eq!(count(READ), 0, "preflight must not submit");
            assert_eq!(count(FETCH), 2, "preflight preserves the owned pair");
            let frame = FrameBuilder::new(8, 8, ChromaSubsampling::Yuv420, 8)
                .build::<u8>()
                .expect("frame");
            assert_eq!(
                scorer.submit_pair(&frame, &frame).expect("typed pair"),
                0,
                "preflight rejection preserves index zero"
            );
            assert_eq!(
                scorer.submit_pair_raw(planes, planes).expect("raw pair"),
                1,
                "second accepted index"
            );
            if scenario == "flush_error" {
                assert_call(
                    &scorer.finish().expect_err("background error"),
                    "vmaf_read_pictures",
                    -73,
                );
            } else {
                assert_eq!(
                    scorer.finish().expect("finish"),
                    [99.0, 99.0],
                    "both submitted scores survive"
                );
            }
            expected_fetch = 6;
            expected_checkout = 6;
            expected_read = 2;
            expected_transfer = 4;
            expected_return = 2;
        }
        drop(scorer);
    }
    let snapshot: Vec<_> = (0..18).map(count).collect();
    eprintln!("{scenario} counters: {snapshot:?}");
    assert_eq!(count(FETCH), expected_fetch, "attempted fetches");
    assert_eq!(count(CHECKOUT), expected_checkout, "successful checkouts");
    assert_eq!(count(READ), expected_read, "native reads");
    assert_eq!(count(TRANSFER), expected_transfer, "transfers");
    assert_eq!(count(RUST_RETURN), expected_return, "explicit Rust returns");
    assert_eq!(
        count(RUST_UNREF),
        expected_return,
        "only owned pictures may be unref'd"
    );
    assert_eq!(
        count(CHECKOUT),
        count(TRANSFER) + count(RUST_RETURN),
        "one disposition per checkout"
    );
    assert_eq!(
        count(NATIVE_RELEASE),
        expected_transfer,
        "native releases transferred refs"
    );
    let worker_refs = if threads == 0 { 0 } else { expected_read * 2 };
    assert_eq!(count(WORKER_REF), worker_refs, "worker copies");
    assert_eq!(
        count(WORKER_RELEASE),
        worker_refs,
        "worker copies released independently"
    );
    for index in [DUPLICATE, ZERO_UNREF, CLOSE_OWNED, CLOSE_REFS, BAD_INDEX] {
        assert_eq!(
            count(index),
            0,
            "ownership/index violation at counter {index}: {snapshot:?}"
        );
    }
    assert_eq!(count(CLOSE), 1, "context closed exactly once");
    assert_eq!(
        count(ALLOC),
        count(FREE),
        "fixture tombstones freed at close"
    );
}
