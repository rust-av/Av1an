use std::{collections::HashMap, path::PathBuf};

use serde::{Deserialize, Serialize};
use strum::{Display, EnumString, IntoStaticStr};

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
    // FFmpeg {}, // Unsupported
    FFMS2 {},
}

#[derive(Debug, Clone, Serialize, Deserialize, EnumString, IntoStaticStr, Display)]
pub enum VapourSynthScriptSource {
    Path(PathBuf),
    Text(String),
}
