//! Native unknown pixel-format writes run only with integer-safe descriptors.
#![cfg(unix)]
use av_metrics_vmaf::{
    BackendPreference, PlaneSource, VideoFormat, VmafConfig, VmafError, VmafScorer,
};
use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use v_frame::chroma::ChromaSubsampling;

#[test]
fn invalid_initial_reference_format_is_adopted_and_returned_once() {
    isolated_formats(
        "invalid_initial_reference_format_is_adopted_and_returned_once",
        1,
        false,
    );
}

#[test]
fn invalid_initial_distorted_format_returns_both_owned_pictures() {
    isolated_formats(
        "invalid_initial_distorted_format_returns_both_owned_pictures",
        2,
        false,
    );
}

#[test]
fn invalid_reference_replacement_format_is_returned_and_makes_scorer_terminal() {
    isolated_formats(
        "invalid_reference_replacement_format_is_returned_and_makes_scorer_terminal",
        3,
        false,
    );
}

#[test]
fn invalid_distorted_replacement_format_is_returned_and_makes_scorer_terminal() {
    isolated_formats(
        "invalid_distorted_replacement_format_is_returned_and_makes_scorer_terminal",
        4,
        false,
    );
}

#[test]
fn failed_fetch_with_invalid_format_preserves_status_without_adopting_output() {
    isolated_formats(
        "failed_fetch_with_invalid_format_preserves_status_without_adopting_output",
        2,
        true,
    );
}

fn isolated_formats(test: &str, fetch: u32, failed: bool) {
    if std::env::var_os("VMAF_PIXEL_FORMAT_FIXTURE_CHILD").is_some() {
        assert_eq!(
            std::env::var("VMAF_PIXEL_FORMAT_FIXTURE_FETCH").expect("fetch"),
            fetch.to_string(),
            "child fetch fault belongs to the selected contract"
        );
        assert_eq!(
            std::env::var("VMAF_SETUP_FIXTURE_STAGE").expect("stage"),
            if failed { "9" } else { "0" },
            "child native status belongs to the selected contract"
        );
        assert_pool_format_ownership();
        println!("pool-format ownership assertions completed");
        return;
    }
    let module = module_path!().split_once("::").expect("test crate prefix").1;
    let exact = format!("{module}::{test}");
    let dir = tempfile::tempdir().expect("fixture directory");
    let library = dir.path().join(if cfg!(target_os = "macos") {
        "libvmaf.dylib"
    } else {
        "libvmaf.so"
    });
    let out = Command::new("cc")
        .args(["-shared", "-fPIC", "-std=c11", "-Wall", "-Wextra", "-Werror", "-pthread"])
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/pixel_format.c"
        ))
        .arg("-o")
        .arg(&library)
        .output()
        .expect("C compiler required");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    for format in [777, -123, 0, 4] {
        let mut child = Command::new(std::env::current_exe().expect("executable"))
            .args(["--exact", &exact, "--nocapture"])
            .env("VMAF_PIXEL_FORMAT_FIXTURE_CHILD", "1")
            .env("VMAF_PIXEL_FORMAT_FIXTURE_FETCH", fetch.to_string())
            .env("VMAF_PIXEL_FORMAT_FIXTURE_PIXFMT", format.to_string())
            .env("VMAF_SETUP_FIXTURE_STAGE", if failed { "9" } else { "0" })
            .env("VMAF_PARTIAL_SETUP_FIXTURE_CHILD", "1")
            .env("VMAF_PIXEL_FORMAT_FIXTURE_LIBRARY", &library)
            .env("VMAF_LIB_DIR", dir.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("child");
        let start = Instant::now();
        while child.try_wait().expect("status").is_none() {
            if start.elapsed() > Duration::from_secs(30) {
                child.kill().expect("kill");
                child.wait().expect("reap");
                panic!("pool close hung");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let out = child.wait_with_output().expect("output");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "{fetch}/{format}/{failed}: {stdout}\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            stdout.contains("1 passed; 0 failed")
                && stdout.contains("pool-format ownership assertions completed"),
            "exact nonzero child: {stdout}"
        );
    }
}
fn assert_pool_format_ownership() {
    // SAFETY: exact fixture mapping remains live through cleanup and counters.
    let lib = unsafe {
        libloading::Library::new(
            std::env::var_os("VMAF_PIXEL_FORMAT_FIXTURE_LIBRARY").expect("library"),
        )
    }
    .expect("mapping");
    // SAFETY: exact fixture signatures with valid indices below.
    let counter = unsafe { lib.get::<unsafe extern "C" fn(u32, u32) -> u32>(b"setup_count\0") }
        .expect("counter");
    // SAFETY: fixture signatures, called while mapped.
    let invalid =
        unsafe { lib.get::<unsafe extern "C" fn() -> u32>(b"pixel_format_invalid_returns\0") }
            .expect("invalid returns");
    // SAFETY: cleanup runs once after every Rust owner is dropped.
    let cleanup =
        unsafe { lib.get::<unsafe extern "C" fn()>(b"setup_cleanup\0") }.expect("cleanup");
    // SAFETY: all used counter indices are defined by setup.c.
    let count = |b, i| unsafe { counter(b, i) };
    let format = VideoFormat {
        width: 8,
        height: 8,
        bit_depth: 8,
        chroma_sampling: ChromaSubsampling::Yuv420,
    };
    let fetch: u32 = std::env::var("VMAF_PIXEL_FORMAT_FIXTURE_FETCH")
        .expect("fetch")
        .parse()
        .expect("integer");
    let failed = std::env::var("VMAF_SETUP_FIXTURE_STAGE").expect("stage") == "9";
    let result = VmafScorer::new(
        VmafConfig::default().with_backend(BackendPreference::CpuOnly).with_threads(2),
        format,
    );
    let error = if fetch <= 2 {
        result.err().expect("bad initial picture")
    } else {
        let mut scorer = result.expect("initial pair");
        let a = [11u8; 64];
        let b = [22u8; 64];
        let planes = |bytes| {
            [(8, 8), (4, 4), (4, 4)].map(|(w, h)| {
                PlaneSource::from_bytes(bytes, w, h, w as isize, 0, 1).expect("planes")
            })
        };
        let error = scorer.submit_pair_raw(planes(&a), planes(&b)).expect_err("bad replacement");
        assert!(
            scorer.submit_pair_raw(planes(&a), planes(&b)).is_err(),
            "no retry after replacement failure"
        );
        drop(scorer);
        error
    };
    if failed {
        assert!(
            matches!(error, VmafError::CallFailed { status: -109, .. }),
            "preserve failed-fetch status before inspecting unowned output"
        );
    } else {
        assert!(
            matches!(&error, VmafError::UnsupportedFormat { .. }),
            "{error:?}"
        );
        assert!(
            error
                .to_string()
                .contains(&std::env::var("VMAF_PIXEL_FORMAT_FIXTURE_PIXFMT").expect("format")),
            "preserve raw pixel-format integer"
        );
    }
    assert_eq!(
        count(0, 14),
        u32::from(fetch > 2),
        "no read_pictures for invalid initial format"
    );
    assert_eq!(
        count(0, 11),
        count(0, 12) + count(0, 13),
        "every successful checkout returned/transferred once"
    );
    assert_eq!(count(0, 2), 1, "context close unblocked");
    assert_eq!(count(2, 6), 0, "ownership/canary violations");
    // SAFETY: fixture remains mapped; failed fetch has no owning return.
    let invalid_returns = unsafe { invalid() };
    assert_eq!(
        invalid_returns,
        u32::from(!failed),
        "adopt successful invalid picture exactly once, never failed fetch output"
    );
    // SAFETY: all native/Rust owners are gone, free tombstones exactly once.
    unsafe { cleanup() };
}
