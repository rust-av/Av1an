use andean_condor::metrics::probe::{OutputIndexing, Side, probe_selected_frames};

#[test]
fn vapoursynth_probe_spans_end_at_the_last_visible_u16_sample() {
    let mut decoder = av_decoders::Decoder::from_script(
        "import vapoursynth as vs\nvs.core.std.BlankClip(width=6, height=4, length=1, format=vs.YUV420P10).set_output()",
        Default::default()).expect("VS span fixture");
    Side::open(&mut decoder, 10)
        .expect("VS side")
        .with_planes(0, |planes| {
            for plane in planes {
                assert_eq!(
                    plane.readable_len(),
                    (plane.height() - 1) * plane.stride() as usize + plane.width() * 2
                );
                assert_eq!(plane.bytes_per_sample(), 2);
                assert!(
                    plane.stride() as usize > plane.width() * 2,
                    "fixture must exercise row padding"
                );
            }
            Ok(())
        })
        .expect("borrow VS rows without trailing allocation guesses");
}

#[test]
fn probe_transports_independent_vapoursynth_color_properties() {
    let mut reference = av_decoders::Decoder::from_script(
        "import vapoursynth as vs\nvs.core.std.BlankClip(width=16, height=16, length=1, format=vs.YUV420P8).std.SetFrameProps(_Range=1, _Matrix=6, _Primaries=1, _Transfer=13).set_output()",
        Default::default()).expect("open reference script");
    let mut distorted = av_decoders::Decoder::from_script(
        "import vapoursynth as vs\nvs.core.std.BlankClip(width=16, height=16, length=1, format=vs.YUV420P8).std.SetFrameProps(_Range=0, _Matrix=9, _Primaries=9, _Transfer=16).set_output()",
        Default::default()).expect("open distorted script");
    probe_selected_frames::<(), _>(
        &mut reference,
        &mut distorted,
        &[0],
        OutputIndexing::Aligned,
        None,
        |reference, distorted| {
            let reference =
                av_metrics_vship::VideoFormat::from_details(&reference).colorspace(None)?;
            let distorted =
                av_metrics_vship::VideoFormat::from_details(&distorted).colorspace(None)?;
            assert_eq!(reference.range, av_metrics_vship::ffi::VshipRange::Full);
            assert_eq!(
                reference.yuv_matrix,
                av_metrics_vship::ffi::VshipYuvMatrix::St170M
            );
            assert_eq!(distorted.range, av_metrics_vship::ffi::VshipRange::Limited);
            assert_eq!(
                distorted.primaries,
                av_metrics_vship::ffi::VshipPrimaries::Bt2020
            );
            assert_eq!(
                distorted.transfer_function,
                av_metrics_vship::ffi::VshipTransferFunction::Pq
            );
            Ok(())
        },
        |_, _, _| Ok(()),
    )
    .expect("probe transports metadata before opening the planes");
}

#[test]
fn a_non_seekable_side_tracks_the_next_frame_across_reads() {
    let file = tempfile::Builder::new()
        .suffix(".y4m")
        .tempfile()
        .expect("create a y4m fixture");
    let path = file.into_temp_path();
    let mut bytes = b"YUV4MPEG2 W4 H4 F24:1 Ip A1:1 C420jpeg\n".to_vec();
    for luma in [10u8, 110, 210] {
        bytes.extend_from_slice(b"FRAME\n");
        bytes.extend_from_slice(&[luma; 16]);
        bytes.extend_from_slice(&[128; 4]);
        bytes.extend_from_slice(&[128; 4]);
    }
    std::fs::write(&path, &bytes).expect("write the y4m fixture");
    {
        let mut decoder = av_decoders::Decoder::from_file(&path).expect("open the y4m fixture");
        let mut side = Side::open(&mut decoder, 8).expect("open the fixture as a scoring side");
        assert!(!side.is_seekable(), "y4m must be detected as non-seekable");
        for target in 0..3usize {
            side.advance(target).expect("advance sequentially");
            side.with_planes(target, |_| Ok(())).expect("read the frame");
            let Side::Native { next, seekable, .. } = &side else {
                panic!("the fixture must open as a native side");
            };
            assert!(!*seekable, "y4m has no seek support");
            assert_eq!(
                *next,
                target + 1,
                "the tracked position must follow the frame just read"
            );
        }
    }
    path.close().expect("remove the y4m fixture");
}
