use crate::common::condor_cmd;
use crate::common::config::{init_config, load};
use andean_condor::models::sequence::speed_scaler::SpeedScalerConfig;
use california_condor::{
    commands::handlers::load_configuration, test_helpers::*, utils::hash_path::hash_path,
};

#[cfg(test)]
mod handler_configuration {
    use super::*;
    use california_condor::commands::handlers::{
        init::init_handler,
        scale_speed::{SpeedScalerError, scale_speed_handler},
    };
    use std::assert_matches;

    #[test]
    fn scale_speed_handler_without_overrides_retains_default_pairs_and_config_path() {
        let test_video = get_test_video();
        let input_abs = path_abs::PathAbs::new(&test_video.path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let temp = tempfile::tempdir().expect("temp directory");
        let temp_abs = path_abs::PathAbs::new(temp.path().join(hash_path(&input_abs)))
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let output = temp.path().join("out.mkv");
        let config_path = temp.path().join("condor.json");
        let config_path_abs = path_abs::PathAbs::new(&config_path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let expected_config = default_config(&test_video, &output, &temp_abs);
        init_handler(
            Some(&config_path),
            Some(&temp.path().join(hash_path(&input_abs))),
            &test_video.path,
            &output,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            true,
            None,
            None,
            None,
            false,
        )
        .expect("init_handler should succeed");
        let (config, found_config_path) = scale_speed_handler(Some(&config_path), None, None)
            .expect("scale_speed_handler should succeed");
        assert_eq!(
            found_config_path,
            config_path_abs,
            "config path is {}",
            config_path_abs.display()
        );
        check_basic_config(&config, &expected_config);
        assert!(config.condor.scenes.is_empty(), "scenes is empty");
    }

    #[test]
    fn scale_speed_handler_saves_requested_quantizer_speed_pairs_and_other_checked_settings() {
        let test_video = get_test_video();
        let input_abs = path_abs::PathAbs::new(&test_video.path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let temp = tempfile::tempdir().expect("temp directory");
        let temp_abs = path_abs::PathAbs::new(temp.path().join(hash_path(&input_abs)))
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let output = temp.path().join("out.mkv");
        let config_path = temp.path().join("condor.json");
        let config_path_abs = path_abs::PathAbs::new(&config_path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let mut expected_config = default_config(&test_video, &output, &temp_abs);
        expected_config.condor.sequence_config.speed_scaler = SpeedScalerConfig {
            speed_quantizers: vec![(8, 10.0), (5, 25.0), (3, 35.0)],
        };
        let expected_config = expected_config;
        init_handler(
            Some(&config_path),
            Some(&temp.path().join(hash_path(&input_abs))),
            &test_video.path,
            &output,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            true,
            None,
            None,
            None,
            false,
        )
        .expect("init_handler should succeed");
        let (config, found_config_path) = scale_speed_handler(
            Some(&config_path),
            Some(&[10_i8, 25, 35]),
            Some(&[8_i8, 5, 3]),
        )
        .expect("scale_speed_handler should succeed");
        assert_eq!(
            found_config_path,
            config_path_abs,
            "config path is {}",
            config_path_abs.display()
        );
        check_basic_config(&config, &expected_config);
        assert!(config.condor.scenes.is_empty(), "scenes is empty");
    }

    #[test]
    fn scale_speed_handler_unequal_pair_counts_returns_typed_error_and_preserves_checked_saved_fields()
     {
        let test_video = get_test_video();
        let input_abs = path_abs::PathAbs::new(&test_video.path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let temp = tempfile::tempdir().expect("temp directory");
        let temp_abs = path_abs::PathAbs::new(temp.path().join(hash_path(&input_abs)))
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let output = temp.path().join("out.mkv");
        let config_path = temp.path().join("condor.json");
        let config_path_abs = path_abs::PathAbs::new(&config_path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let expected_config = default_config(&test_video, &output, &temp_abs);
        init_handler(
            Some(&config_path),
            Some(&temp.path().join(hash_path(&input_abs))),
            &test_video.path,
            &output,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            true,
            None,
            None,
            None,
            false,
        )
        .expect("init_handler should succeed");
        let result =
            scale_speed_handler(Some(&config_path), Some(&[8_i8, 5, 3]), Some(&[10_i8, 25]));
        let error = result
            .expect_err("scale_speed_handler should fail")
            .downcast::<SpeedScalerError>()
            .expect("scale_speed_handler error should be SpeedScalerError");
        assert_matches!(
            error,
            SpeedScalerError::MismatchedPairs {
                quantizers: 3,
                speeds: 2
            }
        );
        let (config, found_config_path) =
            load_configuration(Some(&config_path)).expect("load_configuration should succeed");
        assert_eq!(
            found_config_path,
            config_path_abs,
            "config path is {}",
            config_path_abs.display()
        );
        check_basic_config(&config, &expected_config);
        assert!(config.condor.scenes.is_empty(), "scenes is empty");
    }

    #[test]
    fn scale_speed_handler_fewer_than_two_pairs_returns_typed_error_and_preserves_checked_saved_fields()
     {
        let test_video = get_test_video();
        let input_abs = path_abs::PathAbs::new(&test_video.path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let temp = tempfile::tempdir().expect("temp directory");
        let temp_abs = path_abs::PathAbs::new(temp.path().join(hash_path(&input_abs)))
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let output = temp.path().join("out.mkv");
        let config_path = temp.path().join("condor.json");
        let config_path_abs = path_abs::PathAbs::new(&config_path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let expected_config = default_config(&test_video, &output, &temp_abs);
        init_handler(
            Some(&config_path),
            Some(&temp.path().join(hash_path(&input_abs))),
            &test_video.path,
            &output,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            true,
            None,
            None,
            None,
            false,
        )
        .expect("init_handler should succeed");
        let result = scale_speed_handler(Some(&config_path), Some(&[8_i8]), Some(&[10_i8]));
        let error = result
            .expect_err("scale_speed_handler should fail")
            .downcast::<SpeedScalerError>()
            .expect("scale_speed_handler error should be SpeedScalerError");
        assert_matches!(error, SpeedScalerError::MinimumPairsRequired);
        let (config, found_config_path) =
            load_configuration(Some(&config_path)).expect("load_configuration should succeed");
        assert_eq!(
            found_config_path,
            config_path_abs,
            "config path is {}",
            config_path_abs.display()
        );
        check_basic_config(&config, &expected_config);
        assert!(config.condor.scenes.is_empty(), "scenes is empty");
    }
}

#[cfg(test)]
mod tests {
    use serial_test::serial;

    use super::*;

    fn speed_scaler_configuration(
        temp: &std::path::Path,
    ) -> california_condor::configuration::Configuration {
        let video = TestVideo {
            path: temp.join("must-not-open.mkv"),
            width: 2,
            height: 2,
            frames: 2,
            fps_rational: (24, 1),
            format: andean_condor::ffmpeg::FFPixelFormat::YUV420P,
            scenes: vec![(0, 2)],
        };
        let mut configuration =
            default_config(&video, &temp.join("output.mkv"), &temp.join("scratch"));
        configuration.condor.encoder.set_executable(Some(temp.join("must-not-run")));
        configuration.condor.scenes = video.mock_scenes(&configuration.condor.encoder);
        configuration.condor.sequence_config.speed_scaler.speed_quantizers =
            vec![(4, 20.25), (8, 40.75)];
        configuration
    }

    #[test]
    fn flags_retain_loaded_fractional_quantizers_on_speed_only_edits() {
        use california_condor::commands::handlers::scale_speed::{
            configure_scale_speed, scale_speed_handler,
        };
        let temp = tempfile::tempdir().expect("fixture");
        let path = temp.path().join("condor.json");
        let mut config = speed_scaler_configuration(temp.path());
        config.condor.sequence_config.speed_scaler.speed_quantizers.push((6, 200.5));
        config.save(&path).expect("save");
        let (config, _) = scale_speed_handler(Some(&path), None, Some(&[5, 7, 9]))
            .expect("offline speed-only edit");
        assert_eq!(
            config.condor.sequence_config.speed_scaler.speed_quantizers,
            vec![(5, 20.25), (7, 40.75), (9, 200.5)]
        );
        assert_eq!(
            load(&path).condor.sequence_config.speed_scaler.speed_quantizers,
            vec![(5, 20.25), (7, 40.75), (9, 200.5)]
        );
        let mut config = config;
        configure_scale_speed(&mut config, Some(&[10, 20, 30]), None).expect("quantizer-only edit");
        assert_eq!(
            config.condor.sequence_config.speed_scaler.speed_quantizers,
            vec![(5, 10.0), (7, 20.0), (9, 30.0)]
        );
        assert!(!temp.path().join("must-not-open.mkv").exists());
        assert!(!temp.path().join("scratch").exists());
    }

    #[test]
    fn flags_and_crud_numeric_errors_preserve_memory_and_saved_bytes_offline() {
        use andean_condor::models::encoder::{Encoder, EncoderBase};
        use california_condor::commands::{
            config::ConfigCommand,
            handlers::scale_speed::{
                ScaleSpeedCrud, ScaleSpeedSet, configure_scale_speed, scale_speed_handler,
            },
        };
        let temp = tempfile::tempdir().expect("fixture");
        let path = temp.path().join("condor.json");
        for (base, bad) in [(EncoderBase::RAV1E, 11), (EncoderBase::SVTAV1, 14)] {
            let mut config = speed_scaler_configuration(temp.path());
            config.condor.encoder = Encoder::default_from_base(&base, false);
            config.condor.encoder.set_executable(Some(temp.path().join("must-not-run")));
            config.condor.scenes[0].encoder = config.condor.encoder.clone();
            config.save(&path).expect("save");
            let before = serde_json::to_vec(&config).expect("snapshot");
            let bytes = std::fs::read(&path).expect("saved bytes");
            assert!(configure_scale_speed(&mut config, Some(&[20, 40]), Some(&[4, bad])).is_err());
            assert_eq!(serde_json::to_vec(&config).expect("config"), before);
            assert!(
                ScaleSpeedCrud::apply(
                    &mut config,
                    &ScaleSpeedSet::SpeedQuantizers {
                        points: vec![(20, 4), (40, bad)]
                    }
                )
                .is_err()
            );
            assert_eq!(serde_json::to_vec(&config).expect("config"), before);
            assert!(scale_speed_handler(Some(&path), Some(&[20, 40]), Some(&[4, bad])).is_err());
            assert_eq!(std::fs::read(&path).expect("saved bytes"), bytes);
            let bad = bad.to_string();
            condor_cmd(&temp)
                .env("PATH", temp.path())
                .args(["scale-speed", "--quantizers", "20", "40", "--speeds", "4", &bad])
                .assert()
                .failure();
            assert_eq!(std::fs::read(&path).expect("saved bytes"), bytes);
            let point = format!("40={bad}");
            condor_cmd(&temp)
                .env("PATH", temp.path())
                .args(["scale-speed", "set", "speed-quantizers", "20=4", &point])
                .assert()
                .failure();
            assert_eq!(std::fs::read(&path).expect("saved bytes"), bytes);
        }
        assert!(!temp.path().join("scratch").exists());
    }

    #[test]
    fn actual_flags_duplicate_rejection_and_crud_repair_are_offline_and_atomic() {
        use california_condor::commands::handlers::scale_speed::scale_speed_handler;
        let temp = tempfile::tempdir().expect("fixture");
        let path = temp.path().join("condor.json");
        let mut config = speed_scaler_configuration(temp.path());
        config.save(&path).expect("save");
        let before = std::fs::read(&path).expect("bytes");
        condor_cmd(&temp)
            .env("PATH", temp.path())
            .args(["scale-speed", "--quantizers", "20", "20", "--speeds", "4", "8"])
            .assert()
            .failure();
        assert_eq!(std::fs::read(&path).expect("bytes"), before);
        condor_cmd(&temp)
            .env("PATH", temp.path())
            .args(["scale-speed", "set", "speed-quantizers", "20=4", "20=8"])
            .assert()
            .failure();
        assert_eq!(std::fs::read(&path).expect("bytes"), before);
        config.condor.sequence_config.speed_scaler.speed_quantizers = vec![(4, 20.0), (8, 20.0)];
        config.save(&path).expect("bad loaded config");
        let (repaired, _) = scale_speed_handler(Some(&path), Some(&[20, 40]), Some(&[4, 8]))
            .expect("offline complete flags repair");
        assert_eq!(
            repaired.condor.sequence_config.speed_scaler.speed_quantizers,
            vec![(4, 20.0), (8, 40.0)]
        );
        config.save(&path).expect("bad loaded config again");
        condor_cmd(&temp)
            .env("PATH", temp.path())
            .args(["scale-speed", "set", "speed-quantizers", "40=8", "20=4"])
            .assert()
            .success();
        assert_eq!(
            load(&path).condor.sequence_config.speed_scaler.speed_quantizers,
            vec![(4, 20.0), (8, 40.0)]
        );
        assert!(!temp.path().join("scratch").exists());
    }

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
            .args(["scale-speed"])
            .assert()
            .success();

        let expected_config = default_config(&test_video, &output, &temp_abs);

        let (config, _) = load_configuration(Some(&config_path)).expect("config should load");

        check_basic_config(&config, &expected_config);
    }

    #[serial]
    #[test]
    fn with_pairs_and_no_scenes() {
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

        let mut expected_config = default_config(&test_video, &output, &temp_abs);
        expected_config.condor.sequence_config.speed_scaler = SpeedScalerConfig {
            speed_quantizers: vec![(6, 20.0), (4, 35.0), (2, 50.0)],
        };
        let expected_config = expected_config;

        condor_cmd(&temp)
            .args(["init", path_str(&test_video.path), path_str(&output)])
            .assert()
            .success();

        condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .args([
                "scale-speed",
                "--quantizers",
                "20",
                "--speeds",
                "6",
                "--quantizers",
                "35",
                "--speeds",
                "4",
                "--quantizers",
                "50",
                "--speeds",
                "2",
            ])
            .assert()
            .success();

        let (config, _) = load_configuration(Some(&config_path)).expect("config should load");

        check_basic_config(&config, &expected_config);
    }

    #[serial]
    #[test]
    fn with_pairs_and_scenes() {
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

        let mut expected_config = default_config(&test_video, &output, &temp_abs);
        expected_config.condor.scenes = test_video.mock_scenes(&expected_config.condor.encoder);
        expected_config.condor.sequence_config.speed_scaler = SpeedScalerConfig {
            speed_quantizers: vec![(8, 10.0), (5, 25.0), (3, 35.0)],
        };
        let expected_config = expected_config;

        condor_cmd(&temp)
            .args(["init", path_str(&test_video.path), path_str(&output)])
            .assert()
            .success();

        {
            let (mut config, _) =
                load_configuration(Some(&config_path)).expect("load_config should succeed");
            config.condor.scenes = test_video.mock_scenes(&config.condor.encoder);
            config.save(&config_path).expect("save should succeed");
        }

        condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .args([
                "scale-speed",
                "--quantizers",
                "10",
                "--speeds",
                "8",
                "--quantizers",
                "25",
                "--speeds",
                "5",
                "--quantizers",
                "35",
                "--speeds",
                "3",
            ])
            .assert()
            .success();

        let (config, _) = load_configuration(Some(&config_path)).expect("config should load");

        check_basic_config(&config, &expected_config);
        assert_eq!(
            config.condor.scenes.len(),
            test_video.scenes.len(),
            "scene count matches"
        );
    }

    /// `scale-speed` stores speed first, quantizer second, sorted by
    /// quantizer, and refuses fewer than two pairs.
    #[serial]
    #[test]
    fn scale_speed_set_points_store_speed_first() {
        if !ffmpeg_is_available() {
            return;
        }
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let config_path = init_config(&temp);

        condor_cmd(&temp)
            .args(["scale-speed", "set", "speed-quantizers", "50=2", "20=6"])
            .assert()
            .success();
        let scaler = load(&config_path).condor.sequence_config.speed_scaler;
        assert_eq!(
            scaler.speed_quantizers,
            vec![(6, 20.0), (2, 50.0)],
            "pairs are sorted by quantizer and stored speed-first"
        );

        condor_cmd(&temp)
            .args(["scale-speed", "set", "speed-quantizers", "20=6"])
            .assert()
            .failure();
        condor_cmd(&temp)
            .args(["scale-speed", "set", "speed-quantizers", "20=6", "20=4"])
            .assert()
            .failure();

        condor_cmd(&temp)
            .args(["scale-speed", "reset", "speed-quantizers"])
            .assert()
            .success();
        let scaler = load(&config_path).condor.sequence_config.speed_scaler;
        assert!(scaler.speed_quantizers.is_empty(), "pairs were cleared");
    }
}
