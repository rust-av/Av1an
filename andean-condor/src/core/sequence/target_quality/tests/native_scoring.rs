//! Real QC/TQ scoring boundaries and decoders; only the native ABI is doubled.
#![cfg(unix)]
#![expect(clippy::unwrap_used, reason = "fixture assertions fail loudly")]

use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc,
};

use crate::core::sequence::{
    SequenceCompletion, SequenceStatus, Status, target_quality::TargetQuality,
};
use crate::{
    metrics::{self, Engine, OutputIndexing},
    models::sequence::target_quality::types::QualityMetric,
};

use crate::core::sequence::tests::support::native_scoring::*;

mod internal_integration {
    use super::*;
    #[test]
    fn native_invalid_values_are_terminal_before_callbacks() {
        native_children(
            "core::sequence::target_quality::tests::native_scoring::internal_integration::native_invalid_values_are_terminal_before_callbacks",
            &[
                ("scene-f-tq", "invalid-score-nan"),
                ("scene-f-tq", "invalid-score-inf"),
                ("scene-f-tq", "invalid-score-neginf"),
                ("scene-v-tq", "invalid-score-nan"),
                ("scene-v-tq", "invalid-score-inf"),
                ("scene-v-tq", "invalid-score-neginf"),
            ],
            || {
                let engine = if std::env::var("TEMPORAL_FIXTURE_SCOPE").unwrap().contains("-f-") {
                    Engine::Fmetrics
                } else {
                    Engine::Vship
                };
                let (tx, rx) = mpsc::channel();
                let (compare, relay) =
                    crate::core::sequence::quality_check::spawn_compare_progress(tx);
                clear();
                let native = TargetQuality::score_native_scenes(
                    engine,
                    &mut decoder(&[1, 2]),
                    &mut decoder(&[10, 20]),
                    &metric(None),
                    &[&[0, 1]],
                    &compare,
                    &AtomicBool::new(false),
                );
                let error = TargetQuality::route_compare_scores(
                    Some(native),
                    Some(engine),
                    &metric(None),
                    compare,
                    relay,
                    |_| panic!("corrupt native result retried plugin"),
                    |_| Ok(()),
                )
                .expect_err("corrupt native score must fail");
                assert!(format!("{error:#}").contains("non-finite"), "{error:#}");
                assert!(
                    rx.try_iter().next().is_none(),
                    "invalid value must not publish progress/completion"
                );
                let p = if engine == Engine::Fmetrics { "f" } else { "v" };
                assert_eq!(calls(), format!("{p}c+\n{p}p\n{p}c-\n"));
                if engine == Engine::Fmetrics {
                    let butteraugli = QualityMetric::BUTTERAUGLI {
                        target_range: (0.0, 1.0),
                        resolution: None,
                        threads: None,
                        intensity_multiplier: None,
                        norm: Some(3),
                        gpu_id: None,
                    };
                    let (tx, _rx) = mpsc::channel();
                    let (compare, relay) =
                        crate::core::sequence::quality_check::spawn_compare_progress(tx);
                    let native = TargetQuality::score_native_scenes(
                        engine,
                        &mut decoder(&[1]),
                        &mut decoder(&[10]),
                        &butteraugli,
                        &[&[0]],
                        &compare,
                        &AtomicBool::new(false),
                    );
                    let error = TargetQuality::route_compare_scores(
                        Some(native),
                        Some(engine),
                        &butteraugli,
                        compare,
                        relay,
                        |_| panic!("native-rejected nonfinite Butteraugli retried plugin"),
                        |_| Ok(()),
                    )
                    .expect_err("corrupt native Butteraugli");
                    assert!(
                        error.is::<av_metrics_fmetrics::FmetricsError>(),
                        "retain original typed cause: {error:#}"
                    );
                    assert!(
                        error
                            .is::<crate::models::sequence::target_quality::types::MeasurementError>(
                            ),
                        "tag native-rejected corruption: {error:#}"
                    );
                }
            },
        );
    }

    #[test]
    fn tq_caller_failure_after_progress_does_not_fallback() {
        tq_caller(
            "core::sequence::target_quality::tests::native_scoring::internal_integration::tq_caller_failure_after_progress_does_not_fallback",
            false,
        );
    }

    #[test]
    fn tq_caller_cancel_after_progress_is_incomplete() {
        tq_caller(
            "core::sequence::target_quality::tests::native_scoring::internal_integration::tq_caller_cancel_after_progress_is_incomplete",
            true,
        );
    }

    fn tq_caller(test: &str, cancel: bool) {
        native_children(
            test,
            &[("scene-f-tq", "fp2"), ("scene-v-tq", "vp2")],
            || {
                let engine = if std::env::var("TEMPORAL_FIXTURE_SCOPE").unwrap().contains("-f-") {
                    Engine::Fmetrics
                } else {
                    Engine::Vship
                };
                for closed in [false, true] {
                    let (progress, receiver) = mpsc::channel();
                    let receiver = (!closed).then_some(receiver);
                    let (compare, relay) =
                        crate::core::sequence::quality_check::spawn_compare_progress(progress);
                    let mut a = decoder(&[1, 2, 3, 4, 5, 6]);
                    let mut b = decoder(&[10, 20, 50, 60]);
                    clear();
                    let cancelled = AtomicBool::new(false);
                    let mut positions = Vec::new();
                    let native = if cancel {
                        metrics::score_scene_frames(
                            engine,
                            &mut a,
                            &mut b,
                            &metric(None),
                            &[&[0, 1], &[4, 5]],
                            OutputIndexing::Compacted,
                            Some(&cancelled),
                            |position, _| {
                                positions.push(position);
                                let _ = compare.send(SequenceStatus::Whole(Status::Processing {
                                    id: "Compare".to_owned(),
                                    completion: SequenceCompletion::Frames {
                                        completed: 1,
                                        total: 4,
                                    },
                                }));
                                cancelled.store(true, Ordering::Relaxed);
                            },
                        )
                    } else {
                        TargetQuality::score_native_scenes(
                            engine,
                            &mut a,
                            &mut b,
                            &metric(None),
                            &[&[0, 1], &[4, 5]],
                            &compare,
                            &cancelled,
                        )
                    };
                    let result = TargetQuality::route_compare_scores(
                        Some(native),
                        Some(engine),
                        &metric(None),
                        compare,
                        relay,
                        |_| panic!("native partial failure/cancel retried a plugin"),
                        |_| Ok(()),
                    );
                    if cancel {
                        assert!(result.expect("cancel is an incomplete pass").is_none());
                    } else {
                        assert_native_failure(&result.expect_err("partial native failure"));
                    }
                    if cancel {
                        assert_eq!(positions, [0]);
                        let prefix = if engine == Engine::Fmetrics { "f" } else { "v" };
                        assert_eq!(calls(), format!("{prefix}c+\n{prefix}p\n{prefix}c-\n"));
                    } else {
                        assert_native_calls(false);
                    }
                    if let Some(receiver) = receiver {
                        let counts = receiver
                            .try_iter()
                            .map(|event| match event {
                                SequenceStatus::Whole(Status::Processing {
                                    completion: SequenceCompletion::Frames { completed, total },
                                    ..
                                }) => (completed, total),
                                other => panic!("unexpected {other:?}"),
                            })
                            .collect::<Vec<_>>();
                        assert_eq!(
                            counts,
                            if cancel {
                                vec![(1, 4)]
                            } else {
                                vec![(0, 4), (1, 4)]
                            }
                        );
                        assert!(
                            matches!(receiver.try_recv(), Err(mpsc::TryRecvError::Disconnected)),
                            "relay must have released its progress sender before return"
                        );
                    }
                }
            },
        );
    }
}

#[test]
fn tq_plugin_cancel_after_relay_cleanup_is_incomplete() {
    for closed in [false, true] {
        let (tx, rx) = mpsc::channel();
        let receiver = (!closed).then_some(rx);
        let (compare, relay) = crate::core::sequence::quality_check::spawn_compare_progress(tx);
        let result = TargetQuality::route_compare_scores(
            None,
            None,
            &metric(None),
            compare,
            relay,
            |tx| {
                let _ = tx.send(SequenceStatus::Whole(Status::Processing {
                    id: "CVVDP".to_owned(),
                    completion: SequenceCompletion::Frames {
                        completed: 1,
                        total: 4,
                    },
                }));
                Err(metrics::probe::Cancelled.into())
            },
            |_| Ok(()),
        );
        assert!(result.expect("plugin cancel remains incomplete after cleanup").is_none());
        if let Some(receiver) = receiver {
            assert!(matches!(
                receiver.try_recv(),
                Ok(SequenceStatus::Whole(Status::Processing { .. }))
            ));
            assert!(matches!(
                receiver.try_recv(),
                Err(mpsc::TryRecvError::Disconnected)
            ));
        }
    }
}
