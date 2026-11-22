use super::*;

#[test]
fn aom_default_keyframe_flag_respects_false_removal_and_excluded_defaults() {
    for mode in ["default", "false", "removed", "excluded"] {
        let mut encoder = Encoder::default_from_base(&EncoderBase::AOM, mode == "excluded");
        match mode {
            "false" => {
                encoder
                    .parameters_mut()
                    .insert("disable-kf".into(), CLIParameter::new_bool("--", false));
            },
            "removed" => {
                encoder.parameters_mut().remove("disable-kf");
            },
            _ => {},
        }
        for pass in [(1, 1), (1, 2), (2, 2)] {
            *encoder.passes_mut().expect("passes") = EncoderPasses::All(pass.1);
            let before = encoder.clone();
            for argv in [
                encoder.parameters_with_output(
                    pass,
                    Path::new("out.ivf"),
                    &crate::core::encoder::tests::clip_info(),
                    None,
                ),
                encoder.parameters_with_output(
                    pass,
                    Path::new("out.ivf"),
                    &crate::core::encoder::tests::clip_info(),
                    Some(Path::new("stats")),
                ),
            ] {
                let argv = argv.expect("production argv");
                assert_eq!(
                    argv.iter().filter(|arg| arg.starts_with("--disable-kf")).count(),
                    usize::from(mode == "default"),
                    "{mode} {pass:?}: {argv:?}"
                );
                assert_eq!(
                    argv.contains(&"--kf-max-dist=9999".to_owned()),
                    mode != "excluded",
                    "{mode} {pass:?}: {argv:?}"
                );
            }
            assert_eq!(encoder, before, "argv must not override caller options");
        }
    }
}

#[test]
fn aom_pass_argv_uses_native_statistics_and_final_output_only() {
    // Omitting --passes=1 lets aomenc attempt to reread exhausted piped stdin.
    let output = Path::new("job space 映像/scene0.ivf");
    let base = std::env::current_dir().expect("cwd").join("job space 映像/scene0");
    let stats = format!("{}.log", base.display());
    let mut encoder = Encoder::default_from_base(&EncoderBase::AOM, true);
    encoder.set_append_color_data(false);
    for pass in [(1, 2), (2, 2), (1, 1)] {
        *encoder.passes_mut().expect("passes") = EncoderPasses::All(pass.1);
        let mut expected = HashMap::from([(
            "passes".into(),
            CLIParameter::new_number("--", "=", f64::from(pass.1)),
        )]);
        if pass.1 == 2 {
            expected.extend([
                (
                    "pass".into(),
                    CLIParameter::new_number("--", "=", f64::from(pass.0)),
                ),
                ("fpf".into(), CLIParameter::new_string("--", "=", &stats)),
            ]);
        }
        assert_eq!(
            EncoderBase::AOM
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
        let mut actual = argv[1..].to_vec();
        let mut expected = vec![
            "-o".to_owned(),
            if pass.0 == pass.1 {
                output.to_str().expect("utf8")
            } else {
                NULL_OUTPUT
            }
            .to_owned(),
            format!("--passes={}", pass.1),
        ];
        if pass.1 == 2 {
            expected.extend([format!("--pass={}", pass.0), format!("--fpf={stats}")]);
        }
        actual.sort();
        expected.sort();
        assert_eq!(actual, expected, "{pass:?}");
    }
}
