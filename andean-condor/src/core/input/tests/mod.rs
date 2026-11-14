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
fn ffms2_pixel_format_preserves_requested_bit_depth() {
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
fn native_yuv420_format_mapping_rejects_sixteen_bit_output() {
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
