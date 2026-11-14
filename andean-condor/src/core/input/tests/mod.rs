mod frame_reader;

use av_decoders::Rational32;

use super::*;
use crate::{
    core::input::{
        ffms2_filter::{ChromaSampling, ffms2_pixel_format},
        pixel_format::PixelFormat,
    },
    ffmpeg::FFPixelFormat,
    vapoursynth::plugins::resize::Scaler,
};

/// `VideoDetails::default` only exists under av-decoders' own `cfg(test)`.
fn video_details(
    width: usize,
    height: usize,
    bit_depth: usize,
    chroma_sampling: ChromaSubsampling,
) -> VideoDetails {
    VideoDetails {
        width,
        height,
        bit_depth,
        chroma_sampling,
        frame_rate: Rational32::new(24, 1),
        total_frames: Some(100),
        color_range: None,
        color_primaries: None,
        color_transfer_characteristic: None,
        color_space: None,
        chroma_location: None,
        mastering_display: None,
        content_light_level: None,
    }
}

/// A mismatch here hands the encoder mis-parsed frames, since `y4m_frames`
/// picks the sample type from `ClipInfo`.
#[test]
fn chroma_tag_agrees_with_clip_info_bit_depth() {
    for (bit_depth, chroma) in [
        (8, ChromaSubsampling::Yuv420),
        (10, ChromaSubsampling::Yuv420),
        (12, ChromaSubsampling::Yuv420),
        (8, ChromaSubsampling::Yuv444),
        (10, ChromaSubsampling::Yuv444),
        (8, ChromaSubsampling::Monochrome),
        (10, ChromaSubsampling::Monochrome),
    ] {
        let tag = Input::y4m_chroma_tag(bit_depth, chroma);
        let format =
            ffms2_pixel_format(bit_depth, chroma).expect("FFMS2 should support this format");
        let info = PixelFormat::FFmpeg { format };

        assert_eq!(
            info.as_bit_depth().expect("bit depth should resolve"),
            bit_depth,
            "tag {tag} reports a different depth than ClipInfo"
        );
    }
}

#[test]
fn chroma_tag_matches_y4m_convention() {
    for (chroma, tags) in [
        (
            ChromaSubsampling::Monochrome,
            ["mono", "mono10", "mono12", "mono16"],
        ),
        (
            ChromaSubsampling::Yuv420,
            ["420", "420p10", "420p12", "420p16"],
        ),
        (
            ChromaSubsampling::Yuv422,
            ["422", "422p10", "422p12", "422p16"],
        ),
        (
            ChromaSubsampling::Yuv444,
            ["444", "444p10", "444p12", "444p16"],
        ),
    ] {
        for (bit_depth, tag) in [8, 10, 12, 16].into_iter().zip(tags) {
            assert_eq!(Input::y4m_chroma_tag(bit_depth, chroma), tag);
        }
    }
}

#[test]
fn y4m_header_length_is_one_extension_token() {
    let mut input = Input::from_data(&InputModel::VapourSynthScript {
        source: VapourSynthScriptSource::Text(
            "import vapoursynth as vs\n\
             vs.core.std.BlankClip(width=18, height=8, length=1, format=vs.YUV420P10, \
             fpsnum=30000, fpsden=1001).set_output()"
                .to_owned(),
        ),
        variables: HashMap::new(),
        index: 0,
        filters: Vec::new(),
        stream_concurrently: true,
    })
    .expect("y4m-length requires the installed native VapourSynth runtime");

    for (frames, extension) in [
        (None, None),
        (Some(0), Some("XLENGTH=0")),
        (Some(42), Some("XLENGTH=42")),
        (Some(4_294_967_295), Some("XLENGTH=4294967295")),
    ] {
        let header = input.y4m_header(frames).expect("actual decoder header");
        let tokens: Vec<_> = header.split_ascii_whitespace().collect();
        let mut expected = vec!["YUV4MPEG2", "C420p10", "W18", "H8", "F30000:1001", "Ip", "A0:0"];
        if let Some(extension) = extension {
            expected.push(extension);
        }
        assert_eq!(tokens, expected, "frames={frames:?}, header={header:?}");
        assert_eq!(
            tokens.iter().filter(|token| token.starts_with("XLENGTH")).count(),
            usize::from(frames.is_some()),
            "exactly one length extension when supplied"
        );
        assert!(header.ends_with('\n'));
        assert_eq!(header.lines().count(), 1);
    }
}

fn gray_roundtrip(bit_depth: usize, colors: [u16; 2], tag: &str, pixel_format: &str) {
    use std::{
        fs::{self, File},
        process::{Command, Stdio},
        thread,
        time::{Duration, Instant},
    };

    let mut input = Input::from_data(&InputModel::VapourSynthScript {
        source: VapourSynthScriptSource::Text(format!(
            "import vapoursynth as vs\nc = vs.core\n\
             a = c.std.BlankClip(width=18, height=3, length=1, format=vs.GRAY{bit_depth}, color=[{}])\n\
             b = c.std.BlankClip(a, color=[{}])\n(a + b).set_output()",
            colors[0], colors[1]
        )),
        variables: HashMap::new(),
        index: 0,
        filters: Vec::new(),
        stream_concurrently: true,
    })
    .expect("grayscale requires the installed native VapourSynth runtime");
    let details = input.decoder().get_video_details();
    assert_eq!(details.bit_depth, bit_depth);
    assert_eq!(details.chroma_sampling, ChromaSubsampling::Monochrome);
    // I10 owns XLENGTH syntax; use the actual production header without that extension.
    let header = input.y4m_header(None).expect("actual decoder header");
    let mut stream = header.as_bytes().to_vec();
    for index in 0..2 {
        stream.extend(input.y4m_frame(index).expect("actual gray frame").into_inner());
    }

    let temp = tempfile::tempdir().expect("roundtrip directory");
    let source = temp.path().join("input.y4m");
    let decoded = temp.path().join("decoded.raw");
    let log = temp.path().join("ffmpeg.log");
    fs::write(&source, stream).expect("write actual header and production frames");
    // File-backed I/O cannot block on full pipes; bound and reap the child as well.
    let mut child = Command::new("ffmpeg")
        .args(["-nostdin", "-v", "error", "-xerror", "-i"])
        .arg(&source)
        .args(["-map", "0:v:0", "-c:v", "rawvideo", "-pix_fmt", pixel_format, "-f", "rawvideo"])
        .arg(&decoded)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(File::create(&log).expect("ffmpeg log"))
        .spawn()
        .expect("grayscale requires ffmpeg on PATH");
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll ffmpeg") {
            break status;
        }
        if start.elapsed() > Duration::from_secs(30) {
            child.kill().expect("kill timed-out ffmpeg");
            child.wait().expect("reap timed-out ffmpeg");
            panic!("GRAY{bit_depth}: ffmpeg timed out");
        }
        thread::sleep(Duration::from_millis(10));
    };
    assert!(
        status.success(),
        "GRAY{bit_depth}, {header:?}: {}",
        fs::read_to_string(&log).expect("read ffmpeg log")
    );
    assert_eq!(header.split_ascii_whitespace().nth(1), Some(tag));
    let actual = fs::read(decoded).expect("decoded raw frames");
    let expected: Vec<u8> = colors
        .into_iter()
        .flat_map(|color| {
            if bit_depth == 8 {
                vec![u8::try_from(color).expect("8-bit sample"); 54]
            } else {
                color.to_le_bytes().repeat(54)
            }
        })
        .collect();
    assert_eq!(
        actual.len(),
        2 * 18 * 3 * if bit_depth == 8 { 1 } else { 2 },
        "exactly two complete frames"
    );
    assert_eq!(
        actual, expected,
        "GRAY{bit_depth}: exact sample values and frame alignment"
    );
}

#[test]
fn gray8_ffmpeg_roundtrip() {
    gray_roundtrip(8, [37, 219], "Cmono", "gray");
}

#[test]
fn gray10_ffmpeg_roundtrip() {
    gray_roundtrip(10, [0x0123, 0x03ab], "Cmono10", "gray10le");
}

#[test]
fn gray12_ffmpeg_roundtrip() {
    gray_roundtrip(12, [0x0234, 0x0bcd], "Cmono12", "gray12le");
}

#[test]
fn gray16_ffmpeg_roundtrip() {
    gray_roundtrip(16, [0x1234, 0xabcd], "Cmono16", "gray16le");
}

#[test]
fn unsupported_output_format_is_rejected() {
    // FFMS2 has no 16-bit output, so the mapping must not invent one.
    assert!(ffms2_pixel_format(16, ChromaSubsampling::Yuv420).is_none());
    assert!(ffms2_pixel_format(10, ChromaSubsampling::Yuv420).is_some());
}

#[test]
fn baseline_conversion_reports_unsupported_format() {
    let baseline = video_details(32, 16, 16, ChromaSubsampling::Yuv422);
    let filter = Ffms2Filter::OutputFormat {
        bit_depth: None,
        chroma: None,
        width: Some(16),
        height: None,
    };
    let error = filter
        .to_vapoursynth_filters(&baseline)
        .expect_err("unsupported native depth cannot become format=None");
    assert!(error.to_string().contains("16-bit"));
}

/// A native filter must become a VapourSynth resize, so a VS-only path sees
/// the same converted clip the native path does.
#[test]
fn native_filter_becomes_vapoursynth_resize() {
    let baseline = video_details(1920, 1080, 8, ChromaSubsampling::Yuv420);

    let ten_bit = Ffms2Filter::OutputFormat {
        bit_depth: Some(10),
        chroma: None,
        width: None,
        height: None,
    };
    assert_eq!(
        ten_bit.to_vapoursynth_filters(&baseline).expect("format conversion"),
        vec![VapourSynthFilter::Resize {
            scaler: Some(Scaler::Bicubic),
            width: None,
            height: None,
            format: Some(FFPixelFormat::YUV420P10LE),
        }]
    );

    let downscale = Ffms2Filter::OutputFormat {
        bit_depth: None,
        chroma: None,
        width: Some(1280),
        height: Some(720),
    };
    assert_eq!(
        downscale.to_vapoursynth_filters(&baseline).expect("format conversion"),
        vec![VapourSynthFilter::Resize {
            scaler: Some(Scaler::Bicubic),
            width: Some(1280),
            height: Some(720),
            format: None,
        }]
    );
}

/// A filter matching the decoded stream is a no-op and must not add work.
#[test]
fn native_filter_matching_baseline_is_dropped() {
    let baseline = video_details(1920, 1080, 10, ChromaSubsampling::Yuv420);

    let noop = Ffms2Filter::OutputFormat {
        bit_depth: Some(10),
        chroma: Some(ChromaSampling::Yuv420),
        width: Some(1920),
        height: Some(1080),
    };
    assert!(noop.to_vapoursynth_filters(&baseline).expect("format conversion").is_empty());

    let empty = Ffms2Filter::OutputFormat {
        bit_depth: None,
        chroma: None,
        width: None,
        height: None,
    };
    assert!(empty.to_vapoursynth_filters(&baseline).expect("format conversion").is_empty());
}

/// `set_output_format` rewrites the decoder's details, so the filtered
/// format equals each filter's resolved result. Resolving against it
/// would make every filter a no-op and drop them from the VapourSynth
/// script.
#[test]
fn filters_resolve_against_the_source_not_the_filtered_format() {
    let source = video_details(1920, 1080, 8, ChromaSubsampling::Yuv420);

    let filters = [
        Ffms2Filter::OutputFormat {
            bit_depth: Some(10),
            chroma: Some(ChromaSampling::Yuv444),
            width: None,
            height: None,
        },
        // Unset fields inherit from the previous filter, as they do when the
        // decoder applies them in sequence.
        Ffms2Filter::OutputFormat {
            bit_depth: None,
            chroma: None,
            width: Some(1280),
            height: Some(720),
        },
    ];

    // Stand in for the decoder's details after the filters were applied.
    let mut current = source;
    for filter in &filters {
        let Ffms2Filter::OutputFormat {
            bit_depth,
            chroma,
            width,
            height,
        } = filter;
        let bit_depth = bit_depth.map_or(current.bit_depth, usize::from);
        let chroma = chroma.map_or(current.chroma_sampling, |chroma| {
            chroma.to_chroma_subsampling()
        });
        current = VideoDetails {
            width: width.map_or(current.width, |width| width as usize),
            height: height.map_or(current.height, |height| height as usize),
            bit_depth,
            chroma_sampling: chroma,
            ..current
        };
    }
    assert_eq!(
        (
            current.width,
            current.height,
            current.bit_depth,
            current.chroma_sampling
        ),
        (1280, 720, 10, ChromaSubsampling::Yuv444)
    );

    // Against the filtered format every filter looks like a no-op...
    for filter in &filters {
        assert!(
            filter.to_vapoursynth_filters(&current).expect("format conversion").is_empty(),
            "a filter should not be dropped just because the decoder already applied it"
        );
    }

    // ...whereas against the source it resolves to a real conversion.
    let converted = filters
        .iter()
        .flat_map(|filter| filter.to_vapoursynth_filters(&source).expect("format conversion"))
        .collect::<Vec<_>>();
    assert_eq!(
        converted,
        vec![
            VapourSynthFilter::Resize {
                scaler: Some(Scaler::Bicubic),
                width: None,
                height: None,
                format: Some(FFPixelFormat::YUV444P10LE),
            },
            VapourSynthFilter::Resize {
                scaler: Some(Scaler::Bicubic),
                width: Some(1280),
                height: Some(720),
                format: None,
            },
        ]
    );
}
