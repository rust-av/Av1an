use super::support::run_bounded;
use super::*;
use std::{fs, io::Write};

#[test]
fn vpx_pipe_outputs_decode_three_frames_with_codec_specific_colour() {
    use av_decoders::ColorSpace;

    let temp = tempfile::tempdir().expect("fixture");
    let root = temp.path();
    let vpx = which::which("vpxenc").expect("install vpxenc to run the native regression");
    let ffprobe = which::which("ffprobe").expect("install ffprobe to verify decoded frames");
    let help = run_bounded(Command::new(&vpx).arg("--help"), &root.join("help.log"));
    let (vp8_help, vp9_help) = help.split_once("VP9 Specific Options:").expect("VP9 options");
    assert!(!vp8_help.contains("--color-space"));
    assert!(vp9_help.contains("--color-space"));
    for version in help.lines().filter(|line| line.contains("WebM Project")) {
        eprintln!("VPX installed {}: {version}", vpx.display());
    }

    for codec in ["vp8", "vp9"] {
        let mut clip = super::clip_info();
        clip.num_frames = 3;
        clip.resolution = (64, 64);
        clip.format_info =
            andean_condor::core::input::pixel_format::PixelFormat::VapourSynth { bit_depth: 8 };
        clip.color_space = Some(ColorSpace::BT709);
        let mut y4m = b"YUV4MPEG2 W64 H64 F24:1 Ip A1:1 C420jpeg\n".to_vec();
        for frame in 0..3 {
            y4m.extend_from_slice(b"FRAME\n");
            y4m.extend(vec![32 + frame * 64; 64 * 64]);
            y4m.extend(vec![128; 64 * 64 / 2]);
        }
        let output = root.join(format!("{codec}.ivf"));
        // VPX defaults target VP9; select native-valid options, not a VP8 sanitizer.
        let mut encoder = Encoder::default_from_base(&EncoderBase::VPX, true);
        *encoder.passes_mut().expect("passes") = EncoderPasses::All(1);
        encoder.set_executable(Some(vpx.clone()));
        encoder
            .parameters_mut()
            .insert("codec".into(), CLIParameter::new_string("--", "=", codec));
        encoder
            .parameters_mut()
            .insert("ivf".into(), CLIParameter::new_bool("--", true));
        encoder
            .parameters_mut()
            .insert("cpu-used".into(), CLIParameter::new_number("--", "=", 6.0));
        let argv = encoder.parameters_with_output((1, 1), &output, &clip).expect("production argv");
        eprintln!("VPX {codec} production argv: {argv:?}");
        let (reader, mut writer) = std::io::pipe().expect("nonseekable stdin");
        let producer = thread::spawn(move || writer.write_all(&y4m));
        let log = run_bounded(
            Command::new(encoder.executable()).args(&argv).stdin(reader),
            &root.join(format!("{codec}-encode.log")),
        );
        producer.join().expect("producer thread").expect("finite pipe write");
        let bytes = fs::metadata(&output).expect("encoded output").len();
        assert!(bytes > 0);
        eprintln!("VPX {codec} native output: {bytes} bytes\n{log}");
        let probe = run_bounded(
            Command::new(&ffprobe)
                .args([
                    "-v",
                    "error",
                    "-select_streams",
                    "v:0",
                    "-count_frames",
                    "-show_entries",
                    "stream=codec_name,nb_read_frames,color_space",
                    "-of",
                    "default=noprint_wrappers=1",
                ])
                .arg(&output)
                .stdin(Stdio::null()),
            &root.join(format!("{codec}-probe.log")),
        );
        assert_eq!(
            probe.trim(),
            format!(
                "codec_name={codec}\ncolor_space={}\nnb_read_frames=3",
                if codec == "vp9" { "bt709" } else { "unknown" }
            )
        );
        eprintln!("VPX independently decoded {codec} output:\n{probe}");
    }
}
