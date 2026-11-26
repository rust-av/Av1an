//! Correctness tests for vship scoring.
//!
//! Tests that need libvship are skipped when it is unavailable, so the suite
//! stays green in environments without it. That includes CI: there is no GPU
//! and no libvship binary there, so the skip is the expected outcome rather
//! than a failure. `just test-gpu` makes availability mandatory on a GPU host.
//! Everything that does not require libvship always runs.

// Test code asserts and unwraps freely: a failure panics with a readable
// message, which is exactly what a test wants.
#![allow(clippy::unwrap_used, reason = "assertion failures should be loud")]

mod common;

use av_decoders::VideoDetails;
use av_metrics_vship::{
    PlaneSet, PlaneSource, PoolMethod, VideoFormat, VshipBackend, VshipConfig, VshipError,
    VshipMetric, VshipScorer, device_name, is_available, libvship_version,
};
use common::{Distortion, SyntheticClip, SyntheticPair};
use v_frame::chroma::ChromaSubsampling;

/// Skip the test body when libvship is not installed.
macro_rules! require_vship {
    () => {
        if !is_available() {
            eprintln!("skipping: libvship is not installed, or no device passes GPUFullCheck");
            return;
        }
    };
}

/// A config retaining a non-default legacy handler-thread request.
fn config_for(metric: VshipMetric) -> VshipConfig {
    VshipConfig::default().with_metric(metric).with_handler_threads(2)
}

/// Video details in the shape `av-decoders` reports them.
fn details(
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
        frame_rate: av_decoders::Rational32::new(24, 1),
        total_frames: None,
        color_range: None,
        color_primaries: None,
        color_transfer_characteristic: None,
        color_space: None,
        chroma_location: None,
        mastering_display: None,
        content_light_level: None,
    }
}

/// A small 8-bit 4:2:0 reference/distorted pair.
fn small_pair(distortion: Distortion) -> SyntheticPair {
    SyntheticPair::generate(64, 48, 8, ChromaSubsampling::Yuv420, 12, distortion)
}

/// The format of a generated clip, as the scorer wants it.
fn format_for(clip: &SyntheticClip) -> VideoFormat {
    VideoFormat::from_details(clip.decoder().get_video_details())
}

/// Three constant planes of the geometry a 64x48 4:2:0 frame declares.
///
fn grey_planes<'a>(luma: &'a [u8], chroma: &'a [u8]) -> PlaneSet<'a> {
    [
        PlaneSource::from_bytes(luma, 64, 48, 64, 0, 1).unwrap(),
        PlaneSource::from_bytes(chroma, 32, 24, 32, 0, 1).unwrap(),
        PlaneSource::from_bytes(chroma, 32, 24, 32, 0, 1).unwrap(),
    ]
}

/// A grey 64x48 4:2:0 frame.
fn grey_frame() -> (Vec<u8>, Vec<u8>) {
    (vec![128u8; 64 * 48], vec![128u8; 32 * 24])
}

// --- Availability -----------------------------------------------------------

/// A machine without libvship must report unavailability rather than panicking.
#[test]
fn availability_probe_is_consistent() {
    if std::env::var_os("AV_METRICS_REQUIRE_GPU").is_some() {
        assert!(
            is_available(),
            "required GPU checks need libvship and a device passing GPUFullCheck"
        );
    }
    if is_available() {
        assert!(
            libvship_version().is_some(),
            "a loadable libvship must report a version"
        );
        assert!(
            device_name().is_some(),
            "an available libvship must have a device"
        );
    } else {
        assert_eq!(libvship_version(), None);
        assert_eq!(device_name(), None);
    }
}

/// Shared by the pure sensitivity check and the actual backend gate.
fn upload_fixture(padded: bool, distorted: bool) -> [Vec<u16>; 3] {
    std::array::from_fn(|plane| {
        let (width, height, pitch) = [(65, 49, 68), (33, 25, 36), (33, 25, 36)][plane];
        let pitch = if padded { pitch } else { width };
        let mut data =
            vec![1000 + plane as u16 * 2 + u16::from(distorted); (height - 1) * pitch + width];
        for row in 0..height {
            for column in 0..width {
                let value = 128
                    + ((column * (17 + plane * 4) + row * (29 + plane * 6) + plane * 101) % 640);
                let difference = if distorted {
                    1 + (column + row * 3 + plane * 7) % 23
                } else {
                    0
                };
                data[row * pitch + column] = (value + difference) as u16;
            }
        }
        data
    })
}

mod fixture_unit {
    use super::*;

    #[test]
    fn gpu_fixture_detects_ignored_stride_without_a_gpu() {
        for distorted in [false, true] {
            let packed = upload_fixture(false, distorted);
            let padded = upload_fixture(true, distorted);
            assert_eq!(packed.each_ref().map(Vec::len), [3185, 825, 825]);
            assert_eq!(padded.each_ref().map(Vec::len), [3329, 897, 897]);
            for (plane, (width, height, pitch, sentinel)) in
                [(65, 49, 68, 1000), (33, 25, 36, 1002), (33, 25, 36, 1004)]
                    .into_iter()
                    .enumerate()
            {
                for row in 0..height {
                    assert_eq!(
                        &packed[plane][row * width..(row + 1) * width],
                        &padded[plane][row * pitch..row * pitch + width]
                    );
                }
                assert_eq!(padded[plane][width], sentinel + u16::from(distorted));
                assert_ne!(
                    padded[plane][width], packed[plane][width],
                    "ignored pitch must read padding"
                );
                assert_ne!(&padded[plane][..width * height], packed[plane].as_slice());
                assert_ne!(
                    packed[plane][0], packed[plane][1],
                    "visible pixels must not be flat"
                );
            }
        }
        let reference = upload_fixture(false, false);
        let distorted = upload_fixture(false, true);
        assert_eq!(reference.each_ref().map(|p| p[0]), [128, 229, 330]);
        assert_eq!(distorted.each_ref().map(|p| p[0]), [129, 237, 345]);
        for plane in 0..3 {
            assert_ne!(reference[plane], distorted[plane]);
        }
    }
}

/// Hardware gate: ABI sinks cannot prove backend upload completion or accuracy.
#[test]
fn gpu_ten_bit_scorer_rejects_u8_frame_without_submitting() {
    if !upload_gpu_available() {
        return;
    }
    let format = VideoFormat::from_details(&details(64, 48, 10, ChromaSubsampling::Yuv420));
    let mut scorer = upload_gpu_scorer(format);
    let byte_frame = v_frame::frame::FrameBuilder::new(64, 48, ChromaSubsampling::Yuv420, 8)
        .build::<u8>()
        .unwrap();
    assert!(matches!(
        scorer.submit_pair(&byte_frame, &byte_frame),
        Err(VshipError::InputMismatch { .. })
    ));
    assert_eq!(scorer.submitted().unwrap(), 0);
}

#[test]
fn gpu_padded_reference_upload_matches_packed_score() {
    padded_upload_case(true);
}

#[test]
fn gpu_padded_distorted_upload_matches_packed_score() {
    padded_upload_case(false);
}

fn upload_gpu_available() -> bool {
    if !is_available() {
        assert!(
            std::env::var_os("AV_METRICS_REQUIRE_GPU").is_none(),
            "required upload GPU gate needs libvship and a supported device"
        );
        eprintln!("unexecuted upload hardware gate: no supported libvship/GPU runtime");
        return false;
    }
    true
}

fn upload_gpu_scorer(format: VideoFormat) -> VshipScorer {
    let scorer =
        VshipScorer::new(config_for(VshipMetric::Ssimulacra2), format, format, None).unwrap();
    assert!(
        matches!(
            scorer.backend(),
            VshipBackend::Hip | VshipBackend::Cuda | VshipBackend::Vulkan
        ),
        "upload gate requires a supported GPU backend, not a CPU/sink"
    );
    scorer
}

fn padded_upload_case(pad_reference: bool) {
    if !upload_gpu_available() {
        return;
    }
    let format = VideoFormat::from_details(&details(65, 49, 10, ChromaSubsampling::Yuv420));
    let mut scorer = upload_gpu_scorer(format);
    let reference = upload_fixture(false, false);
    let distorted = upload_fixture(false, true);
    let padded_reference = upload_fixture(true, false);
    let padded_distorted = upload_fixture(true, true);
    fn planes(data: &[Vec<u16>; 3], padded: bool) -> PlaneSet<'_> {
        [
            PlaneSource::from_u16(&data[0], 65, 49, if padded { 136 } else { 130 }, 0).unwrap(),
            PlaneSource::from_u16(&data[1], 33, 25, if padded { 72 } else { 66 }, 0).unwrap(),
            PlaneSource::from_u16(&data[2], 33, 25, if padded { 72 } else { 66 }, 0).unwrap(),
        ]
    }
    let baseline = scorer
        .submit_pair_raw(planes(&reference, false), planes(&distorted, false), false)
        .unwrap()
        .score
        .unwrap();
    assert!(
        baseline.is_finite(),
        "packed baseline must yield a finite GPU score"
    );
    // Change only ONE side's layout at a time so wrong reads cannot cancel.
    let (side, a, b) = if pad_reference {
        (
            "reference",
            planes(&padded_reference, true),
            planes(&distorted, false),
        )
    } else {
        (
            "distorted",
            planes(&reference, false),
            planes(&padded_distorted, true),
        )
    };
    let score = scorer.submit_pair_raw(a, b, false).unwrap().score.unwrap();
    assert!(
        score.is_finite(),
        "padded {side} must yield a finite GPU score"
    );
    assert!(
        (score - baseline).abs() <= 1e-6,
        "{side} padded={score}, packed={baseline}"
    );
}

/// Constructing a scorer must never panic, whether or not libvship is present.
#[test]
fn construction_fails_cleanly_without_vship() {
    let format = VideoFormat::from_details(&details(64, 48, 8, ChromaSubsampling::Yuv420));

    let outcome = VshipScorer::new(VshipConfig::default(), format, format, None);
    if !is_available() {
        assert!(
            outcome.is_err(),
            "an unavailable libvship must not yield a scorer"
        );
    }
}

/// Every metric must fail with a reported error rather than a panic.
#[test]
fn all_metric_constructors_complete_without_panicking() {
    let format = VideoFormat::from_details(&details(64, 48, 8, ChromaSubsampling::Yuv420));

    for metric in VshipMetric::all() {
        // Whether this succeeds depends on the machine; what must not happen is
        // a panic.
        let _ = VshipScorer::new(config_for(metric), format, format, None);
    }
}

#[test]
fn the_backend_is_reported_when_constructing_a_scorer() {
    require_vship!();

    let pair = small_pair(Distortion::Mild);
    let format = format_for(&pair.reference);
    let scorer = VshipScorer::new(config_for(VshipMetric::Ssimulacra2), format, format, None)
        .expect("scorer should initialise");

    // A loaded libvship must name one of the four backends.
    assert!(matches!(
        scorer.backend(),
        VshipBackend::Hip | VshipBackend::Cuda | VshipBackend::Vulkan | VshipBackend::Cpu
    ));
    assert_eq!(Some(scorer.gpu_id()), av_metrics_vship::default_gpu_id());
    assert_eq!(scorer.metric(), VshipMetric::Ssimulacra2);
}

/// A per-pair metric needs only one synchronous handler.
#[test]
fn a_nontemporal_metric_builds_exactly_one_handler() {
    require_vship!();

    let pair = small_pair(Distortion::Mild);
    let format = format_for(&pair.reference);
    let scorer = VshipScorer::new(
        config_for(VshipMetric::Ssimulacra2).with_handler_threads(3),
        format,
        format,
        None,
    )
    .expect("scorer should initialise");

    assert_eq!(scorer.handler_count(), 1);
}

/// CVVDP is temporal, so it keeps exactly one handler however many threads the
/// configuration asks for.
#[test]
fn a_temporal_metric_builds_exactly_one_handler() {
    require_vship!();

    let pair = small_pair(Distortion::Mild);
    let format = format_for(&pair.reference);
    let scorer = VshipScorer::new(
        config_for(VshipMetric::Cvvdp).with_handler_threads(8),
        format,
        format,
        None,
    )
    .expect("scorer should initialise");

    assert_eq!(scorer.handler_count(), 1);
}

// --- Scoring ----------------------------------------------------------------

#[test]
fn identical_clips_have_ssimulacra2_scores_above_point_nine_nine() {
    require_vship!();

    let pair = small_pair(Distortion::None);
    let format = format_for(&pair.reference);
    let mut scorer =
        VshipScorer::new(config_for(VshipMetric::Ssimulacra2), format, format, None).unwrap();

    // Score the undistorted clip against itself.
    let mut reference = pair.reference.decoder();
    let mut distorted = pair.reference.decoder();
    let scores = scorer.score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {}).unwrap();

    assert!(!scores.is_empty(), "every pair must produce a score");
    for score in &scores {
        let value = score.value(VshipMetric::Ssimulacra2, true).expect("SSIMULACRA2 sets `score`");
        // SSIMULACRA2's maximum is 1.0, and identical inputs must reach it
        // within the metric's numerical slack.
        assert!(
            value > 0.99,
            "identical inputs scored {value}, expected near 1.0"
        );
    }
}

/// A score arrives for every pair, with no flush step in between.
#[test]
fn decoder_scoring_returns_twelve_scores_callbacks_and_submissions_without_flush() {
    require_vship!();

    let pair = small_pair(Distortion::Mild);
    let format = format_for(&pair.reference);
    let mut scorer =
        VshipScorer::new(config_for(VshipMetric::Ssimulacra2), format, format, None).unwrap();

    let (mut reference, mut distorted) = pair.decoders();

    let mut callback_count = 0;
    let scores = scorer
        .score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {
            callback_count += 1;
        })
        .unwrap();

    assert_eq!(scores.len(), 12, "one score per frame pair");
    assert_eq!(callback_count, 12, "the callback fires once per pair");
    assert_eq!(scorer.submitted().unwrap(), 12);
}

#[test]
fn ssimulacra2_pooled_score_does_not_increase_with_distortion() {
    require_vship!();

    let format = format_for(&small_pair(Distortion::Mild).reference);
    let mut previous = f64::INFINITY;

    for distortion in [Distortion::Mild, Distortion::Moderate, Distortion::Severe] {
        let pair = small_pair(distortion);
        let mut scorer =
            VshipScorer::new(config_for(VshipMetric::Ssimulacra2), format, format, None).unwrap();

        let (mut reference, mut distorted) = pair.decoders();
        let scores =
            scorer.score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {}).unwrap();
        let pooled =
            VshipScorer::pool(&scores, VshipMetric::Ssimulacra2, PoolMethod::Mean, true).unwrap();

        assert!(
            pooled <= previous,
            "{distortion:?} scored {pooled}, above the milder level's {previous}"
        );
        previous = pooled;
    }
}

#[test]
fn butteraugli_reports_all_three_norms() {
    require_vship!();

    let pair = small_pair(Distortion::Moderate);
    let format = format_for(&pair.reference);
    let mut scorer =
        VshipScorer::new(config_for(VshipMetric::Butteraugli), format, format, None).unwrap();

    let (mut reference, mut distorted) = pair.decoders();
    let scores = scorer.score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {}).unwrap();

    let first = scores.first().expect("the pair is not empty");
    assert!(first.norm_q.is_some(), "Butteraugli sets `norm_q`");
    assert!(first.norm_3.is_some(), "Butteraugli sets `norm_3`");
    assert!(first.norm_inf.is_some(), "Butteraugli sets `norm_inf`");
    assert!(first.score.is_none(), "Butteraugli does not set `score`");

    // Butteraugli is a difference, so two different clips must score above
    // zero.
    let pooled =
        VshipScorer::pool(&scores, VshipMetric::Butteraugli, PoolMethod::Mean, true).unwrap();
    assert!(
        pooled > 0.0,
        "a distorted clip must score above zero, got {pooled}"
    );
}

/// The headline value for Butteraugli depends on the configured norm, not on
/// the metric alone.
///
/// The VapourSynth plugin reports `BUTTERAUGLI_INFNorm` unless a norm is
/// requested and `BUTTERAUGLI_QNorm` when one is, so a caller that always reads
/// `norm_q` silently reports a different quantity from the plugin it replaces.
/// Both selections must come from the same pair and must differ.
#[test]
fn butteraugli_headline_and_pool_select_distinct_q_or_infinity_norms() {
    require_vship!();

    let pair = small_pair(Distortion::Moderate);
    let format = format_for(&pair.reference);
    let mut scorer =
        VshipScorer::new(config_for(VshipMetric::Butteraugli), format, format, None).unwrap();

    let (mut reference, mut distorted) = pair.decoders();
    let scores = scorer.score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {}).unwrap();

    let first = scores.first().expect("the pair is not empty");
    let q_norm = first.norm_q.expect("Butteraugli sets `norm_q`");
    let infinity = first.norm_inf.expect("Butteraugli sets `norm_inf`");

    assert_ne!(
        q_norm, infinity,
        "the Q-norm and the infinity norm must be distinguishable, or the choice between them \
         cannot matter"
    );
    assert_eq!(
        first.value(VshipMetric::Butteraugli, true),
        Some(q_norm),
        "`true` selects the Q-norm"
    );
    assert_eq!(
        first.value(VshipMetric::Butteraugli, false),
        Some(infinity),
        "`false` selects the infinity norm, matching the plugin's default"
    );

    // `pool` keeps the Q-norm for callers that configure `q_norm`, which is the
    // default a Butteraugli-specific pool has always used.
    let pooled =
        VshipScorer::pool(&scores, VshipMetric::Butteraugli, PoolMethod::Mean, true).unwrap();
    assert!(
        pooled > 0.0,
        "a distorted clip must score above zero, got {pooled}"
    );

    let pooled_inf =
        VshipScorer::pool(&scores, VshipMetric::Butteraugli, PoolMethod::Mean, false).unwrap();
    assert!(
        pooled_inf > 0.0,
        "the infinity norm of a distorted clip must be above zero, got {pooled_inf}"
    );
    assert_ne!(
        pooled, pooled_inf,
        "pooling the two norms must not coincidentally agree, or the selection is untested"
    );
}

/// The configured norm exponent must actually reach libvship.
#[test]
fn butteraugli_q_norm_changes_the_score() {
    require_vship!();

    let pair = small_pair(Distortion::Moderate);
    let format = format_for(&pair.reference);
    let mut pooled = Vec::new();

    for q_norm in [2, 3] {
        let mut scorer = VshipScorer::new(
            config_for(VshipMetric::Butteraugli).with_q_norm(q_norm),
            format,
            format,
            None,
        )
        .unwrap();

        let (mut reference, mut distorted) = pair.decoders();
        let scores =
            scorer.score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {}).unwrap();
        pooled.push(
            VshipScorer::pool(&scores, VshipMetric::Butteraugli, PoolMethod::Mean, true).unwrap(),
        );
    }

    assert!(
        (pooled[0] - pooled[1]).abs() > f64::EPSILON,
        "q_norm must reach libvship, but both settings scored {pooled:?}"
    );
}

#[test]
fn cvvdp_first_and_last_scores_differ_on_varying_clip() {
    require_vship!();

    let pair = small_pair(Distortion::Mild);
    let format = format_for(&pair.reference);
    let mut scorer = VshipScorer::new(
        config_for(VshipMetric::Cvvdp).with_fps(24.0),
        format,
        format,
        None,
    )
    .unwrap();

    let (mut reference, mut distorted) = pair.decoders();
    let scores = scorer.score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {}).unwrap();

    assert!(!scores.is_empty());
    // A temporal metric's score is a running mean over everything it has seen,
    // so it moves as frames arrive rather than staying per-frame constant.
    let first = scores[0].value(VshipMetric::Cvvdp, true).unwrap();
    let last = scores[scores.len() - 1].value(VshipMetric::Cvvdp, true).unwrap();
    assert_ne!(
        first, last,
        "CVVDP's running mean must move as frames are fed"
    );
}

#[test]
fn cvvdp_accepts_temporal_and_score_resets_after_a_submission() {
    require_vship!();

    let pair = small_pair(Distortion::Mild);
    let format = format_for(&pair.reference);
    let mut scorer = VshipScorer::new(
        config_for(VshipMetric::Cvvdp).with_fps(24.0),
        format,
        format,
        None,
    )
    .unwrap();

    let mut reference = pair.reference.decoder();
    let mut distorted = pair.distorted.decoder();

    let frame_reference = reference.read_video_frame::<u8>().unwrap();
    let frame_distorted = distorted.read_video_frame::<u8>().unwrap();
    scorer.submit_pair(&frame_reference, &frame_distorted).unwrap();

    // `Vship_Reset` clears the temporal history without recreating the handler,
    // which is what a scene break requires.
    scorer.reset_temporal().expect("reset should be accepted");
    scorer.reset_score().expect("reset_score should be accepted");
}

#[test]
fn clips_of_different_lengths_are_reported() {
    require_vship!();

    let long = SyntheticClip::generate(64, 48, 8, ChromaSubsampling::Yuv420, 12, Distortion::None);
    let short = SyntheticClip::generate(64, 48, 8, ChromaSubsampling::Yuv420, 5, Distortion::None);
    let format = format_for(&long);

    let mut scorer =
        VshipScorer::new(config_for(VshipMetric::Ssimulacra2), format, format, None).unwrap();

    let mut reference = long.decoder();
    let mut distorted = short.decoder();

    assert!(matches!(
        scorer.score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {}),
        Err(VshipError::FrameCountMismatch { .. })
    ));
}

/// A mismatched pair needs an explicit target, because libvship rejects it
/// otherwise with `DifferingInputType`.
#[test]
fn a_lower_resolution_encode_can_be_scored_against_a_larger_reference() {
    require_vship!();

    let reference_clip =
        SyntheticClip::generate(128, 96, 8, ChromaSubsampling::Yuv420, 8, Distortion::None);
    let encode_clip = SyntheticClip::generate(
        64,
        48,
        8,
        ChromaSubsampling::Yuv420,
        8,
        Distortion::Moderate,
    );

    let reference_format = format_for(&reference_clip);
    let encode_format = format_for(&encode_clip);

    let mut scorer = VshipScorer::new(
        config_for(VshipMetric::Ssimulacra2),
        reference_format,
        encode_format,
        Some((64, 48)),
    )
    .unwrap();

    let mut reference = reference_clip.decoder();
    let mut distorted = encode_clip.decoder();
    let scores = scorer.score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {}).unwrap();

    assert_eq!(scores.len(), 8);
}

/// With no target, a mismatched pair adopts the reference size rather than
/// failing.
///
/// libvship has no geometry-equality check: it converts each side per its own
/// colorspace and rejects the pair with `DifferingInputType` if the results
/// differ. Passing no target used to leave both sides unscaled and so fail
/// exactly where the VapourSynth plugin succeeded. The reference size is
/// adopted instead, matching what FFVship does by always scaling the encode to
/// the source.
#[test]
fn mismatched_resolutions_score_eight_pairs_without_explicit_target() {
    require_vship!();

    let reference_clip =
        SyntheticClip::generate(128, 96, 8, ChromaSubsampling::Yuv420, 8, Distortion::None);
    let encode_clip = SyntheticClip::generate(
        64,
        48,
        8,
        ChromaSubsampling::Yuv420,
        8,
        Distortion::Moderate,
    );

    let mut scorer = VshipScorer::new(
        config_for(VshipMetric::Ssimulacra2),
        format_for(&reference_clip),
        format_for(&encode_clip),
        None,
    )
    .expect("a mismatched pair should score against the reference size");

    let mut reference = reference_clip.decoder();
    let mut distorted = encode_clip.decoder();
    let scores = scorer.score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {}).unwrap();

    assert_eq!(scores.len(), 8);
}

/// Matching sizes must be left alone when no target is given, or every ordinary
/// pass would pay for a rescale it does not need.
#[test]
fn matching_pair_scores_agree_with_absent_or_identity_target() {
    require_vship!();

    let reference_clip =
        SyntheticClip::generate(64, 48, 8, ChromaSubsampling::Yuv420, 8, Distortion::None);
    let encode_clip = SyntheticClip::generate(
        64,
        48,
        8,
        ChromaSubsampling::Yuv420,
        8,
        Distortion::Moderate,
    );

    let mut scorer = VshipScorer::new(
        config_for(VshipMetric::Ssimulacra2),
        format_for(&reference_clip),
        format_for(&encode_clip),
        None,
    )
    .expect("a matching pair should score without a target");

    let mut reference = reference_clip.decoder();
    let mut distorted = encode_clip.decoder();
    let scores = scorer.score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {}).unwrap();

    // Identical geometry is what makes this equivalent to scoring without any
    // scaling. Comparing against the same pair given an explicit target equal
    // to its own size pins that: both must produce identical scores.
    let mut targeted = VshipScorer::new(
        config_for(VshipMetric::Ssimulacra2),
        format_for(&reference_clip),
        format_for(&encode_clip),
        Some((64, 48)),
    )
    .expect("an explicit target matching the native size should also score");

    let mut reference = reference_clip.decoder();
    let mut distorted = encode_clip.decoder();
    let untargeted_values: Vec<f64> = scores.iter().filter_map(|score| score.score).collect();

    let targeted_scores = targeted
        .score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {})
        .unwrap();
    let targeted_values: Vec<f64> =
        targeted_scores.iter().filter_map(|score| score.score).collect();

    assert_eq!(untargeted_values, targeted_values);
}

/// A plane whose geometry disagrees with its colorspace must be rejected before
/// any pointer reaches libvship.
#[test]
fn a_plane_whose_geometry_disagrees_is_rejected() {
    require_vship!();

    let format = VideoFormat::from_details(&details(64, 48, 8, ChromaSubsampling::Yuv420));
    let mut scorer =
        VshipScorer::new(config_for(VshipMetric::Ssimulacra2), format, format, None).unwrap();

    let (luma, chroma) = grey_frame();
    let mut planes = grey_planes(&luma, &chroma);
    planes[1] = PlaneSource::from_bytes(&chroma, 16, 24, 32, 0, 1).unwrap();

    assert!(matches!(
        scorer.submit_pair_raw(planes, planes, false),
        Err(VshipError::InputMismatch { .. })
    ));
}

#[test]
fn repeated_identical_raw_ssimulacra2_pairs_return_equal_values() {
    require_vship!();

    let format = VideoFormat::from_details(&details(64, 48, 8, ChromaSubsampling::Yuv420));
    let mut scorer =
        VshipScorer::new(config_for(VshipMetric::Ssimulacra2), format, format, None).unwrap();

    let (luma, chroma) = grey_frame();
    let first = {
        let planes = grey_planes(&luma, &chroma);
        scorer.submit_pair_raw(planes, planes, false)
    };
    let second = {
        let planes = grey_planes(&luma, &chroma);
        scorer.submit_pair_raw(planes, planes, false)
    };

    // Identical pairs must score identically, since a non-temporal metric
    // carries no state between them.
    assert_eq!(
        first.expect("geometry is correct").value(VshipMetric::Ssimulacra2, true),
        second.expect("geometry is correct").value(VshipMetric::Ssimulacra2, true)
    );
}
