use std::{collections::HashMap, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::ConcatMethod;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    pub path:                 PathBuf,
    pub concatenation_method: ConcatMethod,
    pub tags:                 HashMap<String, String>,
    pub video_tags:           HashMap<String, String>,
    // extra_inputs: Vec<PathBuf>, // TODO: consider allowing selecting which tracks to copy
}
