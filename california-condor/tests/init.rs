use andean_condor::{
    core::sequence::target_quality::TargetQuality,
    ffmpeg::FFPixelFormat,
    models::{
        encoder::{
            Encoder,
            EncoderBase,
            EncoderPasses,
            cli_parameter::CLIParameter,
            photon_noise::PhotonNoise,
        },
        input::{Input as InputModel, VapourSynthImportMethod},
        sequence::{
            scene_concatenator::ConcatMethod,
            target_quality::{TargetQualityConfig, types::QualityMetric},
        },
    },
    vapoursynth::{plugins::resize::Scaler, vapoursynth_filters::VapourSynthFilter},
};
use california_condor::{
    commands::handlers::load_configuration,
    test_helpers::*,
    utils::hash_path::hash_path,
};

#[path = "common.rs"]
mod common;
#[path = "config_helpers.rs"]
mod config_helpers;

use common::condor_cmd;
use config_helpers::{init_config, load};

#[cfg(test)]
mod tests {
    use andean_condor::models::input::{ImportMethod, Input};
    use serial_test::serial;

    use super::*;

    #[test]
    fn default() {
        if !ffmpeg_is_available() {
            return;
        }
        let test_video = get_test_video();
        let input_abs = path_abs::PathAbs::new(test_video.path.clone())
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let temp_abs = path_abs::PathAbs::new(temp.path().join(hash_path(&input_abs)))
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let output = temp.path().join("out.mkv");
        let config_path = temp.path().join("condor.json");
        condor_cmd(&temp)
            .args(["init", path_str(&test_video.path), path_str(&output)])
            .assert()
            .success();

        let expected_config = default_config(&test_video, &output, &temp_abs);

        assert!(config_path.exists(), "config file exists");
        let (config, _) = load_configuration(Some(&temp.path().join("condor.json")))
            .expect("config file should load");

        check_basic_config(&config, &expected_config);
        assert!(config.condor.scenes.is_empty(), "scenes is empty");
    }

    #[test]
    fn with_custom_paths() {
        if !ffmpeg_is_available() {
            return;
        }
        let test_video = get_test_video();
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let temp_abs = path_abs::PathAbs::new(temp.path())
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let output = temp.path().join("smaller-video.mkv");
        let config_path = temp.path().join("savefile.json");
        condor_cmd(&temp)
            .args([
                "init",
                path_str(&test_video.path),
                path_str(&output),
                "--temp",
                path_str(temp.path()),
                "--config-file",
                path_str(&config_path),
                "--logs",
                path_str(temp.path().join("custom.log").as_path()),
            ])
            .assert()
            .success();

        let expected_config = default_config(&test_video, &output, &temp_abs);

        assert!(config_path.exists(), "config file exists");
        let (config, _) = load_configuration(Some(&temp.path().join("savefile.json")))
            .expect("config file should load");

        check_basic_config(&config, &expected_config);
        assert!(config.condor.scenes.is_empty(), "scenes is empty");
    }

    #[test]
    fn with_options() {
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
        condor_cmd(&temp)
            .args([
                "init",
                path_str(&test_video.path),
                path_str(&output),
                "--decoder",
                "vs-ffms2",
                "--filters",
                "trim:start=24;",
                "--filters",
                "resize:scaler=bilinear;width=1280;height=720;format=yuv420p;",
                "--concat",
                "ivf",
                "--workers",
                "8",
                "--encoder",
                "aom",
                "--params",
                "--cpu-used=4 --cq-level=23",
                "--photon-noise",
                "400",
                "--target",
                "9.5",
                "--target-metric",
                "cvvdp",
            ])
            .assert()
            .success();

        let mut expected_config = default_config(&test_video, &output, &temp_abs);
        expected_config.condor.input = InputModel::VapourSynth {
            path:          input_abs,
            import_method: VapourSynthImportMethod::FFMS2 {
                index: None
            },
            cache_path:    None,
            filters:       vec![
                VapourSynthFilter::Trim {
                    start: Some(24),
                    end:   None,
                },
                VapourSynthFilter::Resize {
                    scaler: Some(Scaler::Bilinear),
                    width:  Some(1280),
                    height: Some(720),
                    format: Some(FFPixelFormat::YUV420P),
                },
            ],
        };
        let mut custom_encoder_parameters = EncoderBase::AOM.default_parameters();
        custom_encoder_parameters.extend(CLIParameter::new_numbers("--", "=", &[
            ("cpu-used", 4.0),
            ("cq-level", 23.0),
        ]));
        expected_config.condor.encoder = Encoder::AOM {
            executable:        None,
            pass:              EncoderPasses::All(2),
            options:           custom_encoder_parameters,
            photon_noise:      Some(PhotonNoise {
                iso:        400,
                chroma_iso: None,
                width:      None,
                height:     None,
                c_y:        None,
                ccb:        None,
                ccr:        None,
            }),
            ffmpeg_filter:     None,
            append_color_data: true,
        };
        expected_config.condor.sequence_config.parallel_encoder.workers = Some(8);
        expected_config.condor.sequence_config.target_quality = Some(TargetQualityConfig {
            input: None,
            metric: QualityMetric::CVVDP {
                target_range:      (9.4, 9.6),
                resolution:        None,
                display_model:     None,
                resize_to_display: None,
                disable_temporal:  None,
                gpu_id:            None,
            },
            quantizer_range: TargetQuality::default_quantizer_range(&EncoderBase::AOM),
            ..Default::default()
        });
        expected_config.condor.sequence_config.scene_concatenator.method = ConcatMethod::Ivf;
        let expected_config = expected_config;

        let (config, _) = load_configuration(Some(&temp.path().join("condor.json")))
            .expect("config file should load");

        check_basic_config(&config, &expected_config);
        assert!(config.condor.scenes.is_empty(), "scenes is empty");
    }

    #[test]
    fn fails_if_config_already_exists() {
        if !ffmpeg_is_available() {
            return;
        }
        let test_video = get_test_video();
        let input = &test_video.path;
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let output = temp.path().join("out.mkv");

        // Mock an existing config file
        std::fs::write(temp.path().join("condor.json"), "{}").expect("config file writes to disk");

        condor_cmd(&temp)
            .args(["init", path_str(input), path_str(&output)])
            .assert()
            .failure();
        let config_content = std::fs::read_to_string(temp.path().join("condor.json"))
            .expect("config file reads from disk");
        assert_eq!(config_content, "{}", "config file is unchanged");
    }

    /// The input and output live at the top level, so `init` is where they are
    /// edited; neither has a default to reset to.
    #[serial]
    #[test]
    fn init_set_input_and_output() {
        if !ffmpeg_is_available() {
            return;
        }
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let config_path = init_config(&temp);
        let video = get_test_video();
        let new_output = temp.path().join("renamed.mkv");

        condor_cmd(&temp)
            .args(["init", "set", "output", path_str(&new_output)])
            .assert()
            .success();
        let condor = load(&config_path).condor;
        assert_eq!(condor.output.path, new_output, "output path is set");

        condor_cmd(&temp)
            .args(["init", "set", "input", "--path", path_str(&video.path), "--decoder", "ffms2"])
            .assert()
            .success();
        let input = load(&config_path).condor.input;
        assert!(
            matches!(input, Input::Video {
                import_method: ImportMethod::FFMS2 { .. },
                ..
            }),
            "root input is {input:?}"
        );

        // A property with no flags changes nothing.
        condor_cmd(&temp).args(["init", "set", "input"]).assert().failure();

        // The paths are what `init` exists to capture, so there is no default
        // to restore - not for a key and not for the whole section.
        condor_cmd(&temp).args(["init", "reset", "input"]).assert().failure();
        condor_cmd(&temp).args(["init", "reset", "output"]).assert().failure();
        condor_cmd(&temp).args(["init", "reset"]).assert().failure();
    }
}
