#[cfg(test)]
mod handler_configuration {
    use std::{assert_matches, path::Path};

    use andean_condor::{
        ffmpeg::FFPixelFormat,
        models::{
            encoder::{Encoder, EncoderBase, EncoderPasses, cli_parameter::CLIParameter},
            input::{ImportMethod, Input, VapourSynthImportMethod, VapourSynthScriptSource},
            sequence::{
                scene_concatenator::ConcatMethod,
                target_quality::{
                    TargetQualityConfig,
                    types::{
                        ProbeStatistic, ProbeStrategy, QualityMetric, SubsetProbeLength,
                        SubsetProbePosition, TargetQualityProbing,
                    },
                },
            },
        },
        vapoursynth::{plugins::resize::Scaler, vapoursynth_filters::VapourSynthFilter},
    };
    use std::{collections::HashMap, fs};

    use california_condor::{
        commands::{
            ConcatenationMethod, DecoderMethod, EncoderMethod, QualityMetric as QualityMetricBase,
            QualityProfile, handlers::start::start_handler,
        },
        test_helpers::*,
        utils::hash_path::hash_path,
    };

    fn check_custom_start(update: bool) {
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
        let output_abs = path_abs::PathAbs::new(&output)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let config_path = temp.path().join("custom-condor.json");
        let config_path_abs = path_abs::PathAbs::new(&config_path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let custom_filters = vec![VapourSynthFilter::Crop {
            top: Some(42),
            bottom: Some(42),
            left: None,
            right: None,
        }];
        let custom_scd_filters = vec![VapourSynthFilter::Crop {
            top: None,
            bottom: None,
            left: Some(100),
            right: Some(100),
        }];
        let custom_tq_filters = vec![VapourSynthFilter::Resize {
            scaler: Some(Scaler::Lanczos),
            width: Some(960),
            height: Some(540),
            format: Some(FFPixelFormat::YUV444P10LE),
        }];
        let custom_vs_args = vec!["key=value".to_string()];
        let mut custom_encoder_parameters = EncoderBase::X265.default_parameters();
        custom_encoder_parameters.insert(
            "preset".to_owned(),
            CLIParameter::new_string("--", " ", "slow"),
        );
        custom_encoder_parameters
            .insert("crf".to_owned(), CLIParameter::new_number("--", " ", 18.0));
        let mut expected_config = default_config(&test_video, &output, &temp_abs);
        expected_config.condor.input = Input::Video {
            path: input_abs.clone(),
            import_method: ImportMethod::FFMS2 {
                index: None,
                cache_path: None,
            },
            // Native FFMS2 cannot apply the requested crop after replacing default filters.
            filters: Vec::new(),
        };
        expected_config.condor.encoder = Encoder::X265 {
            executable: None,
            pass: EncoderPasses::All(2),
            options: custom_encoder_parameters,
            ffmpeg_filter: None,
            append_color_data: true,
        };
        expected_config.condor.sequence_config.scene_detector.input = Some(Input::VapourSynth {
            path: input_abs.clone(),
            import_method: VapourSynthImportMethod::BestSource { index: None },
            cache_path: None,
            filters: custom_scd_filters.clone(),
        });
        expected_config.condor.sequence_config.parallel_encoder.workers = Some(6);
        let mut tq_encoder_parameters = EncoderBase::X265.default_parameters();
        tq_encoder_parameters.insert(
            "preset".to_owned(),
            CLIParameter::new_string("--", " ", "ultrafast"),
        );
        expected_config.condor.sequence_config.target_quality = Some(TargetQualityConfig {
            input: Some(Input::VapourSynth {
                path: input_abs.clone(),
                import_method: VapourSynthImportMethod::FFMS2 { index: None },
                cache_path: None,
                filters: custom_tq_filters.clone(),
            }),
            metric: QualityMetric::XPSNR {
                target_range: (39.0, 41.0),
                resolution: None,
            },
            quantizer_range: (4, 35),
            probing: TargetQualityProbing {
                encoder_options: Some(tq_encoder_parameters),
                statistic: ProbeStatistic::Percentile(10.0),
                strategy: ProbeStrategy::Whole,
            },
            ..Default::default()
        });
        expected_config.condor.sequence_config.scene_concatenator.method = ConcatMethod::Ivf;
        let expected_config = expected_config;
        let (config, found_config_path) = start_handler(
            Some(&config_path),
            Some(&temp.path().join("custom-temp")),
            Some(&test_video.path),
            Some(&test_video.path),
            Some(&test_video.path),
            Some(&output),
            Some(&DecoderMethod::FFMS2),
            Some(&DecoderMethod::BestSource),
            Some(&DecoderMethod::VSFFMS2),
            Some(&custom_filters),
            Some(&custom_scd_filters),
            Some(&custom_tq_filters),
            Some(&custom_vs_args.clone()),
            Some(&custom_vs_args.clone()),
            Some(&custom_vs_args),
            Some(&ConcatenationMethod::Ivf),
            None,
            Some(6),
            Some(&EncoderMethod::X265),
            Some(2),
            Some("--preset slow --crf 18"),
            Some("--preset ultrafast"),
            None,
            None,
            None,
            None,
            Some(&QualityMetricBase::XPSNR),
            Some(40.0),
            Some(4),
            Some(35),
            Some(&QualityProfile::Slow),
            None,
            false,
            &Default::default(),
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
        if !update {
            return;
        }
        drop(config);
        drop(found_config_path);
        let script_input = temp.path().join("condor-test-script.vpy");
        let script_input_abs = path_abs::PathAbs::new(&script_input)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let vpy_script = vapoursynth_script(&test_video, Some(&custom_filters));
        fs::write(&script_input, vpy_script).expect("write should succeed");
        let mut expected_config = expected_config;
        let mut custom_variables = HashMap::new();
        custom_variables.insert("key".to_owned(), "value".to_owned());
        expected_config.input = script_input_abs.clone();
        expected_config.condor.input = Input::VapourSynthScript {
            source: VapourSynthScriptSource::Path(script_input_abs),
            variables: custom_variables,
            index: 0,
            filters: custom_filters.clone(),
            stream_concurrently: true,
        };
        let mut custom_encoder_parameters = EncoderBase::X264.default_parameters();
        custom_encoder_parameters.insert(
            "preset".to_owned(),
            CLIParameter::new_string("--", " ", "medium"),
        );
        custom_encoder_parameters
            .insert("crf".to_owned(), CLIParameter::new_number("--", " ", 12.0));
        expected_config.condor.encoder = Encoder::X264 {
            executable: None,
            pass: EncoderPasses::All(1),
            options: custom_encoder_parameters.clone(),
            ffmpeg_filter: None,
            append_color_data: true,
        };
        if let Some(ref mut tq) = expected_config.condor.sequence_config.target_quality {
            tq.metric = QualityMetric::CVVDP {
                target_range: (9.4, 9.6),
                resolution: None,
                display_model: None,
                resize_to_display: None,
                disable_temporal: None,
                gpu_id: None,
            };
            tq.probing.statistic = ProbeStatistic::RootMeanSquare;
            tq.probing.strategy = ProbeStrategy::Subset {
                position: SubsetProbePosition::Middle,
                length: SubsetProbeLength::Percentage(25.0),
            };
            tq.quantizer_range.1 = 25;
        }
        expected_config.condor.sequence_config.parallel_encoder.workers = Some(12);
        expected_config.condor.sequence_config.scene_concatenator.method =
            ConcatMethod::MKVMerge(Default::default());
        let expected_config = expected_config;
        let (config, found_config_path) = start_handler(
            Some(&config_path),
            Some(&temp.path().join("custom-temp")),
            Some(&script_input),
            Some(&test_video.path),
            Some(&test_video.path),
            Some(&output),
            None,
            None,
            None,
            Some(&custom_filters),
            None,
            None,
            Some(&custom_vs_args),
            None,
            None,
            Some(&ConcatenationMethod::MkvMerge),
            None,
            Some(12),
            Some(&EncoderMethod::X264),
            Some(1),
            Some("--preset medium --crf 12"),
            None,
            None,
            None,
            None,
            None,
            Some(&QualityMetricBase::CVVDP),
            Some(9.5),
            Some(4),
            Some(25),
            Some(&QualityProfile::Standard),
            None,
            false,
            &Default::default(),
        )
        .expect("start_handler should succeed");
        assert!(config_path.exists(), "config file exists");
        assert_eq!(
            found_config_path,
            config_path_abs,
            "config path is {}",
            config_path_abs.display()
        );
        assert_eq!(config.temp, temp_abs, "temp path is {}", temp_abs.display());
        assert_eq!(
            config.input,
            expected_config.input,
            "input path is {}",
            expected_config.input.display()
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
    fn start_new_job_applies_native_input_encoder_stage_overrides_workers_and_ivf_method() {
        check_custom_start(false);
    }

    #[test]
    fn start_existing_job_replaces_requested_input_encoder_target_workers_and_concatenation_settings()
     {
        check_custom_start(true);
    }

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
            &Default::default(),
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
            &Default::default(),
        )
        .expect("start_handler should succeed");
        assert!(
            !config.condor.encoder.append_color_data(),
            "append_color_data is false"
        );
    }

    #[test]
    fn start_invalid_saved_scene_lengths_do_not_publish_configuration() {
        use crate::common::config::{init_config, load};
        use andean_condor::models::sequence::scene_detector::SceneDetectionMethod;

        let temp = tempfile::tempdir().expect("test directory");
        let config_path = init_config(&temp);
        let mut configuration = load(&config_path);
        for (minimum_length, maximum_length, diagnostic) in
            [(0, 0, "at least one frame"), (48, 24, "exceeds maximum length")]
        {
            configuration.condor.sequence_config.scene_detector.method =
                SceneDetectionMethod::None {
                    minimum_length,
                    maximum_length,
                };
            configuration.save(&config_path).expect("save invalid lengths");
            let before = fs::read(&config_path).expect("saved job");
            let result = start_handler(
                Some(&config_path),
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
                Some(2),
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
                &Default::default(),
            );
            let error = result.expect_err("invalid saved lengths must fail preparation");
            assert!(error.to_string().contains(diagnostic), "{error:#}");
            assert_eq!(
                fs::read(&config_path).expect("saved job after rejection"),
                before,
                "failed preparation must not publish the worker override"
            );
        }
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
            &Default::default(),
        );
        assert_matches!(result, Err(_), "start_handler should fail");
        assert!(!config_path.exists(), "config file does not exist");
    }
}
