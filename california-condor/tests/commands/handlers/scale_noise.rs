use std::time::SystemTime;

use crate::common::condor_cmd;
use crate::common::config::{init_config, load};
use andean_condor::models::{
    encoder::photon_noise::PhotonNoise,
    sequence::{noise_detector::NoiseDetectorData, noise_scaler::NoiseScalerConfig},
};
use california_condor::{
    commands::handlers::load_configuration, test_helpers::*, utils::hash_path::hash_path,
};

#[cfg(test)]
mod handler_configuration {
    use super::*;
    use california_condor::commands::handlers::{
        init::init_handler, scale_noise::scale_noise_handler,
    };

    #[test]
    fn scale_noise_handler_applies_threshold_bounds_and_chroma_policy_and_keeps_scenes_nonempty() {
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
        expected_config.condor.scenes = test_video.mock_scenes(&expected_config.condor.encoder);
        expected_config.condor.sequence_config.noise_scaler = Some(NoiseScalerConfig {
            threshold: 0.002,
            minimum_scaler: 4.0,
            maximum_scaler: 12.0,
            scale_chroma: false,
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
        )
        .expect("init_handler should succeed");
        let (mut config, _) =
            load_configuration(Some(&config_path)).expect("load_config should succeed");
        config.condor.scenes = test_video.mock_scenes(&config.condor.encoder);
        config.save(&config_path).expect("save should succeed");
        let (config, found_config_path) = scale_noise_handler(
            Some(&config_path),
            Some(0.002),
            Some(4.0),
            Some(12.0),
            false,
        )
        .expect("scale_scenes_handler should succeed");
        assert_eq!(
            found_config_path,
            config_path_abs,
            "config path is {}",
            config_path_abs.display()
        );
        check_basic_config(&config, &expected_config);
        assert!(!config.condor.scenes.is_empty(), "scenes is not empty");
    }
}

#[cfg(test)]
mod tests {
    use andean_condor::models::sequence::noise_detector::NoiseDetectorConfig;
    use serial_test::serial;

    use super::*;

    fn noise_scaler_configuration(
        temp: &std::path::Path,
    ) -> california_condor::configuration::Configuration {
        let video = TestVideo {
            path: temp.join("must-not-open.mkv"),
            width: 2,
            height: 2,
            frames: 2,
            fps_rational: (24, 1),
            format: andean_condor::ffmpeg::FFPixelFormat::YUV420P,
            scenes: vec![(0, 2)],
        };
        let mut configuration =
            default_config(&video, &temp.join("out.mkv"), &temp.join("scratch"));
        configuration.condor.encoder.set_executable(Some(temp.join("must-not-run")));
        configuration.condor.scenes = video.mock_scenes(&configuration.condor.encoder);
        configuration.condor.sequence_config.noise_scaler = Some(NoiseScalerConfig {
            threshold: 0.2,
            minimum_scaler: 0.5,
            maximum_scaler: 1.5,
            scale_chroma: true,
        });
        configuration
    }

    #[serial]
    #[test]
    fn owning_noise_scaler_scene_edit_recovery_never_scales_twice() {
        let video = get_test_video();
        let temp = tempfile::tempdir().expect("fixture");
        let path = temp.path().join("condor.json");
        let mut config = default_config(
            &video,
            &temp.path().join("out.mkv"),
            &temp.path().join("work"),
        );
        config.condor.encoder.set_photon_noise(Some(PhotonNoise {
            iso: 1003,
            chroma_iso: Some(1003),
            width: None,
            height: None,
            c_y: None,
            ccb: None,
            ccr: None,
        }));
        config.condor.scenes = video.mock_scenes(&config.condor.encoder);
        for scene in &mut config.condor.scenes {
            scene.sequence_data.noise_detection = Some(NoiseDetectorData {
                noise: 0.6,
                luminance: 0.3,
                created_on: SystemTime::UNIX_EPOCH,
            });
        }
        config.save(&path).expect("original");
        condor_cmd(&temp)
            .args(["zone", "0 121 svt-av1 \"--preset 3\" --iso 801 --chroma-iso 801"])
            .assert()
            .success();
        // Zone edits retain valid range measurements; seed only if exercising the old bug.
        let mut measured = load(&path);
        for scene in &mut measured.condor.scenes {
            scene.sequence_data.noise_detection = Some(NoiseDetectorData {
                noise: 0.6,
                luminance: 0.3,
                created_on: SystemTime::UNIX_EPOCH,
            });
            scene.sequence_data.parallel_encoder = Default::default();
        }
        measured.save(&path).expect("fixed detector inputs");
        condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .args([
                "scale-noise",
                "--threshold",
                "0.2",
                "--minimum-scaler",
                "0.5",
                "--maximum-scaler",
                "0.5",
                "--scale-chroma",
            ])
            .assert()
            .success();
        let scaled = load(&path);
        assert_eq!(
            scaled.condor.scenes[0].encoder.photon_noise().expect("scaled").iso,
            401
        );
        assert_eq!(
            scaled.condor.scenes[0].encoder.photon_noise().expect("scaled").chroma_iso,
            Some(401)
        );
        let peer = serde_json::to_value(&scaled.condor.scenes[1]).expect("peer");
        for (edit, range_change) in [
            (vec!["scenes", "0", "set", "params", "crf=19"], false),
            (vec!["scenes", "0", "set", "end", "100"], true),
        ] {
            scaled.save(&path).expect("restore actually scaled job");
            condor_cmd(&temp).args(edit).assert().success();
            let edited = load(&path);
            if range_change {
                assert_eq!(
                    edited.condor.scenes[0]
                        .encoder
                        .photon_noise()
                        .expect("original ISO restored")
                        .iso,
                    801
                );
                assert!(edited.condor.scenes[0].sequence_data.noise_detection.is_none());
            }
            // Controlled same-job recovery uses explicit records and no encoder reset.
            condor_cmd(&temp)
                .args(["scenes", "0", "reset", "parallel-encoder"])
                .assert()
                .success();
            let mut recovered = load(&path);
            recovered.condor.scenes[0].sequence_data.noise_detection = Some(NoiseDetectorData {
                noise: 0.6,
                luminance: 0.3,
                created_on: SystemTime::UNIX_EPOCH,
            });
            recovered.save(&path).expect("measurement for retained/new range");
            condor_cmd(&temp)
                .env("CONDOR_TEST_MODE", "1")
                .arg("scale-noise")
                .assert()
                .success();
            let resumed = load(&path);
            assert_eq!(
                resumed.condor.scenes[0].encoder.photon_noise().expect("not scaled twice").iso,
                401,
                "an already-scaled ISO must not become 201 after edit/recovery"
            );
            assert_eq!(
                resumed.condor.scenes[0].encoder.photon_noise().expect("chroma").chroma_iso,
                Some(401)
            );
            assert_eq!(
                resumed.condor.scenes[0].encoder.parameters()["preset"].to_string_value(),
                "3"
            );
            assert_eq!(
                serde_json::to_value(&resumed.condor.scenes[1]).expect("peer"),
                peer
            );
            if !range_change {
                assert_eq!(
                    serde_json::to_value(&edited.condor.scenes[0].sequence_data.noise_detection)
                        .expect("retained detection"),
                    serde_json::to_value(&scaled.condor.scenes[0].sequence_data.noise_detection)
                        .expect("original detection")
                );
                assert_eq!(
                    serde_json::to_value(&edited.condor.scenes[0].sequence_data.noise_scaling)
                        .expect("retained scaling"),
                    serde_json::to_value(&scaled.condor.scenes[0].sequence_data.noise_scaling)
                        .expect("original scaling")
                );
                assert!(
                    !edited.condor.scenes[0].sequence_data.parallel_encoder.invalidated,
                    "valid noise is not a withdrawn claim"
                );
                assert_eq!(
                    resumed.condor.scenes[0].encoder.parameters()["crf"].to_string_value(),
                    "19"
                );
            }
        }
        // Legacy factor-only records cannot prove the original rounded ISO.
        let mut legacy_json = serde_json::to_value(&scaled).expect("legacy fixture");
        for scene in legacy_json["condor"]["scenes"].as_array_mut().expect("scenes") {
            scene["sequence_data"]["noise_scaling"]
                .as_object_mut()
                .expect("scaling")
                .remove("baseline");
        }
        let legacy: california_condor::configuration::Configuration =
            serde_json::from_value(legacy_json).expect("old JSON still parses");
        for edit in [
            vec!["scenes", "0", "set", "end", "100"],
            vec!["scenes", "0", "reset", "noise-scaling"],
        ] {
            legacy.save(&path).expect("legacy record");
            let json = std::fs::read(&path).expect("before refusal");
            let refused = condor_cmd(&temp).args(edit).assert().failure();
            assert!(
                String::from_utf8_lossy(&refused.get_output().stderr)
                    .contains("original PhotonNoise baseline")
            );
            assert_eq!(std::fs::read(&path).expect("legacy preserved"), json);
        }
        // A PhotonNoise edit must not ambiguously mix new ISO with scaled chroma.
        scaled.save(&path).expect("known producer record");
        let json = std::fs::read(&path).expect("before conflicting edit");
        condor_cmd(&temp)
            .args(["scenes", "0", "set", "photon-noise", "803"])
            .assert()
            .failure();
        assert_eq!(std::fs::read(&path).expect("conflict preserved"), json);
        condor_cmd(&temp)
            .args(["scenes", "0", "reset", "noise-scaling"])
            .assert()
            .success();
        let reset = load(&path);
        assert_eq!(
            reset.condor.scenes[0].encoder.photon_noise().expect("exact original").iso,
            801
        );
        assert_eq!(
            reset.condor.scenes[0]
                .encoder
                .photon_noise()
                .expect("original chroma")
                .chroma_iso,
            Some(801)
        );
        assert!(reset.condor.scenes[0].sequence_data.noise_scaling.is_none());
        assert_eq!(
            reset.condor.scenes[0].encoder.parameters()["preset"].to_string_value(),
            "3"
        );
        condor_cmd(&temp)
            .args(["scenes", "0", "set", "photon-noise", "803", "--chroma-noise", "803"])
            .assert()
            .success();
        condor_cmd(&temp)
            .args(["scenes", "0", "reset", "parallel-encoder"])
            .assert()
            .success();
        condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .arg("scale-noise")
            .assert()
            .success();
        assert_eq!(
            load(&path).condor.scenes[0].encoder.photon_noise().expect("new scale once").iso,
            402
        );
        // Global maximum, calibration and detector/source context are dependencies,
        // unlike CRF/preset options. Known-invalid scaling must be withdrawn.
        for dependency in ["maximum", "calibration", "detector"] {
            let mut changed = scaled.clone();
            match dependency {
                "maximum" => {
                    changed.condor.scenes[1]
                        .sequence_data
                        .noise_detection
                        .as_mut()
                        .expect("noise")
                        .noise = 1.0;
                },
                "calibration" => {
                    changed
                        .condor
                        .sequence_config
                        .noise_scaler
                        .as_mut()
                        .expect("config")
                        .maximum_scaler = 0.75;
                },
                _ => {
                    changed.condor.sequence_config.noise_detector = Some(NoiseDetectorConfig {
                        input: Some(changed.condor.input.clone()),
                        ..Default::default()
                    });
                },
            }
            changed.save(&path).expect("dependency edit");
            let mut direct = changed
                .instantiate_condor(Box::new(|_| {
                    anyhow::bail!("changed dependencies must refuse before saving")
                }))
                .expect("direct producer");
            let before = serde_json::to_value(direct.as_data()).expect("before direct refusal");
            let (tx, rx) = std::sync::mpsc::channel();
            let error = andean_condor::core::sequence::Sequence::execute(
                &mut andean_condor::core::sequence::noise_scaler::NoiseScaler::default(),
                &mut direct,
                tx,
                std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            )
            .expect_err("known changed dependencies cannot skip as already scaled");
            assert!(format!("{error:#}").contains("noise scaling dependencies changed"));
            assert_eq!(
                serde_json::to_value(direct.as_data()).expect("direct rollback"),
                before
            );
            assert!(rx.try_iter().next().is_none());
            condor_cmd(&temp)
                .args(["scenes", "0", "set", "params", "crf=19"])
                .assert()
                .success();
            let withdrawn = load(&path);
            assert_eq!(
                withdrawn.condor.scenes[0].encoder.photon_noise().expect("proven original").iso,
                801,
                "{dependency}"
            );
            assert!(
                withdrawn.condor.scenes.iter().all(|s| s.sequence_data.noise_scaling.is_none())
            );
            assert!(
                withdrawn
                    .condor
                    .scenes
                    .iter()
                    .all(|s| s.sequence_data.parallel_encoder.invalidated)
            );
            assert_eq!(
                withdrawn.condor.scenes[0].encoder.parameters()["preset"].to_string_value(),
                "3"
            );
            if dependency == "detector" {
                assert!(
                    withdrawn
                        .condor
                        .scenes
                        .iter()
                        .all(|s| s.sequence_data.noise_detection.is_none())
                );
                changed.save(&path).expect("known old source dependency");
                condor_cmd(&temp)
                    .args(["scenes", "0", "reset", "noise-scaling"])
                    .assert()
                    .success();
                assert!(
                    load(&path).condor.scenes[0].sequence_data.noise_detection.is_none(),
                    "clearing the scaling record must not erase its proof that the measurement source changed"
                );
            }
        }

        // An unsupported encoder still contributes to the global maximum.
        // Its measurement must be withdrawn even though it has no scaling proof.
        let mut population = measured.clone();
        population.condor.sequence_config.noise_scaler = Some(NoiseScalerConfig {
            threshold: 0.2,
            minimum_scaler: 0.5,
            maximum_scaler: 1.5,
            scale_chroma: true,
        });
        population.condor.scenes[0]
            .sequence_data
            .noise_detection
            .as_mut()
            .expect("noise")
            .noise = 0.4;
        population.condor.scenes[2]
            .sequence_data
            .noise_detection
            .as_mut()
            .expect("below threshold")
            .noise = 0.1;
        let unscaled = population.condor.scenes.len() - 1;
        population.condor.scenes[unscaled].encoder =
            andean_condor::models::encoder::Encoder::default_from_base(
                &andean_condor::models::encoder::EncoderBase::X264,
                false,
            );
        population.condor.scenes[unscaled]
            .sequence_data
            .noise_detection
            .as_mut()
            .expect("old maximum")
            .noise = 1.0;
        population.save(&path).expect("mixed scaling population");
        condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .arg("scale-noise")
            .assert()
            .success();
        let old_population = load(&path);
        assert!(old_population.condor.scenes[unscaled].sequence_data.noise_scaling.is_none());
        assert!(old_population.condor.scenes[2].sequence_data.noise_scaling.is_none());
        assert_eq!(
            old_population.condor.scenes[0]
                .encoder
                .photon_noise()
                .expect("old population scale")
                .iso,
            601
        );
        let mut expected_peer =
            serde_json::to_value(&old_population.condor.scenes[unscaled]).expect("unscaled peer");
        expected_peer["sequence_data"]["noise_detection"] = serde_json::Value::Null;
        let source_bytes = std::fs::read(&video.path).expect("source");
        let sentinel = temp.path().join("work/unowned.dat");
        std::fs::create_dir_all(sentinel.parent().expect("workdir")).expect("workdir");
        std::fs::write(&sentinel, b"preserve foreign file").expect("sentinel");
        for edit in [
            vec!["scenes", "0", "set", "params", "crf=19"],
            vec!["scenes", "all", "reset", "noise-scaling"],
        ] {
            let mut changed = old_population.clone();
            changed.condor.sequence_config.noise_detector = Some(NoiseDetectorConfig {
                input: Some(changed.condor.input.clone()),
                reference_filters: vec![],
                ..Default::default()
            });
            changed.save(&path).expect("changed detector for whole population");
            condor_cmd(&temp).args(edit).assert().success();
            let withdrawn = load(&path);
            assert!(
                withdrawn.condor.scenes[unscaled].sequence_data.noise_detection.is_none(),
                "N1: the unscaled peer must not retain the old detector's maximum 1.0"
            );
            assert!(
                withdrawn
                    .condor
                    .scenes
                    .iter()
                    .all(|scene| scene.sequence_data.noise_detection.is_none())
            );
            assert!(
                withdrawn
                    .condor
                    .scenes
                    .iter()
                    .all(|scene| scene.sequence_data.noise_scaling.is_none())
            );
            assert_eq!(
                withdrawn.condor.scenes[0].encoder.photon_noise().expect("exact original").iso,
                801
            );
            assert_eq!(
                withdrawn.condor.scenes[0]
                    .encoder
                    .photon_noise()
                    .expect("original chroma")
                    .chroma_iso,
                Some(801)
            );
            assert_eq!(
                withdrawn.condor.scenes[0].encoder.parameters()["preset"].to_string_value(),
                "3"
            );
            assert_eq!(
                serde_json::to_value(&withdrawn.condor.scenes[unscaled])
                    .expect("unscaled peer retained metadata"),
                expected_peer
            );
            for (before, after) in old_population.condor.scenes.iter().zip(&withdrawn.condor.scenes)
            {
                assert_eq!(
                    serde_json::to_value((
                        &after.sequence_data.scene_detection,
                        &after.sequence_data.target_quality,
                        &after.sequence_data.quality_check,
                        &after.sequence_data.zoner
                    ))
                    .expect("other metadata"),
                    serde_json::to_value((
                        &before.sequence_data.scene_detection,
                        &before.sequence_data.target_quality,
                        &before.sequence_data.quality_check,
                        &before.sequence_data.zoner
                    ))
                    .expect("unchanged other metadata")
                );
            }
            // Noise Detector's None-only scheduling can now remeasure every scene.
            // Supply fixed new measurement inputs; run the actual owning scaler.
            condor_cmd(&temp)
                .args(["scenes", "all", "reset", "parallel-encoder"])
                .assert()
                .success();
            let mut recovered = load(&path);
            for scene in &mut recovered.condor.scenes {
                scene.sequence_data.noise_detection = Some(NoiseDetectorData {
                    noise: 0.4,
                    luminance: 0.3,
                    created_on: SystemTime::UNIX_EPOCH,
                });
            }
            recovered
                .save(&path)
                .expect("new population measurements, not native detector claims");
            condor_cmd(&temp)
                .env("CONDOR_TEST_MODE", "1")
                .arg("scale-noise")
                .assert()
                .success();
            let recomputed = load(&path);
            assert_eq!(
                recomputed.condor.scenes[0].encoder.photon_noise().expect("new maximum 0.4").iso,
                1202,
                "new measurements must give factor 1.5, not the old mixed-population factor 0.75"
            );
            assert_eq!(
                recomputed.condor.scenes[0]
                    .encoder
                    .photon_noise()
                    .expect("new chroma")
                    .chroma_iso,
                Some(1202)
            );
            assert_eq!(
                recomputed.condor.scenes[0].encoder.parameters()["preset"].to_string_value(),
                "3"
            );
            let snapshot = serde_json::to_value(&recomputed).expect("new producer snapshot");
            condor_cmd(&temp)
                .env("CONDOR_TEST_MODE", "1")
                .arg("scale-noise")
                .assert()
                .success();
            assert_eq!(
                serde_json::to_value(load(&path)).expect("no second multiplication"),
                snapshot
            );
            assert_eq!(
                std::fs::read(&video.path).expect("preserved source"),
                source_bytes
            );
            assert_eq!(
                std::fs::read(&sentinel).expect("preserved foreign file"),
                b"preserve foreign file"
            );
        }
    }

    #[test]
    fn flags_and_crud_errors_preserve_memory_and_saved_bytes_offline() {
        use california_condor::commands::{
            config::ConfigCommand,
            handlers::scale_noise::{
                ScaleNoiseCrud, ScaleNoiseKey, ScaleNoiseSet, configure_noise_scaler,
                scale_noise_handler,
            },
        };
        let temp = tempfile::tempdir().expect("fixture");
        let path = temp.path().join("condor.json");
        let mut failures = vec![];
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.1] {
            for field in 0..3 {
                let mut config = noise_scaler_configuration(temp.path());
                config.save(&path).expect("save");
                let before = serde_json::to_vec(&config).expect("snapshot");
                let bytes = std::fs::read(&path).expect("bytes");
                let args = match field {
                    0 => (Some(bad), None, None),
                    1 => (None, Some(bad), None),
                    _ => (None, None, Some(bad)),
                };
                let result = configure_noise_scaler(&mut config, args.0, args.1, args.2, false);
                if result.is_ok() || serde_json::to_vec(&config).expect("config") != before {
                    failures.push(format!("flags field {field} bad={bad}: {result:?}"));
                }
                config = noise_scaler_configuration(temp.path());
                let before = serde_json::to_vec(&config).expect("CRUD snapshot");
                let set = match field {
                    0 => ScaleNoiseSet::Threshold { value: bad },
                    1 => ScaleNoiseSet::MinimumScaler { value: bad },
                    _ => ScaleNoiseSet::MaximumScaler { value: bad },
                };
                let result = ScaleNoiseCrud::apply(&mut config, &set);
                if result.is_ok() || serde_json::to_vec(&config).expect("config") != before {
                    failures.push(format!("CRUD field {field} bad={bad}: {result:?}"));
                }
                if scale_noise_handler(Some(&path), args.0, args.1, args.2, false).is_ok()
                    || std::fs::read(&path).expect("bytes") != bytes
                {
                    failures.push(format!(
                        "saved flags field {field} bad={bad} accepted/changed bytes"
                    ));
                }
            }
        }
        let mut config = noise_scaler_configuration(temp.path());
        let before = serde_json::to_vec(&config).expect("snapshot");
        let result = ScaleNoiseCrud::restore(&mut config, Some(&ScaleNoiseKey::MaximumScaler));
        if result.is_ok() || serde_json::to_vec(&config).expect("config") != before {
            failures.push(format!("reset max below min accepted/mutated: {result:?}"));
        }
        config.condor.sequence_config.noise_scaler = None;
        let before = serde_json::to_vec(&config).expect("snapshot");
        let result = configure_noise_scaler(&mut config, Some(f64::NAN), None, None, false);
        if result.is_ok() || serde_json::to_vec(&config).expect("config") != before {
            failures.push("invalid flags materialized disabled section".into());
        }
        assert!(!temp.path().join("scratch").exists());
        assert!(
            failures.is_empty(),
            "invalid config accepted or mutated: {failures:?}"
        );
    }

    #[test]
    fn actual_cli_invalid_flags_set_reset_preserve_file_bytes_offline() {
        let temp = tempfile::tempdir().expect("fixture");
        let path = temp.path().join("condor.json");
        let config = noise_scaler_configuration(temp.path());
        let mut failures = vec![];
        for args in [
            vec!["scale-noise", "--threshold", "NaN"],
            vec!["scale-noise", "--minimum-scaler", "inf"],
            vec!["scale-noise", "--maximum-scaler=-1"],
            vec!["scale-noise", "--minimum-scaler", "2"],
            vec!["scale-noise", "set", "threshold", "NaN"],
            vec!["scale-noise", "set", "maximum-scaler", "inf"],
            vec!["scale-noise", "set", "minimum-scaler", "2"],
            vec!["scale-noise", "reset", "maximum-scaler"],
        ] {
            config.save(&path).expect("save");
            let bytes = std::fs::read(&path).expect("bytes");
            let output =
                condor_cmd(&temp).env("PATH", temp.path()).args(&args).output().expect("CLI");
            let unchanged = std::fs::read(&path).expect("bytes") == bytes;
            eprintln!(
                "noise scaler CLI {args:?}: status={} unchanged={unchanged} stderr={}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            );
            if output.status.success() || !unchanged {
                failures.push(format!(
                    "{args:?}: status={}, unchanged={unchanged}",
                    output.status
                ));
            }
        }
        assert!(!temp.path().join("scratch").exists());
        assert!(
            failures.is_empty(),
            "CLI accepted invalid edit or changed bytes: {failures:?}"
        );
    }

    #[test]
    fn candidate_repair_defaults_and_crud_off_are_offline() {
        use california_condor::commands::{
            config::ConfigCommand,
            handlers::scale_noise::{
                ScaleNoiseCrud, ScaleNoiseKey, ScaleNoiseSet, configure_noise_scaler,
                scale_noise_handler,
            },
        };
        let temp = tempfile::tempdir().expect("fixture");
        let path = temp.path().join("condor.json");
        let mut config = noise_scaler_configuration(temp.path());
        config
            .condor
            .sequence_config
            .noise_scaler
            .as_mut()
            .expect("scaler")
            .minimum_scaler = 2.0;
        config.save(&path).expect("bad finite bounds");
        let (mut repaired, _) =
            scale_noise_handler(Some(&path), Some(0.2), Some(0.5), Some(1.5), false)
                .expect("complete flags repair");
        assert!(
            repaired
                .condor
                .sequence_config
                .noise_scaler
                .as_ref()
                .expect("scaler")
                .scale_chroma,
            "false flag retains true"
        );
        ScaleNoiseCrud::apply(
            &mut repaired,
            &ScaleNoiseSet::ScaleChroma {
                value: Some(california_condor::commands::config::Toggle::Off),
            },
        )
        .expect("CRUD off");
        assert!(
            !repaired
                .condor
                .sequence_config
                .noise_scaler
                .as_ref()
                .expect("scaler")
                .scale_chroma
        );
        config.save(&path).expect("bad bounds again");
        condor_cmd(&temp)
            .env("PATH", temp.path())
            .args(["scale-noise", "set", "maximum-scaler", "3"])
            .assert()
            .success();
        assert_eq!(
            load(&path).condor.sequence_config.noise_scaler.expect("scaler").maximum_scaler,
            3.0
        );
        config.save(&path).expect("bad bounds again");
        condor_cmd(&temp)
            .env("PATH", temp.path())
            .args(["scale-noise", "reset", "minimum-scaler"])
            .assert()
            .success();
        assert_eq!(
            load(&path).condor.sequence_config.noise_scaler.expect("scaler").minimum_scaler,
            0.0
        );
        config.save(&path).expect("bad bounds again");
        condor_cmd(&temp)
            .env("PATH", temp.path())
            .args(["scale-noise", "reset"])
            .assert()
            .success();
        assert!(load(&path).condor.sequence_config.noise_scaler.is_none());
        config.condor.sequence_config.noise_scaler.as_mut().expect("scaler").threshold = f64::NAN;
        configure_noise_scaler(&mut config, Some(0.2), Some(0.5), Some(1.5), false)
            .expect("repair before validation");
        ScaleNoiseCrud::restore(&mut config, None).expect("whole reset");
        assert!(config.condor.sequence_config.noise_scaler.is_none());
        ScaleNoiseCrud::restore(&mut config, Some(&ScaleNoiseKey::Threshold))
            .expect("disabled reset");
        assert!(config.condor.sequence_config.noise_scaler.is_none());
        configure_noise_scaler(&mut config, None, None, None, false).expect("defaults unchanged");
        let defaults = config.condor.sequence_config.noise_scaler.expect("defaults");
        assert_eq!(
            (
                defaults.threshold,
                defaults.minimum_scaler,
                defaults.maximum_scaler,
                defaults.scale_chroma
            ),
            (0.0, 0.0, 0.0, false)
        );
        assert!(!temp.path().join("scratch").exists());
        assert!(!temp.path().join("must-not-open.mkv").exists());
    }

    #[serial]
    #[test]
    fn default_and_no_noise_detector_data() {
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

        condor_cmd(&temp)
            .args(["init", path_str(&test_video.path), path_str(&output)])
            .assert()
            .success();

        condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .args(["scale-noise"])
            .assert()
            .success();

        let mut expected_config = default_config(&test_video, &output, &temp_abs);
        expected_config.condor.sequence_config.noise_scaler = Some(NoiseScalerConfig::default());
        // immutable shadow
        let expected_config = expected_config;

        let (config, _) =
            load_configuration(Some(&config_path)).expect("load_configuration should succeed");

        check_basic_config(&config, &expected_config);
    }

    #[serial]
    #[test]
    fn with_custom_options_and_iso_100() {
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

        // Mock an existing config file with Noise Detector data
        let mut config = default_config(&test_video, &output, &temp_abs);
        config.condor.sequence_config.noise_detector = Some(NoiseDetectorConfig::default());
        config.condor.encoder.set_photon_noise(Some(PhotonNoise {
            iso: 100,
            chroma_iso: None,
            width: None,
            height: None,
            c_y: None,
            ccb: None,
            ccr: None,
        }));
        let expected_noise = |index| match index {
            0 => 0.0,
            1 => 0.0003550650245139927,
            2 => 0.0002258349829248295,
            3 => 0.0001817853846724563,
            4 => 0.0000033027436124321854,
            _ => 0.0,
        };
        config.condor.scenes = test_video.mock_scenes(&config.condor.encoder);
        config.condor.scenes.iter_mut().enumerate().for_each(|(index, scene)| {
            scene.encoder.set_photon_noise(Some(PhotonNoise {
                iso: 100,
                chroma_iso: None,
                width: None,
                height: None,
                c_y: None,
                ccb: None,
                ccr: None,
            }));
            scene.sequence_data.noise_detection = Some(NoiseDetectorData {
                created_on: SystemTime::now(),
                noise: expected_noise(index),
                luminance: 0.0,
            });
        });
        config.save(&config_path).expect("configuration save should succeed");

        condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .args([
                "scale-noise",
                "--threshold",
                "0.0002",
                "--minimum-scaler",
                "1.0",
                "--maximum-scaler",
                "2.0",
                "--scale-chroma",
            ])
            .assert()
            .success();

        let mut expected_config = config;
        let expected_isos = |index| match index {
            0 => 100,
            1 => 200,
            2 => 117,
            3 => 100,
            4 => 100,
            _ => 100,
        };
        expected_config.condor.sequence_config.noise_scaler = Some(NoiseScalerConfig {
            threshold: 0.0002,
            minimum_scaler: 1.0,
            maximum_scaler: 2.0,
            scale_chroma: true,
        });
        expected_config.condor.scenes.iter_mut().enumerate().for_each(|(index, scene)| {
            scene.encoder.set_photon_noise(Some(PhotonNoise {
                iso: expected_isos(index),
                chroma_iso: None,
                width: None,
                height: None,
                c_y: None,
                ccb: None,
                ccr: None,
            }));
            scene.sequence_data.noise_detection = Some(NoiseDetectorData {
                created_on: SystemTime::now(),
                noise: expected_noise(index),
                luminance: 0.0,
            });
        });

        let (config, _) =
            load_configuration(Some(&config_path)).expect("load_configuration should succeed");

        check_basic_config(&config, &expected_config);
        assert_eq!(
            config.condor.scenes.len(),
            test_video.scenes.len(),
            "scenes contains {} scenes",
            test_video.scenes.len()
        );
        config
            .condor
            .scenes
            .iter()
            .zip(expected_config.condor.scenes.iter())
            .enumerate()
            .for_each(|(index, (scene, expected_scene))| {
                check_encoder(
                    &scene.encoder,
                    &expected_scene.encoder,
                    &format!("scene {} encoder", index),
                );
            });
    }

    /// `scale-noise` materializes its optional section, validates the scaler
    /// bounds, and `reset` resets single settings.
    #[serial]
    #[test]
    fn scale_noise_set_reset_and_validation() {
        if !ffmpeg_is_available() {
            return;
        }
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let config_path = init_config(&temp);

        condor_cmd(&temp)
            .args(["scale-noise", "set", "threshold", "0.002"])
            .assert()
            .success();
        condor_cmd(&temp)
            .args(["scale-noise", "set", "scale-chroma", "on"])
            .assert()
            .success();

        // A minimum above the default maximum (0) is refused.
        condor_cmd(&temp)
            .args(["scale-noise", "set", "minimum-scaler", "1.5"])
            .assert()
            .failure();

        condor_cmd(&temp)
            .args(["scale-noise", "set", "maximum-scaler", "2"])
            .assert()
            .success();
        condor_cmd(&temp)
            .args(["scale-noise", "set", "minimum-scaler", "1.5"])
            .assert()
            .success();

        let scaler = load(&config_path)
            .condor
            .sequence_config
            .noise_scaler
            .expect("noise scaler is set");
        assert_eq!(scaler.threshold, 0.002, "threshold is set");
        assert_eq!(scaler.minimum_scaler, 1.5, "minimum scaler is set");
        assert_eq!(scaler.maximum_scaler, 2.0, "maximum scaler is set");
        assert!(scaler.scale_chroma, "chroma scaling is on");

        condor_cmd(&temp)
            .args(["scale-noise", "reset", "scale-chroma"])
            .assert()
            .success();
        let scaler = load(&config_path)
            .condor
            .sequence_config
            .noise_scaler
            .expect("noise scaler still exists");
        assert!(!scaler.scale_chroma, "chroma scaling is off again");
        assert_eq!(scaler.threshold, 0.002, "the other settings are untouched");
    }
}
