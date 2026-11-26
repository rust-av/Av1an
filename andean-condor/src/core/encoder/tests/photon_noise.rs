//! Settings-only keys alias different generated tables; exercise the generator and real cache owner.
use super::*;
use std::collections::HashSet;

fn noise() -> PhotonNoise {
    PhotonNoise {
        iso: 800,
        chroma_iso: Some(800),
        width: None,
        height: None,
        c_y: Some(vec![1, 2, 3, 4]),
        ccb: Some(vec![5, 6, 7, 8, 9]),
        ccr: Some(vec![-5, -6, -7, -8, -9]),
    }
}

fn encoder(noise: &PhotonNoise, base: EncoderBase) -> Encoder {
    let mut encoder = Encoder::default_from_base(&base, true);
    encoder.set_photon_noise(Some(noise.clone()));
    encoder
}

fn table(
    encoder: &Encoder,
    width: u32,
    height: u32,
    transfer: TransferFunction,
    range: Option<ColorRange>,
) -> (String, GrainTableSegment) {
    let before = encoder.clone();
    let generated = encoder
        .generate_photon_noise_table(width, height, transfer, range)
        .expect("valid settings")
        .expect("grain enabled");
    assert_eq!(
        *encoder, before,
        "resolving a cache key must not mutate settings"
    );
    generated
}

fn expected_params(
    noise: &PhotonNoise,
    width: u32,
    height: u32,
    transfer: TransferFunction,
    full_range: bool,
) -> GrainTableSegment {
    let args = NoiseGenArgs {
        iso_setting: noise.iso,
        width,
        height,
        transfer_function: transfer,
        chroma_grain: noise.chroma_iso == Some(noise.iso),
        random_seed: None,
        full_range,
    };
    let mut expected = generate_photon_noise_params(0, u64::MAX, args);
    expected.ar_coeff_lag = 1;
    expected.ar_coeffs_y = noise.c_y.as_ref().expect("fixture Y").iter().copied().collect();
    expected.ar_coeffs_cb = noise.ccb.as_ref().expect("fixture Cb").iter().copied().collect();
    expected.ar_coeffs_cr = noise.ccr.as_ref().expect("fixture Cr").iter().copied().collect();
    expected
}

#[test]
fn photon_noise_cache_keys_distinguish_source_geometry_transfer_and_range() {
    let noise = noise();
    for base in [EncoderBase::AOM, EncoderBase::RAV1E, EncoderBase::SVTAV1] {
        let encoder = encoder(&noise, base);
        let mut keys = HashSet::new();
        for (width, height) in [(1920, 1080), (1280, 1080), (1920, 720)] {
            for transfer in [TransferFunction::BT1886, TransferFunction::SMPTE2084] {
                for range in [None, Some(ColorRange::Full)] {
                    let generated = table(&encoder, width, height, transfer, range);
                    assert_eq!(generated, table(&encoder, width, height, transfer, range));
                    assert_eq!(
                        generated.1,
                        expected_params(&noise, width, height, transfer, range.is_some())
                    );
                    if range.is_none() {
                        assert_eq!(
                            generated,
                            table(&encoder, width, height, transfer, Some(ColorRange::Limited)),
                            "unknown and limited range both generate full_range=false"
                        );
                    }
                    keys.insert(generated.0);
                }
            }
        }
        assert_eq!(
            keys.len(),
            12,
            "each effective geometry/transfer/range needs its own table"
        );
        assert!(
            !keys.contains(&noise.hash_name()),
            "legacy settings-only names are ineligible"
        );
    }
}

#[test]
fn photon_noise_cache_keys_use_effective_dimensions_and_coefficients() {
    let original = noise();
    let generate = |noise: &PhotonNoise, width, height| {
        table(
            &encoder(noise, EncoderBase::AOM),
            width,
            height,
            TransferFunction::BT1886,
            None,
        )
    };
    let baseline = generate(&original, 1920, 1080);
    for (width, height, source) in [
        (Some(1920), None, (640, 1080)),
        (None, Some(1080), (1920, 360)),
        (Some(1920), Some(1080), (640, 360)),
    ] {
        let mut overridden = original.clone();
        overridden.width = width;
        overridden.height = height;
        assert_eq!(generate(&overridden, source.0, source.1), baseline);
        assert_eq!(generate(&overridden, 1920, 1080), baseline);
        if height.is_none() {
            assert_ne!(generate(&overridden, source.0, 720).0, baseline.0);
        }
        if width.is_none() {
            assert_ne!(generate(&overridden, 1280, source.1).0, baseline.0);
        }
    }
    let mut keys = HashSet::from([baseline.0.clone()]);
    for field in [
        "iso",
        "chroma_iso",
        "no_chroma",
        "same_chroma",
        "width",
        "height",
        "Y",
        "Cb",
        "Cr",
    ] {
        let mut changed = original.clone();
        match field {
            "iso" => {
                changed.iso = 1600;
                changed.chroma_iso = Some(1600);
            },
            "chroma_iso" => changed.chroma_iso = Some(200),
            "no_chroma" => changed.chroma_iso = None,
            "same_chroma" => changed.chroma_iso = Some(800),
            "width" => changed.width = Some(1280),
            "height" => changed.height = Some(720),
            "Y" => changed.c_y.as_mut().expect("Y")[0] = 10,
            "Cb" => changed.ccb.as_mut().expect("Cb")[0] = 10,
            "Cr" => changed.ccr.as_mut().expect("Cr")[0] = 10,
            _ => unreachable!(),
        }
        if field == "chroma_iso" {
            let error = encoder(&changed, EncoderBase::AOM)
                .generate_photon_noise_table(1920, 1080, TransferFunction::BT1886, None)
                .expect_err("independent chroma ISO must not get a cache key/table");
            assert!(error.is::<crate::models::encoder::EncoderSettingsError>());
            continue;
        }
        let generated = generate(&changed, 1920, 1080);
        if field == "same_chroma" {
            assert_eq!(
                generated, baseline,
                "same ISO is the same generated request"
            );
            continue;
        }
        assert!(
            keys.insert(generated.0),
            "material grain setting {field} missing from key"
        );
        assert_eq!(
            generated.1,
            expected_params(
                &changed,
                changed.width.unwrap_or(1920),
                changed.height.unwrap_or(1080),
                TransferFunction::BT1886,
                false
            )
        );
    }
}

fn avm_noise() -> PhotonNoise {
    PhotonNoise {
        iso: 800,
        chroma_iso: Some(800),
        width: None,
        height: None,
        c_y: None,
        ccb: None,
        ccr: None,
    }
}

#[test]
fn avm_generation_rejects_instead_of_silently_omitting_noise() {
    use crate::models::encoder::EncoderSettingsError;
    let mut encoder = Encoder::default_from_base(&EncoderBase::AVM, false);
    encoder.set_photon_noise(Some(avm_noise()));
    let before = encoder.clone();
    let error = encoder
        .generate_photon_noise_table(64, 64, TransferFunction::BT1886, None)
        .expect_err("AVM cannot use generated filmgrn1 tables");
    assert_eq!(
        error.downcast_ref::<EncoderSettingsError>(),
        Some(&EncoderSettingsError::PhotonNoiseNotSupported(
            EncoderBase::AVM
        ))
    );
    assert_eq!(encoder, before);
}

#[test]
fn avm_explicit_pass_and_argv_reject_noise() {
    use crate::models::encoder::EncoderSettingsError;
    let mut encoder = Encoder::default_from_base(&EncoderBase::AVM, false);
    encoder.set_photon_noise(Some(avm_noise()));
    let before = encoder.clone();
    assert_eq!(
        encoder.validate_pass_settings((1, 1)),
        Err(EncoderSettingsError::PhotonNoiseNotSupported(
            EncoderBase::AVM
        ))
    );
    for result in [
        encoder.parameters_with_output(
            (1, 1),
            Path::new("out.ivf"),
            &crate::core::encoder::tests::clip_info(),
            None,
        ),
        encoder.parameters_with_output(
            (1, 1),
            Path::new("out.ivf"),
            &crate::core::encoder::tests::clip_info(),
            Some(Path::new("stats")),
        ),
    ] {
        assert_eq!(
            result.expect_err("unsupported argv").downcast_ref::<EncoderSettingsError>(),
            Some(&EncoderSettingsError::PhotonNoiseNotSupported(
                EncoderBase::AVM
            ))
        );
    }
    assert_eq!(encoder, before);
}

#[test]
fn avm_raw_native_table_remains_available_without_application_noise() {
    let mut encoder = Encoder::default_from_base(&EncoderBase::AVM, true);
    encoder.set_append_color_data(false);
    encoder.parameters_mut().insert(
        "film-grain-table".into(),
        CLIParameter::new_string("--", "=", "caller filmgrn2.tbl"),
    );
    assert_eq!(encoder.validate_settings(), Ok(()));
    let mut argv = encoder
        .parameters_with_output(
            (1, 1),
            Path::new("out.ivf"),
            &crate::core::encoder::tests::clip_info(),
            None,
        )
        .expect("raw native options are caller-owned");
    argv.sort();
    assert_eq!(
        argv,
        ["-", "--film-grain-table=caller filmgrn2.tbl", "-o", "out.ivf"]
    );
    encoder.set_photon_noise(Some(avm_noise()));
    let before = encoder.clone();
    let error = encoder
        .apply_photon_noise_parameters(Path::new("generated filmgrn1.tbl"))
        .expect_err("application table is unsupported");
    assert!(matches!(
        error.downcast_ref::<EncoderError>(),
        Some(EncoderError::PhotonNoiseNotSupported(EncoderBase::AVM))
    ));
    assert_eq!(
        encoder, before,
        "application must not replace a raw native option"
    );
    encoder.set_photon_noise(None);
    assert_eq!(encoder.validate_settings(), Ok(()));
}
