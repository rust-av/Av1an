//! Real QC/TQ scoring boundaries and decoders; only the native ABI is doubled.
#![cfg(unix)]
#![expect(clippy::unwrap_used, reason = "fixture assertions fail loudly")]

use std::{
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use super::{
    SequenceCompletion, SequenceStatus, Status, quality_check::QualityCheck,
    target_quality::TargetQuality,
};
use crate::{
    metrics::{self, Engine, OutputIndexing},
    models::sequence::target_quality::types::{ProbeStatistic, QualityMetric},
};

fn metric(disabled: Option<bool>) -> QualityMetric {
    QualityMetric::CVVDP {
        target_range: (0.0, 1.0),
        resolution: None,
        display_model: None,
        resize_to_display: Some(false),
        disable_temporal: disabled,
        gpu_id: Some(0),
    }
}

fn c06_qc_output_case(test: &str, root_kind: &str) {
    native_children(test, &[("m27a-f-qc", ""), ("m27a-v-qc", "")], || {
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
            super::c14_tests::current_quality_fixture(&mut job);
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
                    matches!(warning.downcast_ref::<super::quality_check::QualityCheckError>(),Some(super::quality_check::QualityCheckError::OutputMissing{path}) if path==output.path()),
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
            let expected = if std::env::var("M15_SCOPE").unwrap().contains("-f-") {
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
            let prefix = if std::env::var("M15_SCOPE").unwrap().contains("-f-") {
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
fn c06_qc_override_scores_distinct_native_output() {
    c06_qc_output_case(
        "core::sequence::m27a_tests::c06_qc_override_scores_distinct_native_output",
        "distinct",
    );
}

#[test]
fn c06_qc_override_ignores_missing_root() {
    c06_qc_output_case(
        "core::sequence::m27a_tests::c06_qc_override_ignores_missing_root",
        "missing",
    );
}

#[test]
fn c06_qc_override_ignores_invalid_root() {
    c06_qc_output_case(
        "core::sequence::m27a_tests::c06_qc_override_ignores_invalid_root",
        "invalid",
    );
}

#[test]
fn c06_qc_missing_effective_output_warns_without_scoring_root() {
    c06_qc_output_case(
        "core::sequence::m27a_tests::c06_qc_missing_effective_output_warns_without_scoring_root",
        "missing-override",
    );
}

#[test]
fn c06_qc_without_override_scores_fallback() {
    c06_qc_output_case(
        "core::sequence::m27a_tests::c06_qc_without_override_scores_fallback",
        "fallback",
    );
}

#[test]
fn c06_qc_invalid_effective_output_reports_actual_error_without_save() {
    c06_qc_output_case(
        "core::sequence::m27a_tests::c06_qc_invalid_effective_output_reports_actual_error_without_save",
        "invalid-override",
    );
}

fn decoder(samples: &[u8]) -> av_decoders::Decoder {
    let clips = samples.iter().map(|sample| format!("c.std.BlankClip(width=32,height=32,length=1,format=vs.YUV420P8,color=[{sample},128,128])")).collect::<Vec<_>>().join(",");
    av_decoders::Decoder::from_script(&format!("import vapoursynth as vs\nc=vs.core\nc.std.Splice([{clips}]).std.SetFrameProps(_Matrix=1,_Transfer=13,_Primaries=1,_Range=1).set_output()"), Default::default()).unwrap()
}

fn calls() -> String {
    std::fs::read_to_string(std::env::var_os("M15_LOG").unwrap()).unwrap_or_default()
}
fn clear() {
    std::fs::write(std::env::var_os("M15_LOG").unwrap(), "").unwrap();
}

#[test]
fn q05_native_invalid_values_are_terminal_before_callbacks() {
    native_children(
        "core::sequence::m27a_tests::q05_native_invalid_values_are_terminal_before_callbacks",
        &[
            ("m27a-f-tq", "q05-nan"),
            ("m27a-f-tq", "q05-inf"),
            ("m27a-f-tq", "q05-neginf"),
            ("m27a-v-tq", "q05-nan"),
            ("m27a-v-tq", "q05-inf"),
            ("m27a-v-tq", "q05-neginf"),
        ],
        || {
            let engine = if std::env::var("M15_SCOPE").unwrap().contains("-f-") {
                Engine::Fmetrics
            } else {
                Engine::Vship
            };
            let (tx, rx) = mpsc::channel();
            let (compare, relay) = super::quality_check::spawn_compare_progress(tx);
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
                let (compare, relay) = super::quality_check::spawn_compare_progress(tx);
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
                    error.is::<crate::models::sequence::target_quality::types::MeasurementError>(),
                    "tag native-rejected corruption: {error:#}"
                );
            }
        },
    );
}

#[test]
fn q05_qc_execute_corrupt_last_scene_does_not_attach_save_or_complete() {
    native_children(
        "core::sequence::m27a_tests::q05_qc_execute_corrupt_last_scene_does_not_attach_save_or_complete",
        &[("m27a-f-qc", "q05-last"), ("m27a-v-qc", "q05-last")],
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
            let p = if std::env::var("M15_SCOPE").unwrap().contains("-f-") {
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
fn q05_m1_qc_refuses_tampered_history_until_explicit_reset() {
    native_children(
        "core::sequence::m27a_tests::q05_m1_qc_refuses_tampered_history_until_explicit_reset",
        &[
            ("m27a-f-qc", ""),
            ("m27a-v-qc", ""),
            ("m27a-f-qc", "q05-last"),
            ("m27a-v-qc", "q05-last"),
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
                let failure = std::env::var("M15_FAIL").unwrap();
                // SAFETY: native_children runs this body in a dedicated child;
                // measurement workers have joined before restoring the setting.
                unsafe { std::env::set_var("M15_FAIL", "") };
                let (tx, _rx) = mpsc::channel();
                QualityCheck::default()
                    .execute(&mut job, tx, Arc::new(AtomicBool::new(false)))
                    .expect("real valid QC publication before external tampering");
                // SAFETY: the dedicated child's measurement workers have joined.
                unsafe { std::env::set_var("M15_FAIL", failure) };
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
            let failed = !std::env::var("M15_FAIL").unwrap().is_empty();
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
            let p = if std::env::var("M15_SCOPE").unwrap().contains("-f-") {
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
fn m27a_native_scene_consumers() {
    native_children(
        "core::sequence::m27a_tests::m27a_native_scene_consumers",
        &[
            ("m27a-f-qc", ""),
            ("m27a-v-qc", ""),
            ("m27a-f-tq", ""),
            ("m27a-v-tq", ""),
            ("m27a-f-qc", "fp"),
            ("m27a-v-qc", "vp"),
            ("m27a-f-qc", "fp2"),
            ("m27a-v-qc", "vp2"),
        ],
        || {
            let engine = if std::env::var("M15_SCOPE").unwrap().contains("-f-") {
                Engine::Fmetrics
            } else {
                Engine::Vship
            };
            let failure = std::env::var("M15_FAIL").unwrap();
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
fn m27_native_scene_final_reduction() {
    native_children(
        "core::sequence::m27a_tests::m27_native_scene_final_reduction",
        &[("m27a-f-qc", ""), ("m27a-v-qc", ""), ("m27a-f-tq", ""), ("m27a-v-tq", "")],
        || {
            let engine = if std::env::var("M15_SCOPE").unwrap().contains("-f-") {
                Engine::Fmetrics
            } else {
                Engine::Vship
            };
            native_consumers(engine);
        },
    );
}

fn native_children(test: &str, scopes: &[(&str, &str)], child_test: impl FnOnce()) {
    if std::env::var_os("M27A_CHILD").is_some() {
        child_test();
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let library = dir.path().join(if cfg!(target_os = "macos") {
        "libfmetrics.dylib"
    } else {
        "libfmetrics.so"
    });
    let output = Command::new("cc")
        .args(["-std=c11", "-Wall", "-Wextra", "-Werror", "-shared", "-fPIC", "-o"])
        .arg(&library)
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/temporal.c"
        ))
        .output()
        .expect("native boundary requires cc");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for (engine, name) in [
        (
            "f",
            if cfg!(target_os = "macos") {
                "libfmetrics.dylib"
            } else {
                "libfmetrics.so"
            },
        ),
        (
            "v",
            if cfg!(target_os = "macos") {
                "libvship.dylib"
            } else {
                "libvship.so"
            },
        ),
    ] {
        let path = dir.path().join(engine);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::copy(&library, path.join(name)).unwrap();
    }
    // A valid, device-less fixture prevents the CPU caller test from finding
    // and selecting a real system GPU library after a missing override path.
    let no_devices = dir.path().join("no_devices.c");
    std::fs::write(&no_devices, "#undef Vship_GetDeviceCount\n#define VSHIP_STATIC\n#include \"VshipAPI.h\"\nVship_Exception Vship_GetDeviceCount(int *n) { *n = 0; return Vship_NoError; }\n").unwrap();
    let output = Command::new("cc")
        .args([
            "-std=c11",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-shared",
            "-fPIC",
            "-DVship_GetDeviceCount=M27a_unused_device_count",
            "-I",
        ])
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../av-metrics-vship/tests/headers"
        ))
        .arg("-o")
        .arg(dir.path().join("f").join(if cfg!(target_os = "macos") {
            "libvship.dylib"
        } else {
            "libvship.so"
        }))
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/temporal.c"
        ))
        .arg(&no_devices)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut errors = Vec::new();
    for &(scope, failure) in scopes {
        let library_path = dir.path().join(if scope.contains("-f-") { "f" } else { "v" });
        // File-backed output cannot fill a pipe while the watchdog waits.
        let stdout = dir.path().join("stdout");
        let stderr = dir.path().join("stderr");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", test, "--nocapture"])
            .env("M27A_CHILD", "1")
            .env("M15_SCOPE", scope)
            .env("M15_FAIL", failure)
            .env("M16_FPS_AUDIT", "")
            .env("M15_LOG", dir.path().join("calls"))
            .env("FMETRICS_LIB_PATH", &library_path)
            .env("VSHIP_PLUGIN_PATH", &library_path)
            .stdout(std::fs::File::create(&stdout).unwrap())
            .stderr(std::fs::File::create(&stderr).unwrap())
            .spawn()
            .unwrap();
        let start = Instant::now();
        while child.try_wait().unwrap().is_none() {
            if start.elapsed() > Duration::from_secs(120) {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("native caller child timed out: {scope}/{failure}");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let status = child.wait().unwrap();
        let stdout = std::fs::read_to_string(stdout).unwrap();
        let stderr = std::fs::read_to_string(stderr).unwrap();
        if !status.success() || !stdout.contains("1 passed; 0 failed") {
            errors.push(format!("{scope}/{failure}: {stdout}\n{stderr}"));
        }
    }
    assert!(errors.is_empty(), "{}", errors.join("\n"));
}

pub(super) fn qc_native_caller(
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
    native_children(test, &[("m27a-f-qc", "fp2"), ("m27a-v-qc", "vp2")], || {
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
    });
}

fn y4m_fixture(samples: &[u8]) -> tempfile::NamedTempFile {
    let log = std::path::PathBuf::from(std::env::var_os("M15_LOG").unwrap());
    let file = tempfile::Builder::new()
        .suffix(".y4m")
        .tempfile_in(log.parent().unwrap())
        .unwrap();
    let mut data = b"YUV4MPEG2 W32 H32 F24:1 Ip A1:1 C420jpeg\n".to_vec();
    for sample in samples {
        data.extend_from_slice(b"FRAME\n");
        data.extend_from_slice(&[*sample; 1024]);
        data.extend_from_slice(&[128; 512]);
    }
    std::fs::write(file.path(), data).unwrap();
    file
}

fn assert_native_failure(error: &anyhow::Error) {
    assert!(error.is::<metrics::SceneScoringStarted>(), "{error:#}");
    if std::env::var("M15_SCOPE").unwrap().contains("-f-") {
        assert!(
            matches!(
                error.downcast_ref::<av_metrics_fmetrics::FmetricsError>(),
                Some(av_metrics_fmetrics::FmetricsError::CallFailed { code: 6, .. })
            ),
            "{error:#}"
        );
    } else {
        assert!(
            matches!(
                error.downcast_ref::<av_metrics_vship::VshipError>(),
                Some(av_metrics_vship::VshipError::CallFailed { status: 1, .. })
            ),
            "{error:#}"
        );
    }
}

fn assert_native_calls(cancel: bool) {
    let prefix = if std::env::var("M15_SCOPE").unwrap().contains("-f-") {
        "f"
    } else {
        "v"
    };
    assert_eq!(
        calls(),
        if cancel {
            String::new()
        } else {
            format!("{prefix}c+\n{prefix}p\n{prefix}p\n{prefix}c-\n")
        }
    );
}

#[test]
fn m27a_tq_caller_failure_after_progress_does_not_fallback() {
    tq_caller(
        "core::sequence::m27a_tests::m27a_tq_caller_failure_after_progress_does_not_fallback",
        false,
    );
}

#[test]
fn m27a_tq_caller_cancel_after_progress_is_incomplete() {
    tq_caller(
        "core::sequence::m27a_tests::m27a_tq_caller_cancel_after_progress_is_incomplete",
        true,
    );
}

fn tq_caller(test: &str, cancel: bool) {
    native_children(test, &[("m27a-f-tq", "fp2"), ("m27a-v-tq", "vp2")], || {
        let engine = if std::env::var("M15_SCOPE").unwrap().contains("-f-") {
            Engine::Fmetrics
        } else {
            Engine::Vship
        };
        for closed in [false, true] {
            let (progress, receiver) = mpsc::channel();
            let receiver = (!closed).then_some(receiver);
            let (compare, relay) = super::quality_check::spawn_compare_progress(progress);
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
    });
}

#[test]
fn m27a_temporal_relay_primary_error_is_retained() {
    temporal_relay_result(true);
}

#[test]
fn m27a_temporal_relay_disconnect_is_advisory() {
    temporal_relay_result(false);
    let (tx, rx) = mpsc::channel();
    drop(rx);
    let (compare, relay) = super::quality_check::spawn_compare_progress(tx);
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
    let (compare, relay) = super::quality_check::spawn_compare_progress(tx);
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
    let (compare, relay) = super::quality_check::spawn_compare_progress(tx);
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
    let (compare, relay) = super::quality_check::spawn_compare_progress(tx);
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
    let result = super::quality_check::finish_temporal_compare(primary, relay);
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
fn m27a_tq_plugin_cancel_after_relay_cleanup_is_incomplete() {
    for closed in [false, true] {
        let (tx, rx) = mpsc::channel();
        let receiver = (!closed).then_some(rx);
        let (compare, relay) = super::quality_check::spawn_compare_progress(tx);
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

#[test]
fn m27a_temporal_finalization_waits_for_relay_and_retains_genuine_errors() {
    let (release_tx, release_rx) = mpsc::channel();
    let (joined_tx, joined_rx) = mpsc::channel();
    let relay = std::thread::spawn(move || -> anyhow::Result<()> {
        release_rx.recv().unwrap();
        Ok(())
    });
    let worker = std::thread::spawn(move || {
        let result = super::quality_check::finish_temporal_compare(Ok(17), relay);
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
        super::quality_check::finish_temporal_compare(Ok(()), relay)
            .unwrap_err()
            .to_string()
            .contains("genuine relay failure")
    );
    // A relay panic is observable even when scoring failed: finalization must
    // join before returning the primary error, not merely on the success path.
    let relay = std::thread::spawn(|| -> anyhow::Result<()> { panic!("genuine relay panic") });
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            super::quality_check::finish_temporal_compare::<()>(
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

fn native_failure(engine: Engine, failure: &str) {
    clear();
    let prefix = if engine == Engine::Fmetrics { "f" } else { "v" };
    let mut a = decoder(&[1, 2, 3, 4, 5, 6]);
    let mut b = decoder(&[10, 20, 50, 60]);
    let mut positions = Vec::new();
    let error = metrics::score_scene_frames(
        engine,
        &mut a,
        &mut b,
        &metric(None),
        &[&[0, 1], &[4, 5]],
        OutputIndexing::Compacted,
        None,
        |position, _| positions.push(position),
    )
    .unwrap_err();
    assert!(!error.is::<metrics::probe::Cancelled>());
    assert_eq!(
        error.is::<metrics::SceneScoringStarted>(),
        failure.ends_with('2')
    );
    assert_eq!(
        positions,
        if failure.ends_with('2') {
            vec![0]
        } else {
            vec![]
        }
    );
    assert_eq!(
        calls(),
        if failure.ends_with('2') {
            format!("{prefix}c+\n{prefix}p\n{prefix}p\n{prefix}c-\n")
        } else {
            format!("{prefix}c+\n{prefix}p\n{prefix}c-\n")
        }
    );
}

fn sequential_decoders(engine: Engine) {
    let reference = y4m_fixture(&[32, 48, 64, 80, 96, 112]);
    let aligned = y4m_fixture(&[40, 56, 72, 88, 104, 120]);
    let compacted = y4m_fixture(&[40, 56, 104, 120]);
    let second = y4m_fixture(&[104, 120]);
    for indexing in [OutputIndexing::Aligned, OutputIndexing::Compacted] {
        let output = if indexing == OutputIndexing::Aligned {
            aligned.path()
        } else {
            compacted.path()
        };
        let mut a = av_decoders::Decoder::from_file(reference.path()).unwrap();
        let mut b = av_decoders::Decoder::from_file(output).unwrap();
        assert!(
            a.seek_to_frame(0).is_err(),
            "fixture must exercise a sequential reader"
        );
        clear();
        let values = metrics::score_scene_frames(
            engine,
            &mut a,
            &mut b,
            &metric(None),
            &[&[0, 1], &[4, 5]],
            indexing,
            None,
            |_, _| {},
        )
        .expect("reuse sequential readers across fresh scorers");
        let prefix = if engine == Engine::Fmetrics { "f" } else { "v" };
        assert_eq!(
            calls().lines().filter(|s| *s == format!("{prefix}c+")).count(),
            2
        );
        let mut a = av_decoders::Decoder::from_file(reference.path()).unwrap();
        let mut b = av_decoders::Decoder::from_file(second.path()).unwrap();
        let standalone = if engine == Engine::Fmetrics {
            metrics::fmetrics::score_probed_frames(
                &mut a,
                &mut b,
                &metric(None),
                &[4, 5],
                OutputIndexing::Compacted,
                None,
                |_, _| {},
            )
        } else {
            metrics::vship::score_probed_frames(
                &mut a,
                &mut b,
                &metric(None),
                &[4, 5],
                OutputIndexing::Compacted,
                None,
                |_, _| {},
            )
        }
        .unwrap();
        assert_eq!(&values[2..], standalone);
    }
}

fn native_consumers(engine: Engine) {
    let prefix = if engine == Engine::Fmetrics { "f" } else { "v" };
    for second in [[2, 3], [4, 5]] {
        let scenes: &[&[usize]] = &[&[0, 1], &second];
        let selected = [0, 1, second[0], second[1]];
        for qc in [true, false] {
            if qc != std::env::var("M15_SCOPE").unwrap().ends_with("-qc") {
                continue;
            }
            let distorted = if qc {
                vec![10, 20, 30, 40, 50, 60]
            } else {
                vec![10, 20, second[0] as u8 * 10 + 10, second[1] as u8 * 10 + 10]
            };
            for disabled in [None, Some(false)] {
                let mut a = decoder(&[1, 2, 3, 4, 5, 6]);
                let mut b = decoder(&distorted);
                let cancel = AtomicBool::new(false);
                let (tx, rx) = mpsc::channel();
                clear();
                let values = if qc {
                    QualityCheck::score_native_scenes(
                        engine,
                        &mut a,
                        &mut b,
                        &metric(disabled),
                        scenes,
                        &tx,
                        &cancel,
                    )
                } else {
                    TargetQuality::score_native_scenes(
                        engine,
                        &mut a,
                        &mut b,
                        &metric(disabled),
                        scenes,
                        &tx,
                        &cancel,
                    )
                }
                .unwrap();
                let expected = if second == [2, 3] {
                    [31.0, 31372.0, 93.0, 94054.0]
                } else {
                    [31.0, 31372.0, 155.0, 156736.0]
                };
                assert_eq!(
                    values, expected,
                    "each scene is an independent prefix vector; QC={qc} {second:?}"
                );
                let log = calls();
                assert_eq!(
                    log.lines().filter(|s| *s == format!("{prefix}c+")).count(),
                    2,
                    "{log}"
                );
                assert_eq!(
                    log.lines().filter(|s| *s == format!("{prefix}c-")).count(),
                    2,
                    "{log}"
                );
                // Independently submitted scene sessions have identical prefixes.
                let mut standalone = Vec::new();
                for scene in scenes {
                    let mut reference = decoder(&[1, 2, 3, 4, 5, 6]);
                    let samples: Vec<_> = scene.iter().map(|i| (*i as u8 + 1) * 10).collect();
                    let mut output = decoder(&samples);
                    let values = if engine == Engine::Fmetrics {
                        metrics::fmetrics::score_probed_frames(
                            &mut reference,
                            &mut output,
                            &metric(disabled),
                            scene,
                            OutputIndexing::Compacted,
                            None,
                            |_, _| {},
                        )
                    } else {
                        metrics::vship::score_probed_frames(
                            &mut reference,
                            &mut output,
                            &metric(disabled),
                            scene,
                            OutputIndexing::Compacted,
                            None,
                            |_, _| {},
                        )
                    }
                    .unwrap();
                    standalone.extend(values);
                }
                assert_eq!(values, standalone);
                for (scene_index, scores) in values.as_chunks::<2>().0.iter().enumerate() {
                    let final_score = expected[scene_index * 2 + 1];
                    let mut model = metric(disabled);
                    *model.target_range_mut() = (final_score - 0.5, final_score + 0.5);
                    assert_eq!(
                        model.calculate_score(ProbeStatistic::Mean, scores).expect("score"),
                        final_score
                    );
                    assert!(model.score_within_target(
                        model.calculate_score(ProbeStatistic::Mean, scores).expect("score")
                    ));
                    assert!(!model.score_within_target(
                        ProbeStatistic::Mean.calculate(scores).expect("mean")
                    ));
                }
                let progress: Vec<_> = rx.try_iter().collect();
                if qc {
                    let events: Vec<_> = progress
                        .into_iter()
                        .map(|event| match event {
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
                            } => (completed, total, frame, score),
                            other => panic!("unexpected QC progress {other:?}"),
                        })
                        .collect();
                    assert_eq!(
                        events,
                        (0..4)
                            .map(|i| ((i + 1) as f64, 4.0, selected[i] as u64, expected[i]))
                            .collect::<Vec<_>>()
                    );
                } else {
                    let counts: Vec<_> = progress
                        .into_iter()
                        .map(|event| match event {
                            SequenceStatus::Whole(Status::Processing {
                                completion: SequenceCompletion::Frames { completed, total },
                                ..
                            }) => (completed, total),
                            other => panic!("unexpected TQ progress {other:?}"),
                        })
                        .collect();
                    assert_eq!(counts, vec![(0, 4), (1, 4), (2, 4), (3, 4), (4, 4)]);
                }
                // A direct clip-wide API intentionally still has one accumulating session.
                if second == [2, 3] {
                    clear();
                    let mut a = decoder(&[1, 2, 3, 4, 5, 6]);
                    let mut b = decoder(&distorted);
                    let direct = if engine == Engine::Fmetrics {
                        metrics::fmetrics::score_probed_frames(
                            &mut a,
                            &mut b,
                            &metric(disabled),
                            &selected,
                            OutputIndexing::Aligned,
                            None,
                            |_, _| {},
                        )
                    } else {
                        metrics::vship::score_probed_frames(
                            &mut a,
                            &mut b,
                            &metric(disabled),
                            &selected,
                            OutputIndexing::Aligned,
                            None,
                            |_, _| {},
                        )
                    }
                    .unwrap();
                    assert_ne!(direct[2], values[2]);
                    assert_eq!(
                        calls().lines().filter(|s| *s == format!("{prefix}c+")).count(),
                        1
                    );
                }
            }
        }
    }
    // A callback cancellation must prevent both later frames and the next context.
    for cancel_at in [0, 1] {
        clear();
        let mut a = decoder(&[1, 2, 3, 4, 5, 6]);
        let mut b = decoder(&[10, 20, 30, 40]);
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut positions = Vec::new();
        let result = metrics::score_scene_frames(
            engine,
            &mut a,
            &mut b,
            &metric(None),
            &[&[0, 1], &[4, 5]],
            OutputIndexing::Compacted,
            Some(&cancelled),
            |position, _| {
                positions.push(position);
                if position == cancel_at {
                    cancelled.store(true, Ordering::Relaxed);
                }
            },
        );
        let error = result.unwrap_err();
        assert!(error.is::<metrics::probe::Cancelled>());
        assert_eq!(positions, if cancel_at == 0 { vec![0] } else { vec![0, 1] });
        assert_eq!(
            calls(),
            if cancel_at == 0 {
                format!("{prefix}c+\n{prefix}p\n{prefix}c-\n")
            } else {
                format!("{prefix}c+\n{prefix}p\n{prefix}p\n{prefix}c-\n")
            }
        );
    }
    for scenes in [&[&[1, 0][..]][..], &[&[0, 1][..], &[1, 2][..]][..], &[&[usize::MAX][..]][..]] {
        clear();
        let mut a = decoder(&[1]);
        let mut b = decoder(&[10]);
        assert!(
            metrics::score_scene_frames(
                engine,
                &mut a,
                &mut b,
                &metric(None),
                scenes,
                OutputIndexing::Compacted,
                None,
                |_, _| panic!("invalid scene scored")
            )
            .is_err()
        );
        assert_eq!(calls(), "");
    }
    clear();
    let mut a = decoder(&[1]);
    let mut b = decoder(&[10]);
    assert!(
        metrics::score_scene_frames(
            engine,
            &mut a,
            &mut b,
            &metric(None),
            &[&[], &[]],
            OutputIndexing::Compacted,
            None,
            |_, _| panic!("empty scene scored")
        )
        .unwrap()
        .is_empty()
    );
    assert_eq!(calls(), "");
    let cancelled = AtomicBool::new(true);
    assert!(
        metrics::score_scene_frames(
            engine,
            &mut a,
            &mut b,
            &metric(None),
            &[&[0]],
            OutputIndexing::Aligned,
            Some(&cancelled),
            |_, _| panic!("cancelled scene scored")
        )
        .unwrap_err()
        .is::<metrics::probe::Cancelled>()
    );
    assert_eq!(calls(), "");
    // Independent-frame CVVDP keeps a single session and resets after each pair.
    let mut a = decoder(&[1, 2, 3, 4, 5, 6]);
    let mut b = decoder(&[10, 20, 50, 60]);
    let values = metrics::score_scene_frames(
        engine,
        &mut a,
        &mut b,
        &metric(Some(true)),
        &[&[0, 1], &[4, 5]],
        OutputIndexing::Compacted,
        None,
        |_, _| {},
    )
    .unwrap();
    assert_eq!(values, [31.0, 62.0, 155.0, 186.0]);
    assert_eq!(
        metric(Some(true))
            .calculate_score(ProbeStatistic::Mean, &values[..2])
            .expect("score"),
        46.5
    );
    assert_eq!(
        metric(Some(true))
            .calculate_score(ProbeStatistic::Mean, &values[2..])
            .expect("score"),
        170.5
    );
    assert_eq!(
        calls().lines().filter(|s| *s == format!("{prefix}c+")).count(),
        1
    );
    // Within-scene gaps still use the M15 capability rules.
    clear();
    let mut a = decoder(&[1, 2, 3, 4, 5, 6]);
    let mut b = decoder(&[10, 30, 50, 60]);
    let values = metrics::score_scene_frames(
        engine,
        &mut a,
        &mut b,
        &metric(None),
        &[&[0, 2], &[4, 5]],
        OutputIndexing::Compacted,
        None,
        |_, _| {},
    );
    if engine == Engine::Fmetrics {
        let error = values.unwrap_err();
        assert!(format!("{error:#}").contains("history-only"), "{error:#}");
        assert_eq!(calls(), "");
    } else {
        assert_eq!(values.unwrap(), [31.0, 403.0, 155.0, 156736.0]);
        assert_eq!(calls(), "vc+\nvp\nvr\nvp\nvc-\nvc+\nvp\nvp\nvc-\n");
    }
}
