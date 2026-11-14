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
