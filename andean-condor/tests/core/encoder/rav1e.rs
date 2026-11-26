use super::support::run_bounded;
use super::*;
use std::{fs, path::PathBuf};

#[test]
fn rav1e_two_pass_output_decodes_three_av1_frames() {
    let temp = tempfile::tempdir().expect("fixture");
    let root = temp.path();
    let rav1e = which::which("rav1e").expect("install rav1e to run the native regression");
    let ffprobe = which::which("ffprobe").expect("install ffprobe to verify decoded frames");
    let help = run_bounded(Command::new(&rav1e).arg("--help"), &root.join("help.log"));
    for option in ["--first-pass <STATS_FILE>", "--second-pass <STATS_FILE>", "--quiet"] {
        assert!(help.contains(option), "missing {option}: {help}");
    }
    assert!(!help.split_whitespace().any(|word| matches!(word, "--passes" | "--pass")));
    let version = run_bounded(
        Command::new(&rav1e).arg("--version"),
        &root.join("version.log"),
    );
    eprintln!("rav1e installed {}: {version}", rav1e.display());

    let job = root.join("job space 映像");
    let cwd = root.join("cwd");
    fs::create_dir_all(&job).expect("job");
    fs::create_dir_all(&cwd).expect("cwd");
    fs::write(cwd.join("scene0.stat"), "unrelated CWD sentinel").expect("sentinel");
    let input = root.join("three frames.y4m");
    let mut y4m = b"YUV4MPEG2 W64 H64 F24:1 Ip A1:1 C420jpeg\n".to_vec();
    for frame in 0..3 {
        y4m.extend_from_slice(b"FRAME\n");
        y4m.extend(vec![32 + frame * 64; 64 * 64]);
        y4m.extend(vec![128; 64 * 64 / 2]);
    }
    fs::write(&input, y4m).expect("finite Y4M");
    let output = job.join("scene0.ivf");
    let stats = PassStatistics::new(&output).expect("shared statistics context");
    let stats_file = PathBuf::from(format!("{}.stat", stats.base().display()));
    let mut encoder = Encoder::default_from_base(&EncoderBase::RAV1E, true);
    *encoder.passes_mut().expect("passes") = EncoderPasses::All(2);
    encoder.set_append_color_data(false);
    encoder.parameters_mut().extend(CLIParameter::new_numbers(
        "--",
        " ",
        &[("speed", 10.0), ("threads", 1.0), ("bitrate", 100.0)],
    ));
    // Use the production argv with fresh file-backed stdin for each manual pass;
    // the independent input-backed All(2) loop is not part of this regression.
    for pass in [1, 2] {
        let argv = encoder
            .parameters_with_output((pass, 2), &output, &super::clip_info(), Some(stats.base()))
            .expect("production argv");
        eprintln!("rav1e pass {pass} argv: {argv:?}");
        run_bounded(
            Command::new(&rav1e)
                .args(&argv)
                .stdin(fs::File::open(&input).expect("reopen input"))
                .current_dir(&cwd),
            &root.join(format!("pass{pass}.log")),
        );
        let bytes = fs::metadata(&stats_file).expect("shared native statistics").len();
        assert!(bytes > 0);
        eprintln!("rav1e pass {pass}: success, shared statistics {bytes} bytes");
    }
    let probe = run_bounded(
        Command::new(ffprobe)
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-count_frames",
                "-show_entries",
                "stream=codec_name,nb_read_frames",
                "-of",
                "default=noprint_wrappers=1",
            ])
            .arg(&output)
            .stdin(Stdio::null()),
        &root.join("probe.log"),
    );
    assert_eq!(probe.trim(), "codec_name=av1\nnb_read_frames=3");
    eprintln!("rav1e decoded final output: {probe}");
    let owned_dir = stats.base().parent().expect("private dir").to_path_buf();
    drop(stats);
    assert!(!owned_dir.exists(), "owned native statistics cleaned");
    assert_eq!(fs::read_dir(&job).expect("job listing").count(), 1);
    assert_eq!(fs::read_dir(&cwd).expect("cwd listing").count(), 1);
    assert_eq!(
        fs::read_to_string(cwd.join("scene0.stat")).expect("sentinel"),
        "unrelated CWD sentinel"
    );
}
