use crate::common::condor_cmd;
use crate::common::config::{init_config, load};
use andean_condor::models::{
    input::Input,
    sequence::{benchmarker::BenchmarkerConfig, target_quality::types::ProbeStatistic},
};
use california_condor::test_helpers::*;
use serial_test::serial;

#[cfg(test)]
mod provenance {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn inherited_probe_options_survive_speed_scaling_final_encode_and_cli_resume() {
        tq_speed_scaler_pipeline(false);
    }

    #[cfg(unix)]
    #[test]
    fn explicit_probe_options_survive_speed_scaling_but_reset_refuses_before_reprobe_or_resave() {
        tq_speed_scaler_pipeline(true);
    }

    #[cfg(unix)]
    fn tq_speed_scaler_pipeline(explicit_probe_options: bool) {
        use andean_condor::{
            core::sequence::{
                Sequence, parallel_encoder::ParallelEncoder, speed_scaler::SpeedScaler,
                target_quality::TargetQuality,
            },
            models::{
                encoder::{Encoder, EncoderBase, cli_parameter::CLIParameter},
                input::ImportMethod,
                sequence::target_quality::{TargetQualityConfig, types::QualityMetric},
            },
        };
        use california_condor::configuration::Configuration;
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().expect("fixture");
        let source = temp.path().join("source.mkv");
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
            .arg(&source)
            .output()
            .expect("installed ffmpeg");
        assert!(generated.status.success(), "{generated:?}");
        let video = TestVideo {
            path: source.clone(),
            width: 128,
            height: 128,
            frames: 2,
            fps_rational: (24, 1),
            format: andean_condor::ffmpeg::FFPixelFormat::YUV420P,
            scenes: vec![(0, 2)],
        };
        let mut config = default_config(
            &video,
            &temp.path().join("out.mkv"),
            &temp.path().join("work"),
        );
        config.condor.input = Input::Video {
            path: source,
            import_method: ImportMethod::FFMS2 {
                index: None,
                cache_path: None,
            },
            filters: vec![],
        };
        let wrapper = temp.path().join("codec");
        std::fs::write(&wrapper, format!("#!/bin/sh\nfor argument do if [ \"$argument\" = -o ]; then printf 'encode\\n' >> '{}'; break; fi; done\nexec x264 \"$@\"\n",temp.path().join("calls").display())).expect("counter");
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755))
            .expect("executable");
        config.condor.encoder = Encoder::default_from_base(&EncoderBase::X264, false);
        config.condor.encoder.set_executable(Some(wrapper));
        config.condor.encoder.set_quantizer(20.0);
        config.condor.encoder.parameters_mut().insert(
            "preset".into(),
            CLIParameter::new_string("--", "=", "medium"),
        );
        config.condor.scenes = video.mock_scenes(&config.condor.encoder);
        config.condor.sequence_config.target_quality = Some(TargetQualityConfig {
            maximum_probes: 1,
            quantizer_range: (20, 20),
            metric: QualityMetric::VMAF {
                target_range: (-1000.0, 1000.0),
                resolution: None,
                scaler: "bicubic".into(),
                threads: 0,
                model: None,
                features: vec![],
            },
            ..Default::default()
        });
        config.condor.sequence_config.speed_scaler.speed_quantizers = vec![(9, 0.0), (9, 51.0)];
        if explicit_probe_options {
            let mut parameters = config.condor.encoder.parameters().clone();
            parameters.insert("preset".into(), CLIParameter::new_string("--", "=", "slow"));
            config
                .condor
                .sequence_config
                .target_quality
                .as_mut()
                .expect("TQ")
                .probing
                .encoder_options = Some(parameters);
        }
        assert_eq!(
            config
                .condor
                .sequence_config
                .target_quality
                .as_ref()
                .expect("TQ")
                .probing
                .encoder_options
                .is_some(),
            explicit_probe_options,
            "the original pipeline control inherits probe options"
        );
        let scenes = &config.condor.sequence_config.parallel_encoder.scenes_directory;
        std::fs::create_dir_all(scenes.join("Target Quality/1/Scene Concatenator"))
            .expect("scratch");
        let file = temp.path().join("condor.json");
        let original = config.clone();
        let saved_path = file.clone();
        let mut job = config
            .instantiate_condor(Box::new(move |data| {
                let mut saved = original.clone();
                saved.condor = data;
                saved.save(&saved_path)?;
                Ok(())
            }))
            .expect("native job");
        let (tx, _rx) = std::sync::mpsc::channel();
        TargetQuality::default()
            .execute_cancel(&mut job, tx, Default::default())
            .expect("real TQ");
        let measured = job.scenes[0].sequence_data.target_quality.passes.clone();
        let raw_encoder = job.scenes[0].encoder.clone();
        let measured_encoder: Encoder = serde_json::from_value(
            measured[0].provenance.as_ref().expect("probe certificate").context["encoder"].clone(),
        )
        .expect("stamped encoder");
        assert_eq!(
            raw_encoder.parameters()["preset"].to_string_value(),
            "medium"
        );
        assert_eq!(
            measured_encoder.parameters()["preset"].to_string_value(),
            if explicit_probe_options {
                "slow"
            } else {
                "medium"
            }
        );
        let before = serde_json::to_vec(&job.scenes).expect("before failed callback");
        let save = std::mem::replace(
            &mut job.save_callback,
            Box::new(|_| anyhow::bail!("speed save failure")),
        );
        let (tx, _rx) = std::sync::mpsc::channel();
        assert!(SpeedScaler::default().execute_cancel(&mut job, tx, Default::default()).is_err());
        assert_eq!(
            serde_json::to_vec(&job.scenes).expect("full rollback"),
            before
        );
        job.save_callback = save;
        let (tx, _rx) = std::sync::mpsc::channel();
        SpeedScaler::default()
            .execute_cancel(&mut job, tx, Default::default())
            .expect("actual downstream speed transition");
        assert_eq!(
            job.scenes[0].encoder.parameters()["preset"].to_string_value(),
            "ultrafast"
        );
        if explicit_probe_options {
            // Both reruns must carry the original raw base, even across a q-only
            // downstream adjustment and a changed SpeedScaler preset.
            let calls = std::fs::read(temp.path().join("calls")).expect("probe calls");
            job.scenes[0].encoder.set_quantizer(21.0);
            for speed in [8, 9] {
                job.sequence_config.speed_scaler.speed_quantizers =
                    vec![(speed, 0.0), (speed, 51.0)];
                let (tx, _rx) = std::sync::mpsc::channel();
                SpeedScaler::default()
                    .execute_cancel(&mut job, tx, Default::default())
                    .expect("legitimate repeated speed transition");
                Configuration::load(&file)
                    .expect("saved")
                    .expect("job")
                    .admit_results()
                    .expect("unchanged Some probe options remain eligible");
            }
            let tq = job.sequence_config.target_quality.as_mut().expect("TQ");
            tq.maximum_probes = 2;
            *tq.metric.target_range_mut() = (-999.0, 999.0);
            let (tx, _rx) = std::sync::mpsc::channel();
            TargetQuality::default()
                .execute_cancel(&mut job, tx, Default::default())
                .expect("target/budget reuse under unchanged Some options");
            assert_eq!(
                std::fs::read(temp.path().join("calls")).expect("no reprobe"),
                calls
            );
        }
        let (tx, _rx) = std::sync::mpsc::channel();
        ParallelEncoder::default().execute_cancel(&mut job,tx,Default::default()).expect("H1: first TQ -> SpeedScaler -> PE must accept certified probe input and actual final preset");
        let saved = Configuration::load(&file).expect("saved model").expect("job");
        saved.admit_results().expect("saved CLI admission after speed scaling");
        let calls = std::fs::read(temp.path().join("calls")).expect("encode counter");
        condor_cmd(&temp)
            .env("CONDOR_TEST_MODE", "1")
            .args(["encode"])
            .assert()
            .success();
        assert_eq!(
            std::fs::read(temp.path().join("calls")).expect("no repeat encode"),
            calls
        );
        assert_eq!(
            serde_json::to_value(&load(&file).condor.scenes[0].sequence_data.target_quality.passes)
                .expect("retained raw history"),
            serde_json::to_value(&measured).expect("measured history")
        );
        if explicit_probe_options {
            let chunk =
                saved.condor.sequence_config.parallel_encoder.scenes_directory.join("00000.264");
            let chunk_bytes = std::fs::read(&chunk).expect("certified chunk");
            condor_cmd(&temp).args(["target-quality", "reset", "params"]).assert().success();
            let reset = load(&file);
            assert!(
                reset
                    .condor
                    .sequence_config
                    .target_quality
                    .as_ref()
                    .expect("TQ")
                    .probing
                    .encoder_options
                    .is_none()
            );
            assert_eq!(
                serde_json::to_value(&reset.condor.scenes).expect("reset preserves results"),
                serde_json::to_value(&saved.condor.scenes).expect("before explicit reset")
            );
            let json = std::fs::read(&file).expect("bytes after explicit reset");
            let admission = reset.admit_results();
            let saves = std::rc::Rc::new(std::cell::Cell::new(0));
            let observed = std::rc::Rc::clone(&saves);
            let mut resumed = reset
                .instantiate_condor(Box::new(move |_| {
                    observed.set(observed.get() + 1);
                    Ok(())
                }))
                .expect("owning direct job");
            let before = serde_json::to_vec(&resumed.scenes).expect("saved history/q");
            let (tx, rx) = std::sync::mpsc::channel();
            let direct =
                TargetQuality::default().execute_cancel(&mut resumed, tx, Default::default());
            let cli = condor_cmd(&temp)
                .args(["encode", "-w", "2"])
                .output()
                .expect("actual saved CLI run");
            let diagnostic = String::from_utf8_lossy(&cli.stderr);
            let transition = &saved.condor.scenes[0]
                .sequence_data
                .target_quality
                .speed_transition
                .as_ref()
                .expect("transition")
                .before;
            assert!(
                admission.is_err() && direct.is_err() && diagnostic.contains("TQ probe"),
                "N1: raw preset=medium, explicit P=slow, recorded before={}; reset Some->None must refuse: admission={admission:?}, direct={direct:?}, CLI={diagnostic}",
                transition.parameters()["preset"].to_string_value()
            );
            assert!(format!("{:#}", admission.expect_err("refusal")).contains("TQ probe"));
            assert!(format!("{:#}", direct.expect_err("refusal")).contains("TQ probe"));
            assert_eq!(
                transition, &raw_encoder,
                "the persisted base is raw, not P and not the first scaled preset"
            );
            assert_eq!(std::fs::read(&file).expect("no handler resave"), json);
            assert_eq!(
                std::fs::read(&chunk).expect("no chunk rewrite"),
                chunk_bytes
            );
            assert_eq!(
                std::fs::read(temp.path().join("calls")).expect("no native work"),
                calls
            );
            assert_eq!(
                serde_json::to_vec(&resumed.scenes).expect("unchanged history/q"),
                before
            );
            assert_eq!(saves.get(), 0);
            assert!(rx.try_iter().next().is_none());
        }
        for edit in ["preset", "probe-options", "encoder"] {
            let mut edited = saved.clone();
            match edit {
                "preset" => {
                    edited.condor.scenes[0]
                        .encoder
                        .parameters_mut()
                        .insert("preset".into(), CLIParameter::new_string("--", "=", "fast"));
                },
                "probe-options" => {
                    edited
                        .condor
                        .sequence_config
                        .target_quality
                        .as_mut()
                        .expect("TQ")
                        .probing
                        .encoder_options =
                        Some(CLIParameter::new_strings("--", "=", &[("preset", "fast")]));
                },
                _ => {
                    edited.condor.scenes[0].encoder =
                        Encoder::default_from_base(&EncoderBase::X265, false);
                },
            }
            let error = edited
                .admit_results()
                .expect_err("only the owning speed transition preserves probe eligibility");
            // An explicit probe preset makes a final-only preset edit a chunk
            // change; inherited probe options must still refuse at TQ itself.
            let owner = if explicit_probe_options && edit == "preset" {
                "encoding"
            } else {
                "TQ probe"
            };
            assert!(
                format!("{error:#}").contains(owner),
                "{edit}: expected {owner} invalidation: {error:#}"
            );
        }
    }

    #[test]
    fn cli_legacy_admission_precedes_handler_resave() {
        let temp = tempfile::tempdir().expect("fixture");
        let mut config = root_edit_fixture(temp.path());
        config.condor.scenes[0].sequence_data.parallel_encoder.bytes = Some(123);
        let file = temp.path().join("job.json");
        let mut value = serde_json::to_value(&config).expect("legacy model");
        value["condor"]["scenes"][0]["sequence_data"]["parallel_encoder"]
            .as_object_mut()
            .expect("object")
            .remove("provenance");
        let original = serde_json::to_vec(&value).expect("original unformatted JSON");
        std::fs::write(&file, &original).expect("legacy bytes");
        let result = condor_cmd(&temp)
            .args(["--config-file", "job.json", "encode", "-w", "2"])
            .output()
            .expect("actual CLI");
        assert_eq!(
            std::fs::read(&file).expect("preserved bytes"),
            original,
            "run handler must not resave untrusted results"
        );
        let message = String::from_utf8_lossy(&result.stderr);
        assert!(
            message.contains("scene 0") && message.contains("new job"),
            "{message}"
        );
        assert!(!temp.path().join("source-opened").exists());
        // Isolate the encoding owner for unknown/future and current-mismatch
        // admission; none of these claimed records can be repaired by a save.
        config.condor.scenes[0].sequence_data = Default::default();
        config.condor.scenes[0].sequence_data.parallel_encoder.bytes = Some(123);
        for revision in [0, 999, 1] {
            let mut value = serde_json::to_value(&config).expect("claimed result");
            value["condor"]["scenes"][0]["sequence_data"]["parallel_encoder"]["provenance"] =
                serde_json::json!({"revision": revision, "context": {}});
            let original = serde_json::to_vec(&value).expect("original exact bytes");
            std::fs::write(&file, &original).expect("original");
            let result = condor_cmd(&temp)
                .args(["--config-file", "job.json", "encode", "-w", "2"])
                .output()
                .expect("CLI admission");
            let message = String::from_utf8_lossy(&result.stderr);
            assert!(
                message.contains("scene 0") && message.contains("new job"),
                "revision {revision}: {message}"
            );
            assert_eq!(std::fs::read(&file).expect("unchanged bytes"), original);
            assert!(!temp.path().join("source-opened").exists());
        }
    }

    fn check_legacy_settings_upgrade(claim_results: bool) {
        use california_condor::configuration::Configuration;
        let temp = tempfile::tempdir().expect("fixture");
        let mut config = root_edit_fixture(temp.path());
        config.condor.scenes[0].sequence_data = Default::default();
        let file = temp.path().join("job.json");
        let mut value = serde_json::to_value(&config).expect("old settings");
        for scene in value["condor"]["scenes"].as_array_mut().expect("scenes") {
            scene["sequence_data"]["parallel_encoder"]
                .as_object_mut()
                .expect("record")
                .remove("provenance");
            scene["sequence_data"]["quality_check"]["quality"]
                .as_object_mut()
                .expect("quality")
                .remove("provenance");
            scene["sequence_data"]["quality_check"]["quality"]
                .as_object_mut()
                .expect("quality")
                .remove("backend");
        }
        std::fs::write(&file, serde_json::to_vec(&value).expect("legacy JSON")).expect("saved");
        condor_cmd(&temp)
            .args(["--config-file", "job.json", "init", "set", "workers", "2"])
            .assert()
            .success();
        let mut upgraded = Configuration::load(&file).expect("raw load").expect("job");
        assert!(upgraded.admit_results().is_ok());
        assert!(
            !temp.path().join("source-opened").exists(),
            "offline settings upgrade does not open a decoder"
        );
        let record = &mut upgraded.condor.scenes[0].sequence_data.parallel_encoder;
        assert!(record.provenance.is_none());
        if !claim_results {
            return;
        }
        record.bytes = Some(999);
        upgraded.save(&file).expect("explicit raw offline save remains possible");
        let claimed = Configuration::load(&file).expect("legacy raw results parse").expect("job");
        assert!(claimed.condor.scenes[0].sequence_data.parallel_encoder.provenance.is_none());
        assert!(
            claimed.admit_results().is_err(),
            "save never mints result provenance"
        );
    }

    #[test]
    fn settings_only_legacy_job_upgrades_offline_without_result_provenance() {
        check_legacy_settings_upgrade(false);
    }

    #[test]
    fn raw_configuration_save_never_certifies_unstamped_claimed_results() {
        check_legacy_settings_upgrade(true);
    }
}

#[cfg(test)]
mod persistence {
    use super::*;

    #[test]
    fn benchmarker_preserves_omitted_memory_and_persists_update_reset() {
        use california_condor::{
            commands::{
                config::ConfigAction,
                handlers::benchmarker::{self, BenchmarkKey, benchmarker_handler},
            },
            configuration::JobLock,
        };

        let temp = tempfile::tempdir().expect("fixture");
        let guard = JobLock::acquire(&temp.path().join("job.json")).expect("transaction guard");
        let mut configuration = root_edit_fixture(temp.path());
        configuration.condor.scenes[0].sequence_data = Default::default();
        configuration.condor.sequence_config.benchmarker = BenchmarkerConfig {
            threshold: 7,
            max_memory: Some(4096),
        };
        configuration.condor.sequence_config.parallel_encoder.workers = Some(3);
        configuration.save(guard.path()).expect("saved memory limit");

        for (threshold, max_memory, expected_memory, expected_threshold) in
            [(None, None, 4096, 7), (Some(11), None, 4096, 11), (None, Some(2048), 2048, 11)]
        {
            benchmarker_handler(Some(guard.path()), None, threshold, max_memory)
                .expect("configuration handler only");
            let saved = load(guard.path());
            let sequence = &saved.condor.sequence_config;
            assert_eq!(
                (
                    sequence.benchmarker.max_memory,
                    sequence.benchmarker.threshold
                ),
                (Some(expected_memory), expected_threshold)
            );
            assert_eq!(sequence.parallel_encoder.workers, Some(3));
        }

        benchmarker::config(
            Some(guard.path()),
            &ConfigAction::Reset {
                key: Some(BenchmarkKey::MaxMemory),
            },
        )
        .expect("explicit memory reset");
        let saved = load(guard.path());
        assert_eq!(saved.condor.sequence_config.benchmarker.max_memory, None);
        assert_eq!(saved.condor.sequence_config.benchmarker.threshold, 11);
        assert!(!temp.path().join("source-opened").exists());
    }
}

#[cfg(test)]
struct JobChild(std::process::Child);

#[cfg(test)]
impl Drop for JobChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(test)]
fn wait_for_marker(child: &mut JobChild, marker: &std::path::Path) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !marker.exists() {
        assert!(
            child.0.try_wait().expect("child status").is_none(),
            "child exited before {}",
            marker.display()
        );
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {}",
            marker.display()
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[cfg(test)]
mod job_lock {
    use super::*;

    fn check_job_contention(phase: u8) {
        assert!((1..=3).contains(&phase));
        let temp = tempfile::tempdir().expect("fixture");
        let mut config = root_edit_fixture(temp.path());
        config.condor.scenes[0].sequence_data = Default::default();
        let file = temp.path().join("job.json");
        config.save(&file).expect("job");
        config.save(&temp.path().join("job.other")).expect("independent extension");
        let literal = |name: &str| serde_json::to_string(&temp.path().join(name)).expect("literal");
        std::fs::write(temp.path().join("a.vpy"), format!(
            "from pathlib import Path\nimport time\nPath({}).write_text('entered')\ndeadline=time.monotonic()+45\nwhile not Path({}).exists():\n    if time.monotonic()>deadline: raise RuntimeError('job lock watchdog')\n    time.sleep(0.01)\nraise RuntimeError('job lock released without encoding')\n", literal("entered"), literal("release")
        )).expect("bounded native pause");
        let original = std::fs::read(&file).expect("original");
        let mut child = JobChild(
            std::process::Command::new(env!("CARGO_BIN_EXE_av1an"))
                .current_dir(temp.path())
                .args(["--config-file", "job.json", "encode", "-w", "2"])
                .stdout(std::process::Stdio::null())
                .stderr(std::fs::File::create(temp.path().join("holder.log")).expect("child log"))
                .spawn()
                .expect("real CLI holder"),
        );
        wait_for_marker(&mut child, &temp.path().join("entered"));
        let saved = std::fs::read(&file).expect("real handler save before native open");
        assert_ne!(saved, original);
        assert_eq!(
            load(&file).condor.sequence_config.parallel_encoder.workers,
            Some(2)
        );
        let mut aliases = vec!["job.json", "./job.json", "child/../job.json"];
        std::fs::create_dir_all(temp.path().join("child")).expect("lexical alias");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("job.json", temp.path().join("alias.json"))
                .expect("file alias");
            std::os::unix::fs::symlink(".", temp.path().join("parent-alias"))
                .expect("parent alias");
            aliases.extend(["alias.json", "parent-alias/job.json"]);
        }
        let mut failures = vec![];
        for alias in aliases {
            for action in [
                vec!["init", "set", "workers", "7"],
                vec!["scenes", "0", "set", "end", "10"],
                vec!["init", "info"],
            ] {
                let output = condor_cmd(&temp)
                    .timeout(std::time::Duration::from_secs(5))
                    .args(["--config-file", alias])
                    .args(&action)
                    .output()
                    .expect("contender CLI");
                let message = String::from_utf8_lossy(&output.stderr);
                if output.status.code() != Some(1)
                    || !message.contains("another process")
                    || !message.contains("job.json.lock")
                {
                    failures.push(format!(
                        "{alias}/{action:?}: expected busy before load/save, got {:?}: {message}",
                        output.status
                    ));
                }
            }
        }
        if phase >= 2 {
            condor_cmd(&temp)
                .timeout(std::time::Duration::from_secs(5))
                .args(["--config-file", "job.other", "init", "set", "workers", "3"])
                .assert()
                .success();
            assert_eq!(
                load(&temp.path().join("job.other"))
                    .condor
                    .sequence_config
                    .parallel_encoder
                    .workers,
                Some(3)
            );
        }
        if std::fs::read(&file).expect("contended bytes") != saved {
            failures.push("contender changed the holder's saved job".into());
        }
        if phase == 3 {
            child.0.kill().expect("crash holder");
            child.0.wait().expect("reap holder");
            condor_cmd(&temp)
                .args(["--config-file", "job.json", "scenes", "0", "set", "end", "11"])
                .assert()
                .success();
            let recovered = load(&file);
            assert_eq!(
                recovered.condor.sequence_config.parallel_encoder.workers,
                Some(2),
                "holder edit retained after crash"
            );
            assert_eq!(recovered.condor.scenes[0].end_frame, 11);
            assert!(
                temp.path().join("job.json.lock").exists(),
                "crash release does not delete stable sidecar"
            );
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn cli_job_lock_alias_contenders_remain_busy_after_holder_saves() {
        check_job_contention(1);
    }

    #[test]
    fn cli_job_lock_different_extension_allows_independent_edit() {
        check_job_contention(2);
    }

    #[test]
    fn cli_job_lock_holder_crash_releases_lock_without_losing_saved_edits() {
        check_job_contention(3);
    }
}

#[cfg(test)]
mod publication {
    use super::*;

    #[test]
    fn save_preserves_caller_owned_fixed_temp_sentinel() {
        let temp = tempfile::tempdir().expect("fixture");
        let config = root_edit_fixture(temp.path());
        let file = temp.path().join("job.json");
        let sentinel = temp.path().join("job.temp");
        std::fs::write(&sentinel, b"caller-owned staging sentinel").expect("sentinel");
        config.save(&file).expect("actual Configuration save");
        assert_eq!(
            std::fs::read(&sentinel).expect("must not consume caller temp"),
            b"caller-owned staging sentinel"
        );
        assert_eq!(load(&file).input, config.input);
        let mut names = std::fs::read_dir(temp.path())
            .expect("fixture")
            .map(|e| e.expect("entry").file_name())
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(
            names,
            ["a.vpy", "b.vpy", "job.json", "job.temp"].map(std::ffi::OsString::from)
        );
    }
}

#[cfg(test)]
mod job_admission {
    use super::*;

    #[test]
    fn missing_existing_job_routes_leave_no_artifacts() {
        fn snapshot(
            directory: &std::path::Path,
        ) -> std::collections::BTreeMap<std::path::PathBuf, Option<Vec<u8>>> {
            let mut entries = std::collections::BTreeMap::new();
            for entry in std::fs::read_dir(directory).expect("fixture directory") {
                let path = entry.expect("entry").path();
                if path.is_dir() {
                    entries.insert(path.clone(), None);
                    entries.extend(snapshot(&path));
                } else {
                    entries.insert(path.clone(), Some(std::fs::read(path).expect("bytes")));
                }
            }
            entries
        }
        let mut failures = vec![];
        for selected in [Some("typo/deep/job.json"), Some("existing/job.json"), None] {
            for route in [
                vec!["scenes"],
                vec!["scenes", "0", "set", "end", "11"],
                vec!["scenes", "all", "reset", "encoder"],
                vec!["init", "info"],
                vec!["init", "set", "workers", "2"],
                vec!["init", "reset"],
                vec!["encode", "info"],
                vec!["encode", "reset"],
                vec!["encode"],
                vec!["zone"],
            ] {
                let temp = tempfile::tempdir().expect("fixture");
                root_edit_fixture(temp.path());
                std::fs::create_dir_all(temp.path().join("existing")).expect("existing parent");
                let before = snapshot(temp.path());
                let mut command = condor_cmd(&temp);
                command.env("PATH", "").timeout(std::time::Duration::from_secs(5));
                if let Some(selected) = selected {
                    command.args(["--config-file", selected]);
                }
                let result = command.args(&route).output().expect("real missing-job CLI");
                let message = String::from_utf8_lossy(&result.stderr);
                let expected = if selected.is_some() {
                    "No config file found at:"
                } else {
                    "Run 'av1an init' to create a configuration."
                };
                if result.status.code() != Some(1) || !message.contains(expected) {
                    failures.push(format!(
                        "{selected:?}/{route:?}: missing guidance: {message}"
                    ));
                }
                let after = snapshot(temp.path());
                if after != before {
                    failures.push(format!(
                        "{selected:?}/{route:?}: created artifacts {:?}",
                        after.keys().filter(|path| !before.contains_key(*path)).collect::<Vec<_>>()
                    ));
                }
                assert!(!temp.path().join("source-opened").exists());
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn init_and_new_root_create_missing_config_parents_and_lock_before_processing() {
        for root in [false, true] {
            let temp = tempfile::tempdir().expect("fixture");
            let source = temp.path().join("source.vpy");
            let config_path =
                serde_json::to_string(&temp.path().join("new/deep/job.json")).expect("literal");
            std::fs::write(&source, format!("from pathlib import Path\nif Path({config_path}).exists(): raise RuntimeError('job lock root stopped after creation')\nimport vapoursynth as vs\nvs.core.std.BlankClip(width=32,height=16,length=2,format=vs.YUV420P8).set_output()\n")).expect("bounded source; root stops before processing");
            let mut command = condor_cmd(&temp);
            command
                .timeout(std::time::Duration::from_secs(30))
                .args(["--config-file", "new/deep/job.json"]);
            if root {
                command.args(["-i", "source.vpy", "-o", "out.mkv", "-w", "1"]);
            } else {
                command.args(["init", "source.vpy", "out.mkv"]);
            }
            let result = command.output().expect("actual creating CLI");
            if root {
                assert_eq!(result.status.code(), Some(1));
                assert!(
                    String::from_utf8_lossy(&result.stderr)
                        .contains("job lock root stopped after creation")
                );
            } else {
                assert!(result.status.success(), "{result:?}");
            }
            let file = temp.path().join("new/deep/job.json");
            assert_eq!(load(&file).input, source);
            assert!(temp.path().join("new/deep/job.json.lock").is_file());
            assert!(!temp.path().join("out.mkv").exists());
        }
    }

    #[test]
    fn config_parent_is_file_reports_context_without_missing_config_guidance_or_artifacts() {
        let temp = tempfile::tempdir().expect("fixture");
        std::fs::write(temp.path().join("blocked"), b"not a directory").expect("fixture");
        let result = condor_cmd(&temp)
            .args(["--config-file", "blocked/job.json", "scenes"])
            .output()
            .expect("CLI");
        assert_eq!(result.status.code(), Some(1));
        let message = String::from_utf8_lossy(&result.stderr);
        assert!(!message.contains("No config file found"), "{message}");
        assert!(message.contains("blocked/job.json"), "{message}");
        assert_eq!(
            std::fs::read_dir(temp.path()).expect("no artifacts").count(),
            1
        );
        assert_eq!(
            std::fs::read(temp.path().join("blocked")).expect("retained bytes"),
            b"not a directory"
        );
    }

    fn check_busy_job_admission(phase: u8) {
        assert!((1..=4).contains(&phase));
        use california_condor::configuration::JobLock;
        let temp = tempfile::tempdir().expect("fixture");
        let config = root_edit_fixture(temp.path());
        let file = temp.path().join("job.json");
        let guard = JobLock::acquire(&file).expect("direct API guard before first load");
        for route in [vec!["init", "a.vpy", "out.mkv"], vec!["-i", "a.vpy", "-o", "out.mkv"]] {
            let result = condor_cmd(&temp)
                .timeout(std::time::Duration::from_secs(5))
                .args(["--config-file", "job.json"])
                .args(&route)
                .output()
                .expect("CLI");
            assert_eq!(result.status.code(), Some(1), "{route:?}");
            let message = String::from_utf8_lossy(&result.stderr);
            assert!(
                message.contains("another process") && message.contains("job.json.lock"),
                "{route:?}: {message}"
            );
            assert!(!file.exists());
            assert!(!temp.path().join("source-opened").exists());
        }
        if phase == 1 {
            return;
        }
        // A corrupt file proves busy admission precedes parsing, not just writes.
        std::fs::write(&file, b"not JSON").expect("corrupt config");
        for route in [
            vec!["encode"],
            vec!["init", "info"],
            vec!["zone", "0,12,crf=20"],
            vec!["scenes"],
        ] {
            let result = condor_cmd(&temp)
                .timeout(std::time::Duration::from_secs(5))
                .args(["--config-file", "job.json"])
                .args(&route)
                .output()
                .expect("existing-job CLI");
            assert_eq!(result.status.code(), Some(1));
            assert!(String::from_utf8_lossy(&result.stderr).contains("another process"));
            assert_eq!(std::fs::read(&file).expect("bytes"), b"not JSON");
            assert!(!temp.path().join("source-opened").exists());
        }
        if phase == 2 {
            return;
        }
        drop(guard);
        config.save(&file).expect("fresh valid snapshot after release");
        let guard = JobLock::acquire(&file).expect("reacquire despite existing sidecar");
        for args in [vec!["--help"], vec!["--version"], vec!["clean"]] {
            let result = condor_cmd(&temp)
                .args(["--config-file", "job.json"])
                .args(&args)
                .output()
                .expect("offline control");
            let message = String::from_utf8_lossy(&result.stderr);
            assert!(!message.contains("another process"), "{args:?}: {message}");
            assert_eq!(result.status.success(), args[0] != "clean");
        }
        if phase == 3 {
            return;
        }
        let invalid = condor_cmd(&temp)
            .args(["--config-file", "job.json", "-w", "8", "encode"])
            .output()
            .expect("invalid placement");
        assert_eq!(invalid.status.code(), Some(2));
        assert!(!String::from_utf8_lossy(&invalid.stderr).contains("another process"));
        assert!(!temp.path().join("source-opened").exists());
        drop(guard);
    }

    #[test]
    fn busy_job_lock_refuses_new_job_before_config_creation_or_source_open() {
        check_busy_job_admission(1);
    }

    #[test]
    fn busy_job_lock_precedes_corrupt_config_parsing_for_existing_routes() {
        check_busy_job_admission(2);
    }

    #[test]
    fn help_version_and_unavailable_clean_bypass_busy_job_lock() {
        check_busy_job_admission(3);
    }

    #[test]
    fn misplaced_root_flags_fail_parsing_before_busy_job_lock() {
        check_busy_job_admission(4);
    }

    #[test]
    fn cli_guard_survives_real_zoner_callback_until_run_returns() {
        use andean_condor::models::sequence::noise_detector::NoiseDetectorConfig;
        use california_condor::{
            configuration::Configuration,
            zoner::{ZonerConfig, parse_zone},
        };
        let temp = tempfile::tempdir().expect("fixture");
        let mut config = root_edit_fixture(temp.path());
        config.condor.scenes[0].sequence_data = Default::default();
        config.condor.sequence_config.zoner = Some(ZonerConfig {
            zones: vec![parse_zone("0 12 svt-av1 \"--preset 3\"").expect("zone")],
        });
        let literal = |name: &str| serde_json::to_string(&temp.path().join(name)).expect("literal");
        std::fs::write(temp.path().join("b.vpy"), format!(
            "from pathlib import Path\nimport time\nPath({}).write_text('after Zoner callback')\ndeadline=time.monotonic()+45\nwhile not Path({}).exists():\n    if time.monotonic()>deadline: raise RuntimeError('job lock watchdog')\n    time.sleep(0.01)\nraise RuntimeError('job lock intentional stage abort before processing')\n", literal("entered"), literal("release")
        )).expect("pause next native stage, never run noise detection/benchmark/encoding");
        config.condor.sequence_config.noise_detector = Some(NoiseDetectorConfig {
            input: Some(
                Configuration::new_input_model(&temp.path().join("b.vpy"), None, None, None, None)
                    .expect("stage model"),
            ),
            reference_filters: vec![],
            denoised_filters: vec![],
        });
        let file = temp.path().join("job.json");
        config.save(&file).expect("job");
        let mut child = JobChild(
            std::process::Command::new(env!("CARGO_BIN_EXE_av1an"))
                .current_dir(temp.path())
                .args(["--config-file", "job.json", "--skip-scd", "-w", "2"])
                .stdout(std::process::Stdio::null())
                .stderr(std::fs::File::create(temp.path().join("holder.log")).expect("log"))
                .spawn()
                .expect("root CLI"),
        );
        wait_for_marker(&mut child, &temp.path().join("entered"));
        let snapshot = std::fs::read(&file).expect("callback saved bytes");
        assert!(
            load(&file).condor.scenes[0].sequence_data.zoner.zoned,
            "real instantiate_saved_condor callback finished"
        );
        let result = condor_cmd(&temp)
            .timeout(std::time::Duration::from_secs(5))
            .args(["--config-file", "job.json", "init", "set", "workers", "7"])
            .output()
            .expect("contender");
        assert_eq!(result.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&result.stderr).contains("another process"));
        assert_eq!(
            std::fs::read(&file).expect("unchanged callback snapshot"),
            snapshot
        );
        std::fs::write(temp.path().join("release"), b"return").expect("release native stage");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let status = loop {
            if let Some(status) = child.0.try_wait().expect("child status") {
                break status;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "root run did not return"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        assert_eq!(status.code(), Some(1));
        assert!(
            std::fs::read_to_string(temp.path().join("holder.log"))
                .expect("log")
                .contains("job lock intentional stage abort")
        );
        condor_cmd(&temp)
            .args(["--config-file", "job.json", "init", "set", "workers", "3"])
            .assert()
            .success();
        let saved = load(&file);
        assert_eq!(
            saved.condor.sequence_config.parallel_encoder.workers,
            Some(3)
        );
        assert!(
            saved.condor.scenes[0].sequence_data.zoner.zoned,
            "callback update retained after later edit"
        );
    }
}

#[cfg(test)]
fn root_edit_fixture(
    directory: &std::path::Path,
) -> california_condor::configuration::Configuration {
    use andean_condor::models::input::VapourSynthScriptSource;
    let source = directory.join("a.vpy");
    for (name, color) in [("a.vpy", 16), ("b.vpy", 235)] {
        let marker =
            serde_json::to_string(&directory.join("source-opened")).expect("Python path literal");
        std::fs::write(directory.join(name), format!("from pathlib import Path\nPath({marker}).write_text('opened')\nimport vapoursynth as vs\nvs.core.std.BlankClip(width=32,height=16,length=12,format=vs.YUV420P8,color=[{color},128,128]).set_output()\n")).expect("equal-length script sources");
    }
    let video = TestVideo {
        path: source.clone(),
        width: 32,
        height: 16,
        frames: 12,
        fps_rational: (24, 1),
        format: andean_condor::ffmpeg::FFPixelFormat::YUV420P,
        scenes: vec![(0, 12)],
    };
    let mut configuration = default_config(
        &video,
        &directory.join("out.mkv"),
        &directory.join(california_condor::utils::hash_path::hash_path(&source)),
    );
    configuration.condor.input = Input::VapourSynthScript {
        source: VapourSynthScriptSource::Path(source),
        variables: [("keep".into(), "yes".into())].into(),
        index: 0,
        filters: vec![],
        stream_concurrently: false,
    };
    configuration.condor.scenes = video.mock_scenes(&configuration.condor.encoder);
    let data = &mut configuration.condor.scenes[0].sequence_data;
    data.parallel_encoder.started_on = Some(10);
    data.parallel_encoder.completed_on = Some(20);
    data.parallel_encoder.bytes = Some(1234);
    data.quality_check.quality.scores = vec![91.5, 92.5];
    data.target_quality.passes.push(data.quality_check.quality.clone());
    configuration
}

#[cfg(test)]
fn root_edit_args(path: std::path::PathBuf) -> california_condor::commands::config::InputOverride {
    california_condor::commands::config::InputOverride {
        path: Some(path),
        decoder: None,
        filters: None,
        vs_args: None,
        stream_concurrently: None,
    }
}

#[cfg(test)]
mod offline_cli {
    use super::*;

    #[test]
    fn clean_is_an_offline_error_and_preserves_all_job_files() {
        use std::{collections::BTreeMap, path::PathBuf};

        use andean_condor::core::sequence::{
            parallel_encoder::ParallelEncoder, target_quality::DETAILS,
        };

        fn snapshot(directory: &std::path::Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
            let mut entries = BTreeMap::new();
            for entry in std::fs::read_dir(directory).expect("fixture directory") {
                let path = entry.expect("entry").path();
                if path.file_name().expect("filename") == "clean.log" {
                    continue;
                }
                if path.is_dir() {
                    entries.insert(path.clone(), None);
                    entries.extend(snapshot(&path));
                } else {
                    entries.insert(path.clone(), Some(std::fs::read(path).expect("file bytes")));
                }
            }
            entries
        }

        let mut failures = vec![];
        for config in ["valid", "missing", "corrupt", "default-missing"] {
            for all in [false, true] {
                let temp = tempfile::tempdir().expect("fixture");
                let configuration = root_edit_fixture(temp.path());
                configuration.save(&temp.path().join("existing.json")).expect("saved job");
                let file = temp.path().join("custom.json");
                match config {
                    "valid" => configuration.save(&file).expect("custom config"),
                    "corrupt" => {
                        std::fs::write(&file, b"not a configuration\0").expect("corrupt config");
                    },
                    _ => {},
                }
                let scenes =
                    &configuration.condor.sequence_config.parallel_encoder.scenes_directory;
                let chunk = format!("{}.ivf", ParallelEncoder::scene_id(0));
                let probe = scenes.join(DETAILS.name).join("pass-1").join(&chunk);
                std::fs::create_dir_all(probe.parent().expect("probe parent"))
                    .expect("probe directory");
                std::fs::create_dir_all(configuration.temp.join("empty"))
                    .expect("empty work directory");
                for (path, bytes) in [
                    (scenes.join(&chunk), b"encoded chunk\0".as_slice()),
                    (probe, b"probe bytes\0".as_slice()),
                    (temp.path().join("out.mkv"), b"final output\0".as_slice()),
                    (temp.path().join("a.ffindex"), b"source index\0".as_slice()),
                ] {
                    std::fs::write(path, bytes).expect("sentinel");
                }
                let before = snapshot(temp.path());
                let mut command = condor_cmd(&temp);
                command.env("PATH", "").env("RUST_BACKTRACE", "1").args(["--logs", "clean.log"]);
                if config != "default-missing" {
                    command.arg("--config-file").arg(&file);
                }
                command.arg("clean");
                if all {
                    command.arg("--all");
                }
                let output = command.output().expect("actual clean CLI");
                assert_eq!(
                    snapshot(temp.path()),
                    before,
                    "{config}, --all={all}: only the selected log may change"
                );
                assert!(
                    temp.path().join("clean.log").is_file(),
                    "normal logging remains supported"
                );
                let message = String::from_utf8_lossy(&output.stderr);
                if output.status.code() != Some(1)
                    || ![
                        "cleanup is unavailable",
                        "no files removed",
                        "new job",
                        "different --config-file",
                        "distinct empty --temp directory",
                        "distinct output",
                    ]
                    .iter()
                    .all(|text| message.contains(text))
                    || ["panicked", "stack backtrace", "not yet implemented", "todo!"]
                        .iter()
                        .any(|text| message.contains(text))
                {
                    failures.push(format!(
                        "{config}, --all={all}: {:?}: {message}",
                        output.status
                    ));
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn clean_help_discloses_unavailable_command_and_all_flag() {
        let temp = tempfile::tempdir().expect("fixture");
        for args in [vec!["--help"], vec!["clean", "--help"], vec!["clean", "--help", "--verbose"]]
        {
            let output = condor_cmd(&temp).env("PATH", "").args(&args).assert().success();
            let help = String::from_utf8_lossy(&output.get_output().stdout);
            assert!(help.contains("Cleanup is unavailable"), "{args:?}: {help}");
            if args[0] == "clean" {
                let all = help
                    .split("--all")
                    .nth(1)
                    .expect("--all help")
                    .split("\n\n")
                    .next()
                    .expect("flag description");
                assert!(all.contains("not implemented"), "{all}");
                let help = help.split_whitespace().collect::<Vec<_>>().join(" ");
                for text in [
                    "no files removed",
                    "different --config-file",
                    "distinct empty --temp directory",
                    "distinct output",
                ] {
                    assert!(help.contains(text), "missing {text}: {help}");
                }
                for advice in help.split("new job").skip(1) {
                    let advice = advice.split('.').next().expect("new-job guidance");
                    assert!(
                        advice.contains("distinct empty --temp directory")
                            && advice.contains("distinct output"),
                        "every new-job path needs isolated work and output: {advice}"
                    );
                }
            }
        }
        assert_eq!(
            std::fs::read_dir(temp.path()).expect("untouched fixture").count(),
            0
        );
    }

    #[test]
    fn stale_chunk_warning_gives_safe_new_job_advice_without_touching_files() {
        use andean_condor::core::sequence::parallel_encoder::ParallelEncoder;

        let temp = tempfile::tempdir().expect("fixture");
        let configuration = root_edit_fixture(temp.path());
        let file = temp.path().join("custom.json");
        configuration.save(&file).expect("saved config");
        let scenes = &configuration.condor.sequence_config.parallel_encoder.scenes_directory;
        std::fs::create_dir_all(scenes).expect("scene directory");
        let chunk = scenes.join(format!("{}.ivf", ParallelEncoder::scene_id(0)));
        std::fs::write(&chunk, b"stale encode\0").expect("physical chunk");
        let output = condor_cmd(&temp)
            .env("PATH", "")
            .args([
                "--config-file",
                "custom.json",
                "--logs",
                "edit.log",
                "scenes",
                "0",
                "set",
                "end",
                "10",
            ])
            .assert()
            .success();
        let warnings = String::from_utf8_lossy(&output.get_output().stderr);
        let warning = warnings
            .lines()
            .find(|line| line.contains("existing encode/probe file"))
            .expect("physical-file warning");
        for text in [
            "scene index 0",
            chunk.to_str().expect("path"),
            "may be stale after scene edits",
            "file was left untouched",
            "new job",
            "different --config-file",
            "distinct empty --temp directory",
            "distinct output",
        ] {
            assert!(warning.contains(text), "missing {text}: {warning}");
        }
        assert!(
            !warning.contains("clean and recompute") && !warning.contains("av1an clean"),
            "{warning}"
        );
        assert!(
            warnings.contains("Nothing was cleaned or recomputed"),
            "{warnings}"
        );
        assert_eq!(
            std::fs::read(chunk).expect("retained chunk"),
            b"stale encode\0"
        );
        let updated = load(&file);
        assert_eq!(updated.condor.scenes[0].end_frame, 10);
        let data = &updated.condor.scenes[0].sequence_data;
        assert!(data.parallel_encoder.invalidated);
        assert!(
            data.parallel_encoder.bytes.is_none() && data.parallel_encoder.provenance.is_none()
        );
        assert!(data.target_quality.passes.is_empty() && data.target_quality.selection.is_none());
        assert!(data.quality_check.quality.scores.is_empty());
        assert!(
            updated.admit_results().is_err(),
            "preserved files are not eligible chunks"
        );
        assert!(!temp.path().join("source-opened").exists());
    }

    #[test]
    fn misplaced_root_options_fail_before_logging_config_or_native_work() {
        let temp = tempfile::tempdir().expect("fixture");
        let configuration = root_edit_fixture(temp.path());
        let file = temp.path().join("condor.json");
        configuration.save(&file).expect("save fixture");
        let before = std::fs::read(&file).expect("snapshot");
        let source = std::fs::read(temp.path().join("a.vpy")).expect("source");
        let mut failures = vec![];
        for args in [
            vec!["-w", "8", "encode"],
            vec!["--skip-scd", "encode"],
            vec!["-i", "a.vpy", "encode"],
            vec!["-o", "new.mkv", "scenes"],
            vec!["--encoder", "aom", "encode"],
            vec!["--params", "--cpu-used=8", "encode"],
            vec!["--target-metric", "vmaf", "target-quality"],
            vec!["--overwrite", "concatenate"],
        ] {
            let output = condor_cmd(&temp).env("PATH", "").args(&args).output().expect("CLI");
            let message = String::from_utf8_lossy(&output.stderr);
            if output.status.code() != Some(2)
                || !message.contains("cannot be used with")
                || !message.contains("Usage:")
            {
                failures.push(format!(
                    "{args:?}: expected placement conflict, got {message}"
                ));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
        assert_eq!(std::fs::read(file).expect("unchanged config"), before);
        assert_eq!(
            std::fs::read(temp.path().join("a.vpy")).expect("source"),
            source
        );
        for path in ["logs", "source-opened", "out.mkv", "new.mkv"] {
            assert!(!temp.path().join(path).exists(), "no {path}");
        }
        assert!(!configuration.temp.exists());
    }

    #[test]
    fn temp_rejects_unsupported_runs_and_crud_before_logging_or_save() {
        let temp = tempfile::tempdir().expect("fixture");
        let configuration = root_edit_fixture(temp.path());
        let file = temp.path().join("condor.json");
        configuration.save(&file).expect("save fixture");
        let before = std::fs::read(&file).expect("snapshot");
        let mut routes = vec![
            vec!["zone"],
            vec!["scenes"],
            vec!["clean"],
            vec!["detect-noise"],
            vec!["scale-noise"],
            vec!["scale-speed"],
            vec!["optimize-bitrate"],
        ];
        for route in [
            "init",
            "encode",
            "detect-scenes",
            "target-quality",
            "concatenate",
            "quality-check",
            "benchmark",
            "detect-noise",
            "scale-noise",
            "scale-speed",
            "optimize-bitrate",
        ] {
            routes.extend([vec![route, "info"], vec![route, "reset"]]);
        }
        routes.extend([
            vec!["init", "set", "workers", "2"],
            vec!["target-quality", "set", "metric", "vmaf"],
        ]);
        let mut failures = vec![];
        for route in routes {
            for before_command in [true, false] {
                let flags = vec!["--temp", "unused-work"];
                let args = if before_command {
                    [flags, route.clone()].concat()
                } else {
                    [route.clone(), flags].concat()
                };
                let output = condor_cmd(&temp).env("PATH", "").args(&args).output().expect("CLI");
                let message = String::from_utf8_lossy(&output.stderr);
                if output.status.code() != Some(2)
                    || !message.contains("--temp")
                    || !message.contains("remove --temp")
                    || !message.contains("Usage:")
                {
                    failures.push(format!("{args:?}: {message}"));
                }
                assert_eq!(std::fs::read(&file).expect("unchanged"), before, "{args:?}");
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
        for path in ["logs", "unused-work", "source-opened", "out.mkv"] {
            assert!(!temp.path().join(path).exists(), "no {path}");
        }
    }

    #[test]
    fn offline_target_quality_crud_accepts_global_config_log_and_verbose_flags_on_either_side() {
        let temp = tempfile::tempdir().expect("fixture");
        let configuration = root_edit_fixture(temp.path());
        let file = temp.path().join("custom.json");
        for route in ["init", "target-quality"] {
            for before_command in [false, true] {
                configuration.save(&file).expect("fresh absent TQ");
                let globals =
                    vec!["--config-file", "custom.json", "--logs", "custom.log", "--verbose"];
                let action = vec![
                    route,
                    "set",
                    if route == "init" {
                        "target-metric"
                    } else {
                        "metric"
                    },
                    "vmaf",
                ];
                let args = if before_command {
                    [globals.clone(), action].concat()
                } else {
                    [action, globals.clone()].concat()
                };
                condor_cmd(&temp).env("PATH", "").args(args).assert().success();
                let quality = load(&file)
                    .condor
                    .sequence_config
                    .target_quality
                    .expect("CRUD materializes TQ");
                assert!(matches!(
                    quality.metric,
                    andean_condor::models::sequence::target_quality::types::QualityMetric::VMAF { .. }
                ));
                condor_cmd(&temp)
                    .env("PATH", "")
                    .args([vec![route, "set", "target", "90"], globals].concat())
                    .assert()
                    .success();
                assert_eq!(
                    load(&file)
                        .condor
                        .sequence_config
                        .target_quality
                        .expect("TQ")
                        .metric
                        .target_range(),
                    (89.0, 91.0)
                );
                assert_eq!(
                    serde_json::to_value(load(&file).condor.scenes).expect("history"),
                    serde_json::to_value(&configuration.condor.scenes).expect("snapshot")
                );
            }
        }
        assert!(temp.path().join("custom.log").exists());
        assert!(!temp.path().join("source-opened").exists());
        assert!(!temp.path().join("condor.json").exists());
    }
}

#[cfg(test)]
mod output {
    use super::*;

    #[test]
    fn output_crud_rejects_alias_without_mutation() {
        use california_condor::commands::{
            config::ConfigCommand,
            handlers::{
                concatenate::{ConcatenateCrud, ConcatenateSet},
                init::{InitCrud, InitSet},
            },
        };
        for concat in [false, true] {
            let temp = tempfile::tempdir().expect("fixture");
            let mut config = root_edit_fixture(temp.path());
            let before = serde_json::to_value(&config).expect("snapshot");
            let file = temp.path().join("condor.json");
            config.save(&file).expect("save");
            let disk = std::fs::read(&file).expect("disk snapshot");
            let source = config.input.clone();
            let result = if concat {
                ConcatenateCrud::apply(
                    &mut config,
                    &ConcatenateSet::Output {
                        path: source.clone(),
                    },
                )
            } else {
                InitCrud::apply(
                    &mut config,
                    &InitSet::Output {
                        path: source.clone(),
                    },
                )
            };
            assert!(result.is_err(), "output CRUD must refuse source aliases");
            assert_eq!(serde_json::to_value(config).expect("after"), before);
            assert_eq!(std::fs::read(&file).expect("disk"), disk);
            assert!(!temp.path().join("source-opened").exists());
        }
    }

    fn check_declared_output_aliases(validate_aliases: bool) {
        use andean_condor::models::{
            input::VapourSynthScriptSource,
            sequence::{
                noise_detector::NoiseDetectorConfig,
                quality_check::QualityCheckConfig,
                scene_concatenator::{ConcatMethod, FfmpegConfig, MkvmergeConfig},
                target_quality::TargetQualityConfig,
            },
        };
        use california_condor::commands::{
            config::ConfigCommand,
            handlers::init::{InitCrud, InitSet},
        };
        if validate_aliases {
            for slot in [
                "root-video",
                "root-vs",
                "root-script",
                "scd",
                "parallel",
                "noise",
                "tq",
                "metric",
                "qc",
                "ff-extra",
                "ff-attachment",
                "ff-extra-attachment",
                "mkv-extra",
                "mkv-attachment",
                "mkv-extra-attachment",
                "chunk",
            ] {
                let temp = tempfile::tempdir().expect("fixture");
                let mut config = root_edit_fixture(temp.path());
                let protected = temp.path().join("protected");
                std::fs::write(&protected, b"protected source").expect("source");
                let input = Input::VapourSynthScript {
                    source: VapourSynthScriptSource::Path(protected.clone()),
                    variables: Default::default(),
                    index: 0,
                    filters: vec![],
                    stream_concurrently: false,
                };
                let seq = &mut config.condor.sequence_config;
                match slot {
                    "root-video" => {
                        config.condor.input = Input::Video {
                            path: protected.clone(),
                            import_method: andean_condor::models::input::ImportMethod::FFMS2 {
                                index: None,
                                cache_path: None,
                            },
                            filters: vec![],
                        }
                    },
                    "root-vs" => {
                        config.condor.input = Input::VapourSynth {
                            path: protected.clone(),
                            import_method:
                                andean_condor::models::input::VapourSynthImportMethod::BestSource {
                                    index: None,
                                },
                            cache_path: None,
                            filters: vec![],
                        }
                    },
                    "root-script" => config.condor.input = input,
                    "scd" => seq.scene_detector.input = Some(input),
                    "parallel" => seq.parallel_encoder.input = Some(input),
                    "noise" => {
                        seq.noise_detector = Some(NoiseDetectorConfig {
                            input: Some(input),
                            ..Default::default()
                        });
                    },
                    "tq" => {
                        seq.target_quality = Some(TargetQualityConfig {
                            input: Some(input),
                            ..Default::default()
                        });
                    },
                    "metric" => {
                        seq.target_quality = Some(TargetQualityConfig {
                            metric_input: Some(input),
                            ..Default::default()
                        });
                    },
                    "qc" => {
                        seq.quality_check = Some(QualityCheckConfig {
                            input: Some(input),
                            ..Default::default()
                        });
                    },
                    "chunk" => {
                        let chunk = seq.scene_concatenator.scenes_directory.join(format!(
                        "{}.{}",
                        andean_condor::core::sequence::parallel_encoder::ParallelEncoder::scene_id(
                            0
                        ),
                        config.condor.scenes[0].encoder.output_extension()
                    ));
                        config.condor.output.path = chunk;
                    },
                    _ => {
                        let extra = slot.ends_with("extra");
                        let extra_attachment = slot.ends_with("extra-attachment");
                        let ffmpeg = slot.starts_with("ff");
                        let key = if ffmpeg { "tracks" } else { "attachments" };
                        let tracks = if ffmpeg {
                            serde_json::json!({"7":{"type":"attachment","path":protected,"copy":false}})
                        } else {
                            serde_json::json!({"7":{"path":protected,"copy":false}})
                        };
                        let settings = if extra {
                            serde_json::json!({"extra_inputs":[{"path":protected,"tracks":{}}]})
                        } else if extra_attachment {
                            serde_json::json!({"extra_inputs":[{"path":temp.path().join("other"),(key):tracks}]})
                        } else {
                            serde_json::json!({(key):tracks})
                        };
                        seq.scene_concatenator.method = if slot.starts_with("ff") {
                            ConcatMethod::FFmpeg(
                                serde_json::from_value::<FfmpegConfig>(settings)
                                    .expect("FFmpeg model"),
                            )
                        } else {
                            ConcatMethod::MKVMerge(
                                serde_json::from_value::<MkvmergeConfig>(settings)
                                    .expect("MKV model"),
                            )
                        };
                    },
                }
                if slot != "chunk" {
                    config.condor.output.path = protected.clone();
                }
                config.condor.output.overwrite = true;
                let before = serde_json::to_value(&config).expect("before");
                assert!(config.validate_output(false).is_err(), "declared {slot}");
                assert_eq!(serde_json::to_value(&config).expect("after"), before);
                assert_eq!(
                    std::fs::read(&protected).expect("source"),
                    b"protected source"
                );
                assert!(!temp.path().join("source-opened").exists());
            }
            return;
        }
        let temp = tempfile::tempdir().expect("fixture");
        let mut config = root_edit_fixture(temp.path());
        let missing = temp.path().join("not-created/out.mkv");
        InitCrud::apply(
            &mut config,
            &InitSet::Output {
                path: missing.clone(),
            },
        )
        .expect("offline edit permits missing parent");
        assert!(!missing.parent().expect("parent").exists());
        assert!(!temp.path().join("source-opened").exists());
    }

    #[test]
    fn output_validation_rejects_every_declared_source_or_chunk_alias_even_with_overwrite() {
        check_declared_output_aliases(true);
    }

    #[test]
    fn offline_output_edit_allows_missing_parent_without_creating_it() {
        check_declared_output_aliases(false);
    }
}

#[cfg(test)]
mod temp {
    use super::*;

    #[test]
    fn empty_temp_edit_rebases_only_derived_directories() {
        use california_condor::commands::handlers::configure_temp;
        for kind in ["missing", "empty", "unset", "encoder-custom", "concat-custom", "both-custom"]
        {
            let temp = tempfile::tempdir().expect("fixture");
            let base = std::fs::canonicalize(temp.path()).expect("canonical fixture");
            let mut configuration = root_edit_fixture(&base);
            configuration.condor.scenes.clear();
            configuration.temp = base.join("old");
            let old_scenes = base.join("old/scenes");
            let custom_encoder = base.join("custom-encoder");
            let custom_concat = base.join("custom-concat");
            let sequence = &mut configuration.condor.sequence_config;
            sequence.parallel_encoder.scenes_directory = match kind {
                "unset" => Default::default(),
                "encoder-custom" | "both-custom" => custom_encoder.clone(),
                _ => old_scenes.clone(),
            };
            sequence.scene_concatenator.scenes_directory = match kind {
                "unset" | "encoder-custom" => Default::default(),
                "concat-custom" | "both-custom" => custom_concat.clone(),
                _ => old_scenes.clone(),
            };
            if kind == "empty" {
                std::fs::create_dir_all(&old_scenes).expect("empty old directory");
            }
            let selected = base.join("new");
            // Unadopted default work belongs to somebody else, not to either custom directory.
            if kind == "both-custom" {
                std::fs::create_dir_all(selected.join("scenes")).expect("foreign work");
                std::fs::write(selected.join("scenes/foreign"), b"unowned").expect("sentinel");
            }
            let file = base.join("condor.json");
            configuration.save(&file).expect("save");
            let bytes = std::fs::read(&file).expect("snapshot");
            let mut expected = configuration.clone();
            expected.temp = selected.clone();
            expected.condor.sequence_config.parallel_encoder.scenes_directory =
                if matches!(kind, "encoder-custom" | "both-custom") {
                    custom_encoder
                } else {
                    selected.join("scenes")
                };
            expected.condor.sequence_config.scene_concatenator.scenes_directory =
                if matches!(kind, "concat-custom" | "both-custom") {
                    custom_concat
                } else {
                    selected.join("scenes")
                };
            configure_temp(&mut configuration, Some(&base.join("absent/../new")))
                .expect("empty offline temp edit");
            assert_eq!(
                serde_json::to_value(&configuration).expect("actual"),
                serde_json::to_value(expected).expect("expected"),
                "{kind}"
            );
            assert_eq!(std::fs::read(&file).expect("no implicit save"), bytes);
            assert!(!base.join("source-opened").exists());
            if kind == "empty" {
                assert!(
                    old_scenes.is_dir(),
                    "do not remove even an empty old directory"
                );
            }
            if kind == "both-custom" {
                assert_eq!(
                    std::fs::read(selected.join("scenes/foreign")).expect("foreign"),
                    b"unowned"
                );
            } else {
                assert!(
                    !selected.exists(),
                    "pure config edits do not create directories"
                );
            }
        }
    }

    #[test]
    fn temp_relocation_refuses_saved_scenes_or_orphan_or_foreign_work_without_mutation() {
        use california_condor::commands::handlers::configure_temp;
        for kind in ["saved-scenes", "encoder-orphan", "concat-orphan", "foreign-destination"] {
            let temp = tempfile::tempdir().expect("fixture");
            let base = std::fs::canonicalize(temp.path()).expect("canonical fixture");
            let mut configuration = root_edit_fixture(&base);
            let old = configuration.temp.clone();
            let selected = base.join("new-work");
            if kind != "saved-scenes" {
                configuration.condor.scenes.clear();
            }
            let chunk = match kind {
                "encoder-orphan" => Some(old.join("scenes/orphan")),
                "concat-orphan" => {
                    configuration.condor.sequence_config.scene_concatenator.scenes_directory =
                        base.join("concat");
                    Some(base.join("concat/orphan"))
                },
                "foreign-destination" => Some(selected.join("scenes/foreign")),
                _ => None,
            };
            if let Some(chunk) = &chunk {
                std::fs::create_dir_all(chunk.parent().expect("parent")).expect("work");
                std::fs::write(chunk, b"keep chunk").expect("chunk");
            }
            let file = base.join("condor.json");
            configuration.save(&file).expect("save");
            let bytes = std::fs::read(&file).expect("bytes");
            let before = serde_json::to_value(&configuration).expect("snapshot");
            let error = configure_temp(&mut configuration, Some(&selected))
                .expect_err("must refuse relocation/adoption");
            let message = format!("{error:#}");
            assert!(
                message.contains(path_str(&old))
                    && message.contains(path_str(&selected))
                    && message.contains("--config-file"),
                "{kind}: {message}"
            );
            assert_eq!(serde_json::to_value(&configuration).expect("after"), before);
            assert_eq!(std::fs::read(file).expect("bytes"), bytes);
            if let Some(chunk) = chunk {
                assert_eq!(std::fs::read(chunk).expect("retained chunk"), b"keep chunk");
            }
            assert!(!base.join("source-opened").exists());
        }
    }

    #[test]
    fn cli_temp_relocation_with_saved_or_foreign_work_refuses_run_routes_before_save_or_source_open()
     {
        for kind in ["saved-scenes", "encoder-orphan", "concat-orphan", "foreign-destination"] {
            let temp = tempfile::tempdir().expect("fixture");
            let base = std::fs::canonicalize(temp.path()).expect("canonical fixture");
            let mut configuration = root_edit_fixture(&base);
            // Even the broken baseline cannot reach an actual benchmark or codec.
            std::fs::write(base.join("a.vpy"), format!("from pathlib import Path\nPath({}).write_text('opened')\nraise RuntimeError('temp root source must not open')\n", serde_json::to_string(&base.join("source-opened")).expect("literal"))).expect("guard script");
            if kind != "saved-scenes" {
                configuration.condor.scenes.clear();
            }
            let selected = base.join("new-work");
            let chunk = match kind {
                "encoder-orphan" => Some(configuration.temp.join("scenes/orphan")),
                "concat-orphan" => {
                    configuration.condor.sequence_config.scene_concatenator.scenes_directory =
                        base.join("concat");
                    Some(base.join("concat/orphan"))
                },
                "foreign-destination" => Some(selected.join("scenes/foreign")),
                _ => None,
            };
            if let Some(chunk) = &chunk {
                std::fs::create_dir_all(chunk.parent().expect("parent")).expect("work");
                std::fs::write(chunk, b"CLI sentinel chunk").expect("chunk");
            }
            for name in ["a.ffindex", "b.ffindex"] {
                std::fs::write(base.join(name), b"index sentinel").expect("index");
            }
            let file = base.join("condor.json");
            configuration.save(&file).expect("save");
            let bytes = std::fs::read(&file).expect("bytes");
            let mut failures = vec![];
            for command in [
                "",
                "encode",
                "detect-scenes",
                "target-quality",
                "concatenate",
                "quality-check",
                "benchmark",
            ] {
                let mut cli = condor_cmd(&temp);
                cli.env("PATH", "").arg("--config-file").arg(&file).arg("--temp").arg(&selected);
                if !command.is_empty() {
                    cli.arg(command);
                }
                let output = cli.output().expect("CLI");
                let message = String::from_utf8_lossy(&output.stderr);
                if output.status.success()
                    || !message.contains(path_str(&configuration.temp))
                    || !message.contains(path_str(&selected))
                    || !message.contains("--config-file")
                    || message.contains("unexpected argument")
                {
                    failures.push(format!(
                        "{kind}/{command}: missing temp policy refusal: {message}"
                    ));
                }
                if std::fs::read(&file).expect("after") != bytes {
                    failures.push(format!("{kind}/{command}: saved config changed"));
                }
                std::fs::write(&file, &bytes).expect("restore baseline fixture");
            }
            assert!(failures.is_empty(), "{}", failures.join("\n"));
            assert!(!base.join("source-opened").exists());
            if let Some(chunk) = chunk {
                assert_eq!(std::fs::read(chunk).expect("chunk"), b"CLI sentinel chunk");
            }
            for name in ["a.ffindex", "b.ffindex"] {
                assert_eq!(
                    std::fs::read(base.join(name)).expect("index"),
                    b"index sentinel"
                );
            }
            if kind != "foreign-destination" {
                assert!(!selected.exists());
            }
        }
    }

    fn check_equivalent_temp_roots(phase: u8) {
        assert!((1..=3).contains(&phase));
        use california_condor::commands::handlers::configure_temp;
        let temp = tempfile::tempdir().expect("fixture");
        let base = std::fs::canonicalize(temp.path()).expect("canonical fixture");
        let mut configuration = root_edit_fixture(&base);
        configuration.temp = base.join("./old");
        configuration.condor.sequence_config.parallel_encoder.scenes_directory =
            base.join("old/scenes");
        configuration.condor.sequence_config.scene_concatenator.scenes_directory =
            base.join("custom-concat");
        std::fs::create_dir_all(base.join("old/scenes")).expect("old work");
        std::fs::create_dir_all(base.join("sub")).expect("relative parent");
        std::fs::write(base.join("old/scenes/0.ivf"), b"populated alias chunk").expect("chunk");
        let file = base.join("condor.json");
        configuration.save(&file).expect("save");
        let expected = serde_json::to_value(&configuration).expect("snapshot");
        let bytes = std::fs::read(&file).expect("bytes");
        let mut aliases = vec!["old", "./old", "sub/../old"];
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(base.join("old"), base.join("alias")).expect("alias");
            aliases.push("alias");
            std::fs::create_dir_all(base.join("old/child")).expect("alias child");
            std::os::unix::fs::symlink(base.join("old/child"), base.join("alias-child"))
                .expect("child alias");
            aliases.push("alias-child/..");
        }
        configure_temp(&mut configuration, None).expect("no explicit edit");
        assert_eq!(
            serde_json::to_value(&configuration).expect("None"),
            expected
        );
        if phase == 1 {
            return;
        }
        for alias in aliases {
            std::fs::write(&file, &bytes).expect("restore bytes after the preceding CLI save");
            configure_temp(&mut configuration, Some(&base.join(alias))).expect("direct alias");
            assert_eq!(
                serde_json::to_value(&configuration).expect("direct"),
                expected
            );
            assert_eq!(std::fs::read(&file).expect("no direct save"), bytes);
            let output = condor_cmd(&temp)
                .env("PATH", "")
                .args(["encode", "--temp", alias])
                .output()
                .expect("CLI");
            let message = String::from_utf8_lossy(&output.stderr);
            assert!(
                !message.contains("cannot change temp") && !message.contains("unexpected argument"),
                "{message}"
            );
            assert_eq!(serde_json::to_value(load(&file)).expect("CLI"), expected);
        }
        assert_eq!(
            std::fs::read(base.join("old/scenes/0.ivf")).expect("chunk"),
            b"populated alias chunk"
        );
        if phase == 2 {
            return;
        }
        configuration.temp = base.join("missing");
        let before = serde_json::to_value(&configuration).expect("offline snapshot");
        configure_temp(&mut configuration, Some(&base.join("absent/../missing")))
            .expect("missing lexical equivalent");
        assert_eq!(
            serde_json::to_value(&configuration).expect("offline"),
            before
        );
    }

    #[test]
    fn omitted_temp_edit_preserves_populated_job() {
        check_equivalent_temp_roots(1);
    }

    #[test]
    fn equivalent_temp_path_aliases_preserve_populated_job_and_chunk() {
        check_equivalent_temp_roots(2);
    }

    #[test]
    fn missing_lexically_equivalent_temp_path_preserves_offline_configuration() {
        check_equivalent_temp_roots(3);
    }

    #[test]
    fn temp_relocation_non_directory_work_path_refuses_without_config_or_sentinel_mutation() {
        use california_condor::commands::handlers::configure_temp;
        for kind in ["old-not-directory", "destination-not-directory"] {
            let temp = tempfile::tempdir().expect("fixture");
            let base = std::fs::canonicalize(temp.path()).expect("canonical fixture");
            let mut configuration = root_edit_fixture(&base);
            configuration.condor.scenes.clear();
            let selected = base.join("new");
            let bad = if kind == "old-not-directory" {
                configuration.condor.sequence_config.parallel_encoder.scenes_directory =
                    base.join(kind);
                base.join(kind)
            } else {
                std::fs::create_dir_all(&selected).expect("new root");
                selected.join("scenes")
            };
            std::fs::write(&bad, b"not a directory").expect("sentinel");
            let file = base.join("condor.json");
            configuration.save(&file).expect("save");
            let bytes = std::fs::read(&file).expect("bytes");
            let before = serde_json::to_value(&configuration).expect("snapshot");
            let error =
                configure_temp(&mut configuration, Some(&selected)).expect_err("not a directory");
            assert!(format!("{error:#}").contains(path_str(&bad)), "{error:#}");
            assert_eq!(serde_json::to_value(&configuration).expect("after"), before);
            assert_eq!(std::fs::read(file).expect("bytes"), bytes);
            assert_eq!(std::fs::read(bad).expect("sentinel"), b"not a directory");
        }
    }

    #[cfg(unix)]
    fn check_temp_filesystem_access(kinds: &[&str]) {
        use california_condor::commands::handlers::configure_temp;
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().expect("fixture");
        let base = std::fs::canonicalize(temp.path()).expect("canonical fixture");
        std::os::unix::fs::symlink("loop", base.join("loop")).expect("symlink loop");
        let protected = base.join("protected");
        std::fs::create_dir_all(protected.join("child")).expect("directory");
        for &kind in kinds {
            let mut configuration = root_edit_fixture(&base);
            configuration.condor.scenes.clear();
            let selected = match kind {
                "loop" => base.join("loop"),
                "normalize" => protected.join("child"),
                "destination-read" => {
                    std::fs::create_dir_all(base.join("destination")).expect("destination");
                    std::os::unix::fs::symlink(&protected, base.join("destination/scenes"))
                        .expect("scenes alias");
                    base.join("destination")
                },
                _ => base.join("new"),
            };
            if kind == "old-read" {
                configuration.condor.sequence_config.parallel_encoder.scenes_directory =
                    protected.clone();
            }
            let file = base.join("condor.json");
            configuration.save(&file).expect("save");
            let bytes = std::fs::read(&file).expect("bytes");
            let mut expected = configuration.clone();
            if kind != "loop" {
                std::fs::set_permissions(&protected, std::fs::Permissions::from_mode(0o000))
                    .expect("deny access");
            }
            // chmod may be bypassed (for example by container root); probe the actual operation.
            let access = match kind {
                "loop" => None,
                "normalize" => Some(std::fs::canonicalize(&selected).map(|_| ())),
                _ => Some(
                    std::fs::read_dir(if kind == "old-read" {
                        protected.clone()
                    } else {
                        selected.join("scenes")
                    })
                    .and_then(|mut entries| entries.next().transpose().map(|_| ())),
                ),
            };
            let result = configure_temp(&mut configuration, Some(&selected));
            std::fs::set_permissions(&protected, std::fs::Permissions::from_mode(0o700))
                .expect("restore before assertions and cleanup");
            let denied = match access {
                None => true,
                Some(Err(error)) => {
                    assert_eq!(
                        error.kind(),
                        std::io::ErrorKind::PermissionDenied,
                        "{kind}: unexpected probe error: {error}"
                    );
                    eprintln!(
                        "temp root {kind}: permission denial enforced by the actual operation"
                    );
                    true
                },
                Some(Ok(())) => {
                    eprintln!(
                        "temp root {kind}: chmod 000 did not deny the actual operation; permission-denial proof unavailable; checking allowed/nonempty-work policy behavior"
                    );
                    false
                },
            };
            if denied {
                let error =
                    result.expect_err("filesystem errors cannot masquerade as missing/empty");
                let message = format!("{error:#}");
                let context = if matches!(kind, "loop" | "normalize") {
                    "normalizing temp directory"
                } else {
                    "inspecting work directory"
                };
                assert!(message.contains(context), "{kind}: {message}");
                if kind != "loop" {
                    assert!(
                        error.chain().any(|cause| cause
                            .downcast_ref::<std::io::Error>()
                            .is_some_and(
                                |error| error.kind() == std::io::ErrorKind::PermissionDenied
                            )),
                        "{kind}: {message}"
                    );
                }
            } else if kind == "normalize" {
                result.expect("accessible empty job permits a temp edit");
                expected.temp = selected.clone();
                expected.condor.sequence_config.parallel_encoder.scenes_directory =
                    selected.join("scenes");
                expected.condor.sequence_config.scene_concatenator.scenes_directory =
                    selected.join("scenes");
            } else {
                let error = result.expect_err("readable nonempty work must still refuse");
                let message = format!("{error:#}");
                let work = if kind == "old-read" {
                    protected.clone()
                } else {
                    selected.join("scenes")
                };
                assert!(
                    message.contains("cannot change temp directory")
                        && message.contains("--config-file")
                        && message.contains(path_str(&expected.temp))
                        && message.contains(path_str(&selected))
                        && message.contains(path_str(&work)),
                    "{kind}: {message}"
                );
            }
            assert_eq!(
                serde_json::to_value(&configuration).expect("after"),
                serde_json::to_value(expected).expect("expected")
            );
            assert_eq!(std::fs::read(&file).expect("bytes"), bytes);
        }
        if !kinds.is_empty() {
            return;
        }
        // None is an unconditional no-op, even when the saved root cannot be normalized.
        let mut configuration = root_edit_fixture(&base);
        configuration.temp = base.join("loop");
        let before = serde_json::to_value(&configuration).expect("before");
        configure_temp(&mut configuration, None).expect("no requested edit");
        assert_eq!(serde_json::to_value(configuration).expect("after"), before);
    }

    #[cfg(unix)]
    #[test]
    fn temp_relocation_symlink_loop_reports_normalization_context_without_mutation() {
        check_temp_filesystem_access(&["loop"]);
    }

    #[cfg(unix)]
    #[test]
    fn temp_relocation_permission_errors_are_contextual_or_accessible_paths_follow_work_policy() {
        check_temp_filesystem_access(&["old-read", "destination-read", "normalize"]);
    }

    #[cfg(unix)]
    #[test]
    fn omitted_temp_edit_ignores_unnormalizable_saved_root() {
        check_temp_filesystem_access(&[]);
    }

    fn check_empty_handler_temp_rebase(benchmark: bool) {
        use california_condor::commands::handlers::benchmarker::benchmarker_handler;
        for kind in ["derived", "encoder-custom", "concat-custom", "both-custom"] {
            let temp = tempfile::tempdir().expect("fixture");
            let base = std::fs::canonicalize(temp.path()).expect("canonical fixture");
            let mut configuration = root_edit_fixture(&base);
            configuration.condor.scenes.clear();
            // These handlers save before the later execution prerequisite fails.
            std::fs::write(
                base.join("a.vpy"),
                "raise RuntimeError('temp root execution prerequisite')\n",
            )
            .expect("unavailable execution source");
            let sequence = &mut configuration.condor.sequence_config;
            sequence.quality_check = Some(Default::default());
            if matches!(kind, "encoder-custom" | "both-custom") {
                sequence.parallel_encoder.scenes_directory = base.join("custom-encoder");
            }
            if matches!(kind, "concat-custom" | "both-custom") {
                sequence.scene_concatenator.scenes_directory = base.join("custom-concat");
            } else if kind == "encoder-custom" {
                sequence.scene_concatenator.scenes_directory = Default::default();
            }
            let selected = base.join("new");
            let mut expected = configuration.clone();
            expected.temp = selected.clone();
            if !matches!(kind, "encoder-custom" | "both-custom") {
                expected.condor.sequence_config.parallel_encoder.scenes_directory =
                    selected.join("scenes");
            }
            if !matches!(kind, "concat-custom" | "both-custom") {
                expected.condor.sequence_config.scene_concatenator.scenes_directory =
                    selected.join("scenes");
            }
            let file = base.join("condor.json");
            if !benchmark {
                for command in ["encode", "target-quality", "concatenate", "quality-check"] {
                    configuration.save(&file).expect("restore empty job");
                    let output = condor_cmd(&temp)
                        .env("PATH", "")
                        .args([command, "--temp", "new"])
                        .output()
                        .expect("CLI");
                    let message = String::from_utf8_lossy(&output.stderr);
                    assert!(
                        !output.status.success()
                            && message.contains("temp root execution prerequisite"),
                        "{command}: {message}"
                    );
                    assert_eq!(
                        serde_json::to_value(load(&file)).expect("saved"),
                        serde_json::to_value(&expected).expect("expected"),
                        "{kind}/{command}"
                    );
                    for (custom, directory, editor) in [
                        (
                            matches!(kind, "encoder-custom" | "both-custom"),
                            base.join("custom-encoder"),
                            "encode",
                        ),
                        (
                            matches!(kind, "concat-custom" | "both-custom"),
                            base.join("custom-concat"),
                            "concatenate",
                        ),
                    ] {
                        if custom {
                            assert!(
                                message.contains(path_str(&directory))
                                    && message.contains("--temp does not move those files")
                                    && message.contains(&format!("{editor} set scenes-directory")),
                                "{message}"
                            );
                        }
                    }
                }
            }
            configuration.save(&file).expect("restore empty job");
            let (after, _) = benchmarker_handler(Some(&file), Some(&selected), None, None)
                .expect("handler only; never execute a benchmark");
            assert_eq!(
                serde_json::to_value(after).expect("benchmark handler"),
                serde_json::to_value(expected).expect("expected")
            );
            assert!(!selected.exists());
        }
    }

    #[test]
    fn run_handlers_save_temp_rebase_and_warn_custom_directories_before_source_failure() {
        check_empty_handler_temp_rebase(false);
    }

    #[test]
    fn benchmark_handler_rebases_empty_job_without_running_benchmark() {
        check_empty_handler_temp_rebase(true);
    }
}

#[cfg(test)]
mod input {
    use super::*;

    #[test]
    fn init_input_crud_replacing_populated_root_preserves_config_chunks_and_both_index_files() {
        use andean_condor::models::input::ImportMethod;
        use california_condor::commands::{
            config::ConfigCommand,
            handlers::init::{InitCrud, InitSet},
        };
        let temp = tempfile::tempdir().expect("fixture");
        let mut configuration = root_edit_fixture(temp.path());
        configuration.input = temp.path().join("a.mkv");
        configuration.condor.input = Input::Video {
            path: configuration.input.clone(),
            import_method: ImportMethod::FFMS2 {
                index: Some(1),
                cache_path: Some(temp.path().join("a.ffindex")),
            },
            filters: vec![],
        };
        let file = temp.path().join("condor.json");
        configuration.save(&file).expect("save");
        let before = std::fs::read(&file).expect("snapshot");
        let memory = serde_json::to_value(&configuration).expect("snapshot");
        let chunk = configuration.temp.join("scenes/0.ivf");
        std::fs::create_dir_all(chunk.parent().expect("parent")).expect("workdir");
        std::fs::write(&chunk, b"old chunk").expect("chunk");
        let old_cache = temp.path().join("a.ffindex");
        let new_index = temp.path().join("b.ffindex");
        std::fs::write(&old_cache, b"old cache").expect("cache");
        std::fs::write(&new_index, b"new sentinel").expect("index");
        let replacement = temp.path().join("b.mkv");
        let error = InitCrud::apply(
            &mut configuration,
            &InitSet::Input {
                args: root_edit_args(replacement),
            },
        )
        .expect_err("populated root must refuse");
        let message = format!("{error:#}");
        assert!(
            message.contains("a.mkv")
                && message.contains("b.mkv")
                && message.contains("--config-file"),
            "{message}"
        );
        assert_eq!(serde_json::to_value(&configuration).expect("after"), memory);
        assert_eq!(std::fs::read(&file).expect("bytes"), before);
        assert_eq!(std::fs::read(chunk).expect("chunk"), b"old chunk");
        assert_eq!(std::fs::read(old_cache).expect("cache"), b"old cache");
        assert_eq!(std::fs::read(new_index).expect("index"), b"new sentinel");
        assert!(!temp.path().join("source-opened").exists());
    }

    #[test]
    fn populated_root_cli_refuses_before_save_or_native_work() {
        let temp = tempfile::tempdir().expect("fixture");
        let configuration = root_edit_fixture(temp.path());
        let file = temp.path().join("condor.json");
        configuration.save(&file).expect("save");
        let before = std::fs::read(&file).expect("snapshot");
        let replacement = temp.path().join("b.vpy");
        let chunk = configuration.temp.join("scenes/0.ivf");
        std::fs::create_dir_all(chunk.parent().expect("parent")).expect("workdir");
        std::fs::write(&chunk, b"CLI chunk").expect("chunk");
        let mut failures = vec![];
        for args in [
            vec!["init", "set", "input", "--path", path_str(&replacement)],
            vec![
                "--input",
                path_str(&replacement),
                "--temp",
                "forbidden-work",
                "--output",
                "forbidden.mkv",
                "--discard",
                "audio",
            ],
        ] {
            let output = condor_cmd(&temp).env("PATH", "").args(args).output().expect("CLI");
            let message = String::from_utf8_lossy(&output.stderr);
            if output.status.success()
                || !message.contains("a.vpy")
                || !message.contains("b.vpy")
                || !message.contains("--config-file")
            {
                failures.push(format!("missing source refusal: {message}"));
            }
            if std::fs::read(&file).expect("unchanged bytes") != before {
                failures.push("config bytes changed".into());
            }
            std::fs::write(&file, &before).expect("restore fixture");
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
        assert!(!temp.path().join("forbidden-work").exists());
        assert!(!temp.path().join("source-opened").exists());
        assert_eq!(std::fs::read(chunk).expect("chunk"), b"CLI chunk");
    }

    #[test]
    fn offline_same_source_alias_edit_repairs_stale_duplicate_path_without_changing_input_cache_or_work()
     {
        use andean_condor::models::input::ImportMethod;
        let temp = tempfile::tempdir().expect("fixture");
        let mut configuration = root_edit_fixture(temp.path());
        let source = temp.path().join("a.vpy");
        // A real source path is authoritative even when the duplicated metadata is stale.
        configuration.input = temp.path().join("stale.vpy");
        configuration.condor.input = Input::Video {
            path: source.clone(),
            import_method: ImportMethod::FFMS2 {
                index: Some(2),
                cache_path: Some(temp.path().join("keep.ffindex")),
            },
            filters: vec![],
        };
        let file = temp.path().join("condor.json");
        let chunk = configuration.temp.join("scenes/0.ivf");
        std::fs::create_dir_all(chunk.parent().expect("parent")).expect("workdir");
        std::fs::write(&chunk, b"keep chunk").expect("chunk");
        configuration.save(&file).expect("save");
        let mut paths = vec!["a.vpy", "./a.vpy", "sub/../a.vpy"];
        std::fs::create_dir_all(temp.path().join("sub")).expect("subdir");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&source, temp.path().join("alias.vpy")).expect("symlink");
            paths.push("alias.vpy");
        }
        for path in paths {
            condor_cmd(&temp)
                .env("PATH", "")
                .args(["init", "set", "input", "--path", path])
                .assert()
                .success();
            let after = load(&file);
            assert_eq!(after.input, source);
            assert_eq!(
                serde_json::to_value(&after.condor).expect("after"),
                serde_json::to_value(&configuration.condor).expect("before")
            );
            assert_eq!(after.temp, configuration.temp);
            assert_eq!(std::fs::read(&chunk).expect("chunk"), b"keep chunk");
        }
    }

    #[test]
    fn same_source_vapoursynth_decoder_edit_keeps_cache_only_for_unchanged_decoder_kind() {
        use andean_condor::models::input::VapourSynthImportMethod;
        use california_condor::commands::{
            DecoderMethod,
            config::ConfigCommand,
            handlers::init::{InitCrud, InitSet},
        };
        let temp = tempfile::tempdir().expect("fixture");
        let source = temp.path().join("offline.mkv");
        let cache = temp.path().join("old.cache");
        let executable = temp.path().join("custom-dgindexnv");
        std::fs::write(&cache, b"old plugin cache").expect("cache");
        let methods = [
            (
                DecoderMethod::VSFFMS2,
                VapourSynthImportMethod::FFMS2 { index: Some(3) },
            ),
            (
                DecoderMethod::LSMASHWorks,
                VapourSynthImportMethod::LSMASHWorks { index: Some(3) },
            ),
            (
                DecoderMethod::BestSource,
                VapourSynthImportMethod::BestSource { index: Some(3) },
            ),
            (
                DecoderMethod::DGDecodeNV,
                VapourSynthImportMethod::DGDecNV {
                    dgindexnv_executable: Some(executable.clone()),
                },
            ),
        ];
        for (old_decoder, old_method) in &methods {
            for (new_decoder, _) in &methods {
                let mut configuration = root_edit_fixture(temp.path());
                configuration.input = source.clone();
                configuration.condor.input = Input::VapourSynth {
                    path: source.clone(),
                    import_method: old_method.clone(),
                    cache_path: Some(cache.clone()),
                    filters: vec![],
                };
                let mut expected = configuration.clone();
                let index = if *old_decoder == DecoderMethod::DGDecodeNV {
                    None
                } else {
                    Some(3)
                };
                let method = match new_decoder {
                    DecoderMethod::VSFFMS2 => VapourSynthImportMethod::FFMS2 { index },
                    DecoderMethod::LSMASHWorks => VapourSynthImportMethod::LSMASHWorks { index },
                    DecoderMethod::BestSource => VapourSynthImportMethod::BestSource { index },
                    DecoderMethod::DGDecodeNV => VapourSynthImportMethod::DGDecNV {
                        dgindexnv_executable: (*old_decoder == DecoderMethod::DGDecodeNV)
                            .then(|| executable.clone()),
                    },
                    DecoderMethod::FFMS2 => unreachable!(),
                };
                expected.condor.input = Input::VapourSynth {
                    path: source.clone(),
                    import_method: method,
                    cache_path: (old_decoder == new_decoder).then(|| cache.clone()),
                    filters: vec![],
                };
                let file = temp.path().join("condor.json");
                configuration.save(&file).expect("save");
                let bytes = std::fs::read(&file).expect("saved snapshot");
                let mut args = root_edit_args(source.clone());
                args.decoder = Some(new_decoder.clone());
                args.vs_args = Some(vec!["keep=yes".into()]);
                InitCrud::apply(&mut configuration, &InitSet::Input { args })
                    .expect("populated same-source edit");
                assert_eq!(
                    std::fs::read(&file).expect("direct edit must not save"),
                    bytes
                );
                condor_cmd(&temp)
                    .env("PATH", "")
                    .args([
                        "init",
                        "set",
                        "input",
                        "--path",
                        path_str(&source),
                        "--decoder",
                        &new_decoder.to_string(),
                        "--vs-args",
                        "keep=yes",
                    ])
                    .assert()
                    .success();
                let expected = serde_json::to_value(&expected).expect("expected");
                assert_eq!(
                    serde_json::to_value(&configuration).expect("direct"),
                    expected,
                    "{old_decoder} -> {new_decoder}"
                );
                assert_eq!(
                    serde_json::to_value(load(&file)).expect("CLI"),
                    expected,
                    "{old_decoder} -> {new_decoder}"
                );
                assert_eq!(
                    std::fs::read(&cache).expect("cache retained"),
                    b"old plugin cache"
                );
                assert!(!configuration.temp.exists());
                assert!(!temp.path().join("source-opened").exists());
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn alias_source_replacement_uses_fresh_init_lexical_hash() {
        use andean_condor::models::input::VapourSynthScriptSource;
        use california_condor::{
            commands::{
                config::ConfigCommand,
                handlers::init::{InitCrud, InitSet},
            },
            utils::hash_path::hash_path,
        };
        let temp = tempfile::tempdir().expect("fixture");
        let real = temp.path().join("real");
        std::fs::create_dir_all(&real).expect("real directory");
        std::os::unix::fs::symlink(&real, temp.path().join("alias")).expect("alias directory");
        let old_lexical = temp.path().join("alias/a.vpy");
        let old_canonical = std::fs::canonicalize(&real).expect("real path").join("a.vpy");
        let new_lexical = temp.path().join("alias/b.vpy");
        let expected_temp = temp.path().join(hash_path(&new_lexical));
        let mut failures = vec![];
        for (label, old_hash_path) in [("lexical", &old_lexical), ("canonical", &old_canonical)] {
            let mut configuration = root_edit_fixture(&real);
            configuration.condor.scenes.clear();
            configuration.input = "alias/a.vpy".into();
            if let Input::VapourSynthScript { source, .. } = &mut configuration.condor.input {
                *source = VapourSynthScriptSource::Path("alias/a.vpy".into());
            }
            configuration.temp = temp.path().join(hash_path(old_hash_path));
            configuration.condor.sequence_config.parallel_encoder.scenes_directory =
                configuration.temp.join("scenes");
            configuration.condor.sequence_config.scene_concatenator.scenes_directory =
                configuration.temp.join("scenes");
            let old_scenes = configuration.temp.join("scenes");
            std::fs::create_dir_all(&old_scenes).expect("empty old scenes");
            let mut expected = configuration.clone();
            expected.input = new_lexical.clone();
            expected.temp = expected_temp.clone();
            if let Input::VapourSynthScript { source, .. } = &mut expected.condor.input {
                *source = VapourSynthScriptSource::Path(new_lexical.clone());
            }
            expected.condor.sequence_config.parallel_encoder.scenes_directory =
                expected_temp.join("scenes");
            expected.condor.sequence_config.scene_concatenator.scenes_directory =
                expected_temp.join("scenes");
            let file = temp.path().join("condor.json");
            configuration.save(&file).expect("save");
            let bytes = std::fs::read(&file).expect("saved snapshot");
            let mut direct = configuration.clone();
            // Direct calls use absolute aliases; only the CLI child resolves relative paths.
            if let Input::VapourSynthScript { source, .. } = &mut direct.condor.input {
                *source = VapourSynthScriptSource::Path(old_lexical.clone());
            }
            InitCrud::apply(
                &mut direct,
                &InitSet::Input {
                    args: root_edit_args(new_lexical.clone()),
                },
            )
            .expect("direct alias replacement");
            assert_eq!(
                std::fs::read(&file).expect("direct edit must not save"),
                bytes
            );
            condor_cmd(&temp)
                .env("PATH", "")
                .args(["init", "set", "input", "--path", "alias/b.vpy"])
                .assert()
                .success();
            for (entry, after) in [("direct", direct), ("CLI", load(&file))] {
                if serde_json::to_value(&after).expect("actual")
                    != serde_json::to_value(&expected).expect("expected")
                {
                    failures.push(format!(
                        "{label}/{entry}: input={}, temp={} (wanted input={}, temp={})",
                        after.input.display(),
                        after.temp.display(),
                        new_lexical.display(),
                        expected_temp.display()
                    ));
                }
            }
            assert!(
                old_scenes.is_dir(),
                "do not delete the old empty work directory"
            );
            assert!(
                !expected_temp.exists(),
                "do not open/index/create work just to edit"
            );
            assert!(!real.join("source-opened").exists());
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn empty_offline_replacement_resets_root_cache_and_rebases_only_derived_dirs() {
        use andean_condor::models::input::{
            Ffms2Filter, ImportMethod, VapourSynthImportMethod, VapourSynthScriptSource,
        };
        use california_condor::commands::{
            config::ConfigCommand,
            handlers::init::{InitCrud, InitSet},
        };
        let temp = tempfile::tempdir().expect("fixture");
        let source = temp.path().join("old.mkv");
        let replacement = temp.path().join("offline.mkv");
        std::fs::write(temp.path().join("old.ffindex"), b"unowned cache").expect("cache");
        let empty_work = temp.path().join("empty-work");
        std::fs::create_dir_all(&empty_work).expect("empty directory");
        for custom in [false, true] {
            for kind in ["native", "vs", "lsmash", "bestsource", "dg", "script"] {
                let mut configuration = root_edit_fixture(temp.path());
                configuration.condor.scenes.clear();
                configuration.input = source.clone();
                configuration.temp = temp.path().join(if custom {
                    "custom".into()
                } else {
                    california_condor::utils::hash_path::hash_path(&source)
                });
                configuration.condor.input = match kind {
                    "native" => Input::Video {
                        path: source.clone(),
                        import_method: ImportMethod::FFMS2 {
                            index: Some(3),
                            cache_path: Some(temp.path().join("old.ffindex")),
                        },
                        filters: vec![Ffms2Filter::OutputFormat {
                            bit_depth: Some(10),
                            chroma: None,
                            width: None,
                            height: None,
                        }],
                    },
                    "vs" | "lsmash" | "bestsource" | "dg" => Input::VapourSynth {
                        path: source.clone(),
                        import_method: match kind {
                            "lsmash" => VapourSynthImportMethod::LSMASHWorks { index: Some(3) },
                            "bestsource" => VapourSynthImportMethod::BestSource { index: Some(3) },
                            "dg" => VapourSynthImportMethod::DGDecNV {
                                dgindexnv_executable: Some(temp.path().join("custom-dgindexnv")),
                            },
                            _ => VapourSynthImportMethod::FFMS2 { index: Some(3) },
                        },
                        cache_path: Some(temp.path().join("old.cache")),
                        filters: vec![],
                    },
                    _ => Input::VapourSynthScript {
                        source: VapourSynthScriptSource::Path(source.with_extension("vpy")),
                        variables: [("keep".into(), "yes".into())].into(),
                        index: 3,
                        filters: vec![],
                        stream_concurrently: false,
                    },
                };
                if kind == "script" {
                    configuration.input = source.with_extension("vpy");
                    configuration.temp = temp.path().join(if custom {
                        "custom".into()
                    } else {
                        california_condor::utils::hash_path::hash_path(&configuration.input)
                    });
                }
                let sequence = &mut configuration.condor.sequence_config;
                sequence.parallel_encoder.workers = Some(5);
                sequence.parallel_encoder.max_tries = 7;
                sequence.parallel_encoder.scenes_directory = if kind == "native" {
                    empty_work.clone()
                } else {
                    Default::default()
                };
                sequence.scene_concatenator.scenes_directory = if kind == "native" {
                    temp.path().join("custom-concat")
                } else if kind == "script" {
                    configuration.temp.join("scenes")
                } else {
                    Default::default()
                };
                sequence.scene_concatenator.method =
                    andean_condor::models::sequence::scene_concatenator::ConcatMethod::Ivf;
                sequence.scene_concatenator.output = Some(temp.path().join("separate.ivf"));
                let stage = Input::Video {
                    path: temp.path().join("stage.mkv"),
                    import_method: ImportMethod::FFMS2 {
                        index: Some(1),
                        cache_path: Some(temp.path().join("stage.ffindex")),
                    },
                    filters: vec![],
                };
                sequence.parallel_encoder.input = Some(stage.clone());
                sequence.target_quality = Some(
                    andean_condor::models::sequence::target_quality::TargetQualityConfig {
                        metric_input: Some(stage),
                        ..Default::default()
                    },
                );
                let new_source = if kind == "script" {
                    replacement.with_extension("vpy")
                } else {
                    replacement.clone()
                };
                let original = configuration.clone();
                let file = temp.path().join("condor.json");
                configuration.save(&file).expect("save");
                let bytes = std::fs::read(&file).expect("saved snapshot");
                InitCrud::apply(
                    &mut configuration,
                    &InitSet::Input {
                        args: root_edit_args(new_source.clone()),
                    },
                )
                .expect("offline direct edit");
                assert_eq!(
                    std::fs::read(&file).expect("direct edit must not save"),
                    bytes
                );
                condor_cmd(&temp)
                    .env("PATH", "")
                    .args(["init", "set", "input", "--path", path_str(&new_source)])
                    .assert()
                    .success();
                let after = load(&file);
                assert_eq!(
                    serde_json::to_value(&after).expect("CLI"),
                    serde_json::to_value(&configuration).expect("direct")
                );
                assert_eq!(after.input, new_source);
                let expected_temp = if custom {
                    original.temp.clone()
                } else {
                    temp.path().join(california_condor::utils::hash_path::hash_path(&new_source))
                };
                assert_eq!(after.temp, expected_temp);
                let mut expected = original;
                expected.input = new_source.clone();
                expected.temp = expected_temp.clone();
                expected.condor.sequence_config.parallel_encoder.scenes_directory =
                    if kind == "native" {
                        empty_work.clone()
                    } else {
                        expected_temp.join("scenes")
                    };
                expected.condor.sequence_config.scene_concatenator.scenes_directory =
                    if kind == "native" {
                        temp.path().join("custom-concat")
                    } else {
                        expected_temp.join("scenes")
                    };
                match &mut expected.condor.input {
                    Input::Video {
                        path,
                        import_method: ImportMethod::FFMS2 { index, cache_path },
                        ..
                    } => {
                        *path = new_source;
                        *index = None;
                        *cache_path = None;
                    },
                    Input::VapourSynth {
                        path,
                        import_method,
                        cache_path,
                        ..
                    } => {
                        *path = new_source;
                        match import_method {
                            VapourSynthImportMethod::FFMS2 { index }
                            | VapourSynthImportMethod::LSMASHWorks { index }
                            | VapourSynthImportMethod::BestSource { index } => *index = None,
                            VapourSynthImportMethod::DGDecNV { .. } => {},
                        }
                        *cache_path = None;
                    },
                    Input::VapourSynthScript { source, index, .. } => {
                        *source = VapourSynthScriptSource::Path(new_source);
                        *index = 0;
                    },
                }
                assert_eq!(
                    serde_json::to_value(&after).expect("after"),
                    serde_json::to_value(&expected).expect("expected")
                );
                assert!(
                    !after.temp.exists(),
                    "pure edit must not create work directories"
                );
                assert!(!temp.path().join("source-opened").exists());
                assert!(empty_work.is_dir());
                assert_eq!(
                    std::fs::read(temp.path().join("old.ffindex")).expect("cache retained"),
                    b"unowned cache"
                );
            }
        }
    }

    #[test]
    fn offline_root_replacement_refuses_orphan_or_non_directory_work_without_unlinking() {
        let temp = tempfile::tempdir().expect("fixture");
        for which in ["parallel", "concat", "not-directory"] {
            let mut configuration = root_edit_fixture(temp.path());
            configuration.condor.scenes.clear();
            let work = temp.path().join(which);
            if which == "not-directory" {
                std::fs::write(&work, b"not a directory").expect("file");
            } else {
                std::fs::create_dir_all(&work).expect("workdir");
                std::fs::write(work.join("orphan"), b"keep orphan").expect("orphan");
            }
            configuration.condor.sequence_config.parallel_encoder.scenes_directory =
                if which == "concat" {
                    Default::default()
                } else {
                    work.clone()
                };
            configuration.condor.sequence_config.scene_concatenator.scenes_directory =
                if which == "concat" {
                    work.clone()
                } else {
                    Default::default()
                };
            let file = temp.path().join("condor.json");
            configuration.save(&file).expect("save");
            let before = std::fs::read(&file).expect("before");
            for args in [
                vec!["init", "set", "input", "--path", "b.vpy"],
                vec!["--input", "b.vpy", "--temp", "new-work"],
            ] {
                let output = condor_cmd(&temp).env("PATH", "").args(args).output().expect("CLI");
                assert!(!output.status.success(), "{which} must refuse");
                let message = String::from_utf8_lossy(&output.stderr);
                assert!(message.contains(which), "context missing: {message}");
                assert_eq!(std::fs::read(&file).expect("after"), before);
            }
            assert!(work.exists());
            if which != "not-directory" {
                assert_eq!(
                    std::fs::read(work.join("orphan")).expect("orphan"),
                    b"keep orphan"
                );
            }
        }
    }

    fn check_inline_root_edit(phase: u8) {
        assert!((1..=3).contains(&phase));
        use andean_condor::models::input::VapourSynthScriptSource;
        use california_condor::commands::{
            DecoderMethod,
            config::{ConfigCommand, Toggle},
            handlers::init::{InitCrud, InitSet},
        };
        let temp = tempfile::tempdir().expect("fixture");
        let mut configuration = root_edit_fixture(temp.path());
        configuration.condor.scenes.clear();
        let file = temp.path().join("condor.json");
        configuration.save(&file).expect("save");
        let before = serde_json::to_value(&configuration).expect("before");
        let bytes = std::fs::read(&file).expect("before");
        let mut args = root_edit_args(temp.path().join("offline.mkv"));
        args.decoder = Some(DecoderMethod::FFMS2);
        args.filters = Some(vec![
            andean_condor::vapoursynth::vapoursynth_filters::VapourSynthFilter::Resize {
                scaler: None,
                width: None,
                height: None,
                format: Some(andean_condor::ffmpeg::FFPixelFormat::NV12),
            },
        ]);
        args.stream_concurrently = Some(Toggle::On);
        InitCrud::apply(&mut configuration, &InitSet::Input { args })
            .expect_err("invalid native format");
        assert_eq!(serde_json::to_value(&configuration).expect("after"), before);
        assert_eq!(std::fs::read(&file).expect("after"), bytes);
        condor_cmd(&temp)
            .env("PATH", "")
            .args([
                "init",
                "set",
                "input",
                "--path",
                "offline.mkv",
                "--decoder",
                "ffms2",
                "--filters",
                "resize:format=nv12;",
                "--stream-concurrently",
                "on",
            ])
            .assert()
            .failure();
        assert_eq!(std::fs::read(&file).expect("CLI after"), bytes);
        assert!(!temp.path().join("source-opened").exists());
        if phase == 1 {
            return;
        }
        configuration.input = temp.path().join("missing.vpy");
        if let Input::VapourSynthScript { source, .. } = &mut configuration.condor.input {
            *source = VapourSynthScriptSource::Text("inline source must survive".into());
        }
        configuration.condor.scenes = root_edit_fixture(temp.path()).condor.scenes;
        let before = serde_json::to_value(&configuration).expect("before");
        InitCrud::apply(
            &mut configuration,
            &InitSet::Input {
                args: root_edit_args(temp.path().join("absent/../missing.vpy")),
            },
        )
        .expect("offline lexical equivalent");
        assert_eq!(serde_json::to_value(&configuration).expect("after"), before);
        if phase == 2 {
            return;
        }
        let mut args = root_edit_args(configuration.input.clone());
        args.vs_args = Some(vec!["keep=updated".into()]);
        args.stream_concurrently = Some(Toggle::On);
        InitCrud::apply(&mut configuration, &InitSet::Input { args })
            .expect("inline settings edit");
        assert!(
            matches!(&configuration.condor.input, Input::VapourSynthScript { source: VapourSynthScriptSource::Text(text), variables, stream_concurrently: true, .. } if text == "inline source must survive" && variables["keep"] == "updated")
        );
    }

    #[test]
    fn invalid_native_root_filter_candidate_preserves_memory_and_saved_job() {
        check_inline_root_edit(1);
    }

    #[test]
    fn equivalent_offline_root_path_preserves_inline_script_text() {
        check_inline_root_edit(2);
    }

    #[test]
    fn inline_root_settings_edit_updates_variables_and_streaming_without_replacing_text() {
        check_inline_root_edit(3);
    }

    #[cfg(unix)]
    fn check_input_filesystem_access(permissions: bool) {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().expect("fixture");
        let configuration = root_edit_fixture(temp.path());
        let file = temp.path().join("condor.json");
        configuration.save(&file).expect("save");
        let before = std::fs::read(&file).expect("before");
        std::os::unix::fs::symlink("loop.vpy", temp.path().join("loop.vpy")).expect("symlink loop");
        let output = condor_cmd(&temp)
            .env("PATH", "")
            .args(["init", "set", "input", "--path", "loop.vpy"])
            .output()
            .expect("CLI");
        assert!(!output.status.success());
        let message = String::from_utf8_lossy(&output.stderr);
        assert!(
            message.contains("normaliz") && message.contains("loop.vpy"),
            "{message}"
        );
        assert_eq!(std::fs::read(&file).expect("after"), before);
        assert!(!temp.path().join("source-opened").exists());
        if !permissions {
            return;
        }
        let protected = temp.path().join("protected");
        std::fs::create_dir_all(&protected).expect("directory");
        std::fs::write(protected.join("source.vpy"), b"must not open").expect("source");
        for work_directory in [false, true] {
            let mut candidate = configuration.clone();
            candidate.condor.scenes.clear();
            if work_directory {
                candidate.condor.sequence_config.parallel_encoder.scenes_directory =
                    protected.clone();
            }
            candidate.save(&file).expect("save");
            let before = std::fs::read(&file).expect("before");
            let mut expected = candidate.clone();
            std::fs::set_permissions(&protected, std::fs::Permissions::from_mode(0o000))
                .expect("deny access");
            let replacement = if work_directory {
                "b.vpy"
            } else {
                "protected/source.vpy"
            };
            let access = if work_directory {
                std::fs::read_dir(&protected)
                    .and_then(|mut entries| entries.next().transpose().map(|_| ()))
            } else {
                std::fs::canonicalize(protected.join("source.vpy")).map(|_| ())
            };
            let output = condor_cmd(&temp)
                .env("PATH", "")
                .args(["init", "set", "input", "--path", replacement])
                .output();
            std::fs::set_permissions(&protected, std::fs::Permissions::from_mode(0o700))
                .expect("restore access before assertions/cleanup");
            let output = output.expect("CLI");
            let message = String::from_utf8_lossy(&output.stderr);
            let denied = match access {
                Err(error) => {
                    assert_eq!(
                        error.kind(),
                        std::io::ErrorKind::PermissionDenied,
                        "unexpected probe error: {error}"
                    );
                    eprintln!(
                        "input root work_directory={work_directory}: permission denial enforced by the actual operation"
                    );
                    true
                },
                Ok(()) => {
                    eprintln!(
                        "input root work_directory={work_directory}: chmod 000 did not deny the actual operation; permission-denial proof unavailable; checking allowed/nonempty-work policy behavior"
                    );
                    false
                },
            };
            if denied {
                let context = if work_directory {
                    "inspecting work directory"
                } else {
                    "normalizing input"
                };
                assert!(
                    !output.status.success()
                        && message.contains(context)
                        && message.contains("protected")
                        && message.contains("Permission denied"),
                    "{message}"
                );
            } else if work_directory {
                assert!(
                    !output.status.success()
                        && message.contains("cannot change job input")
                        && message.contains("a.vpy")
                        && message.contains("b.vpy")
                        && message.contains("--config-file"),
                    "{message}"
                );
            } else {
                assert!(
                    output.status.success(),
                    "accessible source edit must succeed: {message}"
                );
                let source = protected.join("source.vpy");
                expected.input = source.clone();
                let Input::VapourSynthScript {
                    source: saved_source,
                    ..
                } = &mut expected.condor.input
                else {
                    panic!("script fixture")
                };
                *saved_source =
                    andean_condor::models::input::VapourSynthScriptSource::Path(source.clone());
                expected.temp =
                    temp.path().join(california_condor::utils::hash_path::hash_path(&source));
                expected.condor.sequence_config.parallel_encoder.scenes_directory =
                    expected.temp.join("scenes");
                expected.condor.sequence_config.scene_concatenator.scenes_directory =
                    expected.temp.join("scenes");
            }
            if denied || work_directory {
                assert_eq!(std::fs::read(&file).expect("after"), before);
            }
            assert_eq!(
                serde_json::to_value(load(&file)).expect("after"),
                serde_json::to_value(expected).expect("expected")
            );
            assert_eq!(
                std::fs::read(protected.join("source.vpy")).expect("source sentinel"),
                b"must not open"
            );
            assert!(!temp.path().join("source-opened").exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn offline_input_edit_symlink_loop_reports_normalization_error_without_resaving() {
        check_input_filesystem_access(false);
    }

    #[cfg(unix)]
    #[test]
    fn offline_input_edit_permission_errors_preserve_job_or_accessible_source_follows_work_policy()
    {
        check_input_filesystem_access(true);
    }
}

#[cfg(test)]
mod crud {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn encode_executable_crud_accepts_path_resolvable_name_without_probing_or_missing_path_warning()
    {
        use california_condor::configuration::Configuration;
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("fixture");
        let script = temp.path().join("blank.vpy");
        std::fs::write(&script, "from vapoursynth import core, YUV420P8\ncore.std.BlankClip(width=32,height=16,format=YUV420P8).set_output()\n").expect("script");
        let config = Configuration::new(
            &script,
            &temp.path().join("out.mkv"),
            Some(temp.path()),
            None,
            None,
        )
        .expect("config");
        let file = temp.path().join("condor.json");
        config.save(&file).expect("save");
        let bin = temp.path().join("bin");
        std::fs::create_dir_all(&bin).expect("bin");
        let executable = bin.join("renamed-encoder");
        std::fs::write(
            &executable,
            "#!/bin/sh\nprintf unexpected > \"$ENCODER_IDENTITY_MARKER\"\n",
        )
        .expect("binary");
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))
            .expect("permissions");
        let marker = temp.path().join("must-not-probe");
        let output = condor_cmd(&temp)
            .env("PATH", &bin)
            .env("ENCODER_IDENTITY_MARKER", &marker)
            .args(["encode", "set", "executable", "renamed-encoder"])
            .assert()
            .success()
            .get_output()
            .clone();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("does not exist"),
            "valid PATH name: {stderr}"
        );
        let output = condor_cmd(&temp)
            .env("PATH", &bin)
            .args(["encode", "info", "executable"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&output).expect("info"),
            serde_json::json!("renamed-encoder")
        );
        assert_eq!(
            load(&file).condor.encoder.executable(),
            std::path::Path::new("renamed-encoder")
        );
        assert!(
            !marker.exists(),
            "pure edits must not probe even an available binary"
        );
    }

    fn check_format_crud(phase: u8) {
        assert!((1..=3).contains(&phase));
        use andean_condor::models::input::{Ffms2Filter, ImportMethod};
        use california_condor::configuration::Configuration;
        let temp = tempfile::tempdir().expect("CLI format fixture");
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
        let source = temp.path().join("configuration-only.mkv");
        std::fs::write(&source, b"must not be decoded or indexed").expect("existing source path");
        config.condor.input = Input::Video {
            path: source,
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
        let path = temp.path().join("condor.json");
        config.save(&path).expect("save native config");
        let native = std::fs::read(&path).expect("native snapshot");
        condor_cmd(&temp)
            .args(["init", "set", "input", "--decoder", "vs-ffms2"])
            .assert()
            .success();
        assert!(matches!(
            load(&path).condor.input.vapoursynth_filters(),
            [
                andean_condor::vapoursynth::vapoursynth_filters::VapourSynthFilter::OutputFormat {
                    bit_depth: Some(10),
                    chroma: None,
                    ..
                }
            ]
        ));
        if phase >= 2 {
            let deferred = std::fs::read(&path).expect("deferred snapshot");
            for format in ["nv12", "gbrp", "yuv440p"] {
                std::fs::write(&path, &native).expect("restore native fixture");
                let filter = format!("resize:format={format};width=16;");
                let output = condor_cmd(&temp)
                    .args(["init", "set", "input", "--filters", &filter])
                    .assert()
                    .failure()
                    .get_output()
                    .stderr
                    .clone();
                assert!(
                    String::from_utf8_lossy(&output).contains(format),
                    "{output:?}"
                );
                assert_eq!(
                    std::fs::read(&path).expect("native config after error"),
                    native
                );
                if phase == 2 {
                    continue;
                }
                std::fs::write(&path, &deferred).expect("restore VS fixture");
                condor_cmd(&temp)
                    .args(["init", "set", "input", "--filters", &filter])
                    .assert()
                    .success();
                let explicit = std::fs::read(&path).expect("explicit VS format");
                let output = condor_cmd(&temp)
                    .args(["init", "set", "input", "--decoder", "ffms2"])
                    .assert()
                    .failure()
                    .get_output()
                    .stderr
                    .clone();
                assert!(
                    String::from_utf8_lossy(&output).contains(format),
                    "{output:?}"
                );
                assert_eq!(
                    std::fs::read(&path).expect("VS config after error"),
                    explicit
                );
            }
        }
        assert!(!temp.path().join("must-not-exist.ffindex").exists());
        let mut entries = std::fs::read_dir(temp.path())
            .expect("source directory")
            .map(|entry| entry.expect("entry").file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        entries.sort();
        assert_eq!(
            entries,
            ["blank.vpy", "condor.json", "condor.json.lock", "configuration-only.mkv", "logs"],
            "CLI logs and the stable job sidecar are expected, source indexes are not"
        );
    }

    #[test]
    fn native_to_vapoursynth_decoder_edit_preserves_deferred_output_format() {
        check_format_crud(1);
    }

    #[test]
    fn cli_native_format_rejection_preserves_saved_bytes_without_indexing() {
        check_format_crud(2);
    }

    #[test]
    fn cli_vapoursynth_format_edit_saves_but_native_decoder_switch_refuses_unsupported_format() {
        check_format_crud(3);
    }

    /// The noise scaler defaults stay consistent when nothing is configured
    /// yet, so `info` never fails on a fresh configuration.
    #[serial]
    #[test]
    fn component_info_on_fresh_job_emits_valid_json_without_materializing_noise_scaler() {
        if !ffmpeg_is_available() {
            return;
        }
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        init_config(&temp);

        for args in [
            vec!["detect-scenes", "info"],
            vec!["scale-noise", "info"],
            vec!["benchmark", "info"],
            vec!["optimize-bitrate", "info"],
            vec!["detect-noise", "info"],
            vec!["target-quality", "info"],
            vec!["scale-speed", "info"],
            vec!["concatenate", "info"],
            vec!["quality-check", "info"],
        ] {
            let output =
                condor_cmd(&temp).args(&args).assert().success().get_output().stdout.clone();
            serde_json::from_slice::<serde_json::Value>(&output)
                .unwrap_or_else(|err| panic!("{args:?} prints valid JSON: {err}"));
        }

        // Optional sections report themselves as null until they are set.
        let scaler = load(&temp.path().join("condor.json")).condor.sequence_config.noise_scaler;
        assert!(scaler.is_none(), "noise scaler is unset");
    }

    /// The whole section goes back to null, as the CLI contract promises for
    /// every optional section.
    #[serial]
    #[test]
    fn reset_section_nulls_optional_sections() {
        if !ffmpeg_is_available() {
            return;
        }
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let config_path = init_config(&temp);

        // Nothing configured yet: resetting the section is a no-op, not an
        // error, and never materialises it.
        for command in ["detect-noise", "target-quality", "quality-check", "scale-noise"] {
            condor_cmd(&temp).args([command, "reset"]).assert().success();
        }

        condor_cmd(&temp)
            .args(["detect-noise", "set", "reference-filters", "resize:width=960;"])
            .assert()
            .success();
        condor_cmd(&temp)
            .args(["target-quality", "set", "target", "80"])
            .assert()
            .success();
        condor_cmd(&temp)
            .args(["quality-check", "set", "statistic", "mean"])
            .assert()
            .success();
        condor_cmd(&temp)
            .args(["scale-noise", "set", "threshold", "0.002"])
            .assert()
            .success();

        let config = load(&config_path);
        assert!(
            config.condor.sequence_config.noise_detector.is_some(),
            "noise detector is materialized"
        );
        assert!(
            config.condor.sequence_config.target_quality.is_some(),
            "target quality is materialized"
        );
        assert!(
            config.condor.sequence_config.quality_check.is_some(),
            "quality check is materialized"
        );
        assert!(
            config.condor.sequence_config.noise_scaler.is_some(),
            "noise scaler is materialized"
        );

        for command in ["detect-noise", "target-quality", "quality-check", "scale-noise"] {
            condor_cmd(&temp).args([command, "reset"]).assert().success();
        }

        let config = load(&config_path);
        assert!(
            config.condor.sequence_config.noise_detector.is_none(),
            "noise detector is null again"
        );
        assert!(
            config.condor.sequence_config.target_quality.is_none(),
            "target quality is null again"
        );
        assert!(
            config.condor.sequence_config.quality_check.is_none(),
            "quality check is null again"
        );
        assert!(
            config.condor.sequence_config.noise_scaler.is_none(),
            "noise scaler is null again"
        );
    }

    /// A write reports the property it changed, which is enough to explain the
    /// run; the value it left behind is printed only under `--verbose`.
    #[serial]
    #[test]
    fn benchmark_threshold_set_and_reset_print_values_only_with_verbose() {
        if !ffmpeg_is_available() {
            return;
        }
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        init_config(&temp);

        let plain = condor_cmd(&temp)
            .args(["benchmark", "set", "threshold", "8"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let plain = String::from_utf8(plain).expect("stdout is UTF-8");
        assert!(plain.contains("threshold updated"), "output is {plain}");
        assert!(
            !plain.contains('8'),
            "the value waits for --verbose: {plain}"
        );

        let verbose = condor_cmd(&temp)
            .args(["--verbose", "benchmark", "set", "threshold", "8"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let verbose = String::from_utf8(verbose).expect("stdout is UTF-8");
        assert!(
            verbose.contains("threshold updated") && verbose.contains('8'),
            "--verbose adds the value: {verbose}"
        );

        let plain = condor_cmd(&temp)
            .args(["benchmark", "reset", "threshold"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let plain = String::from_utf8(plain).expect("stdout is UTF-8");
        assert!(plain.contains("reset to its default"), "output is {plain}");
        assert!(
            !plain.contains(&BenchmarkerConfig::default().threshold.to_string()),
            "the restored value waits for --verbose: {plain}"
        );

        let verbose = condor_cmd(&temp)
            .args(["--verbose", "benchmark", "reset", "threshold"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let verbose = String::from_utf8(verbose).expect("stdout is UTF-8");
        assert!(
            verbose.contains(&BenchmarkerConfig::default().threshold.to_string()),
            "--verbose adds the restored default: {verbose}"
        );
    }

    fn check_positional_statistic(phase: u8) {
        assert!((1..=3).contains(&phase));
        if !ffmpeg_is_available() {
            return;
        }
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let config_path = init_config(&temp);

        condor_cmd(&temp)
            .args(["target-quality", "set", "statistic", "percentile", "90"])
            .assert()
            .success();
        let quality = load(&config_path)
            .condor
            .sequence_config
            .target_quality
            .expect("target quality exists");
        assert_eq!(
            quality.probing.statistic,
            ProbeStatistic::Percentile(90.0),
            "positional percentile is set"
        );
        if phase == 1 {
            return;
        }

        condor_cmd(&temp)
            .args(["quality-check", "set", "statistic", "standard-deviation-distance", "-1.5"])
            .assert()
            .success();
        let check = load(&config_path)
            .condor
            .sequence_config
            .quality_check
            .expect("quality check exists");
        assert_eq!(
            check.statistic,
            ProbeStatistic::StandardDeviationDistance { sigma: -1.5 },
            "positional sigma is set"
        );
        if phase == 2 {
            return;
        }

        // Passing both positional and flag is rejected.
        condor_cmd(&temp)
            .args(["target-quality", "set", "statistic", "percentile", "90", "--percentile", "10"])
            .assert()
            .failure();
    }

    #[serial]
    #[test]
    fn target_quality_percentile_crud_accepts_positional_value() {
        check_positional_statistic(1);
    }

    #[serial]
    #[test]
    fn quality_check_standard_deviation_crud_accepts_negative_positional_sigma() {
        check_positional_statistic(2);
    }

    #[serial]
    #[test]
    fn statistic_crud_rejects_simultaneous_positional_and_flag_value() {
        check_positional_statistic(3);
    }

    fn check_streaming_and_executable(executable: bool) {
        if !ffmpeg_is_available() {
            return;
        }
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let config_path = init_config(&temp);
        let script = temp.path().join("script.vpy");
        std::fs::write(
            &script,
            "import vapoursynth as vs\ncore = vs.core\nclip = core.std.BlankClip(width=16, \
             height=16, format=vs.YUV420P8)\nclip.set_output(0)\n",
        )
        .expect("script writes");

        condor_cmd(&temp)
            .args([
                "init",
                "set",
                "input",
                "--path",
                path_str(&script),
                "--stream-concurrently",
                "off",
            ])
            .assert()
            .success();
        let input = load(&config_path).condor.input;
        assert!(
            matches!(
                input,
                Input::VapourSynthScript {
                    stream_concurrently: false,
                    ..
                }
            ),
            "stream_concurrently is false: {input:?}"
        );
        if !executable {
            return;
        }

        // Setting executable on encode warns when the binary path does not
        // exist, and still saves the path; reset clears it.
        let missing_bin = temp.path().join("no-such-encoder");
        let stderr = condor_cmd(&temp)
            .args(["encode", "set", "executable", path_str(&missing_bin)])
            .assert()
            .success()
            .get_output()
            .stderr
            .clone();
        let stderr = String::from_utf8(stderr).expect("stderr is UTF-8");
        assert!(
            stderr.contains("does not exist"),
            "missing executable path warns: {stderr}"
        );
        assert_eq!(
            load(&config_path).condor.encoder.custom_executable(),
            Some(&missing_bin),
            "custom executable is saved"
        );

        condor_cmd(&temp).args(["encode", "reset", "executable"]).assert().success();
        assert_eq!(
            load(&config_path).condor.encoder.custom_executable(),
            None,
            "custom executable is cleared"
        );
    }

    #[serial]
    #[test]
    fn init_script_input_crud_saves_stream_concurrently_off() {
        check_streaming_and_executable(false);
    }

    #[serial]
    #[test]
    fn encode_missing_executable_crud_warns_saves_path_and_reset_clears_it() {
        check_streaming_and_executable(true);
    }
}
