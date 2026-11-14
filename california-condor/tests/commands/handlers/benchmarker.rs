use california_condor::{
    commands::handlers::load_configuration, test_helpers::*, utils::hash_path::hash_path,
};

use crate::common::condor_cmd;
use crate::common::config::{init_config, load};

#[cfg(test)]
mod tests {
    use andean_condor::{
        ffmpeg::FFPixelFormat,
        models::{
            encoder::cli_parameter::CLIParameter, input::Input,
            sequence::benchmarker::BenchmarkerConfig,
        },
        vapoursynth::{plugins::resize::Scaler, vapoursynth_filters::VapourSynthFilter},
    };
    use serial_test::serial;

    use super::*;

    /// Downscaling the clip to 540p cuts the benchmarker's encode cost roughly
    /// fourfold, and nothing here depends on the source resolution.
    const WIDTH: usize = 960;
    const HEIGHT: usize = 540;

    #[serial]
    #[test]
    fn default_and_no_scenes() {
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

        condor_cmd(&temp)
            .args(["init", path_str(&test_video.path), path_str(&output)])
            .assert()
            .success();

        condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .args(["benchmark"])
            .assert()
            .success();

        let expected_config = default_config(&test_video, &output, &temp_abs);

        let (config, _) = load_configuration(Some(&config_path)).expect("config should load");

        check_basic_config(&config, &expected_config);
    }

    #[serial]
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
        if let Input::VapourSynth { filters, .. } = &mut config.condor.input {
            *filters = vec![VapourSynthFilter::Resize {
                scaler: Some(Scaler::Bicubic),
                width: Some(WIDTH),
                height: Some(HEIGHT),
                format: Some(FFPixelFormat::YUV420P10LE),
            }];
        }
        config.condor.encoder.parameters_mut().insert(
            "preset".to_owned(),
            CLIParameter::new_number("--", " ", 10.0),
        );
        config.condor.scenes = test_video.mock_scenes(&config.condor.encoder);
        config.save(&config_path).expect("configuration save should succeed");

        condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .args(["benchmark", "--threshold", "100"])
            .assert()
            .success();

        let mut expected_config = config;
        expected_config.condor.sequence_config.benchmarker.threshold = 100;

        let (config, _) =
            load_configuration(Some(&config_path)).expect("load_configuration should succeed");

        // RATIONALE: Worker selection depends on measured throughput, not the threshold alone.
        let workers = config
            .condor
            .sequence_config
            .parallel_encoder
            .workers
            .expect("benchmark should select a worker count");
        assert!(
            workers > 0,
            "benchmark should select a positive worker count"
        );
        expected_config.condor.sequence_config.parallel_encoder.workers = Some(workers);
        // immutable shadow
        let expected_config = expected_config;

        check_basic_config(&config, &expected_config);
        assert_eq!(
            config.condor.scenes.len(),
            test_video.scenes.len(),
            "scenes contains {} scenes",
            test_video.scenes.len()
        );
    }

    #[serial]
    #[test]
    fn with_workers_preconfigured() {
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
        config.condor.encoder.parameters_mut().insert(
            "preset".to_owned(),
            CLIParameter::new_number("--", " ", 10.0),
        );
        config.condor.scenes = test_video.mock_scenes(&config.condor.encoder);
        config.condor.sequence_config.parallel_encoder.workers = Some(10);
        config.save(&config_path).expect("configuration save should succeed");

        // Should skip when workers is already set
        condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .args(["benchmark", "--threshold", "100"])
            .assert()
            .success();

        let mut expected_config = config;
        expected_config.condor.sequence_config.benchmarker.threshold = 100;
        // immutable shadow
        let expected_config = expected_config;

        let (config, _) =
            load_configuration(Some(&config_path)).expect("load_configuration should succeed");

        check_basic_config(&config, &expected_config);
    }

    /// `benchmark` writes scalars and restores defaults per key.
    #[serial]
    #[test]
    fn benchmark_set_and_reset() {
        if !ffmpeg_is_available() {
            return;
        }
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let config_path = init_config(&temp);

        condor_cmd(&temp)
            .args(["benchmark", "set", "threshold", "8"])
            .assert()
            .success();
        condor_cmd(&temp)
            .args(["benchmark", "set", "max-memory", "8192"])
            .assert()
            .success();
        // Out of the documented 0..=100 range.
        condor_cmd(&temp)
            .args(["benchmark", "set", "threshold", "101"])
            .assert()
            .failure();

        let benchmarker = load(&config_path).condor.sequence_config.benchmarker;
        assert_eq!(benchmarker.threshold, 8, "threshold is set");
        assert_eq!(benchmarker.max_memory, Some(8192), "max memory is set");

        condor_cmd(&temp).args(["benchmark", "reset", "max-memory"]).assert().success();
        condor_cmd(&temp).args(["benchmark", "reset", "threshold"]).assert().success();
        let benchmarker = load(&config_path).condor.sequence_config.benchmarker;
        let defaults = BenchmarkerConfig::default();
        assert_eq!(
            benchmarker.threshold, defaults.threshold,
            "reset restores the default threshold"
        );
        assert_eq!(
            benchmarker.max_memory, defaults.max_memory,
            "reset clears the max memory"
        );
    }
}
