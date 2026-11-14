use super::support::run_bounded;
use super::*;
use std::{fs, io::Write};

#[test]
fn installed_aom_scene_default_has_only_initial_decoded_keyframe() {
    let temp = tempfile::tempdir().expect("fixture");
    let root = temp.path();
    let aom = which::which("aomenc").expect("install aomenc to run the native regression");
    let ffprobe = which::which("ffprobe").expect("install ffprobe to verify decoded keyframes");
    let help = run_bounded(Command::new(&aom).arg("--help"), &root.join("help.log"));
    assert!(help.contains("--disable-kf"));
    eprintln!(
        "AOM keyframes installed {}: {}",
        aom.display(),
        help.lines()
            .find(|line| line.contains("AOMedia Project AV1 Encoder"))
            .expect("version")
    );
    let mut clip = super::clip_info();
    clip.num_frames = 32;
    clip.resolution = (64, 64);
    clip.format_info =
        andean_condor::core::input::pixel_format::PixelFormat::VapourSynth { bit_depth: 8 };
    let mut y4m = b"YUV4MPEG2 W64 H64 F24:1 Ip A1:1 C420jpeg\n".to_vec();
    // A hard cut from flat luma to a checkerboard must not split an already cut scene.
    for frame in 0..32 {
        y4m.extend_from_slice(b"FRAME\n");
        for y in 0..64 {
            for x in 0..64 {
                y4m.push(if frame < 16 {
                    32
                } else if (x / 8 + y / 8) % 2 == 0 {
                    64
                } else {
                    224
                });
            }
        }
        y4m.extend(vec![128; 64 * 64 / 2]);
    }
    let input = root.join("internal-cut.y4m");
    fs::write(&input, y4m).expect("finite Y4M");
    for user_enabled in [true, false] {
        let name = if user_enabled {
            "user-enabled"
        } else {
            "default"
        };
        let output = root.join(format!("{name}.ivf"));
        let mut encoder = Encoder::default_from_base(&EncoderBase::AOM, false);
        *encoder.passes_mut().expect("passes") = EncoderPasses::All(1);
        encoder.set_append_color_data(false);
        encoder
            .parameters_mut()
            .insert("ivf".into(), CLIParameter::new_bool("--", true));
        if user_enabled {
            encoder
                .parameters_mut()
                .insert("disable-kf".into(), CLIParameter::new_bool("--", false));
            encoder.parameters_mut().insert(
                "kf-max-dist".into(),
                CLIParameter::new_number("--", "=", 4.0),
            );
        }
        let argv = encoder.parameters_with_output((1, 1), &output, &clip).expect("production argv");
        eprintln!("AOM keyframes {name} production argv: {argv:?}");
        run_bounded(
            Command::new(&aom).args(&argv).stdin(fs::File::open(&input).expect("input")),
            &root.join(format!("{name}-encode.log")),
        );
        let probe = run_bounded(
            Command::new(&ffprobe)
                .args([
                    "-v",
                    "error",
                    "-select_streams",
                    "v:0",
                    "-show_frames",
                    "-show_entries",
                    "frame=key_frame",
                    "-of",
                    "csv=p=0",
                ])
                .arg(&output)
                .stdin(Stdio::null()),
            &root.join(format!("{name}-probe.log")),
        );
        let frames: Vec<u8> = probe
            .lines()
            .map(|line| line.parse().expect("decoded key_frame flag"))
            .collect();
        assert_eq!(frames.len(), 32, "{name}: {probe}");
        assert!(frames.iter().all(|flag| *flag <= 1), "{probe}");
        let keyframes: Vec<_> = frames
            .iter()
            .enumerate()
            .filter_map(|(index, flag)| (*flag == 1).then_some(index))
            .collect();
        eprintln!(
            "AOM keyframes {name}: {} decoded frames, keyframe indices {keyframes:?}",
            frames.len()
        );
        if user_enabled {
            assert_eq!(keyframes.first(), Some(&0));
            assert!(
                keyframes.len() > 1,
                "positive oracle must see additional keyframes"
            );
        } else {
            assert_eq!(
                keyframes,
                [0],
                "already cut scenes must not gain automatic keyframes"
            );
        }
    }
}

#[test]
fn aom_one_pass_pipe_output_decodes_three_av1_frames() {
    let temp = tempfile::tempdir().expect("fixture");
    let root = temp.path();
    let aom = which::which("aomenc").expect("install aomenc to run the native regression");
    let ffprobe = which::which("ffprobe").expect("install ffprobe to verify decoded frames");
    let help = run_bounded(Command::new(&aom).arg("--help"), &root.join("help.log"));
    assert!(help.contains("--passes=<arg>"), "{help}");
    eprintln!(
        "AOM installed {}: {}",
        aom.display(),
        help.lines()
            .find(|line| line.contains("AOMedia Project AV1 Encoder"))
            .expect("version")
    );

    let job = root.join("job space 映像");
    let cwd = root.join("cwd");
    fs::create_dir_all(&job).expect("job");
    fs::create_dir_all(&cwd).expect("cwd");
    fs::write(cwd.join("scene0.log"), "unrelated CWD sentinel").expect("sentinel");
    let mut y4m = b"YUV4MPEG2 W64 H64 F24:1 Ip A1:1 C420jpeg\n".to_vec();
    for frame in 0..3 {
        y4m.extend_from_slice(b"FRAME\n");
        y4m.extend(vec![32 + frame * 64; 64 * 64]);
        y4m.extend(vec![128; 64 * 64 / 2]);
    }
    let output = job.join("scene0.ivf");
    let mut encoder = Encoder::default_from_base(&EncoderBase::AOM, true);
    *encoder.passes_mut().expect("passes") = EncoderPasses::All(1);
    encoder.set_append_color_data(false);
    encoder.parameters_mut().extend(CLIParameter::new_numbers(
        "--",
        "=",
        &[("cpu-used", 6.0), ("threads", 1.0)],
    ));
    let argv = encoder
        .parameters_with_output((1, 1), &output, &super::clip_info())
        .expect("production argv");
    eprintln!("AOM one-pass argv: {argv:?}");
    // A real OS pipe cannot rewind. EOF follows this finite producer; the reused
    // watchdog kills/reaps a stalled encoder, releasing any blocked write.
    let (reader, mut writer) = std::io::pipe().expect("nonseekable stdin");
    let producer = thread::spawn(move || writer.write_all(&y4m));
    let log = run_bounded(
        Command::new(&aom).args(&argv).stdin(reader).current_dir(&cwd),
        &root.join("encode.log"),
    );
    producer.join().expect("producer thread").expect("finite pipe write");
    eprintln!("AOM nonseekable PIPE native log:\n{log}");
    let probe = run_bounded(
        Command::new(ffprobe)
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-count_frames",
                "-show_entries",
                "stream=codec_name,nb_read_frames",
                "-of",
                "default=noprint_wrappers=1",
            ])
            .arg(&output)
            .stdin(Stdio::null()),
        &root.join("probe.log"),
    );
    assert_eq!(probe.trim(), "codec_name=av1\nnb_read_frames=3");
    eprintln!("AOM decoded final PIPE output: {probe}");
    assert_eq!(fs::read_dir(&job).expect("job listing").count(), 1);
    assert_eq!(fs::read_dir(&cwd).expect("cwd listing").count(), 1);
    assert_eq!(
        fs::read_to_string(cwd.join("scene0.log")).expect("sentinel"),
        "unrelated CWD sentinel"
    );
}
