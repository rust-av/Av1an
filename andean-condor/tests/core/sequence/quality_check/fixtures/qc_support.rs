//! Public QC output, history admission and atomic replacement; native ABI is doubled.
#![cfg(unix)]
#![expect(clippy::unwrap_used, reason = "fixture assertions fail loudly")]

use super::condor;
use std::sync::{Arc, atomic::AtomicBool, mpsc};

use condor::core::sequence::{
    SequenceCompletion, SequenceStatus, Status, quality_check::QualityCheck,
};
use condor::models::sequence::target_quality::types::ProbeStatistic;

use super::current_quality_fixture;
use super::{calls, clear, metric, native_children, y4m_fixture};

pub(super) fn qc_output_case(test: &str, root_kind: &str) {
    native_children(test, &[("scene-f-qc", ""), ("scene-v-qc", "")], || {
        use condor::{
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
            current_quality_fixture(&mut job);
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
                    matches!(warning.downcast_ref::<condor::core::sequence::quality_check::QualityCheckError>(),Some(condor::core::sequence::quality_check::QualityCheckError::OutputMissing{path}) if path==output.path()),
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

pub(super) fn corrupt_batch(test: &str, started: Option<&dyn Fn(&anyhow::Error)>) {
    native_children(
        test,
        &[("scene-f-qc", "invalid-score-last"), ("scene-v-qc", "invalid-score-last")],
        || {
            use condor::{
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
                error.is::<condor::models::sequence::target_quality::types::MeasurementError>(),
                "{error:#}"
            );
            if let Some(started) = started {
                started(&error);
            }
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

pub(super) fn check_qc_history(test: &str, reset: bool, started: Option<&dyn Fn(&anyhow::Error)>) {
    native_children(
        test,
        &[
            ("scene-f-qc", ""),
            ("scene-v-qc", ""),
            ("scene-f-qc", "invalid-score-last"),
            ("scene-v-qc", "invalid-score-last"),
        ],
        || {
            use condor::{
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

            if !reset {
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
            } else {
                // An explicit metadata reset, unlike a count mismatch, permits fresh
                // measurement; a failed fresh batch still attaches no partial result.
                let mut condor = make_condor(ProbeStatistic::Mean, None);
                for scene in &mut condor.scenes {
                    scene.sequence_data.quality_check.quality = QualityPass::default();
                }
                let before = serde_json::to_vec(&condor.scenes).unwrap();
                let (tx, rx) = mpsc::channel();
                clear();
                let result = QualityCheck::default().execute(
                    &mut condor,
                    tx,
                    Arc::new(AtomicBool::new(false)),
                );
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
                    assert!(error.is::<MeasurementError>(), "{error:#}");
                    if let Some(started) = started {
                        started(&error);
                    }
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
                    let ((), warnings) =
                        result.expect("explicitly reset records can be remeasured");
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
            }
        },
    );
}
