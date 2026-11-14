use crate::common::condor_cmd;
use crate::common::config::{init_config, load};
use andean_condor::models::sequence::quality_check::QualityCheckConfig;
use california_condor::{
    commands::handlers::load_configuration, test_helpers::*, utils::hash_path::hash_path,
};

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
    fn qc_cli_effective_reference_precedence_and_resume() {
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

    #[test]
    fn qc_cli_plugin_scores_override_with_missing_root_like_fallback() {
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

    /// Downscaling the clip to 540p cuts the encode and metric cost roughly
    /// fourfold, and nothing here depends on the source resolution.
    const WIDTH: usize = 960;
    const HEIGHT: usize = 540;
    /// The subset profile scores this many frames per scene.
    const SCORED_FRAMES: u32 = 11;

    /// One encode, then both quality-check paths run against it.
    ///
    /// Encoding dominates this file's runtime, so the full run and the resume
    /// run share a single encode. The resume case rewrites the config to look
    /// partially scored and re-runs the same command.
    #[serial]
    #[test]
    fn scores_every_scene_and_resumes_partial_scenes() {
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

        // Now the resume path: mark two scenes as already scored and re-run the
        // same command. Those scenes keep their existing scores, so this also
        // covers that a partial run is not redone from scratch.
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

    /// `quality-check` probes and reduces exactly as configured, and refuses
    /// flags that do not belong to the chosen kind.
    #[serial]
    #[test]
    fn quality_check_set_strategy_and_statistic() {
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
}
