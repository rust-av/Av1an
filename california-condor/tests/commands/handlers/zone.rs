use crate::common::condor_cmd;
use andean_condor::models::encoder::{Encoder, EncoderBase, cli_parameter::CLIParameter};
use california_condor::{
    commands::handlers::load_configuration,
    test_helpers::{check_encoder, ffmpeg_is_available, get_test_video, path_str},
};

#[cfg(test)]
mod handler_configuration {
    use super::*;
    use california_condor::{
        commands::handlers::zone::zone_handler,
        test_helpers::default_config,
        utils::hash_path::hash_path,
        zoner::{ZonerConfig, parse_zone},
    };

    #[test]
    fn zone_handler_without_requested_zones_retains_saved_definitions_and_config_path() {
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
        let config_path = temp.path().join("condor.json");
        let config_path_abs = path_abs::PathAbs::new(&config_path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let mut configuration =
            default_config(&test_video, &temp.path().join("out.mkv"), &temp_abs);
        configuration.condor.sequence_config.zoner = Some(ZonerConfig {
            zones: vec![parse_zone("-1 200 svt-av1").expect("the zone parses")],
        });
        configuration.save(&config_path).expect("the config saves");
        let (config, found_config_path) =
            zone_handler(Some(&config_path), &[]).expect("the handler succeeds");
        assert_eq!(
            found_config_path, config_path_abs,
            "the handler reports the config it loaded"
        );
        assert_eq!(
            config.condor.sequence_config.zoner, configuration.condor.sequence_config.zoner,
            "the configured zones survive"
        );
    }

    #[test]
    fn zone_handler_without_requested_or_saved_zones_reports_missing_zones() {
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
        let config_path = temp.path().join("condor.json");
        default_config(&test_video, &temp.path().join("out.mkv"), &temp_abs)
            .save(&config_path)
            .expect("the config saves");
        let error = zone_handler(Some(&config_path), &[]).expect_err("there is nothing to zone");
        assert!(
            error.to_string().contains("no zones provided"),
            "error explains the missing zones: {error}"
        );
    }
}

#[cfg(test)]
mod tests {
    use serial_test::serial;

    use super::*;

    fn assert_info_logs(output: &std::process::Output, messages: &[&str]) {
        let stderr = String::from_utf8_lossy(&output.stderr);
        for message in messages {
            assert!(
                stderr.lines().any(|line| line.contains("INFO") && line.contains(message)),
                "expected INFO log {message:?} in console output:\n{stderr}"
            );
        }
    }

    /// `x265` with the zone's CRF over the base's defaults.
    fn x265(crf: f64) -> Encoder {
        let mut encoder = Encoder::default_from_base(&EncoderBase::X265, false);
        encoder
            .parameters_mut()
            .insert("crf".to_owned(), CLIParameter::new_number("--", " ", crf));

        encoder
    }

    /// `svt-av1` with `preset` merged over the caller's encoder.
    fn with_preset(mut encoder: Encoder, preset: f64) -> Encoder {
        encoder.parameters_mut().insert(
            "preset".to_owned(),
            CLIParameter::new_number("--", " ", preset),
        );

        encoder
    }

    fn check_zone_partitions(phase: u8) {
        assert!((1..=3).contains(&phase));
        let video = get_test_video();
        let temp = tempfile::tempdir().expect("fixture");
        let path = temp.path().join("condor.json");
        let mut config = california_condor::test_helpers::default_config(
            &video,
            &temp.path().join("out.mkv"),
            temp.path(),
        );
        config.condor.scenes = video.mock_scenes(&config.condor.encoder);
        config.condor.scenes.truncate(1);
        config.condor.scenes[0].end_frame = 100;
        let source = std::fs::read(&video.path).expect("source");
        // Serialize the actual model for the supported JSON encoder representation.
        let zone =
            california_condor::zoner::parse_zone("25 75 replace x265 \"--crf 18\"").expect("zone");
        std::fs::write(
            temp.path().join("zones.json"),
            serde_json::to_vec(&vec![zone]).expect("zone JSON"),
        )
        .expect("zone file");
        for definition in ["25 75 replace x265 \"--crf 18\"", "zones.json"] {
            config.save(&path).expect("baseline");
            let before = std::fs::read(&path).expect("preview bytes");
            let preview = condor_cmd(&temp)
                .env("RUST_LOG", "california_condor=info")
                .args(["zone", "--dry-run", definition])
                .assert()
                .success();
            assert_info_logs(
                preview.get_output(),
                &[
                    "zone boundaries added 2 scenes (1 -> 3 partitions) before overrides and caps",
                    "zone 1 (25..75) matched 1 scenes, modified 1, split 0, added 0 (0 unchanged)",
                ],
            );
            assert_eq!(std::fs::read(&path).expect("preview unchanged"), before);
            if phase == 2 {
                continue;
            }
            let applied = condor_cmd(&temp)
                .env("RUST_LOG", "california_condor=info")
                .args(["zone", definition])
                .assert()
                .success();
            assert_info_logs(
                applied.get_output(),
                &[
                    "Zoner finished: 1 zones, 1 scene changes across zones, 2 scenes added (1 -> 3 total), 0 warnings",
                ],
            );
            let (saved, _) = load_configuration(Some(&path)).expect("saved");
            assert_eq!(
                saved
                    .condor
                    .scenes
                    .iter()
                    .map(|s| (s.start_frame, s.end_frame))
                    .collect::<Vec<_>>(),
                vec![(0, 25), (25, 75), (75, 100)],
                "CLI must cut before overriding"
            );
            assert_eq!(saved.condor.scenes[0].encoder, config.condor.encoder);
            assert_eq!(saved.condor.scenes[1].encoder, x265(18.0));
            assert_eq!(saved.condor.scenes[2].encoder, config.condor.encoder);
            assert!(
                saved
                    .condor
                    .scenes
                    .iter()
                    .all(|s| !s.sequence_data.parallel_encoder.invalidated)
            );
            assert_ne!(std::fs::read(&path).expect("published"), before);
            if phase == 1 {
                continue;
            }
            let bytes = std::fs::read(&path).expect("current");
            condor_cmd(&temp).arg("zone").assert().success();
            assert_eq!(std::fs::read(&path).expect("no resave"), bytes);
        }
        assert_eq!(
            std::fs::read(&video.path).expect("source preserved"),
            source
        );
    }

    fn check_legacy_zoner_migration(phase: u8) {
        assert!((1..=4).contains(&phase));
        use california_condor::zoner::{SceneDefinition, Zoner, ZonerConfig, parse_zone};
        let video = get_test_video();
        let temp = tempfile::tempdir().expect("fixture");
        let path = temp.path().join("condor.json");
        let mut config = california_condor::test_helpers::default_config(
            &video,
            &temp.path().join("out.mkv"),
            temp.path(),
        );
        config.condor.scenes = video.mock_scenes(&config.condor.encoder);
        config.condor.scenes.truncate(1);
        config.condor.scenes[0].end_frame = 100;
        config.condor.scenes[0].encoder = x265(17.0);
        let baseline = vec![SceneDefinition::from(&config.condor.scenes[0])];
        config.condor.sequence_config.zoner = Some(ZonerConfig {
            zones: vec![parse_zone("25 75 replace x264").expect("zone")],
        });
        config.condor.scenes[0].encoder = Encoder::default_from_base(&EncoderBase::X264, false);
        config.condor.scenes[0].sequence_data.zoner.zoned = true;
        let applied = vec![SceneDefinition::from(&config.condor.scenes[0])];
        // Explicit pre-Z01 schema, not a newly produced/current-result certificate.
        let mut old = serde_json::to_value(&config).expect("legacy config");
        old["condor"]["scenes"][0]["sequence_data"]["zoner"]["lifecycle"] = serde_json::json!({"baseline":baseline,"zones":config.condor.sequence_config.zoner.as_ref().expect("zones").zones,"applied":applied});
        let legacy: california_condor::configuration::Configuration =
            serde_json::from_value(old.clone()).expect("old schema");
        let dir = &legacy.condor.sequence_config.parallel_encoder.scenes_directory;
        std::fs::create_dir_all(dir).expect("old artifacts");
        let chunk = dir.join("00000.264");
        std::fs::write(&chunk, b"old whole-scene bytes; never certify").expect("legacy artifact");
        let new_index = dir.join("00001.264");
        std::fs::write(&new_index, b"foreign new-index bytes; never adopt")
            .expect("new index artifact");
        legacy.save(&path).expect("old job");
        let original = std::fs::read(&path).expect("old JSON");
        let blocked = temp.path().join("not-a-file");
        std::fs::create_dir_all(&blocked).expect("publication barrier");
        std::fs::write(blocked.join("sentinel"), b"preserve").expect("sentinel");
        let candidate = legacy.clone();
        let mut direct = legacy
            .instantiate_condor(
                Box::new(move |data| {
                    let mut staged = candidate.clone();
                    staged.condor = data;
                    staged.save(&blocked)?;
                    Ok(())
                }),
                &Default::default(),
            )
            .expect("direct");
        let before = serde_json::to_value(direct.as_data()).expect("runtime");
        assert!(
            Zoner::run(&mut direct, Default::default()).is_err(),
            "old unchanged definitions must migrate, not skip the callback"
        );
        assert_eq!(
            serde_json::to_value(direct.as_data()).expect("rollback"),
            before
        );
        assert_eq!(std::fs::read(&path).expect("old JSON"), original);
        assert_eq!(
            std::fs::read(temp.path().join("not-a-file/sentinel")).expect("sentinel"),
            b"preserve"
        );
        if phase == 1 {
            return;
        }
        for (revision, manual, message) in
            [(None, true, "reset/redetect"), (Some(99), false, "algorithm revision")]
        {
            let mut refused = old.clone();
            if let Some(revision) = revision {
                refused["condor"]["scenes"][0]["sequence_data"]["zoner"]["lifecycle"]["algorithm_revision"] =
                    revision.into();
            }
            let mut refused: california_condor::configuration::Configuration =
                serde_json::from_value(refused).expect("fixture");
            if manual {
                refused.condor.scenes[0].encoder.set_quantizer(19.0);
            }
            refused.save(&path).expect("unsafe job");
            let bytes = std::fs::read(&path).expect("unsafe bytes");
            let output = condor_cmd(&temp).arg("zone").assert().failure();
            assert!(String::from_utf8_lossy(&output.get_output().stderr).contains(message));
            assert_eq!(std::fs::read(&path).expect("refused bytes"), bytes);
        }
        if phase == 2 {
            return;
        }
        legacy.save(&path).expect("restore valid old job");
        condor_cmd(&temp).arg("zone").assert().success();
        let (migrated, _) = load_configuration(Some(&path)).expect("migrated");
        assert_eq!(
            migrated
                .condor
                .scenes
                .iter()
                .map(|s| (s.start_frame, s.end_frame))
                .collect::<Vec<_>>(),
            vec![(0, 25), (25, 75), (75, 100)]
        );
        assert_eq!(migrated.condor.scenes[0].encoder, x265(17.0));
        assert_eq!(migrated.condor.scenes[1].encoder.base(), EncoderBase::X264);
        assert_eq!(migrated.condor.scenes[2].encoder, x265(17.0));
        assert!(
            migrated
                .condor
                .scenes
                .iter()
                .all(|s| s.sequence_data.parallel_encoder.invalidated
                    && s.sequence_data.parallel_encoder.provenance.is_none())
        );
        assert!(migrated.admit_results().is_err());
        assert_eq!(
            std::fs::read(&chunk).expect("preserved artifact"),
            b"old whole-scene bytes; never certify"
        );
        assert_eq!(
            std::fs::read(&new_index).expect("new-index bytes"),
            b"foreign new-index bytes; never adopt"
        );
        if phase == 3 {
            return;
        }
        let bytes = std::fs::read(&path).expect("current bytes");
        condor_cmd(&temp).arg("zone").assert().success();
        assert_eq!(std::fs::read(&path).expect("current no save"), bytes);
        let mut direct = migrated
            .instantiate_condor(
                Box::new(|_| anyhow::bail!("no extra save")),
                &Default::default(),
            )
            .expect("current roundtrip");
        Zoner::run(&mut direct, Default::default()).expect("current exact definitions skip");
    }

    fn check_equivalent_zoner_migration(phase: u8) {
        assert!((1..=4).contains(&phase));
        use andean_condor::models::{
            encoder::photon_noise::PhotonNoise, sequence::noise_detector::NoiseDetectorData,
        };
        use california_condor::zoner::Zoner;
        use std::{cell::Cell, rc::Rc, time::SystemTime};

        let video = get_test_video();
        for definition in [
            "0 121 svt-av1 \"--preset 3\"",
            "-1 -1 svt-av1 \"--preset 3\"",
            "-1 -1 --maximum-frames 96",
        ] {
            let temp = tempfile::tempdir().expect("fixture");
            let path = temp.path().join("condor.json");
            let mut config = california_condor::test_helpers::default_config(
                &video,
                &temp.path().join("out.mkv"),
                temp.path(),
            );
            config.condor.encoder.set_photon_noise(Some(PhotonNoise {
                iso: 801,
                chroma_iso: Some(403),
                width: None,
                height: None,
                c_y: None,
                ccb: None,
                ccr: None,
            }));
            config.condor.scenes = video.mock_scenes(&config.condor.encoder);
            config.save(&path).expect("original");
            // These aligned/whole-input definitions have the same old/new output.
            condor_cmd(&temp).args(["zone", definition]).assert().success();
            let (mut measured, _) = load_configuration(Some(&path)).expect("zoned");
            for scene in &mut measured.condor.scenes {
                // Fixed detector inputs, not claimed native measurements/certificates.
                scene.sequence_data.noise_detection = Some(NoiseDetectorData {
                    noise: 0.4,
                    luminance: 0.3,
                    created_on: SystemTime::UNIX_EPOCH,
                });
            }
            measured.save(&path).expect("detector input");
            condor_cmd(&temp)
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
            let (mut legacy, _) = load_configuration(Some(&path)).expect("actual scaling");
            assert!(legacy.condor.scenes.iter().all(|s| {
                let photon = s.encoder.photon_noise().expect("scaled PhotonNoise");
                let baseline = s
                    .sequence_data
                    .noise_scaling
                    .as_ref()
                    .expect("producer result")
                    .baseline
                    .as_ref()
                    .expect("producer baseline");
                photon.iso == 401
                    && photon.chroma_iso == Some(202)
                    && baseline.original.iso == 801
                    && baseline.original.chroma_iso == Some(403)
            }));
            legacy.condor.scenes[0]
                .sequence_data
                .zoner
                .lifecycle
                .as_mut()
                .expect("proven lifecycle")
                .algorithm_revision = 0;
            // An unrelated manual option must also survive a proven no-effect migration.
            legacy.condor.scenes[0].encoder.set_quantizer(19.0);
            legacy.save(&path).expect("revision-0 job with actual downstream results");
            let source = std::fs::read(&video.path).expect("source");
            let artifact = temp.path().join("00000.ivf");
            std::fs::write(&artifact, b"preserve old artifact; never certify").expect("artifact");
            let saves = Rc::new(Cell::new(0));
            let observed = Rc::clone(&saves);
            let publication = legacy.clone();
            let publication_path = path.clone();
            let mut direct = legacy
                .instantiate_condor(
                    Box::new(move |data| {
                        observed.set(observed.get() + 1);
                        let mut staged = publication.clone();
                        staged.condor = data;
                        staged.save(&publication_path)?;
                        Ok(())
                    }),
                    &Default::default(),
                )
                .expect("owning direct Zoner");
            let mut expected = serde_json::to_value(direct.as_data()).expect("all original state");
            expected["scenes"][0]["sequence_data"]["zoner"]["lifecycle"]["algorithm_revision"] =
                1.into();
            Zoner::run(&mut direct, Default::default()).expect(
                "M1: an equivalent algorithm must preserve downstream scaling, not refuse replay",
            );
            assert_eq!(
                saves.get(),
                1,
                "revision-only publication once: {definition}"
            );
            assert_eq!(
                serde_json::to_value(direct.as_data()).expect("revision-only state"),
                expected
            );
            Zoner::run(&mut direct, Default::default()).expect("current skip");
            assert_eq!(saves.get(), 1, "current definitions never save again");
            if phase == 1 {
                continue;
            }

            // A publication failure must be the callback error, not the old unsafe-rebase refusal.
            legacy.save(&path).expect("restore old revision");
            let bytes = std::fs::read(&path).expect("old bytes");
            let blocked = temp.path().join("not-a-file");
            std::fs::create_dir_all(&blocked).expect("publication barrier");
            std::fs::write(blocked.join("sentinel"), b"preserve").expect("sentinel");
            let publication = legacy.clone();
            let observed = Rc::clone(&saves);
            let mut failed = legacy
                .instantiate_condor(
                    Box::new(move |data| {
                        observed.set(observed.get() + 1);
                        let mut staged = publication.clone();
                        staged.condor = data;
                        staged.save(&blocked).map_err(|error| {
                            anyhow::anyhow!("scene publication failed: {error:#}")
                        })?;
                        Ok(())
                    }),
                    &Default::default(),
                )
                .expect("rollback job");
            let before = serde_json::to_value(failed.as_data()).expect("before failure");
            let error = Zoner::run(&mut failed, Default::default())
                .expect_err("callback must reject migration");
            assert!(format!("{error:#}").contains("scene publication failed"));
            assert_eq!(saves.get(), 2);
            assert_eq!(
                serde_json::to_value(failed.as_data()).expect("rollback"),
                before
            );
            assert_eq!(std::fs::read(&path).expect("unchanged JSON"), bytes);
            assert_eq!(
                std::fs::read(temp.path().join("not-a-file/sentinel")).expect("sentinel"),
                b"preserve"
            );
            if phase == 2 {
                continue;
            }

            let mut expected = serde_json::to_value(&legacy).expect("saved snapshot");
            expected["condor"]["scenes"][0]["sequence_data"]["zoner"]["lifecycle"]["algorithm_revision"] =
                1.into();
            let preview = condor_cmd(&temp)
                .env("RUST_LOG", "california_condor=info")
                .args(["zone", "--dry-run"])
                .assert()
                .success();
            assert_info_logs(
                preview.get_output(),
                &["equivalent zoning algorithms; updated algorithm revision only"],
            );
            assert!(
                !String::from_utf8_lossy(&preview.get_output().stderr)
                    .contains("modified encoder settings")
            );
            assert_eq!(std::fs::read(&path).expect("dry-run bytes"), bytes);
            condor_cmd(&temp).arg("zone").assert().success();
            let (migrated, _) = load_configuration(Some(&path)).expect("CLI migration");
            assert_eq!(
                serde_json::to_value(migrated).expect("only revision changed"),
                expected
            );
            let bytes = std::fs::read(&path).expect("current bytes");
            condor_cmd(&temp).arg("zone").assert().success();
            assert_eq!(std::fs::read(&path).expect("CLI current no-save"), bytes);
            condor_cmd(&temp).arg("scale-noise").assert().success();
            let (repeated, _) = load_configuration(Some(&path)).expect("scaler repeat");
            assert_eq!(
                serde_json::to_value(repeated).expect("no double scaling or changed proof"),
                expected
            );
            if phase == 3 {
                continue;
            }
            for (kind, message) in [
                ("changed definitions", "reset/redetect"),
                ("future", "algorithm revision"),
                ("missing proof", "original baseline"),
            ] {
                let mut refused = legacy.clone();
                match kind {
                    "changed definitions" => {
                        let zone = &mut refused
                            .condor
                            .sequence_config
                            .zoner
                            .as_mut()
                            .expect("zones")
                            .zones[0];
                        zone.start_frame = if zone.start_frame.is_some() {
                            None
                        } else {
                            Some(0)
                        };
                    },
                    "future" => {
                        refused.condor.scenes[0]
                            .sequence_data
                            .zoner
                            .lifecycle
                            .as_mut()
                            .expect("proof")
                            .algorithm_revision = 99;
                    },
                    _ => refused.condor.scenes[0].sequence_data.zoner.lifecycle = None,
                }
                refused.save(&path).expect("guard fixture");
                let bytes = std::fs::read(&path).expect("refusal bytes");
                let output = condor_cmd(&temp).arg("zone").assert().failure();
                assert!(
                    String::from_utf8_lossy(&output.get_output().stderr).contains(message),
                    "{kind}"
                );
                assert_eq!(std::fs::read(&path).expect("guard leaves JSON"), bytes);
            }
            assert_eq!(
                std::fs::read(&artifact).expect("old artifact preserved"),
                b"preserve old artifact; never certify"
            );
            assert_eq!(
                std::fs::read(&video.path).expect("source preserved"),
                source
            );
        }
    }

    #[serial]
    #[test]
    fn changed_zones_restore_original_scene_baseline() {
        let video = get_test_video();
        let temp = tempfile::tempdir().expect("temp");
        let path = temp.path().join("condor.json");
        condor_cmd(&temp)
            .args(["init", path_str(&video.path), "out.mkv"])
            .assert()
            .success();
        let (mut config, _) = load_configuration(Some(&path)).expect("config");
        config.condor.scenes = video.mock_scenes(&config.condor.encoder);
        config.condor.scenes[0].encoder = x265(17.0);
        let original = config.condor.scenes[0].encoder.clone();
        config.save(&path).expect("save");
        condor_cmd(&temp)
            .args(["zone", "0 121 replace x264 \"--crf 22\""])
            .assert()
            .success();
        // Roundtrip and changed definitions must not be skipped by the old boolean.
        condor_cmd(&temp)
            .args(["zone", "121 240 replace x264 \"--crf 24\""])
            .assert()
            .success();
        let (updated, _) = load_configuration(Some(&path)).expect("updated");
        assert_eq!(
            updated.condor.scenes[0].encoder, original,
            "a removed override must restore the heterogeneous original encoder"
        );
        assert_eq!(updated.condor.scenes[1].encoder.base(), EncoderBase::X264);
        let saved = std::fs::read(&path).expect("saved");
        condor_cmd(&temp).arg("zone").assert().success();
        assert_eq!(std::fs::read(&path).expect("unchanged roundtrip"), saved);
        let mut removed = updated;
        removed.condor.sequence_config.zoner = None;
        removed.save(&path).expect("remove all definitions");
        condor_cmd(&temp).arg("zone").assert().success();
        let (restored, _) = load_configuration(Some(&path)).expect("restored");
        assert_eq!(restored.condor.scenes[0].encoder, original);
        assert_eq!(restored.condor.scenes[1].encoder, restored.condor.encoder);
    }

    fn check_direct_zoner_publication(phase: u8) {
        assert!((1..=6).contains(&phase));
        use andean_condor::core::sequence::Sequence;
        use california_condor::{
            test_helpers::default_config,
            zoner::{Zoner, ZonerConfig, parse_zone},
        };
        use std::{cell::Cell, rc::Rc};
        let video = get_test_video();
        let temp = tempfile::tempdir().expect("temp");
        let mut config = default_config(&video, &temp.path().join("out.mkv"), temp.path());
        config.condor.scenes = video.mock_scenes(&config.condor.encoder);
        config.condor.scenes[0].sequence_data.parallel_encoder.bytes = Some(123);
        config.condor.scenes[0].encoder = x265(17.0);
        config.condor.scenes[0].sub_scenes = Some(vec![andean_condor::models::scene::SubScene {
            start_frame: 0,
            end_frame: 121,
        }]);
        config.condor.sequence_config.zoner = Some(ZonerConfig {
            zones: vec![parse_zone("0 121 --maximum-frames 96").expect("zone")],
        });
        let before = serde_json::to_value(&config.condor).expect("before");
        let path = temp.path().join("condor.json");
        config.save(&path).expect("original JSON");
        let json = std::fs::read(&path).expect("original bytes");
        let blocked = temp.path().join("not-a-file");
        std::fs::create_dir_all(&blocked).expect("publication barrier");
        std::fs::write(blocked.join("sentinel"), b"preserve").expect("sentinel");
        let staged_config = config.clone();
        let mut direct = config
            .instantiate_condor(
                Box::new(move |data| {
                    let mut candidate = staged_config.clone();
                    candidate.condor = data;
                    candidate.save(&blocked)?;
                    Ok(())
                }),
                &Default::default(),
            )
            .expect("direct");
        let runtime_before = serde_json::to_value(direct.as_data()).expect("runtime before");
        let (tx, _rx) = std::sync::mpsc::channel();
        assert!(
            Zoner::default()
                .execute(
                    &mut direct,
                    tx,
                    andean_condor::utils::cancellation::Cancellation::default()
                )
                .is_err()
        );
        // instantiate_condor adds a source_path, so compare against the exact runtime snapshot.
        assert_eq!(
            serde_json::to_value(&direct.scenes).expect("rollback"),
            before["scenes"]
        );
        assert_eq!(
            serde_json::to_value(direct.as_data()).expect("full rollback"),
            runtime_before
        );
        assert_eq!(std::fs::read(&path).expect("old JSON"), json);
        assert_eq!(
            std::fs::read(temp.path().join("not-a-file/sentinel")).expect("sentinel"),
            b"preserve"
        );
        assert!(direct.scenes[0].sequence_data.zoner.lifecycle.is_none());
        if phase == 1 {
            return;
        }
        let saves = Rc::new(Cell::new(0));
        let observed = Rc::clone(&saves);
        direct.save_callback = Box::new(move |_| {
            observed.set(observed.get() + 1);
            Ok(())
        });
        Zoner::run(&mut direct, Default::default()).expect("successful stage");
        assert_eq!(saves.get(), 1);
        assert!(direct.scenes[0].sequence_data.parallel_encoder.invalidated);
        let roundtrip =
            serde_json::from_slice(&serde_json::to_vec(&direct.as_data()).expect("serialize"))
                .expect("roundtrip");
        config.condor = roundtrip;
        let observed = Rc::clone(&saves);
        let mut direct = config
            .instantiate_condor(
                Box::new(move |_| {
                    observed.set(observed.get() + 1);
                    Ok(())
                }),
                &Default::default(),
            )
            .expect("reload");
        Zoner::run(&mut direct, Default::default()).expect("known unchanged");
        let (tx, _rx) = std::sync::mpsc::channel();
        Zoner::default()
            .execute(
                &mut direct,
                tx,
                andean_condor::utils::cancellation::Cancellation::default(),
            )
            .expect("direct unchanged");
        assert_eq!(
            saves.get(),
            1,
            "neither entrypoint resaves known unchanged definitions"
        );
        if phase == 2 {
            return;
        }
        let mut removed = config.clone();
        removed.condor.sequence_config.zoner = None;
        let mut restored = removed
            .instantiate_condor(Box::new(|_| Ok(())), &Default::default())
            .expect("remove length cap");
        Zoner::run(&mut restored, Default::default()).expect("restore original partition");
        assert_eq!(
            restored.scenes.iter().map(|s| (s.start_frame, s.end_frame)).collect::<Vec<_>>(),
            video.scenes
        );
        assert_eq!(restored.scenes[0].encoder, x265(17.0));
        assert_eq!(
            restored.scenes[0].sub_scenes.as_ref().expect("original sub-scenes")[0].end_frame,
            121
        );
        if phase == 3 {
            return;
        }
        direct.sequence_config.zoner = None;
        let before_failed_rebase =
            serde_json::to_value(direct.as_data()).expect("before failed removal");
        let callback = std::mem::replace(
            &mut direct.save_callback,
            Box::new(|_| anyhow::bail!("reject rebased candidate")),
        );
        assert!(Zoner::run(&mut direct, Default::default()).is_err());
        assert_eq!(
            serde_json::to_value(direct.as_data()).expect("rebase rollback"),
            before_failed_rebase
        );
        direct.save_callback = callback;
        if phase == 4 {
            return;
        }
        direct.scenes[0].encoder.set_quantizer(18.0);
        direct.sequence_config.zoner = None;
        let manual = serde_json::to_value(direct.as_data()).expect("manual edits");
        let error = Zoner::run(&mut direct, Default::default()).expect_err("unsafe rebase");
        assert!(format!("{error:#}").contains("reset/redetect"));
        assert_eq!(
            serde_json::to_value(direct.as_data()).expect("preserved"),
            manual
        );
        if phase == 5 {
            return;
        }
        for scene in &mut direct.scenes {
            scene.sequence_data.zoner.lifecycle = None;
        }
        let legacy = serde_json::to_value(direct.as_data()).expect("legacy");
        for execute in [false, true] {
            let result = if execute {
                let (tx, _rx) = std::sync::mpsc::channel();
                Zoner::default()
                    .execute(
                        &mut direct,
                        tx,
                        andean_condor::utils::cancellation::Cancellation::default(),
                    )
                    .map(|_| ())
            } else {
                Zoner::run(&mut direct, Default::default())
            };
            assert!(
                format!("{:#}", result.expect_err("legacy unknown refusal"))
                    .contains("original baseline")
            );
            assert_eq!(
                serde_json::to_value(direct.as_data()).expect("legacy preserved"),
                legacy
            );
        }
        assert_eq!(saves.get(), 1);
    }

    fn check_legacy_zone_recovery(recover: bool) {
        let video = get_test_video();
        let temp = tempfile::tempdir().expect("fixture");
        let path = temp.path().join("condor.json");
        let mut config = california_condor::test_helpers::default_config(
            &video,
            &temp.path().join("out.mkv"),
            temp.path(),
        );
        config.condor.scenes = video.mock_scenes(&config.condor.encoder);
        for scene in &mut config.condor.scenes {
            scene.sequence_data.zoner.zoned = true;
        }
        config.save(&path).expect("legacy JSON");
        let json = std::fs::read(&path).expect("original");
        for args in [
            vec!["zone"],
            vec!["zone", "0 121 replace x264"],
            vec!["scenes", "all", "reset", "zoner"],
        ] {
            let refused = condor_cmd(&temp).args(args).assert().failure();
            assert!(
                String::from_utf8_lossy(&refused.get_output().stderr).contains("reset/redetect")
            );
            assert_eq!(std::fs::read(&path).expect("unchanged"), json);
        }
        if !recover {
            return;
        }
        condor_cmd(&temp).args(["scenes", "reset"]).assert().success();
        condor_cmd(&temp).args(["scenes", "add", "0", "121"]).assert().success();
        condor_cmd(&temp).args(["zone", "0 121 replace x264"]).assert().success();
        let (recovered, _) = load_configuration(Some(&path)).expect("recovered");
        assert!(!recovered.condor.scenes[0].sequence_data.parallel_encoder.invalidated);
        assert!(recovered.condor.scenes[0].sequence_data.zoner.lifecycle.is_some());
    }

    fn check_zone_anchor_edit(phase: u8) {
        assert!((1..=3).contains(&phase));
        use california_condor::zoner::Zoner;
        let video = get_test_video();
        let temp = tempfile::tempdir().expect("fixture");
        let path = temp.path().join("condor.json");
        let mut config = california_condor::test_helpers::default_config(
            &video,
            &temp.path().join("out.mkv"),
            temp.path(),
        );
        config.condor.scenes = video.mock_scenes(&config.condor.encoder);
        config.save(&path).expect("original scenes");
        condor_cmd(&temp)
            .args(["zone", "0 240 svt-av1 \"--preset 3\""])
            .assert()
            .success();
        let (zoned, _) = load_configuration(Some(&path)).expect("actual zoning proof");
        let proof = serde_json::to_value(&zoned.condor.scenes[0].sequence_data.zoner.lifecycle)
            .expect("proof");
        for args in [
            vec!["scenes", "0", "delete"],
            vec!["scenes", "add", "0", "60", "--prepend"],
            vec!["scenes", "add", "0", "60"],
        ] {
            zoned.save(&path).expect("restore actual proof");
            condor_cmd(&temp).args(args).assert().success();
            let json = std::fs::read(&path).expect("edited JSON");
            condor_cmd(&temp).arg("zone").assert().success();
            assert_eq!(std::fs::read(&path).expect("no extra save"), json);
            let (edited, _) = load_configuration(Some(&path)).expect("edited");
            assert_eq!(
                serde_json::to_value(&edited.condor.scenes[0].sequence_data.zoner.lifecycle)
                    .expect("relocated proof"),
                proof
            );
            assert!(
                edited.condor.scenes.iter().skip(1).all(|s| s
                    .sequence_data
                    .zoner
                    .lifecycle
                    .is_none())
            );
            let mut direct = edited
                .instantiate_condor(
                    Box::new(|_| anyhow::bail!("unchanged definitions must not save")),
                    &Default::default(),
                )
                .expect("direct");
            Zoner::run(&mut direct, Default::default()).expect("direct unchanged skip after edit");
            if phase >= 2 {
                condor_cmd(&temp).args(["zone", "240 480 replace x264"]).assert().failure();
                assert_eq!(std::fs::read(&path).expect("unsafe rebase preserved"), json);
            }
        }
        if phase <= 2 {
            return;
        }
        condor_cmd(&temp).args(["scenes", "reset"]).assert().success();
        let (empty, _) = load_configuration(Some(&path)).expect("empty");
        assert!(empty.condor.scenes.is_empty());
        condor_cmd(&temp).args(["scenes", "add", "0", "60"]).assert().success();
        let (fresh, _) = load_configuration(Some(&path)).expect("fresh");
        assert!(fresh.condor.scenes[0].sequence_data.zoner.lifecycle.is_none());
    }

    fn check_zone_encoder_changes(phase: u8) {
        assert!((1..=3).contains(&phase));
        if !ffmpeg_is_available() {
            return;
        }
        let test_video = get_test_video();
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let output = temp.path().join("out.mkv");
        let config_path = temp.path().join("condor.json");

        condor_cmd(&temp)
            .args(["init", path_str(&test_video.path), path_str(&output)])
            .assert()
            .success();

        // Mock scenes and save to config file (simulates Scene Detector)
        let (mut config, _) =
            load_configuration(Some(&config_path)).expect("load_configuration should succeed");
        let original = config.condor.encoder.clone();
        config.condor.scenes = test_video.mock_scenes(&original);
        config.save(&config_path).expect("save should succeed");

        let result = condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .env("RUST_LOG", "california_condor=info")
            .args(["zone", "0 240 replace x265 \"--crf 18\"", "480 -1 svt-av1 \"--preset 3\""])
            .assert()
            .success();
        assert_info_logs(
            result.get_output(),
            &[
                "Zoner: applying 2 zones to 5 scenes",
                "zone 1 (0..240) modified encoder settings for scene 0..121 (svt-av1 -> x265)",
                "zone 1 (0..240) modified encoder settings for scene 121..240",
                "zone 2 (480..720) modified encoder settings for scene 480..528",
                "zone 2 (480..720) modified encoder settings for scene 528..720",
                "zone 1 (0..240) matched 2 scenes, modified 2, split 0, added 0 (0 unchanged)",
                "Zoner finished: 2 zones, 4 scene changes across zones, 0 scenes added (5 -> 5 \
             total), 0 warnings",
            ],
        );
        assert!(
            !String::from_utf8_lossy(&result.get_output().stderr)
                .contains("modified encoder settings for scene 240..480"),
            "untouched scenes are not reported as modified"
        );

        let (config, _) =
            load_configuration(Some(&config_path)).expect("load_configuration should succeed");

        let zoner = config.condor.sequence_config.zoner.as_ref().expect("the zones are configured");
        assert_eq!(zoner.zones.len(), 2, "both zones are configured");
        assert_eq!(
            config.condor.scenes.len(),
            test_video.scenes.len(),
            "no scene is split"
        );

        // The test video's scenes are (0, 121), (121, 240), (240, 480), (480,
        // 528), (528, 720).
        let expected = [
            x265(18.0),
            x265(18.0),
            original.clone(),
            with_preset(original.clone(), 3.0),
            with_preset(original, 3.0),
        ];
        config.condor.scenes.iter().zip(expected.iter()).enumerate().for_each(
            |(index, (scene, expected_encoder))| {
                check_encoder(
                    &scene.encoder,
                    expected_encoder,
                    &format!("scene {index} encoder"),
                );
            },
        );

        // The zoning pass records itself on every scene, which is the
        // evidence later runs read.
        assert!(
            config.condor.scenes.iter().all(|scene| scene.sequence_data.zoner.zoned),
            "every scene records that the zones were applied"
        );
        if phase == 1 {
            return;
        }

        let saved = std::fs::read(&config_path).expect("config bytes");
        let repeated = condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .env("RUST_LOG", "california_condor=info")
            .arg("zone")
            .assert()
            .success();
        assert_info_logs(
            repeated.get_output(),
            &["Zoner: the scenes already \
                                                   record a zoning pass; nothing \
                                                   to apply"],
        );
        let stderr = String::from_utf8_lossy(&repeated.get_output().stderr);
        assert!(
            !stderr.contains("Zoner finished") && !stderr.contains("applying"),
            "the recorded pass keeps the second run from applying anything"
        );
        assert_eq!(
            std::fs::read(&config_path).expect("config bytes"),
            saved,
            "a skipped run saves nothing"
        );
        if phase == 2 {
            return;
        }

        // Re-applying needs the record cleared, which `av1an scenes reset
        // zoner` does; the zones themselves were rewritten in between.
        let (mut reloaded, _) =
            load_configuration(Some(&config_path)).expect("load_configuration should succeed");
        for scene in &mut reloaded.condor.scenes {
            scene.sequence_data.zoner.zoned = false;
        }
        reloaded.save(&config_path).expect("save should succeed");

        let mismatched = condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .env("RUST_LOG", "california_condor=info")
            .args(["zone", "0 240 x264 \"--crf 20\""])
            .assert()
            .success();
        assert_info_logs(
            mismatched.get_output(),
            &[
                "zone 1 (0..240) matched 2 scenes, modified 0, split 0, added 0 (2 unchanged)",
                "Zoner finished: 1 zones, 0 scene changes across zones, 0 scenes added (5 -> 5 \
             total), 2 warnings",
            ],
        );
    }

    #[serial]
    #[test]
    fn cli_bad_json_after_valid_inline_zone_preserves_job_and_files() {
        let video = get_test_video();
        let temp = tempfile::tempdir().expect("fixture");
        let config_path = temp.path().join("condor.json");
        let mut config = california_condor::test_helpers::default_config(
            &video,
            &temp.path().join("out.mkv"),
            temp.path(),
        );
        config.condor.scenes = video.mock_scenes(&config.condor.encoder);
        config.condor.scenes[0].sequence_data.parallel_encoder.bytes = Some(123);
        config.condor.sequence_config.zoner = Some(california_condor::zoner::ZonerConfig {
            zones: vec![california_condor::zoner::parse_zone("200 300 x264").expect("saved zone")],
        });
        let scenes = &config.condor.sequence_config.parallel_encoder.scenes_directory;
        std::fs::create_dir_all(scenes.join("Target Quality/0")).expect("artifact directories");
        let encode = scenes.join("00000.ivf");
        let probe = scenes.join("Target Quality/0/00000.ivf");
        std::fs::write(&encode, b"saved encode").expect("encode");
        std::fs::write(&probe, b"saved probe").expect("probe");
        config.save(&config_path).expect("saved config");
        let original = std::fs::read(&config_path).expect("config bytes");
        let source = std::fs::read(&video.path).expect("source bytes");
        std::fs::write(
            temp.path().join("typo.json"),
            r#"{"zone": [{"end_frame": 200}]}"#,
        )
        .expect("bad zone file");

        for dry_run in [false, true] {
            let mut command = condor_cmd(&temp);
            command.arg("zone");
            if dry_run {
                command.arg("--dry-run");
            }
            let result = command.args(["0 100 replace x265", "typo.json"]).assert().failure();
            let stderr = String::from_utf8_lossy(&result.get_output().stderr);
            assert!(stderr.contains("typo.json"), "{stderr}");
            assert!(stderr.contains("unknown field `zone`"), "{stderr}");
            assert_eq!(
                std::fs::read(&config_path).expect("unchanged config"),
                original
            );
            assert!(!config_path.with_extension("temp").exists());
            assert_eq!(
                std::fs::read(&video.path).expect("unchanged source"),
                source
            );
            assert_eq!(
                std::fs::read(&encode).expect("unchanged encode"),
                b"saved encode"
            );
            assert_eq!(
                std::fs::read(&probe).expect("unchanged probe"),
                b"saved probe"
            );
        }
    }

    fn check_invalid_zoning(empty_scenes: bool) {
        let video = get_test_video();
        let temp = tempfile::tempdir().expect("temp directory");
        let config_path = temp.path().join("condor.json");
        condor_cmd(&temp)
            .args(["init", path_str(&video.path), "out.mkv"])
            .assert()
            .success();
        let (mut config, _) = load_configuration(Some(&config_path)).expect("config loads");
        config.condor.scenes = video.mock_scenes(&config.condor.encoder);
        config.save(&config_path).expect("config saves");
        let original = std::fs::read(&config_path).expect("config bytes");
        std::fs::write(temp.path().join("empty.json"), "[]").expect("empty zone file");
        std::fs::write(temp.path().join("invalid-filter.json"),
            r#"[{"start_frame":0,"end_frame":100,"encoder":{"Merge":{"base":"FFmpeg","ffmpeg_filter":"hflip"}}}]"#
        ).expect("invalid filter zone file");
        for zones in [
            vec!["0 721 svt-av1"],
            vec!["100 100 svt-av1"],
            vec!["0 100 --maximum-frames 0"],
            vec!["0 200 svt-av1", "100 300 svt-av1"],
            vec!["0 100 ffmpeg --ffmpeg-filter hflip"],
            vec!["empty.json"],
            vec!["invalid-filter.json"],
            vec!["0 100 av2"],
            vec![],
        ] {
            for dry_run in [false, true] {
                let mut command = condor_cmd(&temp);
                command.env("CONDOR_TEST_MODE", "1").arg("zone");
                if dry_run {
                    command.arg("--dry-run");
                }
                command.args(&zones).assert().failure();
                assert_eq!(std::fs::read(&config_path).expect("config bytes"), original);
                assert!(!config_path.with_extension("temp").exists());
            }
        }
        if !empty_scenes {
            return;
        }
        config.condor.scenes.clear();
        config.save(&config_path).expect("empty scenes save");
        let original = std::fs::read(&config_path).expect("config bytes");
        condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .args(["zone", "0 100 svt-av1"])
            .assert()
            .failure();
        assert_eq!(std::fs::read(&config_path).expect("config bytes"), original);
    }

    fn check_zoning_result_withdrawal(phase: u8) {
        assert!((1..=3).contains(&phase));
        use andean_condor::core::sequence::parallel_encoder::ParallelEncoder;
        use california_condor::zoner::{ZonerConfig, parse_zone};

        let video = get_test_video();
        let temp = tempfile::tempdir().expect("temp directory");
        let config_path = temp.path().join("condor.json");
        condor_cmd(&temp)
            .args(["init", path_str(&video.path), "out.mkv"])
            .assert()
            .success();
        let (mut config, _) = load_configuration(Some(&config_path)).expect("config loads");
        config.condor.scenes = video.mock_scenes(&config.condor.encoder);
        config.condor.scenes[0].sequence_data.parallel_encoder.bytes = Some(123);
        config.condor.scenes[1].sequence_data.quality_check.quality.scores = vec![42.0];
        let definition = "-1 121 svt-av1 \"--crf 23\" --maximum-frames 96";
        config.condor.sequence_config.zoner = Some(ZonerConfig {
            zones: vec![parse_zone(definition).expect("zone parses")],
        });
        let scenes_dir = &config.condor.sequence_config.parallel_encoder.scenes_directory;
        let probe_dir = scenes_dir.join("Target Quality").join("1");
        std::fs::create_dir_all(&probe_dir).expect("probe directory");
        let encode = scenes_dir.join(format!("{}.ivf", ParallelEncoder::scene_id(4)));
        let probe = probe_dir.join(format!("{}.ivf", ParallelEncoder::scene_id(1)));
        std::fs::write(&encode, "existing encode").expect("encode fixture");
        std::fs::write(&probe, "existing probe").expect("probe fixture");
        let zone_file = temp.path().join("zones.txt");
        std::fs::write(&zone_file, definition).expect("zone file");
        let zone =
            &config.condor.sequence_config.zoner.as_ref().expect("configured zones").zones[0];
        std::fs::write(
            temp.path().join("zones.json"),
            serde_json::to_vec(&vec![zone]).expect("zone serializes"),
        )
        .expect("JSON zone file");
        std::fs::write(
            temp.path().join("zones.jsonl"),
            serde_json::to_string(zone).expect("zone serializes"),
        )
        .expect("JSONL zone file");
        config.save(&config_path).expect("config saves");
        let original = std::fs::read(&config_path).expect("config bytes");

        for args in [
            vec!["zone", "--dry-run", definition],
            vec!["zone", "zones.txt", "--dry-run"],
            vec!["zone", "--dry-run", "zones.json"],
            vec!["zone", "--dry-run", "zones.jsonl"],
            vec!["zone", "--dry-run"],
        ] {
            let result = condor_cmd(&temp)
                .env("CONDOR_TEST_MODE", "1")
                .env("RUST_LOG", "california_condor=info")
                .args(args)
                .assert()
                .success();
            assert_info_logs(
                result.get_output(),
                &[
                    "Zoner dry-run:",
                    "modified encoder settings for scene 0..121",
                    "split scene 0..121 into 2 scenes (1 added)",
                    "matched 1 scenes, modified 1, split 1, added 1 (0 unchanged)",
                    "no configuration changes were saved",
                ],
            );
            let stderr = String::from_utf8_lossy(&result.get_output().stderr);
            assert!(stderr.contains("scene index 0") && stderr.contains("saved encode results"));
            assert!(stderr.contains("scene index 4 has an existing encode/probe file"));
            assert!(stderr.contains(path_str(&probe)));
            assert_eq!(std::fs::read(&config_path).expect("config bytes"), original);
            assert!(!config_path.with_extension("temp").exists());
            assert_eq!(
                std::fs::read_to_string(&encode).expect("encode file"),
                "existing encode"
            );
            assert_eq!(
                std::fs::read_to_string(&probe).expect("probe file"),
                "existing probe"
            );
        }
        if phase == 1 {
            return;
        }

        let result = condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .env("RUST_LOG", "california_condor=info")
            .arg("zone")
            .assert()
            .success();
        assert!(String::from_utf8_lossy(&result.get_output().stderr).contains("may be stale"));
        let (updated, _) = load_configuration(Some(&config_path)).expect("config loads");
        assert_eq!(updated.condor.scenes.len(), 6);
        assert_eq!(
            updated.condor.scenes[0].sequence_data.parallel_encoder.bytes,
            None
        );
        assert_eq!(
            updated.condor.scenes[2].sequence_data.quality_check.quality.scores,
            Vec::<f64>::new()
        );
        assert!(updated.condor.scenes[0].sequence_data.parallel_encoder.invalidated);
        assert!(updated.condor.scenes[2].sequence_data.parallel_encoder.invalidated);
        assert_eq!(
            std::fs::read_to_string(&encode).expect("encode file"),
            "existing encode"
        );
        assert_eq!(
            std::fs::read_to_string(&probe).expect("probe file"),
            "existing probe"
        );
        if phase == 2 {
            return;
        }

        let no_op = condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .env("RUST_LOG", "california_condor=info")
            .args(["zone", "--dry-run"])
            .assert()
            .success();
        assert!(
            !String::from_utf8_lossy(&no_op.get_output().stderr).contains("may be stale"),
            "no-op zoning does not warn about cleanup"
        );
    }

    #[serial]
    #[test]
    fn zone_inline_or_json_definition_saves_exact_half_open_partitions_and_selected_encoder() {
        check_zone_partitions(1);
    }

    #[serial]
    #[test]
    fn zone_dry_run_reports_partition_and_override_without_saving() {
        check_zone_partitions(2);
    }

    #[serial]
    #[test]
    fn zone_unchanged_saved_definitions_do_not_resave() {
        check_zone_partitions(3);
    }

    #[serial]
    #[test]
    fn legacy_zoner_migration_publication_failure_restores_runtime_and_saved_job() {
        check_legacy_zoner_migration(1);
    }

    #[serial]
    #[test]
    fn legacy_zoner_manual_edits_or_future_revision_refuse_without_resave() {
        check_legacy_zoner_migration(2);
    }

    #[serial]
    #[test]
    fn legacy_zoner_migration_repartitions_from_baseline_and_invalidates_old_results_without_unlinking()
     {
        check_legacy_zoner_migration(3);
    }

    #[serial]
    #[test]
    fn current_zoner_definitions_skip_cli_and_direct_resave_after_migration() {
        check_legacy_zoner_migration(4);
    }

    #[serial]
    #[test]
    fn equivalent_zoner_algorithm_migration_updates_only_revision_preserving_noise_scaler_results()
    {
        check_equivalent_zoner_migration(1);
    }

    #[serial]
    #[test]
    fn equivalent_zoner_migration_callback_failure_rolls_back_without_losing_old_job() {
        check_equivalent_zoner_migration(2);
    }

    #[serial]
    #[test]
    fn equivalent_zoner_cli_dry_run_does_not_save_and_current_reruns_do_not_scale_noise_twice() {
        check_equivalent_zoner_migration(3);
    }

    #[serial]
    #[test]
    fn legacy_zoner_changed_definitions_future_revision_or_missing_proof_refuse_without_resave() {
        check_equivalent_zoner_migration(4);
    }

    #[serial]
    #[test]
    fn zoner_execute_publication_failure_restores_full_runtime_and_saved_files() {
        check_direct_zoner_publication(1);
    }

    #[serial]
    #[test]
    fn zoner_success_saves_once_and_unchanged_run_or_execute_does_not_resave() {
        check_direct_zoner_publication(2);
    }

    #[serial]
    #[test]
    fn zoner_run_respects_the_job_token_at_admission_and_return() {
        use andean_condor::utils::cancellation::{Cancellation, Cancelled};
        use california_condor::{
            test_helpers::default_config,
            zoner::{Zoner, ZonerConfig, parse_zone},
        };
        use std::{cell::Cell, rc::Rc};

        let video = get_test_video();
        let temp = tempfile::tempdir().expect("temp");
        let mut config = default_config(&video, &temp.path().join("out.mkv"), temp.path());
        config.condor.scenes = video.mock_scenes(&config.condor.encoder);
        config.condor.sequence_config.zoner = Some(ZonerConfig {
            zones: vec![parse_zone("0 121 --maximum-frames 96").expect("zone")],
        });
        let cancellation = Cancellation::default();
        let saves = Rc::new(Cell::new(0));
        let mut job = config
            .instantiate_condor(
                Box::new({
                    let controls = cancellation.clone();
                    let saves = Rc::clone(&saves);
                    move |_| {
                        saves.set(saves.get() + 1);
                        controls.request()?;
                        controls.request()
                    }
                }),
                &cancellation,
            )
            .expect("open fixture");

        let stopped = Cancellation::default();
        stopped.request().expect("graceful before admission");
        let error = Zoner::run(&mut job, stopped).expect_err("do not apply a zone after shutdown");
        assert_eq!(
            error.downcast_ref::<Cancelled>(),
            Some(&Cancelled::Graceful)
        );
        assert_eq!(saves.get(), 0);
        assert!(job.scenes[0].sequence_data.zoner.lifecycle.is_none());

        let error =
            Zoner::run(&mut job, cancellation).expect_err("force during save is not success");
        assert_eq!(error.downcast_ref::<Cancelled>(), Some(&Cancelled::Forced));
        assert_eq!(saves.get(), 1);
    }

    #[serial]
    #[test]
    fn zoner_removing_length_cap_restores_original_partitions_encoder_and_subscenes() {
        check_direct_zoner_publication(3);
    }

    #[serial]
    #[test]
    fn zoner_failed_rebase_callback_preserves_pre_rebase_state() {
        check_direct_zoner_publication(4);
    }

    #[serial]
    #[test]
    fn zoner_manual_encoder_edit_prevents_unsafe_definition_rebase() {
        check_direct_zoner_publication(5);
    }

    #[serial]
    #[test]
    fn zoner_missing_legacy_baseline_refuses_run_and_execute_without_mutation() {
        check_direct_zoner_publication(6);
    }

    #[serial]
    #[test]
    fn legacy_zoned_job_cannot_apply_new_zones_or_erase_flags_without_baseline() {
        check_legacy_zone_recovery(false);
    }

    #[serial]
    #[test]
    fn legacy_zoned_job_full_scene_list_reset_and_rebuild_recovers_fresh_zoning() {
        check_legacy_zone_recovery(true);
    }

    #[serial]
    #[test]
    fn scene_list_edit_moves_zoner_proof_anchor_without_resaving_unchanged_definitions() {
        check_zone_anchor_edit(1);
    }

    #[serial]
    #[test]
    fn zoned_scene_list_edit_does_not_authorize_changed_definition_rebase() {
        check_zone_anchor_edit(2);
    }

    #[serial]
    #[test]
    fn full_scene_list_reset_removes_zoner_proof_from_new_scene() {
        check_zone_anchor_edit(3);
    }

    #[serial]
    #[test]
    fn zones_replace_or_merge_selected_scene_encoders_and_report_only_modified_scenes() {
        check_zone_encoder_changes(1);
    }

    #[serial]
    #[test]
    fn recorded_zoning_pass_skips_reapplication_without_saving() {
        check_zone_encoder_changes(2);
    }

    #[serial]
    #[test]
    fn zone_merge_with_mismatched_encoder_reports_matched_scenes_unchanged_and_two_warnings() {
        check_zone_encoder_changes(3);
    }

    #[serial]
    #[test]
    fn invalid_or_missing_zone_definitions_refuse_normal_and_dry_run_without_saving() {
        check_invalid_zoning(false);
    }

    #[serial]
    #[test]
    fn zoning_without_scenes_refuses_without_resaving() {
        check_invalid_zoning(true);
    }

    #[serial]
    #[test]
    fn zone_dry_run_all_definition_formats_warns_about_affected_results_and_preserves_job_and_chunks()
     {
        check_zoning_result_withdrawal(1);
    }

    #[serial]
    #[test]
    fn zone_partition_change_withdraws_saved_results_but_preserves_shifted_chunk_and_probe_files() {
        check_zoning_result_withdrawal(2);
    }

    #[serial]
    #[test]
    fn noop_zone_dry_run_does_not_warn_about_stale_files() {
        check_zoning_result_withdrawal(3);
    }

    #[serial]
    #[test]
    fn a_zone_file_caps_scene_length() {
        if !ffmpeg_is_available() {
            return;
        }
        let test_video = get_test_video();
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let output = temp.path().join("out.mkv");
        let config_path = temp.path().join("condor.json");
        let zones_path = temp.path().join("zones.json");

        condor_cmd(&temp)
            .args(["init", path_str(&test_video.path), path_str(&output)])
            .assert()
            .success();

        let (mut config, _) =
            load_configuration(Some(&config_path)).expect("load_configuration should succeed");
        let original = config.condor.encoder.clone();
        config.condor.scenes = test_video.mock_scenes(&original);
        config.save(&config_path).expect("save should succeed");

        std::fs::write(
            &zones_path,
            r#"{"zones": [{"start_frame": 0, "end_frame": 240, "maximum_frames": 96, "encoder": null}]}"#,
        )
        .expect("the zone file is written");

        let result = condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .env("RUST_LOG", "california_condor=info")
            .args(["zone", "zones.json"])
            .assert()
            .success();
        assert_info_logs(
            result.get_output(),
            &[
                "zone 1 (0..240) split scene 0..121 into 2 scenes (1 added), frame ranges: [(0, 60), \
             (60, 121)]",
                "zone 1 (0..240) split scene 121..240 into 2 scenes (1 added), frame ranges: [(121, \
             180), (180, 240)]",
                "zone 1 (0..240) matched 2 scenes, modified 2, split 2, added 2 (0 unchanged)",
                "Zoner finished: 1 zones, 2 scene changes across zones, 2 scenes added (5 -> 7 \
             total), 0 warnings",
            ],
        );

        let (config, _) =
            load_configuration(Some(&config_path)).expect("load_configuration should succeed");

        let ranges = config
            .condor
            .scenes
            .iter()
            .map(|scene| (scene.start_frame, scene.end_frame))
            .collect::<Vec<_>>();
        assert_eq!(
            ranges,
            vec![(0, 60), (60, 121), (121, 180), (180, 240), (240, 480), (480, 528), (528, 720)],
            "the zoned scenes are split and the rest are untouched"
        );
        assert!(
            config.condor.scenes.iter().all(|scene| scene.encoder == original),
            "a length cap changes no encoder"
        );
    }
}
