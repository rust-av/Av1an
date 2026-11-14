mod ivf_splitting;
#[cfg(target_os = "linux")]
mod runtime;

use super::*;
use crate::models::encoder::photon_noise::PhotonNoise;

fn temp_directory(name: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(&format!("av1an-zone-{name}-"))
        .tempdir()
        .expect("zone fixture")
}

fn x264(quantizer: f64) -> Encoder {
    let mut encoder = Encoder::default_from_base(&EncoderBase::X264, false);
    encoder.set_quantizer(quantizer);
    encoder
}

fn x265_encoder(quantizer: f64) -> Encoder {
    let mut encoder = Encoder::default_from_base(&EncoderBase::X265, false);
    encoder.set_quantizer(quantizer);
    encoder
}

fn svt_av1(quantizer: f64) -> Encoder {
    let mut encoder = Encoder::default_from_base(&EncoderBase::SVTAV1, false);
    encoder.set_quantizer(quantizer);
    encoder
}

/// Sets the photon noise the noise scaler would have written for a scene.
fn set_photon_noise_iso(encoder: &mut Encoder, iso: u32) {
    let Encoder::SVTAV1 { photon_noise, .. } = encoder else {
        panic!("only SVT-AV1 carries photon noise");
    };
    *photon_noise = Some(PhotonNoise {
        iso,
        chroma_iso: Some(iso / 3),
        width: None,
        height: None,
        c_y: None,
        ccb: None,
        ccr: None,
    });
}

fn task(original_index: usize, frames: usize, encoder: Encoder) -> Task {
    Task {
        original_index,
        index: original_index,
        frame_indices: (0..frames).collect(),
        sub_scenes: None,
        encoder,
        output: PathBuf::from(format!("scene{original_index}.264")),
    }
}

#[test]
fn zones_cover_every_frame_of_every_scene() {
    let tasks = vec![task(0, 5, x264(20.0)), task(1, 3, x264(30.0)), task(2, 7, x264(40.5))];
    let zones = ZonePlan::from_tasks(&tasks).expect("zones built");
    assert_eq!(
        zones,
        vec![
            Zone {
                start: 0,
                end: 4,
                quantizer: 20.0,
            },
            Zone {
                start: 5,
                end: 7,
                quantizer: 30.0,
            },
            Zone {
                start: 8,
                end: 14,
                quantizer: 40.5,
            },
        ]
    );
}

#[test]
fn a_scene_without_a_quantizer_is_ineligible() {
    let mut scene = x264(30.0);
    scene.parameters_mut().remove("crf");
    let tasks = vec![task(0, 5, x264(20.0)), task(1, 3, scene)];
    assert_eq!(
        ZonePlan::try_build(&tasks),
        Err(ZoneIneligible::NoQuantizer { scene: 1 })
    );
}

#[test]
fn encoders_differing_beyond_the_quantizer_are_ineligible() {
    let mut scene = x264(30.0);
    scene.parameters_mut().insert(
        "preset".to_owned(),
        CLIParameter::new_string("--", " ", "fast"),
    );
    let tasks = vec![task(0, 5, x264(20.0)), task(1, 3, scene)];
    assert_eq!(
        ZonePlan::try_build(&tasks),
        Err(ZoneIneligible::DifferingEncoder {
            scene: 1,
            field: DifferingField::Options,
        })
    );
}

#[test]
fn missing_custom_binaries_do_not_group_as_stock() {
    let mut first = x264(20.0);
    let mut second = x264(30.0);
    first.set_executable(Some(PathBuf::from("missing-custom-encoder-one")));
    second.set_executable(Some(PathBuf::from("missing-custom-encoder-two")));
    assert_eq!(
        ZonePlan::try_build(&[task(0, 5, first), task(1, 3, second)]),
        Err(ZoneIneligible::DifferingEncoder {
            scene: 1,
            field: DifferingField::Executable,
        })
    );
}

#[test]
fn a_different_encoder_is_ineligible() {
    let mut other = Encoder::default_from_base(&EncoderBase::X265, false);
    other.set_quantizer(30.0);
    let tasks = vec![task(0, 5, x264(20.0)), task(1, 3, other)];
    assert_eq!(
        ZonePlan::try_build(&tasks),
        Err(ZoneIneligible::DifferingEncoder {
            scene: 1,
            field: DifferingField::Base,
        })
    );
}

#[test]
fn a_shared_filter_keeps_the_pass_zone_eligible() {
    let mut first = x264(20.0);
    first.set_ffmpeg_filter(Some("crop=iw-16:ih-16".to_owned()));
    let mut second = x264(30.0);
    second.set_ffmpeg_filter(Some("crop=iw-16:ih-16".to_owned()));
    let tasks = vec![task(0, 5, first), task(1, 3, second)];
    assert!(
        ZonePlan::try_build(&tasks).is_ok(),
        "scenes sharing one filtergraph still zone encode"
    );
}

#[test]
fn differing_filters_are_ineligible() {
    let mut first = x264(20.0);
    first.set_ffmpeg_filter(Some("crop=iw-16:ih-16".to_owned()));
    let mut second = x264(30.0);
    second.set_ffmpeg_filter(Some("hflip".to_owned()));
    let tasks = vec![task(0, 5, first), task(1, 3, second)];
    assert_eq!(
        ZonePlan::try_build(&tasks),
        Err(ZoneIneligible::DifferingEncoder {
            scene: 1,
            field: DifferingField::FfmpegFilter,
        })
    );
}

#[test]
fn a_filter_on_only_one_scene_is_ineligible() {
    let mut filtered = x264(30.0);
    filtered.set_ffmpeg_filter(Some("crop=iw-16:ih-16".to_owned()));
    let tasks = vec![task(0, 5, x264(20.0)), task(1, 3, filtered)];
    assert_eq!(
        ZonePlan::try_build(&tasks),
        Err(ZoneIneligible::DifferingEncoder {
            scene: 1,
            field: DifferingField::FfmpegFilter,
        })
    );
}

/// The zone encoder compares photon noise, so a scene the noise scaler
/// gave a stronger iso must be reported as a photon-noise difference, and
/// the fallback log must name the field.
#[test]
fn scenes_differing_only_in_photon_noise_name_the_field() {
    let mut first = svt_av1(20.0);
    set_photon_noise_iso(&mut first, 600);
    let mut second = svt_av1(30.0);
    set_photon_noise_iso(&mut second, 12000);
    let tasks = vec![task(0, 5, first), task(1, 3, second)];
    let reason = ZonePlan::try_build(&tasks).expect_err("scenes differ");
    assert_eq!(
        reason,
        ZoneIneligible::DifferingEncoder {
            scene: 1,
            field: DifferingField::PhotonNoise,
        }
    );
    assert_eq!(
        reason.to_string(),
        "scene 1 differs from the first scene beyond the quantizer: photon_noise"
    );
}

/// Equality is structural: two photon noises that serialize identically
/// are not a difference, so the quantizer remains the only thing scenes
/// may vary in.
#[test]
fn identical_photon_noise_is_not_a_difference() {
    let mut first = svt_av1(20.0);
    set_photon_noise_iso(&mut first, 6177);
    let mut second = svt_av1(30.0);
    set_photon_noise_iso(&mut second, 6177);
    assert_eq!(
        differing_field(&first, &second),
        None,
        "identical photon noise leaves the quantizer as the only difference"
    );
}

#[test]
fn the_ffmpeg_encoder_never_carries_a_filter() {
    let mut encoder = Encoder::default_from_base(&EncoderBase::FFmpeg, false);
    encoder.set_ffmpeg_filter(Some("crop=iw-16:ih-16".to_owned()));
    assert_eq!(
        encoder.ffmpeg_filter(),
        None,
        "the FFmpeg encoder filters through its options instead"
    );
}

#[test]
fn multi_pass_encodes_are_ineligible() {
    let mut encoder = x264(20.0);
    *encoder.passes_mut().expect("x264 has passes") = EncoderPasses::All(2);
    let tasks = vec![task(0, 5, encoder)];
    assert_eq!(
        ZonePlan::try_build(&tasks),
        Err(ZoneIneligible::NotSinglePass { scene: 0 })
    );
}

#[test]
fn user_configured_zone_options_are_ineligible() {
    let mut encoder = x264(20.0);
    encoder.parameters_mut().insert(
        "qpfile".to_owned(),
        CLIParameter::new_string("--", " ", "mine.txt"),
    );
    let tasks = vec![task(0, 5, encoder)];
    assert_eq!(
        ZonePlan::try_build(&tasks),
        Err(ZoneIneligible::ReservedOption { scene: 0 })
    );
}

#[test]
fn unsupported_encoders_are_ineligible() {
    let mut encoder = Encoder::default_from_base(&EncoderBase::AOM, false);
    encoder.set_quantizer(30.0);
    let tasks = vec![task(0, 5, encoder)];
    assert_eq!(
        ZonePlan::try_build(&tasks),
        Err(ZoneIneligible::UnsupportedEncoder(EncoderBase::AOM))
    );
}

#[test]
fn empty_passes_are_ineligible() {
    assert_eq!(ZonePlan::try_build(&[]), Err(ZoneIneligible::Empty));
}

#[test]
fn x264_options_carry_zones_and_keyframes() {
    let zones = ZonePlan::from_tasks(&[
        task(0, 5, x264(20.0)),
        task(1, 3, x264(30.0)),
        task(2, 7, x264(40.5)),
    ])
    .expect("zones built");
    let (zone_option, qpfile_content, config_content) =
        format_zone_options(EncoderBase::X264, &zones);
    assert_eq!(zone_option, "0,4,crf=20/5,7,crf=30/8,14,crf=40.5");
    assert_eq!(qpfile_content, "5 I\n8 I\n");
    assert_eq!(config_content, "");
}

#[test]
fn x265_zone_options_format_one_line_per_scene() {
    let zones = ZonePlan::from_tasks(&[task(0, 5, x264(20.0)), task(1, 3, x264(30.0))])
        .expect("zones built");
    let (zone_option, qpfile_content, config_content) =
        format_zone_options(EncoderBase::X265, &zones);
    assert_eq!(zone_option, "0 --crf 20\n5 --crf 30\n");
    assert_eq!(qpfile_content, "5 I\n");
    assert_eq!(config_content, "");
}

/// SVT-AV1 carries its zones and keyframes in a config file, keyed on the
/// option's descriptive name.
#[test]
fn svt_zone_options_format_config_lines_keyed_by_option_name() {
    let zones = ZonePlan::from_tasks(&[task(0, 5, x264(20.0)), task(1, 3, x264(30.0))])
        .expect("zones built");
    let (zone_option, qpfile_content, config_content) =
        format_zone_options(EncoderBase::SVTAV1, &zones);
    assert_eq!(zone_option, "");
    assert_eq!(qpfile_content, "");
    assert_eq!(
        config_content,
        "Zones : 0,4,20;5,7,30\nForceKeyFrames : 5f\n"
    );
}

#[test]
fn a_single_scene_needs_no_keyframe_options() {
    let zones = ZonePlan::from_tasks(&[task(0, 5, x264(20.0))]).expect("zones built");
    let (zone_option, qpfile_content, config_content) =
        format_zone_options(EncoderBase::X264, &zones);
    assert_eq!(zone_option, "0,4,crf=20");
    assert_eq!(qpfile_content, "");
    assert_eq!(config_content, "");
    let (_, qpfile_content, config_content) = format_zone_options(EncoderBase::SVTAV1, &zones);
    assert_eq!(qpfile_content, "");
    // No zone starts past frame 0, so no keyframe list is needed.
    assert_eq!(config_content, "Zones : 0,4,20\n");
}

fn check_zone_option_files() {
    let owner = temp_directory("options");
    let directory = owner.path().to_path_buf();
    let zones = ZonePlan::from_tasks(&[task(0, 5, x264(20.0)), task(1, 3, x264(30.0))])
        .expect("zones built");
    let plan = ZonePlan {
        zone_option: "0,4,crf=20/5,7,crf=30".to_owned(),
        qpfile_content: "5 I\n".to_owned(),
        config_content: String::new(),
        zones,
    };
    let mut encoder = x264(0.0);
    let zone_files = apply_zone_options(&mut encoder, &plan, &directory).expect("options applied");
    let zones = encoder.parameters().get("zones").expect("zones option");
    let CLIParameter::String {
        value: zones_value, ..
    } = zones
    else {
        panic!("zones option should be a string");
    };
    assert_eq!(zones_value, "0,4,crf=20/5,7,crf=30");
    assert_eq!(zone_files.len(), 1);
    let qpfile = fs::read_to_string(&zone_files[0]).expect("qpfile written");
    assert_eq!(qpfile, "5 I\n");
    let qpfile_option = encoder.parameters().get("qpfile").expect("qpfile option");
    let CLIParameter::String {
        value: qpfile_value,
        ..
    } = qpfile_option
    else {
        panic!("qpfile option should be a string");
    };
    assert_eq!(qpfile_value, &zone_files[0].display().to_string());
}

#[test]
fn frame_starts_ignore_headers_and_slice_continuations() {
    // IDR with first_mb_in_slice == 0 (MSB of the first RBSP byte set).
    assert!(is_frame_start(Codec::H264, &[0, 0, 1, 0x65, 0x88]));
    // P slice starting a picture.
    assert!(is_frame_start(Codec::H264, &[0, 0, 1, 0x41, 0x80]));
    // P slice continuing a picture (first_mb_in_slice != 0).
    assert!(!is_frame_start(Codec::H264, &[0, 0, 1, 0x41, 0x00]));
    // Access unit delimiter and SPS are not frames.
    assert!(!is_frame_start(Codec::H264, &[0, 0, 1, 0x09, 0x10]));
    assert!(!is_frame_start(Codec::H264, &[0, 0, 1, 0x67, 0x42]));
    // Trailing zeros alone are not a frame.
    assert!(!is_frame_start(Codec::H264, &[0, 0, 0, 0, 0]));

    // IDR_W_RADL (type 19) with first_slice_segment_in_pic_flag set.
    assert!(is_frame_start(Codec::H265, &[0, 0, 1, 0x26, 0x01, 0x80]));
    // TRAIL_R (type 1) starting a picture.
    assert!(is_frame_start(Codec::H265, &[0, 0, 1, 0x02, 0x01, 0x80]));
    // TRAIL_R continuing a picture.
    assert!(!is_frame_start(Codec::H265, &[0, 0, 1, 0x02, 0x01, 0x01]));
    // VPS (type 32) is not a frame.
    assert!(!is_frame_start(Codec::H265, &[0, 0, 1, 0x40, 0x01, 0x80]));
}

/// A real zone-encoded stream must split back into decodable parts.
///
/// The synthetic stream cannot catch the mistakes that matter most: a
/// boundary landing inside a slice instead of at a picture, or the
/// per-file headers missing so a part no longer decodes. This runs the
/// actual x264 binary when it is available.
fn check_x264_zone_stream_split() {
    let Ok(x264_binary) = which::which("x264") else {
        return; // no encoder available to test against
    };
    let owner = temp_directory("real");
    let directory = owner.path().to_path_buf();
    let source = directory.join("full.y4m");
    // 30 frames of a cheap gradient, 10 per scene.
    write_test_y4m(&source, 0, 30, 8);

    let scene_frames = [10usize, 10, 10];
    let crfs = [20u32, 40, 30];
    // Each scene encoded on its own, as Parallel Encoder does.
    for (index, crf) in crfs.iter().enumerate() {
        let scene = directory.join(format!("par{index}.y4m"));
        write_test_y4m(&scene, index * 10, 10, 8);
        let output = directory.join(format!("par{index}.264"));
        let status = std::process::Command::new(&x264_binary)
            .args([
                "--demuxer",
                "y4m",
                "--keyint",
                "infinite",
                "--scenecut",
                "0",
                "--preset",
                "ultrafast",
                "--log-level",
                "error",
                "--crf",
                &crf.to_string(),
                "-o",
                output.to_str().expect("output path"),
                scene.to_str().expect("scene path"),
            ])
            .status()
            .expect("x264 runs");
        assert!(status.success(), "per-scene encode {index} succeeded");
    }

    // The whole pass encoded once, zoned, with IDRs at the scene starts.
    let qpfile = directory.join("qpf");
    fs::write(&qpfile, "10 I\n20 I\n").expect("qpfile written");
    let whole = directory.join("zone.264");
    let status = std::process::Command::new(&x264_binary)
        .args([
            "--demuxer",
            "y4m",
            "--keyint",
            "infinite",
            "--scenecut",
            "0",
            "--preset",
            "ultrafast",
            "--log-level",
            "error",
            "--crf",
            "25",
            "--zones",
            "0,9,crf=20/10,19,crf=40/20,29,crf=30",
            "--qpfile",
            qpfile.to_str().expect("qpfile path"),
            "-o",
            whole.to_str().expect("output path"),
            source.to_str().expect("source path"),
        ])
        .status()
        .expect("x264 runs");
    assert!(status.success(), "zoned encode succeeded");

    let tasks: Vec<Task> = scene_frames
        .iter()
        .enumerate()
        .map(|(index, frames)| Task {
            original_index: index,
            index,
            frame_indices: (0..*frames).collect(),
            sub_scenes: None,
            encoder: x264(crfs[index] as f64),
            output: directory.join(format!("part{index}.264")),
        })
        .collect();
    let split = split_annexb(&whole, Codec::H264, &tasks, "264", true).expect("split");
    assert_eq!(
        split.iter().map(|(_, frames)| *frames).collect::<Vec<_>>(),
        scene_frames.to_vec(),
        "each part holds its scene's frames"
    );

    // Every part decodes on its own, to the same picture as the independent
    // encode it stands in for.
    let ffmpeg = which::which("ffmpeg").ok();
    for index in 0..scene_frames.len() {
        let part = fs::read(&tasks[index].output).expect("part readable");
        assert!(
            part.starts_with(&[0x00, 0x00, 0x00, 0x01, 0x67]),
            "part {index} opens with the stream headers"
        );
        let Some(ffmpeg) = &ffmpeg else {
            continue;
        };
        let mut decoded = Vec::new();
        for name in ["part", "par"] {
            let file = directory.join(format!("{name}{index}.264"));
            let raw = directory.join(format!("{name}{index}.raw"));
            let output = std::process::Command::new(ffmpeg)
                .args([
                    "-v",
                    "error",
                    "-i",
                    file.to_str().expect("file path"),
                    "-f",
                    "rawvideo",
                    "-pix_fmt",
                    "yuv420p",
                    "-y",
                    raw.to_str().expect("raw path"),
                ])
                .output()
                .expect("ffmpeg runs");
            let pixels = fs::read(&raw).expect("raw frames written");
            let _ = fs::remove_file(&raw);
            let frames = pixels.len() / (WIDTH_CY * HEIGHT_CY * 3 / 2);
            assert!(
                output.status.success() && frames == scene_frames[index],
                "{name}{index}.264 decoded {frames} frames, expected {}",
                scene_frames[index]
            );
            decoded.push(pixels);
        }
        // The first zone starts from the encoder's initial state, so it
        // matches the independent encode exactly. Later zones inherit
        // rate-control state across the boundary, so they differ
        // slightly -- the cost of encoding the pass in one process.
        let (worst_delta, differing) = pixel_difference(&decoded[0], &decoded[1]);
        if index == 0 {
            assert_eq!(
                differing, 0,
                "the first zone must be pixel-identical to its own encode"
            );
        } else {
            assert!(
                worst_delta <= MAX_RATE_CONTROL_CARRY_DELTA,
                "part {index} differs from the independent encode by {worst_delta}, beyond \
                 rate-control carryover"
            );
        }
    }
}

/// The same guarantee for x265, which splits on
/// `first_slice_segment_in_pic_flag` and reconfigures the quantizer from a
/// zone file rather than an inline option.
fn check_x265_zone_stream_split() {
    let Ok(x265) = which::which("x265") else {
        return; // no encoder available to test against
    };
    let Ok(ffmpeg) = which::which("ffmpeg") else {
        return; // decoding is how the parts are verified
    };
    let owner = temp_directory("x265");
    let directory = owner.path().to_path_buf();
    let source = directory.join("full.y4m");
    write_test_y4m(&source, 0, 30, 8);

    let scene_frames = [10usize, 10, 10];
    let crfs = [20u32, 40, 30];
    for (index, crf) in crfs.iter().enumerate() {
        let scene = directory.join(format!("par{index}.y4m"));
        write_test_y4m(&scene, index * 10, 10, 8);
        let output = directory.join(format!("par{index}.hevc"));
        let status = std::process::Command::new(&x265)
            .args([
                "--y4m",
                "--keyint",
                "-1",
                "--scenecut",
                "0",
                "--preset",
                "ultrafast",
                "--log-level",
                "error",
                "--crf",
                &crf.to_string(),
                "-o",
                output.to_str().expect("output path"),
                scene.to_str().expect("scene path"),
            ])
            .status()
            .expect("x265 runs");
        assert!(status.success(), "per-scene encode {index} succeeded");
    }

    let qpfile = directory.join("qpf");
    fs::write(&qpfile, "10 I\n20 I\n").expect("qpfile written");
    let zonefile = directory.join("zonefile");
    // A zone line for every scene, including the first: without one, x265
    // leaves the first zone on the global CRF rather than the
    // scene's.
    let mut zonefile_content = String::new();
    for (index, crf) in crfs.iter().enumerate() {
        let _ = writeln!(zonefile_content, "{} --crf {}", index * 10, crf);
    }
    fs::write(&zonefile, zonefile_content).expect("zonefile written");
    let whole = directory.join("zone.hevc");
    let status = std::process::Command::new(&x265)
        .args([
            "--y4m",
            "--keyint",
            "-1",
            "--scenecut",
            "0",
            "--preset",
            "ultrafast",
            "--log-level",
            "error",
            "--crf",
            "25",
            "--zonefile",
            zonefile.to_str().expect("zonefile path"),
            "--qpfile",
            qpfile.to_str().expect("qpfile path"),
            "-o",
            whole.to_str().expect("output path"),
            source.to_str().expect("source path"),
        ])
        .status()
        .expect("x265 runs");
    assert!(status.success(), "zoned encode succeeded");

    let tasks: Vec<Task> = scene_frames
        .iter()
        .enumerate()
        .map(|(index, frames)| Task {
            original_index: index,
            index,
            frame_indices: (0..*frames).collect(),
            sub_scenes: None,
            encoder: x265_encoder(crfs[index] as f64),
            output: directory.join(format!("part{index}.hevc")),
        })
        .collect();
    let split = split_annexb(&whole, Codec::H265, &tasks, "hevc", true).expect("split");
    assert_eq!(
        split.iter().map(|(_, frames)| *frames).collect::<Vec<_>>(),
        scene_frames.to_vec(),
        "each part holds its scene's frames"
    );

    for (index, expected_frames) in scene_frames.iter().enumerate() {
        let mut decoded = Vec::new();
        for name in ["part", "par"] {
            let file = directory.join(format!("{name}{index}.hevc"));
            let raw = directory.join(format!("{name}{index}.raw"));
            let output = std::process::Command::new(&ffmpeg)
                .args([
                    "-v",
                    "error",
                    "-f",
                    "hevc",
                    "-i",
                    file.to_str().expect("file path"),
                    "-f",
                    "rawvideo",
                    "-pix_fmt",
                    "yuv420p",
                    "-y",
                    raw.to_str().expect("raw path"),
                ])
                .output()
                .expect("ffmpeg runs");
            let pixels = fs::read(&raw).expect("raw frames written");
            let _ = fs::remove_file(&raw);
            let frames = pixels.len() / (WIDTH_CY * HEIGHT_CY * 3 / 2);
            assert!(
                output.status.success() && frames == *expected_frames,
                "{name}{index}.hevc decoded {frames} frames, expected {expected_frames}"
            );
            decoded.push(pixels);
        }
        let (worst_delta, differing) = pixel_difference(&decoded[0], &decoded[1]);
        if index == 0 {
            assert_eq!(
                differing, 0,
                "the first zone must be pixel-identical to its own encode"
            );
        } else {
            assert!(
                worst_delta <= MAX_RATE_CONTROL_CARRY_DELTA,
                "part {index} differs from the independent encode by {worst_delta}, beyond \
                 rate-control carryover"
            );
        }
    }
}

const WIDTH_CY: usize = 64;
const HEIGHT_CY: usize = 64;

/// Largest per-sample difference tolerated across a zone boundary.
///
/// Only the first zone starts from the encoder's initial state and so
/// is identical to an independent encode. Every later zone inherits
/// rate-control history — the lookahead and QP curve carry over the
/// boundary — so its pixels drift. Measured drift is a small quantizer
/// step (x264 ~16, x265 ~28 of 255); this bounds it loosely enough to
/// stay encoder- and content-independent while still failing if a
/// boundary lands in the wrong place.
const MAX_RATE_CONTROL_CARRY_DELTA: u8 = 32;

/// SVT-AV1 splits on IVF frame headers and forces keyframes with
/// `--force-key-frames` rather than a qpfile. Only a build with WebM IO
/// (the SVT-AV1-Essential fork) supports `--zones` at all, so this is
/// skipped unless the installed binary does.
fn check_svt_zone_keyframes() {
    let Ok(svt) = which::which("SvtAv1EncApp") else {
        return; // no encoder available to test against
    };
    let owner = temp_directory("svt");
    let directory = owner.path().to_path_buf();
    let source = directory.join("full.y4m");
    // SVT-AV1-Essential is 10-bit only.
    write_test_y4m(&source, 0, 30, 10);

    let scene_frames = [10usize, 10, 10];
    let crfs = [20u32, 40, 30];
    // The config file carries the zones and keyframes; everything else
    // stays on the command line.
    let config = directory.join("zone.config");
    fs::write(
        &config,
        "Zones : 0,9,20;10,19,40;20,29,30\nForceKeyFrames : 10f,20f\n",
    )
    .expect("config written");
    let base = vec![
        "--preset".to_owned(),
        "12".to_owned(),
        "--progress".to_owned(),
        "0".to_owned(),
        "--keyint".to_owned(),
        "0".to_owned(),
        "--scd".to_owned(),
        "0".to_owned(),
        "--rc".to_owned(),
        "0".to_owned(),
    ];

    let mut args: Vec<String> = base;
    args.extend(["-i".to_owned(), "stdin".to_owned()]);
    args.extend([
        "-b".to_owned(),
        directory.join("zone.ivf").display().to_string(),
        "--crf".to_owned(),
        "25".to_owned(),
        // A WebM IO build writes Matroska to a `.ivf` path; this forces
        // the IVF that Av1an splits.
        "--webm".to_owned(),
        "0".to_owned(),
        "-c".to_owned(),
        config.display().to_string(),
    ]);
    let Ok(mut child) = std::process::Command::new(&svt)
        .args(&args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return; // the binary exists but cannot be driven this way
    };
    if let Some(stdin) = child.stdin.take() {
        let bytes = fs::read(&source).expect("y4m readable");
        let mut stdin = stdin;
        use std::io::Write as _;
        let _ = stdin.write_all(&bytes);
    }
    let status = child.wait().expect("SvtAv1EncApp runs");
    if !status.success() {
        // Stock SVT-AV1 has no `--zones`; nothing to assert here.
        return;
    }

    let tasks: Vec<Task> = scene_frames
        .iter()
        .enumerate()
        .map(|(index, frames)| Task {
            original_index: index,
            index,
            frame_indices: (0..*frames).collect(),
            sub_scenes: None,
            encoder: svt_av1(crfs[index] as f64),
            output: directory.join(format!("part{index}.ivf")),
        })
        .collect();
    let split = split_ivf(&directory.join("zone.ivf"), &tasks, "ivf", true).expect("split");
    assert_eq!(
        split.iter().map(|(_, frames)| *frames).collect::<Vec<_>>(),
        scene_frames.to_vec(),
        "each part holds its scene's frames"
    );

    // Every part opens with an IVF header and decodes standalone.
    for task in &tasks {
        let part = fs::read(&task.output).expect("part readable");
        assert!(part.starts_with(b"DKIF"), "part opens with the IVF header");
    }
    if let Ok(ffprobe) = which::which("ffprobe") {
        let file = directory.join("zone.ivf");
        let output = std::process::Command::new(ffprobe)
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "frame=pict_type",
                "-of",
                "csv=p=0",
                file.to_str().expect("file path"),
            ])
            .output()
            .expect("ffprobe runs");
        let types: Vec<String> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|line| line.trim().to_owned())
            .collect();
        assert_eq!(
            types.len(),
            30,
            "the whole pass decoded {} frames",
            types.len()
        );
        // A keyframe at every zone start is what makes each part stand
        // in for an independent per-scene encode.
        for zone in [0usize, 10, 20] {
            assert_eq!(
                types[zone], "I",
                "frame {zone} should open a zone with a keyframe, found {}",
                types[zone]
            );
        }
    }
}

/// Returns the largest per-sample difference and how many samples differ.
fn pixel_difference(left: &[u8], right: &[u8]) -> (u8, usize) {
    assert_eq!(left.len(), right.len(), "compared frames are the same size");
    let mut worst = 0u8;
    let mut differing = 0usize;
    for (a, b) in left.iter().zip(right.iter()) {
        let delta = a.abs_diff(*b);
        if delta != 0 {
            differing += 1;
            worst = worst.max(delta);
        }
    }
    (worst, differing)
}

/// Writes `count` Y4M frames of a distinct per-frame gradient, starting at
/// source position `start`. `bit_depth` 8 writes `C420`, 10 writes
/// `C420p10` with two bytes per sample.
fn write_test_y4m(path: &Path, start: usize, count: usize, bit_depth: u8) {
    const WIDTH: usize = WIDTH_CY;
    const HEIGHT: usize = HEIGHT_CY;
    let sample_bytes = usize::from(bit_depth > 8) + 1;
    let mut bytes = Vec::new();
    let chroma = if bit_depth == 8 {
        "C420".to_owned()
    } else {
        format!("C420p{bit_depth}")
    };
    bytes.extend_from_slice(
        format!("YUV4MPEG2 W{WIDTH} H{HEIGHT} F30:1 Ip A1:1 {chroma}\n").as_bytes(),
    );
    for index in start..start + count {
        bytes.extend_from_slice(b"FRAME\n");
        for offset in 0..HEIGHT {
            let mut row = vec![0u8; WIDTH * sample_bytes];
            // A diagonal ramp differs in every frame, so a wrong boundary
            // shows up as a visibly different picture.
            row[0] = ((index * 4 + offset) % 256) as u8;
            row[WIDTH * sample_bytes / 2] = ((index * 7) % 256) as u8;
            bytes.extend_from_slice(&row);
        }
        let plane = (WIDTH / 2) * (HEIGHT / 2) * sample_bytes;
        bytes.extend(std::iter::repeat_n((index % 256) as u8, plane * 2));
    }
    fs::write(path, bytes).expect("y4m written");
}

#[test]
fn nal_reader_reassembles_every_segment() {
    let mut stream = Vec::new();
    stream.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0x00, 0x1f]);
    stream.extend_from_slice(&[0x00, 0x00, 0x01, 0x68, 0xce, 0x06, 0xe2]);
    stream.extend_from_slice(&[0x00, 0x00, 0x01, 0x65, 0x88, 0x84, 0x21, 0xa0]);
    stream.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x41, 0x9a, 0x22]);
    stream.extend_from_slice(&[0x00, 0x00, 0x01, 0x41, 0x11]);
    stream.extend_from_slice(&[0x00, 0x00, 0x00]); // trailing zeros

    let mut reader = NalReader::new(Cursor::new(stream.clone()));
    let mut segments = Vec::new();
    while let Some(segment) = reader.next_segment().expect("readable") {
        segments.push(segment);
    }
    let reassembled: Vec<u8> = segments.iter().flatten().copied().collect();
    assert_eq!(reassembled, stream);
    // One segment per start code: the orphan leading zero rides with the
    // first, and the trailing zeros ride with the last.
    assert_eq!(segments.len(), 5);
    // The orphan leading zero stays with the first segment.
    assert_eq!(
        segments[0],
        vec![0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0x00, 0x1f]
    );
    assert!(segments[2].starts_with(&[0x00, 0x00, 0x01, 0x65]));
}

#[test]
fn nal_reader_finds_start_codes_split_across_reads() {
    let stream: Vec<u8> =
        vec![0x00, 0x00, 0x00, 0x01, 0x65, 0x88, 0x00, 0x00, 0x00, 0x01, 0x41, 0x80];
    // One byte at a time so every boundary splits the stream awkwardly.
    struct Trickle<R> {
        inner: R,
    }
    impl Read for Trickle<Cursor<Vec<u8>>> {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let limit = 1.min(buffer.len());
            self.inner.read(&mut buffer[..limit])
        }
    }
    impl BufRead for Trickle<Cursor<Vec<u8>>> {
        fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
            let buffer = self.inner.fill_buf()?;
            // One byte per read, so every start code straddles a boundary.
            Ok(&buffer[..buffer.len().min(1)])
        }
        fn consume(&mut self, amount: usize) {
            self.inner.consume(amount);
        }
    }
    let mut reader = NalReader::new(Trickle {
        inner: Cursor::new(stream.clone()),
    });
    let mut segments = Vec::new();
    while let Some(segment) = reader.next_segment().expect("readable") {
        segments.push(segment);
    }
    let reassembled: Vec<u8> = segments.iter().flatten().copied().collect();
    assert_eq!(reassembled, stream);
}

/// A whole-pass IVF file, with the frame count the encoder writes into the
/// header covering every frame of the pass.
fn synthetic_ivf(frame_sizes: &[u32]) -> Vec<u8> {
    let mut ivf = vec![0u8; 32];
    ivf[..4].copy_from_slice(b"DKIF");
    ivf[6..8].copy_from_slice(&32u16.to_le_bytes());
    ivf[8..12].copy_from_slice(b"AV01");
    ivf[12..14].copy_from_slice(&64u16.to_le_bytes());
    ivf[14..16].copy_from_slice(&64u16.to_le_bytes());
    ivf[16..20].copy_from_slice(&24000u32.to_le_bytes());
    ivf[20..24].copy_from_slice(&1001u32.to_le_bytes());
    ivf[24..28].copy_from_slice(&(frame_sizes.len() as u32).to_le_bytes());
    for (index, size) in frame_sizes.iter().enumerate() {
        ivf.extend_from_slice(&size.to_le_bytes());
        ivf.extend_from_slice(&(index as u64).to_le_bytes());
        ivf.extend(std::iter::repeat_n(0xAB, *size as usize));
    }
    ivf
}

/// The frame count a file's IVF header declares.
fn ivf_nframe(path: &Path) -> u32 {
    u32::from_le_bytes(fs::read(path).expect("part read")[24..28].try_into().expect("4 bytes"))
}

fn check_ivf_scene_files() {
    let owner = temp_directory("ivf");
    let directory = owner.path().to_path_buf();
    let whole = directory.join("zone-pass.temp.ivf");
    // Frames: scene 0 gets 2, scene 1 gets 3, scene 2 gets 1.
    fs::write(&whole, synthetic_ivf(&[3, 5, 7, 2, 9, 4])).expect("ivf written");
    let tasks = vec![
        Task {
            original_index: 0,
            index: 0,
            frame_indices: (0..2).collect(),
            sub_scenes: None,
            encoder: x264(20.0),
            output: directory.join("scene0.ivf"),
        },
        Task {
            original_index: 1,
            index: 1,
            frame_indices: (0..3).collect(),
            sub_scenes: None,
            encoder: x264(30.0),
            output: directory.join("scene1.ivf"),
        },
        Task {
            original_index: 2,
            index: 2,
            frame_indices: (0..1).collect(),
            sub_scenes: None,
            encoder: x264(40.0),
            output: directory.join("scene2.ivf"),
        },
    ];
    let split = split_ivf(&whole, &tasks, "ivf", false).expect("split");
    // 32-byte header + frame headers + payloads.
    assert_eq!(
        split,
        vec![(32 + 12 + 3 + 12 + 5, 2), (32 + 12 + 7 + 12 + 2 + 12 + 9, 3), (32 + 12 + 4, 1)]
    );
    for task in &tasks {
        assert!(task.output.exists(), "{} saved", task.output.display());
    }
    // Each part starts with the IVF header.
    let head = fs::read(&tasks[1].output).expect("scene1 read");
    assert_eq!(&head[..4], b"DKIF");
    assert_eq!(
        &head[20..24],
        &1001u32.to_le_bytes(),
        "scale is not a frame count"
    );
    assert_eq!(head.len() as u64, split[1].0);
    // Each part declares its own frame count, not the whole pass's: the
    // scene concatenator sums these headers to rebuild the duration.
    assert_eq!(
        tasks.iter().map(|task| ivf_nframe(&task.output)).collect::<Vec<_>>(),
        vec![2, 3, 1],
        "per-scene frame counts"
    );
    let _ = fs::remove_file(&whole);
}

fn synthetic_h264(frames: usize) -> Vec<u8> {
    let mut stream = Vec::new();
    stream.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0x00, 0x1f]);
    stream.extend_from_slice(&[0x00, 0x00, 0x01, 0x68, 0xce, 0x06, 0xe2]);
    for frame in 0..frames {
        let nal_type = if frame % 2 == 0 { 0x65 } else { 0x41 };
        stream.extend_from_slice(&[0x00, 0x00, 0x01, nal_type, 0x88, 0xC0]);
        stream.extend(std::iter::repeat_n(frame as u8 + 1, 4));
    }
    stream
}

fn check_annexb_scene_headers() {
    let owner = temp_directory("annexb");
    let directory = owner.path().to_path_buf();
    let whole = directory.join("zone-pass.temp.264");
    fs::write(&whole, synthetic_h264(6)).expect("stream written");
    let tasks = vec![
        Task {
            original_index: 0,
            index: 0,
            frame_indices: (0..2).collect(),
            sub_scenes: None,
            encoder: x264(20.0),
            output: directory.join("scene0.264"),
        },
        Task {
            original_index: 1,
            index: 1,
            frame_indices: (0..3).collect(),
            sub_scenes: None,
            encoder: x264(30.0),
            output: directory.join("scene1.264"),
        },
        Task {
            original_index: 2,
            index: 2,
            frame_indices: (0..1).collect(),
            sub_scenes: None,
            encoder: x264(40.0),
            output: directory.join("scene2.264"),
        },
    ];
    let split = split_annexb(&whole, Codec::H264, &tasks, "264", false).expect("split");
    assert_eq!(
        split.iter().map(|(_, frames)| *frames).collect::<Vec<_>>(),
        vec![2, 3, 1]
    );
    let prefix: Vec<u8> = synthetic_h264(6)[..15].to_vec(); // SPS + PPS
    // Every file opens with the headers, so each decodes standalone.
    for task in &tasks {
        let bytes = fs::read(&task.output).expect("part read");
        assert!(
            bytes.starts_with(&prefix),
            "{} lacks the stream headers",
            task.output.display()
        );
    }
    // Stripping the repeated headers reconstructs the original stream.
    let mut reassembled = fs::read(&tasks[0].output).expect("part 0");
    for task in tasks.iter().skip(1) {
        reassembled.extend_from_slice(&fs::read(&task.output).expect("part")[prefix.len()..]);
    }
    assert_eq!(reassembled, fs::read(&whole).expect("whole"));
    let _ = fs::remove_file(&whole);
}

fn check_zone_part_validation() {
    let directory = tempfile::tempdir().expect("zone fixture");
    let whole = directory.path().join("whole.264");
    fs::write(&whole, synthetic_h264(3)).expect("structurally splittable but undecodable");
    let task = Task {
        original_index: 7,
        index: 0,
        frame_indices: vec![0, 1, 2],
        sub_scenes: None,
        encoder: x264(20.0),
        output: directory.path().join("scene.264"),
    };
    let result = split_annexb_cancel(
        &whole,
        Codec::H264,
        std::slice::from_ref(&task),
        "264",
        &Cancellation::default(),
        true,
    );
    assert!(
        result.is_err(),
        "NAL starts alone do not establish decodability"
    );
    assert!(!task.output.exists(), "invalid zone part is not published");
    assert_eq!(
        fs::read_dir(directory.path()).expect("cleanup").count(),
        1,
        "only caller-owned whole file remains"
    );
}

// File formatting, splitter outputs and native payloads require private
// ZonePlan/split_ivf/split_annexb seams; public encode cases stay separate.
mod internal_integration {
    use super::*;

    #[test]
    fn zone_options_are_written_next_to_the_scene_files() {
        check_zone_option_files();
    }

    #[test]
    fn x264_zone_stream_splits_into_standalone_parts_with_bounded_rate_control_carryover() {
        check_x264_zone_stream_split();
    }

    #[test]
    fn x265_zone_stream_splits_into_standalone_parts_with_bounded_rate_control_carryover() {
        check_x265_zone_stream_split();
    }

    #[test]
    fn a_real_svt_av1_zone_stream_forces_a_keyframe_at_every_zone_start() {
        check_svt_zone_keyframes();
    }

    #[test]
    fn ivf_output_splits_into_per_scene_files() {
        check_ivf_scene_files();
    }

    #[test]
    fn annexb_output_splits_with_headers_in_every_file() {
        check_annexb_scene_headers();
    }

    #[test]
    fn zone_parts_require_decodable_frames_before_publication() {
        check_zone_part_validation();
    }
}
