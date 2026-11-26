//! Public native scorer contracts; optional real-library gates are retained.
#![cfg(test)]

use av_metrics_fmetrics::{
    ColorInfo, FmetricsConfig, FmetricsError, FmetricsMetric, FmetricsScorer, FrameScore, PlaneSet,
    RgbView,
};
use v_frame::chroma::ChromaSubsampling;

fn color(bit_depth: usize) -> ColorInfo {
    ColorInfo::from_details(&av_decoders::VideoDetails {
        width: 64,
        height: 64,
        bit_depth,
        chroma_sampling: ChromaSubsampling::Yuv420,
        frame_rate: av_decoders::Rational32::new(24, 1),
        total_frames: None,
        color_range: None,
        color_primaries: None,
        color_transfer_characteristic: None,
        color_space: None,
        chroma_location: None,
        mastering_display: None,
        content_light_level: None,
    })
    .expect("fixture color metadata")
}

fn frame(width: usize, height: usize, fill: u8) -> v_frame::frame::Frame<u8> {
    let mut frame = v_frame::frame::FrameBuilder::new(width, height, ChromaSubsampling::Yuv444, 8)
        .build::<u8>()
        .expect("fixture frame");
    for index in 0..3 {
        for row in frame.plane_mut(index).expect("fixture plane").rows_mut() {
            row.fill(if index == 0 { fill } else { 128 });
        }
    }
    frame
}

#[test]
fn scorer_construction_succeeds_with_availability_or_reports_library_not_found() {
    // Without fmetrics this must be a clean error, not a panic: the caller
    // falls back to the VapourSynth plugin when this tier is unavailable.
    let config = FmetricsConfig::new(FmetricsMetric::Ssimulacra2);
    match FmetricsScorer::new(config, color(8), color(8)) {
        Ok(_) => assert!(av_metrics_fmetrics::is_available()),
        Err(error) => assert!(
            matches!(error, FmetricsError::LibraryNotFound { .. }),
            "expected unavailability, got {error:?}"
        ),
    }
}

#[test]
fn a_temporal_metric_needs_the_temporal_entry_point() {
    let Ok(mut scorer) = FmetricsScorer::new(
        FmetricsConfig::new(FmetricsMetric::Cvvdp).with_frame_rate(24.0),
        color(8),
        color(8),
    ) else {
        return;
    };
    let size = 64;
    let frame = frame(size, size, 100);
    let planes = PlaneSet::from_frame(&frame).expect("fixture plane set");
    let error = scorer
        .submit_pair(&planes, &planes)
        .expect_err("submit_pair must refuse a temporal metric");
    assert!(
        matches!(error, FmetricsError::MetricUnavailable { .. }),
        "expected an unavailability error naming the remedy, got {error:?}"
    );
    assert!(
        error.to_string().contains("submit_pair_temporal"),
        "the error should name the entry point that does work, got {error}"
    );
    assert!(
        scorer.submit_pair_temporal(&planes, &planes).is_ok(),
        "submit_pair_temporal must accept a temporal metric"
    );
}

#[test]
fn concurrent_scoring_matches_serial_scoring() {
    // A native scratch arena must never be shared by concurrent callers.
    let Ok(scorer) = FmetricsScorer::new(
        FmetricsConfig::new(FmetricsMetric::Ssimulacra2).with_threads(4),
        color(8),
        color(8),
    ) else {
        return;
    };
    let size = 64;
    let reference_bytes = vec![128u8; size * size * 3];
    let distorted_bytes = vec![100u8; size * size * 3];
    let reference = RgbView::new(&reference_bytes, size, size, false).expect("reference RGB");
    let distorted = RgbView::new(&distorted_bytes, size, size, false).expect("distorted RGB");
    let expected = scorer.submit_rgb(&reference, &distorted).expect("serial score");
    let scorer = std::sync::Arc::new(scorer);
    // Four threads for four workspaces; exhaustion is tested separately.
    let results: Vec<FrameScore> = std::thread::scope(|scope| {
        (0..4)
            .map(|_| {
                let scorer = std::sync::Arc::clone(&scorer);
                scope.spawn(move || {
                    scorer.submit_rgb(&reference, &distorted).expect("concurrent score")
                })
            })
            .map(|handle| handle.join().expect("scoring worker"))
            .collect()
    });
    for score in &results {
        assert_eq!(
            score.ssimulacra2.expect("concurrent SSIMULACRA2 value").to_bits(),
            expected.ssimulacra2.expect("serial SSIMULACRA2 value").to_bits(),
            "a concurrent score differed from the serial one, so a workspace was shared"
        );
    }
}
