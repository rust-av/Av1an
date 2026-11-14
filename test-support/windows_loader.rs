//! Native DLL acceptance gate. Included beside each production load helper.
use super::*;
use std::{
    process::Command,
    time::{Duration, Instant},
};

fn marker(candidate: &Path) -> i32 {
    // SAFETY: the test builds this harmless DLL and keeps it mapped for the call.
    let library =
        unsafe { load_library(candidate) }.expect("load trusted marker with private dependency");
    // SAFETY: our fixture exports exactly this signature; the library is alive.
    unsafe {
        library
            .get::<unsafe extern "C" fn() -> i32>(b"windows_loader_marker\0")
            .expect("marker export")()
    }
}

fn child_case(case: &str) {
    let root =
        PathBuf::from(std::env::var_os("WINDOWS_LOADER_FIXTURE_ROOT").expect("fixture root"));
    let name = std::env::var_os("WINDOWS_LOADER_FIXTURE_NAME").expect("unique marker name");
    // A bare VSSCRIPT_PATH must not inject media/plugins or any other relative
    // directory. This also checks the production candidate builders, not a replica.
    assert!(
        library_candidates().iter().all(|path| {
            path.is_absolute() || path.file_name().is_some_and(|name| name == path.as_os_str())
        }),
        "candidate builder invented an unqualified directory"
    );
    match case {
        "bare" | "path" => {
            // SAFETY: only a test-controlled decoy could be found for this unique name.
            assert!(
                unsafe { load_library(Path::new(&name)) }.is_err(),
                "bare fallback loaded a media-CWD/PATH decoy"
            );
        },
        "absolute" | "shadow" => assert_eq!(marker(&root.join("trusted").join(&name)), 7),
        "override_absolute" | "override_relative" => {
            let expected = root.join("trusted").join(
                std::env::var_os("WINDOWS_LOADER_FIXTURE_STANDARD_NAME")
                    .expect("production library name"),
            );
            let candidate = library_candidates()
                .into_iter()
                .find(|candidate| candidate == &expected)
                .expect("production builder must qualify the explicit directory override");
            assert_eq!(
                marker(&candidate),
                7,
                "override did not select the trusted sibling"
            );
        },
        "invalid" => {
            let file = root.join("trusted").join(&name);
            assert!(
                !library_candidates().iter().any(|candidate| candidate.starts_with(&file)),
                "a DLL filename override must not be treated as a directory"
            );
        },
        "missing" => {
            let candidate = root.join("missing").join(&name);
            // SAFETY: harmless fixture; its private dependency is intentionally absent.
            let error = unsafe { load_library(&candidate) }
                .expect_err("missing sibling must not fall back to media-CWD dependency");
            assert!(
                error.to_string().contains("126"),
                "original missing-dependency OS error lost: {error}"
            );
            assert!(
                error.get_ref().is_some(),
                "underlying libloading diagnostic must survive"
            );
        },
        _ => panic!("unknown Windows loader child case: {case}"),
    }
}

fn compile(compiler: &std::ffi::OsStr, source: &Path, dll: &Path, extra: &[std::ffi::OsString]) {
    let output = Command::new(compiler)
        .args(["-shared", "-std=c11", "-O0", "-Wall", "-Wextra", "-Werror", "-o"])
        .arg(dll).arg(source).args(extra).output()
        .expect("Windows loader gate requires AV_METRICS_WINDOWS_CC pointing to a native MinGW GCC-compatible C compiler; see test-support/README.md");
    assert!(
        output.status.success(),
        "Windows loader DLL fixture compilation failed; use a compiler matching the Rust process architecture: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "required native Windows gate; see test-support/README.md for compiler prerequisites"]
fn windows_loader() {
    if let Ok(case) = std::env::var("WINDOWS_LOADER_FIXTURE_CASE") {
        child_case(&case);
        return;
    }
    let compiler = std::env::var_os("AV_METRICS_WINDOWS_CC")
        .expect("set AV_METRICS_WINDOWS_CC to a native MinGW GCC-compatible C compiler; see test-support/README.md");
    let root = tempfile::tempdir().expect("DLL fixture directory");
    for directory in ["trusted", "media", "missing", "path"] {
        std::fs::create_dir_all(root.path().join(directory)).expect("fixture subdirectory");
    }
    let name = format!("windows_loader_marker_{}.dll", std::process::id());
    let dependency = format!(
        "windows_loader_private_dependency_{}.dll",
        std::process::id()
    );
    let import = root.path().join("private.a");
    let dep_source = root.path().join("dependency.c");
    let main_source = root.path().join("marker.c");
    let decoy_source = root.path().join("decoy.c");
    std::fs::write(
        &dep_source,
        "__declspec(dllexport) int windows_loader_dependency(void) { return WINDOWS_LOADER_FIXTURE_VALUE; }\n",
    )
    .expect("dependency C fixture");
    std::fs::write(&main_source,
        "__declspec(dllimport) int windows_loader_dependency(void);\n__declspec(dllexport) int windows_loader_marker(void) { return windows_loader_dependency(); }\n").expect("marker C fixture");
    std::fs::write(
        &decoy_source,
        "__declspec(dllexport) int windows_loader_marker(void) { return 99; }\n",
    )
    .expect("decoy C fixture");
    let trusted = root.path().join("trusted");
    compile(
        &compiler,
        &dep_source,
        &trusted.join(&dependency),
        &[
            "-DWINDOWS_LOADER_FIXTURE_VALUE=7".into(),
            format!("-Wl,--out-implib,{}", import.display()).into(),
        ],
    );
    compile(
        &compiler,
        &main_source,
        &trusted.join(&name),
        &[import.into_os_string()],
    );
    compile(
        &compiler,
        &decoy_source,
        &root.path().join("media").join(&name),
        &[],
    );
    std::fs::copy(
        root.path().join("media").join(&name),
        root.path().join("path").join(&name),
    )
    .expect("PATH decoy DLL");
    // The shadow has the exact imported DLL name, not just the same export.
    compile(
        &compiler,
        &dep_source,
        &root.path().join("media").join(&dependency),
        &["-DWINDOWS_LOADER_FIXTURE_VALUE=99".into()],
    );
    std::fs::copy(trusted.join(&name), root.path().join("missing").join(&name))
        .expect("missing sibling fixture");
    let standard_name = library_candidates()
        .last()
        .and_then(|path| path.file_name())
        .expect("last production candidate is bare")
        .to_owned();
    std::fs::copy(trusted.join(&name), trusted.join(&standard_name))
        .expect("explicit override fixture");

    for case in [
        "absolute",
        "bare",
        "path",
        "override_absolute",
        "override_relative",
        "invalid",
        "missing",
        "shadow",
    ] {
        let mut command = Command::new(std::env::current_exe().expect("test binary"));
        command
            .args([
                "--exact",
                "ffi::windows_loader_tests::windows_loader",
                "--ignored",
                "--nocapture",
            ])
            .current_dir(root.path().join("media"))
            .env("WINDOWS_LOADER_FIXTURE_CASE", case)
            .env("WINDOWS_LOADER_FIXTURE_ROOT", root.path())
            .env("WINDOWS_LOADER_FIXTURE_NAME", &name)
            .env("WINDOWS_LOADER_FIXTURE_STANDARD_NAME", &standard_name)
            .env_remove("PATH")
            .env("VSSCRIPT_PATH", "vsscript.dll")
            .env_remove("VAPOURSYNTH_EXTRA_PLUGIN_PATH")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        for variable in [
            "VMAF_LIB_DIR",
            "VSHIP_PLUGIN_PATH",
            "VSHIP_LIB_DIR",
            "FMETRICS_LIB_PATH",
            "FMETRICS_LIB_DIR",
        ] {
            command.env_remove(variable);
            if case == "override_absolute" {
                command.env(variable, &trusted);
            } else if case == "override_relative" {
                command.env(variable, "../trusted");
            } else if case == "invalid" {
                command.env(variable, trusted.join(&name));
            }
        }
        if case == "path" {
            command.env("PATH", root.path().join("path"));
        }
        let mut child = command.spawn().expect("isolated DLL test process");
        let deadline = Instant::now() + Duration::from_secs(30);
        while child.try_wait().expect("poll DLL child").is_none() {
            if Instant::now() >= deadline {
                child.kill().expect("kill stalled DLL child");
                let output = child.wait_with_output().expect("reap stalled DLL child");
                panic!(
                    "Windows loader {case} timed out: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = child.wait_with_output().expect("DLL child output");
        assert!(
            output.status.success(),
            "Windows loader {case}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed"),
            "Windows loader {case}: exact child must execute one test: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        println!("Windows loader native DLL case {case}: passed");
    }
}
