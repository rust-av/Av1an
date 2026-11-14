use crate::common::condor_cmd;
use crate::common::config::{init_config, load};
use california_condor::{
    commands::handlers::load_configuration, test_helpers::*, utils::hash_path::hash_path,
};

#[cfg(test)]
mod tests {
    use andean_condor::models::{
        encoder::cli_parameter::CLIParameter,
        sequence::target_quality::{
            TargetQualityConfig,
            types::{
                ProbeStatistic, ProbeStrategy, QualityMetric, QualityPass, SubsetProbeLength,
                SubsetProbePosition, TargetQualityProbing,
            },
        },
    };
    use serial_test::serial;

    use super::*;

    #[test]
    fn with_custom_options() {
        if !ffmpeg_is_available() {
            return;
        }
        let test_video = get_test_video();
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let output = temp.path().join("out.mkv");
        let input_abs = path_abs::PathAbs::new(test_video.path.clone())
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let temp_abs = path_abs::PathAbs::new(temp.path().join(hash_path(&input_abs)))
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let config_path = temp.path().join("condor.json");

        // Mock an existing config file with scenes
        let mut config = default_config(&test_video, &output, &temp_abs);
        config.condor.scenes = test_video.mock_scenes(&config.condor.encoder);
        config.condor.sequence_config.target_quality = Some(TargetQualityConfig {
            metric: QualityMetric::SSIMULACRA2 {
                target_range: (79.0, 81.0),
                resolution: None,
                threads: None,
                gpu_id: None,
            },
            quantizer_range: (20, 40),
            input: None,
            probing: TargetQualityProbing {
                encoder_options: None,
                strategy: ProbeStrategy::Subset {
                    position: SubsetProbePosition::Start,
                    length: SubsetProbeLength::Frames(1),
                },
                statistic: ProbeStatistic::Mean,
            },
            ..Default::default()
        });
        config.condor.scenes.iter_mut().enumerate().for_each(|(index, scene)| {
            scene.encoder.parameters_mut().insert(
                "crf".to_owned(),
                CLIParameter::new_number("--", " ", if index == 0 { 20.0 } else { 40.0 }),
            );
            scene.sequence_data.target_quality.passes = vec![QualityPass {
                quantizer: if index == 0 { 20.0 } else { 40.0 },
                scores: vec![80.0],
                bitrate: if index == 0 { 10000.0 } else { 1000.0 },
                ..Default::default()
            }];
        });
        // Explicit current measurement/selection fixture: this test exercises the
        // optimizer policy, not migration or native measurement correctness.
        use andean_condor::{
            metrics::{self, Engine},
            models::sequence::provenance::{self, MetricBackend, ResultProvenance},
        };
        let tq = config.condor.sequence_config.target_quality.as_ref().expect("TQ");
        let pe = &config.condor.sequence_config.parallel_encoder;
        let input = provenance::probe_input(&config.condor.input, pe, Some(tq));
        let engine = metrics::engine(
            &tq.metric,
            !matches!(input, andean_condor::models::input::Input::Video { .. }),
        )
        .unwrap_or(Engine::VapourSynth);
        let backend = if engine == Engine::VapourSynth {
            MetricBackend::available_plugin(&tq.metric).expect("installed fixture plugin")
        } else {
            MetricBackend::current(engine)
        };
        for scene in &mut config.condor.scenes {
            let mut pass = scene.sequence_data.target_quality.passes[0].clone();
            pass.backend = Some(backend.clone());
            pass.provenance = Some(ResultProvenance::new(
                provenance::target_context(input, tq.metric_input.as_ref(), scene, tq, &pass)
                    .expect("current probe context"),
            ));
            scene.sequence_data.target_quality.passes[0] = pass;
            scene.sequence_data.target_quality.selection = Some(ResultProvenance::new(
                provenance::selection_context(scene, tq).expect("current selection context"),
            ));
        }
        config.save(&config_path).expect("configuration save should succeed");

        condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .args(["optimize-bitrate", "--sigma-threshold", "1"])
            .assert()
            .success();

        let mut expected_config = config.clone();
        expected_config.condor.sequence_config.bitrate_optimizer.bitrate_sigma_threshold = Some(1);
        let expected_crf = |index| match index {
            0 => 36.0,
            1 => 40.0,
            2 => 40.0,
            3 => 40.0,
            4 => 40.0,
            _ => 0.0,
        };
        expected_config.condor.scenes.iter_mut().enumerate().for_each(|(index, scene)| {
            scene.encoder.parameters_mut().insert(
                "crf".to_owned(),
                CLIParameter::new_number("--", " ", expected_crf(index)),
            );
        });
        // immutable shadow
        let expected_config = expected_config;

        let (config, _) =
            load_configuration(Some(&config_path)).expect("load_configuration should succeed");

        check_basic_config(&config, &expected_config);
        assert_eq!(
            config.condor.scenes.len(),
            test_video.scenes.len(),
            "scenes contains {} scenes",
            test_video.scenes.len()
        );
        config.condor.scenes.iter().enumerate().for_each(|(index, scene)| {
            assert_eq!(
                scene.encoder.parameters().get("crf").expect("crf should exist"),
                &CLIParameter::new_number("--", " ", expected_crf(index)),
                "scene {} should have crf {}",
                index,
                expected_crf(index)
            );
        });
    }

    /// `optimize-bitrate` owns an optional sigma threshold.
    #[serial]
    #[test]
    fn optimize_bitrate_set_and_reset() {
        if !ffmpeg_is_available() {
            return;
        }
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let config_path = init_config(&temp);

        condor_cmd(&temp)
            .args(["optimize-bitrate", "set", "sigma-threshold", "4"])
            .assert()
            .success();
        condor_cmd(&temp)
            .args(["optimize-bitrate", "set", "sigma-threshold", "0"])
            .assert()
            .failure();
        let optimizer = load(&config_path)
            .condor
            .sequence_config
            .bitrate_optimizer
            .bitrate_sigma_threshold;
        assert_eq!(optimizer, Some(4), "sigma threshold is set");

        condor_cmd(&temp)
            .args(["optimize-bitrate", "reset", "sigma-threshold"])
            .assert()
            .success();
        let optimizer = load(&config_path)
            .condor
            .sequence_config
            .bitrate_optimizer
            .bitrate_sigma_threshold;
        assert_eq!(optimizer, None, "sigma threshold is cleared");
    }
}
