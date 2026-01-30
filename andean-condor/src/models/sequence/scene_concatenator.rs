use std::{
    fmt::Display,
    path::{Path, PathBuf},
};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::models::sequence::SequenceConfigHandler;

pub trait SceneConcatenatorConfigHandler
where
    Self: SequenceConfigHandler,
{
    fn scene_concatenator(&self) -> Result<&SceneConcatenatorConfig>;
    fn scene_concatenator_mut(&mut self) -> Result<&mut SceneConcatenatorConfig>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneConcatenatorConfig
where
    Self: SequenceConfigHandler,
{
    pub method:           ConcatMethod,
    pub scenes_directory: PathBuf,
    pub output:           Option<PathBuf>,
}

impl Default for SceneConcatenatorConfig {
    #[inline]
    fn default() -> Self {
        Self {
            method:           ConcatMethod::MKVMerge,
            scenes_directory: PathBuf::new(),
            output:           None,
        }
    }
}

impl SceneConcatenatorConfig {
    #[inline]
    pub fn new(scenes_directory: &Path) -> Self {
        Self {
            scenes_directory: scenes_directory.to_path_buf(),
            ..Default::default()
        }
    }
}

impl SequenceConfigHandler for SceneConcatenatorConfig {
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

impl ConcatMethod {
    #[inline]
    pub fn extension(&self) -> &'static str {
        match self {
            Self::MKVMerge => "mkv",
            Self::FFmpeg => "mkv",
            Self::Ivf => "ivf",
        }
    }

    #[inline]
    pub fn with_extension(&self, path: &Path) -> PathBuf {
        let mut path = path.to_path_buf();
        path.set_extension(self.extension());
        path
    }
}
