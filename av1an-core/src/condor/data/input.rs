use std::{collections::HashMap, path::PathBuf};

use serde::{Deserialize, Serialize};
use strum::{Display, EnumString, IntoStaticStr};

use crate::condor::core::input::Input as InputInstance;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Input {
    Video {
        path:          PathBuf,
        import_method: ImportMethod,
    },
    VapourSynth {
        path:          PathBuf,
        import_method: VapourSynthImportMethod,
        cache_path:    Option<PathBuf>,
    },
    VapourSynthScript {
        source:    VapourSynthScriptSource,
        variables: HashMap<String, String>,
        index:     u8,
    },
}

impl Input {
    #[inline]
    pub fn from_instance(input: &InputInstance) -> Input {
        match input {
            InputInstance::Video {
                path,
                import_method,
                ..
            } => Input::Video {
                path:          path.clone(),
                import_method: import_method.clone(),
            },
            InputInstance::VapourSynth {
                path,
                import_method,
                cache_path,
                ..
            } => Input::VapourSynth {
                path:          path.clone(),
                import_method: import_method.clone(),
                cache_path:    cache_path.clone(),
            },
            InputInstance::VapourSynthScript {
                source,
                variables,
                index,
                ..
            } => Input::VapourSynthScript {
                source:    source.clone(),
                variables: variables.clone(),
                index:     *index,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, EnumString, IntoStaticStr, Display)]
pub enum VapourSynthImportMethod {
    LSMASHWorks {
        // plugin_path: Option<PathBuf>,
        index: Option<u8>,
        // cache_path: Option<PathBuf>,
    },
    DGDecNV {
        // plugin_path:          Option<PathBuf>,
        // cache_path:           Option<PathBuf>,
        dgindexnv_executable: Option<PathBuf>,
    },
    FFMS2 {
        // plugin_path: Option<PathBuf>,
        index: Option<u8>,
        // cache_path: Option<PathBuf>,
    },
    BestSource {
        // plugin_path: Option<PathBuf>,
        index: Option<u8>,
        // cache_path: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, EnumString, IntoStaticStr, Display)]
pub enum ImportMethod {
    FFmpeg {},
    FFMS2 {},
}

#[derive(Debug, Clone, Serialize, Deserialize, EnumString, IntoStaticStr, Display)]
pub enum VapourSynthScriptSource {
    Path(PathBuf),
    Text(String),
}
