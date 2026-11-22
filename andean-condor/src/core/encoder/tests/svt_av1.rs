use super::*;

#[test]
fn svt_av1_pass_argv_uses_native_stats_and_final_output_only() {
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
            .parameters_with_output(
                pass,
                output,
                &crate::core::encoder::tests::clip_info(),
                None,
            )
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
