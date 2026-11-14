//! Runtime retention, not a reproduction: the removed build hints were inert.
use std::{
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// Exercise the supplied override names or explicit `bundled` release mode.
pub fn run(
    test: &str,
    library_name: &str,
    modes: &[&str],
    bare_loader: bool,
    compile: impl FnOnce(&Path),
    configure: impl Fn(&mut Command, &Path),
) {
    let directory = tempfile::Builder::new()
        .prefix("runtime-discovery-")
        .tempdir()
        .expect("staged release");
    let release = directory.path().join("release");
    let media = directory.path().join("media");
    let native = directory.path().join("runtime-override");
    std::fs::create_dir_all(&release).expect("release directory");
    std::fs::create_dir_all(&media).expect("separate media CWD");
    std::fs::create_dir_all(&native).expect("runtime override directory");
    let library = native.join(library_name);
    compile(&library);
    let executable = release.join("runtime-test");
    std::fs::copy(
        std::env::current_exe().expect("test executable"),
        &executable,
    )
    .expect("copy relocatable test artifact");
    for mode in modes.iter().copied() {
        // Keep the sibling absent in override cases so fmetrics cannot bypass
        // a broken override by discovering the same fixture beside the binary.
        let library = if mode == "bundled" {
            let staged = release.join(library_name);
            std::fs::rename(&library, &staged).expect("stage release sibling");
            staged
        } else {
            library.clone()
        };
        let mut command = Command::new(&executable);
        command
            .args(["--exact", test, "--nocapture"])
            .env_clear()
            .env("RUNTIME_DISCOVERY_CHILD", mode)
            .current_dir(&media)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for variable in ["CONDA_PREFIX", "VCPKG_ROOT", "CARGO_MANIFEST_DIR", "OUT_DIR"] {
            command.env(variable, "/runtime-discovery-unconsumed-build-host");
        }
        if mode != "bundled" {
            command.env(mode, &native);
        } else if bare_loader {
            // Unix bare-name loading needs the staged runtime loader path;
            // fmetrics instead must find current_exe's sibling without it.
            command.env(
                if cfg!(target_os = "macos") {
                    "DYLD_LIBRARY_PATH"
                } else {
                    "LD_LIBRARY_PATH"
                },
                &release,
            );
        }
        configure(&mut command, &library);
        eprintln!("Runtime discovery {mode}: {command:?}");
        let start = Instant::now();
        let mut child = loop {
            match command.spawn() {
                Ok(child) => break child,
                // A parallel fork can briefly inherit copy's write descriptor
                // until exec closes it. Retry only that transient OS condition.
                Err(error)
                    if error.kind() == std::io::ErrorKind::ExecutableFileBusy
                        && start.elapsed() < Duration::from_secs(30) =>
                {
                    std::thread::sleep(Duration::from_millis(10));
                },
                Err(error) => panic!("isolated staged runtime child: {error}"),
            }
        };
        while child.try_wait().expect("child status").is_none() {
            if start.elapsed() > Duration::from_secs(30) {
                child.kill().expect("kill timed out child");
                child.wait().expect("reap timed out child");
                panic!("Runtime discovery {mode} exceeded 30s");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = child.wait_with_output().expect("child output");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success() && stdout.contains("1 passed; 0 failed"),
            "Runtime discovery {mode}: {stdout}\n{stderr}"
        );
        eprintln!("Runtime discovery {mode}: {stdout}{stderr}");
    }
}
