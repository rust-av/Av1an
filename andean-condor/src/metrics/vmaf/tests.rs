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
                |_| ScorerConfig::new(),
                |path| ScorerConfig::new().with_model_path(path),
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

#[cfg(unix)]
mod driver {
    //! Real decode/driver/scorer, with only the native boundary doubled.
    use std::{
        io::Write,
        process::{Command, Stdio},
        sync::atomic::Ordering,
        time::{Duration, Instant},
    };

    use super::*;

    const EXACT: &str = "metrics::vmaf::tests::driver::driver_events";

    #[test]
    fn vmaf_invalid_last_value_has_no_score_callbacks() {
        if let Ok(scenario) = std::env::var("VMAF_DRIVER_CHILD") {
            driver_child(&scenario);
            return;
        }
        let directory = tempfile::tempdir().expect("score-validation fixture");
        let library = directory.path().join(if cfg!(target_os = "macos") {
            "libvmaf.dylib"
        } else {
            "libvmaf.so"
        });
        let output = Command::new("cc")
            .args(["-shared", "-fPIC", "-std=c99", "-Wall", "-Wextra", "-Werror", "-o"])
            .arg(library)
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../av-metrics-vmaf/tests/fixtures/ownership.c"
            ))
            .output()
            .expect("cc");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        for bad in ["1", "2", "3"] {
            let output = Command::new(std::env::current_exe().expect("executable"))
                .args([
                    "--exact",
                    "metrics::vmaf::tests::driver::vmaf_invalid_last_value_has_no_score_callbacks",
                    "--nocapture",
                ])
                .env("VMAF_DRIVER_CHILD", "invalid-score-invalid")
                .env("VMAF_FINALIZATION_FIXTURE_BAD_SCORE", bad)
                .env("VMAF_FINALIZATION_FIXTURE_MODE", "1")
                .env(
                    "VMAF_FINALIZATION_FIXTURE_EVENTS",
                    directory.path().join(bad),
                )
                .env("VMAF_LIB_DIR", directory.path())
                .output()
                .expect("child");
            assert!(
                output.status.success()
                    && String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed"),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    #[test]
    fn driver_invalid_override_preserves_error_before_callbacks() {
        if std::env::var_os("VMAF_MODEL_DRIVER_CHILD").is_some() {
            driver_child("model_override");
            return;
        }
        let directory = tempfile::tempdir().expect("vmaf-model driver fixture");
        let mut child = Command::new(std::env::current_exe().expect("test executable"))
        .args([
            "--exact",
            "metrics::vmaf::tests::driver::driver_invalid_override_preserves_error_before_callbacks",
            "--nocapture",
        ])
        .env("VMAF_MODEL_DRIVER_CHILD", "1")
        .env("VMAF_MODEL_PATH", directory.path().join("missing models 雪"))
        .env("VMAF_FINALIZATION_FIXTURE_EVENTS", directory.path().join("events"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("isolated vmaf-model driver child");
        let start = Instant::now();
        while child.try_wait().expect("status").is_none() {
            if start.elapsed() > Duration::from_secs(30) {
                child.kill().expect("kill");
                child.wait().expect("reap");
                panic!("vmaf-model driver timeout");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = child.wait_with_output().expect("output");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("vmaf-model driver: {stdout}\n{stderr}");
        assert!(output.status.success(), "{stdout}\n{stderr}");
        assert!(stdout.contains("running 1 test") && stdout.contains("1 passed; 0 failed"));
    }

    #[test]
    fn driver_events() {
        if let Ok(scenario) = std::env::var("VMAF_DRIVER_CHILD") {
            driver_child(&scenario);
            return;
        }
        let directory = tempfile::tempdir().expect("vmaf-driver driver fixture");
        let library = directory.path().join(if cfg!(target_os = "macos") {
            "libvmaf.dylib"
        } else {
            "libvmaf.so"
        });
        let output = Command::new("cc")
            .args(["-shared", "-fPIC", "-std=c99", "-Wall", "-Wextra", "-Werror", "-o"])
            .arg(library)
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../av-metrics-vmaf/tests/fixtures/ownership.c"
            ))
            .output()
            .expect("vmaf-driver requires cc");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        for scenario in [
            "aligned",
            "compacted",
            "reject",
            "cancelled",
            "cancel_after",
            "flush_error",
            "query_error",
            "empty",
        ] {
            let mut child = Command::new(std::env::current_exe().expect("test executable"))
                .args(["--exact", EXACT, "--nocapture"])
                .env("VMAF_DRIVER_CHILD", scenario)
                .env(
                    "VMAF_FINALIZATION_FIXTURE_MODE",
                    if scenario == "query_error" { "2" } else { "1" },
                )
                .env(
                    "VMAF_FINALIZATION_FIXTURE_EVENTS",
                    directory.path().join(scenario),
                )
                .env(
                    "VMAF_OWNERSHIP_FIXTURE_REJECT_READ",
                    if scenario == "reject" { "1" } else { "0" },
                )
                .env(
                    "VMAF_OWNERSHIP_FIXTURE_FAIL_FLUSH",
                    if scenario == "flush_error" { "1" } else { "0" },
                )
                .env("VMAF_LIB_DIR", directory.path())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("child");
            let start = Instant::now();
            while child.try_wait().expect("status").is_none() {
                if start.elapsed() > Duration::from_secs(30) {
                    child.kill().expect("kill");
                    child.wait().expect("reap");
                    panic!("driver timeout");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            let output = child.wait_with_output().expect("output");
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("vmaf-driver driver {scenario}: {stdout}\n{stderr}");
            assert!(output.status.success(), "{stdout}\n{stderr}");
            assert!(stdout.contains("running 1 test") && stdout.contains("1 passed; 0 failed"));
        }
    }

    fn driver_child(scenario: &str) {
        let fixture = tempfile::Builder::new().suffix(".y4m").tempfile().expect("Y4M");
        let mut y4m = b"YUV4MPEG2 W8 H8 F24:1 Ip A1:1 C420jpeg\n".to_vec();
        for value in [40, 80, 120, 160] {
            y4m.extend_from_slice(b"FRAME\n");
            y4m.extend([value; 64]);
            y4m.extend([128; 32]);
        }
        std::fs::write(fixture.path(), y4m).expect("write");
        let mut reference = av_decoders::Decoder::from_file(fixture.path()).expect("reference");
        let mut distorted = av_decoders::Decoder::from_file(fixture.path()).expect("distorted");
        let event_path = std::path::PathBuf::from(
            std::env::var_os("VMAF_FINALIZATION_FIXTURE_EVENTS").expect("events"),
        );
        let events = || std::fs::read_to_string(&event_path).unwrap_or_default();
        let log = |name, position| {
            writeln!(
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&event_path)
                    .expect("open log"),
                "{name}{position}"
            )
            .expect("log callback");
        };
        let cancelled = AtomicBool::new(scenario == "cancelled");
        let selected: &[usize] = if scenario == "empty" { &[] } else { &[0, 2, 3] };
        let indexing = if scenario == "compacted" {
            OutputIndexing::Compacted
        } else {
            OutputIndexing::Aligned
        };
        let mut submitted = Vec::new();
        let mut reported = Vec::new();
        let scores = score_probed_frames_with_progress(
            &mut reference,
            &mut distorted,
            &tests::metric(vec![]),
            selected,
            indexing,
            Some(&cancelled),
            |position| {
                assert!(
                    !events().contains("flush"),
                    "submission before finalization"
                );
                submitted.push(position);
                log("progress", position);
                if scenario == "cancel_after" {
                    cancelled.store(true, Ordering::Relaxed);
                }
            },
            |position, score| {
                assert!(
                    events().contains("flush0"),
                    "real score only after finalization"
                );
                if scenario != "invalid-score-invalid" {
                    assert_eq!(score, 90.0 + position as f64);
                }
                reported.push((selected[position], score));
                log("score", position);
            },
        );
        match scenario {
            "invalid-score-invalid" => {
                let error = scores.expect_err("invalid final native value");
                assert!(format!("{error:#}").contains("non-finite"), "{error:#}");
                assert_eq!(submitted, [0, 1, 2]);
                assert!(
                    reported.is_empty(),
                    "ALL values validated before first score callback"
                );
            },
            "model_override" => {
                let error = scores.expect_err("invalid explicit directory must fail");
                let message = error.to_string();
                eprintln!(
                    "vmaf-model driver submitted={submitted:?}, reported={reported:?}, error={message}"
                );
                assert!(
                    submitted.is_empty() && reported.is_empty() && events().is_empty(),
                    "invalid override must fail before either progress or score callbacks"
                );
                let expected = std::path::PathBuf::from(
                    std::env::var_os("VMAF_MODEL_PATH").expect("override"),
                )
                .join("vmaf_v0.6.1.json");
                assert!(
                    message.contains("VMAF_MODEL_PATH")
                        && message.contains(&*expected.to_string_lossy())
                        && message.contains("missing"),
                    "production driver must retain the missing override diagnostic: {message}"
                );
                assert!(
                    matches!(
                        error.downcast_ref::<VmafError>(),
                        Some(VmafError::ModelNotFound { .. })
                    ),
                    "production driver must preserve the typed native cause: {error:?}"
                );
            },
            "aligned" | "compacted" => {
                assert_eq!(scores.expect("all scores"), [90.0, 91.0, 92.0]);
                assert_eq!(submitted, [0, 1, 2]);
                assert_eq!(reported, [(0, 90.0), (2, 91.0), (3, 92.0)]);
                let expected = if scenario == "compacted" {
                    "init0\nsubmit0\nreference40\ndistorted40\nprogress0\nsubmit1\nreference120\ndistorted80\nprogress1\nsubmit2\nreference160\ndistorted120\nprogress2\nflush0\nquery0\nquery1\nquery2\nscore0\nscore1\nscore2\n"
                } else {
                    "init0\nsubmit0\nreference40\ndistorted40\nprogress0\nsubmit1\nreference120\ndistorted120\nprogress1\nsubmit2\nreference160\ndistorted160\nprogress2\nflush0\nquery0\nquery1\nquery2\nscore0\nscore1\nscore2\n"
                };
                assert_eq!(
                    events(),
                    expected,
                    "native and callback order, no interim query"
                );
            },
            "empty" => {
                assert!(scores.expect("empty selection").is_empty());
                assert!(submitted.is_empty() && reported.is_empty() && events().is_empty());
            },
            _ => {
                let error = scores.expect_err("injected rejection/cancellation/native error");
                assert!(reported.is_empty(), "no fabricated/partial score callbacks");
                if matches!(scenario, "cancelled" | "cancel_after") {
                    assert!(error.is::<probe::Cancelled>(), "{error:?}");
                    assert_eq!(submitted.len(), usize::from(scenario == "cancel_after"));
                    assert!(!events().contains("flush"));
                } else {
                    let native =
                        error.downcast_ref::<VmafError>().expect("original Rust native error");
                    if scenario == "query_error" {
                        assert!(matches!(
                            native,
                            VmafError::ScoreQueryFailed {
                                index: 1,
                                status: -74
                            }
                        ));
                    } else {
                        assert!(
                            matches!(native, VmafError::CallFailed { function: "vmaf_read_pictures", status } if *status == if scenario == "reject" { -71 } else { -73 })
                        );
                    }
                    assert_eq!(submitted.len(), if scenario == "reject" { 0 } else { 3 });
                    if scenario == "flush_error" {
                        assert!(!events().contains("query"));
                    }
                }
            },
        }
        eprintln!("vmaf-driver driver {scenario} events:\n{}", events());
    }
}
