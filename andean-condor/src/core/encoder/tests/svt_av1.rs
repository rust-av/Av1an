use super::*;
use crate::core::encoder::tests::support::run_bounded;
use std::fs;

#[test]
fn svt_exact_pass_parameters_and_argv() {
    // A single-dash stats option is rejected by SVT, even when it exits zero.
    let output = Path::new("job space 映像/scene0.ivf");
    let base = std::env::current_dir().expect("cwd").join("job space 映像/scene0");
    let stats = format!("{}.stat", base.display());
    let mut encoder = Encoder::default_from_base(&EncoderBase::SVTAV1, true);
    encoder.set_append_color_data(false);
    encoder.parameters_mut().extend(CLIParameter::new_numbers(
        "--",
        " ",
        &[("rc", 1.0), ("keyint", 48.0)],
    ));
    for pass in [(1, 2), (2, 2), (1, 1)] {
        *encoder.passes_mut().expect("passes") = EncoderPasses::All(pass.1);
        let expected = if pass.1 == 1 {
            HashMap::new()
        } else {
            HashMap::from([
                (
                    "irefresh-type".into(),
                    CLIParameter::new_number("--", " ", 2.0),
                ),
                (
                    "pass".into(),
                    CLIParameter::new_number("--", " ", f64::from(pass.0)),
                ),
                ("stats".into(), CLIParameter::new_string("--", " ", &stats)),
            ])
        };
        assert_eq!(
            EncoderBase::SVTAV1
                .pass_parameters(pass, base.to_str().expect("utf8"))
                .expect("params"),
            expected,
            "{pass:?}"
        );
        let argv = encoder
            .parameters_with_output(pass, output, &crate::core::encoder::tests::clip_info())
            .expect("argv");
        assert!(!argv.iter().any(|arg| matches!(arg.as_str(), "-stats" | "--quiet")));
        assert_eq!(argv.len() % 2, 0, "separate key/value pairs: {argv:?}");
        let mut actual: Vec<_> = argv.as_chunks::<2>().0.iter().map(|pair| pair.to_vec()).collect();
        let mut expected = vec![
            vec!["-i".to_owned(), "stdin".to_owned()],
            vec![
                "-b".into(),
                if pass.0 == pass.1 {
                    output.to_str().expect("utf8")
                } else {
                    NULL_OUTPUT
                }
                .into(),
            ],
            vec!["--progress".into(), "2".into()],
            vec!["--rc".into(), "1".into()],
            vec!["--keyint".into(), "48".into()],
        ];
        if pass.1 == 2 {
            expected.extend([
                vec!["--irefresh-type".into(), "2".into()],
                vec!["--pass".into(), pass.0.to_string()],
                vec!["--stats".into(), stats.clone()],
            ]);
        }
        actual.sort();
        expected.sort();
        assert_eq!(actual, expected, "{pass:?}");
    }
}

#[test]
fn installed_svt_vbr_two_pass_output_decodes_all_frames() {
    let temp = tempfile::tempdir().expect("fixture");
    let root = temp.path();
    let svt = which::which("SvtAv1EncApp").expect("install SVT-AV1 to run the native regression");
    let ffprobe = which::which("ffprobe").expect("install ffprobe to verify decoded frames");
    let help = run_bounded(Command::new(&svt).arg("--help"), &root.join("help.log"));
    for option in ["--stats ", "--pass ", "pass 2 is only available for VBR"] {
        assert!(help.contains(option), "missing {option}: {help}");
    }
    assert!(!help.split_whitespace().any(|word| word == "-stats"));
    let version = run_bounded(
        Command::new(&svt).arg("--version"),
        &root.join("version.log"),
    );
    eprintln!("SVT-AV1 installed {}: {version}", svt.display());

    let job = root.join("job space 映像");
    let cwd = root.join("cwd");
    fs::create_dir_all(&job).expect("job");
    fs::create_dir_all(&cwd).expect("cwd");
    fs::write(cwd.join("scene0.stat"), "unrelated CWD sentinel").expect("sentinel");
    let input = root.join("four frames.y4m");
    let mut y4m = b"YUV4MPEG2 W64 H64 F24:1 Ip A1:1 C420jpeg\n".to_vec();
    for frame in 0..4 {
        y4m.extend_from_slice(b"FRAME\n");
        y4m.extend(vec![32 + frame * 48; 64 * 64]);
        y4m.extend(vec![128; 64 * 64 / 2]);
    }
    fs::write(&input, y4m).expect("finite Y4M");
    let output = job.join("scene0.ivf");
    let stats = PassStatistics::new(&output).expect("shared statistics context");
    let stats_file = PathBuf::from(format!("{}.stat", stats.base().display()));
    assert!(!stats_file.exists());
    let mut encoder = Encoder::default_from_base(&EncoderBase::SVTAV1, false);
    *encoder.passes_mut().expect("passes") = EncoderPasses::All(2);
    encoder.set_append_color_data(false);
    encoder.parameters_mut().remove("crf");
    encoder.parameters_mut().extend(CLIParameter::new_numbers(
        "--",
        " ",
        &[
            ("rc", 1.0),
            ("preset", 9.0),
            ("pred-struct", 2.0),
            ("keyint", 48.0),
            ("tbr", 100.0),
            ("lp", 1.0),
            ("frames", 4.0),
        ],
    ));
    let mut first_stats = Vec::new();
    // ponytail: manual production argv isolates the stats fix from the separate
    // input-backed All(2) orchestration repair; reopen finite stdin per pass.
    for pass in [1, 2] {
        let argv = encoder
            .parameters_with_stats(
                (pass, 2),
                &output,
                &crate::core::encoder::tests::clip_info(),
                stats.base(),
            )
            .expect("production argv");
        eprintln!("SVT-AV1 pass {pass} argv: {argv:?}");
        let log = run_bounded(
            Command::new(&svt)
                .args(&argv)
                .stdin(fs::File::open(&input).expect("reopen input"))
                .current_dir(&cwd),
            &root.join(format!("pass{pass}.log")),
        );
        eprintln!("SVT-AV1 pass {pass} native log:\n{log}");
        assert!(
            !log.to_ascii_lowercase().contains("error") && !log.contains("Unprocessed tokens"),
            "SVT can report errors and exit zero: {log}"
        );
        let bytes = fs::read(&stats_file).expect("shared native statistics");
        assert!(!bytes.is_empty(), "pass {pass}: empty statistics");
        eprintln!(
            "SVT-AV1 pass {pass}: success, shared statistics {} bytes",
            bytes.len()
        );
        if pass == 1 {
            first_stats = bytes;
        } else {
            assert_eq!(
                bytes, first_stats,
                "pass two must not rewrite first-pass statistics"
            );
        }
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
    assert_eq!(probe.trim(), "codec_name=av1\nnb_read_frames=4");
    eprintln!("SVT-AV1 decoded final output: {probe}");
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
