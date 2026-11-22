use crate::common::condor_cmd;
use andean_condor::ffmpeg::FFPixelFormat;
use california_condor::{
    commands::handlers::load_configuration,
    test_helpers::{TestVideo, default_config},
};

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use serde_json::Value;
    use serial_test::serial;

    use super::*;

    /// The five scenes `get_test_video` describes, without generating the
    /// clip the scenes command never opens: (0, 121), (121, 240), (240, 480),
    /// (480, 528), (528, 720).
    fn test_video() -> TestVideo {
        TestVideo {
            path: PathBuf::from("video.mkv"),
            width: 1920,
            height: 1080,
            frames: 720,
            fps_rational: (24, 1),
            format: FFPixelFormat::YUV420P,
            scenes: vec![(0, 121), (121, 240), (240, 480), (480, 528), (528, 720)],
        }
    }

    /// Writes a configuration whose scenes a detection run produced. The
    /// `scenes` command never opens the input, so the paths only need to
    /// load.
    fn scenes_config(temp: &tempfile::TempDir) -> PathBuf {
        let video = test_video();
        let mut configuration = default_config(&video, &temp.path().join("out.mkv"), temp.path());
        configuration.condor.scenes = video.mock_scenes(&configuration.condor.encoder);
        let config_path = temp.path().join("condor.json");
        configuration.save(&config_path).expect("the config saves");

        config_path
    }

    fn load(config_path: &Path) -> california_condor::configuration::Configuration {
        load_configuration(Some(config_path))
            .expect("load_configuration should succeed")
            .0
    }

    fn ranges(
        configuration: &california_condor::configuration::Configuration,
    ) -> Vec<(usize, usize)> {
        configuration
            .condor
            .scenes
            .iter()
            .map(|scene| (scene.start_frame, scene.end_frame))
            .collect()
    }

    fn stdout_json(output: &std::process::Output) -> Value {
        serde_json::from_slice(&output.stdout).expect("stdout is JSON")
    }

    fn stderr(output: &std::process::Output) -> String {
        String::from_utf8_lossy(&output.stderr).into_owned()
    }

    #[test]
    fn encoder_crud_offline_save_preserves_requested_executable_identity() {
        use california_condor::{
            commands::{
                config::ConfigCommand,
                handlers::encode::{EncodeCrud, EncodeKey, EncodeSet},
            },
            configuration::Configuration,
        };
        use serde_json::json;

        let temp = tempfile::tempdir().expect("fixture");
        let video = test_video();
        let mut configuration = default_config(&video, &temp.path().join("out.mkv"), temp.path());
        configuration.condor.scenes = video.mock_scenes(&configuration.condor.encoder);
        let requested = PathBuf::from("missing-custom-encoder");
        EncodeCrud::apply(
            &mut configuration,
            &EncodeSet::Executable {
                path: requested.clone(),
            },
        )
        .expect("offline global edit");
        assert_eq!(
            EncodeCrud::render(&configuration, Some(&EncodeKey::Executable)).expect("info"),
            json!("missing-custom-encoder")
        );
        let file = temp.path().join("condor.json");
        configuration.save(&file).expect("save");
        let saved = Configuration::load(&file).expect("load").expect("saved config");
        assert_eq!(saved.condor.encoder.executable(), requested);
    }

    #[test]
    fn cli_edit_persists_cleared_results_and_blocks_inherited_quality() {
        let temp = tempfile::tempdir().expect("temp directory");
        let path = scenes_config(&temp);
        let mut config = load(&path);
        config.condor.scenes[1].sequence_data.parallel_encoder.bytes = Some(123);
        config.condor.scenes[1].sequence_data.quality_check.quality.scores = vec![42.0];
        let peer = serde_json::to_value(&config.condor.scenes[2]).expect("peer");
        config.save(&path).expect("save");
        condor_cmd(&temp)
            .args(["scenes", "1", "set", "params", "crf=19"])
            .assert()
            .success();
        let updated = load(&path);
        assert!(
            updated.condor.scenes[1].sequence_data.parallel_encoder.bytes.is_none(),
            "editing must clear stale saved completion, not merely warn"
        );
        assert!(updated.condor.scenes[1].sequence_data.quality_check.quality.scores.is_empty());
        assert_eq!(
            serde_json::to_value(&updated.condor.scenes[2]).expect("peer"),
            peer
        );
        let error =
            updated.admit_results().expect_err("cleared history must not bless inherited q");
        assert!(format!("{error:#}").contains("scene 1"));
    }

    #[cfg(unix)]
    #[serial]
    #[test]
    fn current_native_results_resume_and_scene_edits_withdraw_affected_indices_until_explicit_reset()
     {
        use andean_condor::{
            core::sequence::{
                Sequence, parallel_encoder::ParallelEncoder, quality_check::QualityCheck,
                scene_concatenator::SceneConcatenator, target_quality::TargetQuality,
            },
            models::{
                encoder::{Encoder, EncoderBase, cli_parameter::CLIParameter},
                input::{ImportMethod, Input},
                sequence::{
                    quality_check::QualityCheckConfig,
                    scene_concatenator::ConcatMethod,
                    target_quality::{TargetQualityConfig, types::QualityMetric},
                },
            },
        };
        use std::{cell::Cell, os::unix::fs::PermissionsExt, rc::Rc};
        let temp = tempfile::tempdir().expect("fixture");
        let source = temp.path().join("source.mkv");
        let generated = std::process::Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=128x128:rate=24",
                "-frames:v",
                "9",
                "-c:v",
                "ffv1",
                "-y",
            ])
            .arg(&source)
            .output()
            .expect("installed ffmpeg");
        assert!(generated.status.success(), "{generated:?}");
        let video = TestVideo {
            path: source.clone(),
            width: 128,
            height: 128,
            frames: 9,
            fps_rational: (24, 1),
            format: FFPixelFormat::YUV420P,
            scenes: vec![(0, 3), (3, 6), (6, 9)],
        };
        let mut config = default_config(
            &video,
            &temp.path().join("out.mkv"),
            &temp.path().join("custom-work"),
        );
        config.condor.input = Input::Video {
            path: source.clone(),
            import_method: ImportMethod::FFMS2 {
                index: None,
                cache_path: None,
            },
            filters: vec![],
        };
        let wrapper = temp.path().join("codec");
        std::fs::write(&wrapper, format!("#!/bin/sh\nfor argument do if [ \"$argument\" = -o ]; then printf 'encode\\n' >> '{}'; break; fi; done\nexec x264 \"$@\"\n", temp.path().join("calls").display())).expect("counter");
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755))
            .expect("executable");
        config.condor.encoder = Encoder::default_from_base(&EncoderBase::X264, false);
        config.condor.encoder.set_executable(Some(wrapper));
        config.condor.encoder.set_quantizer(20.0);
        config.condor.encoder.parameters_mut().insert(
            "preset".into(),
            CLIParameter::new_string("--", "=", "ultrafast"),
        );
        config.condor.scenes = video.mock_scenes(&config.condor.encoder);
        let metric = QualityMetric::VMAF {
            target_range: (-1000.0, 1000.0),
            resolution: None,
            scaler: "bicubic".into(),
            threads: 0,
            model: None,
            features: vec![],
        };
        config.condor.sequence_config.target_quality = Some(TargetQualityConfig {
            maximum_probes: 1,
            quantizer_range: (20, 20),
            metric: metric.clone(),
            ..Default::default()
        });
        config.condor.sequence_config.quality_check = Some(QualityCheckConfig {
            metric,
            ..Default::default()
        });
        config.condor.sequence_config.scene_concatenator.method =
            ConcatMethod::FFmpeg(Default::default());
        let dir = config.condor.sequence_config.parallel_encoder.scenes_directory.clone();
        std::fs::create_dir_all(dir.join("Target Quality/1/Scene Concatenator")).expect("scratch");
        std::fs::create_dir_all(dir.join("Scene Concatenator")).expect("final concat scratch");
        let mut job = config
            .instantiate_condor(Box::new(|_| Ok(())), &Default::default())
            .expect("native job");
        let (tx, _rx) = std::sync::mpsc::channel();
        TargetQuality::default()
            .execute(&mut job, tx, Default::default())
            .expect("real probes");
        let (tx, _rx) = std::sync::mpsc::channel();
        ParallelEncoder::default()
            .execute(&mut job, tx, Default::default())
            .expect("real chunks");
        let (tx, _rx) = std::sync::mpsc::channel();
        SceneConcatenator::default()
            .execute(&mut job, tx, Default::default())
            .expect("real final output");
        let (tx, _rx) = std::sync::mpsc::channel();
        QualityCheck::default()
            .execute(&mut job, tx, Default::default())
            .expect("real QC");
        config.condor = job.as_data();
        for scene in &mut config.condor.scenes {
            assert!(scene.sequence_data.target_quality.passes[0].provenance.is_some());
            assert!(scene.sequence_data.parallel_encoder.provenance.is_some());
            assert!(scene.sequence_data.quality_check.quality.provenance.is_some());
            scene.sequence_data.scene_detection.transnetv2_scores =
                Some([(scene.start_frame, 0.5)].into());
            // Controlled detector input, not a native measurement/certificate claim.
            scene.sequence_data.noise_detection = Some(
                andean_condor::models::sequence::noise_detector::NoiseDetectorData {
                    noise: 0.4,
                    luminance: 0.3,
                    created_on: std::time::SystemTime::UNIX_EPOCH,
                },
            );
        }
        let path = temp.path().join("condor.json");
        config.save(&path).expect("current saved config");
        config.admit_results().expect("current reuse control");
        let calls = std::fs::read(temp.path().join("calls")).expect("calls");
        condor_cmd(&temp).env("CONDOR_TEST_MODE", "1").arg("encode").assert().success();
        assert_eq!(
            std::fs::read(temp.path().join("calls")).expect("no reencode"),
            calls
        );
        // Already aligned/no-op zoning preserves actual current certificates and resume.
        condor_cmd(&temp).args(["zone", "0 3"]).assert().success();
        let aligned = load(&path);
        for (actual, expected) in aligned.condor.scenes.iter().zip(&config.condor.scenes) {
            assert_eq!(
                serde_json::to_value(&actual.sequence_data.parallel_encoder).expect("PE"),
                serde_json::to_value(&expected.sequence_data.parallel_encoder)
                    .expect("original PE")
            );
            assert!(!actual.sequence_data.parallel_encoder.invalidated);
        }
        aligned.admit_results().expect("aligned current results still admitted");
        condor_cmd(&temp).env("CONDOR_TEST_MODE", "1").arg("encode").assert().success();
        assert_eq!(
            std::fs::read(temp.path().join("calls")).expect("aligned resume without work"),
            calls
        );
        let mut files = vec![source, temp.path().join("out.mkv")];
        files.extend((0..3).map(|i| dir.join(format!("{i:05}.264"))));
        files.extend((0..3).map(|i| dir.join(format!("Target Quality/1/{i:05}.264"))));
        let sentinel = temp.path().join("custom-work/user-sentinel");
        std::fs::write(&sentinel, b"not owned by condor").expect("sentinel");
        files.push(sentinel);
        let bytes = files
            .iter()
            .map(|p| std::fs::read(p).expect("owned/foreign file"))
            .collect::<Vec<_>>();
        for (args, affected) in [
            (vec!["zone", "1 2 x264 \"--crf 19\""], vec![0, 1, 2, 3, 4]),
            (vec!["scenes", "1", "set", "params", "crf=19"], vec![1]),
            (vec!["scenes", "1", "set", "end", "5"], vec![1]),
            (vec!["scenes", "1", "set", "sub-scenes", "3-5"], vec![1]),
            (vec!["scenes", "1", "delete"], vec![1]),
            (vec!["scenes", "add", "3", "5"], vec![1, 2, 3]),
            (vec!["zone", "3 6 --maximum-frames 2"], vec![1, 2, 3]),
            (
                vec!["--skip-scd", "--overwrite", "--zones", "3 6 --maximum-frames 2"],
                vec![1, 2, 3],
            ),
        ] {
            config.save(&path).expect("restore current metadata, not forge certificates");
            let result = condor_cmd(&temp).env("CONDOR_TEST_MODE", "1").args(&args).assert();
            if args[0] == "--skip-scd" {
                result.failure();
            } else {
                result.success();
            }
            let edited = load(&path);
            if args[0] == "zone" && args[1].starts_with("1 2 ") {
                assert_eq!(
                    ranges(&edited),
                    vec![(0, 1), (1, 2), (2, 3), (3, 6), (6, 9)]
                );
                assert_eq!(
                    edited.condor.scenes[0].encoder,
                    config.condor.scenes[0].encoder
                );
                assert_eq!(
                    edited.condor.scenes[2].encoder,
                    config.condor.scenes[0].encoder
                );
                assert_eq!(
                    edited.condor.scenes[1].encoder.parameters().get("crf"),
                    Some(&CLIParameter::new_number("--", " ", 19.0))
                );
                assert!(
                    edited.condor.scenes[..3]
                        .iter()
                        .all(|s| s.sequence_data.noise_detection.is_none())
                );
                assert_eq!(
                    edited.condor.scenes[3].sequence_data.noise_detection,
                    config.condor.scenes[1].sequence_data.noise_detection
                );
                assert_eq!(
                    edited.condor.scenes[4].sequence_data.noise_detection,
                    config.condor.scenes[2].sequence_data.noise_detection
                );
            }
            for (index, scene) in edited.condor.scenes.iter().enumerate() {
                if affected.contains(&index) {
                    let data = &scene.sequence_data;
                    assert!(data.parallel_encoder.invalidated, "{args:?}: scene {index}");
                    assert!(
                        data.parallel_encoder.provenance.is_none()
                            && data.parallel_encoder.bytes.is_none()
                            && data.parallel_encoder.started_on.is_none()
                            && data.parallel_encoder.completed_on.is_none()
                    );
                    assert!(
                        data.target_quality.passes.is_empty()
                            && data.target_quality.selection.is_none()
                            && data.target_quality.speed_transition.is_none()
                    );
                    assert!(
                        data.quality_check.quality.scores.is_empty()
                            && data.quality_check.quality.provenance.is_none()
                    );
                } else {
                    let mut actual = serde_json::to_value(scene).expect("peer");
                    let mut expected =
                        serde_json::to_value(&config.condor.scenes[index]).expect("original peer");
                    actual["sequence_data"].as_object_mut().expect("data").remove("zoner");
                    expected["sequence_data"].as_object_mut().expect("data").remove("zoner");
                    assert_eq!(actual, expected, "{args:?}: unaffected scene {index}");
                }
            }
            let json = std::fs::read(&path).expect("saved barrier");
            let denied =
                condor_cmd(&temp).env("CONDOR_TEST_MODE", "1").arg("encode").assert().failure();
            let first_affected = affected[0];
            assert!(stderr(denied.get_output()).contains(&format!("scene {first_affected}")));
            let saves = Rc::new(Cell::new(0));
            let observed = Rc::clone(&saves);
            let mut direct = edited
                .instantiate_condor(
                    Box::new(move |_| {
                        observed.set(observed.get() + 1);
                        Ok(())
                    }),
                    &Default::default(),
                )
                .expect("direct job");
            let before = serde_json::to_value(direct.as_data()).expect("before");
            let (tx, rx) = std::sync::mpsc::channel();
            let error = ParallelEncoder::default()
                .execute(&mut direct, tx, Default::default())
                .expect_err("direct core barrier");
            assert!(format!("{error:#}").contains(&format!("scene {first_affected}")));
            assert_eq!(saves.get(), 0);
            assert!(rx.try_iter().next().is_none());
            assert_eq!(
                serde_json::to_value(direct.as_data()).expect("after"),
                before
            );
            assert_eq!(std::fs::read(&path).expect("no resave"), json);
            assert_eq!(
                std::fs::read(temp.path().join("calls")).expect("no native work"),
                calls
            );
            for (file, expected) in files.iter().zip(&bytes) {
                assert_eq!(std::fs::read(file).expect("preserved"), *expected);
            }
        }
        // Even with *all* old physical files absent in a distinct empty workdir,
        // cleared TQ history cannot authorize an inherited quantizer.
        let mut edited = load(&path);
        edited.condor.sequence_config.parallel_encoder.scenes_directory =
            temp.path().join("empty-work");
        std::fs::create_dir_all(&edited.condor.sequence_config.parallel_encoder.scenes_directory)
            .expect("empty workdir");
        assert!(edited.admit_results().is_err());
        let mut absent = edited
            .instantiate_condor(
                Box::new(|_| anyhow::bail!("must not save")),
                &Default::default(),
            )
            .expect("absent artifacts");
        let (tx, rx) = std::sync::mpsc::channel();
        let error = ParallelEncoder::default()
            .execute(&mut absent, tx, Default::default())
            .expect_err("marker before source/artifact checks");
        assert!(format!("{error:#}").contains("scene 1"));
        assert!(rx.try_iter().next().is_none());
        for scene in &mut edited.condor.scenes {
            scene.encoder = edited.condor.encoder.clone();
            scene.sequence_data.target_quality = Default::default();
            scene.sequence_data.quality_check = Default::default();
            scene.sequence_data.parallel_encoder = Default::default();
        }
        edited.condor.sequence_config.target_quality = None;
        let mut recovered = edited
            .instantiate_condor(Box::new(|_| Ok(())), &Default::default())
            .expect("explicit reset");
        let (tx, _rx) = std::sync::mpsc::channel();
        ParallelEncoder::default()
            .execute(&mut recovered, tx, Default::default())
            .expect("normal producer after explicit reset in empty workdir");
        assert!(
            recovered
                .scenes
                .iter()
                .all(|s| s.sequence_data.parallel_encoder.provenance.is_some())
        );
        for (file, expected) in files.iter().zip(&bytes) {
            assert_eq!(std::fs::read(file).expect("original preserved"), *expected);
        }
    }

    fn check_scene_info(phase: u8) {
        assert!((1..=4).contains(&phase));
        let temp = tempfile::tempdir().expect("temp directory");
        let config_path = scenes_config(&temp);
        let mut configuration = load(&config_path);
        configuration.condor.scenes[0].sequence_data.parallel_encoder.bytes = Some(123);
        configuration.condor.scenes[1].sub_scenes = Some(vec![
            andean_condor::models::scene::SubScene {
                start_frame: 121,
                end_frame: 180,
            },
            andean_condor::models::scene::SubScene {
                start_frame: 180,
                end_frame: 240,
            },
        ]);
        configuration.save(&config_path).expect("the config saves");

        // Without an action the whole list is printed, with all their values,
        // and the totals follow on stderr.
        let bare = condor_cmd(&temp).arg("scenes").assert().success();
        let all = stdout_json(bare.get_output());
        let all = all.as_array().expect("an array of scenes");
        assert_eq!(all.len(), 5);
        assert_eq!(all[0]["start_frame"], 0);
        assert_eq!(all[0]["end_frame"], 121);
        assert_eq!(
            all[0]["sequence_data"]["parallel_encoder"]["bytes"], 123,
            "saved results are part of the report"
        );
        assert_eq!(all[1]["sub_scenes"][1]["end_frame"], 240);
        assert!(
            stderr(bare.get_output()).contains("Scenes: 5 Frames: 0-720"),
            "the totals name the count and the covered frames: {}",
            stderr(bare.get_output())
        );

        let info = condor_cmd(&temp).args(["scenes", "info"]).assert().success();
        assert_eq!(stdout_json(info.get_output()), serde_json::json!(all));
        if phase == 1 {
            return;
        }

        // An index addresses one scene, which is reported on its own, and the
        // summary that follows describes that scene rather than the list.
        let scene = condor_cmd(&temp).args(["scenes", "2", "info"]).assert().success();
        assert_eq!(stdout_json(scene.get_output())["start_frame"], 240);
        assert!(
            stderr(scene.get_output()).contains("Scenes: 1 Frames: 240-480"),
            "an indexed read reports the scenes it addressed: {}",
            stderr(scene.get_output())
        );
        let frames = condor_cmd(&temp).args(["scenes", "2", "info", "frames"]).assert().success();
        assert_eq!(
            stdout_json(frames.get_output()),
            serde_json::json!({
                "start_frame": 240,
                "end_frame":   480,
            })
        );
        assert!(
            stderr(frames.get_output()).contains("Scenes: 1 Frames: 240-480"),
            "a key read reports the scene it addressed: {}",
            stderr(frames.get_output())
        );

        // Several indices report an array, in ascending order.
        if phase != 2 {
            let many =
                condor_cmd(&temp).args(["scenes", "1,4", "info", "frames"]).assert().success();
            assert_eq!(
                stdout_json(many.get_output()),
                serde_json::json!([
                    {"start_frame": 121, "end_frame": 240},
                    {"start_frame": 528, "end_frame": 720},
                ])
            );
            assert!(
                stderr(many.get_output()).contains("Scenes: 2 Frames: 121-720"),
                "a list of indices reports just those scenes: {}",
                stderr(many.get_output())
            );
        }

        let sub_scenes =
            condor_cmd(&temp).args(["scenes", "1", "info", "sub-scenes"]).assert().success();
        assert_eq!(
            stdout_json(sub_scenes.get_output()).as_array().expect("two sub-scenes").len(),
            2
        );
        if phase <= 3 {
            return;
        }

        let missing = condor_cmd(&temp).args(["scenes", "9", "info"]).assert().failure();
        assert!(
            stderr(missing.get_output()).contains("scene index 9 does not exist"),
            "a missing index is named"
        );
        let partial = condor_cmd(&temp).args(["scenes", "1-2,8-10,16", "info"]).assert().failure();
        assert!(
            stderr(partial.get_output()).contains("scene index 8 does not exist"),
            "the lowest missing index is named"
        );
        let bad = condor_cmd(&temp).args(["scenes", "scene1", "info"]).assert().failure();
        assert!(
            stderr(bad.get_output()).contains("is not a scene index"),
            "a bad index explains what an index looks like"
        );
    }

    fn check_scene_frame_edit(phase: u8) {
        assert!((1..=4).contains(&phase));
        use andean_condor::core::sequence::parallel_encoder::ParallelEncoder;

        let temp = tempfile::tempdir().expect("temp directory");
        let config_path = scenes_config(&temp);
        let mut configuration = load(&config_path);
        configuration.condor.scenes[0].sequence_data.parallel_encoder.bytes = Some(123);
        let scenes_directory =
            &configuration.condor.sequence_config.parallel_encoder.scenes_directory;
        std::fs::create_dir_all(scenes_directory).expect("scenes directory");
        let encode = scenes_directory.join(format!("{}.ivf", ParallelEncoder::scene_id(0)));
        std::fs::write(&encode, "existing encode").expect("encode fixture");
        configuration.save(&config_path).expect("the config saves");
        let original = std::fs::read(&config_path).expect("config bytes");

        let moved = condor_cmd(&temp)
            .env("RUST_LOG", "california_condor=info")
            .args(["scenes", "0", "set", "end", "100"])
            .assert()
            .success();
        assert!(
            String::from_utf8_lossy(&moved.get_output().stdout)
                .contains("Scenes scene 0 end_frame updated"),
            "the change is reported: {}",
            String::from_utf8_lossy(&moved.get_output().stdout)
        );
        let warnings = stderr(moved.get_output());
        assert!(
            warnings.contains("scene index 0")
                && warnings.contains("may be stale after scene edits"),
            "saved results and files warn as stale: {warnings}"
        );

        let configuration = load(&config_path);
        assert_eq!(
            ranges(&configuration)[0],
            (0, 100),
            "only the addressed frame range moved"
        );
        assert_eq!(
            std::fs::read_to_string(&encode).expect("encode file"),
            "existing encode",
            "stale files are never touched"
        );
        if phase == 1 {
            return;
        }

        // A frame that would empty the scene is rejected and never saves.
        let rejected = condor_cmd(&temp)
            .args(["scenes", "0", "set", "start", "100"])
            .assert()
            .failure();
        assert!(
            stderr(rejected.get_output()).contains("would be empty"),
            "the error explains the frame"
        );
        assert_ne!(
            std::fs::read(&config_path).expect("config bytes"),
            original,
            "the earlier edit saved"
        );
        let unchanged = std::fs::read(&config_path).expect("config bytes");
        condor_cmd(&temp)
            .args(["scenes", "0", "set", "start", "100"])
            .assert()
            .failure();
        assert_eq!(
            std::fs::read(&config_path).expect("config bytes"),
            unchanged,
            "a rejected edit never saves"
        );
        if phase == 2 {
            return;
        }

        // A frame that reaches into a neighbour warns and names it.
        let overlapping =
            condor_cmd(&temp).args(["scenes", "1", "set", "start", "50"]).assert().success();
        let warnings = stderr(overlapping.get_output());
        assert!(
            warnings.contains("scene 1") && warnings.contains("overlaps scene 0"),
            "the overlap is named: {warnings}"
        );
        assert_eq!(
            ranges(&load(&config_path))[1],
            (50, 240),
            "the edit itself is accepted"
        );
        if phase == 3 {
            return;
        }

        // A write reports the scenes it addressed too, with the frames they
        // were left holding.
        let narrowed =
            condor_cmd(&temp).args(["scenes", "0", "set", "start", "60"]).assert().success();
        assert!(
            stderr(narrowed.get_output()).contains("Scenes: 1 Frames: 60-100"),
            "an edited selection reports itself, with the frames it was left holding: {}",
            stderr(narrowed.get_output())
        );

        // The reported span covers the frames the scenes actually describe,
        // even once edits leave the list unsorted: scene 0 now starts after
        // scene 1 and ends after the last scene does.
        condor_cmd(&temp).args(["scenes", "0", "set", "end", "900"]).assert().success();
        let stretched = condor_cmd(&temp).arg("scenes").assert().success();
        assert!(
            stderr(stretched.get_output()).contains("Scenes: 5 Frames: 50-900"),
            "an unindexed read reports the whole list, spanning past the last scene's end and \
             below the first scene's start: {}",
            stderr(stretched.get_output())
        );
    }

    fn check_scene_encoder_edit(phase: u8) {
        assert!((1..=6).contains(&phase));
        let temp = tempfile::tempdir().expect("temp directory");
        let config_path = scenes_config(&temp);
        let global = load(&config_path).condor.encoder;

        condor_cmd(&temp)
            .args(["scenes", "1", "set", "encoder", "x265"])
            .assert()
            .success();
        condor_cmd(&temp)
            .args(["scenes", "1", "set", "params", "crf=18"])
            .assert()
            .success();
        condor_cmd(&temp)
            .args(["scenes", "4", "set", "photon-noise", "800", "--chroma-noise", "800"])
            .assert()
            .success();
        condor_cmd(&temp).args(["scenes", "3", "set", "passes", "2"]).assert().failure();
        condor_cmd(&temp)
            .args(["scenes", "3", "set", "params", "--rc 1 --tbr 1000 --keyint 48"])
            .assert()
            .success();
        condor_cmd(&temp).args(["scenes", "3", "set", "passes", "2"]).assert().success();
        condor_cmd(&temp)
            .args(["scenes", "0", "set", "ffmpeg-filter", "hflip"])
            .assert()
            .success();
        condor_cmd(&temp)
            .args(["scenes", "2", "set", "sub-scenes", "240-300", "300-480"])
            .assert()
            .success();

        let configuration = load(&config_path);
        assert_eq!(
            configuration.condor.scenes[1].encoder.base().to_string(),
            "x265"
        );
        assert_eq!(
            configuration.condor.scenes[1]
                .encoder
                .parameters()
                .get("crf")
                .expect("crf merged"),
            &andean_condor::models::encoder::cli_parameter::CLIParameter::new_number("", "=", 18.0)
        );
        if phase == 1 {
            return;
        }
        let photon_noise = configuration.condor.scenes[4]
            .encoder
            .photon_noise()
            .expect("scene 4 carries Photon Noise");
        assert_eq!(photon_noise.iso, 800);
        assert_eq!(photon_noise.chroma_iso, Some(800));
        assert_eq!(
            configuration.condor.scenes[3].encoder.passes(),
            andean_condor::models::encoder::EncoderPasses::All(2)
        );
        assert_eq!(
            configuration.condor.scenes[0].encoder.ffmpeg_filter(),
            Some("hflip")
        );
        assert_eq!(
            configuration.condor.scenes[2].sub_scenes.as_ref().map(Vec::len),
            Some(2)
        );
        assert_eq!(
            configuration.condor.scenes[0].encoder.base(),
            global.base(),
            "a scene that only took a filtergraph keeps its encoder"
        );
        if phase == 2 {
            return;
        }

        // One line can switch an encoder and set it up at once.
        if phase != 3 {
            condor_cmd(&temp)
                .args([
                    "scenes",
                    "4",
                    "set",
                    "encoder",
                    "svt-av1",
                    "--passes",
                    "1",
                    "--params",
                    "crf=16",
                    "--iso",
                    "1200",
                    "--chroma-iso",
                    "1200",
                    "--ffmpeg-filter",
                    "crop=iw-16:ih-16",
                ])
                .assert()
                .success();
            let encoder = &load(&config_path).condor.scenes[4].encoder;
            assert_eq!(encoder.base().to_string(), "svt-av1");
            assert_eq!(
                encoder.passes(),
                andean_condor::models::encoder::EncoderPasses::All(1)
            );
            let photon_noise = encoder.photon_noise().expect("Photon Noise was set");
            assert_eq!(photon_noise.iso, 1200);
            assert_eq!(photon_noise.chroma_iso, Some(1200));
            assert_eq!(encoder.ffmpeg_filter(), Some("crop=iw-16:ih-16"));
            if phase == 4 {
                return;
            }

            // Unsupported totals fail for the whole selection without saving.
            let before = std::fs::read(&config_path).expect("snapshot");
            condor_cmd(&temp)
                .args(["scenes", "-1", "set", "passes", "4"])
                .assert()
                .failure();
            assert_eq!(std::fs::read(&config_path).expect("unchanged"), before);
            // A supported whole-selection edit reports the array.
            let all = condor_cmd(&temp)
                .args(["scenes", "-1", "set", "passes", "1"])
                .assert()
                .success();
            let printed = String::from_utf8_lossy(&all.get_output().stdout).into_owned();
            assert!(
                printed.contains("Scenes scenes 0-4 passes updated"),
                "a whole-selection change is reported once: {printed}"
            );
            assert!(
                !printed.contains('['),
                "the value waits for --verbose: {printed}"
            );

            // `--verbose` prints every addressed scene's new value.
            let all = condor_cmd(&temp)
                .args(["--verbose", "scenes", "-1", "set", "passes", "1"])
                .assert()
                .success();
            let printed = String::from_utf8_lossy(&all.get_output().stdout).into_owned();
            let (_, values) = printed.split_once('\n').expect("a value line");
            let values: Value = serde_json::from_str(values).expect("the reported value is JSON");
            assert_eq!(
                values.as_array().expect("five values").len(),
                5,
                "every addressed scene reports its new value: {printed}"
            );
            assert!(
                load(&config_path).condor.scenes.iter().all(|scene| scene.encoder.passes()
                    == andean_condor::models::encoder::EncoderPasses::All(1)),
                "the whole list took the pass count"
            );
            if phase == 5 {
                return;
            }

            // Capability errors name the encoder and never reach the file.
            let photon_noise = condor_cmd(&temp)
                .args(["scenes", "1", "set", "encoder", "x265", "--iso", "800"])
                .assert()
                .failure();
            assert!(
                stderr(photon_noise.get_output()).contains("does not support Photon Noise"),
                "x265 cannot carry Photon Noise"
            );
            let chroma = condor_cmd(&temp)
                .args(["scenes", "1", "set", "encoder", "svt-av1", "--chroma-iso", "200"])
                .assert()
                .failure();
            assert!(
                stderr(chroma.get_output())
                    .contains("chroma Photon Noise requires luma Photon Noise"),
                "a chroma ISO alone is rejected"
            );
        }
        let outside = condor_cmd(&temp)
            .args(["scenes", "2", "set", "sub-scenes", "0-999"])
            .assert()
            .failure();
        assert!(
            stderr(outside.get_output()).contains("lies outside scene"),
            "sub-scenes must stay inside their scene"
        );
    }

    fn check_scene_reset(phase: u8) {
        assert!((1..=4).contains(&phase));
        let temp = tempfile::tempdir().expect("temp directory");
        let config_path = scenes_config(&temp);
        let global = load(&config_path).condor.encoder;

        condor_cmd(&temp)
            .args(["scenes", "2", "set", "encoder", "x265"])
            .assert()
            .success();
        condor_cmd(&temp)
            .args(["scenes", "2", "set", "sub-scenes", "240-300"])
            .assert()
            .success();

        let encoder =
            condor_cmd(&temp).args(["scenes", "2", "reset", "encoder"]).assert().success();
        assert!(
            String::from_utf8_lossy(&encoder.get_output().stdout)
                .contains("Scenes scene 2 encoder reset to its default"),
            "the reset is reported"
        );
        let mut configuration = load(&config_path);
        assert_eq!(configuration.condor.scenes[2].encoder, global);

        condor_cmd(&temp)
            .args(["scenes", "2", "reset", "sub-scenes"])
            .assert()
            .success();
        configuration = load(&config_path);
        assert_eq!(configuration.condor.scenes[2].sub_scenes, None);
        if phase == 1 {
            return;
        }

        // A keyless reset restores the scene itself and names it once.
        let scene = condor_cmd(&temp).args(["scenes", "2", "reset"]).assert().success();
        let printed = String::from_utf8_lossy(&scene.get_output().stdout).into_owned();
        assert!(
            printed.contains("Scenes scene 2 reset to its default"),
            "the scene reset is named once: {printed}"
        );
        if phase == 2 {
            return;
        }

        let frames = condor_cmd(&temp).args(["scenes", "0", "reset", "frames"]).assert().failure();
        assert!(
            stderr(frames.get_output()).contains("has no default"),
            "frame ranges come from detection"
        );
        if phase == 3 {
            return;
        }

        let reset = condor_cmd(&temp).args(["scenes", "reset"]).assert().success();
        assert!(
            String::from_utf8_lossy(&reset.get_output().stdout).contains("av1an detect-scenes"),
            "the reset hints at rebuilding the scenes"
        );
        assert!(
            load(&config_path).condor.scenes.is_empty(),
            "the list is empty"
        );
    }

    fn check_scene_sequence_data(reset: bool) {
        let temp = tempfile::tempdir().expect("temp directory");
        let config_path = scenes_config(&temp);
        let mut configuration = load(&config_path);
        configuration.condor.scenes[2].sequence_data.parallel_encoder.bytes = Some(456);
        configuration.save(&config_path).expect("the config saves");

        // The saved step results are readable per scene and per selection.
        let one = condor_cmd(&temp)
            .args(["scenes", "2", "info", "parallel-encoder"])
            .assert()
            .success();
        assert_eq!(stdout_json(one.get_output())["bytes"], 456);

        let many = condor_cmd(&temp)
            .args(["scenes", "43-46", "info", "quality-check"])
            .assert()
            .failure();
        assert!(
            stderr(many.get_output()).contains("scene index 43 does not exist"),
            "a selection can be out of range: {}",
            stderr(many.get_output())
        );
        let pair = condor_cmd(&temp)
            .args(["scenes", "1-2", "info", "quality-check"])
            .assert()
            .success();
        assert_eq!(
            stdout_json(pair.get_output()).as_array().expect("two scenes").len(),
            2
        );
        if !reset {
            return;
        }

        // A reset clears the saved results of every addressed scene.
        condor_cmd(&temp)
            .args(["scenes", "-1", "reset", "target-quality"])
            .assert()
            .success();
        let reset = condor_cmd(&temp)
            .args(["scenes", "-1", "reset", "parallel-encoder"])
            .assert()
            .success();
        assert!(
            String::from_utf8_lossy(&reset.get_output().stdout)
                .contains("Scenes scenes 0-4 parallel-encoder reset to its default"),
            "a whole-list reset is reported once: {}",
            String::from_utf8_lossy(&reset.get_output().stdout)
        );
        let configuration = load(&config_path);
        assert!(configuration.condor.scenes.iter().all(|scene| {
            scene.sequence_data.parallel_encoder.bytes.is_none()
                && scene.sequence_data.target_quality.passes.is_empty()
        }));
        assert_eq!(
            configuration.condor.scenes[2].start_frame, 240,
            "resetting results leaves the frame range alone"
        );
    }

    fn check_scene_list_edit(phase: u8) {
        assert!((1..=4).contains(&phase));
        let temp = tempfile::tempdir().expect("temp directory");
        let config_path = scenes_config(&temp);

        let deleted = condor_cmd(&temp).args(["scenes", "1,3", "delete"]).assert().success();
        assert!(
            String::from_utf8_lossy(&deleted.get_output().stdout)
                .contains("Deleted scenes 1,3: 121-240, 480-528"),
            "the deletion names the scenes and the frames they held: {}",
            String::from_utf8_lossy(&deleted.get_output().stdout)
        );
        assert!(
            stderr(deleted.get_output()).contains("Scenes: 2 Frames: 121-528"),
            "the summary covers the scenes that were removed: {}",
            stderr(deleted.get_output())
        );
        assert_eq!(
            ranges(&load(&config_path)),
            vec![(0, 121), (240, 480), (528, 720)],
            "the remaining scenes keep their order"
        );
        if phase == 1 {
            return;
        }

        // A new scene sorts by start frame unless told where to go.
        let added = condor_cmd(&temp).args(["scenes", "add", "240", "400"]).assert().success();
        assert!(
            String::from_utf8_lossy(&added.get_output().stdout).contains("Added scene 1: 240-400"),
            "the addition names the scene and the frames it covers: {}",
            String::from_utf8_lossy(&added.get_output().stdout)
        );
        assert!(
            stderr(added.get_output()).contains("Scenes: 1 Frames: 240-400"),
            "the summary covers the scene that was added: {}",
            stderr(added.get_output())
        );
        assert_eq!(
            ranges(&load(&config_path)),
            vec![(0, 121), (240, 400), (240, 480), (528, 720)],
            "an equal start goes before the existing scene"
        );
        if phase == 2 {
            return;
        }

        condor_cmd(&temp)
            .args(["scenes", "add", "900", "1000", "--append"])
            .assert()
            .success();
        condor_cmd(&temp)
            .args(["scenes", "add", "5", "10", "--prepend"])
            .assert()
            .success();
        condor_cmd(&temp)
            .args(["scenes", "add", "700", "800", "--after", "1"])
            .assert()
            .success();
        assert_eq!(
            ranges(&load(&config_path)),
            vec![(5, 10), (0, 121), (700, 800), (240, 400), (240, 480), (528, 720), (900, 1000),],
            "every placement landed where it was asked to"
        );
        if phase == 3 {
            return;
        }

        let empty = condor_cmd(&temp).args(["scenes", "add", "10", "10"]).assert().failure();
        assert!(
            stderr(empty.get_output()).contains("is empty"),
            "an empty range is rejected"
        );
        let indexed = condor_cmd(&temp).args(["scenes", "1", "add", "10", "20"]).assert().failure();
        assert!(
            stderr(indexed.get_output()).contains("takes no index"),
            "`add` takes no index"
        );
    }

    #[serial]
    #[test]
    fn scenes_bare_and_info_report_saved_results_subscenes_and_list_summary() {
        check_scene_info(1);
    }

    #[serial]
    #[test]
    fn scenes_indexed_info_reports_selected_frames_or_subscenes_and_summary() {
        check_scene_info(2);
    }

    #[serial]
    #[test]
    fn scenes_multi_index_info_prints_array_and_selected_span() {
        check_scene_info(3);
    }

    #[serial]
    #[test]
    fn scenes_info_invalid_or_missing_index_reports_lowest_missing_index() {
        check_scene_info(4);
    }

    #[serial]
    #[test]
    fn scene_frame_edit_saves_selected_range_and_warns_without_touching_stale_chunk() {
        check_scene_frame_edit(1);
    }

    #[serial]
    #[test]
    fn scene_frame_edit_that_empties_scene_refuses_without_save() {
        check_scene_frame_edit(2);
    }

    #[serial]
    #[test]
    fn scene_frame_edit_accepts_overlap_and_names_neighbor_in_warning() {
        check_scene_frame_edit(3);
    }

    #[serial]
    #[test]
    fn scene_selection_summary_uses_actual_extrema_even_when_scene_list_is_unsorted() {
        check_scene_frame_edit(4);
    }

    #[serial]
    #[test]
    fn scene_encoder_and_parameter_crud_saves_selected_x265_crf() {
        check_scene_encoder_edit(1);
    }

    #[serial]
    #[test]
    fn scene_photon_noise_and_filtergraph_crud_save_supported_values() {
        check_scene_encoder_edit(2);
    }

    #[serial]
    #[test]
    fn scene_subscene_crud_saves_two_subranges_and_rejects_outside_range() {
        check_scene_encoder_edit(3);
    }

    #[serial]
    #[test]
    fn scene_encoder_combined_set_saves_passes_noise_and_filtergraph() {
        check_scene_encoder_edit(4);
    }

    #[serial]
    #[test]
    fn scene_passes_crud_rejects_unsupported_selection_atomically_and_reports_all_values_when_verbose()
     {
        check_scene_encoder_edit(5);
    }

    #[serial]
    #[test]
    fn scene_encoder_capability_errors_name_unsupported_noise_or_missing_luma() {
        check_scene_encoder_edit(6);
    }

    #[serial]
    #[test]
    fn scene_encoder_reset_restores_global_encoder_and_subscene_reset_clears_ranges() {
        check_scene_reset(1);
    }

    #[serial]
    #[test]
    fn scene_keyless_reset_reports_selected_scene() {
        check_scene_reset(2);
    }

    #[serial]
    #[test]
    fn scene_frame_reset_refuses_because_detection_range_has_no_default() {
        check_scene_reset(3);
    }

    #[serial]
    #[test]
    fn scenes_list_reset_clears_list_and_advises_redetection() {
        check_scene_reset(4);
    }

    #[serial]
    #[test]
    fn scene_sequence_info_reports_selected_saved_results_or_missing_index() {
        check_scene_sequence_data(false);
    }

    #[serial]
    #[test]
    fn scene_sequence_resets_clear_selected_results_without_changing_frame_boundaries() {
        check_scene_sequence_data(true);
    }

    #[serial]
    #[test]
    fn scenes_delete_removes_selected_indices_preserving_peer_order_and_reports_removed_span() {
        check_scene_list_edit(1);
    }

    #[serial]
    #[test]
    fn scenes_add_sorts_before_equal_start_and_reports_inserted_span() {
        check_scene_list_edit(2);
    }

    #[serial]
    #[test]
    fn scenes_add_explicit_append_prepend_or_after_position_is_honored() {
        check_scene_list_edit(3);
    }

    #[serial]
    #[test]
    fn scenes_add_rejects_empty_range_or_selection_index() {
        check_scene_list_edit(4);
    }

    #[serial]
    #[test]
    fn scenes_set_and_delete_need_an_index() {
        let temp = tempfile::tempdir().expect("temp directory");
        let config_path = scenes_config(&temp);
        let before = std::fs::read(&config_path).expect("config bytes");

        let set = condor_cmd(&temp).args(["scenes", "set", "start", "23"]).assert().failure();
        assert!(
            stderr(set.get_output()).contains("which scenes?"),
            "set names the missing index: {}",
            stderr(set.get_output())
        );
        let deleted = condor_cmd(&temp).args(["scenes", "delete"]).assert().failure();
        assert!(
            stderr(deleted.get_output()).contains("which scenes?"),
            "delete names the missing index: {}",
            stderr(deleted.get_output())
        );
        assert_eq!(
            std::fs::read(&config_path).expect("config bytes"),
            before,
            "nothing was written"
        );
    }
}

#[cfg(test)]
mod scene_encoders {
    use crate::common::condor_cmd;
    use andean_condor::models::{
        encoder::{Encoder, EncoderBase, photon_noise::PhotonNoise},
        input::{Input, VapourSynthScriptSource},
        sequence::target_quality::types::QualityPass,
    };
    use california_condor::{
        configuration::Configuration,
        test_helpers::{TestVideo, default_config},
    };
    use std::{fs, process::Output, time::Duration};

    fn fixture(temp: &tempfile::TempDir, saved_scenes: bool) -> Configuration {
        let source = temp.path().join("inert.vpy");
        fs::write(&source, "from vapoursynth import core, YUV420P8\ncore.std.BlankClip(width=64,height=64,length=4,format=YUV420P8).set_output()\n").expect("inert source");
        let video = TestVideo {
            path: source.clone(),
            width: 64,
            height: 64,
            frames: 4,
            fps_rational: (24, 1),
            format: andean_condor::ffmpeg::FFPixelFormat::YUV420P,
            scenes: vec![(0, 2), (2, 4)],
        };
        let mut config = default_config(&video, &temp.path().join("out.mkv"), temp.path());
        config.condor.input = Input::VapourSynthScript {
            source: VapourSynthScriptSource::Path(source),
            variables: Default::default(),
            index: 0,
            filters: vec![],
            stream_concurrently: true,
        };
        config.condor.encoder = Encoder::default_from_base(&EncoderBase::AOM, false);
        config.condor.encoder.set_photon_noise(Some(PhotonNoise {
            iso: 100,
            chroma_iso: None,
            width: None,
            height: None,
            c_y: None,
            ccb: None,
            ccr: None,
        }));
        if saved_scenes {
            config.condor.scenes =
                video.mock_scenes(&Encoder::default_from_base(&EncoderBase::X264, false));
            config.condor.scenes[0].sub_scenes = Some(vec![
                andean_condor::models::scene::SubScene {
                    start_frame: 0,
                    end_frame: 1,
                },
                andean_condor::models::scene::SubScene {
                    start_frame: 1,
                    end_frame: 2,
                },
            ]);
            config.condor.scenes[0].sequence_data.parallel_encoder.bytes = Some(123);
            config.condor.scenes[0].sequence_data.target_quality.passes.push(QualityPass {
                provenance: None,
                backend: None,
                quantizer: 17.0,
                scores: vec![50.0],
                bitrate: 1000.0,
                started_on: 100,
                completed_on: 200,
            });
            // These tests require current saved work so execution can reach the
            // global-flag handler; C14 owns the intentionally unstamped fixtures.
            use andean_condor::models::sequence::{
                provenance::{self, MetricBackend, ResultProvenance},
                target_quality::{
                    TargetQualityConfig,
                    types::{ProbeStrategy, QualityMetric, SubsetProbeLength, SubsetProbePosition},
                },
            };
            config.condor.sequence_config.target_quality = Some(TargetQualityConfig {
                metric: QualityMetric::VMAF {
                    target_range: (50.0, 50.0),
                    resolution: None,
                    scaler: "bicubic".into(),
                    threads: 0,
                    model: None,
                    features: vec![],
                },
                ..Default::default()
            });
            let tq = config.condor.sequence_config.target_quality.as_mut().expect("TQ");
            tq.probing.strategy = ProbeStrategy::Subset {
                position: SubsetProbePosition::Start,
                length: SubsetProbeLength::Frames(1),
            };
            let pe = &config.condor.sequence_config.parallel_encoder;
            let input = provenance::probe_input(&config.condor.input, pe, Some(tq));
            let scene = &mut config.condor.scenes[0];
            scene.encoder.set_quantizer(17.0);
            let mut pass = scene.sequence_data.target_quality.passes[0].clone();
            pass.backend = Some(MetricBackend::current(andean_condor::metrics::Engine::Vmaf));
            pass.provenance = Some(ResultProvenance::new(
                provenance::Admission::default()
                    .target_context(input, tq.metric_input.as_ref(), scene, tq, &pass)
                    .expect("explicit current probe fixture"),
            ));
            scene.sequence_data.target_quality.passes[0] = pass;
            scene.sequence_data.target_quality.selection = Some(ResultProvenance::new(
                provenance::selection_context(scene, tq)
                    .expect("explicit current selection fixture"),
            ));
            fs::create_dir_all(&pe.scenes_directory).expect("fixture chunks");
            let chunk = pe.scenes_directory.join("00000.264");
            fs::write(&chunk, [0_u8; 123])
                .expect("synthetic current chunk, not native integrity acceptance");
            let context = provenance::Admission::default()
                .encode_context(
                    pe.input.as_ref().unwrap_or(&config.condor.input),
                    input,
                    tq.metric_input.as_ref(),
                    scene,
                    Some(tq),
                    &chunk,
                    0,
                )
                .expect("explicit current encode fixture");
            scene.sequence_data.parallel_encoder.provenance = Some(ResultProvenance::new(context));
            scene.sequence_data.parallel_encoder.started_on = Some(100);
            scene.sequence_data.parallel_encoder.completed_on = Some(200);
        }
        config.save(&temp.path().join("condor.json")).expect("save fixture");
        config
    }

    fn run(temp: &tempfile::TempDir, args: &[&str]) -> Output {
        condor_cmd(temp)
            .env("PATH", "")
            .args(args)
            .timeout(Duration::from_secs(10))
            .output()
            .expect("CLI output")
    }

    fn load(temp: &tempfile::TempDir) -> Configuration {
        Configuration::load(&temp.path().join("condor.json"))
            .expect("load")
            .expect("saved configuration")
    }

    fn warning(output: &Output, expected: bool, args: &[&str]) {
        let stderr = String::from_utf8_lossy(&output.stderr).to_lowercase();
        assert_eq!(
            stderr.contains("existing scenes unchanged"),
            expected,
            "{args:?}: {stderr}"
        );
        if expected {
            assert!(stderr.contains("future scenes"), "{stderr}");
            assert!(
                stderr.contains("av1an scenes all reset encoder"),
                "{stderr}"
            );
        }
    }

    // Removing a flag-presence check or propagating global settings breaks this
    // real CLI/save oracle; an empty PATH stops native encoder resolution later.
    #[test]
    fn global_flags_warn_and_preserve_saved_scene_json() {
        let temp = tempfile::tempdir().expect("fixture");
        let config = fixture(&temp, true);
        for command in [vec![], vec!["encode"]] {
            for flags in [
                vec!["--encoder", "rav1e"],
                vec!["--params", "--cpu-used=7"],
                vec!["--passes", "1"],
                vec!["--photon-noise", "800"],
                vec!["--chroma-noise", "100"],
                vec!["--ffmpeg-filter", "hflip"],
            ] {
                config.save(&temp.path().join("condor.json")).expect("restore");
                let args = [command.clone(), flags].concat();
                let output = run(&temp, &args);
                assert!(
                    !output.status.success(),
                    "empty PATH must prevent native encoding"
                );
                let saved = load(&temp);
                assert_ne!(
                    serde_json::to_value(&saved.condor.encoder).unwrap(),
                    serde_json::to_value(&config.condor.encoder).unwrap(),
                    "handler must save the requested global edit before native failure: {args:?}: {output:?}"
                );
                assert_eq!(
                    serde_json::to_value(&saved.condor.scenes).unwrap(),
                    serde_json::to_value(&config.condor.scenes).unwrap(),
                    "{args:?} must not change scene overrides, boundaries or history"
                );
                warning(&output, true, &args);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let warning_at = stderr.find("Existing scenes unchanged").unwrap();
                let failure_at = stderr.find("Error:").unwrap_or_else(|| {
                    panic!("expected the later native failure: {args:?}: {stderr}")
                });
                assert!(
                    warning_at < failure_at,
                    "warning must precede native failure: {stderr}"
                );
            }
        }
        config.save(&temp.path().join("condor.json")).expect("restore");
        let output = run(&temp, &["--disable-color-data"]);
        assert!(!load(&temp).condor.encoder.append_color_data());
        assert_eq!(
            serde_json::to_value(&load(&temp).condor.scenes).unwrap(),
            serde_json::to_value(&config.condor.scenes).unwrap()
        );
        warning(&output, true, &["--disable-color-data"]);
        config.save(&temp.path().join("condor.json")).expect("restore");
        warning(
            &run(&temp, &["encode", "--passes", "2"]),
            true,
            &["encode", "--passes", "2"],
        );
    }

    fn check_scene_warning_absence(phase: u8) {
        assert!((1..=3).contains(&phase));
        let temp = tempfile::tempdir().expect("fixture");
        let config = fixture(&temp, true);
        for args in [
            vec![],
            vec!["encode"],
            vec!["--workers", "2"],
            vec!["encode", "--workers", "2"],
            vec!["--max-tries", "2"],
            vec!["encode", "--max-tries", "2"],
            vec!["--target", "50"],
            vec!["--target-profile", "fast"],
            vec!["--tq-params", "--cpu-used=7"],
            vec!["encode", "--filters", "resize:format=yuv420p"],
            vec!["encode", "info"],
            vec!["encode", "set", "workers", "2"],
        ] {
            config.save(&temp.path().join("condor.json")).expect("restore");
            let output = run(&temp, &args);
            assert!(
                !["unexpected argument", "invalid value", "unrecognized subcommand"]
                    .iter()
                    .any(|error| String::from_utf8_lossy(&output.stderr).contains(error)),
                "test must reach the owning route: {args:?}: {output:?}"
            );
            warning(&output, false, &args);
            assert_eq!(
                load(&temp).condor.encoder,
                config.condor.encoder,
                "{args:?}"
            );
        }
        if phase == 1 {
            return;
        }
        for args in [
            vec!["--params", "--cq-level=NaN"],
            vec!["encode", "--passes", "3"],
            vec!["--encoder", "x264", "--photon-noise", "100"],
            vec!["encode", "set", "passes", "3"],
            vec!["init", "set", "encoder", "--encoder", "x264", "--photon-noise", "100"],
        ] {
            config.save(&temp.path().join("condor.json")).expect("restore");
            let before = fs::read(temp.path().join("condor.json")).unwrap();
            let output = run(&temp, &args);
            assert!(!output.status.success());
            assert_eq!(fs::read(temp.path().join("condor.json")).unwrap(), before);
            warning(&output, false, &args);
        }
        if phase == 2 {
            return;
        }
        let empty = fixture(&temp, false);
        for args in [
            vec!["--passes", "1"],
            vec!["encode", "--passes", "1"],
            vec!["encode", "set", "passes", "1"],
        ] {
            empty.save(&temp.path().join("condor.json")).expect("restore");
            warning(&run(&temp, &args), false, &args);
        }
    }

    fn check_global_encoder_guidance(reset: bool) {
        let temp = tempfile::tempdir().expect("fixture");
        fixture(&temp, true);
        let scene_edit = run(&temp, &["scenes", "0", "set", "params", "--crf=17"]);
        assert!(scene_edit.status.success(), "{scene_edit:?}");
        let original = load(&temp);
        let chunk = temp.path().join("scenes/00000.mkv");
        fs::create_dir_all(chunk.parent().unwrap()).unwrap();
        fs::write(&chunk, b"historical chunk").unwrap();
        for args in [
            vec!["encode", "set", "encoder", "rav1e"],
            vec!["encode", "set", "params", "--cpu-used=7"],
            vec!["encode", "set", "passes", "1"],
            vec!["encode", "set", "executable", "/missing/scene-aomenc"],
            vec!["encode", "set", "photon-noise", "800", "--chroma-noise", "800"],
            vec!["encode", "set", "ffmpeg-filter", "hflip"],
            vec!["encode", "set", "append-color-data", "off"],
            vec!["encode", "reset", "encoder"],
            vec!["encode", "reset", "executable"],
            vec!["encode", "reset", "passes"],
            vec!["encode", "reset", "params"],
            vec!["encode", "reset", "photon-noise"],
            vec!["encode", "reset", "ffmpeg-filter"],
            vec!["encode", "reset", "append-color-data"],
            vec!["encode", "reset"],
            vec!["init", "set", "encoder", "--encoder", "rav1e"],
            vec!["init", "reset", "encoder"],
        ] {
            original.save(&temp.path().join("condor.json")).expect("restore");
            let output = run(&temp, &args);
            assert!(output.status.success(), "{args:?}: {output:?}");
            assert_eq!(
                serde_json::to_value(&load(&temp).condor.scenes).unwrap(),
                serde_json::to_value(&original.condor.scenes).unwrap()
            );
            warning(&output, true, &args);
        }
        if !reset {
            return;
        }
        let before_reset = load(&temp);
        let output = run(&temp, &["scenes", "all", "reset", "encoder"]);
        assert!(output.status.success(), "{output:?}");
        warning(&output, false, &["scenes", "all", "reset", "encoder"]);
        let saved = load(&temp);
        for (before, after) in before_reset.condor.scenes.iter().zip(&saved.condor.scenes) {
            assert_eq!(after.encoder, saved.condor.encoder);
            let mut expected = serde_json::to_value(before).unwrap();
            expected["encoder"] = serde_json::to_value(&saved.condor.encoder).unwrap();
            assert_eq!(serde_json::to_value(after).unwrap(), expected);
        }
        assert_eq!(fs::read(chunk).unwrap(), b"historical chunk");
    }

    #[test]
    fn non_encoder_routes_with_saved_scenes_do_not_warn_or_change_global_encoder() {
        check_scene_warning_absence(1);
    }

    #[test]
    fn invalid_global_encoder_edits_do_not_warn_or_change_saved_bytes() {
        check_scene_warning_absence(2);
    }

    #[test]
    fn global_encoder_edits_without_saved_scenes_do_not_emit_scene_preservation_warning() {
        check_scene_warning_absence(3);
    }

    #[test]
    fn global_encoder_crud_warns_but_preserves_all_saved_scene_metadata() {
        check_global_encoder_guidance(false);
    }

    #[test]
    fn explicit_scene_encoder_reset_applies_global_encoder_without_changing_other_scene_data_or_files()
     {
        check_global_encoder_guidance(true);
    }

    #[test]
    fn probe_params_long_help_explains_existing_scene_probe_overrides_not_global_reset() {
        let temp = tempfile::tempdir().expect("help working directory");
        for args in [
            vec!["target-quality", "--help", "--verbose"],
            vec!["encode", "--help", "--verbose"],
            vec!["--help", "--verbose"],
        ] {
            let output = run(&temp, &args);
            assert!(output.status.success(), "{args:?}: {output:?}");
            let stdout = String::from_utf8(output.stdout).expect("help text");
            let help = stdout.split_whitespace().collect::<Vec<_>>().join(" ");
            if args[0] == "target-quality" {
                assert!(
                    !help.contains("av1an scenes all reset encoder"),
                    "probe options apply to existing scenes: {help}"
                );
                assert!(!help.contains("future scenes"), "{help}");
                assert!(
                    help.contains("Replaces each scene's encoder options during probes"),
                    "{help}"
                );
                assert!(
                    help.contains("This override does not replace the final scene encoder options"),
                    "{help}"
                );
                assert!(help.contains("psychovisual"), "{help}");
                assert!(
                    help.contains("--params \"--preset 8 --crf 30\""),
                    "examples use the subcommand's flag: {help}"
                );
                assert!(
                    !help.contains("--tq-params"),
                    "examples must use this subcommand's flag: {help}"
                );
            } else {
                assert!(
                    help.contains("av1an scenes all reset encoder"),
                    "global encoder guidance must remain: {help}"
                );
                assert!(
                    help.contains("Global parameter edits leave existing scenes unchanged"),
                    "{help}"
                );
            }
        }
    }
}
