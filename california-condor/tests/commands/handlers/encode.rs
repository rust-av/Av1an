use crate::common::condor_cmd;
use crate::common::config::{init_config, load};
use andean_condor::models::encoder::{
    Encoder, EncoderBase, EncoderPasses, cli_parameter::CLIParameter,
};
use california_condor::{
    commands::handlers::load_configuration, test_helpers::*, utils::hash_path::hash_path,
};

#[cfg(test)]
mod handler_configuration {
    use super::*;
    use andean_condor::{
        models::{
            encoder::photon_noise::PhotonNoise,
            input::{Input, VapourSynthImportMethod},
            sequence::parallel_encoder::ParallelEncoderConfig,
        },
        vapoursynth::{plugins::resize::Scaler, vapoursynth_filters::VapourSynthFilter},
    };
    use california_condor::commands::{
        DecoderMethod, EncoderMethod,
        handlers::{encode::encode_handler, init::init_handler},
    };

    #[test]
    fn encode_handler_without_overrides_retains_default_encoder_settings_and_config_path() {
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
        let (config, found_config_path) = encode_handler(
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
        )
        .expect("encode_handler should succeed");
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
    fn encode_handler_applies_rav1e_passes_parameters_noise_workers_and_override_input() {
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
        expected_config.condor.encoder = Encoder::default_from_base(&EncoderBase::RAV1E, false);
        expected_config.condor.encoder.parameters_mut().insert(
            "speed".to_owned(),
            CLIParameter::Number {
                prefix: "--".to_owned(),
                delimiter: " ".to_owned(),
                value: 10.0,
            },
        );
        if let Some(encoder_passes) = expected_config.condor.encoder.passes_mut() {
            *encoder_passes = EncoderPasses::All(2);
        }
        expected_config.condor.encoder.set_photon_noise(Some(PhotonNoise {
            iso: 1600,
            chroma_iso: Some(1600),
            width: None,
            height: None,
            c_y: None,
            ccb: None,
            ccr: None,
        }));
        expected_config.condor.sequence_config.parallel_encoder = ParallelEncoderConfig {
            input: Some(Input::VapourSynth {
                path: input_abs.clone(),
                import_method: VapourSynthImportMethod::DGDecNV {
                    dgindexnv_executable: None,
                },
                cache_path: None,
                filters: custom_filters.clone(),
            }),
            workers: Some(2),
            scenes_directory: temp_abs.join("scenes"),
            validate_frame_count: true,
            max_tries: 3,
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
        let (config, found_config_path) = encode_handler(
            Some(&config_path),
            None,
            Some(&test_video.path),
            Some(&DecoderMethod::DGDecodeNV),
            Some(&custom_filters),
            Some(&custom_vs_args),
            Some(2),
            Some(&EncoderMethod::RAV1E),
            Some(2),
            Some("--speed 10"),
            Some(1600),
            Some(1600),
            None,
            None,
        )
        .expect("encode_handler should succeed");
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
    fn encode_handler_filters_target_existing_override() {
        let test_video = get_test_video();
        let input_abs = path_abs::PathAbs::new(&test_video.path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let temp = tempfile::tempdir().expect("temp directory");
        let output = temp.path().join("out.mkv");
        let config_path = temp.path().join("condor.json");
        let custom_filters = vec![VapourSynthFilter::Resize {
            scaler: Some(Scaler::Point),
            width: Some(960),
            height: Some(540),
            format: None,
        }];
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
        let (mut config, _) =
            load_configuration(Some(&config_path)).expect("load_configuration should succeed");
        config.condor.sequence_config.parallel_encoder.input = Some(Input::VapourSynth {
            path: input_abs,
            import_method: VapourSynthImportMethod::DGDecNV {
                dgindexnv_executable: None,
            },
            cache_path: None,
            filters: vec![],
        });
        let mut expected = config.clone();
        if let Some(Input::VapourSynth { filters, .. }) =
            &mut expected.condor.sequence_config.parallel_encoder.input
        {
            *filters = custom_filters.clone();
        }
        config.save(&config_path).expect("save should succeed");
        let (config, _) = encode_handler(
            Some(&config_path),
            None,
            None,
            None,
            Some(&custom_filters),
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
        .expect("encode_handler should succeed");
        assert_eq!(
            serde_json::to_value(&config).expect("actual configuration"),
            serde_json::to_value(&expected).expect("only override filters replaced")
        );
    }
}

#[cfg(test)]
mod avm_persistence {
    use super::*;
    use andean_condor::models::encoder::photon_noise::PhotonNoise;
    use california_condor::{
        commands::{
            EncoderMethod,
            config::{ConfigAction, ConfigCommand},
            handlers::encode::{
                EncodeConfigError, EncodeCrud, EncodeKey, EncodeSet, config, encode_handler,
            },
        },
        configuration::Configuration,
    };
    use std::fs;

    fn check_avm_persistence(phase: u8) {
        assert!((1..=4).contains(&phase));
        use california_condor::test_helpers::TestVideo;
        let temp = tempfile::tempdir().expect("configuration fixture");
        let video = TestVideo {
            path: temp.path().join("missing-source.mkv"),
            width: 64,
            height: 64,
            frames: 1,
            fps_rational: (24, 1),
            format: andean_condor::ffmpeg::FFPixelFormat::YUV420P,
            scenes: vec![],
        };
        let mut configuration = default_config(&video, &temp.path().join("out.mkv"), temp.path());
        configuration.condor.encoder = Encoder::default_from_base(&EncoderBase::AVM, false);
        configuration
            .condor
            .encoder
            .set_executable(Some(temp.path().join("missing-avmenc")));
        let path = temp.path().join("condor.json");
        configuration.save(&path).expect("saved config");
        let before = fs::read(&path).expect("original bytes");
        let original = serde_json::to_value(&configuration).expect("original config");
        let set = EncodeSet::PhotonNoise {
            iso: 800,
            chroma_noise: Some(400),
        };
        let error = EncodeCrud::apply(&mut configuration, &set).expect_err("atomic in-memory edit");
        assert_eq!(
            error.downcast_ref::<EncodeConfigError>(),
            Some(&EncodeConfigError::PhotonNoiseNotSupported(
                EncoderBase::AVM
            ))
        );
        assert_eq!(
            serde_json::to_value(&configuration).expect("unchanged config"),
            original
        );
        let error = config(Some(&path), &ConfigAction::Set { setting: set })
            .expect_err("atomic saved edit");
        assert_eq!(
            error.downcast_ref::<EncodeConfigError>(),
            Some(&EncodeConfigError::PhotonNoiseNotSupported(
                EncoderBase::AVM
            ))
        );
        assert_eq!(fs::read(&path).expect("saved bytes"), before);
        if phase == 1 {
            return;
        }
        configuration.condor.encoder.set_photon_noise(Some(PhotonNoise {
            iso: 800,
            chroma_iso: Some(400),
            width: None,
            height: None,
            c_y: None,
            ccb: None,
            ccr: None,
        }));
        configuration.save(&path).expect("legacy saved request");
        let legacy = fs::read(&path).expect("legacy bytes");
        let error = encode_handler(
            Some(&path),
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
        )
        .expect_err("loaded noise must not silently run");
        assert_eq!(
            error.downcast_ref::<EncodeConfigError>(),
            Some(&EncodeConfigError::PhotonNoiseNotSupported(
                EncoderBase::AVM
            ))
        );
        assert_eq!(fs::read(&path).expect("unchanged legacy bytes"), legacy);
        config(
            Some(&path),
            &ConfigAction::Reset {
                key: Some(EncodeKey::PhotonNoise),
            },
        )
        .expect("clear legacy noise");
        let repaired = Configuration::load(&path).expect("load").expect("saved repair");
        assert!(repaired.condor.encoder.photon_noise().is_none());
        assert_eq!(repaired.condor.encoder.validate_settings(), Ok(()));
        if phase == 2 {
            return;
        }
        config(
            Some(&path),
            &ConfigAction::Set {
                setting: EncodeSet::Params {
                    params: "--film-grain-table=caller-filmgrn2.tbl".into(),
                },
            },
        )
        .expect("raw native option stays available");
        let raw = Configuration::load(&path).expect("load").expect("saved raw option");
        assert_eq!(
            raw.condor.encoder.parameters()["film-grain-table"].to_string_value(),
            "caller-filmgrn2.tbl"
        );
        if phase == 3 {
            return;
        }
        configuration.save(&path).expect("restore legacy fixture");
        config(
            Some(&path),
            &ConfigAction::Set {
                setting: EncodeSet::Encoder {
                    encoder: EncoderMethod::AOM,
                },
            },
        )
        .expect("switch encoder to repair legacy settings");
        let switched = Configuration::load(&path).expect("load").expect("saved switch");
        assert_eq!(switched.condor.encoder.base(), EncoderBase::AOM);
        assert_eq!(switched.condor.encoder.validate_settings(), Ok(()));
        assert_eq!(
            fs::read_dir(temp.path()).expect("no indexing or side effects").count(),
            1
        );
    }

    #[test]
    fn avm_noise_crud_rejects_unsupported_edits_without_changing_saved_bytes() {
        check_avm_persistence(1);
    }

    #[test]
    fn avm_legacy_noise_clear_repairs_configuration_without_resolving_source_or_encoder() {
        check_avm_persistence(2);
    }

    #[test]
    fn avm_raw_grain_parameter_survives_offline_edit() {
        check_avm_persistence(3);
    }

    #[test]
    fn avm_switch_to_capable_encoder_repairs_legacy_noise_settings_without_indexing() {
        check_avm_persistence(4);
    }
}

#[cfg(test)]
mod tests {
    use andean_condor::{
        ffmpeg::FFPixelFormat,
        models::{
            input::Input,
            sequence::scene_concatenator::{ConcatMethod, FfmpegConfig, MkvmergeConfig},
        },
        vapoursynth::{plugins::resize::Scaler, vapoursynth_filters::VapourSynthFilter},
    };
    use serial_test::serial;

    use super::*;

    const WIDTH: usize = 960;
    const HEIGHT: usize = 540;

    fn input_override_configuration(
        directory: &std::path::Path,
    ) -> california_condor::configuration::Configuration {
        use andean_condor::models::{
            input::{ChromaSampling, Ffms2Filter, ImportMethod},
            sequence::{quality_check::QualityCheckConfig, target_quality::TargetQualityConfig},
        };
        let video = TestVideo {
            path: directory.join("root.mkv"),
            width: 32,
            height: 16,
            frames: 2,
            fps_rational: (24, 1),
            format: FFPixelFormat::YUV420P,
            scenes: vec![],
        };
        let mut configuration =
            default_config(&video, &directory.join("out.mkv"), &directory.join("work"));
        configuration.condor.input = Input::Video {
            path: video.path,
            import_method: ImportMethod::FFMS2 {
                index: Some(0),
                cache_path: Some(directory.join("root.ffindex")),
            },
            filters: vec![Ffms2Filter::OutputFormat {
                bit_depth: Some(10),
                chroma: Some(ChromaSampling::Yuv420),
                width: None,
                height: None,
            }],
        };
        let mut reference = configuration.condor.input.clone();
        if let Input::Video { path, .. } = &mut reference {
            *path = directory.join("metric.mkv");
        }
        configuration.condor.sequence_config.target_quality = Some(TargetQualityConfig {
            input: Some(reference.clone()),
            metric_input: Some(reference.clone()),
            ..Default::default()
        });
        configuration.condor.sequence_config.quality_check = Some(QualityCheckConfig {
            input: Some(reference),
            ..Default::default()
        });
        configuration
    }

    fn rgb_input(path: std::path::PathBuf) -> Input {
        Input::VapourSynth {
            path,
            import_method: andean_condor::models::input::VapourSynthImportMethod::BestSource {
                index: Some(0),
            },
            cache_path: None,
            filters: vec![VapourSynthFilter::Resize {
                scaler: None,
                width: None,
                height: None,
                format: Some(FFPixelFormat::GBRP),
            }],
        }
    }

    fn check_filter_conversion(reject_invalid: bool) {
        use andean_condor::models::input::{ChromaSampling, Ffms2Filter, ImportMethod};
        use california_condor::commands::{
            DecoderMethod, handlers::encode::configure_parallel_encoder,
        };
        let temp = tempfile::tempdir().expect("fixture");
        for with_override in [false, true] {
            let mut configuration = input_override_configuration(temp.path());
            configuration.condor.input = rgb_input(temp.path().join("root.mkv"));
            let source = temp.path().join(if with_override {
                "override.mkv"
            } else {
                "root.mkv"
            });
            if with_override {
                configuration.condor.sequence_config.parallel_encoder.input =
                    Some(rgb_input(source.clone()));
            }
            let original = configuration.clone();
            let mut expected = original.clone();
            expected.condor.sequence_config.parallel_encoder.input = Some(Input::Video {
                path: source,
                import_method: ImportMethod::FFMS2 {
                    index: Some(0),
                    cache_path: None,
                },
                filters: vec![Ffms2Filter::OutputFormat {
                    bit_depth: Some(8),
                    chroma: Some(ChromaSampling::Yuv420),
                    width: None,
                    height: None,
                }],
            });
            expected.condor.sequence_config.parallel_encoder.workers = Some(7);
            expected.condor.sequence_config.parallel_encoder.max_tries = 9;
            let replacement = VapourSynthFilter::Resize {
                scaler: None,
                width: None,
                height: None,
                format: Some(FFPixelFormat::YUV420P),
            };
            configure_parallel_encoder(
                &mut configuration,
                None,
                Some(&DecoderMethod::FFMS2),
                Some(&[replacement]),
                None,
                Some(7),
                Some(9),
            )
            .expect("explicit replacement must not convert obsolete GBRP filters");
            assert_eq!(
                serde_json::to_value(configuration).expect("actual"),
                serde_json::to_value(expected)
                    .expect("only override and execution settings change")
            );

            if !reject_invalid {
                continue;
            }
            let invalid = [VapourSynthFilter::Resize {
                scaler: None,
                width: None,
                height: None,
                format: Some(FFPixelFormat::NV12),
            }];
            for (filters, message) in [(None, "gbrp"), (Some(invalid.as_slice()), "nv12")] {
                let mut configuration = original.clone();
                let error = configure_parallel_encoder(
                    &mut configuration,
                    None,
                    Some(&DecoderMethod::FFMS2),
                    filters,
                    None,
                    Some(7),
                    Some(9),
                )
                .expect_err("omitted filters validate inheritance; bad replacement still fails");
                assert!(error.to_string().contains(message), "{error:#}");
                assert_eq!(
                    serde_json::to_value(configuration).expect("atomic error"),
                    serde_json::to_value(&original)
                        .expect("unchanged inputs and execution settings")
                );
            }
        }
        assert_eq!(
            std::fs::read_dir(temp.path()).expect("no offline source/index work").count(),
            0
        );
    }

    #[test]
    fn explicit_native_filters_replace_obsolete_inheritance_without_source_or_index_files() {
        check_filter_conversion(false);
    }

    #[test]
    fn invalid_or_omitted_native_filter_conversion_is_atomic_and_creates_no_files() {
        check_filter_conversion(true);
    }

    fn check_native_filter_cli(invalid: bool) {
        use andean_condor::models::input::{ChromaSampling, Ffms2Filter, ImportMethod};
        let temp = tempfile::tempdir().expect("fixture");
        let file = temp.path().join("condor.json");
        for with_override in [false, true] {
            let mut configuration = input_override_configuration(temp.path());
            configuration.condor.input = rgb_input(temp.path().join("root.mkv"));
            let source = temp.path().join(if with_override {
                "override.mkv"
            } else {
                "root.mkv"
            });
            if with_override {
                configuration.condor.sequence_config.parallel_encoder.input =
                    Some(rgb_input(source.clone()));
            }
            let mut expected = configuration.clone();
            expected.condor.sequence_config.parallel_encoder.input = Some(Input::Video {
                path: source,
                import_method: ImportMethod::FFMS2 {
                    index: Some(0),
                    cache_path: None,
                },
                filters: vec![Ffms2Filter::OutputFormat {
                    bit_depth: Some(8),
                    chroma: Some(ChromaSampling::Yuv420),
                    width: None,
                    height: None,
                }],
            });
            configuration.save(&file).expect("save RGB input");
            condor_cmd(&temp)
                .env("PATH", "")
                .args([
                    "encode",
                    "set",
                    "input",
                    "--decoder",
                    "ffms2",
                    "--filters",
                    "resize:format=yuv420p",
                ])
                .assert()
                .success();
            assert_eq!(
                serde_json::to_value(load(&file)).expect("saved CLI edit"),
                serde_json::to_value(expected)
                    .expect("root and separate reference configurations unchanged")
            );
            if !invalid {
                continue;
            }
            configuration.save(&file).expect("restore RGB input");
            let disk = std::fs::read(&file).expect("snapshot");
            for (args, message) in [
                (vec!["encode", "set", "input", "--decoder", "ffms2"], "gbrp"),
                (
                    vec![
                        "encode",
                        "set",
                        "input",
                        "--decoder",
                        "ffms2",
                        "--filters",
                        "resize:format=nv12",
                    ],
                    "nv12",
                ),
            ] {
                let output = condor_cmd(&temp)
                    .env("PATH", "")
                    .args(args)
                    .assert()
                    .failure()
                    .get_output()
                    .clone();
                assert!(
                    String::from_utf8_lossy(&output.stderr).contains(message),
                    "{output:?}"
                );
                assert_eq!(std::fs::read(&file).expect("atomic saved bytes"), disk);
            }
        }
        let mut entries = std::fs::read_dir(temp.path())
            .expect("offline side effects")
            .map(|entry| entry.expect("entry").file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        entries.sort();
        assert_eq!(entries, ["condor.json", "condor.json.lock", "logs"]);
    }

    #[test]
    fn encode_input_crud_explicit_native_filters_replace_gbrp_without_changing_references() {
        check_native_filter_cli(false);
    }

    #[test]
    fn encode_input_crud_omitted_or_invalid_native_filters_preserve_saved_bytes() {
        check_native_filter_cli(true);
    }

    #[test]
    fn filter_only_crud_preserves_override_identity_and_references_offline() {
        use andean_condor::models::input::{
            Ffms2Filter, ImportMethod, VapourSynthImportMethod, VapourSynthScriptSource,
        };
        use california_condor::commands::{
            config::{ConfigCommand, InputOverride},
            handlers::encode::{EncodeCrud, EncodeSet},
        };
        let temp = tempfile::tempdir().expect("fixture");
        let filters = vec![VapourSynthFilter::Resize {
            scaler: Some(Scaler::Point),
            width: Some(16),
            height: Some(8),
            format: None,
        }];
        let native = Input::Video {
            path: temp.path().join("override.mkv"),
            import_method: ImportMethod::FFMS2 {
                index: Some(3),
                cache_path: Some(temp.path().join("keep.ffindex")),
            },
            filters: vec![],
        };
        let indexed = Input::VapourSynth {
            path: temp.path().join("override.mkv"),
            import_method: VapourSynthImportMethod::FFMS2 { index: Some(3) },
            cache_path: Some(temp.path().join("keep.vsindex")),
            filters: vec![],
        };
        let scripts = [
            VapourSynthScriptSource::Path(temp.path().join("missing.vpy")),
            VapourSynthScriptSource::Text("raise RuntimeError('must not execute')".into()),
        ]
        .map(|source| Input::VapourSynthScript {
            source,
            variables: [("keep".into(), "yes".into())].into(),
            index: 3,
            filters: vec![],
            stream_concurrently: false,
        });
        for input in [native, indexed, scripts[0].clone(), scripts[1].clone()] {
            let mut configuration = input_override_configuration(temp.path());
            configuration.condor.sequence_config.parallel_encoder.input = Some(input.clone());
            let mut expected = configuration.clone();
            let mut filtered = input;
            match &mut filtered {
                Input::Video { filters, .. } => {
                    *filters = vec![Ffms2Filter::OutputFormat {
                        bit_depth: None,
                        chroma: None,
                        width: Some(16),
                        height: Some(8),
                    }];
                },
                Input::VapourSynth {
                    filters: target, ..
                }
                | Input::VapourSynthScript {
                    filters: target, ..
                } => *target = filters.clone(),
            }
            expected.condor.sequence_config.parallel_encoder.input = Some(filtered);
            let file = temp.path().join("condor.json");
            configuration.save(&file).expect("save");
            let disk = std::fs::read(&file).expect("snapshot");
            EncodeCrud::apply(
                &mut configuration,
                &EncodeSet::Input {
                    args: InputOverride {
                        path: None,
                        decoder: None,
                        filters: Some(filters.clone()),
                        vs_args: None,
                        stream_concurrently: None,
                    },
                },
            )
            .expect("filter-only edit without rebuilding or opening source");
            assert_eq!(
                serde_json::to_value(&configuration).expect("after"),
                serde_json::to_value(expected).expect("only override filters changed")
            );
            assert_eq!(std::fs::read(&file).expect("no implicit save"), disk);
        }
        assert_eq!(
            std::fs::read_dir(temp.path()).expect("no indexes or sources opened").count(),
            1
        );
    }

    fn check_input_override(phase: u8) {
        assert!((1..=4).contains(&phase));
        use andean_condor::models::input::{Ffms2Filter, ImportMethod};
        use california_condor::commands::{
            DecoderMethod, handlers::encode::configure_parallel_encoder,
        };
        let temp = tempfile::tempdir().expect("fixture");
        let original = input_override_configuration(temp.path());
        for with_override in [false, true] {
            let mut configuration = original.clone();
            if with_override {
                configuration.condor.sequence_config.parallel_encoder.input = Some(Input::Video {
                    path: temp.path().join("old.mkv"),
                    import_method: ImportMethod::FFMS2 {
                        index: Some(2),
                        cache_path: None,
                    },
                    filters: vec![Ffms2Filter::OutputFormat {
                        bit_depth: Some(12),
                        chroma: None,
                        width: Some(24),
                        height: Some(12),
                    }],
                });
            }
            let snapshot = serde_json::to_value(&configuration).expect("snapshot");
            configure_parallel_encoder(&mut configuration, None, None, None, None, None, None)
                .expect("no flags");
            assert_eq!(
                serde_json::to_value(&configuration).expect("no flags"),
                snapshot
            );
            let mut expected = configuration.clone();
            expected.condor.sequence_config.parallel_encoder.workers = Some(2);
            expected.condor.sequence_config.parallel_encoder.max_tries = 4;
            configure_parallel_encoder(
                &mut configuration,
                None,
                None,
                None,
                None,
                Some(2),
                Some(4),
            )
            .expect("workers and attempts only");
            assert_eq!(
                serde_json::to_value(&configuration).expect("settings only"),
                serde_json::to_value(&expected).expect("inputs unchanged")
            );
            if phase == 1 {
                continue;
            }
            let mut inherited = if with_override {
                expected
                    .condor
                    .sequence_config
                    .parallel_encoder
                    .input
                    .clone()
                    .expect("override")
            } else {
                expected.condor.input.clone()
            };
            if let Input::Video { path, .. } = &mut inherited {
                *path = temp.path().join("new.mkv");
            }
            expected.condor.sequence_config.parallel_encoder.input = Some(inherited);
            configure_parallel_encoder(
                &mut configuration,
                Some(&temp.path().join("new.mkv")),
                None,
                None,
                None,
                None,
                None,
            )
            .expect("new path inherits prior effective filters");
            assert_eq!(
                serde_json::to_value(&configuration).expect("inherited"),
                serde_json::to_value(&expected).expect("root and metric inputs unchanged")
            );
            if phase == 2 {
                continue;
            }
            configure_parallel_encoder(
                &mut configuration,
                None,
                Some(&DecoderMethod::VSFFMS2),
                None,
                None,
                None,
                None,
            )
            .expect("native to VS deferred filters");
            configure_parallel_encoder(
                &mut configuration,
                None,
                Some(&DecoderMethod::FFMS2),
                None,
                None,
                None,
                None,
            )
            .expect("VS to native filters");
            // Generic decoder rebuilding does not carry a native index cache through VS.
            if let Some(Input::Video {
                import_method: ImportMethod::FFMS2 { cache_path, .. },
                ..
            }) = &mut expected.condor.sequence_config.parallel_encoder.input
            {
                *cache_path = None;
            }
            assert_eq!(
                serde_json::to_value(&configuration).expect("roundtrip"),
                serde_json::to_value(expected).expect("inherited conversions retained")
            );
        }
        if phase == 4 {
            let mut configuration = original.clone();
            let filters = [VapourSynthFilter::Resize {
                scaler: None,
                width: Some(16),
                height: Some(8),
                format: Some(FFPixelFormat::YUV420P),
            }];
            let mut expected = original;
            if let Input::Video { filters, .. } = &mut expected.condor.input {
                *filters = vec![Ffms2Filter::OutputFormat {
                    bit_depth: Some(8),
                    chroma: Some(andean_condor::models::input::ChromaSampling::Yuv420),
                    width: Some(16),
                    height: Some(8),
                }];
            }
            configure_parallel_encoder(
                &mut configuration,
                None,
                None,
                Some(&filters),
                None,
                None,
                None,
            )
            .expect("main-only edit");
            assert_eq!(
                serde_json::to_value(configuration).expect("main only"),
                serde_json::to_value(expected)
                    .expect("no override created; separate metric inputs unchanged")
            );
        }
        assert_eq!(
            std::fs::read_dir(temp.path()).expect("offline edits").count(),
            0
        );
    }

    #[test]
    fn no_input_flags_preserve_models_without_creating_source_or_index_files() {
        check_input_override(1);
    }

    #[test]
    fn new_input_override_inherits_effective_filters_without_creating_files() {
        check_input_override(2);
    }

    #[test]
    fn decoder_roundtrip_preserves_filters_drops_native_cache_and_creates_no_files() {
        check_input_override(3);
    }

    #[test]
    fn filters_only_edit_changes_main_input_without_creating_override_or_files() {
        check_input_override(4);
    }

    #[test]
    fn parallel_encoder_invalid_input_candidates_preserve_memory_and_saved_bytes_across_public_edit_routes()
     {
        use andean_condor::models::input::VapourSynthImportMethod;
        use california_condor::commands::{
            DecoderMethod,
            config::{ConfigAction, ConfigCommand, InputOverride},
            handlers::encode::{
                self, EncodeCrud, EncodeSet, configure_parallel_encoder, encode_handler,
            },
        };
        let temp = tempfile::tempdir().expect("fixture");
        for (decoder, filter, message) in [
            (
                None,
                VapourSynthFilter::Resize {
                    scaler: None,
                    width: Some(16),
                    height: None,
                    format: Some(FFPixelFormat::NV12),
                },
                "nv12",
            ),
            (
                None,
                VapourSynthFilter::Resize {
                    scaler: None,
                    width: Some(0),
                    height: None,
                    format: None,
                },
                "width",
            ),
            (
                Some(DecoderMethod::FFMS2),
                VapourSynthFilter::Resize {
                    scaler: None,
                    width: None,
                    height: None,
                    format: Some(FFPixelFormat::GBRP),
                },
                "gbrp",
            ),
        ] {
            let mut configuration = input_override_configuration(temp.path());
            let input = if decoder.is_some() {
                Input::VapourSynth {
                    path: temp.path().join("missing.mkv"),
                    import_method: VapourSynthImportMethod::FFMS2 { index: Some(2) },
                    cache_path: None,
                    filters: vec![filter.clone()],
                }
            } else {
                configuration.condor.input.clone()
            };
            configuration.condor.sequence_config.parallel_encoder.input = Some(input);
            let before = serde_json::to_value(&configuration).expect("snapshot");
            let file = temp.path().join("condor.json");
            configuration.save(&file).expect("save");
            let disk = std::fs::read(&file).expect("disk");
            let error = configure_parallel_encoder(
                &mut configuration,
                Some(&temp.path().join("new.mkv")),
                decoder.as_ref(),
                Some(std::slice::from_ref(&filter)),
                None,
                Some(7),
                Some(9),
            )
            .expect_err("invalid candidate");
            assert!(error.to_string().contains(message), "{error:#}");
            assert_eq!(
                serde_json::to_value(&configuration).expect("atomic"),
                before
            );
            let setting = EncodeSet::Input {
                args: InputOverride {
                    path: None,
                    decoder: decoder.clone(),
                    filters: Some(vec![filter.clone()]),
                    vs_args: None,
                    stream_concurrently: None,
                },
            };
            assert!(EncodeCrud::apply(&mut configuration, &setting).is_err());
            assert_eq!(
                serde_json::to_value(&configuration).expect("atomic CRUD"),
                before
            );
            assert!(encode::config(Some(&file), &ConfigAction::Set { setting }).is_err());
            assert!(
                encode_handler(
                    Some(&file),
                    None,
                    None,
                    decoder.as_ref(),
                    Some(&[filter]),
                    None,
                    Some(7),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some(9)
                )
                .is_err()
            );
            assert_eq!(std::fs::read(&file).expect("disk unchanged"), disk);
        }
        assert_eq!(
            std::fs::read_dir(temp.path()).expect("no source/index work").count(),
            1
        );
    }

    fn check_filters_only_cli(invalid: bool) {
        use andean_condor::models::input::Ffms2Filter;
        let temp = tempfile::tempdir().expect("fixture");
        let mut configuration = input_override_configuration(temp.path());
        let mut input = configuration.condor.input.clone();
        if let Input::Video { path, .. } = &mut input {
            *path = temp.path().join("override.mkv");
        }
        configuration.condor.sequence_config.parallel_encoder.input = Some(input);
        let mut expected = configuration.clone();
        if let Some(Input::Video { filters, .. }) =
            &mut expected.condor.sequence_config.parallel_encoder.input
        {
            *filters = vec![Ffms2Filter::OutputFormat {
                bit_depth: None,
                chroma: None,
                width: Some(16),
                height: Some(8),
            }];
        }
        let file = temp.path().join("condor.json");
        for args in [
            vec!["encode", "set", "input", "--filters", "resize:width=16;height=8"],
            vec!["encode", "--filters", "resize:width=16;height=8"],
        ] {
            configuration.save(&file).expect("restore saved override");
            let output = condor_cmd(&temp).env("PATH", "").args(&args).output().expect("real CLI");
            if args.contains(&"set") {
                assert!(output.status.success(), "{output:?}");
            } else {
                assert!(
                    !output.status.success(),
                    "missing root must prevent execution"
                );
            }
            assert_eq!(
                serde_json::to_value(load(&file)).expect("CLI saved edit"),
                serde_json::to_value(&expected).expect("only effective filters changed")
            );
        }
        if invalid {
            let disk = std::fs::read(&file).expect("saved good edit");
            for filter in ["resize:format=nv12", "resize:width=0"] {
                condor_cmd(&temp)
                    .env("PATH", "")
                    .args(["encode", "set", "input", "--filters", filter])
                    .assert()
                    .failure();
                assert_eq!(std::fs::read(&file).expect("failed CLI bytes"), disk);
            }
        }
        let mut entries = std::fs::read_dir(temp.path())
            .expect("offline side effects")
            .map(|entry| entry.expect("entry").file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        entries.sort();
        assert_eq!(entries, ["condor.json", "condor.json.lock", "logs"]);
    }

    #[test]
    fn encode_filters_only_cli_and_crud_save_effective_override_before_execution() {
        check_filters_only_cli(false);
    }

    #[test]
    fn encode_filters_only_invalid_crud_preserves_saved_job_without_native_work() {
        check_filters_only_cli(true);
    }

    #[test]
    fn native_effective_inputs_decode_literal_filtered_shapes() {
        use andean_condor::models::input::ImportMethod;
        use california_condor::{
            commands::DecoderMethod, commands::handlers::encode::configure_parallel_encoder,
            tui::open_input,
        };
        use std::sync::{Arc, atomic::AtomicBool};
        let temp = tempfile::tempdir().expect("fixture");
        for (name, size) in [("root.mkv", "32x16"), ("override.mkv", "48x24")] {
            let generated = std::process::Command::new("ffmpeg")
                .args([
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    &format!("color=c=black:s={size}:r=24"),
                    "-frames:v",
                    "2",
                    "-pix_fmt",
                    "yuv420p",
                    "-c:v",
                    "ffv1",
                ])
                .arg(temp.path().join(name))
                .output()
                .expect("FFmpeg required for finite fixture");
            assert!(
                generated.status.success(),
                "{}",
                String::from_utf8_lossy(&generated.stderr)
            );
        }
        let root_bytes = std::fs::read(temp.path().join("root.mkv")).expect("root bytes");
        let override_bytes =
            std::fs::read(temp.path().join("override.mkv")).expect("override bytes");
        let mut original = input_override_configuration(temp.path());
        if let Input::Video { filters, .. } = &mut original.condor.input {
            filters.clear();
        }
        let override_input = Input::Video {
            path: temp.path().join("override.mkv"),
            import_method: ImportMethod::FFMS2 {
                index: Some(0),
                cache_path: Some(temp.path().join("override.ffindex")),
            },
            filters: vec![],
        };
        let filters = [VapourSynthFilter::Resize {
            scaler: None,
            width: Some(16),
            height: Some(8),
            format: Some(FFPixelFormat::YUV420P),
        }];
        for case in [
            "existing-override",
            "new-override",
            "main-only",
            "rgb-main-rebuild",
            "rgb-override-rebuild",
        ] {
            let mut configuration = original.clone();
            if case == "existing-override" {
                configuration.condor.sequence_config.parallel_encoder.input =
                    Some(override_input.clone());
            }
            let rgb_rebuild = case.starts_with("rgb-");
            if rgb_rebuild {
                configuration.condor.input = rgb_input(temp.path().join("root.mkv"));
                if case == "rgb-override-rebuild" {
                    configuration.condor.sequence_config.parallel_encoder.input =
                        Some(rgb_input(temp.path().join("override.mkv")));
                }
            }
            let before = serde_json::to_value(&configuration.condor.input).expect("root reference");
            let path = temp.path().join("override.mkv");
            configure_parallel_encoder(
                &mut configuration,
                (case == "new-override").then_some(path.as_path()),
                rgb_rebuild.then_some(&DecoderMethod::FFMS2),
                Some(&filters),
                None,
                None,
                None,
            )
            .expect("effective input edit");
            let effective = if case == "main-only" {
                assert!(configuration.condor.sequence_config.parallel_encoder.input.is_none());
                &configuration.condor.input
            } else {
                assert_eq!(
                    serde_json::to_value(&configuration.condor.input).expect("root unchanged"),
                    before
                );
                configuration
                    .condor
                    .sequence_config
                    .parallel_encoder
                    .input
                    .as_ref()
                    .expect("override")
            };
            let cancelled = Arc::new(AtomicBool::new(false));
            let (mut decoded, info) =
                open_input(effective, &cancelled).expect("actual FFMS2 filtered decode");
            assert_eq!(info.resolution, (16, 8), "{case}");
            assert_eq!(
                decoded.y4m_frame(0).expect("real first frame").into_inner().len(),
                6 + 16 * 8 * 3 / 2,
                "{case}: FRAME header plus 8-bit 420 planes"
            );
            let (mut reference, info) =
                open_input(&original.condor.input, &cancelled).expect("unmodified root decode");
            assert_eq!(info.resolution, (32, 16));
            assert_eq!(
                reference.y4m_frame(0).expect("reference frame").into_inner().len(),
                6 + 32 * 16 * 3 / 2
            );
            let (mut source, info) =
                open_input(&override_input, &cancelled).expect("unmodified override source decode");
            assert_eq!(info.resolution, (48, 24));
            assert_eq!(
                source.y4m_frame(0).expect("source frame").into_inner().len(),
                6 + 48 * 24 * 3 / 2
            );
            assert_eq!(
                serde_json::to_value(&configuration.condor.sequence_config.target_quality)
                    .expect("TQ"),
                serde_json::to_value(&original.condor.sequence_config.target_quality)
                    .expect("TQ unchanged")
            );
            assert_eq!(
                serde_json::to_value(&configuration.condor.sequence_config.quality_check)
                    .expect("QC"),
                serde_json::to_value(&original.condor.sequence_config.quality_check)
                    .expect("QC unchanged")
            );
            eprintln!(
                "input override {case}: effective=16x8, root=32x16, override source=48x24; decoded frame payloads verified"
            );
        }
        assert_eq!(
            std::fs::read(temp.path().join("root.mkv")).expect("root unchanged"),
            root_bytes
        );
        assert_eq!(
            std::fs::read(temp.path().join("override.mkv")).expect("override unchanged"),
            override_bytes
        );
    }

    #[test]
    fn parallel_encoder_temp_rebase_publishes_chunk_completion_and_tq_initializer_uses_new_root() {
        use andean_condor::{
            core::sequence::{
                Sequence, SequenceCompletion, SequenceStatus, Status,
                parallel_encoder::ParallelEncoder, target_quality::TargetQuality,
            },
            models::{
                input::ImportMethod,
                sequence::target_quality::{TargetQualityConfig, types::ProbeStrategy},
            },
        };
        use california_condor::commands::handlers::encode::encode_handler;
        use std::sync::{Arc, atomic::AtomicBool};

        let temp = tempfile::tempdir().expect("fixture");
        let base = std::fs::canonicalize(temp.path()).expect("canonical fixture");
        let source = base.join("finite.y4m");
        let generated = std::process::Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=black:s=32x16:r=24",
                "-frames:v",
                "4",
                "-pix_fmt",
                "yuv420p",
                "-f",
                "yuv4mpegpipe",
            ])
            .arg(&source)
            .output()
            .expect("FFmpeg fixture generator is required");
        assert!(
            generated.status.success(),
            "{}",
            String::from_utf8_lossy(&generated.stderr)
        );
        let video = TestVideo {
            path: source.clone(),
            width: 32,
            height: 16,
            frames: 4,
            fps_rational: (24, 1),
            format: FFPixelFormat::YUV420P,
            scenes: vec![(0, 4)],
        };
        let old = base.join("old");
        std::fs::create_dir_all(&old).expect("old root");
        std::fs::write(old.join("sentinel"), b"not moved").expect("old root sentinel");
        let mut configuration = default_config(&video, &base.join("out.mkv"), &old);
        configuration.condor.input = Input::Video {
            path: source,
            import_method: ImportMethod::FFMS2 {
                index: None,
                cache_path: Some(base.join("source.ffindex")),
            },
            filters: vec![],
        };
        configuration.condor.encoder = Encoder::default_from_base(&EncoderBase::FFmpeg, true);
        configuration.condor.encoder.set_append_color_data(false);
        configuration
            .condor
            .encoder
            .parameters_mut()
            .insert("c:v".into(), CLIParameter::new_string("-", " ", "ffv1"));
        configuration.condor.sequence_config.parallel_encoder.workers = Some(1);
        configuration.condor.sequence_config.parallel_encoder.max_tries = 1;
        let file = base.join("condor.json");
        configuration.save(&file).expect("save empty job");
        let selected = base.join("new");
        let (mut configuration, _) = encode_handler(
            Some(&file),
            Some(&selected),
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
        )
        .expect("edit empty job before adding scenes");
        assert_eq!(
            configuration.condor.sequence_config.parallel_encoder.scenes_directory,
            selected.join("scenes")
        );
        assert_eq!(
            configuration.condor.sequence_config.scene_concatenator.scenes_directory,
            selected.join("scenes")
        );
        assert!(!selected.exists());
        configuration.condor.scenes = video.mock_scenes(&configuration.condor.encoder);
        configuration.save(&file).expect("add one real scene");
        let template = configuration.clone();
        let saved_file = file.clone();
        let mut condor = configuration
            .instantiate_condor(Box::new(move |model| {
                let mut saved = template.clone();
                saved.condor = model;
                saved.save(&saved_file)?;
                Ok(())
            }))
            .expect("real native FFMS2 input");
        let (tx, rx) = std::sync::mpsc::channel();
        let mut sequence = ParallelEncoder::default();
        sequence.validate(&mut condor).expect("real FFmpeg + frame-count prerequisites");
        sequence
            .initialize(&mut condor, tx.clone())
            .expect("owning work directory initialization");
        sequence
            .execute(&mut condor, tx, Arc::new(AtomicBool::new(false)))
            .expect("finite real FFmpeg encode and decoder-backed validation");
        let chunk = selected.join("scenes/00000.mkv");
        let bytes = std::fs::metadata(&chunk).expect("new-root published chunk").len();
        assert!(bytes > 0);
        let saved = load(&file);
        let data = &saved.condor.scenes[0].sequence_data.parallel_encoder;
        assert_eq!(data.bytes, Some(bytes));
        assert!(data.started_on.is_some() && data.completed_on.is_some());
        let events: Vec<_> = rx.try_iter().collect();
        assert!(events.iter().any(|event| matches!(event, SequenceStatus::Whole(Status::Processing { id, completion: SequenceCompletion::Frames { completed: 4, total: 4 } }) if id == "Parallel Encoder")), "owning sequence reports all four usable frames: {events:?}");
        assert!(events.iter().any(|event| matches!(event, SequenceStatus::Subprocess { child: Status::Completed { id }, .. } if id == "0")), "published scene completion: {events:?}");
        assert_eq!(
            std::fs::read(old.join("sentinel")).expect("old sentinel"),
            b"not moved"
        );
        assert!(!old.join("scenes").exists());
        assert_eq!(
            std::fs::read_dir(selected.join("scenes")).expect("owned artifacts").count(),
            1
        );

        // Probe-parent proof uses the actual TQ initializer, without running a quality probe.
        condor.encoder = Encoder::default_from_base(&EncoderBase::X264, false);
        condor.scenes[0].encoder = condor.encoder.clone();
        let mut target = TargetQualityConfig::default();
        target.probing.strategy = ProbeStrategy::Whole;
        condor.sequence_config.target_quality = Some(target);
        let (tx, _rx) = std::sync::mpsc::channel();
        TargetQuality::default()
            .initialize(&mut condor, tx)
            .expect("actual probe directory model");
        assert!(selected.join("scenes/Target Quality").is_dir());
        assert!(!old.join("scenes").exists());
    }

    #[test]
    fn stage_input_edits_on_populated_job_preserve_root_input_and_saved_scenes() {
        use andean_condor::models::input::ImportMethod;
        use california_condor::commands::handlers::encode::encode_handler;
        let temp = tempfile::tempdir().expect("fixture");
        let video = get_test_video();
        let mut configuration = default_config(
            &video,
            &temp.path().join("out.mkv"),
            &temp.path().join("work"),
        );
        configuration.condor.scenes = video.mock_scenes(&configuration.condor.encoder);
        let root = serde_json::to_value(&configuration.condor.input).expect("root");
        let scenes = serde_json::to_value(&configuration.condor.scenes).expect("scenes");
        let file = temp.path().join("condor.json");
        configuration.save(&file).expect("save");
        let replacement = temp.path().join("stage-only.mkv");
        let (after, _) = encode_handler(
            Some(&file),
            None,
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
        )
        .expect("encode --input admission");
        assert_eq!(
            serde_json::to_value(&after.condor.input).expect("after"),
            root
        );
        assert!(
            matches!(&after.condor.sequence_config.parallel_encoder.input, Some(Input::VapourSynth { path, .. }) if path == &replacement)
        );
        for command in ["encode", "target-quality", "detect-scenes", "quality-check"] {
            let property = if command == "target-quality" {
                "metric-input"
            } else {
                "input"
            };
            condor_cmd(&temp)
                .env("PATH", "")
                .args([
                    command,
                    "set",
                    property,
                    "--path",
                    path_str(&replacement),
                    "--decoder",
                    "ffms2",
                ])
                .assert()
                .success();
            let after = load(&file);
            assert_eq!(
                serde_json::to_value(&after.condor.input).expect("root"),
                root
            );
            assert_eq!(
                serde_json::to_value(&after.condor.scenes).expect("scenes"),
                scenes
            );
            assert_eq!(after.input, configuration.input);
        }
        assert!(
            matches!(load(&file).condor.sequence_config.parallel_encoder.input, Some(Input::Video { import_method: ImportMethod::FFMS2 { .. }, path, .. }) if path == replacement)
        );
        assert!(!configuration.temp.exists());
    }

    #[serial]
    #[test]
    fn encode_without_scenes_preserves_settings_and_creates_empty_scenes_directory() {
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

        // Mock an existing config file without scenes
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
            CLIParameter::new_number("--", " ", 8.0),
        );
        // config.condor.scenes =
        // test_video.mock_scenes(&config.condor.encoder);
        config.save(&config_path).expect("configuration save should succeed");

        condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .args(["encode"])
            .assert()
            .success();

        let expected_config = config;

        let (config, _) =
            load_configuration(Some(&config_path)).expect("load_configuration should succeed");

        check_basic_config(&config, &expected_config);
        assert!(
            config.condor.sequence_config.parallel_encoder.scenes_directory.exists(),
            "scenes directory exists"
        );
        let encoded_chunks =
            std::fs::read_dir(&config.condor.sequence_config.parallel_encoder.scenes_directory)
                .expect("scenes read_dir should succeed");
        assert_eq!(
            encoded_chunks.count(),
            0,
            "scenes directory contains no encoded scenes",
        );
    }

    #[serial]
    #[test]
    fn encode_without_scenes_saves_x264_options_and_workers_without_creating_chunks() {
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

        // Mock an existing config file without scenes
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
        config.save(&config_path).expect("configuration save should succeed");

        condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .args([
                "encode",
                "--encoder",
                "x264",
                "--passes",
                "2",
                "--workers",
                "2",
                "--params",
                "--preset ultrafast --crf 18",
            ])
            .assert()
            .success();

        let mut expected_config = config;
        expected_config.condor.sequence_config.parallel_encoder.workers = Some(2);
        let mut encoder_params = EncoderBase::X264.default_parameters();
        encoder_params.insert(
            "preset".to_owned(),
            CLIParameter::new_string("--", " ", "ultrafast"),
        );
        encoder_params.insert("crf".to_owned(), CLIParameter::new_number("--", " ", 18.0));
        expected_config.condor.encoder = Encoder::X264 {
            executable: None,
            pass: EncoderPasses::All(2),
            options: encoder_params,
            ffmpeg_filter: None,
            append_color_data: true,
        };
        // immutable shadow
        let expected_config = expected_config;

        let (config, _) =
            load_configuration(Some(&config_path)).expect("load_configuration should succeed");

        check_basic_config(&config, &expected_config);
        assert!(
            config.condor.sequence_config.parallel_encoder.scenes_directory.exists(),
            "scenes directory exists"
        );
        let encoded_chunks =
            std::fs::read_dir(&config.condor.sequence_config.parallel_encoder.scenes_directory)
                .expect("scenes read_dir should succeed");
        assert_eq!(
            encoded_chunks.count(),
            0,
            "scenes directory contains no encoded scenes",
        );
    }

    #[serial]
    #[test]
    fn resized_encoding_creates_scene_count_files_and_both_concatenators_create_output() {
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
        // Allow testing with SVT Essential (does not support 8-bit)
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
            CLIParameter::new_number("--", " ", 8.0),
        );
        config.condor.scenes = test_video.mock_scenes(&config.condor.encoder);
        config.save(&config_path).expect("configuration save should succeed");

        condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .args([
                "encode",
                "--workers",
                "2",
                "--filters",
                &format!("resize:scaler=bicubic;width={WIDTH};height={HEIGHT};format=yuv420p10le;"),
            ])
            .assert()
            .success();

        let mut expected_config = config;
        expected_config.condor.sequence_config.parallel_encoder.workers = Some(2);
        if let Input::VapourSynth { filters, .. } = &mut expected_config.condor.input {
            *filters = vec![VapourSynthFilter::Resize {
                scaler: Some(Scaler::Bicubic),
                width: Some(WIDTH),
                height: Some(HEIGHT),
                format: Some(FFPixelFormat::YUV420P10LE),
            }];
        }
        // immutable shadow
        let expected_config = expected_config;

        let (config, _) =
            load_configuration(Some(&config_path)).expect("load_configuration should succeed");

        check_basic_config(&config, &expected_config);
        assert!(
            config.condor.sequence_config.parallel_encoder.scenes_directory.exists(),
            "scenes directory exists"
        );
        let encoded_chunks =
            std::fs::read_dir(&config.condor.sequence_config.parallel_encoder.scenes_directory)
                .expect("scenes read_dir should succeed");
        assert_eq!(
            encoded_chunks.count(),
            config.condor.scenes.len(),
            "scenes directory contains {} encoded scenes",
            config.condor.scenes.len()
        );

        // The encoded scenes are now on disk, so both concatenation methods can
        // be exercised against them. Concatenating is far cheaper than
        // encoding, and it leaves the scene files in place, so this
        // needs no second encode.
        for label in ["mkvmerge", "ffmpeg"] {
            // Clear the previous run's output so each method starts clean. The
            // first iteration has none yet, since encoding only writes chunks.
            if output.exists() {
                std::fs::remove_file(&output).expect("concatenated output should be removable");
            }

            condor_cmd(&temp)
                .env("CONDOR_TEST_MODE", "1")
                .args(["concatenate", "--method", label])
                .assert()
                .success();

            // Switching to a method that is not the current one starts from
            // that method's defaults; the baseline config is
            // mkvmerge, so the ffmpeg run lands on default FFmpeg
            // settings.
            let method = match label {
                "mkvmerge" => ConcatMethod::MKVMerge(MkvmergeConfig::default()),
                _ => ConcatMethod::FFmpeg(FfmpegConfig::default()),
            };

            let mut expected = expected_config.clone();
            expected.condor.sequence_config.scene_concatenator.method = method;

            let (config, _) =
                load_configuration(Some(&config_path)).expect("load_configuration should succeed");

            check_basic_config(&config, &expected);
            assert!(
                config
                    .condor
                    .sequence_config
                    .scene_concatenator
                    .output
                    .unwrap_or(config.condor.output.path)
                    .exists(),
                "{label} output exists"
            );
        }
    }

    /// `--ffmpeg-filter` pipes each scene through FFmpeg before the encoder,
    /// so the encoded chunk must carry the filtered (cropped) dimensions
    /// rather than the input's.
    #[serial]
    #[test]
    fn encode_ffmpeg_crop_filter_publishes_chunk_with_cropped_dimensions() {
        if !ffmpeg_is_available() {
            return;
        }
        let ffprobe = std::process::Command::new("ffprobe").arg("-version").output();
        if ffprobe.is_err() {
            return;
        }
        const FILTER: &str = "crop=iw-16:ih-16";
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

        // One short scene keeps this encode cheap; nothing here depends on
        // scene count.
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
        config.condor.scenes = test_video.mock_scenes(&config.condor.encoder);
        config.condor.scenes.truncate(1);
        // The filter lives on the encoder, and scenes carry their own encoder
        // copies from detection, so the scene's encoder is what filters.
        config.condor.scenes[0].encoder.set_ffmpeg_filter(Some(FILTER.to_owned()));
        config.save(&config_path).expect("configuration save should succeed");

        condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .args(["encode", "--workers", "1", "--ffmpeg-filter", FILTER])
            .assert()
            .success();

        let mut expected_config = config;
        expected_config.condor.sequence_config.parallel_encoder.workers = Some(1);
        expected_config.condor.encoder.set_ffmpeg_filter(Some(FILTER.to_owned()));
        // immutable shadow
        let expected_config = expected_config;

        let (config, _) =
            load_configuration(Some(&config_path)).expect("load_configuration should succeed");

        check_basic_config(&config, &expected_config);

        let chunk =
            std::fs::read_dir(&config.condor.sequence_config.parallel_encoder.scenes_directory)
                .expect("scenes read_dir should succeed")
                .filter_map(|entry| entry.ok())
                .find(|entry| entry.path().extension().is_some_and(|ext| ext == "ivf"))
                .expect("at least one encoded scene chunk exists");

        let probe = std::process::Command::new("ffprobe")
            .args(["-v", "error", "-select_streams", "v:0"])
            .args(["-show_entries", "stream=width,height"])
            .args(["-of", "csv=p=0"])
            .arg(chunk.path())
            .output()
            .expect("ffprobe should run");
        assert!(probe.status.success(), "ffprobe should succeed");
        let probed = String::from_utf8_lossy(&probe.stdout);
        let (width, height) =
            probed.trim().split_once(',').expect("ffprobe reports width and height");
        assert_eq!(
            width.parse::<usize>().expect("width is numeric"),
            WIDTH - 16,
            "encoded chunk width is cropped"
        );
        assert_eq!(
            height.parse::<usize>().expect("height is numeric"),
            HEIGHT - 16,
            "encoded chunk height is cropped"
        );
    }

    fn check_encoder_crud(phase: u8) {
        assert!((1..=5).contains(&phase));
        if !ffmpeg_is_available() {
            return;
        }
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let config_path = init_config(&temp);

        condor_cmd(&temp).args(["encode", "set", "workers", "4"]).assert().success();
        condor_cmd(&temp).args(["encode", "set", "passes", "2"]).assert().failure();
        condor_cmd(&temp)
            .args(["encode", "set", "params", "--rc 1 --tbr 1000 --keyint 48"])
            .assert()
            .success();
        condor_cmd(&temp).args(["encode", "set", "passes", "2"]).assert().success();
        let before = std::fs::read(&config_path).expect("before invalid ISO edit");
        condor_cmd(&temp)
            .args(["encode", "set", "photon-noise", "800", "--chroma-noise", "200"])
            .assert()
            .failure();
        assert_eq!(
            std::fs::read(&config_path).expect("invalid edit unchanged"),
            before
        );
        condor_cmd(&temp)
            .args(["encode", "set", "photon-noise", "800", "--chroma-noise", "800"])
            .assert()
            .success();
        condor_cmd(&temp)
            .args(["encode", "set", "ffmpeg-filter", "crop=iw-16:ih-16"])
            .assert()
            .success();

        let config = load(&config_path);
        assert_eq!(
            config.condor.sequence_config.parallel_encoder.workers,
            Some(4),
            "workers is set"
        );
        assert_eq!(config.condor.encoder.total_passes(), 2, "passes is set");
        if phase == 2 {
            return;
        }
        assert_eq!(
            config.condor.encoder.photon_noise().map(|pn| (pn.iso, pn.chroma_iso)),
            Some((800, Some(800))),
            "photon noise is set"
        );
        if phase == 3 {
            return;
        }
        assert_eq!(
            config.condor.encoder.ffmpeg_filter(),
            Some("crop=iw-16:ih-16"),
            "ffmpeg filter is set"
        );
        if phase == 4 {
            return;
        }

        if phase != 1 {
            // Switching to x264 and attempting to set photon-noise is rejected.
            condor_cmd(&temp).args(["encode", "set", "encoder", "x264"]).assert().success();
            condor_cmd(&temp)
                .args(["encode", "set", "photon-noise", "800"])
                .assert()
                .failure();

            // Resetting encoder restores the default SVT-AV1 encoder.
            condor_cmd(&temp).args(["encode", "reset", "encoder"]).assert().success();
        }
        condor_cmd(&temp).args(["encode", "reset", "workers"]).assert().success();
        let config = load(&config_path);
        assert_eq!(
            config.condor.sequence_config.parallel_encoder.workers, None,
            "workers is cleared"
        );
    }

    #[serial]
    #[test]
    fn encode_workers_crud_saves_value_and_reset_clears_it() {
        check_encoder_crud(1);
    }

    #[serial]
    #[test]
    fn encode_multipass_requires_vbr_parameters() {
        check_encoder_crud(2);
    }

    #[serial]
    #[test]
    fn encode_photon_noise_crud_rejects_incompatible_chroma_atomically_and_saves_supported_values()
    {
        check_encoder_crud(3);
    }

    #[serial]
    #[test]
    fn encode_filter_crud_saves_filtergraph() {
        check_encoder_crud(4);
    }

    #[serial]
    #[test]
    fn encode_x264_rejects_photon_noise_and_encoder_reset_succeeds() {
        check_encoder_crud(5);
    }
}

#[cfg(test)]
mod parameters {
    use andean_condor::{
        core::input::{clip_info::ClipInfo, pixel_format::PixelFormat},
        models::{
            encoder::{Encoder, EncoderBase},
            input::{ImportMethod, Input},
        },
    };
    use california_condor::configuration::Configuration;

    use crate::common::condor_cmd;

    fn clip_info() -> ClipInfo {
        ClipInfo {
            num_frames: 1,
            format_info: PixelFormat::VapourSynth { bit_depth: 8 },
            frame_rate: av_format::rational::Rational64::new(24, 1),
            resolution: (64, 64),
            color_range: None,
            color_primaries: None,
            color_transfer_characteristic: None,
            color_space: None,
            chroma_location: None,
            mastering_display: None,
            content_light_level: None,
        }
    }

    fn check_parameter_cli(valid_edit: bool) {
        let temp = tempfile::tempdir().expect("CLI fixture");
        let script = temp.path().join("blank.vpy");
        std::fs::write(&script, "from vapoursynth import core, YUV420P8\ncore.std.BlankClip(width=64,height=64,format=YUV420P8).set_output()\n").expect("script");
        let output = temp.path().join("out.mkv");
        let mut config =
            Configuration::new(&script, &output, Some(temp.path()), None, None).expect("config");
        let source = temp.path().join("must-not-open.mkv");
        std::fs::write(&source, b"not a video").expect("inert input");
        config.condor.input = Input::Video {
            path: source,
            import_method: ImportMethod::FFMS2 {
                index: Some(0),
                cache_path: Some(temp.path().join("forbidden.ffindex")),
            },
            filters: vec![],
        };
        config.condor.encoder = Encoder::default_from_base(&EncoderBase::X265, false);
        config.condor.encoder.set_append_color_data(false);
        let path = temp.path().join("condor.json");
        config.save(&path).expect("save");
        let before = std::fs::read(&path).expect("snapshot");
        for input in [
            "--key \"unfinished",
            "--key value\\",
            "--crf 20 --crf 21",
            "--ok=1 --",
            "--ok=1 trailing",
        ] {
            for mut args in [
                vec!["encode", "--params"],
                vec!["encode", "set", "params"],
                vec!["target-quality", "set", "params"],
            ] {
                args.push(input);
                let stderr = condor_cmd(&temp)
                    .args(&args)
                    .timeout(std::time::Duration::from_secs(10))
                    .assert()
                    .failure()
                    .get_output()
                    .stderr
                    .clone();
                assert!(
                    String::from_utf8_lossy(&stderr).contains("Invalid parameter"),
                    "{args:?}: {}",
                    String::from_utf8_lossy(&stderr)
                );
                assert_eq!(
                    std::fs::read(&path).expect("unchanged config"),
                    before,
                    "{args:?}"
                );
                assert!(!temp.path().join("forbidden.ffindex").exists());
                assert!(!output.exists());
            }
        }
        if !valid_edit {
            return;
        }
        condor_cmd(&temp)
            .args(["encode", "set", "params", r#"--no-sao --crf 20 --analysis-save 'a b.dat'"#])
            .assert()
            .success();
        let loaded = Configuration::load(&path).expect("load").expect("persisted config");
        let argv = loaded
            .condor
            .encoder
            .parameters_with_stats((1, 1), &output, &clip_info(), &temp.path().join("stats"))
            .expect("persisted emitted argv");
        assert!(argv.iter().any(|arg| arg == "--no-sao"), "{argv:?}");
        assert!(
            argv.windows(2).any(|pair| pair == ["--crf", "20"]),
            "{argv:?}"
        );
        assert!(
            argv.windows(2).any(|pair| pair == ["--analysis-save", "a b.dat"]),
            "{argv:?}"
        );
        assert_eq!(
            loaded.condor.encoder.parameters().len(),
            config.condor.encoder.parameters().len() + 2
        );
        assert_eq!(
            argv.len(),
            config
                .condor
                .encoder
                .parameters_with_stats((1, 1), &output, &clip_info(), &temp.path().join("stats"))
                .expect("default argv")
                .len()
                + 3
        );
        assert!(!temp.path().join("forbidden.ffindex").exists());
        assert!(!output.exists());
    }

    #[test]
    fn encoder_parameter_cli_parse_errors_preserve_job_and_prevent_index_or_output_work() {
        check_parameter_cli(false);
    }

    #[test]
    fn encoder_parameter_crud_persists_flags_and_quoted_values_as_unsplit_argv() {
        check_parameter_cli(true);
    }
}
