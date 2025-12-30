use std::{collections::HashMap, path::PathBuf};

use anyhow::Result;
use thiserror::Error;

use crate::{condor::data::output::Output as OutputData, ConcatMethod};

pub struct Output {
    pub path:                 PathBuf,
    pub concatenation_method: ConcatMethod,
    pub tags:                 HashMap<String, String>,
    pub video_tags:           HashMap<String, String>,
}

impl Output {
    #[inline]
    pub fn as_data(&self) -> OutputData {
        OutputData {
            path:                 self.path.clone(),
            concatenation_method: self.concatenation_method,
            tags:                 self.tags.clone(),
            video_tags:           self.video_tags.clone(),
        }
    }

    #[inline]
    pub fn validate(data: &OutputData) -> Result<((), Vec<Box<OutputError>>)> {
        let mut warnings = vec![];
        if data.path.parent().is_some_and(|parent| !parent.exists()) {
            warnings.push(Box::new(OutputError::ValidationFailed(data.path.clone())));
        }
        // TODO: Validate we can write to the parent folder

        match data.concatenation_method {
            ConcatMethod::FFmpeg => {
                which::which("ffmpeg")?;
            },
            ConcatMethod::MKVMerge => {
                which::which("mkvmerge")?;
            },
            ConcatMethod::Ivf => todo!(),
        }

        Ok(((), warnings))
    }

    #[inline]
    pub fn new(data: &OutputData) -> Result<Self> {
        Ok(Output {
            path:                 data.path.clone(),
            concatenation_method: data.concatenation_method,
            tags:                 data.tags.clone(),
            video_tags:           data.video_tags.clone(),
        })
    }
}

#[derive(Debug, Error)]
pub enum OutputError {
    #[error("Output path parent folder {0} does not exist or is invalid")]
    ValidationFailed(PathBuf),
}
