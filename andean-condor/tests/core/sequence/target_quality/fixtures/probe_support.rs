//! Real FFmpeg/FFMS2/libvmaf fixture shared by public workflows and private TQ seams.
use super::condor;
use condor::{
    core::{
        input::Input,
        sequence::target_quality::{TargetQuality, Task},
    },
    models::{
        encoder::{Encoder, EncoderBase, cli_parameter::CLIParameter},
        input::{ImportMethod, Input as InputModel},
        sequence::target_quality::{TargetQualityConfig, types::QualityMetric},
    },
};
use std::{
    path::{Path, PathBuf},
    sync,
};

pub(super) fn fixture() -> (
    tempfile::TempDir,
    Input,
    Encoder,
    TargetQualityConfig,
    PathBuf,
) {
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
        .expect("existing FFmpeg");
    assert!(generated.status.success(), "{generated:?}");
    let input = Input::from_video(&InputModel::Video {
        path: source,
        import_method: ImportMethod::FFMS2 {
            index: None,
            cache_path: None,
        },
        filters: vec![],
    })
    .expect("FFMS2");
    let mut encoder = Encoder::default_from_base(&EncoderBase::FFmpeg, false);
    encoder.parameters_mut().extend(CLIParameter::new_strings(
        "-",
        " ",
        &[("c:v", "libx264"), ("preset", "ultrafast")],
    ));
    encoder.set_quantizer(26.0);
    let config = TargetQualityConfig {
        quantizer_range: (26, 34),
        maximum_probes: 2,
        metric: QualityMetric::VMAF {
            target_range: (0.0, 0.0),
            resolution: None,
            scaler: "bicubic".into(),
            threads: 0,
            model: None,
            features: vec![],
        },
        ..Default::default()
    };
    let directory = temp.path().join("Target Quality");
    for pass in 1..=2 {
        std::fs::create_dir_all(directory.join(pass.to_string()).join("Scene Concatenator"))
            .expect("scratch");
    }
    (temp, input, encoder, config, directory)
}

pub(super) fn tasks(encoder: &Encoder, directory: &Path, pass: u8) -> Vec<Task> {
    [vec![0, 1], vec![2, 3]]
        .into_iter()
        .enumerate()
        .map(|(index, frame_indices)| Task {
            original_index: index,
            frame_indices,
            encoder: encoder.clone(),
            output: directory.join(pass.to_string()).join(format!("{index:05}.mkv")),
            passes: vec![],
        })
        .collect()
}

pub(super) fn probe(
    pass: u8,
    directory: &Path,
    config: &TargetQualityConfig,
    input: &mut Input,
    tasks: &[Task],
) -> anyhow::Result<Vec<Task>> {
    let (tx, _rx) = sync::mpsc::channel();
    let (completed, warnings) = TargetQuality::probe_pass(
        pass,
        directory,
        config,
        input,
        None,
        1,
        &condor::models::sequence::scene_concatenator::ConcatMethod::FFmpeg(Default::default()),
        tasks,
        tx,
        &Default::default(),
        Default::default(),
    )?;
    assert!(warnings.is_empty(), "{warnings:?}");
    Ok(completed)
}

#[cfg(unix)]
pub(super) fn executable(path: &Path, source: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, source).expect("script");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("executable");
}

use super::current_target_fixture;
use condor::{
    core::{
        Condor,
        output::Output,
        sequence::{SequenceCompletion, SequenceStatus, Status},
    },
    metrics::Engine,
    models::{
        scene::Scene,
        sequence::{
            DefaultSequenceConfig, DefaultSequenceData, parallel_encoder::ParallelEncoderConfig,
            provenance::MetricBackend, scene_concatenator::ConcatMethod,
            target_quality::types::QualityPass,
        },
    },
};
pub(super) type SavedScenes = std::rc::Rc<std::cell::RefCell<Vec<Vec<Scene<DefaultSequenceData>>>>>;

pub(super) fn probe_execution_fixture(
    counts: &[usize],
    maximum_probes: u8,
) -> (
    tempfile::TempDir,
    Condor<DefaultSequenceData, DefaultSequenceConfig>,
    SavedScenes,
) {
    let (temp, input, encoder, mut config, directory) = fixture();
    config.maximum_probes = maximum_probes;
    for pass in 1..=maximum_probes {
        std::fs::create_dir_all(directory.join(pass.to_string()).join("Scene Concatenator"))
            .expect("concat scratch");
    }
    // Synthetic, admitted saved records; all appended records use real FFmpeg/FFMS2/VMAF.
    let scenes = counts
        .iter()
        .enumerate()
        .map(|(index, &count)| {
            let mut data = DefaultSequenceData::default();
            data.target_quality.passes = (0..count)
                .map(|pass| QualityPass {
                    provenance: None,
                    backend: Some(MetricBackend::current(Engine::Vmaf)),
                    quantizer: [26.0, 30.0, 34.0][pass],
                    scores: vec![10.0 + pass as f64; 2],
                    bitrate: 100.0 + pass as f64,
                    started_on: 12 + pass as u128,
                    completed_on: 34 + pass as u128,
                })
                .collect();
            Scene {
                start_frame: index,
                end_frame: index + 2,
                sub_scenes: None,
                encoder: encoder.clone(),
                sequence_data: data,
            }
        })
        .collect();
    let saved = SavedScenes::default();
    let snapshots = std::rc::Rc::clone(&saved);
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
            snapshots.borrow_mut().push(data.scenes);
            Ok(())
        }),
    );
    condor.sequence_config.scene_concatenator.method = ConcatMethod::FFmpeg(Default::default());
    current_target_fixture(&mut condor);
    (temp, condor, saved)
}

pub(super) fn probe_budget_progress(
    events: Vec<SequenceStatus>,
    maximum: u8,
) -> (Vec<u8>, Vec<u8>) {
    let mut rounds = Vec::new();
    let mut quality = Vec::new();
    for event in events {
        match event {
            SequenceStatus::Whole(Status::Processing {
                completion: SequenceCompletion::Passes { completed, total },
                ..
            }) => {
                assert!(total > 0 && total <= maximum);
                assert_eq!(
                    completed,
                    total - 1,
                    "zero completed is valid only before pass1"
                );
                rounds.push(total);
            },
            SequenceStatus::Subprocess {
                parent:
                    Status::Processing {
                        completion: SequenceCompletion::Passes { completed, total },
                        ..
                    },
                child,
            } => {
                assert!(completed > 0 && completed <= maximum);
                assert_eq!(total, maximum);
                if matches!(
                    child,
                    Status::Processing {
                        completion: SequenceCompletion::SceneQuality { .. },
                        ..
                    }
                ) {
                    quality.push(completed);
                }
            },
            other => panic!("unexpected owning progress: {other:?}"),
        }
    }
    (rounds, quality)
}

pub(super) fn probe_budget_watchdog(case: &str, test: &str) -> String {
    use std::{
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    // File-backed output cannot fill a pipe and masquerade as an overflow spin.
    // The parent owns child scratch, including cleanup after a killed child.
    let temp = tempfile::tempdir().expect("probe-budget watchdog root");
    let stdout_path = temp.path().join("stdout.log");
    let stderr_path = temp.path().join("stderr.log");
    let mut child = Command::new(std::env::current_exe().expect("test binary"))
        .args(["--exact", test, "--nocapture"])
        .env("PROBE_EXECUTION_CASE", case)
        .env("TMPDIR", temp.path())
        .stdout(Stdio::from(
            std::fs::File::create(&stdout_path).expect("child stdout"),
        ))
        .stderr(Stdio::from(
            std::fs::File::create(&stderr_path).expect("child stderr"),
        ))
        .spawn()
        .expect("bounded probe-budget child");
    let start = Instant::now();
    let mut timed_out = false;
    while child.try_wait().expect("child status").is_none() {
        if start.elapsed() > Duration::from_secs(60) {
            timed_out = true;
            child.kill().expect("kill stuck probe-budget child");
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let status = child.wait().expect("reap probe-budget child");
    let stdout = std::fs::read_to_string(stdout_path).expect("child stdout log");
    let stderr = std::fs::read_to_string(stderr_path).expect("child stderr log");
    eprintln!(
        "probe-budget {case}: watchdog timeout={timed_out}, status={status}\n{stdout}\n{stderr}"
    );
    assert!(
        !timed_out && status.success(),
        "probe-budget {case}: timeout={timed_out}, {status}"
    );
    assert!(
        stdout.contains("1 passed; 0 failed"),
        "child must execute its oracle"
    );
    stdout
}
