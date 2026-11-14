//! Real decode/probe/scorer transport with doubles only at the native ABI.
//! Child processes isolate each library loader's OnceLock and environment.
#![cfg(unix)]

#[cfg(test)]
mod tests {
    use std::process::Command;

    use andean_condor::{
        metrics::{fmetrics, probe::OutputIndexing, vship},
        models::sequence::target_quality::types::QualityMetric,
    };

    #[test]
    fn m04_native_driver_metadata_transport() {
        if std::env::var_os("M04_NATIVE_CHILD").is_some() {
            check_drivers();
            return;
        }
        run_sink(
            "tests::m04_native_driver_metadata_transport",
            "M04_NATIVE_CHILD",
            false,
            "/tests/fixtures/metadata.c",
        );
    }

    #[test]
    fn m07_native_upload_storage_boundary() {
        if std::env::var_os("M07_NATIVE_CHILD").is_some() {
            check_uploads();
            return;
        }
        run_sink(
            "tests::m07_native_upload_storage_boundary",
            "M07_NATIVE_CHILD",
            true,
            "/tests/fixtures/metadata.c",
        );
    }

    #[test]
    fn m17_butteraugli_production_entry_rejects_infinity_and_nonfinite_results() {
        if std::env::var_os("M17_NATIVE_CHILD").is_some() {
            check_butteraugli();
            return;
        }
        run_sink(
            "tests::m17_butteraugli_production_entry_rejects_infinity_and_nonfinite_results",
            "M17_NATIVE_CHILD",
            false,
            "/../av-metrics-fmetrics/tests/fixtures/boundary.c",
        );
    }

    fn run_sink(test: &str, marker: &str, upload: bool, source: &str) {
        let directory = tempfile::tempdir().expect("temporary native ABI sink");
        let library = directory.path().join(if cfg!(target_os = "macos") {
            "libfmetrics.dylib"
        } else {
            "libfmetrics.so"
        });
        let output = Command::new("cc")
            .args(["-shared", "-fPIC", "-o"])
            .arg(&library)
            .arg(format!("{}{source}", env!("CARGO_MANIFEST_DIR")))
            .output()
            .expect("M04 ABI transport check requires a C compiler (cc)");
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
        .expect("copy libvship ABI sink");
        let mut child = Command::new(std::env::current_exe().expect("test executable"));
        if upload {
            child.env("M07_ABI_LOG", directory.path().join("uploads.log"));
        }
        if marker == "M17_NATIVE_CHILD" {
            child
                .env(
                    "FMETRICS_RESULT_FIXTURE_ABI_LOG",
                    directory.path().join("calls.log"),
                )
                .env(
                    "FMETRICS_RESULT_FIXTURE_RESULT_FILE",
                    directory.path().join("result"),
                )
                .env("FMETRICS_BOUNDARY_FIXTURE_VERSION", "0.1.0")
                .env("FMETRICS_BOUNDARY_FIXTURE_STATUS", "0");
        }
        let output = child
            .args(["--exact", test, "--nocapture"])
            .env(marker, "1")
            .env("FMETRICS_LIB_PATH", directory.path())
            .env("VSHIP_PLUGIN_PATH", directory.path())
            .output()
            .expect("run isolated transport test");
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed"));
        if marker == "M17_NATIVE_CHILD" {
            eprint!("{}", String::from_utf8_lossy(&output.stderr));
        }
    }

    #[expect(clippy::unwrap_used, reason = "fixture assertions should fail loudly")]
    fn check_uploads() {
        use av_decoders::v_frame::{chroma::ChromaSubsampling, frame::FrameBuilder};
        use av_metrics_vship::{PlaneSource, VideoFormat, VshipConfig, VshipScorer};
        let log = std::env::var_os("M07_ABI_LOG").expect("ABI log");
        let calls = || std::fs::read_to_string(&log).unwrap_or_default();
        let format = VideoFormat {
            width: 3,
            height: 3,
            bit_depth: 10,
            chroma_sampling: ChromaSubsampling::Yuv420,
            color_range: None,
            color_primaries: None,
            color_transfer_characteristic: None,
            color_space: None,
            chroma_location: None,
        };
        let mut scorer = VshipScorer::new(
            VshipConfig::new().with_gpu_id(0).with_handler_threads(2),
            format,
            format,
            None,
        )
        .expect("ABI scorer");
        let mut typed_scorer = VshipScorer::new(
            VshipConfig::new().with_gpu_id(0).with_handler_threads(1),
            VideoFormat {
                width: 4,
                height: 4,
                ..format
            },
            VideoFormat {
                width: 4,
                height: 4,
                ..format
            },
            None,
        )
        .unwrap();
        let frame = FrameBuilder::new(4, 4, ChromaSubsampling::Yuv420, 8).build::<u8>().unwrap();
        assert!(typed_scorer.submit_pair(&frame, &frame).is_err());
        assert_eq!(typed_scorer.submitted().unwrap(), 0);
        assert_eq!(
            calls(),
            "",
            "typed mismatch must not call ComputeHandler or Reset"
        );

        // Odd host origin, sample-aligned pitch and exact last-visible extent, with no
        // final trailing padding. First/last visible samples prove origin folding.
        let mut y = [0u8; 27];
        let mut u = [0u8; 13];
        let mut v = [0u8; 13];
        for (data, width, height, stride, value) in [
            (&mut y[..], 3, 3, 10, 100u16),
            (&mut u[..], 2, 2, 8, 140u16),
            (&mut v[..], 2, 2, 8, 160u16),
        ] {
            for row in 0..height {
                for column in 0..width {
                    let offset = 1 + row * stride + column * 2;
                    data[offset..offset + 2].copy_from_slice(&value.to_ne_bytes());
                }
            }
        }
        let planes = [
            PlaneSource::from_bytes(&y, 3, 3, 10, 1, 2).unwrap(),
            PlaneSource::from_bytes(&u, 2, 2, 8, 1, 2).unwrap(),
            // SAFETY: v is one live initialized allocation with exactly this
            // span, unchanged until all synchronous submissions below return.
            unsafe { PlaneSource::from_raw_parts(v.as_ptr(), v.len(), 2, 2, 8, 1, 2) }.unwrap(),
        ];
        // Every plane on BOTH sides must be checked before counters/native state.
        for side in 0..2 {
            for index in 0..3 {
                let mut invalid = planes;
                let p = planes[index];
                invalid[index] =
                    PlaneSource::from_bytes(&y, p.width(), p.height(), p.stride(), 1, 1).unwrap();
                let result = if side == 0 {
                    scorer.submit_pair_raw(invalid, planes)
                } else {
                    scorer.submit_pair_raw(planes, invalid)
                };
                let error = result.expect_err("raw storage mismatch").to_string();
                assert!(
                    error.contains(if side == 0 { "reference" } else { "distorted" }),
                    "{error}"
                );
                assert_eq!(scorer.submitted().unwrap(), 0);
                let result = if side == 0 {
                    scorer.submit_pair_raw_with_reset(invalid, planes, true)
                } else {
                    scorer.submit_pair_raw_with_reset(planes, invalid, true)
                };
                assert!(result.is_err());
                assert_eq!(scorer.submitted().unwrap(), 0);
                assert_eq!(
                    calls(),
                    "",
                    "raw mismatch must not reach native compute or reset"
                );
            }
        }
        assert_eq!(
            scorer.submit_pair_raw(planes, planes).unwrap().score,
            Some(42.0)
        );
        assert_eq!(calls(), "3 3 5 10 8 8 10 8 8\n");
        assert_eq!(scorer.submitted().unwrap(), 1);
        assert_eq!(
            scorer.submit_pair_raw_with_reset(planes, planes, true).unwrap().score,
            Some(42.0)
        );
        assert_eq!(
            calls(),
            "3 3 5 10 8 8 10 8 8\n3 3 5 10 8 8 10 8 8\n",
            "stateless raw submissions never reset native handlers"
        );

        // The real typed adapter also preserves padded u16 strides.
        let mut frame = FrameBuilder::new(4, 4, ChromaSubsampling::Yuv420, 10)
            .luma_padding_left(2)
            .luma_padding_right(4)
            .luma_padding_top(2)
            .luma_padding_bottom(2)
            .build::<u16>()
            .unwrap();
        assert!(
            frame.y_plane.geometry().data_origin() > 0,
            "typed fixture must actually have padding"
        );
        for (plane, value, origin, stride) in [
            (&mut frame.y_plane, 100, 44, 20),
            (frame.u_plane.as_mut().unwrap(), 140, 12, 10),
            (frame.v_plane.as_mut().unwrap(), 160, 12, 10),
        ] {
            let geometry = plane.geometry();
            let (width, height, pitch, start) = (
                geometry.width(),
                geometry.height(),
                geometry.stride(),
                geometry.data_origin(),
            );
            plane.data_mut().fill(1023);
            for row in 0..height {
                plane.data_mut()[start + row * pitch..start + row * pitch + width].fill(value);
            }
            let view = PlaneSource::from_plane(plane).unwrap();
            assert_eq!((view.data_origin(), view.stride()), (origin, stride));
            assert!(view.stride() as usize > view.width() * 2);
        }
        assert_eq!(
            typed_scorer.submit_pair(&frame, &frame).unwrap().score,
            Some(42.0)
        );
        assert!(
            calls().ends_with("4 4 5 20 10 10 20 10 10\n"),
            "{}",
            calls()
        );

        // No GPU double above the ABI: actual VS frames -> shared probe -> metric
        // adapter -> submit_pair_raw. Retain integer 8/10/12 metadata support.
        for (depth, sample) in [(8, 2), (10, 5), (12, 7)] {
            let script = format!(
                "import vapoursynth as vs\nvs.core.std.BlankClip(width=34, height=32, length=1, format=vs.YUV420P{depth}, color=[100,140,160]).set_output()"
            );
            let mut a = av_decoders::Decoder::from_script(&script, Default::default()).unwrap();
            let mut b = av_decoders::Decoder::from_script(&script, Default::default()).unwrap();
            let scores = vship::score_probed_frames(
                &mut a,
                &mut b,
                &QualityMetric::SSIMULACRA2 {
                    target_range: (0.0, 1.0),
                    resolution: None,
                    threads: Some(1),
                    gpu_id: Some(0),
                },
                &[0],
                OutputIndexing::Aligned,
                None,
                |_, _| {},
            )
            .unwrap();
            assert_eq!(scores, [42.0], "boundary marker, not metric accuracy");
            assert!(calls().lines().last().unwrap().starts_with(&format!("34 32 {sample} ")));
        }
        // Preserve the driver's existing external-gap reset decision, now inside
        // the same validated submission. This checks ABI ordering, not CVVDP accuracy.
        let script = "import vapoursynth as vs\nvs.core.std.BlankClip(width=34, height=32, length=3, format=vs.YUV420P10, color=[100,140,160]).set_output()";
        let mut a = av_decoders::Decoder::from_script(script, Default::default()).unwrap();
        let mut b = av_decoders::Decoder::from_script(script, Default::default()).unwrap();
        let before = calls().lines().filter(|line| *line == "reset").count();
        let scores = vship::score_probed_frames(
            &mut a,
            &mut b,
            &QualityMetric::CVVDP {
                target_range: (0.0, 1.0),
                resolution: None,
                display_model: None,
                resize_to_display: None,
                disable_temporal: None,
                gpu_id: Some(0),
            },
            &[0, 2],
            OutputIndexing::Aligned,
            None,
            |_, _| {},
        )
        .unwrap();
        assert_eq!(scores, [42.0, 42.0]);
        assert_eq!(
            calls().lines().filter(|line| *line == "reset").count(),
            before + 1
        );
    }

    fn decoder(properties: &str) -> av_decoders::Decoder {
        av_decoders::Decoder::from_script(&format!(
        "import vapoursynth as vs\nvs.core.std.BlankClip(width=32, height=32, length=1, format=vs.YUV420P8, color=[100,140,160]).std.SetFrameProps({properties}).set_output()"
    ), Default::default()).expect("open color-tagged fixture")
    }

    fn check_butteraugli() {
        let log = std::env::var_os("FMETRICS_RESULT_FIXTURE_ABI_LOG").expect("ABI log");
        let result_file =
            std::env::var_os("FMETRICS_RESULT_FIXTURE_RESULT_FILE").expect("ABI result");
        let calls = || std::fs::read_to_string(&log).unwrap_or_default();
        let mut metric = QualityMetric::BUTTERAUGLI {
            target_range: (1.0, 3.0),
            resolution: None,
            threads: Some(2),
            intensity_multiplier: Some(150.0),
            norm: None,
            gpu_id: None,
        };
        // Fresh native-file decoders exercise the CPU path without a plugin graph.
        let fixture = tempfile::Builder::new().suffix(".y4m").tempfile().expect("Y4M");
        let mut bytes = b"YUV4MPEG2 W32 H32 F24:1 Ip A1:1 C420jpeg\nFRAME\n".to_vec();
        bytes.extend_from_slice(&[100; 32 * 32]);
        bytes.extend_from_slice(&[128; 2 * 16 * 16]);
        std::fs::write(fixture.path(), bytes).expect("write fixture");
        let mut callbacks = Vec::new();
        let mut score = |metric: &QualityMetric| {
            let mut a = av_decoders::Decoder::from_file(fixture.path()).expect("fresh reference");
            let mut b = av_decoders::Decoder::from_file(fixture.path()).expect("fresh distorted");
            fmetrics::score_probed_frames(
                &mut a,
                &mut b,
                metric,
                &[0],
                OutputIndexing::Aligned,
                None,
                |position, value| callbacks.push((position, value)),
            )
        };
        std::fs::write(&result_file, "42").expect("finite result");
        let error = score(&metric).expect_err("infinity cannot be substituted by p3").to_string();
        assert!(
            error.contains("infinity") && error.contains("finite") && error.contains("plugin"),
            "{error}"
        );
        assert_eq!(calls(), "", "reject before workspace/scoring");
        assert!(!fmetrics::native_supported(&metric));
        if let QualityMetric::BUTTERAUGLI { norm, .. } = &mut metric {
            *norm = Some(3);
        }
        assert!(
            fmetrics::native_supported(&metric),
            "injected pinned library is usable"
        );
        assert_eq!(score(&metric).expect("explicit finite norm"), [42.0]);
        assert_eq!(calls(), "create\ncreate\nscore 3 150\ndestroy\ndestroy\n");
        if let QualityMetric::BUTTERAUGLI { norm, .. } = &mut metric {
            *norm = Some(u8::MAX);
        }
        for value in ["nan", "inf", "-inf"] {
            std::fs::write(&result_file, value).expect("injected nonfinite result");
            let error = score(&metric).expect_err("nonfinite must not reach callback").to_string();
            assert!(error.contains("non-finite"), "{error}");
        }
        std::fs::write(&result_file, "42").expect("finite recovery");
        assert_eq!(score(&metric).expect("valid followup"), [42.0]);
        assert_eq!(
            callbacks,
            [(0, 42.0), (0, 42.0)],
            "only successful finite results reach callback"
        );
        let log = calls();
        assert_eq!(log.lines().filter(|s| *s == "create").count(), 10);
        assert_eq!(log.lines().filter(|s| *s == "destroy").count(), 10);
        assert_eq!(log.lines().filter(|s| s.starts_with("score ")).count(), 5);
        assert_eq!(log.lines().filter(|s| *s == "score 255 150").count(), 4);
        eprintln!(
            "M17 driver: infinity allocations/calls/callbacks=0; finite callbacks=2; nonfinite rejects=3; workspaces=10/10 calls=5"
        );
    }

    fn check_drivers() {
        let ssimulacra2 = QualityMetric::SSIMULACRA2 {
            target_range: (74.0, 76.0),
            resolution: None,
            threads: Some(1),
            gpu_id: Some(0),
        };
        let cvvdp = QualityMetric::CVVDP {
            target_range: (40.0, 50.0),
            resolution: None,
            display_model: None,
            resize_to_display: None,
            disable_temporal: None,
            gpu_id: None,
        };
        for metric in [&ssimulacra2, &cvvdp] {
            let mut reference = decoder("_Range=1, _Matrix=6, _Primaries=1, _Transfer=8");
            let mut distorted = decoder("_Range=0, _Matrix=9, _Primaries=1, _Transfer=13");
            let scores = fmetrics::score_probed_frames(
                &mut reference,
                &mut distorted,
                metric,
                &[0],
                OutputIndexing::Aligned,
                None,
                |_, _| {},
            )
            .expect("independent RGB metadata reaches fmetrics ABI");
            assert_eq!(scores, [42.0]);
        }
        // M05: exercise native frame allocation spans, not only VS pointers.
        // Untagged Y4M retains the approved sRGB fallback:
        // limited Y=29 -> round((29-16)*255/219) = 15, with no transfer conversion.
        let fixture = tempfile::Builder::new().suffix(".y4m").tempfile().expect("fresh Y4M");
        let mut y4m = b"YUV4MPEG2 W34 H32 F24:1 Ip A1:1 C420jpeg\nFRAME\n".to_vec();
        y4m.extend_from_slice(&[29; 34 * 32]);
        y4m.extend_from_slice(&[128; 2 * 17 * 16]);
        std::fs::write(fixture.path(), y4m).expect("write native frame fixture");
        for metric in [&ssimulacra2, &cvvdp] {
            let mut reference =
                av_decoders::Decoder::from_file(fixture.path()).expect("native reference");
            let mut distorted =
                av_decoders::Decoder::from_file(fixture.path()).expect("native distorted");
            let scores = fmetrics::score_probed_frames(
                &mut reference,
                &mut distorted,
                metric,
                &[0],
                OutputIndexing::Aligned,
                None,
                |_, _| {},
            )
            .expect("native spans reach fmetrics ABI");
            assert_eq!(scores, [42.0]);
        }
        for transfer in [1, 6, 14, 15] {
            let properties = format!("_Range=1, _Matrix=1, _Primaries=1, _Transfer={transfer}");
            let script = format!(
                "import vapoursynth as vs\nvs.core.std.BlankClip(width=32, height=32, length=1, format=vs.YUV420P8, color=[16,128,128]).std.SetFrameProps({properties}).set_output()"
            );
            let mut reference = av_decoders::Decoder::from_script(&script, Default::default())
                .expect("tagged SDR reference");
            let mut distorted = av_decoders::Decoder::from_script(&script, Default::default())
                .expect("tagged SDR distorted");
            let scores = fmetrics::score_probed_frames(
                &mut reference,
                &mut distorted,
                &ssimulacra2,
                &[0],
                OutputIndexing::Aligned,
                None,
                |_, _| {},
            )
            .expect("tagged SDR reaches native scoring");
            assert_eq!(scores, [42.0]);
        }
        let mut reference =
            decoder("_Range=1, _Matrix=6, _Primaries=1, _Transfer=13, _ChromaLocation=1");
        let mut distorted =
            decoder("_Range=0, _Matrix=9, _Primaries=9, _Transfer=16, _ChromaLocation=2");
        let scores = vship::score_probed_frames(
            &mut reference,
            &mut distorted,
            &ssimulacra2,
            &[0],
            OutputIndexing::Aligned,
            None,
            |_, _| {},
        )
        .expect("independent HDR/SDR metadata reaches libvship ABI");
        assert_eq!(scores, [42.0]);
    }
}
