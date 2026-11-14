pub mod config;

use assert_cmd::Command;

pub fn condor_bin() -> Command {
    Command::cargo_bin("av1an").expect("av1an binary should be built")
}

pub fn condor_cmd(working_directory: &tempfile::TempDir) -> Command {
    let mut cmd = condor_bin();
    cmd.current_dir(working_directory.path());
    cmd
}
