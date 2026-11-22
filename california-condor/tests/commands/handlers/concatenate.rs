use std::path::Path;

use crate::common::condor_cmd;
use crate::common::config::{init_config, load};
use andean_condor::models::sequence::scene_concatenator::ConcatMethod;
use california_condor::test_helpers::*;

#[cfg(test)]
mod handler_configuration {
    use super::*;
    use california_condor::{
        commands::{
            ConcatenationMethod, DiscardCategory,
            handlers::{
                concatenate::{apply_discard, concatenate_handler},
                init::init_handler,
            },
        },
        utils::hash_path::hash_path,
    };

    fn check_native_discard(phase: u8) {
        assert!((1..=3).contains(&phase));
        let video = get_test_video();
        let temp = tempfile::tempdir().expect("native fixture");
        let output = temp.path().join("out.mkv");
        for method in [
            ConcatMethod::MKVMerge(Default::default()),
            ConcatMethod::FFmpeg(Default::default()),
        ] {
            let mut config = default_config(&video, &output, temp.path());
            config.condor.sequence_config.scene_concatenator.method = method;
            let before = serde_json::to_vec(&config).expect("snapshot");
            apply_discard(
                &mut config,
                &[DiscardCategory::Audio, DiscardCategory::Subtitle, DiscardCategory::Attachment],
            )
            .expect("genuine video-only source is not a failed identification");
            assert_eq!(serde_json::to_vec(&config).expect("after"), before);
        }
        if phase == 1 {
            return;
        }
        let path = temp.path().join("one-frame.y4m");
        let mut data = b"YUV4MPEG2 W2 H2 F24:1 Ip A1:1 C420jpeg\nFRAME\n".to_vec();
        data.extend([16, 16, 16, 16, 128, 128]);
        std::fs::write(&path, data).expect("finite Y4M");
        let video = TestVideo { path, ..video };
        let mut config = default_config(&video, &output, temp.path());
        let before = serde_json::to_vec(&config).expect("snapshot");
        apply_discard(
            &mut config,
            &[DiscardCategory::Audio, DiscardCategory::Chapters],
        )
        .expect_err("native unrecognized Y4M cannot silently skip MKV discard");
        assert_eq!(serde_json::to_vec(&config).expect("after"), before);
        if phase == 2 {
            return;
        }
        config.condor.sequence_config.scene_concatenator.method =
            ConcatMethod::FFmpeg(Default::default());
        let before = serde_json::to_vec(&config).expect("snapshot");
        apply_discard(
            &mut config,
            &[DiscardCategory::Audio, DiscardCategory::Subtitle, DiscardCategory::Attachment],
        )
        .expect("native FFmpeg fallback keeps video-only no-match behavior");
        assert_eq!(serde_json::to_vec(&config).expect("after"), before);
    }

    #[test]
    fn discard_ancillary_tracks_on_native_video_only_source_leaves_configuration_unchanged() {
        check_native_discard(1);
    }

    #[test]
    fn discard_on_unrecognized_y4m_with_mkvmerge_fails_atomically() {
        check_native_discard(2);
    }

    #[test]
    fn discard_on_y4m_with_ffmpeg_fallback_preserves_empty_ancillary_selection() {
        check_native_discard(3);
    }

    #[test]
    fn concatenate_handler_without_overrides_retains_default_method_and_config_path() {
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
        let (config, found_config_path) =
            concatenate_handler(Some(&config_path), None, None, None, false)
                .expect("concatenate_handler should succeed");
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
    fn concatenate_handler_selects_ffmpeg_and_scene_directory_without_changing_other_checked_settings()
     {
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
        expected_config.condor.sequence_config.scene_concatenator.method =
            ConcatMethod::FFmpeg(Default::default());
        expected_config.condor.sequence_config.scene_concatenator.scenes_directory =
            temp_abs.join("scenes");
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
        let (config, found_config_path) = concatenate_handler(
            Some(&config_path),
            Some(&temp.path().join(hash_path(&input_abs))),
            Some(&ConcatenationMethod::FFmpeg),
            None,
            false,
        )
        .expect("concatanate_handler should succeed");
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
    use serial_test::serial;

    use super::*;

    #[cfg(unix)]
    fn check_discard_probe(case: &str) {
        use std::{os::unix::fs::PermissionsExt, time::Duration};

        assert!(["rejected", "fallback", "warning", "invalid-ffprobe"].contains(&case));
        let temp = tempfile::tempdir().expect("CLI fixture");
        let source = temp.path().join("source.mkv");
        let output = temp.path().join("out.mkv");
        std::fs::write(&source, b"source sentinel").expect("source");
        std::fs::write(&output, b"output sentinel").expect("output");
        let video = TestVideo {
            path: source.clone(),
            width: 2,
            height: 2,
            frames: 1,
            fps_rational: (24, 1),
            format: andean_condor::ffmpeg::FFPixelFormat::YUV420P,
            scenes: vec![],
        };
        let mut config = default_config(&video, &output, &temp.path().join("work"));
        config.condor.output.overwrite = true;
        if matches!(case, "fallback" | "invalid-ffprobe") {
            config.condor.sequence_config.scene_concatenator.method =
                ConcatMethod::FFmpeg(Default::default());
        }
        let file = temp.path().join("condor.json");
        config.save(&file).expect("existing job");
        let before = std::fs::read(&file).expect("before");
        let tools = temp.path().join("tools");
        std::fs::create_dir_all(&tools).expect("isolated PATH");
        let mkvmerge = if matches!(case, "warning" | "invalid-ffprobe") {
            (
                r#"{"container":{"recognized":true,"supported":true},"errors":[],"warnings":["recoverable warning"],"tracks":[{"id":0,"type":"video"},{"id":3,"type":"audio"}]}"#,
                1,
            )
        } else {
            (r#"{"errors":["native identify error"]}"#, 2)
        };
        let ffprobe = if case == "invalid-ffprobe" {
            "{invalid"
        } else {
            r#"{"streams":[{"index":0,"codec_type":"video"},{"index":7,"codec_type":"audio"},{"index":11,"codec_type":"subtitle"},{"index":15,"codec_type":"attachment"}]}"#
        };
        for (name, json, status) in [("mkvmerge", mkvmerge.0, mkvmerge.1), ("ffprobe", ffprobe, 0)]
        {
            let script = format!(
                "#!/bin/sh\nprintf '%s\\n' '{name}' \"$@\" >> \"$TRACK_PROBE_CALLS\"\nprintf '%s\\n' '{json}'\nprintf '%s\\n' 'native diagnostic' >&2\nexit {status}\n"
            );
            let path = tools.join(name);
            std::fs::write(&path, script).expect("fake process");
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
                .expect("executable");
        }
        // The sentinel is deliberately undecodable: after saving, opening the
        // source must fail before a TUI/concatenation/encoding pipeline starts.
        let result = condor_cmd(&temp)
            .env("PATH", &tools)
            .env("TRACK_PROBE_CALLS", temp.path().join("calls"))
            .env("RUST_LOG", "california_condor=debug")
            .args(["concatenate", "--discard", "audio,chapters,metadata"])
            .timeout(Duration::from_secs(20))
            .output()
            .expect("bounded CLI");
        assert!(
            !result.status.success(),
            "{case}: undecodable source must stop"
        );
        let message = String::from_utf8_lossy(&result.stderr);
        let calls = std::fs::read_to_string(temp.path().join("calls")).expect("probes called");
        if matches!(case, "rejected" | "invalid-ffprobe") {
            assert_eq!(
                std::fs::read(&file).expect("after"),
                before,
                "{case}: {message}"
            );
            assert!(
                message.contains(source.to_str().expect("path")),
                "{case}: {message}"
            );
            if case == "rejected" {
                assert!(
                    message.contains("mkvmerge") && message.contains("native diagnostic"),
                    "{message}"
                );
                assert!(!calls.contains("ffprobe"));
            } else {
                assert!(message.contains("ffprobe"), "{message}");
                assert!(
                    !calls.contains("mkvmerge"),
                    "invalid ffprobe cannot use MKV-only success"
                );
            }
        } else {
            let concat = load(&file).condor.sequence_config.scene_concatenator;
            match concat.method {
                ConcatMethod::FFmpeg(ffmpeg) => {
                    assert!(
                        message.contains("probing the FFmpeg streams only")
                            && message.contains("native identify error")
                            && message.contains("native diagnostic"),
                        "fallback retains native diagnostics: {message}"
                    );
                    let tracks = ffmpeg.tracks.expect("real fallback discard saved");
                    assert_eq!(tracks.len(), 3);
                    assert!(!tracks[&7].copy());
                    assert!(tracks[&11].copy());
                    assert!(tracks[&15].copy());
                    assert!(!ffmpeg.chapters.expect("chapters").copy);
                    assert!(!ffmpeg.metadata.expect("metadata").copy);
                    assert!(calls.contains(
                        "ffprobe\n-v\nerror\n-show_entries\nstream=index,codec_type\n-of\njson\n"
                    ));
                },
                ConcatMethod::MKVMerge(mkvmerge) => {
                    let tracks = mkvmerge.tracks.expect("valid warning discard saved");
                    assert_eq!(tracks.len(), 1);
                    assert!(!tracks[&3].copy());
                },
                ConcatMethod::Ivf => panic!("method changed"),
            }
        }
        assert_eq!(
            std::fs::read(&source).expect("source unchanged"),
            b"source sentinel"
        );
        assert_eq!(
            std::fs::read(&output).expect("output unchanged"),
            b"output sentinel"
        );
        eprintln!("track probe CLI {case}: {message}");
    }

    #[cfg(unix)]
    #[test]
    fn concatenate_mkvmerge_probe_error_preserves_job_without_ffprobe_fallback() {
        check_discard_probe("rejected");
    }

    #[cfg(unix)]
    #[test]
    fn concatenate_ffmpeg_fallback_saves_reported_stream_indices_and_discard_flags() {
        check_discard_probe("fallback");
    }

    #[cfg(unix)]
    #[test]
    fn concatenate_mkvmerge_warning_keeps_valid_probe_and_saves_discard() {
        check_discard_probe("warning");
    }

    #[cfg(unix)]
    #[test]
    fn concatenate_invalid_ffprobe_json_preserves_job_without_mkvmerge_probe() {
        check_discard_probe("invalid-ffprobe");
    }

    #[test]
    fn concatenator_validation_rejects_declared_inline_source_output_alias_after_instantiation() {
        use andean_condor::{
            core::sequence::{Sequence, scene_concatenator::SceneConcatenator},
            models::input::{Input, VapourSynthScriptSource},
        };
        let temp = tempfile::tempdir().expect("fixture");
        let source = temp.path().join("declared-script.vpy");
        std::fs::write(&source, b"declared source bytes").expect("source");
        let video = TestVideo {
            path: source.clone(),
            width: 32,
            height: 16,
            frames: 2,
            fps_rational: (24, 1),
            format: andean_condor::ffmpeg::FFPixelFormat::YUV420P,
            scenes: vec![],
        };
        let mut config = default_config(&video, &source, temp.path());
        config.condor.input=Input::VapourSynthScript{source:VapourSynthScriptSource::Text("import vapoursynth as vs\nvs.core.std.BlankClip(width=32,height=16,length=2,format=vs.YUV420P8).set_output()".into()),variables:Default::default(),index:0,filters:vec![],stream_concurrently:false};
        config.condor.output.overwrite = true;
        config.condor.sequence_config.scene_concatenator.method = ConcatMethod::Ivf;
        let mut condor = config
            .instantiate_condor(Box::new(|_| Ok(())), &Default::default())
            .expect("open inline source, retain declared identity");
        assert!(
            SceneConcatenator::default().validate(&mut condor).is_err(),
            "final core boundary protects CLI's duplicated inline source path"
        );
        assert_eq!(
            std::fs::read(source).expect("source preserved"),
            b"declared source bytes"
        );
    }

    fn check_concatenate_output(consent: bool) {
        use california_condor::commands::handlers::concatenate::concatenate_handler;
        for case in ["alias", "distinct", "override", "missing-parent"] {
            let temp = tempfile::tempdir().expect("fixture");
            let source = temp.path().join("must-not-open.mkv");
            std::fs::write(&source, b"source sentinel").expect("source");
            let video = TestVideo {
                path: source.clone(),
                width: 32,
                height: 16,
                frames: 2,
                fps_rational: (24, 1),
                format: andean_condor::ffmpeg::FFPixelFormat::YUV420P,
                scenes: vec![],
            };
            let output = if case == "alias" {
                source.clone()
            } else if case == "missing-parent" {
                temp.path().join("missing/out.mkv")
            } else {
                temp.path().join("out.mkv")
            };
            let mut config = default_config(&video, &output, &temp.path().join("work"));
            if case == "override" {
                config.condor.sequence_config.scene_concatenator.output = Some(source.clone());
                config.condor.output.overwrite = true;
            }
            if case == "distinct" {
                std::fs::write(&output, b"old final").expect("old");
            }
            let file = temp.path().join("condor.json");
            config.save(&file).expect("save");
            let before = std::fs::read(&file).expect("before");
            let result = condor_cmd(&temp)
                .env("PATH", "")
                .args(["concatenate", "--discard", "audio"])
                .output()
                .expect("CLI");
            let message = String::from_utf8_lossy(&result.stderr);
            assert!(!result.status.success());
            assert!(
                message.contains("Output") && !message.contains("not installed"),
                "{case}: {message}"
            );
            assert_eq!(std::fs::read(&file).expect("after"), before);
            assert_eq!(std::fs::read(&source).expect("source"), b"source sentinel");
            if case == "distinct" && consent {
                condor_cmd(&temp)
                    .env("PATH", "")
                    .args(["concatenate", "--overwrite"])
                    .assert()
                    .failure();
                assert!(load(&file).condor.output.overwrite);
                concatenate_handler(Some(&file), None, None, None, false)
                    .expect("omission retains saved true");
                assert!(load(&file).condor.output.overwrite);
                assert_eq!(std::fs::read(&output).expect("old"), b"old final");
            }
            assert!(!temp.path().join("missing").exists());
        }
    }

    #[test]
    fn concatenate_unsafe_output_refuses_before_discard_probe_or_save() {
        check_concatenate_output(false);
    }

    #[test]
    fn concatenate_overwrite_flag_saves_consent_and_omitted_flag_retains_it() {
        check_concatenate_output(true);
    }

    fn check_concatenate_crud(output_only: bool) {
        if !ffmpeg_is_available() {
            return;
        }
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let config_path = init_config(&temp);

        condor_cmd(&temp)
            .args(["concatenate", "set", "method", "ffmpeg"])
            .assert()
            .success();
        let concat = load(&config_path).condor.sequence_config.scene_concatenator;
        assert!(
            matches!(concat.method, ConcatMethod::FFmpeg(_)),
            "method is {}",
            concat.method
        );

        condor_cmd(&temp)
            .args(["concatenate", "set", "output", "final.mkv"])
            .assert()
            .success();
        let concat = load(&config_path).condor.sequence_config.scene_concatenator;
        assert_eq!(
            concat.output.as_deref(),
            Some(Path::new("final.mkv")),
            "output is set"
        );

        condor_cmd(&temp).args(["concatenate", "reset", "output"]).assert().success();
        let concat = load(&config_path).condor.sequence_config.scene_concatenator;
        assert!(concat.output.is_none(), "output was cleared");
        if output_only {
            return;
        }

        condor_cmd(&temp)
            .args(["concatenate", "set", "method", "mkvmerge"])
            .assert()
            .success();
        let concat = load(&config_path).condor.sequence_config.scene_concatenator;
        assert!(
            matches!(concat.method, ConcatMethod::MKVMerge(_)),
            "method is {}",
            concat.method
        );
    }

    #[serial]
    #[test]
    fn concatenate_set_method_switches_between_ffmpeg_and_mkvmerge() {
        check_concatenate_crud(false);
    }

    #[serial]
    #[test]
    fn concatenate_output_crud_saves_override_and_reset_clears_it() {
        check_concatenate_crud(true);
    }

    /// `--discard` probes the input and writes `copy: false` entries for the
    /// tracks it names, so `condor.json` describes the drop explicitly.
    #[serial]
    #[test]
    fn init_mkvmerge_discard_audio_subtitle_and_chapters_saves_explicit_drop_entries() {
        if !ffmpeg_is_available() || !mkvmerge_is_available() {
            return;
        }
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let input = get_test_multitrack_video();
        let output = temp.path().join("out.mkv");

        condor_cmd(&temp)
            .args([
                "init",
                path_str(&input),
                path_str(&output),
                "--concat",
                "mkvmerge",
                "--discard",
                "audio,subtitle,chapters",
            ])
            .assert()
            .success();

        let config_path = temp.path().join("condor.json");
        let concat = load(&config_path).condor.sequence_config.scene_concatenator;
        let ConcatMethod::MKVMerge(mkvmerge) = concat.method else {
            panic!("the method stays mkvmerge, but is {}", concat.method);
        };
        let tracks = mkvmerge.tracks.expect("a track map is written");
        assert_eq!(
            tracks.len(),
            3,
            "only the named tracks are written, not the whole map: {tracks:?}"
        );
        for (id, track) in &tracks {
            assert!(!track.copy(), "track {id} is dropped");
        }
        let ids = tracks.keys().copied().collect::<Vec<_>>();
        assert!(
            ids.contains(&1) && ids.contains(&2) && ids.contains(&3),
            "both audio tracks and the subtitle are dropped: {ids:?}"
        );
        assert!(!mkvmerge.chapters.expect("chapters are written").copy);
    }

    /// The FFmpeg selection map has to list every kept stream, because a stream
    /// that is absent from the map is not copied.
    #[serial]
    #[test]
    fn init_ffmpeg_discard_audio_saves_two_drops_and_at_least_one_kept_stream() {
        if !ffmpeg_is_available() || !mkvmerge_is_available() {
            return;
        }
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let input = get_test_multitrack_video();
        let output = temp.path().join("out.mkv");

        condor_cmd(&temp)
            .args([
                "init",
                path_str(&input),
                path_str(&output),
                "--concat",
                "ffmpeg",
                "--discard",
                "audio",
            ])
            .assert()
            .success();

        let config_path = temp.path().join("condor.json");
        let concat = load(&config_path).condor.sequence_config.scene_concatenator;
        let ConcatMethod::FFmpeg(ffmpeg) = concat.method else {
            panic!("the method stays ffmpeg, but is {}", concat.method);
        };
        let tracks = ffmpeg.tracks.expect("a stream map is written");
        assert!(
            tracks.iter().any(|(_, track)| track.copy()),
            "the subtitle and attachment stay selected: {tracks:?}"
        );
        assert_eq!(
            tracks.values().filter(|track| !track.copy()).count(),
            2,
            "both audio streams are dropped: {tracks:?}"
        );
    }

    /// A video-only input has nothing to drop, so the flag leaves the settings
    /// unset rather than writing an empty map.
    #[serial]
    #[test]
    fn init_mkvmerge_discard_audio_on_video_only_source_leaves_track_map_unset() {
        if !ffmpeg_is_available() || !mkvmerge_is_available() {
            return;
        }
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let video = get_test_video();
        let output = temp.path().join("out.mkv");

        condor_cmd(&temp)
            .args([
                "init",
                path_str(&video.path),
                path_str(&output),
                "--concat",
                "mkvmerge",
                "--discard",
                "audio",
            ])
            .assert()
            .success();

        let config_path = temp.path().join("condor.json");
        let concat = load(&config_path).condor.sequence_config.scene_concatenator;
        let ConcatMethod::MKVMerge(mkvmerge) = concat.method else {
            panic!("the method stays mkvmerge, but is {}", concat.method);
        };
        assert!(
            mkvmerge.tracks.is_none(),
            "no map is written for a source without audio: {:?}",
            mkvmerge.tracks
        );
    }
}

#[cfg(test)]
mod attachments {
    use crate::common;
    use std::{
        path::Path,
        process::Command,
        sync::{Arc, atomic::AtomicBool, mpsc},
    };

    use andean_condor::{
        core::sequence::scene_concatenator::SceneConcatenator,
        models::sequence::scene_concatenator::{ConcatMethod, MkvmergeConfig},
    };
    use california_condor::{
        commands::{DiscardCategory, handlers::concatenate::concatenate_handler},
        configuration::Configuration,
        test_helpers::{TestVideo, default_config},
    };
    use serde_json::{Value, json};

    fn run(command: &mut Command) -> Vec<u8> {
        let result = command.output().expect("required native tool");
        assert!(
            result.status.success(),
            "{command:?}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        result.stdout
    }

    fn identify(path: &Path) -> Value {
        serde_json::from_slice(&run(Command::new("mkvmerge").arg("-J").arg(path)))
            .expect("native identify")
    }

    fn fixture(root: &Path) -> (std::path::PathBuf, Vec<std::path::PathBuf>) {
        let attachment = root.join("source attachment.txt");
        std::fs::write(&attachment, b"attachment exact attachment bytes\x00\xff")
            .expect("attachment");
        let base = root.join("base.mkv");
        run(Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=red:size=32x16:rate=24",
                "-f",
                "lavfi",
                "-i",
                "sine=sample_rate=48000",
                "-t",
                "0.2",
                "-c:v",
                "ffv1",
                "-c:a",
                "flac",
            ])
            .arg(&base));
        let source = root.join("source.mkv");
        run(Command::new("mkvmerge")
            .arg("-o")
            .arg(&source)
            .args([
                "--attachment-name",
                "original.txt",
                "--attachment-description",
                "original description",
                "--attachment-mime-type",
                "text/plain",
                "--attach-file",
            ])
            .arg(&attachment)
            .arg(&base));
        let ids = identify(&source);
        assert!(
            ids["tracks"]
                .as_array()
                .expect("tracks")
                .iter()
                .any(|t| t["id"] == 1 && t["type"] == "audio")
        );
        assert_eq!(
            ids["attachments"][0]["id"], 1,
            "real independent ID collision"
        );
        let chunks = ["black", "white"]
            .into_iter()
            .enumerate()
            .map(|(i, color)| {
                let path = root.join(format!("chunk-{i}.mkv"));
                run(Command::new("ffmpeg")
                    .args([
                        "-v",
                        "error",
                        "-f",
                        "lavfi",
                        "-i",
                        &format!("color={color}:size=32x16:rate=24"),
                        "-frames:v",
                        "2",
                        "-c:v",
                        "ffv1",
                    ])
                    .arg(&path));
                path
            })
            .collect();
        (source, chunks)
    }

    fn native_discard(category: DiscardCategory) {
        let dir = tempfile::tempdir().expect("fixture");
        let (source, chunks) = fixture(dir.path());
        let original = std::fs::read(&source).expect("original");
        let output = dir.path().join("out.mkv");
        let video = TestVideo {
            path: source.clone(),
            width: 32,
            height: 16,
            frames: 4,
            fps_rational: (24, 1),
            format: andean_condor::ffmpeg::FFPixelFormat::YUV420P,
            scenes: vec![],
        };
        let mut config = default_config(&video, &output, &dir.path().join("work"));
        config.condor.sequence_config.scene_concatenator.method = ConcatMethod::MKVMerge(serde_json::from_value(json!({
        "tracks":{"0":{"type":"video", "name":"encoded scenes"},"1":{"type":"audio", "name":"kept audio", "language":"jpn", "default_track":false}},
        "attachments":{"1":{"copy":true}}
    })).expect("canonical config"));
        let file = dir.path().join("condor.json");
        config.save(&file).expect("save");
        let (config, _) = concatenate_handler(Some(&file), None, None, Some(&[category]), false)
            .expect("real source probe/discard/save");
        let ConcatMethod::MKVMerge(settings) =
            config.condor.sequence_config.scene_concatenator.method
        else {
            panic!("mkvmerge")
        };
        let (tx, _rx) = mpsc::channel();
        SceneConcatenator::mkvmerge(
            dir.path(),
            &output,
            &chunks,
            Some(&source),
            (24, 1).into(),
            Some(&settings),
            &tx,
            &andean_condor::utils::cancellation::Cancellation::from(std::sync::Arc::clone(
                &Arc::new(AtomicBool::new(false)),
            )),
        )
        .expect("native final mux");
        let ids = identify(&output);
        let audio: Vec<_> = ids["tracks"]
            .as_array()
            .expect("tracks")
            .iter()
            .filter(|t| t["type"] == "audio")
            .collect();
        if matches!(category, DiscardCategory::Attachment) {
            assert_eq!(audio.len(), 1, "discarding attachment1 must keep audio1");
            assert!(ids["attachments"].as_array().expect("attachments").is_empty());
            assert_eq!(audio[0]["properties"]["language"], "jpn");
            assert_eq!(audio[0]["properties"]["track_name"], "kept audio");
            assert_eq!(audio[0]["properties"]["default_track"], false);
            let pcm = |path: &Path| {
                run(Command::new("ffmpeg")
                    .args(["-v", "error", "-i"])
                    .arg(path)
                    .args(["-map", "0:a:0", "-f", "s16le", "-"]))
            };
            assert_eq!(pcm(&source), pcm(&output), "exact decoded audio identity");
            let packets = |path: &Path| {
                let data: Value = serde_json::from_slice(&run(Command::new("ffprobe")
                    .args([
                        "-v",
                        "error",
                        "-select_streams",
                        "a",
                        "-show_packets",
                        "-show_data_hash",
                        "sha256",
                        "-of",
                        "json",
                    ])
                    .arg(path)))
                .expect("packet hashes");
                data["packets"]
                    .as_array()
                    .expect("packets")
                    .iter()
                    .map(|p| p["data_hash"].clone())
                    .collect::<Vec<_>>()
            };
            let original_packets = packets(&source);
            assert!(!original_packets.is_empty());
            assert_eq!(
                packets(&output),
                original_packets,
                "compressed audio bytes unchanged"
            );
        } else {
            assert!(audio.is_empty(), "discarding audio1 must drop audio1");
            assert_eq!(
                ids["attachments"].as_array().expect("attachments").len(),
                1,
                "discarding audio1 must keep attachment1"
            );
            assert_eq!(ids["attachments"][0]["file_name"], "original.txt");
            assert_eq!(ids["attachments"][0]["description"], "original description");
            assert_eq!(ids["attachments"][0]["content_type"], "text/plain");
            let extracted = dir.path().join("extracted");
            run(Command::new("mkvextract")
                .arg(&output)
                .arg("attachments")
                .arg(format!("1:{}", extracted.display())));
            assert_eq!(
                std::fs::read(extracted).expect("extracted"),
                b"attachment exact attachment bytes\x00\xff"
            );
        }
        let pixels = run(Command::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(&output)
            .args(["-map", "0:v:0", "-pix_fmt", "gray", "-f", "rawvideo", "-"]));
        assert_eq!(pixels.len(), 32 * 16 * 4);
        assert!(pixels[..32 * 16 * 2].iter().all(|&p| p < 5));
        assert!(pixels[32 * 16 * 2..].iter().all(|&p| p > 250));
        assert_eq!(std::fs::read(source).expect("unchanged source"), original);
        assert_eq!(
            Configuration::load(&file)
                .expect("load")
                .expect("saved")
                .condor
                .sequence_config
                .scene_concatenator
                .method,
            ConcatMethod::MKVMerge(settings)
        );
    }

    #[test]
    fn mkvmerge_discard_attachment_id_one_preserves_colliding_audio_bytes_and_metadata() {
        native_discard(DiscardCategory::Attachment);
    }

    #[test]
    fn mkvmerge_discard_audio_id_one_preserves_colliding_attachment_bytes_and_metadata() {
        native_discard(DiscardCategory::Audio);
    }

    fn check_attachment_namespace_crud(phase: u8) {
        assert!((1..=4).contains(&phase));
        let dir = tempfile::tempdir().expect("fixture");
        let (source, _) = fixture(dir.path());
        common::condor_cmd(&dir)
            .args([
                "init",
                source.to_str().expect("path"),
                dir.path().join("out.mkv").to_str().expect("path"),
                "--concat",
                "mkvmerge",
                "--discard",
                "audio,attachment",
            ])
            .assert()
            .success();
        let saved = Configuration::load(&dir.path().join("condor.json"))
            .expect("load")
            .expect("saved");
        let saved = serde_json::to_value(saved).expect("canonical CLI save");
        let method =
            &saved["condor"]["sequence_config"]["scene_concatenator"]["method"]["mkvmerge"];
        assert_eq!(method["tracks"]["1"]["type"], "audio");
        assert_eq!(method["tracks"]["1"]["copy"], false);
        assert_eq!(method["attachments"]["1"]["copy"], false);
        if phase == 1 {
            return;
        }
        let video = TestVideo {
            path: source.clone(),
            width: 32,
            height: 16,
            frames: 4,
            fps_rational: (24, 1),
            format: andean_condor::ffmpeg::FFPixelFormat::YUV420P,
            scenes: vec![],
        };
        let mut config = default_config(
            &video,
            &dir.path().join("out.mkv"),
            &dir.path().join("work"),
        );
        let settings: MkvmergeConfig = serde_json::from_value(json!({
        "tracks":{"1":{"type":"audio","language":"jpn"}}, "attachments":{"1":{"copy":true}},
        "extra_inputs":[{"path":source,"tracks":{"0":{"type":"video","name":"extra video"},"1":{"type":"audio"}},"attachments":{"1":{"copy":false}}}]
    })).expect("canonical");
        config.condor.sequence_config.scene_concatenator.method =
            ConcatMethod::MKVMerge(settings.clone());
        let file = dir.path().join("condor.json");
        config.save(&file).expect("save");
        let info = common::condor_cmd(&dir)
            .args(["concatenate", "info", "method"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let info = String::from_utf8(info).expect("text");
        assert!(
            info.contains("attachments") && info.contains("jpn"),
            "{info}"
        );
        common::condor_cmd(&dir)
            .args(["concatenate", "set", "method", "mkvmerge"])
            .assert()
            .success();
        assert_eq!(
            Configuration::load(&file)
                .expect("load")
                .expect("saved")
                .condor
                .sequence_config
                .scene_concatenator
                .method,
            ConcatMethod::MKVMerge(settings)
        );
        if phase == 2 {
            return;
        }
        common::condor_cmd(&dir)
            .args(["concatenate", "set", "method", "ffmpeg"])
            .assert()
            .success();
        common::condor_cmd(&dir)
            .args(["concatenate", "set", "method", "mkvmerge"])
            .assert()
            .success();
        assert_eq!(
            Configuration::load(&file)
                .expect("load")
                .expect("saved")
                .condor
                .sequence_config
                .scene_concatenator
                .method,
            ConcatMethod::MKVMerge(MkvmergeConfig::default())
        );
        config.save(&file).expect("restore nondefault namespaces");
        common::condor_cmd(&dir)
            .args(["concatenate", "reset", "method"])
            .assert()
            .success();
        assert_eq!(
            Configuration::load(&file)
                .expect("load")
                .expect("saved")
                .condor
                .sequence_config
                .scene_concatenator
                .method,
            ConcatMethod::MKVMerge(MkvmergeConfig::default())
        );
        if phase == 3 {
            return;
        }
        let mut text = serde_json::to_value(&config).expect("JSON");
        text["condor"]["sequence_config"]["scene_concatenator"]["method"] =
            json!({"mkvmerge":{"tracks":{"1":{"type":"attachment"}},"attachments":{"1":{}}}});
        let before = serde_json::to_vec_pretty(&text).expect("ambiguous");
        std::fs::write(&file, &before).expect("legacy job");
        let result = common::condor_cmd(&dir)
            .args(["concatenate", "set", "method", "mkvmerge"])
            .output()
            .expect("CLI");
        assert!(!result.status.success());
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(
            error.contains("attachment ID 1") && error.contains("collision"),
            "{error}"
        );
        assert_eq!(std::fs::read(&file).expect("no partial save"), before);
        // The duplicate-key check is also exercised through the full root reader.
        let mut job = serde_json::to_string(&config).expect("job");
        let method =
            serde_json::to_string(&config.condor.sequence_config.scene_concatenator.method)
                .expect("method");
        job = job.replace(
            &method,
            r#"{"mkvmerge":{"tracks":{"1":{"type":"attachment"},"1":{"type":"audio"}}}}"#,
        );
        std::fs::write(&file, job.as_bytes()).expect("duplicate legacy key");
        let result = common::condor_cmd(&dir)
            .args(["concatenate", "reset", "method"])
            .output()
            .expect("CLI");
        assert!(!result.status.success());
        assert!(
            String::from_utf8_lossy(&result.stderr)
                .contains("duplicate mkvmerge track/attachment ID 1")
        );
        assert_eq!(std::fs::read(&file).expect("no save"), job.as_bytes());
    }

    #[test]
    fn init_discard_saves_audio_and_attachment_id_one_in_separate_namespaces() {
        check_attachment_namespace_crud(1);
    }

    #[test]
    fn concatenate_info_and_same_method_set_preserve_attachment_settings() {
        check_attachment_namespace_crud(2);
    }

    #[test]
    fn concatenate_method_switch_or_reset_restores_default_namespaces() {
        check_attachment_namespace_crud(3);
    }

    #[test]
    fn concatenate_ambiguous_or_duplicate_attachment_ids_refuse_without_resaving() {
        check_attachment_namespace_crud(4);
    }

    fn check_extra_input_namespaces(audio_selection: bool) {
        let dir = tempfile::tempdir().expect("fixture");
        let (source, chunks) = fixture(dir.path());
        let extra = dir.path().join("extra.mkv");
        let extra_attachment = dir.path().join("extra attachment.txt");
        std::fs::write(&extra_attachment, b"distinct extra attachment bytes")
            .expect("extra attachment");
        run(Command::new("mkvmerge")
            .arg("-o")
            .arg(&extra)
            .args([
                "--track-order",
                "0:1,0:0",
                "--attachment-name",
                "extra.txt",
                "--attachment-mime-type",
                "text/plain",
                "--attach-file",
            ])
            .arg(&extra_attachment)
            .arg(dir.path().join("base.mkv")));
        let ids = identify(&extra);
        assert_eq!(ids["tracks"][1]["id"], 1);
        assert_eq!(ids["tracks"][1]["type"], "video");
        assert_eq!(ids["attachments"][0]["id"], 1);
        let original = std::fs::read(&extra).expect("extra original");
        let added = dir.path().join("new attachment.txt");
        std::fs::write(&added, b"attachment new attachment identity").expect("new attachment");
        let extra_added = dir.path().join("new extra attachment.txt");
        std::fs::write(&extra_added, b"attachment extra new attachment identity")
            .expect("extra new attachment");
        for drop_video in [false, true] {
            let settings: MkvmergeConfig = serde_json::from_value(json!({
            "tracks":{"0":{"type":"video","copy":!drop_video,"name":"encoded only"},"1":{"type":"audio","language":"jpn"}},
            "attachments":{"1":{"path":added,"attachment_name":"new exact.txt","attachment_description":"new exact description","attachment_mime_type":"text/plain"},"2":{"copy":false,"path":"must not read","attachment_mime_type":"text/plain"}},
            "extra_inputs":[{"path":extra,"tracks":{"0":{"type":"audio","copy":false},"1":{"type":"video","copy":drop_video,"name":"extra actual video1"}},"attachments":{"1":{"copy":!drop_video},"7":{"path":extra_added,"attachment_name":"extra new.txt","attachment_description":"extra new description","attachment_mime_type":"text/plain"},"8":{"copy":false,"path":"must not read extra"}}}]
        })).expect("coexisting canonical maps");
            let output = dir.path().join(format!("extra-{drop_video}.mkv"));
            let (tx, _rx) = mpsc::channel();
            SceneConcatenator::mkvmerge(
                dir.path(),
                &output,
                &chunks,
                Some(&source),
                (24, 1).into(),
                Some(&settings),
                &tx,
                &andean_condor::utils::cancellation::Cancellation::from(std::sync::Arc::clone(
                    &Arc::new(AtomicBool::new(false)),
                )),
            )
            .expect("native extra namespaces");
            let ids = identify(&output);
            let tracks = ids["tracks"].as_array().expect("tracks");
            let video: Vec<_> = tracks.iter().filter(|t| t["type"] == "video").collect();
            assert_eq!(video.len(), 1);
            assert_eq!(
                video[0]["properties"]["track_name"],
                if drop_video {
                    "extra actual video1"
                } else {
                    "encoded only"
                }
            );
            let attachments = ids["attachments"].as_array().expect("attachments");
            assert_eq!(
                attachments.len(),
                if drop_video { 3 } else { 4 },
                "main copied1 plus new1 and extra new7, optionally extra copied1"
            );
            let new = attachments
                .iter()
                .find(|a| a["file_name"] == "new exact.txt")
                .expect("new attachment");
            assert_eq!(new["description"], "new exact description");
            assert_eq!(new["content_type"], "text/plain");
            let extracted = dir.path().join(format!("new-{drop_video}"));
            run(
                Command::new("mkvextract").arg(&output).arg("attachments").arg(format!(
                    "{}:{}",
                    new["id"],
                    extracted.display()
                )),
            );
            assert_eq!(
                std::fs::read(extracted).expect("bytes"),
                b"attachment new attachment identity"
            );
            let new = attachments
                .iter()
                .find(|a| a["file_name"] == "extra new.txt")
                .expect("extra new attachment");
            assert_eq!(new["description"], "extra new description");
            assert_eq!(new["content_type"], "text/plain");
            let extracted = dir.path().join(format!("extra-new-{drop_video}"));
            run(
                Command::new("mkvextract").arg(&output).arg("attachments").arg(format!(
                    "{}:{}",
                    new["id"],
                    extracted.display()
                )),
            );
            assert_eq!(
                std::fs::read(extracted).expect("extra bytes"),
                b"attachment extra new attachment identity"
            );
            let audio: Vec<_> = tracks.iter().filter(|t| t["type"] == "audio").collect();
            assert_eq!(audio.len(), 1);
            assert_eq!(audio[0]["properties"]["language"], "jpn");
        }
        assert_eq!(std::fs::read(&extra).expect("extra unchanged"), original);
        if !audio_selection {
            return;
        }
        for drop_audio in [false, true] {
            let settings: MkvmergeConfig = serde_json::from_value(json!({"extra_inputs":[{"path":source,"tracks":{"0":{"type":"video","copy":false},"1":{"type":"audio","copy":!drop_audio,"language":"jpn"}},"attachments":{"1":{"copy":drop_audio}}}]})).expect("extra audio1 attachment1");
            assert_eq!(
                serde_json::from_value::<MkvmergeConfig>(
                    serde_json::to_value(&settings).expect("write")
                )
                .expect("reload"),
                settings
            );
            let output = dir.path().join(format!("extra-audio-{drop_audio}.mkv"));
            let (tx, _rx) = mpsc::channel();
            SceneConcatenator::mkvmerge(
                dir.path(),
                &output,
                &chunks,
                None,
                (24, 1).into(),
                Some(&settings),
                &tx,
                &andean_condor::utils::cancellation::Cancellation::from(std::sync::Arc::clone(
                    &Arc::new(AtomicBool::new(false)),
                )),
            )
            .expect("extra independent audio/attachment");
            let ids = identify(&output);
            let audio: Vec<_> = ids["tracks"]
                .as_array()
                .expect("tracks")
                .iter()
                .filter(|t| t["type"] == "audio")
                .collect();
            assert_eq!(audio.len(), usize::from(!drop_audio));
            assert_eq!(
                ids["attachments"].as_array().expect("attachments").len(),
                usize::from(drop_audio)
            );
            if !drop_audio {
                assert_eq!(audio[0]["properties"]["language"], "jpn");
            }
        }
    }

    #[test]
    fn mkvmerge_extra_video_selection_and_new_attachment_bytes_use_independent_namespaces() {
        check_extra_input_namespaces(false);
    }

    #[test]
    fn mkvmerge_extra_audio_and_attachment_with_same_id_can_be_selected_independently() {
        check_extra_input_namespaces(true);
    }

    fn check_unmapped_attachment_source(phase: u8) {
        assert!((1..=3).contains(&phase));
        let dir = tempfile::tempdir().expect("fixture");
        let (source, chunks) = fixture(dir.path());
        let output = dir.path().join("sentinel.mkv");
        std::fs::write(&output, b"existing output").expect("sentinel");
        let (tx, _rx) = mpsc::channel();
        let settings = MkvmergeConfig {
            tracks: Some(std::collections::HashMap::from([(
                9,
                andean_condor::models::sequence::scene_concatenator::MkvmergeTrack::plain(
                    andean_condor::models::sequence::scene_concatenator::MkvmergeTrackKind::Video,
                    true,
                ),
            )])),
            ..Default::default()
        };
        let scratch = dir.path().join("not-created/scenes");
        let error = SceneConcatenator::mkvmerge(
            &scratch,
            &output,
            &chunks,
            Some(&source),
            (24, 1).into(),
            Some(&settings),
            &tx,
            &andean_condor::utils::cancellation::Cancellation::from(std::sync::Arc::clone(
                &Arc::new(AtomicBool::new(false)),
            )),
        )
        .expect_err("direct invalid config");
        assert!(error.to_string().contains("video track entry must use track ID 0"));
        assert!(!dir.path().join("not-created").exists());
        assert_eq!(
            std::fs::read(&output).expect("unchanged"),
            b"existing output"
        );
        if phase == 1 {
            return;
        }
        let y4m = dir.path().join("source.y4m");
        run(Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=size=32x16:rate=24",
                "-frames:v",
                "2",
            ])
            .arg(&y4m));
        let added = dir.path().join("independent.txt");
        std::fs::write(&added, b"new file not source ID").expect("new file");
        let settings = serde_json::from_value(
            json!({"attachments":{"1":{"path":added,"attachment_mime_type":"text/plain"}}}),
        )
        .expect("new path");
        SceneConcatenator::mkvmerge(
            dir.path(),
            &output,
            &chunks,
            Some(&y4m),
            (24, 1).into(),
            Some(&settings),
            &tx,
            &andean_condor::utils::cancellation::Cancellation::from(std::sync::Arc::clone(
                &Arc::new(AtomicBool::new(false)),
            )),
        )
        .expect("new file needs no unsupported source ID");
        assert_eq!(
            identify(&output)["attachments"].as_array().expect("attachments").len(),
            1
        );
        if phase == 2 {
            return;
        }
        for copy in [true, false] {
            std::fs::write(&output, b"existing output").expect("sentinel");
            let settings = serde_json::from_value(json!({"attachments":{"1":{"copy":copy}}}))
                .expect("source ID");
            let error = SceneConcatenator::mkvmerge(
                dir.path(),
                &output,
                &chunks,
                Some(&y4m),
                (24, 1).into(),
                Some(&settings),
                &tx,
                &andean_condor::utils::cancellation::Cancellation::from(std::sync::Arc::clone(
                    &Arc::new(AtomicBool::new(false)),
                )),
            )
            .expect_err("unsupported source has no IDs");
            assert!(error.to_string().contains("no provable mkvmerge track/attachment ID mapping"));
            assert_eq!(
                std::fs::read(&output).expect("unchanged"),
                b"existing output"
            );
        }
    }

    #[test]
    fn mkvmerge_invalid_output_video_id_refuses_before_scratch_or_output_write() {
        check_unmapped_attachment_source(1);
    }

    #[test]
    fn mkvmerge_unmapped_source_allows_new_attachment_file() {
        check_unmapped_attachment_source(2);
    }

    #[test]
    fn mkvmerge_unmapped_source_attachment_id_refuses_copy_and_drop_without_overwriting_output() {
        check_unmapped_attachment_source(3);
    }
}
