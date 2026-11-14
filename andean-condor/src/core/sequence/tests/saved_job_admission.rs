//! Private identity-counter admission checks; public workflows live in tests/core.
use crate as condor;
#[path = "../../../../tests/core/sequence/fixtures/admission_support.rs"]
#[allow(
    dead_code,
    reason = "shared native fixture also contains the public-only owning producer"
)]
mod fixture;
mod internal_integration {
    use super::fixture::fixture;
    use crate::{
        core::sequence::{Sequence, target_quality, tests::support::current_target_fixture},
        models::sequence::target_quality::{TargetQualityConfig, types::QualityPass},
    };
    fn check_identity_capture(matching: bool) {
        use crate::models::sequence::{provenance, target_quality::types::QualityMetric};
        let mut observed = Vec::new();
        for plugin in [false, true] {
            let temp = tempfile::tempdir().expect("fixture");
            let mut job = fixture(temp.path());
            let model = temp.path().join("model.json");
            std::fs::write(&model, b"declared identity only; no scoring is requested")
                .expect("model");
            job.sequence_config.target_quality = Some(TargetQualityConfig {
                maximum_probes: 3,
                metric: if plugin {
                    // XPSNR stays plugin-only even when native metric engines are installed.
                    QualityMetric::XPSNR {
                        target_range: (70.0, 80.0),
                        resolution: None,
                    }
                } else {
                    QualityMetric::VMAF {
                        target_range: (70.0, 80.0),
                        resolution: None,
                        scaler: "bicubic".into(),
                        threads: 0,
                        model: Some(model),
                        features: vec![],
                    }
                },
                ..Default::default()
            });
            job.scenes[0].sequence_data.target_quality.passes = [20.0, 26.0, 30.0]
                .map(|quantizer| QualityPass {
                    quantizer,
                    scores: vec![75.0; 3],
                    started_on: 1,
                    completed_on: 2,
                    ..Default::default()
                })
                .to_vec();
            current_target_fixture(&mut job);
            if plugin {
                assert_eq!(
                    job.scenes[0].sequence_data.target_quality.passes[0]
                        .backend
                        .as_ref()
                        .expect("installed XPSNR plugin backend")
                        .engine,
                    crate::metrics::Engine::VapourSynth
                );
            }
            job.scenes = vec![job.scenes[0].clone(); 3];
            let current = job.scenes.clone();
            job.scenes[0].sequence_data.target_quality.passes[0]
                .provenance
                .as_mut()
                .expect("stamp")
                .revision = 0;
            provenance::take_identity_counts();
            let (tx, _rx) = std::sync::mpsc::channel();
            assert!(
                target_quality::TargetQuality::default()
                    .execute_cancel(&mut job, tx, Default::default())
                    .is_err()
            );
            assert_eq!(
                provenance::take_identity_counts(),
                (0, 0),
                "legacy refusal precedes identity initialization"
            );
            if !matching {
                continue;
            }
            job.scenes = current;
            for _ in 0..2 {
                let (tx, _rx) = std::sync::mpsc::channel();
                target_quality::TargetQuality::default()
                    .execute_cancel(&mut job, tx, Default::default())
                    .expect("owning matching admission without a probe");
                observed.push(provenance::take_identity_counts());
            }
        }
        if !matching {
            return;
        }
        assert_eq!(
            observed,
            [(1, 0), (1, 0), (0, 1), (0, 1)],
            "each invocation captures once, with no process-global cache"
        );
    }

    #[test]
    fn legacy_history_refusal_precedes_identity_initialization() {
        check_identity_capture(false);
    }
    #[test]
    fn each_saved_job_admission_captures_model_or_plugin_identity_once_per_batch() {
        check_identity_capture(true);
    }
}
