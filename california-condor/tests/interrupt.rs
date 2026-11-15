//! Actual process frontend, raw keyboard, OS interrupts and native restart coverage.
#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
    fn check_cli_interrupt(modes: &[&str]) {
        use andean_condor::models::{
            encoder::{Encoder, EncoderBase},
            input::{Input, VapourSynthScriptSource},
        };
        use california_condor::{configuration::Configuration, test_helpers::*};
        use std::{collections::HashMap, process::Command};

        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path();
        let source = root.join("source.vpy");
        std::fs::write(&source, "from vapoursynth import core,YUV420P8\ncore.std.BlankClip(width=64,height=64,length=4,format=YUV420P8).set_output()\n").unwrap();
        let video = TestVideo {
            path: source.clone(),
            width: 64,
            height: 64,
            frames: 4,
            fps_rational: (24, 1),
            format: andean_condor::ffmpeg::FFPixelFormat::YUV420P,
            scenes: vec![(0, 2), (2, 4)],
        };
        let mut config = default_config(&video, &root.join("output.mkv"), &root.join("work"));
        config.condor.input = Input::VapourSynthScript {
            source: VapourSynthScriptSource::Path(source),
            variables: HashMap::new(),
            index: 0,
            filters: vec![],
            stream_concurrently: false,
        };
        let mut encoder = Encoder::default_from_base(&EncoderBase::X264, true);
        encoder.set_executable(Some(root.join("encoder")));
        encoder.set_append_color_data(false);
        config.condor.encoder = encoder.clone();
        config.condor.output.overwrite = true;
        config.condor.scenes = video.mock_scenes(&encoder);
        config.condor.sequence_config.parallel_encoder.workers = Some(1);
        config.condor.sequence_config.parallel_encoder.validate_frame_count = false;
        Configuration::save(&config, &root.join("template.json")).unwrap();
        let output = Command::new("python3")
            .arg("-B")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/interrupt/pty.py"
            ))
            .arg("--cli-modes")
            .arg(assert_cmd::cargo::cargo_bin!("av1an"))
            .arg(root)
            .args(modes)
            .output()
            .expect("python3 is required for the Linux PTY regression");
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8_lossy(&output.stdout);
        for mode in modes {
            for route in ["standalone", "root"] {
                assert_eq!(
                    text.matches(&format!("{mode}/{route}: status=")).count(),
                    1,
                    "{text}"
                );
            }
        }
        assert_eq!(text.lines().filter(|line| *line == format!("CLI_INTERRUPT_CASES_COMPLETED={}", modes.len() * 2)).count(), 1, "{text}");
        println!("{}", String::from_utf8_lossy(&output.stdout));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cli_keyboard_graceful_interrupt_finishes_only_active_scene_and_restores_terminal() {
        check_cli_interrupt(&["key-graceful"]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cli_keyboard_repeated_interrupt_forces_joined_child_tree_cleanup() {
        check_cli_interrupt(&["key-force"]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cli_keyboard_then_signal_interrupt_forces_joined_cleanup() {
        check_cli_interrupt(&["key-mixed"]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cli_sigint_or_sigterm_graceful_interrupt_preserves_active_scene_without_starting_next() {
        check_cli_interrupt(&["int-graceful", "term-graceful"]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cli_repeated_signal_interrupt_forces_cleanup_without_partial_chunks() {
        check_cli_interrupt(&["int-force", "term-force"]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cli_forced_input_or_override_indexing_never_publishes_index() {
        check_cli_interrupt(&["open-force", "override-force"]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn native_cli_pipeline_preserves_order_and_provenance_across_interrupt_restart_and_stale_identity_refusal()
     {
        use andean_condor::{
            ffmpeg::FFPixelFormat,
            models::{
                encoder::{Encoder, EncoderBase, cli_parameter::CLIParameter},
                input::{Input, VapourSynthScriptSource},
                sequence::{
                    quality_check::QualityCheckConfig,
                    scene_detector::{SceneDetectionMethod, ScenecutMethod},
                    target_quality::{TargetQualityConfig, types::QualityMetric},
                },
            },
        };
        use california_condor::test_helpers::{TestVideo, default_config};
        use std::{collections::HashMap, process::Command};

        let fixture = tempfile::tempdir().expect("native pipeline fixture");
        let root = fixture.path();
        let source = root.join("source.vpy");
        std::fs::write(&source, "from vapoursynth import core,YUV420P8\nframes=[]\nfor n in range(16):\n a=core.std.BlankClip(width=64,height=128,length=1,format=YUV420P8,fpsnum=24,fpsden=1,color=[32+8*n,64+4*n,192-4*n])\n b=core.std.BlankClip(a,color=[48+8*n,72+4*n,184-4*n])\n frames.append(core.std.StackHorizontal([a,b]))\ncore.std.Splice(frames).set_output()\n").expect("pattern source");
        let video = TestVideo {
            path: source.clone(),
            width: 128,
            height: 128,
            frames: 16,
            fps_rational: (24, 1),
            format: FFPixelFormat::YUV420P,
            scenes: vec![],
        };
        let mut config = default_config(&video, &root.join("final.mkv"), &root.join("work"));
        config.condor.input = Input::VapourSynthScript {
            source: VapourSynthScriptSource::Path(source),
            variables: HashMap::new(),
            index: 0,
            filters: vec![],
            stream_concurrently: false,
        };
        let mut encoder = Encoder::default_from_base(&EncoderBase::X264, false);
        encoder.set_append_color_data(false);
        encoder.set_quantizer(20.0);
        encoder.parameters_mut().extend(CLIParameter::new_strings(
            "--",
            " ",
            &[("preset", "ultrafast"), ("threads", "1")],
        ));
        config.condor.encoder = encoder;
        config.condor.sequence_config.parallel_encoder.workers = Some(1);
        config.condor.sequence_config.parallel_encoder.max_tries = 1;
        config.condor.sequence_config.scene_detector.method = SceneDetectionMethod::AVSceneChange {
            minimum_length: 16,
            maximum_length: 100,
            method: ScenecutMethod::Standard,
            save_scores: true,
        };
        let metric = QualityMetric::VMAF {
            target_range: (98.0, 100.0),
            resolution: None,
            scaler: "bicubic".into(),
            threads: 0,
            model: None,
            features: vec![],
        };
        config.condor.sequence_config.target_quality = Some(TargetQualityConfig {
            metric: metric.clone(),
            quantizer_range: (0, 12),
            maximum_probes: 2,
            ..Default::default()
        });
        config.condor.sequence_config.quality_check = Some(QualityCheckConfig {
            metric,
            ..Default::default()
        });
        assert!(config.condor.sequence_config.speed_scaler.speed_quantizers.is_empty());
        config.save(&root.join("condor.json")).expect("real configuration");
        let output = Command::new("python3")
            .arg("-B")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/interrupt/pty.py"
            ))
            .arg("--native-pipeline")
            .arg(assert_cmd::cargo::cargo_bin!("av1an"))
            .arg(root)
            .output()
            .expect("required Python PTY supervisor");
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        println!("{}", String::from_utf8_lossy(&output.stdout));
    }
}
