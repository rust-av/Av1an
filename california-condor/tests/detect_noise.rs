#[path = "common.rs"]
mod common;
#[path = "config_helpers.rs"]
mod config_helpers;

use california_condor::{
    commands::handlers::load_configuration, test_helpers::*, utils::hash_path::hash_path,
};
use common::condor_cmd;
use config_helpers::{init_config, load};

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use andean_condor::{
        models::sequence::noise_detector::{NoiseDetectorConfig, NoiseDetectorData},
        vapoursynth::vapoursynth_filters::VapourSynthFilter,
    };
    use serial_test::serial;

    use super::*;

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
            .args(["detect-noise"])
            .assert()
            .success();

        let mut expected_config = default_config(&test_video, &output, &temp_abs);
        expected_config.condor.sequence_config.noise_detector =
            Some(NoiseDetectorConfig::default());
        // immutable shadow
        let expected_config = expected_config;

        let (config, _) =
            load_configuration(Some(&config_path)).expect("load_configuration should succeed");

        check_basic_config(&config, &expected_config);
    }

    #[serial]
    #[test]
    fn with_custom_filters() {
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
        let custom_reference_filters = vec![VapourSynthFilter::WNNM {
            sigma: Some(vec![2.0, 0.0, 0.0]),
            block_size: None,
            block_step: None,
            group_size: None,
            bm_range: None,
            radius: None,
            ps_num: None,
            ps_range: None,
            residual: None,
            adaptive_aggregation: None,
        }];
        let custom_denoised_filters = vec![VapourSynthFilter::WNNM {
            sigma: Some(vec![4.0, 0.0, 0.0]),
            block_size: None,
            block_step: None,
            group_size: None,
            bm_range: None,
            radius: None,
            ps_num: None,
            ps_range: None,
            residual: None,
            adaptive_aggregation: None,
        }];

        // Mock an existing config file
        let mut config = default_config(&test_video, &output, &temp_abs);
        config.condor.scenes = test_video.mock_scenes(&config.condor.encoder);
        config.save(&config_path).expect("configuration save should succeed");

        condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .args([
                "detect-noise",
                "--reference-filters",
                "wnnm:sigma=2.0,0,0;",
                "--denoised-filters",
                "wnnm:sigma=4.0,0,0;",
            ])
            .assert()
            .success();

        let mut expected_config = config;
        let expected_noise = |index| match index {
            0 => 0.0,
            1 => 0.00012943333423844175,
            2 => 0.00014239571265220908,
            // Exact producer value; the former JSON parser rounded this by one ULP.
            3 => 0.00010794408013830057,
            4 => 0.000002177445753834643,
            _ => 0.0,
        };
        expected_config.condor.sequence_config.noise_detector = Some(NoiseDetectorConfig {
            reference_filters: custom_reference_filters,
            denoised_filters: custom_denoised_filters,
            ..Default::default()
        });
        expected_config.condor.scenes.iter_mut().enumerate().for_each(|(index, scene)| {
            scene.sequence_data.noise_detection = Some(NoiseDetectorData {
                created_on: SystemTime::now(),
                noise: expected_noise(index),
                luminance: 0.0,
            });
        });

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
            assert!(
                scene.sequence_data.noise_detection.is_some(),
                "scene {} contains Noise Detector data",
                index
            );
            let sequence_data = scene
                .sequence_data
                .noise_detection
                .as_ref()
                .expect("scene contains Noise Detector data");
            assert_eq!(
                sequence_data.noise,
                expected_noise(index),
                "scene {} noise is {}",
                index,
                expected_noise(index)
            );
        });
    }

    /// `detect-noise` owns its filter lists: replace, clear and reset, and
    /// resetting a key from a never-enabled section stays a no-op.
    #[serial]
    #[test]
    fn detect_noise_set_and_reset_filter_lists() {
        if !ffmpeg_is_available() {
            return;
        }
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let config_path = init_config(&temp);

        // Resetting from a section that was never enabled must not enable it.
        condor_cmd(&temp).args(["detect-noise", "reset", "input"]).assert().success();
        let detector = load(&config_path).condor.sequence_config.noise_detector;
        assert!(detector.is_none(), "noise detector stays unconfigured");

        condor_cmd(&temp)
            .args(["detect-noise", "set", "reference-filters", "resize:width=960;height=540;"])
            .assert()
            .success();

        let detector = load(&config_path)
            .condor
            .sequence_config
            .noise_detector
            .expect("noise detector is set");
        assert_eq!(
            detector.reference_filters.len(),
            1,
            "one reference filter was set"
        );
        assert!(
            matches!(
                &detector.reference_filters[0],
                VapourSynthFilter::Resize {
                    width: Some(960),
                    height: Some(540),
                    ..
                }
            ),
            "reference filter is {:?}",
            detector.reference_filters[0]
        );

        // No values at all empties the list, and so does a clear word.
        condor_cmd(&temp)
            .args(["detect-noise", "set", "reference-filters"])
            .assert()
            .success();
        let detector = load(&config_path)
            .condor
            .sequence_config
            .noise_detector
            .expect("noise detector exists");
        assert!(
            detector.reference_filters.is_empty(),
            "reference filters were cleared by an empty list"
        );

        condor_cmd(&temp)
            .args(["detect-noise", "set", "reference-filters", "none"])
            .assert()
            .success();
        let detector = load(&config_path)
            .condor
            .sequence_config
            .noise_detector
            .expect("noise detector exists");
        assert!(
            detector.reference_filters.is_empty(),
            "a clear word empties the filters too"
        );

        condor_cmd(&temp)
            .args(["detect-noise", "reset", "reference-filters"])
            .assert()
            .success();
        let detector = load(&config_path)
            .condor
            .sequence_config
            .noise_detector
            .expect("noise detector exists");
        assert_eq!(
            detector.reference_filters.len(),
            1,
            "the default reference filter was restored"
        );
    }

    /// Resetting a filter list says how to empty it outright.
    #[serial]
    #[test]
    fn reset_hints_how_to_clear_a_filter_list() {
        if !ffmpeg_is_available() {
            return;
        }
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        init_config(&temp);

        condor_cmd(&temp)
            .args(["detect-noise", "set", "reference-filters", "resize:width=960;"])
            .assert()
            .success();

        let stdout = condor_cmd(&temp)
            .args(["detect-noise", "reset", "reference-filters"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let stdout = String::from_utf8(stdout).expect("stdout is UTF-8");
        assert!(
            stdout.contains("av1an detect-noise set reference-filters none"),
            "output is {stdout}"
        );
    }
}
