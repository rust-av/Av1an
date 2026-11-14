//! Non-skipping real-C boundary tests; no fmetrics/Zig installation needed.
use av_metrics_fmetrics::ffi::*;
#[cfg(unix)]
use av_metrics_fmetrics::{
    ColorInfo, FmetricsConfig, FmetricsError, FmetricsMetric, FmetricsScorer, RgbView,
};
use std::{
    mem::{align_of, offset_of, size_of},
    process::Command,
};
#[cfg(unix)]
use std::{
    process::Stdio,
    time::{Duration, Instant},
};

#[cfg(unix)]
#[path = "../../test-support/runtime_discovery.rs"]
mod runtime_discovery;

#[test]
#[cfg(unix)]
fn fmetrics_lib_path_loads_fixture_without_bundled_sibling() {
    runtime_case(
        "fmetrics_lib_path_loads_fixture_without_bundled_sibling",
        "FMETRICS_LIB_PATH",
    );
}

#[test]
#[cfg(unix)]
fn fmetrics_lib_dir_loads_fixture_without_bundled_sibling() {
    runtime_case(
        "fmetrics_lib_dir_loads_fixture_without_bundled_sibling",
        "FMETRICS_LIB_DIR",
    );
}

#[test]
#[cfg(unix)]
fn staged_fmetrics_loads_executable_sibling_without_loader_path_from_media_cwd() {
    runtime_case(
        "staged_fmetrics_loads_executable_sibling_without_loader_path_from_media_cwd",
        "bundled",
    );
}

#[cfg(unix)]
fn runtime_case(test: &str, mode: &str) {
    if let Ok(selected) = std::env::var("RUNTIME_DISCOVERY_CHILD") {
        assert_eq!(selected, mode, "case belongs to this contract");
        // Load through production discovery before mapping fixture counters.
        assert_eq!(
            av_metrics_fmetrics::fmetrics_version().as_deref(),
            Some("0.1.0"),
            "production discovery selects the pinned runtime"
        );
        child();
        return;
    }
    runtime_discovery::run(
        test,
        if cfg!(target_os = "macos") {
            "libfmetrics.dylib"
        } else {
            "libfmetrics.so"
        },
        &[mode],
        false,
        |library| compile(library, false, &[]),
        |command, library| {
            command
                .env("FMETRICS_BOUNDARY_FIXTURE_VERSION", "0.1.0")
                .env("FMETRICS_BOUNDARY_FIXTURE_STATUS", "0")
                .env("FMETRICS_BOUNDARY_FIXTURE_LIBRARY", library);
        },
    );
}

#[test]
#[cfg(unix)]
fn unverified_fmetrics_version_rejects_before_workspace_allocation() {
    status_cases(
        "unverified_fmetrics_version_rejects_before_workspace_allocation",
        &["0.2.0", "0.1.0-dev", "", "null", "non-UTF8", "m04-test"],
        &[0],
    );
}

#[test]
#[cfg(unix)]
fn accepted_fmetrics_abi_fixture_returns_ssimulacra2_marker() {
    status_cases(
        "accepted_fmetrics_abi_fixture_returns_ssimulacra2_marker",
        &["0.1.0"],
        &[0],
    );
}

#[test]
#[cfg(unix)]
fn known_native_fmetrics_failure_preserves_code_and_native_message() {
    status_cases(
        "known_native_fmetrics_failure_preserves_code_and_native_message",
        &["0.1.0"],
        &[1, 6],
    );
}

#[test]
#[cfg(unix)]
fn unknown_native_fmetrics_failure_preserves_raw_code_without_enum_translation() {
    status_cases(
        "unknown_native_fmetrics_failure_preserves_raw_code_without_enum_translation",
        &["0.1.0"],
        &[777, -123],
    );
}

#[cfg(unix)]
fn status_cases(test: &str, versions: &[&str], statuses: &[i32]) {
    if std::env::var_os("FMETRICS_BOUNDARY_FIXTURE_CHILD").is_some() {
        let version = std::env::var("FMETRICS_BOUNDARY_FIXTURE_VERSION").expect("version");
        let status = std::env::var("FMETRICS_BOUNDARY_FIXTURE_STATUS")
            .expect("status")
            .parse()
            .expect("integer");
        assert!(
            versions.contains(&version.as_str()) && statuses.contains(&status),
            "case belongs to this contract"
        );
        child();
        println!("native status/version assertions completed");
        return;
    }
    let dir = tempfile::tempdir().expect("fixture directory");
    let library = dir.path().join(if cfg!(target_os = "macos") {
        "libfmetrics.dylib"
    } else {
        "libfmetrics.so"
    });
    compile(&library, false, &[]);
    for version in versions {
        for status in statuses {
            let mut child = Command::new(std::env::current_exe().expect("test executable"))
                .args(["--exact", test, "--nocapture"])
                .env("FMETRICS_BOUNDARY_FIXTURE_CHILD", "1")
                .env("FMETRICS_BOUNDARY_FIXTURE_VERSION", version)
                .env("FMETRICS_BOUNDARY_FIXTURE_STATUS", status.to_string())
                .env("FMETRICS_LIB_PATH", dir.path())
                .env("FMETRICS_BOUNDARY_FIXTURE_LIBRARY", &library)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("child");
            let start = Instant::now();
            while child.try_wait().expect("status").is_none() {
                if start.elapsed() > Duration::from_secs(30) {
                    child.kill().expect("kill");
                    child.wait().expect("reap");
                    panic!("native status/version child timed out");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            let out = child.wait_with_output().expect("output");
            let stdout = String::from_utf8_lossy(&out.stdout);
            assert!(
                out.status.success(),
                "{version}/{status}: {stdout}\n{}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert!(
                stdout.contains("1 passed; 0 failed")
                    && stdout.contains("native status/version assertions completed"),
                "exact nonzero test count: {stdout}"
            );
        }
    }
}

#[test]
#[cfg(unix)]
fn nonpositive_butteraugli_norm_from_field_or_builder_rejects_before_probe_and_allocation() {
    result_case(
        "nonpositive_butteraugli_norm_from_field_or_builder_rejects_before_probe_and_allocation",
        "invalid",
    );
}

#[test]
#[cfg(unix)]
fn butteraugli_default_and_positive_norm_options_reach_native_sink_verbatim() {
    result_case(
        "butteraugli_default_and_positive_norm_options_reach_native_sink_verbatim",
        "options",
    );
}

#[test]
#[cfg(unix)]
fn nonfinite_butteraugli_result_reports_norm_and_returns_sole_workspace_lease() {
    result_case(
        "nonfinite_butteraugli_result_reports_norm_and_returns_sole_workspace_lease",
        "nonfinite",
    );
}

#[test]
#[cfg(unix)]
fn native_status_error_precedes_nonfinite_result_validation_and_returns_lease() {
    result_case(
        "native_status_error_precedes_nonfinite_result_validation_and_returns_lease",
        "status",
    );
}

#[cfg(unix)]
fn result_case(test: &str, scenario: &str) {
    if let Ok(selected) = std::env::var("FMETRICS_RESULT_FIXTURE_CHILD") {
        assert_eq!(selected, scenario, "case belongs to this contract");
        finite_options_child(scenario);
        println!("finite options {scenario}: assertions completed");
        return;
    }
    let dir = tempfile::tempdir().expect("fixture directory");
    let library = dir.path().join(if cfg!(target_os = "macos") {
        "libfmetrics.dylib"
    } else {
        "libfmetrics.so"
    });
    compile(&library, false, &[]);
    let out = Command::new(std::env::current_exe().expect("test executable"))
        .args(["--exact", test, "--nocapture"])
        .env("FMETRICS_RESULT_FIXTURE_CHILD", scenario)
        .env("FMETRICS_BOUNDARY_FIXTURE_VERSION", "0.1.0")
        .env("FMETRICS_BOUNDARY_FIXTURE_STATUS", "0")
        .env("FMETRICS_LIB_PATH", dir.path())
        .env("FMETRICS_BOUNDARY_FIXTURE_LIBRARY", &library)
        .env(
            "FMETRICS_RESULT_FIXTURE_RESULT_FILE",
            dir.path().join("result"),
        )
        .env("FMETRICS_RESULT_FIXTURE_ABI_LOG", dir.path().join("calls"))
        .output()
        .expect("isolated norm boundary");
    assert!(
        out.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("1 passed; 0 failed")
            && stdout.contains(&format!("finite options {scenario}: assertions completed")),
        "selected result contract ran: {stdout}"
    );
    eprint!("{}", String::from_utf8_lossy(&out.stderr));
}

#[cfg(unix)]
fn finite_options_child(scenario: &str) {
    // SAFETY: exact fixture counter signature; mapping outlives every call.
    let lib = unsafe {
        libloading::Library::new(
            std::env::var_os("FMETRICS_BOUNDARY_FIXTURE_LIBRARY").expect("library"),
        )
    }
    .expect("mapping");
    // SAFETY: fixture exports this C signature and accepts indices 0..=3.
    let counter = unsafe { lib.get::<unsafe extern "C" fn(u32) -> u32>(b"boundary_count\0") }
        .expect("counter");
    // SAFETY: all indices below are defined, and the library remains mapped.
    let count = |i| unsafe { counter(i) };
    let color = ColorInfo {
        matrix: av_metrics_fmetrics::YuvMatrix::Bt709,
        range: av_metrics_fmetrics::SampleRange::Limited,
        bit_depth: 8,
        transfer: av_decoders::TransferCharacteristic::IEC61966_2_1,
        colorspace: FmetricsColorspace::Srgb,
        hdr: false,
    };
    let result_file = std::env::var_os("FMETRICS_RESULT_FIXTURE_RESULT_FILE").expect("result file");
    let log = std::env::var_os("FMETRICS_RESULT_FIXTURE_ABI_LOG").expect("log");
    std::fs::write(&result_file, "42").expect("valid result");
    let config = FmetricsConfig::new(FmetricsMetric::Butteraugli).with_threads(1);
    for p in [0, -1, i32::MIN] {
        for direct in [false, true] {
            let mut invalid = config.clone();
            if direct {
                invalid.butteraugli_pnorm = Some(p);
            } else {
                invalid = invalid.with_pnorm(p);
            }
            let error = FmetricsScorer::new(invalid, color).expect_err("nonpositive exponent");
            assert!(
                matches!(error, FmetricsError::InvalidConfiguration { .. }),
                "{error}"
            );
            assert_eq!(
                (count(0), count(2), count(3)),
                (0, 0, 0),
                "reject before library probe/allocation/scoring"
            );
        }
    }
    if scenario == "invalid" {
        return;
    }
    let bytes = [128u8; 32 * 32 * 3];
    let image = RgbView::new(&bytes, 32, 32, false).expect("RGB");
    for p in [None, Some(3), Some(7), Some(i32::MAX)] {
        let mut config = config.clone().with_intensity_target(150.0);
        config.butteraugli_pnorm = p;
        let scorer = FmetricsScorer::new(config, color).expect("positive norm");
        assert_eq!(
            scorer.submit_rgb(&image, &image).expect("finite marker").butteraugli,
            Some(42.0),
            "finite exponent {p:?} must retain the native marker"
        );
        let log = std::fs::read_to_string(&log).expect("calls");
        assert!(
            log.ends_with(&format!("score {} 150\n", p.unwrap_or(3))),
            "{log}"
        );
    }
    if scenario == "options" {
        assert_eq!(
            (count(0), count(1), count(2)),
            (4, 4, 4),
            "finite option workspaces all destroyed"
        );
        return;
    }
    let scorer =
        FmetricsScorer::new(config.with_pnorm(i32::MAX), color).expect("large finite norm");
    for value in ["nan", "inf", "-inf"] {
        std::fs::write(&result_file, value).expect("nonfinite ABI result");
        let before = count(2);
        let error = scorer.submit_rgb(&image, &image).expect_err("nonfinite is not a score");
        assert!(
            matches!(
                error,
                FmetricsError::NonFiniteButteraugli {
                    pnorm: i32::MAX,
                    ..
                }
            ),
            "{error}"
        );
        assert!(error.to_string().contains("non-finite"), "{error}");
        assert_eq!(
            count(2),
            before + 1,
            "nonfinite {value} must reach native scoring exactly once"
        );
        std::fs::write(&result_file, "42").expect("valid ABI result");
        assert_eq!(
            scorer.submit_rgb(&image, &image).expect("lease returned on error").butteraugli,
            Some(42.0),
            "the sole workspace must be reusable after {value}"
        );
    }
    if scenario == "nonfinite" {
        drop(scorer);
        assert_eq!(
            (count(0), count(1), count(2)),
            (5, 5, 10),
            "nonfinite errors return every lease"
        );
        return;
    }
    for raw in [1, 777, -123] {
        std::fs::write(&result_file, format!("nan {raw}")).expect("raw native failure");
        let error = scorer
            .submit_rgb(&image, &image)
            .expect_err("status error precedes result validation");
        assert!(
            matches!(error, FmetricsError::CallFailed { code, .. } if code == raw),
            "{error}"
        );
        std::fs::write(&result_file, "42").expect("valid ABI result");
        assert_eq!(
            scorer
                .submit_rgb(&image, &image)
                .expect("lease returned on native failure")
                .butteraugli,
            Some(42.0),
            "the sole workspace must be reusable after native status {raw}"
        );
    }
    drop(scorer);
    assert_eq!(
        (count(0), count(1), count(2)),
        (5, 5, 16),
        "every workspace destroyed and every success/error path reached"
    );
    eprintln!(
        "finite options: 6 invalid preprobe rejects, 4 finite options, 3 nonfinite + 3 raw-status errors, 6 lease recoveries; workspaces=5/5 calls=16"
    );
}

#[cfg(unix)]
fn child() {
    // SAFETY: isolated fixture mapping stays alive through all counter calls.
    let lib = unsafe {
        libloading::Library::new(
            std::env::var_os("FMETRICS_BOUNDARY_FIXTURE_LIBRARY").expect("library"),
        )
    }
    .expect("mapping");
    // SAFETY: exact fixture counter signature and indices 0/1.
    let counter = unsafe { lib.get::<unsafe extern "C" fn(u32) -> u32>(b"boundary_count\0") }
        .expect("counter");
    // SAFETY: only defined indices 0/1 are used while the fixture stays mapped.
    let count = |index| unsafe { counter(index) };
    let color = ColorInfo {
        matrix: av_metrics_fmetrics::YuvMatrix::Bt709,
        range: av_metrics_fmetrics::SampleRange::Limited,
        bit_depth: 8,
        transfer: av_decoders::TransferCharacteristic::IEC61966_2_1,
        colorspace: FmetricsColorspace::Srgb,
        hdr: false,
    };
    let result = FmetricsScorer::new(FmetricsConfig::new(FmetricsMetric::Ssimulacra2), color);
    if std::env::var("FMETRICS_BOUNDARY_FIXTURE_VERSION").expect("version") != "0.1.0" {
        assert!(
            matches!(&result, Err(FmetricsError::IncompatibleLibrary { .. })),
            "reject incompatible ABI"
        );
        let error = result.expect_err("incompatible version").to_string();
        assert!(
            error.contains(&std::env::var("FMETRICS_BOUNDARY_FIXTURE_VERSION").expect("version"))
                && error.contains("0.1.0"),
            "observed version and actionable pinned build: {error}"
        );
        assert_eq!(count(0), 0, "reject before workspace allocation");
        return;
    }
    let scorer = result.expect("pinned ABI");
    let bytes = [128u8; 32 * 32 * 3];
    let image = RgbView::new(&bytes, 32, 32, false).expect("RGB");
    let result = scorer.submit_rgb(&image, &image);
    let raw: i32 = std::env::var("FMETRICS_BOUNDARY_FIXTURE_STATUS")
        .expect("status")
        .parse()
        .expect("integer");
    if raw == 0 {
        assert_eq!(
            result.expect("score").ssimulacra2,
            Some(42.0),
            "known success marker"
        );
    } else {
        let error = result.expect_err("native failure");
        assert!(
            matches!(&error, FmetricsError::CallFailed { code, .. } if *code == raw),
            "{error:?}"
        );
        if (1..=6).contains(&raw) {
            assert!(
                error.to_string().contains("known fmetrics status"),
                "retain native known-code formatting"
            );
        } else {
            assert!(
                error.to_string().contains(&raw.to_string()),
                "retain raw unknown code"
            );
        }
    }
    drop(scorer);
    assert_eq!(count(0), count(1), "all workspaces destroyed");
}

fn compile(path: &std::path::Path, probe: bool, flags: &[&str]) {
    let mut cmd = Command::new(std::env::var_os("CC").unwrap_or_else(|| "cc".into()));
    cmd.args(["-std=c11", "-Wall", "-Wextra", "-Werror"]);
    cmd.args(flags);
    if probe {
        cmd.arg("-DFMETRICS_BOUNDARY_FIXTURE_PROBE");
    } else {
        cmd.args(["-shared", "-fPIC"]);
    }
    let out = cmd
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/boundary.c"
        ))
        .arg("-o")
        .arg(path)
        .output()
        .expect("C compiler required");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn pinned_fmetrics_c_header_layouts_and_export_signatures_match_rust() {
    check_header_abi(false);
}

#[test]
fn short_enum_fmetrics_c_abi_is_rejected_by_same_layout_oracle() {
    check_header_abi(true);
}

fn check_header_abi(drift_check: bool) {
    let dir = tempfile::tempdir().expect("probe directory");
    let exe = dir.path().join(if cfg!(windows) { "probe.exe" } else { "probe" });
    compile(&exe, true, &[]);
    let out = Command::new(exe).output().expect("probe");
    assert!(out.status.success(), "C header probe exited successfully");
    let actual: Vec<usize> = String::from_utf8(out.stdout)
        .expect("ASCII")
        .split_whitespace()
        .map(|n| n.parse().expect("layout"))
        .collect();
    let mut expected = Vec::new();
    macro_rules! ty {
        ($t:ty) => {
            expected.extend([size_of::<$t>(), align_of::<$t>()]);
        };
    }
    macro_rules! field {
        ($t:ty, $f:ident, $ft:ty) => {
            expected.extend([offset_of!($t, $f), size_of::<$ft>()]);
        };
    }
    ty!(FmetricsErr);
    ty!(FmetricsPixFmt);
    ty!(FmetricsColorspace);
    ty!(FmetricsCvvdpDisplayModel);
    ty!(FmetricsImg);
    field!(FmetricsImg, data, *const std::ffi::c_void);
    field!(FmetricsImg, width, u32);
    field!(FmetricsImg, height, u32);
    field!(FmetricsImg, stride, u32);
    field!(FmetricsImg, format, FmetricsPixFmt);
    field!(FmetricsImg, colorspace, FmetricsColorspace);
    field!(FmetricsImg, hdr, bool);
    ty!(FmetricsButteraugliOptions);
    field!(FmetricsButteraugliOptions, intensity_target, f32);
    field!(FmetricsButteraugliOptions, pnorm, i32);
    ty!(FmetricsCvvdpDisplayParams);
    field!(FmetricsCvvdpDisplayParams, resolution_width, i32);
    field!(FmetricsCvvdpDisplayParams, resolution_height, i32);
    field!(FmetricsCvvdpDisplayParams, viewing_distance_meters, f32);
    field!(FmetricsCvvdpDisplayParams, diagonal_size_inches, f32);
    field!(FmetricsCvvdpDisplayParams, max_luminance, f32);
    field!(FmetricsCvvdpDisplayParams, contrast, f32);
    field!(FmetricsCvvdpDisplayParams, ambient_light, f32);
    field!(FmetricsCvvdpDisplayParams, reflectivity, f32);
    field!(FmetricsCvvdpDisplayParams, is_hdr, bool);
    ty!(FmetricsCvvdpResult);
    field!(FmetricsCvvdpResult, jod, f64);
    field!(FmetricsCvvdpResult, quality, f64);
    expected.extend([0, 1, 2, 3, 4, 5, 6, 1, 2, 3, 1, 2, 0, 27]);
    assert_eq!(actual, expected, "real pinned public C schema/signatures");
    eprintln!(
        "fmetrics ABI: {} real-header values, all 17 exported signatures",
        actual.len()
    );
    if !drift_check {
        return;
    }
    let drift = dir.path().join(if cfg!(windows) {
        "short-enums.exe"
    } else {
        "short-enums"
    });
    compile(&drift, true, &["-fshort-enums"]);
    let out = Command::new(drift).output().expect("ABI drift probe");
    assert!(out.status.success(), "C-only drift probe must run");
    let short: Vec<usize> = String::from_utf8(out.stdout)
        .expect("ASCII")
        .split_whitespace()
        .map(|n| n.parse().expect("layout"))
        .collect();
    assert_ne!(
        short, expected,
        "same oracle rejects nonstandard enum-width ABI"
    );
    assert_ne!(
        short[0],
        size_of::<FmetricsErr>(),
        "detect C status width drift"
    );
}
