use super::*;

#[test]
fn rav1e_pass_argv_uses_first_and_second_pass_statistics_flags() {
    // Reintroducing --passes/--pass, losing the parent, or splitting the stats
    // filename would produce argv that rav1e cannot use for its second pass.
    let output = Path::new("job space 映像/scene0.ivf");
    let base = std::env::current_dir().expect("cwd").join("job space 映像/scene0");
    let stats = format!("{}.stat", base.display());
    let mut encoder = Encoder::default_from_base(&EncoderBase::RAV1E, true);
    encoder.set_append_color_data(false);
    for (pass, key) in [((1, 2), Some("first-pass")), ((2, 2), Some("second-pass")), ((1, 1), None)]
    {
        *encoder.passes_mut().expect("passes") = EncoderPasses::All(pass.1);
        let expected = key.map_or_else(HashMap::new, |key| {
            HashMap::from([
                ("quiet".into(), CLIParameter::new_bool("--", true)),
                (key.into(), CLIParameter::new_string("--", " ", &stats)),
            ])
        });
        assert_eq!(
            EncoderBase::RAV1E
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
        assert_eq!(argv[0], "-");
        assert!(
            !argv
                .iter()
                .any(|arg| matches!(arg.split('=').next(), Some("--passes" | "--pass")))
        );
        let mut tokens = argv[1..].iter();
        let mut actual = Vec::new();
        while let Some(key) = tokens.next() {
            actual.push(if key == "--quiet" {
                vec![key.clone()]
            } else {
                vec![key.clone(), tokens.next().expect("separate value").clone()]
            });
        }
        let mut expected = vec![vec![
            "--output".to_owned(),
            if pass.0 == pass.1 {
                output.to_str().expect("utf8")
            } else {
                NULL_OUTPUT
            }
            .to_owned(),
        ]];
        if let Some(key) = key {
            expected.extend([vec!["--quiet".into()], vec![format!("--{key}"), stats.clone()]]);
        }
        actual.sort();
        expected.sort();
        assert_eq!(actual, expected, "{pass:?}");
    }
}
