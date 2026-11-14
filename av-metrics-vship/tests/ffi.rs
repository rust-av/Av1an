//! Real-header ABI sinks; CPU backend is a valid tag, not a GPU accuracy claim.
use av_metrics_vship::ffi::*;

/// A machine without libvship must be reported as unavailable rather than
/// panicking, which is what keeps CI green with no GPU and no binary.
#[test]
fn loading_is_total_and_never_panics() {
    let outcome = VshipApi::load();
    match outcome {
        Ok(api) => assert!(!api.version_string().is_empty()),
        Err(error) => assert!(!error.to_string().is_empty()),
    }
}

#[test]
fn status_check_accepts_zero_and_preserves_bad_handler_failure() {
    assert!(
        check(
            VshipException::NoError as std::ffi::c_int,
            "Vship_InitHandler"
        )
        .is_ok()
    );
    assert!(matches!(
        check(
            VshipException::BadHandler as std::ffi::c_int,
            "Vship_InitHandler"
        ),
        Err(av_metrics_vship::VshipError::CallFailed {
            function: "Vship_InitHandler",
            status: 10,
            ..
        })
    ));
}
#[cfg(unix)]
use av_metrics_vship::{VideoFormat, VshipConfig, VshipError, VshipMetric, VshipScorer};
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
use v_frame::{chroma::ChromaSubsampling, frame::FrameBuilder};

#[cfg(unix)]
mod common;

#[cfg(unix)]
#[path = "../../test-support/runtime_discovery.rs"]
mod runtime_discovery;

/// Catches repeated default probes, check-then-store races, cached failures,
/// skipped explicit admission, and routing a selected device to the wrong init.
#[test]
#[cfg(unix)]
fn default_capability_cache() {
    if let Ok(scenario) = std::env::var("VSHIP_CAPABILITY_FIXTURE_CHILD") {
        capability_child(&scenario);
        return;
    }
    let dir = tempfile::tempdir().expect("capability fixture directory");
    let library = dir.path().join(if cfg!(target_os = "macos") {
        "libvship.dylib"
    } else {
        "libvship.so"
    });
    compile(&library, false, &[]);
    let mut errors = Vec::new();
    for scenario in ["mixed", "constructor-first", "concurrent", "all-integrated", "retry"] {
        let mut child = Command::new(std::env::current_exe().expect("executable"))
            .args(["--exact", "default_capability_cache", "--nocapture"])
            .env("VSHIP_CAPABILITY_FIXTURE_CHILD", scenario)
            .env(
                "VSHIP_CAPABILITY_FIXTURE_DEVICES",
                if scenario == "all-integrated" {
                    "2"
                } else {
                    "1"
                },
            )
            .env("VSHIP_BOUNDARY_FIXTURE_LIBRARY", &library)
            .env("VSHIP_BOUNDARY_FIXTURE_BACKEND", "3")
            .env("VSHIP_BOUNDARY_FIXTURE_PATCH", "2")
            .env(
                "VSHIP_BOUNDARY_FIXTURE_PROBE_STATUS",
                if scenario == "retry" { "777" } else { "0" },
            )
            .env("VSHIP_BOUNDARY_FIXTURE_COMPUTE_STATUS", "0")
            .env("VSHIP_HANDLER_FIXTURE_INIT_STATUS", "0")
            .env_remove("VSHIP_BOUNDARY_FIXTURE_TAG")
            .env("VSHIP_PLUGIN_PATH", dir.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("capability child");
        let start = Instant::now();
        while child.try_wait().expect("status").is_none() {
            if start.elapsed() > Duration::from_secs(30) {
                child.kill().expect("kill");
                child.wait().expect("reap");
                panic!("capability cache timed out: {scenario}");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let out = child.wait_with_output().expect("output");
        let stdout = String::from_utf8_lossy(&out.stdout);
        if !out.status.success() || !stdout.contains("1 passed; 0 failed") {
            errors.push(format!(
                "{scenario}: {stdout}\n{}",
                String::from_utf8_lossy(&out.stderr)
            ));
        } else {
            print!("{stdout}");
        }
    }
    assert!(errors.is_empty(), "{}", errors.join("\n"));
}

#[cfg(unix)]
fn capability_child(scenario: &str) {
    // SAFETY: exact fixture signatures; the mapping outlives calls and threads.
    let lib = unsafe {
        libloading::Library::new(
            std::env::var_os("VSHIP_BOUNDARY_FIXTURE_LIBRARY").expect("library"),
        )
    }
    .expect("mapping");
    // SAFETY: these exports are defined in boundary.c.
    let (counter, gpu_counter, control) = unsafe {
        (
            lib.get::<unsafe extern "C" fn(u32) -> u32>(b"boundary_count\0")
                .expect("counter"),
            lib.get::<unsafe extern "C" fn(u32, u32) -> u32>(b"capability_gpu_count\0")
                .expect("GPU counter"),
            lib.get::<unsafe extern "C" fn(u32, i32)>(b"handler_set\0").expect("control"),
        )
    };
    // SAFETY: bounded indices, live mapping; counters are C11 atomics.
    let count = |i| unsafe { counter(i) };
    // SAFETY: only kinds 0..2 and devices 0..4 are used.
    let gpu_count = |kind, gpu| unsafe { gpu_counter(kind, gpu) };
    // SAFETY: defined controls; mutations happen only outside concurrent calls.
    let set = |i, value| unsafe { control(i, value) };
    let format = VideoFormat {
        width: 32,
        height: 32,
        bit_depth: 8,
        chroma_sampling: ChromaSubsampling::Yuv420,
        color_range: None,
        color_primaries: None,
        color_transfer_characteristic: None,
        color_space: None,
        chroma_location: None,
    };
    let new = |config| VshipScorer::new(config, format, format, None);
    if scenario == "retry" {
        assert!(
            !av_metrics_vship::is_available(),
            "failed first probe is unavailable"
        );
        assert!(
            VshipScorer::libvship_version().is_none(),
            "failed default probe hides version"
        );
        assert!(
            matches!(new(VshipConfig::new()), Err(VshipError::CallFailed {
            function: "Vship_GPUFullCheck", status: 777, message
        }) if message == "libvship error 777"),
            "default retry retains native function/status/message"
        );
        assert_eq!(
            (count(0), count(1), count(2)),
            (3, 0, 0),
            "failed probes retry without handlers"
        );
        set(2, 0);
    }
    if scenario == "constructor-first" {
        let scorer = new(VshipConfig::new()).expect("first default constructor");
        assert_eq!(
            scorer.gpu_id(),
            1,
            "constructor resolves the discrete default"
        );
        drop(scorer);
        assert_eq!(
            (count(0), count(1), count(2)),
            (1, 1, 1),
            "first constructor probes and owns one handler"
        );
    }
    let versions = if scenario == "concurrent" {
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(12));
        let mut threads = Vec::new();
        for i in 0..12 {
            let barrier = std::sync::Arc::clone(&barrier);
            threads.push(std::thread::spawn(move || {
                barrier.wait();
                if i % 2 == 0 {
                    assert!(
                        av_metrics_vship::is_available(),
                        "concurrent free availability"
                    );
                    VshipScorer::libvship_version().expect("concurrent version")
                } else {
                    assert!(
                        VshipScorer::is_available(),
                        "concurrent associated availability"
                    );
                    av_metrics_vship::libvship_version().expect("concurrent version")
                }
            }));
        }
        threads
            .into_iter()
            .map(|thread| thread.join().expect("joined"))
            .collect::<Vec<_>>()
    } else {
        (0..4)
            .map(|i| {
                if i % 2 == 0 {
                    assert!(av_metrics_vship::is_available(), "free availability");
                    VshipScorer::libvship_version().expect("version")
                } else {
                    assert!(VshipScorer::is_available(), "associated availability");
                    av_metrics_vship::libvship_version().expect("version")
                }
            })
            .collect::<Vec<_>>()
    };
    for version in &versions {
        assert_eq!(*version, "5.1.2", "pinned fixture version");
        assert_eq!(
            version.as_ptr(),
            versions[0].as_ptr(),
            "static borrowed version identity"
        );
    }
    let expected_probes = if scenario == "retry" { 4 } else { 1 };
    assert_eq!(
        count(0),
        expected_probes,
        "one successful default probe, including concurrent first requests"
    );
    let chosen = if scenario == "all-integrated" { 2 } else { 1 };
    assert_eq!(
        av_metrics_vship::default_gpu_id(),
        Some(chosen),
        "discrete preference or last integrated device"
    );
    assert_eq!(
        gpu_count(0, chosen),
        expected_probes,
        "probe the chosen index, never assumed zero"
    );
    assert_eq!(
        gpu_count(0, 0),
        0,
        "default never probes device zero in the multi-GPU fixture"
    );
    assert_eq!(
        (count(6), count(7)),
        (1, if chosen == 2 { 3 } else { 2 }),
        "selection enumerated only once"
    );
    assert_eq!(
        count(1),
        if scenario == "constructor-first" {
            1
        } else {
            0
        },
        "availability creates no handlers"
    );
    println!(
        "capability cache {scenario}: default={chosen}, probes={}, enumeration/info={}/{}, borrowed version={:p}",
        count(0),
        count(6),
        count(7),
        versions[0].as_ptr()
    );
    if scenario == "all-integrated" {
        let scorer = new(VshipConfig::new()).expect("last integrated default constructor");
        assert_eq!(scorer.gpu_id(), 2, "last integrated device is the default");
        drop(scorer);
        assert_eq!(
            (gpu_count(0, 2), gpu_count(1, 2), count(2)),
            (1, 1, 1),
            "default two reaches init, reuses probe, and frees its handler"
        );
    }
    if scenario != "mixed" {
        return;
    }

    for metric in VshipMetric::all() {
        let config = VshipConfig::new().with_metric(metric);
        let probes = count(0);
        let scorer = new(config.clone()).expect("cached default constructor");
        assert_eq!(
            scorer.gpu_id(),
            1,
            "default constructor keeps chosen device"
        );
        drop(scorer);
        assert_eq!(
            count(0),
            probes,
            "default constructor reuses capability success"
        );
        for gpu in [0, 2] {
            let before = (gpu_count(0, gpu), gpu_count(1, gpu));
            let scorer = new(config.clone().with_gpu_id(gpu)).expect("explicit constructor");
            assert_eq!(
                scorer.gpu_id(),
                gpu,
                "explicit constructor keeps requested device"
            );
            drop(scorer);
            assert_eq!(
                (gpu_count(0, gpu), gpu_count(1, gpu)),
                (before.0 + 1, before.1 + 1),
                "{metric:?}: requested device reaches both FullCheck and Init"
            );
        }
    }
    drop(new(VshipConfig::new().with_gpu_id(1)).expect("explicit default also validates"));
    assert_eq!(
        (gpu_count(0, 0), gpu_count(0, 1), gpu_count(0, 2)),
        (3, 2, 3),
        "every explicit constructor probes, including explicit default index"
    );
    assert_eq!(
        (gpu_count(1, 0), gpu_count(1, 1), gpu_count(1, 2)),
        (3, 4, 3),
        "FullCheck and Init both receive selected IDs for every metric"
    );
    assert_eq!(
        (count(1), count(2)),
        (10, 10),
        "fresh owned handler per constructor"
    );
    assert!(
        matches!(new(VshipConfig::new().with_gpu_id(3)), Err(VshipError::CallFailed {
        function: "Vship_GPUFullCheck", status: 8, message
    }) if message == "known vship status"),
        "invalid explicit device retains native function/status/message"
    );
    assert_eq!(
        (gpu_count(0, 3), gpu_count(1, 3), count(1)),
        (1, 0, 10),
        "invalid explicit device never allocates or falls back"
    );
    set(2, 777);
    assert!(
        matches!(new(VshipConfig::new().with_gpu_id(0)), Err(VshipError::CallFailed {
        function: "Vship_GPUFullCheck", status: 777, message
    }) if message == "libvship error 777"),
        "explicit probe failure retains native function/status/message"
    );
    assert_eq!(count(1), 10, "failed explicit probe creates no handler");
    assert!(
        av_metrics_vship::is_available(),
        "explicit failure cannot poison default success"
    );
    set(2, 0);
    drop(new(VshipConfig::new().with_gpu_id(0)).expect("explicit recovery"));
    let probes = count(0);
    set(1, 777);
    assert!(
        matches!(new(VshipConfig::new()), Err(VshipError::MetricUnavailable { reason, .. })
        if reason.contains("777") && reason.contains("Vship_InitHandler")),
        "cached admission does not hide init failure"
    );
    assert!(
        VshipScorer::is_available(),
        "init failure cannot poison default success"
    );
    set(1, 0);
    let frame = FrameBuilder::new(32, 32, ChromaSubsampling::Yuv420, 8)
        .build::<u8>()
        .expect("frame");
    let mut scorer = new(VshipConfig::new()).expect("init recovery");
    set(3, 777);
    assert!(
        matches!(scorer.submit_pair(&frame, &frame), Err(VshipError::CallFailed {
        function: "Vship_ComputeHandler", status: 777, message
    }) if message == "libvship error 777"),
        "cached admission does not hide compute function/status/message"
    );
    assert!(
        av_metrics_vship::is_available(),
        "compute failure cannot poison default success"
    );
    set(3, 0);
    assert_eq!(
        scorer.submit_pair(&frame, &frame).expect("compute recovery").score,
        Some(42.0),
        "compute recovers on the owned handler"
    );
    drop(scorer);
    drop(new(VshipConfig::new()).expect("later default constructor"));
    assert_eq!(
        count(0),
        probes,
        "init/compute failures neither clear nor re-probe cached capability"
    );
    assert_eq!(
        (count(1), count(2)),
        (14, 13),
        "only failed init has no owned output to free"
    );
    assert_eq!(
        av_metrics_vship::libvship_version().expect("version").as_ptr(),
        versions[0].as_ptr(),
        "native scoring failures retain the borrowed version"
    );
    println!(
        "capability cache mixed: explicit FullCheck/Init IDs checked; status 8/777, probe/init/compute recovery; probes={}, init/free={}/{}, compute={}",
        count(0),
        count(1),
        count(2),
        count(3)
    );
}

#[test]
#[cfg(unix)]
fn runtime_overrides_and_staged_release() {
    if std::env::var_os("RUNTIME_DISCOVERY_CHILD").is_some() {
        // Load through production discovery before mapping fixture counters.
        assert_eq!(
            av_metrics_vship::libvship_version(),
            Some("5.1.2"),
            "production discovery selects the pinned runtime"
        );
        child();
        return;
    }
    runtime_discovery::run(
        "runtime_overrides_and_staged_release",
        if cfg!(target_os = "macos") {
            "libvship.dylib"
        } else {
            "libvship.so"
        },
        &["VSHIP_PLUGIN_PATH", "VSHIP_LIB_DIR", "bundled"],
        true,
        |library| compile(library, false, &[]),
        |command, library| {
            command
                .env("VSHIP_BOUNDARY_FIXTURE_BACKEND", "3")
                .env("VSHIP_BOUNDARY_FIXTURE_PATCH", "2")
                .env("VSHIP_BOUNDARY_FIXTURE_PROBE_STATUS", "0")
                .env("VSHIP_BOUNDARY_FIXTURE_COMPUTE_STATUS", "0")
                .env("VSHIP_BOUNDARY_FIXTURE_LIBRARY", library);
        },
    );
}

/// Catches per-request version clones/leaks while retaining fresh explicit admission.
#[test]
#[cfg(unix)]
fn version_borrows_cached_storage() {
    if std::env::var_os("VSHIP_VERSION_FIXTURE_CHILD").is_some() {
        version_child();
        return;
    }
    let dir = tempfile::tempdir().expect("version fixture directory");
    let library = dir.path().join(if cfg!(target_os = "macos") {
        "libvship.dylib"
    } else {
        "libvship.so"
    });
    compile(&library, false, &[]);
    let mut child = Command::new(std::env::current_exe().expect("executable"))
        .args(["--exact", "version_borrows_cached_storage", "--nocapture"])
        .env("VSHIP_VERSION_FIXTURE_CHILD", "1")
        .env("VSHIP_BOUNDARY_FIXTURE_LIBRARY", &library)
        .env("VSHIP_BOUNDARY_FIXTURE_BACKEND", "3")
        .env("VSHIP_BOUNDARY_FIXTURE_PATCH", "2")
        .env("VSHIP_BOUNDARY_FIXTURE_PROBE_STATUS", "0")
        .env("VSHIP_PLUGIN_PATH", dir.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("version child");
    let start = Instant::now();
    while child.try_wait().expect("status").is_none() {
        if start.elapsed() > Duration::from_secs(30) {
            child.kill().expect("kill");
            child.wait().expect("reap");
            panic!("version storage timed out");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let out = child.wait_with_output().expect("output");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && stdout.contains("1 passed; 0 failed"),
        "{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    print!("{stdout}");
}

#[cfg(unix)]
fn version_child() {
    // SAFETY: exact fixture exports; mapping outlives all calls and reads.
    let lib = unsafe {
        libloading::Library::new(
            std::env::var_os("VSHIP_BOUNDARY_FIXTURE_LIBRARY").expect("library"),
        )
    }
    .expect("mapping");
    // SAFETY: these signatures are defined in boundary.c.
    let (counter, control) = unsafe {
        (
            lib.get::<unsafe extern "C" fn(u32) -> u32>(b"boundary_count\0")
                .expect("counter"),
            lib.get::<unsafe extern "C" fn(u32, i32)>(b"handler_set\0").expect("control"),
        )
    };
    // SAFETY: only defined counter indices 0..3 are used while mapped.
    let count = |i| unsafe { counter(i) };
    let mut versions = Vec::new();
    for request in 1..=4 {
        let version = if request % 2 == 0 {
            VshipScorer::libvship_version()
        } else {
            av_metrics_vship::libvship_version()
        }
        .expect("fixture version requests must not skip");
        assert_eq!(version, "5.1.2", "pinned fixture version");
        versions.push(version);
        assert_eq!(count(0), 1, "default requests share successful admission");
    }
    println!(
        "version storage version=5.1.2 pointers={:?}; probes={}",
        versions.iter().map(|version| version.as_ptr()).collect::<Vec<_>>(),
        count(0)
    );
    for version in &versions[1..] {
        assert_eq!(
            version.as_ptr(),
            versions[0].as_ptr(),
            "repeated requests borrow the same stored version"
        );
    }
    assert_eq!(
        (count(1), count(2)),
        (0, 0),
        "version probes allocate no handlers"
    );
    // SAFETY: control 2 changes only probe status in the isolated child.
    unsafe { control(2, 777) };
    assert!(
        av_metrics_vship::libvship_version().is_some(),
        "default version retains successful admission"
    );
    assert!(
        VshipScorer::libvship_version().is_some(),
        "associated version shares successful admission"
    );
    assert!(
        av_metrics_vship::is_available(),
        "default availability shares successful admission"
    );
    assert!(
        VshipScorer::is_available(),
        "associated availability shares successful admission"
    );
    let format = VideoFormat {
        width: 32,
        height: 32,
        bit_depth: 8,
        chroma_sampling: ChromaSubsampling::Yuv420,
        color_range: None,
        color_primaries: None,
        color_transfer_characteristic: None,
        color_space: None,
        chroma_location: None,
    };
    assert!(
        matches!(
            VshipScorer::new(
                VshipConfig::new()
                    .with_gpu_id(av_metrics_vship::default_gpu_id().expect("default")),
                format,
                format,
                None
            ),
            Err(VshipError::CallFailed {
                function: "Vship_GPUFullCheck",
                status: 777,
                ..
            })
        ),
        "cached default version must not mask explicit native probe errors"
    );
    assert_eq!(
        (count(0), count(1), count(2)),
        (2, 0, 0),
        "failed probes allocate no handlers"
    );
    // SAFETY: restore the same child-local probe control to success.
    unsafe { control(2, 0) };
    let recovered = av_metrics_vship::libvship_version().expect("device recovered");
    assert_eq!(recovered, "5.1.2", "cached content survives probe failures");
    assert_eq!(
        recovered.as_ptr(),
        versions[0].as_ptr(),
        "recovery reuses the stored version"
    );
    assert_eq!(
        (count(0), count(1), count(2)),
        (2, 0, 0),
        "default version continues borrowing without probes or handlers"
    );
    drop(
        VshipScorer::new(
            VshipConfig::new().with_gpu_id(av_metrics_vship::default_gpu_id().expect("default")),
            format,
            format,
            None,
        )
        .expect("explicit device recovered"),
    );
    assert_eq!(
        (count(0), count(1), count(2)),
        (3, 1, 1),
        "explicit recovery revalidates and owns one handler"
    );
    println!(
        "version storage explicit failed probe status=777 retained; recovered pointer={:p}; probes=3, init/free=1/1 (version probes allocate none)",
        recovered.as_ptr()
    );
}

/// Catches serial handler multiplication, lost ownership on errors, and resets
/// before preflight. This drives the production constructor/submission boundary.
#[test]
#[cfg(unix)]
fn single_handler_native_ownership() {
    if std::env::var_os("VSHIP_HANDLER_FIXTURE_CHILD").is_some() {
        handler_child();
        return;
    }
    let dir = tempfile::tempdir().expect("handler fixture directory");
    let library = dir.path().join(if cfg!(target_os = "macos") {
        "libvship.dylib"
    } else {
        "libvship.so"
    });
    compile(&library, false, &[]);
    let mut errors = Vec::new();
    for (scenario, requested) in [
        ("ok", "1"),
        ("ok", "3"),
        ("ok", "default"),
        ("ok", "max"),
        ("compute17", "3"),
        ("compute777", "3"),
        ("compute-123", "3"),
        ("tag", "3"),
        ("history", "3"),
        ("score", "3"),
        ("gap", "3"),
        ("init", "3"),
    ] {
        let mut child = Command::new(std::env::current_exe().expect("executable"))
            .args(["--exact", "single_handler_native_ownership", "--nocapture"])
            .env("VSHIP_HANDLER_FIXTURE_CHILD", scenario)
            .env("VSHIP_HANDLER_FIXTURE_REQUESTED", requested)
            .env("VSHIP_BOUNDARY_FIXTURE_LIBRARY", &library)
            .env("VSHIP_BOUNDARY_FIXTURE_BACKEND", "3")
            .env("VSHIP_BOUNDARY_FIXTURE_PATCH", "2")
            .env("VSHIP_BOUNDARY_FIXTURE_PROBE_STATUS", "0")
            .env("VSHIP_BOUNDARY_FIXTURE_COMPUTE_STATUS", "0")
            .env_remove("VSHIP_BOUNDARY_FIXTURE_TAG")
            .env_remove("VSHIP_RESET_FIXTURE_RESET_STATUS")
            .env("VSHIP_PLUGIN_PATH", dir.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("handler child");
        let start = Instant::now();
        while child.try_wait().expect("status").is_none() {
            if start.elapsed() > Duration::from_secs(30) {
                child.kill().expect("kill");
                child.wait().expect("reap");
                panic!("handler ownership timed out: {scenario}/{requested}");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let out = child.wait_with_output().expect("output");
        let stdout = String::from_utf8_lossy(&out.stdout);
        if !out.status.success() || !stdout.contains("1 passed; 0 failed") {
            errors.push(format!(
                "{scenario}/{requested}: {stdout}\n{}",
                String::from_utf8_lossy(&out.stderr)
            ));
        } else {
            print!("{stdout}");
        }
    }
    assert!(errors.is_empty(), "{}", errors.join("\n"));
}

#[cfg(unix)]
fn handler_child() {
    use av_metrics_vship::{PlaneSet, PlaneSource};
    let scenario = std::env::var("VSHIP_HANDLER_FIXTURE_CHILD").expect("scenario");
    let requested = std::env::var("VSHIP_HANDLER_FIXTURE_REQUESTED").expect("requested");
    // SAFETY: exact fixture exports; mapping outlives all scorers and reads.
    let lib = unsafe {
        libloading::Library::new(
            std::env::var_os("VSHIP_BOUNDARY_FIXTURE_LIBRARY").expect("library"),
        )
    }
    .expect("mapping");
    // SAFETY: these signatures are defined in boundary.c.
    let (counter, control, trace) = unsafe {
        (
            lib.get::<unsafe extern "C" fn(u32) -> u32>(b"boundary_count\0")
                .expect("counter"),
            lib.get::<unsafe extern "C" fn(u32, i32)>(b"handler_set\0").expect("control"),
            lib.get::<unsafe extern "C" fn(u32) -> u32>(b"handler_trace\0").expect("trace"),
        )
    };
    // SAFETY: bounded fixture indices and live mapping throughout this function.
    let count = |i| unsafe { counter(i) };
    // SAFETY: only defined control indices 0..6 are used.
    let set = |i, value| unsafe { control(i, value) };
    let events = || {
        (0..256)
            .map(|i| {
                // SAFETY: every trace index is checked by the fixture.
                unsafe { trace(i) }
            })
            .take_while(|&n| n != 0)
            .collect::<Vec<_>>()
    };
    let clip = common::SyntheticClip::generate(
        32,
        32,
        8,
        ChromaSubsampling::Yuv420,
        1,
        common::Distortion::Mild,
    );
    let format = VideoFormat::from_details(clip.decoder().get_video_details());
    let frame = FrameBuilder::new(32, 32, ChromaSubsampling::Yuv420, 8)
        .build::<u8>()
        .expect("frame");
    let bad = FrameBuilder::new(16, 32, ChromaSubsampling::Yuv420, 8)
        .build::<u8>()
        .expect("bad frame");
    fn raw(f: &v_frame::frame::Frame<u8>) -> PlaneSet<'_> {
        [
            PlaneSource::from_plane(&f.y_plane).expect("Y"),
            PlaneSource::from_plane(f.u_plane.as_ref().expect("U")).expect("U"),
            PlaneSource::from_plane(f.v_plane.as_ref().expect("V")).expect("V"),
        ]
    }
    for metric in VshipMetric::all() {
        if matches!(scenario.as_str(), "history" | "score" | "gap") && metric != VshipMetric::Cvvdp
        {
            continue;
        }
        for i in [0, 1, 3, 4, 5] {
            set(i, 0);
        }
        // Restore the normal metric tag after the preceding metric's error case.
        let tag = match metric {
            VshipMetric::Ssimulacra2 => 4,
            VshipMetric::Butteraugli => 5,
            VshipMetric::Cvvdp => 6,
        };
        set(0, tag);
        let mut config = VshipConfig::new()
            .with_metric(metric)
            .with_disable_temporal(matches!(scenario.as_str(), "history" | "score"));
        if requested != "default" {
            config = config.with_handler_threads(if requested == "max" {
                u32::MAX
            } else {
                requested.parse().expect("integer")
            });
        }
        let legacy = if requested == "default" {
            4
        } else if requested == "max" {
            u32::MAX
        } else {
            requested.parse().expect("integer")
        };
        if requested == "max" {
            // Fail safely on the old implementation, before a giant allocation.
            assert_eq!(
                config.handler_count(),
                1,
                "extreme legacy request is not an allocation count"
            );
        }
        let before = (count(1), count(2));
        if scenario == "init" {
            set(1, 777);
        }
        let result = VshipScorer::new(config, format, format, None);
        if scenario == "init" {
            assert!(
                matches!(result, Err(VshipError::MetricUnavailable { reason, .. }) if reason.contains("777")),
                "failed init retains its native error"
            );
            assert_eq!(
                (count(1), count(2)),
                (before.0 + 1, before.1),
                "failed init never owns its output"
            );
            continue;
        }
        let mut scorer = result.expect("production scorer");
        assert_eq!(
            count(1) - before.0,
            1,
            "one native init for {metric:?}, legacy request {legacy}"
        );
        assert_eq!(
            (scorer.handler_count(), scorer.config().handler_count()),
            (1, 1),
            "both public counts report actual ownership"
        );
        assert_eq!(
            scorer.config().handler_threads(),
            legacy,
            "legacy request retained"
        );
        let id = count(1);
        let expected = if metric == VshipMetric::Butteraugli {
            (None, Some(42.0), Some(43.0), Some(44.0))
        } else {
            (Some(42.0), None, None, None)
        };
        let score = scorer.submit_pair(&frame, &frame).expect("first score");
        assert_eq!(
            (score.score, score.norm_q, score.norm_3, score.norm_inf),
            expected,
            "metric output fields retain their values"
        );
        let start = events().len();
        let snapshot = (count(3), count(4), count(5));
        for (a, b) in [(&frame, &bad), (&bad, &frame)] {
            assert!(
                matches!(
                    scorer.submit_pair_raw_with_reset(raw(a), raw(b), true),
                    Err(VshipError::InputMismatch { .. })
                ),
                "reject either invalid side before reset"
            );
        }
        assert_eq!(
            (count(3), count(4), count(5)),
            snapshot,
            "both sides preflight before native mutation"
        );
        assert_eq!(
            scorer.submitted().expect("submitted"),
            1,
            "invalid pairs do not advance submissions"
        );
        match scenario.as_str() {
            "compute17" => set(3, 17),
            "compute777" => set(3, 777),
            "compute-123" => set(3, -123),
            "tag" => set(0, 777),
            "history" | "gap" => set(4, -123),
            "score" => set(5, 777),
            _ => {},
        }
        let mut callbacks = Vec::new();
        let result = if scenario == "gap" {
            scorer
                .submit_pair_raw_with_reset(raw(&frame), raw(&frame), true)
                .map(|s| vec![s])
        } else {
            scorer.score_decoders::<u8>(&mut clip.decoder(), &mut clip.decoder(), |i, s| {
                callbacks.push((i, *s))
            })
        };
        if scenario == "ok" {
            let scores = result.expect("second score");
            assert_eq!(scores, vec![score], "decoder score retains metric output");
            assert_eq!(
                callbacks,
                vec![(0, score)],
                "one callback for a successful frame"
            );
            let third = scorer
                .submit_pair_raw_with_reset(raw(&frame), raw(&frame), true)
                .expect("raw reset score");
            assert_eq!(third, score, "raw submission retains metric output");
            assert_eq!(
                scorer.submitted().expect("submitted"),
                3,
                "all three successful calls count"
            );
            // Trace encoding: kind * 10000 + native id * 100 + per-handle calls.
            let mut expected_events = vec![30000 + id * 100 + 2];
            if metric == VshipMetric::Cvvdp {
                expected_events.push(40000 + id * 100 + 2);
            }
            expected_events.push(30000 + id * 100 + 3);
            assert_eq!(
                &events()[start..],
                expected_events,
                "same native handle and increasing call order, no round robin"
            );
            assert_eq!(
                (count(4), count(5)),
                if metric == VshipMetric::Cvvdp {
                    (1, 0)
                } else {
                    (0, 0)
                },
                "raw reset is a stateless no-op"
            );
        } else {
            let error = result.expect_err("injected native error");
            if scenario == "tag" {
                assert!(
                    matches!(error, VshipError::InvalidScoreTag { actual: 777, expected } if expected == tag),
                    "reject unknown native score tag without returning its score"
                );
            } else {
                let (function, status) = match scenario.as_str() {
                    "compute17" => ("Vship_ComputeHandler", 17),
                    "compute777" => ("Vship_ComputeHandler", 777),
                    "compute-123" => ("Vship_ComputeHandler", -123),
                    "history" | "gap" => ("Vship_Reset", -123),
                    "score" => ("Vship_ResetScore", 777),
                    _ => unreachable!(),
                };
                assert!(
                    matches!(error, VshipError::CallFailed { function: f, status: s, .. } if f == function && s == status),
                    "retain failing native function and raw status"
                );
            }
            assert!(callbacks.is_empty(), "failed frame never reaches callback");
            assert_eq!(
                scorer.submitted().expect("submitted"),
                1,
                "only successful frames advance submitted"
            );
            let base = id * 100;
            let expected_events = match scenario.as_str() {
                "gap" => vec![40000 + base + 1],
                "history" => vec![30000 + base + 2, 40000 + base + 2],
                "score" => vec![30000 + base + 2, 40000 + base + 2, 50000 + base + 2],
                _ => vec![30000 + base + 2],
            };
            assert_eq!(
                &events()[start..],
                expected_events,
                "native failure stops subsequent work on the owned handle"
            );
        }
        assert_eq!(count(2), before.1, "errors retain native ownership");
        drop(scorer);
        assert_eq!(
            (count(1), count(2)),
            (before.0 + 1, before.1 + 1),
            "one native init/free per scorer"
        );
        assert_eq!(
            events().last().copied(),
            Some(
                20000
                    + id * 100
                    + if scenario == "gap" {
                        1
                    } else if scenario == "ok" {
                        3
                    } else {
                        2
                    }
            ),
            "free the same native handle exactly once, after its last call"
        );
        println!(
            "handler ownership {metric:?}/{requested}/{scenario}: init/free=1/1, native identity={id}, ordered trace and success-only submissions checked"
        );
    }
}

#[test]
#[cfg(unix)]
fn native_discriminants_and_admission() {
    if std::env::var_os("VSHIP_BOUNDARY_FIXTURE_CHILD").is_some() {
        child();
        return;
    }
    let dir = tempfile::tempdir().expect("fixture directory");
    let library = dir.path().join(if cfg!(target_os = "macos") {
        "libvship.dylib"
    } else {
        "libvship.so"
    });
    compile(&library, false, &[]);
    // backend, patch, probe status, compute status, score tag (-1 = retain).
    let mut cases =
        vec![(777, 2, 0, 0, -1), (-123, 2, 0, 0, -1), (3, 3, 0, 0, -1), (3, 1, 0, 0, -1)];
    for backend in 0..4 {
        cases.push((backend, 2, 0, 0, -1));
    }
    for status in [1, 17, 777, -123] {
        cases.push((3, 2, status, 0, -1));
        cases.push((3, 2, 0, status, -1));
    }
    for tag in [0, 1, 4, 5, 6, 777, -123] {
        cases.push((3, 2, 0, 0, tag));
    }
    for (backend, patch, probe, status, tag) in cases {
        let mut child = Command::new(std::env::current_exe().expect("executable"));
        child
            .args(["--exact", "native_discriminants_and_admission", "--nocapture"])
            .env("VSHIP_BOUNDARY_FIXTURE_CHILD", "1")
            .env("VSHIP_BOUNDARY_FIXTURE_BACKEND", backend.to_string())
            .env("VSHIP_BOUNDARY_FIXTURE_PATCH", patch.to_string())
            .env("VSHIP_BOUNDARY_FIXTURE_PROBE_STATUS", probe.to_string())
            .env("VSHIP_BOUNDARY_FIXTURE_COMPUTE_STATUS", status.to_string())
            .env("VSHIP_PLUGIN_PATH", dir.path())
            .env("VSHIP_BOUNDARY_FIXTURE_LIBRARY", &library)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if tag != -1 {
            child.env("VSHIP_BOUNDARY_FIXTURE_TAG", tag.to_string());
        } else {
            child.env_remove("VSHIP_BOUNDARY_FIXTURE_TAG");
        }
        let mut child = child.spawn().expect("child");
        let start = Instant::now();
        while child.try_wait().expect("status").is_none() {
            if start.elapsed() > Duration::from_secs(30) {
                child.kill().expect("kill");
                child.wait().expect("reap");
                panic!("native admission timed out");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let out = child.wait_with_output().expect("output");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "{backend}/{patch}/{probe}/{status}/{tag}: {stdout}\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            stdout.contains("1 passed; 0 failed"),
            "exact nonzero test count: {stdout}"
        );
    }
}
#[cfg(unix)]
fn child() {
    let setting = |name| std::env::var(name).expect("setting").parse::<i32>().expect("integer");
    // SAFETY: isolated fixture and exact counter signature, mapping remains live.
    let lib = unsafe {
        libloading::Library::new(
            std::env::var_os("VSHIP_BOUNDARY_FIXTURE_LIBRARY").expect("library"),
        )
    }
    .expect("mapping");
    // SAFETY: signature defined in the fixture, valid indices 0..4.
    let counter = unsafe { lib.get::<unsafe extern "C" fn(u32) -> u32>(b"boundary_count\0") }
        .expect("counter");
    // SAFETY: only defined fixture indices 0..4 are used while mapped.
    let count = |index| unsafe { counter(index) };
    let format = VideoFormat {
        width: 32,
        height: 32,
        bit_depth: 8,
        chroma_sampling: ChromaSubsampling::Yuv420,
        color_range: None,
        color_primaries: None,
        color_transfer_characteristic: None,
        color_space: None,
        chroma_location: None,
    };
    for metric in VshipMetric::all() {
        let result = VshipScorer::new(
            VshipConfig::new().with_metric(metric).with_handler_threads(1),
            format,
            format,
            None,
        );
        if !(0..4).contains(&setting("VSHIP_BOUNDARY_FIXTURE_BACKEND"))
            || setting("VSHIP_BOUNDARY_FIXTURE_PATCH") != 2
        {
            assert!(
                matches!(&result, Err(VshipError::IncompatibleLibrary { .. })),
                "ABI rejection"
            );
            assert_eq!(
                (count(0), count(1)),
                (0, 0),
                "reject before GPU probe and handler allocation"
            );
            if !(0..4).contains(&setting("VSHIP_BOUNDARY_FIXTURE_BACKEND")) {
                assert!(
                    result
                        .err()
                        .expect("error")
                        .to_string()
                        .contains(&setting("VSHIP_BOUNDARY_FIXTURE_BACKEND").to_string()),
                    "retain unknown backend integer"
                );
            }
            continue;
        }
        let probe = setting("VSHIP_BOUNDARY_FIXTURE_PROBE_STATUS");
        if probe != 0 {
            assert!(
                matches!(result,Err(VshipError::CallFailed { status,.. }) if status==probe),
                "raw probe status"
            );
            assert_eq!(count(1), 0, "probe failure creates no handler");
            continue;
        }
        let mut scorer = result.expect("pinned ABI");
        assert_eq!(
            scorer.backend() as i32,
            setting("VSHIP_BOUNDARY_FIXTURE_BACKEND"),
            "checked backend roundtrip"
        );
        let frame = FrameBuilder::new(32, 32, ChromaSubsampling::Yuv420, 8)
            .build::<u8>()
            .expect("frame");
        let result = scorer.submit_pair(&frame, &frame);
        let status = setting("VSHIP_BOUNDARY_FIXTURE_COMPUTE_STATUS");
        let tag = std::env::var("VSHIP_BOUNDARY_FIXTURE_TAG")
            .ok()
            .map(|s| s.parse::<i32>().expect("tag"));
        let expected = match metric {
            VshipMetric::Ssimulacra2 => 4,
            VshipMetric::Butteraugli => 5,
            VshipMetric::Cvvdp => 6,
        };
        if status != 0 {
            let error = result.expect_err("status");
            assert!(
                matches!(&error,VshipError::CallFailed {status:s,..} if *s==status),
                "{error:?}"
            );
            if (1..=17).contains(&status) {
                assert!(
                    error.to_string().contains("known vship status"),
                    "known-code native formatting"
                );
            }
        } else if tag.is_some_and(|t| t != expected) {
            assert!(
                matches!(result,Err(VshipError::InvalidScoreTag {actual,expected:e}) if Some(actual)==tag && e==expected),
                "reject wrong or unknown native output tag"
            );
        } else {
            assert_eq!(
                result.expect("score").value(metric),
                Some(42.0),
                "known metric output marker"
            );
        }
        drop(scorer);
        assert_eq!(count(1), count(2), "all handlers freed");
    }
}
#[test]
#[cfg(unix)]
fn public_resets() {
    if std::env::var_os("VSHIP_RESET_FIXTURE_CHILD").is_some() {
        reset_child();
        return;
    }
    let dir = tempfile::tempdir().expect("reset fixture directory");
    let library = dir.path().join(if cfg!(target_os = "macos") {
        "libvship.dylib"
    } else {
        "libvship.so"
    });
    compile(&library, false, &[]);
    let mut errors = Vec::new();
    for (metric, handlers, status) in [
        ("SSIMULACRA2", 1, 777),
        ("SSIMULACRA2", 3, 777),
        ("Butteraugli", 1, 777),
        ("Butteraugli", 3, 777),
        ("CVVDP", 1, 0),
        ("CVVDP", 1, 17),
        ("CVVDP", 1, 777),
        ("CVVDP", 1, -123),
    ] {
        let mut child = Command::new(std::env::current_exe().expect("executable"))
            .args(["--exact", "public_resets", "--nocapture"])
            .env("VSHIP_RESET_FIXTURE_CHILD", "1")
            .env("VSHIP_RESET_FIXTURE_METRIC", metric)
            .env("VSHIP_RESET_FIXTURE_HANDLERS", handlers.to_string())
            .env("VSHIP_RESET_FIXTURE_RESET_STATUS", status.to_string())
            .env("VSHIP_BOUNDARY_FIXTURE_LIBRARY", &library)
            .env("VSHIP_BOUNDARY_FIXTURE_BACKEND", "3")
            .env("VSHIP_BOUNDARY_FIXTURE_PATCH", "2")
            .env("VSHIP_BOUNDARY_FIXTURE_PROBE_STATUS", "0")
            .env("VSHIP_BOUNDARY_FIXTURE_COMPUTE_STATUS", "0")
            .env_remove("VSHIP_BOUNDARY_FIXTURE_TAG")
            .env("VSHIP_PLUGIN_PATH", dir.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("reset child");
        let start = Instant::now();
        while child.try_wait().expect("status").is_none() {
            if start.elapsed() > Duration::from_secs(30) {
                child.kill().expect("kill");
                child.wait().expect("reap");
                panic!("public reset timed out: {metric}/{handlers}/{status}");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let out = child.wait_with_output().expect("output");
        let stdout = String::from_utf8_lossy(&out.stdout);
        if !out.status.success() || !stdout.contains("1 passed; 0 failed") {
            errors.push(format!(
                "{metric}/{handlers}/{status}: {stdout}\n{}",
                String::from_utf8_lossy(&out.stderr)
            ));
        } else {
            print!("{stdout}");
        }
    }
    assert!(errors.is_empty(), "{}", errors.join("\n"));
}

#[cfg(unix)]
fn reset_child() {
    const PROBE: u32 = 0;
    const INIT: u32 = 1;
    const FREE: u32 = 2;
    const COMPUTE: u32 = 3;
    const RESET: u32 = 4;
    const RESET_SCORE: u32 = 5;
    let metric =
        VshipMetric::from_name(&std::env::var("VSHIP_RESET_FIXTURE_METRIC").expect("metric"))
            .expect("known metric");
    let handlers = std::env::var("VSHIP_RESET_FIXTURE_HANDLERS")
        .expect("handlers")
        .parse::<u32>()
        .expect("integer");
    let status = std::env::var("VSHIP_RESET_FIXTURE_RESET_STATUS")
        .expect("status")
        .parse::<i32>()
        .expect("integer");
    // SAFETY: the isolated real-header fixture owns every handle; the mapping
    // stays live through scorer destruction and all counter reads.
    let lib = unsafe {
        libloading::Library::new(
            std::env::var_os("VSHIP_BOUNDARY_FIXTURE_LIBRARY").expect("library"),
        )
    }
    .expect("mapping");
    // SAFETY: exact fixture signature; only the six defined indices are used.
    let counter = unsafe { lib.get::<unsafe extern "C" fn(u32) -> u32>(b"boundary_count\0") }
        .expect("counter");
    // SAFETY: callers use the named indices above while the fixture is mapped.
    let count = |index| unsafe { counter(index) };
    let frame = FrameBuilder::new(32, 32, ChromaSubsampling::Yuv420, 8)
        .build::<u8>()
        .expect("frame");
    let format = VideoFormat {
        width: 32,
        height: 32,
        bit_depth: 8,
        chroma_sampling: ChromaSubsampling::Yuv420,
        color_range: None,
        color_primaries: None,
        color_transfer_characteristic: None,
        color_space: None,
        chroma_location: None,
    };
    let mut scorer = VshipScorer::new(
        VshipConfig::new().with_metric(metric).with_handler_threads(handlers),
        format,
        format,
        None,
    )
    .expect("real public scorer");
    let score = scorer.submit_pair(&frame, &frame).expect("score before resets");
    let expected_score = match metric {
        VshipMetric::Butteraugli => (None, Some(42.0), Some(43.0), Some(44.0)),
        _ => (Some(42.0), None, None, None),
    };
    assert_eq!(
        (score.score, score.norm_q, score.norm_3, score.norm_inf),
        expected_score,
        "known metric score before resets"
    );
    let temporal = scorer.reset_temporal();
    let accumulated = scorer.reset_score();
    let resets = (count(RESET), count(RESET_SCORE));
    println!(
        "public reset {metric:?}/{handlers}/{status}: reset/reset_score={resets:?}; results={temporal:?}/{accumulated:?}"
    );
    assert_eq!(
        resets,
        if metric == VshipMetric::Cvvdp {
            (1, 1)
        } else {
            (0, 0)
        },
        "stateless resets must not cross the native boundary"
    );
    for (result, function) in [(temporal, "Vship_Reset"), (accumulated, "Vship_ResetScore")] {
        if metric == VshipMetric::Cvvdp && status != 0 {
            assert!(
                matches!(result, Err(VshipError::CallFailed { function: f, status: s, .. }) if f == function && s == status),
                "retain native error function/raw status: {function}/{status}"
            );
        } else {
            result.expect("reset succeeds");
        }
    }
    assert_eq!(
        scorer.submitted().expect("submitted"),
        1,
        "resets do not submit"
    );
    assert_eq!(
        (count(PROBE), count(INIT), count(FREE), count(COMPUTE)),
        (1, 1, 0, 1),
        "resets do not allocate, free, probe or compute"
    );
    for _ in 0..handlers {
        let score = scorer.submit_pair(&frame, &frame).expect("score after resets");
        assert_eq!(
            (score.score, score.norm_q, score.norm_3, score.norm_inf),
            expected_score,
            "scores survive resets across repeated submissions"
        );
    }
    assert_eq!(
        scorer.submitted().expect("submitted"),
        1 + handlers,
        "submissions retained"
    );
    drop(scorer);
    assert_eq!(
        (count(INIT), count(FREE)),
        (1, 1),
        "balanced native cleanup"
    );
    println!(
        "public reset {metric:?}: init/free=1/1; submitted/compute={}/{}; scores unchanged",
        1 + handlers,
        count(COMPUTE)
    );
}

fn compile(path: &std::path::Path, probe: bool, flags: &[&str]) {
    let mut cmd = Command::new(std::env::var_os("CC").unwrap_or_else(|| "cc".into()));
    cmd.args(["-std=c11", "-Wall", "-Wextra", "-Werror"]);
    cmd.args(flags);
    if probe {
        cmd.arg("-DVSHIP_BOUNDARY_FIXTURE_PROBE");
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
fn real_header_layouts_and_signatures() {
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
    macro_rules! fields {($t:ty; $($f:ident:$ft:ty),+)=>{$(expected.extend([offset_of!($t,$f),size_of::<$ft>()]);)+};}
    ty!(i32);
    ty!(VshipBackend);
    ty!(VshipStructType);
    ty!(VshipSample);
    ty!(VshipRange);
    ty!(VshipChromaLocation);
    ty!(VshipColorFamily);
    ty!(VshipYuvMatrix);
    ty!(VshipTransferFunction);
    ty!(VshipPrimaries);
    ty!(VshipVersion);
    fields!(VshipVersion;major:i32,minor:i32,minor_minor:i32,backend:i32);
    ty!(VshipColorspace);
    fields!(VshipColorspace;width:i64,height:i64,target_width:i64,target_height:i64,sample:VshipSample,range:VshipRange,subsampling:VshipChromaSubsample,chroma_location:VshipChromaLocation,color_family:VshipColorFamily,yuv_matrix:VshipYuvMatrix,transfer_function:VshipTransferFunction,primaries:VshipPrimaries,crop:VshipCropRectangle);
    ty!(VshipInitSsimulacra2_1);
    fields!(VshipInitSsimulacra2_1;struct_type:VshipStructType,src_colorspace:VshipColorspace,dis_colorspace:VshipColorspace,gpu_id:i32);
    ty!(VshipInitButteraugli_1);
    fields!(VshipInitButteraugli_1;struct_type:VshipStructType,src_colorspace:VshipColorspace,dis_colorspace:VshipColorspace,q_norm:i32,intensity_multiplier:f32,gpu_id:i32);
    ty!(VshipInitCvvdp_1);
    fields!(VshipInitCvvdp_1;struct_type:VshipStructType,src_colorspace:VshipColorspace,dis_colorspace:VshipColorspace,fps:f32,resize_to_display:bool,model_key_cstr:*const std::ffi::c_char,model_config_json_cstr:*const std::ffi::c_char,gpu_id:i32);
    ty!(VshipScoreSsimulacra2);
    fields!(VshipScoreSsimulacra2;struct_type:i32,score:f64);
    ty!(VshipScoreButteraugli);
    fields!(VshipScoreButteraugli;struct_type:i32,norm_q:f64,norm_3:f64,norm_inf:f64,dstp:*const u8,dst_stride:i64);
    ty!(VshipScoreCvvdp);
    fields!(VshipScoreCvvdp;struct_type:i32,score:f64,dstp:*const u8,dst_stride:i64);
    ty!(VshipDeviceInfo);
    fields!(VshipDeviceInfo;name:[std::ffi::c_char;256],vram_size:u64,integrated:i32,multi_processor_count:i32,warp_size:i32,vulkan_feature_matrix:VshipVulkanFeatureMatrix);
    expected.extend([0, 17, 0, 1, 2, 3, 0, 1, 2, 3, 4, 5, 6]);
    assert_eq!(actual, expected, "real pinned public C schema/signatures");
    eprintln!(
        "Vship ABI: {} real-header values, all 14 resolved signatures",
        actual.len()
    );
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
    assert_ne!(short[0], size_of::<i32>(), "detect C status width drift");
}
