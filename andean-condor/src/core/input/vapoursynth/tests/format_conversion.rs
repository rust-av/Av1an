mod internal_integration {
    use super::super::*;
    use crate::core::input::Ffms2Filter;

    // Only the private node-less builder can distinguish deferred format
    // validation from invalid output fields rejected during script generation.
    #[test]
    fn generated_filters_defer_source_format_validation_until_a_node_exists() {
        for (preset, bit_depth, width, height, message) in [
            (
                "RGB24",
                Some(10),
                None,
                None,
                "Unsupported FFMS2 source format",
            ),
            (
                "YUV440P8",
                None,
                Some(16),
                None,
                "Unsupported FFMS2 source format",
            ),
            (
                "YUV420PS",
                Some(10),
                None,
                None,
                "Unsupported FFMS2 source format",
            ),
            (
                "YUV420P16",
                Some(10),
                None,
                None,
                "Unsupported FFMS2 source format",
            ),
            ("YUV420P8", Some(16), None, None, "Unsupported bit depth"),
            (
                "YUV420P8",
                None,
                Some(0),
                None,
                "must be positive and fit i32",
            ),
            (
                "YUV420P8",
                None,
                None,
                Some(u32::MAX),
                "must be positive and fit i32",
            ),
        ] {
            let native = Ffms2Filter::OutputFormat {
                bit_depth,
                chroma: None,
                width,
                height,
            };
            let filter = native.to_vapoursynth().remove(0);
            let source = format!(
                "from vapoursynth import core, {preset}\nclip = core.std.BlankClip(width=32, height=16, format={preset})\n"
            );
            let data = InputModel::VapourSynthScript {
                source: VapourSynthScriptSource::Text(format!("{source}clip.set_output()\n")),
                variables: HashMap::new(),
                index: 0,
                filters: vec![filter.clone()],
                stream_concurrently: true,
            };
            let error = Input::from_data(&data).err().expect("direct runtime error").to_string();
            assert!(error.contains(message), "direct {preset}: {error}");
            let mut script = VapourSynthScript::default();
            let generated = add_source_and_filters(
                &mut script,
                None,
                vec![Line::Term(source)],
                std::iter::once(&filter),
            );
            if generated.is_ok() {
                script.outputs.insert(0, "clip".to_owned());
                let data = InputModel::VapourSynthScript {
                    source: VapourSynthScriptSource::Text(script.to_string()),
                    variables: HashMap::new(),
                    index: 0,
                    filters: vec![],
                    stream_concurrently: true,
                };
                let error =
                    Input::from_data(&data).err().expect("script runtime error").to_string();
                assert!(error.contains(message), "scripted {preset}: {error}");
            } else {
                assert!(
                    matches!(preset, "YUV420P8"),
                    "supported fields must defer validation until a node exists"
                );
                assert!(generated.expect_err("invalid fields").to_string().contains(message));
            }
        }
    }
}
