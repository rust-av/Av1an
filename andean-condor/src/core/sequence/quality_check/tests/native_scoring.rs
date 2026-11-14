//! Real QC/TQ scoring boundaries and decoders; only the native ABI is doubled.
#![cfg(unix)]
#![expect(clippy::unwrap_used, reason = "fixture assertions fail loudly")]

use std::{
    sync::{Arc, atomic::AtomicBool, mpsc},
    time::Duration,
};

use crate::core::sequence::{
    SequenceCompletion, SequenceStatus, Status, quality_check::QualityCheck,
    target_quality::TargetQuality,
};
use crate::{
    metrics::{self, Engine},
    models::sequence::target_quality::types::ProbeStatistic,
};

use crate::core::sequence::tests::support::native_scoring::*;

fn qc_output_case(test: &str, root_kind: &str) {
    native_children(test, &[("scene-f-qc", ""), ("scene-v-qc", "")], || {
        use crate::{
            core::{Condor, input::Input, output::Output, sequence::Sequence},
            models::{
                input::{ImportMethod, Input as InputModel},
                scene::Scene,
                sequence::{
                    DefaultSequenceConfig, DefaultSequenceData, quality_check::QualityCheckConfig,
                    target_quality::types::QualityPass,
                },
            },
        };
        let source = y4m_fixture(&[32, 48, 64, 80]);
        let output = y4m_fixture(&[40, 56, 72, 88]);
        let root = y4m_fixture(&[1, 2, 3, 4]);
        let source_bytes = std::fs::read(source.path()).unwrap();
        match root_kind {
            "missing" => std::fs::remove_file(root.path()).unwrap(),
            "invalid" => std::fs::write(root.path(), b"not a video").unwrap(),
            "missing-override" => std::fs::remove_file(output.path()).unwrap(),
            "invalid-override" => {
                std::fs::write(output.path(), b"invalid effective output").unwrap();
            },
            _ => (),
        }
        let output_bytes = std::fs::read(output.path()).ok();
        let root_bytes = std::fs::read(root.path()).ok();
        let input = Input::from_video(&InputModel::Video {
            path: source.path().into(),
            import_method: ImportMethod::FFMS2 {
                index: None,
                cache_path: None,
            },
            filters: vec![],
        })
        .unwrap();
        let mut scenes = (0..2)
            .map(|index| Scene {
                start_frame: index * 2,
                end_frame: index * 2 + 2,
                sub_scenes: None,
                encoder: Default::default(),
                sequence_data: DefaultSequenceData::default(),
            })
            .collect::<Vec<_>>();
        // This is the fresh/unclaimed missing-output warning control, not a
        // previously certified result whose measured output disappeared.
        if root_kind != "missing-override" {
            scenes[0].sequence_data.quality_check.quality = QualityPass {
                scores: vec![17.0, 18.0],
                started_on: 100,
                completed_on: 200,
                ..Default::default()
            };
        }
        let saved = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let recorded = std::rc::Rc::clone(&saved);
        let mut config = DefaultSequenceConfig {
            quality_check: Some(QualityCheckConfig {
                metric: metric(None),
                ..Default::default()
            }),
            ..Default::default()
        };
        if root_kind != "fallback" {
            config.scene_concatenator.output = Some(output.path().into());
        }
        let root_path = if root_kind == "fallback" {
            output.path()
        } else {
            root.path()
        };
        let mut job = Condor::new(
            input,
            Output {
                path: root_path.into(),
                overwrite: false,
                tags: Default::default(),
                video_tags: Default::default(),
            },
            Default::default(),
            scenes,
            Some(config),
            Box::new(move |data| {
                recorded.borrow_mut().push(data);
                Ok(())
            }),
        );
        if root_kind != "missing-override" {
            crate::core::sequence::tests::support::current_quality_fixture(&mut job);
        }
        let history = serde_json::to_vec(&job.scenes[0]).unwrap();
        let before = serde_json::to_vec(&job.scenes).unwrap();
        let mut qc = QualityCheck::default();
        let (_, warnings) = qc.validate(&mut job).unwrap();
        let (tx, rx) = mpsc::channel();
        clear();
        let result = qc.execute(&mut job, tx, Arc::new(AtomicBool::new(false)));
        if root_kind == "missing-override" {
            for warning in warnings
                .iter()
                .chain(result.as_ref().expect("missing effective output is a warning").1.iter())
            {
                assert!(
                    matches!(warning.downcast_ref::<crate::core::sequence::quality_check::QualityCheckError>(),Some(crate::core::sequence::quality_check::QualityCheckError::OutputMissing{path}) if path==output.path()),
                    "effective output warning: {warning:#}"
                );
            }
            assert_eq!(warnings.len(), 1);
            assert_eq!(result.unwrap().1.len(), 1);
            assert_eq!(serde_json::to_vec(&job.scenes).unwrap(), before);
            assert!(
                saved.borrow().is_empty() && calls().is_empty() && rx.try_iter().next().is_none()
            );
        } else if root_kind == "invalid-override" {
            assert!(warnings.is_empty());
            let error =
                result.expect_err("invalid effective output must not fall back to the valid root");
            assert!(
                error.is::<av_decoders::DecoderError>(),
                "native decoder cause: {error:#}"
            );
            assert!(
                format!("{error:#}").contains(output.path().to_str().unwrap()),
                "effective path error context: {error:#}"
            );
            assert_eq!(serde_json::to_vec(&job.scenes).unwrap(), before);
            assert!(
                saved.borrow().is_empty() && calls().is_empty() && rx.try_iter().next().is_none()
            );
        } else {
            let (_, execution_warnings) = result.unwrap_or_else(|error| {
                panic!("QC must measure the effective output, root={root_kind}: {error:#}")
            });
            assert!(
                warnings.is_empty() && execution_warnings.is_empty(),
                "root={root_kind}, warnings={warnings:?}, execute={execution_warnings:?}"
            );
            // The ABI fixture marks raw Y for vship, limited-range RGB8 for fmetrics.
            // Neutral Y 64/72/80/88 becomes RGB8 56/65/75/84 (255/219 scaling).
            let expected = if std::env::var("TEMPORAL_FIXTURE_SCOPE").unwrap().contains("-f-") {
                [251.0, 253837.0]
            } else {
                [280.0, 283144.0]
            };
            assert_eq!(
                job.scenes[1].sequence_data.quality_check.quality.scores, expected,
                "actual decoded override samples, not the distinct root"
            );
            assert_eq!(saved.borrow().len(), 1);
            assert_eq!(
                saved.borrow()[0].scenes[1].sequence_data.quality_check.quality.scores,
                expected
            );
            assert_eq!(saved.borrow()[0].output.path, root_path);
            assert_eq!(
                saved.borrow()[0].sequence_config.scene_concatenator.output,
                job.sequence_config.scene_concatenator.output
            );
            let prefix = if std::env::var("TEMPORAL_FIXTURE_SCOPE").unwrap().contains("-f-") {
                "f"
            } else {
                "v"
            };
            assert_eq!(
                calls(),
                format!("{prefix}c+\n{prefix}p\n{prefix}p\n{prefix}c-\n")
            );
        }
        assert_eq!(
            serde_json::to_vec(&job.scenes[0]).unwrap(),
            history,
            "retained valid scene history"
        );
        assert_eq!(
            std::fs::read(source.path()).unwrap(),
            source_bytes,
            "reference bytes unchanged"
        );
        assert_eq!(std::fs::read(output.path()).ok(), output_bytes);
        assert_eq!(std::fs::read(root.path()).ok(), root_bytes);
    });
}

#[test]
fn qc_override_scores_distinct_native_output() {
    qc_output_case(
        "core::sequence::quality_check::tests::native_scoring::qc_override_scores_distinct_native_output",
        "distinct",
    );
}

#[test]
fn qc_override_ignores_missing_root() {
    qc_output_case(
        "core::sequence::quality_check::tests::native_scoring::qc_override_ignores_missing_root",
        "missing",
    );
}

#[test]
fn qc_override_ignores_invalid_root() {
    qc_output_case(
        "core::sequence::quality_check::tests::native_scoring::qc_override_ignores_invalid_root",
        "invalid",
    );
}

#[test]
fn qc_missing_effective_output_warns_without_scoring_root() {
    qc_output_case(
        "core::sequence::quality_check::tests::native_scoring::qc_missing_effective_output_warns_without_scoring_root",
        "missing-override",
    );
}

#[test]
fn qc_without_override_scores_fallback() {
    qc_output_case(
        "core::sequence::quality_check::tests::native_scoring::qc_without_override_scores_fallback",
        "fallback",
    );
}

#[test]
fn qc_invalid_effective_output_reports_actual_error_without_save() {
    qc_output_case(
        "core::sequence::quality_check::tests::native_scoring::qc_invalid_effective_output_reports_actual_error_without_save",
        "invalid-override",
    );
}

#[test]
fn qc_execute_corrupt_last_scene_does_not_attach_save_or_complete() {
    native_children(
        "core::sequence::quality_check::tests::native_scoring::qc_execute_corrupt_last_scene_does_not_attach_save_or_complete",
        &[("scene-f-qc", "invalid-score-last"), ("scene-v-qc", "invalid-score-last")],
        || {
            use crate::{
                core::{Condor, input::Input, output::Output, sequence::Sequence},
                models::{
                    input::{ImportMethod, Input as InputModel},
                    scene::Scene,
                    sequence::{
                        DefaultSequenceConfig, DefaultSequenceData,
                        quality_check::QualityCheckConfig,
                    },
                },
            };
            let source = y4m_fixture(&[32, 48, 64, 80, 96, 112]);
            let output = y4m_fixture(&[40, 56, 72, 88, 104, 120]);
            let input = Input::from_video(&InputModel::Video {
                path: source.path().to_owned(),
                import_method: ImportMethod::FFMS2 {
                    index: None,
                    cache_path: None,
                },
                filters: vec![],
            })
            .unwrap();
            let scenes = [(0, 2), (4, 6)]
                .into_iter()
                .map(|(start_frame, end_frame)| Scene {
                    start_frame,
                    end_frame,
                    sub_scenes: None,
                    encoder: Default::default(),
                    sequence_data: DefaultSequenceData::default(),
                })
                .collect::<Vec<_>>();
            let before = serde_json::to_value(&scenes).unwrap();
            let saves = std::rc::Rc::new(std::cell::Cell::new(0));
            let saved = std::rc::Rc::clone(&saves);
            let mut condor = Condor::new(
                input,
                Output {
                    path: output.path().to_owned(),
                    overwrite: false,
                    tags: Default::default(),
                    video_tags: Default::default(),
                },
                Default::default(),
                scenes,
                Some(DefaultSequenceConfig {
                    quality_check: Some(QualityCheckConfig {
                        metric: metric(None),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                Box::new(move |_| {
                    saved.set(saved.get() + 1);
                    Ok(())
                }),
            );
            let (tx, rx) = mpsc::channel();
            clear();
            let error = QualityCheck::default()
                .execute(&mut condor, tx, Arc::new(AtomicBool::new(false)))
                .expect_err("invalid last scene");
            assert!(
                error.is::<crate::models::sequence::target_quality::types::MeasurementError>(),
                "{error:#}"
            );
            assert!(
                error.is::<metrics::SceneScoringStarted>(),
                "earlier valid frames were reported"
            );
            assert!(format!("{error:#}").contains("scene 1"));
            assert_eq!(serde_json::to_value(&condor.scenes).unwrap(), before);
            assert_eq!(saves.get(), 0);
            let events = rx.try_iter().collect::<Vec<_>>();
            assert_eq!(
                events.len(),
                3,
                "three valid frame callbacks, no invalid frame or completion"
            );
            assert!(events.iter().all(|event| matches!(event, SequenceStatus::Subprocess { child: Status::Processing { completion: SequenceCompletion::FrameScore { score, .. }, .. }, .. } if score.is_finite())));
            let p = if std::env::var("TEMPORAL_FIXTURE_SCOPE").unwrap().contains("-f-") {
                "f"
            } else {
                "v"
            };
            assert_eq!(
                calls(),
                format!("{p}c+\n{p}p\n{p}p\n{p}c-\n{p}c+\n{p}p\n{p}p\n{p}c-\n")
            );
        },
    );
}

#[test]
fn qc_refuses_tampered_history_until_explicit_reset() {
    native_children(
        "core::sequence::quality_check::tests::native_scoring::qc_refuses_tampered_history_until_explicit_reset",
        &[
            ("scene-f-qc", ""),
            ("scene-v-qc", ""),
            ("scene-f-qc", "invalid-score-last"),
            ("scene-v-qc", "invalid-score-last"),
        ],
        || {
            use crate::{
                core::{Condor, input::Input, output::Output, sequence::Sequence},
                models::{
                    input::{ImportMethod, Input as InputModel},
                    scene::Scene,
                    sequence::{
                        DefaultSequenceConfig, DefaultSequenceData,
                        quality_check::QualityCheckConfig,
                        target_quality::types::{MeasurementError, QualityPass},
                    },
                },
            };
            let source = y4m_fixture(&[32, 48, 64, 80, 96, 112]);
            let output = y4m_fixture(&[40, 56, 72, 88, 104, 120]);
            let saved = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
            let make_condor = |statistic, disabled| {
                let input = Input::from_video(&InputModel::Video {
                    path: source.path().to_owned(),
                    import_method: ImportMethod::FFMS2 {
                        index: None,
                        cache_path: None,
                    },
                    filters: vec![],
                })
                .unwrap();
                let scenes = [(0, 2), (4, 6)]
                    .into_iter()
                    .map(|(start_frame, end_frame)| Scene {
                        start_frame,
                        end_frame,
                        sub_scenes: None,
                        encoder: Default::default(),
                        sequence_data: DefaultSequenceData::default(),
                    })
                    .collect();
                let recorded = std::rc::Rc::clone(&saved);
                let mut job = Condor::new(
                    input,
                    Output {
                        path: output.path().to_owned(),
                        overwrite: false,
                        tags: Default::default(),
                        video_tags: Default::default(),
                    },
                    Default::default(),
                    scenes,
                    Some(DefaultSequenceConfig {
                        quality_check: Some(QualityCheckConfig {
                            metric: metric(disabled),
                            statistic,
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
                    Box::new(move |data| {
                        recorded.borrow_mut().push(serde_json::to_vec(&data.scenes)?);
                        Ok(())
                    }),
                );
                let failure = std::env::var("TEMPORAL_FIXTURE_FAIL").unwrap();
                // SAFETY: native_children runs this body in a dedicated child;
                // measurement workers have joined before restoring the setting.
                unsafe { std::env::set_var("TEMPORAL_FIXTURE_FAIL", "") };
                let (tx, _rx) = mpsc::channel();
                QualityCheck::default()
                    .execute(&mut job, tx, Arc::new(AtomicBool::new(false)))
                    .expect("real valid QC publication before external tampering");
                // SAFETY: the dedicated child's measurement workers have joined.
                unsafe { std::env::set_var("TEMPORAL_FIXTURE_FAIL", failure) };
                saved.borrow_mut().clear();
                job
            };

            for scores in [vec![1.0, f64::NAN], vec![1.0, -1.0], vec![10.0], vec![20.0; 3]] {
                let mut condor = make_condor(ProbeStatistic::Harmonic, Some(true));
                condor.scenes[0].sequence_data.quality_check.quality.scores = scores;
                let before = serde_json::to_vec(&condor.scenes).unwrap();
                let (tx, rx) = mpsc::channel();
                clear();
                let error = QualityCheck::default()
                    .execute(&mut condor, tx, Arc::new(AtomicBool::new(false)))
                    .expect_err("corrupt certified history requires an explicit reset");
                assert!(error.is::<MeasurementError>(), "{error:#}");
                assert!(format!("{error:#}").contains("recorded Quality Check history"));
                assert!(
                    calls().is_empty(),
                    "no native scorer work for corrupt matching history"
                );
                assert!(saved.borrow().is_empty() && rx.try_iter().next().is_none());
                assert_eq!(serde_json::to_vec(&condor.scenes).unwrap(), before);
            }

            // An explicit metadata reset, unlike a count mismatch, permits fresh
            // measurement; a failed fresh batch still attaches no partial result.
            let mut condor = make_condor(ProbeStatistic::Mean, None);
            for scene in &mut condor.scenes {
                scene.sequence_data.quality_check.quality = QualityPass::default();
            }
            let before = serde_json::to_vec(&condor.scenes).unwrap();
            let (tx, rx) = mpsc::channel();
            clear();
            let result =
                QualityCheck::default().execute(&mut condor, tx, Arc::new(AtomicBool::new(false)));
            let reported = rx
                .try_iter()
                .map(|event| match event {
                    SequenceStatus::Subprocess {
                        child:
                            Status::Processing {
                                completion: SequenceCompletion::FrameScore { frame, score },
                                ..
                            },
                        ..
                    } => {
                        assert!(score.is_finite());
                        (frame, score)
                    },
                    other => panic!("unexpected completion/invalid event: {other:?}"),
                })
                .collect::<Vec<_>>();
            let failed = !std::env::var("TEMPORAL_FIXTURE_FAIL").unwrap().is_empty();
            if failed {
                let error = result.expect_err("invalid replacement remains terminal");
                assert!(
                    error.is::<MeasurementError>() && error.is::<metrics::SceneScoringStarted>(),
                    "{error:#}"
                );
                assert_eq!(
                    serde_json::to_vec(&condor.scenes).unwrap(),
                    before,
                    "retain old records until the entire replacement is admitted"
                );
                assert!(
                    saved.borrow().is_empty(),
                    "failed replacement must not save"
                );
            } else {
                let ((), warnings) = result.expect("explicitly reset records can be remeasured");
                assert!(warnings.is_empty());
                // The ABI double sees RGB on Fmetrics and YUV on Vship; this test
                // checks selection/attachment, not pixel conversion or metric math.
                for (scene, values) in condor.scenes.iter().zip(reported.as_chunks::<2>().0) {
                    let pass = &scene.sequence_data.quality_check.quality;
                    assert_eq!(
                        pass.scores,
                        values.iter().map(|(_, score)| *score).collect::<Vec<_>>(),
                        "attach each scene's fresh native results in selection order"
                    );
                    assert!(pass.started_on > 200 && pass.completed_on >= pass.started_on);
                }
                assert_eq!(
                    *saved.borrow(),
                    [serde_json::to_vec(&condor.scenes).unwrap()],
                    "one save of the admitted replacement, not stale records"
                );
            }
            let frames = reported.iter().map(|(frame, _)| *frame).collect::<Vec<_>>();
            assert_eq!(
                frames,
                if failed {
                    vec![0, 1, 4]
                } else {
                    vec![0, 1, 4, 5]
                }
            );
            let p = if std::env::var("TEMPORAL_FIXTURE_SCOPE").unwrap().contains("-f-") {
                "f"
            } else {
                "v"
            };
            assert_eq!(
                calls(),
                format!("{p}c+\n{p}p\n{p}p\n{p}c-\n{p}c+\n{p}p\n{p}p\n{p}c-\n"),
                "all selected pairs attempted and both scene sessions released"
            );
        },
    );
}

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
    // A pre-progress failure still requests the existing fallback, exactly once.
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
        matches!(rx.try_recv(), Ok(SequenceStatus::Whole(Status::Completed { id })) if id == "Compare")
    );
    assert!(rx.try_iter().next().is_none());
    // Independent-frame CVVDP keeps its previous fatal relay-send policy.
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
    // A relay panic is observable even when scoring failed: finalization must
    // join before returning the primary error, not merely on the success path.
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
