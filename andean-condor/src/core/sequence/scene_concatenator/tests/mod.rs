mod internal_integration;
#[cfg(unix)]
mod mux_status;
pub(super) mod publication;
mod scene_list;

use super::*;
#[cfg(unix)]
use crate::core::sequence::tests::support::current_encoding_fixture;
use crate::models::{
    input::{Input as InputModel, VapourSynthScriptSource},
    output::Output as OutputModel,
    scene::Scene,
    sequence::{DefaultSequenceConfig, DefaultSequenceData},
};

#[cfg(unix)]
#[path = "../../../../../tests/core/sequence/scene_concatenator/fixtures/final_mux.rs"]
mod final_mux;

fn concat_job(
    directory: &Path,
    output: &Path,
) -> Condor<DefaultSequenceData, DefaultSequenceConfig> {
    let input = Input::from_data(&InputModel::VapourSynthScript {
        source: VapourSynthScriptSource::Text("import vapoursynth as vs\nvs.core.std.BlankClip(width=32,height=16,length=2,format=vs.YUV420P8).set_output()".into()),
        variables: Default::default(), index: 0, filters: vec![], stream_concurrently: false,
    }).expect("finite native input");
    let output: OutputModel = serde_json::from_value(
        serde_json::json!({"path":output,"tags":{},"video_tags":{},"overwrite":true}),
    )
    .expect("output model");
    let mut config = DefaultSequenceConfig::default();
    config.scene_concatenator.method = ConcatMethod::Ivf;
    config.scene_concatenator.scenes_directory = directory.into();
    Condor::new(
        input,
        crate::core::output::Output::new(&output).expect("output"),
        Default::default(),
        vec![],
        Some(config),
        Box::new(|_| Ok(())),
    )
}
