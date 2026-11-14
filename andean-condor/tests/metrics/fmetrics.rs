use andean_condor::{
    metrics::{
        fmetrics::{native_supported, score_probed_frames},
        probe::OutputIndexing,
    },
    models::sequence::target_quality::types::QualityMetric,
};
use anyhow::Result;

fn ssimulacra2(threads: Option<u8>) -> QualityMetric {
    QualityMetric::SSIMULACRA2 {
        target_range: (74.0, 76.0),
        resolution: None,
        threads,
        gpu_id: None,
    }
}

fn cvvdp() -> QualityMetric {
    QualityMetric::CVVDP {
        target_range: (40.0, 50.0),
        resolution: None,
        display_model: None,
        resize_to_display: None,
        disable_temporal: None,
        gpu_id: None,
    }
}

const FIXTURE_SIZE: usize = 32;

// Nonuniform frames avoid a scale-invariant metric treating a flat offset as identical.
fn y4m_gradient(shift: u8) -> Vec<u8> {
    let size = FIXTURE_SIZE;
    let mut bytes = format!("YUV4MPEG2 W{size} H{size} F24:1 Ip A1:1 C420mpeg2\n").into_bytes();
    for frame in 0..4usize {
        bytes.extend_from_slice(b"FRAME\n");
        for row in 0..size {
            let ramp: Vec<u8> = (0..size)
                .map(|column| {
                    let base = (row * 4 + column * 3 + frame * 5) as u16;
                    (base + u16::from(shift)).clamp(16, 235) as u8
                })
                .collect();
            bytes.extend_from_slice(&ramp);
        }
        let chroma_len = (size / 2) * (size / 2);
        bytes.extend(std::iter::repeat_n(128u8, chroma_len));
        bytes.extend(std::iter::repeat_n(128u8, chroma_len));
    }
    bytes
}

fn y4m_fixture(bytes: &[u8]) -> tempfile::TempPath {
    let file = tempfile::Builder::new()
        .suffix(".y4m")
        .tempfile()
        .expect("create a y4m fixture");
    let path = file.into_temp_path();
    std::fs::write(&path, bytes).expect("write the y4m fixture");
    path
}

fn score_pair(metric: &QualityMetric, shift: u8) -> Result<Vec<f64>> {
    let reference = y4m_fixture(&y4m_gradient(0));
    let distorted = y4m_fixture(&y4m_gradient(shift));
    let selected: Vec<usize> = (0..4).collect();
    let mut reference =
        av_decoders::Decoder::from_file(&reference).expect("the reference fixture decodes");
    let mut distorted =
        av_decoders::Decoder::from_file(&distorted).expect("the distorted fixture decodes");
    score_probed_frames(
        &mut reference,
        &mut distorted,
        metric,
        &selected,
        OutputIndexing::Aligned,
        None,
        |_, _| {},
    )
}

#[test]
fn driver_rejects_hdr_on_either_side_before_native_scoring() {
    for hdr_side in [0, 1] {
        let mut decoders: Vec<_> = (0..2).map(|side| {
            let transfer = if side == hdr_side { 16 } else { 13 };
            av_decoders::Decoder::from_script(&format!(
                "import vapoursynth as vs\nvs.core.std.BlankClip(width=32, height=32, length=1, format=vs.YUV420P8).std.SetFrameProps(_Primaries=1, _Transfer={transfer}).set_output()"
            ),Default::default()).expect("open metadata fixture")
        }).collect();
        let (reference, distorted) = decoders.split_at_mut(1);
        let error = score_probed_frames(
            &mut reference[0],
            &mut distorted[0],
            &ssimulacra2(None),
            &[0],
            OutputIndexing::Aligned,
            None,
            |_, _| panic!("unsupported HDR must not score"),
        )
        .expect_err("unsupported HDR must fail before native scoring");
        let error = format!("{error:#}");
        assert!(
            error.contains("transfer")
                && error.contains(if hdr_side == 0 {
                    "reference"
                } else {
                    "distorted"
                }),
            "{error}"
        );
    }
}

#[test]
fn fmetrics_supported_metrics_return_four_finite_selected_frame_scores() {
    if !av_metrics_fmetrics::is_available() {
        eprintln!("fmetrics real driver scoring UNEXECUTED: native library unavailable");
        return;
    }
    let butteraugli = QualityMetric::BUTTERAUGLI {
        target_range: (1.0, 3.0),
        resolution: None,
        threads: None,
        intensity_multiplier: None,
        norm: Some(3),
        gpu_id: None,
    };
    for metric in [ssimulacra2(None), butteraugli, cvvdp()] {
        let scores = score_pair(&metric, 30).unwrap_or_else(|error| {
            panic!("{} failed to score: {error:#}", metric.friendly_name())
        });
        assert_eq!(
            scores.len(),
            4,
            "{} should score every selected frame",
            metric.friendly_name()
        );
        assert!(
            scores.iter().all(|score| score.is_finite()),
            "{} produced a non-finite score",
            metric.friendly_name()
        );
    }
}

// Changing images can change per-frame scores without proving temporal accumulation.
#[test]
fn fmetrics_cvvdp_gradient_scores_change_across_frames() {
    if !av_metrics_fmetrics::is_available() {
        return;
    }
    let scores = score_pair(&cvvdp(), 30).expect("CVVDP scores");
    assert!(
        scores.windows(2).any(|pair| pair[0] != pair[1]),
        "CVVDP gradient scores should change across frames, got {scores:?}"
    );
}

#[test]
fn fmetrics_ssimulacra2_shifted_gradient_scores_below_the_ceiling() {
    if !av_metrics_fmetrics::is_available() {
        return;
    }
    let scores = score_pair(&ssimulacra2(None), 30).expect("SSIMULACRA2 scores");
    assert!(
        scores.iter().all(|score| *score < 100.0),
        "a shifted gradient should not score at the ceiling, got {scores:?}"
    );
}

#[test]
fn fmetrics_butteraugli_shifted_gradient_has_positive_distance() {
    if !av_metrics_fmetrics::is_available() {
        return;
    }
    let butteraugli = QualityMetric::BUTTERAUGLI {
        target_range: (1.0, 3.0),
        resolution: None,
        threads: None,
        intensity_multiplier: None,
        norm: Some(3),
        gpu_id: None,
    };
    let distances = score_pair(&butteraugli, 30).expect("Butteraugli scores");
    assert!(
        distances.iter().all(|distance| *distance > 0.0),
        "a shifted gradient should register a non-zero distance, got {distances:?}"
    );
}

#[test]
fn availability_follows_the_metric_and_the_library() {
    assert_eq!(
        native_supported(&ssimulacra2(None)),
        av_metrics_fmetrics::is_available(),
        "SSIMULACRA2 is supported exactly when the library loaded"
    );
    assert!(
        !native_supported(&QualityMetric::XPSNR {
            target_range: (45.0, 55.0),
            resolution: None
        }),
        "a metric fmetrics cannot score is never supported, whatever the library state"
    );
}
