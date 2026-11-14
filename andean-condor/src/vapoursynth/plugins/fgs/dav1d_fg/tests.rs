use std::mem::{offset_of, size_of};

use super::*;
use crate::{
    models::encoder::photon_noise::PhotonNoise,
    vapoursynth::script_builder::{VapourSynthPluginScript, script::Line},
};

fn blob_fixture() -> FGS {
    FGS {
        photon_noise: PhotonNoise {
            iso: 800,
            chroma_iso: Some(800),
            width: Some(1280),
            height: Some(720),
            c_y: Some(vec![-128, 0, 127, 1]),
            ccb: Some(vec![-127, 126, 0, 1, -1]),
            ccr: Some(vec![125, -126, 124, 0, 1]),
        },
        dynamic_seed: None,
        simd_mask: None,
    }
}

#[test]
fn fgs_blob_preserves_coefficient_overrides_zero_tails_and_script_hex() {
    let fgs = blob_fixture();
    let bytes = fgs
        .build_grain_binary(None, None, TransferFunction::BT1886, None)
        .expect("FGS blob");
    assert_eq!(bytes.len(), size_of::<Dav1dFilmGrainDataRaw>());
    for (offset, capacity, expected) in [
        (
            offset_of!(Dav1dFilmGrainDataRaw, ar_coeffs_y),
            24,
            vec![128, 0, 127, 1],
        ),
        (
            offset_of!(Dav1dFilmGrainDataRaw, ar_coeffs_uv),
            28,
            vec![129, 126, 0, 1, 255],
        ),
        (
            offset_of!(Dav1dFilmGrainDataRaw, ar_coeffs_uv) + 28,
            28,
            vec![125, 130, 124, 0, 1],
        ),
    ] {
        assert_eq!(&bytes[offset..offset + expected.len()], expected);
        assert!(bytes[offset + expected.len()..offset + capacity].iter().all(|&b| b == 0));
    }

    // The generated scaling points must keep their unused ABI slots zero.
    let count_at = |offset| {
        i32::from_ne_bytes(bytes[offset..offset + 4].try_into().expect("i32 count")) as usize
    };
    for (count_offset, points_offset, capacity) in [
        (
            offset_of!(Dav1dFilmGrainDataRaw, num_y_points),
            offset_of!(Dav1dFilmGrainDataRaw, y_points),
            14,
        ),
        (
            offset_of!(Dav1dFilmGrainDataRaw, num_uv_points),
            offset_of!(Dav1dFilmGrainDataRaw, uv_points),
            10,
        ),
        (
            offset_of!(Dav1dFilmGrainDataRaw, num_uv_points) + 4,
            offset_of!(Dav1dFilmGrainDataRaw, uv_points) + 20,
            10,
        ),
    ] {
        let count = count_at(count_offset);
        assert!(count <= capacity);
        assert!(
            bytes[points_offset + count * 2..points_offset + capacity * 2]
                .iter()
                .all(|&b| b == 0)
        );
    }

    let (_, lines) = fgs.generate_script("clip".to_owned()).expect("FGS script");
    let Line::Expression(_, expression) = &lines[0] else {
        panic!("expected FGS expression")
    };
    let hex = expression
        .split("bytes.fromhex(\"")
        .nth(1)
        .expect("hex literal")
        .split('"')
        .next()
        .expect("closing quote");
    let (pairs, remainder) = hex.as_bytes().as_chunks::<2>();
    assert!(remainder.is_empty());
    let decoded: Vec<u8> = pairs
        .iter()
        .map(|pair| {
            u8::from_str_radix(std::str::from_utf8(pair).expect("ASCII hex"), 16).expect("hex byte")
        })
        .collect();
    assert_eq!(decoded, bytes);
    assert_eq!(
        fgs.build_grain_binary(None, None, TransferFunction::BT1886, None)
            .expect("repeated FGS blob"),
        bytes
    );
}

#[test]
fn fgs_coefficient_arrays_reject_values_beyond_native_capacity() {
    for (field, capacity) in [("c_y", 24), ("ccb", 25), ("ccr", 25)] {
        let mut fgs = blob_fixture();
        fgs.photon_noise.c_y = Some(vec![127; 24]);
        fgs.photon_noise.ccb = Some(vec![127; 25]);
        fgs.photon_noise.ccr = Some(vec![127; 25]);
        for len in [capacity, capacity + 1] {
            let coefficients = match field {
                "c_y" => &mut fgs.photon_noise.c_y,
                "ccb" => &mut fgs.photon_noise.ccb,
                _ => &mut fgs.photon_noise.ccr,
            };
            *coefficients = Some(vec![127; len]);
            let result = fgs.build_grain_binary(None, None, TransferFunction::BT1886, None);
            if len == capacity {
                assert!(result.is_ok());
            } else {
                let error = result.expect_err("reject coefficients exceeding capacity");
                assert!(error.to_string().contains(&format!("{field} must contain exactly")));
            }
        }
    }
}

use crate::models::encoder::{Encoder, EncoderBase};

fn noise(y: usize, cb: usize, cr: usize) -> PhotonNoise {
    let coefficients = |len| (0..len).map(|i| [-128, 0, 127][i % 3]).collect();
    PhotonNoise {
        iso: 800,
        chroma_iso: Some(800),
        width: Some(1920),
        height: Some(1080),
        c_y: Some(coefficients(y)),
        ccb: Some(coefficients(cb)),
        ccr: Some(coefficients(cr)),
    }
}

fn encoder(noise: &PhotonNoise) -> Encoder {
    let mut encoder = Encoder::default_from_base(&EncoderBase::AOM, true);
    encoder.set_photon_noise(Some(noise.clone()));
    encoder
}

fn fgs(noise: &PhotonNoise) -> FGS {
    FGS {
        photon_noise: noise.clone(),
        dynamic_seed: None,
        simd_mask: None,
    }
}

fn table(noise: &PhotonNoise) -> GrainTableSegment {
    encoder(noise)
        .generate_photon_noise_table(64, 64, TransferFunction::BT1886, None)
        .expect("valid table")
        .expect("configured grain")
        .1
}

#[test]
fn unequal_chroma_iso_refuses_fgs_blob_and_script_with_typed_error() {
    use crate::models::encoder::EncoderSettingsError;
    let mut noise = noise(0, 1, 1);
    noise.chroma_iso = Some(400);
    let fgs = fgs(&noise);
    let error = anyhow::Error::new(
        fgs.build_grain_binary(None, None, TransferFunction::BT1886, None)
            .expect_err("unequal chroma ISO must not serialize a blob"),
    );
    assert_eq!(
        error.chain().find_map(|cause| cause.downcast_ref::<EncoderSettingsError>()),
        Some(&EncoderSettingsError::UnequalChromaIso {
            iso: 800,
            chroma_iso: 400
        }),
        "typed FGS settings error: {error:#}"
    );
    let error = fgs.generate_script("clip".into()).err().expect("no invalid grain script");
    assert!(error.chain().any(|cause| cause.is::<EncoderSettingsError>()));
}

#[test]
fn equal_chroma_and_no_chroma_preserve_table_and_fgs_flags() {
    for chroma_iso in [None, Some(800)] {
        let mut noise = noise(0, 1, 1);
        noise.chroma_iso = chroma_iso;
        let segment = table(&noise);
        assert_eq!(segment.chroma_scaling_from_luma, chroma_iso.is_some());
        let bytes = fgs(&noise)
            .build_grain_binary(None, None, TransferFunction::BT1886, None)
            .expect("supported FGS blob");
        assert_eq!(bytes, Dav1dFilmGrainDataRaw::from(&segment).to_bytes());
        let offset = offset_of!(Dav1dFilmGrainDataRaw, chroma_scaling_from_luma);
        assert_eq!(
            i32::from_ne_bytes(bytes[offset..offset + 4].try_into().expect("flag")),
            i32::from(chroma_iso.is_some())
        );
    }
}

#[test]
fn fgs_matches_table_lag_coefficients_and_zero_tails() {
    for (lag, y, uv) in [(0, 0, 1), (1, 4, 5), (2, 12, 13), (3, 24, 25)] {
        for chroma_iso in [None, Some(400), Some(800)] {
            let mut noise = noise(y, uv, uv);
            noise.chroma_iso = chroma_iso;
            if chroma_iso == Some(400) {
                assert!(
                    fgs(&noise)
                        .build_grain_binary(Some(64), Some(64), TransferFunction::BT1886, None)
                        .is_err(),
                    "unequal ISO rejected before lag {lag} blob"
                );
                continue;
            }
            let bytes = fgs(&noise)
                .build_grain_binary(Some(64), Some(64), TransferFunction::BT1886, None)
                .expect("valid FGS blob");
            let offset = offset_of!(Dav1dFilmGrainDataRaw, ar_coeff_lag);
            assert_eq!(
                i32::from_ne_bytes(bytes[offset..offset + 4].try_into().expect("lag")),
                lag
            );
            let segment = table(&noise);
            assert_eq!(bytes, Dav1dFilmGrainDataRaw::from(&segment).to_bytes());
            for (offset, capacity, coefficients) in [
                (
                    offset_of!(Dav1dFilmGrainDataRaw, ar_coeffs_y),
                    24,
                    &segment.ar_coeffs_y[..],
                ),
                (
                    offset_of!(Dav1dFilmGrainDataRaw, ar_coeffs_uv),
                    28,
                    &segment.ar_coeffs_cb[..],
                ),
                (
                    offset_of!(Dav1dFilmGrainDataRaw, ar_coeffs_uv) + 28,
                    28,
                    &segment.ar_coeffs_cr[..],
                ),
            ] {
                let expected: Vec<u8> = coefficients.iter().map(|&v| v as u8).collect();
                assert_eq!(&bytes[offset..offset + expected.len()], expected);
                assert!(bytes[offset + expected.len()..offset + capacity].iter().all(|&v| v == 0));
            }
        }
    }
}

#[test]
fn invalid_grain_coefficient_shapes_are_rejected_by_every_pure_entry_point() {
    let mut cases = Vec::new();
    for y in [1, 2, 3, 5, 11, 13, 23, 25, 100] {
        cases.push(noise(y, 1, 1));
    }
    for (y, uv) in [(0, 1), (4, 5), (12, 13), (24, 25)] {
        for len in [0, uv - 1, uv + 1, 26, 100] {
            cases.push(noise(y, len, uv));
            cases.push(noise(y, uv, len));
        }
        if y != 0 {
            let mut partial = noise(y, uv, uv);
            partial.ccb = None;
            cases.push(partial);
            let mut partial = noise(y, uv, uv);
            partial.ccr = None;
            cases.push(partial);
        }
    }
    let mut inherited_y = noise(4, 5, 5);
    inherited_y.c_y = None;
    cases.push(inherited_y);
    for noise in cases {
        let encoder = encoder(&noise);
        let loaded: Encoder = serde_json::from_str(&serde_json::to_string(&encoder).expect("save"))
            .expect("load custom coefficients");
        assert!(loaded.validate_settings().is_err(), "{noise:?}");
        assert!(
            encoder
                .generate_photon_noise_table(64, 64, TransferFunction::BT1886, None)
                .is_err()
        );
        let fgs = fgs(&noise);
        assert!(fgs.build_grain_binary(None, None, TransferFunction::BT1886, None).is_err());
        assert!(fgs.generate_script("clip".into()).is_err());
    }
}

#[test]
fn defaults_and_compatible_partial_overrides_preserve_original_generation() {
    for chroma_iso in [None, Some(400), Some(800)] {
        for (transfer, range) in [
            (TransferFunction::BT1886, None),
            (TransferFunction::SMPTE2084, Some(ColorRange::Full)),
        ] {
            let mut noise = noise(0, 1, 1);
            noise.chroma_iso = chroma_iso;
            noise.c_y = None;
            noise.ccb = None;
            noise.ccr = None;
            if chroma_iso == Some(400) {
                assert!(
                    encoder(&noise).generate_photon_noise_table(64, 64, transfer, range).is_err()
                );
                assert!(
                    fgs(&noise).build_grain_binary(Some(64), Some(64), transfer, range).is_err()
                );
                continue;
            }
            let original = generate_photon_noise_params(
                0,
                u64::MAX,
                av1_grain::NoiseGenArgs {
                    iso_setting: 800,
                    width: 1920,
                    height: 1080,
                    transfer_function: transfer,
                    chroma_grain: chroma_iso == Some(800),
                    random_seed: None,
                    full_range: matches!(range, Some(ColorRange::Full)),
                },
            );
            let generated = encoder(&noise)
                .generate_photon_noise_table(64, 64, transfer, range)
                .expect("defaults")
                .expect("grain")
                .1;
            assert_eq!(
                generated, original,
                "independent unmodified dependency generator"
            );
            assert_eq!(
                fgs(&noise)
                    .build_grain_binary(Some(64), Some(64), transfer, range)
                    .expect("default blob"),
                Dav1dFilmGrainDataRaw::from(&original).to_bytes()
            );
            noise.ccb = Some(vec![-128]);
            assert_eq!(table(&noise).ar_coeffs_cb.as_slice(), [-128]);
            assert!(fgs(&noise).build_grain_binary(None, None, transfer, range).is_ok());
            noise.c_y = Some(vec![]);
            assert_eq!(table(&noise).ar_coeff_lag, 0);
        }
    }
}

#[test]
fn grain_coefficient_application_rejects_incompatible_segments_without_mutation() {
    let valid = noise(4, 5, 5);
    let mut segment = table(&noise(0, 1, 1));
    let mut incompatible = valid.clone();
    incompatible.ccr = None;
    let before = segment.clone();
    assert!(incompatible.apply_coefficients(&mut segment).is_err());
    assert_eq!(
        segment, before,
        "no partially applied coefficient overrides"
    );
    segment.scaling_points_y.clear();
    let before = segment.clone();
    assert!(
        valid.apply_coefficients(&mut segment).is_err(),
        "no-luma-point AV1 segments have a different chroma arity; outside generated format"
    );
    assert_eq!(segment, before);
}
