use super::*;

fn encoder(base: EncoderBase) -> Encoder {
    let mut encoder = Encoder::default_from_base(&base, true);
    *encoder.passes_mut().expect("passes") = EncoderPasses::All(2);
    if base == EncoderBase::SVTAV1 {
        encoder.parameters_mut().extend(CLIParameter::new_numbers(
            "--",
            " ",
            &[("rc", 1.0), ("keyint", 48.0)],
        ));
    }
    if base == EncoderBase::VVenC {
        encoder.parameters_mut().extend(CLIParameter::new_numbers(
            "--",
            " ",
            &[("bitrate", 1000.0)],
        ));
    }
    encoder
}

fn value(argv: &[String], key: &str) -> String {
    argv.iter()
        .enumerate()
        .find_map(|(index, arg)| {
            arg.strip_prefix(&format!("{key}="))
                .map(str::to_owned)
                .or_else(|| (arg == key).then(|| argv[index + 1].clone()))
        })
        .unwrap_or_else(|| panic!("missing {key}: {argv:?}"))
}

#[test]
fn encoder_statistics_argv_preserves_job_paths_and_native_suffixes() {
    // Dropping the output parent would make all jobs' scene0 statistics collide.
    for output in [PathBuf::from("scene0"), PathBuf::from("job space 映像/scene0.ivf")] {
        let base_path = std::env::current_dir().expect("cwd").join(&output).with_extension("");
        for (base, first, second, suffix) in [
            (EncoderBase::AOM, "--fpf", "--fpf", ".log"),
            (EncoderBase::VPX, "--fpf", "--fpf", ".log"),
            (EncoderBase::RAV1E, "--first-pass", "--second-pass", ".stat"),
            (EncoderBase::SVTAV1, "--stats", "--stats", ".stat"),
            (EncoderBase::X264, "--stats", "--stats", ".log"),
            (EncoderBase::X265, "--stats", "--stats", ".log"),
            (
                EncoderBase::VVenC,
                "--rcstatsfile",
                "--rcstatsfile",
                ".json",
            ),
        ] {
            let encoder = encoder(base);
            for (pass, key) in [(1, first), (2, second)] {
                let argv = encoder
                    .parameters_with_output(
                        (pass, 2),
                        &output,
                        &crate::core::encoder::tests::clip_info(),
                        None,
                    )
                    .expect("valid argv");
                assert_eq!(
                    value(&argv, key),
                    format!("{}{suffix}", base_path.display()),
                    "{base}/{pass}"
                );
                if base == EncoderBase::X265 {
                    assert_eq!(
                        value(&argv, "--analysis-reuse-file"),
                        format!("{}_analysis.dat", base_path.display())
                    );
                }
                let explicit = base_path.with_file_name("shared attempt 映像");
                let parameters = base
                    .pass_parameters((pass, 2), explicit.to_str().expect("utf8"))
                    .expect("pure pass parameters");
                assert_eq!(
                    parameters[key.trim_start_matches('-')].to_string_value(),
                    format!("{}{suffix}", explicit.display())
                );
                let argv = encoder
                    .parameters_with_output(
                        (pass, 2),
                        &output,
                        &crate::core::encoder::tests::clip_info(),
                        Some(&explicit),
                    )
                    .expect("explicit stats argv");
                assert_eq!(value(&argv, key), format!("{}{suffix}", explicit.display()));
            }
        }
    }
}

#[test]
fn native_statistics_path_options_override_generated_defaults() {
    let output = PathBuf::from("job space 映像/scene0");
    for (base, key) in [
        (EncoderBase::AOM, "fpf"),
        (EncoderBase::VPX, "fpf"),
        (EncoderBase::RAV1E, "first-pass"),
        (EncoderBase::SVTAV1, "stats"),
        (EncoderBase::X264, "stats"),
        (EncoderBase::X265, "analysis-reuse-file"),
        (EncoderBase::VVenC, "rcstatsfile"),
    ] {
        let mut encoder = encoder(base);
        encoder.parameters_mut().insert(
            key.into(),
            CLIParameter::new_string("--", " ", "caller relative 映像.stats"),
        );
        let argv = encoder
            .parameters_with_output(
                (1, 2),
                &output,
                &crate::core::encoder::tests::clip_info(),
                Some(Path::new("generated base")),
            )
            .expect("argv");
        assert_eq!(
            value(&argv, &format!("--{key}")),
            "caller relative 映像.stats"
        );
    }
    let mut encoder = encoder(EncoderBase::VVenC);
    *encoder.passes_mut().expect("passes") = EncoderPasses::All(1);
    encoder.parameters_mut().insert(
        "rcstatsfile".into(),
        CLIParameter::new_string("--", " ", "manual VVenC.json"),
    );
    for pass in [1.0, 2.0] {
        encoder
            .parameters_mut()
            .insert("pass".into(), CLIParameter::new_number("--", " ", pass));
        let argv = encoder
            .parameters_with_output(
                (1, 1),
                &output,
                &crate::core::encoder::tests::clip_info(),
                None,
            )
            .expect("native manual tuple");
        assert_eq!(value(&argv, "--rcstatsfile"), "manual VVenC.json");
        assert_eq!(value(&argv, "--pass"), pass.to_string());
    }
}
