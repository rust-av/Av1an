use crate::common::condor_cmd;
use crate::common::config::{init_config, load};
use california_condor::{
    commands::handlers::load_configuration, test_helpers::*, utils::hash_path::hash_path,
};

#[cfg(test)]
mod handler_configuration {
    use super::*;
    use andean_condor::{
        models::{
            input::{Input, VapourSynthImportMethod},
            sequence::scene_detector::{
                SceneDetectionMethod as SceneDetectionMethodModel, ScenecutMethod,
            },
        },
        vapoursynth::{plugins::resize::Scaler, vapoursynth_filters::VapourSynthFilter},
    };
    use california_condor::commands::{
        DecoderMethod, SceneDetectionMethod,
        handlers::{detect_scenes::detect_scenes_handler, init::init_handler},
    };

    #[test]
    fn detect_scenes_handler_without_overrides_retains_default_method_lengths_and_config_path() {
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
        let (config, found_config_path) = detect_scenes_handler(
            Some(&config_path),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("detect_scenes_handler should succeed");
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
    fn detect_scenes_handler_applies_decoder_filters_fast_method_and_frame_rate_scaled_bounds() {
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
        let custom_filters = vec![VapourSynthFilter::Resize {
            scaler: Some(Scaler::Point),
            width: Some(960),
            height: Some(540),
            format: None,
        }];
        let min_scene_seconds = 2usize;
        let max_scene_seconds = 12usize;
        let expected_min = (test_video.fps() * min_scene_seconds as f64).round() as usize;
        let expected_max = (test_video.fps() * max_scene_seconds as f64).round() as usize;
        let mut expected_config = default_config(&test_video, &output, &temp_abs);
        expected_config.condor.sequence_config.scene_detector.input = Some(Input::VapourSynth {
            path: input_abs.clone(),
            import_method: VapourSynthImportMethod::FFMS2 { index: None },
            cache_path: None,
            filters: custom_filters.clone(),
        });
        expected_config.condor.sequence_config.scene_detector.method =
            SceneDetectionMethodModel::AVSceneChange {
                minimum_length: expected_min,
                maximum_length: expected_max,
                method: ScenecutMethod::Fast,
                save_scores: false,
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
        let (config, found_config_path) = detect_scenes_handler(
            Some(&config_path),
            Some(&temp.path().join(hash_path(&input_abs))),
            Some(&test_video.path),
            Some(&DecoderMethod::VSFFMS2),
            Some(&custom_filters),
            Some(&["method=scd".to_owned()]),
            Some(&SceneDetectionMethod::Fast),
            Some(min_scene_seconds),
            Some(max_scene_seconds),
        )
        .expect("detect_scenes_handler should succeed");
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
    fn detect_scenes_filters_only_do_not_touch_main_input() {
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
        let custom_filters = vec![VapourSynthFilter::Resize {
            scaler: Some(Scaler::Point),
            width: Some(960),
            height: Some(540),
            format: None,
        }];
        let mut expected_config = default_config(&test_video, &output, &temp_abs);
        expected_config.condor.sequence_config.scene_detector.input = Some(Input::VapourSynth {
            path: input_abs.clone(),
            import_method: VapourSynthImportMethod::BestSource { index: None },
            cache_path: None,
            filters: custom_filters.clone(),
        });
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
        let (config, _) = detect_scenes_handler(
            Some(&config_path),
            None,
            None,
            None,
            Some(&custom_filters),
            None,
            None,
            None,
            None,
        )
        .expect("detect_scenes_handler should succeed");
        check_basic_config(&config, &expected_config);
        let scd_input = config
            .condor
            .sequence_config
            .scene_detector
            .input
            .as_ref()
            .expect("Scene Detector input is Some");
        assert_eq!(
            scd_input.vapoursynth_filters(),
            custom_filters,
            "Scene Detector input has the requested filters"
        );
        assert_eq!(
            match scd_input {
                Input::VapourSynth { path, .. } | Input::Video { path, .. } => path,
                Input::VapourSynthScript { .. } => panic!("Scene Detector input is a script"),
            },
            &input_abs,
            "Scene Detector input inherits the main input's path"
        );
    }
}

#[cfg(test)]
mod tests {
    use andean_condor::{
        models::{
            input::{ImportMethod, Input, VapourSynthImportMethod},
            sequence::scene_detector::{
                SceneDetectionMethod, SceneDetectionMethod as CoreMethod, ScenecutMethod,
            },
        },
        vapoursynth::vapoursynth_filters::VapourSynthFilter,
    };
    use serial_test::serial;

    use super::*;

    #[serial]
    #[test]
    fn detect_scenes_default_method_saves_expected_boundaries_and_global_encoder_copies() {
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
            .args(["detect-scenes"])
            .assert()
            .success();

        let expected_config = default_config(&test_video, &output, &temp_abs);

        let (config, _) =
            load_configuration(Some(&config_path)).expect("load_configuration should succeed");

        check_basic_config(&config, &expected_config);
        assert_eq!(
            config.condor.scenes.len(),
            test_video.scenes.len(),
            "scenes contains {} scenes",
            test_video.scenes.len()
        );
        let scene_boundaries = config
            .condor
            .scenes
            .iter()
            .map(|scene| (scene.start_frame, scene.end_frame))
            .collect::<Vec<_>>();
        assert_eq!(
            scene_boundaries, test_video.scenes,
            "scene start and end frames are correct"
        );
        config.condor.scenes.iter().enumerate().for_each(|(index, scene)| {
            check_encoder(
                &scene.encoder,
                &expected_config.condor.encoder,
                &format!("scene {} encoder", index),
            );
        });
    }

    #[serial]
    #[test]
    fn detect_scenes_resized_input_override_preserves_expected_scene_boundaries() {
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
            .args([
                "detect-scenes",
                "--input",
                path_str(&test_video.path),
                "--decoder",
                "vs-ffms2",
                "--filters",
                "resize:width=960;height=540;",
            ])
            .assert()
            .success();

        let mut expected_config = default_config(&test_video, &output, &temp_abs);
        expected_config.condor.sequence_config.scene_detector.input = Some(Input::VapourSynth {
            path: input_abs,
            import_method: VapourSynthImportMethod::FFMS2 { index: None },
            cache_path: None,
            filters: vec![VapourSynthFilter::Resize {
                scaler: None,
                width: Some(960),
                height: Some(540),
                format: None,
            }],
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
        let scene_boundaries = config
            .condor
            .scenes
            .iter()
            .map(|scene| (scene.start_frame, scene.end_frame))
            .collect::<Vec<_>>();
        assert_eq!(
            scene_boundaries, test_video.scenes,
            "scene start and end frames are correct"
        );
        config.condor.scenes.iter().enumerate().for_each(|(index, scene)| {
            check_encoder(
                &scene.encoder,
                &expected_config.condor.encoder,
                &format!("scene {} encoder", index),
            );
        });
    }

    #[serial]
    #[test]
    fn detect_scenes_fast_two_second_limit_partitions_filtered_clip_every_forty_eight_frames() {
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
            .args(["detect-scenes", "--method", "fast", "--max-scene-seconds", "2"])
            .assert()
            .success();

        let mut expected_config = default_config(&test_video, &output, &temp_abs);
        expected_config.condor.sequence_config.scene_detector.method =
            SceneDetectionMethod::AVSceneChange {
                minimum_length: test_video.fps().round() as usize,
                maximum_length: (test_video.fps() * 2.0).round() as usize,
                method: ScenecutMethod::Fast,
                save_scores: false,
            };
        // immutable shadow
        let expected_config = expected_config;
        // The scene detector runs on the main input, which `init` gave a 10-bit
        // resize. That conversion no longer exposes the source's cuts, so the
        // two-second maximum length alone decides every boundary.
        let expected_boundaries = [
            (0, 48),
            (48, 96),
            (96, 144),
            (144, 192),
            (192, 240),
            (240, 288),
            (288, 336),
            (336, 384),
            (384, 432),
            (432, 480),
            (480, 528),
            (528, 576),
            (576, 624),
            (624, 672),
            (672, 720),
        ];

        let (config, _) =
            load_configuration(Some(&config_path)).expect("load_configuration should succeed");

        check_basic_config(&config, &expected_config);

        let scene_boundaries = config
            .condor
            .scenes
            .iter()
            .map(|scene| (scene.start_frame, scene.end_frame))
            .collect::<Vec<_>>();
        assert_eq!(
            scene_boundaries, expected_boundaries,
            "scene start and end frames are correct"
        );
        config.condor.scenes.iter().enumerate().for_each(|(index, scene)| {
            check_encoder(
                &scene.encoder,
                &expected_config.condor.encoder,
                &format!("scene {} encoder", index),
            );
        });
    }

    #[serial]
    #[test]
    fn detect_scenes_none_method_partitions_only_at_ten_second_limit() {
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
            .args(["detect-scenes", "--method", "none"])
            .assert()
            .success();

        let mut expected_config = default_config(&test_video, &output, &temp_abs);
        expected_config.condor.sequence_config.scene_detector.method = SceneDetectionMethod::None {
            minimum_length: test_video.fps().round() as usize,
            maximum_length: (test_video.fps() * 10.0).round() as usize,
        };
        // immutable shadow
        let expected_config = expected_config;
        let expected_boundaries = [(0, 240), (240, 480), (480, 720)];

        let (config, _) =
            load_configuration(Some(&config_path)).expect("load_configuration should succeed");

        check_basic_config(&config, &expected_config);

        let scene_boundaries = config
            .condor
            .scenes
            .iter()
            .map(|scene| (scene.start_frame, scene.end_frame))
            .collect::<Vec<_>>();
        assert_eq!(
            scene_boundaries, expected_boundaries,
            "scene start and end frames are correct"
        );
        config.condor.scenes.iter().enumerate().for_each(|(index, scene)| {
            check_encoder(
                &scene.encoder,
                &expected_config.condor.encoder,
                &format!("scene {} encoder", index),
            );
        });
    }

    /// Runs the full CLI path through TransNetV2. Ignored by default because
    /// it needs the ~31 MB model resolved from one of the search locations.
    #[serial]
    #[test]
    #[ignore = "requires the TransNetV2 model (~31 MB) to be present"]
    fn detect_scenes_transnetv2_saves_contiguous_cuts_within_five_frames_and_final_end() {
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
            .args(["detect-scenes", "--method", "transnetv2"])
            .assert()
            .success();

        let mut expected_config = default_config(&test_video, &output, &temp_abs);
        expected_config.condor.sequence_config.scene_detector.method =
            SceneDetectionMethod::TransNetV2 {
                minimum_length: test_video.fps().round() as usize,
                maximum_length: (test_video.fps() * 10.0).round() as usize,
                model_path: None,
                save_scores: false,
            };
        // immutable shadow
        let expected_config = expected_config;

        let (config, _) =
            load_configuration(Some(&config_path)).expect("load_configuration should succeed");

        check_basic_config(&config, &expected_config);

        let scene_boundaries = config
            .condor
            .scenes
            .iter()
            .map(|scene| (scene.start_frame, scene.end_frame))
            .collect::<Vec<_>>();

        // TransNetV2 lands within a few frames of the source's cuts (121, 240,
        // 528); the 10-second maximum length forces the remaining split (480).
        let expected_starts = [0usize, 121, 240, 480, 528];
        assert_eq!(
            scene_boundaries.len(),
            expected_starts.len(),
            "scene boundaries are {scene_boundaries:?}"
        );
        for (index, (start, end)) in scene_boundaries.iter().enumerate() {
            assert!(
                (*start as i64 - expected_starts[index] as i64).abs() <= 5,
                "scene {index} starts at {start}, expected within 5 of {}",
                expected_starts[index]
            );
            if let Some(next) = scene_boundaries.get(index + 1) {
                assert_eq!(*end, next.0, "scenes are contiguous");
            }
        }
        assert_eq!(
            scene_boundaries.last().expect("non-empty").1,
            test_video.frames,
            "last scene ends at the final frame"
        );
    }

    /// `set method` must carry the settings the user did not restate across an
    /// algorithm switch instead of dropping them with the old variant.
    #[serial]
    #[test]
    fn detect_scenes_set_method_keeps_settings_across_algorithm_switch() {
        if !ffmpeg_is_available() {
            return;
        }
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let config_path = init_config(&temp);

        condor_cmd(&temp)
            .args([
                "detect-scenes",
                "set",
                "method",
                "avscenechange",
                "--minimum-frames",
                "20",
                "--maximum-frames",
                "280",
                "--method",
                "fast",
                "--save-scores",
            ])
            .assert()
            .success();
        let method = load(&config_path).condor.sequence_config.scene_detector.method;
        assert!(
            matches!(
                method,
                CoreMethod::AVSceneChange {
                    minimum_length: 20,
                    maximum_length: 280,
                    method: ScenecutMethod::Fast,
                    save_scores: true,
                }
            ),
            "method is {method:?}"
        );

        // Switching algorithms keeps the frame limits and the score setting.
        condor_cmd(&temp)
            .args(["detect-scenes", "set", "method", "transnetv2"])
            .assert()
            .success();
        let method = load(&config_path).condor.sequence_config.scene_detector.method;
        assert!(
            matches!(
                method,
                CoreMethod::TransNetV2 {
                    minimum_length: 20,
                    maximum_length: 280,
                    model_path: None,
                    save_scores: true,
                }
            ),
            "method is {method:?}"
        );

        // The explicit switch wins over the inherited setting.
        condor_cmd(&temp)
            .args(["detect-scenes", "set", "method", "transnetv2", "--save-scores", "off"])
            .assert()
            .success();
        let method = load(&config_path).condor.sequence_config.scene_detector.method;
        assert!(
            matches!(
                method,
                CoreMethod::TransNetV2 {
                    save_scores: false,
                    ..
                }
            ),
            "method is {method:?}"
        );

        condor_cmd(&temp)
            .args(["detect-scenes", "set", "method", "none"])
            .assert()
            .success();
        let method = load(&config_path).condor.sequence_config.scene_detector.method;
        assert!(
            matches!(
                method,
                CoreMethod::None {
                    minimum_length: 20,
                    maximum_length: 280,
                }
            ),
            "method is {method:?}"
        );
    }

    /// An impossible frame range is rejected before anything is written.
    #[serial]
    #[test]
    fn detect_scenes_set_method_rejects_impossible_frame_range() {
        if !ffmpeg_is_available() {
            return;
        }
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let config_path = init_config(&temp);

        condor_cmd(&temp)
            .args([
                "detect-scenes",
                "set",
                "method",
                "avscenechange",
                "--minimum-frames",
                "300",
                "--maximum-frames",
                "100",
            ])
            .assert()
            .failure();

        let method = load(&config_path).condor.sequence_config.scene_detector.method;
        assert!(
            matches!(
                method,
                CoreMethod::AVSceneChange {
                    minimum_length: 24,
                    maximum_length: 240,
                    ..
                }
            ),
            "the rejected edit left the method at {method:?}"
        );
    }

    fn check_scene_detector_info_reset(reset: bool) {
        if !ffmpeg_is_available() {
            return;
        }
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let config_path = init_config(&temp);

        condor_cmd(&temp)
            .args(["detect-scenes", "set", "method", "avscenechange", "--minimum-frames", "30"])
            .assert()
            .success();

        let output = condor_cmd(&temp)
            .args(["detect-scenes", "info"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let section: serde_json::Value =
            serde_json::from_slice(&output).expect("info prints valid JSON");
        assert_eq!(
            section["method"]["AVSceneChange"]["minimum_length"], 30,
            "info shows the saved method: {section}"
        );

        let output = condor_cmd(&temp)
            .args(["detect-scenes", "info", "method"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let method: serde_json::Value =
            serde_json::from_slice(&output).expect("info prints valid JSON");
        assert_eq!(
            method["AVSceneChange"]["minimum_length"], 30,
            "info method shows the saved method: {method}"
        );
        if !reset {
            return;
        }

        condor_cmd(&temp).args(["detect-scenes", "reset", "method"]).assert().success();
        let restored = load(&config_path).condor.sequence_config.scene_detector.method;
        assert!(
            matches!(
                restored,
                CoreMethod::AVSceneChange {
                    minimum_length: 24,
                    maximum_length: 240,
                    method: ScenecutMethod::Standard,
                    save_scores: false,
                }
            ),
            "reset restores the default, found {restored:?}"
        );
    }

    #[serial]
    #[test]
    fn detect_scenes_info_emits_saved_section_or_method_as_json() {
        check_scene_detector_info_reset(false);
    }

    #[serial]
    #[test]
    fn detect_scenes_reset_method_restores_default_limits_algorithm_and_score_flag() {
        check_scene_detector_info_reset(true);
    }

    /// `set input` builds the same override the run flags would, and an input
    /// property without flags changes nothing.
    #[serial]
    #[test]
    fn detect_scenes_input_crud_requires_flags_saves_native_override_and_reset_clears_it() {
        if !ffmpeg_is_available() {
            return;
        }
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let config_path = init_config(&temp);
        let video = get_test_video();

        condor_cmd(&temp).args(["detect-scenes", "set", "input"]).assert().failure();

        condor_cmd(&temp)
            .args([
                "detect-scenes",
                "set",
                "input",
                "--path",
                path_str(&video.path),
                "--decoder",
                "ffms2",
            ])
            .assert()
            .success();
        let detector = load(&config_path).condor.sequence_config.scene_detector;
        let input = detector.input.expect("scene detector input is set");
        assert!(
            matches!(
                input,
                Input::Video {
                    import_method: ImportMethod::FFMS2 { .. },
                    ..
                }
            ),
            "scene detector input is {input:?}"
        );

        condor_cmd(&temp).args(["detect-scenes", "reset", "input"]).assert().success();
        let detector = load(&config_path).condor.sequence_config.scene_detector;
        assert!(detector.input.is_none(), "scene detector input is cleared");
    }

    /// A native input reads and writes its `.ffindex` wherever `cache_path`
    /// points, instead of next to the source.
    #[serial]
    #[test]
    fn a_native_input_indexes_to_its_cache_path() {
        if !ffmpeg_is_available() {
            return;
        }
        let video = get_test_video();
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let output = temp.path().join("out.mkv");
        let config_path = temp.path().join("condor.json");
        // A fresh path in a fresh temp directory, so its appearance can only
        // come from this run honouring it.
        let cache_path = temp.path().join("custom.ffindex");

        condor_cmd(&temp)
            .args(["init", path_str(&video.path), path_str(&output), "--decoder", "ffms2"])
            .assert()
            .success();

        let mut configuration = load(&config_path);
        let Input::Video { import_method, .. } = &mut configuration.condor.input else {
            panic!("expected a native input");
        };
        let ImportMethod::FFMS2 {
            cache_path: configured,
            ..
        } = import_method;
        *configured = Some(cache_path.clone());
        configuration.save(&config_path).expect("configuration save should succeed");

        // `init` already indexed to the default location, so a run that
        // ignored the cache path would find that index and never build this
        // one.
        condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .args(["detect-scenes"])
            .assert()
            .success();

        assert!(
            cache_path.exists(),
            "the index is written to the configured cache path"
        );
        assert!(
            std::fs::metadata(&cache_path).expect("index metadata should resolve").len() > 0,
            "the index at the configured cache path is not empty"
        );
    }

    /// `--vs-args` only reaches a `.vpy`/`.py` input. For any other input the
    /// flag is reported instead of dropped in silence.
    #[serial]
    #[test]
    fn vs_args_on_inherited_or_rebuilt_non_script_input_warns_with_script_guidance() {
        if !ffmpeg_is_available() {
            return;
        }
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        init_config(&temp);
        let video = get_test_video();

        // Inheriting the main input.
        let stderr = condor_cmd(&temp)
            .args(["detect-scenes", "set", "input", "--vs-args", "somekey=somevalue"])
            .assert()
            .success()
            .get_output()
            .stderr
            .clone();
        let stderr = String::from_utf8(stderr).expect("stderr is UTF-8");
        assert!(
            stderr.contains("--vs-args applies only to VapourSynth script")
                && stderr.contains("BestSource video input"),
            "stderr is {stderr}"
        );

        // Rebuilding the input from a new path changes nothing about that.
        let stderr = condor_cmd(&temp)
            .args([
                "detect-scenes",
                "set",
                "input",
                "--path",
                path_str(&video.path),
                "--vs-args",
                "somekey=somevalue",
            ])
            .assert()
            .success()
            .get_output()
            .stderr
            .clone();
        let stderr = String::from_utf8(stderr).expect("stderr is UTF-8");
        assert!(
            stderr.contains("--vs-args applies only to VapourSynth script")
                && stderr.contains("drop --vs-args or use a script input"),
            "stderr is {stderr}"
        );
    }
}
