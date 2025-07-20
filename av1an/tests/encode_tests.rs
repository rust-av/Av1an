#![expect(
    clippy::tests_outside_test_module,
    reason = "integration tests are only compiled in test mode"
)]

use std::{
    fs::remove_file,
    io::Write,
    path::{Path, PathBuf},
};

use assert_cmd::Command;
use serial_test::serial;
use tempfile::{NamedTempFile, TempDir, TempPath};

fn input_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("tt_sif.y4m")
}

fn create_vpy_input() -> TempPath {
    let mut input_file = NamedTempFile::with_suffix(".vpy").unwrap();
    write!(
        input_file.as_file_mut(),
        r#"import vapoursynth as vs
core = vs.core
clip = core.lsmas.LWLibavSource(source="{}")
clip.set_output(0)"#,
        input_path().to_string_lossy()
    )
    .unwrap();
    input_file.flush().unwrap();
    input_file.into_temp_path()
}

// The baseline tests should not include the faster default params, because we
// want to also test that it works without params passed
#[test]
#[serial]
fn encode_test_baseline_aom() {
    let mut cmd = Command::cargo_bin("av1an").unwrap();
    let input_file = input_path();
    let output_file = NamedTempFile::with_suffix(".mkv").unwrap().into_temp_path();
    let temp_dir = TempDir::new().unwrap();

    cmd.arg("-i")
        .arg(&input_file)
        .arg("-e")
        .arg("aom")
        .arg("--pix-format")
        .arg("yuv420p")
        .arg("--sc-method")
        .arg("fast")
        .arg("-y")
        .arg("--temp")
        .arg(temp_dir.path())
        .arg("-o")
        .arg(&output_file)
        .assert()
        .success();
    assert!(output_file.exists());
    assert!(output_file.metadata().unwrap().len() > 0);
}

#[test]
#[serial]
fn encode_test_baseline_rav1e() {
    let mut cmd = Command::cargo_bin("av1an").unwrap();
    let input_file = input_path();
    let output_file = NamedTempFile::with_suffix(".mkv").unwrap().into_temp_path();
    let temp_dir = TempDir::new().unwrap();

    cmd.arg("-i")
        .arg(&input_file)
        .arg("-e")
        .arg("rav1e")
        .arg("--pix-format")
        .arg("yuv420p")
        .arg("--sc-method")
        .arg("fast")
        .arg("-y")
        .arg("--temp")
        .arg(temp_dir.path())
        .arg("-o")
        .arg(&output_file)
        .assert()
        .success();
    assert!(output_file.exists());
    assert!(output_file.metadata().unwrap().len() > 0);
}

#[test]
#[serial]
fn encode_test_baseline_svt_av1() {
    let mut cmd = Command::cargo_bin("av1an").unwrap();
    let input_file = input_path();
    let output_file = NamedTempFile::with_suffix(".mkv").unwrap().into_temp_path();
    let temp_dir = TempDir::new().unwrap();

    cmd.arg("-i")
        .arg(&input_file)
        .arg("-e")
        .arg("svt-av1")
        .arg("--pix-format")
        .arg("yuv420p")
        .arg("--sc-method")
        .arg("fast")
        .arg("-y")
        .arg("--temp")
        .arg(temp_dir.path())
        .arg("-o")
        .arg(&output_file)
        .assert()
        .success();
    assert!(output_file.exists());
    assert!(output_file.metadata().unwrap().len() > 0);
}

#[test]
#[serial]
fn encode_test_baseline_vpx() {
    let mut cmd = Command::cargo_bin("av1an").unwrap();
    let input_file = input_path();
    let output_file = NamedTempFile::with_suffix(".mkv").unwrap().into_temp_path();
    let temp_dir = TempDir::new().unwrap();

    cmd.arg("-i")
        .arg(&input_file)
        .arg("-e")
        .arg("vpx")
        .arg("--pix-format")
        .arg("yuv420p")
        .arg("--sc-method")
        .arg("fast")
        .arg("-y")
        .arg("--temp")
        .arg(temp_dir.path())
        .arg("-o")
        .arg(&output_file)
        .assert()
        .success();
    assert!(output_file.exists());
    assert!(output_file.metadata().unwrap().len() > 0);
}

#[test]
#[serial]
fn encode_test_baseline_x265() {
    let mut cmd = Command::cargo_bin("av1an").unwrap();
    let input_file = input_path();
    let output_file = NamedTempFile::with_suffix(".mkv").unwrap().into_temp_path();
    let temp_dir = TempDir::new().unwrap();

    cmd.arg("-i")
        .arg(&input_file)
        .arg("-e")
        .arg("x265")
        .arg("--pix-format")
        .arg("yuv420p")
        .arg("--sc-method")
        .arg("fast")
        .arg("-y")
        .arg("--temp")
        .arg(temp_dir.path())
        .arg("-o")
        .arg(&output_file)
        .assert()
        .success();
    assert!(output_file.exists());
    assert!(output_file.metadata().unwrap().len() > 0);
}

#[test]
#[serial]
fn encode_test_baseline_x264() {
    let mut cmd = Command::cargo_bin("av1an").unwrap();
    let input_file = input_path();
    let output_file = NamedTempFile::with_suffix(".mkv").unwrap().into_temp_path();
    let temp_dir = TempDir::new().unwrap();

    cmd.arg("-i")
        .arg(&input_file)
        .arg("-e")
        .arg("x264")
        .arg("--pix-format")
        .arg("yuv420p")
        .arg("--sc-method")
        .arg("fast")
        .arg("-y")
        .arg("--temp")
        .arg(temp_dir.path())
        .arg("-o")
        .arg(&output_file)
        .assert()
        .success();
    assert!(output_file.exists());
    assert!(output_file.metadata().unwrap().len() > 0);
}

const AOM_FAST_PARAMS: &str = " --cpu-used=10 --rt --threads=4";
const RAV1E_FAST_PARAMS: &str = " --speed 10 --low-latency";
const SVT_AV1_FAST_PARAMS: &str = " --preset 8";
const VPX_FAST_PARAMS: &str = " --cpu-used=9 --rt --threads=4";
const X26X_FAST_PARAMS: &str = " --preset ultrafast";

#[test]
#[serial]
fn encode_test_baseline_select_aom() {
    let mut cmd = Command::cargo_bin("av1an").unwrap();
    let input_file = input_path();
    let output_file = NamedTempFile::with_suffix(".mkv").unwrap().into_temp_path();
    let temp_dir = TempDir::new().unwrap();

    cmd.arg("-i")
        .arg(&input_file)
        .arg("-e")
        .arg("aom")
        .arg("--pix-format")
        .arg("yuv420p")
        .arg("--sc-method")
        .arg("fast")
        .arg("-y")
        .arg("--temp")
        .arg(temp_dir.path())
        .arg("-o")
        .arg(&output_file)
        .arg("-v")
        .arg(AOM_FAST_PARAMS)
        .arg("--chunk-method")
        .arg("select")
        .assert()
        .success();
    assert!(output_file.exists());
    assert!(output_file.metadata().unwrap().len() > 0);
}

#[test]
#[serial]
fn encode_test_baseline_select_rav1e() {
    let mut cmd = Command::cargo_bin("av1an").unwrap();
    let input_file = input_path();
    let output_file = NamedTempFile::with_suffix(".mkv").unwrap().into_temp_path();
    let temp_dir = TempDir::new().unwrap();

    cmd.arg("-i")
        .arg(&input_file)
        .arg("-e")
        .arg("rav1e")
        .arg("--pix-format")
        .arg("yuv420p")
        .arg("--sc-method")
        .arg("fast")
        .arg("-y")
        .arg("--temp")
        .arg(temp_dir.path())
        .arg("-o")
        .arg(&output_file)
        .arg("-v")
        .arg(RAV1E_FAST_PARAMS)
        .arg("--chunk-method")
        .arg("select")
        .assert()
        .success();
    assert!(output_file.exists());
    assert!(output_file.metadata().unwrap().len() > 0);
}

#[test]
#[serial]
fn encode_test_baseline_select_svt_av1() {
    let mut cmd = Command::cargo_bin("av1an").unwrap();
    let input_file = input_path();
    let output_file = NamedTempFile::with_suffix(".mkv").unwrap().into_temp_path();
    let temp_dir = TempDir::new().unwrap();

    cmd.arg("-i")
        .arg(&input_file)
        .arg("-e")
        .arg("svt-av1")
        .arg("--pix-format")
        .arg("yuv420p")
        .arg("--sc-method")
        .arg("fast")
        .arg("-y")
        .arg("--temp")
        .arg(temp_dir.path())
        .arg("-o")
        .arg(&output_file)
        .arg("-v")
        .arg(SVT_AV1_FAST_PARAMS)
        .arg("--chunk-method")
        .arg("select")
        .assert()
        .success();
    assert!(output_file.exists());
    assert!(output_file.metadata().unwrap().len() > 0);
}

#[test]
#[serial]
fn encode_test_baseline_select_vpx() {
    let mut cmd = Command::cargo_bin("av1an").unwrap();
    let input_file = input_path();
    let output_file = NamedTempFile::with_suffix(".mkv").unwrap().into_temp_path();
    let temp_dir = TempDir::new().unwrap();

    cmd.arg("-i")
        .arg(&input_file)
        .arg("-e")
        .arg("vpx")
        .arg("--pix-format")
        .arg("yuv420p")
        .arg("--sc-method")
        .arg("fast")
        .arg("-y")
        .arg("--temp")
        .arg(temp_dir.path())
        .arg("-o")
        .arg(&output_file)
        .arg("-v")
        .arg(VPX_FAST_PARAMS)
        .arg("--chunk-method")
        .arg("select")
        .assert()
        .success();
    assert!(output_file.exists());
    assert!(output_file.metadata().unwrap().len() > 0);
}

#[test]
#[serial]
fn encode_test_baseline_select_x265() {
    let mut cmd = Command::cargo_bin("av1an").unwrap();
    let input_file = input_path();
    let output_file = NamedTempFile::with_suffix(".mkv").unwrap().into_temp_path();
    let temp_dir = TempDir::new().unwrap();

    cmd.arg("-i")
        .arg(&input_file)
        .arg("-e")
        .arg("x265")
        .arg("--pix-format")
        .arg("yuv420p")
        .arg("--sc-method")
        .arg("fast")
        .arg("-y")
        .arg("--temp")
        .arg(temp_dir.path())
        .arg("-o")
        .arg(&output_file)
        .arg("-v")
        .arg(X26X_FAST_PARAMS)
        .arg("--chunk-method")
        .arg("select")
        .assert()
        .success();
    assert!(output_file.exists());
    assert!(output_file.metadata().unwrap().len() > 0);
}

#[test]
#[serial]
fn encode_test_baseline_select_x264() {
    let mut cmd = Command::cargo_bin("av1an").unwrap();
    let input_file = input_path();
    let output_file = NamedTempFile::with_suffix(".mkv").unwrap().into_temp_path();
    let temp_dir = TempDir::new().unwrap();

    cmd.arg("-i")
        .arg(&input_file)
        .arg("-e")
        .arg("x264")
        .arg("--pix-format")
        .arg("yuv420p")
        .arg("--sc-method")
        .arg("fast")
        .arg("-y")
        .arg("--temp")
        .arg(temp_dir.path())
        .arg("-o")
        .arg(&output_file)
        .arg("-v")
        .arg(X26X_FAST_PARAMS)
        .arg("--chunk-method")
        .arg("select")
        .assert()
        .success();
    assert!(output_file.exists());
    assert!(output_file.metadata().unwrap().len() > 0);
}

#[test]
#[serial]
fn encode_test_target_quality() {
    let mut cmd = Command::cargo_bin("av1an").unwrap();
    let input_file = input_path();
    let output_file_95 = NamedTempFile::with_suffix(".mkv").unwrap().into_temp_path();
    let output_file_80 = NamedTempFile::with_suffix(".mkv").unwrap().into_temp_path();
    let temp_dir1 = TempDir::new().unwrap();
    cmd.arg("-i")
        .arg(&input_file)
        .arg("-e")
        .arg("x265")
        .arg("--pix-format")
        .arg("yuv420p")
        .arg("-c")
        .arg("mkvmerge")
        .arg("--sc-method")
        .arg("fast")
        .arg("-y")
        .arg("-v")
        .arg(X26X_FAST_PARAMS)
        .arg("--target-quality")
        .arg("95")
        .arg("--temp")
        .arg(temp_dir1.path())
        .arg("-o")
        .arg(&output_file_95)
        .arg("--log-file")
        .arg("95")
        .assert()
        .success();
    assert!(output_file_95.exists());
    assert!(output_file_95.metadata().unwrap().len() > 0);

    let temp_dir2 = TempDir::new().unwrap();
    let mut cmd = Command::cargo_bin("av1an").unwrap();
    cmd.arg("-i")
        .arg(&input_file)
        .arg("-e")
        .arg("x265")
        .arg("--pix-format")
        .arg("yuv420p")
        .arg("-c")
        .arg("mkvmerge")
        .arg("--sc-method")
        .arg("fast")
        .arg("-y")
        .arg("-v")
        .arg(X26X_FAST_PARAMS)
        .arg("--target-quality")
        .arg("80")
        .arg("--temp")
        .arg(temp_dir2.path())
        .arg("-o")
        .arg(&output_file_80)
        .arg("--log-file")
        .arg("80")
        .assert()
        .success();
    assert!(output_file_80.exists());
    assert!(output_file_80.metadata().unwrap().len() > 0);

    assert!(output_file_95.metadata().unwrap().len() > output_file_80.metadata().unwrap().len());
}

#[test]
#[serial]
fn encode_test_target_quality_aom() {
    let mut cmd = Command::cargo_bin("av1an").unwrap();
    let input_file = input_path();
    let output_file = NamedTempFile::with_suffix(".mkv").unwrap().into_temp_path();
    let temp_dir = TempDir::new().unwrap();

    cmd.arg("-i")
        .arg(&input_file)
        .arg("-e")
        .arg("aom")
        .arg("--pix-format")
        .arg("yuv420p")
        .arg("--sc-method")
        .arg("fast")
        .arg("-y")
        .arg("-v")
        .arg(AOM_FAST_PARAMS)
        .arg("--target-quality")
        .arg("95")
        .arg("--temp")
        .arg(temp_dir.path())
        .arg("-o")
        .arg(&output_file)
        .assert()
        .success();
    assert!(output_file.exists());
    assert!(output_file.metadata().unwrap().len() > 0);
}

#[test]
#[serial]
fn encode_test_target_quality_rav1e() {
    let mut cmd = Command::cargo_bin("av1an").unwrap();
    let input_file = input_path();
    let output_file = NamedTempFile::with_suffix(".mkv").unwrap().into_temp_path();
    let temp_dir = TempDir::new().unwrap();

    cmd.arg("-i")
        .arg(&input_file)
        .arg("-e")
        .arg("rav1e")
        .arg("--pix-format")
        .arg("yuv420p")
        .arg("--sc-method")
        .arg("fast")
        .arg("-y")
        .arg("-v")
        .arg(RAV1E_FAST_PARAMS)
        .arg("--target-quality")
        .arg("95")
        .arg("--temp")
        .arg(temp_dir.path())
        .arg("-o")
        .arg(&output_file)
        .assert()
        .success();
    assert!(output_file.exists());
    assert!(output_file.metadata().unwrap().len() > 0);
}

#[test]
#[serial]
fn encode_test_target_quality_svt_av1() {
    let mut cmd = Command::cargo_bin("av1an").unwrap();
    let input_file = input_path();
    let output_file = NamedTempFile::with_suffix(".mkv").unwrap().into_temp_path();
    let temp_dir = TempDir::new().unwrap();

    cmd.arg("-i")
        .arg(&input_file)
        .arg("-e")
        .arg("svt-av1")
        .arg("--pix-format")
        .arg("yuv420p")
        .arg("--sc-method")
        .arg("fast")
        .arg("-y")
        .arg("-v")
        .arg(SVT_AV1_FAST_PARAMS)
        .arg("--target-quality")
        .arg("95")
        .arg("--temp")
        .arg(temp_dir.path())
        .arg("-o")
        .arg(&output_file)
        .assert()
        .success();
    assert!(output_file.exists());
    assert!(output_file.metadata().unwrap().len() > 0);
}

#[test]
#[serial]
fn encode_test_target_quality_vpx() {
    let mut cmd = Command::cargo_bin("av1an").unwrap();
    let input_file = input_path();
    let output_file = NamedTempFile::with_suffix(".mkv").unwrap().into_temp_path();
    let temp_dir = TempDir::new().unwrap();

    cmd.arg("-i")
        .arg(&input_file)
        .arg("-e")
        .arg("vpx")
        .arg("--pix-format")
        .arg("yuv420p")
        .arg("--sc-method")
        .arg("fast")
        .arg("-y")
        .arg("-v")
        .arg(VPX_FAST_PARAMS)
        .arg("--target-quality")
        .arg("95")
        .arg("--temp")
        .arg(temp_dir.path())
        .arg("-o")
        .arg(&output_file)
        .assert()
        .success();
    assert!(output_file.exists());
    assert!(output_file.metadata().unwrap().len() > 0);
}

#[test]
#[serial]
fn encode_test_target_quality_x265() {
    let mut cmd = Command::cargo_bin("av1an").unwrap();
    let input_file = input_path();
    let output_file = NamedTempFile::with_suffix(".mkv").unwrap().into_temp_path();
    let temp_dir = TempDir::new().unwrap();

    cmd.arg("-i")
        .arg(&input_file)
        .arg("-e")
        .arg("x265")
        .arg("--pix-format")
        .arg("yuv420p")
        .arg("--sc-method")
        .arg("fast")
        .arg("-y")
        .arg("-v")
        .arg(X26X_FAST_PARAMS)
        .arg("--target-quality")
        .arg("95")
        .arg("--temp")
        .arg(temp_dir.path())
        .arg("-o")
        .arg(&output_file)
        .assert()
        .success();
    assert!(output_file.exists());
    assert!(output_file.metadata().unwrap().len() > 0);
}

#[test]
#[serial]
fn encode_test_target_quality_x264() {
    let mut cmd = Command::cargo_bin("av1an").unwrap();
    let input_file = input_path();
    let output_file = NamedTempFile::with_suffix(".mkv").unwrap().into_temp_path();
    let temp_dir = TempDir::new().unwrap();

    cmd.arg("-i")
        .arg(&input_file)
        .arg("-e")
        .arg("x264")
        .arg("--pix-format")
        .arg("yuv420p")
        .arg("--sc-method")
        .arg("fast")
        .arg("-y")
        .arg("-v")
        .arg(X26X_FAST_PARAMS)
        .arg("--target-quality")
        .arg("95")
        .arg("--temp")
        .arg(temp_dir.path())
        .arg("-o")
        .arg(&output_file)
        .assert()
        .success();
    assert!(output_file.exists());
    assert!(output_file.metadata().unwrap().len() > 0);
}

#[test]
#[serial]
fn encode_test_target_quality_replace_crf() {
    let mut cmd = Command::cargo_bin("av1an").unwrap();
    let input_file = input_path();
    let output_file = NamedTempFile::with_suffix(".mkv").unwrap().into_temp_path();
    let temp_dir = TempDir::new().unwrap();

    cmd.arg("-i")
        .arg(&input_file)
        .arg("-e")
        .arg("x264")
        .arg("--pix-format")
        .arg("yuv420p")
        .arg("--sc-method")
        .arg("fast")
        .arg("-y")
        .arg("-v")
        .arg(format!("{X26X_FAST_PARAMS} --crf 0"))
        .arg("--target-quality")
        .arg("95")
        .arg("--temp")
        .arg(temp_dir.path())
        .arg("-o")
        .arg(&output_file)
        .assert()
        .success();
    assert!(output_file.exists());
    assert!(output_file.metadata().unwrap().len() > 0);
}

#[test]
#[serial]
fn encode_test_probe_slow_aom() {
    let mut cmd = Command::cargo_bin("av1an").unwrap();
    let input_file = input_path();
    let output_file = NamedTempFile::with_suffix(".mkv").unwrap().into_temp_path();
    let temp_dir = TempDir::new().unwrap();

    cmd.arg("-i")
        .arg(&input_file)
        .arg("-e")
        .arg("aom")
        .arg("--pix-format")
        .arg("yuv420p")
        .arg("--sc-method")
        .arg("fast")
        .arg("-y")
        .arg("-v")
        .arg(AOM_FAST_PARAMS)
        .arg("--target-quality")
        .arg("95")
        .arg("--probe-slow")
        .arg("--temp")
        .arg(temp_dir.path())
        .arg("-o")
        .arg(&output_file)
        .assert()
        .success();
    assert!(output_file.exists());
    assert!(output_file.metadata().unwrap().len() > 0);
}

#[test]
#[serial]
fn encode_test_probe_slow_rav1e() {
    let mut cmd = Command::cargo_bin("av1an").unwrap();
    let input_file = input_path();
    let output_file = NamedTempFile::with_suffix(".mkv").unwrap().into_temp_path();
    let temp_dir = TempDir::new().unwrap();

    cmd.arg("-i")
        .arg(&input_file)
        .arg("-e")
        .arg("rav1e")
        .arg("--pix-format")
        .arg("yuv420p")
        .arg("--sc-method")
        .arg("fast")
        .arg("-y")
        .arg("-v")
        .arg(RAV1E_FAST_PARAMS)
        .arg("--target-quality")
        .arg("95")
        .arg("--probe-slow")
        .arg("--temp")
        .arg(temp_dir.path())
        .arg("-o")
        .arg(&output_file)
        .assert()
        .success();
    assert!(output_file.exists());
    assert!(output_file.metadata().unwrap().len() > 0);
}

#[test]
#[serial]
fn encode_test_probe_slow_svt_av1() {
    let mut cmd = Command::cargo_bin("av1an").unwrap();
    let input_file = input_path();
    let output_file = NamedTempFile::with_suffix(".mkv").unwrap().into_temp_path();
    let temp_dir = TempDir::new().unwrap();

    cmd.arg("-i")
        .arg(&input_file)
        .arg("-e")
        .arg("svt-av1")
        .arg("--pix-format")
        .arg("yuv420p")
        .arg("--sc-method")
        .arg("fast")
        .arg("-y")
        .arg("-v")
        .arg(SVT_AV1_FAST_PARAMS)
        .arg("--target-quality")
        .arg("95")
        .arg("--probe-slow")
        .arg("--temp")
        .arg(temp_dir.path())
        .arg("-o")
        .arg(&output_file)
        .assert()
        .success();
    assert!(output_file.exists());
    assert!(output_file.metadata().unwrap().len() > 0);
}

#[test]
#[serial]
fn encode_test_probe_slow_vpx() {
    let mut cmd = Command::cargo_bin("av1an").unwrap();
    let input_file = input_path();
    let output_file = NamedTempFile::with_suffix(".mkv").unwrap().into_temp_path();
    let temp_dir = TempDir::new().unwrap();

    cmd.arg("-i")
        .arg(&input_file)
        .arg("-e")
        .arg("vpx")
        .arg("--pix-format")
        .arg("yuv420p")
        .arg("--sc-method")
        .arg("fast")
        .arg("-y")
        .arg("-v")
        .arg(VPX_FAST_PARAMS)
        .arg("--target-quality")
        .arg("95")
        .arg("--probe-slow")
        .arg("--temp")
        .arg(temp_dir.path())
        .arg("-o")
        .arg(&output_file)
        .assert()
        .success();
    assert!(output_file.exists());
    assert!(output_file.metadata().unwrap().len() > 0);
}

#[test]
#[serial]
fn encode_test_probe_slow_x265() {
    let mut cmd = Command::cargo_bin("av1an").unwrap();
    let input_file = input_path();
    let output_file = NamedTempFile::with_suffix(".mkv").unwrap().into_temp_path();
    let temp_dir = TempDir::new().unwrap();

    cmd.arg("-i")
        .arg(&input_file)
        .arg("-e")
        .arg("x265")
        .arg("--pix-format")
        .arg("yuv420p")
        .arg("--sc-method")
        .arg("fast")
        .arg("-y")
        .arg("-v")
        .arg(X26X_FAST_PARAMS)
        .arg("--target-quality")
        .arg("95")
        .arg("--probe-slow")
        .arg("--temp")
        .arg(temp_dir.path())
        .arg("-o")
        .arg(&output_file)
        .assert()
        .success();
    assert!(output_file.exists());
    assert!(output_file.metadata().unwrap().len() > 0);
}

#[test]
#[serial]
fn encode_test_probe_slow_x264() {
    let mut cmd = Command::cargo_bin("av1an").unwrap();
    let input_file = input_path();
    let output_file = NamedTempFile::with_suffix(".mkv").unwrap().into_temp_path();
    let temp_dir = TempDir::new().unwrap();

    cmd.arg("-i")
        .arg(&input_file)
        .arg("-e")
        .arg("x264")
        .arg("--pix-format")
        .arg("yuv420p")
        .arg("--sc-method")
        .arg("fast")
        .arg("-y")
        .arg("-v")
        .arg(X26X_FAST_PARAMS)
        .arg("--target-quality")
        .arg("95")
        .arg("--probe-slow")
        .arg("--temp")
        .arg(temp_dir.path())
        .arg("-o")
        .arg(&output_file)
        .assert()
        .success();
    assert!(output_file.exists());
    assert!(output_file.metadata().unwrap().len() > 0);
}

#[test]
#[serial]
fn encode_test_chunk_hybrid_aom() {
    let mut cmd = Command::cargo_bin("av1an").unwrap();
    let input_file = input_path();
    let output_file = NamedTempFile::with_suffix(".mkv").unwrap().into_temp_path();
    let temp_dir = TempDir::new().unwrap();

    cmd.arg("-i")
        .arg(&input_file)
        .arg("-e")
        .arg("aom")
        .arg("--pix-format")
        .arg("yuv420p")
        .arg("--sc-method")
        .arg("fast")
        .arg("-y")
        .arg("-v")
        .arg(AOM_FAST_PARAMS)
        .arg("--chunk-method")
        .arg("hybrid")
        .arg("--temp")
        .arg(temp_dir.path())
        .arg("-o")
        .arg(&output_file)
        .assert()
        .success();
    assert!(output_file.exists());
    assert!(output_file.metadata().unwrap().len() > 0);
}

#[test]
#[serial]
fn encode_test_chunk_select_aom() {
    let mut cmd = Command::cargo_bin("av1an").unwrap();
    let input_file = input_path();
    let output_file = NamedTempFile::with_suffix(".mkv").unwrap().into_temp_path();
    let temp_dir = TempDir::new().unwrap();

    cmd.arg("-i")
        .arg(&input_file)
        .arg("-e")
        .arg("aom")
        .arg("--pix-format")
        .arg("yuv420p")
        .arg("--sc-method")
        .arg("fast")
        .arg("-y")
        .arg("-v")
        .arg(AOM_FAST_PARAMS)
        .arg("--chunk-method")
        .arg("select")
        .arg("--temp")
        .arg(temp_dir.path())
        .arg("-o")
        .arg(&output_file)
        .assert()
        .success();
    assert!(output_file.exists());
    assert!(output_file.metadata().unwrap().len() > 0);
}

#[test]
#[serial]
fn encode_test_chunk_ffms2_aom() {
    let mut cmd = Command::cargo_bin("av1an").unwrap();
    let input_file = input_path();
    let output_file = NamedTempFile::with_suffix(".mkv").unwrap().into_temp_path();
    let temp_dir = TempDir::new().unwrap();

    cmd.arg("-i")
        .arg(&input_file)
        .arg("-e")
        .arg("aom")
        .arg("--pix-format")
        .arg("yuv420p")
        .arg("--sc-method")
        .arg("fast")
        .arg("-y")
        .arg("-v")
        .arg(AOM_FAST_PARAMS)
        .arg("--chunk-method")
        .arg("ffms2")
        .arg("--temp")
        .arg(temp_dir.path())
        .arg("-o")
        .arg(&output_file)
        .assert()
        .success();
    assert!(output_file.exists());
    assert!(output_file.metadata().unwrap().len() > 0);
}

#[test]
#[serial]
fn encode_test_chunk_lsmash_aom() {
    let mut cmd = Command::cargo_bin("av1an").unwrap();
    let input_file = input_path();
    let output_file = NamedTempFile::with_suffix(".mkv").unwrap().into_temp_path();
    let temp_dir = TempDir::new().unwrap();

    cmd.arg("-i")
        .arg(&input_file)
        .arg("-e")
        .arg("aom")
        .arg("--pix-format")
        .arg("yuv420p")
        .arg("--sc-method")
        .arg("fast")
        .arg("-y")
        .arg("-v")
        .arg(AOM_FAST_PARAMS)
        .arg("--chunk-method")
        .arg("lsmash")
        .arg("--temp")
        .arg(temp_dir.path())
        .arg("-o")
        .arg(&output_file)
        .assert()
        .success();
    assert!(output_file.exists());
    assert!(output_file.metadata().unwrap().len() > 0);
}

#[test]
#[serial]
fn encode_test_scenes_aom() {
    let mut cmd = Command::cargo_bin("av1an").unwrap();
    let input_file = input_path();
    let output_file = NamedTempFile::with_suffix(".mkv").unwrap().into_temp_path();
    let temp_dir = TempDir::new().unwrap();
    let scenes_file = NamedTempFile::with_suffix(".json").unwrap().into_temp_path();
    // delete the temp file. we only need the path. av1an will create the file
    remove_file(&scenes_file).unwrap();

    cmd.arg("-i")
        .arg(&input_file)
        .arg("-e")
        .arg("aom")
        .arg("--pix-format")
        .arg("yuv420p")
        .arg("--sc-method")
        .arg("fast")
        .arg("-y")
        .arg("-v")
        .arg(AOM_FAST_PARAMS)
        .arg("-s")
        .arg(&scenes_file)
        .arg("--temp")
        .arg(temp_dir.path())
        .arg("-o")
        .arg(&output_file)
        .assert()
        .success();
    assert!(output_file.exists());
    assert!(output_file.metadata().unwrap().len() > 0);
    assert!(scenes_file.exists());
    assert!(scenes_file.metadata().unwrap().len() > 0);
}

#[test]
#[serial]
fn encode_test_workers_aom() {
    let mut cmd = Command::cargo_bin("av1an").unwrap();
    let input_file = input_path();
    let output_file = NamedTempFile::with_suffix(".mkv").unwrap().into_temp_path();
    let temp_dir = TempDir::new().unwrap();

    cmd.arg("-i")
        .arg(&input_file)
        .arg("-e")
        .arg("aom")
        .arg("--pix-format")
        .arg("yuv420p")
        .arg("--sc-method")
        .arg("fast")
        .arg("-y")
        .arg("-v")
        .arg(AOM_FAST_PARAMS)
        .arg("-w")
        .arg("2")
        .arg("--temp")
        .arg(temp_dir.path())
        .arg("-o")
        .arg(&output_file)
        .assert()
        .success();
    assert!(output_file.exists());
    assert!(output_file.metadata().unwrap().len() > 0);
}

#[test]
#[serial]
fn encode_test_vmaf_aom() {
    let mut cmd = Command::cargo_bin("av1an").unwrap();
    let input_file = input_path();
    let output_file = NamedTempFile::with_suffix(".mkv").unwrap().into_temp_path();
    let temp_dir = TempDir::new().unwrap();

    cmd.arg("-i")
        .arg(&input_file)
        .arg("-e")
        .arg("aom")
        .arg("--pix-format")
        .arg("yuv420p")
        .arg("--sc-method")
        .arg("fast")
        .arg("-y")
        .arg("-v")
        .arg(AOM_FAST_PARAMS)
        .arg("--vmaf")
        .arg("--temp")
        .arg(temp_dir.path())
        .arg("-o")
        .arg(&output_file)
        .assert()
        .success();
    assert!(output_file.exists());
    assert!(output_file.metadata().unwrap().len() > 0);
}

#[test]
#[serial]
fn encode_test_extra_splits_aom() {
    let mut cmd = Command::cargo_bin("av1an").unwrap();
    let input_file = input_path();
    let output_file = NamedTempFile::with_suffix(".mkv").unwrap().into_temp_path();
    let temp_dir = TempDir::new().unwrap();

    cmd.arg("-i")
        .arg(&input_file)
        .arg("-e")
        .arg("aom")
        .arg("--pix-format")
        .arg("yuv420p")
        .arg("--sc-method")
        .arg("fast")
        .arg("-y")
        .arg("-v")
        .arg(AOM_FAST_PARAMS)
        .arg("-x")
        .arg("10")
        .arg("--temp")
        .arg(temp_dir.path())
        .arg("-o")
        .arg(&output_file)
        .assert()
        .success();
    assert!(output_file.exists());
    assert!(output_file.metadata().unwrap().len() > 0);
}

#[test]
#[serial]
fn encode_test_concat_ffmpeg_aom() {
    let mut cmd = Command::cargo_bin("av1an").unwrap();
    let input_file = input_path();
    let output_file = NamedTempFile::with_suffix(".mkv").unwrap().into_temp_path();
    let temp_dir = TempDir::new().unwrap();

    cmd.arg("-i")
        .arg(&input_file)
        .arg("-e")
        .arg("aom")
        .arg("--pix-format")
        .arg("yuv420p")
        .arg("--sc-method")
        .arg("fast")
        .arg("-y")
        .arg("-v")
        .arg(AOM_FAST_PARAMS)
        .arg("-c")
        .arg("ffmpeg")
        .arg("--temp")
        .arg(temp_dir.path())
        .arg("-o")
        .arg(&output_file)
        .assert()
        .success();
    assert!(output_file.exists());
    assert!(output_file.metadata().unwrap().len() > 0);
}

#[test]
#[serial]
fn encode_test_slow_scenechange_aom() {
    let mut cmd = Command::cargo_bin("av1an").unwrap();
    let input_file = input_path();
    let output_file = NamedTempFile::with_suffix(".mkv").unwrap().into_temp_path();
    let temp_dir = TempDir::new().unwrap();

    cmd.arg("-i")
        .arg(&input_file)
        .arg("-e")
        .arg("aom")
        .arg("--pix-format")
        .arg("yuv420p")
        .arg("-y")
        .arg("-v")
        .arg(AOM_FAST_PARAMS)
        .arg("--temp")
        .arg(temp_dir.path())
        .arg("-o")
        .arg(&output_file)
        .assert()
        .success();
    assert!(output_file.exists());
    assert!(output_file.metadata().unwrap().len() > 0);
}

#[test]
#[serial]
fn encode_test_sc_only() {
    let mut cmd = Command::cargo_bin("av1an").unwrap();
    let input_file = input_path();
    let output_file = NamedTempFile::with_suffix(".mkv").unwrap().into_temp_path();
    let scenes_file = NamedTempFile::with_suffix(".json").unwrap().into_temp_path();
    // delete the temp file. we only need the path. av1an will create the file
    remove_file(&scenes_file).unwrap();
    let temp_dir = TempDir::new().unwrap();

    cmd.arg("-i")
        .arg(&input_file)
        .arg("-e")
        .arg("aom")
        .arg("--pix-format")
        .arg("yuv420p")
        .arg("--sc-method")
        .arg("fast")
        .arg("-y")
        .arg("-s")
        .arg(&scenes_file)
        .arg("--sc-only")
        .arg("--temp")
        .arg(temp_dir.path())
        .arg("-o")
        .arg(&output_file)
        .assert()
        .success();
    assert!(scenes_file.exists());
    assert!(scenes_file.metadata().unwrap().len() > 0);

    let mut cmd = Command::cargo_bin("av1an").unwrap();
    cmd.arg("-i")
        .arg(&input_file)
        .arg("-e")
        .arg("aom")
        .arg("--pix-format")
        .arg("yuv420p")
        .arg("--sc-method")
        .arg("fast")
        .arg("-y")
        .arg("-v")
        .arg(AOM_FAST_PARAMS)
        .arg("-s")
        .arg(&scenes_file)
        .arg("--temp")
        .arg(temp_dir.path())
        .arg("-o")
        .arg(&output_file)
        .assert()
        .success();
    assert!(output_file.exists());
    assert!(output_file.metadata().unwrap().len() > 0);
}

#[test]
#[serial]
fn encode_test_vpy_input() {
    let mut cmd = Command::cargo_bin("av1an").unwrap();
    let output_file = NamedTempFile::with_suffix(".mkv").unwrap().into_temp_path();
    let temp_dir = TempDir::new().unwrap();
    let input_file = create_vpy_input();

    cmd.arg("-i")
        .arg(&input_file)
        .arg("-e")
        .arg("aom")
        .arg("--pix-format")
        .arg("yuv420p")
        .arg("--sc-method")
        .arg("fast")
        .arg("-y")
        .arg("--temp")
        .arg(temp_dir.path())
        .arg("-o")
        .arg(&output_file)
        .assert()
        .success();
    assert!(output_file.exists());
    assert!(output_file.metadata().unwrap().len() > 0);
}

#[test]
#[serial]
fn encode_test_sc_downscale() {
    let mut cmd = Command::cargo_bin("av1an").unwrap();
    let output_file = NamedTempFile::with_suffix(".mkv").unwrap().into_temp_path();
    let temp_dir = TempDir::new().unwrap();
    let input_file = input_path();

    cmd.arg("-i")
        .arg(&input_file)
        .arg("-e")
        .arg("aom")
        .arg("--pix-format")
        .arg("yuv420p")
        .arg("--sc-method")
        .arg("fast")
        .arg("--sc-downscale-height")
        .arg("160")
        .arg("-y")
        .arg("--temp")
        .arg(temp_dir.path())
        .arg("-o")
        .arg(&output_file)
        .assert()
        .success();
    assert!(output_file.exists());
    assert!(output_file.metadata().unwrap().len() > 0);
}

#[test]
#[serial]
fn encode_test_sc_pix_format() {
    let mut cmd = Command::cargo_bin("av1an").unwrap();
    let output_file = NamedTempFile::with_suffix(".mkv").unwrap().into_temp_path();
    let temp_dir = TempDir::new().unwrap();
    let input_file = input_path();

    cmd.arg("-i")
        .arg(&input_file)
        .arg("-e")
        .arg("aom")
        .arg("--pix-format")
        .arg("yuv420p")
        .arg("--sc-method")
        .arg("fast")
        .arg("--sc-pix-format")
        .arg("yuv420p10le")
        .arg("-y")
        .arg("--temp")
        .arg(temp_dir.path())
        .arg("-o")
        .arg(&output_file)
        .assert()
        .success();
    assert!(output_file.exists());
    assert!(output_file.metadata().unwrap().len() > 0);
}

#[test]
#[serial]
fn encode_test_sc_downscale_vpy_input() {
    let mut cmd = Command::cargo_bin("av1an").unwrap();
    let output_file = NamedTempFile::with_suffix(".mkv").unwrap().into_temp_path();
    let temp_dir = TempDir::new().unwrap();
    let input_file = create_vpy_input();

    cmd.arg("-i")
        .arg(&input_file)
        .arg("-e")
        .arg("aom")
        .arg("--pix-format")
        .arg("yuv420p")
        .arg("--sc-method")
        .arg("fast")
        .arg("--sc-downscale-height")
        .arg("160")
        .arg("-y")
        .arg("--temp")
        .arg(temp_dir.path())
        .arg("-o")
        .arg(&output_file)
        .assert()
        .success();
    assert!(output_file.exists());
    assert!(output_file.metadata().unwrap().len() > 0);
}

#[test]
#[serial]
fn encode_test_sc_pix_format_vpy_input() {
    let mut cmd = Command::cargo_bin("av1an").unwrap();
    let output_file = NamedTempFile::with_suffix(".mkv").unwrap().into_temp_path();
    let temp_dir = TempDir::new().unwrap();
    let input_file = create_vpy_input();

    cmd.arg("-i")
        .arg(&input_file)
        .arg("-e")
        .arg("aom")
        .arg("--pix-format")
        .arg("yuv420p")
        .arg("--sc-method")
        .arg("fast")
        .arg("--sc-pix-format")
        .arg("yuv420p10le")
        .arg("-y")
        .arg("--temp")
        .arg(temp_dir.path())
        .arg("-o")
        .arg(&output_file)
        .assert()
        .success();
    assert!(output_file.exists());
    assert!(output_file.metadata().unwrap().len() > 0);
}
