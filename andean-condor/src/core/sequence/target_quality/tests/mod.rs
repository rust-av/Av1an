#[cfg(unix)]
pub(in crate::core::sequence) mod native_scoring;

use crate as condor;
use crate::core::sequence::tests::support::current_target_fixture;
use std::sync;
#[path = "../../../../../tests/core/sequence/target_quality/fixtures/duplicate_resume.rs"]
mod duplicate_fixture;
#[path = "../../../../../tests/core/sequence/target_quality/fixtures/probe_support.rs"]
#[allow(
    dead_code,
    reason = "shared fixture also contains public-only probe drivers"
)]
mod probe_fixture;

mod measured_history {
    mod internal_integration {
        #[test]
        fn resumed_native_probe_records_pass_the_private_history_reducer() {
            super::super::duplicate_fixture::check_duplicate_resume(
                1,
                super::super::TargetQuality::quantizer_score_history,
            );
        }
    }
}

use super::{
    SequenceCompletion, SequenceStatus, Status, TargetQuality, Task, spawn_compare_progress,
};
use crate::{
    metrics::Engine,
    models::sequence::target_quality::{
        TargetQualityConfig,
        types::{MeasurementError, QualityPass},
    },
};

use crate::models::{
    encoder::{Encoder, EncoderBase, photon_noise::PhotonNoise},
    sequence::target_quality::types::QualityMetric,
};

const PREDICTION_METHODS_METHODS:
    [crate::models::sequence::target_quality::types::InterpolationMethod; 8] = {
    use crate::models::sequence::target_quality::types::InterpolationMethod::*;
    [
        Linear,
        Quadratic,
        Natural,
        FritschCarlson,
        Pchip,
        Catmull,
        Akima,
        CubicPolynomial,
    ]
};

#[test]
fn three_record_pchip_runs_the_curve_not_midpoint() {
    use crate::models::sequence::target_quality::types::InterpolationMethod::Pchip;
    assert_eq!(
        TargetQuality::predict_quantizer(
            (0.0, 40.0),
            1.5,
            (Pchip, Pchip),
            &[(1.0, 0.0), (4.0, 1.0), (9.0, 2.0)],
            1.0 / 1024.0,
        )
        .expect("three-point PCHIP"),
        6.34375
    );
}

#[test]
fn three_record_four_point_methods_use_bracketing_linear_not_midpoint() {
    use crate::models::sequence::target_quality::types::InterpolationMethod::*;
    for method in [Catmull, Akima, CubicPolynomial] {
        assert_eq!(
            TargetQuality::predict_quantizer(
                (0.0, 40.0),
                1.5,
                (method, Pchip),
                &[(1.0, 0.0), (4.0, 1.0), (9.0, 2.0)],
                0.25,
            )
            .expect("insufficient-point fallback"),
            6.5,
            "{method:?}"
        );
    }
}

#[test]
fn prediction_uses_midpoint_then_sparse_and_configured_interpolators_by_history_count() {
    // q = (score + 1)^2. Literal curve evaluations, not a second dispatch implementation.
    // Hermite at t=1/2 is (q0+q1)/2 + (m0-m1)/8.
    let history = [(1.0, 0.0), (4.0, 1.0), (9.0, 2.0), (16.0, 3.0), (25.0, 4.0), (36.0, 5.0)];
    let cases = [
        (0, 0.5, [20.0; 8]),
        (1, 0.5, [20.0; 8]),
        (2, 0.5, [2.5; 8]),
        (3, 1.5, [6.5, 6.25, 6.3125, 6.34375, 6.34375, 6.5, 6.5, 6.5]),
        (
            4,
            1.5,
            [6.5, 6.25, 6.2, 6.34375, 6.239583333333333, 6.375, 6.25, 6.25],
        ),
        (5, 1.5, [6.239583333333333; 8]),
        (6, 1.5, [6.239583333333333; 8]),
    ];
    for (count, target, expected) in cases {
        // Deliberately unsorted: admission must order by score, not by quantizer or arrival.
        let mut samples = history[..count].to_vec();
        samples.reverse();
        for (method, want) in PREDICTION_METHODS_METHODS.into_iter().zip(expected) {
            let got = TargetQuality::predict_quantizer(
                (0.0, 40.0),
                target,
                (method, method),
                &samples,
                1.0 / 1024.0,
            )
            .expect("finite prediction");
            assert!(
                (got - want).abs() <= 1.0 / 2048.0,
                "{method:?}, {count} records: {got} != {want}"
            );
        }
    }
}

#[test]
fn prediction_uses_phase_specific_interpolator_slots_and_all_bracketing_records() {
    use crate::models::sequence::target_quality::types::InterpolationMethod::*;
    let history = [(1.0, 0.0), (4.0, 1.0), (9.0, 2.0), (16.0, 3.0)];
    for (count, target, methods, want) in [
        (3, 1.5, (Quadratic, Natural), 6.25),
        (4, 1.5, (Quadratic, Natural), 6.2),
        (4, 2.5, (Linear, Linear), 12.5),
        (4, 2.5, (Linear, Quadratic), 12.25),
        (4, 2.5, (Linear, FritschCarlson), 12.354166666666666),
        (4, 2.5, (Linear, Natural), 12.35),
        (4, 2.5, (Linear, Pchip), 12.354166666666666),
        // Catmull rejects the outer interval: use its nearest bracketing pair.
        (4, 0.5, (Linear, Catmull), 2.5),
        (4, 2.5, (Linear, Catmull), 12.5),
        (4, 2.5, (Linear, Akima), 12.375),
        (4, 2.5, (Linear, CubicPolynomial), 12.25),
    ] {
        let got = TargetQuality::predict_quantizer(
            (0.0, 40.0),
            target,
            methods,
            &history[..count],
            1.0 / 1024.0,
        )
        .expect("window prediction");
        assert!(
            (got - want).abs() <= 1.0 / 2048.0,
            "{methods:?}, {count}/{target}: {got} != {want}"
        );
    }
    // The fifth and sixth records must participate, not just trigger a phase switch.
    for history in [
        &[(1.0, 0.0), (4.0, 1.0), (9.0, 2.0), (16.0, 3.0), (25.0, 4.0)][..],
        &[(1.0, 0.0), (4.0, 1.0), (9.0, 2.0), (16.0, 3.0), (25.0, 4.0), (36.0, 5.0)][..],
    ] {
        let got = TargetQuality::predict_quantizer(
            (0.0, 40.0),
            history.len() as f64 - 1.0,
            (Catmull, Quadratic),
            history,
            0.25,
        )
        .expect("whole history PCHIP");
        assert_eq!(got, if history.len() == 5 { 25.0 } else { 36.0 });
    }
}

#[test]
fn prediction_uses_bounded_midpoint_for_duplicate_flat_or_unbracketed_history() {
    let history = [(1.0, 0.0), (4.0, 1.0), (9.0, 2.0), (16.0, 3.0), (25.0, 4.0), (36.0, 5.0)];
    for method in PREDICTION_METHODS_METHODS {
        for count in 2..=6 {
            let mut duplicate = history[..count].to_vec();
            duplicate[count - 1].1 = duplicate[count - 2].1;
            let flat: Vec<_> = history[..count].iter().map(|(q, _)| (*q, 0.0)).collect();
            for (samples, target) in [
                (&duplicate[..], 0.5),
                (&flat[..], 0.0),
                (&history[..count], -1.0),
                (&history[..count], 10.0),
            ] {
                assert_eq!(
                    TargetQuality::predict_quantizer(
                        (0.0, 40.0),
                        target,
                        (method, method),
                        samples,
                        0.25,
                    )
                    .expect("bounded midpoint"),
                    20.0,
                    "{method:?}/{count}/{samples:?}/{target}"
                );
            }
        }
    }
}

#[test]
fn public_prediction_keeps_noisy_measurements_and_clamps_finite_estimates() {
    let noisy = [(1.0, 0.0), (9.0, 1.0), (4.0, 2.0), (16.0, 3.0), (25.0, 4.0), (36.0, 5.0)];
    for method in PREDICTION_METHODS_METHODS {
        for count in 2..=6 {
            assert_eq!(
                TargetQuality::predict_quantizer(
                    (0.0, 40.0),
                    1.0,
                    (method, method),
                    &noisy[..count],
                    0.25,
                )
                .expect("retain measured knot"),
                9.0,
                "{method:?}/{count}"
            );
            let got = TargetQuality::predict_quantizer(
                (0.0, 40.0),
                0.5,
                (method, method),
                &noisy[..count],
                0.25,
            )
            .expect("noisy interior prediction");
            assert!(got.is_finite() && (0.0..=40.0).contains(&got));
            for (range, want) in [((0.0, 2.0), 2.0), ((10.0, 20.0), 10.0)] {
                assert_eq!(
                    TargetQuality::predict_quantizer(
                        range,
                        1.0,
                        (method, method),
                        &noisy[..count],
                        0.25,
                    )
                    .expect("finite estimate is clamped"),
                    want,
                    "{method:?}/{count}"
                );
            }
        }
    }
}

#[test]
fn public_prediction_rejects_nonfinite_inputs_even_before_midpoint() {
    let history = [(1.0, 0.0), (4.0, 1.0), (9.0, 2.0), (16.0, 3.0), (25.0, 4.0), (36.0, 5.0)];
    for method in PREDICTION_METHODS_METHODS {
        for count in 0..=6 {
            for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
                let error = TargetQuality::predict_quantizer(
                    (0.0, 40.0),
                    invalid,
                    (method, method),
                    &history[..count],
                    0.25,
                )
                .expect_err("nonfinite target must be rejected");
                assert!(error.to_string().contains("target score"), "{error:#}");
                for index in 0..count {
                    for field in ["quantizer", "score"] {
                        let mut samples = history[..count].to_vec();
                        if field == "quantizer" {
                            samples[index].0 = invalid;
                        } else {
                            samples[index].1 = invalid;
                        }
                        let error = TargetQuality::predict_quantizer(
                            (0.0, 40.0),
                            0.5,
                            (method, method),
                            &samples,
                            0.25,
                        )
                        .expect_err("every nonfinite record must be rejected");
                        let message = error.to_string();
                        assert!(
                            message.contains(&format!("history[{index}].{field}")),
                            "{message}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn prediction_falls_back_from_nonfinite_interpolators_before_clamping() {
    use crate::models::sequence::target_quality::types::InterpolationMethod::*;
    let tiny = [(1.0, 0.0), (4.0, 1e-200), (9.0, 2e-200), (16.0, 3e-200)];
    // Lagrange products underflow to zero; the nearest linear pair still gives 6.5.
    for (count, methods) in [(3, (Quadratic, Linear)), (4, (Linear, CubicPolynomial))] {
        assert_eq!(
            TargetQuality::predict_quantizer((0.0, 40.0), 1.5e-200, methods, &tiny[..count], 0.25,)
                .expect("finite linear fallback"),
            6.5
        );
    }
    // Even the secant overflows: no infinity/NaN may reach the final clamp.
    let huge = [(f64::MAX, 0.0), (-f64::MAX, 1.0)];
    for method in PREDICTION_METHODS_METHODS {
        assert_eq!(
            TargetQuality::predict_quantizer((0.0, 40.0), 0.5, (method, method), &huge, 0.25,)
                .expect("finite midpoint fallback"),
            20.0
        );
        let extreme = [
            (f64::MAX, 0.0),
            (-f64::MAX, 1.0),
            (f64::MAX, 2.0),
            (-f64::MAX, 3.0),
            (f64::MAX, 4.0),
            (-f64::MAX, 5.0),
        ];
        for count in 2..=6 {
            let got = TargetQuality::predict_quantizer(
                (0.0, 40.0),
                0.5,
                (method, method),
                &extreme[..count],
                0.25,
            )
            .expect("overflow must stay bounded in every phase");
            assert!(
                got.is_finite() && (0.0..=40.0).contains(&got),
                "{method:?}/{count}: {got}"
            );
        }
    }
}

#[test]
fn duplicate_rounding_admits_an_untested_interior_probe() {
    use crate::models::sequence::target_quality::TargetQualityConfig;
    let config = TargetQualityConfig {
        quantizer_range: (26, 30),
        metric: QualityMetric::SSIMULACRA2 {
            target_range: (75.0, 75.0),
            resolution: None,
            threads: None,
            gpu_id: None,
        },
        ..Default::default()
    };
    let encoder = Encoder::default_from_base(&EncoderBase::FFmpeg, false);
    let history = [(26.0, 76.05), (30.0, 67.5)];
    assert_eq!(
        TargetQuality::predict_quantizer((26.0, 30.0), 75.0, config.interpolators, &history, 1.0)
            .expect("old interpolation remains public"),
        26.0
    );
    assert_eq!(
        TargetQuality::next_probe_quantizer(&config, &encoder, &history, &Default::default())
            .expect("next probe"),
        Some(28.0)
    );
    let single = TargetQualityConfig {
        quantizer_range: (26, 26),
        ..config
    };
    assert_eq!(
        TargetQuality::next_probe_quantizer(&single, &encoder, &[], &Default::default())
            .expect("single initial probe"),
        Some(26.0)
    );
    assert_eq!(
        TargetQuality::next_probe_quantizer(
            &single,
            &encoder,
            &[(26.0, 76.05)],
            &Default::default()
        )
        .expect("single exhausted probe"),
        None
    );
}

fn check_quantizer_grid(selected: &[&str]) {
    let cases = [
        (
            "quarter fallback",
            (26.0, 27.0),
            26.0,
            0.25,
            &[(26.0, 76.05), (27.0, 67.5)][..],
            Some(26.5),
        ),
        (
            "interior interpolation first",
            (26.0, 30.0),
            27.0,
            1.0,
            &[(26.0, 76.05), (30.0, 67.5)],
            Some(27.0),
        ),
        (
            "untested endpoint loses to interior",
            (26.0, 30.0),
            26.0,
            1.0,
            &[],
            Some(28.0),
        ),
        (
            "outside uses midpoint",
            (26.0, 30.0),
            40.0,
            1.0,
            &[],
            Some(28.0),
        ),
        (
            "unsorted repeated tied history",
            (26.0, 30.0),
            26.0,
            1.0,
            &[(30.0, 67.5), (28.0, 76.05), (26.0, 76.05), (28.0, 76.05)],
            Some(27.0),
        ),
        (
            "midpoint tested lower tie",
            (26.0, 30.0),
            28.0,
            1.0,
            &[(28.0, 75.0)],
            Some(27.0),
        ),
        (
            "midpoint and lower tested",
            (26.0, 30.0),
            28.0,
            1.0,
            &[(28.0, 75.0), (27.0, 76.0)],
            Some(29.0),
        ),
        ("initial", (26.0, 30.0), 28.0, 1.0, &[], Some(28.0)),
        ("equal initial", (26.0, 26.0), 26.0, 1.0, &[], Some(26.0)),
        (
            "equal measured",
            (26.0, 26.0),
            26.0,
            1.0,
            &[(26.0, 75.0)],
            None,
        ),
        (
            "adjacent measured",
            (26.0, 27.0),
            26.0,
            1.0,
            &[(26.0, 76.05), (27.0, 67.5)],
            None,
        ),
        (
            "fully exhausted",
            (26.0, 28.0),
            26.0,
            1.0,
            &[(26.0, 76.0), (28.0, 74.0), (27.0, 75.0)],
            None,
        ),
        (
            "missing side configured endpoint",
            (26.0, 28.0),
            27.0,
            1.0,
            &[(27.0, 76.0), (28.0, 74.0)],
            Some(26.0),
        ),
        (
            "bounds not grid aligned",
            (26.1, 27.9),
            26.1,
            1.0,
            &[],
            Some(27.0),
        ),
        ("no grid point", (26.1, 26.9), 26.5, 1.0, &[], None),
        (
            "large sparse range",
            (0.0, u32::MAX as f64),
            0.0,
            0.25,
            &[(0.0, 1.0), (2_147_483_647.5, 2.0)],
            Some(2_147_483_647.25),
        ),
    ];
    let mut checked = 0;
    for (name, bounds, predicted, step, history, expected) in cases {
        if !selected.contains(&name) {
            continue;
        }
        checked += 1;
        let actual = TargetQuality::untested_quantizer(bounds, predicted, history, step);
        assert_eq!(actual, expected, "{name}");
        if let Some(q) = actual {
            assert!((bounds.0..=bounds.1).contains(&q), "{name}");
            assert_eq!(q / step, (q / step).round(), "{name}");
            assert!(!history.iter().any(|(tested, _)| *tested == q), "{name}");
        }
    }
    assert_eq!(
        checked,
        selected.len(),
        "every selected grid scenario must run"
    );
}

#[test]
fn untested_quantizer_prefers_an_interior_prediction() {
    check_quantizer_grid(&[
        "interior interpolation first",
        "untested endpoint loses to interior",
        "outside uses midpoint",
        "initial",
        "equal initial",
    ]);
}

#[test]
fn untested_quantizer_uses_nearest_unmeasured_grid_point_with_lower_ties() {
    check_quantizer_grid(&[
        "quarter fallback",
        "unsorted repeated tied history",
        "midpoint tested lower tie",
        "midpoint and lower tested",
        "missing side configured endpoint",
    ]);
}

#[test]
fn untested_quantizer_returns_none_when_grid_is_exhausted() {
    check_quantizer_grid(&["equal measured", "adjacent measured", "fully exhausted"]);
}

#[test]
fn untested_quantizer_aligns_fractional_bounds_and_large_sparse_ranges() {
    check_quantizer_grid(&["bounds not grid aligned", "no grid point", "large sparse range"]);
}

#[test]
fn public_prediction_rejects_invalid_grid_before_arithmetic() {
    use crate::models::sequence::target_quality::types::InterpolationMethod;
    for (range, step, context) in [
        ((30.0, 26.0), 1.0, "range"),
        ((f64::NAN, 30.0), 1.0, "range"),
        ((26.0, f64::INFINITY), 1.0, "range"),
        ((26.0, 30.0), 0.0, "step"),
        ((26.0, 30.0), -1.0, "step"),
        ((26.0, 30.0), f64::NAN, "step"),
        ((26.0, 30.0), f64::INFINITY, "step"),
        ((26.0, 30.0), f64::MIN_POSITIVE, "grid"),
        ((-f64::MAX, f64::MAX), 1.0, "grid"),
    ] {
        let error = TargetQuality::predict_quantizer(
            range,
            75.0,
            (InterpolationMethod::Linear, InterpolationMethod::Linear),
            &[],
            step,
        )
        .expect_err("invalid public grid");
        assert!(format!("{error:#}").contains(context), "{error:#}");
    }
}

#[test]
fn final_selection_prefers_verified_then_closest_measured_with_higher_q_ties() {
    let metric = ssimulacra2();
    for (history, expected) in [
        (&[][..], None),
        (&[(28.0, 70.0)], Some(28.0)),
        (&[(26.0, 76.05), (30.0, 67.5)], Some(26.0)),
        (&[(24.0, 77.0), (28.0, 73.0)], Some(28.0)),
        (&[(28.0, 73.0), (24.0, 77.0)], Some(28.0)),
        (&[(26.0, 75.0), (28.0, 74.1), (30.0, 73.9)], Some(28.0)),
    ] {
        assert_eq!(
            TargetQuality::measured_quantizer(&metric, history),
            expected
        );
    }
    let inverse = QualityMetric::BUTTERAUGLI {
        target_range: (1.0, 1.0),
        resolution: None,
        threads: None,
        intensity_multiplier: None,
        norm: None,
        gpu_id: None,
    };
    assert_eq!(
        TargetQuality::measured_quantizer(&inverse, &[(20.0, 0.5), (28.0, 1.1), (30.0, 1.5)]),
        Some(28.0)
    );
}

// Finite explicit binaries exercise the actual cached capability probe;
// they establish step routing, not native SVT acceptance.
#[cfg(unix)]
fn check_svt_step_resolver() {
    use crate::{
        models::sequence::target_quality::TargetQualityConfig,
        utils::cancellation::{Cancellation, Cancelled},
    };
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().expect("executables");
    let config = TargetQualityConfig {
        quantizer_range: (26, 27),
        metric: QualityMetric::SSIMULACRA2 {
            target_range: (75.0, 75.0),
            resolution: None,
            threads: None,
            gpu_id: None,
        },
        ..Default::default()
    };
    for (name, exit, expected, next_expected) in
        [("quarter", 0, Some(26.5), Some(26.25)), ("integer", 1, None, None)]
    {
        let path = temp.path().join(name);
        std::fs::write(&path, format!("#!/bin/sh\nset -eu\n[ \"$3\" = --crf ]\n[ \"$4\" = 50.75 ]\nprintf probe >> \"$0.calls\"\n/bin/cat >/dev/null\nexit {exit}\n")).expect("fixture source");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("executable");
        let mut encoder = Encoder::default_from_base(&EncoderBase::SVTAV1, false);
        encoder.set_executable(Some(path.clone()));
        let history = [(26.0, 76.05), (27.0, 67.5)];
        let cancellation = Cancellation::default();
        for _ in 0..2 {
            assert_eq!(
                TargetQuality::next_probe_quantizer(&config, &encoder, &history, &cancellation)
                    .expect("actual resolver"),
                expected
            );
        }
        let completed = [(26.0, 76.05), (27.0, 67.5), (26.5, 74.0)];
        assert_eq!(
            TargetQuality::next_probe_quantizer(&config, &encoder, &completed, &cancellation)
                .expect("updated-history admission"),
            next_expected
        );
        assert_eq!(
            std::fs::read(path.with_extension("calls")).expect("capability invocation"),
            b"probe",
            "cache is keyed by the actual requested binary"
        );
        cancellation.request().expect("graceful");
        cancellation.request().expect("forced");
        let error = TargetQuality::next_probe_quantizer(&config, &encoder, &history, &cancellation)
            .expect_err("force must not become integer-step fallback");
        assert_eq!(error.downcast_ref::<Cancelled>(), Some(&Cancelled::Forced));
    }
    for base in [EncoderBase::X264, EncoderBase::X265] {
        let encoder = Encoder::default_from_base(&base, false);
        assert_eq!(
            TargetQuality::next_probe_quantizer(
                &config,
                &encoder,
                &[(26.0, 76.05), (27.0, 67.5)],
                &Default::default()
            )
            .expect("quarter step"),
            Some(26.5)
        );
    }
}

#[cfg(unix)]
mod internal_integration {
    use super::*;
    #[test]
    fn actual_svt_step_resolver_uses_requested_binary_cache_and_forced_cancellation() {
        check_svt_step_resolver();
    }
}

#[test]
fn inverse_bounds_use_nearest_probes() {
    let history = [(20.0, 0.5), (28.0, 0.9), (34.0, 1.2), (40.0, 1.6)];
    assert_eq!(
        TargetQuality::quantizer_bounds((5, 55), 1.0, &history, true),
        (28.0, 34.0)
    );
}

#[test]
fn quantizer_bounds_use_strict_directional_neighbors_and_stable_ties_or_configured_fallback() {
    let cases = [
        (
            "inverse unsorted",
            true,
            &[(34.0, 1.2), (20.0, 0.5), (40.0, 1.6), (28.0, 0.9)][..],
            (28.0, 34.0),
        ),
        (
            "higher mirror unsorted",
            false,
            &[(34.0, 0.8), (20.0, 1.5), (40.0, 0.4), (28.0, 1.1)],
            (28.0, 34.0),
        ),
        ("inverse empty", true, &[], (7.0, 51.0)),
        ("higher empty", false, &[], (7.0, 51.0)),
        (
            "inverse below singleton",
            true,
            &[(28.0, 0.9)],
            (28.0, 51.0),
        ),
        ("inverse above singleton", true, &[(34.0, 1.2)], (7.0, 34.0)),
        ("higher below singleton", false, &[(34.0, 0.8)], (7.0, 34.0)),
        (
            "higher above singleton",
            false,
            &[(28.0, 1.1)],
            (28.0, 51.0),
        ),
        ("inverse exact singleton", true, &[(30.0, 1.0)], (7.0, 51.0)),
        ("higher exact singleton", false, &[(30.0, 1.0)], (7.0, 51.0)),
        (
            "inverse below only",
            true,
            &[(20.0, 0.5), (28.0, 0.9)],
            (28.0, 51.0),
        ),
        (
            "inverse above only",
            true,
            &[(34.0, 1.2), (40.0, 1.6)],
            (7.0, 34.0),
        ),
        (
            "higher above only",
            false,
            &[(20.0, 1.5), (28.0, 1.1)],
            (28.0, 51.0),
        ),
        (
            "higher below only",
            false,
            &[(34.0, 0.8), (40.0, 0.4)],
            (7.0, 34.0),
        ),
        (
            "inverse stable ties and exact target",
            true,
            &[
                (20.0, 0.5),
                (28.0, 0.9),
                (27.0, 0.9),
                (30.0, 1.0),
                (35.0, 1.2),
                (34.0, 1.2),
                (40.0, 1.6),
            ],
            (27.0, 35.0),
        ),
        (
            "higher stable ties and exact target",
            false,
            &[
                (20.0, 1.5),
                (28.0, 1.1),
                (27.0, 1.1),
                (30.0, 1.0),
                (35.0, 0.8),
                (34.0, 0.8),
                (40.0, 0.4),
            ],
            (28.0, 34.0),
        ),
    ];
    for (name, inverse, history, expected) in cases {
        assert_eq!(
            TargetQuality::quantizer_bounds((7, 51), 1.0, history, inverse),
            expected,
            "{name}"
        );
    }
}

#[test]
fn noisy_inverse_bounds_predict_within_configured_range() {
    use crate::models::sequence::target_quality::types::InterpolationMethod;

    let histories = [
        [(20.0, 0.5), (40.0, 1.6), (30.0, 1.02), (31.0, 0.98)],
        [(31.0, 0.98), (30.0, 1.02), (40.0, 1.6), (20.0, 0.5)],
        [(20.0, 0.5), (40.0, 1.6), (30.0, 1.0002), (30.25, 0.9998)],
    ];
    for history in histories {
        let bounds = TargetQuality::quantizer_bounds((7, 51), 1.0, &history, true);
        let predicted = TargetQuality::predict_quantizer(
            bounds,
            1.0,
            (InterpolationMethod::Linear, InterpolationMethod::Linear),
            &history,
            0.25,
        )
        .expect("noisy inverse history must remain predictable");
        assert_eq!(bounds, (7.0, 51.0));
        assert!(
            predicted.is_finite() && (7.0..=51.0).contains(&predicted),
            "prediction {predicted} for {history:?}"
        );
    }
}

#[test]
fn noisy_higher_bounds_use_safe_configured_fallback() {
    // Q04 deliberately extends the Q03 inverse safety guard to either direction.
    let history = [(20.0, 1.6), (40.0, 0.5), (31.0, 1.02), (30.0, 0.98)];
    assert_eq!(
        TargetQuality::quantizer_bounds((7, 51), 1.0, &history, false),
        (7.0, 51.0)
    );
}

#[test]
fn target_history_gateway_and_verified_quantizer_use_scene_finals() {
    use crate::models::sequence::target_quality::{
        TargetQualityConfig,
        types::{ProbeStatistic, QualityPass},
    };
    let passes = [
        QualityPass {
            quantizer: 20.0,
            scores: vec![2.0, 9.0, 4.0],
            ..Default::default()
        },
        QualityPass {
            quantizer: 30.0,
            scores: vec![1.0, 8.0, 3.0],
            ..Default::default()
        },
    ];
    for disabled in [None, Some(false), Some(true)] {
        let config = TargetQualityConfig {
            metric: QualityMetric::CVVDP {
                target_range: (3.9, 4.1),
                resolution: None,
                display_model: None,
                resize_to_display: None,
                disable_temporal: disabled,
                gpu_id: None,
            },
            ..Default::default()
        };
        let history = TargetQuality::quantizer_score_history(&config, &passes).expect("history");
        match TargetQuality::quality_completion(&config, 7, &passes[0]).expect("completion") {
            SequenceCompletion::SceneQuality {
                index,
                quantizer,
                score,
                bitrate,
            } => {
                assert_eq!(
                    (index, quantizer, score, bitrate),
                    (7, 20.0, if disabled == Some(true) { 5.0 } else { 4.0 }, 0.0)
                );
            },
            other => panic!("unexpected completion {other:?}"),
        }
        assert_eq!(
            history,
            if disabled == Some(true) {
                vec![(20.0, 5.0), (30.0, 4.0)]
            } else {
                vec![(20.0, 4.0), (30.0, 3.0)]
            }
        );
        assert_eq!(
            TargetQuality::verified_quantizer(&config.metric, &history),
            Some(if disabled == Some(true) { 30.0 } else { 20.0 })
        );
        assert_eq!(config.probing.statistic, ProbeStatistic::Mean);
    }
}

#[test]
fn compare_submission_progress_declares_phase_once() {
    let (tx, rx) = sync::mpsc::channel();
    for completed in 1..=3 {
        super::report_compare_progress(&tx, 3, completed);
    }
    for expected in [0, 1, 2, 3] {
        assert!(
            matches!(rx.try_recv().expect("compare status"), SequenceStatus::Whole(Status::Processing {
                id, completion: SequenceCompletion::Frames { completed, total }
            }) if id == "Compare" && completed == expected && total == 3),
            "compare declares zero once, then reports each accepted frame"
        );
    }
    assert!(
        rx.try_recv().is_err(),
        "final scores do not count submission twice"
    );
}

/// A `Task` with `passes` recorded passes; only the count is consulted.
fn task_with_passes(index: usize, passes: usize) -> Task {
    Task {
        original_index: index,
        frame_indices: vec![0, 1],
        encoder: Default::default(),
        output: Default::default(),
        passes: vec![Default::default(); passes],
    }
}

fn ssimulacra2() -> QualityMetric {
    QualityMetric::SSIMULACRA2 {
        target_range: (74.0, 76.0),
        resolution: None,
        threads: None,
        gpu_id: None,
    }
}

mod probe_resume {
    use super::super::*;
    use super::probe_fixture::{
        probe_budget_progress, probe_budget_watchdog, probe_execution_fixture,
    };
    use crate::utils::cancellation::Cancelled;

    fn check_probe_publication(edits: &[&str]) {
        for &edit in edits {
            let (temp, mut condor, saved) = probe_execution_fixture(&[0, 0], 1);
            let model = temp.path().join("custom.json");
            std::fs::write(&model, b"AAA").expect("model identity fixture");
            if let QualityMetric::VMAF {
                model: configured, ..
            } = &mut condor.sequence_config.target_quality.as_mut().expect("TQ").metric
            {
                *configured = Some(model.clone());
            }
            let source = condor.input.as_data().path().expect("declared source").to_path_buf();
            let before = serde_json::to_vec(&condor.scenes).expect("before");
            let (tx, _rx) = sync::mpsc::channel();
            provenance::take_identity_counts();
            let result = TargetQuality::default().execute_with_probe(
                &mut condor,
                &tx,
                &Default::default(),
                |_, _, _, _, _, _, _, tasks, _, _, _| {
                    if matches!(edit, "model" | "source") {
                        let path = if edit == "model" { &model } else { &source };
                        let modified = path.metadata()?.modified()?;
                        std::fs::write(
                            path,
                            if edit == "model" {
                                b"BBB".as_slice()
                            } else {
                                b"ordinary source replacement".as_slice()
                            },
                        )?;
                        if edit == "model" {
                            std::fs::File::open(path)?
                                .set_times(std::fs::FileTimes::new().set_modified(modified))?;
                        }
                    }
                    let mut backend = MetricBackend::current(Engine::Vmaf);
                    if edit == "backend" {
                        backend.version = Some("changed during work".into());
                    }
                    let mut completed = tasks.to_vec();
                    for task in &mut completed {
                        task.passes.push(QualityPass {
                            quantizer: task.encoder.quantizer().expect("q"),
                            scores: vec![75.0; 2],
                            started_on: 1,
                            completed_on: 2,
                            backend: Some(backend.clone()),
                            ..Default::default()
                        });
                    }
                    Ok((completed, vec![]))
                },
            );
            assert_eq!(
                provenance::take_identity_counts(),
                (2, 0),
                "one model capture before work and one at publication, shared by both scenes"
            );
            if edit == "matching" {
                result.expect("matching batch publication");
                assert_eq!(saved.borrow().len(), 1);
                assert!(condor.scenes.iter().all(|scene| {
                    scene.sequence_data.target_quality.passes[0].provenance.is_some()
                }));
            } else {
                let error = result.expect_err("changed inputs cannot receive a cached certificate");
                let expected = if edit == "backend" {
                    "backend version/plugin admission identity changed"
                } else {
                    "inputs changed during measurement"
                };
                assert!(format!("{error:#}").contains(expected), "{error:#}");
                assert!(saved.borrow().is_empty());
                assert_eq!(
                    serde_json::to_vec(&condor.scenes).expect("no attached pass/selection"),
                    before
                );
            }
        }
    }

    fn check_fallback_attachment() {
        let (_temp, mut condor, saved) = probe_execution_fixture(&[0], 1);
        let config = condor.sequence_config.target_quality.as_mut().expect("TQ");
        config.metric = QualityMetric::default();
        *config.metric.target_range_mut() = (70.0, 80.0);
        let (tx, _rx) = sync::mpsc::channel();
        let mut calls = 0;
        TargetQuality::default()
            .execute_with_probe(
                &mut condor,
                &tx,
                &Default::default(),
                |_, _, config, _, _, _, _, tasks, progress, _, _| {
                    calls += 1;
                    let (compare, relay) = spawn_compare_progress(progress);
                    let (scores, actual) = TargetQuality::route_compare_scores(
                        Some(Err(anyhow::anyhow!(
                            "bounded native ABI failure before scores"
                        ))),
                        Some(Engine::Fmetrics),
                        &config.metric,
                        compare,
                        relay,
                        |sender| {
                            drop(sender);
                            Ok(vec![75.0; 2])
                        },
                        |scores| TargetQuality::validate_task_scores(config, tasks, scores),
                    )?
                    .context("successful fallback")?;
                    let mut completed = tasks.to_vec();
                    let quantizer = completed[0].encoder.quantizer().expect("q");
                    assert_eq!(actual, Engine::VapourSynth);
                    completed[0].passes.push(QualityPass {
                        quantizer,
                        scores,
                        started_on: 1,
                        completed_on: 2,
                        backend: Some(MetricBackend::available_plugin(&config.metric)?),
                        ..Default::default()
                    });
                    Ok((completed, vec![]))
                },
            )
            .expect("owning fallback attachment");
        assert_eq!(calls, 1);
        assert_eq!(saved.borrow().len(), 1);
        let pass = &condor.scenes[0].sequence_data.target_quality.passes[0];
        assert_eq!(
            pass.backend.as_ref().expect("actual").engine,
            Engine::VapourSynth
        );
        assert_eq!(
            pass.provenance.as_ref().expect("stamp").context["backend"]["actual"]["engine"],
            "VapourSynth"
        );
        let (tx, _rx) = sync::mpsc::channel();
        TargetQuality::default()
            .execute_with_probe(
                &mut condor,
                &tx,
                &Default::default(),
                |_, _, _, _, _, _, _, _, _, _, _| {
                    panic!("matching actual fallback must resume without another measurement")
                },
            )
            .expect("fallback matching resume");
    }

    fn check_probe_budget(test: &str, cases: &[&str]) {
        if let Ok(case) = std::env::var("PROBE_EXECUTION_CASE") {
            assert!(
                cases.contains(&case.as_str()),
                "budget case belongs to this contract"
            );
            let (maximum, expected, expected_calls) = match case.as_str() {
                "max-1" => (1, 1, 1),
                "max-2" => (2, 2, 2),
                "max-255" => (255, 255, 255),
                "early-hit" => (255, 1, 1),
                "failure" | "cancel" => (255, 1, 2),
                "forced" => (255, 2, 2),
                _ => panic!("unknown probe-budget case: {case}"),
            };
            let (temp, mut condor, saved) = probe_execution_fixture(&[0], 1);
            let config = condor.sequence_config.target_quality.as_mut().expect("TQ");
            config.maximum_probes = maximum;
            // Synthetic model-only FFmpeg grid, not a native codec range claim.
            config.quantizer_range = (0, u32::MAX);
            *config.metric.target_range_mut() = (1.0, 1.0);
            let (tx, rx) = sync::mpsc::channel();
            let mut ordinals = Vec::new();
            let result = TargetQuality::default().execute_with_probe(
                &mut condor,
                &tx,
                &Default::default(),
                |pass, _, _, _, _, _, _, tasks, progress, cancellation, _| {
                    ordinals.push(pass);
                    assert_ne!(pass, 0, "never call a wrapped pass");
                    assert_eq!(tasks.len(), 1);
                    assert_eq!(tasks[0].passes.len(), usize::from(pass - 1));
                    eprintln!(
                        "probe-budget {case}: callback={pass}, prefix={}, saves={}",
                        tasks[0].passes.len(),
                        saved.borrow().len()
                    );
                    if pass == 2 && case == "failure" {
                        bail!("probe-budget injected probe failure");
                    }
                    if pass == 2 && case == "cancel" {
                        cancellation.request()?;
                        return Ok((Vec::new(), Vec::new()));
                    }
                    let mut completed = tasks.to_vec();
                    let task = &mut completed[0];
                    let quantizer = task.encoder.quantizer().expect("scheduled q");
                    assert!(!task.passes.iter().any(|p| p.quantizer == quantizer));
                    task.passes.push(QualityPass {
                        provenance: None,
                        backend: Some(MetricBackend::current(Engine::Vmaf)),
                        quantizer,
                        scores: vec![
                            if case == "early-hit" {
                                1.0
                            } else if pass == 1 {
                                0.0
                            } else {
                                -f64::from(pass)
                            };
                            task.frame_indices.len()
                        ],
                        bitrate: 100.0 + f64::from(pass),
                        started_on: u128::from(pass) * 2,
                        completed_on: u128::from(pass) * 2 + 1,
                    });
                    for id in ["Encode", "Compare"] {
                        progress.send(SequenceStatus::Whole(Status::Completed {
                            id: id.to_owned(),
                        }))?;
                    }
                    if pass == 2 && case == "forced" {
                        cancellation.request()?;
                        cancellation.request()?;
                    }
                    // Dropping the real sender allows the owning progress relay to join.
                    Ok((completed, Vec::new()))
                },
            );
            match case.as_str() {
                "failure" => assert!(
                    result
                        .expect_err("probe failure")
                        .to_string()
                        .contains("probe-budget injected")
                ),
                "forced" => assert_eq!(
                    result.expect_err("force after accepted record").downcast_ref::<Cancelled>(),
                    Some(&Cancelled::Forced)
                ),
                _ => assert!(result.expect("bounded owning execute").1.is_empty()),
            }
            assert_eq!(ordinals, (1..=expected_calls).collect::<Vec<u8>>());
            let passes = &condor.scenes[0].sequence_data.target_quality.passes;
            eprintln!(
                "probe-budget {case}: returned callbacks={}, history={}, saves={}",
                ordinals.len(),
                passes.len(),
                saved.borrow().len()
            );
            assert_eq!(passes.len(), usize::from(expected));
            assert_eq!(saved.borrow().len(), usize::from(expected));
            for (ordinal, snapshot) in (1..=expected).zip(saved.borrow().iter()) {
                assert_eq!(
                    snapshot[0].sequence_data.target_quality.passes.len(),
                    usize::from(ordinal)
                );
            }
            assert_eq!(
                serde_json::to_vec(saved.borrow().last().expect("last save")).expect("saved"),
                serde_json::to_vec(&condor.scenes).expect("attached")
            );
            let final_q = condor.scenes[0].encoder.quantizer().expect("measured final q");
            assert_eq!(
                final_q, passes[0].quantizer,
                "initial score is closest to target"
            );
            assert!(passes.iter().any(|p| p.quantizer == final_q));
            let (rounds, quality) = probe_budget_progress(rx.try_iter().collect(), maximum);
            assert_eq!(rounds, ordinals);
            assert_eq!(quality, (1..=expected).collect::<Vec<u8>>());
            assert!(!temp.path().join("Target Quality/0").exists());
            assert!(!temp.path().join("Target Quality/256").exists());
            println!("probe-budget {case}: assertions completed");
            return;
        }
        for case in cases {
            let stdout = probe_budget_watchdog(
                case,
                &format!("core::sequence::target_quality::tests::probe_resume::{test}"),
            );
            assert!(stdout.contains(&format!("probe-budget {case}: assertions completed")));
        }
    }

    mod internal_integration {
        use super::*;

        #[test]
        fn changed_model_source_or_backend_refuses_probe_publication_atomically() {
            check_probe_publication(&["model", "source", "backend"]);
        }
        #[test]
        fn matching_probe_batch_stamps_provenance_with_one_identity_capture_per_boundary() {
            check_probe_publication(&["matching"]);
        }
        #[test]
        fn fallback_router_stamps_actual_branch_at_owning_attachment() {
            check_fallback_attachment();
        }

        macro_rules! budget_contract {
            ($name:ident, [$($case:literal),+ $(,)?]) => {
                #[test]
                fn $name() { check_probe_budget(concat!("internal_integration::",stringify!($name)), &[$($case),+]); }
            };
        }
        budget_contract!(
            probe_budget_one_two_and_255_never_wraps_ordinals_or_exceeds_maximum,
            ["max-1", "max-2", "max-255"]
        );
        budget_contract!(
            early_target_hit_stops_before_unused_probe_budget,
            ["early-hit"]
        );
        budget_contract!(
            probe_failure_or_cancellation_preserves_only_accepted_records,
            ["failure", "cancel", "forced"]
        );
    }
}

/// An incomplete pass must not be reused, or a resumed pass is scored as a
/// mixture of the abandoned attempt and the new one.
#[test]
fn a_pass_is_complete_only_when_every_scene_recorded_it() {
    assert!(!TargetQuality::pass_is_complete(
        &[task_with_passes(0, 0)],
        1
    ));
    // Recorded for one scene only, so its leftovers are purged.
    assert!(!TargetQuality::pass_is_complete(
        &[task_with_passes(0, 1), task_with_passes(1, 0)],
        1
    ));
    assert!(TargetQuality::pass_is_complete(
        &[task_with_passes(0, 1), task_with_passes(1, 1)],
        1
    ));
    // Completing an earlier pass says nothing about the current one.
    assert!(!TargetQuality::pass_is_complete(
        &[task_with_passes(0, 1)],
        2
    ));
}

#[test]
fn grouping_tq_scores_rejects_wrong_counts_or_nonfinite_values_without_mutating_tasks() {
    let mut tasks = [task_with_passes(3, 0), task_with_passes(7, 0)];
    for task in &mut tasks {
        task.frame_indices = vec![task.original_index];
    }
    for scores in [
        vec![],
        vec![1.0],
        vec![1.0, 2.0, 3.0],
        vec![1.0, f64::NAN],
        vec![1.0, f64::INFINITY],
        vec![1.0, f64::NEG_INFINITY],
    ] {
        let result = std::panic::catch_unwind(|| {
            TargetQuality::group_task_scores(&Default::default(), &tasks, scores)
        });
        assert!(
            matches!(result, Ok(Err(_))),
            "missing/extra/nonfinite must return Err, never drain/panic: {result:?}"
        );
        assert!(tasks.iter().all(|task| task.passes.is_empty()));
    }
}

fn check_score_batch_admission(corrupt: bool) {
    use crate::models::sequence::target_quality::types::ProbeStatistic;
    let mut tasks = [task_with_passes(3, 0), task_with_passes(7, 0)];
    for task in &mut tasks {
        task.frame_indices = vec![task.original_index, task.original_index + 1];
    }
    let mut config = TargetQualityConfig::default();
    config.probing.statistic = ProbeStatistic::Harmonic;
    if corrupt {
        for scores in [
            vec![],
            vec![2.0; 3],
            vec![2.0; 5],
            vec![2.0, 2.0, 1.0, f64::NAN],
            vec![2.0, 2.0, 1.0, f64::INFINITY],
            vec![2.0, 2.0, 1.0, f64::NEG_INFINITY],
            vec![2.0, 2.0, 1.0, -1.0],
        ] {
            for plugin in [false, true] {
                let (tx, rx) = sync::mpsc::channel();
                let (compare, relay) = spawn_compare_progress(tx);
                let native = (!plugin).then(|| Ok(scores.clone()));
                let error = TargetQuality::route_compare_scores(
                    native,
                    Some(Engine::Fmetrics),
                    &config.metric,
                    compare,
                    relay,
                    |tx| {
                        assert!(plugin, "corruption retried plugin");
                        drop(tx);
                        Ok(scores.clone())
                    },
                    |values| TargetQuality::validate_task_scores(&config, &tasks, values),
                )
                .expect_err("invalid batch");
                assert!(error.is::<MeasurementError>(), "{error:#}");
                if scores == [2.0, 2.0, 1.0, -1.0] {
                    assert!(format!("{error:#}").contains("scene 7 pass 1"), "{error:#}");
                }
                assert!(tasks.iter().all(|task| task.passes.is_empty()));
                assert!(
                    matches!(rx.try_recv(), Err(sync::mpsc::TryRecvError::Disconnected)),
                    "no completed event; relay joined/disconnected"
                );
            }
        }
    } else {
        let (tx, rx) = sync::mpsc::channel();
        let (compare, relay) = spawn_compare_progress(tx);
        let mut fallbacks = 0;
        let scores = TargetQuality::route_compare_scores(
            Some(Err(anyhow::anyhow!(
                av_metrics_fmetrics::FmetricsError::LibraryNotFound {
                    reason: "score validation test library absent".to_owned()
                }
            )
            .context("opening native library"))),
            Some(Engine::Fmetrics),
            &config.metric,
            compare,
            relay,
            |tx| {
                fallbacks += 1;
                drop(tx);
                Ok(vec![2.0; 4])
            },
            |values| TargetQuality::validate_task_scores(&config, &tasks, values),
        )
        .expect("availability fallback")
        .expect("complete");
        assert_eq!(fallbacks, 1);
        assert_eq!(scores.0, [2.0; 4]);
        assert_eq!(scores.1, Engine::VapourSynth);
        assert!(
            matches!(rx.try_recv(), Ok(SequenceStatus::Whole(Status::Completed { id })) if id == "Compare")
        );
    }
}

#[test]
fn corrupt_native_or_plugin_score_batch_never_mutates_tasks_or_emits_completion() {
    check_score_batch_admission(true);
}

#[test]
fn preprogress_native_failure_falls_back_once_and_completes_only_an_admitted_batch() {
    check_score_batch_admission(false);
}

#[test]
fn claimed_tq_history_rejects_bad_counts_values_and_reductions() {
    use crate::models::sequence::target_quality::types::ProbeStatistic;
    let mut config = TargetQualityConfig::default();
    config.probing.statistic = ProbeStatistic::Harmonic;
    for scores in [
        vec![],
        vec![1.0],
        vec![1.0; 3],
        vec![1.0, f64::NAN],
        vec![1.0, f64::INFINITY],
        vec![1.0, -1.0],
    ] {
        let error = TargetQuality::validate_history(
            &config,
            7,
            2,
            &[QualityPass {
                scores,
                ..Default::default()
            }],
        )
        .expect_err("claimed corrupt history");
        assert!(error.is::<MeasurementError>());
        assert!(format!("{error:#}").contains("scene 7 history pass 1"));
    }
    TargetQuality::validate_history(&config, 7, 2, &[]).expect("no history is unmeasured");
}

#[test]
fn verified_quantizer_uses_single_in_range_probe() {
    // One probe inside the target must be used as-is
    let history = [(30.0, 74.32)];
    assert_eq!(
        TargetQuality::verified_quantizer(&ssimulacra2(), &history),
        Some(30.0)
    );
}

#[test]
fn verified_quantizer_prefers_highest_in_range_quantizer() {
    let history = [(30.0, 71.96), (17.5, 79.76), (25.25, 75.31), (26.0, 74.1)];
    assert_eq!(
        TargetQuality::verified_quantizer(&ssimulacra2(), &history),
        Some(26.0)
    );
}

#[test]
fn verified_quantizer_is_none_without_in_range_probe() {
    let history = [(30.0, 64.67), (17.5, 82.31), (22.75, 77.86), (25.25, 73.98)];
    assert_eq!(
        TargetQuality::verified_quantizer(&ssimulacra2(), &history),
        None
    );
    assert_eq!(TargetQuality::verified_quantizer(&ssimulacra2(), &[]), None);
}

#[test]
fn verified_quantizer_chooses_highest_quantizer_within_an_inverse_metric_target() {
    let butteraugli = QualityMetric::BUTTERAUGLI {
        target_range: (0.8, 1.2),
        resolution: None,
        threads: None,
        intensity_multiplier: None,
        norm: None,
        gpu_id: None,
    };
    let history = [(20.0, 0.6), (28.0, 0.95), (32.0, 1.15), (40.0, 1.6)];
    assert_eq!(
        TargetQuality::verified_quantizer(&butteraugli, &history),
        Some(32.0)
    );
}

/// Models a local callback's event order, not a production or native scoring pass.
#[test]
fn local_compare_callback_emits_initial_zero_then_each_submission() {
    let (tx, rx) = sync::mpsc::channel();
    let total = 44_usize;

    let mut declared = true;
    let mut report = |position: usize, _score: f64| {
        if declared {
            declared = false;
            tx.send(SequenceStatus::Whole(Status::Processing {
                id: "Compare".to_owned(),
                completion: SequenceCompletion::Frames {
                    completed: 0,
                    total: total as u64,
                },
            }))
            .expect("send the phase declaration");
        }
        tx.send(SequenceStatus::Whole(Status::Processing {
            id: "Compare".to_owned(),
            completion: SequenceCompletion::Frames {
                completed: (position + 1) as u64,
                total: total as u64,
            },
        }))
        .expect("send progress");
    };

    for position in 0..total {
        report(position, 90.0);
    }
    drop(tx);

    let reports: Vec<(u64, u64)> = rx
        .iter()
        .filter_map(|status| match status {
            SequenceStatus::Whole(Status::Processing {
                completion: SequenceCompletion::Frames { completed, total },
                ..
            }) => Some((completed, total)),
            _ => None,
        })
        .collect();

    // The first report is the zero-count declaration, then one report per
    // frame, and the last reaches the total.
    assert_eq!(
        reports.first().map(|(completed, _)| *completed),
        Some(0),
        "the phase must be declared before any score is reported"
    );
    assert_eq!(
        reports.len(),
        total + 1,
        "one declaration plus one report per frame"
    );
    assert_eq!(
        reports.last(),
        Some(&(total as u64, total as u64)),
        "the last report must reach the total"
    );
}

/// Psychovisual sanitization preserves an already-unfiltered probe encoder.
#[test]
fn probe_encoder_without_an_ffmpeg_filter_stays_unfiltered() {
    let mut encoder = Encoder::default_from_base(&EncoderBase::X264, false);
    encoder.set_ffmpeg_filter(Some("crop=iw-16:ih-16".to_owned()));
    assert!(
        encoder.ffmpeg_filter().is_some(),
        "the scene encoder filters"
    );

    encoder.set_ffmpeg_filter(None);
    let probe = TargetQuality::remove_psychovisual_parameters(&encoder);

    assert_eq!(
        probe.ffmpeg_filter(),
        None,
        "the probe encoder must not filter"
    );
}

/// Probes are scored against the unfiltered reference, and a per-scene iso
/// would also make scenes differ beyond the quantizer, so the probe drops
/// the scene's photon noise while the final encode keeps it.
#[test]
fn a_probe_encoder_drops_the_photon_noise() {
    let mut encoder = Encoder::default_from_base(&EncoderBase::SVTAV1, false);
    let Encoder::SVTAV1 { photon_noise, .. } = &mut encoder else {
        panic!("the base is SVT-AV1");
    };
    *photon_noise = Some(PhotonNoise {
        iso: 12000,
        chroma_iso: Some(4000),
        width: None,
        height: None,
        c_y: None,
        ccb: None,
        ccr: None,
    });

    let Encoder::SVTAV1 {
        photon_noise: scene,
        ..
    } = &encoder
    else {
        panic!("the base is SVT-AV1");
    };
    assert!(scene.is_some(), "the scene encoder applies grain");

    let probe = TargetQuality::remove_psychovisual_parameters(&encoder);
    let Encoder::SVTAV1 {
        photon_noise: probe,
        ..
    } = &probe
    else {
        panic!("the base is SVT-AV1");
    };
    assert!(
        probe.is_none(),
        "the probe encoder must not apply photon noise"
    );
}
