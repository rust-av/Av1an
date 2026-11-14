//! Real wrappers and decode/selection drivers; only native C ABI is doubled.
#![cfg(unix)]

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "fixture assertions fail loudly")]
mod tests {
    use std::{
        process::{Command, Stdio},
        time::{Duration, Instant},
    };

    use andean_condor::{
        metrics::{fmetrics, probe::OutputIndexing, vship},
        models::sequence::target_quality::types::QualityMetric,
    };
    use av_decoders::v_frame::{chroma::ChromaSubsampling, frame::FrameBuilder};
    use av_metrics_fmetrics::{ColorInfo, FmetricsConfig, FmetricsMetric, FmetricsScorer};
    use av_metrics_vship::{VideoFormat, VshipConfig, VshipMetric, VshipScorer};

    macro_rules! state_case {
        ($name:ident, [$($scope:expr),+ $(,)?]) => {
            #[test]
            fn $name() { state_cases(concat!("metrics::temporal::tests::",stringify!($name)), &[$($scope),+]); }
        };
    }

    state_case!(
        fmetrics_temporal_disable_resets_combined_history_and_score_each_frame,
        [("f", "")]
    );
    state_case!(
        vship_temporal_disable_resets_history_and_score_each_frame,
        [("v", "")]
    );
    state_case!(
        metric_drivers_preserve_temporal_history_and_source_gap_policy,
        [("drivers", "")]
    );
    state_case!(
        temporal_reset_failures_return_typed_errors_without_success_callbacks,
        [
            ("failures", "fr"),
            ("failures", "vr"),
            ("failures", "vs"),
            ("gap-failure", "vr")
        ]
    );

    fn state_cases(test: &str, scopes: &[(&str, &str)]) {
        if std::env::var_os("TEMPORAL_FIXTURE_CHILD").is_some() {
            let scope = std::env::var("TEMPORAL_FIXTURE_SCOPE").unwrap();
            let failure = std::env::var("TEMPORAL_FIXTURE_FAIL").unwrap();
            assert!(
                scopes.contains(&(scope.as_str(), failure.as_str())),
                "selected temporal scope belongs to this contract"
            );
            if std::env::var("TEMPORAL_FIXTURE_FAIL").unwrap().is_empty() {
                if std::env::var("TEMPORAL_FIXTURE_SCOPE").unwrap() == "drivers" {
                    drivers();
                } else {
                    wrappers();
                }
            } else {
                failures();
            }
            println!("temporal scope={scope} failure={failure:?}: assertions completed");
            return;
        }
        native_children(test, scopes, false);
    }

    macro_rules! fps_case {
        ($name:ident, $scope:literal, $phase:literal) => {
            #[test]
            fn $name() {
                fps_case(
                    concat!("metrics::temporal::tests::", stringify!($name)),
                    $scope,
                    $phase,
                );
            }
        };
    }

    fps_case!(
        cvvdp_invalid_frame_rates_fail_before_native_context_creation,
        "invalid-fps",
        "invalid"
    );
    fps_case!(
        cvvdp_native_constructors_receive_exact_configured_frame_rates,
        "fps",
        "constructors"
    );
    fps_case!(
        cvvdp_drivers_forward_the_source_frame_rate,
        "fps",
        "drivers"
    );
    fps_case!(
        stateless_metrics_do_not_initialize_temporal_frame_rate_state,
        "fps",
        "stateless"
    );

    fn fps_case(test: &str, scope: &str, phase: &str) {
        if std::env::var_os("TEMPORAL_FIXTURE_CHILD").is_some() {
            assert_eq!(std::env::var("TEMPORAL_FIXTURE_SCOPE").unwrap(), scope);
            fps(phase);
            println!("temporal scope={scope} failure=\"\": assertions completed");
            return;
        }
        native_children(test, &[(scope, "")], true);
    }

    fn native_children(test: &str, scopes: &[(&str, &str)], fps_audit: bool) {
        let directory = tempfile::tempdir().expect("native boundary fixture");
        let library = directory.path().join(if cfg!(target_os = "macos") {
            "libfmetrics.dylib"
        } else {
            "libfmetrics.so"
        });
        let output = Command::new("cc")
            .args(["-std=c11", "-Wall", "-Wextra", "-Werror", "-shared", "-fPIC", "-o"])
            .arg(&library)
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/temporal.c"
            ))
            .output()
            .expect("temporal requires cc");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        std::fs::copy(
            &library,
            directory.path().join(if cfg!(target_os = "macos") {
                "libvship.dylib"
            } else {
                "libvship.so"
            }),
        )
        .unwrap();
        let mut errors = Vec::new();
        for (scope, failure) in scopes {
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", test, "--nocapture"])
                .env("TEMPORAL_FIXTURE_CHILD", "1")
                .env("TEMPORAL_FIXTURE_FAIL", failure)
                .env("TEMPORAL_FIXTURE_SCOPE", scope)
                .env(
                    "FRAME_RATE_FIXTURE_FPS_AUDIT",
                    if fps_audit { "1" } else { "" },
                )
                .env("TEMPORAL_FIXTURE_LOG", directory.path().join("calls"))
                .env("FMETRICS_LIB_PATH", directory.path())
                .env("VSHIP_PLUGIN_PATH", directory.path())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("isolated boundary child");
            let start = Instant::now();
            while child.try_wait().unwrap().is_none() {
                if start.elapsed() > Duration::from_secs(120) {
                    child.kill().unwrap();
                    child.wait().unwrap();
                    panic!("native boundary child timed out: {scope}/{failure}");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            let output = child.wait_with_output().unwrap();
            if !output.status.success()
                || !String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed")
                || !String::from_utf8_lossy(&output.stdout).contains(&format!(
                    "temporal scope={scope} failure={failure:?}: assertions completed"
                ))
            {
                errors.push(format!(
                    "{scope}/{failure}: {}\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                ));
            } else if fps_audit {
                print!("{}", String::from_utf8_lossy(&output.stdout));
            } else {
                println!("temporal scope={scope} failure={failure:?}: assertions completed");
            }
        }
        assert!(errors.is_empty(), "{}", errors.join("\n"));
    }

    fn fps(phase: &str) {
        assert!(["invalid", "constructors", "drivers", "stateless"].contains(&phase));
        let source = decoder();
        let color = ColorInfo::from_details(source.get_video_details()).unwrap();
        let format = VideoFormat::from_details(source.get_video_details());
        if std::env::var("TEMPORAL_FIXTURE_SCOPE").unwrap() == "invalid-fps" {
            let mut errors = Vec::new();
            for rate in [
                0.0,
                -0.0,
                -1.0,
                f32::NAN,
                f32::INFINITY,
                f32::NEG_INFINITY,
                100_000.01,
                f32::MAX,
            ] {
                for disabled in [false, true] {
                    let mut direct =
                        FmetricsConfig::new(FmetricsMetric::Cvvdp).with_disable_temporal(disabled);
                    direct.frame_rate = rate;
                    for config in [
                        direct,
                        FmetricsConfig::new(FmetricsMetric::Cvvdp)
                            .with_frame_rate(rate)
                            .with_disable_temporal(disabled),
                        fmetrics::scorer_config(&metric(disabled), rate).unwrap(),
                    ] {
                        clear();
                        let result = FmetricsScorer::new(config, color);
                        let invalid = matches!(
                            result,
                            Err(av_metrics_fmetrics::FmetricsError::InvalidConfiguration { .. })
                        );
                        drop(result);
                        let log = calls();
                        if !invalid || !log.is_empty() {
                            errors.push(format!("f fps={rate:?} disabled={disabled}: InvalidConfiguration={invalid}, calls={log:?}"));
                        }
                    }
                    for config in [
                        VshipConfig::new()
                            .with_metric(VshipMetric::Cvvdp)
                            .with_fps(rate)
                            .with_disable_temporal(disabled),
                        vship::scorer_config(&metric(disabled), rate).unwrap(),
                    ] {
                        clear();
                        let result = VshipScorer::new(config, format, format, None);
                        let invalid = matches!(
                            result,
                            Err(av_metrics_vship::VshipError::InvalidConfiguration { .. })
                        );
                        drop(result);
                        let log = calls();
                        if !invalid || !log.is_empty() {
                            errors.push(format!("v fps={rate:?} disabled={disabled}: InvalidConfiguration={invalid}, calls={log:?}"));
                        }
                    }
                }
            }
            assert!(errors.is_empty(), "{}", errors.join("\n"));
            println!(
                "frame-rate: 80 invalid constructors; zero version/device/workspace/context/handler/upload calls"
            );
            return;
        }
        if phase == "constructors" {
            let frame =
                FrameBuilder::new(32, 32, ChromaSubsampling::Yuv420, 8).build::<u8>().unwrap();
            for disabled in [false, true] {
                // The ABI sink allocates only State, even at the inclusive maximum.
                for rate in [None, Some(24.0), Some(24_000.0 / 1001.0), Some(100_000.0)] {
                    let expected: f32 = rate.unwrap_or(60.0);
                    clear();
                    let mut config =
                        FmetricsConfig::new(FmetricsMetric::Cvvdp).with_disable_temporal(disabled);
                    if let Some(rate) = rate {
                        config = config.with_frame_rate(rate);
                    }
                    let mut scorer = FmetricsScorer::new(config, color).unwrap();
                    let planes = av_metrics_fmetrics::PlaneSet::from_frame(&frame).unwrap();
                    scorer.submit_pair_temporal(&planes, &planes).unwrap();
                    drop(scorer);
                    assert!(
                        calls().contains(&format!("ffps={:08x}\n", expected.to_bits())),
                        "{}",
                        calls()
                    );
                    clear();
                    let mut config = VshipConfig::new()
                        .with_metric(VshipMetric::Cvvdp)
                        .with_disable_temporal(disabled);
                    if let Some(rate) = rate {
                        config = config.with_fps(rate);
                    }
                    let mut scorer = VshipScorer::new(config, format, format, None).unwrap();
                    scorer.submit_pair(&frame, &frame).unwrap();
                    drop(scorer);
                    assert!(
                        calls().contains(&format!("vfps={:08x}\n", expected.to_bits())),
                        "{}",
                        calls()
                    );
                    println!(
                        "frame-rate: both native FPS={expected} bits={:08x} disabled={disabled}",
                        expected.to_bits()
                    );
                }
            }
        }
        if phase == "drivers" {
            // Real decoding and drivers must preserve the rational source rate too.
            for gpu in [false, true] {
                clear();
                let script = "import vapoursynth as vs\nvs.core.std.BlankClip(width=32, height=32, length=1, fpsnum=24000, fpsden=1001, format=vs.YUV420P8).std.SetFrameProps(_Matrix=1, _Transfer=13, _Primaries=1, _Range=1).set_output()";
                let mut a = av_decoders::Decoder::from_script(script, Default::default()).unwrap();
                let mut b = av_decoders::Decoder::from_script(script, Default::default()).unwrap();
                let scores = if gpu {
                    vship::score_probed_frames(
                        &mut a,
                        &mut b,
                        &metric(false),
                        &[0],
                        OutputIndexing::Aligned,
                        None,
                        |_, _| {},
                    )
                } else {
                    fmetrics::score_probed_frames(
                        &mut a,
                        &mut b,
                        &metric(false),
                        &[0],
                        OutputIndexing::Aligned,
                        None,
                        |_, _| {},
                    )
                }
                .unwrap();
                assert_eq!(scores.len(), 1);
                assert!(
                    calls().contains(if gpu {
                        "vfps=41bfcee6\n"
                    } else {
                        "ffps=41bfcee6\n"
                    }),
                    "{}",
                    calls()
                );
            }
        }
        if phase == "stateless" {
            for rate in [0.0, f32::NAN, f32::MAX] {
                for metric in [
                    FmetricsMetric::Ssimulacra2,
                    FmetricsMetric::Butteraugli,
                    FmetricsMetric::Iwssim,
                    FmetricsMetric::Msssim,
                ] {
                    clear();
                    drop(
                        FmetricsScorer::new(
                            FmetricsConfig::new(metric).with_threads(1).with_frame_rate(rate),
                            color,
                        )
                        .unwrap(),
                    );
                    assert!(calls().contains("fw+\n"));
                    assert!(!calls().contains("fc+\n"));
                }
                for metric in [VshipMetric::Ssimulacra2, VshipMetric::Butteraugli] {
                    clear();
                    drop(
                        VshipScorer::new(
                            VshipConfig::new()
                                .with_metric(metric)
                                .with_handler_threads(1)
                                .with_fps(rate),
                            format,
                            format,
                            None,
                        )
                        .unwrap(),
                    );
                    assert!(calls().contains("vc+\n"));
                    assert!(!calls().contains("vfps="));
                }
            }
        }
    }

    fn clear() {
        std::fs::write(std::env::var_os("TEMPORAL_FIXTURE_LOG").unwrap(), "").unwrap();
    }
    fn calls() -> String {
        std::fs::read_to_string(std::env::var_os("TEMPORAL_FIXTURE_LOG").unwrap()).unwrap()
    }
    fn metric(disabled: bool) -> QualityMetric {
        QualityMetric::CVVDP {
            target_range: (0.0, 1.0),
            resolution: None,
            display_model: None,
            resize_to_display: Some(false),
            disable_temporal: Some(disabled),
            gpu_id: Some(0),
        }
    }
    fn decoder() -> av_decoders::Decoder {
        av_decoders::Decoder::from_script(
            "import vapoursynth as vs\nvs.core.std.BlankClip(width=32, height=32, length=11, fpsnum=24, format=vs.YUV420P8, color=[64,128,128]).std.SetFrameProps(_Matrix=1, _Transfer=13, _Primaries=1, _Range=1).set_output()",
            Default::default()).unwrap()
    }

    fn wrappers() {
        let a = decoder();
        let details = a.get_video_details();
        let color = ColorInfo::from_details(details).unwrap();
        let format = VideoFormat::from_details(details);
        let frame = |luma| {
            let mut f =
                FrameBuilder::new(32, 32, ChromaSubsampling::Yuv420, 8).build::<u8>().unwrap();
            f.y_plane.data_mut().fill(luma);
            f.u_plane.as_mut().unwrap().data_mut().fill(128);
            f.v_plane.as_mut().unwrap().data_mut().fill(128);
            f
        };
        let frames = [frame(64), frame(96)];
        let malformed =
            FrameBuilder::new(16, 32, ChromaSubsampling::Yuv420, 8).build::<u8>().unwrap();
        for disabled in [false, true] {
            for gap in [false, true] {
                if std::env::var("TEMPORAL_FIXTURE_SCOPE").unwrap() == "f" {
                    clear();
                    let config = FmetricsConfig::new(FmetricsMetric::Cvvdp)
                        .with_frame_rate(24.0)
                        .with_reset_on_discontinuity(gap)
                        .with_disable_temporal(disabled);
                    let run = |indices: &[usize]| {
                        let mut scorer = FmetricsScorer::new(config.clone(), color).unwrap();
                        indices
                            .iter()
                            .map(|&i| {
                                let p =
                                    av_metrics_fmetrics::PlaneSet::from_frame(&frames[i]).unwrap();
                                let bad =
                                    av_metrics_fmetrics::PlaneSet::from_frame(&malformed).unwrap();
                                let before = calls();
                                assert!(scorer.submit_pair_temporal(&p, &bad).is_err());
                                assert!(scorer.submit_pair_temporal(&bad, &p).is_err());
                                assert_eq!(
                                    calls(),
                                    before,
                                    "invalid pair must not create/compute/reset"
                                );
                                scorer.submit_pair_temporal(&p, &p).unwrap()
                            })
                            .collect::<Vec<_>>()
                    };
                    let pair = run(&[0, 1]);
                    let fresh = [run(&[0])[0], run(&[1])[0]];
                    assert_eq!(pair[0].cvvdp_jod, fresh[0].cvvdp_jod);
                    assert_eq!(pair[1].cvvdp_jod == fresh[1].cvvdp_jod, disabled);
                    assert_eq!(pair[1].cvvdp_quality == fresh[1].cvvdp_quality, disabled);
                    assert_eq!(
                        calls(),
                        if disabled {
                            "fc+\nfp\nfr\nfp\nfr\nfc-\nfc+\nfp\nfr\nfc-\nfc+\nfp\nfr\nfc-\n"
                        } else {
                            "fc+\nfp\nfp\nfc-\nfc+\nfp\nfc-\nfc+\nfp\nfc-\n"
                        }
                    );
                } else {
                    clear();
                    let config = VshipConfig::new()
                        .with_metric(VshipMetric::Cvvdp)
                        .with_gpu_id(0)
                        .with_fps(24.0)
                        .with_reset_on_discontinuity(gap)
                        .with_disable_temporal(disabled);
                    let run = |indices: &[usize]| {
                        let mut scorer =
                            VshipScorer::new(config.clone(), format, format, None).unwrap();
                        indices
                            .iter()
                            .map(|&i| {
                                let before = calls();
                                let submitted = scorer.submitted().unwrap();
                                assert!(scorer.submit_pair(&frames[i], &malformed).is_err());
                                assert!(scorer.submit_pair(&malformed, &frames[i]).is_err());
                                fn raw(
                                    f: &av_decoders::v_frame::frame::Frame<u8>,
                                ) -> av_metrics_vship::PlaneSet<'_>
                                {
                                    [
                                        av_metrics_vship::PlaneSource::from_plane(&f.y_plane)
                                            .unwrap(),
                                        av_metrics_vship::PlaneSource::from_plane(
                                            f.u_plane.as_ref().unwrap(),
                                        )
                                        .unwrap(),
                                        av_metrics_vship::PlaneSource::from_plane(
                                            f.v_plane.as_ref().unwrap(),
                                        )
                                        .unwrap(),
                                    ]
                                }
                                assert!(
                                    scorer
                                        .submit_pair_raw_with_reset(
                                            raw(&frames[i]),
                                            raw(&malformed),
                                            true
                                        )
                                        .is_err()
                                );
                                assert!(
                                    scorer
                                        .submit_pair_raw_with_reset(
                                            raw(&malformed),
                                            raw(&frames[i]),
                                            true
                                        )
                                        .is_err()
                                );
                                assert_eq!(scorer.submitted().unwrap(), submitted);
                                assert_eq!(calls(), before, "invalid pair must not compute/reset");
                                scorer.submit_pair(&frames[i], &frames[i]).unwrap().score.unwrap()
                            })
                            .collect::<Vec<_>>()
                    };
                    let pair = run(&[0, 1]);
                    let fresh = [run(&[0])[0], run(&[1])[0]];
                    assert_eq!(pair[0], fresh[0]);
                    assert_eq!(pair[1] == fresh[1], disabled);
                    assert_eq!(
                        calls(),
                        if disabled {
                            "vc+\nvp\nvr\nvs\nvp\nvr\nvs\nvc-\nvc+\nvp\nvr\nvs\nvc-\nvc+\nvp\nvr\nvs\nvc-\n"
                        } else {
                            "vc+\nvp\nvp\nvc-\nvc+\nvp\nvc-\nvc+\nvp\nvc-\n"
                        }
                    );
                }
            }
        }
    }

    fn drivers() {
        for disabled in [false, true] {
            for selected in [&[0, 1, 2][..], &[0, 1, 10][..]] {
                clear();
                let mut a = decoder();
                let mut b = decoder();
                let mut emitted = Vec::new();
                let result = fmetrics::score_probed_frames(
                    &mut a,
                    &mut b,
                    &metric(disabled),
                    selected,
                    OutputIndexing::Aligned,
                    None,
                    |i, s| emitted.push((i, s)),
                );
                if !disabled && selected[2] == 10 {
                    let error = result.unwrap_err().to_string();
                    assert!(
                        error.contains("history-only") && error.contains("disable_temporal"),
                        "{error}"
                    );
                    assert!(emitted.is_empty());
                    assert_eq!(calls(), "");
                } else {
                    let scores = result.unwrap();
                    assert_eq!(emitted.len(), 3);
                    assert_eq!(scores[0] == scores[1], disabled);
                    assert_eq!(
                        calls(),
                        if disabled {
                            "fc+\nfp\nfr\nfp\nfr\nfp\nfr\nfc-\n"
                        } else {
                            "fc+\nfp\nfp\nfp\nfc-\n"
                        }
                    );
                }
                clear();
                let mut a = decoder();
                let mut b = decoder();
                let mut emitted = Vec::new();
                let scores = vship::score_probed_frames(
                    &mut a,
                    &mut b,
                    &metric(disabled),
                    selected,
                    OutputIndexing::Aligned,
                    None,
                    |i, s| emitted.push((i, s)),
                )
                .unwrap();
                assert_eq!(emitted.len(), 3);
                assert_eq!(scores[0] == scores[1], disabled);
                assert_eq!(
                    calls(),
                    if disabled {
                        "vc+\nvp\nvr\nvs\nvp\nvr\nvs\nvp\nvr\nvs\nvc-\n"
                    } else if selected[2] == 10 {
                        "vc+\nvp\nvp\nvr\nvp\nvc-\n"
                    } else {
                        "vc+\nvp\nvp\nvp\nvc-\n"
                    }
                );
                if !disabled && selected[2] == 10 {
                    assert_eq!(
                        scores,
                        [64.0, 64704.0, 1344.0],
                        "gap clears history, NOT accumulated score"
                    );
                }
            }
        }
        clear();
        let mut a = decoder();
        let mut b = decoder();
        let scores = fmetrics::score_probed_frames(
            &mut a,
            &mut b,
            &QualityMetric::SSIMULACRA2 {
                target_range: (0.0, 100.0),
                resolution: None,
                threads: Some(1),
                gpu_id: None,
            },
            &[0, 1, 10],
            OutputIndexing::Aligned,
            None,
            |_, _| {},
        )
        .unwrap();
        assert_eq!(scores, [42.0; 3], "non-CVVDP selections remain supported");
        assert_eq!(calls(), "", "no CVVDP calls for stateless metrics");
    }

    fn failures() {
        let failure = std::env::var("TEMPORAL_FIXTURE_FAIL").unwrap();
        clear();
        let mut a = decoder();
        let mut b = decoder();
        let mut emitted = 0;
        let gap = std::env::var("TEMPORAL_FIXTURE_SCOPE").unwrap() == "gap-failure";
        let result = if failure == "fr" {
            fmetrics::score_probed_frames(
                &mut a,
                &mut b,
                &metric(true),
                &[0, 1, 10],
                OutputIndexing::Aligned,
                None,
                |_, _| emitted += 1,
            )
        } else {
            vship::score_probed_frames(
                &mut a,
                &mut b,
                &metric(!gap),
                &[0, 1, 10],
                OutputIndexing::Aligned,
                None,
                |_, _| emitted += 1,
            )
        };
        let error = result.unwrap_err();
        if failure == "fr" {
            assert!(matches!(
                error.downcast_ref::<av_metrics_fmetrics::FmetricsError>(),
                Some(av_metrics_fmetrics::FmetricsError::CallFailed { code: 6, .. })
            ));
        } else {
            assert!(matches!(
                error.downcast_ref::<av_metrics_vship::VshipError>(),
                Some(av_metrics_vship::VshipError::CallFailed { status: 1, .. })
            ));
        }
        let message = error.to_string();
        assert!(
            message.contains(if failure == "fr" {
                "fmetrics_cvvdp_reset"
            } else if failure == "vr" {
                "Vship_Reset"
            } else {
                "Vship_ResetScore"
            }),
            "{message}"
        );
        assert_eq!(
            emitted,
            if gap { 2 } else { 0 },
            "failed reset cannot emit success"
        );
        assert_eq!(
            calls(),
            if gap {
                "vc+\nvp\nvp\nvr\nvc-\n"
            } else {
                match failure.as_str() {
                    "fr" => "fc+\nfp\nfr\nfc-\n",
                    "vr" => "vc+\nvp\nvr\nvc-\n",
                    _ => "vc+\nvp\nvr\nvs\nvc-\n",
                }
            }
        );
    }
}
