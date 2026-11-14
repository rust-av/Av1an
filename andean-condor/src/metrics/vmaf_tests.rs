//! Real decode/driver/scorer, with only the native boundary doubled.
use std::{
    io::Write,
    process::{Command, Stdio},
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

use super::*;

const EXACT: &str = "metrics::vmaf::m18_tests::m18_driver_events";

#[test]
fn q05_vmaf_invalid_last_value_has_no_score_callbacks() {
    if let Ok(scenario) = std::env::var("M18_DRIVER_CHILD") {
        driver_child(&scenario);
        return;
    }
    let directory = tempfile::tempdir().expect("Q05 fixture");
    let library = directory.path().join(if cfg!(target_os = "macos") {
        "libvmaf.dylib"
    } else {
        "libvmaf.so"
    });
    let output = Command::new("cc")
        .args(["-shared", "-fPIC", "-std=c99", "-Wall", "-Wextra", "-Werror", "-o"])
        .arg(library)
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../av-metrics-vmaf/tests/fixtures/ownership.c"
        ))
        .output()
        .expect("cc");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for bad in ["1", "2", "3"] {
        let output = Command::new(std::env::current_exe().expect("executable"))
            .args([
                "--exact",
                "metrics::vmaf::m18_tests::q05_vmaf_invalid_last_value_has_no_score_callbacks",
                "--nocapture",
            ])
            .env("M18_DRIVER_CHILD", "q05-invalid")
            .env("VMAF_FINALIZATION_FIXTURE_BAD_SCORE", bad)
            .env("VMAF_FINALIZATION_FIXTURE_MODE", "1")
            .env(
                "VMAF_FINALIZATION_FIXTURE_EVENTS",
                directory.path().join(bad),
            )
            .env("VMAF_LIB_DIR", directory.path())
            .output()
            .expect("child");
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn m25_driver_invalid_override_preserves_error_before_callbacks() {
    if std::env::var_os("M25_DRIVER_CHILD").is_some() {
        driver_child("m25_override");
        return;
    }
    let directory = tempfile::tempdir().expect("M25 driver fixture");
    let mut child = Command::new(std::env::current_exe().expect("test executable"))
        .args([
            "--exact",
            "metrics::vmaf::m18_tests::m25_driver_invalid_override_preserves_error_before_callbacks",
            "--nocapture",
        ])
        .env("M25_DRIVER_CHILD", "1")
        .env("VMAF_MODEL_PATH", directory.path().join("missing models 雪"))
        .env("VMAF_FINALIZATION_FIXTURE_EVENTS", directory.path().join("events"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("isolated M25 driver child");
    let start = Instant::now();
    while child.try_wait().expect("status").is_none() {
        if start.elapsed() > Duration::from_secs(30) {
            child.kill().expect("kill");
            child.wait().expect("reap");
            panic!("M25 driver timeout");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().expect("output");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    eprintln!("M25 driver: {stdout}\n{stderr}");
    assert!(output.status.success(), "{stdout}\n{stderr}");
    assert!(stdout.contains("running 1 test") && stdout.contains("1 passed; 0 failed"));
}

#[test]
fn m18_driver_events() {
    if let Ok(scenario) = std::env::var("M18_DRIVER_CHILD") {
        driver_child(&scenario);
        return;
    }
    let directory = tempfile::tempdir().expect("M18 driver fixture");
    let library = directory.path().join(if cfg!(target_os = "macos") {
        "libvmaf.dylib"
    } else {
        "libvmaf.so"
    });
    let output = Command::new("cc")
        .args(["-shared", "-fPIC", "-std=c99", "-Wall", "-Wextra", "-Werror", "-o"])
        .arg(library)
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../av-metrics-vmaf/tests/fixtures/ownership.c"
        ))
        .output()
        .expect("M18 requires cc");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for scenario in [
        "aligned",
        "compacted",
        "reject",
        "cancelled",
        "cancel_after",
        "flush_error",
        "query_error",
        "empty",
    ] {
        let mut child = Command::new(std::env::current_exe().expect("test executable"))
            .args(["--exact", EXACT, "--nocapture"])
            .env("M18_DRIVER_CHILD", scenario)
            .env(
                "VMAF_FINALIZATION_FIXTURE_MODE",
                if scenario == "query_error" { "2" } else { "1" },
            )
            .env(
                "VMAF_FINALIZATION_FIXTURE_EVENTS",
                directory.path().join(scenario),
            )
            .env(
                "VMAF_OWNERSHIP_FIXTURE_REJECT_READ",
                if scenario == "reject" { "1" } else { "0" },
            )
            .env(
                "VMAF_OWNERSHIP_FIXTURE_FAIL_FLUSH",
                if scenario == "flush_error" { "1" } else { "0" },
            )
            .env("VMAF_LIB_DIR", directory.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("child");
        let start = Instant::now();
        while child.try_wait().expect("status").is_none() {
            if start.elapsed() > Duration::from_secs(30) {
                child.kill().expect("kill");
                child.wait().expect("reap");
                panic!("driver timeout");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = child.wait_with_output().expect("output");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("M18 driver {scenario}: {stdout}\n{stderr}");
        assert!(output.status.success(), "{stdout}\n{stderr}");
        assert!(stdout.contains("running 1 test") && stdout.contains("1 passed; 0 failed"));
    }
}

fn driver_child(scenario: &str) {
    let fixture = tempfile::Builder::new().suffix(".y4m").tempfile().expect("Y4M");
    let mut y4m = b"YUV4MPEG2 W8 H8 F24:1 Ip A1:1 C420jpeg\n".to_vec();
    for value in [40, 80, 120, 160] {
        y4m.extend_from_slice(b"FRAME\n");
        y4m.extend([value; 64]);
        y4m.extend([128; 32]);
    }
    std::fs::write(fixture.path(), y4m).expect("write");
    let mut reference = av_decoders::Decoder::from_file(fixture.path()).expect("reference");
    let mut distorted = av_decoders::Decoder::from_file(fixture.path()).expect("distorted");
    let event_path = std::path::PathBuf::from(
        std::env::var_os("VMAF_FINALIZATION_FIXTURE_EVENTS").expect("events"),
    );
    let events = || std::fs::read_to_string(&event_path).unwrap_or_default();
    let log = |name, position| {
        writeln!(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&event_path)
                .expect("open log"),
            "{name}{position}"
        )
        .expect("log callback");
    };
    let cancelled = AtomicBool::new(scenario == "cancelled");
    let selected: &[usize] = if scenario == "empty" { &[] } else { &[0, 2, 3] };
    let indexing = if scenario == "compacted" {
        OutputIndexing::Compacted
    } else {
        OutputIndexing::Aligned
    };
    let mut submitted = Vec::new();
    let mut reported = Vec::new();
    let scores = score_probed_frames_with_progress(
        &mut reference,
        &mut distorted,
        &tests::metric(vec![]),
        selected,
        indexing,
        Some(&cancelled),
        |position| {
            assert!(
                !events().contains("flush"),
                "submission before finalization"
            );
            submitted.push(position);
            log("progress", position);
            if scenario == "cancel_after" {
                cancelled.store(true, Ordering::Relaxed);
            }
        },
        |position, score| {
            assert!(
                events().contains("flush0"),
                "real score only after finalization"
            );
            if scenario != "q05-invalid" {
                assert_eq!(score, 90.0 + position as f64);
            }
            reported.push((selected[position], score));
            log("score", position);
        },
    );
    match scenario {
        "q05-invalid" => {
            let error = scores.expect_err("invalid final native value");
            assert!(format!("{error:#}").contains("non-finite"), "{error:#}");
            assert_eq!(submitted, [0, 1, 2]);
            assert!(
                reported.is_empty(),
                "ALL values validated before first score callback"
            );
        },
        "m25_override" => {
            let error = scores.expect_err("invalid explicit directory must fail");
            let message = error.to_string();
            eprintln!("M25 driver submitted={submitted:?}, reported={reported:?}, error={message}");
            assert!(
                submitted.is_empty() && reported.is_empty() && events().is_empty(),
                "invalid override must fail before either progress or score callbacks"
            );
            let expected =
                std::path::PathBuf::from(std::env::var_os("VMAF_MODEL_PATH").expect("override"))
                    .join("vmaf_v0.6.1.json");
            assert!(
                message.contains("VMAF_MODEL_PATH")
                    && message.contains(&*expected.to_string_lossy())
                    && message.contains("missing"),
                "production driver must retain the missing override diagnostic: {message}"
            );
            assert!(
                matches!(
                    error.downcast_ref::<VmafError>(),
                    Some(VmafError::ModelNotFound { .. })
                ),
                "production driver must preserve the typed native cause: {error:?}"
            );
        },
        "aligned" | "compacted" => {
            assert_eq!(scores.expect("all scores"), [90.0, 91.0, 92.0]);
            assert_eq!(submitted, [0, 1, 2]);
            assert_eq!(reported, [(0, 90.0), (2, 91.0), (3, 92.0)]);
            let expected = if scenario == "compacted" {
                "init0\nsubmit0\nreference40\ndistorted40\nprogress0\nsubmit1\nreference120\ndistorted80\nprogress1\nsubmit2\nreference160\ndistorted120\nprogress2\nflush0\nquery0\nquery1\nquery2\nscore0\nscore1\nscore2\n"
            } else {
                "init0\nsubmit0\nreference40\ndistorted40\nprogress0\nsubmit1\nreference120\ndistorted120\nprogress1\nsubmit2\nreference160\ndistorted160\nprogress2\nflush0\nquery0\nquery1\nquery2\nscore0\nscore1\nscore2\n"
            };
            assert_eq!(
                events(),
                expected,
                "native and callback order, no interim query"
            );
        },
        "empty" => {
            assert!(scores.expect("empty selection").is_empty());
            assert!(submitted.is_empty() && reported.is_empty() && events().is_empty());
        },
        _ => {
            let error = scores.expect_err("injected rejection/cancellation/native error");
            assert!(reported.is_empty(), "no fabricated/partial score callbacks");
            if matches!(scenario, "cancelled" | "cancel_after") {
                assert!(error.is::<probe::Cancelled>(), "{error:?}");
                assert_eq!(submitted.len(), usize::from(scenario == "cancel_after"));
                assert!(!events().contains("flush"));
            } else {
                let native = error.downcast_ref::<VmafError>().expect("original Rust native error");
                if scenario == "query_error" {
                    assert!(matches!(
                        native,
                        VmafError::ScoreQueryFailed {
                            index: 1,
                            status: -74
                        }
                    ));
                } else {
                    assert!(
                        matches!(native, VmafError::CallFailed { function: "vmaf_read_pictures", status } if *status == if scenario == "reject" { -71 } else { -73 })
                    );
                }
                assert_eq!(submitted.len(), if scenario == "reject" { 0 } else { 3 });
                if scenario == "flush_error" {
                    assert!(!events().contains("query"));
                }
            }
        },
    }
    eprintln!("M18 driver {scenario} events:\n{}", events());
}
