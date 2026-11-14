mod retry_budget;

use std::assert_matches;

use andean_condor::models::{
    encoder::EncoderBase,
    sequence::scene_detector::{SceneDetectionMethod as CoreSCDMethod, ScenecutMethod},
};
use clap::Parser;

use super::{
    Commands, ConcatenationMethod, CondorCli, DecoderMethod, DiscardCategory, EncoderMethod,
    QualityMetric, QualityProfile, SceneDetectionMethod,
};
use crate::test_helpers::get_test_video;

mod cli_parser {
    use super::*;

    #[test]
    fn root_execution_options_conflict_with_subcommands() {
        for flags in [
            vec!["-w", "8"],
            vec!["--skip-scd"],
            vec!["-i", "source.vpy"],
            vec!["-o", "out.mkv"],
            vec!["--encoder", "aom"],
            vec!["--params", "--cpu-used=8"],
            vec!["--target", "75"],
            vec!["--target-metric", "vmaf"],
            vec!["--tq-input", "source.vpy"],
            vec!["--overwrite"],
            vec!["--disable-color-data"],
        ] {
            for command in ["encode", "detect-scenes", "scenes"] {
                let args = [vec!["condor"], flags.clone(), vec![command]].concat();
                use clap::CommandFactory;
                let mut cmd = CondorCli::command();
                let matches = cmd
                    .try_get_matches_from_mut(&args)
                    .expect("known options parse before admission");
                let error = CondorCli::admit_root_arguments(&matches, &mut cmd)
                    .expect_err("misplaced root flag");
                assert_eq!(
                    error.kind(),
                    clap::error::ErrorKind::ArgumentConflict,
                    "{args:?}: {error}"
                );
                assert!(error.to_string().contains("Usage:"), "{error}");
            }
        }
    }

    #[test]
    fn cli_default_and_command_local_run_options_pass_root_admission() {
        use clap::CommandFactory;
        for args in [
            vec![],
            vec!["encode"],
            vec!["scenes"],
            vec!["encode", "-w", "8"],
            vec!["init", "source.vpy", "out.mkv", "--params", "--preset=8"],
            vec!["init", "source.vpy", "out.mkv", "--overwrite"],
            vec!["concatenate", "--overwrite"],
        ] {
            let mut cmd = CondorCli::command();
            let matches = cmd
                .try_get_matches_from_mut([vec!["condor"], args].concat())
                .expect("valid local/default grammar");
            CondorCli::admit_root_arguments(&matches, &mut cmd)
                .expect("local/default options admitted");
        }
    }

    #[test]
    fn cli_global_options_before_or_after_run_subcommand_preserve_values() {
        use clap::{CommandFactory, FromArgMatches};
        for command in [
            "init",
            "encode",
            "detect-scenes",
            "target-quality",
            "concatenate",
            "quality-check",
            "benchmark",
        ] {
            for before in [false, true] {
                let globals = vec![
                    "--config-file",
                    "job.json",
                    "--logs",
                    "job.log",
                    "--verbose",
                    "--version",
                    "--temp",
                    "work",
                ];
                let args = if before {
                    [vec!["condor"], globals, vec![command]].concat()
                } else {
                    [vec!["condor", command], globals].concat()
                };
                let mut cmd = CondorCli::command();
                let matches = cmd
                    .try_get_matches_from_mut(args)
                    .expect("genuine globals parse on run routes");
                CondorCli::admit_root_arguments(&matches, &mut cmd)
                    .expect("genuine globals admitted");
                let cli = CondorCli::from_arg_matches(&matches).expect("CLI globals");
                assert_eq!(cli.config_file, Some("job.json".into()));
                assert_eq!(cli.logs, Some("job.log".into()));
                assert_eq!(cli.temp, Some("work".into()));
                assert!(cli.verbose);
                assert!(cli.version);
            }
        }
    }

    #[test]
    fn cli_version_before_or_after_subcommand_sets_version_flag() {
        for command in [
            "init",
            "encode",
            "detect-scenes",
            "target-quality",
            "concatenate",
            "quality-check",
            "benchmark",
        ] {
            for args in [["condor", "--version", command], ["condor", command, "--version"]] {
                assert!(CondorCli::try_parse_from(args).expect("global version").version);
            }
        }
    }

    mod init {
        use super::*;

        #[test]
        fn default() {
            let test_video = get_test_video();
            let temp = tempfile::tempdir().expect("temp directory");
            let output = temp.path().join("out.mkv");

            let result = CondorCli::try_parse_from([
                "condor",
                "init",
                test_video.path.to_str().expect("test_video path is valid"),
                output.to_str().expect("output path is valid"),
            ]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::Init { .. }),
                    ..
                }),
                "\"condor init INPUT OUTPUT\" parses"
            );
        }

        #[test]
        fn init_relative_input_and_output_paths_parse_without_filesystem_admission() {
            let result = CondorCli::try_parse_from([
                "condor",
                "init",
                "./relative-input.mov",
                "./relative-output.ivf",
            ]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::Init { .. }),
                    ..
                }),
                "\"condor init ./relative-input.mov ./relative-output.ivf\" parses"
            );
        }

        #[test]
        fn with_optional_flags() {
            let test_video = get_test_video();
            let temp = tempfile::tempdir().expect("temp directory");
            let output = temp.path().join("out.mkv");
            let config_path = temp.path().join("condor.json");
            let logs_path = temp.path().join("condor.log");

            let result = CondorCli::try_parse_from([
                "condor",
                "--config-file",
                config_path.to_str().expect("config path is valid"),
                "--temp",
                temp.path().to_str().expect("temp path is valid"),
                "--logs",
                logs_path.to_str().expect("log path is valid"),
                "init",
                test_video.path.to_str().expect("test_video path is valid"),
                output.to_str().expect("output path is valid"),
                "--encoder",
                "x264",
                "--workers",
                "4",
                "--photon-noise",
                "2400",
            ]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::Init { .. }),
                    ..
                }),
                "\"condor init INPUT OUTPUT --encoder x264 --workers 4 --photon-noise 2400\" \
                 parses"
            );
        }

        /// INPUT and OUTPUT are optional at parse time because `init` also
        /// carries `info`/`set`/`reset`; the missing one is reported by the
        /// command itself (see `handlers::init::require_paths`).
        #[test]
        fn without_output() {
            let test_video = get_test_video();

            let result = CondorCli::try_parse_from([
                "condor",
                "init",
                test_video.path.to_str().expect("test_video path is valid"),
            ]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::Init {
                        edit: None,
                        input: Some(_),
                        output: None,
                        ..
                    }),
                    ..
                }),
                "\"condor init input\" parses and leaves output unset"
            );
        }

        #[test]
        fn with_disable_color_data() {
            let test_video = get_test_video();
            let temp = tempfile::tempdir().expect("temp directory");
            let output = temp.path().join("out.mkv");

            let result = CondorCli::try_parse_from([
                "condor",
                "init",
                test_video.path.to_str().expect("test_video path is valid"),
                output.to_str().expect("output path is valid"),
                "--disable-color-data",
            ]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::Init {
                        disable_color_data: true,
                        ..
                    }),
                    ..
                }),
                "\"condor init INPUT OUTPUT --disable-color-data\" parses"
            );
        }

        #[test]
        fn init_unknown_encoder_is_rejected() {
            let result = CondorCli::try_parse_from([
                "condor",
                "init",
                "./relative-input.mov",
                "./relative-output.ivf",
                "--encoder",
                "x263",
            ]);
            assert_matches!(
                result,
                Err(_),
                "\"condor init INPUT OUTPUT --encoder x263\" does not parse"
            );
        }

        #[test]
        fn init_negative_workers_are_rejected() {
            let result = CondorCli::try_parse_from([
                "condor",
                "init",
                "C:\\absolute-input.mov",
                "C:\\output\\absolute-output.mkv",
                "--workers",
                "-2",
            ]);
            assert_matches!(
                result,
                Err(_),
                "\"condor init INPUT OUTPUT --workers -2\" does not parse"
            );
        }

        #[test]
        fn init_negative_photon_noise_is_rejected() {
            let result = CondorCli::try_parse_from([
                "condor",
                "init",
                "../relative-input.mov",
                "../relative-output.ivf",
                "--photon-noise",
                "-24.8",
            ]);
            assert_matches!(
                result,
                Err(_),
                "\"condor init INPUT OUTPUT --photon-noise -24.8\" does not parse"
            );
        }

        #[test]
        fn with_discard() {
            let test_video = get_test_video();
            let temp = tempfile::tempdir().expect("temp directory");
            let output = temp.path().join("out.mkv");

            let result = CondorCli::try_parse_from([
                "condor",
                "init",
                test_video.path.to_str().expect("test_video path is valid"),
                output.to_str().expect("output path is valid"),
                "--discard",
                "audio,subtitle",
            ]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::Init {
                        discard: Some(_),
                        ..
                    }),
                    ..
                }),
                "\"condor init INPUT OUTPUT --discard audio,subtitle\" parses"
            );
            if let Ok(CondorCli {
                command:
                    Some(Commands::Init {
                        discard: Some(categories),
                        ..
                    }),
                ..
            }) = CondorCli::try_parse_from([
                "condor",
                "init",
                test_video.path.to_str().expect("test_video path is valid"),
                output.to_str().expect("output path is valid"),
                "--discard",
                "audio,subtitle",
            ]) {
                assert_eq!(
                    categories,
                    vec![DiscardCategory::Audio, DiscardCategory::Subtitle]
                );
            }
        }

        #[test]
        fn init_unknown_discard_category_is_rejected() {
            let result = CondorCli::try_parse_from([
                "condor",
                "init",
                "./relative-input.mov",
                "./relative-output.ivf",
                "--discard",
                "fonts",
            ]);
            assert_matches!(
                result,
                Err(_),
                "\"condor init INPUT OUTPUT --discard fonts\" does not parse"
            );
        }
    }

    mod start {
        use super::*;

        #[test]
        fn default() {
            let test_video = get_test_video();

            let result = CondorCli::try_parse_from([
                "condor",
                "--input",
                test_video.path.to_str().expect("test_video path is valid"),
                "--output",
                "./out.mkv",
            ]);
            assert_matches!(
                result,
                Ok(CondorCli { command: None, .. }),
                "\"condor --input INPUT --output OUTPUT\" parses"
            );
        }

        #[test]
        fn root_output_without_input_parses_without_subcommand() {
            let result = CondorCli::try_parse_from(["condor", "--output", "./out.mkv"]);
            assert_matches!(
                result,
                Ok(CondorCli { command: None, .. }),
                "\"condor --output OUTPUT\" without --input parses"
            );
        }

        #[test]
        fn without_output() {
            let test_video = get_test_video();

            let result = CondorCli::try_parse_from([
                "condor",
                "--input",
                test_video.path.to_str().expect("test_video path exists"),
            ]);
            assert_matches!(
                result,
                Ok(CondorCli { command: None, .. }),
                "\"condor init --input INPUT\" without --output parses"
            );
        }

        #[test]
        fn cli_no_arguments_parses_root_command() {
            let result = CondorCli::try_parse_from(["condor"]);
            assert_matches!(
                result,
                Ok(CondorCli { command: None, .. }),
                "\"condor\" parses"
            );
        }

        #[test]
        fn with_ffmpeg_filter() {
            let test_video = get_test_video();

            let result = CondorCli::try_parse_from([
                "condor",
                "--input",
                test_video.path.to_str().expect("test_video path is valid"),
                "--output",
                "./out.mkv",
                "--ffmpeg-filter",
                "crop=iw-16:ih-16",
            ]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: None,
                    ffmpeg_filter: Some(_),
                    ..
                }),
                "\"condor --input INPUT --output OUTPUT --ffmpeg-filter ...\" parses"
            );
        }

        #[test]
        fn with_disable_color_data() {
            let test_video = get_test_video();

            let result = CondorCli::try_parse_from([
                "condor",
                "--input",
                test_video.path.to_str().expect("test_video path is valid"),
                "--output",
                "./out.mkv",
                "--disable-color-data",
            ]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: None,
                    disable_color_data: true,
                    ..
                }),
                "\"condor --input INPUT --output OUTPUT --disable-color-data\" parses"
            );
        }

        #[test]
        fn root_unknown_encoder_is_rejected() {
            let result = CondorCli::try_parse_from([
                "condor",
                "--input",
                "./relative-input.mp4",
                "--output",
                "/absolute/path/to/output.mkv",
                "--encoder",
                "x263",
            ]);
            assert_matches!(
                result,
                Err(_),
                "\"condor --input INPUT --output OUTPUT --encoder x263\" does not parse"
            );
        }

        #[test]
        fn root_negative_workers_are_rejected() {
            let result = CondorCli::try_parse_from([
                "condor",
                "--input",
                "../relative-input.mp4",
                "--output",
                "/absolute/path/to/output.mkv",
                "--workers",
                "-2",
            ]);
            assert_matches!(
                result,
                Err(_),
                "\"condor --input INPUT --output OUTPUT --workers -2\" does not parse"
            );
        }

        #[test]
        fn root_negative_photon_noise_is_rejected() {
            let result = CondorCli::try_parse_from([
                "condor",
                "--input",
                "./relative-input.mp4",
                "--output",
                "/absolute/path/to/output.mkv",
                "--photon-noise",
                "-24.8",
            ]);
            assert_matches!(
                result,
                Err(_),
                "\"condor --input INPUT --output OUTPUT --photon-noise -24.8\" does not parse"
            );
        }

        #[test]
        fn root_repeated_zones_preserve_inline_definition_and_file_argument() {
            let result = CondorCli::try_parse_from([
                "condor",
                "--input",
                "./relative-input.mp4",
                "--output",
                "/absolute/path/to/output.mkv",
                "--zones",
                "-1 200 svt-av1 \"--preset 3\"",
                "--zones",
                "./zones.json",
            ]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: None,
                    zones: Some(ref zones),
                    ..
                }) if zones.len() == 2 && zones[1] == "./zones.json",
                "\"condor --zones ZONE --zones FILE\" parses"
            );
        }
    }

    mod zone {
        use super::*;

        #[test]
        fn zone_dry_run_flag_parses_around_negative_bound_definitions() {
            for args in [
                vec!["condor", "zone", "--dry-run", "-1 200 svt-av1 \"--preset 3\""],
                vec!["condor", "zone", "-1 200 svt-av1 \"--preset 3\"", "--dry-run"],
                vec!["condor", "zone", "--dry-run"],
            ] {
                let mut cli = CondorCli::try_parse_from(args).expect("dry-run flag parses");
                cli.normalize_zone_flags();
                assert_matches!(cli.command, Some(Commands::Zone { dry_run: true, ref zones })
                    if zones.len() <= 1 && zones.iter().all(|zone| zone.starts_with("-1 200")));
            }
        }

        #[test]
        fn zone_without_arguments_parses_empty_zones_and_dry_run_false() {
            let result = CondorCli::try_parse_from(["condor", "zone"]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::Zone { ref zones, dry_run: false }),
                    ..
                }) if zones.is_empty(),
                "\"condor zone\" parses"
            );
        }

        #[test]
        fn zone_inline_definition_and_file_parse_in_order_with_dry_run_false() {
            let result = CondorCli::try_parse_from([
                "condor",
                "zone",
                "-1 200 svt-av1 \"--preset 3\"",
                "./zones.json",
            ]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::Zone { ref zones, dry_run: false }),
                    ..
                }) if zones.len() == 2 && zones[0].starts_with("-1 200"),
                "\"condor zone ZONE FILE\" parses"
            );
        }
    }

    mod scenes {
        use super::*;
        use crate::{
            commands::handlers::scenes::{ScenesAction, ScenesKey},
            scenes::SceneSelection,
        };

        /// The index comes first and names what the action works on;
        /// without an action the addressed scenes are printed. The shapes
        /// the index binds are pinned here so the grammar cannot drift.
        #[test]
        fn scenes_selection_before_info_action_binds_ranges_and_key() {
            let cli = CondorCli::try_parse_from([
                "condor",
                "scenes",
                "1-2,8-10,16",
                "info",
                "quality-check",
            ])
            .expect("\"condor scenes 1-2,8-10,16 info quality-check\" parses");
            assert_matches!(
                cli.command,
                Some(Commands::Scenes {
                    index: Some(SceneSelection::Ranges(ref ranges)),
                    action: Some(ScenesAction::Info {
                        key: Some(ScenesKey::QualityCheck),
                    }),
                    ..
                }) if ranges == &[(1, 2), (8, 10), (16, 16)]
            );
        }

        #[test]
        fn scenes_all_before_reset_binds_all_selection() {
            let cli = CondorCli::try_parse_from(["condor", "scenes", "all", "reset"])
                .expect("\"condor scenes all reset\" parses");
            assert_matches!(
                cli.command,
                Some(Commands::Scenes {
                    index: Some(SceneSelection::All),
                    action: Some(ScenesAction::Reset { key: None }),
                    ..
                })
            );
        }

        #[test]
        fn scenes_without_selection_or_action_parses_both_as_unset() {
            // Without an action, and without an index, the whole list is
            // what a bare `condor scenes` prints.
            let cli =
                CondorCli::try_parse_from(["condor", "scenes"]).expect("\"condor scenes\" parses");
            assert_matches!(
                cli.command,
                Some(Commands::Scenes {
                    index: None,
                    action: None,
                    ..
                })
            );
        }

        /// `add` places its own scene, so its placements are exclusive.
        #[test]
        fn scenes_add_after_binds_placement_without_selection() {
            let cli =
                CondorCli::try_parse_from(["condor", "scenes", "add", "0", "100", "--after=3"])
                    .expect("\"condor scenes add 0 100 --after=3\" parses");
            assert_matches!(
                cli.command,
                Some(Commands::Scenes {
                    index: None,
                    action: Some(ScenesAction::Add { after: Some(3), .. }),
                    ..
                })
            );
        }

        #[test]
        fn scenes_add_append_and_prepend_together_are_rejected() {
            let result = CondorCli::try_parse_from([
                "condor",
                "scenes",
                "add",
                "0",
                "100",
                "--append",
                "--prepend",
            ]);
            assert!(result.is_err(), "the placements are exclusive");
        }
    }
    mod config {
        use super::*;
        use crate::commands::{
            config::{ConfigAction, Toggle},
            handlers::{
                detect_noise::DetectNoiseSet,
                detect_scenes::{DetectScenesKey, DetectScenesSet},
                scale_noise::{ScaleNoiseKey, ScaleNoiseSet},
                scale_speed::ScaleSpeedSet,
            },
        };

        /// The `info` / `set` / `reset` triple is one generic
        /// `ConfigAction`, so a single command pins the wiring every
        /// editor shares: a keyed read, a typed write and both reset
        /// forms.
        #[test]
        fn config_info_set_and_reset_bind_typed_command_action_and_property() {
            let cli = CondorCli::try_parse_from(["condor", "detect-scenes", "info", "method"])
                .expect("\"detect-scenes info method\" parses");
            assert_matches!(
                cli.command,
                Some(Commands::DetectScenes {
                    edit: Some(ConfigAction::Info {
                        key: Some(DetectScenesKey::Method),
                    }),
                    ..
                })
            );

            let cli = CondorCli::try_parse_from([
                "condor",
                "init",
                "set",
                "input",
                "--path",
                "./alt.mkv",
                "--decoder",
                "ffms2",
            ])
            .expect("\"init set input ...\" parses");
            assert_matches!(
                cli.command,
                Some(Commands::Init {
                    edit: Some(ConfigAction::Set { .. }),
                    ..
                })
            );

            let cli = CondorCli::try_parse_from(["condor", "detect-noise", "reset"])
                .expect("\"detect-noise reset\" parses");
            assert_matches!(
                cli.command,
                Some(Commands::DetectNoise {
                    edit: Some(ConfigAction::Reset { key: None }),
                    ..
                })
            );

            let cli = CondorCli::try_parse_from(["condor", "scale-noise", "reset", "threshold"])
                .expect("\"scale-noise reset threshold\" parses");
            assert_matches!(
                cli.command,
                Some(Commands::ScaleNoise {
                    edit: Some(ConfigAction::Reset {
                        key: Some(ScaleNoiseKey::Threshold),
                    }),
                    ..
                })
            );
        }

        #[test]
        fn config_reset_section_key_is_rejected() {
            // The redundant `section` key is gone; a bare `reset`
            // replaces it.
            assert!(
                CondorCli::try_parse_from(["condor", "scale-noise", "reset", "section"]).is_err(),
                "\"reset section\" is no longer a property"
            );
        }

        /// Values that are never optional are positionals, and the value
        /// tables the parser owns — the switch spellings, the alias names —
        /// are pinned here rather than end to end.
        #[test]
        fn scale_speed_set_pairs_parse_quantizer_and_speed_values() {
            let cli = CondorCli::try_parse_from([
                "condor",
                "scale-speed",
                "set",
                "speed-scalers",
                "20=6",
                "35=4",
            ])
            .expect("\"scale-speed set speed-scalers 20=6 35=4\" parses");
            assert_matches!(
                cli.command,
                Some(Commands::ScaleSpeed {
                    edit: Some(ConfigAction::Set {
                        setting: ScaleSpeedSet::SpeedQuantizers {
                            points,
                        },
                    }),
                    ..
                }) if points == vec![(20, 6), (35, 4)]
            );
        }

        #[test]
        fn detect_noise_set_reference_filters_keeps_positional_filter() {
            let cli = CondorCli::try_parse_from([
                "condor",
                "detect-noise",
                "set",
                "reference-filters",
                "resize:width=960;",
            ])
            .expect("\"detect-noise set reference-filters resize:...\" parses");
            assert_matches!(
                cli.command,
                Some(Commands::DetectNoise {
                    edit: Some(ConfigAction::Set {
                        setting: DetectNoiseSet::ReferenceFilters {
                            ref filters,
                        },
                    }),
                    ..
                }) if filters == &["resize:width=960;".to_owned()]
            );
        }

        #[test]
        fn config_toggle_aliases_parse_on_and_off() {
            // One flag turns a setting both ways: every spelling of the
            // on/off value lands on the switch, and a following flag is
            // never swallowed as its value.
            for (value, expected) in [
                ("on", Toggle::On),
                ("enable", Toggle::On),
                ("enabled", Toggle::On),
                ("yes", Toggle::On),
                ("true", Toggle::On),
                ("1", Toggle::On),
                ("off", Toggle::Off),
                ("disable", Toggle::Off),
                ("disabled", Toggle::Off),
                ("no", Toggle::Off),
                ("false", Toggle::Off),
                ("0", Toggle::Off),
            ] {
                let cli = CondorCli::try_parse_from([
                    "condor",
                    "detect-scenes",
                    "set",
                    "method",
                    "avscenechange",
                    "--save-scores",
                    value,
                ])
                .unwrap_or_else(|err| panic!("--save-scores {value} parses: {err}"));
                assert_matches!(
                    cli.command,
                    Some(Commands::DetectScenes {
                        edit: Some(ConfigAction::Set {
                            setting: DetectScenesSet::Method {
                                save_scores, ..
                            },
                        }),
                        ..
                    }) if save_scores == Some(expected)
                );
            }
        }

        #[test]
        fn config_optional_toggle_does_not_consume_following_flag() {
            let cli = CondorCli::try_parse_from([
                "condor",
                "detect-scenes",
                "set",
                "method",
                "avscenechange",
                "--save-scores",
                "--minimum-frames",
                "20",
            ])
            .expect("\"--save-scores --minimum-frames 20\" parses");
            assert_matches!(
                cli.command,
                Some(Commands::DetectScenes {
                    edit: Some(ConfigAction::Set {
                        setting: DetectScenesSet::Method {
                            save_scores: Some(Toggle::On),
                            minimum_frames: Some(20),
                            ..
                        },
                    }),
                    ..
                })
            );
        }

        #[test]
        fn config_omitted_scale_chroma_toggle_leaves_optional_value_unset() {
            // The switch value is optional and means on when omitted.
            let cli = CondorCli::try_parse_from(["condor", "scale-noise", "set", "scale-chroma"])
                .expect("\"scale-noise set scale-chroma\" parses");
            assert_matches!(
                cli.command,
                Some(Commands::ScaleNoise {
                    edit: Some(ConfigAction::Set {
                        setting: ScaleNoiseSet::ScaleChroma { value: None },
                    }),
                    ..
                })
            );
        }

        /// Unknown properties and kinds are parse errors, and run flags
        /// never mix with an editing subcommand.
        #[test]
        fn config_unknown_properties_invalid_values_and_removed_settings_are_rejected() {
            for args in [
                vec!["condor", "detect-scenes", "set", "nonsense"],
                vec!["condor", "detect-scenes", "set", "method", "not-an-algorithm"],
                vec!["condor", "quality-check", "set", "strategy", "bogus"],
                vec!["condor", "target-quality", "set", "metric", "bogus"],
                vec!["condor", "concatenate", "set", "method", "bogus"],
                vec!["condor", "scale-speed", "set", "speed-quantizers", "not-a-pair", "x=1"],
                vec!["condor", "target-quality", "set", "quantizer-range", "30-10"],
                vec!["condor", "target-quality", "set", "interpolators", "linear"],
                vec!["condor", "target-quality", "set", "plot"],
            ] {
                assert!(
                    CondorCli::try_parse_from(&args).is_err(),
                    "{args:?} is rejected"
                );
            }
        }

        #[test]
        fn detect_scenes_run_flags_parse_without_edit_action() {
            // Run flags still parse on their own, but conflict with a
            // configuration subcommand.
            let cli =
                CondorCli::try_parse_from(["condor", "detect-scenes", "--min-scene-seconds", "3"])
                    .expect("run flags still parse");
            assert_matches!(
                cli.command,
                Some(Commands::DetectScenes {
                    edit: None,
                    min_scene_seconds: Some(3),
                    ..
                })
            );
        }

        #[test]
        fn detect_scenes_run_flags_conflict_with_info_action() {
            assert!(
                CondorCli::try_parse_from([
                    "condor",
                    "detect-scenes",
                    "--min-scene-seconds",
                    "3",
                    "info",
                ])
                .is_err(),
                "flags and subcommands conflict"
            );
        }
    }
    mod detect_scenes {
        use super::*;

        #[test]
        fn detect_scenes_without_options_parses_detect_scenes_command() {
            let result = CondorCli::try_parse_from(["condor", "detect-scenes"]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::DetectScenes { .. }),
                    ..
                }),
                "\"condor detect-scenes\" parses"
            );
        }

        #[test]
        fn detect_scenes_input_flag_parses_as_present() {
            let result =
                CondorCli::try_parse_from(["condor", "detect-scenes", "--input", "./input.mp4"]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::DetectScenes { input: Some(_), .. }),
                    ..
                }),
                "\"condor detect-scenes --input INPUT\" parses"
            );
        }

        #[test]
        fn detect_scenes_fast_method_and_scene_duration_bounds_parse_expected_values() {
            let result = CondorCli::try_parse_from([
                "condor",
                "detect-scenes",
                "--method",
                "fast",
                "--min-scene-seconds",
                "2",
                "--max-scene-seconds",
                "5",
                "--config-file",
                "./condor.json",
                "--temp",
                "./work",
                "--logs",
                "./condor.log",
            ]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::DetectScenes {
                        method: Some(SceneDetectionMethod::Fast),
                        min_scene_seconds: Some(2),
                        max_scene_seconds: Some(5),
                        ..
                    }),
                    ..
                }),
                "\"condor detect-scenes --method fast --min-scene-seconds 2 \
                 --max-scene-seconds 5\" parses"
            );
        }

        #[test]
        fn detect_scenes_nonnumeric_minimum_duration_is_rejected() {
            let result =
                CondorCli::try_parse_from(["condor", "detect-scenes", "--min-scene-seconds", "pi"]);
            assert_matches!(
                result,
                Err(_),
                "\"condor detect-scenes --min-scene-seconds pi\" does not parse"
            );
        }

        #[test]
        fn detect_scenes_negative_maximum_duration_is_rejected() {
            let result = CondorCli::try_parse_from([
                "condor",
                "detect-scenes",
                "--max-scene-seconds",
                "-12",
            ]);
            assert_matches!(
                result,
                Err(_),
                "\"condor detect-scenes --max-scene-seconds -12\" does not parse"
            );
        }

        #[test]
        fn detect_scenes_unknown_method_is_rejected() {
            let result = CondorCli::try_parse_from([
                "condor",
                "detect-scenes",
                "--method",
                "thefastestoneyoucangivemeplease",
            ]);
            assert_matches!(
                result,
                Err(_),
                "\"condor --input INPUT --output OUTPUT --method \
                 thefastestoneyoucangivemeplease\" does not parse"
            );
        }
    }

    mod detect_noise {
        use super::*;

        #[test]
        fn detect_noise_without_options_parses_detect_noise_command() {
            let result = CondorCli::try_parse_from(["condor", "detect-noise"]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::DetectNoise { .. }),
                    ..
                }),
                "\"condor detect-noise\" parses"
            );
        }

        #[test]
        fn detect_noise_input_flag_parses_as_present() {
            let result =
                CondorCli::try_parse_from(["condor", "detect-noise", "--input", "./input.mp4"]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::DetectNoise { input: Some(_), .. }),
                    ..
                }),
                "\"condor detect-noise --input INPUT\" parses"
            );
        }

        #[test]
        fn detect_noise_repeated_vs_args_parse_two_values_with_script_input() {
            let result = CondorCli::try_parse_from([
                "condor",
                "detect-noise",
                "--input",
                "./script.vpy",
                "--vs-args",
                "preset=slow",
                "--vs-args",
                "threads=4",
            ]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command:
                        Some(Commands::DetectNoise {
                            input: Some(_),
                            vs_args: Some(ref args),
                            ..
                        }),
                    ..
                }) if args.len() == 2,
                "\"condor detect-noise --input ./script.vpy --vs_args \"preset=slow\" --vs-args \"threads=4\" parses"
            );
        }
    }

    mod scale_noise {
        use super::*;

        #[test]
        fn scale_noise_without_options_parses_scale_noise_command() {
            let result = CondorCli::try_parse_from(["condor", "scale-noise"]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::ScaleNoise { .. }),
                    ..
                }),
                "\"condor scale-noise\" parses"
            );
        }

        #[test]
        fn scale_noise_threshold_parses_expected_fraction() {
            let result =
                CondorCli::try_parse_from(["condor", "scale-noise", "--threshold", "0.002"]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::ScaleNoise {
                        threshold: Some(0.002),
                        ..
                    }),
                    ..
                }),
                "\"condor scale-noise --threshold 0.002\" parses"
            );
        }

        #[test]
        fn scale_noise_minimum_and_maximum_scalers_parse_expected_values() {
            let result = CondorCli::try_parse_from([
                "condor",
                "scale-noise",
                "--minimum-scaler",
                "0.5",
                "--maximum-scaler",
                "2.0",
            ]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::ScaleNoise {
                        minimum_scaler: Some(0.5),
                        maximum_scaler: Some(2.0),
                        ..
                    }),
                    ..
                }),
                "\"condor scale-noise --minimum-scaler 0.5 --maximum-scaler 2.0\" parses"
            );
        }

        #[test]
        fn scale_noise_scale_chroma_flag_parses_as_true() {
            let result = CondorCli::try_parse_from(["condor", "scale-noise", "--scale-chroma"]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::ScaleNoise {
                        scale_chroma: true,
                        ..
                    }),
                    ..
                }),
                "\"condor scale-noise --scale-chroma\" parses"
            );
        }

        #[test]
        fn scale_noise_combined_threshold_bounds_and_chroma_flag_preserve_all_values() {
            let result = CondorCli::try_parse_from([
                "condor",
                "scale-noise",
                "--threshold",
                "0.005",
                "--minimum-scaler",
                "0.3",
                "--maximum-scaler",
                "1.5",
                "--scale-chroma",
            ]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::ScaleNoise {
                        threshold: Some(0.005),
                        minimum_scaler: Some(0.3),
                        maximum_scaler: Some(1.5),
                        scale_chroma: true,
                        ..
                    }),
                    ..
                }),
                "\"condor scale-noise\" with all options parses"
            );
        }
    }

    mod benchmark {
        use super::*;

        #[test]
        fn benchmark_without_options_parses_benchmark_command() {
            let result = CondorCli::try_parse_from(["condor", "benchmark"]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::Benchmark { .. }),
                    ..
                }),
                "\"condor benchmark\" parses"
            );
        }

        #[test]
        fn benchmark_threshold_parses_expected_integer() {
            let result = CondorCli::try_parse_from(["condor", "benchmark", "--threshold", "10"]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::Benchmark {
                        threshold: Some(10),
                        ..
                    }),
                    ..
                }),
                "\"condor benchmark --threshold 10\" parses"
            );
        }

        #[test]
        fn benchmark_max_memory_parses_expected_limit() {
            let result = CondorCli::try_parse_from(["condor", "benchmark", "--max-memory", "4096"]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::Benchmark {
                        max_memory: Some(4096),
                        ..
                    }),
                    ..
                }),
                "\"condor benchmark --max-memory 4096\" parses"
            );
        }

        #[test]
        fn benchmark_threshold_above_one_hundred_is_rejected() {
            let result = CondorCli::try_parse_from(["condor", "benchmark", "--threshold", "101"]);
            assert_matches!(
                result,
                Err(_),
                "\"condor benchmark --threshold 101\" does not parse (range 0..=100)"
            );
        }

        #[test]
        fn benchmark_negative_threshold_is_rejected() {
            let result = CondorCli::try_parse_from(["condor", "benchmark", "--threshold", "-5"]);
            assert_matches!(
                result,
                Err(_),
                "\"condor benchmark --threshold -5\" does not parse"
            );
        }
    }

    mod target_quality {
        use super::*;

        #[test]
        fn target_quality_without_options_parses_target_quality_command() {
            let result = CondorCli::try_parse_from(["condor", "target-quality"]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::TargetQuality { .. }),
                    ..
                }),
                "\"condor target-quality\" parses"
            );
        }

        #[test]
        fn target_quality_input_flag_parses_as_present() {
            let result =
                CondorCli::try_parse_from(["condor", "target-quality", "--input", "./input.mp4"]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::TargetQuality { input: Some(_), .. }),
                    ..
                }),
                "\"condor target-quality --input INPUT\" parses"
            );
        }

        #[test]
        fn target_quality_ssimulacra2_metric_and_numeric_target_parse_expected_values() {
            let result = CondorCli::try_parse_from([
                "condor",
                "target-quality",
                "--metric",
                "ssimulacra2",
                "--target",
                "90",
            ]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::TargetQuality {
                        metric: Some(QualityMetric::SSIMULACRA2),
                        target: Some(90.0),
                        ..
                    }),
                    ..
                }),
                "\"condor target-quality --metric ssimulacra2 --target 90\" parses"
            );
        }

        #[test]
        fn target_quality_slow_profile_parses_slow_variant() {
            let result =
                CondorCli::try_parse_from(["condor", "target-quality", "--profile", "slow"]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::TargetQuality {
                        profile: Some(QualityProfile::Slow),
                        ..
                    }),
                    ..
                }),
                "\"condor target-quality --profile slow\" parses"
            );
        }

        #[test]
        fn target_quality_each_metric_name_parses_corresponding_variant() {
            for (name, expected) in [
                ("vmaf", QualityMetric::VMAF),
                ("ssimulacra2", QualityMetric::SSIMULACRA2),
                ("butteraugli", QualityMetric::BUTTERAUGLI),
                ("butteraugli-3", QualityMetric::BUTTERAUGLI3Norm),
                ("xpsnr", QualityMetric::XPSNR),
                ("cvvdp", QualityMetric::CVVDP),
            ] {
                let result = CondorCli::try_parse_from([
                    "condor",
                    "target-quality",
                    "--metric",
                    name,
                    "--target",
                    "80",
                ]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::TargetQuality {
                            metric: Some(_),
                            ..
                        }),
                        ..
                    }),
                    "\"condor target-quality --metric {name}\" parses"
                );
                if let Ok(CondorCli {
                    command:
                        Some(Commands::TargetQuality {
                            metric: Some(ref m),
                            ..
                        }),
                    ..
                }) = result
                {
                    assert_eq!(m, &expected, "metric variant matches for {name}");
                }
            }
        }

        #[test]
        fn target_quality_each_profile_name_parses_corresponding_variant() {
            for (name, expected) in [
                ("fast", QualityProfile::Fast),
                ("standard", QualityProfile::Standard),
                ("slow", QualityProfile::Slow),
            ] {
                let result =
                    CondorCli::try_parse_from(["condor", "target-quality", "--profile", name]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::TargetQuality {
                            profile: Some(_),
                            ..
                        }),
                        ..
                    }),
                    "\"condor target-quality --profile {name}\" parses"
                );
                if let Ok(CondorCli {
                    command:
                        Some(Commands::TargetQuality {
                            profile: Some(ref p),
                            ..
                        }),
                    ..
                }) = result
                {
                    assert_eq!(p, &expected, "profile variant matches for {name}");
                }
            }
        }

        #[test]
        fn target_quality_min_q_and_max_q_parse_expected_bounds() {
            let result = CondorCli::try_parse_from([
                "condor",
                "target-quality",
                "--min-q",
                "10",
                "--max-q",
                "50",
            ]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::TargetQuality {
                        minimum_quantizer: Some(10),
                        maximum_quantizer: Some(50),
                        ..
                    }),
                    ..
                }),
                "\"condor target-quality --min-q 10 --max-q 50\" parses"
            );
        }

        #[test]
        fn target_quality_parameter_string_parses_as_present() {
            let result = CondorCli::try_parse_from([
                "condor",
                "target-quality",
                "--params",
                "--preset 2 --crf 24",
            ]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::TargetQuality {
                        params: Some(_),
                        ..
                    }),
                    ..
                }),
                "\"condor target-quality --params ...\" parses"
            );
        }

        #[test]
        fn target_quality_unknown_metric_is_rejected() {
            let result =
                CondorCli::try_parse_from(["condor", "target-quality", "--metric", "nonexistent"]);
            assert_matches!(
                result,
                Err(_),
                "\"condor target-quality --metric nonexistent\" does not parse"
            );
        }

        #[test]
        fn target_quality_unknown_profile_is_rejected() {
            let result =
                CondorCli::try_parse_from(["condor", "target-quality", "--profile", "nonexistent"]);
            assert_matches!(
                result,
                Err(_),
                "\"condor target-quality --profile nonexistent\" does not parse"
            );
        }
    }

    mod quality_check {
        use super::*;

        #[test]
        fn quality_check_without_options_parses_quality_check_command() {
            let result = CondorCli::try_parse_from(["condor", "quality-check"]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::QualityCheck { .. }),
                    ..
                }),
                "\"condor quality-check\" parses"
            );
        }

        #[test]
        fn quality_check_ssimulacra2_metric_parses_corresponding_variant() {
            let result =
                CondorCli::try_parse_from(["condor", "quality-check", "--metric", "ssimulacra2"]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::QualityCheck {
                        metric: Some(QualityMetric::SSIMULACRA2),
                        ..
                    }),
                    ..
                }),
                "\"condor quality-check --metric ssimulacra2\" parses"
            );
        }

        #[test]
        fn quality_check_fast_profile_parses_fast_variant() {
            let result =
                CondorCli::try_parse_from(["condor", "quality-check", "--profile", "fast"]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::QualityCheck {
                        profile: Some(QualityProfile::Fast),
                        ..
                    }),
                    ..
                }),
                "\"condor quality-check --profile fast\" parses"
            );
        }

        #[test]
        fn quality_check_each_metric_name_parses_corresponding_variant() {
            for (name, expected) in [
                ("vmaf", QualityMetric::VMAF),
                ("ssimulacra2", QualityMetric::SSIMULACRA2),
                ("butteraugli", QualityMetric::BUTTERAUGLI),
                ("butteraugli-3", QualityMetric::BUTTERAUGLI3Norm),
                ("xpsnr", QualityMetric::XPSNR),
                ("cvvdp", QualityMetric::CVVDP),
            ] {
                let result =
                    CondorCli::try_parse_from(["condor", "quality-check", "--metric", name]);
                assert_matches!(
                    result,
                    Ok(CondorCli {
                        command: Some(Commands::QualityCheck {
                            metric: Some(_),
                            ..
                        }),
                        ..
                    }),
                    "\"condor quality-check --metric {name}\" parses"
                );
                if let Ok(CondorCli {
                    command:
                        Some(Commands::QualityCheck {
                            metric: Some(ref m),
                            ..
                        }),
                    ..
                }) = result
                {
                    assert_eq!(m, &expected, "metric variant matches for {name}");
                }
            }
        }

        #[test]
        fn quality_check_unknown_metric_is_rejected() {
            let result =
                CondorCli::try_parse_from(["condor", "quality-check", "--metric", "nonexistent"]);
            assert_matches!(
                result,
                Err(_),
                "\"condor quality-check --metric nonexistent\" does not parse"
            );
        }
    }

    mod optimize_bitrate {
        use super::*;

        #[test]
        fn optimize_bitrate_without_options_parses_optimize_bitrate_command() {
            let result = CondorCli::try_parse_from(["condor", "optimize-bitrate"]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::OptimizeBitrate { .. }),
                    ..
                }),
                "\"condor optimize-bitrate\" parses"
            );
        }

        #[test]
        fn optimize_bitrate_sigma_threshold_parses_expected_integer() {
            let result =
                CondorCli::try_parse_from(["condor", "optimize-bitrate", "--sigma-threshold", "3"]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::OptimizeBitrate {
                        sigma_threshold: Some(3),
                        ..
                    }),
                    ..
                }),
                "\"condor optimize-bitrate --sigma-threshold 3\" parses"
            );
        }

        #[test]
        fn optimize_bitrate_sigma_below_one_is_rejected() {
            let result =
                CondorCli::try_parse_from(["condor", "optimize-bitrate", "--sigma-threshold", "0"]);
            assert_matches!(
                result,
                Err(_),
                "\"condor optimize-bitrate --sigma-threshold 0\" does not parse (range 1..=10)"
            );
        }

        #[test]
        fn optimize_bitrate_sigma_above_ten_is_rejected() {
            let result = CondorCli::try_parse_from([
                "condor",
                "optimize-bitrate",
                "--sigma-threshold",
                "11",
            ]);
            assert_matches!(
                result,
                Err(_),
                "\"condor optimize-bitrate --sigma-threshold 11\" does not parse (range \
                 1..=10)"
            );
        }
    }

    mod scale_speed {
        use super::*;

        #[test]
        fn scale_speed_one_quantizer_speed_pair_parses() {
            let result = CondorCli::try_parse_from([
                "condor",
                "scale-speed",
                "--quantizers",
                "20",
                "--speeds",
                "5",
            ]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::ScaleSpeed { .. }),
                    ..
                }),
                "\"condor scale-speed --quantizers 20 --speeds 5\" parses"
            );
        }

        #[test]
        fn scale_speed_repeated_quantizer_and_speed_flags_preserve_three_ordered_pairs() {
            let result = CondorCli::try_parse_from([
                "condor",
                "scale-speed",
                "--quantizers",
                "20",
                "--speeds",
                "5",
                "--quantizers",
                "30",
                "--speeds",
                "4",
                "--quantizers",
                "55",
                "--speeds",
                "2",
            ]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::ScaleSpeed {
                        quantizers: Some(_),
                        speeds: Some(_),
                        ..
                    }),
                    ..
                }),
                "\"condor scale-speed\" with three pairs parses"
            );
            if let Ok(CondorCli {
                command:
                    Some(Commands::ScaleSpeed {
                        quantizers: Some(ref q),
                        speeds: Some(ref s),
                        ..
                    }),
                ..
            }) = result
            {
                assert_eq!(q.len(), 3, "three quantizer values");
                assert_eq!(s.len(), 3, "three speed values");
                assert_eq!(q[0], 20);
                assert_eq!(s[0], 5);
                assert_eq!(q[1], 30);
                assert_eq!(s[1], 4);
                assert_eq!(q[2], 55);
                assert_eq!(s[2], 2);
            }
        }

        #[test]
        fn scale_speed_quantizers_without_speeds_are_rejected() {
            let result = CondorCli::try_parse_from(["condor", "scale-speed", "--quantizers", "20"]);
            assert_matches!(
                result,
                Err(_),
                "\"condor scale-speed --quantizers 20\" without --speeds fails"
            );
        }

        #[test]
        fn scale_speed_speeds_without_quantizers_are_rejected() {
            let result = CondorCli::try_parse_from(["condor", "scale-speed", "--speeds", "5"]);
            assert_matches!(
                result,
                Err(_),
                "\"condor scale-speed --speeds 5\" without --quantizers fails"
            );
        }

        #[test]
        fn scale_speed_negative_quantizer_argument_is_rejected() {
            let result = CondorCli::try_parse_from([
                "condor",
                "scale-speed",
                "--quantizers",
                "-10",
                "--speeds",
                "7",
            ]);
            assert_matches!(
                result,
                Err(_),
                "\"condor scale-speed --quantizers -10 ...\" fails (no allow_hyphen_values)"
            );
        }
    }

    mod encode {
        use super::*;

        #[test]
        fn encode_without_options_parses_encode_command() {
            let result = CondorCli::try_parse_from(["condor", "encode"]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::Encode { .. }),
                    ..
                }),
                "\"condor encode\" parses"
            );
        }

        #[test]
        fn encode_input_flag_parses_as_present() {
            let result = CondorCli::try_parse_from(["condor", "encode", "--input", "./input.mp4"]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::Encode { input: Some(_), .. }),
                    ..
                }),
                "\"condor encode --input INPUT\" parses"
            );
        }

        #[test]
        fn encode_x264_and_worker_count_parse_expected_values() {
            let result = CondorCli::try_parse_from([
                "condor",
                "encode",
                "--encoder",
                "x264",
                "--workers",
                "4",
            ]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::Encode {
                        encoder: Some(EncoderMethod::X264),
                        workers: Some(4),
                        ..
                    }),
                    ..
                }),
                "\"condor encode --encoder x264 --workers 4\" parses"
            );
        }

        #[test]
        fn encode_pass_count_parses_expected_integer() {
            let result = CondorCli::try_parse_from(["condor", "encode", "--passes", "2"]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::Encode {
                        passes: Some(2),
                        ..
                    }),
                    ..
                }),
                "\"condor encode --passes 2\" parses"
            );
        }

        #[test]
        fn encode_parameter_string_parses_as_present() {
            let result =
                CondorCli::try_parse_from(["condor", "encode", "--params", "--preset 8 --crf 30"]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::Encode {
                        params: Some(_),
                        ..
                    }),
                    ..
                }),
                "\"condor encode --params ...\" parses"
            );
        }

        #[test]
        fn encode_luma_and_chroma_photon_noise_parse_expected_iso_values() {
            let result = CondorCli::try_parse_from([
                "condor",
                "encode",
                "--photon-noise",
                "2400",
                "--chroma-noise",
                "1200",
            ]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::Encode {
                        photon_noise: Some(2400),
                        chroma_noise: Some(1200),
                        ..
                    }),
                    ..
                }),
                "\"condor encode --photon-noise 2400 --chroma-noise 1200\" parses"
            );
        }

        #[test]
        fn encode_bestsource_decoder_and_filter_string_parse_as_requested() {
            let result = CondorCli::try_parse_from([
                "condor",
                "encode",
                "--decoder",
                "bestsource",
                "--filters",
                "resize:scaler=bilinear;width=1280;height=720;format=yuv420p;",
            ]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::Encode {
                        decoder: Some(DecoderMethod::BestSource),
                        filters: Some(_),
                        ..
                    }),
                    ..
                }),
                "\"condor encode --decoder bestsource --filters ...\" parses"
            );
        }

        #[test]
        fn encode_vs_args_flag_parses_as_present() {
            let result = CondorCli::try_parse_from(["condor", "encode", "--vs-args", "threads=8"]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::Encode {
                        vs_args: Some(_),
                        ..
                    }),
                    ..
                }),
                "\"condor encode --vs-args threads=8\" parses"
            );
        }

        #[test]
        fn encode_ffmpeg_filter_parses_as_present() {
            let result = CondorCli::try_parse_from([
                "condor",
                "encode",
                "--ffmpeg-filter",
                "crop=iw-16:ih-16",
            ]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::Encode {
                        ffmpeg_filter: Some(_),
                        ..
                    }),
                    ..
                }),
                "\"condor encode --ffmpeg-filter ...\" parses"
            );
        }

        #[test]
        fn encode_unknown_encoder_is_rejected() {
            let result = CondorCli::try_parse_from(["condor", "encode", "--encoder", "x263"]);
            assert_matches!(
                result,
                Err(_),
                "\"condor encode --encoder x263\" does not parse"
            );
        }

        #[test]
        fn encode_negative_worker_count_is_rejected() {
            let result = CondorCli::try_parse_from(["condor", "encode", "--workers", "-1"]);
            assert_matches!(
                result,
                Err(_),
                "\"condor encode --workers -1\" does not parse"
            );
        }
    }

    mod concatenate {
        use super::*;

        #[test]
        fn concatenate_without_options_parses_concatenate_command() {
            let result = CondorCli::try_parse_from(["condor", "concatenate"]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::Concatenate { .. }),
                    ..
                }),
                "\"condor concatenate\" parses"
            );
        }

        #[test]
        fn concatenate_mkvmerge_method_parses_corresponding_variant() {
            let result =
                CondorCli::try_parse_from(["condor", "concatenate", "--method", "mkvmerge"]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::Concatenate {
                        method: Some(ConcatenationMethod::MkvMerge),
                        ..
                    }),
                    ..
                }),
                "\"condor concatenate --method mkvmerge\" parses"
            );
        }

        #[test]
        fn concatenate_ffmpeg_method_parses_corresponding_variant() {
            let result = CondorCli::try_parse_from(["condor", "concatenate", "--method", "ffmpeg"]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::Concatenate {
                        method: Some(ConcatenationMethod::FFmpeg),
                        ..
                    }),
                    ..
                }),
                "\"condor concatenate --method ffmpeg\" parses"
            );
        }

        #[test]
        fn concatenate_ivf_method_parses_corresponding_variant() {
            let result = CondorCli::try_parse_from(["condor", "concatenate", "--method", "ivf"]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::Concatenate {
                        method: Some(ConcatenationMethod::Ivf),
                        ..
                    }),
                    ..
                }),
                "\"condor concatenate --method ivf\" parses"
            );
        }

        #[test]
        fn concatenate_unknown_method_is_rejected() {
            let result =
                CondorCli::try_parse_from(["condor", "concatenate", "--method", "invalid"]);
            assert_matches!(
                result,
                Err(_),
                "\"condor concatenate --method invalid\" does not parse"
            );
        }

        #[test]
        fn concatenate_audio_discard_parses_audio_category() {
            let result = CondorCli::try_parse_from(["condor", "concatenate", "--discard", "audio"]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::Concatenate {
                        discard: Some(_),
                        ..
                    }),
                    ..
                }),
                "\"condor concatenate --discard audio\" parses"
            );
            if let Ok(CondorCli {
                command:
                    Some(Commands::Concatenate {
                        discard: Some(categories),
                        ..
                    }),
                ..
            }) = CondorCli::try_parse_from(["condor", "concatenate", "--discard", "audio"])
            {
                assert_eq!(categories, vec![DiscardCategory::Audio]);
            }
        }

        #[test]
        fn concatenate_repeated_discard_flags_preserve_audio_and_subtitle_order() {
            let result = CondorCli::try_parse_from([
                "condor",
                "concatenate",
                "--discard",
                "audio",
                "--discard",
                "subtitle",
            ]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::Concatenate {
                        discard: Some(_),
                        ..
                    }),
                    ..
                }),
                "\"condor concatenate --discard audio --discard subtitle\" parses"
            );
            if let Ok(CondorCli {
                command:
                    Some(Commands::Concatenate {
                        discard: Some(categories),
                        ..
                    }),
                ..
            }) = CondorCli::try_parse_from([
                "condor",
                "concatenate",
                "--discard",
                "audio",
                "--discard",
                "subtitle",
            ]) {
                assert_eq!(
                    categories,
                    vec![DiscardCategory::Audio, DiscardCategory::Subtitle]
                );
            }
        }

        #[test]
        fn concatenate_comma_delimited_discard_preserves_audio_subtitle_and_chapters_order() {
            let result = CondorCli::try_parse_from([
                "condor",
                "concatenate",
                "--discard",
                "audio,subtitle,chapters",
            ]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::Concatenate {
                        discard: Some(_),
                        ..
                    }),
                    ..
                }),
                "\"condor concatenate --discard audio,subtitle,chapters\" parses"
            );
            if let Ok(CondorCli {
                command:
                    Some(Commands::Concatenate {
                        discard: Some(categories),
                        ..
                    }),
                ..
            }) = CondorCli::try_parse_from([
                "condor",
                "concatenate",
                "--discard",
                "audio,subtitle,chapters",
            ]) {
                assert_eq!(
                    categories,
                    vec![
                        DiscardCategory::Audio,
                        DiscardCategory::Subtitle,
                        DiscardCategory::Chapters
                    ]
                );
            }
        }

        #[test]
        fn concatenate_unknown_discard_category_is_rejected() {
            let result = CondorCli::try_parse_from(["condor", "concatenate", "--discard", "font"]);
            assert_matches!(
                result,
                Err(_),
                "\"condor concatenate --discard font\" does not parse"
            );
        }

        #[test]
        fn concatenate_without_discard_parses_with_no_discard_categories() {
            let result = CondorCli::try_parse_from(["condor", "concatenate"]);
            let CondorCli {
                command: Some(Commands::Concatenate { discard, .. }),
                ..
            } = result.expect("concatenate defaults parse")
            else {
                panic!("expected concatenate command");
            };
            assert!(discard.is_none(), "--discard defaults to unset");
        }
    }

    mod clean {
        use super::*;

        #[test]
        fn clean_without_options_parses_clean_command() {
            let result = CondorCli::try_parse_from(["condor", "clean"]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::Clean { .. }),
                    ..
                }),
                "\"condor clean\" parses"
            );
        }

        #[test]
        fn clean_all_flag_parses_as_true() {
            let result = CondorCli::try_parse_from(["condor", "clean", "--all"]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::Clean { all: true, .. }),
                    ..
                }),
                "\"condor clean --all\" parses"
            );
        }

        #[test]
        fn clean_without_all_parses_with_all_false() {
            let result = CondorCli::try_parse_from(["condor", "clean"]);
            let CondorCli {
                command: Some(Commands::Clean { all, .. }),
                ..
            } = result.expect("clean defaults parse")
            else {
                panic!("expected clean command");
            };
            assert!(!all, "--all defaults to false");
        }
    }
}

mod discard {
    use std::str::FromStr;

    use clap::{CommandFactory, FromArgMatches};

    use super::*;
    use crate::commands::help_text::process_command_tree;

    #[test]
    fn concatenate_discard_each_supported_category_name_parses() {
        for name in ["audio", "subtitle", "attachment", "metadata", "chapters", "video"] {
            let result = CondorCli::try_parse_from(["condor", "concatenate", "--discard", name]);
            assert_matches!(
                result,
                Ok(CondorCli {
                    command: Some(Commands::Concatenate {
                        discard: Some(_),
                        ..
                    }),
                    ..
                }),
                "\"--discard {name}\" parses"
            );
        }
    }

    #[test]
    fn discard_categories_format_canonical_names_and_parse_back_to_original_variants() {
        assert_eq!(DiscardCategory::Audio.to_string(), "audio");
        assert_eq!(DiscardCategory::Subtitle.to_string(), "subtitle");
        assert_eq!(DiscardCategory::Attachment.to_string(), "attachment");
        assert_eq!(DiscardCategory::Metadata.to_string(), "metadata");
        assert_eq!(DiscardCategory::Chapters.to_string(), "chapters");
        assert_eq!(DiscardCategory::Video.to_string(), "video");
        for category in [
            DiscardCategory::Audio,
            DiscardCategory::Subtitle,
            DiscardCategory::Attachment,
            DiscardCategory::Metadata,
            DiscardCategory::Chapters,
            DiscardCategory::Video,
        ] {
            let name: &'static str = category.into();
            assert_eq!(DiscardCategory::from_str(name), Ok(category));
        }
    }

    /// Root discard options belong to a full run, not a selected subcommand.
    #[test]
    fn root_discard_before_subcommand_is_rejected() {
        let mut cmd = process_command_tree(CondorCli::command());
        let matches = cmd
            .try_get_matches_from_mut(["condor", "--discard", "audio", "detect-scenes"])
            .expect("known root options parse before admission");
        let error = CondorCli::admit_root_arguments(&matches, &mut cmd)
            .expect_err("root discard cannot be combined with detect-scenes");
        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
        assert!(error.to_string().contains("--discard"), "{error}");
    }

    /// Local discard is valid, but cannot make misplaced root discard valid.
    #[test]
    fn root_discard_conflicts_even_with_valid_local_discard() {
        let mut cmd = process_command_tree(CondorCli::command());
        let matches = cmd
            .try_get_matches_from_mut([
                "condor",
                "--discard",
                "audio",
                "concatenate",
                "--discard",
                "subtitle",
            ])
            .expect("known root and local options parse before admission");
        let error = CondorCli::admit_root_arguments(&matches, &mut cmd)
            .expect_err("local discard does not excuse misplaced root discard");
        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
        assert!(error.to_string().contains("--discard"), "{error}");

        let mut cmd = process_command_tree(CondorCli::command());
        let matches = cmd
            .try_get_matches_from_mut(["condor", "concatenate", "--discard", "subtitle"])
            .expect("command-local discard parses");
        CondorCli::admit_root_arguments(&matches, &mut cmd)
            .expect("command-local discard is admitted");
        let cli = CondorCli::from_arg_matches(&matches).expect("admitted CLI");
        assert_eq!(cli.discard, None);
        let Commands::Concatenate {
            discard: Some(categories),
            ..
        } = cli.command.expect("the concatenate subcommand is set")
        else {
            panic!("the command is concatenate");
        };
        assert_eq!(categories, vec![DiscardCategory::Subtitle]);
    }
}

mod enum_mappings {
    use super::*;

    #[test]
    fn scene_detection_cli_methods_map_to_core_algorithms_and_preserve_explicit_frame_bounds() {
        // None -> CoreSCDMethod::None
        let result = SceneDetectionMethod::None.as_core_method(Some(120), Some(240));
        assert_matches!(result, CoreSCDMethod::None { .. });
        if let CoreSCDMethod::None {
            minimum_length,
            maximum_length,
        } = result
        {
            assert_eq!(minimum_length, 120);
            assert_eq!(maximum_length, 240);
        }

        // Fast -> CoreSCDMethod::AVSceneChange with Fast
        let result = SceneDetectionMethod::Fast.as_core_method(Some(60), Some(300));
        assert_matches!(
            result,
            CoreSCDMethod::AVSceneChange {
                method: ScenecutMethod::Fast,
                ..
            }
        );
        if let CoreSCDMethod::AVSceneChange {
            minimum_length,
            maximum_length,
            ..
        } = result
        {
            assert_eq!(minimum_length, 60);
            assert_eq!(maximum_length, 300);
        }

        // Standard -> CoreSCDMethod::AVSceneChange with Standard
        let result = SceneDetectionMethod::Standard.as_core_method(Some(120), Some(600));
        assert_matches!(
            result,
            CoreSCDMethod::AVSceneChange {
                method: ScenecutMethod::Standard,
                ..
            }
        );
        if let CoreSCDMethod::AVSceneChange {
            minimum_length,
            maximum_length,
            ..
        } = result
        {
            assert_eq!(minimum_length, 120);
            assert_eq!(maximum_length, 600);
        }
    }

    #[test]
    fn scene_detection_none_without_lengths_uses_default_minimum_and_maximum_frames() {
        // None with None lengths uses defaults
        let result = SceneDetectionMethod::None.as_core_method(None, None);
        assert_matches!(result, CoreSCDMethod::None { .. });
        if let CoreSCDMethod::None {
            minimum_length,
            maximum_length,
        } = result
        {
            assert_eq!(
                minimum_length,
                andean_condor::models::sequence::scene_detector::DEFAULT_MIN_SCENE_LENGTH_FRAMES
                    as usize
            );
            assert_eq!(
                maximum_length,
                andean_condor::models::sequence::scene_detector::
                    DEFAULT_MAX_SCENE_LENGTH_SECONDS as usize
                    * andean_condor::models::sequence::scene_detector::
                        DEFAULT_MIN_SCENE_LENGTH_FRAMES as usize
            );
        }
    }

    #[test]
    fn cli_encoder_methods_map_to_corresponding_core_encoder_bases() {
        assert_matches!(EncoderMethod::AOM.as_encoder_base(), EncoderBase::AOM);
        assert_matches!(EncoderMethod::RAV1E.as_encoder_base(), EncoderBase::RAV1E);
        assert_matches!(EncoderMethod::VPX.as_encoder_base(), EncoderBase::VPX);
        assert_matches!(EncoderMethod::SVTAV1.as_encoder_base(), EncoderBase::SVTAV1);
        assert_matches!(EncoderMethod::AVM.as_encoder_base(), EncoderBase::AVM);
        assert_matches!(EncoderMethod::X264.as_encoder_base(), EncoderBase::X264);
        assert_matches!(EncoderMethod::X265.as_encoder_base(), EncoderBase::X265);
        assert_matches!(EncoderMethod::VVenC.as_encoder_base(), EncoderBase::VVenC);
        assert_matches!(EncoderMethod::FFmpeg.as_encoder_base(), EncoderBase::FFmpeg);
    }

    #[test]
    fn decoder_methods_convert_to_canonical_cli_names() {
        use super::DecoderMethod;
        assert_eq!(<&str>::from(&DecoderMethod::BestSource), "bestsource");
        assert_eq!(<&str>::from(&DecoderMethod::VSFFMS2), "vs-ffms2");
        assert_eq!(<&str>::from(&DecoderMethod::LSMASHWorks), "lsmash");
        assert_eq!(<&str>::from(&DecoderMethod::DGDecodeNV), "dgdecnv");
        assert_eq!(<&str>::from(&DecoderMethod::FFMS2), "ffms2");
    }

    #[test]
    fn decoder_canonical_names_parse_to_variants_and_unknown_name_is_rejected() {
        use std::str::FromStr;
        assert_matches!(
            DecoderMethod::from_str("bestsource"),
            Ok(DecoderMethod::BestSource)
        );
        assert_matches!(
            DecoderMethod::from_str("vs-ffms2"),
            Ok(DecoderMethod::VSFFMS2)
        );
        assert_matches!(
            DecoderMethod::from_str("lsmash"),
            Ok(DecoderMethod::LSMASHWorks)
        );
        assert_matches!(
            DecoderMethod::from_str("dgdecnv"),
            Ok(DecoderMethod::DGDecodeNV)
        );
        assert_matches!(DecoderMethod::from_str("ffms2"), Ok(DecoderMethod::FFMS2));
        assert_matches!(DecoderMethod::from_str("nonexistent"), Err(_));
    }

    #[test]
    fn concatenation_methods_convert_to_canonical_cli_names() {
        assert_eq!(<&str>::from(&ConcatenationMethod::MkvMerge), "mkvmerge");
        assert_eq!(<&str>::from(&ConcatenationMethod::FFmpeg), "ffmpeg");
        assert_eq!(<&str>::from(&ConcatenationMethod::Ivf), "ivf");
    }

    #[test]
    fn encoder_methods_convert_to_canonical_cli_names() {
        assert_eq!(<&str>::from(&EncoderMethod::AOM), "aom");
        assert_eq!(<&str>::from(&EncoderMethod::RAV1E), "rav1e");
        assert_eq!(<&str>::from(&EncoderMethod::VPX), "vpx");
        assert_eq!(<&str>::from(&EncoderMethod::SVTAV1), "svt-av1");
        assert_eq!(<&str>::from(&EncoderMethod::AVM), "avm");
        assert_eq!(<&str>::from(&EncoderMethod::X264), "x264");
        assert_eq!(<&str>::from(&EncoderMethod::X265), "x265");
        assert_eq!(<&str>::from(&EncoderMethod::VVenC), "vvenc");
        assert_eq!(<&str>::from(&EncoderMethod::FFmpeg), "ffmpeg");
    }

    #[test]
    fn quality_metrics_convert_to_canonical_cli_names() {
        assert_eq!(<&str>::from(&QualityMetric::VMAF), "vmaf");
        assert_eq!(<&str>::from(&QualityMetric::SSIMULACRA2), "ssimulacra2");
        assert_eq!(<&str>::from(&QualityMetric::BUTTERAUGLI), "butteraugli");
        assert_eq!(
            <&str>::from(&QualityMetric::BUTTERAUGLI3Norm),
            "butteraugli-3"
        );
        assert_eq!(<&str>::from(&QualityMetric::XPSNR), "xpsnr");
        assert_eq!(<&str>::from(&QualityMetric::CVVDP), "cvvdp");
    }

    #[test]
    fn quality_profiles_convert_to_canonical_cli_names() {
        assert_eq!(<&str>::from(&QualityProfile::Fast), "fast");
        assert_eq!(<&str>::from(&QualityProfile::Standard), "standard");
        assert_eq!(<&str>::from(&QualityProfile::Slow), "slow");
    }

    #[test]
    fn scene_detection_methods_convert_to_canonical_cli_names() {
        assert_eq!(<&str>::from(&SceneDetectionMethod::None), "none");
        assert_eq!(<&str>::from(&SceneDetectionMethod::Fast), "fast");
        assert_eq!(<&str>::from(&SceneDetectionMethod::Standard), "standard");
    }
}
