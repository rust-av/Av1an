//! Actual public constructors with child-only environment and a safe native sink.
#![cfg(unix)]

use std::{
    ffi::{CStr, c_char},
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use av_metrics_vmaf::{
    BackendPreference, VideoFormat, VmafConfig, VmafError, VmafModel, VmafScorer,
};
use v_frame::chroma::ChromaSubsampling;

#[test]
fn explicit_stock_directory_is_exclusive() {
    if let Ok(scenario) = std::env::var("VMAF_MODEL_FIXTURE_CHILD") {
        check_child(&scenario);
        return;
    }
    let directory = tempfile::Builder::new()
        .prefix("vmaf-model-fixture-")
        .tempdir()
        .expect("fixture");
    let root = directory.path();
    let native = root.join("lib");
    let explicit = root.join("explicit models 雪;literal:directory");
    let system = root.join("share/vmaf/model");
    let adjacent = root.join("model");
    let missing_version = root.join("wrong-version");
    let corrupt = root.join("corrupt");
    for path in [&native, &explicit, &system, &adjacent, &missing_version, &corrupt] {
        std::fs::create_dir_all(path).expect("fixture directories");
    }
    for (path, marker) in [(&explicit, "E"), (&system, "S"), (&adjacent, "A"), (&corrupt, "!")] {
        std::fs::write(path.join("vmaf_v0.6.1.json"), marker).expect("distinct model markers");
    }
    std::fs::write(missing_version.join("vmaf_v0.6.1neg.json"), "N").expect("wrong version");
    let library = native.join(if cfg!(target_os = "macos") {
        "libvmaf.dylib"
    } else {
        "libvmaf.so"
    });
    let output = Command::new("cc")
        .args(["-shared", "-fPIC", "-std=c99", "-Wall", "-Wextra", "-Werror", "-o"])
        .arg(&library)
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/models.c"
        ))
        .output()
        .expect("model fixture requires cc");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut failed = Vec::new();
    for scenario in [
        "valid",
        "missing_directory",
        "missing_version",
        "corrupt",
        "direct",
        "unset_builtin",
        "empty_builtin",
        "unset_fallback",
        "empty_fallback",
    ] {
        let mut command = Command::new(std::env::current_exe().expect("test executable"));
        command
            .args(["--exact", "explicit_stock_directory_is_exclusive", "--nocapture"])
            .env("VMAF_MODEL_FIXTURE_CHILD", scenario)
            .env("VMAF_MODEL_FIXTURE_ROOT", root)
            .env("VMAF_MODEL_FIXTURE_LIBRARY", &library)
            .env(
                "VMAF_MODEL_FIXTURE_NO_BUILTIN",
                if scenario.ends_with("fallback") {
                    "1"
                } else {
                    "0"
                },
            )
            .env("VMAF_LIB_DIR", &native)
            .env_remove("VMAF_MODEL_PATH")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        match scenario {
            "missing_directory" | "direct" => {
                command.env("VMAF_MODEL_PATH", root.join("absent"));
            },
            "missing_version" => {
                command.env("VMAF_MODEL_PATH", &missing_version);
            },
            "corrupt" => {
                command.env("VMAF_MODEL_PATH", &corrupt);
            },
            "valid" => {
                command.env("VMAF_MODEL_PATH", &explicit);
            },
            name if name.starts_with("empty") => {
                command.env("VMAF_MODEL_PATH", "");
            },
            _ => {},
        }
        let mut child = command.spawn().expect("model selection isolated child");
        let start = Instant::now();
        while child.try_wait().expect("child status").is_none() {
            if start.elapsed() > Duration::from_secs(30) {
                child.kill().expect("kill timed out child");
                child.wait().expect("reap child");
                panic!("model selection {scenario} exceeded 30s");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = child.wait_with_output().expect("child output");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("model selection {scenario}: {stdout}{stderr}");
        assert!(stdout.contains("running 1 test"), "exact child filter");
        if !output.status.success() {
            failed.push(scenario);
        }
    }
    assert!(
        failed.is_empty(),
        "failed model selection scenarios: {failed:?}"
    );
}

fn check_child(scenario: &str) {
    // SAFETY: the compiled fixture remains mapped for all exact-signature reads.
    let library = unsafe {
        libloading::Library::new(std::env::var_os("VMAF_MODEL_FIXTURE_LIBRARY").expect("fixture"))
    }
    .expect("fixture counters");
    // SAFETY: exact fixture signature; every index below is in 0..8.
    let counter =
        unsafe { library.get::<unsafe extern "C" fn(u32) -> u32>(b"model_fixture_count\0") }
            .expect("counter");
    // SAFETY: exact fixture signature; returns a live static NUL-terminated buffer.
    let path =
        unsafe { library.get::<unsafe extern "C" fn() -> *const c_char>(b"model_fixture_path\0") }
            .expect("path");
    // SAFETY: all counter indices are defined while the fixture is mapped.
    let count = |index| unsafe { counter(index) };
    let root = PathBuf::from(std::env::var_os("VMAF_MODEL_FIXTURE_ROOT").expect("root"));
    let explicit = root.join("explicit models 雪;literal:directory/vmaf_v0.6.1.json");
    let mut config = VmafConfig::new().with_backend(BackendPreference::CpuOnly);
    if scenario == "direct" {
        config = config.with_model_path(&explicit);
    }
    let requested_model = config.model.clone();
    let result = VmafScorer::new(
        config,
        VideoFormat {
            width: 8,
            height: 8,
            bit_depth: 8,
            chroma_sampling: ChromaSubsampling::Yuv420,
        },
    );
    // Even RED drops erroneously accepted scorers before inspecting cleanup.
    let error = result
        .map(|scorer| {
            assert_eq!(
                scorer.config().model,
                requested_model,
                "preserve the caller's public configuration"
            );
        })
        .err();
    let snapshot: Vec<_> = (0..8).map(count).collect();
    // SAFETY: fixture-owned static buffer remains mapped and NUL-terminated.
    let selected = unsafe { CStr::from_ptr(path()) }.to_str().expect("UTF-8 fixture path");
    eprintln!(
        "model selection {scenario} path={selected:?}, counters={snapshot:?}, error={error:?}"
    );
    if scenario.starts_with("missing") {
        assert_eq!(
            snapshot, [0; 8],
            "missing override fails before any native work"
        );
        let error = error.expect("missing explicit model must fail");
        assert!(
            matches!(error, VmafError::ModelNotFound { .. }),
            "override context: {error:?}"
        );
        let expected = std::env::var_os("VMAF_MODEL_PATH").expect("override");
        let expected = PathBuf::from(expected).join("vmaf_v0.6.1.json");
        let message = error.to_string();
        assert!(
            message.contains("VMAF_MODEL_PATH") && message.contains(&*expected.to_string_lossy()),
            "override and requested file: {message}"
        );
        assert_eq!(
            VmafModel::Default.find_stock_model_file(),
            None,
            "invalid override cannot advertise a fallback"
        );
    } else if scenario == "corrupt" {
        assert_eq!(
            &snapshot[..7],
            [1, 1, 0, 1, 1, 1, 1],
            "owned partial destroyed and context closed without another model attempt"
        );
        assert_eq!(
            selected,
            root.join("corrupt/vmaf_v0.6.1.json").to_str().expect("path"),
            "only corrupt override loaded"
        );
        let error = error.expect("corrupt explicit model must fail");
        let message = error.to_string();
        assert!(
            message.contains("VMAF_MODEL_PATH")
                && message.contains(selected)
                && message.contains("vmaf_model_load_from_path")
                && message.contains("-87"),
            "native error context: {message}"
        );
        assert!(
            !message.contains("no built-in models"),
            "do not misdiagnose intentional override: {message}"
        );
    } else {
        assert!(error.is_none(), "valid selection: {error:?}");
        let from_path = matches!(scenario, "valid" | "direct") || scenario.ends_with("fallback");
        let builtin = scenario != "valid" && scenario != "direct";
        assert_eq!(
            &snapshot[..7],
            [1, 1, u32::from(builtin), u32::from(from_path), 1, 1, 1],
            "exclusive model loader and balanced cleanup"
        );
        if scenario == "valid" || scenario == "direct" {
            assert_eq!(
                selected,
                explicit.to_str().expect("path"),
                "literal spaces, Unicode, colon and semicolon are not split"
            );
            assert_eq!(
                count(7),
                u32::from(b'E'),
                "explicit marker, not system/adjacent"
            );
        }
        if scenario == "valid" {
            assert_eq!(
                VmafModel::model_search_paths().first(),
                explicit.parent().map(std::path::Path::to_path_buf).as_ref(),
                "explicit directory reported first"
            );
            assert_eq!(
                VmafModel::Default.find_stock_model_file(),
                Some(explicit),
                "finder honors override too"
            );
        }
        if scenario.ends_with("fallback") {
            assert!(
                selected.ends_with("vmaf_v0.6.1.json"),
                "ordinary stock fallback: {selected}"
            );
        }
    }
    // SAFETY: ownership.c's existing counter ABI is shared by this pinned fixture.
    let ownership =
        unsafe { library.get::<unsafe extern "C" fn(u32) -> u32>(b"ownership_count\0") }
            .expect("ownership counters");
    // SAFETY: all indices refer to existing fixture counters.
    let owned = |index| unsafe { ownership(index) };
    assert_eq!(
        owned(16),
        owned(17),
        "all context/picture allocations freed"
    );
    for index in [8, 9, 10, 12, 13] {
        assert_eq!(owned(index), 0, "no picture ownership violation at {index}");
    }
}
