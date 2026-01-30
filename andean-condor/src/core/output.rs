use std::{collections::HashMap, path::PathBuf};

use anyhow::Result;
use thiserror::Error;

use crate::models::output::Output as OutputModel;

pub struct Output {
    pub path:       PathBuf,
    pub tags:       HashMap<String, String>,
    pub video_tags: HashMap<String, String>,
}

impl Output {
    #[inline]
    pub fn as_data(&self) -> OutputModel {
        OutputModel {
            path:       self.path.clone(),
            tags:       self.tags.clone(),
            video_tags: self.video_tags.clone(),
        }
    }

    #[inline]
    pub fn validate(data: &OutputModel) -> Result<((), Vec<Box<OutputError>>)> {
        let mut warnings = vec![];
        if data.path.parent().is_some_and(|parent| !parent.exists()) {
            warnings.push(Box::new(OutputError::ValidationFailed(data.path.clone())));
        }

        Ok(((), warnings))
    }

    #[inline]
    pub fn new(data: &OutputModel) -> Result<Self> {
        Ok(Output {
            path:       data.path.clone(),
            tags:       data.tags.clone(),
            video_tags: data.video_tags.clone(),
        })
    }
}

#[derive(Debug, Error)]
pub enum OutputError {
    #[error("Output path parent folder {0} does not exist or is invalid")]
    ValidationFailed(PathBuf),
}
