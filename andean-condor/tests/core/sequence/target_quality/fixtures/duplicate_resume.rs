//! Shared public orchestration drive with the private history reducer supplied only by source tests.
use super::{condor, current_target_fixture};
use condor::{
    core::sequence::{
        SequenceCompletion, SequenceStatus, Status, target_quality::TargetQuality,
        target_quality::Task,
    },
    models::{
        encoder::{Encoder, EncoderBase},
        sequence::target_quality::{
            TargetQualityConfig,
            types::{QualityMetric, QualityPass},
        },
    },
};
use std::sync;
// Real FFmpeg encodes, FFMS2 decode and libvmaf measurement; the resume
// history is produced by probe_pass, not a test score formula or fake backend.
type HistoryReducer = fn(&TargetQualityConfig, &[QualityPass]) -> anyhow::Result<Vec<(f64, f64)>>;

pub(super) fn check_duplicate_resume(phase: u8, reduce_history: HistoryReducer) {
    assert!((1..=3).contains(&phase));
    use condor::{
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
    let history = reduce_history(&config, &task.passes).expect("history");
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
    current_target_fixture(&mut condor);
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
    let accepted = reduce_history(&config, measured).expect("history");
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

    if phase == 1 {
        return;
    }
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

    if phase == 2 {
        return;
    }
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
