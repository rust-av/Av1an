mod aom;
mod avm;
mod cli;
#[cfg(target_os = "linux")]
mod diagnostics;
#[cfg(unix)]
mod executable_resolution;
mod photon_noise;
#[cfg(target_os = "linux")]
pub(crate) mod process_lifecycle;
#[cfg(target_os = "linux")]
mod public_encoding;
mod rav1e;
mod statistics;
pub(super) mod support;
mod svt_av1;
mod vpx;

use av_decoders::{ColorPrimaries, ColorSpace, TransferCharacteristic};

use super::*;

#[cfg(unix)]
#[test]
fn public_preflight_before_spawn_or_receive() {
    use std::{os::unix::fs::PermissionsExt, time::Duration};
    let temp = tempfile::tempdir().expect("fixture");
    let fake = temp.path().join("encoder");
    std::fs::write(
        &fake,
        "#!/bin/sh\nprintf spawned > \"$ENCODER_PREFLIGHT_DIR/spawned\"\nexit 42\n",
    )
    .expect("fake");
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).expect("executable");
    std::fs::write(temp.path().join("blank.vpy"), "from vapoursynth import core, YUV420P8\ncore.std.BlankClip(width=64,height=64,length=1,format=YUV420P8).set_output()\n").expect("script");
    for route in [
        "input",
        "input-pass",
        "stream",
        "stream-pass",
        "tuple-input",
        "tuple-stream",
        "vvenc-stream",
    ] {
        let mut child = Command::new(std::env::current_exe().expect("test binary"))
            .args(["--exact", "core::encoder::tests::preflight_child", "--nocapture"])
            .env("ENCODER_PREFLIGHT_DIR", temp.path())
            .env("ENCODER_PREFLIGHT_ROUTE", route)
            .spawn()
            .expect("watchdog child");
        let start = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait().expect("wait") {
                break status;
            }
            if start.elapsed() > Duration::from_secs(5) {
                child.kill().expect("kill blocked child");
                child.wait().expect("reap blocked child");
                panic!("{route} blocked before validation");
            }
            thread::sleep(Duration::from_millis(10));
        };
        assert!(
            status.success()
                && std::fs::read_to_string(temp.path().join("preflight-selected"))
                    .is_ok_and(|selected| selected == route),
            "{route}: child failed or was not selected"
        );
        assert!(
            !temp.path().join("spawned").exists(),
            "{route}: encoder spawned"
        );
        assert!(
            !temp.path().join("out.ivf").exists(),
            "{route}: output created"
        );
    }
}

#[cfg(unix)]
#[test]
fn preflight_child() {
    let Some(dir) = std::env::var_os("ENCODER_PREFLIGHT_DIR") else {
        return;
    };
    let dir = PathBuf::from(dir);
    let route = std::env::var("ENCODER_PREFLIGHT_ROUTE").expect("route");
    std::fs::write(dir.join("preflight-selected"), &route).expect("selection handshake");
    let mut encoder = Encoder::default_from_base(
        &if route == "vvenc-stream" {
            EncoderBase::VVenC
        } else {
            EncoderBase::AOM
        },
        false,
    );
    encoder.set_executable(Some(dir.join("encoder")));
    let pass = if route.starts_with("tuple") {
        (3, 3)
    } else {
        (1, 1)
    };
    *encoder.passes_mut().expect("passes") =
        EncoderPasses::All(if route.starts_with("tuple") { 1 } else { 0 });
    if route == "vvenc-stream" {
        *encoder.passes_mut().expect("passes") = EncoderPasses::All(2);
        encoder.parameters_mut().extend(CLIParameter::new_numbers(
            "--",
            " ",
            &[("passes", 1.0), ("bitrate", 1000.0)],
        ));
    }
    let (tx, _rx) = mpsc::channel();
    let output = dir.join("out.ivf");
    let result = if route.contains("input") {
        let model = crate::models::input::Input::VapourSynthScript {
            source: crate::models::input::VapourSynthScriptSource::Path(dir.join("blank.vpy")),
            variables: HashMap::new(),
            index: 0,
            filters: vec![],
            stream_concurrently: false,
        };
        let mut input = Input::from_data(&model).expect("blank input");
        if route == "input" {
            encoder.encode_with_input(&mut input, &output, 0, 1, tx)
        } else {
            encoder.encode_pass_with_input(&mut input, &output, 0, 1, pass, tx, None)
        }
    } else {
        let (_sender, receiver) = crossbeam_channel::unbounded();
        // Keep the sender alive: a misplaced receive must trip the outer watchdog.
        let result = if route == "stream" || route == "vvenc-stream" {
            encoder.encode_with_stream(receiver, &output, tx, &clip_info())
        } else {
            encoder.encode_pass_with_stream(pass, receiver, &output, tx, &clip_info(), None)
        };
        drop(_sender);
        result
    };
    let error = result.expect_err("invalid settings must fail before encoding");
    if route == "vvenc-stream" {
        use crate::models::encoder::EncoderSettingsError;
        assert!(
            matches!(error.downcast_ref::<EncoderSettingsError>(), Some(EncoderSettingsError::UnsupportedMode(EncoderBase::VVenC, reason)) if reason.contains("frontend")),
            "{error:#}"
        );
        let error = encoder
            .parameters_with_output((1, 2), &output, &clip_info())
            .expect_err("first-pass argv must reject the whole-job conflict");
        assert!(
            matches!(
                error.downcast_ref::<EncoderSettingsError>(),
                Some(EncoderSettingsError::UnsupportedMode(EncoderBase::VVenC, _))
            ),
            "{error:#}"
        );
        return;
    }
    assert!(error.to_string().contains("pass"), "{error:#}");
}

pub(super) fn clip_info() -> ClipInfo {
    ClipInfo {
        num_frames: 24,
        format_info: crate::core::input::pixel_format::PixelFormat::VapourSynth { bit_depth: 10 },
        frame_rate: av_format::rational::Rational64::new(24, 1),
        resolution: (1920, 1080),
        color_range: None,
        color_primaries: Some(ColorPrimaries::BT2020),
        color_transfer_characteristic: Some(TransferCharacteristic::SMPTE2084),
        color_space: Some(ColorSpace::BT2020NCL),
        chroma_location: None,
        mastering_display: None,
        content_light_level: None,
    }
}

fn parameters(encoder: &Encoder) -> Vec<String> {
    encoder
        .parameters_with_output((1, 1), &PathBuf::from("output.ivf"), &clip_info())
        .expect("valid settings")
}

/// The value of `flag` in an argv vector built with a separate-value
/// delimiter.
fn value_of<'a>(parameters: &'a [String], flag: &str) -> Option<&'a str> {
    parameters
        .iter()
        .position(|argument| argument == flag)
        .and_then(|index| parameters.get(index + 1))
        .map(String::as_str)
}

#[test]
fn vvenc_default_parameters_inherit_y4m() {
    let expected = HashMap::from([(
        "y4m".to_owned(),
        CLIParameter::Bool {
            prefix: "--".to_owned(),
            value: true,
        },
    )]);
    assert_eq!(EncoderBase::VVenC.default_parameters(), expected);
    let encoder = Encoder::default_from_base(&EncoderBase::VVenC, false);
    assert_eq!(encoder.parameters(), &expected);
}

#[test]
fn vvenc_default_one_pass_argv_declares_y4m_pipe() {
    let output = "job space 映像/scene0.vvc";
    let mut encoder = Encoder::default_from_base(&EncoderBase::VVenC, false);
    encoder.set_append_color_data(false);
    let argv = encoder
        .parameters_with_output((1, 1), Path::new(output), &clip_info())
        .expect("argv");
    // HashMap option order can vary; input/output pairs must remain intact.
    assert!(
        argv == ["-i", "-", "--y4m", "-o", output] || argv == ["-i", "-", "-o", output, "--y4m"],
        "{argv:?}"
    );
}

#[test]
fn vvenc_disabled_or_excluded_y4m_default_is_not_reinserted() {
    let output = "job space 映像/scene0.vvc";
    for exclude_defaults in [false, true] {
        let mut encoder = Encoder::default_from_base(&EncoderBase::VVenC, exclude_defaults);
        encoder.set_append_color_data(false);
        if !exclude_defaults {
            encoder
                .parameters_mut()
                .insert("y4m".to_owned(), CLIParameter::new_bool("--", false));
        }
        assert_eq!(
            encoder
                .parameters_with_output((1, 1), Path::new(output), &clip_info())
                .expect("argv"),
            ["-i", "-", "-o", output],
            "exclude_defaults={exclude_defaults}"
        );
    }
}

#[test]
fn svt_av1_default_argv_includes_source_colour_metadata() {
    let encoder = Encoder::default_from_base(&EncoderBase::SVTAV1, true);
    let parameters = parameters(&encoder);

    assert_eq!(value_of(&parameters, "--color-primaries"), Some("bt2020"));
    assert_eq!(
        value_of(&parameters, "--transfer-characteristics"),
        Some("smpte2084")
    );
    assert_eq!(
        value_of(&parameters, "--matrix-coefficients"),
        Some("bt2020-ncl")
    );
}

#[test]
fn disabling_colour_data_appends_nothing() {
    let mut encoder = Encoder::default_from_base(&EncoderBase::SVTAV1, true);
    encoder.set_append_color_data(false);
    let parameters = parameters(&encoder);

    assert_eq!(value_of(&parameters, "--color-primaries"), None);
    assert_eq!(value_of(&parameters, "--transfer-characteristics"), None);
    assert_eq!(value_of(&parameters, "--matrix-coefficients"), None);
}

#[test]
fn a_user_option_overrides_the_appended_colour_data() {
    let mut encoder = Encoder::default_from_base(&EncoderBase::SVTAV1, true);
    encoder.parameters_mut().insert(
        "color-primaries".to_owned(),
        CLIParameter::new_string("--", " ", "bt709"),
    );

    let parameters = parameters(&encoder);
    assert_eq!(value_of(&parameters, "--color-primaries"), Some("bt709"));
    assert!(
        !parameters.iter().any(|argument| argument == "bt2020"),
        "the appended value must be replaced rather than duplicated"
    );
}

#[test]
fn a_user_option_overrides_the_appended_hdr_metadata() {
    let mut encoder = Encoder::default_from_base(&EncoderBase::X265, true);
    encoder.parameters_mut().insert(
        "max-cll".to_owned(),
        CLIParameter::new_string("--", " ", "400,200"),
    );
    let clip_info = ClipInfo {
        content_light_level: Some(av_decoders::ContentLightLevel {
            max_cll: 1000,
            max_fall: 400,
        }),
        ..clip_info()
    };

    let parameters = encoder
        .parameters_with_output((1, 1), &PathBuf::from("output.hevc"), &clip_info)
        .expect("valid settings");
    assert_eq!(value_of(&parameters, "--max-cll"), Some("400,200"));
}

#[test]
fn svt_av1_multipass_argv_keeps_pass_and_colour_options() {
    let mut encoder = Encoder::default_from_base(&EncoderBase::SVTAV1, true);
    encoder
        .passes_mut()
        .expect("svt-av1 takes passes")
        .clone_from(&EncoderPasses::All(2));
    encoder
        .parameters_mut()
        .insert("rc".into(), CLIParameter::new_number("--", " ", 1.0));

    let parameters = encoder
        .parameters_with_output((1, 2), &PathBuf::from("output.ivf"), &clip_info())
        .expect("VBR two-pass");

    // A two-pass SVT-AV1 command carries `--pass 1`; the colour data is
    // merged after it, and both are present.
    assert_eq!(value_of(&parameters, "--pass"), Some("1"));
    assert_eq!(value_of(&parameters, "--color-primaries"), Some("bt2020"));
}

#[test]
fn ffmpeg_colour_data_precedes_the_input() {
    let encoder = Encoder::default_from_base(&EncoderBase::FFmpeg, true);
    let parameters = encoder
        .parameters_with_output(
            (1, 1),
            &PathBuf::from("output.mkv"),
            &ClipInfo {
                chroma_location: Some(av_decoders::ChromaLocation::Left),
                ..clip_info()
            },
        )
        .expect("valid settings");
    let input = parameters
        .iter()
        .position(|argument| argument == "-i")
        .expect("ffmpeg reads the Y4M stream from stdin");

    // `fftools/ffmpeg_enc.c` copies the colour properties out of every
    // frame, so these only survive as input options.
    for flag in ["-color_primaries", "-color_trc", "-colorspace", "-chroma_sample_location"] {
        let index = parameters
            .iter()
            .position(|argument| argument == flag)
            .unwrap_or_else(|| panic!("{flag} is missing"));
        assert!(index < input, "{flag} must precede -i");
    }
    assert_eq!(value_of(&parameters, "-color_primaries"), Some("bt2020"));
    assert_eq!(value_of(&parameters, "-color_trc"), Some("smpte2084"));
    assert_eq!(value_of(&parameters, "-colorspace"), Some("bt2020nc"));
}

#[test]
fn a_user_option_overrides_the_appended_ffmpeg_colour_data() {
    let mut encoder = Encoder::default_from_base(&EncoderBase::FFmpeg, true);
    encoder.parameters_mut().insert(
        "color_primaries".to_owned(),
        CLIParameter::new_string("-", " ", "bt709"),
    );

    let parameters = parameters(&encoder);
    assert_eq!(value_of(&parameters, "-color_primaries"), Some("bt709"));
    assert!(
        !parameters.iter().any(|argument| argument == "bt2020"),
        "the appended value must be replaced rather than duplicated"
    );
}
