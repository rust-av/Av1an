use super::*;

#[test]
fn model_errors_preserve_diagnostics_and_typed_cause() {
    let model = "/explicit models 雪/vmaf_v0.6.1.json";
    for reason in [
        "VMAF_MODEL_PATH explicitly selected this file, but it is missing",
        "VMAF_MODEL_PATH override failed in vmaf_model_load_from_path with status -87",
    ] {
        let error = describe_error(VmafError::ModelNotFound {
            model: model.to_owned(),
            reason: reason.to_owned(),
        });
        let message = error.to_string();
        eprintln!("vmaf-model application model error: {message}");
        assert!(
            message.contains(model) && message.contains(reason),
            "retain selected file and full missing/corrupt override diagnostic: {message}"
        );
        assert!(
            !message.contains("pass an explicit model path instead"),
            "do not replace an intentional override failure with built-in advice: {message}"
        );
        assert!(
            matches!(error.downcast_ref::<VmafError>(), Some(VmafError::ModelNotFound { model: actual_model, reason: actual_reason }) if actual_model == model && actual_reason == reason),
            "retain the original typed model error: {error:?}"
        );
    }
}

// Direct metric_planes descriptor identity is private adapter evidence and
// must survive separately from public driver callback contracts.
mod internal_integration {
    use super::*;

    #[test]
    fn probe_adapter_retains_real_spans_and_submits_to_cpu_vmaf() {
        if !av_metrics_vmaf::is_available() {
            assert!(
                std::env::var_os("AV_METRICS_REQUIRE_CPU").is_none(),
                "required CPU-native check needs libvmaf"
            );
            eprintln!("skipping: real VMAF probe integration needs libvmaf");
            return;
        }
        // Fresh decoders exercise both native v_frame and VS's padded visible span.
        for depth in [8, 10] {
            let fixture = tempfile::Builder::new().suffix(".y4m").tempfile().expect("fresh Y4M");
            let mut y4m = format!(
                "YUV4MPEG2 W66 H48 F24:1 Ip A1:1 C{}\nFRAME\n",
                if depth == 8 { "420jpeg" } else { "420p10" }
            )
            .into_bytes();
            if depth == 8 {
                y4m.extend_from_slice(&[100; 66 * 48]);
                y4m.extend_from_slice(&[128; 2 * 33 * 24]);
            } else {
                y4m.extend([400u16; 66 * 48].into_iter().flat_map(u16::to_le_bytes));
                y4m.extend([512u16; 2 * 33 * 24].into_iter().flat_map(u16::to_le_bytes));
            }
            std::fs::write(fixture.path(), y4m).expect("write native fixture");
            for native in [true, false] {
                let script = format!(
                    "import vapoursynth as vs\nvs.core.std.BlankClip(width=66, height=48, length=1, format=vs.YUV420P{depth}).set_output()"
                );
                let open = || {
                    if native {
                        av_decoders::Decoder::from_file(fixture.path()).expect("native decoder")
                    } else {
                        av_decoders::Decoder::from_script(&script, Default::default())
                            .expect("VS decoder")
                    }
                };
                let mut reference = open();
                let mut distorted = open();
                let config = std::env::var("VMAF_TEST_MODEL").map_or_else(
                    |_| ScorerConfig::default(),
                    |path| ScorerConfig::default().with_model(VmafModel::Path(path.into())),
                );
                let mut scorer = VmafScorer::new(
                    config.with_backend(BackendPreference::CpuOnly),
                    VideoFormat {
                        width: 66,
                        height: 48,
                        bit_depth: depth,
                        chroma_sampling: av_decoders::v_frame::chroma::ChromaSubsampling::Yuv420,
                    },
                )
                .expect("real CPU VMAF scorer");
                let indices = probe::probe_selected_frames::<(), _>(
                    &mut reference,
                    &mut distorted,
                    &[0],
                    OutputIndexing::Aligned,
                    None,
                    probe::check_geometry,
                    |_, reference, distorted| {
                        if !native {
                            assert!(
                                reference.iter().all(|plane| plane.stride() as usize
                                    > plane.width() * plane.bytes_per_sample()),
                                "VS fixture must have row padding"
                            );
                        }
                        let expected = reference.each_ref().map(|plane| {
                            (
                                plane.data_ptr(),
                                plane.readable_len(),
                                plane.width(),
                                plane.height(),
                                plane.stride(),
                                plane.data_origin(),
                                plane.bytes_per_sample(),
                            )
                        });
                        let reference = metric_planes(reference)?;
                        for (plane, expected) in reference.iter().zip(expected) {
                            assert_eq!(
                                (
                                    plane.data_ptr(),
                                    plane.readable_len(),
                                    plane.width(),
                                    plane.height(),
                                    plane.stride(),
                                    plane.data_origin(),
                                    plane.bytes_per_sample()
                                ),
                                expected
                            );
                        }
                        scorer
                            .submit_pair_raw(reference, metric_planes(distorted)?)
                            .map_err(describe_error)
                    },
                )
                .expect("decode/probe/checked VMAF submission");
                assert_eq!(indices, [0]);
                let scores = scorer.finish().expect("real VMAF score");
                assert_eq!(scores.len(), 1);
                assert!(scores[0].is_finite());
            }
        }
    }
}

/// A VMAF metric configuration.
///
/// Every field is spelled out rather than derived from another value,
/// because `QualityMetric` is an enum and so cannot use struct-update
/// syntax.
pub(super) fn metric(features: Vec<VmafFeature>) -> QualityMetric {
    QualityMetric::VMAF {
        target_range: (94.0, 96.0),
        resolution: None,
        scaler: String::new(),
        threads: 1,
        model: None,
        features,
    }
}

#[test]
fn the_default_metric_selects_the_default_model() {
    let config = scorer_config(&metric(Vec::new())).expect("config should build");
    assert_eq!(config.model, VmafModel::Default);
}

/// The test helper configures a single thread, which must be honoured
/// verbatim so a capped pool stays capped.
#[test]
fn vmaf_config_preserves_explicit_thread_count() {
    let config = scorer_config(&metric(Vec::new())).expect("config should build");
    assert_eq!(config.n_threads, 1);
}

#[test]
fn zero_threads_expands_to_one_worker_per_available_core() {
    let configured = QualityMetric::VMAF {
        target_range: (94.0, 96.0),
        resolution: None,
        scaler: String::new(),
        threads: 0,
        model: None,
        features: vec![],
    };
    let config = scorer_config(&configured).expect("config should build");
    let available = std::thread::available_parallelism().map_or(1, |p| p.get());
    assert_eq!(
        config.n_threads,
        u32::try_from(available).unwrap_or(u32::MAX),
        "threads: 0 must expand to one worker per available core"
    );
    assert_ne!(
        config.n_threads, 0,
        "passing 0 through would give libvmaf no thread pool"
    );
}

#[test]
fn feature_selectors_pick_the_matching_stock_model() {
    for (feature, expected) in [
        (VmafFeature::Neg, VmafModel::Neg),
        (VmafFeature::Uhd, VmafModel::Uhd),
        (VmafFeature::Weighted, VmafModel::Weighted),
    ] {
        let config = scorer_config(&metric(vec![feature])).expect("config should build");
        assert_eq!(config.model, expected, "{feature} should select {expected}");
    }
}

/// `uhd` and `neg` each name a different model, and together they name a
/// third: the 4K model trained for negative content. Reporting the plain
/// 4K model would silently drop the `neg` half of the request.
#[test]
fn the_uhd_and_neg_features_select_the_negative_4k_model() {
    let config = scorer_config(&metric(vec![VmafFeature::Uhd, VmafFeature::Neg]))
        .expect("config should build");
    assert_eq!(
        config.model,
        VmafModel::UhdNeg,
        "uhd + neg must select vmaf_4k_v0.6.1neg"
    );
    assert_eq!(
        config.model.as_libvmaf_model().expect("a stock model name"),
        "vmaf_4k_v0.6.1neg"
    );

    // The order the features are listed in must not matter.
    let reversed = scorer_config(&metric(vec![VmafFeature::Neg, VmafFeature::Uhd]))
        .expect("config should build");
    assert_eq!(reversed.model, VmafModel::UhdNeg);
}

#[test]
fn an_explicit_model_path_wins_over_the_feature_selectors() {
    let configured = QualityMetric::VMAF {
        target_range: (94.0, 96.0),
        resolution: None,
        scaler: String::new(),
        threads: 1,
        model: Some(std::path::PathBuf::from("/nonexistent/model.json")),
        features: vec![VmafFeature::Neg],
    };
    let config = scorer_config(&configured).expect("config should build");
    assert!(
        matches!(config.model, VmafModel::Path(_)),
        "an explicit path must take precedence over the feature selectors"
    );
}

#[test]
fn motionless_is_rejected_rather_than_silently_ignored() {
    let error = scorer_config(&metric(vec![VmafFeature::Motionless]))
        .expect_err("motionless has no libvmaf equivalent");
    assert!(
        error.to_string().contains("motionless"),
        "the error should name the offending feature, got: {error}"
    );
}

#[test]
fn a_non_vmaf_metric_is_rejected() {
    let other = QualityMetric::SSIMULACRA2 {
        target_range: (74.0, 76.0),
        resolution: None,
        threads: None,
        gpu_id: None,
    };
    assert!(scorer_config(&other).is_err());
}
