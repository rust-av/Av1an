mod benchmarker;
mod concatenate;
mod config;
mod detect_noise;
mod detect_scenes;
mod encode;
mod init;
mod optimize_bitrate;
mod quality_check;
mod scale_noise;
mod scale_speed;
mod scenes;
mod start;
mod target_quality;
mod zone;

#[cfg(test)]
mod input_configuration {
    use andean_condor::{
        models::input::{ImportMethod, Input as InputModel, VapourSynthScriptSource},
        vapoursynth::vapoursynth_filters::VapourSynthFilter,
    };
    use california_condor::{
        commands::{
            CondorCliError, DecoderMethod,
            handlers::{apply_input_filters, configure_input, encode, load_configuration},
        },
        configuration::Configuration,
    };
    use std::{assert_matches, collections::HashMap, path::PathBuf};

    fn script_input(stream_concurrently: bool) -> InputModel {
        InputModel::VapourSynthScript {
            source: VapourSynthScriptSource::Path("script.vpy".into()),
            variables: HashMap::new(),
            index: 0,
            filters: Vec::new(),
            stream_concurrently,
        }
    }

    fn built_stream_concurrently(input: &InputModel) -> bool {
        let InputModel::VapourSynthScript {
            stream_concurrently,
            ..
        } = input
        else {
            panic!("expected a VapourSynthScript input");
        };
        *stream_concurrently
    }

    #[test]
    fn configure_and_apply_filters_are_atomic_without_source_indexing() {
        use andean_condor::{ffmpeg::FFPixelFormat, models::input::Ffms2Filter};
        let temp = tempfile::tempdir().expect("CLI fixture");
        let script = temp.path().join("blank.vpy");
        std::fs::write(&script, "from vapoursynth import core, YUV420P8\ncore.std.BlankClip(width=32, height=16, format=YUV420P8).set_output()\n").expect("blank script");
        let mut configuration = Configuration::new(
            &script,
            &temp.path().join("out.mkv"),
            Some(temp.path()),
            None,
            None,
        )
        .expect("configuration");
        let media = temp.path().join("not-a-decodable-source.mkv");
        std::fs::write(
            &media,
            b"configuration edits must not decode/index this file",
        )
        .expect("existing path");
        configuration.condor.input = InputModel::Video {
            path: media,
            import_method: ImportMethod::FFMS2 {
                index: Some(0),
                cache_path: Some(temp.path().join("must-not-exist.ffindex")),
            },
            filters: vec![Ffms2Filter::OutputFormat {
                bit_depth: Some(10),
                chroma: None,
                width: None,
                height: None,
            }],
        };
        let disk = temp.path().join("config.json");
        let before = serde_json::to_vec(&configuration).expect("configuration JSON");
        std::fs::write(&disk, &before).expect("saved configuration");
        let converted = configure_input(
            &configuration,
            &configuration.condor.input,
            None,
            Some(&DecoderMethod::VSFFMS2),
            None,
            None,
            None,
        )
        .expect("model-only copy, no decoder/index");
        for format in [FFPixelFormat::NV12, FFPixelFormat::GBRP, FFPixelFormat::YUV440P] {
            let invalid = VapourSynthFilter::Resize {
                scaler: None,
                width: Some(16),
                height: None,
                format: Some(format),
            };
            let mut override_config = configuration.clone();
            override_config.condor.input = converted.clone();
            let override_before = serde_json::to_vec(&override_config).expect("VS config snapshot");
            let error = encode::configure_parallel_encoder(
                &mut override_config,
                None,
                Some(&DecoderMethod::FFMS2),
                Some(std::slice::from_ref(&invalid)),
                None,
                Some(2),
                None,
            )
            .expect_err("native override cannot represent explicit format");
            assert!(error.to_string().contains(format.to_pix_fmt_string()));
            assert_eq!(
                serde_json::to_vec(&override_config).expect("unchanged override config"),
                override_before
            );
            let mut vs = converted.clone();
            vs.set_filters(vec![invalid.clone()]).expect("VS retains resize intent");
            assert!(
                configure_input(
                    &configuration,
                    &vs,
                    None,
                    Some(&DecoderMethod::FFMS2),
                    None,
                    None,
                    None
                )
                .is_err(),
                "{format:?}"
            );
            let error = apply_input_filters(&mut configuration.condor.input, &[invalid])
                .expect_err("unsupported explicit format");
            assert!(error.to_string().contains(format.to_pix_fmt_string()));
            assert_eq!(
                serde_json::to_vec(&configuration).expect("unchanged model"),
                before
            );
            assert_eq!(std::fs::read(&disk).expect("saved config"), before);
        }
        assert!(!temp.path().join("must-not-exist.ffindex").exists());
        assert_eq!(
            std::fs::read_dir(temp.path()).expect("fixture listing").count(),
            3,
            "only script, media and config; no index or temp output"
        );
    }

    #[test]
    fn rebuilding_a_script_input_keeps_concurrent_streaming_off() {
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let script = temp.path().join("script.vpy");
        // BlankClip supplies the real output node required by Configuration::new.
        std::fs::write(
            &script,
            "import vapoursynth as vs\ncore = vs.core\nclip = core.std.BlankClip(width=16, \
             height=16, format=vs.YUV420P8)\nclip.set_output(0)\n",
        )
        .expect("script file writes to disk");
        let configuration =
            Configuration::new(&script, &PathBuf::from("out.mkv"), None, None, None)
                .expect("a configuration can be built from a script");
        let rebuilt = configure_input(
            &configuration,
            &script_input(false),
            None,
            None,
            None,
            None,
            None,
        )
        .expect("the input can be rebuilt");
        assert!(
            !built_stream_concurrently(&rebuilt),
            "stream_concurrently: false must survive the rebuild"
        );
    }

    #[test]
    fn load_configuration_invalid_json_returns_typed_parse_error_with_line_detail() {
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let config_path = temp.path().join("condor.json");
        std::fs::write(&config_path, "{ invalid JSON").expect("config file writes to disk");
        let error = load_configuration(Some(&config_path))
            .expect_err("invalid JSON should fail to load")
            .downcast::<CondorCliError>()
            .expect("load_configuration error should be CondorCliError");
        let error_string = format!("{error:#}");
        assert_matches!(
            error,
            CondorCliError::ConfigParseError(_),
            "invalid JSON should be ConfigParseError"
        );
        assert!(
            error_string.contains("line"),
            "error should include the JSON parse detail, got: {error_string}"
        );
    }
}

#[cfg(test)]
mod encoder_admission {
    use crate::common::condor_cmd;
    use andean_condor::models::{
        encoder::{Encoder, EncoderBase},
        input::{ImportMethod, Input},
    };
    use california_condor::{configuration::Configuration, test_helpers::TestVideo};

    #[test]
    fn legacy_jobs_can_be_repaired_in_either_order_without_running() {
        use andean_condor::models::encoder::EncoderPasses;
        let temp = tempfile::tempdir().expect("legacy fixture");
        let script = temp.path().join("blank.vpy");
        std::fs::write(&script, "from vapoursynth import core, YUV420P8\ncore.std.BlankClip(width=64,height=64,format=YUV420P8).set_output()\n").expect("script");
        let mut legacy = Configuration::new(
            &script,
            &temp.path().join("out.mkv"),
            Some(temp.path()),
            None,
            None,
        )
        .expect("seed config");
        let source = temp.path().join("must-not-open.mkv");
        std::fs::write(&source, b"not a video").expect("source");
        legacy.condor.input = Input::Video {
            path: source,
            import_method: ImportMethod::FFMS2 {
                index: Some(0),
                cache_path: Some(temp.path().join("forbidden.ffindex")),
            },
            filters: vec![],
        };
        legacy.condor.encoder = Encoder::default_from_base(&EncoderBase::SVTAV1, false);
        *legacy.condor.encoder.passes_mut().expect("passes") = EncoderPasses::All(2);
        let video = TestVideo {
            path: script,
            width: 64,
            height: 64,
            frames: 2,
            fps_rational: (24, 1),
            format: andean_condor::ffmpeg::FFPixelFormat::YUV420P,
            scenes: vec![(0, 1), (1, 2)],
        };
        legacy.condor.scenes = video.mock_scenes(&legacy.condor.encoder);
        let path = temp.path().join("condor.json");
        for global_first in [true, false] {
            legacy.save(&path).expect("legacy JSON");
            condor_cmd(&temp)
                .args(["init", "set", "output", "changed.mkv"])
                .assert()
                .success();
            condor_cmd(&temp).args(["encode", "set", "workers", "2"]).assert().success();
            condor_cmd(&temp)
                .args(["scenes", "all", "reset", "parallel-encoder"])
                .assert()
                .success();
            let global = ["encode", "set", "passes", "1"];
            let scenes = ["scenes", "all", "set", "passes", "1"];
            let (first, second) = if global_first {
                (&global[..], &scenes[..])
            } else {
                (&scenes[..], &global[..])
            };
            condor_cmd(&temp).args(first).assert().success();
            let before = std::fs::read(&path).expect("partially repaired");
            for run in [vec![], vec!["encode"]] {
                let output =
                    condor_cmd(&temp).args(run).assert().failure().get_output().stderr.clone();
                assert!(
                    String::from_utf8_lossy(&output).contains("multi-pass requires VBR"),
                    "{}",
                    String::from_utf8_lossy(&output)
                );
                assert_eq!(
                    std::fs::read(&path).expect("failed execution snapshot"),
                    before
                );
            }
            condor_cmd(&temp).args(second).assert().success();
            let repaired = Configuration::load(&path).expect("load").expect("config");
            assert_eq!(repaired.condor.encoder.validate_settings(), Ok(()));
            assert!(
                repaired
                    .condor
                    .scenes
                    .iter()
                    .all(|scene| scene.encoder.validate_settings() == Ok(()))
            );
            let before = std::fs::read(&path).expect("repaired snapshot");
            for invalid in [
                vec!["encode", "set", "passes", "2"],
                vec!["scenes", "all", "set", "passes", "2"],
            ] {
                let output =
                    condor_cmd(&temp).args(invalid).assert().failure().get_output().stderr.clone();
                assert!(String::from_utf8_lossy(&output).contains("multi-pass requires VBR"));
                assert_eq!(
                    std::fs::read(&path).expect("rejected edit snapshot"),
                    before
                );
            }
        }
        legacy.save(&path).expect("legacy reset fixture");
        let before = std::fs::read(&path).expect("snapshot");
        for targeted in [
            vec!["scenes", "all", "reset"],
            vec!["scenes", "add", "2", "3"],
            vec!["encode", "set", "append-color-data", "off"],
            vec!["encode", "set", "executable", "/missing/settings-encoder"],
            vec!["scenes", "all", "set", "append-color-data", "off"],
            vec!["scenes", "all", "set", "executable", "/missing/settings-encoder"],
        ] {
            let output =
                condor_cmd(&temp).args(&targeted).assert().failure().get_output().stderr.clone();
            assert!(
                String::from_utf8_lossy(&output).contains("multi-pass requires VBR"),
                "invalid target must not be changed/copied: {targeted:?}: {}",
                String::from_utf8_lossy(&output)
            );
            assert_eq!(
                std::fs::read(&path).expect("unchanged targeted edit"),
                before
            );
        }
        condor_cmd(&temp).args(["encode", "reset", "passes"]).assert().success();
        condor_cmd(&temp).args(["scenes", "all", "reset"]).assert().success();
        legacy.save(&path).expect("legacy clear fixture");
        condor_cmd(&temp).args(["scenes", "reset"]).assert().success();
        condor_cmd(&temp).args(["init", "reset", "encoder"]).assert().success();
        assert!(!temp.path().join("forbidden.ffindex").exists());
    }

    #[test]
    fn routes_reject_before_save_or_index() {
        let temp = tempfile::tempdir().expect("fixture");
        let script = temp.path().join("blank.vpy");
        std::fs::write(&script, "from vapoursynth import core, YUV420P8\ncore.std.BlankClip(width=64,height=64,format=YUV420P8).set_output()\n").expect("script");
        let mut config = Configuration::new(
            &script,
            &temp.path().join("out.mkv"),
            Some(temp.path()),
            None,
            None,
        )
        .expect("config");
        let source = temp.path().join("must-not-open.mkv");
        std::fs::write(&source, b"not a video").expect("source");
        config.condor.input = Input::Video {
            path: source,
            import_method: ImportMethod::FFMS2 {
                index: Some(0),
                cache_path: Some(temp.path().join("forbidden.ffindex")),
            },
            filters: vec![],
        };
        config.condor.encoder = Encoder::default_from_base(&EncoderBase::AOM, false);
        let video = TestVideo {
            path: script,
            width: 64,
            height: 64,
            frames: 1,
            fps_rational: (24, 1),
            format: andean_condor::ffmpeg::FFPixelFormat::YUV420P,
            scenes: vec![(0, 1)],
        };
        config.condor.scenes = video.mock_scenes(&config.condor.encoder);
        let path = temp.path().join("condor.json");
        config.save(&path).expect("save");
        let before = std::fs::read(&path).expect("snapshot");
        for args in [
            vec!["--encoder", "x264", "--photon-noise", "100"],
            vec!["encode", "--encoder", "x264", "--photon-noise", "100"],
            vec!["init", "set", "encoder", "--encoder", "x264", "--photon-noise", "100"],
            vec!["encode", "set", "passes", "3"],
            vec!["encode", "--encoder", "svt-av1", "--passes", "2"],
            vec!["encode", "--encoder", "ffmpeg", "--ffmpeg-filter", "hflip"],
            vec!["encode", "--chroma-noise", "100"],
            vec!["encode", "set", "params", "--cq-level NaN"],
            vec!["scenes", "0", "set", "encoder", "x264", "--iso", "100"],
            vec!["scenes", "0", "set", "passes", "3"],
        ] {
            let output = condor_cmd(&temp)
                .args(&args)
                .timeout(std::time::Duration::from_secs(10))
                .assert()
                .failure()
                .get_output()
                .stderr
                .clone();
            let stderr = String::from_utf8_lossy(&output);
            assert!(
                stderr.contains("Photon Noise")
                    || stderr.contains("pass")
                    || stderr.contains("ffmpeg-filter")
                    || stderr.contains("finite"),
                "{args:?}: {stderr}"
            );
            assert!(
                !stderr.contains("unexpected argument"),
                "not a validation failure: {stderr}"
            );
            assert_eq!(
                std::fs::read(&path).expect("unchanged config"),
                before,
                "{args:?}"
            );
            assert!(!temp.path().join("forbidden.ffindex").exists());
        }
        let new = temp.path().join("new.json");
        for params in [
            "--passes 1 --bitrate 1000",
            "-p 1 --bitrate 1000",
            "--pass 1 --bitrate 1000",
            "--pass 2 --bitrate 1000",
            "--bitrate 0Mbps",
        ] {
            let output = condor_cmd(&temp)
                .args(["encode", "--encoder", "vvenc", "--passes", "2", "--params", params])
                .timeout(std::time::Duration::from_secs(10))
                .assert()
                .failure()
                .get_output()
                .stderr
                .clone();
            let stderr = String::from_utf8_lossy(&output);
            assert!(
                stderr.contains("unsupported vvenc mode")
                    && (stderr.contains("frontend") || stderr.contains("bitrate")),
                "{params}: {stderr}"
            );
            assert_eq!(
                std::fs::read(&path).expect("VVenC rejected candidate"),
                before,
                "{params}"
            );
        }
        for args in [
            vec![
                "init",
                "must-not-open.mkv",
                "out.mkv",
                "--encoder",
                "x264",
                "--photon-noise",
                "100",
            ],
            vec![
                "--input",
                "must-not-open.mkv",
                "--output",
                "out.mkv",
                "--encoder",
                "x264",
                "--photon-noise",
                "100",
            ],
        ] {
            let output = condor_cmd(&temp)
                .args(["--config-file", new.to_str().expect("path")])
                .args(args)
                .assert()
                .failure()
                .get_output()
                .stderr
                .clone();
            assert!(
                String::from_utf8_lossy(&output).contains("does not support Photon Noise"),
                "{output:?}"
            );
            assert!(!new.exists(), "invalid new config must not save");
        }
        condor_cmd(&temp)
            .args(["encode", "set", "encoder", "svt-av1"])
            .assert()
            .success();
        condor_cmd(&temp)
            .args(["encode", "set", "params", "--rc 1 --tbr 1000 --keyint 48"])
            .assert()
            .success();
        condor_cmd(&temp).args(["encode", "set", "passes", "2"]).assert().success();
        let before = std::fs::read(&path).expect("VBR snapshot");
        condor_cmd(&temp).args(["encode", "reset", "params"]).assert().failure();
        assert_eq!(std::fs::read(&path).expect("unchanged reset"), before);

        *config.condor.scenes[0].encoder.passes_mut().expect("passes") =
            andean_condor::models::encoder::EncoderPasses::All(3);
        config.save(&path).expect("loaded invalid scene fixture");
        let before = std::fs::read(&path).expect("invalid scene snapshot");
        for args in [vec![], vec!["encode"]] {
            let output =
                condor_cmd(&temp).args(args).assert().failure().get_output().stderr.clone();
            assert!(
                String::from_utf8_lossy(&output).contains("does not support pass"),
                "{}",
                String::from_utf8_lossy(&output)
            );
            assert_eq!(
                std::fs::read(&path).expect("unchanged loaded config"),
                before
            );
        }
    }
}

#[cfg(test)]
mod filter_rejection {
    use crate::common::condor_cmd;
    use andean_condor::models::input::{ImportMethod, Input};
    use california_condor::configuration::Configuration;

    fn rejects_without_saving(filter: &str, field: &str) {
        let temp = tempfile::tempdir().expect("filter CLI fixture");
        let script = temp.path().join("blank.vpy");
        std::fs::write(&script, "from vapoursynth import core, YUV420P8\ncore.std.BlankClip(width=32, height=16, format=YUV420P8).set_output()\n").expect("blank script");
        let mut config = Configuration::new(
            &script,
            &temp.path().join("out.mkv"),
            Some(temp.path()),
            None,
            None,
        )
        .expect("seed config");
        let source = temp.path().join("must-not-be-decoded.mkv");
        std::fs::write(&source, b"not video; parser must fail before decoding").expect("source");
        config.condor.input = Input::Video {
            path: source,
            import_method: ImportMethod::FFMS2 {
                index: Some(0),
                cache_path: Some(temp.path().join("must-not-exist.ffindex")),
            },
            filters: Vec::new(),
        };
        let path = temp.path().join("condor.json");
        config.save(&path).expect("seed config");
        let before = std::fs::read(&path).expect("snapshot");
        // Exercise typed Clap flags and the CRUD string-list parser, including a valid first item.
        let outputs = [
            vec!["encode", "--filters", "crop:top=0", "--filters", filter],
            vec!["init", "set", "input", "--filters", "crop:top=0", "--filters", filter],
            vec!["detect-noise", "set", "reference-filters", "crop:top=0", filter],
        ]
        .map(|args| {
            let output = condor_cmd(&temp).args(args).output().expect("run real condor");
            let saved = std::fs::read(&path).expect("config after command");
            (output, saved)
        });
        for (output, saved) in outputs {
            let error = String::from_utf8_lossy(&output.stderr);
            assert!(!output.status.success(), "{filter}: {output:?}");
            assert!(
                error.contains(filter.split(':').next().expect("name")) && error.contains(field),
                "{filter}: {error}"
            );
            if let Some(value) = filter.strip_prefix("zoom_degrain:plane=") {
                assert!(
                    error.contains(value) && error.contains("0..=4"),
                    "{filter}: {error}"
                );
            }
            for forbidden in ["panicked", "backtrace", "index out of bounds", "Failed to open"] {
                assert!(!error.contains(forbidden), "{filter}: {error}");
            }
            assert!(
                saved == before,
                "{filter}: failed parse changed the saved config"
            );
        }
        assert!(!temp.path().join("must-not-exist.ffindex").exists());
        assert!(!temp.path().join("condor.temp").exists());
        let mut entries = std::fs::read_dir(temp.path())
            .expect("directory")
            .map(|entry| entry.expect("entry").file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        entries.sort();
        assert_eq!(
            entries,
            [
                "blank.vpy",
                "condor.json",
                "condor.json.lock",
                "logs",
                "must-not-be-decoded.mkv"
            ]
        );
    }

    macro_rules! cli_error {
        ($name:ident, $filter:expr, $field:expr) => {
            #[test]
            fn $name() {
                rejects_without_saving($filter, $field);
            }
        };
    }

    cli_error!(cli_crop, "crop:top=abc", "top");
    cli_error!(cli_resize, "resize:width=0", "width");
    cli_error!(cli_trim, "trim:start=10;end=5", "end");
    cli_error!(
        cli_rescale,
        "rescale:width=1280;height=720;doubler=ArtCNN",
        "kernel"
    );
    cli_error!(cli_wnnm, "wnnm:sigma=1,abc", "sigma");
    cli_error!(cli_bilateral, "bilateral:planes=true,maybe", "planes");
    cli_error!(cli_degrain, "degrain:recalculate=maybe", "recalculate");
    cli_error!(cli_zoom_degrain, "zoom_degrain:hpad=abc", "hpad");
    cli_error!(cli_zoom_plane_negative, "zoom_degrain:plane=-1", "plane");
    cli_error!(cli_zoom_plane_above_range, "zoom_degrain:plane=5", "plane");
    cli_error!(cli_fgs, "fgs:", "iso");
    cli_error!(cli_output_format, "output-format:typo=1", "typo");
    cli_error!(cli_missing_delimiter, "crop", ":");
}

#[cfg(test)]
mod quality_admission {
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
            sequence::{Sequence, quality_check::QualityCheck, target_quality::TargetQuality},
        },
        models::{
            input::{ImportMethod, Input as InputModel, VapourSynthScriptSource},
            sequence::target_quality::{
                TargetQualityConfig,
                types::{ProbeStrategy, SubsetProbeLength, SubsetProbePosition},
            },
        },
    };
    use california_condor::{
        commands::{
            config::{ConfigAction, ProbeStrategyKind, StrategyArgs},
            handlers::{quality_check, target_quality},
        },
        configuration::{CliSequenceConfig, CliSequenceData, Configuration},
        test_helpers::{TestVideo, default_config},
    };

    fn fixture(temp: &std::path::Path) -> Configuration {
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
        configuration.condor.sequence_config.target_quality = Some(TargetQualityConfig::default());
        configuration.condor.sequence_config.quality_check = Some(Default::default());
        configuration
    }

    fn runtime(
        configuration: &Configuration,
        saved: Arc<AtomicUsize>,
    ) -> Condor<CliSequenceData, CliSequenceConfig> {
        let seed = InputModel::VapourSynthScript {
        source: VapourSynthScriptSource::Text("from vapoursynth import core, YUV420P8\ncore.std.BlankClip(width=32,height=32,length=2,format=YUV420P8).set_output()\n".into()),
        variables: Default::default(), index: 0, filters: vec![], stream_concurrently: false,
    };
        Condor::new(
            Input::from_data(&seed).expect("blank input"),
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

    fn sequence(stage: &str) -> Box<dyn Sequence<CliSequenceData, CliSequenceConfig>> {
        if stage == "TQ" {
            Box::new(TargetQuality::new(None, None))
        } else {
            Box::new(QualityCheck::new(None))
        }
    }

    #[test]
    fn failed_strategy_edits_are_offline_and_preserve_saved_bytes() {
        let temp = tempfile::tempdir().expect("fixture");
        let configuration = fixture(temp.path());
        let path = temp.path().join("condor.json");
        configuration.save(&path).expect("save");
        let before = std::fs::read(&path).expect("snapshot");
        let args = StrategyArgs {
            kind: ProbeStrategyKind::Subset,
            skip: None,
            position: None,
            length_frames: None,
            length_percent: None,
        };
        for args in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 101.0]
            .map(|percent| StrategyArgs {
                length_percent: Some(percent),
                ..args
            })
            .into_iter()
            .chain([
                StrategyArgs {
                    length_frames: Some(0),
                    ..args
                },
                StrategyArgs {
                    kind: ProbeStrategyKind::Skip,
                    skip: Some(0),
                    ..args
                },
            ])
        {
            assert!(args.build().is_err(), "accepted {args:?}");
            assert!(
                target_quality::config(
                    Some(&path),
                    &ConfigAction::Set {
                        setting: target_quality::TargetQualitySet::Strategy { args: args.clone() }
                    }
                )
                .is_err()
            );
            assert!(
                quality_check::config(
                    Some(&path),
                    &ConfigAction::Set {
                        setting: quality_check::QualityCheckSet::Strategy { args }
                    }
                )
                .is_err()
            );
            assert_eq!(std::fs::read(&path).expect("unchanged config"), before);
        }
        for percent in [f64::MIN_POSITIVE, 25.0, 100.0] {
            assert!(
                StrategyArgs {
                    length_percent: Some(percent),
                    ..args
                }
                .build()
                .is_ok()
            );
        }
        assert!(!temp.path().join("forbidden.ffindex").exists());
        assert!(!configuration.temp.exists());
    }

    #[test]
    fn stages_reject_loaded_selection_before_work() {
        let temp = tempfile::tempdir().expect("fixture");
        for (invalid_strategy, invalid_end, expected_error) in [
            (
                Some(ProbeStrategy::Subset {
                    position: SubsetProbePosition::Start,
                    length: SubsetProbeLength::Percentage(0.0),
                }),
                None,
                "percent",
            ),
            (
                Some(ProbeStrategy::Subset {
                    position: SubsetProbePosition::Middle,
                    length: SubsetProbeLength::Percentage(-1.0),
                }),
                None,
                "percent",
            ),
            (Some(ProbeStrategy::Skip { skip: 0 }), None, "skip"),
            (
                Some(ProbeStrategy::Subset {
                    position: SubsetProbePosition::End,
                    length: SubsetProbeLength::Frames(0),
                }),
                None,
                "frames",
            ),
            (None, Some(50), "range"),
            (None, Some(49), "range"),
        ] {
            for stage in ["TQ", "QC"] {
                for entry in ["validate", "initialize", "execute", "execute_cancel"] {
                    let mut configuration = fixture(temp.path());
                    if let Some(end) = invalid_end {
                        configuration.condor.scenes[1].end_frame = end;
                    }
                    if let Some(strategy) = &invalid_strategy {
                        configuration
                            .condor
                            .sequence_config
                            .target_quality
                            .as_mut()
                            .expect("TQ")
                            .probing
                            .strategy = strategy.clone();
                        configuration
                            .condor
                            .sequence_config
                            .quality_check
                            .as_mut()
                            .expect("QC")
                            .strategy = strategy.clone();
                    }
                    let bad_input = configuration.condor.input.clone();
                    configuration
                        .condor
                        .sequence_config
                        .target_quality
                        .as_mut()
                        .expect("TQ")
                        .input = Some(bad_input.clone());
                    configuration
                        .condor
                        .sequence_config
                        .quality_check
                        .as_mut()
                        .expect("QC")
                        .input = Some(bad_input);
                    let path = temp.path().join("condor.json");
                    // Simulate malformed hand-edited JSON, bypassing validated saves.
                    std::fs::write(&path, serde_json::to_vec(&configuration).expect("JSON"))
                        .expect("save loaded fixture");
                    let before = std::fs::read(&path).expect("snapshot");
                    let configuration =
                        Configuration::load(&path).expect("load").expect("saved fixture");
                    let saves = Arc::new(AtomicUsize::new(0));
                    let mut condor = runtime(&configuration, Arc::clone(&saves));
                    let mut sequence = sequence(stage);
                    let (tx, rx) = mpsc::channel();
                    let result = match entry {
                        "validate" => sequence.validate(&mut condor),
                        "initialize" => sequence.initialize(&mut condor, tx),
                        "execute" => {
                            sequence.execute(&mut condor, tx, Arc::new(AtomicBool::new(false)))
                        },
                        _ => sequence.execute_cancel(&mut condor, tx, Default::default()),
                    };
                    let error = result.expect_err("invalid selection must fail before work");
                    let message = error.to_string();
                    assert!(
                        message.contains(expected_error),
                        "{stage}/{entry}: {error:#}"
                    );
                    assert_eq!(saves.load(Ordering::Relaxed), 0);
                    assert_eq!(
                        serde_json::to_value(&condor.scenes).expect("scenes"),
                        serde_json::to_value(&configuration.condor.scenes)
                            .expect("original scenes")
                    );
                    assert_eq!(std::fs::read(&path).expect("unchanged metadata"), before);
                    assert!(rx.try_recv().is_err(), "no progress before validation");
                    assert!(!configuration.temp.exists());
                    assert!(!temp.path().join("forbidden.ffindex").exists());
                }
            }
        }
    }

    #[test]
    fn stages_keep_no_work_and_cancellation_precedence() {
        use andean_condor::utils::cancellation::{Cancellation, Cancelled};
        let temp = tempfile::tempdir().expect("fixture");
        for stage in ["TQ", "QC"] {
            for mode in ["empty", "disabled", "graceful", "forced"] {
                let mut configuration = fixture(temp.path());
                configuration
                    .condor
                    .sequence_config
                    .target_quality
                    .as_mut()
                    .expect("TQ")
                    .probing
                    .strategy = ProbeStrategy::Skip { skip: 0 };
                configuration
                    .condor
                    .sequence_config
                    .quality_check
                    .as_mut()
                    .expect("QC")
                    .strategy = ProbeStrategy::Skip { skip: 0 };
                match mode {
                    "empty" => configuration.condor.scenes.clear(),
                    "disabled" => {
                        configuration.condor.sequence_config.target_quality = None;
                        configuration.condor.sequence_config.quality_check = None;
                    },
                    _ => (),
                }
                let saves = Arc::new(AtomicUsize::new(0));
                let mut condor = runtime(&configuration, Arc::clone(&saves));
                let mut sequence = sequence(stage);
                for entry in ["initialize", "execute", "execute_cancel"] {
                    let cancellation = Cancellation::default();
                    if matches!(mode, "graceful" | "forced") {
                        cancellation.request().expect("cancel");
                        if mode == "forced" {
                            cancellation.request().expect("force");
                        }
                    }
                    let (tx, rx) = mpsc::channel();
                    let result = match entry {
                        "initialize" => sequence.initialize_cancel(&mut condor, tx, &cancellation),
                        "execute" => sequence.execute(&mut condor, tx, cancellation.legacy_flag()),
                        _ => sequence.execute_cancel(&mut condor, tx, cancellation),
                    };
                    if matches!(mode, "graceful" | "forced") {
                        assert!(
                            result.expect_err("cancel before selection").is::<Cancelled>(),
                            "{stage}/{mode}/{entry}"
                        );
                    } else {
                        let (_, warnings) = result.expect("no work");
                        assert_eq!(warnings.len(), usize::from(mode == "empty"));
                    }
                    assert!(rx.try_recv().is_err());
                }
                assert_eq!(saves.load(Ordering::Relaxed), 0);
                assert!(!configuration.temp.exists());
                assert!(!temp.path().join("forbidden.ffindex").exists());
            }
        }
    }
}

#[cfg(test)]
mod target_quality_admission {
    use super::target_quality::validation::{blank_input, fixture};
    use crate::common;
    use andean_condor::models::encoder::{Encoder, EncoderBase};
    use california_condor::configuration::Configuration;

    #[test]
    fn root_and_start_apply_repairs_before_final_validation() {
        use andean_condor::models::{
            encoder::cli_parameter::CLIParameter, sequence::target_quality::types::ProbeStrategy,
        };
        use california_condor::commands::{CondorCli, handlers::start::start_handler};
        use clap::Parser;

        let temp = tempfile::tempdir().expect("fixture");
        let path = temp.path().join("condor.json");
        let mut failures = vec![];
        for (case, flags) in [
            (
                "range",
                vec!["--minimum-quantizer", "10", "--maximum-quantizer", "30"],
            ),
            (
                "encoder",
                vec![
                    "--encoder",
                    "svt-av1",
                    "--minimum-quantizer",
                    "20",
                    "--maximum-quantizer",
                    "50",
                ],
            ),
            ("options", vec!["--tq-params", "--preset 6 --rc 0"]),
            ("profile", vec!["--target-profile", "standard"]),
        ] {
            let mut configuration = fixture(temp.path());
            // Start reads SCD metadata even without SCD flags. A generated clip
            // satisfies that existing requirement without opening/indexing the source.
            configuration.condor.sequence_config.scene_detector.input = Some(blank_input());
            let quality = configuration.condor.sequence_config.target_quality.as_mut().expect("TQ");
            match case {
                "range" => quality.quantizer_range = (40, 20),
                "encoder" => {
                    quality.quantizer_range = (50, 140);
                    configuration.condor.encoder =
                        Encoder::default_from_base(&EncoderBase::RAV1E, false);
                    for scene in &mut configuration.condor.scenes {
                        scene.encoder = configuration.condor.encoder.clone();
                    }
                },
                "options" => {
                    quality.probing.encoder_options =
                        Some(CLIParameter::new_numbers("--", " ", &[("rc", 1.0)]));
                },
                _ => quality.probing.strategy = ProbeStrategy::Skip { skip: 0 },
            }
            let before = serde_json::to_vec(&configuration).expect("hand-edited JSON");
            for entry in ["root CLI", "start handler"] {
                std::fs::write(&path, &before).expect("fixture");
                if entry == "root CLI" {
                    let output = common::condor_cmd(&temp).args(&flags).output().expect("root CLI");
                    // The start handler must save the repaired candidate before the runner
                    // reaches the missing-input sentinel; no native encode is needed.
                    assert!(!output.status.success());
                    let message = String::from_utf8_lossy(&output.stderr);
                    if !message.contains("must-not-open.mkv") {
                        failures.push(format!("{case}/{entry}: {message}"));
                    }
                } else {
                    let cli = CondorCli::try_parse_from(
                        std::iter::once("condor").chain(flags.iter().copied()),
                    )
                    .expect("root flags");
                    let result = start_handler(
                        Some(&path),
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
                    );
                    if let Err(error) = result {
                        failures.push(format!("{case}/{entry}: {error:#}"));
                    }
                }
                let repaired = Configuration::load(&path).expect("load").expect("exists");
                let quality = repaired.condor.sequence_config.target_quality.as_ref().expect("TQ");
                let expected_range = match case {
                    "range" => (10, 30),
                    "encoder" => (20, 50),
                    _ => (0, 51),
                };
                if quality.quantizer_range != expected_range
                    || quality.validate(&repaired.condor.encoder).is_err()
                {
                    failures.push(format!(
                        "{case}/{entry}: stale candidate saved: {quality:?}"
                    ));
                }
                if case == "encoder" && repaired.condor.encoder.base() != EncoderBase::SVTAV1 {
                    failures.push(format!("{case}/{entry}: actual encoder switch not saved"));
                }
                if case == "options" && quality.validate(&repaired.condor.encoder).is_ok() {
                    assert_eq!(quality.probing.encoder_options.as_ref().expect("explicit probe options")["preset"].to_string_value(), "6");
                }
                if case == "profile" && quality.validate(&repaired.condor.encoder).is_ok() {
                    assert_eq!(
                        quality
                            .probing
                            .strategy
                            .frame_indices(0, 100)
                            .expect("repaired probe strategy selection"),
                        (38..63).collect::<Vec<_>>()
                    );
                }
                assert_eq!(
                    serde_json::to_value(&repaired.condor.scenes).expect("scenes"),
                    serde_json::to_value(&configuration.condor.scenes).expect("original scenes")
                );
                assert!(!configuration.temp.exists());
                assert!(!temp.path().join("forbidden.ffindex").exists());
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn root_still_rejects_invalid_final_candidates_and_early_encoder_controls() {
        let temp = tempfile::tempdir().expect("fixture");
        let mut configuration = fixture(temp.path());
        configuration.condor.sequence_config.scene_detector.input = Some(blank_input());
        let path = temp.path().join("condor.json");
        configuration.save(&path).expect("valid save");
        let before = std::fs::read(&path).expect("snapshot");
        for (flags, expected) in [
            (
                vec!["--minimum-quantizer", "40", "--maximum-quantizer", "20"],
                "quantizer",
            ),
            (vec!["--tq-params", "--rc 1 --keyint 24"], "rc"),
            (vec!["--target", "NaN"], "finite"),
            (vec!["--params", "--preset 99"], "preset"),
            (vec!["--max-tries", "0"], "max-tries"),
        ] {
            let output = common::condor_cmd(&temp).args(flags).output().expect("root CLI");
            assert!(!output.status.success());
            let message = String::from_utf8_lossy(&output.stderr);
            assert!(message.contains(expected), "{message}");
            assert_eq!(std::fs::read(&path).expect("unchanged bytes"), before);
            assert!(!configuration.temp.exists());
            assert!(!temp.path().join("forbidden.ffindex").exists());
        }
    }
}
