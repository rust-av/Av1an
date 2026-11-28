mod aom;
mod avm;
mod photon_noise;
#[cfg(target_os = "linux")]
pub(crate) mod process_lifecycle;
mod rav1e;
mod statistics;
#[cfg(unix)]
pub(super) mod support;
mod svt_av1;
mod vpx;

use av_decoders::{ColorPrimaries, ColorSpace, TransferCharacteristic};

use super::*;

#[test]
fn version_lines_without_the_expected_separator_do_not_panic() {
    assert_eq!(aom_style_version("    av1"), "av1");
    assert_eq!(aom_style_version("    av1    - 3.12.0 (default)"), "3.12.0");
    assert_eq!(after_first_space("x264"), "x264");
    assert_eq!(after_first_space("x264 - core 164"), "- core 164");
    assert_eq!(after_first_colon("x265"), "x265");
    assert_eq!(after_first_colon("x265 4.1+1: build"), "build");
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
        .parameters_with_output((1, 1), &PathBuf::from("output.ivf"), &clip_info(), None)
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
        .parameters_with_output((1, 1), Path::new(output), &clip_info(), None)
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
                .parameters_with_output((1, 1), Path::new(output), &clip_info(), None)
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
        .parameters_with_output((1, 1), &PathBuf::from("output.hevc"), &clip_info, None)
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
        .parameters_with_output((1, 2), &PathBuf::from("output.ivf"), &clip_info(), None)
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
            None,
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
