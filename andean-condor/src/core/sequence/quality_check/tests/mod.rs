#[cfg(unix)]
pub(in crate::core::sequence) mod native_scoring;

fn check_qc_attachment(phase: &str) {
    assert!(["invalid", "count", "valid"].contains(&phase));
    use crate::models::{
        scene::Scene,
        sequence::{DefaultSequenceData, target_quality::types::ProbeStatistic},
    };
    let original = (0..2)
        .map(|index| Scene {
            start_frame: index * 2,
            end_frame: index * 2 + 2,
            sub_scenes: None,
            encoder: Default::default(),
            sequence_data: DefaultSequenceData::default(),
        })
        .collect::<Vec<_>>();
    let before = serde_json::to_value(&original).expect("scenes");
    let selection = [(0, vec![0, 1]), (1, vec![2, 3])];
    if phase == "invalid" {
        for bad in [
            vec![],
            vec![1.0],
            vec![1.0, 2.0, 3.0],
            vec![1.0, f64::NAN],
            vec![1.0, f64::INFINITY],
            vec![1.0, f64::NEG_INFINITY],
            vec![1.0, -1.0],
        ] {
            let mut scenes = original.clone();
            let config = QualityCheckConfig {
                statistic: ProbeStatistic::Harmonic,
                ..Default::default()
            };
            let measurements = [vec![2.0, 2.0], bad]
                .into_iter()
                .map(|scores| SceneMeasurement {
                    scores,
                    started_on: 1,
                    completed_on: 2,
                })
                .collect();
            let error =
                QualityCheck::attach_measurements(&mut scenes, &config, &selection, measurements)
                    .expect_err("bad final scene");
            assert!(error.is::<MeasurementError>(), "{error:#}");
            let message = format!("{error:#}");
            assert!(message.contains("SSIMULACRA2 scene 1 pass 1"), "{message}");
            assert_eq!(
                serde_json::to_value(&scenes).expect("unchanged scenes"),
                before
            );
        }
    }
    if phase == "count" {
        for count in [0, 1, 3] {
            let mut scenes = original.clone();
            let measurements = std::iter::repeat_with(|| SceneMeasurement {
                scores: vec![2.0, 2.0],
                started_on: 1,
                completed_on: 2,
            })
            .take(count)
            .collect();
            let error = QualityCheck::attach_measurements(
                &mut scenes,
                &Default::default(),
                &selection,
                measurements,
            )
            .expect_err("measurement count");
            assert!(
                matches!(error.downcast_ref::<MeasurementError>(), Some(MeasurementError::Count { expected: 2, actual }) if *actual == count)
            );
            assert_eq!(serde_json::to_value(&scenes).expect("scenes"), before);
        }
    }
    if phase == "valid" {
        let mut scenes = original;
        let measurements = std::iter::repeat_with(|| SceneMeasurement {
            scores: vec![-1.0, 2.0],
            started_on: 1,
            completed_on: 2,
        })
        .take(2)
        .collect();
        QualityCheck::attach_measurements(
            &mut scenes,
            &Default::default(),
            &selection,
            measurements,
        )
        .expect("valid batch");
        assert!(
            scenes
                .iter()
                .all(|scene| scene.sequence_data.quality_check.quality.completed_on == 2)
        );
    }
}

#[test]
fn invalid_scene_measurements_leave_the_entire_qc_batch_unchanged() {
    check_qc_attachment("invalid");
}

#[test]
fn wrong_measurement_batch_size_leaves_qc_scenes_unchanged() {
    check_qc_attachment("count");
}

#[test]
fn valid_qc_measurements_attach_completion_for_every_scene() {
    check_qc_attachment("valid");
}

#[test]
fn plugin_callbacks_keep_sample_order() {
    let selection = [(7, vec![4, 9])];
    let mut state = (vec![vec![None; 2]], vec![0], vec![0], vec![0]);
    let (tx, rx) = sync::mpsc::channel();
    for (position, score) in [(1, 20.0), (0, 10.0)] {
        QualityCheck::record_plugin_score(
            &Default::default(),
            &selection,
            &mut state,
            position,
            score,
            &tx,
        )
        .expect("valid callback");
    }
    assert_eq!(
        state.0[0],
        [Some(10.0), Some(20.0)],
        "sample order, not callback arrival"
    );
    assert!(state.1[0] == 2 && state.2[0] > 0 && state.3[0] >= state.2[0]);
    assert_eq!(rx.try_iter().count(), 2, "no early scene summary");
}

#[test]
fn plugin_duplicate_does_not_mutate_or_report() {
    let selection = [(7, vec![4, 9])];
    let mut state = (vec![vec![None; 2]], vec![0], vec![0], vec![0]);
    let (tx, rx) = sync::mpsc::channel();
    QualityCheck::record_plugin_score(&Default::default(), &selection, &mut state, 1, 20.0, &tx)
        .expect("first callback");
    let before = state.clone();
    assert_eq!(rx.try_iter().count(), 1);
    let error = QualityCheck::record_plugin_score(
        &Default::default(),
        &selection,
        &mut state,
        1,
        99.0,
        &tx,
    )
    .expect_err("duplicate position");
    assert!(matches!(
        error.downcast_ref::<MeasurementError>(),
        Some(MeasurementError::Duplicate { position: 1 })
    ));
    assert_eq!(state, before, "scores, timestamps and completion unchanged");
    assert_eq!(rx.try_iter().count(), 0, "duplicate emits no progress");
    QualityCheck::record_plugin_score(&Default::default(), &selection, &mut state, 0, 10.0, &tx)
        .expect("missing slot still completes normally");
    let completed = state.clone();
    assert_eq!(rx.try_iter().count(), 1);
    QualityCheck::record_plugin_score(&Default::default(), &selection, &mut state, 0, 99.0, &tx)
        .expect_err("duplicate after completion");
    assert_eq!(state, completed);
    assert_eq!(rx.try_iter().count(), 0);
}

#[test]
fn plugin_callback_validates_before_index_progress_or_completion() {
    use crate::models::sequence::target_quality::types::ProbeStatistic;
    let selection = [(7, vec![4, 5])];
    for (position, score) in [
        (2, 1.0),
        (usize::MAX, 1.0),
        (0, f64::NAN),
        (0, f64::INFINITY),
        (0, f64::NEG_INFINITY),
    ] {
        let mut state = (vec![vec![None; 2]], vec![0], vec![0], vec![0]);
        let (tx, rx) = sync::mpsc::channel();
        let error = QualityCheck::record_plugin_score(
            &Default::default(),
            &selection,
            &mut state,
            position,
            score,
            &tx,
        )
        .expect_err("bad callback");
        assert!(error.is::<MeasurementError>());
        assert!(state.0[0].iter().all(Option::is_none) && state.1[0] == 0 && state.3[0] == 0);
        assert!(rx.try_iter().next().is_none());
    }
    let mut state = (vec![vec![None; 2]], vec![0], vec![0], vec![0]);
    let (tx, rx) = sync::mpsc::channel();
    let config = QualityCheckConfig {
        statistic: ProbeStatistic::Harmonic,
        ..Default::default()
    };
    QualityCheck::record_plugin_score(&config, &selection, &mut state, 0, 1.0, &tx)
        .expect("first value");
    assert!(
        QualityCheck::record_plugin_score(&config, &selection, &mut state, 1, -1.0, &tx).is_err()
    );
    assert_eq!(state.0[0], [Some(1.0), None]);
    assert!(state.1[0] == 1 && state.3[0] == 0);
    assert_eq!(
        rx.try_iter().count(),
        1,
        "no scene summary or invalid final frame event"
    );
}
#[test]
fn grouping_native_qc_scores_rejects_wrong_counts_and_nonfinite_values() {
    let scenes = [(3, vec![0]), (7, vec![2])];
    for scores in [
        vec![],
        vec![1.0],
        vec![1.0, 2.0, 3.0],
        vec![1.0, f64::NAN],
        vec![1.0, f64::INFINITY],
        vec![1.0, f64::NEG_INFINITY],
    ] {
        assert!(
            super::QualityCheck::group_native_scores(&scores, 2, &scenes).is_err(),
            "{scores:?}"
        );
    }
}
#[test]
fn live_scene_quality_producer_uses_final_prefix() {
    use super::*;
    use crate::models::sequence::target_quality::types::{ProbeStatistic, QualityMetric};
    for disabled in [None, Some(false), Some(true)] {
        for (statistic, independent) in [
            (ProbeStatistic::Mean, 5.0),
            (ProbeStatistic::Minimum, 2.0),
            (ProbeStatistic::Maximum, 9.0),
        ] {
            let config = QualityCheckConfig {
                metric: QualityMetric::CVVDP {
                    target_range: (3.9, 4.1),
                    resolution: None,
                    display_model: None,
                    resize_to_display: None,
                    disable_temporal: disabled,
                    gpu_id: None,
                },
                statistic,
                ..Default::default()
            };
            let (tx, rx) = sync::mpsc::channel();
            QualityCheck::report_scene_score(&config, 7, &[2.0, 9.0, 4.0], 3, 6, &tx)
                .expect("score");
            match rx.try_recv().expect("live scene event") {
                SequenceStatus::Subprocess {
                    parent:
                        Status::Processing {
                            completion:
                                SequenceCompletion::Custom {
                                    completed, total, ..
                                },
                            ..
                        },
                    child:
                        Status::Processing {
                            completion: SequenceCompletion::SceneQuality { index, score, .. },
                            ..
                        },
                } => {
                    assert_eq!((index, completed, total), (7, 3.0, 6.0));
                    assert_eq!(
                        score,
                        if disabled == Some(true) {
                            independent
                        } else {
                            4.0
                        }
                    );
                },
                other => panic!("unexpected event {other:?}"),
            }
        }
    }
}

use super::*;

#[cfg(unix)]
mod internal_integration {
    use super::*;
    fn native_caller(test: &str, cancel: bool) {
        native_scoring::native_caller::qc_native_caller(
            test,
            cancel,
            |input, output, config, scenes, progress, cancelled| {
                QualityCheck::measure_native(input, output, config, scenes, progress, cancelled)
                    .map(|scores| scores.is_some())
            },
        );
    }

    #[test]
    fn qc_native_failure_after_progress_retains_typed_cause_without_plugin_fallback() {
        native_caller(
            "core::sequence::quality_check::tests::internal_integration::qc_native_failure_after_progress_retains_typed_cause_without_plugin_fallback",
            false,
        );
    }

    #[test]
    fn qc_native_cancellation_does_not_request_plugin_fallback() {
        native_caller(
            "core::sequence::quality_check::tests::internal_integration::qc_native_cancellation_does_not_request_plugin_fallback",
            true,
        );
    }
}

#[test]
fn final_scores_do_not_regress_submission_progress() {
    let (tx, rx) = sync::mpsc::channel();
    for position in 0..3 {
        QualityCheck::report_vmaf_submission(3, position, &tx);
    }
    for position in 0..3 {
        QualityCheck::report_frame_score(&[4, 7, 9], 3, position, 90.0 + position as f64, 3, &tx);
    }
    for expected in 1..=3 {
        assert!(
            matches!(rx.try_recv().expect("submitted status"),SequenceStatus::Whole(Status::Processing {
            completion: SequenceCompletion::Custom { completed,total,.. },..
        }) if completed == expected as f64 && total == 3.0)
        );
    }
    for (expected_frame, expected_score) in [(4, 90.0), (7, 91.0), (9, 92.0)] {
        match rx.try_recv().expect("score status") {
            SequenceStatus::Subprocess {
                parent:
                    Status::Processing {
                        completion:
                            SequenceCompletion::Custom {
                                completed, total, ..
                            },
                        ..
                    },
                child:
                    Status::Processing {
                        completion: SequenceCompletion::FrameScore { frame, score },
                        ..
                    },
            } => {
                assert_eq!(
                    (completed, total),
                    (3.0, 3.0),
                    "final scoring must not restart the progress bar"
                );
                assert_eq!((frame, score), (expected_frame, expected_score));
            },
            other => panic!("unexpected status: {other:?}"),
        }
    }
    assert!(rx.try_recv().is_err(), "no double-counted progress");
}
