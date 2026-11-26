//! Public TQ orchestration; synthetic prefixes are consumer-admission fixtures.
use super::{condor, probe_fixture};
#[path = "fixtures/duplicate_resume.rs"]
mod duplicate_fixture;
use crate::core::support::provenance::current_target_fixture;
use condor::{
    core::{
        Condor,
        input::Input,
        output::Output,
        sequence::{
            Sequence, SequenceCompletion, SequenceStatus, Status, target_quality::TargetQuality,
        },
    },
    metrics::Engine,
    models::{
        encoder::{Encoder, EncoderBase},
        input::Input as InputModel,
        scene::Scene,
        sequence::{
            DefaultSequenceConfig, DefaultSequenceData,
            parallel_encoder::ParallelEncoderConfig,
            provenance::MetricBackend,
            scene_concatenator::ConcatMethod,
            target_quality::{TargetQualityDataHandler, types::QualityPass},
        },
    },
    utils::cancellation::{Cancellation, Cancelled},
};
#[cfg(unix)]
use probe_fixture::executable;
use probe_fixture::{
    fixture, probe, probe_budget_progress, probe_budget_watchdog, probe_execution_fixture, tasks,
};
use std::{sync, thread};

#[test]
fn resumed_duplicate_prediction_measures_an_untested_quantizer_and_finalizes_closest_measurement() {
    duplicate_fixture::check_duplicate_resume(1, public_history);
}

#[test]
fn maximum_probe_budget_never_finalizes_an_unmeasured_next_prediction() {
    duplicate_fixture::check_duplicate_resume(2, public_history);
}

#[test]
fn force_after_accepted_measurement_saves_the_measured_final_quantizer_before_returning() {
    duplicate_fixture::check_duplicate_resume(3, public_history);
}

// Removing the normal wrapper's probe call or the last save must fail this test.
#[test]
fn public_execute_saves_255th_native_record() {
    if std::env::var("PROBE_EXECUTION_CASE").as_deref() != Ok("native-255") {
        let stdout = probe_budget_watchdog(
            "native-255",
            "core::sequence::target_quality::execute::public_execute_saves_255th_native_record",
        );
        assert!(stdout.contains("probe-budget native-255: assertions completed"));
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
    current_target_fixture(&mut condor);
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
        .execute(
            &mut condor,
            tx,
            andean_condor::utils::cancellation::Cancellation::default(),
        )
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

    println!("probe-budget native-255: assertions completed");
}

#[test]
fn exhausted_or_satisfied_probe_budget_preserves_history_without_new_work_or_extra_saves() {
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
        current_target_fixture(&mut condor);
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
                .execute(&mut condor, tx, Default::default())
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
        println!("probe-no-work {case}: assertions completed");
        return;
    }

    let mut failures = Vec::new();
    for case in ["early-hit", "budget-255"] {
        // Parent owns all child temporary files even if the watchdog must kill it.
        let temp = tempfile::tempdir().expect("watchdog root");
        let mut child = Command::new(std::env::current_exe().expect("test binary"))
                .args(["--exact", "core::sequence::target_quality::execute::exhausted_or_satisfied_probe_budget_preserves_history_without_new_work_or_extra_saves", "--nocapture"])
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
        if timed_out
            || !output.status.success()
            || !stdout.contains("1 passed; 0 failed")
            || !stdout.contains(&format!("probe-no-work {case}: assertions completed"))
        {
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
        std::fs::write(directory.join(path), b"verified peer scratch/history").expect("sentinel");
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
    current_target_fixture(&mut condor);
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
    current_target_fixture(&mut condor);
    let prefix = serde_json::to_vec(&condor.scenes[0].sequence_data.target_quality.passes)
        .expect("current fixture prefix");
    let peer_history = serde_json::to_vec(&condor.scenes[1].sequence_data.target_quality.passes)
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

fn execute_probe_rounds(
    condor: &mut Condor<DefaultSequenceData, DefaultSequenceConfig>,
) -> Vec<(u8, u64, f64, f64)> {
    let (tx, rx) = sync::mpsc::channel();
    let (_, warnings) = TargetQuality::default()
        .execute(condor, tx, Default::default())
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

fn check_lowered_budget(phase: u8) {
    assert!((1..=3).contains(&phase));
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
    if phase == 1 {
        return;
    }
    condor.scenes[0].encoder.set_quantizer(20.0);
    assert!(execute_probe_rounds(&mut condor).is_empty());
    assert_eq!(condor.scenes[0].encoder.quantizer(), Some(26.0));
    assert!(execute_probe_rounds(&mut condor).is_empty());
    assert_eq!(
        saved.borrow().len(),
        1,
        "only changed measured-q restoration saves"
    );
    if phase == 2 {
        return;
    }
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
            serde_json::to_vec(&old.sequence_data.target_quality.passes).expect("original prefix")
        );
    }
    assert_eq!(saved.borrow().len(), 2);
    eprintln!(
        "probe-rounds lowered max1 [3,1] no work/scratch/save, one changed-q restoration save only; raised max2 -> [3,2] with progress={raised:?}"
    );
}

#[test]
fn lowered_probe_budget_preserves_history_without_creating_scratch() {
    check_lowered_budget(1);
}

#[test]
fn no_work_restores_measured_quantizer_and_saves_only_when_changed() {
    check_lowered_budget(2);
}

#[test]
fn raised_probe_budget_resumes_only_scenes_below_the_new_limit() {
    check_lowered_budget(3);
}

fn check_measured_restoration(phase: u8) {
    assert!((1..=3).contains(&phase));
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
        data.get_target_quality_mut().expect("TQ").passes = history;
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
        current_target_fixture(&mut condor);
        let history = condor.scenes[0].sequence_data.target_quality.passes.clone();
        for _ in 0..2 {
            let (tx, _rx) = sync::mpsc::channel();
            TargetQuality::default()
                .execute(&mut condor, tx, Default::default())
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
        if phase == 1 {
            continue;
        }
        for forced in [false, true] {
            let cancelled = Cancellation::default();
            cancelled.request().expect("graceful");
            if forced {
                cancelled.request().expect("force");
            }
            let (tx, _rx) = sync::mpsc::channel();
            let error = TargetQuality::default()
                .execute(&mut condor, tx, cancelled)
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
        if phase == 2 {
            continue;
        }
        condor.scenes[0].encoder.set_quantizer(20.0);
        condor.sequence_config.target_quality = None;
        let (tx, _rx) = sync::mpsc::channel();
        TargetQuality::default()
            .execute(&mut condor, tx, Default::default())
            .expect("disabled");
        assert_eq!(condor.scenes[0].encoder.quantizer(), Some(20.0));
        condor.scenes.clear();
        let (tx, _rx) = sync::mpsc::channel();
        TargetQuality::default()
            .execute(&mut condor, tx, Default::default())
            .expect("empty");
        assert_eq!(snapshots.borrow().len(), 1);
        eprintln!(
            "probe-resume no-work target_hit={target_hit}: measured q26 restored from configured q20, exactly one actual save across repeated execution; disabled/empty/cancel controls unchanged"
        );
    }
}

#[test]
fn no_work_tq_restores_a_measured_quantizer_and_saves_once() {
    check_measured_restoration(1);
}

#[test]
fn no_work_tq_retains_typed_cancellation_precedence() {
    check_measured_restoration(2);
}

#[test]
fn disabled_or_empty_tq_does_not_restore_a_quantizer_or_save() {
    check_measured_restoration(3);
}

#[cfg(unix)]
fn check_probe_interruption(modes: &[&'static str], replay: bool) {
    use std::time::{Duration, Instant};
    for &mode in modes {
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
        condor.sequence_config.scene_concatenator.method = ConcatMethod::FFmpeg(Default::default());
        current_target_fixture(&mut condor);
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
                    source: condor::models::input::VapourSynthScriptSource::Text(callback),
                    variables: Default::default(),
                    index: 0,
                    filters: vec![],
                    stream_concurrently: false,
                })
                .expect("real lazy native reference input"),
            );
            condor.sequence_config.target_quality.as_mut().expect("TQ").metric_input =
                sequence.metric_input.as_ref().map(Input::as_data);
            current_target_fixture(&mut condor);
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
            let result = sequence.execute(&mut condor, tx, cancellation);
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
            let result = sequence.execute(&mut condor, tx, cancellation);
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
        if !replay {
            continue;
        }
        for scene in &mut condor.scenes {
            scene.encoder.set_executable(None);
        }
        condor.sequence_config.target_quality.as_mut().expect("TQ").quantizer_range = (34, 34);
        condor.sequence_config.target_quality.as_mut().expect("TQ").metric_input = None;
        current_target_fixture(&mut condor);
        let (tx, _rx) = sync::mpsc::channel();
        TargetQuality::default()
            .execute(&mut condor, tx, Default::default())
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

#[cfg(unix)]
#[test]
fn probe_cleanup_failure_leaves_history_and_caller_files_unchanged_before_native_work() {
    check_probe_interruption(&["cleanup-failure"], false);
}

#[cfg(unix)]
#[test]
fn cancelled_or_callback_failed_probe_keeps_accepted_prefix_without_partial_completion() {
    check_probe_interruption(
        &[
            "graceful",
            "forced",
            "closed-progress",
            "native-graceful",
            "native-forced",
            "native-callback-fault",
        ],
        false,
    );
}

#[cfg(unix)]
#[test]
fn resumed_probe_reencodes_current_quantizer_and_preserves_previous_measured_prefix() {
    check_probe_interruption(
        &[
            "graceful",
            "forced",
            "closed-progress",
            "native-graceful",
            "native-forced",
            "native-callback-fault",
            "cleanup-failure",
        ],
        true,
    );
}

fn public_history(
    config: &condor::models::sequence::target_quality::TargetQualityConfig,
    passes: &[QualityPass],
) -> anyhow::Result<Vec<(f64, f64)>> {
    passes
        .iter()
        .map(|pass| {
            Ok((
                pass.quantizer,
                config.metric.calculate_score(config.probing.statistic, &pass.scores)?,
            ))
        })
        .collect()
}
