use andean_condor::{
    core::sequence::target_quality::TargetQuality,
    ffmpeg::FFPixelFormat,
    models::{
        encoder::{
            Encoder, EncoderBase, EncoderPasses, cli_parameter::CLIParameter,
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
    commands::handlers::load_configuration, test_helpers::*, utils::hash_path::hash_path,
};

use crate::common::condor_cmd;
use crate::common::config::{init_config, load};

#[cfg(test)]
mod handler_configuration {
    use super::*;
    use california_condor::commands::{
        ConcatenationMethod, DecoderMethod, EncoderMethod, QualityMetric as QualityMetricBase,
        handlers::init::init_handler,
    };
    use std::{assert_matches, path::Path};

    #[test]
    fn init_without_config_path_saves_condor_json_and_default_settings_in_current_directory() {
        crate::common::config::isolated_default_path_test(
            "commands::handlers::init::handler_configuration::init_without_config_path_saves_condor_json_and_default_settings_in_current_directory",
            init_creates_default_config,
        );
    }

    fn init_creates_default_config(temp: &Path) {
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
        let config_path = temp.join("condor.json");
        let config_path_abs = path_abs::PathAbs::new(&config_path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let expected_config = default_config(&test_video, &output, &temp_abs);
        init_handler(
            None,
            None,
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
        assert!(config_path.exists(), "config file exists");
        let (config, found_config_path) =
            load_configuration(Some(&config_path)).expect("config loads");
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
    fn init_saves_requested_input_encoder_workers_target_and_concatenation_settings() {
        let test_video = get_test_video();
        let input_abs = path_abs::PathAbs::new(&test_video.path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let temp = tempfile::tempdir().expect("temp directory");
        let temp_abs = path_abs::PathAbs::new(temp.path().join("custom-temp"))
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let output = temp.path().join("out.mkv");
        let config_path = temp.path().join("custom-condor.json");
        let config_path_abs = path_abs::PathAbs::new(&config_path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let custom_filters = vec![VapourSynthFilter::Crop {
            top: Some(2),
            bottom: Some(2),
            left: None,
            right: None,
        }];
        let custom_vs_args = vec!["key=value".to_string()];
        let mut custom_encoder_parameters = EncoderBase::AOM.default_parameters();
        custom_encoder_parameters.insert(
            "cpu-used".to_owned(),
            CLIParameter::new_number("--", "=", 7.0),
        );
        let mut expected_config = default_config(&test_video, &output, &temp_abs);
        expected_config.condor.input = InputModel::VapourSynth {
            path: input_abs,
            import_method: VapourSynthImportMethod::FFMS2 { index: None },
            cache_path: None,
            filters: custom_filters.clone(),
        };
        expected_config.condor.encoder = Encoder::AOM {
            executable: None,
            pass: EncoderPasses::All(2),
            options: custom_encoder_parameters,
            photon_noise: Some(PhotonNoise {
                iso: 4800,
                chroma_iso: None,
                width: None,
                height: None,
                c_y: None,
                ccb: None,
                ccr: None,
            }),
            ffmpeg_filter: None,
            append_color_data: true,
        };
        expected_config.condor.sequence_config.parallel_encoder.workers = Some(2);
        expected_config.condor.sequence_config.target_quality = Some(TargetQualityConfig {
            input: None,
            metric: QualityMetric::SSIMULACRA2 {
                target_range: (74.0, 76.0),
                resolution: None,
                threads: None,
                gpu_id: None,
            },
            quantizer_range: TargetQuality::default_quantizer_range(&EncoderBase::AOM),
            ..Default::default()
        });
        expected_config.condor.sequence_config.scene_concatenator.method =
            ConcatMethod::FFmpeg(Default::default());
        let expected_config = expected_config;
        init_handler(
            Some(&config_path),
            Some(&temp.path().join("custom-temp")),
            &test_video.path,
            &output,
            Some(&DecoderMethod::VSFFMS2),
            Some(&custom_filters),
            Some(&custom_vs_args),
            Some(&ConcatenationMethod::FFmpeg),
            None,
            Some(2),
            Some(&EncoderMethod::AOM),
            Some("--cpu-used=7"),
            Some(4800),
            true,
            Some(&QualityMetricBase::SSIMULACRA2),
            Some(75.0),
            None,
            false,
        )
        .expect("init_handler should succeed");
        assert!(config_path.exists(), "config file exists");
        let (config, found_config_path) =
            load_configuration(Some(&config_path)).expect("config loads");
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
    fn init_disable_color_data_turns_appending_off() {
        let test_video = get_test_video();
        let temp = tempfile::tempdir().expect("temp directory");
        let output = temp.path().join("out.mkv");
        let config_path = temp.path().join("condor.json");
        init_handler(
            Some(&config_path),
            None,
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
            false,
            None,
            None,
            None,
            false,
        )
        .expect("init_handler should succeed");
        let (config, _) = load_configuration(Some(&config_path)).expect("config loads");
        assert!(
            !config.condor.encoder.append_color_data(),
            "append_color_data is false"
        );
    }

    #[test]
    fn init_missing_input_returns_error_without_creating_configuration() {
        let test_video = std::env::temp_dir().join("not_found.mkv");
        let input_abs = path_abs::PathAbs::new(&test_video)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let temp = tempfile::tempdir().expect("temp directory");
        let output = temp.path().join("out.mkv");
        let config_path = temp.path().join("condor.json");
        let result = init_handler(
            Some(&config_path),
            Some(&temp.path().join(hash_path(&input_abs))),
            &test_video,
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
        );
        assert_matches!(result, Err(_), "init_handler should fail");
        assert!(!config_path.exists(), "config file does not exist");
    }
}

#[cfg(test)]
mod tests {
    use andean_condor::models::input::{ImportMethod, Input};
    use serial_test::serial;

    use super::*;

    #[test]
    fn new_init_and_root_tq_options_require_target_before_source_open_or_save() {
        let mut failures = vec![];
        for init in [false, true] {
            let temp = tempfile::tempdir().expect("fixture");
            let source = temp.path().join("source.vpy");
            let bytes = format!(
                "from pathlib import Path\nPath({}).write_text('opened')\nraise RuntimeError('TARGET_QUALITY_SOURCE_WAS_OPENED')\n",
                serde_json::to_string(&temp.path().join("opened")).expect("literal")
            );
            std::fs::write(&source, &bytes).expect("script");
            let args = if init {
                vec!["init", "source.vpy", "out.mkv", "--target-metric", "vmaf"]
            } else {
                vec!["-i", "source.vpy", "-o", "out.mkv", "--target-metric", "vmaf"]
            };
            let output = condor_cmd(&temp).env("PATH", "").args(&args).output().expect("CLI");
            let message = String::from_utf8_lossy(&output.stderr);
            if output.status.success()
                || !message.contains("--target")
                || !message.contains("saved target")
            {
                failures.push(format!("{args:?}: missing target refusal, got {message}"));
            }
            if temp.path().join("opened").exists() {
                failures.push(format!("{args:?}: opened source before admission"));
            }
            assert!(!temp.path().join("condor.json").exists());
            assert!(!temp.path().join("out.mkv").exists());
            assert_eq!(std::fs::read(&source).expect("source"), bytes.as_bytes());
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn direct_init_missing_target_is_typed_and_pre_native() {
        use california_condor::commands::{
            QualityMetric,
            handlers::{init::init_handler, target_quality::TargetQualityConfigError},
        };
        let temp = tempfile::tempdir().expect("fixture");
        let source = temp.path().join("source.vpy");
        std::fs::write(
            &source,
            "raise RuntimeError('TARGET_QUALITY_SOURCE_WAS_OPENED')",
        )
        .expect("script");
        let file = temp.path().join("condor.json");
        let error = init_handler(
            Some(&file),
            None,
            &source,
            &temp.path().join("out.mkv"),
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
            Some(&QualityMetric::VMAF),
            None,
            None,
            false,
        )
        .expect_err("metric needs target");
        assert!(
            matches!(
                error.downcast_ref::<TargetQualityConfigError>(),
                Some(TargetQualityConfigError::MissingTarget)
            ),
            "typed cause, not native failure: {error:#}"
        );
        assert!(error.to_string().contains("--target"), "{error:#}");
        assert!(!file.exists());
    }

    #[test]
    fn direct_new_root_missing_target_is_typed_and_pre_native() {
        use california_condor::commands::{
            CondorCli,
            handlers::{start::start_handler, target_quality::TargetQualityConfigError},
        };
        use clap::Parser;
        let temp = tempfile::tempdir().expect("fixture");
        let source = temp.path().join("source.vpy");
        std::fs::write(
            &source,
            "raise RuntimeError('TARGET_QUALITY_SOURCE_WAS_OPENED')",
        )
        .expect("script");
        let file = temp.path().join("condor.json");
        let cli =
            CondorCli::try_parse_from(["condor", "--target-metric", "vmaf"]).expect("root grammar");
        let error = start_handler(
            Some(&file),
            cli.temp.as_deref(),
            Some(&source),
            cli.scd_input.as_deref(),
            cli.tq_input.as_deref(),
            Some(&temp.path().join("out.mkv")),
            cli.decoder.as_ref(),
            cli.scd_decoder.as_ref(),
            cli.tq_decoder.as_ref(),
            cli.filters.as_deref(),
            cli.scd_filters.as_deref(),
            cli.tq_filters.as_deref(),
            cli.vs_args.as_deref(),
            cli.scd_vs_args.as_deref(),
            cli.tq_vs_args.as_deref(),
            cli.concat.as_ref(),
            cli.discard.as_deref(),
            cli.workers,
            cli.encoder.as_ref(),
            cli.passes,
            cli.params.as_deref(),
            cli.tq_params.as_deref(),
            cli.photon_noise,
            cli.chroma_noise,
            cli.ffmpeg_filter.as_deref(),
            None,
            cli.target_metric.as_ref(),
            cli.target,
            cli.minimum_quantizer,
            cli.maximum_quantizer,
            cli.target_profile.as_ref(),
            cli.max_tries,
            cli.overwrite,
        )
        .expect_err("metric needs target");
        assert!(
            matches!(
                error.downcast_ref::<TargetQualityConfigError>(),
                Some(TargetQualityConfigError::MissingTarget)
            ),
            "typed cause, not native failure: {error:#}"
        );
        assert!(error.to_string().contains("--target"), "{error:#}");
        assert!(!file.exists());
    }

    fn shadowed_output_case(direct: bool) {
        use andean_condor::models::input::VapourSynthScriptSource;
        use california_condor::commands::{CondorCli, handlers::start::start_handler};
        use clap::Parser;
        let temp = tempfile::tempdir().expect("fixture");
        let source = temp.path().join("source.vpy");
        let marker = serde_json::to_string(&temp.path().join("opened")).expect("marker");
        std::fs::write(&source,format!("from pathlib import Path\nPath({marker}).write_text('opened')\nraise RuntimeError('SOURCE WAS OPENED')\n")).expect("script");
        let source_bytes = std::fs::read(&source).expect("source snapshot");
        let video = TestVideo {
            path: source.clone(),
            width: 32,
            height: 32,
            frames: 2,
            fps_rational: (24, 1),
            format: FFPixelFormat::YUV420P,
            scenes: vec![],
        };
        let mut config = default_config(
            &video,
            &temp.path().join("fallback.mkv"),
            &temp.path().join("work"),
        );
        config.condor.input = Input::VapourSynthScript {
            source: VapourSynthScriptSource::Path(source.clone()),
            variables: Default::default(),
            index: 0,
            filters: vec![],
            stream_concurrently: false,
        };
        let actual = temp.path().join("actual.mkv");
        config.condor.sequence_config.scene_concatenator.output = Some(actual.clone());
        let file = temp.path().join("condor.json");
        config.save(&file).expect("save fixture");
        let before = std::fs::read(&file).expect("snapshot");
        let cli = CondorCli::try_parse_from(["condor", "-o", "new-root.mkv", "--overwrite"])
            .expect("root flags");
        let message = if direct {
            let error = start_handler(
                Some(&file),
                cli.temp.as_deref(),
                cli.input.as_deref(),
                cli.scd_input.as_deref(),
                cli.tq_input.as_deref(),
                cli.output.as_deref(),
                cli.decoder.as_ref(),
                cli.scd_decoder.as_ref(),
                cli.tq_decoder.as_ref(),
                cli.filters.as_deref(),
                cli.scd_filters.as_deref(),
                cli.tq_filters.as_deref(),
                cli.vs_args.as_deref(),
                cli.scd_vs_args.as_deref(),
                cli.tq_vs_args.as_deref(),
                cli.concat.as_ref(),
                cli.discard.as_deref(),
                cli.workers,
                cli.encoder.as_ref(),
                cli.passes,
                cli.params.as_deref(),
                cli.tq_params.as_deref(),
                cli.photon_noise,
                cli.chroma_noise,
                cli.ffmpeg_filter.as_deref(),
                None,
                cli.target_metric.as_ref(),
                cli.target,
                cli.minimum_quantizer,
                cli.maximum_quantizer,
                cli.target_profile.as_ref(),
                cli.max_tries,
                cli.overwrite,
            )
            .expect_err("shadowed explicit output");
            format!("{error:#}")
        } else {
            let result = condor_cmd(&temp)
                .env("PATH", "")
                .args(["-o", "new-root.mkv", "--overwrite"])
                .output()
                .expect("CLI");
            assert!(!result.status.success());
            String::from_utf8_lossy(&result.stderr).into_owned()
        };
        assert!(
            message.contains("--output")
                && message.contains("concatenate reset output")
                && message.contains(path_str(&actual)),
            "actionable conflict: {message}"
        );
        assert!(
            !temp.path().join("opened").exists(),
            "no native work: {message}"
        );
        assert_eq!(
            std::fs::read(source).expect("unchanged source"),
            source_bytes
        );
        assert_eq!(std::fs::read(file).expect("saved bytes"), before);
        assert!(!temp.path().join("new-root.mkv").exists());
        assert_eq!(
            config.condor.sequence_config.scene_concatenator.output,
            Some(actual)
        );
    }

    #[test]
    fn shadowed_output_direct_start_rejects_before_native_and_save() {
        shadowed_output_case(true);
    }

    #[test]
    fn shadowed_output_cli_start_rejects_before_native_and_save() {
        shadowed_output_case(false);
    }

    #[test]
    fn root_pipeline_writes_selected_concat_output_and_reports_it_in_landing_message() {
        let temp = tempfile::tempdir().expect("fixture");
        let source = temp.path().join("source.mkv");
        let encoded = std::process::Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=size=32x32:rate=24",
                "-frames:v",
                "2",
                "-c:v",
                "ffv1",
            ])
            .arg(&source)
            .output()
            .expect("required FFmpeg CLI");
        assert!(
            encoded.status.success(),
            "{}",
            String::from_utf8_lossy(&encoded.stderr)
        );
        let video = TestVideo {
            path: source.clone(),
            width: 32,
            height: 32,
            frames: 2,
            fps_rational: (24, 1),
            format: FFPixelFormat::YUV420P,
            scenes: vec![(0, 2)],
        };
        let fallback = temp.path().join("fallback-must-not-be-written.mkv");
        let actual = temp.path().join("actual.mkv");
        let mut config = default_config(&video, &fallback, &temp.path().join("work"));
        config.condor.input = Input::Video {
            path: source,
            import_method: ImportMethod::FFMS2 {
                index: None,
                cache_path: None,
            },
            filters: vec![],
        };
        config.condor.encoder = Encoder::default_from_base(&EncoderBase::AOM, false);
        config
            .condor
            .encoder
            .parameters_mut()
            .insert("cpu-used".into(), CLIParameter::new_number("--", "=", 8.0));
        config.condor.scenes = video.mock_scenes(&config.condor.encoder);
        config.condor.sequence_config.parallel_encoder.workers = Some(1);
        config.condor.sequence_config.parallel_encoder.max_tries = 1;
        config.condor.sequence_config.scene_concatenator.method =
            ConcatMethod::FFmpeg(Default::default());
        config.condor.sequence_config.scene_concatenator.output = Some(actual.clone());
        let file = temp.path().join("condor.json");
        config.save(&file).expect("save tiny native job");
        let result = condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .args(["--skip-scd"])
            .output()
            .expect("root pipeline");
        let message = String::from_utf8_lossy(&result.stderr);
        assert!(result.status.success(), "{message}");
        assert!(
            actual.is_file() && !fallback.exists(),
            "actual final override"
        );
        assert!(
            message.contains(&format!("Condor has landed. Output: {}", actual.display())),
            "actual landing consumer: {message}"
        );
        let saved = load(&file);
        assert_eq!(saved.condor.output.path, fallback);
        assert_eq!(
            saved.condor.sequence_config.scene_concatenator.output,
            Some(actual)
        );
    }

    #[test]
    fn init_and_new_start_reject_alias_before_source_evaluation_or_save() {
        for (init, overwrite) in [(true, false), (true, true), (false, false), (false, true)] {
            let temp = tempfile::tempdir().expect("fixture");
            let source = temp.path().join("source.vpy");
            let marker = serde_json::to_string(&temp.path().join("opened")).expect("path literal");
            let bytes = format!(
                "from pathlib import Path\nPath({marker}).write_text('opened')\nraise RuntimeError('SOURCE WAS OPENED')\n"
            );
            std::fs::write(&source, &bytes).expect("script");
            let mut args = if init {
                vec!["init", "source.vpy", "./source.vpy"]
            } else {
                vec!["--input", "source.vpy", "--output", "./source.vpy"]
            };
            if overwrite {
                args.push("--overwrite");
            }
            let output = condor_cmd(&temp).env("PATH", "").args(args).output().expect("CLI");
            assert!(!output.status.success());
            assert!(
                !temp.path().join("opened").exists(),
                "output rejection must precede script evaluation: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(!temp.path().join("condor.json").exists());
            assert_eq!(std::fs::read(source).expect("source"), bytes.as_bytes());
        }
    }

    #[test]
    fn init_refuses_existing_distinct_output_without_consent() {
        let temp = tempfile::tempdir().expect("fixture");
        std::fs::write(temp.path().join("source.vpy"), "import vapoursynth as vs\nvs.core.std.BlankClip(width=32,height=16,length=2,format=vs.YUV420P8).set_output()\n").expect("script");
        std::fs::write(temp.path().join("out.mkv"), b"old final sentinel").expect("output");
        condor_cmd(&temp)
            .env("PATH", "")
            .args(["init", "source.vpy", "out.mkv"])
            .assert()
            .failure();
        assert!(!temp.path().join("condor.json").exists());
        assert_eq!(
            std::fs::read(temp.path().join("out.mkv")).expect("output"),
            b"old final sentinel"
        );
    }

    #[test]
    fn existing_root_start_unsafe_output_refuses_before_source_open_or_resave() {
        use andean_condor::models::input::VapourSynthScriptSource;
        for case in ["root", "distinct", "new-scd", "new-tq", "missing-parent"] {
            let temp = tempfile::tempdir().expect("fixture");
            let source = temp.path().join("source.vpy");
            let marker = serde_json::to_string(&temp.path().join("opened")).expect("marker");
            std::fs::write(&source,format!("from pathlib import Path\nPath({marker}).write_text('opened')\nraise RuntimeError('SOURCE WAS OPENED')\n")).expect("script");
            let video = TestVideo {
                path: source.clone(),
                width: 32,
                height: 16,
                frames: 2,
                fps_rational: (24, 1),
                format: FFPixelFormat::YUV420P,
                scenes: vec![],
            };
            let output = match case {
                "root" => source.clone(),
                "missing-parent" => temp.path().join("absent/out.mkv"),
                _ => temp.path().join("out.mkv"),
            };
            if case == "distinct" {
                std::fs::write(&output, b"old final").expect("old");
            }
            let mut config = default_config(&video, &output, &temp.path().join("work"));
            config.condor.input = Input::VapourSynthScript {
                source: VapourSynthScriptSource::Path(source.clone()),
                variables: Default::default(),
                index: 0,
                filters: vec![],
                stream_concurrently: false,
            };
            config.input = temp.path().join("duplicated-but-not-actual.vpy");
            let file = temp.path().join("condor.json");
            config.save(&file).expect("save fixture");
            let before = std::fs::read(&file).expect("snapshot");
            let mut cmd = condor_cmd(&temp);
            cmd.env("PATH", "");
            if case == "new-scd" {
                cmd.args(["--scd-input", path_str(&output)]);
            }
            if case == "new-tq" {
                cmd.args(["--tq-input", path_str(&output)]);
            }
            let result = cmd.output().expect("CLI");
            assert!(!result.status.success(), "{case}");
            let message = String::from_utf8_lossy(&result.stderr);
            assert!(
                message.contains("Output") || message.contains("output"),
                "{case}: {message}"
            );
            assert!(!temp.path().join("opened").exists(), "{case}: {message}");
            assert_eq!(std::fs::read(file).expect("saved bytes"), before);
            assert!(!temp.path().join("absent").exists());
            if case == "distinct" {
                assert_eq!(std::fs::read(output).expect("old"), b"old final");
            }
        }
    }

    fn check_overwrite_routes(phase: u8) {
        assert!((1..=3).contains(&phase));
        let temp = tempfile::tempdir().expect("fixture");
        std::fs::write(temp.path().join("source.vpy"),"import vapoursynth as vs\nvs.core.std.BlankClip(width=32,height=16,length=2,format=vs.YUV420P8).set_output()\n").expect("script");
        std::fs::write(temp.path().join("out.mkv"), b"old final").expect("old");
        condor_cmd(&temp)
            .env("PATH", "")
            .args(["init", "source.vpy", "out.mkv", "--overwrite"])
            .assert()
            .success();
        let file = temp.path().join("condor.json");
        assert!(load(&file).condor.output.overwrite);
        condor_cmd(&temp)
            .env("PATH", "")
            .env("CONDOR_TEST_MODE", "1")
            .args(["--concat", "ivf"])
            .assert()
            .failure();
        assert!(
            load(&file).condor.output.overwrite,
            "omission retains saved consent even when native validation later fails"
        );
        if phase != 1 {
            for args in
                [vec!["init", "set", "overwrite", "false"], vec!["init", "reset", "overwrite"]]
            {
                condor_cmd(&temp)
                    .env("PATH", "")
                    .args(["init", "set", "overwrite", "true"])
                    .assert()
                    .success();
                condor_cmd(&temp).env("PATH", "").args(args).assert().success();
                assert!(!load(&file).condor.output.overwrite);
            }
            let before = std::fs::read(&file).expect("before refusal");
            condor_cmd(&temp).env("PATH", "").assert().failure();
            assert_eq!(std::fs::read(&file).expect("unchanged"), before);
        }
        condor_cmd(&temp)
            .env("PATH", "")
            .env("CONDOR_TEST_MODE", "1")
            .args(["--overwrite", "--concat", "ivf"])
            .assert()
            .failure();
        assert!(
            load(&file).condor.output.overwrite,
            "root flag saves consent"
        );
        assert_eq!(
            std::fs::read(temp.path().join("out.mkv")).expect("old"),
            b"old final"
        );
        if phase <= 2 {
            return;
        }
        condor_cmd(&temp).args(["scenes", "--overwrite"]).assert().failure();
        condor_cmd(&temp).args(["--overwrite", "scenes"]).assert().failure();
    }

    #[test]
    fn init_and_root_overwrite_flags_persist_consent_and_omission_retains_it() {
        check_overwrite_routes(1);
    }

    #[test]
    fn overwrite_crud_false_or_reset_revokes_consent_and_next_run_refuses_without_resave() {
        check_overwrite_routes(2);
    }

    #[test]
    fn overwrite_flag_is_rejected_on_scenes_route() {
        check_overwrite_routes(3);
    }

    #[test]
    fn combined_root_input_and_temp_edit_matches_cli_and_keeps_custom_paths() {
        use andean_condor::models::input::VapourSynthScriptSource;
        use california_condor::commands::handlers::start::start_handler;
        for custom in [false, true] {
            let temp = tempfile::tempdir().expect("fixture");
            let base = std::fs::canonicalize(temp.path()).expect("canonical fixture");
            for name in ["a.vpy", "b.vpy"] {
                std::fs::write(base.join(name), "from vapoursynth import core, YUV420P8\ncore.std.BlankClip(width=32,height=16,length=12,format=YUV420P8,fpsnum=24,fpsden=1).set_output()\n").expect("finite script");
            }
            let source = base.join("a.vpy");
            let replacement = base.join("b.vpy");
            let video = TestVideo {
                path: source.clone(),
                width: 32,
                height: 16,
                frames: 12,
                fps_rational: (24, 1),
                format: FFPixelFormat::YUV420P,
                scenes: vec![(0, 12)],
            };
            let mut configuration = default_config(
                &video,
                &base.join("out.mkv"),
                &base.join(hash_path(&source)),
            );
            configuration.condor.input = Input::VapourSynthScript {
                source: VapourSynthScriptSource::Path(source),
                variables: Default::default(),
                index: 0,
                filters: vec![],
                stream_concurrently: false,
            };
            if custom {
                configuration.condor.sequence_config.parallel_encoder.scenes_directory =
                    base.join("custom-encoder");
                configuration.condor.sequence_config.scene_concatenator.scenes_directory =
                    Default::default();
            }
            let file = base.join("condor.json");
            configuration.save(&file).expect("save empty job");
            let selected = base.join("selected");
            let (direct, _) = start_handler(
                Some(&file),
                Some(&selected),
                Some(&replacement),
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
                None,
                None,
                None,
                false,
            )
            .expect("empty combined direct handler");
            assert_eq!(direct.input, replacement);
            assert_eq!(direct.temp, selected);
            assert_eq!(
                direct.condor.sequence_config.parallel_encoder.scenes_directory,
                if custom {
                    base.join("custom-encoder")
                } else {
                    selected.join("scenes")
                }
            );
            assert_eq!(
                direct.condor.sequence_config.scene_concatenator.scenes_directory,
                selected.join("scenes")
            );
            assert!(
                !selected.exists(),
                "a handler edit is not artifact migration"
            );
            configuration.save(&file).expect("restore original empty job");
            let output = condor_cmd(&temp)
                .env("PATH", "")
                .env("CONDOR_TEST_MODE", "1")
                .args(["--input", path_str(&replacement), "--temp", "selected"])
                .output()
                .expect("root CLI");
            let message = String::from_utf8_lossy(&output.stderr);
            assert!(
                !message.contains("unexpected argument") && !message.contains("cannot change"),
                "{message}"
            );
            let cli = load(&file);
            assert_eq!(cli.input, replacement);
            assert_eq!(cli.temp, selected);
            assert_eq!(
                cli.condor.sequence_config.parallel_encoder.scenes_directory,
                direct.condor.sequence_config.parallel_encoder.scenes_directory
            );
            assert_eq!(
                cli.condor.sequence_config.scene_concatenator.scenes_directory,
                selected.join("scenes")
            );
            if custom {
                assert!(
                    message.contains(path_str(&base.join("custom-encoder")))
                        && message.contains("--temp does not move those files"),
                    "{message}"
                );
            }
            assert!(!configuration.temp.exists());
            assert!(
                !base.join(hash_path(&replacement)).exists(),
                "input root's intermediate auto root is not created"
            );
        }
    }

    fn check_selected_work_roots(phase: u8) {
        assert!((1..=3).contains(&phase));
        let temp = tempfile::tempdir().expect("fixture");
        let base = std::fs::canonicalize(temp.path()).expect("canonical fixture");
        let source = base.join("blank.vpy");
        std::fs::write(&source, "from vapoursynth import core, YUV420P8\ncore.std.BlankClip(width=32,height=16,length=12,format=YUV420P8,fpsnum=24,fpsden=1).set_output()\n").expect("finite script");
        condor_cmd(&temp)
            .env("PATH", "")
            .args(["init", "blank.vpy", "out.mkv", "--temp", "initial"])
            .assert()
            .success();
        let file = base.join("condor.json");
        let initial = load(&file);
        assert_eq!(initial.temp, base.join("initial"));
        assert_eq!(
            initial.condor.sequence_config.parallel_encoder.scenes_directory,
            base.join("initial/scenes")
        );
        assert_eq!(
            initial.condor.sequence_config.scene_concatenator.scenes_directory,
            base.join("initial/scenes")
        );
        assert!(!base.join("initial").exists());
        if phase == 1 {
            return;
        }
        condor_cmd(&temp)
            .env("PATH", "")
            .env("CONDOR_TEST_MODE", "1")
            .args(["detect-scenes", "--method", "none", "--temp", "detected"])
            .assert()
            .success();
        let after = load(&file);
        assert!(
            !after.condor.scenes.is_empty(),
            "real detection saved scenes"
        );
        assert_eq!(after.temp, base.join("detected"));
        assert_eq!(
            after.condor.sequence_config.parallel_encoder.scenes_directory,
            base.join("detected/scenes")
        );
        assert_eq!(
            after.condor.sequence_config.scene_concatenator.scenes_directory,
            base.join("detected/scenes")
        );
        assert!(
            !base.join("initial").exists(),
            "nothing moved or written to the old root"
        );
        if phase == 2 {
            return;
        }
        let before = std::fs::read(&file).expect("detected job bytes");
        condor_cmd(&temp)
            .env("PATH", "")
            .args(["encode", "--temp", "refused"])
            .assert()
            .failure();
        assert_eq!(std::fs::read(file).expect("unchanged detected job"), before);
        assert!(!base.join("refused").exists());
    }

    #[test]
    fn init_selected_temp_saves_derived_directories_without_creating_work() {
        check_selected_work_roots(1);
    }

    #[test]
    fn detect_scenes_on_empty_job_rebases_to_selected_temp_and_saves_scenes() {
        check_selected_work_roots(2);
    }

    #[test]
    fn encode_temp_change_after_scene_detection_refuses_without_save() {
        check_selected_work_roots(3);
    }

    #[test]
    fn start_handler_rejects_new_root_before_other_edits_and_save() {
        use california_condor::commands::{CondorCli, handlers::start::start_handler};
        use clap::Parser;
        let temp = tempfile::tempdir().expect("fixture");
        let video = get_test_video();
        let mut configuration = default_config(
            &video,
            &temp.path().join("out.mkv"),
            &temp.path().join("work"),
        );
        configuration.condor.scenes = video.mock_scenes(&configuration.condor.encoder);
        let file = temp.path().join("condor.json");
        configuration.save(&file).expect("save");
        let before = std::fs::read(&file).expect("before");
        let cli = CondorCli::try_parse_from([
            "condor",
            "--input",
            "different.mkv",
            "--temp",
            "new-work",
            "--output",
            "new.mkv",
            "--params",
            "--preset 99",
        ])
        .expect("root flags");
        let error = start_handler(
            Some(&file),
            cli.temp.as_deref(),
            cli.input.as_deref(),
            cli.scd_input.as_deref(),
            cli.tq_input.as_deref(),
            cli.output.as_deref(),
            cli.decoder.as_ref(),
            cli.scd_decoder.as_ref(),
            cli.tq_decoder.as_ref(),
            cli.filters.as_deref(),
            cli.scd_filters.as_deref(),
            cli.tq_filters.as_deref(),
            cli.vs_args.as_deref(),
            cli.scd_vs_args.as_deref(),
            cli.tq_vs_args.as_deref(),
            cli.concat.as_ref(),
            cli.discard.as_deref(),
            cli.workers,
            cli.encoder.as_ref(),
            cli.passes,
            cli.params.as_deref(),
            cli.tq_params.as_deref(),
            cli.photon_noise,
            cli.chroma_noise,
            cli.ffmpeg_filter.as_deref(),
            None,
            cli.target_metric.as_ref(),
            cli.target,
            cli.minimum_quantizer,
            cli.maximum_quantizer,
            cli.target_profile.as_ref(),
            cli.max_tries,
            cli.overwrite,
        )
        .expect_err("source guard precedes unrelated encoder validation");
        let message = format!("{error:#}");
        assert!(
            message.contains("different.mkv") && message.contains("--config-file"),
            "{message}"
        );
        assert_eq!(std::fs::read(file).expect("after"), before);
        assert!(!configuration.temp.exists());
    }

    #[test]
    fn init_with_default_options_saves_configuration_without_scenes() {
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
    fn init_with_custom_config_temp_and_output_paths_saves_selected_configuration() {
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
    fn init_with_filters_aom_photon_noise_workers_and_cvvdp_target_saves_selected_options() {
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
            path: input_abs,
            import_method: VapourSynthImportMethod::FFMS2 { index: None },
            cache_path: None,
            filters: vec![
                VapourSynthFilter::Trim {
                    start: Some(24),
                    end: None,
                },
                VapourSynthFilter::Resize {
                    scaler: Some(Scaler::Bilinear),
                    width: Some(1280),
                    height: Some(720),
                    format: Some(FFPixelFormat::YUV420P),
                },
            ],
        };
        let mut custom_encoder_parameters = EncoderBase::AOM.default_parameters();
        custom_encoder_parameters.extend(CLIParameter::new_numbers(
            "--",
            "=",
            &[("cpu-used", 4.0), ("cq-level", 23.0)],
        ));
        expected_config.condor.encoder = Encoder::AOM {
            executable: None,
            pass: EncoderPasses::All(2),
            options: custom_encoder_parameters,
            photon_noise: Some(PhotonNoise {
                iso: 400,
                chroma_iso: None,
                width: None,
                height: None,
                c_y: None,
                ccb: None,
                ccr: None,
            }),
            ffmpeg_filter: None,
            append_color_data: true,
        };
        expected_config.condor.sequence_config.parallel_encoder.workers = Some(8);
        expected_config.condor.sequence_config.target_quality = Some(TargetQualityConfig {
            input: None,
            metric: QualityMetric::CVVDP {
                target_range: (9.4, 9.6),
                resolution: None,
                display_model: None,
                resize_to_display: None,
                disable_temporal: None,
                gpu_id: None,
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
    fn init_existing_config_refuses_and_preserves_original_bytes() {
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

    fn check_init_path_crud(phase: u8) {
        assert!((1..=3).contains(&phase));
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
        if phase == 1 {
            return;
        }

        condor_cmd(&temp)
            .args(["init", "set", "input", "--path", path_str(&video.path), "--decoder", "ffms2"])
            .assert()
            .success();
        let input = load(&config_path).condor.input;
        assert!(
            matches!(
                input,
                Input::Video {
                    import_method: ImportMethod::FFMS2 { .. },
                    ..
                }
            ),
            "root input is {input:?}"
        );

        // A property with no flags changes nothing.
        condor_cmd(&temp).args(["init", "set", "input"]).assert().failure();
        if phase == 2 {
            return;
        }

        // The paths are what `init` exists to capture, so there is no default
        // to restore - not for a key and not for the whole section.
        condor_cmd(&temp).args(["init", "reset", "input"]).assert().failure();
        condor_cmd(&temp).args(["init", "reset", "output"]).assert().failure();
        condor_cmd(&temp).args(["init", "reset"]).assert().failure();
    }

    #[serial]
    #[test]
    fn init_output_crud_updates_root_destination() {
        check_init_path_crud(1);
    }

    #[serial]
    #[test]
    fn init_input_crud_saves_native_decoder_and_requires_at_least_one_flag() {
        check_init_path_crud(2);
    }

    #[serial]
    #[test]
    fn init_path_or_whole_section_reset_has_no_default_and_refuses() {
        check_init_path_crud(3);
    }
}
