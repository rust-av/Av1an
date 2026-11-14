//! Format translation uses each filter's actual input, not guessed 420/8.

use std::{fs, process::Command};

use super::*;
use andean_condor::{
    core::input::Ffms2Filter, ffmpeg::FFPixelFormat, models::input::VapourSynthImportMethod,
    vapoursynth::vapoursynth_filters::VapourSynthFilter,
};

fn check_formats(
    source_format: &str,
    source_depth: u8,
    source_chroma: &str,
    filters: Vec<Ffms2Filter>,
    depth: u8,
    chroma: &str,
    width: usize,
) {
    let directory = tempfile::tempdir().expect("format fixture");
    let raw = directory.path().join("source.raw");
    let source = directory.path().join("source.mkv");
    let cache = directory.path().join("source.ffindex");
    let image = |bits: u8, layout: &str, width: usize| {
        let chroma_size = match layout {
            "420" => width * 4,
            "422" => width * 8,
            "444" => width * 16,
            "mono" => 0,
            _ => panic!("layout"),
        };
        [(width * 16, 64_u16), (chroma_size, 96), (chroma_size, 160)]
            .into_iter()
            .flat_map(|(count, sample)| {
                // Full-range gray maps round(64 * 4095 / 255) to 1028, not 64 << 4.
                let sample = if layout == "mono" && source_depth == 8 && bits == 12 {
                    1028
                } else {
                    sample << (bits - 8)
                };
                let bytes = if bits == 8 {
                    vec![sample as u8]
                } else {
                    sample.to_le_bytes().to_vec()
                };
                bytes.repeat(count)
            })
            .collect::<Vec<_>>()
    };
    fs::write(&raw, image(source_depth, source_chroma, 32)).expect("raw planes");
    let output = Command::new("timeout")
        .args([
            "30s",
            "ffmpeg",
            "-nostdin",
            "-v",
            "error",
            "-y",
            "-f",
            "rawvideo",
            "-pixel_format",
            source_format,
            "-video_size",
            "32x16",
            "-framerate",
            "24",
            "-i",
        ])
        .arg(&raw)
        .args(["-frames:v", "1", "-c:v", "ffv1", "-threads", "1"])
        .arg(&source)
        .output()
        .expect("ffmpeg and timeout are required");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let model = InputModel::Video {
        path: source.clone(),
        import_method: ImportMethod::FFMS2 {
            index: Some(0),
            cache_path: Some(cache.clone()),
        },
        filters,
    };
    let mut vs_model = InputModel::VapourSynth {
        path: source,
        import_method: VapourSynthImportMethod::FFMS2 { index: Some(0) },
        cache_path: Some(cache),
        filters: vec![],
    };
    vs_model.adopt_filters(&model).expect("model copy");
    let mut native = Input::from_data(&model).expect("native format conversion");
    let mut reference = native
        .as_vapoursynth_script()
        .expect("metric reference")
        .expect("native reference");
    let mut direct = Input::from_data(&vs_model).expect("direct node conversion");
    let mut scripted =
        Input::from_vapoursynth_scripted(&vs_model).expect("generated script conversion");
    for (route, input) in [
        ("native", &mut native),
        ("reference", &mut reference),
        ("direct", &mut direct),
        ("scripted", &mut scripted),
    ] {
        let info = input.clip_info().expect("metadata");
        assert_eq!(info.resolution, (width as u32, 16), "{route}");
        assert_eq!(
            info.format_info.as_bit_depth().expect("depth"),
            usize::from(depth),
            "{route}"
        );
        let tag = if depth == 8 {
            format!("C{chroma}")
        } else if chroma == "mono" {
            format!("Cmono{depth}")
        } else {
            format!("C{chroma}p{depth}")
        };
        assert!(
            input.y4m_header(None).expect("Y4M header").contains(&tag),
            "{route}: {tag}"
        );
        assert_eq!(
            input.y4m_frame(0).expect("pixels").into_inner(),
            [b"FRAME\n".to_vec(), image(depth, chroma, width)].concat(),
            "{route}: exact plane lengths, LE samples and precision"
        );
    }
}

fn output(arguments: &str) -> Ffms2Filter {
    format!("output-format:{arguments}").parse().expect("native filter")
}

#[test]
fn explicit_eight_bit_conversion_preserves_metadata_and_exact_samples() {
    let filter = Ffms2Filter::from_vapoursynth_filter(&VapourSynthFilter::Resize {
        scaler: None,
        width: None,
        height: None,
        format: Some(FFPixelFormat::YUV420P),
    })
    .expect("valid conversion")
    .expect("resize");
    check_formats("yuv420p10le", 10, "420", vec![filter], 8, "420", 32);
}

#[test]
fn depth_only_conversion_preserves_yuv444_chroma() {
    check_formats(
        "yuv444p",
        8,
        "444",
        vec![output("bit_depth=10;")],
        10,
        "444",
        32,
    );
}

#[test]
fn resolution_only_retains_monochrome_twelve_bits() {
    check_formats(
        "gray12le",
        12,
        "mono",
        vec![output("width=16;")],
        12,
        "mono",
        16,
    );
}

#[test]
fn depth_only_retains_monochrome() {
    check_formats(
        "gray",
        8,
        "mono",
        vec![output("bit_depth=12;")],
        12,
        "mono",
        32,
    );
}

#[test]
fn resolution_only_retains_monochrome_ten_bits() {
    check_formats(
        "gray10le",
        10,
        "mono",
        vec![output("width=16;")],
        10,
        "mono",
        16,
    );
}

#[test]
fn depth_only_conversion_preserves_yuv422_chroma() {
    check_formats(
        "yuv422p10le",
        10,
        "422",
        vec![output("bit_depth=12;")],
        12,
        "422",
        32,
    );
}

#[test]
fn resolution_only_conversion_preserves_ten_bit_yuv422() {
    check_formats(
        "yuv422p10le",
        10,
        "422",
        vec![output("width=16;")],
        10,
        "422",
        16,
    );
}

#[test]
fn chroma_only_retains_ten_bits() {
    check_formats(
        "yuv422p10le",
        10,
        "422",
        vec![output("chroma=444;")],
        10,
        "444",
        32,
    );
}

#[test]
fn chained_resolution_uses_previous_node() {
    check_formats(
        "yuv420p",
        8,
        "420",
        vec![output("chroma=444;"), output("bit_depth=10;")],
        10,
        "444",
        32,
    );
}

#[test]
fn direct_inputs_reject_unsupported_deferred_output_formats() {
    for (preset, bit_depth, width, height, message) in [
        (
            "RGB24",
            Some(10),
            None,
            None,
            "Unsupported FFMS2 source format",
        ),
        (
            "YUV440P8",
            None,
            Some(16),
            None,
            "Unsupported FFMS2 source format",
        ),
        (
            "YUV420PS",
            Some(10),
            None,
            None,
            "Unsupported FFMS2 source format",
        ),
        (
            "YUV420P16",
            Some(10),
            None,
            None,
            "Unsupported FFMS2 source format",
        ),
        ("YUV420P8", Some(16), None, None, "Unsupported bit depth"),
        (
            "YUV420P8",
            None,
            Some(0),
            None,
            "must be positive and fit i32",
        ),
        (
            "YUV420P8",
            None,
            None,
            Some(u32::MAX),
            "must be positive and fit i32",
        ),
    ] {
        let native = Ffms2Filter::OutputFormat {
            bit_depth,
            chroma: None,
            width,
            height,
        };
        let filter = native.to_vapoursynth().remove(0);
        let source = format!(
            "from vapoursynth import core, {preset}\nclip = core.std.BlankClip(width=32, height=16, format={preset})\n"
        );
        let data = InputModel::VapourSynthScript {
            source: VapourSynthScriptSource::Text(format!("{source}clip.set_output()\n")),
            variables: HashMap::new(),
            index: 0,
            filters: vec![filter],
            stream_concurrently: true,
        };
        let error = Input::from_data(&data).err().expect("direct runtime error").to_string();
        assert!(error.contains(message), "direct {preset}: {error}");
    }
}
