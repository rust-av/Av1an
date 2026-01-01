use std::{collections::HashMap, path::PathBuf};

use serde::{Deserialize, Serialize};
use strum::{Display, EnumString, IntoStaticStr};

use crate::condor::data::encoding::{cli_parameter::CLIParameter, photon_noise::PhotonNoise};

pub mod cli_parameter;
pub mod photon_noise;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, EnumString, IntoStaticStr, Display)]
pub enum EncoderBase {
    #[strum(serialize = "aom")]
    AOM,
    #[strum(serialize = "rav1e")]
    RAV1E,
    #[strum(serialize = "vpx")]
    VPX,
    #[strum(serialize = "svt-av1")]
    SVTAV1,
    #[strum(serialize = "x264")]
    X264,
    #[strum(serialize = "x265")]
    X265,
    #[strum(serialize = "vvenc")]
    VVenC,
    #[strum(serialize = "ffmpeg")]
    FFmpeg,
}

impl EncoderBase {
    #[inline]
    pub fn default_passes(&self) -> EncoderPasses {
        match self {
            Self::AOM | Self::VPX => EncoderPasses::All(2),
            _ => EncoderPasses::All(1),
        }
    }

    #[inline]
    pub fn default_parameters(&self) -> HashMap<String, CLIParameter> {
        match self {
            EncoderBase::AOM => {
                let mut parameters = CLIParameter::new_numbers("--", "=", &[
                    ("threads", 8.0),
                    ("cpu-used", 6.0),
                    ("cq-level", 30.0),
                    ("kf-max-dist", 9999.0),
                ]);
                parameters.insert(
                    "end-usage".to_owned(),
                    CLIParameter::new_string("--", "=", "q"),
                );
                parameters
            },
            EncoderBase::RAV1E => {
                let mut parameters = CLIParameter::new_numbers("--", " ", &[
                    ("speed", 8.0),
                    ("quantizer", 100.0),
                    ("keyint", 0.0),
                ]);
                parameters.insert(
                    "no-scene-detection".to_owned(),
                    CLIParameter::new_bool("--", true),
                );
                parameters.extend(CLIParameter::new_bools("-", &[("y", true)]));
                parameters
            },
            EncoderBase::VPX => {
                let mut parameters = CLIParameter::new_numbers("--", "=", &[
                    ("profile", 2.0),
                    ("threads", 4.0),
                    ("cpu-used", 2.0),
                    ("cq-level", 30.0),
                    ("row-mt", 1.0),
                    ("auto-alt-ref", 6.0),
                    ("kf-max-dist", 9999.0),
                ]);
                parameters.insert("b".to_owned(), CLIParameter::new_number("-", " ", 10.0));
                let string_defaults =
                    CLIParameter::new_strings("--", "=", &[("codec", "vp9"), ("end-usage", "q")]);
                parameters.insert("disable-kf".to_owned(), CLIParameter::new_bool("--", true));
                parameters.extend(string_defaults);
                parameters
            },
            EncoderBase::SVTAV1 => CLIParameter::new_numbers("--", " ", &[
                ("preset", 4.0),
                ("keyint", 0.0),
                ("scd", 0.0),
                ("rc", 0.0),
                ("crf", 25.0),
                ("progress", 2.0),
            ]),
            EncoderBase::X264 => {
                let mut parameters =
                    CLIParameter::new_numbers("--", " ", &[("crf", 25.0), ("scenecut", 0.0)]);
                parameters.extend(CLIParameter::new_strings("--", "=", &[
                    ("preset", "slow"),
                    ("keyint", "infinite"),
                    ("log-level", "error"),
                    ("demuxer", "y4m"),
                ]));
                parameters.extend(CLIParameter::new_bools("--", &[("stitchable", true)]));
                parameters
            },
            EncoderBase::X265 => {
                let mut parameters = CLIParameter::new_numbers("--", " ", &[
                    ("crf", 25.0),
                    ("level-idc", 5.0),
                    ("keyint", -1.0),
                    ("scenecut", 0.0),
                ]);
                parameters.extend(CLIParameter::new_numbers("-", " ", &[("D", 10.0)]));
                parameters.extend(CLIParameter::new_bools("--", &[("y4m", true)]));
                parameters.extend(CLIParameter::new_strings("--", " ", &[("preset", "slow")]));
                parameters
            },
            EncoderBase::VVenC => todo!(),
            EncoderBase::FFmpeg => HashMap::new(),
        }
    }

    #[inline]
    pub fn pass_parameters(
        &self,
        pass: (u8, u8),
        first_pass_file_name: &str,
    ) -> HashMap<String, CLIParameter> {
        match self {
            EncoderBase::AOM => {
                let mut parameters = HashMap::new();
                match pass {
                    (1, 1) => parameters,
                    (1, _) => {
                        parameters.extend(CLIParameter::new_numbers("--", "=", &[
                            ("passes", 2.0),
                            ("pass", 1.0),
                        ]));
                        parameters.insert(
                            "fpf".to_owned(),
                            CLIParameter::new_string(
                                "--",
                                "=",
                                &format!("{}.log", first_pass_file_name),
                            ),
                        );

                        parameters
                    },
                    (_, _) => {
                        parameters.extend(CLIParameter::new_numbers("--", "=", &[
                            ("passes", 2.0),
                            ("pass", 2.0),
                        ]));
                        parameters.insert(
                            "fpf".to_owned(),
                            CLIParameter::new_string(
                                "--",
                                "=",
                                &format!("{}.log", first_pass_file_name),
                            ),
                        );

                        parameters
                    },
                }
            },
            EncoderBase::RAV1E => {
                let mut parameters = HashMap::new();

                match pass {
                    (1, 1) => parameters,
                    (1, _) => {
                        parameters.extend(CLIParameter::new_numbers("--", " ", &[
                            ("passes", 2.0),
                            ("pass", 1.0),
                        ]));
                        parameters.extend(CLIParameter::new_bools("--", &[("quiet", true)]));
                        parameters.extend(CLIParameter::new_strings("--", " ", &[(
                            "first-pass",
                            &format!("{}.stat", first_pass_file_name),
                        )]));

                        parameters
                    },
                    (_, _) => {
                        parameters.extend(CLIParameter::new_numbers("--", " ", &[
                            ("passes", 2.0),
                            ("pass", 2.0),
                        ]));
                        parameters.extend(CLIParameter::new_bools("--", &[("quiet", true)]));
                        parameters.extend(CLIParameter::new_strings("--", " ", &[(
                            "second-pass",
                            &format!("{}.stat", first_pass_file_name),
                        )]));

                        parameters
                    },
                }
            },
            EncoderBase::VPX => {
                let mut parameters = HashMap::new();

                match pass {
                    (1, 1) => {
                        parameters.extend(CLIParameter::new_numbers("--", "=", &[("passes", 1.0)]));

                        parameters
                    },
                    (1, _) => {
                        parameters.extend(CLIParameter::new_numbers("--", "=", &[
                            ("passes", 2.0),
                            ("pass", 1.0),
                        ]));
                        parameters.extend(CLIParameter::new_strings("--", "=", &[(
                            "fpf",
                            &format!("{}.log", first_pass_file_name),
                        )]));

                        parameters
                    },
                    (_, _) => {
                        parameters.extend(CLIParameter::new_numbers("--", "=", &[
                            ("passes", 2.0),
                            ("pass", 2.0),
                        ]));
                        parameters.extend(CLIParameter::new_strings("--", "=", &[(
                            "fpf",
                            &format!("{}.log", first_pass_file_name),
                        )]));

                        parameters
                    },
                }
            },
            EncoderBase::SVTAV1 => {
                let mut parameters = HashMap::new();

                match pass {
                    (1, 1) => parameters,
                    (1, _) => {
                        parameters.extend(CLIParameter::new_numbers("--", " ", &[
                            ("irefresh-type", 2.0),
                            ("pass", 1.0),
                        ]));

                        parameters.insert(
                            "stats".to_owned(),
                            CLIParameter::new_string(
                                "-",
                                " ",
                                &format!("{}.stat", first_pass_file_name),
                            ),
                        );

                        parameters
                    },
                    (_, _) => {
                        parameters.extend(CLIParameter::new_numbers("--", " ", &[
                            ("irefresh-type", 2.0),
                            ("pass", 2.0),
                        ]));

                        parameters.insert(
                            "stats".to_owned(),
                            CLIParameter::new_string(
                                "-",
                                " ",
                                &format!("{}.stat", first_pass_file_name),
                            ),
                        );

                        parameters
                    },
                }
            },
            EncoderBase::X264 => {
                let mut parameters = HashMap::new();

                match pass {
                    (1, 1) => parameters,
                    (1, _) => {
                        parameters.extend(CLIParameter::new_numbers("--", " ", &[("pass", 1.0)]));
                        parameters.extend(CLIParameter::new_strings("--", " ", &[(
                            "stats",
                            &format!("{}.log", first_pass_file_name),
                        )]));

                        parameters
                    },
                    (_, _) => {
                        parameters.extend(CLIParameter::new_numbers("--", " ", &[("pass", 2.0)]));
                        parameters.extend(CLIParameter::new_strings("--", " ", &[(
                            "stats",
                            &format!("{}.log", first_pass_file_name),
                        )]));

                        parameters
                    },
                }
            },
            EncoderBase::X265 => {
                let mut parameters = HashMap::new();

                match pass {
                    (1, 1) => parameters,
                    (1, _) => {
                        parameters
                            .extend(CLIParameter::new_bools("--", &[("repeat-headers", true)]));
                        parameters.extend(CLIParameter::new_numbers("--", " ", &[("pass", 1.0)]));
                        parameters.extend(CLIParameter::new_strings("--", " ", &[
                            ("log-level", "error"),
                            ("stats", &format!("{}.log", first_pass_file_name)),
                            (
                                "analysis-reuse-file",
                                &format!("{}_analysis.dat", first_pass_file_name),
                            ),
                        ]));

                        parameters
                    },
                    (_, _) => {
                        parameters
                            .extend(CLIParameter::new_bools("--", &[("repeat-headers", true)]));
                        parameters.extend(CLIParameter::new_numbers("--", " ", &[("pass", 2.0)]));
                        parameters.extend(CLIParameter::new_strings("--", " ", &[
                            ("log-level", "error"),
                            ("stats", &format!("{}.log", first_pass_file_name)),
                            (
                                "analysis-reuse-file",
                                &format!("{}_analysis.dat", first_pass_file_name),
                            ),
                        ]));

                        parameters
                    },
                }
            },
            EncoderBase::VVenC => todo!(),
            EncoderBase::FFmpeg => HashMap::new(),
        }
    }

    #[inline]
    pub fn output_extension(&self) -> &'static str {
        match self {
            EncoderBase::AOM | EncoderBase::RAV1E | EncoderBase::VPX | EncoderBase::SVTAV1 => "ivf",
            EncoderBase::X264 => "264",
            EncoderBase::X265 => "hevc",
            EncoderBase::VVenC => todo!(),
            EncoderBase::FFmpeg => ".mkv",
        }
    }

    #[inline]
    pub fn friendly_name(&self) -> &'static str {
        match self {
            EncoderBase::AOM => "Alliance for Open Media AV1",
            EncoderBase::RAV1E => "rav1e",
            EncoderBase::VPX => "WebM VP8/VP9",
            EncoderBase::SVTAV1 => "Scalable Video Technology for AV1",
            EncoderBase::X264 => "x264",
            EncoderBase::X265 => "x265",
            EncoderBase::VVenC => "Fraunhofer Versatile Video Encoder",
            EncoderBase::FFmpeg => "FFmpeg",
        }
    }
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

impl Encoder {
    #[inline]
    pub fn base(&self) -> EncoderBase {
        match self {
            Encoder::AOM {
                ..
            } => EncoderBase::AOM,
            Encoder::RAV1E {
                ..
            } => EncoderBase::RAV1E,
            Encoder::VPX {
                ..
            } => EncoderBase::VPX,
            Encoder::SVTAV1 {
                ..
            } => EncoderBase::SVTAV1,
            Encoder::X264 {
                ..
            } => EncoderBase::X264,
            Encoder::X265 {
                ..
            } => EncoderBase::X265,
            Encoder::VVenC {
                ..
            } => EncoderBase::VVenC,
            Encoder::FFmpeg {
                ..
            } => EncoderBase::FFmpeg,
        }
    }

    #[inline]
    pub fn default_from_base(encoder: &EncoderBase, exclude_default_options: bool) -> Self {
        let pass = encoder.default_passes();
        let options = if exclude_default_options {
            HashMap::new()
        } else {
            encoder.default_parameters()
        };
        match encoder {
            EncoderBase::AOM => Encoder::AOM {
                executable: None,
                pass,
                options,
                photon_noise: None,
            },
            EncoderBase::RAV1E => Encoder::RAV1E {
                executable: None,
                pass,
                options,
                photon_noise: None,
            },
            EncoderBase::VPX => Encoder::VPX {
                executable: None,
                pass,
                options,
            },
            EncoderBase::SVTAV1 => Encoder::SVTAV1 {
                executable: None,
                pass,
                options,
                photon_noise: None,
            },
            EncoderBase::X264 => Encoder::X264 {
                executable: None,
                pass,
                options,
            },
            EncoderBase::X265 => Encoder::X265 {
                executable: None,
                pass,
                options,
            },
            EncoderBase::VVenC => Encoder::VVenC {
                executable: None,
                pass,
                options,
            },
            EncoderBase::FFmpeg => Encoder::FFmpeg {
                executable: None,
                options,
            },
        }
    }

    #[inline]
    pub fn passes(&self) -> EncoderPasses {
        match self {
            Encoder::AOM {
                pass, ..
            } => *pass,
            Encoder::RAV1E {
                pass, ..
            } => *pass,
            Encoder::VPX {
                pass, ..
            } => *pass,
            Encoder::SVTAV1 {
                pass, ..
            } => *pass,
            Encoder::X264 {
                pass, ..
            } => *pass,
            Encoder::X265 {
                pass, ..
            } => *pass,
            Encoder::VVenC {
                pass, ..
            } => *pass,
            Encoder::FFmpeg {
                ..
            } => EncoderPasses::All(1),
        }
    }

    #[inline]
    pub fn passes_mut(&mut self) -> Option<&mut EncoderPasses> {
        match self {
            Encoder::AOM {
                pass, ..
            } => Some(pass),
            Encoder::RAV1E {
                pass, ..
            } => Some(pass),
            Encoder::VPX {
                pass, ..
            } => Some(pass),
            Encoder::SVTAV1 {
                pass, ..
            } => Some(pass),
            Encoder::X264 {
                pass, ..
            } => Some(pass),
            Encoder::X265 {
                pass, ..
            } => Some(pass),
            Encoder::VVenC {
                pass, ..
            } => Some(pass),
            Encoder::FFmpeg {
                ..
            } => None,
        }
    }

    #[inline]
    pub fn total_passes(&self) -> u8 {
        match self.passes() {
            EncoderPasses::All(passes) => passes,
            EncoderPasses::Specific(_, _) => 1,
        }
    }

    #[inline]
    pub fn parameters(&self) -> &HashMap<String, CLIParameter> {
        match self {
            Encoder::AOM {
                options, ..
            } => options,
            Encoder::RAV1E {
                options, ..
            } => options,
            Encoder::VPX {
                options, ..
            } => options,
            Encoder::SVTAV1 {
                options, ..
            } => options,
            Encoder::X264 {
                options, ..
            } => options,
            Encoder::X265 {
                options, ..
            } => options,
            Encoder::VVenC {
                options, ..
            } => options,
            Encoder::FFmpeg {
                options, ..
            } => options,
        }
    }

    #[inline]
    pub fn parameters_mut(&mut self) -> &mut HashMap<String, CLIParameter> {
        match self {
            Encoder::AOM {
                options, ..
            } => options,
            Encoder::RAV1E {
                options, ..
            } => options,
            Encoder::VPX {
                options, ..
            } => options,
            Encoder::SVTAV1 {
                options, ..
            } => options,
            Encoder::X264 {
                options, ..
            } => options,
            Encoder::X265 {
                options, ..
            } => options,
            Encoder::VVenC {
                options, ..
            } => options,
            Encoder::FFmpeg {
                options, ..
            } => options,
        }
    }

    #[inline]
    pub fn output_extension(&self) -> &'static str {
        self.base().output_extension()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum EncoderPasses {
    All(u8),
    Specific(u8, u8),
}
