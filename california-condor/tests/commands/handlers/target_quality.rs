use crate::common::condor_cmd;
use crate::common::config::{init_config, load};
use andean_condor::models::{
    encoder::{EncoderBase, cli_parameter::CLIParameter},
    sequence::target_quality::{
        TargetQualityConfig,
        types::{QualityMetric, TargetQualityProbing},
    },
};
use california_condor::{
    commands::handlers::load_configuration,
    test_helpers::*,
    utils::{hash_path::hash_path, parameter_parser::EncoderParamsParser},
};

#[cfg(test)]
mod handler_configuration {
    use super::*;
    use andean_condor::{
        models::{
            input::{Input, VapourSynthImportMethod},
            sequence::target_quality::types::{
                InterpolationMethod, ProbeStatistic, ProbeStrategy, SubsetProbeLength,
                SubsetProbePosition,
            },
        },
        vapoursynth::vapoursynth_filters::VapourSynthFilter,
    };
    use california_condor::commands::{
        DecoderMethod, QualityMetric as QualityMetricBase, QualityProfile,
        handlers::{init::init_handler, target_quality::target_quality_handler},
    };

    #[test]
    fn target_quality_handler_without_overrides_retains_checked_defaults_and_config_path() {
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
            &Default::default(),
        )
        .expect("init_handler should succeed");
        let (config, found_config_path) = target_quality_handler(
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
        )
        .expect("target_quality_handler should succeed");
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
    fn target_quality_handler_applies_butteraugli_target_quantizer_range_and_probe_settings() {
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
        let custom_filters = vec![VapourSynthFilter::Crop {
            top: Some(140),
            bottom: Some(140),
            left: None,
            right: None,
        }];
        let custom_vs_args = vec!["method=target quality".to_string()];
        let mut expected_config = default_config(&test_video, &output, &temp_abs);
        let mut tq_encoder_parameters = EncoderBase::SVTAV1.default_parameters();
        tq_encoder_parameters.extend(CLIParameter::new_numbers(
            "--",
            " ",
            &[("preset", 6.0), ("tune", 3.0)],
        ));
        expected_config.condor.sequence_config.target_quality = Some(TargetQualityConfig {
            metric: QualityMetric::BUTTERAUGLI {
                target_range: (1.4, 1.6),
                resolution: None,
                threads: None,
                intensity_multiplier: None,
                norm: None,
                gpu_id: None,
            },
            maximum_probes: 4,
            quantizer_range: (8, 40),
            interpolators: (
                InterpolationMethod::FritschCarlson,
                InterpolationMethod::Pchip,
            ),
            input: Some(Input::VapourSynth {
                path: input_abs.clone(),
                import_method: VapourSynthImportMethod::LSMASHWorks { index: None },
                cache_path: None,
                filters: custom_filters.clone(),
            }),
            metric_input: None,
            probing: TargetQualityProbing {
                encoder_options: Some(tq_encoder_parameters),
                statistic: ProbeStatistic::Mean,
                strategy: ProbeStrategy::Subset {
                    position: SubsetProbePosition::Middle,
                    length: SubsetProbeLength::Frames(11),
                },
            },
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
            &Default::default(),
        )
        .expect("init_handler should succeed");
        let (config, found_config_path) = target_quality_handler(
            Some(&config_path),
            Some(&temp.path().join(hash_path(&input_abs))),
            Some(&test_video.path),
            Some(&DecoderMethod::LSMASHWorks),
            Some(&custom_filters),
            Some(&custom_vs_args),
            Some("--preset 6 --tune 3"),
            Some(&QualityMetricBase::BUTTERAUGLI),
            Some(1.5),
            Some(8),
            Some(40),
            Some(&QualityProfile::Fast),
        )
        .expect("target_quality_handler should succeed");
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
mod flag_configuration {
    use super::*;
    use andean_condor::{
        core::sequence::target_quality::TargetQuality,
        models::{
            input::Input,
            sequence::target_quality::types::{
                DEFAULT_XPSNR_TARGET_RANGE, ProbeStatistic, ProbeStrategy, SubsetProbeLength,
                SubsetProbePosition,
            },
        },
    };
    use anyhow::Result;
    use california_condor::{
        commands::handlers::target_quality::configure_target_quality, configuration::Configuration,
    };
    use std::path::Path;

    fn apply_target_quality_flags(configuration: &mut Configuration, flags: &[&str]) -> Result<()> {
        use california_condor::commands::{Commands, CondorCli};
        use clap::Parser;
        let cli = CondorCli::try_parse_from([&["condor", "target-quality"][..], flags].concat())?;
        let Some(Commands::TargetQuality {
            input,
            decoder,
            filters,
            vs_args,
            params,
            metric,
            target,
            minimum_quantizer,
            maximum_quantizer,
            profile,
            ..
        }) = cli.command
        else {
            panic!("TQ flags")
        };
        configure_target_quality(
            configuration,
            input.as_deref(),
            decoder.as_ref(),
            filters.as_deref(),
            vs_args.as_deref(),
            params.as_deref(),
            metric.as_ref(),
            target,
            minimum_quantizer,
            maximum_quantizer,
            profile.as_ref(),
        )
    }

    fn offline_target_quality_configuration(directory: &Path) -> Configuration {
        let video = california_condor::test_helpers::TestVideo {
            path: directory.join("must-not-open.vpy"),
            width: 32,
            height: 32,
            frames: 2,
            fps_rational: (24, 1),
            format: andean_condor::ffmpeg::FFPixelFormat::YUV420P,
            scenes: vec![(0, 2)],
        };
        let mut configuration =
            default_config(&video, &directory.join("out.mkv"), &directory.join("work"));
        configuration.condor.scenes = video.mock_scenes(&configuration.condor.encoder);
        configuration.condor.scenes[0].sequence_data.parallel_encoder.bytes = Some(1234);
        configuration
    }

    #[test]
    fn target_quality_existing_targetless_edits_change_only_tq_and_create_no_scratch() {
        let temp = tempfile::tempdir().expect("fixture");
        let mut configuration = offline_target_quality_configuration(temp.path());
        let before = serde_json::to_value(&configuration).expect("snapshot");
        apply_target_quality_flags(&mut configuration, &[])
            .expect("intentional absent/allNone no-op");
        assert_eq!(
            serde_json::to_value(&configuration).expect("unchanged"),
            before
        );
        apply_target_quality_flags(&mut configuration, &["--target", "75"])
            .expect("explicit creation");
        let quality =
            configuration.condor.sequence_config.target_quality.as_ref().expect("created");
        assert_eq!(quality.metric.target_range(), (74.0, 76.0));
        assert_eq!(
            quality.quantizer_range,
            TargetQuality::default_quantizer_range(&configuration.condor.encoder.base())
        );
        assert_eq!(
            serde_json::to_value(&quality.probing).expect("default probing"),
            serde_json::to_value(TargetQualityConfig::default().probing).expect("defaults")
        );
        let untouched = configuration.clone();
        apply_target_quality_flags(
            &mut configuration,
            &[
                "--metric",
                "xpsnr",
                "--profile",
                "fast",
                "--min-q",
                "20",
                "--max-q",
                "40",
                "--params",
                "--preset=8",
            ],
        )
        .expect("saved section is sufficient");
        let quality = configuration.condor.sequence_config.target_quality.as_ref().expect("TQ");
        assert!(matches!(quality.metric, QualityMetric::XPSNR { .. }));
        assert_eq!(quality.metric.target_range(), DEFAULT_XPSNR_TARGET_RANGE);
        assert_eq!(quality.quantizer_range, (20, 40));
        assert_eq!(
            quality.probing.strategy,
            ProbeStrategy::Subset {
                position: SubsetProbePosition::Middle,
                length: SubsetProbeLength::Frames(11)
            }
        );
        assert_eq!(quality.probing.statistic, ProbeStatistic::Mean);
        assert_eq!(
            quality.probing.encoder_options.as_ref().expect("params").get("preset"),
            Some(&CLIParameter::new_number("--", "=", 8.0))
        );
        apply_target_quality_flags(
            &mut configuration,
            &["--input", "other.vpy", "--vs-args", "keep=yes"],
        )
        .expect("configured input edit stays offline");
        let input = configuration
            .condor
            .sequence_config
            .target_quality
            .as_ref()
            .expect("TQ")
            .input
            .as_ref()
            .expect("override");
        assert!(matches!(input, Input::VapourSynthScript { .. }));
        let mut outside_tq = configuration.clone();
        outside_tq.condor.sequence_config.target_quality =
            untouched.condor.sequence_config.target_quality.clone();
        assert_eq!(
            serde_json::to_value(outside_tq).expect("unchanged outside TQ"),
            serde_json::to_value(untouched).expect("snapshot")
        );
        assert!(!configuration.temp.exists());
    }
}

#[cfg(test)]
mod tests {
    use std::assert_matches;

    use andean_condor::{
        ffmpeg::FFPixelFormat,
        models::{
            input::{Input, VapourSynthImportMethod},
            sequence::target_quality::types::{
                DEFAULT_SSIMULACRA2_TARGET_RANGE, DEFAULT_XPSNR_TARGET_RANGE, ProbeStatistic,
                ProbeStrategy, QualityMetric as MetricModel, SubsetProbeLength,
                SubsetProbePosition,
            },
        },
        vapoursynth::vapoursynth_filters::VapourSynthFilter,
    };
    use serial_test::serial;

    use super::*;

    fn check_missing_target(phase: u8) {
        assert!((1..=3).contains(&phase));
        use california_condor::commands::{
            QualityMetric as CliMetric,
            handlers::target_quality::{TargetQualityConfigError, target_quality_handler},
        };
        let temp = tempfile::tempdir().expect("fixture");
        let source = temp.path().join("source.vpy");
        let bytes = format!(
            "from pathlib import Path\nPath({}).write_text('opened')\nraise RuntimeError('TARGET_QUALITY_SOURCE_WAS_OPENED')\n",
            serde_json::to_string(&temp.path().join("opened")).expect("literal")
        );
        std::fs::write(&source, &bytes).expect("script");
        let video = TestVideo {
            path: source.clone(),
            width: 32,
            height: 32,
            frames: 2,
            fps_rational: (24, 1),
            format: FFPixelFormat::YUV420P,
            scenes: vec![(0, 2)],
        };
        let mut config = default_config(
            &video,
            &temp.path().join("out.mkv"),
            &temp.path().join("work"),
        );
        config.condor.scenes = video.mock_scenes(&config.condor.encoder);
        config.condor.scenes[0].sequence_data.parallel_encoder.bytes = Some(1234);
        let file = temp.path().join("condor.json");
        config.save(&file).expect("fixture save");
        let before = std::fs::read(&file).expect("snapshot");
        for args in [vec!["target-quality", "--metric", "vmaf"], vec!["--target-metric", "vmaf"]] {
            let result = condor_cmd(&temp).env("PATH", "").args(&args).output().expect("CLI");
            let message = String::from_utf8_lossy(&result.stderr);
            assert!(
                !result.status.success()
                    && message.contains("--target")
                    && message.contains("saved target"),
                "{args:?}: {message}"
            );
            assert_eq!(std::fs::read(&file).expect("no save"), before);
            assert!(!temp.path().join("opened").exists());
        }
        if phase == 1 {
            return;
        }
        let error = target_quality_handler(
            Some(&file),
            None,
            None,
            None,
            None,
            None,
            None,
            Some(&CliMetric::VMAF),
            None,
            None,
            None,
            None,
        )
        .expect_err("missing target");
        assert!(
            matches!(
                error.downcast_ref::<TargetQualityConfigError>(),
                Some(TargetQualityConfigError::MissingTarget)
            ),
            "{error:#}"
        );
        assert_eq!(std::fs::read(&file).expect("no save"), before);
        if phase == 2 {
            return;
        }
        // Positive settings-only control; legacy claimed chunks now require C14
        // admission rather than being resaved by a run handler.
        config.condor.scenes[0].sequence_data.parallel_encoder = Default::default();
        config.condor.sequence_config.target_quality = Some(TargetQualityConfig::default());
        config.save(&file).expect("configured job");
        let (edited, _) = target_quality_handler(
            Some(&file),
            None,
            None,
            None,
            None,
            None,
            None,
            Some(&CliMetric::VMAF),
            None,
            Some(20),
            Some(40),
            None,
        )
        .expect("configured targetless edit saves offline");
        assert!(matches!(
            edited.condor.sequence_config.target_quality.as_ref().expect("TQ").metric,
            QualityMetric::VMAF { .. }
        ));
        assert_eq!(
            serde_json::to_value(edited.condor.scenes).expect("history"),
            serde_json::to_value(config.condor.scenes).expect("snapshot")
        );
        assert_eq!(
            serde_json::to_value(edited.condor.output).expect("output"),
            serde_json::to_value(config.condor.output).expect("snapshot")
        );
        assert_eq!(std::fs::read(source).expect("source"), bytes.as_bytes());
        assert!(!temp.path().join("opened").exists());
        assert!(!temp.path().join("out.mkv").exists());
    }

    #[test]
    fn target_quality_cli_without_saved_target_refuses_before_source_open_or_save() {
        check_missing_target(1);
    }

    #[test]
    fn target_quality_handler_missing_target_returns_typed_error_without_save() {
        check_missing_target(2);
    }

    #[test]
    fn target_quality_configured_targetless_handler_saves_metric_without_changing_scene_history_or_output()
     {
        check_missing_target(3);
    }

    #[serial]
    #[test]
    fn target_quality_custom_xpsnr_profile_persists_options_and_bounded_scene_probe_quantizers() {
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
        let mut tq_encoder_params = EncoderBase::SVTAV1.default_parameters();
        tq_encoder_params.extend(CLIParameter::new_numbers(
            "--",
            " ",
            &[("preset", 8.0), ("tune", 1.0)],
        ));

        // Mock an existing config file with scenes
        let mut config = default_config(&test_video, &output, &temp_abs);
        // Allow testing with SVT Essential (does not support 8-bit)
        if let Input::VapourSynth { filters, .. } = &mut config.condor.input {
            *filters = vec![VapourSynthFilter::Resize {
                scaler: None,
                width: None,
                height: None,
                format: Some(FFPixelFormat::YUV420P10LE),
            }];
        }
        config.condor.encoder.parameters_mut().insert(
            "preset".to_owned(),
            CLIParameter::new_number("--", " ", 6.0),
        );
        config.condor.scenes = test_video.mock_scenes(&config.condor.encoder);
        config.save(&config_path).expect("configuration save should succeed");

        condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .args([
                "target-quality",
                "--input",
                path_str(&test_video.path),
                "--decoder",
                "vs-ffms2",
                "--filters",
                "resize:format=yuv420p10le;",
                "--metric",
                "xpsnr",
                "--target",
                "45",
                "--min-q",
                "20",
                "--max-q",
                "40",
                "--profile",
                "fast",
                "--params",
                "--preset 8 --tune 1",
            ])
            .assert()
            .success();

        let mut expected_config = config.clone();
        expected_config.condor.sequence_config.target_quality = Some(TargetQualityConfig {
            metric: QualityMetric::XPSNR {
                target_range: (44.0, 46.0),
                resolution: None,
            },
            quantizer_range: (20, 40),
            input: Some(Input::VapourSynth {
                path: input_abs,
                import_method: VapourSynthImportMethod::FFMS2 { index: None },
                cache_path: None,
                filters: config.condor.input.vapoursynth_filters().to_vec(),
            }),
            probing: TargetQualityProbing {
                encoder_options: Some(tq_encoder_params),
                strategy: ProbeStrategy::Subset {
                    position: SubsetProbePosition::Middle,
                    length: SubsetProbeLength::Frames(11),
                },
                statistic: ProbeStatistic::Mean,
            },
            ..Default::default()
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
        let quantizer_range = config
            .condor
            .sequence_config
            .target_quality
            .as_ref()
            .expect("Target Quality sequence_config should exist")
            .quantizer_range;
        config.condor.scenes.iter().enumerate().for_each(|(index, scene)| {
            assert!(
                !scene.sequence_data.target_quality.passes.is_empty(),
                "scene {} should have Target Quality passes",
                index
            );
            let maximum_probes = config
                .condor
                .sequence_config
                .target_quality
                .as_ref()
                .expect("Target Quality sequence_config should exist")
                .maximum_probes;
            assert!(
                scene.sequence_data.target_quality.passes.len() <= maximum_probes as usize,
                "scene {} should have at most {} Target Quality passes",
                index,
                maximum_probes
            );
            let crf = scene.encoder.parameters().get("crf").expect("crf should exist");
            assert_matches!(
                crf,
                CLIParameter::Number { .. },
                "scene {} crf should be a number",
                index
            );
            let crf_value = match crf {
                CLIParameter::Number { value, .. } => *value,
                _ => panic!("scene {} crf should be a number", index),
            };
            assert!(
                crf_value >= quantizer_range.0 as f64 && crf_value <= quantizer_range.1 as f64,
                "scene {} crf {} should be within {} and {} CRF",
                index,
                crf_value,
                quantizer_range.0,
                quantizer_range.1
            );
        });
    }

    fn check_target_metric_range(phase: u8) {
        assert!((1..=3).contains(&phase));
        if !ffmpeg_is_available() {
            return;
        }
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let config_path = init_config(&temp);

        condor_cmd(&temp)
            .args(["target-quality", "set", "target", "90.5"])
            .assert()
            .success();
        let quality = load(&config_path)
            .condor
            .sequence_config
            .target_quality
            .expect("target quality was materialized");
        assert_eq!(
            quality.metric.target_range(),
            (89.5, 91.5),
            "the target is folded into the metric range"
        );

        condor_cmd(&temp).args(["target-quality", "reset", "target"]).assert().success();
        let quality = load(&config_path)
            .condor
            .sequence_config
            .target_quality
            .expect("target quality exists");
        assert!(
            matches!(quality.metric, MetricModel::SSIMULACRA2 { .. }),
            "the metric is unchanged: {:?}",
            quality.metric
        );
        assert_eq!(
            quality.metric.target_range(),
            DEFAULT_SSIMULACRA2_TARGET_RANGE,
            "reset restores the metric's default range"
        );
        if phase == 1 {
            return;
        }

        condor_cmd(&temp)
            .args(["target-quality", "set", "metric", "xpsnr"])
            .assert()
            .success();
        let quality = load(&config_path)
            .condor
            .sequence_config
            .target_quality
            .expect("target quality exists");
        assert!(
            matches!(quality.metric, MetricModel::XPSNR { .. }),
            "the metric switched: {:?}",
            quality.metric
        );
        assert_eq!(
            quality.metric.target_range(),
            DEFAULT_XPSNR_TARGET_RANGE,
            "switching the metric resets its target range"
        );

        condor_cmd(&temp)
            .args(["target-quality", "set", "quantizer-range", "30-10"])
            .assert()
            .failure();
        condor_cmd(&temp)
            .args(["target-quality", "set", "quantizer-range", "18-30"])
            .assert()
            .success();
        // Out of the documented probe count.
        condor_cmd(&temp)
            .args(["target-quality", "set", "maximum-probes", "0"])
            .assert()
            .failure();
        condor_cmd(&temp)
            .args(["target-quality", "set", "maximum-probes", "12"])
            .assert()
            .success();
        let quality = load(&config_path)
            .condor
            .sequence_config
            .target_quality
            .expect("target quality exists");
        assert_eq!(quality.quantizer_range, (18, 30), "range is set");
        assert_eq!(quality.maximum_probes, 12, "probes are set");
        if phase == 3 {
            return;
        }

        let output = condor_cmd(&temp)
            .args(["target-quality", "info", "target"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let target: serde_json::Value =
            serde_json::from_slice(&output).expect("info prints valid JSON");
        assert_eq!(
            target,
            serde_json::to_value(DEFAULT_XPSNR_TARGET_RANGE).expect("range serializes"),
            "info target reports the metric's target range: {target}"
        );
    }

    #[serial]
    #[test]
    fn target_quality_target_crud_materializes_metric_range_and_reset_restores_metric_default() {
        check_target_metric_range(1);
    }

    #[serial]
    #[test]
    fn target_quality_metric_switch_restores_new_metric_default_target_range_and_info_reports_it() {
        check_target_metric_range(2);
    }

    #[serial]
    #[test]
    fn target_quality_quantizer_range_and_probe_budget_crud_reject_invalid_values_and_save_valid_values()
     {
        check_target_metric_range(3);
    }

    fn check_probing_crud(params: bool) {
        if !ffmpeg_is_available() {
            return;
        }
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let config_path = init_config(&temp);

        condor_cmd(&temp)
            .args(["target-quality", "set", "statistic", "percentile", "--percentile", "90"])
            .assert()
            .success();
        let quality = load(&config_path)
            .condor
            .sequence_config
            .target_quality
            .expect("target quality was materialized");
        assert_eq!(
            quality.probing.statistic,
            ProbeStatistic::Percentile(90.0),
            "the probe statistic is set"
        );

        // The params string is the run flag's, and omitted options stay.
        if params {
            condor_cmd(&temp)
                .args(["target-quality", "set", "params", "crf=16"])
                .assert()
                .success();
            condor_cmd(&temp)
                .args(["target-quality", "set", "params", "tune=0"])
                .assert()
                .success();
            let config = load(&config_path);
            let quality =
                config.condor.sequence_config.target_quality.expect("target quality exists");
            let mut expected = config.condor.encoder.base().default_parameters();
            expected.extend(EncoderParamsParser::parse_string("crf=16").expect("parses"));
            expected.extend(EncoderParamsParser::parse_string("tune=0").expect("parses"));
            assert_eq!(
                quality.probing.encoder_options,
                Some(expected),
                "a later set params keeps the earlier pairs"
            );
        }

        // info prints the property, reset resets it.
        let output = condor_cmd(&temp)
            .args(["target-quality", "info", "statistic"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let statistic: serde_json::Value =
            serde_json::from_slice(&output).expect("info prints valid JSON");
        assert_eq!(
            statistic,
            serde_json::to_value(ProbeStatistic::Percentile(90.0)).expect("serializes"),
            "info statistic reports the probe statistic"
        );

        condor_cmd(&temp)
            .args(["target-quality", "reset", "statistic"])
            .assert()
            .success();
        if params {
            condor_cmd(&temp).args(["target-quality", "reset", "params"]).assert().success();
        }
        let quality = load(&config_path)
            .condor
            .sequence_config
            .target_quality
            .expect("target quality exists");
        assert_eq!(
            quality.probing.statistic,
            ProbeStatistic::default(),
            "reset restores the default statistic"
        );
        if params {
            assert!(
                quality.probing.encoder_options.is_none(),
                "reset drops the probing encoder options"
            );
        }
    }

    #[serial]
    #[test]
    fn target_quality_statistic_crud_reports_saved_percentile_and_reset_restores_default() {
        check_probing_crud(false);
    }

    #[serial]
    #[test]
    fn target_quality_probe_params_crud_merges_omitted_pairs_and_reset_drops_override() {
        check_probing_crud(true);
    }
}

#[cfg(test)]
pub(super) mod validation {
    use crate::common;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    };

    use andean_condor::{
        core::{
            Condor,
            input::Input,
            output::Output,
            sequence::{Sequence, target_quality::TargetQuality},
        },
        models::{
            encoder::{Encoder, EncoderBase},
            input::{ImportMethod, Input as InputModel, VapourSynthScriptSource},
            sequence::target_quality::TargetQualityConfig,
        },
    };
    use california_condor::{
        commands::{
            config::ConfigAction,
            handlers::target_quality::{self, TargetQualityKey, TargetQualitySet},
        },
        configuration::{CliSequenceConfig, CliSequenceData, Configuration},
        test_helpers::{TestVideo, default_config},
        tui::run_target_quality_tui,
    };

    pub(in super::super) fn fixture(temp: &std::path::Path) -> Configuration {
        let video = TestVideo {
            path: temp.join("must-not-open.mkv"),
            width: 32,
            height: 32,
            frames: 100,
            fps_rational: (24, 1),
            format: andean_condor::ffmpeg::FFPixelFormat::YUV420P,
            scenes: vec![(0, 50), (50, 100)],
        };
        let mut configuration =
            default_config(&video, &temp.join("output.mkv"), &temp.join("scratch"));
        configuration.condor.scenes = video.mock_scenes(&configuration.condor.encoder);
        configuration.condor.input = InputModel::Video {
            path: video.path,
            import_method: ImportMethod::FFMS2 {
                index: Some(0),
                cache_path: Some(temp.join("forbidden.ffindex")),
            },
            filters: vec![],
        };
        configuration.condor.encoder.set_executable(Some(temp.join("must-not-run")));
        configuration.condor.sequence_config.target_quality = Some(TargetQualityConfig {
            input: Some(configuration.condor.input.clone()),
            ..Default::default()
        });
        configuration
    }

    pub(in super::super) fn blank_input() -> InputModel {
        InputModel::VapourSynthScript {
        source: VapourSynthScriptSource::Text("from vapoursynth import core, YUV420P8\ncore.std.BlankClip(width=32,height=32,length=2,format=YUV420P8).set_output()\n".into()),
        variables: Default::default(), index: 0, filters: vec![], stream_concurrently: false,
        }
    }

    fn runtime(
        configuration: &Configuration,
        saved: Arc<AtomicUsize>,
    ) -> Condor<CliSequenceData, CliSequenceConfig> {
        Condor::new(
            Input::from_data(&blank_input()).expect("blank input"),
            Output::new(&configuration.condor.output).expect("output"),
            configuration.condor.encoder.clone(),
            configuration.condor.scenes.clone(),
            Some(configuration.condor.sequence_config.clone()),
            Box::new(move |_| {
                saved.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }),
        )
    }

    #[test]
    fn target_quality_cli_invalid_rav1e_bounds_or_nonfinite_target_refuse_before_save_or_input_work()
     {
        let temp = tempfile::tempdir().expect("fixture");
        let mut configuration = fixture(temp.path());
        configuration.condor.encoder = Encoder::default_from_base(&EncoderBase::RAV1E, false);
        configuration.condor.sequence_config.target_quality = None;
        let path = temp.path().join("condor.json");
        configuration.save(&path).expect("save");
        let before = std::fs::read(&path).expect("snapshot");
        for args in [vec!["--target", "75", "--max-q", "40"], vec!["--target", "NaN"]] {
            let output = common::condor_cmd(&temp)
                .arg("target-quality")
                .args(args)
                .output()
                .expect("CLI");
            assert!(!output.status.success());
            let message = String::from_utf8_lossy(&output.stderr);
            assert!(
                message.contains("quantizer") || message.contains("finite"),
                "{message}"
            );
            assert_eq!(std::fs::read(&path).expect("unchanged bytes"), before);
        }
        assert!(!configuration.temp.exists());
        assert!(!temp.path().join("forbidden.ffindex").exists());
    }

    #[test]
    fn target_quality_public_entries_reject_invalid_loaded_config_before_save_progress_or_backend_work()
     {
        let temp = tempfile::tempdir().expect("fixture");
        for invalid in
            ["range", "target", "probes", "scene", "options", "nan", "infinity", "endpoint"]
        {
            for entry in ["validate", "initialize", "execute", "tui", "instantiate"] {
                let mut configuration = fixture(temp.path());
                let quality =
                    configuration.condor.sequence_config.target_quality.as_mut().expect("TQ");
                let expected = match invalid {
                    "range" => {
                        quality.quantizer_range = (40, 20);
                        "quantizer"
                    },
                    "target" => {
                        *quality.metric.target_range_mut() = (90.0, 70.0);
                        "target"
                    },
                    "probes" => {
                        quality.maximum_probes = 0;
                        "probes"
                    },
                    "endpoint" => {
                        quality.quantizer_range = (71, 71);
                        "quantizer"
                    },
                    "nan" => {
                        quality.metric.target_range_mut().0 = f64::NAN;
                        "finite"
                    },
                    "infinity" => {
                        quality.metric.target_range_mut().1 = f64::INFINITY;
                        "finite"
                    },
                    "scene" => {
                        quality.quantizer_range = (50, 70);
                        configuration.condor.scenes[1].encoder =
                            Encoder::default_from_base(&EncoderBase::X264, false);
                        "scene 1"
                    },
                    _ => {
                        quality.probing.encoder_options = Some(
                        andean_condor::models::encoder::cli_parameter::CLIParameter::new_numbers(
                            "--",
                            " ",
                            &[("rc", 1.0)],
                        ),
                    );
                        "rc"
                    },
                };
                let path = temp.path().join("condor.json");
                // Simulate hand-edited JSON, not a validated save.
                let nonfinite = matches!(invalid, "nan" | "infinity");
                if nonfinite {
                    // JSON cannot represent nonfinite scores; public library callers can.
                    std::fs::write(&path, b"nonfinite library sentinel").expect("fixture");
                } else {
                    std::fs::write(&path, serde_json::to_vec(&configuration).expect("JSON"))
                        .expect("fixture");
                }
                let before = std::fs::read(&path).expect("snapshot");
                let configuration = if nonfinite {
                    configuration
                } else {
                    Configuration::load(&path).expect("load").expect("exists")
                };
                let saves = Arc::new(AtomicUsize::new(0));
                let mut condor = runtime(&configuration, Arc::clone(&saves));
                let mut sequence = TargetQuality::new(None, None);
                let (tx, rx) = mpsc::channel();
                let result = match entry {
                    "validate" => sequence.validate(&mut condor).map(|_| ()),
                    "initialize" => {
                        sequence.initialize(&mut condor, tx, &Default::default()).map(|_| ())
                    },
                    "execute" => sequence.execute(&mut condor, tx, Default::default()).map(|_| ()),
                    "tui" => run_target_quality_tui(&mut condor, Arc::new(AtomicBool::new(false))),
                    _ => configuration
                        .instantiate_condor(Box::new(|_| Ok(())), &Default::default())
                        .map(|_| ()),
                };
                let error = result.expect_err("invalid TQ rejected");
                assert!(
                    format!("{error:#}").contains(expected),
                    "{invalid}/{entry}: {error:#}"
                );
                assert_eq!(saves.load(Ordering::Relaxed), 0);
                assert_eq!(
                    serde_json::to_value(&condor.scenes).expect("scenes"),
                    serde_json::to_value(&configuration.condor.scenes).expect("before")
                );
                assert!(rx.try_recv().is_err());
                assert_eq!(std::fs::read(&path).expect("unchanged bytes"), before);
                assert!(!configuration.temp.exists());
                assert!(!temp.path().join("forbidden.ffindex").exists());
            }
        }
    }

    fn check_loaded_quality_repair(phase: u8) {
        assert!((1..=3).contains(&phase));
        let temp = tempfile::tempdir().expect("fixture");
        let mut configuration = fixture(temp.path());
        configuration
            .condor
            .sequence_config
            .target_quality
            .as_mut()
            .expect("TQ")
            .quantizer_range = (40, 20);
        let path = temp.path().join("condor.json");
        std::fs::write(&path, serde_json::to_vec(&configuration).expect("JSON")).expect("fixture");
        let before = std::fs::read(&path).expect("snapshot");
        assert!(
            configuration.save(&path).is_err(),
            "invalid loaded section saved"
        );
        assert_eq!(std::fs::read(&path).expect("unchanged"), before);
        assert!(
            target_quality::config(
                Some(&path),
                &ConfigAction::Set {
                    setting: TargetQualitySet::MaximumProbes { value: 8 }
                }
            )
            .is_err()
        );
        assert_eq!(std::fs::read(&path).expect("unchanged"), before);
        common::condor_cmd(&temp)
            .args(["quality-check", "set", "strategy", "whole"])
            .assert()
            .failure();
        assert_eq!(
            std::fs::read(&path).expect("unrelated CRUD cannot save invalid TQ"),
            before
        );
        if phase == 1 {
            return;
        }
        target_quality::config(
            Some(&path),
            &ConfigAction::Set {
                setting: TargetQualitySet::QuantizerRange { range: (20, 20) },
            },
        )
        .expect("repair equal bounds");
        let repaired = Configuration::load(&path).expect("load").expect("exists");
        assert_eq!(
            repaired
                .condor
                .sequence_config
                .target_quality
                .as_ref()
                .expect("TQ")
                .quantizer_range,
            (20, 20)
        );
        assert_eq!(
            serde_json::to_value(&repaired.condor.scenes).expect("scenes"),
            serde_json::to_value(&configuration.condor.scenes).expect("original")
        );
        target_quality::config(
            Some(&path),
            &ConfigAction::Reset {
                key: Some(TargetQualityKey::QuantizerRange),
            },
        )
        .expect("reset range");
        std::fs::write(&path, &before).expect("invalid fixture");
        target_quality::target_quality_handler(
            Some(&path),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(20),
            Some(30),
            None,
        )
        .expect("explicit flags repair before validation");
        if phase == 2 {
            return;
        }
        std::fs::write(&path, &before).expect("invalid fixture");
        target_quality::config(Some(&path), &ConfigAction::Reset { key: None })
            .expect("disable invalid TQ");
        assert!(
            Configuration::load(&path)
                .expect("load")
                .expect("exists")
                .condor
                .sequence_config
                .target_quality
                .is_none()
        );
        assert!(!configuration.temp.exists());
        assert!(!temp.path().join("forbidden.ffindex").exists());
    }

    #[test]
    fn invalid_loaded_target_quality_cannot_be_saved_by_unrelated_edits() {
        check_loaded_quality_repair(1);
    }

    #[test]
    fn target_quality_explicit_range_crud_or_flags_repairs_invalid_loaded_bounds_offline() {
        check_loaded_quality_repair(2);
    }

    #[test]
    fn target_quality_whole_section_reset_disables_invalid_loaded_config_offline() {
        check_loaded_quality_repair(3);
    }

    fn check_crud_validation(phase: u8) {
        assert!((1..=3).contains(&phase));
        use california_condor::commands::{
            QualityMetric,
            config::ConfigCommand,
            handlers::target_quality::{Interpolator, TargetQualityCrud},
        };
        let temp = tempfile::tempdir().expect("fixture");
        for set in [
            TargetQualitySet::MaximumProbes { value: 0 },
            TargetQualitySet::QuantizerRange { range: (71, 71) },
            TargetQualitySet::Target { value: f64::NAN },
            TargetQualitySet::Params {
                params: "--rc 1 --keyint 24".into(),
            },
        ] {
            let mut configuration = fixture(temp.path());
            let before = serde_json::to_value(&configuration).expect("snapshot");
            assert!(TargetQualityCrud::apply(&mut configuration, &set).is_err());
            assert_eq!(
                serde_json::to_value(&configuration).expect("unchanged"),
                before
            );
        }
        if phase == 1 {
            assert!(!temp.path().join("scratch").exists());
            return;
        }
        let mut configuration = fixture(temp.path());
        configuration
            .condor
            .sequence_config
            .target_quality
            .as_mut()
            .expect("TQ")
            .quantizer_range = (40, 20);
        let before = serde_json::to_value(&configuration).expect("snapshot");
        for set in [
            TargetQualitySet::Metric {
                metric: QualityMetric::XPSNR,
            },
            TargetQualitySet::Interpolators {
                first: Interpolator::Linear,
                second: Interpolator::Pchip,
            },
        ] {
            assert!(TargetQualityCrud::apply(&mut configuration, &set).is_err());
            assert_eq!(
                serde_json::to_value(&configuration).expect("unchanged"),
                before
            );
        }
        assert!(
            TargetQualityCrud::restore(&mut configuration, Some(&TargetQualityKey::MaximumProbes))
                .is_err()
        );
        assert_eq!(
            serde_json::to_value(&configuration).expect("unchanged"),
            before
        );
        if phase == 2 {
            assert!(!configuration.temp.exists());
            return;
        }
        TargetQualityCrud::restore(&mut configuration, Some(&TargetQualityKey::QuantizerRange))
            .expect("reset invalid bounds");
        let quality = configuration.condor.sequence_config.target_quality.as_mut().expect("TQ");
        quality.maximum_probes = 0;
        TargetQualityCrud::restore(&mut configuration, Some(&TargetQualityKey::MaximumProbes))
            .expect("reset invalid probes");
        configuration
            .condor
            .sequence_config
            .target_quality
            .as_mut()
            .expect("TQ")
            .metric
            .target_range_mut()
            .0 = 100.0;
        TargetQualityCrud::apply(
            &mut configuration,
            &TargetQualitySet::Metric {
                metric: QualityMetric::XPSNR,
            },
        )
        .expect("metric repairs invalid target");
        assert!(!configuration.temp.exists());
    }

    #[test]
    fn invalid_tq_crud_candidates_preserve_configuration_without_creating_scratch() {
        check_crud_validation(1);
    }

    #[test]
    fn unrelated_tq_crud_edits_refuse_invalid_section_without_mutation_or_scratch() {
        check_crud_validation(2);
    }

    #[test]
    fn tq_crud_reset_or_metric_switch_repairs_invalid_fields_without_creating_scratch() {
        check_crud_validation(3);
    }

    fn check_effective_quality_settings(phase: u8) {
        assert!((1..=3).contains(&phase));
        use andean_condor::models::{
            encoder::cli_parameter::CLIParameter, sequence::target_quality::types::ProbeStrategy,
        };
        use california_condor::commands::QualityProfile;
        let temp = tempfile::tempdir().expect("fixture");
        let mut configuration = fixture(temp.path());
        let path = temp.path().join("condor.json");
        configuration.save(&path).expect("valid initial save");
        let before = std::fs::read(&path).expect("snapshot");
        configuration
            .condor
            .sequence_config
            .target_quality
            .as_mut()
            .expect("TQ")
            .quantizer_range = (50, 70);
        configuration.condor.scenes[1].encoder =
            Encoder::default_from_base(&EncoderBase::X264, false);
        assert!(
            configuration.save(&path).is_err(),
            "scene endpoint, not just global encoder"
        );
        assert_eq!(std::fs::read(&path).expect("unchanged"), before);
        if phase == 1 {
            return;
        }
        configuration.condor.scenes[1].encoder = configuration.condor.encoder.clone();
        configuration.condor.scenes[1]
            .encoder
            .parameters_mut()
            .insert("preset".into(), CLIParameter::new_number("--", " ", 99.0));
        assert!(
            configuration.save(&path).is_err(),
            "bad scene options without override"
        );
        configuration
            .condor
            .sequence_config
            .target_quality
            .as_mut()
            .expect("TQ")
            .probing
            .encoder_options = Some(EncoderBase::SVTAV1.default_parameters());
        configuration.save(&path).expect("actual probe override replaces scene options");
        if phase == 2 {
            return;
        }
        configuration
            .condor
            .sequence_config
            .target_quality
            .as_mut()
            .expect("TQ")
            .probing
            .strategy = ProbeStrategy::Skip { skip: 0 };
        target_quality::configure_target_quality(
            &mut configuration,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(&QualityProfile::Standard),
        )
        .expect("profile repairs strategy before validation");
        configuration.save(&path).expect("repaired save");
        assert!(!configuration.temp.exists());
        assert!(!temp.path().join("forbidden.ffindex").exists());
    }

    #[test]
    fn configuration_save_validates_each_scene_encoder_quantizer_endpoints() {
        check_effective_quality_settings(1);
    }

    #[test]
    fn configuration_save_uses_explicit_probe_options_instead_of_invalid_scene_options() {
        check_effective_quality_settings(2);
    }

    #[test]
    fn target_quality_profile_repairs_invalid_strategy_before_validation_and_save() {
        check_effective_quality_settings(3);
    }

    fn check_tui_precedence(modes: &[&str]) {
        use andean_condor::utils::cancellation::Cancelled;
        let temp = tempfile::tempdir().expect("fixture");
        for &mode in modes {
            let mut configuration = fixture(temp.path());
            configuration
                .condor
                .sequence_config
                .target_quality
                .as_mut()
                .expect("TQ")
                .maximum_probes = 0;
            match mode {
                "empty" => configuration.condor.scenes.clear(),
                "disabled" => configuration.condor.sequence_config.target_quality = None,
                _ => (),
            }
            let saved = Arc::new(AtomicUsize::new(0));
            let mut condor = runtime(&configuration, Arc::clone(&saved));
            let result =
                run_target_quality_tui(&mut condor, Arc::new(AtomicBool::new(mode == "cancelled")));
            if mode == "cancelled" {
                assert!(result.expect_err("cancel first").is::<Cancelled>());
            } else {
                result.expect("no work");
            }
            assert_eq!(saved.load(Ordering::Relaxed), 0);
            assert!(!configuration.temp.exists());
            assert!(!temp.path().join("forbidden.ffindex").exists());
        }
    }

    #[test]
    fn target_quality_tui_cancellation_precedes_invalid_config_validation() {
        check_tui_precedence(&["cancelled"]);
    }

    #[test]
    fn target_quality_tui_empty_or_disabled_job_returns_without_backend_work() {
        check_tui_precedence(&["empty", "disabled"]);
    }

    #[test]
    fn empty_scene_target_quality_validation_rejects_invalid_global_config_but_accepts_valid_or_disabled_config_without_detection()
     {
        use andean_condor::{
            core::{AndeanCondor, DefaultAndeanCondor},
            models::sequence::{
                DefaultSequenceConfig, DefaultSequenceData, target_quality::types::ProbeStrategy,
            },
        };
        let temp = tempfile::tempdir().expect("fixture");
        let mut failures = vec![];
        for invalid in [
            "range", "target", "probes", "strategy", "endpoint", "options", "valid", "disabled",
        ] {
            let mut configuration = fixture(temp.path());
            configuration.condor.scenes.clear();
            configuration.condor.sequence_config.parallel_encoder.validate_frame_count = false;
            let quality = configuration.condor.sequence_config.target_quality.as_mut().expect("TQ");
            let expected = match invalid {
                "range" => {
                    quality.quantizer_range = (40, 20);
                    "quantizer"
                },
                "target" => {
                    quality.metric.target_range_mut().0 = f64::NAN;
                    "finite"
                },
                "probes" => {
                    quality.maximum_probes = 0;
                    "probes"
                },
                "strategy" => {
                    quality.probing.strategy = ProbeStrategy::Skip { skip: 0 };
                    "skip"
                },
                "endpoint" => {
                    quality.quantizer_range = (71, 71);
                    "quantizer"
                },
                "valid" => "",
                "disabled" => {
                    configuration.condor.sequence_config.target_quality = None;
                    ""
                },
                _ => {
                    quality.probing.encoder_options = Some(
                        andean_condor::models::encoder::cli_parameter::CLIParameter::new_numbers(
                            "--",
                            " ",
                            &[("rc", 1.0)],
                        ),
                    );
                    "rc"
                },
            };
            for entry in ["pure admission", "sequence validate", "Condor validate"] {
                let saved = Arc::new(AtomicUsize::new(0));
                let mut condor = runtime(&configuration, Arc::clone(&saved));
                let result = match entry {
                    "pure admission" => TargetQuality::validate_config(&condor),
                    "sequence validate" => {
                        TargetQuality::new(None, None).validate(&mut condor).map(|_| ())
                    },
                    _ => {
                        // The pipeline checks executable existence before its sequences;
                        // the current test binary satisfies that without any native probe.
                        condor
                            .encoder
                            .set_executable(Some(std::env::current_exe().expect("test binary")));
                        let config = DefaultSequenceConfig {
                            target_quality: condor.sequence_config.target_quality,
                            parallel_encoder: condor.sequence_config.parallel_encoder,
                            ..Default::default()
                        };
                        let condor: Condor<DefaultSequenceData, DefaultSequenceConfig> =
                            Condor::new(
                                condor.input,
                                condor.output,
                                condor.encoder,
                                vec![],
                                Some(config),
                                Box::new(|_| panic!("validation must not persist")),
                            );
                        let mut pipeline = DefaultAndeanCondor {
                            condor,
                            sequences: vec![Box::new(TargetQuality::new(None, None))],
                        };
                        let result = pipeline.validate().map(|_| ());
                        assert!(pipeline.condor.scenes.is_empty(), "no scene detection");
                        result
                    },
                };
                match result {
                    Ok(()) if expected.is_empty() => (),
                    Ok(()) => failures.push(format!(
                        "{invalid}/{entry}: accepted invalid global TQ before detection"
                    )),
                    Err(error) if expected.is_empty() => failures.push(format!(
                        "{invalid}/{entry}: rejected valid no-work config: {error:#}"
                    )),
                    Err(error) if !format!("{error:#}").contains(expected) => {
                        failures.push(format!("{invalid}/{entry}: {error:#}"));
                    },
                    Err(_) => (),
                }
                assert_eq!(saved.load(Ordering::Relaxed), 0);
                assert!(!configuration.temp.exists());
                assert!(!temp.path().join("forbidden.ffindex").exists());
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}
