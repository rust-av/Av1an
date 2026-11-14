//! Production Context ownership at an isolated C boundary, never real CUDA.
use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use super::*;

#[test]
fn cuda_context_success_releases_imported_resource_and_wrapper_once() {
    isolated_case(
        "cuda_context_success_releases_imported_resource_and_wrapper_once",
        0,
    );
}

#[test]
fn cuda_state_init_error_with_null_output_allocates_nothing() {
    isolated_case(
        "cuda_state_init_error_with_null_output_allocates_nothing",
        1,
    );
}

#[test]
fn cuda_state_init_error_with_empty_wrapper_releases_and_frees_it() {
    isolated_case(
        "cuda_state_init_error_with_empty_wrapper_releases_and_frees_it",
        2,
    );
}

#[test]
fn cuda_state_init_success_with_null_output_is_rejected() {
    isolated_case("cuda_state_init_success_with_null_output_is_rejected", 3);
}

#[test]
fn cuda_context_init_error_releases_original_state() {
    isolated_case("cuda_context_init_error_releases_original_state", 4);
}

#[test]
fn cuda_context_init_success_with_null_output_releases_original_state() {
    isolated_case(
        "cuda_context_init_success_with_null_output_releases_original_state",
        5,
    );
}

#[test]
fn cuda_import_error_closes_context_and_releases_original_state() {
    isolated_case(
        "cuda_import_error_closes_context_and_releases_original_state",
        6,
    );
}

#[test]
fn cuda_pool_error_closes_import_owner_and_frees_wrapper() {
    isolated_case("cuda_pool_error_closes_import_owner_and_frees_wrapper", 7);
}

#[test]
fn cuda_state_init_error_with_resource_releases_and_frees_it() {
    isolated_case(
        "cuda_state_init_error_with_resource_releases_and_frees_it",
        8,
    );
}

#[test]
fn missing_cuda_release_rejects_before_native_calls() {
    isolated_case("missing_cuda_release_rejects_before_native_calls", 9);
}

#[test]
fn missing_cuda_free_rejects_before_native_calls() {
    isolated_case("missing_cuda_free_rejects_before_native_calls", 10);
}

#[test]
fn cpu_only_without_cuda_symbols_uses_cpu_pool() {
    isolated_case("cpu_only_without_cuda_symbols_uses_cpu_pool", 11);
}

#[test]
fn auto_without_cuda_free_uses_cpu_pool() {
    isolated_case("auto_without_cuda_free_uses_cpu_pool", 12);
}

#[test]
fn require_cuda_without_free_reports_feature_or_symbol_error() {
    isolated_case(
        "require_cuda_without_free_reports_feature_or_symbol_error",
        13,
    );
}

#[test]
fn require_cuda_without_release_reports_feature_or_symbol_error() {
    isolated_case(
        "require_cuda_without_release_reports_feature_or_symbol_error",
        14,
    );
}

#[test]
fn auto_uses_cpu_when_cuda_is_disabled_or_state_initialization_fails() {
    isolated_case(
        "auto_uses_cpu_when_cuda_is_disabled_or_state_initialization_fails",
        15,
    );
}

#[test]
fn missing_cuda_state_init_rejects_before_native_calls() {
    isolated_case("missing_cuda_state_init_rejects_before_native_calls", 16);
}

#[test]
fn missing_cuda_import_rejects_before_native_calls() {
    isolated_case("missing_cuda_import_rejects_before_native_calls", 17);
}

#[test]
fn missing_cuda_preallocate_rejects_before_native_calls() {
    isolated_case("missing_cuda_preallocate_rejects_before_native_calls", 18);
}

fn isolated_case(test: &str, scenario: u32) {
    if std::env::var_os("VMAF_CUDA_FIXTURE_CHILD").is_some() {
        assert_eq!(
            std::env::var("VMAF_CUDA_FIXTURE_SCENARIO").expect("scenario"),
            scenario.to_string()
        );
        check_scenario();
        println!("CUDA fixture scenario {scenario}: assertions completed");
        return;
    }
    let module = module_path!().split_once("::").expect("test crate prefix").1;
    let exact = format!("{module}::{test}");
    let directory = tempfile::tempdir().expect("temporary CUDA ABI fixture");
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
            "/tests/fixtures/cuda.c"
        ))
        .output()
        .expect("CUDA ownership ABI tests require cc");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let mut child = Command::new(std::env::current_exe().expect("test executable"))
        .args(["--exact", &exact, "--nocapture"])
        .env("VMAF_CUDA_FIXTURE_CHILD", "1")
        .env("VMAF_CUDA_FIXTURE_SCENARIO", scenario.to_string())
        .env("VMAF_CUDA_FIXTURE_LIBRARY", &library)
        .env("VMAF_LIB_DIR", directory.path())
        .env_remove("VMAF_MODEL_PATH")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("isolated CUDA ABI child");
    let start = Instant::now();
    while child.try_wait().expect("child status").is_none() {
        if start.elapsed() > Duration::from_secs(30) {
            child.kill().expect("kill timed out child");
            child.wait().expect("reap timed out child");
            panic!("scenario {scenario}: child exceeded 30s");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().expect("child output");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "scenario {scenario}: {stdout}\n{stderr}"
    );
    assert!(
        stdout.contains("running 1 test")
            && stdout.contains("1 passed; 0 failed")
            && stdout.contains(&format!(
                "CUDA fixture scenario {scenario}: assertions completed"
            )),
        "exact filter must execute one test: {stdout}"
    );
    eprintln!("scenario {scenario}: {stdout}\n{stderr}");
}

fn check_scenario() {
    // SAFETY: our compiled fixture stays mapped through all counter reads.
    let library = unsafe {
        libloading::Library::new(
            std::env::var_os("VMAF_CUDA_FIXTURE_LIBRARY").expect("fixture path"),
        )
    }
    .expect("load fixture");
    // SAFETY: these are the exact fixture signatures; indices below are in range.
    let (counter, cleanup) = unsafe {
        (
            library
                .get::<unsafe extern "C" fn(u32) -> u32>(b"cuda_fixture_count\0")
                .expect("counter"),
            library
                .get::<unsafe extern "C" fn()>(b"cuda_fixture_cleanup\0")
                .expect("cleanup"),
        )
    };
    let scenario: u32 = std::env::var("VMAF_CUDA_FIXTURE_SCENARIO")
        .expect("scenario")
        .parse()
        .expect("number");
    static API: std::sync::OnceLock<VmafApi> = std::sync::OnceLock::new();
    let api = API.get_or_init(|| {
        let mut api = *VmafApi::load().expect("fixture API");
        // SAFETY: this callback frees the fixture's own malloc allocation. The
        // production guard must use this same field, not a test-only free path.
        api.cuda.free_state =
            Some(unsafe { *library.get(b"cuda_fixture_free\0").expect("C free counter") });
        match scenario {
            9 | 14 => api.cuda.release = None,
            10 | 12 | 13 => api.cuda.free_state = None,
            11 => api.cuda = crate::ffi::CudaSymbols::default(),
            16 => api.cuda.state_init = None,
            17 => api.cuda.import_state = None,
            18 => api.cuda.preallocate_pictures = None,
            _ => {},
        }
        api
    });
    let configuration = crate::ffi::VmafConfiguration::default();
    let geometry = Geometry {
        width: 8,
        height: 8,
        bit_depth: 8,
        pixel_format: crate::ffi::VmafPixelFormat::Yuv420p,
    };
    let result = match scenario {
        11 => initialise(api, configuration, BackendPreference::CpuOnly, geometry, Ok),
        12 | 15 => initialise(api, configuration, BackendPreference::Auto, geometry, Ok),
        13 | 14 => initialise(
            api,
            configuration,
            BackendPreference::RequireCuda,
            geometry,
            Ok,
        ),
        _ => Context::new_cuda(api, configuration, geometry),
    };
    let (backend, error) = match result {
        Ok(context) => {
            let backend = context.backend;
            drop(context);
            (Some(backend), None)
        },
        Err(error) => (None, Some(error)),
    };
    // SAFETY: fixture remains live and counter indices are defined in cuda.c.
    let snapshot: Vec<_> = (0..13).map(|index| unsafe { counter(index) }).collect();
    // SAFETY: snapshot is captured; fixture reclaims any RED-only leaked wrapper.
    unsafe { cleanup() };
    eprintln!("CUDA ownership counters: {snapshot:?}");
    // Literal expectations derived from upstream copy/release/malloc semantics.
    // Order: init_state, alloc, free, original_release, resource_release,
    // context_release, import, CUDA_pool, close, init_context, CPU_pool, duplicate, bad_call.
    let expected = match scenario {
        0 => [1, 1, 1, 0, 1, 1, 1, 1, 1, 1, 1, 0, 0],
        7 => [1, 1, 1, 0, 1, 1, 1, 1, 1, 1, 0, 0, 0],
        1 | 3 => [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        2 => [1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        4 | 5 => [1, 1, 1, 1, 1, 0, 0, 0, 0, 1, 0, 0, 0],
        6 => [1, 1, 1, 1, 1, 0, 1, 0, 1, 1, 0, 0, 0],
        8 => [1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0],
        11 | 12 => [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 0, 0],
        15 if !cfg!(feature = "cuda") => [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 0, 0],
        15 => [1, 1, 1, 1, 0, 0, 0, 0, 1, 1, 1, 0, 0],
        _ => [0; 13],
    };
    assert_eq!(
        snapshot, expected,
        "scenario {scenario}: native ownership counters"
    );
    match scenario {
        0 => assert_eq!(backend, Some(Backend::Cuda), "successful CUDA context"),
        11 | 12 | 15 => assert_eq!(backend, Some(Backend::Cpu), "CPU path remains usable"),
        1..=8 => {
            let (function, status) = match scenario {
                1 | 2 | 8 => ("vmaf_cuda_state_init", -81),
                3 => ("vmaf_cuda_state_init", 0),
                4 => ("vmaf_init", -82),
                5 => ("vmaf_init", 0),
                6 => ("vmaf_cuda_import_state", -83),
                7 => ("vmaf_cuda_preallocate_pictures", -84),
                _ => unreachable!(),
            };
            assert!(
                matches!(error, Some(VmafError::CallFailed { function: actual, status: code })
                if actual == function && code == status),
                "exact failure: {error:?}"
            );
        },
        13 | 14 if !cfg!(feature = "cuda") => assert!(
            matches!(&error, Some(VmafError::UnsupportedFeature(message))
                if message.contains("Cargo") && message.contains("--features cuda")),
            "feature rejection precedes missing native cleanup: {error:?}"
        ),
        9 | 10 | 13 | 14 => {
            let expected_symbol = if scenario == 9 || scenario == 14 {
                "vmaf_cuda_release"
            } else {
                "free"
            };
            assert!(
                matches!(&error, Some(VmafError::SymbolNotFound { symbol, message })
                if symbol == expected_symbol && message.contains("CUDA")),
                "actionable cleanup error: {error:?}"
            );
        },
        16..=18 => assert!(error.is_some(), "missing CUDA entry point rejected"),
        _ => unreachable!(),
    }
}
