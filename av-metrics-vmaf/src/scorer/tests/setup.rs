//! Full production constructor at a deterministic C boundary, without GPU claims.
use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use super::*;
use crate::config::{BackendPreference, VmafFeature};

const EXACT: &str = "scorer::tests::setup::complete_backend_attempts";

#[test]
fn auto_feature_policy() {
    check_feature_policy("scorer::tests::setup::auto_feature_policy", "auto");
}

#[test]
fn cpu_only_feature_policy() {
    check_feature_policy("scorer::tests::setup::cpu_only_feature_policy", "cpu");
}

#[test]
fn require_cuda_feature_policy() {
    check_feature_policy(
        "scorer::tests::setup::require_cuda_feature_policy",
        "require",
    );
}

fn check_feature_policy(exact: &str, preference: &str) {
    if std::env::var_os("VMAF_SETUP_FIXTURE_CHILD").is_some() {
        check_attempt();
        return;
    }
    let (directory, library) = setup_fixture();
    for stage in [0, 7, 9] {
        run_child(exact, stage, preference, &library, directory.path());
    }
}

#[test]
fn cuda_availability_requires_feature() {
    if std::env::var_os("VMAF_SETUP_FIXTURE_CHILD").is_none() {
        let (directory, library) = setup_fixture();
        run_child(
            "scorer::tests::setup::cuda_availability_requires_feature",
            0,
            "auto",
            &library,
            directory.path(),
        );
        return;
    }
    // SAFETY: this fixture remains mapped through all pointer and counter uses.
    let library = unsafe {
        libloading::Library::new(std::env::var_os("VMAF_SETUP_FIXTURE_LIBRARY").expect("fixture"))
    }
    .expect("fixture library");
    let mut api = *ffi::VmafApi::load().expect("fixture API");
    // SAFETY: exact allocator-matched fixture callback and counter signatures.
    let counter = unsafe {
        api.cuda.free_state = Some(*library.get(b"setup_free\0").expect("free"));
        library
            .get::<unsafe extern "C" fn(u32, u32) -> u32>(b"setup_count\0")
            .expect("count")
    };
    let api_available = api.cuda_available();
    let public_available = crate::cuda_available();
    // SAFETY: all indices are within the fixture's tables.
    let globals: Vec<_> = (0..7).map(|i| unsafe { counter(2, i) }).collect();
    eprintln!(
        "feature policy availability: api={api_available}, public={public_available}, globals={globals:?}"
    );
    assert_eq!(globals, [0; 7], "availability must not initialize CUDA");
    assert_eq!(
        api_available,
        cfg!(feature = "cuda"),
        "feature and native capability are both required"
    );
    assert_eq!(
        public_available,
        cfg!(all(feature = "cuda", target_os = "linux")),
        "public availability also requires verified allocator support"
    );
    api.cuda.release = None;
    assert!(
        !api.cuda_available(),
        "the feature cannot replace native cleanup capability"
    );
}

#[test]
fn required_cuda_feature_precedes_loader_failure() {
    if std::env::var_os("VMAF_SETUP_FIXTURE_CHILD").is_none() {
        let (directory, library) = setup_fixture();
        run_child(
            "scorer::tests::setup::required_cuda_feature_precedes_loader_failure",
            0,
            "require",
            &library,
            directory.path(),
        );
        return;
    }
    let loads = std::cell::Cell::new(0);
    let error = VmafScorer::new_with_loader(
        VmafConfig::new().with_backend(BackendPreference::RequireCuda),
        VideoFormat {
            width: 8,
            height: 8,
            bit_depth: 8,
            chroma_sampling: ChromaSubsampling::Yuv420,
        },
        || {
            loads.set(loads.get() + 1);
            Err(VmafError::LibraryLoadFailed {
                attempts: "feature policy injected missing library".to_owned(),
            })
        },
    )
    .err()
    .expect("constructor must fail");
    eprintln!("feature policy loader calls={}, error={error}", loads.get());
    if cfg!(feature = "cuda") {
        assert_eq!(
            loads.get(),
            1,
            "feature-enabled construction reaches the loader"
        );
        assert!(
            matches!(error, VmafError::LibraryLoadFailed { ref attempts } if attempts == "feature policy injected missing library"),
            "{error:?}"
        );
    } else {
        assert_eq!(
            loads.get(),
            0,
            "missing feature must win over missing library"
        );
        assert_missing_cuda_feature(&error);
    }
}

fn assert_missing_cuda_feature(error: &VmafError) {
    assert!(
        matches!(error, VmafError::UnsupportedFeature(_)),
        "{error:?}"
    );
    let message = error.to_string();
    assert!(
        message.contains("Cargo")
            && message.contains("cuda")
            && message.contains("--features cuda"),
        "actionable feature opt-in: {message}"
    );
}

#[test]
fn complete_backend_attempts() {
    if std::env::var_os("VMAF_SETUP_FIXTURE_CHILD").is_some() {
        check_attempt();
        return;
    }
    let (directory, library) = setup_fixture();
    // Start with a late CUDA failure: the old constructor cannot retry this.
    for stage in [7, 0, 1, 2, 3, 4, 5, 6, 8, 9, 10, 11, 12, 13, 14] {
        for preference in ["auto", "require", "cpu"] {
            run_child(EXACT, stage, preference, &library, directory.path());
        }
    }
}

fn setup_fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let directory = tempfile::tempdir().expect("constructor fixture");
    let library = directory.path().join(if cfg!(target_os = "macos") {
        "libvmaf.dylib"
    } else {
        "libvmaf.so"
    });
    let output = Command::new("cc")
        .args(["-shared", "-fPIC", "-pthread", "-std=c99", "-Wall", "-Wextra", "-Werror", "-o"])
        .arg(&library)
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/setup.c"
        ))
        .output()
        .expect("setup fixture requires cc");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // A real file satisfies path preflight; only the native parser is doubled.
    std::fs::write(directory.path().join("vmaf_v0.6.1.json"), b"{}").expect("model file");
    std::fs::create_dir_all(directory.path().join("model")).expect("adjacent model directory");
    std::fs::write(directory.path().join("model/vmaf_v0.6.1.json"), b"{}")
        .expect("stock fallback file");
    (directory, library)
}

fn run_child(
    exact: &str,
    stage: u32,
    preference: &str,
    library: &std::path::Path,
    directory: &std::path::Path,
) {
    let mut command = Command::new(std::env::current_exe().expect("test executable"));
    command
        .args(["--exact", exact, "--nocapture"])
        .env("VMAF_SETUP_FIXTURE_CHILD", "1")
        .env(
            "VMAF_PARTIAL_SETUP_FIXTURE_CHILD",
            if matches!(
                exact.rsplit("::").next(),
                Some("owned_partial_and_cpu_constructor_failures" | "blocking_pool_pressure")
            ) {
                "1"
            } else {
                "0"
            },
        )
        .env("VMAF_SETUP_FIXTURE_STAGE", stage.to_string())
        .env("VMAF_SETUP_FIXTURE_PREFERENCE", preference)
        .env("VMAF_SETUP_FIXTURE_LIBRARY", library)
        .env("VMAF_LIB_DIR", directory)
        .env("VMAF_MODEL_PATH", directory)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // These cases exercise built-in failure followed by file fallback. A model
    // directory override now intentionally bypasses that first loader entirely.
    if matches!(stage, 14 | 16) {
        command.env_remove("VMAF_MODEL_PATH");
    }
    let mut child = command.spawn().expect("isolated setup child");
    let start = Instant::now();
    while child.try_wait().expect("child status").is_none() {
        if start.elapsed() > Duration::from_secs(30) {
            child.kill().expect("kill child");
            child.wait().expect("reap child");
            panic!("{stage}/{preference}: timeout");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().expect("child output");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "{stage}/{preference}: {stdout}\n{stderr}"
    );
    assert!(
        stdout.contains("running 1 test") && stdout.contains("1 passed; 0 failed"),
        "{stdout}"
    );
    eprintln!("{stage}/{preference}: {stderr}");
}

fn check_attempt() {
    // SAFETY: this is our fixture, kept mapped through cleanup and counter reads.
    let library = unsafe {
        libloading::Library::new(std::env::var_os("VMAF_SETUP_FIXTURE_LIBRARY").expect("fixture"))
    }
    .expect("load fixture");
    // SAFETY: signatures/indices are defined by setup.c.
    let (counter, cleanup) = unsafe {
        (
            library
                .get::<unsafe extern "C" fn(u32, u32) -> u32>(b"setup_count\0")
                .expect("count"),
            library.get::<unsafe extern "C" fn()>(b"setup_cleanup\0").expect("cleanup"),
        )
    };
    // SAFETY: all indices below are within the fixture's tables.
    let count = |backend, index| unsafe { counter(backend, index) };
    let stage: u32 = std::env::var("VMAF_SETUP_FIXTURE_STAGE")
        .expect("stage")
        .parse()
        .expect("number");
    let preference =
        match std::env::var("VMAF_SETUP_FIXTURE_PREFERENCE").expect("preference").as_str() {
            "auto" => BackendPreference::Auto,
            "require" => BackendPreference::RequireCuda,
            "cpu" => BackendPreference::CpuOnly,
            _ => unreachable!(),
        };
    let model_path = std::path::PathBuf::from(std::env::var_os("VMAF_LIB_DIR").expect("fixture"))
        .join("vmaf_v0.6.1.json");
    let mut config = VmafConfig::new()
        .with_threads(2)
        .with_backend(preference)
        .with_feature(VmafFeature::Psnr);
    if stage != 14 {
        config = config.with_model_path(model_path);
    }
    let format = VideoFormat {
        width: 8,
        height: 8,
        bit_depth: 8,
        chroma_sampling: ChromaSubsampling::Yuv420,
    };
    static API: std::sync::OnceLock<ffi::VmafApi> = std::sync::OnceLock::new();
    let api = API.get_or_init(|| {
        let mut api = *ffi::VmafApi::load().expect("fixture API");
        // SAFETY: the fixture's allocator-matched wrapper deallocator counts
        // the production guard's calls. No public injection API is exposed.
        api.cuda.free_state = Some(unsafe { *library.get(b"setup_free\0").expect("free") });
        api
    });
    let result = VmafScorer::new_with_loader(config, format, || Ok(api));
    let feature_disabled = !cfg!(feature = "cuda");
    let rejected = feature_disabled && preference == BackendPreference::RequireCuda;
    let cuda_failed = !matches!(stage, 0 | 10 | 14);
    let cpu = !rejected
        && (preference == BackendPreference::CpuOnly
            || (preference == BackendPreference::Auto && (feature_disabled || cuda_failed)));
    let mut selected_backend = None;
    let mut scores = None;
    let mut feature_error = None;
    if rejected {
        feature_error = result.err();
    } else if (preference == BackendPreference::RequireCuda && cuda_failed) || stage == 11 {
        let error = result.err().expect("failed attempt must fail");
        let status = if cpu {
            -207
        } else if stage == 13 {
            -101
        } else if stage == 11 {
            -107
        } else {
            -(100 + stage as i32)
        };
        if stage == 6 {
            assert!(
                matches!(error, VmafError::ModelNotFound { ref reason, .. } if reason.contains("-106")),
                "{error:?}"
            );
        } else {
            let function = match stage {
                1 | 13 => "vmaf_cuda_state_init",
                2 => "vmaf_init",
                3 => "vmaf_cuda_import_state",
                4 => "vmaf_cuda_preallocate_pictures",
                5 => "vmaf_preallocate_pictures",
                7 | 11 => "vmaf_use_features_from_model",
                8 | 9 => "vmaf_fetch_preallocated_picture",
                12 => "vmaf_model_load_from_path",
                _ => unreachable!(),
            };
            assert!(
                matches!(error, VmafError::CallFailed { function: actual, status: code }
                if actual == function && code == if stage == 12 { 0 } else { status }),
                "{error:?}"
            );
        }
    } else {
        let mut scorer = result.expect("Auto must retry the entire failed CUDA attempt on CPU");
        selected_backend = Some(scorer.backend());
        assert_eq!(
            scorer.config().n_threads,
            2,
            "do not mutate caller configuration"
        );
        let reference = [11u8; 64];
        let distorted = [22u8; 64];
        assert_eq!(
            scorer
                .submit_pair_raw(planes(&reference), planes(&distorted))
                .expect("raw pair"),
            0,
            "first accepted index"
        );
        let frame = v_frame::frame::FrameBuilder::new(8, 8, ChromaSubsampling::Yuv420, 8)
            .build::<u8>()
            .expect("frame");
        assert_eq!(
            scorer.submit_pair(&frame, &frame).expect("typed pair"),
            1,
            "typed path shares the host pool"
        );
        scores = Some(scorer.finish().expect("flush"));
        drop(scorer);
    }
    let snapshot: Vec<Vec<_>> = (0..2).map(|b| (0..18).map(|i| count(b, i)).collect()).collect();
    let globals: Vec<_> = (0..7).map(|i| count(2, i)).collect();
    // SAFETY: all Rust handles have dropped. Reclaim tombstones even during RED.
    unsafe { cleanup() };
    eprintln!("setup CPU/CUDA {snapshot:?}, global {globals:?}");
    if rejected {
        assert_missing_cuda_feature(
            &feature_error.expect("RequireCuda must reject the missing feature"),
        );
    } else if stage != 11 && !(preference == BackendPreference::RequireCuda && cuda_failed) {
        assert_eq!(
            selected_backend,
            Some(if cpu { Backend::Cpu } else { Backend::Cuda }),
            "only a complete opted-in backend may be selected"
        );
        assert_eq!(
            scores,
            Some(if cpu { vec![81.0; 2] } else { vec![91.0; 2] }),
            "both boundary markers survive flush (not metric accuracy)"
        );
    }
    assert_eq!(
        globals[6], 0,
        "native boundary/cleanup/order/canary violation"
    );
    assert_eq!(
        count(0, 0),
        u32::from(cpu),
        "exactly one fresh CPU attempt or none"
    );
    // Literal native-boundary expectations, not counts calculated by the builder.
    // init, alloc, close, import, device, host, load, model alloc/destroy,
    // required, fetch, checkout, return, transfer, read, flush, optional, threads.
    let expected_cuda = if feature_disabled || preference == BackendPreference::CpuOnly {
        [0; 18]
    } else {
        match stage {
            1 | 13 => [0; 18],
            2 => [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            3 => [1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            4 => [1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            5 => [1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            6 => [1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            12 => [1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            7 | 11 => [1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0],
            8 => [1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 1, 0],
            9 => [1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 1, 1, 0, 0, 0, 1, 0],
            14 => [1, 1, 1, 1, 1, 1, 2, 2, 2, 1, 6, 6, 2, 4, 2, 1, 1, 0],
            0 | 10 => [1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 6, 6, 2, 4, 2, 1, 1, 0],
            _ => unreachable!(),
        }
    };
    assert_eq!(
        snapshot[1], expected_cuda,
        "CUDA attempt stops at the failing stage"
    );
    let mut expected_cpu = if !cpu {
        [0; 18]
    } else if stage == 11 {
        [1, 1, 1, 0, 0, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 2]
    } else {
        [1, 1, 1, 0, 0, 1, 1, 1, 1, 1, 6, 6, 2, 4, 2, 1, 1, 2]
    };
    if cpu && stage == 14 {
        expected_cpu[6] = 2;
        expected_cpu[7] = 2;
        expected_cpu[8] = 2;
    }
    assert_eq!(
        snapshot[0], expected_cpu,
        "fresh complete CPU attempt, or exact failure"
    );
    // state init, wrapper alloc/free, original/context/resource release, bad.
    let expected_global = if feature_disabled || preference == BackendPreference::CpuOnly {
        [0; 7]
    } else {
        match stage {
            1 => [1, 0, 0, 0, 0, 0, 0],
            13 => [1, 1, 1, 1, 0, 0, 0],
            2 | 3 => [1, 1, 1, 1, 0, 1, 0],
            _ => [1, 1, 1, 0, 1, 1, 0],
        }
    };
    assert_eq!(
        globals, expected_global,
        "one release owner and one wrapper free"
    );
}

#[test]
fn owned_partial_and_cpu_constructor_failures() {
    if std::env::var_os("VMAF_SETUP_FIXTURE_CHILD").is_some() {
        check_partial_setup_attempt();
        return;
    }
    let (directory, library) = setup_fixture();
    for stage in [6, 7, 8, 9, 12, 15, 14, 16] {
        run_child(
            "scorer::tests::setup::owned_partial_and_cpu_constructor_failures",
            stage,
            "cpu",
            &library,
            directory.path(),
        );
    }
    for stage in [6, 16] {
        run_child(
            "scorer::tests::setup::owned_partial_and_cpu_constructor_failures",
            stage,
            "auto",
            &library,
            directory.path(),
        );
    }
}

#[test]
fn blocking_pool_pressure() {
    if std::env::var_os("VMAF_SETUP_FIXTURE_CHILD").is_some() {
        check_partial_setup_attempt();
        return;
    }
    let (directory, library) = setup_fixture();
    for stage in [17, 18] {
        run_child(
            "scorer::tests::setup::blocking_pool_pressure",
            stage,
            "cpu",
            &library,
            directory.path(),
        );
    }
}

fn check_partial_setup_attempt() {
    // SAFETY: fixture ABI, kept mapped until all scorers and helper threads drop.
    let library = unsafe {
        libloading::Library::new(std::env::var_os("VMAF_SETUP_FIXTURE_LIBRARY").expect("fixture"))
    }
    .expect("load fixture");
    let stage: u32 = std::env::var("VMAF_SETUP_FIXTURE_STAGE")
        .expect("stage")
        .parse()
        .expect("number");
    let auto = std::env::var("VMAF_SETUP_FIXTURE_PREFERENCE").expect("preference") == "auto";
    // SAFETY: signatures match setup.c; copied pointers stay mapped below.
    let (counter, cleanup, pressure_count, release_reserved) = unsafe {
        (
            *library
                .get::<unsafe extern "C" fn(u32, u32) -> u32>(b"setup_count\0")
                .expect("count"),
            *library.get::<unsafe extern "C" fn()>(b"setup_cleanup\0").expect("cleanup"),
            *library
                .get::<unsafe extern "C" fn(u32) -> u32>(b"partial_setup_pressure_count\0")
                .expect("pressure"),
            *library
                .get::<unsafe extern "C" fn()>(b"partial_setup_release_reserved\0")
                .expect("release reserved"),
        )
    };
    static API: std::sync::OnceLock<ffi::VmafApi> = std::sync::OnceLock::new();
    let api = API.get_or_init(|| {
        let mut api = *ffi::VmafApi::load().expect("fixture API");
        // SAFETY: allocator-matched fixture deallocator, no public injection API.
        api.cuda.free_state = Some(unsafe { *library.get(b"setup_free\0").expect("free") });
        api
    });
    let model = std::path::PathBuf::from(std::env::var_os("VMAF_LIB_DIR").expect("fixture"))
        .join("vmaf_v0.6.1.json");
    let mut config = VmafConfig::new().with_threads(2).with_backend(if auto {
        BackendPreference::Auto
    } else {
        BackendPreference::CpuOnly
    });
    if !matches!(stage, 14 | 16) {
        config = config.with_model_path(&model);
    }
    let helper = matches!(stage, 17 | 18).then(|| {
        std::thread::spawn(move || {
            // SAFETY: waits for the actual blocked second fetch before releasing slots.
            unsafe { release_reserved() };
        })
    });
    let result = VmafScorer::new_with_loader(
        config,
        VideoFormat {
            width: 8,
            height: 8,
            bit_depth: 8,
            chroma_sampling: ChromaSubsampling::Yuv420,
        },
        || Ok(api),
    );
    if let Some(helper) = helper {
        helper.join().expect("pool helper");
    }
    // Read/clean even on an unexpected result, so RED never abandons tombstones.
    let (error, selected_backend) = match result {
        Ok(scorer) => {
            let backend = scorer.backend();
            drop(scorer);
            (None, Some(backend))
        },
        Err(error) => (Some(error), None),
    };
    // SAFETY: bounded fixture indices, all owners and helper threads are finished.
    let (snapshot, globals, pressure) = unsafe {
        let snapshot: Vec<Vec<_>> =
            (0..2).map(|b| (0..18).map(|i| counter(b, i)).collect()).collect();
        let globals: Vec<_> = (0..7).map(|i| counter(2, i)).collect();
        let pressure: Vec<_> = (0..3).map(|i| pressure_count(i)).collect();
        cleanup();
        (snapshot, globals, pressure)
    };
    eprintln!("partial setup CPU/CUDA {snapshot:?}, global {globals:?}, pressure {pressure:?}");
    match stage {
        6 | 15 | 16 if !auto || !cfg!(feature = "cuda") => {
            let Some(VmafError::ModelNotFound {
                model: actual,
                reason,
            }) = error.as_ref()
            else {
                panic!("original model/file-fallback error: {error:?}");
            };
            if stage == 16 {
                // Installed stock files precede the fixture's adjacent fallback.
                assert!(
                    std::path::Path::new(actual).is_file() && actual.ends_with("vmaf_v0.6.1.json"),
                    "file fallback reports the selected stock JSON: {actual}"
                );
            } else {
                assert_eq!(actual, &model.display().to_string(), "original model path");
            }
            assert!(
                reason.contains(if stage == 15 { "-115" } else { "-106" }),
                "original loader status: {reason}"
            );
        },
        7 => assert!(
            matches!(
                error,
                Some(VmafError::CallFailed {
                    function: "vmaf_use_features_from_model",
                    status: -107
                })
            ),
            "required feature error: {error:?}"
        ),
        8 | 9 | 18 => assert!(
            matches!(error, Some(VmafError::CallFailed { function: "vmaf_fetch_preallocated_picture", status }) if status == match stage { 8 => -108, 9 => -109, _ => -12 }),
            "fetch error: {error:?}"
        ),
        12 => assert!(
            matches!(
                error,
                Some(VmafError::CallFailed {
                    function: "vmaf_model_load_from_path",
                    status: 0
                })
            ),
            "success-null error: {error:?}"
        ),
        _ => {
            assert!(error.is_none(), "constructor must complete: {error:?}");
            assert_eq!(selected_backend, Some(Backend::Cpu), "fresh CPU scorer");
        },
    }
    // init, alloc, close, import, device, host, load, model alloc/destroy,
    // required, fetch, checkout, return, transfer, read, flush, optional, threads.
    let expected_cpu = match stage {
        6 | 16 if auto && cfg!(feature = "cuda") => {
            if stage == 6 {
                [1, 1, 1, 0, 0, 1, 1, 1, 1, 1, 2, 2, 2, 0, 0, 0, 0, 2]
            } else {
                [1, 1, 1, 0, 0, 1, 2, 2, 2, 1, 2, 2, 2, 0, 0, 0, 0, 2]
            }
        },
        6 => [1, 1, 1, 0, 0, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 2],
        7 => [1, 1, 1, 0, 0, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 2],
        8 => [1, 1, 1, 0, 0, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 2],
        9 | 18 => [1, 1, 1, 0, 0, 1, 1, 1, 1, 1, 2, 1, 1, 0, 0, 0, 0, 2],
        12 | 15 => [1, 1, 1, 0, 0, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2],
        14 => [1, 1, 1, 0, 0, 1, 2, 2, 2, 1, 2, 2, 2, 0, 0, 0, 0, 2],
        16 => [1, 1, 1, 0, 0, 1, 2, 2, 2, 0, 0, 0, 0, 0, 0, 0, 0, 2],
        17 => [1, 1, 1, 0, 0, 1, 1, 1, 1, 1, 2, 2, 2, 0, 0, 0, 0, 2],
        _ => unreachable!(),
    };
    assert_eq!(
        snapshot[0], expected_cpu,
        "all CPU resources balanced before close"
    );
    let expected_cuda = if !auto || !cfg!(feature = "cuda") {
        [0; 18]
    } else if stage == 6 {
        [1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0]
    } else {
        [1, 1, 1, 1, 1, 1, 2, 2, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0]
    };
    assert_eq!(
        snapshot[1], expected_cuda,
        "all failed CUDA partials destroyed before CPU retry"
    );
    assert_eq!(
        globals,
        if auto && cfg!(feature = "cuda") {
            [1, 1, 1, 0, 1, 1, 0]
        } else {
            [0; 7]
        },
        "no invalid/double/late cleanup; correct order"
    );
    assert_eq!(
        pressure,
        if matches!(stage, 17 | 18) {
            [1, 1, 5]
        } else {
            [0; 3]
        },
        "actual blocked second fetch released by joined helper"
    );
}

fn planes(bytes: &[u8]) -> PlaneSet<'_> {
    [(8, 8), (4, 4), (4, 4)]
        .map(|(w, h)| PlaneSource::from_bytes(bytes, w, h, w as isize, 0, 1).expect("planes"))
}
