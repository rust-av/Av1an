#[cfg(unix)]
pub(in crate::core::sequence) mod native_scoring;

use crate as condor;
use std::sync;
#[path = "../../../../../tests/core/sequence/target_quality/fixtures/probe_support.rs"]
mod probe_fixture;

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

// Real FFmpeg encodes, FFMS2 decode and libvmaf measurement; the resume
// history is produced by probe_pass, not a test score formula or fake backend.
#[test]
fn execute_resumed_duplicate_measures_another_probe_and_final_quantizer() {
    use crate::{
        core::{Condor, input::Input, output::Output, sequence::Sequence},
        models::{
            encoder::cli_parameter::CLIParameter,
            input::{ImportMethod, Input as InputModel},
            scene::Scene,
            sequence::{
                DefaultSequenceConfig, SequenceDataHandler,
                parallel_encoder::ParallelEncoderConfig,
                scene_concatenator::ConcatMethod,
                target_quality::{
                    TargetQualityConfig, TargetQualityData, TargetQualityDataHandler,
                },
            },
        },
        utils::cancellation::{Cancellation, Cancelled},
    };
    #[derive(Clone, Default, serde::Serialize)]
    struct AcceptedData {
        target_quality: TargetQualityData,
        #[serde(skip)]
        handoff: Option<(
            crossbeam_channel::Sender<()>,
            crossbeam_channel::Receiver<()>,
        )>,
    }
    impl SequenceDataHandler for AcceptedData {}
    impl TargetQualityDataHandler for AcceptedData {
        fn get_target_quality(&self) -> anyhow::Result<&TargetQualityData> {
            Ok(&self.target_quality)
        }
        fn get_target_quality_mut(&mut self) -> anyhow::Result<&mut TargetQualityData> {
            // This existing data-handler boundary runs after probe_pass accepts
            // real measurements. Pause only the armed test run, not production.
            if let Some((accepted, resume)) = self.handoff.take() {
                accepted
                    .send_timeout((), std::time::Duration::from_secs(10))
                    .expect("accepted handoff");
                resume.recv_timeout(std::time::Duration::from_secs(10)).expect("forced handoff");
            }
            Ok(&mut self.target_quality)
        }
    }
    let temp = tempfile::tempdir().expect("fixture");
    let source = temp.path().join("source.mkv");
    let generated = std::process::Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=128x128:rate=24",
            "-frames:v",
            "4",
            "-c:v",
            "ffv1",
        ])
        .arg(&source)
        .output()
        .expect("existing FFmpeg fixture generator");
    assert!(generated.status.success(), "{generated:?}");
    let input_model = InputModel::Video {
        path: source,
        import_method: ImportMethod::FFMS2 {
            index: None,
            cache_path: None,
        },
        filters: vec![],
    };
    let mut input = Input::from_data(&input_model).expect("FFMS2 input");
    let mut encoder = Encoder::default_from_base(&EncoderBase::FFmpeg, false);
    encoder.parameters_mut().extend(CLIParameter::new_strings(
        "-",
        " ",
        &[("c:v", "libx264"), ("preset", "ultrafast")],
    ));
    let mut config = TargetQualityConfig {
        quantizer_range: (26, 30),
        maximum_probes: 3,
        metric: QualityMetric::VMAF {
            target_range: (75.0, 75.0),
            resolution: None,
            scaler: "bicubic".into(),
            threads: 0,
            model: None,
            features: vec![],
        },
        ..Default::default()
    };
    let directory = temp.path().join("Target Quality");
    // The existing FFmpeg concat entry point requires its caller's scratch directory.
    for pass in 1..=3 {
        std::fs::create_dir_all(directory.join(pass.to_string()).join("Scene Concatenator"))
            .expect("concat scratch");
    }
    let concat = ConcatMethod::FFmpeg(Default::default());
    let (tx, rx) = sync::mpsc::channel();
    let cancellation = Default::default();
    let mut task = Task {
        original_index: 0,
        frame_indices: vec![0, 1, 2, 3],
        encoder: encoder.clone(),
        output: Default::default(),
        passes: vec![],
    };
    for (pass, q) in [(1, 26.0), (2, 30.0)] {
        task.encoder.set_quantizer(q);
        task.output = directory.join(pass.to_string()).join("00000.mkv");
        let (completed, warnings) = TargetQuality::probe_pass_with_options(
            pass,
            &directory,
            &config,
            &mut input,
            None,
            1,
            &concat,
            &[task],
            tx.clone(),
            &cancellation,
            Default::default(),
        )
        .expect("real measured resume history");
        assert!(warnings.is_empty(), "{warnings:?}");
        task = completed.into_iter().next().expect("accepted task");
    }
    let history = TargetQuality::quantizer_score_history(&config, &task.passes).expect("history");
    assert!(
        history[0].1 > history[1].1,
        "fixture must distinguish quantizers: {history:?}"
    );
    // A target 5% of the way from q26 to q30 interpolates to q26.2,
    // rounding to the already measured integer q26.
    let target = history[1].1.mul_add(0.05, history[0].1 * 0.95);
    *config.metric.target_range_mut() = (target, target);
    assert_eq!(
        TargetQuality::predict_quantizer((26.0, 30.0), target, config.interpolators, &history, 1.0)
            .expect("prediction"),
        26.0
    );
    let scene = Scene {
        start_frame: 0,
        end_frame: 4,
        sub_scenes: None,
        encoder: encoder.clone(),
        sequence_data: AcceptedData {
            target_quality: TargetQualityData {
                selection: None,
                speed_transition: None,
                passes: task.passes,
            },
            ..Default::default()
        },
    };
    let mut sequence_config = DefaultSequenceConfig {
        target_quality: Some(config.clone()),
        parallel_encoder: ParallelEncoderConfig::new(temp.path()),
        ..Default::default()
    };
    sequence_config.scene_concatenator.method = concat;
    let saved = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let snapshots = std::sync::Arc::clone(&saved);
    let mut condor = Condor::new(
        input,
        Output {
            path: temp.path().join("final.mkv"),
            overwrite: false,
            tags: Default::default(),
            video_tags: Default::default(),
        },
        encoder,
        vec![scene],
        Some(sequence_config),
        Box::new(move |data| {
            snapshots.lock().expect("snapshots").push(data.scenes);
            Ok(())
        }),
    );
    crate::core::sequence::tests::support::current_target_fixture(&mut condor);
    let mut sequence = TargetQuality::default();
    let (_, warnings) = sequence
        .execute_cancel(&mut condor, tx, cancellation)
        .expect("actual orchestration");
    assert!(warnings.is_empty(), "{warnings:?}");
    let scene = &condor.scenes[0];
    let measured = &scene.sequence_data.target_quality.passes;
    assert_eq!(
        measured.len(),
        3,
        "duplicate prediction must not end the search"
    );
    assert_eq!(measured[2].quantizer, 28.0);
    assert_eq!(measured[2].scores.len(), 4);
    let final_q = scene.encoder.quantizer().expect("final quantizer");
    assert!(measured.iter().any(|pass| pass.quantizer == final_q));
    let accepted = TargetQuality::quantizer_score_history(&config, measured).expect("history");
    eprintln!("quantizer-grid accepted={accepted:?}, target={target}, final={final_q}");
    assert_eq!(
        TargetQuality::verified_quantizer(&config.metric, &accepted),
        None,
        "target intentionally missed at maximum probes"
    );
    let closest = accepted
        .iter()
        .min_by(|(qa, sa), (qb, sb)| {
            (sa - target)
                .abs()
                .total_cmp(&(sb - target).abs())
                .then_with(|| qb.total_cmp(qa))
        })
        .expect("measured closest")
        .0;
    assert_eq!(final_q, closest);
    assert_eq!(
        saved.lock().expect("saved").last().expect("persisted")[0].encoder.quantizer(),
        Some(final_q)
    );
    assert!(rx.try_iter().any(|status| matches!(
        status,
        SequenceStatus::Subprocess {
            child: Status::Processing {
                completion: SequenceCompletion::SceneQuality {
                    quantizer: 28.0,
                    ..
                },
                ..
            },
            ..
        }
    )));

    // Also stop at the maximum immediately after an initial measurement:
    // an unmeasured next candidate must never become final.
    let one_probe = temp.path().join("one-probe");
    std::fs::create_dir_all(one_probe.join("Target Quality/1/Scene Concatenator"))
        .expect("scratch");
    condor.sequence_config.parallel_encoder.scenes_directory = one_probe;
    condor.sequence_config.target_quality.as_mut().expect("TQ").maximum_probes = 1;
    condor.scenes[0].sequence_data.target_quality.passes.clear();
    let (tx, _rx) = sync::mpsc::channel();
    sequence
        .execute_cancel(&mut condor, tx, Default::default())
        .expect("one real probe");
    let scene = &condor.scenes[0];
    assert_eq!(scene.sequence_data.target_quality.passes.len(), 1);
    assert_eq!(scene.sequence_data.target_quality.passes[0].quantizer, 28.0);
    assert_eq!(
        scene.encoder.quantizer(),
        Some(28.0),
        "never finalize the next unmeasured prediction after max probes"
    );

    let forced_probe = temp.path().join("forced-after-measurement");
    std::fs::create_dir_all(forced_probe.join("Target Quality/1/Scene Concatenator"))
        .expect("forced scratch");
    condor.sequence_config.parallel_encoder.scenes_directory = forced_probe;
    condor.scenes[0].sequence_data.target_quality.passes.clear();
    condor.scenes[0].encoder.set_quantizer(20.0);
    let before_saves = saved.lock().expect("saved").len();
    let cancellation = Cancellation::default();
    let force = cancellation.clone();
    let (accepted_tx, accepted_rx) = crossbeam_channel::bounded(0);
    let (resume_tx, resume_rx) = crossbeam_channel::bounded(0);
    condor.scenes[0].sequence_data.handoff = Some((accepted_tx, resume_rx));
    let (tx, rx) = sync::mpsc::channel();
    let progress = std::thread::spawn(move || {
        // Buffered progress alone cannot locate the cancellation window.
        // Rendezvous at accepted-history attachment after real Compare completes.
        while let Ok(status) = rx.recv_timeout(std::time::Duration::from_secs(10)) {
            if matches!(status, SequenceStatus::Subprocess { child: Status::Completed { id }, .. } if id == "Compare")
            {
                accepted_rx
                    .recv_timeout(std::time::Duration::from_secs(10))
                    .expect("accepted measurement");
                force.request().expect("graceful after measurement");
                force.request().expect("forced after measurement");
                resume_tx
                    .send_timeout((), std::time::Duration::from_secs(10))
                    .expect("resume accepted pass");
                // Keep the real UI receiver alive until execute drops every sender.
                for _ in rx {}
                return;
            }
        }
        panic!("real comparison never completed");
    });
    let error = sequence
        .execute_cancel(&mut condor, tx, cancellation)
        .expect_err("force still propagates");
    progress.join().expect("progress handoff");
    assert_eq!(error.downcast_ref::<Cancelled>(), Some(&Cancelled::Forced));
    let snapshots = saved.lock().expect("saved");
    assert_eq!(
        snapshots.len(),
        before_saves + 1,
        "accepted measurement must be saved before reporting Forced"
    );
    let persisted = &snapshots.last().expect("forced snapshot")[0];
    assert_eq!(persisted.sequence_data.target_quality.passes.len(), 1);
    assert_eq!(
        persisted.sequence_data.target_quality.passes[0].scores.len(),
        4
    );
    assert_eq!(
        persisted.sequence_data.target_quality.passes[0].quantizer,
        28.0
    );
    assert_eq!(
        persisted.encoder.quantizer(),
        Some(28.0),
        "save the measured final quantizer, not the old configured q"
    );
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
    use crate::{
        core::{Condor, output::Output, sequence::Sequence},
        models::{
            scene::Scene,
            sequence::{
                DefaultSequenceConfig, DefaultSequenceData, parallel_encoder::ParallelEncoderConfig,
            },
        },
        utils::cancellation::{Cancellation, Cancelled},
    };

    #[cfg(unix)]
    use super::probe_fixture::executable;
    use super::probe_fixture::{fixture, probe, tasks};

    type SavedScenes = std::rc::Rc<std::cell::RefCell<Vec<Vec<Scene<DefaultSequenceData>>>>>;

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

    fn probe_execution_fixture(
        counts: &[usize],
        maximum_probes: u8,
    ) -> (
        tempfile::TempDir,
        Condor<DefaultSequenceData, DefaultSequenceConfig>,
        SavedScenes,
    ) {
        let (temp, input, encoder, mut config, directory) = fixture();
        config.maximum_probes = maximum_probes;
        for pass in 1..=maximum_probes {
            std::fs::create_dir_all(directory.join(pass.to_string()).join("Scene Concatenator"))
                .expect("concat scratch");
        }
        // Synthetic, admitted saved records; all appended records use real FFmpeg/FFMS2/VMAF.
        let scenes = counts
            .iter()
            .enumerate()
            .map(|(index, &count)| {
                let mut data = DefaultSequenceData::default();
                data.target_quality.passes = (0..count)
                    .map(|pass| QualityPass {
                        provenance: None,
                        backend: Some(MetricBackend::current(Engine::Vmaf)),
                        quantizer: [26.0, 30.0, 34.0][pass],
                        scores: vec![10.0 + pass as f64; 2],
                        bitrate: 100.0 + pass as f64,
                        started_on: 12 + pass as u128,
                        completed_on: 34 + pass as u128,
                    })
                    .collect();
                Scene {
                    start_frame: index,
                    end_frame: index + 2,
                    sub_scenes: None,
                    encoder: encoder.clone(),
                    sequence_data: data,
                }
            })
            .collect();
        let saved = SavedScenes::default();
        let snapshots = std::rc::Rc::clone(&saved);
        let mut condor = Condor::new(
            input,
            Output {
                path: temp.path().join("final.mkv"),
                overwrite: false,
                tags: Default::default(),
                video_tags: Default::default(),
            },
            encoder,
            scenes,
            Some(DefaultSequenceConfig {
                target_quality: Some(config),
                parallel_encoder: ParallelEncoderConfig::new(temp.path()),
                ..Default::default()
            }),
            Box::new(move |data| {
                snapshots.borrow_mut().push(data.scenes);
                Ok(())
            }),
        );
        condor.sequence_config.scene_concatenator.method = ConcatMethod::FFmpeg(Default::default());
        crate::core::sequence::tests::support::current_target_fixture(&mut condor);
        (temp, condor, saved)
    }

    fn execute_probe_rounds(
        condor: &mut Condor<DefaultSequenceData, DefaultSequenceConfig>,
    ) -> Vec<(u8, u64, f64, f64)> {
        let (tx, rx) = sync::mpsc::channel();
        let (_, warnings) = TargetQuality::default()
            .execute_cancel(condor, tx, Default::default())
            .expect("actual sequence execution");
        assert!(warnings.is_empty(), "{warnings:?}");
        rx.into_iter()
            .filter_map(|event| match event {
                SequenceStatus::Subprocess {
                    parent:
                        Status::Processing {
                            completion: SequenceCompletion::Passes { completed, .. },
                            ..
                        },
                    child:
                        Status::Processing {
                            completion:
                                SequenceCompletion::SceneQuality {
                                    index,
                                    quantizer,
                                    score,
                                    ..
                                },
                            ..
                        },
                } => Some((completed, index, quantizer, score)),
                _ => None,
            })
            .collect()
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
            let stdout = probe_budget_watchdog(case, test);
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

    // Removing the normal wrapper's probe call or the last save must fail this test.
    #[test]
    fn public_execute_saves_255th_native_record() {
        if std::env::var("PROBE_EXECUTION_CASE").as_deref() != Ok("native-255") {
            probe_budget_watchdog("native-255", "public_execute_saves_255th_native_record");
            return;
        }
        let (temp, mut condor, saved) = probe_execution_fixture(&[0], 1);
        condor.sequence_config.target_quality.as_mut().expect("TQ").maximum_probes = 255;
        // Only the prefix is synthetic. The wrapper must encode/score the one new record.
        condor.scenes[0].sequence_data.target_quality.passes = vec![
            QualityPass {
                provenance: None,
                backend: Some(MetricBackend::current(Engine::Vmaf)),
                quantizer: 30.0,
                scores: vec![10.0; 2],
                bitrate: 100.0,
                started_on: 12,
                completed_on: 34,
            };
            254
        ];
        condor.scenes[0].encoder.set_quantizer(30.0);
        crate::core::sequence::tests::support::current_target_fixture(&mut condor);
        let before = serde_json::to_vec(&condor.scenes[0].sequence_data.target_quality.passes)
            .expect("synthetic prefix bytes");
        // Retain the existing concat-directory fixture workaround; do not fix it here.
        std::fs::create_dir_all(temp.path().join("Target Quality/255/Scene Concatenator"))
            .expect("last-pass concat scratch");
        let (tx, rx) = sync::mpsc::channel();
        eprintln!(
            "probe-budget native-255: entering public Sequence::execute with 254 synthetic records"
        );
        let (_, warnings) = TargetQuality::default()
            .execute(&mut condor, tx, Arc::new(AtomicBool::new(false)))
            .expect("one real final pass through normal wrapper");
        assert!(warnings.is_empty(), "{warnings:?}");
        let passes = &condor.scenes[0].sequence_data.target_quality.passes;
        eprintln!(
            "probe-budget native-255: returned history={}, saves={}",
            passes.len(),
            saved.borrow().len()
        );
        assert_eq!(passes.len(), 255);
        assert_eq!(serde_json::to_vec(&passes[..254]).expect("prefix"), before);
        let last = &passes[254];
        assert_eq!(last.quantizer, 32.0);
        assert_eq!(last.scores.len(), 2);
        assert!(last.scores.iter().all(|s| s.is_finite() && *s > 0.0));
        assert!(last.started_on > 34 && last.completed_on >= last.started_on);
        let bytes = std::fs::metadata(temp.path().join("Target Quality/255/00000.mkv"))
            .expect("one native encoded output")
            .len();
        let expected_bitrate = bytes as f64 * 96.0;
        assert!((last.bitrate - expected_bitrate).abs() < 1e-8);
        assert_eq!(
            saved.borrow().len(),
            1,
            "save actual 255th record before returning"
        );
        assert_eq!(
            serde_json::to_vec(&saved.borrow()[0]).expect("last save"),
            serde_json::to_vec(&condor.scenes).expect("attached last record")
        );
        let final_q = condor.scenes[0].encoder.quantizer().expect("measured q");
        assert!(passes.iter().any(|p| p.quantizer == final_q));
        let (rounds, quality) = probe_budget_progress(rx.into_iter().collect(), 255);
        assert_eq!(rounds, [255]);
        assert_eq!(quality, [255]);
        assert!(!temp.path().join("Target Quality/0").exists());
        assert!(!temp.path().join("Target Quality/256").exists());
        eprintln!(
            "probe-budget native-255: prefix byte-identical; one real q32 record retained/saved; progress255 only; final measured q={final_q}"
        );
    }

    fn probe_budget_progress(events: Vec<SequenceStatus>, maximum: u8) -> (Vec<u8>, Vec<u8>) {
        let mut rounds = Vec::new();
        let mut quality = Vec::new();
        for event in events {
            match event {
                SequenceStatus::Whole(Status::Processing {
                    completion: SequenceCompletion::Passes { completed, total },
                    ..
                }) => {
                    assert!(total > 0 && total <= maximum);
                    assert_eq!(
                        completed,
                        total - 1,
                        "zero completed is valid only before pass1"
                    );
                    rounds.push(total);
                },
                SequenceStatus::Subprocess {
                    parent:
                        Status::Processing {
                            completion: SequenceCompletion::Passes { completed, total },
                            ..
                        },
                    child,
                } => {
                    assert!(completed > 0 && completed <= maximum);
                    assert_eq!(total, maximum);
                    if matches!(
                        child,
                        Status::Processing {
                            completion: SequenceCompletion::SceneQuality { .. },
                            ..
                        }
                    ) {
                        quality.push(completed);
                    }
                },
                other => panic!("unexpected owning progress: {other:?}"),
            }
        }
        (rounds, quality)
    }

    fn probe_budget_watchdog(case: &str, test: &str) -> String {
        use std::{
            process::{Command, Stdio},
            time::{Duration, Instant},
        };
        // File-backed output cannot fill a pipe and masquerade as an overflow spin.
        // The parent owns child scratch, including cleanup after a killed child.
        let temp = tempfile::tempdir().expect("probe-budget watchdog root");
        let stdout_path = temp.path().join("stdout.log");
        let stderr_path = temp.path().join("stderr.log");
        let mut child = Command::new(std::env::current_exe().expect("test binary"))
            .args([
                "--exact",
                &format!("core::sequence::target_quality::tests::probe_resume::{test}"),
                "--nocapture",
            ])
            .env("PROBE_EXECUTION_CASE", case)
            .env("TMPDIR", temp.path())
            .stdout(Stdio::from(
                std::fs::File::create(&stdout_path).expect("child stdout"),
            ))
            .stderr(Stdio::from(
                std::fs::File::create(&stderr_path).expect("child stderr"),
            ))
            .spawn()
            .expect("bounded probe-budget child");
        let start = Instant::now();
        let mut timed_out = false;
        while child.try_wait().expect("child status").is_none() {
            if start.elapsed() > Duration::from_secs(60) {
                timed_out = true;
                child.kill().expect("kill stuck probe-budget child");
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let status = child.wait().expect("reap probe-budget child");
        let stdout = std::fs::read_to_string(stdout_path).expect("child stdout log");
        let stderr = std::fs::read_to_string(stderr_path).expect("child stderr log");
        eprintln!(
            "probe-budget {case}: watchdog timeout={timed_out}, status={status}\n{stdout}\n{stderr}"
        );
        assert!(
            !timed_out && status.success(),
            "probe-budget {case}: timeout={timed_out}, {status}"
        );
        assert!(
            stdout.contains("1 passed; 0 failed"),
            "child must execute its oracle"
        );
        stdout
    }

    #[test]
    fn maximum_budget_no_work_is_bounded() {
        use std::{
            process::{Command, Stdio},
            time::{Duration, Instant},
        };
        if let Ok(case) = std::env::var("PROBE_BUDGET_CASE") {
            let (temp, mut condor, saved) = probe_execution_fixture(&[1], 1);
            let config = condor.sequence_config.target_quality.as_mut().expect("TQ");
            config.maximum_probes = 255;
            match case.as_str() {
                "early-hit" => *config.metric.target_range_mut() = (10.0, 10.0),
                "budget-255" => {
                    // Synthetic valid records: this case needs no 255 native encodes.
                    let passes = &mut condor.scenes[0].sequence_data.target_quality.passes;
                    passes.resize(255, passes[0].clone());
                },
                _ => panic!("unknown H1 case: {case}"),
            }
            let mut unavailable = Encoder::default_from_base(&EncoderBase::SVTAV1, false);
            unavailable.set_executable(Some(temp.path().join("unavailable-custom-svt")));
            condor.scenes[0].encoder = unavailable;
            crate::core::sequence::tests::support::current_target_fixture(&mut condor);
            let before = serde_json::to_vec(&condor.scenes[0].sequence_data.target_quality.passes)
                .expect("prefix bytes");
            let directory = temp.path().join("Target Quality");
            let sentinel = directory.join("1/00000.ivf");
            std::fs::write(&sentinel, b"completed probe scratch").expect("sentinel");
            eprintln!(
                "probe-rounds H1 {case}: entering actual execute, saved count={}, max255",
                condor.scenes[0].sequence_data.target_quality.passes.len()
            );
            for (configured_q, expected_saves) in [(26.0, 0), (20.0, 1), (26.0, 1)] {
                condor.scenes[0].encoder.set_quantizer(configured_q);
                let (tx, rx) = sync::mpsc::channel();
                let (_, warnings) = TargetQuality::default()
                    .execute_cancel(&mut condor, tx, Default::default())
                    .expect("bounded no-work owning execute");
                assert!(warnings.is_empty(), "{warnings:?}");
                assert!(rx.try_iter().next().is_none(), "no skipped-round progress");
                assert_eq!(condor.scenes[0].encoder.quantizer(), Some(26.0));
                assert_eq!(saved.borrow().len(), expected_saves);
                assert_eq!(
                    serde_json::to_vec(&condor.scenes[0].sequence_data.target_quality.passes)
                        .expect("unchanged history"),
                    before
                );
                assert_eq!(
                    std::fs::read(&sentinel).expect("no excluded cleanup"),
                    b"completed probe scratch"
                );
                assert_eq!(
                    std::fs::read_dir(&directory).expect("scratch").count(),
                    2,
                    "no new ordinal directories"
                );
            }
            assert_eq!(saved.borrow()[0][0].encoder.quantizer(), Some(26.0));
            assert_eq!(
                serde_json::to_vec(&saved.borrow()[0][0].sequence_data.target_quality.passes)
                    .expect("saved history"),
                before
            );
            eprintln!(
                "probe-rounds H1 {case}: all three actual executions returned; identical history/scratch, no capability/native work/progress, only one changed-q restoration save"
            );
            return;
        }

        let mut failures = Vec::new();
        for case in ["early-hit", "budget-255"] {
            // Parent owns all child temporary files even if the watchdog must kill it.
            let temp = tempfile::tempdir().expect("watchdog root");
            let mut child = Command::new(std::env::current_exe().expect("test binary"))
                .args(["--exact", "core::sequence::target_quality::tests::probe_resume::maximum_budget_no_work_is_bounded", "--nocapture"])
                .env("PROBE_BUDGET_CASE", case)
                .env("TMPDIR", temp.path())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("bounded owning-execute child");
            let start = Instant::now();
            let mut timed_out = false;
            while child.try_wait().expect("child status").is_none() {
                if start.elapsed() > Duration::from_secs(10) {
                    timed_out = true;
                    child.kill().expect("kill stuck no-work child");
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
            let output = child.wait_with_output().expect("reap child and collect evidence");
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!(
                "probe-rounds H1 {case}: watchdog timeout={timed_out}, status={}\n{stdout}\n{stderr}",
                output.status
            );
            if timed_out || !output.status.success() || !stdout.contains("1 passed; 0 failed") {
                failures.push(format!("{case}: timeout={timed_out}, {}", output.status));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn execute_mixed_counts_preserves_prefixes_and_sparse_progress() {
        let (temp, mut condor, saved) = probe_execution_fixture(&[3, 1, 0], 2);
        let before = condor
            .scenes
            .iter()
            .map(|scene| {
                serde_json::to_vec(&scene.sequence_data.target_quality.passes).expect("saved bytes")
            })
            .collect::<Vec<_>>();
        let directory = temp.path().join("Target Quality");
        let sentinels = ["1/00000.mkv", "2/00000.mkv", "1/00001.mkv", "1/history.json"];
        for path in sentinels {
            std::fs::write(directory.join(path), b"verified peer scratch/history")
                .expect("sentinel");
        }
        let events = execute_probe_rounds(&mut condor);
        assert_eq!(
            condor
                .scenes
                .iter()
                .map(|s| s.sequence_data.target_quality.passes.len())
                .collect::<Vec<_>>(),
            [3, 2, 2]
        );
        assert_eq!(
            events.iter().map(|(pass, index, ..)| (*pass, *index)).collect::<Vec<_>>(),
            [(1, 2), (2, 1), (2, 2)]
        );
        for &(ordinal, index, quantizer, score) in &events {
            let record = &condor.scenes[index as usize].sequence_data.target_quality.passes
                [usize::from(ordinal - 1)];
            assert_eq!(quantizer, record.quantizer);
            assert_eq!(score, record.scores[0].midpoint(record.scores[1]));
        }
        for (index, (&prefix, scene)) in [3, 1, 0].iter().zip(&condor.scenes).enumerate() {
            let passes = &scene.sequence_data.target_quality.passes;
            assert_eq!(
                serde_json::to_vec(&passes[..prefix]).expect("prefix bytes"),
                before[index]
            );
            assert_eq!((scene.start_frame, scene.end_frame), (index, index + 2));
            for (ordinal, pass) in passes.iter().enumerate().skip(prefix) {
                assert_eq!(pass.scores.len(), 2);
                assert!(pass.scores.iter().all(|score| score.is_finite() && *score > 0.0));
                let bytes =
                    std::fs::metadata(directory.join(format!("{}/{index:05}.mkv", ordinal + 1)))
                        .expect("real output")
                        .len();
                let expected_bitrate = bytes as f64 * 96.0;
                assert!((pass.bitrate - expected_bitrate).abs() < 1e-8);
                assert!(pass.started_on > 34 && pass.completed_on >= pass.started_on);
            }
            let closest = passes
                .iter()
                .min_by(|a, b| {
                    let mean = |p: &QualityPass| p.scores[0].midpoint(p.scores[1]);
                    mean(a).total_cmp(&mean(b)).then_with(|| b.quantizer.total_cmp(&a.quantizer))
                })
                .expect("measured final");
            assert_eq!(scene.encoder.quantizer(), Some(closest.quantizer));
        }
        for path in sentinels {
            assert_eq!(
                std::fs::read(directory.join(path)).expect("excluded scratch"),
                b"verified peer scratch/history"
            );
        }
        let snapshots = saved.borrow();
        assert_eq!(snapshots.len(), 2, "one save per accepted nonempty round");
        assert_eq!(
            snapshots[0]
                .iter()
                .map(|s| s.sequence_data.target_quality.passes.len())
                .collect::<Vec<_>>(),
            [3, 1, 1]
        );
        assert_eq!(
            serde_json::to_vec(snapshots.last().expect("last save")).expect("saved"),
            serde_json::to_vec(&condor.scenes).expect("actual")
        );
        eprintln!(
            "probe-rounds mixed [3,1,0] max2 -> [3,2,2], real progress={events:?}; prefixes/timestamps/sentinels unchanged, 2 actual saves"
        );
    }

    #[test]
    fn execute_empty_earlier_rounds_reach_later_ordinal() {
        let (_temp, mut condor, saved) = probe_execution_fixture(&[2, 2], 3);
        let before = condor
            .scenes
            .iter()
            .map(|s| serde_json::to_vec(&s.sequence_data.target_quality.passes).expect("prefix"))
            .collect::<Vec<_>>();
        let events = execute_probe_rounds(&mut condor);
        assert_eq!(
            events.iter().map(|(pass, index, ..)| (*pass, *index)).collect::<Vec<_>>(),
            [(3, 0), (3, 1)]
        );
        for (index, scene) in condor.scenes.iter().enumerate() {
            let passes = &scene.sequence_data.target_quality.passes;
            assert_eq!(passes.len(), 3);
            assert_eq!(
                serde_json::to_vec(&passes[..2]).expect("unchanged prefix"),
                before[index]
            );
            assert_eq!(passes[2].scores.len(), 2);
            assert!(passes[2].scores.iter().all(|s| s.is_finite() && *s > 0.0));
        }
        assert_eq!(saved.borrow().len(), 1, "skipped ordinals never save");
        eprintln!(
            "probe-rounds [2,2] max3 -> [3,3], only real pass3 progress={events:?}, 1 actual save"
        );
    }

    #[test]
    fn execute_any_earlier_hit_skips_missing_encoder_then_changed_target_resumes() {
        let (temp, mut condor, saved) = probe_execution_fixture(&[3, 3], 4);
        let config = condor.sequence_config.target_quality.as_mut().expect("TQ");
        config.quantizer_range = (26, 38);
        *config.metric.target_range_mut() = (20.0, 20.0);
        condor.scenes[0].sequence_data.target_quality.passes[1].scores = vec![20.0; 2];
        let mut unavailable = Encoder::default_from_base(&EncoderBase::SVTAV1, false);
        unavailable.set_executable(Some(temp.path().join("unavailable-custom-svt")));
        unavailable.set_quantizer(30.0);
        condor.scenes[0].encoder = unavailable.clone();
        crate::core::sequence::tests::support::current_target_fixture(&mut condor);
        let prefix = serde_json::to_vec(&condor.scenes[0].sequence_data.target_quality.passes)
            .expect("current fixture prefix");
        let sentinel = temp.path().join("Target Quality/4/00000.ivf");
        std::fs::write(&sentinel, b"completed peer").expect("sentinel");
        let first = execute_probe_rounds(&mut condor);
        assert_eq!(
            first.iter().map(|(pass, index, ..)| (*pass, *index)).collect::<Vec<_>>(),
            [(4, 1)]
        );
        assert_eq!(
            serde_json::to_vec(&condor.scenes[0].sequence_data.target_quality.passes)
                .expect("unchanged"),
            prefix
        );
        assert_eq!(condor.scenes[0].encoder.quantizer(), Some(30.0));
        assert_eq!(
            std::fs::read(&sentinel).expect("no excluded cleanup"),
            b"completed peer"
        );
        assert_eq!(saved.borrow().len(), 1);
        let peer_output = temp.path().join("Target Quality/4/00001.mkv");
        let peer_bytes = std::fs::read(&peer_output).expect("real peer output");
        condor.scenes[0].encoder = condor.encoder.clone();
        unavailable.set_quantizer(condor.scenes[1].encoder.quantizer().expect("measured q"));
        condor.scenes[1].encoder = unavailable;
        *condor
            .sequence_config
            .target_quality
            .as_mut()
            .expect("TQ")
            .metric
            .target_range_mut() = (0.0, 0.0);
        crate::core::sequence::tests::support::current_target_fixture(&mut condor);
        let prefix = serde_json::to_vec(&condor.scenes[0].sequence_data.target_quality.passes)
            .expect("current fixture prefix");
        let peer_history =
            serde_json::to_vec(&condor.scenes[1].sequence_data.target_quality.passes)
                .expect("current fixture peer");
        let second = execute_probe_rounds(&mut condor);
        assert_eq!(
            second.iter().map(|(pass, index, ..)| (*pass, *index)).collect::<Vec<_>>(),
            [(4, 0)]
        );
        assert_eq!(
            condor.scenes[0].sequence_data.target_quality.passes.len(),
            4
        );
        assert_eq!(
            serde_json::to_vec(&condor.scenes[0].sequence_data.target_quality.passes[..3])
                .expect("retained old hit"),
            prefix
        );
        assert_eq!(
            serde_json::to_vec(&condor.scenes[1].sequence_data.target_quality.passes)
                .expect("over-budget peer"),
            peer_history
        );
        assert_eq!(
            std::fs::read(peer_output).expect("over-budget output"),
            peer_bytes
        );
        assert_eq!(saved.borrow().len(), 2);
        assert!(
            first
                .iter()
                .chain(&second)
                .all(|(_, _, _, score)| score.is_finite() && *score > 20.0)
        );
        eprintln!(
            "probe-rounds earlier middle hit excludes missing SVT; changed target resumes only that scene after its 3-record prefix, now-budget-complete missing peer excluded; progress={first:?}/{second:?}, saves=2"
        );
    }

    #[test]
    fn execute_lowered_budget_preserves_history_without_scratch_or_extra_saves() {
        let (temp, mut condor, saved) = probe_execution_fixture(&[3, 1], 1);
        let before = serde_json::to_vec(&condor.scenes).expect("scenes");
        let no_work = temp.path().join("no-work");
        condor.sequence_config.parallel_encoder.scenes_directory = no_work.clone();
        assert!(execute_probe_rounds(&mut condor).is_empty());
        assert_eq!(
            serde_json::to_vec(&condor.scenes).expect("unchanged"),
            before
        );
        assert!(saved.borrow().is_empty());
        assert!(
            !no_work.exists(),
            "budget admission precedes scratch creation"
        );
        condor.scenes[0].encoder.set_quantizer(20.0);
        assert!(execute_probe_rounds(&mut condor).is_empty());
        assert_eq!(condor.scenes[0].encoder.quantizer(), Some(26.0));
        assert!(execute_probe_rounds(&mut condor).is_empty());
        assert_eq!(
            saved.borrow().len(),
            1,
            "only changed measured-q restoration saves"
        );
        condor.sequence_config.parallel_encoder.scenes_directory = temp.path().to_owned();
        condor.sequence_config.target_quality.as_mut().expect("TQ").maximum_probes = 2;
        let raised = execute_probe_rounds(&mut condor);
        assert_eq!(
            raised.iter().map(|(pass, index, ..)| (*pass, *index)).collect::<Vec<_>>(),
            [(2, 1)]
        );
        assert_eq!(
            condor
                .scenes
                .iter()
                .map(|s| s.sequence_data.target_quality.passes.len())
                .collect::<Vec<_>>(),
            [3, 2]
        );
        let original: Vec<Scene<DefaultSequenceData>> =
            serde_json::from_slice(&before).expect("original scenes");
        for (scene, old) in condor.scenes.iter().zip(original) {
            let prefix = &scene.sequence_data.target_quality.passes
                [..old.sequence_data.target_quality.passes.len()];
            assert_eq!(
                serde_json::to_vec(prefix).expect("prefix"),
                serde_json::to_vec(&old.sequence_data.target_quality.passes)
                    .expect("original prefix")
            );
        }
        assert_eq!(saved.borrow().len(), 2);
        eprintln!(
            "probe-rounds lowered max1 [3,1] no work/scratch/save, one changed-q restoration save only; raised max2 -> [3,2] with progress={raised:?}"
        );
    }

    #[test]
    fn actual_probe_retains_bps_in_serialized_history() {
        let (_temp, mut input, encoder, config, directory) = fixture();
        let completed = probe(
            1,
            &directory,
            &config,
            &mut input,
            &tasks(&encoder, &directory, 1),
        )
        .expect("real encode/metric pass");
        for task in completed {
            // Each scene selects two frames at 24 FPS: twelve scenes/second, eight bits/byte.
            let bytes = std::fs::metadata(&task.output).expect("real output").len();
            let expected = bytes as f64 * 96.0;
            assert!((task.passes[0].bitrate - expected).abs() < 1e-8);
            let saved = serde_json::to_value(&task.passes).expect("persistable history");
            assert_eq!(saved[0]["bitrate"].as_f64(), Some(task.passes[0].bitrate));
            assert_eq!(task.passes[0].scores.len(), 2);
        }
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_failure_aborts_before_encoder_with_path_context() {
        let (temp, mut input, mut encoder, config, directory) = fixture();
        let script = temp.path().join("encoder");
        executable(
            &script,
            "#!/bin/sh\nprintf spawned > \"$0.calls\"\nexit 1\n",
        );
        encoder.set_executable(Some(script.clone()));
        let tasks = tasks(&encoder, &directory, 1);
        std::fs::create_dir_all(&tasks[0].output).expect("directory collision");
        let sentinel = tasks[0].output.join("caller-data");
        std::fs::write(&sentinel, b"preserve").expect("sentinel");
        let error =
            probe(1, &directory, &config, &mut input, &tasks).expect_err("cleanup must fail");
        eprintln!("probe-resume cleanup owning-path error: {error:#}");
        assert!(
            format!("{error:#}").contains(&tasks[0].output.display().to_string()),
            "{error:#}"
        );
        assert!(
            !script.with_extension("calls").exists(),
            "cleanup must precede any encoder spawn"
        );
        assert_eq!(
            std::fs::read(sentinel).expect("preserved directory"),
            b"preserve"
        );
        let mut escaped = tasks.clone();
        escaped[1].output = temp.path().join("source.mkv");
        assert!(probe(1, &directory, &config, &mut input, &escaped).is_err());
        assert!(
            escaped[1].output.exists(),
            "never delete outside the owned pass"
        );
        let detached = temp.path().join("caller-directory");
        std::fs::rename(directory.join("1"), &detached).expect("move scratch");
        std::os::unix::fs::symlink(&detached, directory.join("1")).expect("pass-dir link");
        assert!(probe(1, &directory, &config, &mut input, &tasks).is_err());
        assert_eq!(
            std::fs::read(detached.join("00000.mkv/caller-data"))
                .expect("linked directory preserved"),
            b"preserve"
        );
        assert!(!script.with_extension("calls").exists());
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_never_deletes_an_input_even_inside_pass_scratch() {
        let (temp, mut input, mut encoder, config, directory) = fixture();
        let script = temp.path().join("encoder");
        executable(
            &script,
            "#!/bin/sh\nprintf spawned > \"$0.calls\"\nexit 1\n",
        );
        encoder.set_executable(Some(script.clone()));
        let tasks = tasks(&encoder, &directory, 1);
        std::fs::copy(temp.path().join("source.mkv"), &tasks[0].output)
            .expect("source inside scratch");
        if let Input::Video { path, .. } = &mut input {
            *path = directory.join("1/../1/00000.mkv");
        }
        let before = std::fs::read(&tasks[0].output).expect("source bytes");
        assert!(probe(1, &directory, &config, &mut input, &tasks).is_err());
        assert_eq!(
            std::fs::read(&tasks[0].output).expect("source must be preserved"),
            before
        );
        assert!(!script.with_extension("calls").exists());
    }

    #[cfg(unix)]
    #[test]
    fn complete_pass_returns_unchanged_without_scratch_input_or_encoder() {
        let (temp, mut input, mut encoder, config, directory) = fixture();
        let script = temp.path().join("encoder");
        executable(
            &script,
            "#!/bin/sh\nprintf spawned > \"$0.calls\"\nexit 1\n",
        );
        encoder.set_executable(Some(script.clone()));
        let mut tasks = tasks(&encoder, &directory, 1);
        for task in &mut tasks {
            task.passes.push(QualityPass {
                provenance: None,
                backend: Some(MetricBackend::current(Engine::Vmaf)),
                quantizer: 26.0,
                scores: vec![75.0, 76.0],
                bitrate: 100.0,
                started_on: 12,
                completed_on: 34,
            });
        }
        std::fs::remove_file(temp.path().join("source.mkv")).expect("no retained source");
        for available in [true, false] {
            if !available {
                std::fs::remove_file(&script).expect("unavailable encoder");
            }
            let before = format!("{tasks:?}");
            let completed = probe(1, &directory, &config, &mut input, &tasks)
                .expect("complete validated history is sufficient");
            assert_eq!(format!("{completed:?}"), before);
            assert!(!script.with_extension("calls").exists());
            eprintln!(
                "probe-resume complete reuse: records unchanged, missing outputs/source, encoder available={available}, no spawn"
            );
        }
        let mut invalid = tasks.clone();
        invalid[1].passes[0].scores.pop();
        assert!(
            probe(1, &directory, &config, &mut input, &invalid)
                .expect_err("strict history")
                .is::<MeasurementError>()
        );
        let mut invalid_config = config.clone();
        *invalid_config.metric.target_range_mut() = (f64::NAN, 75.0);
        assert!(probe(1, &directory, &invalid_config, &mut input, &tasks).is_err());
        let mut temporal = config.clone();
        temporal.metric = QualityMetric::CVVDP {
            target_range: (0.0, 1.0),
            resolution: None,
            display_model: None,
            resize_to_display: None,
            disable_temporal: None,
            gpu_id: None,
        };
        let mut invalid_selection = tasks.clone();
        invalid_selection[0].frame_indices.reverse();
        let error = probe(1, &directory, &temporal, &mut input, &invalid_selection)
            .expect_err("pure temporal selection validation still precedes reuse");
        assert!(
            format!("{error:#}").contains("strictly increasing"),
            "{error:#}"
        );
        assert!(probe(0, &directory, &config, &mut input, &tasks).is_err());
        assert!(
            probe(1, &directory, &config, &mut input, &[])
                .expect("empty no-work")
                .is_empty()
        );
    }

    #[cfg(unix)]
    #[test]
    fn incomplete_pass_reencodes_current_quantizers_and_replaces_partial_records() {
        let (temp, mut input, encoder, config, directory) = fixture();
        let mut tasks = probe(
            1,
            &directory,
            &config,
            &mut input,
            &tasks(&encoder, &directory, 1),
        )
        .expect("verified prefix");
        let previous = tasks
            .iter()
            .map(|task| serde_json::to_value(&task.passes).expect("history"))
            .collect::<Vec<_>>();
        for task in &mut tasks {
            task.output = directory.join("2").join(format!("{:05}.mkv", task.original_index));
            task.encoder.set_quantizer(28.0);
        }
        let partial = probe(2, &directory, &config, &mut input, &tasks[..1])
            .expect("one scene has a current record");
        tasks[0].passes = partial[0].passes.clone();
        std::fs::write(&tasks[0].output, b"stale q28 marker").expect("abandoned output");
        let outside = temp.path().join("caller-directory");
        std::fs::create_dir_all(&outside).expect("outside directory");
        std::fs::write(outside.join("caller-data"), b"outside sentinel").expect("outside");
        std::os::unix::fs::symlink(&outside, &tasks[1].output).expect("stale output symlink");
        let unrelated = directory.join("2/unrelated.txt");
        std::fs::write(&unrelated, b"not a declared output").expect("caller neighbor");
        for (task, q) in tasks.iter_mut().zip([30.0, 34.0]) {
            task.encoder.set_quantizer(q);
        }
        let completed =
            probe(2, &directory, &config, &mut input, &tasks).expect("whole pass replay");
        eprintln!(
            "probe-resume mixed replay: {:?}",
            completed
                .iter()
                .map(|task| (
                    task.original_index,
                    task.passes.iter().map(|pass| pass.quantizer).collect::<Vec<_>>()
                ))
                .collect::<Vec<_>>()
        );
        for (index, task) in completed.iter().enumerate() {
            assert_eq!(
                task.passes.len(),
                2,
                "replace current record rather than appending a duplicate"
            );
            assert_eq!(
                serde_json::to_value(&task.passes[..1]).expect("prefix"),
                previous[index]
            );
            assert_eq!(task.passes[1].quantizer, [30.0, 34.0][index]);
            assert_eq!(task.passes[1].scores.len(), 2);
            assert!(task.passes[1].scores.iter().all(|score| score.is_finite()));
            assert!(
                !task.output.symlink_metadata().expect("fresh artifact").file_type().is_symlink()
            );
            assert_ne!(
                std::fs::read(&task.output).expect("fresh output"),
                b"stale q28 marker"
            );
        }
        assert_eq!(
            std::fs::read(outside.join("caller-data")).expect("symlink directory target preserved"),
            b"outside sentinel"
        );
        assert_eq!(
            std::fs::read(unrelated).expect("caller data preserved"),
            b"not a declared output"
        );
        tasks[1].passes.clear();
        let before = std::fs::read(&tasks[0].output).expect("output");
        assert!(
            probe(2, &directory, &config, &mut input, &tasks).is_err(),
            "missing earlier prefix must not be invented"
        );
        assert_eq!(
            std::fs::read(&tasks[0].output).expect("not deleted"),
            before
        );
    }

    #[test]
    fn execute_no_new_work_saves_only_changed_measured_restoration() {
        for target_hit in [true, false] {
            let (temp, mut input, encoder, mut config, directory) = fixture();
            let measured = probe(
                1,
                &directory,
                &config,
                &mut input,
                &tasks(&encoder, &directory, 1)[..1],
            )
            .expect("real measured history");
            let history = measured[0].passes.clone();
            config.quantizer_range = (26, 26);
            if target_hit {
                *config.metric.target_range_mut() = (0.0, 100.0);
            }
            let score = config
                .metric
                .calculate_score(config.probing.statistic, &history[0].scores)
                .expect("real measured score");
            assert_eq!(config.metric.score_within_target(score), target_hit);
            let mut scene_encoder = encoder.clone();
            scene_encoder.set_quantizer(20.0);
            let mut data = DefaultSequenceData::default();
            data.get_target_quality_mut().expect("TQ").passes = history.clone();
            let scenes = vec![Scene {
                start_frame: 0,
                end_frame: 2,
                sub_scenes: None,
                encoder: scene_encoder,
                sequence_data: data,
            }];
            let snapshots = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
            let saved = std::rc::Rc::clone(&snapshots);
            let mut condor = Condor::new(
                input,
                Output {
                    path: temp.path().join("final.mkv"),
                    overwrite: false,
                    tags: Default::default(),
                    video_tags: Default::default(),
                },
                encoder,
                scenes,
                Some(DefaultSequenceConfig {
                    target_quality: Some(config),
                    parallel_encoder: ParallelEncoderConfig::new(temp.path()),
                    ..Default::default()
                }),
                Box::new(move |data| {
                    saved.borrow_mut().push(data.scenes);
                    Ok(())
                }),
            );
            crate::core::sequence::tests::support::current_target_fixture(&mut condor);
            let history = condor.scenes[0].sequence_data.target_quality.passes.clone();
            for _ in 0..2 {
                let (tx, _rx) = sync::mpsc::channel();
                TargetQuality::default()
                    .execute_cancel(&mut condor, tx, Default::default())
                    .expect("no new work");
                assert_eq!(condor.scenes[0].encoder.quantizer(), Some(26.0));
                assert_eq!(
                    snapshots.borrow().len(),
                    1,
                    "actual callback once, no gratuitous repeated save"
                );
                let saved = snapshots.borrow();
                assert_eq!(saved[0][0].encoder.quantizer(), Some(26.0));
                assert_eq!(
                    serde_json::to_value(
                        &saved[0][0].sequence_data.get_target_quality().expect("TQ").passes
                    )
                    .expect("history"),
                    serde_json::to_value(&history).expect("history")
                );
            }
            for forced in [false, true] {
                let cancelled = Cancellation::default();
                cancelled.request().expect("graceful");
                if forced {
                    cancelled.request().expect("force");
                }
                let (tx, _rx) = sync::mpsc::channel();
                let error = TargetQuality::default()
                    .execute_cancel(&mut condor, tx, cancelled)
                    .expect_err("cancel precedence");
                assert_eq!(
                    error.downcast_ref::<Cancelled>(),
                    Some(if forced {
                        &Cancelled::Forced
                    } else {
                        &Cancelled::Graceful
                    })
                );
            }
            condor.scenes[0].encoder.set_quantizer(20.0);
            condor.sequence_config.target_quality = None;
            let (tx, _rx) = sync::mpsc::channel();
            TargetQuality::default()
                .execute_cancel(&mut condor, tx, Default::default())
                .expect("disabled");
            assert_eq!(condor.scenes[0].encoder.quantizer(), Some(20.0));
            condor.scenes.clear();
            let (tx, _rx) = sync::mpsc::channel();
            TargetQuality::default()
                .execute_cancel(&mut condor, tx, Default::default())
                .expect("empty");
            assert_eq!(snapshots.borrow().len(), 1);
            eprintln!(
                "probe-resume no-work target_hit={target_hit}: measured q26 restored from configured q20, exactly one actual save across repeated execution; disabled/empty/cancel controls unchanged"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn cancelled_or_callback_failed_execute_keeps_history_then_replays_current_q() {
        use std::time::{Duration, Instant};
        for mode in [
            "graceful",
            "forced",
            "closed-progress",
            "native-graceful",
            "native-forced",
            "native-callback-fault",
            "cleanup-failure",
        ] {
            let (temp, mut input, encoder, mut config, directory) = fixture();
            let measured = probe(
                1,
                &directory,
                &config,
                &mut input,
                &tasks(&encoder, &directory, 1),
            )
            .expect("verified prefix");
            config.quantizer_range = (30, 30);
            let script = temp.path().join("encoder");
            let ffmpeg = which::which("ffmpeg").expect("existing FFmpeg");
            // Finite rendezvous at actual encoder admission, not buffered UI timing.
            executable(
                &script,
                &format!(
                    "#!/bin/sh\nset -eu\nprintf started > \"$0.started\"\nn=0\nwhile [ \"$n\" -lt 500 ]; do\n n=$((n+1))\n if [ -f \"$0.release\" ]; then exec '{}' \"$@\"; fi\n sleep 0.01\ndone\nexit 1\n",
                    ffmpeg.display()
                ),
            );
            let scenes = measured
                .iter()
                .map(|task| {
                    let mut data = DefaultSequenceData::default();
                    data.get_target_quality_mut().expect("TQ").passes = task.passes.clone();
                    let mut encoder = encoder.clone();
                    if !mode.starts_with("native-") {
                        encoder.set_executable(Some(script.clone()));
                    }
                    Scene {
                        start_frame: task.frame_indices[0],
                        end_frame: task.frame_indices[1] + 1,
                        sub_scenes: None,
                        encoder,
                        sequence_data: data,
                    }
                })
                .collect::<Vec<_>>();
            let snapshots = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
            let saved = std::rc::Rc::clone(&snapshots);
            let mut condor = Condor::new(
                input,
                Output {
                    path: temp.path().join("final.mkv"),
                    overwrite: false,
                    tags: Default::default(),
                    video_tags: Default::default(),
                },
                encoder,
                scenes,
                Some(DefaultSequenceConfig {
                    target_quality: Some(config),
                    parallel_encoder: ParallelEncoderConfig::new(temp.path()),
                    ..Default::default()
                }),
                Box::new(move |data| {
                    saved.borrow_mut().push(data.scenes);
                    Ok(())
                }),
            );
            condor.sequence_config.scene_concatenator.method =
                ConcatMethod::FFmpeg(Default::default());
            crate::core::sequence::tests::support::current_target_fixture(&mut condor);
            let mut sequence = TargetQuality::default();
            if mode.starts_with("native-") {
                // Pause frame 1 in the real native VMAF reference decoder.
                // Frame 0 supplies metadata and one actual VMAF submission.
                let callback = format!(
                    "import vapoursynth as vs\nimport pathlib, time\np = pathlib.Path({path:?})\nc = vs.core\nclip = c.std.BlankClip(width=128, height=128, length=4, format=vs.YUV420P8, fpsnum=24)\ndef frame(n, f):\n if n == 1:\n  p.with_suffix('.started').write_text('native')\n  deadline = time.monotonic() + 10\n  while not p.with_suffix('.release').exists():\n   if time.monotonic() > deadline: raise RuntimeError('probe-resume native rendezvous timed out')\n   time.sleep(0.01)\n  if {fault}: raise RuntimeError('probe-resume reference callback fault')\n return f\nclip.std.ModifyFrame(clip, frame).std.SetFrameProps(_Matrix=1, _Transfer=1, _Primaries=1, _Range=1).set_output()\n",
                    path = script.display().to_string(),
                    fault = if mode == "native-callback-fault" {
                        "True"
                    } else {
                        "False"
                    }
                );
                sequence.metric_input = Some(
                    Input::from_data(&InputModel::VapourSynthScript {
                        source: crate::models::input::VapourSynthScriptSource::Text(callback),
                        variables: Default::default(),
                        index: 0,
                        filters: vec![],
                        stream_concurrently: false,
                    })
                    .expect("real lazy native reference input"),
                );
                condor.sequence_config.target_quality.as_mut().expect("TQ").metric_input =
                    sequence.metric_input.as_ref().map(Input::as_data);
                crate::core::sequence::tests::support::current_target_fixture(&mut condor);
            }
            let before = serde_json::to_value(&condor.scenes).expect("current fixture before");
            let cancellation = Cancellation::default();
            let signal = cancellation.clone();
            let barrier = script.clone();
            let (tx, rx) = sync::mpsc::channel();
            let (result, events) = if mode == "cleanup-failure" {
                let collision = directory.join("2/00000.mkv");
                std::fs::create_dir_all(&collision).expect("stale directory");
                std::fs::write(collision.join("sentinel"), b"preserve").expect("sentinel");
                let result = sequence.execute_cancel(&mut condor, tx, cancellation);
                assert!(
                    !script.with_extension("started").exists(),
                    "no native work before cleanup succeeds"
                );
                assert_eq!(
                    std::fs::read(collision.join("sentinel")).expect("preserved"),
                    b"preserve"
                );
                std::fs::remove_file(collision.join("sentinel")).expect("fixture cleanup");
                std::fs::remove_dir(collision).expect("fixture cleanup");
                (result, rx.try_iter().collect::<Vec<_>>())
            } else {
                let controller = thread::spawn(move || {
                    let start = Instant::now();
                    while !barrier.with_extension("started").exists() {
                        assert!(
                            start.elapsed() < Duration::from_secs(10),
                            "encoder never admitted"
                        );
                        thread::sleep(Duration::from_millis(10));
                    }
                    if mode == "closed-progress" {
                        drop(rx);
                    } else {
                        if mode != "native-callback-fault" {
                            signal.request().expect("graceful");
                        }
                        if mode.ends_with("forced") {
                            signal.request().expect("forced");
                        }
                        // Keep UI alive through every production relay join.
                        std::fs::write(barrier.with_extension("release"), b"go").expect("release");
                        return rx.into_iter().collect::<Vec<_>>();
                    }
                    std::fs::write(barrier.with_extension("release"), b"go").expect("release");
                    vec![]
                });
                let result = sequence.execute_cancel(&mut condor, tx, cancellation);
                (result, controller.join().expect("controller"))
            };
            if mode.ends_with("graceful") {
                result.expect("graceful incomplete pass");
            } else {
                let error = result.expect_err("force/callback error");
                if mode.ends_with("forced") {
                    assert_eq!(error.downcast_ref::<Cancelled>(), Some(&Cancelled::Forced));
                }
                if mode == "native-callback-fault" {
                    assert!(
                        format!("{error:#}").contains("probe-resume reference callback fault"),
                        "{error:#}"
                    );
                }
                if mode == "cleanup-failure" {
                    assert!(format!("{error:#}").contains("00000.mkv"), "{error:#}");
                }
            }
            assert_eq!(
                serde_json::to_value(&condor.scenes).expect("after"),
                before,
                "no accepted current record"
            );
            assert!(snapshots.borrow().is_empty(), "no empty-pass save");
            assert!(!events.iter().any(|event| matches!(
                event,
                SequenceStatus::Subprocess {
                    child: Status::Processing {
                        completion: SequenceCompletion::SceneQuality { .. },
                        ..
                    },
                    ..
                }
            )));
            if mode.starts_with("native-") {
                assert!(events.iter().any(|event| matches!(event, SequenceStatus::Subprocess { child: Status::Processing { id, completion: SequenceCompletion::Frames { completed: 1, .. } }, .. } if id == "Compare")), "actual native submission preceded cancellation/fault: {events:?}");
                assert!(!events.iter().any(|event| matches!(event, SequenceStatus::Subprocess { child: Status::Completed { id }, .. } if id == "Compare")));
            }
            for scene in &mut condor.scenes {
                scene.encoder.set_executable(None);
            }
            condor.sequence_config.target_quality.as_mut().expect("TQ").quantizer_range = (34, 34);
            condor.sequence_config.target_quality.as_mut().expect("TQ").metric_input = None;
            crate::core::sequence::tests::support::current_target_fixture(&mut condor);
            let (tx, _rx) = sync::mpsc::channel();
            TargetQuality::default()
                .execute_cancel(&mut condor, tx, Default::default())
                .expect("resume complete pass");
            assert_eq!(snapshots.borrow().len(), 1);
            for (index, scene) in condor.scenes.iter().enumerate() {
                let passes = &scene.sequence_data.get_target_quality().expect("TQ").passes;
                assert_eq!(passes.len(), 2);
                let mut raw_prefix = passes[..1].to_vec();
                for pass in &mut raw_prefix {
                    pass.provenance = None;
                }
                assert_eq!(
                    serde_json::to_value(&raw_prefix).expect("raw measured prefix"),
                    serde_json::to_value(&measured[index].passes).expect("prefix")
                );
                assert_eq!(
                    passes[1].quantizer, 34.0,
                    "resume uses current quantizer, not abandoned q30"
                );
                assert_eq!(passes[1].scores.len(), 2);
            }
            eprintln!(
                "probe-resume {mode}: interrupted owning execute kept both verified prefixes and made zero saves; resumed both at current q34 with one actual save"
            );
        }
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
