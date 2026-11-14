//! Real finite native setup; only produced_job publishes owning result stamps.
use super::condor;
#[cfg(unix)]
use condor::{
    core::sequence::{Sequence, parallel_encoder, quality_check, scene_concatenator},
    models::sequence::quality_check::QualityCheckConfig,
};
use condor::{
    core::{Condor, input::Input, output::Output},
    models::{
        encoder::{Encoder, EncoderBase},
        input::{ImportMethod, Input as InputModel},
        scene::Scene,
        sequence::{DefaultSequenceConfig, DefaultSequenceData, scene_concatenator::ConcatMethod},
    },
};
pub(super) fn fixture(
    root: &std::path::Path,
) -> Condor<DefaultSequenceData, DefaultSequenceConfig> {
    let source = root.join("source.mkv");
    let output = std::process::Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=size=128x128:rate=24",
            "-frames:v",
            "3",
            "-c:v",
            "ffv1",
            "-y",
        ])
        .arg(&source)
        .output()
        .expect("installed ffmpeg");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let input = Input::from_data(&InputModel::Video {
        path: source,
        import_method: ImportMethod::FFMS2 {
            index: None,
            cache_path: None,
        },
        filters: vec![],
    })
    .expect("native input");
    let encoder = Encoder::default_from_base(&EncoderBase::FFmpeg, false);
    let mut config = DefaultSequenceConfig::default();
    config.parallel_encoder.scenes_directory = root.join("scenes");
    config.scene_concatenator.scenes_directory = root.join("scenes");
    config.scene_concatenator.method = ConcatMethod::FFmpeg(Default::default());
    std::fs::create_dir_all(root.join("scenes/Scene Concatenator")).expect("scene dir");
    Condor::new(
        input,
        Output {
            path: root.join("final.mkv"),
            overwrite: true,
            tags: Default::default(),
            video_tags: Default::default(),
        },
        encoder.clone(),
        vec![Scene {
            start_frame: 0,
            end_frame: 3,
            sub_scenes: None,
            encoder,
            sequence_data: DefaultSequenceData::default(),
        }],
        Some(config),
        Box::new(|_| Ok(())),
    )
}
#[cfg(unix)]
pub(super) fn produced_job(
    root: &std::path::Path,
) -> Condor<DefaultSequenceData, DefaultSequenceConfig> {
    use condor::models::{
        encoder::cli_parameter::CLIParameter, sequence::target_quality::types::QualityMetric,
    };
    use std::os::unix::fs::PermissionsExt;
    let mut job = fixture(root);
    let wrapper = root.join("encoder");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nprintf 'encode\\n' >> '{}'\nexec '{}' \"$@\"\n",
            root.join("calls").display(),
            which::which("ffmpeg").expect("ffmpeg").display()
        ),
    )
    .expect("counter");
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).expect("executable");
    job.scenes[0].encoder.set_executable(Some(wrapper));
    job.scenes[0].encoder.parameters_mut().extend(CLIParameter::new_strings(
        "-",
        " ",
        &[("c:v", "libx264"), ("preset", "ultrafast")],
    ));
    job.scenes[0].encoder.set_quantizer(20.0);
    let (tx, _rx) = std::sync::mpsc::channel();
    parallel_encoder::ParallelEncoder::default()
        .execute_cancel(&mut job, tx, Default::default())
        .expect("actual successful certified chunk before corruption");
    let (tx, _rx) = std::sync::mpsc::channel();
    scene_concatenator::SceneConcatenator::default()
        .execute_cancel(&mut job, tx, Default::default())
        .expect("actual final output");
    job.sequence_config.quality_check = Some(QualityCheckConfig {
        metric: QualityMetric::VMAF {
            target_range: (0.0, 100.0),
            resolution: None,
            scaler: "bicubic".into(),
            threads: 0,
            model: None,
            features: vec![],
        },
        ..Default::default()
    });
    let (tx, _rx) = std::sync::mpsc::channel();
    quality_check::QualityCheck::default()
        .execute_cancel(&mut job, tx, Default::default())
        .expect("actual successful QC before corruption");
    assert!(job.scenes[0].sequence_data.parallel_encoder.provenance.is_some());
    assert_eq!(
        job.scenes[0].sequence_data.quality_check.quality.scores.len(),
        3
    );
    assert!(job.scenes[0].sequence_data.quality_check.quality.provenance.is_some());
    job
}
