//! V02: actual staged CLI and native terminal cancellation, never the benchmark stage.
#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
    #[test]
    fn v02_native_pipeline_and_interrupted_restart() {
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

        let fixture = tempfile::tempdir().expect("V02 fixture");
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
                "/tests/t06_interrupts.py"
            ))
            .arg("--v02")
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
