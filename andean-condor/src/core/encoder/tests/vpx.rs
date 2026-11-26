use super::*;

#[test]
fn vpx_color_argv_respects_codec_metadata_and_native_overrides() {
    use av_decoders::ColorSpace;
    for codec in [Some("vp8"), Some("vp9"), None] {
        for append in [true, false] {
            for matrix in [Some(ColorSpace::BT709), None, Some(ColorSpace::Unspecified)] {
                for native_override in [false, true] {
                    let mut encoder = Encoder::default_from_base(&EncoderBase::VPX, false);
                    *encoder.passes_mut().expect("passes") = EncoderPasses::All(1);
                    encoder.set_append_color_data(append);
                    encoder.parameters_mut().remove("codec");
                    if let Some(codec) = codec {
                        encoder
                            .parameters_mut()
                            .insert("codec".into(), CLIParameter::new_string("--", "=", codec));
                    }
                    if native_override {
                        encoder.parameters_mut().insert(
                            "color-space".into(),
                            CLIParameter::new_string("--", "=", "smpte240"),
                        );
                    }
                    let before = encoder.clone();
                    let mut clip = crate::core::encoder::tests::clip_info();
                    clip.color_space = matrix;
                    let expected = if native_override {
                        vec!["--color-space=smpte240"]
                    } else if append && codec != Some("vp8") && matrix == Some(ColorSpace::BT709) {
                        vec!["--color-space=bt709"]
                    } else {
                        vec![]
                    };
                    for argv in [
                        encoder.parameters_with_output((1, 1), Path::new("out.ivf"), &clip, None),
                        encoder.parameters_with_output(
                            (1, 1),
                            Path::new("out.ivf"),
                            &clip,
                            Some(Path::new("stats")),
                        ),
                    ] {
                        let argv = argv.expect("production argv");
                        let color: Vec<_> = argv
                            .iter()
                            .filter(|arg| arg.starts_with("--color-space"))
                            .map(String::as_str)
                            .collect();
                        assert_eq!(
                            color, expected,
                            "codec={codec:?} append={append} matrix={matrix:?} native={native_override}: {argv:?}"
                        );
                    }
                    assert_eq!(encoder, before, "argv must not sanitize caller options");
                }
            }
        }
    }
}
