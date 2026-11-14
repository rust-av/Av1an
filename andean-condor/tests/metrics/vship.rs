use andean_condor::{
    metrics::{
        probe::OutputIndexing,
        vship::{native_supported, score_probed_frames},
    },
    models::sequence::target_quality::types::QualityMetric,
};

fn ssimulacra2(threads: Option<u8>) -> QualityMetric {
    QualityMetric::SSIMULACRA2 {
        target_range: (74.0, 76.0),
        resolution: None,
        threads,
        gpu_id: None,
    }
}

fn y4m(frames: &[u8]) -> Vec<u8> {
    let mut bytes = b"YUV4MPEG2 W4 H4 F24:1 Ip A1:1 C420jpeg\n".to_vec();
    for &luma in frames {
        bytes.extend_from_slice(b"FRAME\n");
        bytes.extend_from_slice(&[luma; 16]);
        bytes.extend_from_slice(&[128; 4]);
        bytes.extend_from_slice(&[128; 4]);
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

#[test]
fn driver_rejects_unsupported_transfer_before_gpu_discovery() {
    let mut reference = av_decoders::Decoder::from_script(
        "import vapoursynth as vs\nvs.core.std.BlankClip(width=16, height=16, length=1, format=vs.YUV420P8).set_output()",Default::default()
    ).expect("open reference");
    let mut distorted = av_decoders::Decoder::from_script(
        "import vapoursynth as vs\nvs.core.std.BlankClip(width=16, height=16, length=1, format=vs.YUV420P8).std.SetFrameProps(_Transfer=3).set_output()",Default::default()
    ).expect("open distorted");
    let error = score_probed_frames(
        &mut reference,
        &mut distorted,
        &ssimulacra2(None),
        &[0],
        OutputIndexing::Aligned,
        None,
        |_, _| panic!("unsupported transfer must not score"),
    )
    .expect_err("unsupported transfer must fail before library discovery");
    let error = format!("{error:#}");
    assert!(
        error.contains("transfer") && error.contains("distorted"),
        "{error}"
    );
}

#[test]
fn native_support_requires_both_a_supported_metric_and_the_library() {
    let vmaf = QualityMetric::VMAF {
        target_range: (94.0, 96.0),
        resolution: None,
        scaler: String::new(),
        threads: 1,
        model: None,
        features: vec![],
    };
    assert!(!native_supported(&vmaf));
    assert_eq!(
        native_supported(&ssimulacra2(None)),
        av_metrics_vship::is_available(),
        "a supported metric is native exactly when libvship is available"
    );
}

#[test]
fn vship_driver_returns_one_finite_score_per_selected_frame() {
    if !av_metrics_vship::is_available() {
        return;
    }
    let reference = y4m_fixture(&y4m(&[10, 20, 30]));
    let distorted = y4m_fixture(&y4m(&[10, 20, 30]));
    let mut reference = av_decoders::Decoder::from_file(&reference).expect("open reference");
    let mut distorted = av_decoders::Decoder::from_file(&distorted).expect("open distorted");
    let selected = [0usize, 1, 2];
    let scores = score_probed_frames(
        &mut reference,
        &mut distorted,
        &ssimulacra2(None),
        &selected,
        OutputIndexing::Aligned,
        None,
        |_position, _score| {},
    )
    .expect("libvship should score the pair");
    assert_eq!(scores.len(), selected.len(), "one score per selected frame");
    for score in &scores {
        assert!(score.is_finite(), "a score must be finite, got {score}");
    }
    eprintln!("vship real driver: three finite selected-frame scores verified");
}

#[test]
fn vship_butteraugli_distance_increases_with_distortion() {
    if !av_metrics_vship::is_available() {
        return;
    }
    let reference = y4m_fixture(&y4m(&[10, 10, 10, 10]));
    let butteraugli = QualityMetric::BUTTERAUGLI {
        target_range: (1.0, 3.0),
        resolution: None,
        threads: None,
        intensity_multiplier: None,
        norm: None,
        gpu_id: None,
    };
    let mut cases = Vec::new();
    for luma in [11u8, 128] {
        let distorted = y4m_fixture(&y4m(&[luma; 4]));
        let mut reference_decoder =
            av_decoders::Decoder::from_file(&reference).expect("open reference");
        let mut distorted_decoder =
            av_decoders::Decoder::from_file(&distorted).expect("open distorted");
        let selected = [0usize, 1, 2, 3];
        cases.push(
            score_probed_frames(
                &mut reference_decoder,
                &mut distorted_decoder,
                &butteraugli,
                &selected,
                OutputIndexing::Aligned,
                None,
                |_position, _score| {},
            )
            .expect("libvship should score butteraugli"),
        );
    }
    let near = cases[0].iter().sum::<f64>() / f64::from(cases[0].len() as u32);
    let far = cases[1].iter().sum::<f64>() / f64::from(cases[1].len() as u32);
    assert!(
        far > near,
        "a larger distortion must score worse: near={near} far={far}"
    );
    eprintln!("vship real Butteraugli distortion: near={near}, far={far}");
}

#[test]
fn vship_cvvdp_returns_scores_across_a_source_discontinuity() {
    if !av_metrics_vship::is_available() {
        return;
    }
    let reference = y4m_fixture(&y4m(&[10, 10, 10, 10]));
    let cvvdp = QualityMetric::CVVDP {
        target_range: (0.0, 1.0),
        resolution: None,
        display_model: None,
        resize_to_display: None,
        disable_temporal: None,
        gpu_id: None,
    };
    let distorted = y4m_fixture(&y4m(&[10, 10, 10, 10]));
    let mut reference_decoder =
        av_decoders::Decoder::from_file(&reference).expect("open reference");
    let mut distorted_decoder =
        av_decoders::Decoder::from_file(&distorted).expect("open distorted");
    let scores = score_probed_frames(
        &mut reference_decoder,
        &mut distorted_decoder,
        &cvvdp,
        &[0usize, 1, 3],
        OutputIndexing::Aligned,
        None,
        |_position, _score| {},
    )
    .expect("libvship should score cvvdp across a discontinuity");
    assert_eq!(scores.len(), 3, "one score per selected frame");
    eprintln!("vship real CVVDP source gap: three selected-frame scores verified");
}
