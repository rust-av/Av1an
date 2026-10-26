use std::{collections::HashMap, path::PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use strum::{Display, EnumString, IntoStaticStr};

use crate::models::encoder::{cli_parameter::CLIParameter, photon_noise::PhotonNoise};

pub mod cli_parameter;
pub mod photon_noise;

#[derive(
    Debug,
    Clone,
    Copy,
    Serialize,
    Deserialize,
    EnumString,
    IntoStaticStr,
    Display,
    PartialEq,
    Eq,
    Hash,
    JsonSchema,
)]
pub enum EncoderBase {
    #[strum(serialize = "aom")]
    AOM,
    #[strum(serialize = "rav1e")]
    RAV1E,
    #[strum(serialize = "vpx")]
    VPX,
    #[strum(serialize = "svt-av1")]
    SVTAV1,
    #[strum(serialize = "avm")]
    AVM,
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

    /// Whether this encoder can carry a Photon Noise table.
    #[inline]
    pub fn supports_photon_noise(&self) -> bool {
        matches!(self, Self::AOM | Self::RAV1E | Self::SVTAV1 | Self::AVM)
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
            EncoderBase::AVM => {
                let mut parameters = CLIParameter::new_numbers("--", "=", &[
                    ("threads", 8.0),
                    ("cpu-used", 6.0),
                    ("qp", 30.0),
                    ("kf-max-dist", 9999.0),
                ]);
                parameters.insert(
                    "end-usage".to_owned(),
                    CLIParameter::new_string("--", "=", "q"),
                );
                parameters
            },
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
            EncoderBase::VVenC => HashMap::new(),
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
            EncoderBase::AVM => {
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
            EncoderBase::VVenC => {
                let mut parameters = HashMap::new();

                match pass {
                    (1, 1) => parameters,
                    (1, passes) => {
                        parameters.extend(CLIParameter::new_numbers("--", " ", &[(
                            "passes",
                            passes as f64,
                        )]));
                        parameters.extend(CLIParameter::new_numbers("--", " ", &[("pass", 1.0)]));
                        parameters.extend(CLIParameter::new_strings("--", " ", &[(
                            "rcstatsfile",
                            &format!("{}.json", first_pass_file_name),
                        )]));

                        parameters
                    },
                    (_, passes) => {
                        parameters.extend(CLIParameter::new_numbers("--", " ", &[(
                            "passes",
                            passes as f64,
                        )]));
                        parameters.extend(CLIParameter::new_numbers("--", " ", &[("pass", 2.0)]));
                        parameters.extend(CLIParameter::new_strings("--", " ", &[(
                            "rcstatsfile",
                            &format!("{}.json", first_pass_file_name),
                        )]));

                        parameters
                    },
                }
            },
            EncoderBase::FFmpeg => HashMap::new(),
        }
    }

    #[inline]
    pub fn output_extension(&self) -> &'static str {
        match self {
            EncoderBase::AOM
            | EncoderBase::RAV1E
            | EncoderBase::VPX
            | EncoderBase::SVTAV1
            | EncoderBase::AVM => "ivf",
            EncoderBase::X264 => "264",
            EncoderBase::X265 => "hevc",
            EncoderBase::VVenC => "266",
            EncoderBase::FFmpeg => ".mkv",
        }
    }

    #[inline]
    pub fn friendly_name(&self) -> &'static str {
        match self {
            EncoderBase::AOM => "Alliance for Open Media AV1",
            EncoderBase::RAV1E => "Rust AV1 Encoder",
            EncoderBase::VPX => "WebM VP8/VP9",
            EncoderBase::SVTAV1 => "Scalable Video Technology for AV1",
            EncoderBase::AVM => "Alliance for Open Media Video Model",
            EncoderBase::X264 => "x264",
            EncoderBase::X265 => "x265",
            EncoderBase::VVenC => "Fraunhofer Versatile Video Encoder",
            EncoderBase::FFmpeg => "FFmpeg",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum Encoder {
    AOM {
        executable:        Option<PathBuf>,
        pass:              EncoderPasses,
        options:           HashMap<String, CLIParameter>,
        photon_noise:      Option<PhotonNoise>,
        /// FFmpeg filtergraph applied to the frames before they reach this
        /// encoder. Requires FFmpeg in PATH.
        ffmpeg_filter:     Option<String>,
        /// Append the source's colour and HDR metadata to the encoder
        /// parameters.
        #[serde(default = "crate::models::sequence::scene_concatenator::common::default_true")]
        append_color_data: bool,
    },
    RAV1E {
        executable:        Option<PathBuf>,
        pass:              EncoderPasses,
        options:           HashMap<String, CLIParameter>,
        photon_noise:      Option<PhotonNoise>,
        /// FFmpeg filtergraph applied to the frames before they reach this
        /// encoder. Requires FFmpeg in PATH.
        ffmpeg_filter:     Option<String>,
        /// Append the source's colour and HDR metadata to the encoder
        /// parameters.
        #[serde(default = "crate::models::sequence::scene_concatenator::common::default_true")]
        append_color_data: bool,
    },
    VPX {
        executable:        Option<PathBuf>,
        pass:              EncoderPasses,
        options:           HashMap<String, CLIParameter>,
        /// FFmpeg filtergraph applied to the frames before they reach this
        /// encoder. Requires FFmpeg in PATH.
        ffmpeg_filter:     Option<String>,
        /// Append the source's colour and HDR metadata to the encoder
        /// parameters.
        #[serde(default = "crate::models::sequence::scene_concatenator::common::default_true")]
        append_color_data: bool,
    },
    SVTAV1 {
        executable:        Option<PathBuf>,
        pass:              EncoderPasses,
        options:           HashMap<String, CLIParameter>,
        photon_noise:      Option<PhotonNoise>,
        /// FFmpeg filtergraph applied to the frames before they reach this
        /// encoder. Requires FFmpeg in PATH.
        ffmpeg_filter:     Option<String>,
        /// Append the source's colour and HDR metadata to the encoder
        /// parameters.
        #[serde(default = "crate::models::sequence::scene_concatenator::common::default_true")]
        append_color_data: bool,
    },
    AVM {
        executable:        Option<PathBuf>,
        pass:              EncoderPasses,
        options:           HashMap<String, CLIParameter>,
        photon_noise:      Option<PhotonNoise>,
        /// FFmpeg filtergraph applied to the frames before they reach this
        /// encoder. Requires FFmpeg in PATH.
        ffmpeg_filter:     Option<String>,
        /// Append the source's colour and HDR metadata to the encoder
        /// parameters.
        #[serde(default = "crate::models::sequence::scene_concatenator::common::default_true")]
        append_color_data: bool,
    },
    X264 {
        executable:        Option<PathBuf>,
        pass:              EncoderPasses,
        options:           HashMap<String, CLIParameter>,
        /// FFmpeg filtergraph applied to the frames before they reach this
        /// encoder. Requires FFmpeg in PATH.
        ffmpeg_filter:     Option<String>,
        /// Append the source's colour and HDR metadata to the encoder
        /// parameters.
        #[serde(default = "crate::models::sequence::scene_concatenator::common::default_true")]
        append_color_data: bool,
    },
    X265 {
        executable:        Option<PathBuf>,
        pass:              EncoderPasses,
        options:           HashMap<String, CLIParameter>,
        /// FFmpeg filtergraph applied to the frames before they reach this
        /// encoder. Requires FFmpeg in PATH.
        ffmpeg_filter:     Option<String>,
        /// Append the source's colour and HDR metadata to the encoder
        /// parameters.
        #[serde(default = "crate::models::sequence::scene_concatenator::common::default_true")]
        append_color_data: bool,
    },
    VVenC {
        executable:        Option<PathBuf>,
        pass:              EncoderPasses,
        options:           HashMap<String, CLIParameter>,
        /// FFmpeg filtergraph applied to the frames before they reach this
        /// encoder. Requires FFmpeg in PATH.
        ffmpeg_filter:     Option<String>,
        /// Append the source's colour and HDR metadata to the encoder
        /// parameters.
        #[serde(default = "crate::models::sequence::scene_concatenator::common::default_true")]
        append_color_data: bool,
    },
    FFmpeg {
        executable:        Option<PathBuf>,
        options:           HashMap<String, CLIParameter>,
        /// Append the source's colour and HDR metadata to the encoder
        /// parameters.
        #[serde(default = "crate::models::sequence::scene_concatenator::common::default_true")]
        append_color_data: bool,
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
            Encoder::AVM {
                ..
            } => EncoderBase::AVM,
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
                ffmpeg_filter: None,
                append_color_data: true,
            },
            EncoderBase::RAV1E => Encoder::RAV1E {
                executable: None,
                pass,
                options,
                photon_noise: None,
                ffmpeg_filter: None,
                append_color_data: true,
            },
            EncoderBase::VPX => Encoder::VPX {
                executable: None,
                pass,
                options,
                ffmpeg_filter: None,
                append_color_data: true,
            },
            EncoderBase::SVTAV1 => Encoder::SVTAV1 {
                executable: None,
                pass,
                options,
                photon_noise: None,
                ffmpeg_filter: None,
                append_color_data: true,
            },
            EncoderBase::AVM => Encoder::AVM {
                executable: None,
                pass,
                options,
                photon_noise: None,
                ffmpeg_filter: None,
                append_color_data: true,
            },
            EncoderBase::X264 => Encoder::X264 {
                executable: None,
                pass,
                options,
                ffmpeg_filter: None,
                append_color_data: true,
            },
            EncoderBase::X265 => Encoder::X265 {
                executable: None,
                pass,
                options,
                ffmpeg_filter: None,
                append_color_data: true,
            },
            EncoderBase::VVenC => Encoder::VVenC {
                executable: None,
                pass,
                options,
                ffmpeg_filter: None,
                append_color_data: true,
            },
            EncoderBase::FFmpeg => Encoder::FFmpeg {
                executable: None,
                options,
                append_color_data: true,
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
            Encoder::AVM {
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
            Encoder::AVM {
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
            Encoder::AVM {
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
            Encoder::AVM {
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

    /// The FFmpeg filtergraph applied to this encoder's frames, if any.
    ///
    /// Always `None` for [`Encoder::FFmpeg`], which filters through `options`.
    #[inline]
    pub fn ffmpeg_filter(&self) -> Option<&str> {
        match self {
            Encoder::AOM {
                ffmpeg_filter, ..
            }
            | Encoder::RAV1E {
                ffmpeg_filter, ..
            }
            | Encoder::VPX {
                ffmpeg_filter, ..
            }
            | Encoder::SVTAV1 {
                ffmpeg_filter, ..
            }
            | Encoder::AVM {
                ffmpeg_filter, ..
            }
            | Encoder::X264 {
                ffmpeg_filter, ..
            }
            | Encoder::X265 {
                ffmpeg_filter, ..
            }
            | Encoder::VVenC {
                ffmpeg_filter, ..
            } => ffmpeg_filter.as_deref(),
            Encoder::FFmpeg {
                ..
            } => None,
        }
    }

    /// The encoder's Photon Noise table, if the base carries one.
    #[inline]
    pub fn photon_noise(&self) -> Option<&PhotonNoise> {
        match self {
            Encoder::AOM {
                photon_noise, ..
            }
            | Encoder::RAV1E {
                photon_noise, ..
            }
            | Encoder::SVTAV1 {
                photon_noise, ..
            }
            | Encoder::AVM {
                photon_noise, ..
            } => photon_noise.as_ref(),
            _ => None,
        }
    }

    /// The custom encoder binary path, if one was configured.
    #[inline]
    pub fn custom_executable(&self) -> Option<&PathBuf> {
        match self {
            Encoder::AOM {
                executable, ..
            }
            | Encoder::RAV1E {
                executable, ..
            }
            | Encoder::VPX {
                executable, ..
            }
            | Encoder::SVTAV1 {
                executable, ..
            }
            | Encoder::AVM {
                executable, ..
            }
            | Encoder::X264 {
                executable, ..
            }
            | Encoder::X265 {
                executable, ..
            }
            | Encoder::VVenC {
                executable, ..
            }
            | Encoder::FFmpeg {
                executable, ..
            } => executable.as_ref(),
        }
    }

    /// Sets the encoder binary, or restores the base's default when `None`.
    #[inline]
    pub fn set_executable(&mut self, executable: Option<PathBuf>) {
        match self {
            Encoder::AOM {
                executable: binary,
                ..
            }
            | Encoder::RAV1E {
                executable: binary,
                ..
            }
            | Encoder::VPX {
                executable: binary,
                ..
            }
            | Encoder::SVTAV1 {
                executable: binary,
                ..
            }
            | Encoder::AVM {
                executable: binary,
                ..
            }
            | Encoder::X264 {
                executable: binary,
                ..
            }
            | Encoder::X265 {
                executable: binary,
                ..
            }
            | Encoder::VVenC {
                executable: binary,
                ..
            }
            | Encoder::FFmpeg {
                executable: binary,
                ..
            } => *binary = executable,
        }
    }

    /// Sets the FFmpeg filtergraph applied to this encoder's frames.
    ///
    /// A no-op for [`Encoder::FFmpeg`], which filters through `options`.
    #[inline]
    pub fn set_ffmpeg_filter(&mut self, filter: Option<String>) {
        match self {
            Encoder::AOM {
                ffmpeg_filter, ..
            }
            | Encoder::RAV1E {
                ffmpeg_filter, ..
            }
            | Encoder::VPX {
                ffmpeg_filter, ..
            }
            | Encoder::SVTAV1 {
                ffmpeg_filter, ..
            }
            | Encoder::AVM {
                ffmpeg_filter, ..
            }
            | Encoder::X264 {
                ffmpeg_filter, ..
            }
            | Encoder::X265 {
                ffmpeg_filter, ..
            }
            | Encoder::VVenC {
                ffmpeg_filter, ..
            } => *ffmpeg_filter = filter,
            Encoder::FFmpeg {
                ..
            } => {},
        }
    }

    /// Whether the source's colour and HDR metadata is appended to the encoder
    /// parameters.
    #[inline]
    pub fn append_color_data(&self) -> bool {
        match self {
            Encoder::AOM {
                append_color_data, ..
            }
            | Encoder::RAV1E {
                append_color_data, ..
            }
            | Encoder::VPX {
                append_color_data, ..
            }
            | Encoder::SVTAV1 {
                append_color_data, ..
            }
            | Encoder::AVM {
                append_color_data, ..
            }
            | Encoder::X264 {
                append_color_data, ..
            }
            | Encoder::X265 {
                append_color_data, ..
            }
            | Encoder::VVenC {
                append_color_data, ..
            }
            | Encoder::FFmpeg {
                append_color_data, ..
            } => *append_color_data,
        }
    }

    /// Sets whether the source's colour and HDR metadata is appended to the
    /// encoder parameters.
    #[inline]
    pub fn set_append_color_data(&mut self, append: bool) {
        match self {
            Encoder::AOM {
                append_color_data, ..
            }
            | Encoder::RAV1E {
                append_color_data, ..
            }
            | Encoder::VPX {
                append_color_data, ..
            }
            | Encoder::SVTAV1 {
                append_color_data, ..
            }
            | Encoder::AVM {
                append_color_data, ..
            }
            | Encoder::X264 {
                append_color_data, ..
            }
            | Encoder::X265 {
                append_color_data, ..
            }
            | Encoder::VVenC {
                append_color_data, ..
            }
            | Encoder::FFmpeg {
                append_color_data, ..
            } => *append_color_data = append,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum EncoderPasses {
    All(u8),
    Specific(u8, u8),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_base_except_ffmpeg_round_trips_a_filter() {
        for base in [
            EncoderBase::AOM,
            EncoderBase::RAV1E,
            EncoderBase::VPX,
            EncoderBase::SVTAV1,
            EncoderBase::AVM,
            EncoderBase::X264,
            EncoderBase::X265,
            EncoderBase::VVenC,
        ] {
            let mut encoder = Encoder::default_from_base(&base, false);
            assert_eq!(encoder.ffmpeg_filter(), None, "{base} starts unfiltered");
            encoder.set_ffmpeg_filter(Some("crop=iw-16:ih-16".to_owned()));
            assert_eq!(
                encoder.ffmpeg_filter(),
                Some("crop=iw-16:ih-16"),
                "{base} keeps the filtergraph"
            );
            encoder.set_ffmpeg_filter(None);
            assert_eq!(
                encoder.ffmpeg_filter(),
                None,
                "{base} clears the filtergraph"
            );
        }
    }

    #[test]
    fn the_ffmpeg_encoder_ignores_a_filter() {
        let mut encoder = Encoder::default_from_base(&EncoderBase::FFmpeg, false);
        encoder.set_ffmpeg_filter(Some("crop=iw-16:ih-16".to_owned()));
        assert_eq!(encoder.ffmpeg_filter(), None);
    }

    #[test]
    fn a_filter_survives_serialization() {
        let mut encoder = Encoder::default_from_base(&EncoderBase::X264, false);
        encoder.set_ffmpeg_filter(Some("eq=brightness=0.05".to_owned()));
        let json = serde_json::to_string(&encoder).expect("encoder serializes");
        let restored: Encoder = serde_json::from_str(&json).expect("encoder deserializes");
        assert_eq!(restored.ffmpeg_filter(), Some("eq=brightness=0.05"));
    }

    #[test]
    fn a_config_without_a_filter_deserializes() {
        let json = r#"{"X264":{"executable":null,"pass":{"All":1},"options":{}}}"#;
        let encoder: Encoder = serde_json::from_str(json).expect("encoder deserializes");
        assert_eq!(encoder.ffmpeg_filter(), None);
    }

    #[test]
    fn every_base_starts_appending_colour_data() {
        for base in [
            EncoderBase::AOM,
            EncoderBase::RAV1E,
            EncoderBase::VPX,
            EncoderBase::SVTAV1,
            EncoderBase::AVM,
            EncoderBase::X264,
            EncoderBase::X265,
            EncoderBase::VVenC,
            EncoderBase::FFmpeg,
        ] {
            assert!(
                Encoder::default_from_base(&base, false).append_color_data(),
                "{base} appends colour data by default"
            );
            assert!(
                Encoder::default_from_base(&base, true).append_color_data(),
                "{base} appends colour data even without default options"
            );
        }
    }

    #[test]
    fn a_config_without_the_colour_field_appends_colour_data() {
        let json = r#"{"X264":{"executable":null,"pass":{"All":1},"options":{}}}"#;
        let encoder: Encoder = serde_json::from_str(json).expect("encoder deserializes");
        assert!(encoder.append_color_data());
    }

    #[test]
    fn the_colour_field_round_trips() {
        for base in [EncoderBase::AOM, EncoderBase::VVenC, EncoderBase::FFmpeg] {
            let mut encoder = Encoder::default_from_base(&base, false);
            encoder.set_append_color_data(false);
            let json = serde_json::to_string(&encoder).expect("encoder serializes");
            let restored: Encoder = serde_json::from_str(&json).expect("encoder deserializes");
            assert!(!restored.append_color_data(), "{base}");
        }
    }
}
