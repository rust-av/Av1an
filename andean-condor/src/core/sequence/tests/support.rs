#[cfg(unix)]
use crate as condor;
use crate::{
    core::Condor,
    models::{
        input::Input as InputModel,
        sequence::{DefaultSequenceConfig, DefaultSequenceData},
    },
};
#[cfg(unix)]
#[path = "../../../../tests/core/sequence/fixtures/native_scoring.rs"]
mod native_fixture;

pub(in crate::core::sequence) fn current_target_fixture<D, C>(job: &mut Condor<D, C>)
where
    D: crate::models::sequence::SequenceDataHandler
        + crate::models::sequence::target_quality::TargetQualityDataHandler,
    C: crate::models::sequence::SequenceConfigHandler
        + crate::models::sequence::target_quality::TargetQualityConfigHandler
        + crate::models::sequence::parallel_encoder::ParallelEncoderConfigHandler,
{
    use crate::models::sequence::provenance::{self, ResultProvenance};
    let root = job.input.as_data();
    let pe = job.sequence_config.parallel_encoder().expect("PE");
    let tq = job
        .sequence_config
        .target_quality()
        .expect("TQ")
        .as_ref()
        .expect("configured TQ");
    let input = provenance::probe_input(&root, pe, Some(tq));
    for scene in &mut job.scenes {
        let mut passes = scene.sequence_data.get_target_quality().expect("data").passes.clone();
        for pass in &mut passes {
            pass.backend.get_or_insert_with(|| {
                let engine = if matches!(
                    tq.metric,
                    crate::models::sequence::target_quality::types::QualityMetric::VMAF { .. }
                ) {
                    crate::metrics::Engine::Vmaf
                } else {
                    crate::metrics::engine(&tq.metric, !matches!(input, InputModel::Video { .. }))
                        .unwrap_or(crate::metrics::Engine::VapourSynth)
                };
                if engine == crate::metrics::Engine::VapourSynth {
                    provenance::MetricBackend::available_plugin(&tq.metric)
                        .expect("actual plugin fixture")
                } else {
                    provenance::MetricBackend::current(engine)
                }
            });
            pass.provenance = Some(ResultProvenance::new(
                provenance::target_context(input, tq.metric_input.as_ref(), scene, tq, pass)
                    .expect("explicit current fixture context"),
            ));
        }
        scene.sequence_data.get_target_quality_mut().expect("data").passes = passes;
        if !scene.sequence_data.get_target_quality().expect("data").passes.is_empty() {
            let selection = ResultProvenance::new(
                provenance::selection_context(scene, tq)
                    .expect("explicit current selection fixture"),
            );
            scene.sequence_data.get_target_quality_mut().expect("data").selection = Some(selection);
        }
    }
}

pub(in crate::core::sequence) fn current_encoding_fixture<D, C>(job: &mut Condor<D, C>)
where
    D: crate::models::sequence::SequenceDataHandler
        + crate::models::sequence::target_quality::TargetQualityDataHandler
        + crate::models::sequence::parallel_encoder::ParallelEncoderDataHandler,
    C: crate::models::sequence::SequenceConfigHandler
        + crate::models::sequence::target_quality::TargetQualityConfigHandler
        + crate::models::sequence::parallel_encoder::ParallelEncoderConfigHandler,
{
    use crate::models::sequence::provenance::{self, ResultProvenance};
    let root = job.input.as_data();
    let pe = job.sequence_config.parallel_encoder().expect("PE");
    let tq = job.sequence_config.target_quality().expect("TQ").as_ref();
    for (index, scene) in job.scenes.iter_mut().enumerate() {
        let path = pe
            .scenes_directory
            .join(format!("{index:05}.{}", scene.encoder.output_extension()));
        if path.is_file() {
            let context = provenance::encode_context(
                pe.input.as_ref().unwrap_or(&root),
                provenance::probe_input(&root, pe, tq),
                tq.and_then(|c| c.metric_input.as_ref()),
                scene,
                tq,
                &path,
                index,
            )
            .expect("explicit current chunk fixture");
            scene.sequence_data.get_parallel_encoder_mut().expect("PE data").provenance =
                Some(ResultProvenance::new(context));
        }
    }
}

pub(in crate::core::sequence) fn current_quality_fixture(
    job: &mut Condor<DefaultSequenceData, DefaultSequenceConfig>,
) {
    use crate::models::sequence::provenance::{self, MetricBackend, ResultProvenance};
    let root = job.input.as_data();
    let qc = job.sequence_config.quality_check.as_ref().expect("QC");
    let input = qc.input.as_ref().unwrap_or(&root);
    let output = job.sequence_config.scene_concatenator.effective_output(&job.output.path);
    for scene in &mut job.scenes {
        if !scene.sequence_data.quality_check.quality.is_unmeasured() {
            let engine =
                crate::metrics::engine(&qc.metric, !matches!(input, InputModel::Video { .. }))
                    .unwrap_or(crate::metrics::Engine::VapourSynth);
            let backend = if engine == crate::metrics::Engine::VapourSynth {
                MetricBackend::available_plugin(&qc.metric).expect("actual plugin fixture")
            } else {
                MetricBackend::current(engine)
            };
            let context = provenance::quality_context(input, output, scene, qc, &backend)
                .expect("explicit current quality fixture");
            let pass = &mut scene.sequence_data.quality_check.quality;
            pass.backend = Some(backend);
            pass.provenance = Some(ResultProvenance::new(context));
        }
    }
}

pub(in crate::core::sequence) mod native_scoring {
    //! Real QC/TQ scoring boundaries and decoders; only the native ABI is doubled.
    #![cfg(unix)]
    #![expect(clippy::unwrap_used, reason = "fixture assertions fail loudly")]

    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };

    use crate::core::sequence::{
        SequenceCompletion, SequenceStatus, Status, quality_check::QualityCheck,
        target_quality::TargetQuality,
    };
    use crate::{
        metrics::{self, Engine, OutputIndexing},
        models::sequence::target_quality::types::ProbeStatistic,
    };

    pub(in crate::core::sequence) use super::native_fixture::{
        calls, clear, metric, native_children, y4m_fixture,
    };

    pub(in crate::core::sequence) fn decoder(samples: &[u8]) -> av_decoders::Decoder {
        let clips = samples.iter().map(|sample| format!("c.std.BlankClip(width=32,height=32,length=1,format=vs.YUV420P8,color=[{sample},128,128])")).collect::<Vec<_>>().join(",");
        av_decoders::Decoder::from_script(&format!("import vapoursynth as vs\nc=vs.core\nc.std.Splice([{clips}]).std.SetFrameProps(_Matrix=1,_Transfer=13,_Primaries=1,_Range=1).set_output()"), Default::default()).unwrap()
    }

    pub(in crate::core::sequence) fn assert_native_failure(error: &anyhow::Error) {
        assert!(error.is::<metrics::SceneScoringStarted>(), "{error:#}");
        if std::env::var("TEMPORAL_FIXTURE_SCOPE").unwrap().contains("-f-") {
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

    pub(in crate::core::sequence) fn assert_native_calls(cancel: bool) {
        let prefix = if std::env::var("TEMPORAL_FIXTURE_SCOPE").unwrap().contains("-f-") {
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

    pub(in crate::core::sequence) fn native_failure(engine: Engine, failure: &str) {
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

    pub(in crate::core::sequence) fn sequential_decoders(engine: Engine) {
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

    pub(in crate::core::sequence) fn native_consumers(engine: Engine, phase: &str) {
        assert!(["sessions", "cancellation", "admission", "independent", "gaps"].contains(&phase));
        let prefix = if engine == Engine::Fmetrics { "f" } else { "v" };
        if phase == "sessions" {
            for second in [[2, 3], [4, 5]] {
                let scenes: &[&[usize]] = &[&[0, 1], &second];
                let selected = [0, 1, second[0], second[1]];
                for qc in [true, false] {
                    if qc != std::env::var("TEMPORAL_FIXTURE_SCOPE").unwrap().ends_with("-qc") {
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
                            let samples: Vec<_> =
                                scene.iter().map(|i| (*i as u8 + 1) * 10).collect();
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
                                                completion:
                                                    SequenceCompletion::FrameScore { frame, score },
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
        }
        // A callback cancellation must prevent both later frames and the next context.
        if phase == "cancellation" {
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
        }
        if phase == "admission" {
            for scenes in
                [&[&[1, 0][..]][..], &[&[0, 1][..], &[1, 2][..]][..], &[&[usize::MAX][..]][..]]
            {
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
        }
        // Independent-frame CVVDP keeps a single session and resets after each pair.
        if phase == "independent" {
            clear();
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
        }
        // Within-scene gaps still use the temporal capability rules.
        if phase == "gaps" {
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
    }
}
