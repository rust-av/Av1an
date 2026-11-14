use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync, thread,
};

use andean_condor::{
    core::{
        Condor,
        input::Input,
        sequence::{
            Sequence, SequenceStatus, Status,
            parallel_encoder::ParallelEncoder,
            scene_concatenator::{SceneConcatenator, SceneConcatenatorError},
        },
    },
    models::{
        input::{Input as InputModel, VapourSynthScriptSource},
        output::Output as OutputModel,
        scene::Scene,
        sequence::{
            DefaultSequenceConfig, DefaultSequenceData,
            scene_concatenator::{ConcatMethod, FfmpegConfig},
        },
    },
    utils::cancellation::{Cancellation, Cancelled},
};
use anyhow::Result;

use crate::core::support::provenance::current_encoding_fixture;

#[cfg(unix)]
#[path = "fixtures/final_mux.rs"]
mod final_mux;
#[cfg(unix)]
mod publication;
mod validation;

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
        andean_condor::core::output::Output::new(&output).expect("output"),
        Default::default(),
        vec![],
        Some(config),
        Box::new(|_| Ok(())),
    )
}
