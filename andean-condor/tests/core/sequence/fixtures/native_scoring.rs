//! Shared native ABI supervisor and literal inputs; private scoring callers remain source-owned.
#![cfg(unix)]
#![expect(clippy::unwrap_used, reason = "fixture assertions fail loudly")]
use super::condor;
use condor::models::sequence::target_quality::types::QualityMetric;
use std::{
    process::Command,
    time::{Duration, Instant},
};

pub(crate) fn metric(disabled: Option<bool>) -> QualityMetric {
    QualityMetric::CVVDP {
        target_range: (0.0, 1.0),
        resolution: None,
        display_model: None,
        resize_to_display: Some(false),
        disable_temporal: disabled,
        gpu_id: Some(0),
    }
}

pub(crate) fn calls() -> String {
    std::fs::read_to_string(std::env::var_os("TEMPORAL_FIXTURE_LOG").unwrap()).unwrap_or_default()
}
pub(crate) fn clear() {
    std::fs::write(std::env::var_os("TEMPORAL_FIXTURE_LOG").unwrap(), "").unwrap();
}

pub(crate) fn native_children(test: &str, scopes: &[(&str, &str)], child_test: impl FnOnce()) {
    if std::env::var_os("NATIVE_SCORING_CHILD").is_some() {
        let scope = std::env::var("TEMPORAL_FIXTURE_SCOPE").unwrap();
        let failure = std::env::var("TEMPORAL_FIXTURE_FAIL").unwrap();
        assert!(
            scopes.contains(&(scope.as_str(), failure.as_str())),
            "native scope belongs to this contract"
        );
        child_test();
        println!("native-scoring {scope}/{failure}: assertions completed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let library = dir.path().join(if cfg!(target_os = "macos") {
        "libfmetrics.dylib"
    } else {
        "libfmetrics.so"
    });
    let output = Command::new("cc")
        .args(["-std=c11", "-Wall", "-Wextra", "-Werror", "-shared", "-fPIC", "-o"])
        .arg(&library)
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/temporal.c"
        ))
        .output()
        .expect("native boundary requires cc");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for (engine, name) in [
        (
            "f",
            if cfg!(target_os = "macos") {
                "libfmetrics.dylib"
            } else {
                "libfmetrics.so"
            },
        ),
        (
            "v",
            if cfg!(target_os = "macos") {
                "libvship.dylib"
            } else {
                "libvship.so"
            },
        ),
    ] {
        let path = dir.path().join(engine);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::copy(&library, path.join(name)).unwrap();
    }
    // Prevent CPU caller cases from selecting a real GPU library via system fallback.
    let no_devices = dir.path().join("no_devices.c");
    std::fs::write(&no_devices, "#undef Vship_GetDeviceCount\n#define VSHIP_STATIC\n#include \"VshipAPI.h\"\nVship_Exception Vship_GetDeviceCount(int *n) { *n = 0; return Vship_NoError; }\n").unwrap();
    let output = Command::new("cc")
        .args([
            "-std=c11",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-shared",
            "-fPIC",
            "-DVship_GetDeviceCount=native_scoring_unused_device_count",
            "-I",
        ])
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../av-metrics-vship/tests/headers"
        ))
        .arg("-o")
        .arg(dir.path().join("f").join(if cfg!(target_os = "macos") {
            "libvship.dylib"
        } else {
            "libvship.so"
        }))
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/temporal.c"
        ))
        .arg(&no_devices)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut errors = Vec::new();
    for &(scope, failure) in scopes {
        let library_path = dir.path().join(if scope.contains("-f-") { "f" } else { "v" });
        // File-backed output cannot fill a pipe while the watchdog waits.
        let stdout = dir.path().join("stdout");
        let stderr = dir.path().join("stderr");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", test, "--nocapture"])
            .env("NATIVE_SCORING_CHILD", "1")
            .env("TEMPORAL_FIXTURE_SCOPE", scope)
            .env("TEMPORAL_FIXTURE_FAIL", failure)
            .env("FRAME_RATE_FIXTURE_FPS_AUDIT", "")
            .env("TEMPORAL_FIXTURE_LOG", dir.path().join("calls"))
            .env("FMETRICS_LIB_PATH", &library_path)
            .env("VSHIP_PLUGIN_PATH", &library_path)
            .stdout(std::fs::File::create(&stdout).unwrap())
            .stderr(std::fs::File::create(&stderr).unwrap())
            .spawn()
            .unwrap();
        let start = Instant::now();
        while child.try_wait().unwrap().is_none() {
            if start.elapsed() > Duration::from_secs(120) {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("native caller child timed out: {scope}/{failure}");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let status = child.wait().unwrap();
        let stdout = std::fs::read_to_string(stdout).unwrap();
        let stderr = std::fs::read_to_string(stderr).unwrap();
        eprint!("{stdout}\n{stderr}");
        if !status.success()
            || !stdout.contains("1 passed; 0 failed")
            || !stdout.contains("running 1 test\n")
            || !stdout.contains(&format!(
                "native-scoring {scope}/{failure}: assertions completed"
            ))
        {
            errors.push(format!("{scope}/{failure}: {stdout}\n{stderr}"));
        }
    }
    assert!(errors.is_empty(), "{}", errors.join("\n"));
}

pub(crate) fn y4m_fixture(samples: &[u8]) -> tempfile::NamedTempFile {
    let log = std::path::PathBuf::from(std::env::var_os("TEMPORAL_FIXTURE_LOG").unwrap());
    let file = tempfile::Builder::new()
        .suffix(".y4m")
        .tempfile_in(log.parent().unwrap())
        .unwrap();
    let mut data = b"YUV4MPEG2 W32 H32 F24:1 Ip A1:1 C420jpeg\n".to_vec();
    for sample in samples {
        data.extend_from_slice(b"FRAME\n");
        data.extend_from_slice(&[*sample; 1024]);
        data.extend_from_slice(&[128; 512]);
    }
    std::fs::write(file.path(), data).unwrap();
    file
}
