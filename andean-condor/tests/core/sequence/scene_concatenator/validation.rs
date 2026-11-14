use super::*;

#[test]
fn final_validation_rejects_source_alias() {
    let dir = tempfile::tempdir().expect("fixture");
    let source = dir.path().join("source.mkv");
    std::fs::write(&source, b"source sentinel").expect("source");
    let mut job = concat_job(dir.path(), &source);
    job.sequence_config.scene_detector.input = Some(InputModel::VapourSynthScript {
        source: VapourSynthScriptSource::Path(source.clone()),
        variables: Default::default(),
        index: 0,
        filters: vec![],
        stream_concurrently: false,
    });
    assert!(
        SceneConcatenator::default().validate(&mut job).is_err(),
        "declared source must not be admitted as output"
    );
    assert_eq!(std::fs::read(source).expect("source"), b"source sentinel");
}

#[test]
fn final_validation_rejects_every_declared_source_and_existing_destination() {
    use andean_condor::models::sequence::{
        noise_detector::NoiseDetectorConfig, quality_check::QualityCheckConfig,
        target_quality::TargetQualityConfig,
    };
    for slot in [
        "root", "scd", "parallel", "noise", "tq", "metric", "qc", "chunk", "override", "existing",
    ] {
        let dir = tempfile::tempdir().expect("fixture");
        let source = dir.path().join("source");
        std::fs::write(&source, b"protected source").expect("source");
        let mut job = concat_job(dir.path(), &source);
        let input = InputModel::VapourSynthScript {
            source: VapourSynthScriptSource::Path(source.clone()),
            variables: Default::default(),
            index: 0,
            filters: vec![],
            stream_concurrently: false,
        };
        let config = &mut job.sequence_config;
        match slot {
            "root" => {
                if let Input::VapourSynthScript { source: root, .. } = &mut job.input {
                    *root = VapourSynthScriptSource::Path(source.clone());
                }
            },
            "scd" => config.scene_detector.input = Some(input),
            "parallel" => config.parallel_encoder.input = Some(input),
            "noise" => {
                config.noise_detector = Some(NoiseDetectorConfig {
                    input: Some(input),
                    ..Default::default()
                });
            },
            "tq" => {
                config.target_quality = Some(TargetQualityConfig {
                    input: Some(input),
                    ..Default::default()
                });
            },
            "metric" => {
                config.target_quality = Some(TargetQualityConfig {
                    metric_input: Some(input),
                    ..Default::default()
                });
            },
            "qc" => {
                config.quality_check = Some(QualityCheckConfig {
                    input: Some(input),
                    ..Default::default()
                });
            },
            "chunk" => {
                job.scenes.push(Scene {
                    start_frame: 0,
                    end_frame: 2,
                    sub_scenes: None,
                    encoder: Default::default(),
                    sequence_data: Default::default(),
                });
                job.output.path = dir.path().join(format!(
                    "{}.{}",
                    ParallelEncoder::scene_id(0),
                    job.scenes[0].encoder.output_extension()
                ));
            },
            "override" => {
                config.scene_detector.input = Some(input);
                config.scene_concatenator.output = Some(source.clone());
                job.output.path = dir.path().join("fallback.ivf");
            },
            "existing" => job.output.overwrite = false,
            _ => unreachable!(),
        }
        assert!(
            SceneConcatenator::default().validate(&mut job).is_err(),
            "{slot}"
        );
        assert_eq!(std::fs::read(&source).expect("source"), b"protected source");
    }
}

#[test]
fn failed_final_concat_preserves_old_output() {
    let dir = tempfile::tempdir().expect("fixture");
    let output = dir.path().join("out.ivf");
    std::fs::write(&output, b"old final sentinel").expect("old final");
    let mut job = concat_job(dir.path(), &output);
    job.scenes.push(Scene {
        start_frame: 0,
        end_frame: 2,
        sub_scenes: None,
        encoder: Default::default(),
        sequence_data: Default::default(),
    });
    let chunk = dir.path().join(format!(
        "{}.{}",
        ParallelEncoder::scene_id(0),
        job.scenes[0].encoder.output_extension()
    ));
    std::fs::write(chunk, b"invalid IVF header").expect("invalid chunk");
    let (tx, rx) = sync::mpsc::channel();
    assert!(
        SceneConcatenator::default()
            .execute_cancel(&mut job, tx, Cancellation::default())
            .is_err()
    );
    assert_eq!(
        std::fs::read(&output).expect("old final"),
        b"old final sentinel"
    );
    assert!(
        !rx.try_iter()
            .any(|status| matches!(status, SequenceStatus::Whole(Status::Completed { .. })))
    );
}
