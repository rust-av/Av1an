use andean_condor::{
    models::input::{ChromaSampling, Ffms2Filter, ImportMethod, Input, VapourSynthImportMethod},
    vapoursynth::vapoursynth_filters::VapourSynthFilter,
};

#[test]
fn filter_model_conversion_performs_no_source_or_index_io() {
    let directory = tempfile::tempdir().expect("absent source fixture");
    let path = directory.path().join("must-not-exist.mkv");
    let filters = vec![
        Ffms2Filter::OutputFormat {
            bit_depth: Some(10),
            chroma: None,
            width: None,
            height: None,
        },
        Ffms2Filter::OutputFormat {
            bit_depth: None,
            chroma: Some(ChromaSampling::Yuv444),
            width: Some(16),
            height: None,
        },
    ];
    let mut native = Input::Video {
        path: path.clone(),
        import_method: ImportMethod::FFMS2 {
            index: None,
            cache_path: None,
        },
        filters,
    };
    let mut vs = Input::VapourSynth {
        path,
        import_method: VapourSynthImportMethod::FFMS2 { index: None },
        cache_path: None,
        filters: vec![],
    };
    vs.adopt_filters(&native).expect("model-only copy");
    let restored: Input = serde_json::from_str(&serde_json::to_string(&vs).expect("deferred JSON"))
        .expect("deferred JSON roundtrip");
    for filter in restored.vapoursynth_filters() {
        assert_eq!(
            filter
                .to_string()
                .parse::<VapourSynthFilter>()
                .expect("deferred display roundtrip"),
            *filter
        );
    }
    native.adopt_filters(&restored).expect("deferred to native");
    let legacy = serde_json::from_str(
        r#"{"Resize":{"scaler":"Bicubic","width":null,"height":null,"format":"YUV422P"}}"#,
    )
    .expect("legacy Resize config");
    native.set_filters(vec![legacy]).expect("legacy concrete resize");
    assert_eq!(
        std::fs::read_dir(directory.path()).expect("directory listing").count(),
        0,
        "no decoder/index/config I/O"
    );
}
