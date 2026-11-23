use std::{path::PathBuf, sync::Arc};

use anyhow::Result;

use crate::{
    core::encoder::EncoderResult,
    models::{encoder::Encoder, scene::SubScene},
    utils::semaphore::Semaphore,
};

/// One scene's slice of an encode pass.
///
/// `original_index` is the scene's position in `condor.scenes`; `index` is its
/// position among the tasks actually being encoded, which differs once
/// already-encoded scenes are filtered out.
#[derive(Debug, Clone)]
pub struct Task {
    pub original_index: usize,
    pub index: usize,
    pub frame_indices: Vec<usize>,
    pub sub_scenes: Option<Vec<SubScene>>,
    pub encoder: Encoder,
    pub output: PathBuf,
}

/// Channel plumbing that persists scene results as they finish rather than
/// only between scene decodes.
pub(super) struct ResultStream<'a, F>
where
    F: FnMut(Vec<PublishedResult>) -> Result<()>,
{
    pub(super) results_tx: crossbeam_channel::Sender<PublishedResult>,
    pub(super) results_rx: &'a crossbeam_channel::Receiver<PublishedResult>,
    pub(super) finished_scenes: Arc<Semaphore>,
    pub(super) on_results: &'a mut F,
}

pub(super) struct PublishedResult {
    pub(super) result: ParallelEncoderResult,
    pub(super) identity: serde_json::Value,
}

/// One finished scene encode.
#[derive(Debug, Clone)]
pub struct ParallelEncoderResult {
    pub scene: usize,
    pub started: std::time::SystemTime,
    pub ended: std::time::SystemTime,
    pub bytes: u64,
    pub bitrate: f64,
    pub result: EncoderResult,
}
