use std::{collections::HashMap, path::PathBuf};

use serde::{Deserialize, Serialize};
use strum::{Display, EnumString, IntoStaticStr};

use crate::condor::data::encoding::{cli_parameter::CLIParameter, photon_noise::PhotonNoise};

pub mod cli_parameter;
pub mod photon_noise;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, EnumString, IntoStaticStr, Display)]
pub enum EncoderBase {
    AOM,
    RAV1E,
    VPX,
    SVTAV1,
    X264,
    X265,
    VVenC,
    FFmpeg,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Encoder {
    AOM {
        executable:   Option<PathBuf>,
        pass:         EncoderPasses,
        options:      HashMap<String, CLIParameter>,
        photon_noise: Option<PhotonNoise>,
    },
    RAV1E {
        executable:   Option<PathBuf>,
        pass:         EncoderPasses,
        options:      HashMap<String, CLIParameter>,
        photon_noise: Option<PhotonNoise>,
    },
    VPX {
        executable: Option<PathBuf>,
        pass:       EncoderPasses,
        options:    HashMap<String, CLIParameter>,
    },
    SVTAV1 {
        executable:   Option<PathBuf>,
        pass:         EncoderPasses,
        options:      HashMap<String, CLIParameter>,
        photon_noise: Option<PhotonNoise>,
    },
    X264 {
        executable: Option<PathBuf>,
        pass:       EncoderPasses,
        options:    HashMap<String, CLIParameter>,
    },
    X265 {
        executable: Option<PathBuf>,
        pass:       EncoderPasses,
        options:    HashMap<String, CLIParameter>,
    },
    VVenC {
        executable: Option<PathBuf>,
        pass:       EncoderPasses,
        options:    HashMap<String, CLIParameter>,
    },
    FFmpeg {
        executable: Option<PathBuf>,
        options:    HashMap<String, CLIParameter>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum EncoderPasses {
    All(u8),
    Specific(u8, u8),
}
