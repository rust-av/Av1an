use crate::models::encoder::{Encoder, EncoderBase, EncoderPasses, cli_parameter::CLIParameter};

#[test]
fn avm_legacy_noise_is_preserved_rejected_and_clearable() {
    use crate::models::encoder::EncoderSettingsError;
    let json =
        r#"{"AVM":{"pass":{"All":1},"options":{},"photon_noise":{"iso":800,"chroma_iso":400}}}"#;
    let mut encoder: Encoder = serde_json::from_str(json).expect("legacy AVM settings");
    let noise = encoder.photon_noise().expect("preserve saved intent").clone();
    assert_eq!(noise.iso, 800);
    assert_eq!(noise.chroma_iso, Some(400));
    assert_eq!(
        encoder.validate_settings(),
        Err(EncoderSettingsError::PhotonNoiseNotSupported(
            EncoderBase::AVM
        ))
    );
    let saved = serde_json::to_value(&encoder).expect("save invalid legacy settings");
    assert_eq!(saved["AVM"]["photon_noise"]["iso"], 800);
    encoder.set_photon_noise(None);
    assert_eq!(encoder.validate_settings(), Ok(()));
    assert!(encoder.photon_noise().is_none());
    encoder.set_photon_noise(Some(noise));
    assert_eq!(
        encoder.validate_settings(),
        Err(EncoderSettingsError::PhotonNoiseNotSupported(
            EncoderBase::AVM
        ))
    );
    assert!(
        encoder.photon_noise().is_some(),
        "setter must not discard the request"
    );
}

#[test]
fn vvenc_frontend_pass_selection_rejects_conflicting_native_overrides() {
    use crate::models::encoder::EncoderSettingsError;
    for (name, value) in [("passes", 1.0), ("p", 1.0), ("pass", 1.0), ("pass", 2.0)] {
        let mut encoder = Encoder::default_from_base(&EncoderBase::VVenC, false);
        *encoder.passes_mut().expect("passes") = EncoderPasses::All(2);
        encoder.parameters_mut().extend(CLIParameter::new_numbers(
            "--",
            " ",
            &[("bitrate", 1000.0), (name, value)],
        ));
        assert!(
            matches!(encoder.validate_settings(), Err(EncoderSettingsError::UnsupportedMode(EncoderBase::VVenC, reason)) if reason.contains("frontend")),
            "All(2), {name}={value}"
        );
    }
    for (name, passes) in [("passes", "2"), ("passes", "-1"), ("p", "2"), ("p", "-1")] {
        let mut encoder = Encoder::default_from_base(&EncoderBase::VVenC, false);
        *encoder.passes_mut().expect("passes") = EncoderPasses::All(2);
        encoder
            .parameters_mut()
            .insert(name.into(), CLIParameter::new_string("--", " ", passes));
        encoder.parameters_mut().insert(
            "bitrate".into(),
            CLIParameter::new_string("--", " ", "1.5Mbps"),
        );
        assert_eq!(encoder.validate_settings(), Ok(()));
    }
    for selection in [
        EncoderPasses::All(1),
        EncoderPasses::Specific(1, 2),
        EncoderPasses::Specific(2, 2),
    ] {
        let mut encoder = Encoder::default_from_base(&EncoderBase::VVenC, false);
        *encoder.passes_mut().expect("passes") = selection;
        let current = match selection {
            EncoderPasses::Specific(current, _) => current,
            _ => 2,
        };
        encoder.parameters_mut().extend(CLIParameter::new_numbers(
            "--",
            " ",
            &[("passes", 2.0), ("pass", f64::from(current)), ("bitrate", 1000.0)],
        ));
        assert_eq!(encoder.validate_settings(), Ok(()), "manual {selection:?}");
    }
    let mut encoder = Encoder::default_from_base(&EncoderBase::VVenC, false);
    *encoder.passes_mut().expect("passes") = EncoderPasses::Specific(2, 2);
    encoder.parameters_mut().extend(CLIParameter::new_numbers(
        "--",
        " ",
        &[("passes", 2.0), ("pass", 1.0), ("bitrate", 1000.0)],
    ));
    assert!(
        matches!(encoder.validate_settings(), Err(EncoderSettingsError::UnsupportedMode(EncoderBase::VVenC, reason)) if reason.contains("frontend pass"))
    );
}

#[test]
fn vvenc_bitrate_units_resolve_to_finite_positive_bits() {
    use crate::models::encoder::EncoderSettingsError;
    let mut encoder = Encoder::default_from_base(&EncoderBase::VVenC, false);
    *encoder.passes_mut().expect("passes") = EncoderPasses::All(2);
    let error = encoder.validate_settings().expect_err("two-pass needs rate control");
    assert!(
        matches!(error, EncoderSettingsError::UnsupportedMode(EncoderBase::VVenC, reason) if reason.contains("bitrate"))
    );
    for value in ["0", "0k", "0Mbps", "0.0001k"] {
        encoder
            .parameters_mut()
            .insert("bitrate".into(), CLIParameter::new_string("--", " ", value));
        assert!(
            matches!(encoder.validate_settings(), Err(EncoderSettingsError::UnsupportedMode(EncoderBase::VVenC, reason)) if reason.contains("bitrate")),
            "zero actual bits: {value}"
        );
    }
    for value in [
        "1",
        "1000",
        "1.5M",
        "1,5Mbps",
        "1500k",
        "1500kbps",
        "1500000bps",
        "0.0005k",
        "0.0000005Mbps",
    ] {
        encoder
            .parameters_mut()
            .insert("bitrate".into(), CLIParameter::new_string("--", " ", value));
        assert_eq!(encoder.validate_settings(), Ok(()), "native units {value}");
    }
    for value in [
        "-1",
        "-1M",
        "NaN",
        "inf",
        "infMbps",
        "1000junk",
        "1.5",
        "1e3M",
        "2147483648",
        "3000M",
    ] {
        encoder
            .parameters_mut()
            .insert("bitrate".into(), CLIParameter::new_string("--", " ", value));
        assert!(
            matches!(encoder.validate_settings(), Err(EncoderSettingsError::InvalidNumber { name, .. }) if name == "bitrate"),
            "malformed bitrate {value}"
        );
    }
    encoder.parameters_mut().clear();
    encoder
        .parameters_mut()
        .insert("b".into(), CLIParameter::new_string("-", " ", "1.5Mbps"));
    assert_eq!(encoder.validate_settings(), Ok(()), "short bitrate alias");
    encoder.parameters_mut().insert(
        "bitrate".into(),
        CLIParameter::new_number("--", " ", 1500000.0),
    );
    assert_eq!(encoder.validate_settings(), Ok(()), "equivalent aliases");
    encoder
        .parameters_mut()
        .insert("bitrate".into(), CLIParameter::new_number("--", " ", 0.0));
    assert!(
        matches!(encoder.validate_settings(), Err(EncoderSettingsError::UnsupportedMode(EncoderBase::VVenC, reason)) if reason.contains("bitrate")),
        "conflicting aliases must not depend on order"
    );
}

#[test]
fn encoder_settings_reject_invalid_passes_rate_control_and_numeric_options() {
    for (base, passes, key, value) in [
        (EncoderBase::AOM, EncoderPasses::All(0), "cq-level", 30.0),
        (EncoderBase::AOM, EncoderPasses::All(3), "cq-level", 30.0),
        (
            EncoderBase::AOM,
            EncoderPasses::Specific(0, 2),
            "cq-level",
            30.0,
        ),
        (
            EncoderBase::AOM,
            EncoderPasses::Specific(3, 2),
            "cq-level",
            30.0,
        ),
        (EncoderBase::SVTAV1, EncoderPasses::All(2), "crf", 25.0),
        (EncoderBase::SVTAV1, EncoderPasses::All(1), "rc", 1.5),
        (EncoderBase::SVTAV1, EncoderPasses::All(1), "crf", 70.25),
        (
            EncoderBase::RAV1E,
            EncoderPasses::All(1),
            "quantizer",
            256.0,
        ),
        (
            EncoderBase::X264,
            EncoderPasses::All(1),
            "custom",
            f64::INFINITY,
        ),
        (EncoderBase::X265, EncoderPasses::All(1), "crf", f64::NAN),
    ] {
        let mut encoder = Encoder::default_from_base(&base, false);
        *encoder.passes_mut().expect("passes") = passes;
        encoder
            .parameters_mut()
            .insert(key.into(), CLIParameter::new_number("--", " ", value));
        assert!(
            encoder.validate_settings().is_err(),
            "accepted {base}/{passes:?}/{key}={value}"
        );
    }
}

#[test]
fn known_numeric_strings_and_types_cannot_bypass_validation() {
    for (base, name, values) in [
        (EncoderBase::RAV1E, "speed", vec!["11", "1.5", "fast"]),
        (EncoderBase::SVTAV1, "rc", vec!["3", "1.5", "NaN"]),
        (
            EncoderBase::SVTAV1,
            "crf",
            vec!["-0.25", "70.25", "25.1", "inf"],
        ),
        (EncoderBase::X264, "crf", vec!["-13", "52"]),
        (EncoderBase::X265, "crf", vec!["-25", "52"]),
        (EncoderBase::AVM, "qp", vec!["-97", "256"]),
        (EncoderBase::VVenC, "qp", vec!["-1", "64"]),
        (EncoderBase::AOM, "passes", vec!["0", "3", "1.5"]),
    ] {
        for value in values {
            let mut encoder = Encoder::default_from_base(&base, false);
            encoder
                .parameters_mut()
                .insert(name.into(), CLIParameter::new_string("--", " ", value));
            assert!(
                encoder.validate_settings().is_err(),
                "accepted {base}/{name}={value}"
            );
        }
        let mut encoder = Encoder::default_from_base(&base, false);
        encoder.parameters_mut().insert(name.into(), CLIParameter::new_bool("--", true));
        assert!(
            encoder.validate_settings().is_err(),
            "accepted boolean {base}/{name}"
        );
    }
}

#[test]
fn svt_prediction_aliases_preserve_cbr_and_vbr_constraints() {
    use crate::models::encoder::EncoderSettingsError;
    for (rc, aliases) in [
        (
            0.0,
            &["0", "all-intra", "ai", "1", "low-delay", "ld", "2", "random-access", "ra"][..],
        ),
        (1.0, &["2", "random-access", "ra"][..]),
        (2.0, &["1", "low-delay", "ld"][..]),
    ] {
        for alias in aliases {
            let mut encoder = Encoder::default_from_base(&EncoderBase::SVTAV1, false);
            encoder.parameters_mut().remove("crf");
            encoder.parameters_mut().extend(CLIParameter::new_numbers(
                "--",
                " ",
                &[("rc", rc), ("keyint", 48.0)],
            ));
            encoder.parameters_mut().insert(
                "pred-struct".into(),
                CLIParameter::new_string("--", " ", alias),
            );
            assert_eq!(encoder.validate_settings(), Ok(()), "rc={rc}, pred={alias}");
        }
    }
    let mut encoder = Encoder::default_from_base(&EncoderBase::SVTAV1, true);
    for (rc, pred) in [(1.0, "ai"), (1.0, "ld"), (2.0, "ai"), (2.0, "ra")] {
        encoder.parameters_mut().extend(CLIParameter::new_numbers(
            "--",
            " ",
            &[("rc", rc), ("keyint", 48.0)],
        ));
        encoder.parameters_mut().insert(
            "pred-struct".into(),
            CLIParameter::new_string("--", " ", pred),
        );
        assert!(
            matches!(encoder.validate_settings(), Err(EncoderSettingsError::SvtMode(reason)) if reason.contains("CBR requires low delay")),
            "rc={rc}, pred={pred}"
        );
    }
    encoder.parameters_mut().insert(
        "pred-struct".into(),
        CLIParameter::new_string("--", " ", "bogus"),
    );
    assert!(
        matches!(encoder.validate_settings(), Err(EncoderSettingsError::InvalidNumber { name, .. }) if name == "pred-struct")
    );
}

#[test]
fn x265_high_bit_depth_crf_accepts_negative_values_within_the_native_envelope() {
    use crate::models::encoder::EncoderSettingsError;
    for (depth, crf) in [(10.0, -5.0), (10.0, -12.0), (12.0, -20.0), (12.0, -24.0)] {
        let mut encoder = Encoder::default_from_base(&EncoderBase::X265, false);
        encoder
            .parameters_mut()
            .extend(CLIParameter::new_numbers("--", " ", &[("crf", crf)]));
        encoder
            .parameters_mut()
            .insert("D".into(), CLIParameter::new_number("-", " ", depth));
        assert_eq!(
            encoder.validate_settings(),
            Ok(()),
            "depth={depth}, crf={crf}"
        );
    }
    for value in ["-24.25", "51.25", "NaN", "not-numeric"] {
        let mut encoder = Encoder::default_from_base(&EncoderBase::X265, false);
        encoder
            .parameters_mut()
            .insert("crf".into(), CLIParameter::new_string("--", " ", value));
        assert!(
            matches!(encoder.validate_settings(), Err(EncoderSettingsError::InvalidNumber { name, .. }) if name == "crf"),
            "{value}"
        );
    }
}

#[test]
fn vvenc_auto_passes_resolve_zero_bitrate_to_one_pass() {
    for options in [
        vec![("passes", -1.0)],
        vec![("pass", -1.0)],
        vec![("passes", -1.0), ("pass", -1.0)],
        vec![("passes", 1.0), ("pass", -1.0)],
    ] {
        let mut encoder = Encoder::default_from_base(&EncoderBase::VVenC, false);
        encoder.parameters_mut().extend(CLIParameter::new_numbers("--", " ", &options));
        assert_eq!(encoder.validate_settings(), Ok(()), "{options:?}");
    }
    for bitrate in ["0", "0k", "0Mbps"] {
        let mut encoder = Encoder::default_from_base(&EncoderBase::VVenC, false);
        encoder
            .parameters_mut()
            .insert("passes".into(), CLIParameter::new_string("--", " ", "-1"));
        encoder.parameters_mut().insert(
            "bitrate".into(),
            CLIParameter::new_string("--", " ", bitrate),
        );
        assert_eq!(
            encoder.validate_settings(),
            Ok(()),
            "zero bitrate {bitrate} resolves auto to single pass"
        );
    }
}

#[test]
fn vvenc_combined_native_passes_reject_nonseekable_stdin() {
    use crate::models::encoder::EncoderSettingsError;
    for (passes, native_pass, bitrate) in [(-1.0, -1.0, 1000.0), (2.0, -1.0, 1000.0)] {
        let mut encoder = Encoder::default_from_base(&EncoderBase::VVenC, false);
        encoder.parameters_mut().extend(CLIParameter::new_numbers(
            "--",
            " ",
            &[("passes", passes), ("pass", native_pass), ("bitrate", bitrate)],
        ));
        let error = encoder
            .validate_settings()
            .expect_err("combined native passes need seekable input");
        assert!(
            error.to_string().contains("stdin") && error.to_string().contains("separate"),
            "{error}"
        );
        assert!(
            matches!(
                error,
                EncoderSettingsError::UnsupportedMode(EncoderBase::VVenC, _)
            ),
            "native -1 is not an invalid number"
        );
    }
    let mut combined = Encoder::default_from_base(&EncoderBase::VVenC, false);
    combined
        .parameters_mut()
        .insert("p".into(), CLIParameter::new_number("-", " ", 2.0));
    assert!(
        matches!(combined.validate_settings(), Err(EncoderSettingsError::UnsupportedMode(EncoderBase::VVenC, reason)) if reason.contains("stdin")),
        "native -p aliases --passes"
    );
    combined.parameters_mut().clear();
    combined.parameters_mut().extend(CLIParameter::new_numbers(
        "--",
        " ",
        &[("passes", 1.0), ("pass", 2.0)],
    ));
    assert!(
        matches!(combined.validate_settings(), Err(EncoderSettingsError::UnsupportedMode(EncoderBase::VVenC, reason)) if reason.contains("pass count")),
        "native separate pass exceeds total"
    );
}

#[test]
fn vvenc_frontend_separate_passes_resolve_auto_and_reject_invalid_pass_numbers() {
    use crate::models::encoder::EncoderSettingsError;
    let mut encoder = Encoder::default_from_base(&EncoderBase::VVenC, false);
    *encoder.passes_mut().expect("passes") = EncoderPasses::All(2);
    encoder.parameters_mut().extend(CLIParameter::new_numbers(
        "--",
        " ",
        &[("passes", -1.0), ("bitrate", 1000.0)],
    ));
    assert_eq!(
        encoder.validate_settings(),
        Ok(()),
        "frontend separate passes resolve native auto to two"
    );
    encoder
        .parameters_mut()
        .insert("pass".into(), CLIParameter::new_number("--", " ", -1.0));
    let error = encoder
        .validate_settings()
        .expect_err("native both overrides frontend separate pass");
    assert!(error.to_string().contains("stdin"), "{error}");
    for value in [0.0, -2.0, 1.5, 3.0] {
        encoder
            .parameters_mut()
            .insert("pass".into(), CLIParameter::new_number("--", " ", value));
        assert!(
            matches!(encoder.validate_settings(), Err(EncoderSettingsError::InvalidNumber { name, .. }) if name == "pass"),
            "{value}"
        );
    }
}

#[test]
fn svt_loaded_quality_options_must_agree_with_qp_and_aq_mode() {
    use crate::models::encoder::EncoderSettingsError;
    for (quality, value, rc, qp, aq, valid) in [
        ("crf", 25.0, "0", Some(30.0), None, false),
        ("crf", 25.0, "cqp", None, None, false),
        ("crf", 25.0, "0", None, Some(0.0), false),
        ("cqp", 25.0, "crf", None, None, false),
        ("cqp", 25.0, "0", None, Some(2.0), false),
        ("crf", 25.0, "crf", Some(25.0), Some(2.0), true),
        ("cqp", 25.25, "cqp", Some(25.0), Some(0.0), true),
    ] {
        let mut encoder = Encoder::default_from_base(&EncoderBase::SVTAV1, true);
        encoder
            .parameters_mut()
            .insert(quality.into(), CLIParameter::new_number("--", " ", value));
        encoder
            .parameters_mut()
            .insert("rc".into(), CLIParameter::new_string("--", " ", rc));
        if let Some(qp) = qp {
            encoder
                .parameters_mut()
                .insert("qp".into(), CLIParameter::new_number("--", " ", qp));
        }
        if let Some(aq) = aq {
            encoder
                .parameters_mut()
                .insert("aq-mode".into(), CLIParameter::new_number("--", " ", aq));
        }
        let result = encoder.validate_settings();
        if valid {
            assert_eq!(result, Ok(()));
        } else {
            assert!(
                matches!(result, Err(EncoderSettingsError::SvtMode(_))),
                "{quality}/{rc}/{qp:?}/{aq:?}: {result:?}"
            );
        }
    }
}

#[test]
fn encoder_pass_pairs_reject_invalid_or_unsupported_pass_counts() {
    for base in [
        EncoderBase::AOM,
        EncoderBase::VPX,
        EncoderBase::RAV1E,
        EncoderBase::SVTAV1,
        EncoderBase::AVM,
        EncoderBase::X264,
        EncoderBase::X265,
        EncoderBase::VVenC,
        EncoderBase::FFmpeg,
    ] {
        let mut encoder = Encoder::default_from_base(&base, false);
        encoder.set_executable(Some("/does-not-exist/encoder-preflight".into()));
        assert!(encoder.validate_settings().is_ok(), "{base} defaults");
        for pass in [(0, 1), (2, 1), (1, 0), (3, 3), (1, 255)] {
            assert!(
                base.pass_parameters(pass, "stats").is_err(),
                "{base}/{pass:?}"
            );
        }
        for pass in [(1, 1), (1, 2), (2, 2)] {
            if pass.1 == 2 && matches!(base, EncoderBase::AVM | EncoderBase::FFmpeg) {
                assert!(base.pass_parameters(pass, "stats").is_err());
                continue;
            }
            assert!(
                base.pass_parameters(pass, "stats").is_ok(),
                "{base}/{pass:?}"
            );
            if base == EncoderBase::SVTAV1 && pass.1 == 2 {
                encoder.parameters_mut().remove("crf");
                encoder
                    .parameters_mut()
                    .insert("rc".into(), CLIParameter::new_number("--", " ", 1.0));
                encoder
                    .parameters_mut()
                    .insert("keyint".into(), CLIParameter::new_number("--", " ", 48.0));
            }
            if base == EncoderBase::VVenC && pass.1 == 2 {
                encoder.parameters_mut().insert(
                    "bitrate".into(),
                    CLIParameter::new_number("--", " ", 1000.0),
                );
            }
            if let Some(passes) = encoder.passes_mut() {
                *passes = EncoderPasses::Specific(pass.0, pass.1);
            }
            assert!(encoder.validate_settings().is_ok(), "{base}/{pass:?}");
        }
    }
}

#[test]
fn valid_encoder_rate_control_boundary_values_are_accepted() {
    for (base, name, values) in [
        (EncoderBase::SVTAV1, "crf", vec![0.0, 0.25, 25.5, 70.0]),
        (EncoderBase::SVTAV1, "qp", vec![0.0, 63.0]),
        (EncoderBase::X264, "crf", vec![-12.0, 25.5, 51.0]),
        (EncoderBase::X264, "qp", vec![0.0, 81.0]),
        (EncoderBase::VVenC, "qp", vec![0.0, 25.5, 63.0]),
        (EncoderBase::AVM, "qp", vec![-96.0, -48.0, 255.0]),
    ] {
        for value in values {
            let mut encoder = Encoder::default_from_base(&base, false);
            if base == EncoderBase::SVTAV1 && name == "qp" {
                encoder.parameters_mut().remove("crf");
            }
            encoder
                .parameters_mut()
                .insert(name.into(), CLIParameter::new_number("--", " ", value));
            encoder.parameters_mut().insert(
                "custom-sentinel".into(),
                CLIParameter::new_number("--", " ", -1.0),
            );
            assert!(encoder.validate_settings().is_ok(), "{base}/{name}={value}");
        }
    }
}

#[test]
fn svt_crf_rejects_native_multipass_overrides() {
    for (name, value) in [("passes", 2.0), ("pass", 1.0), ("pass", 2.0)] {
        let mut encoder = Encoder::default_from_base(&EncoderBase::SVTAV1, false);
        encoder
            .parameters_mut()
            .insert(name.into(), CLIParameter::new_number("--", " ", value));
        assert!(
            encoder.validate_settings().is_err(),
            "CRF with native {name}={value}"
        );
    }
}

#[test]
fn svt_rate_control_mode_admission_matches_preset_and_prediction_structure() {
    for (rc, total, preset, pred, valid) in [
        (0.0, 2, 4.0, 2.0, false),
        (1.0, 2, 9.0, 2.0, true),
        (1.0, 2, 10.0, 2.0, false),
        (2.0, 2, 4.0, 1.0, false),
        (2.0, 1, 4.0, 1.0, true),
        (1.0, 1, 13.0, 2.0, true),
    ] {
        let mut encoder = Encoder::default_from_base(&EncoderBase::SVTAV1, false);
        encoder.parameters_mut().remove("crf");
        encoder.parameters_mut().extend(CLIParameter::new_numbers(
            "--",
            " ",
            &[("rc", rc), ("preset", preset), ("pred-struct", pred), ("keyint", 48.0)],
        ));
        *encoder.passes_mut().expect("passes") = EncoderPasses::All(total);
        assert_eq!(
            encoder.validate_settings().is_ok(),
            valid,
            "{rc}/{total}/{preset}/{pred}"
        );
        if rc != 0.0 {
            encoder
                .parameters_mut()
                .insert("crf".into(), CLIParameter::new_number("--", " ", 25.0));
            assert!(
                encoder.validate_settings().is_err(),
                "contradictory loaded config"
            );
        }
    }
}

#[test]
fn svt_vbr_rejects_explicit_infinite_keyint() {
    for keyint in ["0", "-1", "-2", "1.5"] {
        let mut encoder = Encoder::default_from_base(&EncoderBase::SVTAV1, false);
        encoder.parameters_mut().remove("crf");
        encoder
            .parameters_mut()
            .insert("rc".into(), CLIParameter::new_number("--", " ", 1.0));
        encoder
            .parameters_mut()
            .insert("keyint".into(), CLIParameter::new_string("--", " ", keyint));
        assert!(encoder.validate_settings().is_err(), "VBR keyint {keyint}");
    }
}

#[test]
fn loaded_encoder_pass_count_is_validated() {
    let json = r#"{"AOM":{"executable":null,"pass":{"Specific":[3,3]},"options":{}}}"#;
    let encoder: Encoder = serde_json::from_str(json).expect("loaded encoder");
    assert!(encoder.validate_settings().is_err());
}

#[test]
fn photon_noise_coefficients_enforce_all_channel_shapes() {
    let mut encoder = Encoder::default_from_base(&EncoderBase::AOM, false);
    let noise = crate::models::encoder::photon_noise::PhotonNoise {
        iso: 0,
        chroma_iso: Some(0),
        width: None,
        height: None,
        c_y: Some(vec![0; 24]),
        ccb: Some(vec![0; 25]),
        ccr: Some(vec![0; 25]),
    };
    encoder.set_photon_noise(Some(noise.clone()));
    assert!(
        encoder.validate_settings().is_ok(),
        "existing coefficient maxima and ISO zero"
    );
    for channel in ["c_y", "ccb", "ccr"] {
        let mut invalid = noise.clone();
        match channel {
            "c_y" => invalid.c_y.as_mut(),
            "ccb" => invalid.ccb.as_mut(),
            _ => invalid.ccr.as_mut(),
        }
        .expect("coefficients")
        .push(0);
        encoder.set_photon_noise(Some(invalid));
        assert!(encoder.validate_settings().is_err(), "{channel}");
    }
}
