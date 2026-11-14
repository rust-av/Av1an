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

use std::mem::{align_of, size_of};

use av_decoders::VideoDetails;
use av_metrics_vship::{
    FrameScore, PlaneSet, PlaneSource, PoolMethod, VideoFormat, VshipBackend, VshipChromaLocation,
    VshipChromaSubsample, VshipColorFamily, VshipColorspace, VshipConfig, VshipCropRectangle,
    VshipError, VshipMetric, VshipPrimaries, VshipRange, VshipSample, VshipScorer,
    VshipTransferFunction, VshipYuvMatrix, colorspace_from_details, device_name, is_available,
    libvship_version, sample_for_bit_depth, subsample_for_chroma,
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
    VshipConfig::new().with_metric(metric).with_handler_threads(2)
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

/// Hardware gate: ABI sinks cannot prove backend upload completion or accuracy.
#[test]
fn gpu_padded_uploads_and_typed_mismatch() {
    if !is_available() {
        assert!(
            std::env::var_os("AV_METRICS_REQUIRE_GPU").is_none(),
            "required upload GPU gate needs libvship and a supported device"
        );
        eprintln!("unexecuted upload hardware gate: no supported libvship/GPU runtime");
        return;
    }
    let format = VideoFormat::from_details(&details(64, 48, 10, ChromaSubsampling::Yuv420));
    let mut scorer =
        VshipScorer::new(config_for(VshipMetric::Ssimulacra2), format, format, None).unwrap();
    assert!(
        matches!(
            scorer.backend(),
            VshipBackend::Hip | VshipBackend::Cuda | VshipBackend::Vulkan
        ),
        "upload gate requires a supported GPU backend, not a CPU/sink"
    );
    let byte_frame = v_frame::frame::FrameBuilder::new(64, 48, ChromaSubsampling::Yuv420, 8)
        .build::<u8>()
        .unwrap();
    assert!(matches!(
        scorer.submit_pair(&byte_frame, &byte_frame),
        Err(VshipError::InputMismatch { .. })
    ));
    assert_eq!(scorer.submitted().unwrap(), 0);
    let format = VideoFormat::from_details(&details(65, 49, 10, ChromaSubsampling::Yuv420));
    let mut scorer =
        VshipScorer::new(config_for(VshipMetric::Ssimulacra2), format, format, None).unwrap();
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
        .submit_pair_raw(planes(&reference, false), planes(&distorted, false))
        .unwrap()
        .score
        .unwrap();
    assert!(baseline.is_finite());
    // Change only ONE side's layout at a time so wrong reads cannot cancel.
    for (side, a, b) in [
        (
            "reference",
            planes(&padded_reference, true),
            planes(&distorted, false),
        ),
        (
            "distorted",
            planes(&reference, false),
            planes(&padded_distorted, true),
        ),
    ] {
        let score = scorer.submit_pair_raw(a, b).unwrap().score.unwrap();
        assert!(score.is_finite());
        assert!(
            (score - baseline).abs() <= 1e-6,
            "{side} padded={score}, packed={baseline}"
        );
    }
}

/// Constructing a scorer must never panic, whether or not libvship is present.
#[test]
fn construction_fails_cleanly_without_vship() {
    let format = VideoFormat::from_details(&details(64, 48, 8, ChromaSubsampling::Yuv420));

    let outcome = VshipScorer::new(VshipConfig::new(), format, format, None);
    if !is_available() {
        assert!(
            outcome.is_err(),
            "an unavailable libvship must not yield a scorer"
        );
    }
}

/// Every metric must fail with a reported error rather than a panic.
#[test]
fn every_metric_reports_rather_than_panics() {
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

// --- Struct layout ----------------------------------------------------------

/// Mirrors `typedef struct Vship_Colorspace_t` in VshipColor.h. A wrong field
/// order still compiles, and surfaces only as an opaque status from libvship at
/// runtime.
#[test]
fn colorspace_layout_matches_the_c_header() {
    #[repr(C)]
    struct Expected {
        width: i64,
        height: i64,
        target_width: i64,
        target_height: i64,
        sample: VshipSample,
        range: VshipRange,
        subsampling: VshipChromaSubsample,
        chroma_location: VshipChromaLocation,
        color_family: VshipColorFamily,
        yuv_matrix: VshipYuvMatrix,
        transfer_function: VshipTransferFunction,
        primaries: VshipPrimaries,
        crop: VshipCropRectangle,
    }

    assert_eq!(size_of::<VshipColorspace>(), size_of::<Expected>());
    assert_eq!(align_of::<VshipColorspace>(), align_of::<Expected>());
}

/// `Vship_Sample_t` skips values, so a sequential conversion would reinterpret
/// every pixel buffer.
#[test]
fn sample_discriminants_are_non_contiguous_and_explicit() {
    assert_eq!(VshipSample::Float as i32, 0);
    assert_eq!(VshipSample::Half as i32, 1);
    assert_eq!(VshipSample::Uint8 as i32, 2);
    assert_eq!(VshipSample::Uint9 as i32, 3);
    assert_eq!(VshipSample::Uint10 as i32, 5);
    assert_eq!(VshipSample::Uint12 as i32, 7);
    assert_eq!(VshipSample::Uint14 as i32, 9);
    assert_eq!(VshipSample::Uint16 as i32, 11);
}

/// Four int64 fields precede the first enum, so the enums begin at offset 32.
#[test]
fn colorspace_field_offsets_match_the_c_header() {
    let colorspace = VshipColorspace::bt709_default(1920, 1080);
    let base = std::ptr::from_ref(&colorspace).cast::<u8>() as usize;
    let offset = |field: *const u8| field as usize - base;

    assert_eq!(
        offset(std::ptr::from_ref(&colorspace.width).cast::<u8>()),
        0
    );
    assert_eq!(
        offset(std::ptr::from_ref(&colorspace.height).cast::<u8>()),
        8
    );
    assert_eq!(
        offset(std::ptr::from_ref(&colorspace.target_width).cast::<u8>()),
        16
    );
    assert_eq!(
        offset(std::ptr::from_ref(&colorspace.target_height).cast::<u8>()),
        24
    );
    assert_eq!(
        offset(std::ptr::from_ref(&colorspace.sample).cast::<u8>()),
        32
    );
}

// --- Colorspace mapping -----------------------------------------------------

#[test]
fn bit_depths_map_to_the_vship_sample_enum() {
    assert_eq!(sample_for_bit_depth(8), Some(VshipSample::Uint8));
    assert_eq!(sample_for_bit_depth(9), Some(VshipSample::Uint9));
    assert_eq!(sample_for_bit_depth(10), Some(VshipSample::Uint10));
    assert_eq!(sample_for_bit_depth(12), Some(VshipSample::Uint12));
    assert_eq!(sample_for_bit_depth(14), Some(VshipSample::Uint14));
    assert_eq!(sample_for_bit_depth(16), Some(VshipSample::Uint16));
    // The C enum has no value for these, so no guess may be made.
    assert_eq!(sample_for_bit_depth(11), None);
    assert_eq!(sample_for_bit_depth(13), None);
}

#[test]
fn chroma_sampling_maps_to_the_vship_subsampling() {
    assert_eq!(
        subsample_for_chroma(ChromaSubsampling::Yuv420),
        Some(VshipChromaSubsample::yuv420())
    );
    assert_eq!(
        subsample_for_chroma(ChromaSubsampling::Yuv422),
        Some(VshipChromaSubsample::yuv422())
    );
    assert_eq!(
        subsample_for_chroma(ChromaSubsampling::Yuv444),
        Some(VshipChromaSubsample::yuv444())
    );
    assert_eq!(subsample_for_chroma(ChromaSubsampling::Monochrome), None);
}

#[test]
fn colorspace_mapping_uses_limited_range_and_bt709() {
    let colorspace =
        colorspace_from_details(&details(1920, 1080, 10, ChromaSubsampling::Yuv420), None)
            .expect("10-bit 4:2:0 is supported");

    assert_eq!(colorspace.width, 1920);
    assert_eq!(colorspace.height, 1080);
    assert_eq!(colorspace.sample, VshipSample::Uint10);
    assert_eq!(colorspace.range, VshipRange::Limited);
    assert_eq!(colorspace.color_family, VshipColorFamily::YUV);
    assert_eq!(colorspace.yuv_matrix, VshipYuvMatrix::Bt709);
    assert_eq!(colorspace.transfer_function, VshipTransferFunction::Bt709);
    assert_eq!(colorspace.primaries, VshipPrimaries::Bt709);
}

/// Av1an's encode resolution becomes libvship's `target_width` and
/// `target_height`, and the input planes themselves are untouched.
#[test]
fn target_resolution_maps_to_the_colorspace_target() {
    let colorspace = colorspace_from_details(
        &details(1920, 1080, 8, ChromaSubsampling::Yuv420),
        Some((1280, 720)),
    )
    .expect("8-bit 4:2:0 is supported");

    assert_eq!(colorspace.width, 1920, "the input geometry is unchanged");
    assert_eq!(colorspace.target_width, 1280);
    assert_eq!(colorspace.target_height, 720);
}

#[test]
fn an_absent_target_resolution_means_no_scaling() {
    let colorspace =
        colorspace_from_details(&details(640, 480, 8, ChromaSubsampling::Yuv420), None)
            .expect("8-bit 4:2:0 is supported");

    assert_eq!(colorspace.target_width, -1);
    assert_eq!(colorspace.target_height, -1);
}

#[test]
fn unsupported_formats_are_reported_rather_than_guessed() {
    assert!(matches!(
        colorspace_from_details(&details(1920, 1080, 11, ChromaSubsampling::Yuv420), None),
        Err(VshipError::UnsupportedFormat { .. })
    ));
    assert!(matches!(
        colorspace_from_details(&details(1920, 1080, 8, ChromaSubsampling::Monochrome), None),
        Err(VshipError::UnsupportedFormat { .. })
    ));
}

/// The reference and the encode are described separately, so they need not
/// match in size or depth.
#[test]
fn differing_inputs_produce_independent_colorspaces() {
    let reference = colorspace_from_details(
        &details(1920, 1080, 8, ChromaSubsampling::Yuv420),
        Some((1280, 720)),
    )
    .unwrap();
    let encode = colorspace_from_details(
        &details(1280, 720, 10, ChromaSubsampling::Yuv422),
        Some((1280, 720)),
    )
    .unwrap();

    assert_eq!(reference.sample, VshipSample::Uint8);
    assert_eq!(encode.sample, VshipSample::Uint10);
    assert_eq!(reference.width, 1920);
    assert_eq!(encode.width, 1280);
    assert_eq!(reference.target_width, encode.target_width);
}

// --- Scoring ----------------------------------------------------------------

#[test]
fn identical_clips_score_at_the_metrics_ideal() {
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
        let value = score.value(VshipMetric::Ssimulacra2).expect("SSIMULACRA2 sets `score`");
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
fn every_pair_yields_a_score_immediately() {
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
fn heavier_distortion_scores_lower() {
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
            VshipScorer::pool(&scores, VshipMetric::Ssimulacra2, PoolMethod::Mean).unwrap();

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
    let pooled = VshipScorer::pool(&scores, VshipMetric::Butteraugli, PoolMethod::Mean).unwrap();
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
fn butteraugli_selects_its_norm_from_the_configuration() {
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
        first.butteraugli_value(true),
        Some(q_norm),
        "`true` selects the Q-norm"
    );
    assert_eq!(
        first.butteraugli_value(false),
        Some(infinity),
        "`false` selects the infinity norm, matching the plugin's default"
    );

    // `pool` keeps the Q-norm for callers that configure `q_norm`, which is the
    // default a Butteraugli-specific pool has always used.
    let pooled = VshipScorer::pool(&scores, VshipMetric::Butteraugli, PoolMethod::Mean).unwrap();
    assert!(
        pooled > 0.0,
        "a distorted clip must score above zero, got {pooled}"
    );

    let pooled_inf = VshipScorer::pool_butteraugli(&scores, false, PoolMethod::Mean).unwrap();
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
        pooled
            .push(VshipScorer::pool(&scores, VshipMetric::Butteraugli, PoolMethod::Mean).unwrap());
    }

    assert!(
        (pooled[0] - pooled[1]).abs() > f64::EPSILON,
        "q_norm must reach libvship, but both settings scored {pooled:?}"
    );
}

#[test]
fn cvvdp_accumulates_across_frames() {
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
    let first = scores[0].value(VshipMetric::Cvvdp).unwrap();
    let last = scores[scores.len() - 1].value(VshipMetric::Cvvdp).unwrap();
    assert_ne!(
        first, last,
        "CVVDP's running mean must move as frames are fed"
    );
}

#[test]
fn a_temporal_reset_starts_a_fresh_sequence() {
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
fn a_mismatched_pair_adopts_the_reference_size_when_no_target_is_given() {
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
fn a_matching_pair_is_not_rescaled_when_no_target_is_given() {
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
        scorer.submit_pair_raw(planes, planes),
        Err(VshipError::InputMismatch { .. })
    ));
}

#[test]
fn submit_pair_raw_accepts_correctly_typed_planes() {
    require_vship!();

    let format = VideoFormat::from_details(&details(64, 48, 8, ChromaSubsampling::Yuv420));
    let mut scorer =
        VshipScorer::new(config_for(VshipMetric::Ssimulacra2), format, format, None).unwrap();

    let (luma, chroma) = grey_frame();
    let first = {
        let planes = grey_planes(&luma, &chroma);
        scorer.submit_pair_raw(planes, planes)
    };
    let second = {
        let planes = grey_planes(&luma, &chroma);
        scorer.submit_pair_raw(planes, planes)
    };

    // Identical pairs must score identically, since a non-temporal metric
    // carries no state between them.
    assert_eq!(
        first.expect("geometry is correct").value(VshipMetric::Ssimulacra2),
        second.expect("geometry is correct").value(VshipMetric::Ssimulacra2)
    );
}

// --- Pooling ----------------------------------------------------------------

#[test]
fn pooling_picks_the_requested_extreme_or_the_mean() {
    let scores = [
        FrameScore {
            score: Some(0.8),
            ..FrameScore::default()
        },
        FrameScore {
            score: Some(0.4),
            ..FrameScore::default()
        },
        FrameScore {
            score: Some(0.6),
            ..FrameScore::default()
        },
    ];

    // The mean is compared with a tolerance, because summing three binary
    // fractions rarely lands exactly on 0.6.
    let mean = VshipScorer::pool(&scores, VshipMetric::Ssimulacra2, PoolMethod::Mean).unwrap();
    assert!((mean - 0.6).abs() < 1e-12, "mean was {mean}, expected 0.6");
    assert_eq!(
        VshipScorer::pool(&scores, VshipMetric::Ssimulacra2, PoolMethod::Max).unwrap(),
        0.8
    );
    assert_eq!(
        VshipScorer::pool(&scores, VshipMetric::Ssimulacra2, PoolMethod::Min).unwrap(),
        0.4
    );
    assert!(VshipScorer::pool(&[], VshipMetric::Ssimulacra2, PoolMethod::Mean).is_err());
}
