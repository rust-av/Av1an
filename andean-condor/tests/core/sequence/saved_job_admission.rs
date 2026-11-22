//! Saved-job consumer admission. Synthetic context stamps are not producer evidence.
use crate::core::support::provenance::current_target_fixture;
use andean_condor as condor;
use condor::{
    core::{
        Condor,
        input::Input,
        output::Output,
        sequence::{
            Sequence, SequenceStatus, Status, parallel_encoder, quality_check, scene_concatenator,
            target_quality,
        },
    },
    models::{
        input::Input as InputModel,
        sequence::{
            DefaultSequenceConfig, DefaultSequenceData,
            quality_check::QualityCheckConfig,
            target_quality::{TargetQualityConfig, types::QualityPass},
        },
    },
};
use std::{cell::Cell, rc::Rc};
#[path = "fixtures/admission_support.rs"]
mod fixture;
use fixture::fixture;
#[cfg(unix)]
use fixture::produced_job;

#[cfg(unix)]
#[test]
fn certified_chunk_or_quality_history_corruption_refuses_without_repair_or_native_work() {
    for case in ["chunk-missing", "chunk-truncated", "qc-short", "qc-long", "qc-missing-output"] {
        let temp = tempfile::tempdir().expect("fixture");
        let mut job = produced_job(temp.path());
        let chunk = temp.path().join("scenes/00000.mkv");
        match case {
            "chunk-missing" => {
                std::fs::remove_file(&chunk).expect("simulate removal after certification");
            },
            "chunk-truncated" => {
                std::fs::write(&chunk, []).expect("simulate truncation after certification");
            },
            "qc-short" => {
                job.scenes[0].sequence_data.quality_check.quality.scores.pop();
            },
            "qc-long" => job.scenes[0].sequence_data.quality_check.quality.scores.push(1.0),
            _ => std::fs::remove_file(&job.output.path).expect("remove previously measured output"),
        }
        let before = serde_json::to_vec(&job.scenes).expect("record after external corruption");
        let source = std::fs::read(temp.path().join("source.mkv")).expect("source");
        let chunk_bytes = std::fs::read(&chunk).ok();
        let final_bytes = std::fs::read(&job.output.path).ok();
        let calls = std::fs::read(temp.path().join("calls")).expect("native count");
        let saves = Rc::new(Cell::new(0));
        let observed = Rc::clone(&saves);
        job.save_callback = Box::new(move |_| {
            observed.set(observed.get() + 1);
            Ok(())
        });
        let (tx, rx) = std::sync::mpsc::channel();
        let result = if case.starts_with("chunk") {
            parallel_encoder::ParallelEncoder::default().execute(&mut job, tx, Default::default())
        } else {
            quality_check::QualityCheck::default().execute(&mut job, tx, Default::default())
        };
        let error = result.expect_err(
            "reachable corruption must refuse rather than repair/remeasure current records",
        );
        let text = format!("{error:#}");
        assert!(
            text.contains("scene 0") && text.contains("new job"),
            "{case}: {text}"
        );
        assert_eq!(saves.get(), 0, "{case}");
        assert!(
            rx.try_iter().next().is_none(),
            "no scoring, resumed-size, or completion for {case}"
        );
        assert_eq!(
            serde_json::to_vec(&job.scenes).expect("preserved record"),
            before,
            "{case}"
        );
        assert_eq!(std::fs::read(&chunk).ok(), chunk_bytes);
        assert_eq!(std::fs::read(&job.output.path).ok(), final_bytes);
        assert_eq!(
            std::fs::read(temp.path().join("source.mkv")).expect("source preserved"),
            source
        );
        assert_eq!(
            std::fs::read(temp.path().join("calls")).expect("no encoder work"),
            calls
        );
    }
}

#[test]
fn owning_stages_refuse_uncertified_legacy_results_without_mutation_or_native_work() {
    let mut accepted = vec![];
    for (owner, cumulative) in [
        ("TQ", false),
        ("TQ", true),
        ("QC", false),
        ("QC", true),
        ("PE", false),
        ("concat", false),
    ] {
        let temp = tempfile::tempdir().expect("fixture");
        let mut condor = fixture(temp.path());
        let quality = QualityPass {
            quantizer: 26.0,
            scores: vec![75.0; 3],
            started_on: 1,
            completed_on: 2,
            backend: Some(
                condor::models::sequence::provenance::MetricBackend::current(
                    condor::metrics::Engine::Fmetrics,
                ),
            ),
            ..Default::default()
        };
        if owner == "TQ" {
            condor.sequence_config.target_quality = Some(TargetQualityConfig {
                maximum_probes: 1,
                ..Default::default()
            });
            condor.scenes[0].sequence_data.target_quality.passes.push(quality);
        } else if owner == "QC" {
            condor.sequence_config.quality_check = Some(QualityCheckConfig::default());
            condor.scenes[0].sequence_data.quality_check.quality = quality;
        }
        if cumulative {
            let metric = condor::models::sequence::target_quality::types::QualityMetric::CVVDP {
                target_range: (1.0, 10.0),
                resolution: None,
                display_model: None,
                resize_to_display: None,
                disable_temporal: None,
                gpu_id: None,
            };
            if owner == "TQ" {
                condor.sequence_config.target_quality.as_mut().expect("TQ").metric = metric;
            } else {
                condor.sequence_config.quality_check.as_mut().expect("QC").metric = metric;
            }
        }
        let chunk = temp.path().join("scenes").join(format!(
            "00000.{}",
            condor.encoder.base().output_extension()
        ));
        // An unrelated but correctly decodable three-frame chunk is not provenance.
        std::fs::copy(temp.path().join("source.mkv"), &chunk).expect("unrelated chunk");
        if owner == "PE" || owner == "concat" {
            let data = &mut condor.scenes[0].sequence_data.parallel_encoder;
            data.bytes = Some(std::fs::metadata(&chunk).expect("chunk").len());
            data.started_on = Some(1);
            data.completed_on = Some(2);
        }
        std::fs::write(&condor.output.path, b"final sentinel").expect("sentinel");
        let before = serde_json::to_value(&condor.scenes).expect("snapshot");
        let bytes = std::fs::read(&chunk).expect("chunk bytes");
        let saves = Rc::new(Cell::new(0));
        let count = Rc::clone(&saves);
        condor.save_callback = Box::new(move |_| {
            count.set(count.get() + 1);
            Ok(())
        });
        let (tx, rx) = std::sync::mpsc::channel();
        let result = match owner {
            "TQ" => target_quality::TargetQuality::default().execute(
                &mut condor,
                tx,
                Default::default(),
            ),
            "QC" => {
                quality_check::QualityCheck::new(None).execute(&mut condor, tx, Default::default())
            },
            "PE" => parallel_encoder::ParallelEncoder::default().execute(
                &mut condor,
                tx,
                Default::default(),
            ),
            _ => scene_concatenator::SceneConcatenator::default().execute(
                &mut condor,
                tx,
                Default::default(),
            ),
        };
        if result.is_ok() {
            accepted.push(owner);
            continue;
        }
        let error = format!("{:#}", result.expect_err("refusal"));
        assert!(
            error.contains("scene 0") && error.contains("new job"),
            "{owner}: {error}"
        );
        assert_eq!(
            serde_json::to_value(&condor.scenes).expect("after"),
            before,
            "{owner}"
        );
        assert_eq!(saves.get(), 0, "{owner}");
        assert_eq!(std::fs::read(&chunk).expect("chunk preserved"), bytes);
        assert_eq!(
            std::fs::read(&condor.output.path).expect("final preserved"),
            b"final sentinel"
        );
        assert!(
            !rx.try_iter()
                .any(|event| matches!(event, SequenceStatus::Whole(Status::Completed { .. })))
        );
    }
    assert!(
        accepted.is_empty(),
        "legacy consumers accepted: {accepted:?}"
    );
}

#[test]
fn parallel_encoder_refuses_stale_tq_selection_until_target_quality_restores_a_measured_quantizer()
{
    use condor::models::sequence::target_quality::types::QualityMetric;
    let temp = tempfile::tempdir().expect("fixture");
    let mut job = fixture(temp.path());
    job.scenes[0].encoder.set_executable(Some(temp.path().join("must-not-run")));
    job.scenes[0].encoder.set_quantizer(20.0);
    job.sequence_config.target_quality = Some(TargetQualityConfig {
        maximum_probes: 2,
        metric: QualityMetric::VMAF {
            target_range: (84.0, 86.0),
            resolution: None,
            scaler: "bicubic".into(),
            threads: 0,
            model: None,
            features: vec![],
        },
        ..Default::default()
    });
    job.scenes[0].sequence_data.target_quality.passes = [(20.0, 75.0), (30.0, 85.0)]
        .into_iter()
        .map(|(quantizer, score)| QualityPass {
            quantizer,
            scores: vec![score; 3],
            started_on: 1,
            completed_on: 2,
            ..Default::default()
        })
        .collect();
    *job.sequence_config
        .target_quality
        .as_mut()
        .expect("TQ")
        .metric
        .target_range_mut() = (74.0, 76.0);
    current_target_fixture(&mut job);
    *job.sequence_config
        .target_quality
        .as_mut()
        .expect("TQ")
        .metric
        .target_range_mut() = (84.0, 86.0);
    let before = serde_json::to_vec(&job.scenes).expect("before");
    let (tx, rx) = std::sync::mpsc::channel();
    let error = parallel_encoder::ParallelEncoder::default()
        .execute(&mut job, tx, Default::default())
        .expect_err("do not encode old selected quantizer");
    assert!(format!("{error:#}").contains("TQ selection"));
    assert_eq!(
        serde_json::to_vec(&job.scenes).expect("preserved q/history"),
        before
    );
    assert!(rx.try_iter().next().is_none());
    let saves = Rc::new(Cell::new(0));
    let observed = Rc::clone(&saves);
    job.save_callback = Box::new(move |_| {
        observed.set(observed.get() + 1);
        Ok(())
    });
    let (tx, _rx) = std::sync::mpsc::channel();
    target_quality::TargetQuality::default()
        .execute(&mut job, tx, Default::default())
        .expect("reselect under current target without probing/capability lookup");
    assert_eq!(job.scenes[0].encoder.quantizer(), Some(30.0));
    assert_eq!(saves.get(), 1);
    assert_eq!(job.scenes[0].sequence_data.target_quality.passes.len(), 2);
    assert!(!temp.path().join("scenes/00000.mkv").exists());
}

#[cfg(unix)]
#[test]
fn concat_reuses_certified_chunks_without_a_second_decoded_frame_count_probe() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    if let Some(root) = std::env::var_os("SAVED_JOB_CONCAT_CHILD") {
        let root = std::path::PathBuf::from(root);
        let model: condor::models::Condor<DefaultSequenceData, DefaultSequenceConfig> =
            serde_json::from_slice(
                &std::fs::read(root.join("saved.json")).expect("current saved model"),
            )
            .expect("model");
        let mut job = Condor::new(
            Input::from_data(&model.input).expect("native source"),
            Output::new(&model.output).expect("output"),
            model.encoder,
            model.scenes,
            Some(model.sequence_config),
            Box::new(|_| anyhow::bail!("concat cannot resave chunk metadata")),
        );
        let (tx, _rx) = std::sync::mpsc::channel();
        scene_concatenator::SceneConcatenator::default()
            .execute(&mut job, tx, Default::default())
            .expect("concat may resolve source streams, but must not launch a second full count");
        assert!(!root.join("probe-called").exists());
        assert_eq!(
            std::fs::read_to_string(root.join("source-probes")).expect("one source metadata probe"),
            "source metadata\n"
        );
        println!("admission-concat: assertions completed");
        return;
    }
    let temp = tempfile::tempdir().expect("fixture");
    let job = produced_job(temp.path());
    std::fs::write(
        temp.path().join("saved.json"),
        serde_json::to_vec(&job.as_data()).expect("saved model"),
    )
    .expect("snapshot");
    let bin = temp.path().join("bin");
    std::fs::create_dir_all(&bin).expect("isolated PATH");
    symlink(which::which("ffmpeg").expect("ffmpeg"), bin.join("ffmpeg")).expect("native muxer");
    std::fs::write(
        bin.join("ffprobe"),
          format!(
              // X05 needs source stream identity, not a second decode of certified chunks.
              "#!/bin/sh\nif [ \"$*\" != '-v error -show_streams -show_format -show_chapters -show_data_hash sha256 -of json {}' ]; then\nprintf called > '{}'\nexit 42\nfi\nprintf 'source metadata\\n' >> '{}'\nexec '{}' \"$@\"\n",
              temp.path().join("source.mkv").display(),
              temp.path().join("probe-called").display(),
              temp.path().join("source-probes").display(),
              which::which("ffprobe").expect("ffprobe").display()
          ),
    )
    .expect("probe trap");
    std::fs::set_permissions(bin.join("ffprobe"), std::fs::Permissions::from_mode(0o755))
        .expect("trap executable");
    let child = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args([
            "--exact",
            "core::sequence::saved_job_admission::concat_reuses_certified_chunks_without_a_second_decoded_frame_count_probe",
            "--nocapture",
        ])
        .env("SAVED_JOB_CONCAT_CHILD", temp.path())
        .env("PATH", bin)
        .output()
        .expect("isolated owning concat");
    assert!(
        child.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&child.stdout),
        String::from_utf8_lossy(&child.stderr)
    );
    eprintln!(
        "{}\n{}",
        String::from_utf8_lossy(&child.stdout),
        String::from_utf8_lossy(&child.stderr)
    );
    assert!(String::from_utf8_lossy(&child.stdout).contains("1 passed; 0 failed"));
    assert!(
        String::from_utf8_lossy(&child.stdout).contains("admission-concat: assertions completed")
    );
    assert!(!temp.path().join("probe-called").exists());
}

// Real owning-stage results establish the saved-job roundtrip baseline.
#[cfg(unix)]
fn check_roundtrip_dependencies(phase: u8) {
    assert!((1..=4).contains(&phase));
    use condor::models::{
        encoder::cli_parameter::CLIParameter,
        sequence::{provenance::RESULT_REVISION, target_quality::types::QualityMetric},
    };
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().expect("fixture");
    let mut job = fixture(temp.path());
    let wrapper = temp.path().join("encoder");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nprintf call\\n >> '{}'\nexec '{}' \"$@\"\n",
            temp.path().join("calls").display(),
            which::which("ffmpeg").expect("ffmpeg").display()
        ),
    )
    .expect("wrapper");
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).expect("executable");
    let encoder = &mut job.scenes[0].encoder;
    encoder.set_executable(Some(wrapper));
    encoder.parameters_mut().extend(CLIParameter::new_strings(
        "-",
        " ",
        &[("c:v", "libx264"), ("preset", "ultrafast")],
    ));
    encoder.set_quantizer(20.0);
    job.sequence_config.target_quality = Some(TargetQualityConfig {
        maximum_probes: 1,
        quantizer_range: (20, 20),
        metric: QualityMetric::VMAF {
            target_range: (-1000.0, 1000.0),
            resolution: None,
            scaler: "bicubic".into(),
            threads: 0,
            model: None,
            features: vec![],
        },
        ..Default::default()
    });
    std::fs::create_dir_all(temp.path().join("scenes/Target Quality/1/Scene Concatenator"))
        .expect("probe scratch");
    let (tx, rx) = std::sync::mpsc::channel();
    target_quality::TargetQuality::default()
        .execute(&mut job, tx, Default::default())
        .expect("real TQ");
    assert_eq!(job.scenes[0].sequence_data.target_quality.passes.len(), 1);
    let pass = &job.scenes[0].sequence_data.target_quality.passes[0];
    assert_eq!(
        RESULT_REVISION, 1,
        "H.26x migration must not invalidate other v1 jobs"
    );
    assert!(
        pass.provenance
            .as_ref()
            .expect("actual FFmpeg stamp")
            .context
            .get("probe_encoding")
            .is_none(),
        "the independent-scene policy belongs only to x264/x265; this FFmpeg v1 job resumes below"
    );
    assert_eq!(
        pass.backend.as_ref().expect("actual backend").engine,
        condor::metrics::Engine::Vmaf
    );
    assert_eq!(
        pass.provenance.as_ref().expect("successful owning stamp").revision,
        RESULT_REVISION
    );
    let _: Vec<_> = rx.try_iter().collect();
    let (tx, _rx) = std::sync::mpsc::channel();
    parallel_encoder::ParallelEncoder::default()
        .execute(&mut job, tx, Default::default())
        .expect("real final encode");
    assert!(job.scenes[0].sequence_data.parallel_encoder.provenance.is_some());
    let (tx, _rx) = std::sync::mpsc::channel();
    scene_concatenator::SceneConcatenator::default()
        .execute(&mut job, tx, Default::default())
        .expect("real final mux");
    job.sequence_config.quality_check = Some(QualityCheckConfig {
        metric: job.sequence_config.target_quality.as_ref().expect("TQ").metric.clone(),
        ..Default::default()
    });
    let (tx, _rx) = std::sync::mpsc::channel();
    quality_check::QualityCheck::default()
        .execute(&mut job, tx, Default::default())
        .expect("real QC");
    assert!(job.scenes[0].sequence_data.quality_check.quality.provenance.is_some());
    let bytes = serde_json::to_vec(&job.as_data()).expect("current saved job");
    let model: condor::models::Condor<DefaultSequenceData, DefaultSequenceConfig> =
        serde_json::from_slice(&bytes).expect("roundtrip");
    let mut resumed = Condor::new(
        Input::from_data(&model.input).expect("reopen"),
        Output::new(&model.output).expect("output"),
        model.encoder,
        model.scenes,
        Some(model.sequence_config),
        Box::new(|_| anyhow::bail!("matching current work must not save/measure/encode")),
    );
    assert_eq!(
        job.scenes[0].sequence_data.target_quality.passes[0]
            .scores
            .iter()
            .map(|s| s.to_bits())
            .collect::<Vec<_>>(),
        resumed.scenes[0].sequence_data.target_quality.passes[0]
            .scores
            .iter()
            .map(|s| s.to_bits())
            .collect::<Vec<_>>(),
        "raw measured scores must survive production JSON roundtrip exactly"
    );
    assert_eq!(
        job.scenes[0].sequence_data.target_quality.passes[0].bitrate.to_bits(),
        resumed.scenes[0].sequence_data.target_quality.passes[0].bitrate.to_bits(),
        "measured bitrate roundtrip"
    );
    let calls = std::fs::read(temp.path().join("calls")).expect("actual encode calls");
    resumed.sequence_config.parallel_encoder.workers = Some(2);
    resumed.sequence_config.parallel_encoder.max_tries = 7;
    for owner in ["TQ", "QC", "PE"] {
        let (tx, _rx) = std::sync::mpsc::channel();
        match owner {
            "TQ" => target_quality::TargetQuality::default().execute(
                &mut resumed,
                tx,
                Default::default(),
            ),
            "QC" => {
                quality_check::QualityCheck::default().execute(&mut resumed, tx, Default::default())
            },
            _ => parallel_encoder::ParallelEncoder::default().execute(
                &mut resumed,
                tx,
                Default::default(),
            ),
        }
        .unwrap_or_else(|e| panic!("{owner} matching roundtrip: {e:#}"));
    }
    assert_eq!(
        std::fs::read(temp.path().join("calls")).expect("no new native encode"),
        calls
    );
    if phase == 1 {
        return;
    }
    let original = resumed.scenes.clone();
    let config = resumed.sequence_config.clone();
    let chunk = temp.path().join("scenes/00000.mkv");
    let chunk_bytes = std::fs::read(&chunk).expect("chunk");
    let final_bytes = std::fs::read(&resumed.output.path).expect("final");
    std::fs::copy(
        temp.path().join("source.mkv"),
        temp.path().join("other-source.mkv"),
    )
    .expect("same pixels at another path");
    for change in [
        "source-path",
        "filters",
        "future",
        "metric",
        "model",
        "backend",
        "range",
        "encoder",
        "strategy",
        "target",
        "budget",
    ] {
        resumed.scenes = original.clone();
        resumed.sequence_config = config.clone();
        if let Input::Video { path, filters, .. } = &mut resumed.input {
            *path = temp.path().join("source.mkv");
            filters.clear();
        }
        let tq = resumed.sequence_config.target_quality.as_mut().expect("TQ");
        match change {
            "source-path" => {
                if let Input::Video { path, .. } = &mut resumed.input {
                    *path = temp.path().join("other-source.mkv");
                }
            },
            "filters" => {
                if let Input::Video { filters, .. } = &mut resumed.input {
                    filters.push(condor::models::input::Ffms2Filter::OutputFormat {
                        bit_depth: Some(10),
                        chroma: None,
                        width: None,
                        height: None,
                    });
                }
            },
            "future" => {
                resumed.scenes[0].sequence_data.target_quality.passes[0]
                    .provenance
                    .as_mut()
                    .expect("stamp")
                    .revision += 1;
            },
            "metric" => {
                if let QualityMetric::VMAF { scaler, .. } = &mut tq.metric {
                    *scaler = "lanczos".into();
                }
            },
            "model" => {
                let path = temp.path().join("model.json");
                std::fs::write(&path, "custom model bytes").expect("model");
                if let QualityMetric::VMAF { model, .. } = &mut tq.metric {
                    *model = Some(path);
                }
            },
            "backend" => {
                resumed.scenes[0].sequence_data.target_quality.passes[0]
                    .backend
                    .as_mut()
                    .expect("backend")
                    .engine = condor::metrics::Engine::Fmetrics;
            },
            "range" => resumed.scenes[0].end_frame = 2,
            "encoder" => {
                resumed.scenes[0].encoder.parameters_mut().insert(
                    "preset".into(),
                    CLIParameter::new_string("-", " ", "medium"),
                );
            },
            "strategy" => {
                tq.probing.strategy =
                    condor::models::sequence::target_quality::types::ProbeStrategy::Skip { skip: 2 }
            },
            "target" => *tq.metric.target_range_mut() = (60.0, 80.0),
            _ => tq.maximum_probes = 2,
        }
        let before = serde_json::to_vec(&resumed.scenes).expect("before refusal");
        for owner in ["PE", "concat"] {
            let (tx, rx) = std::sync::mpsc::channel();
            let result = if owner == "PE" {
                parallel_encoder::ParallelEncoder::default().execute(
                    &mut resumed,
                    tx,
                    Default::default(),
                )
            } else {
                scene_concatenator::SceneConcatenator::default().execute(
                    &mut resumed,
                    tx,
                    Default::default(),
                )
            };
            assert!(result.is_err(), "{owner} accepts {change}");
            assert_eq!(
                serde_json::to_vec(&resumed.scenes).expect("unchanged history/q"),
                before
            );
            assert!(
                !rx.try_iter()
                    .any(|e| matches!(e, SequenceStatus::Whole(Status::Completed { .. })))
            );
        }
        assert_eq!(std::fs::read(&chunk).expect("chunk preserved"), chunk_bytes);
        assert_eq!(
            std::fs::read(&resumed.output.path).expect("final preserved"),
            final_bytes
        );
    }
    if phase == 2 {
        return;
    }
    resumed.scenes = original;
    resumed.sequence_config = config;
    // Budget/target edits retain measured history, even when chunks now refuse.
    resumed.sequence_config.target_quality.as_mut().expect("TQ").maximum_probes = 2;
    let policy_saves = Rc::new(Cell::new(0));
    let observed = Rc::clone(&policy_saves);
    resumed.save_callback = Box::new(move |_| {
        observed.set(observed.get() + 1);
        Ok(())
    });
    let (tx, _rx) = std::sync::mpsc::channel();
    target_quality::TargetQuality::default()
        .execute(&mut resumed, tx, Default::default())
        .expect("target hit reuses raw history under new budget");
    assert_eq!(
        policy_saves.get(),
        1,
        "persist only the new selection policy, not another measurement"
    );
    assert_eq!(
        std::fs::read(temp.path().join("calls")).expect("no extra encode"),
        calls
    );
    if phase == 3 {
        return;
    }
    // Ordinary same-path replacement must not be accepted merely by frame count.
    std::fs::write(temp.path().join("source.mkv"), b"replacement source")
        .expect("ordinary source edit");
    let (tx, _rx) = std::sync::mpsc::channel();
    assert!(
        target_quality::TargetQuality::default()
            .execute(&mut resumed, tx, Default::default())
            .is_err()
    );
}

#[cfg(unix)]
#[test]
fn measured_job_json_roundtrip_resumes_without_reencoding_or_score_bit_changes() {
    check_roundtrip_dependencies(1);
}

#[cfg(unix)]
#[test]
fn changed_job_dependencies_refuse_without_mutating_history_or_artifact_bytes() {
    check_roundtrip_dependencies(2);
}

#[cfg(unix)]
#[test]
fn selection_policy_changes_preserve_measurements_and_save_only_the_new_selection() {
    check_roundtrip_dependencies(3);
}

#[cfg(unix)]
#[test]
fn same_path_source_replacement_invalidates_saved_results() {
    check_roundtrip_dependencies(4);
}

fn check_model_inputs(test: &str, phase: u8) {
    assert!((1..=4).contains(&phase));
    use condor::models::{
        input::VapourSynthScriptSource, sequence::target_quality::types::QualityMetric,
    };
    if std::env::var_os("SAVED_JOB_ADMISSION_MODEL_CHILD").is_none() {
        let temp = tempfile::tempdir().expect("override directories");
        for name in ["one", "two"] {
            std::fs::create_dir_all(temp.path().join(name)).expect("model dir");
            std::fs::write(temp.path().join(name).join("vmaf_v0.6.1.json"), name)
                .expect("model bytes");
        }
        let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args(["--exact", test, "--nocapture"])
            .env("SAVED_JOB_ADMISSION_MODEL_CHILD", temp.path())
            .env("VMAF_MODEL_PATH", temp.path().join("one"))
            .output()
            .expect("isolated environment test");
        assert!(
            output.status.success()
                && String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed")
                && String::from_utf8_lossy(&output.stdout)
                    .contains(&format!("admission-model {phase}: assertions completed")),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        eprintln!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let temp = tempfile::tempdir().expect("fixture");
    let mut job = fixture(temp.path());
    job.sequence_config.target_quality = Some(TargetQualityConfig {
        maximum_probes: 1,
        metric: QualityMetric::VMAF {
            target_range: (70.0, 80.0),
            resolution: None,
            scaler: "bicubic".into(),
            threads: 0,
            model: None,
            features: vec![],
        },
        ..Default::default()
    });
    job.scenes[0].sequence_data.target_quality.passes = vec![QualityPass {
        quantizer: 26.0,
        scores: vec![75.0; 3],
        started_on: 1,
        completed_on: 2,
        ..Default::default()
    }];
    job.scenes[0].encoder.set_quantizer(26.0);
    current_target_fixture(&mut job);
    let original = job.scenes.clone();
    let model = std::path::PathBuf::from(std::env::var_os("VMAF_MODEL_PATH").expect("override"))
        .join("vmaf_v0.6.1.json");
    let modified = model.metadata().expect("metadata").modified().expect("mtime");
    std::fs::write(&model, b"ONE").expect("same-length changed model bytes");
    std::fs::File::open(&model)
        .expect("model")
        .set_times(std::fs::FileTimes::new().set_modified(modified))
        .expect("preserve mtime to isolate content proof");
    let (tx, _rx) = std::sync::mpsc::channel();
    assert!(
        target_quality::TargetQuality::default()
            .execute(&mut job, tx, Default::default())
            .is_err()
    );
    assert_eq!(
        serde_json::to_vec(&job.scenes).expect("preserved"),
        serde_json::to_vec(&original).expect("before")
    );
    if phase == 1 {
        println!("admission-model {phase}: assertions completed");
        return;
    }
    // SAFETY: this is a dedicated subprocess; no other test reads its environment.
    unsafe {
        std::env::set_var(
            "VMAF_MODEL_PATH",
            std::path::PathBuf::from(
                std::env::var_os("SAVED_JOB_ADMISSION_MODEL_CHILD").expect("root"),
            )
            .join("two"),
        );
    }
    let (tx, _rx) = std::sync::mpsc::channel();
    assert!(
        target_quality::TargetQuality::default()
            .execute(&mut job, tx, Default::default())
            .is_err()
    );
    current_target_fixture(&mut job);
    std::fs::remove_file(
        std::path::PathBuf::from(std::env::var_os("VMAF_MODEL_PATH").expect("override"))
            .join("vmaf_v0.6.1.json"),
    )
    .expect("missing previous model");
    let (tx, _rx) = std::sync::mpsc::channel();
    assert!(
        target_quality::TargetQuality::default()
            .execute(&mut job, tx, Default::default())
            .is_err()
    );

    if phase == 2 {
        println!("admission-model {phase}: assertions completed");
        return;
    }
    // An explicit custom model ignores the now-missing environment override,
    // but cannot ignore changes to its own bytes, even with preserved stats.
    let custom = temp.path().join("custom.json");
    std::fs::write(&custom, b"AAA").expect("custom model fixture bytes");
    if let QualityMetric::VMAF { model, .. } =
        &mut job.sequence_config.target_quality.as_mut().expect("TQ").metric
    {
        *model = Some(custom.clone());
    }
    current_target_fixture(&mut job);
    let (tx, _rx) = std::sync::mpsc::channel();
    target_quality::TargetQuality::default().execute(&mut job, tx, Default::default()).expect("explicit custom identity excludes environment override; no native measurement required");
    let modified = custom.metadata().expect("model stat").modified().expect("mtime");
    std::fs::write(&custom, b"BBB").expect("same-size model edit");
    std::fs::File::open(&custom)
        .expect("model")
        .set_times(std::fs::FileTimes::new().set_modified(modified))
        .expect("preserve model mtime");
    let (tx, _rx) = std::sync::mpsc::channel();
    assert!(
        target_quality::TargetQuality::default()
            .execute(&mut job, tx, Default::default())
            .is_err()
    );

    if phase == 3 {
        println!("admission-model {phase}: assertions completed");
        return;
    }
    let code = "import vapoursynth as vs\nvs.core.std.BlankClip(width=128,height=128,length=3,format=vs.YUV420P8).set_output()";
    let script = InputModel::VapourSynthScript {
        source: VapourSynthScriptSource::Text(code.into()),
        variables: Default::default(),
        index: 0,
        filters: vec![],
        stream_concurrently: false,
    };
    job.input = Input::from_data(&script).expect("lazy inline input");
    job.sequence_config.target_quality.as_mut().expect("TQ").metric = QualityMetric::default();
    job.scenes[0].sequence_data.target_quality.passes[0].backend = None;
    for edit in ["vars", "text", "index", "filters"] {
        current_target_fixture(&mut job);
        match &mut job.input {
            Input::VapourSynthScript {
                source,
                variables,
                index,
                filters,
                ..
            } => match edit {
                "vars" => {
                    variables.insert("declared".into(), "changed".into());
                },
                "text" => {
                    *source = VapourSynthScriptSource::Text(format!("{code}\n# changed script"));
                },
                "index" => *index = 1,
                _ => filters.push(
                    condor::vapoursynth::vapoursynth_filters::VapourSynthFilter::Resize {
                        scaler: None,
                        width: Some(64),
                        height: None,
                        format: None,
                    },
                ),
            },
            _ => unreachable!(),
        }
        let (tx, _rx) = std::sync::mpsc::channel();
        assert!(
            target_quality::TargetQuality::default()
                .execute(&mut job, tx, Default::default())
                .is_err(),
            "{edit}"
        );
    }

    println!("admission-model {phase}: assertions completed");
}

#[test]
fn model_byte_changes_invalidate_history_even_with_preserved_file_stats() {
    check_model_inputs(
        "core::sequence::saved_job_admission::model_byte_changes_invalidate_history_even_with_preserved_file_stats",
        1,
    );
}

#[test]
fn model_environment_override_changes_invalidate_saved_history() {
    check_model_inputs(
        "core::sequence::saved_job_admission::model_environment_override_changes_invalidate_saved_history",
        2,
    );
}

#[test]
fn explicit_custom_model_takes_precedence_over_environment_override() {
    check_model_inputs(
        "core::sequence::saved_job_admission::explicit_custom_model_takes_precedence_over_environment_override",
        3,
    );
}

#[test]
fn script_text_variables_output_index_and_filters_are_saved_job_dependencies() {
    check_model_inputs(
        "core::sequence::saved_job_admission::script_text_variables_output_index_and_filters_are_saved_job_dependencies",
        4,
    );
}
