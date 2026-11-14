//! Private QC/TQ scoring boundaries and pure compare-relay contracts.
#![cfg(unix)]
#![expect(clippy::unwrap_used, reason = "fixture assertions fail loudly")]
use crate as condor;
use crate::core::sequence::tests::support::current_quality_fixture;
use crate::core::sequence::tests::support::native_scoring::*;
#[path = "../../../../../tests/core/sequence/quality_check/fixtures/qc_support.rs"]
#[allow(
    dead_code,
    reason = "shared fixture also contains public-only output admission drivers"
)]
mod public_fixture;

mod internal_integration {
    use super::*;
    #[test]
    fn corrupt_qc_batch_retains_the_started_scoring_marker() {
        public_fixture::corrupt_batch(
            "core::sequence::quality_check::tests::native_scoring::internal_integration::corrupt_qc_batch_retains_the_started_scoring_marker",
            Some(&|error| {
                assert!(
                    error.is::<metrics::SceneScoringStarted>(),
                    "earlier valid frames were reported"
                );
            }),
        );
    }
    #[test]
    fn failed_qc_replacement_retains_the_started_scoring_marker() {
        public_fixture::check_qc_history(
            "core::sequence::quality_check::tests::native_scoring::internal_integration::failed_qc_replacement_retains_the_started_scoring_marker",
            true,
            Some(&|error| {
                assert!(
                    error.is::<crate::models::sequence::target_quality::types::MeasurementError>()
                        && error.is::<metrics::SceneScoringStarted>(),
                    "{error:#}"
                );
            }),
        );
    }
}
use crate::{
    core::sequence::{SequenceCompletion, SequenceStatus, Status, target_quality::TargetQuality},
    metrics::{self, Engine},
};
use std::{
    sync::{atomic::AtomicBool, mpsc},
    time::Duration,
};

#[test]
fn native_scene_consumers() {
    native_children(
        "core::sequence::quality_check::tests::native_scoring::native_scene_consumers",
        &[
            ("scene-f-qc", ""),
            ("scene-v-qc", ""),
            ("scene-f-tq", ""),
            ("scene-v-tq", ""),
            ("scene-f-qc", "fp"),
            ("scene-v-qc", "vp"),
            ("scene-f-qc", "fp2"),
            ("scene-v-qc", "vp2"),
        ],
        || {
            let engine = if std::env::var("TEMPORAL_FIXTURE_SCOPE").unwrap().contains("-f-") {
                Engine::Fmetrics
            } else {
                Engine::Vship
            };
            let failure = std::env::var("TEMPORAL_FIXTURE_FAIL").unwrap();
            if failure.is_empty() {
                native_consumers(engine);
                sequential_decoders(engine);
            } else {
                native_failure(engine, &failure);
            }
        },
    );
}

#[test]
fn native_scene_final_reduction() {
    native_children(
        "core::sequence::quality_check::tests::native_scoring::native_scene_final_reduction",
        &[("scene-f-qc", ""), ("scene-v-qc", ""), ("scene-f-tq", ""), ("scene-v-tq", "")],
        || {
            let engine = if std::env::var("TEMPORAL_FIXTURE_SCOPE").unwrap().contains("-f-") {
                Engine::Fmetrics
            } else {
                Engine::Vship
            };
            native_consumers(engine);
        },
    );
}

pub(in crate::core::sequence) fn qc_native_caller(
    test: &str,
    measure: impl Fn(
        &mut crate::core::input::Input,
        &std::path::Path,
        &crate::models::sequence::quality_check::QualityCheckConfig,
        &[(usize, Vec<usize>)],
        &mpsc::Sender<SequenceStatus>,
        &AtomicBool,
    ) -> anyhow::Result<bool>,
) {
    native_children(
        test,
        &[("scene-f-qc", "fp2"), ("scene-v-qc", "vp2")],
        || {
            use crate::models::input::{ImportMethod, Input as InputModel};
            let source = y4m_fixture(&[32, 48, 64, 80, 96, 112]);
            let output = y4m_fixture(&[40, 56, 72, 88, 104, 120]);
            let config = crate::models::sequence::quality_check::QualityCheckConfig {
                metric: metric(None),
                ..Default::default()
            };
            for (cancel, closed) in [(false, false), (true, false), (false, true), (true, true)] {
                let mut input = crate::core::input::Input::from_video(&InputModel::Video {
                    path: source.path().to_owned(),
                    import_method: ImportMethod::FFMS2 {
                        index: None,
                        cache_path: None,
                    },
                    filters: Vec::new(),
                })
                .unwrap();
                let (tx, rx) = mpsc::channel();
                let receiver = (!closed).then_some(rx);
                clear();
                let error = measure(
                    &mut input,
                    output.path(),
                    &config,
                    &[(0, vec![0, 1]), (1, vec![4, 5])],
                    &tx,
                    &AtomicBool::new(cancel),
                )
                .expect_err("partial failure/cancel cannot request a plugin fallback");
                assert_eq!(error.is::<metrics::probe::Cancelled>(), cancel);
                if !cancel {
                    assert_native_failure(&error);
                }
                if let Some(receiver) = receiver {
                    let events = receiver.try_iter().collect::<Vec<_>>();
                    assert_eq!(events.len(), usize::from(!cancel));
                    if !cancel {
                        assert!(matches!(
                            &events[0],
                            SequenceStatus::Subprocess {
                                child: Status::Processing {
                                    completion: SequenceCompletion::FrameScore { frame: 0, .. },
                                    ..
                                },
                                ..
                            }
                        ));
                    }
                }
                assert_native_calls(cancel);
            }
        },
    );
}

#[test]
fn temporal_relay_primary_error_is_retained() {
    temporal_relay_result(true);
}

#[test]
fn temporal_relay_disconnect_is_advisory() {
    temporal_relay_result(false);
    let (tx, rx) = mpsc::channel();
    drop(rx);
    let (compare, relay) = crate::core::sequence::quality_check::spawn_compare_progress(tx);
    assert_eq!(
        TargetQuality::route_compare_scores(
            Some(Ok(vec![7.0])),
            Some(Engine::Fmetrics),
            &metric(None),
            compare,
            relay,
            |_| panic!("successful native compare retried plugin"),
            |_| Ok(())
        )
        .unwrap(),
        Some((vec![7.0], Engine::Fmetrics))
    );
    let (tx, rx) = mpsc::channel();
    let (compare, relay) = crate::core::sequence::quality_check::spawn_compare_progress(tx);
    let mut attempts = 0;
    assert_eq!(
        TargetQuality::route_compare_scores(
            Some(Err(anyhow::anyhow!("pre-score failure"))),
            None,
            &metric(None),
            compare,
            relay,
            |_| {
                attempts += 1;
                Ok(vec![8.0])
            },
            |_| Ok(())
        )
        .unwrap(),
        Some((vec![8.0], Engine::VapourSynth))
    );
    assert_eq!(attempts, 1);
    assert!(
        matches!(rx.try_recv(),Ok(SequenceStatus::Whole(Status::Completed { id })) if id == "Compare")
    );
    assert!(rx.try_iter().next().is_none());
    let (tx, rx) = mpsc::channel();
    drop(rx);
    let (compare, relay) = crate::core::sequence::quality_check::spawn_compare_progress(tx);
    let error = TargetQuality::route_compare_scores(
        Some(Ok(vec![7.0])),
        Some(Engine::Fmetrics),
        &metric(Some(true)),
        compare,
        relay,
        |_| panic!("independent native compare retried plugin"),
        |_| Ok(()),
    )
    .unwrap_err();
    assert!(error.is::<mpsc::SendError<SequenceStatus>>());
}

fn temporal_relay_result(failure: bool) {
    use crate::vapoursynth::{
        VapourSynthError,
        plugins::{PluginFunction, vship::cvvdp::CVVDP},
    };
    let (tx, rx) = mpsc::channel();
    drop(rx);
    let (compare, relay) = crate::core::sequence::quality_check::spawn_compare_progress(tx);
    let _ = compare.send(SequenceStatus::Whole(Status::Completed {
        id: "Compare".to_owned(),
    }));
    drop(compare);
    let primary = if failure {
        Err(anyhow::Error::new(CVVDP::new_error(
            "primary comparison failure".to_owned(),
        )))
    } else {
        Ok(vec![7.0])
    };
    let result = crate::core::sequence::quality_check::finish_temporal_compare(primary, relay);
    if failure {
        assert!(result.unwrap_err().is::<VapourSynthError>());
    } else {
        assert_eq!(
            result.expect("advisory receiver disconnection cannot fail scoring"),
            vec![7.0]
        );
    }
}

#[test]
fn temporal_finalization_waits_for_relay_and_retains_genuine_errors() {
    let (release_tx, release_rx) = mpsc::channel();
    let (joined_tx, joined_rx) = mpsc::channel();
    let relay = std::thread::spawn(move || -> anyhow::Result<()> {
        release_rx.recv().unwrap();
        Ok(())
    });
    let worker = std::thread::spawn(move || {
        let result = crate::core::sequence::quality_check::finish_temporal_compare(Ok(17), relay);
        joined_tx.send(result).unwrap();
    });
    assert!(matches!(
        joined_rx.recv_timeout(Duration::from_millis(50)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    release_tx.send(()).unwrap();
    assert_eq!(
        joined_rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap(),
        17
    );
    worker.join().unwrap();
    let relay = std::thread::spawn(|| Err(anyhow::anyhow!("genuine relay failure")));
    assert!(
        crate::core::sequence::quality_check::finish_temporal_compare(Ok(()), relay)
            .unwrap_err()
            .to_string()
            .contains("genuine relay failure")
    );
    let relay = std::thread::spawn(|| -> anyhow::Result<()> { panic!("genuine relay panic") });
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::core::sequence::quality_check::finish_temporal_compare::<()>(
                Err(anyhow::anyhow!("primary score error")),
                relay,
            )
        }))
        .is_err()
    );
    let (compare, _) = mpsc::channel();
    let relay = std::thread::spawn(|| -> anyhow::Result<()> { panic!("TQ relay panic") });
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            TargetQuality::route_compare_scores(
                Some(Err(metrics::probe::Cancelled.into())),
                None,
                &metric(None),
                compare,
                relay,
                |_| panic!("cancel retried plugin"),
                |_| Ok(()),
            )
        }))
        .is_err()
    );
}
