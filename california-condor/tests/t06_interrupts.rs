//! Actual process frontend, raw keyboard and OS interrupts; no parent handlers.
#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
    #[test]
    fn t06_cli_interrupt_lifecycle() {
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
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/t06_interrupts.py"
            ))
            .arg(assert_cmd::cargo::cargo_bin!("av1an"))
            .arg(root)
            .output()
            .expect("python3 is required for the Linux PTY regression");
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        println!("{}", String::from_utf8_lossy(&output.stdout));
    }
}
