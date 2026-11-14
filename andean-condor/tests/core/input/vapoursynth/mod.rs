use super::*;
use andean_condor::vapoursynth::get_api;

#[test]
fn rgb_input_rejected_at_open() {
    let data = InputModel::VapourSynthScript {
        source: VapourSynthScriptSource::Text(
            "import vapoursynth as vs\nvs.core.std.BlankClip(format=vs.RGB24).set_output()"
                .to_owned(),
        ),
        variables: HashMap::new(),
        index: 0,
        filters: Vec::new(),
        stream_concurrently: true,
    };
    let error = Input::from_data(&data).err().expect("RGB input must fail to open");
    assert!(
        matches!(
            error.downcast_ref::<DecoderError>(),
            Some(DecoderError::UnsupportedFormat { .. })
        ),
        "{error:#}"
    );
}

/// Missing native runtime must produce an availability error rather than panic.
#[test]
fn script_input_without_vapoursynth_errors_instead_of_panicking() {
    let available = get_api().is_ok();
    let data = InputModel::VapourSynthScript {
        source: VapourSynthScriptSource::Text(
            "from vapoursynth import core
core.std.BlankClip().set_output()"
                .to_owned(),
        ),
        variables: HashMap::new(),
        index: 0,
        filters: Vec::new(),
        stream_concurrently: false,
    };
    let result = Input::from_data(&data);
    if !available {
        let Err(error) = result else {
            panic!("a VapourSynth input cannot be constructed without VapourSynth");
        };
        assert!(
            error.to_string().contains("VapourSynth is not available"),
            "{error}"
        );
    }
}
