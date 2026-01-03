use std::{fmt::Display, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::models::sequence::SequenceConfigHandler;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneConcatenateConfig
where
    Self: SequenceConfigHandler,
{
    pub method: ConcatMethod,
    pub output: Option<PathBuf>,
}

impl Default for SceneConcatenateConfig {
    #[inline]
    fn default() -> Self {
        Self {
            method: ConcatMethod::MKVMerge,
            output: None,
        }
    }
}

impl SequenceConfigHandler for SceneConcatenateConfig {
}

#[derive(
    PartialEq,
    Eq,
    Copy,
    Clone,
    Serialize,
    Deserialize,
    Debug,
    strum::EnumString,
    strum::IntoStaticStr,
)]
pub enum ConcatMethod {
    #[strum(serialize = "mkvmerge")]
    MKVMerge,
    #[strum(serialize = "ffmpeg")]
    FFmpeg,
    #[strum(serialize = "ivf")]
    Ivf,
}

impl Display for ConcatMethod {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(<&'static str>::from(self))
    }
}
