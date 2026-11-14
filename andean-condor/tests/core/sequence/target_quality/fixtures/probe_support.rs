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
    let (completed, warnings) = TargetQuality::probe_pass_with_options(
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
