use crate::common::condor_cmd;
use crate::common::config::{init_config, load};
use andean_condor::models::sequence::quality_check::QualityCheckConfig;
use california_condor::{
    commands::handlers::load_configuration, test_helpers::*, utils::hash_path::hash_path,
};

#[cfg(test)]
mod handler_configuration {
    use super::*;
    use andean_condor::{
        models::{
            input::{Input, VapourSynthImportMethod},
            sequence::target_quality::types::{
                DEFAULT_BUTTERAUGLI_TARGET_RANGE, ProbeStatistic, ProbeStrategy, QualityMetric,
                SubsetProbeLength, SubsetProbePosition,
            },
        },
        vapoursynth::vapoursynth_filters::VapourSynthFilter,
    };
    use california_condor::commands::{
        DecoderMethod, QualityMetric as QualityMetricBase, QualityProfile,
        handlers::{init::init_handler, quality_check::quality_check_handler},
    };

    #[test]
    fn quality_check_handler_without_overrides_retains_checked_defaults_and_config_path() {
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
        expected_config.condor.sequence_config.quality_check = Some(QualityCheckConfig::default());
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
        let (config, found_config_path) =
            quality_check_handler(Some(&config_path), None, None, None, None, None, None, None)
                .expect("quality_check_handler should succeed");
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
    fn quality_check_handler_applies_butteraugli_input_filters_and_subset_probe_settings() {
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
        let custom_vs_args = vec!["method=quality check".to_string()];
        let mut expected_config = default_config(&test_video, &output, &temp_abs);
        expected_config.condor.sequence_config.quality_check = Some(QualityCheckConfig {
            input: Some(Input::VapourSynth {
                path: input_abs.clone(),
                import_method: VapourSynthImportMethod::LSMASHWorks { index: None },
                cache_path: None,
                filters: custom_filters.clone(),
            }),
            metric: QualityMetric::BUTTERAUGLI {
                target_range: DEFAULT_BUTTERAUGLI_TARGET_RANGE,
                resolution: None,
                threads: None,
                intensity_multiplier: None,
                norm: None,
                gpu_id: None,
            },
            statistic: ProbeStatistic::Mean,
            strategy: ProbeStrategy::Subset {
                position: SubsetProbePosition::Middle,
                length: SubsetProbeLength::Frames(11),
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
        let (config, found_config_path) = quality_check_handler(
            Some(&config_path),
            Some(&temp.path().join(hash_path(&input_abs))),
            Some(&test_video.path),
            Some(&DecoderMethod::LSMASHWorks),
            Some(&custom_filters),
            Some(&custom_vs_args),
            Some(&QualityMetricBase::BUTTERAUGLI),
            Some(&QualityProfile::Fast),
        )
        .expect("quality_check_handler should succeed");
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
    use andean_condor::{
        ffmpeg::FFPixelFormat,
        models::{
            encoder::cli_parameter::CLIParameter,
            input::{Input, VapourSynthImportMethod},
            sequence::target_quality::types::{
                DEFAULT_XPSNR_TARGET_RANGE, ProbeStatistic, ProbeStrategy, QualityMetric,
                QualityMetric as MetricModel, SubsetProbeLength, SubsetProbePosition,
            },
        },
        vapoursynth::{plugins::resize::Scaler, vapoursynth_filters::VapourSynthFilter},
    };
    use serial_test::serial;

    use super::*;

    #[test]
    fn quality_check_cli_prefers_own_reference_then_parallel_then_root_and_reuses_saved_measurement()
     {
        use andean_condor::models::input::VapourSynthScriptSource;
        let temp = tempfile::tempdir().expect("fixture");
        let output = temp.path().join("out.mkv");
        let generated = std::process::Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=size=128x128:rate=24",
                "-frames:v",
                "2",
                "-c:v",
                "ffv1",
            ])
            .arg(&output)
            .output()
            .expect("ffmpeg");
        assert!(generated.status.success(), "{generated:?}");
        let mut inputs = vec![];
        for (name, color) in [("root", 16), ("pe", 64), ("qc", 128)] {
            let script = temp.path().join(format!("{name}.vpy"));
            let marker = serde_json::to_string(&temp.path().join(format!("{name}.frames")))
                .expect("literal");
            std::fs::write(&script,format!("import vapoursynth as vs\nfrom pathlib import Path\nc=vs.core\nclip=c.std.BlankClip(width=128,height=128,length=2,format=vs.YUV420P8,fpsnum=24,color=[{color},128,128])\ndef record(n,f):\n if n==1:\n  with Path({marker}).open('a') as out: out.write('frame\\n')\n return f\nclip.std.ModifyFrame(clip,record).set_output()\n")).expect("script");
            inputs.push(Input::VapourSynthScript {
                source: VapourSynthScriptSource::Path(script),
                variables: Default::default(),
                index: 0,
                filters: vec![],
                stream_concurrently: false,
            });
        }
        let video = TestVideo {
            path: temp.path().join("root.vpy"),
            width: 128,
            height: 128,
            frames: 2,
            fps_rational: (24, 1),
            format: FFPixelFormat::YUV420P,
            scenes: vec![(0, 2)],
        };
        for chosen in [1, 2, 0] {
            let mut config = default_config(&video, &output, &temp.path().join("work"));
            config.condor.input = inputs[0].clone();
            config.condor.scenes = video.mock_scenes(&config.condor.encoder);
            config.condor.sequence_config.parallel_encoder.input =
                (chosen != 0).then(|| inputs[1].clone());
            config.condor.sequence_config.quality_check = Some(QualityCheckConfig {
                input: (chosen == 2).then(|| inputs[2].clone()),
                metric: QualityMetric::VMAF {
                    target_range: (0.0, 100.0),
                    resolution: None,
                    scaler: "bicubic".into(),
                    threads: 0,
                    model: None,
                    features: vec![],
                },
                ..Default::default()
            });
            let file = temp.path().join("condor.json");
            config.save(&file).expect("settings");
            condor_cmd(&temp)
                .env("CONDOR_TEST_MODE", "1")
                .args(["quality-check"])
                .assert()
                .success();
            let saved = load(&file);
            let pass = &saved.condor.scenes[0].sequence_data.quality_check.quality;
            assert_eq!(pass.scores.len(), 2, "actual owning QC measurement");
            assert_eq!(
                pass.provenance.as_ref().expect("stamp").context["input"],
                andean_condor::models::sequence::provenance::input_identity(&inputs[chosen])
                    .expect("literal chosen declared input")
            );
            saved.admit_results().expect(
                "H2: producer and CLI admission must resolve the same effective QC reference",
            );
            let markers = ["root", "pe", "qc"].map(|name| {
                std::fs::read(temp.path().join(format!("{name}.frames"))).unwrap_or_default()
            });
            assert!(
                !markers[chosen].is_empty(),
                "measurement decoded the selected reference's last frame"
            );
            let history = serde_json::to_vec(pass).expect("saved measurement");
            condor_cmd(&temp)
                .env("CONDOR_TEST_MODE", "1")
                .args(["quality-check"])
                .assert()
                .success();
            let resumed = load(&file);
            assert_eq!(
                serde_json::to_vec(&resumed.condor.scenes[0].sequence_data.quality_check.quality)
                    .expect("unchanged measurement"),
                history
            );
            for (name, before) in ["root", "pe", "qc"].iter().zip(markers) {
                assert_eq!(
                    std::fs::read(temp.path().join(format!("{name}.frames"))).unwrap_or_default(),
                    before,
                    "matching CLI resume must not measure another frame"
                );
            }
        }
    }

    fn check_plugin_output_history(phase: u8) {
        assert!((1..=3).contains(&phase));
        let temp = tempfile::tempdir().expect("fixture");
        let source = temp.path().join("source.mkv");
        let actual = temp.path().join("actual.mkv");
        for (path, distorted) in [(&source, false), (&actual, true)] {
            let mut command = std::process::Command::new("ffmpeg");
            command.args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=64x64:rate=24",
                "-frames:v",
                "2",
            ]);
            if distorted {
                command.args(["-vf", "noise=alls=2:allf=u"]);
            }
            let output =
                command.args(["-c:v", "ffv1"]).arg(path).output().expect("required FFmpeg CLI");
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let source_bytes = std::fs::read(&source).expect("source snapshot");
        let output_bytes = std::fs::read(&actual).expect("output snapshot");
        let video = TestVideo {
            path: source.clone(),
            width: 64,
            height: 64,
            frames: 2,
            fps_rational: (24, 1),
            format: FFPixelFormat::YUV420P,
            scenes: vec![(0, 2)],
        };
        let file = temp.path().join("condor.json");
        let mut baseline = None;
        for overridden in [false, true] {
            let root = if overridden {
                temp.path().join("missing-root.mkv")
            } else {
                actual.clone()
            };
            let mut config = default_config(&video, &root, &temp.path().join("work"));
            config.condor.input = Input::Video {
                path: source.clone(),
                import_method: andean_condor::models::input::ImportMethod::FFMS2 {
                    index: None,
                    cache_path: None,
                },
                filters: vec![],
            };
            config.condor.scenes = video.mock_scenes(&config.condor.encoder);
            config.condor.sequence_config.quality_check = Some(QualityCheckConfig {
                metric: MetricModel::XPSNR {
                    target_range: (44.0, 46.0),
                    resolution: None,
                },
                strategy: ProbeStrategy::Whole,
                ..Default::default()
            });
            config.condor.sequence_config.scene_concatenator.output =
                overridden.then(|| actual.clone());
            config.save(&file).expect("save measured job");
            let output = condor_cmd(&temp)
                .env("CONDOR_TEST_MODE", "1")
                .args(["quality-check"])
                .output()
                .expect("owning CLI QC");
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let saved = load(&file);
            let scores = saved.condor.scenes[0].sequence_data.quality_check.quality.scores.clone();
            assert_eq!(
                scores.len(),
                2,
                "actual plugin scored both frames, overridden={overridden}"
            );
            assert!(scores.iter().all(|score| score.is_finite() && *score > 0.0));
            saved.admit_results().expect("current positional plugin history resumes");
            if phase >= 2 {
                let history =
                    serde_json::to_vec(&saved.condor.scenes[0].sequence_data.quality_check.quality)
                        .expect("current history");
                condor_cmd(&temp)
                    .env("CONDOR_TEST_MODE", "1")
                    .args(["quality-check"])
                    .assert()
                    .success();
                assert_eq!(
                    serde_json::to_vec(
                        &load(&file).condor.scenes[0].sequence_data.quality_check.quality
                    )
                    .expect("resumed history"),
                    history,
                    "resume preserves exact positional history"
                );
            }
            if phase == 3 {
                let mut old = load(&file);
                old.condor.scenes[0]
                    .sequence_data
                    .quality_check
                    .quality
                    .provenance
                    .as_mut()
                    .expect("plugin provenance")
                    .context
                    .as_object_mut()
                    .expect("context")
                    .remove("score_order");
                old.admit_results()
                    .expect_err("unindexed historical plugin callbacks are not certified");
            }
            if let Some(baseline) = &baseline {
                assert_eq!(
                    &scores, baseline,
                    "same distorted clip must be measured through either destination route"
                );
            } else {
                baseline = Some(scores);
            }
            assert_eq!(saved.condor.output.path, root);
            assert_eq!(
                saved.condor.sequence_config.scene_concatenator.output,
                config.condor.sequence_config.scene_concatenator.output
            );
            assert_eq!(
                std::fs::read(&source).expect("reference preserved"),
                source_bytes
            );
            assert_eq!(
                std::fs::read(&actual).expect("output preserved"),
                output_bytes
            );
        }
    }

    #[test]
    fn quality_check_xpsnr_scores_concat_output_override_when_root_output_is_missing() {
        check_plugin_output_history(1);
    }

    #[test]
    fn quality_check_xpsnr_matching_saved_history_is_reused_exactly() {
        check_plugin_output_history(2);
    }

    #[test]
    fn quality_check_unindexed_historical_plugin_scores_are_not_admitted() {
        check_plugin_output_history(3);
    }

    /// Downscaling the clip to 540p cuts the encode and metric cost roughly
    /// fourfold, and nothing here depends on the source resolution.
    const WIDTH: usize = 960;
    const HEIGHT: usize = 540;
    /// The subset profile scores this many frames per scene.
    const SCORED_FRAMES: u32 = 11;

    /// Share one encode between the initial and populated-history score-count checks.
    #[serial]
    #[test]
    fn quality_check_fast_xpsnr_initial_and_populated_history_rerun_report_eleven_scores_per_scene()
    {
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
                scaler: None,
                width: Some(WIDTH),
                height: Some(HEIGHT),
                format: Some(FFPixelFormat::YUV420P10LE),
            }];
        }
        config.condor.encoder.parameters_mut().insert(
            "preset".to_owned(),
            CLIParameter::new_number("--", " ", 10.0),
        );
        config.condor.sequence_config.parallel_encoder.workers = Some(2);
        config.condor.scenes = test_video.mock_scenes(&config.condor.encoder);
        config.save(&config_path).expect("configuration save should succeed");

        // Encode and concatenate first so the output file exists
        condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .args(["encode"])
            .assert()
            .success();

        condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .args(["concatenate", "--method", "mkvmerge"])
            .assert()
            .success();

        condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .args([
                "quality-check",
                "--input",
                path_str(&test_video.path),
                "--decoder",
                "vs-ffms2",
                "--filters",
                &format!("resize:scaler=bicubic;width={WIDTH};height={HEIGHT};format=yuv420p10le;"),
                "--metric",
                "xpsnr",
                "--profile",
                "fast",
            ])
            .assert()
            .success();

        let mut expected_config = config;
        expected_config.condor.sequence_config.quality_check = Some(QualityCheckConfig {
            metric: QualityMetric::XPSNR {
                target_range: DEFAULT_XPSNR_TARGET_RANGE,
                resolution: None,
            },
            strategy: ProbeStrategy::Subset {
                position: SubsetProbePosition::Middle,
                length: SubsetProbeLength::Frames(SCORED_FRAMES),
            },
            statistic: ProbeStatistic::Mean,
            // The metric input records the filters it was given, scaler included.
            input: Some(Input::VapourSynth {
                path: input_abs,
                import_method: VapourSynthImportMethod::FFMS2 { index: None },
                cache_path: None,
                filters: vec![VapourSynthFilter::Resize {
                    scaler: Some(Scaler::Bicubic),
                    width: Some(WIDTH),
                    height: Some(HEIGHT),
                    format: Some(FFPixelFormat::YUV420P10LE),
                }],
            }),
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
                scene.sequence_data.quality_check.quality.scores.len(),
                SCORED_FRAMES as usize,
                "scene {} should have {SCORED_FRAMES} quality check scores",
                index,
            );
        });

        // This checks lengths, not sentinel preservation or unfinished-scene resumption.
        let mut config = load_configuration(Some(&config_path))
            .expect("load_configuration should succeed")
            .0;
        let already_scored = vec![45.0; SCORED_FRAMES as usize];
        config.condor.scenes[0].sequence_data.quality_check.quality.scores = already_scored.clone();
        config.condor.scenes[1].sequence_data.quality_check.quality.scores = already_scored;
        config.save(&config_path).expect("configuration save should succeed");

        condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .args([
                "quality-check",
                "--input",
                path_str(&test_video.path),
                "--decoder",
                "vs-ffms2",
                "--filters",
                &format!("resize:scaler=bicubic;width={WIDTH};height={HEIGHT};format=yuv420p10le;"),
                "--metric",
                "xpsnr",
                "--profile",
                "fast",
            ])
            .assert()
            .success();

        let (config, _) =
            load_configuration(Some(&config_path)).expect("load_configuration should succeed");

        config.condor.scenes.iter().enumerate().for_each(|(index, scene)| {
            assert_eq!(
                scene.sequence_data.quality_check.quality.scores.len(),
                SCORED_FRAMES as usize,
                "scene {} should have {SCORED_FRAMES} quality check scores after resuming",
                index,
            );
        });
    }

    fn check_quality_check_crud(phase: u8) {
        assert!((1..=4).contains(&phase));
        if !ffmpeg_is_available() {
            return;
        }
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let config_path = init_config(&temp);

        condor_cmd(&temp)
            .args([
                "quality-check",
                "set",
                "strategy",
                "subset",
                "--position",
                "end",
                "--length-percent",
                "25",
            ])
            .assert()
            .success();
        let check = load(&config_path)
            .condor
            .sequence_config
            .quality_check
            .expect("quality check was materialized");
        assert_eq!(
            check.strategy,
            ProbeStrategy::Subset {
                position: SubsetProbePosition::End,
                length: SubsetProbeLength::Percentage(25.0),
            },
            "strategy is set"
        );

        // Flags that do not belong to the kind are refused.
        condor_cmd(&temp)
            .args(["quality-check", "set", "strategy", "whole", "--skip", "4"])
            .assert()
            .failure();
        condor_cmd(&temp)
            .args(["quality-check", "set", "strategy", "skip"])
            .assert()
            .failure();
        if phase == 1 {
            return;
        }
        condor_cmd(&temp)
            .args(["quality-check", "set", "statistic", "percentile"])
            .assert()
            .failure();
        condor_cmd(&temp)
            .args(["quality-check", "set", "statistic", "percentile", "--percentile", "150"])
            .assert()
            .failure();

        condor_cmd(&temp)
            .args(["quality-check", "set", "statistic", "percentile", "--percentile", "10"])
            .assert()
            .success();
        let check = load(&config_path)
            .condor
            .sequence_config
            .quality_check
            .expect("quality check exists");
        assert_eq!(
            check.statistic,
            ProbeStatistic::Percentile(10.0),
            "statistic is set"
        );
        if phase == 2 {
            return;
        }

        condor_cmd(&temp)
            .args(["quality-check", "set", "metric", "xpsnr"])
            .assert()
            .success();
        let check = load(&config_path)
            .condor
            .sequence_config
            .quality_check
            .expect("quality check exists");
        assert!(
            matches!(check.metric, MetricModel::XPSNR { .. }),
            "metric is {:?}",
            check.metric
        );
        if phase == 3 {
            return;
        }

        condor_cmd(&temp)
            .args(["quality-check", "reset", "strategy"])
            .assert()
            .success();
        let check = load(&config_path)
            .condor
            .sequence_config
            .quality_check
            .expect("quality check exists");
        assert_eq!(
            check.strategy,
            ProbeStrategy::default(),
            "reset restores the default strategy"
        );
    }

    #[serial]
    #[test]
    fn quality_check_strategy_crud_rejects_inapplicable_or_missing_arguments() {
        check_quality_check_crud(1);
    }

    #[serial]
    #[test]
    fn quality_check_percentile_crud_requires_value_within_range_and_saves_it() {
        check_quality_check_crud(2);
    }

    #[serial]
    #[test]
    fn quality_check_metric_crud_switches_to_xpsnr() {
        check_quality_check_crud(3);
    }

    #[serial]
    #[test]
    fn quality_check_subset_strategy_crud_saves_end_percentage_and_reset_restores_default() {
        check_quality_check_crud(4);
    }
}
