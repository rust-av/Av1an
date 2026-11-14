//! Exercise the cached production loader before any layout-sensitive operation.
#![cfg(unix)]

use std::{
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use av_metrics_vmaf::{
    BackendPreference, PlaneSource, VideoFormat, VmafConfig, VmafError, VmafScorer,
};
use v_frame::chroma::ChromaSubsampling;

const MARKERS: [&str; 5] = [
    "",
    "vmaf_picture_convert_context_init",
    "vmaf_picture_convert",
    "vmaf_picture_convert_context_close",
    "vmaf_picture_wrap",
];

#[path = "../../test-support/runtime_discovery.rs"]
mod runtime_discovery;

#[test]
fn runtime_overrides_and_staged_release() {
    if std::env::var_os("RUNTIME_DISCOVERY_CHILD").is_some() {
        // Load through production discovery before mapping fixture counters.
        assert_eq!(
            av_metrics_vmaf::libvmaf_version(),
            Some("3.2.0"),
            "production discovery selects the pinned runtime"
        );
        check_child(true);
        return;
    }
    runtime_discovery::run(
        "runtime_overrides_and_staged_release",
        if cfg!(target_os = "macos") {
            "libvmaf.dylib"
        } else {
            "libvmaf.so"
        },
        &["VMAF_LIB_DIR"],
        true,
        |library| compile(library, 0),
        |command, library| {
            command
                .env("VMAF_RUNTIME_FIXTURE_VERSION", "3.2.0")
                .env("VMAF_RUNTIME_FIXTURE_MARKER_NAME", "")
                .env("VMAF_RUNTIME_FIXTURE_LIBRARY", library);
        },
    );
}

#[test]
fn rejects_unverified_versions_before_native_setup() {
    run_cases(
        "rejects_unverified_versions_before_native_setup",
        0,
        &[
            "3.2.1",
            "3.1.0",
            "3.3.0",
            "3.2.0-dev",
            "v3.2.1-0-gf85a853",
            "",
            "unknown",
            "non-UTF8",
            "null",
        ],
        false,
    );
}

#[test]
fn rejects_same_version_new_layout_markers_before_native_setup() {
    if std::env::var_os("VMAF_RUNTIME_FIXTURE_CHILD").is_some() {
        check_child(false);
        return;
    }
    for marker in 1..MARKERS.len() {
        run_cases(
            "rejects_same_version_new_layout_markers_before_native_setup",
            marker,
            &["3.2.0"],
            false,
        );
    }
}

#[test]
fn accepts_verified_archive_runtime_and_scores() {
    run_cases(
        "accepts_verified_archive_runtime_and_scores",
        0,
        &["3.2.0"],
        true,
    );
}

fn run_cases(test: &str, marker: usize, versions: &[&str], accepted: bool) {
    if std::env::var_os("VMAF_RUNTIME_FIXTURE_CHILD").is_some() {
        check_child(accepted);
        return;
    }
    let directory = tempfile::tempdir().expect("temporary runtime ABI fixture");
    let library = directory.path().join(if cfg!(target_os = "macos") {
        "libvmaf.dylib"
    } else {
        "libvmaf.so"
    });
    compile(&library, marker);
    for version in versions {
        run_child(test, &library, directory.path(), version, MARKERS[marker]);
    }
}

fn compile(library: &Path, marker: usize) {
    let output = Command::new("cc")
        .args(["-shared", "-fPIC", "-std=c11", "-Wall", "-Wextra", "-Werror", "-o"])
        .arg(library)
        .arg(format!("-DVMAF_RUNTIME_FIXTURE_MARKER={marker}"))
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/runtime.c"
        ))
        .output()
        .expect("runtime ABI regression requires cc");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run_child(test: &str, library: &Path, directory: &Path, version: &str, marker: &str) {
    let mut child = Command::new(std::env::current_exe().expect("test executable"))
        .args(["--exact", test, "--nocapture"])
        .env("VMAF_RUNTIME_FIXTURE_CHILD", "1")
        .env("VMAF_RUNTIME_FIXTURE_VERSION", version)
        .env("VMAF_RUNTIME_FIXTURE_MARKER_NAME", marker)
        .env("VMAF_RUNTIME_FIXTURE_LIBRARY", library)
        .env("VMAF_LIB_DIR", directory)
        .env_remove("VMAF_MODEL_PATH")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("isolated runtime ABI child");
    let start = Instant::now();
    while child.try_wait().expect("child status").is_none() {
        if start.elapsed() > Duration::from_secs(30) {
            child.kill().expect("kill timed out child");
            child.wait().expect("reap timed out child");
            panic!("{version}/{marker}: runtime ABI child exceeded 30s");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().expect("runtime ABI child output");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "{version}/{marker}: {stdout}\n{stderr}"
    );
    assert!(
        stdout.contains("running 1 test") && stdout.contains("1 passed; 0 failed"),
        "exact child must execute one test: {stdout}"
    );
    eprintln!("{version}/{marker}: {stderr}");
}

fn check_child(accepted: bool) {
    // SAFETY: our isolated fixture stays mapped throughout the counter reads.
    let library = unsafe {
        libloading::Library::new(
            std::env::var_os("VMAF_RUNTIME_FIXTURE_LIBRARY").expect("fixture library"),
        )
    }
    .expect("fixture counters");
    // SAFETY: exact fixture signature and defined indices 0..3, never layout calls.
    let count =
        unsafe { library.get::<unsafe extern "C" fn(u32) -> u32>(b"runtime_fixture_count\0") }
            .expect("runtime ABI counter");
    // SAFETY: the mapping remains live; every index is in range.
    let snapshot = || (0..3).map(|index| unsafe { count(index) }).collect::<Vec<_>>();
    let format = VideoFormat {
        width: 8,
        height: 8,
        bit_depth: 8,
        chroma_sampling: ChromaSubsampling::Yuv420,
    };
    for backend in [
        BackendPreference::CpuOnly,
        BackendPreference::Auto,
        BackendPreference::RequireCuda,
    ] {
        if accepted && backend != BackendPreference::CpuOnly {
            break;
        }
        let result = VmafScorer::new(VmafConfig::new().with_backend(backend), format);
        if accepted {
            let mut scorer = result.expect("verified archive runtime accepted");
            let bytes = [128u8; 64];
            let planes = [(8, 8), (4, 4), (4, 4)].map(|(w, h)| {
                PlaneSource::from_bytes(&bytes, w, h, w as isize, 0, 1).expect("planes")
            });
            scorer.submit_pair_raw(planes, planes).expect("submit through actual scorer");
            assert_eq!(
                scorer.finish().expect("score marker"),
                [99.0],
                "accepted ABI reaches native scoring"
            );
            assert_eq!(
                snapshot(),
                [1, 1, 4],
                "accepted constructor and replacement pictures"
            );
            assert_eq!(
                av_metrics_vmaf::libvmaf_version(),
                Some("3.2.0"),
                "accepted runtime remains cached"
            );
            eprintln!("accepted native calls: {:?}", snapshot());
        } else {
            // Drop an erroneously accepted scorer before reporting RED; no unsafe overrun.
            let error = result.err();
            eprintln!("rejected native calls: {:?}; error: {error:?}", snapshot());
            assert_eq!(
                snapshot(),
                [0, 0, 0],
                "reject before init/preallocate/fetch"
            );
            let error = error.expect("incompatible runtime must be rejected");
            if !cfg!(feature = "cuda") && backend == BackendPreference::RequireCuda {
                assert!(
                    matches!(error, VmafError::UnsupportedFeature(ref message)
                        if message.contains("Cargo") && message.contains("--features cuda")),
                    "missing feature is rejected before loading the incompatible runtime: {error:?}"
                );
                continue;
            }
            let VmafError::IncompatibleLibrary { version, .. } = &error else {
                panic!("dedicated compatibility error: {error:?}");
            };
            let observed = std::env::var("VMAF_RUNTIME_FIXTURE_VERSION").expect("observed version");
            assert!(
                version.contains(&observed),
                "preserve observed version: {error}"
            );
            let error = error.to_string();
            assert!(
                error.contains("Incompatible libvmaf"),
                "dedicated compatibility error: {error}"
            );
            assert!(
                error.contains("3.2.0") && error.contains("v3.2.1") && error.contains("Reinstall"),
                "actionable error: {error}"
            );
            let marker = std::env::var("VMAF_RUNTIME_FIXTURE_MARKER_NAME").expect("marker");
            assert!(error.contains(&marker), "observed marker: {error}");
        }
    }
}
