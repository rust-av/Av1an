//! Shared setup for the per-component configuration tests: initializing a
//! temporary configuration and reading it back.

use std::path::{Path, PathBuf};

use california_condor::{
    commands::handlers::load_configuration,
    configuration::Configuration,
    test_helpers::{get_test_video, path_str},
};

use super::condor_cmd;

/// Initializes a configuration in `temp` and returns its path.
pub fn init_config(temp: &tempfile::TempDir) -> PathBuf {
    let video = get_test_video();
    let output = temp.path().join("out.mkv");
    condor_cmd(temp)
        .args(["init", path_str(&video.path), path_str(&output)])
        .assert()
        .success();
    temp.path().join("condor.json")
}

/// The saved configuration `config_path` holds.
pub fn load(config_path: &Path) -> Configuration {
    load_configuration(Some(config_path))
        .expect("load_configuration should succeed")
        .0
}

/// Re-exec one default-path body without changing the parent's cwd.
/// The parent owns its cwd fixture through child reaping and assertions.
pub fn isolated_default_path_test(test_name: &str, body: impl FnOnce(&Path)) {
    use std::{env, fs, process, time::Duration};

    const CHILD: &str = "CONDOR_DEFAULT_PATH_CHILD";
    const FIXTURE: &str = "CONDOR_DEFAULT_PATH_FIXTURE";
    const PARENT_PID: &str = "CONDOR_DEFAULT_PATH_PARENT_PID";

    if let Some(marker) = env::var_os(CHILD) {
        assert_eq!(marker, test_name, "exact default-path child marker");
        let parent_pid: u32 = env::var(PARENT_PID)
            .expect("parent pid env")
            .parse()
            .expect("numeric parent pid");
        let child_pid = process::id();
        assert_ne!(child_pid, parent_pid, "body must run in another OS process");
        let fixture = PathBuf::from(env::var_os(FIXTURE).expect("child fixture env"));
        let cwd = env::current_dir().expect("child cwd");
        assert_eq!(
            cwd.canonicalize().expect("canonical child cwd"),
            fixture.canonicalize().expect("canonical fixture cwd"),
            "child must use the explicit fixture cwd"
        );
        body(&cwd);
        fs::File::create_new(cwd.join("completed")).expect("child completed marker");
        println!(
            "default configuration body completed: {test_name}; parent_pid={parent_pid}; child_pid={child_pid}"
        );
        return;
    }
    assert!(
        env::var_os(FIXTURE).is_none() && env::var_os(PARENT_PID).is_none(),
        "fixture or parent pid without child marker"
    );
    // Keep cached native media initialization in the parent before re-exec.
    get_test_video();
    let temp = tempfile::tempdir().expect("parent-owned child fixture");
    let fixture = temp.path().canonicalize().expect("absolute fixture cwd");
    let parent_pid = process::id();
    // The timeout kills/reaps the direct child; descendants can retain pipes.
    let output = assert_cmd::Command::new(env::current_exe().expect("libtest executable"))
        .args(["--exact", test_name, "--nocapture"])
        .env(CHILD, test_name)
        .env(FIXTURE, &fixture)
        .env(PARENT_PID, parent_pid.to_string())
        .current_dir(&fixture)
        .timeout(Duration::from_secs(60))
        .output()
        .expect("run default-path child");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "default-path child failed: {output:?}"
    );
    assert!(
        stdout.contains("running 1 test\n")
            && stdout.contains(&format!("test {test_name} ... "))
            && stdout.contains("test result: ok. 1 passed; 0 failed; 0 ignored;"),
        "child must execute exactly the named body: {output:?}"
    );
    let completion = format!(
        "default configuration body completed: {test_name}; parent_pid={parent_pid}; child_pid="
    );
    let child_pid: u32 = stdout
        .split_once(&completion)
        .expect("named child body completed for this parent")
        .1
        .lines()
        .next()
        .expect("child pid output")
        .parse()
        .expect("numeric child pid output");
    assert_ne!(
        child_pid, parent_pid,
        "completed body must be in another OS process"
    );
    assert!(
        fixture.join("completed").is_file(),
        "body assertions completed"
    );
    assert!(
        fixture.join("condor.json").is_file(),
        "real default config saved"
    );
}
