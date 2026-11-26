mod format_conversion;
#[cfg(target_os = "linux")]
pub(crate) mod indexing;
mod source_options;

use super::*;

// These checks need private output-node and append-buffer writers, not only
// Input's public cursor API, to prove padding and no-partial-write guarantees.
mod internal_integration {
    use super::*;

    fn input(script: &str) -> Input {
        Input::from_data(&InputModel::VapourSynthScript {
            source: VapourSynthScriptSource::Text(script.to_owned()),
            variables: HashMap::new(),
            index: 0,
            filters: Vec::new(),
            stream_concurrently: true,
        })
        .expect("frame-output requires the installed native VapourSynth runtime")
    }

    fn assert_writers(input: &mut Input, payloads: &[Vec<u8>], streaming: bool) {
        let expected: Vec<Vec<u8>> = payloads
            .iter()
            .map(|payload| [b"FRAME\n".as_slice(), payload].concat())
            .collect();
        if streaming {
            let node = output_node(input.decoder()).expect("output node");
            let (sender, receiver) = crossbeam_channel::bounded(payloads.len());
            // One request also keeps the pre-fix panic reproduction free of outstanding callbacks.
            y4m_frames(&node, &sender, &[0, 1], 1, &Default::default()).expect("stream frames");
            drop(sender);
            let actual: Vec<_> = receiver.iter().map(std::io::Cursor::into_inner).collect();
            assert_eq!(actual, expected);
        } else {
            let mut actual = b"retained output\n".to_vec();
            let node = output_node(input.decoder()).expect("output node");
            for index in 0..payloads.len() {
                y4m_frame(&node, index, &mut actual).expect("single frame");
            }
            assert_eq!(
                actual,
                [b"retained output\n".as_slice(), &expected.concat()].concat()
            );
        }
    }

    fn gray(bit_depth: u8, streaming: bool) {
        let (colors, payloads) = if bit_depth == 8 {
            ([37, 219], [vec![37; 54], vec![219; 54]])
        } else {
            (
                [4660, 43981],
                [0x1234_u16.to_ne_bytes().repeat(54), 0xabcd_u16.to_ne_bytes().repeat(54)],
            )
        };
        let mut input = input(&format!(
            "import vapoursynth as vs\nc = vs.core\n\
             a = c.std.BlankClip(width=18, height=3, length=1, format=vs.GRAY{bit_depth}, color=[{}])\n\
             b = c.std.BlankClip(a, color=[{}])\n(a + b).set_output()",
            colors[0], colors[1]
        ));
        {
            let node = output_node(input.decoder()).expect("gray output node");
            let frame = node.get_frame(0).expect("native gray frame");
            assert_eq!(frame.format().plane_count(), 1);
            let visible = 18 * usize::from(bit_depth / 8);
            eprintln!(
                "frame-output GRAY{bit_depth}: stride={}, visible={visible}",
                frame.stride(0)
            );
            if frame.stride(0) != visible {
                assert!(frame.data(0).is_err(), "padded plane must use visible rows");
            }
        }
        assert_writers(&mut input, &payloads, streaming);
    }

    #[test]
    fn vapoursynth_gray8_single_writer_appends_two_exact_frame_payloads() {
        gray(8, false);
    }

    #[test]
    fn vapoursynth_gray8_bulk_writer_emits_two_exact_frame_payloads() {
        gray(8, true);
    }

    #[test]
    fn vapoursynth_gray16_single_writer_appends_exact_native_samples() {
        gray(16, false);
    }

    #[test]
    fn vapoursynth_gray16_bulk_writer_emits_exact_native_samples() {
        gray(16, true);
    }

    #[test]
    fn padded_yuv_keeps_visible_planes_in_order() {
        let mut input = input(
            "import vapoursynth as vs\nc = vs.core\n\
             a = c.std.BlankClip(width=18, height=3, length=1, format=vs.YUV422P8, color=[17, 83, 201])\n\
             b = c.std.BlankClip(a, color=[231, 41, 159])\n(a + b).set_output()",
        );
        {
            let node = output_node(input.decoder()).expect("YUV output node");
            let frame = node.get_frame(0).expect("native YUV frame");
            assert_eq!(frame.format().plane_count(), 3);
            for (plane, visible) in [18, 9, 9].into_iter().enumerate() {
                eprintln!(
                    "frame-output YUV plane {plane}: stride={}, visible={visible}",
                    frame.stride(plane)
                );
                if frame.stride(plane) != visible {
                    assert!(
                        frame.data(plane).is_err(),
                        "padded plane must use visible rows"
                    );
                }
            }
        }
        let payloads = [
            [vec![17; 54], vec![83; 27], vec![201; 27]].concat(),
            [vec![231; 54], vec![41; 27], vec![159; 27]].concat(),
        ];
        assert_writers(&mut input, &payloads, false);
        assert_writers(&mut input, &payloads, true);
    }

    #[test]
    fn rgb_bulk_rejects_before_publication() {
        let decoder = VapoursynthDecoder::from_script(
            "import vapoursynth as vs\nvs.core.std.BlankClip(width=18, height=3, length=2, format=vs.RGB24, color=[17, 83, 201]).set_output()",
            HashMap::new(),
            None,
        ).expect("native RGB fixture");
        let node = decoder.get_output(0, None).expect("actual RGB node");
        assert_eq!(
            node.info().format.color_family(),
            vapoursynth::format::ColorFamily::RGB
        );
        let frame = node.get_frame(1).expect("actual RGB frame");
        let error = write_y4m_frame(&frame).expect_err("RGB planes cannot be serialized as YUV");
        assert!(
            matches!(
                error.downcast_ref::<DecoderError>(),
                Some(DecoderError::UnsupportedFormat { .. })
            ),
            "{error:#}"
        );
        assert!(
            error.to_string().contains("convert to integer Gray or YUV"),
            "{error:#}"
        );
        let (sender, receiver) = crossbeam_channel::bounded(2);
        let error = y4m_frames(&node, &sender, &[1, 0], 2, &Default::default())
            .expect_err("RGB is not YUV");
        assert!(
            matches!(
                error.downcast_ref::<DecoderError>(),
                Some(DecoderError::UnsupportedFormat { .. })
            ),
            "{error:#}"
        );
        assert!(error.to_string().contains("frame 1"), "{error:#}");
        assert!(
            receiver.try_recv().is_err(),
            "rejected RGB must publish no frame"
        );
    }

    #[test]
    fn vapoursynth_rgb_single_writer_preserves_existing_stream_bytes() {
        let mut input = input(
            "import vapoursynth as vs\nvs.core.std.BlankClip(width=18, height=3, length=2, format=vs.GRAY8).set_output()",
        );
        // Opening already rejects RGB. A live script replacement exercises the later frame guard
        // without disguising RGB planes as YUV: both the real node and frame report RGB.
        input.decoder().get_vapoursynth_impl().expect("VS decoder").env.eval_script(
            "vs.core.std.BlankClip(width=18, height=3, length=2, format=vs.RGB24, color=[17, 83, 201]).set_output()",
        ).expect("replace live output");
        {
            let node = output_node(input.decoder()).expect("live RGB output");
            assert_eq!(
                node.info().format.color_family(),
                vapoursynth::format::ColorFamily::RGB
            );
        }
        let mut stream = b"retained output\n".to_vec();
        let node = output_node(input.decoder()).expect("live RGB output");
        let error = y4m_frame(&node, 1, &mut stream).expect_err("reject live RGB output");
        assert!(
            matches!(
                error.downcast_ref::<DecoderError>(),
                Some(DecoderError::UnsupportedFormat { .. })
            ),
            "{error:#}"
        );
        assert!(error.to_string().contains("frame 1"), "{error:#}");
        assert_eq!(
            stream, b"retained output\n",
            "no stray FRAME header or planes"
        );
    }

    #[test]
    fn float_bulk_rejects_before_publication() {
        let decoder = VapoursynthDecoder::from_script(
            "import vapoursynth as vs\nvs.core.std.BlankClip(width=18, height=3, length=2, format=vs.YUV444PS, color=[0.25, 0.5, 0.75]).set_output()",
            HashMap::new(),
            None,
        ).expect("native float fixture");
        let node = decoder.get_output(0, None).expect("actual float node");
        let (sender, receiver) = crossbeam_channel::bounded(2);
        let error = y4m_frames(&node, &sender, &[1, 0], 2, &Default::default())
            .expect_err("float is not integer Y4M");
        assert!(
            matches!(
                error.downcast_ref::<DecoderError>(),
                Some(DecoderError::UnsupportedFormat { .. })
            ),
            "{error:#}"
        );
        assert!(
            receiver.try_recv().is_err(),
            "rejected float must publish no frame"
        );
    }
}
