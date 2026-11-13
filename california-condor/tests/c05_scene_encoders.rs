#[path = "common.rs"]
mod common;

#[cfg(test)]
mod tests {
    use super::common::condor_cmd;
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
                provenance::target_context(input, tq.metric_input.as_ref(), scene, tq, &pass)
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
            let context = provenance::encode_context(
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
    fn c05_global_flags_warn_and_preserve_saved_scene_json() {
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

    #[test]
    fn c05_no_warning_without_saved_scenes_or_global_encoder_request() {
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

    #[test]
    fn c05_crud_guidance_and_real_explicit_reset_preserve_results_and_files() {
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
            vec!["encode", "set", "executable", "/missing/c05-aomenc"],
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
    fn c05_m1_probe_params_long_help_explains_existing_scene_probe_overrides_not_global_reset() {
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
