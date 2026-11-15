#[cfg(test)]
mod handler_configuration {
    use std::{assert_matches, path::Path};

    use california_condor::{
        commands::handlers::start::start_handler, test_helpers::*, utils::hash_path::hash_path,
    };

    #[test]
    fn start_without_config_path_saves_condor_json_and_default_settings_in_current_directory() {
        crate::common::config::isolated_default_path_test(
            "commands::handlers::start::handler_configuration::start_without_config_path_saves_condor_json_and_default_settings_in_current_directory",
            start_creates_default_config,
        );
    }

    fn start_creates_default_config(temp: &Path) {
        let test_video = get_test_video();
        let input_abs = path_abs::PathAbs::new(&test_video.path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let temp_abs = path_abs::PathAbs::new(temp.join(hash_path(&input_abs)))
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let output = temp.join("out.mkv");
        let output_abs = path_abs::PathAbs::new(&output)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let config_path = temp.join("condor.json");
        let config_path_abs = path_abs::PathAbs::new(&config_path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let expected_config = default_config(&test_video, &output, &temp_abs);
        let (config, found_config_path) = start_handler(
            None,
            None,
            Some(&test_video.path),
            None,
            None,
            Some(&output),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            false,
        )
        .expect("start_handler should succeed");
        assert_eq!(
            found_config_path,
            config_path_abs,
            "config path is {}",
            config_path_abs.display()
        );
        assert_eq!(config.temp, temp_abs, "temp path is {}", temp_abs.display());
        assert_eq!(
            config.input,
            input_abs,
            "input path is {}",
            input_abs.display()
        );
        assert_eq!(
            config.condor.output.path,
            expected_config.condor.output.path,
            "output path is {}",
            output_abs.display()
        );
        check_input(
            Some(&config.condor.input),
            Some(&expected_config.condor.input),
            "input",
        );
        check_output(&config.condor.output, &expected_config.condor.output);
        assert!(config.condor.scenes.is_empty(), "scenes is empty");
        check_scene_detector(
            &config.condor.sequence_config.scene_detector,
            &expected_config.condor.sequence_config.scene_detector,
        );
        check_encoder(
            &config.condor.encoder,
            &expected_config.condor.encoder,
            "encoder",
        );
        check_benchmarker(
            &config.condor.sequence_config.benchmarker,
            &expected_config.condor.sequence_config.benchmarker,
        );
        check_noise_detector(
            config.condor.sequence_config.noise_detector.as_ref(),
            expected_config.condor.sequence_config.noise_detector.as_ref(),
        );
        check_noise_scaler(
            config.condor.sequence_config.noise_scaler.as_ref(),
            expected_config.condor.sequence_config.noise_scaler.as_ref(),
        );
        check_target_quality(
            config.condor.sequence_config.target_quality.as_ref(),
            expected_config.condor.sequence_config.target_quality.as_ref(),
        );
        check_bitrate_optimizer(
            &config.condor.sequence_config.bitrate_optimizer,
            &expected_config.condor.sequence_config.bitrate_optimizer,
        );
        check_speed_scaler(
            &config.condor.sequence_config.speed_scaler,
            &expected_config.condor.sequence_config.speed_scaler,
        );
        check_parallel_encoder(
            &config.condor.sequence_config.parallel_encoder,
            &expected_config.condor.sequence_config.parallel_encoder,
        );
        check_scene_concatenator(
            &config.condor.sequence_config.scene_concatenator,
            &expected_config.condor.sequence_config.scene_concatenator,
        );
    }

    #[test]
    fn start_disable_color_data_turns_appending_off() {
        let test_video = get_test_video();
        let temp = tempfile::tempdir().expect("temp directory");
        let output = temp.path().join("out.mkv");
        let config_path = temp.path().join("condor.json");
        let (config, _) = start_handler(
            Some(&config_path),
            None,
            Some(&test_video.path),
            None,
            None,
            Some(&output),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(false),
            None,
            None,
            None,
            None,
            None,
            None,
            false,
        )
        .expect("start_handler should succeed");
        assert!(
            !config.condor.encoder.append_color_data(),
            "append_color_data is false"
        );
    }

    #[test]
    fn start_missing_main_input_returns_error_without_creating_configuration() {
        let invalid_input = std::env::temp_dir().join("not_found.mkv");
        let input_abs = path_abs::PathAbs::new(&invalid_input)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let temp = tempfile::tempdir().expect("temp directory");
        let output = temp.path().join("out.mkv");
        let config_path = temp.path().join("condor.json");
        let result = start_handler(
            Some(&config_path),
            Some(&temp.path().join(hash_path(&input_abs))),
            Some(&invalid_input),
            None,
            None,
            Some(&output),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            false,
        );
        assert_matches!(result, Err(_), "start_handler should fail");
        assert!(!config_path.exists(), "config file does not exist");
    }
}
