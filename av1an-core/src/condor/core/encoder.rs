use std::{
    collections::{BTreeMap, HashMap},
    fmt::Display,
    io::{BufRead, BufReader, Cursor, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self},
        Arc,
        Mutex,
    },
    thread,
    time::Instant,
};

use anyhow::{bail, Result};
use av1_grain::{generate_photon_noise_params, GrainTableSegment, NoiseGenArgs, TransferFunction};
use cfg_if::cfg_if;
use nom::AsBytes;
use path_abs::PathInfo;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    broker::StringOrBytes,
    condor::{
        core::input::Input,
        data::encoding::{cli_parameter::CLIParameter, Encoder, EncoderBase, EncoderPasses},
    },
    parse::*,
};

const NULL_OUTPUT: &str = if cfg!(windows) { "nul" } else { "/dev/null" };

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
    pub fn validate(&self) -> Result<()> {
        which::which(self.executable())?;

        Ok(())
    }

    #[inline]
    pub fn executable(&self) -> &str {
        let (default_name, executable) = match self {
            Encoder::AOM {
                executable, ..
            } => ("aomenc", executable),
            Encoder::RAV1E {
                executable, ..
            } => ("rav1e", executable),
            Encoder::VPX {
                executable, ..
            } => ("vpxenc", executable),
            Encoder::SVTAV1 {
                executable, ..
            } => ("SvtAv1EncApp", executable),
            Encoder::X264 {
                executable, ..
            } => ("x264", executable),
            Encoder::X265 {
                executable, ..
            } => ("x265", executable),
            Encoder::VVenC {
                executable, ..
            } => ("vvenc", executable),
            Encoder::FFmpeg {
                executable, ..
            } => ("ffmpeg", executable),
        };

        // TODO: Update to rust 2024 for let chaining
        // if let Some(executable) = executable && executable.exists() {
        //     executable.to_str().expect("Executable name is not empty")
        if let Some(executable) = executable {
            if executable.exists() {
                return executable.to_str().expect("Executable name is not empty");
            }
        }

        default_name
    }

    #[inline]
    pub fn default_parameters(
        encoder: &EncoderBase,
        pass: (u8, u8),
        first_pass_file_name: &str,
    ) -> HashMap<String, CLIParameter> {
        match encoder {
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
                let mut parameters = CLIParameter::new_numbers("--", " ", &[
                    ("preset", 4.0),
                    ("keyint", 0.0),
                    ("scd", 0.0),
                    ("rc", 0.0),
                    ("crf", 25.0),
                    ("progress", 2.0),
                ]);

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
                let mut parameters =
                    CLIParameter::new_numbers("--", " ", &[("crf", 25.0), ("scenecut", 0.0)]);
                parameters.extend(CLIParameter::new_strings("--", "=", &[
                    ("preset", "slow"),
                    ("keyint", "infinite"),
                    ("log-level", "error"),
                    ("demuxer", "y4m"),
                ]));
                parameters.extend(CLIParameter::new_bools("--", &[("stitchable", true)]));

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
                let mut parameters = CLIParameter::new_numbers("--", " ", &[
                    ("crf", 25.0),
                    ("level-idc", 5.0),
                    ("keyint", -1.0),
                    ("scenecut", 0.0),
                ]);
                parameters.extend(CLIParameter::new_numbers("-", " ", &[("D", 10.0)]));
                parameters.extend(CLIParameter::new_bools("--", &[("y4m", true)]));
                parameters.extend(CLIParameter::new_strings("--", " ", &[("preset", "slow")]));

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
    pub fn parameters(&self, pass: (u8, u8), output: &PathBuf) -> Vec<String> {
        let output_path = output.to_str().expect("Output path should exist");
        let first_pass_file_name = output
            .file_stem()
            .expect("Output path is a file with an extension")
            .to_str()
            .expect("Output path is not empty");

        match self {
            Encoder::AOM {
                options, ..
            } => {
                let mut parameters = ["-".to_owned()].to_vec();
                let mut params =
                    Encoder::default_parameters(&self.base(), pass, first_pass_file_name);
                params.extend(options.clone());
                params.extend(CLIParameter::new_strings("-", " ", &[(
                    "o",
                    if pass.0 == pass.1 {
                        output_path
                    } else {
                        NULL_OUTPUT
                    },
                )]));
                // // Remove required parameters from options to avoid duplicates
                // params.remove("o");
                for (key, value) in params {
                    match value {
                        CLIParameter::String {
                            prefix,
                            value,
                            delimiter,
                        } => match delimiter.as_str() {
                            " " => {
                                parameters.push(format!("{}{}", prefix, key));
                                parameters.push(value);
                            },
                            _ => {
                                parameters.push(format!("{}{}{}{}", prefix, key, delimiter, value));
                            },
                        },
                        CLIParameter::Number {
                            prefix,
                            value,
                            delimiter,
                        } => match delimiter.as_str() {
                            " " => {
                                parameters.push(format!("{}{}", prefix, key));
                                parameters.push(value.to_string());
                            },
                            _ => {
                                parameters.push(format!("{}{}{}{}", prefix, key, delimiter, value));
                            },
                        },
                        CLIParameter::Bool {
                            prefix,
                            value,
                        } => {
                            if value {
                                parameters.push(format!("{}{}", prefix, key));
                            }
                        },
                    }
                }

                parameters
            },
            Encoder::RAV1E {
                options, ..
            } => {
                let mut parameters = ["-".to_owned()].to_vec();
                let mut params =
                    Encoder::default_parameters(&self.base(), pass, first_pass_file_name);
                params.extend(options.clone());
                params.extend(CLIParameter::new_strings("--", " ", &[(
                    "output",
                    if pass.0 == pass.1 {
                        output_path
                    } else {
                        NULL_OUTPUT
                    },
                )]));
                // // Remove required parameters from options to avoid duplicates
                // params.remove("output");
                for (key, value) in params {
                    match value {
                        CLIParameter::String {
                            prefix,
                            value,
                            delimiter,
                        } => match delimiter.as_str() {
                            " " => {
                                parameters.push(format!("{}{}", prefix, key));
                                parameters.push(value);
                            },
                            _ => {
                                parameters.push(format!("{}{}{}{}", prefix, key, delimiter, value));
                            },
                        },
                        CLIParameter::Number {
                            prefix,
                            value,
                            delimiter,
                        } => match delimiter.as_str() {
                            " " => {
                                parameters.push(format!("{}{}", prefix, key));
                                parameters.push(value.to_string());
                            },
                            _ => {
                                parameters.push(format!("{}{}{}{}", prefix, key, delimiter, value));
                            },
                        },
                        CLIParameter::Bool {
                            prefix,
                            value,
                        } => {
                            if value {
                                parameters.push(format!("{}{}", prefix, key));
                            }
                        },
                    }
                }

                parameters
            },
            Encoder::VPX {
                options, ..
            } => {
                let mut parameters = ["-".to_owned()].to_vec();
                let mut params =
                    Encoder::default_parameters(&self.base(), pass, first_pass_file_name);
                params.extend(options.clone());
                params.extend(CLIParameter::new_strings("-", " ", &[(
                    "o",
                    if pass.0 == pass.1 {
                        output_path
                    } else {
                        NULL_OUTPUT
                    },
                )]));
                // // Remove required parameters from options to avoid duplicates
                // params.remove("o");
                for (key, value) in params {
                    match value {
                        CLIParameter::String {
                            prefix,
                            value,
                            delimiter,
                        } => match delimiter.as_str() {
                            " " => {
                                parameters.push(format!("{}{}", prefix, key));
                                parameters.push(value);
                            },
                            _ => {
                                parameters.push(format!("{}{}{}{}", prefix, key, delimiter, value));
                            },
                        },
                        CLIParameter::Number {
                            prefix,
                            value,
                            delimiter,
                        } => match delimiter.as_str() {
                            " " => {
                                parameters.push(format!("{}{}", prefix, key));
                                parameters.push(value.to_string());
                            },
                            _ => {
                                parameters.push(format!("{}{}{}{}", prefix, key, delimiter, value));
                            },
                        },
                        CLIParameter::Bool {
                            prefix,
                            value,
                        } => {
                            if value {
                                parameters.push(format!("{}{}", prefix, key));
                            }
                        },
                    }
                }

                parameters
            },
            Encoder::SVTAV1 {
                options, ..
            } => {
                let mut parameters = Vec::new();
                let mut params =
                    Encoder::default_parameters(&self.base(), pass, first_pass_file_name);
                params.extend(options.clone());
                params.extend(CLIParameter::new_strings("-", " ", &[
                    ("i", "stdin"),
                    (
                        "b",
                        if pass.0 == pass.1 {
                            output_path
                        } else {
                            NULL_OUTPUT
                        },
                    ),
                ]));
                // // Remove required parameters from options to avoid duplicates
                // params.remove("i");
                // params.remove("b");
                for (key, value) in params {
                    match value {
                        CLIParameter::String {
                            prefix,
                            value,
                            delimiter,
                        } => match delimiter.as_str() {
                            " " => {
                                parameters.push(format!("{}{}", prefix, key));
                                parameters.push(value);
                            },
                            _ => {
                                parameters.push(format!("{}{}{}{}", prefix, key, delimiter, value));
                            },
                        },
                        CLIParameter::Number {
                            prefix,
                            value,
                            delimiter,
                        } => match delimiter.as_str() {
                            " " => {
                                parameters.push(format!("{}{}", prefix, key));
                                parameters.push(value.to_string());
                            },
                            _ => {
                                parameters.push(format!("{}{}{}{}", prefix, key, delimiter, value));
                            },
                        },
                        CLIParameter::Bool {
                            prefix,
                            value,
                        } => {
                            if value {
                                parameters.push(format!("{}{}", prefix, key));
                            }
                        },
                    }
                }

                parameters
            },
            Encoder::X264 {
                options, ..
            } => {
                let mut parameters = ["-".to_owned()].to_vec();
                let mut params =
                    Encoder::default_parameters(&self.base(), pass, first_pass_file_name);
                params.extend(options.clone());
                params.extend(CLIParameter::new_strings("-", " ", &[(
                    "o",
                    if pass.0 == pass.1 {
                        output_path
                    } else {
                        NULL_OUTPUT
                    },
                )]));
                // // Remove required parameters from options to avoid duplicates
                // params.remove("o");
                for (key, value) in params {
                    match value {
                        CLIParameter::String {
                            prefix,
                            value,
                            delimiter,
                        } => match delimiter.as_str() {
                            " " => {
                                parameters.push(format!("{}{}", prefix, key));
                                parameters.push(value);
                            },
                            _ => {
                                parameters.push(format!("{}{}{}{}", prefix, key, delimiter, value));
                            },
                        },
                        CLIParameter::Number {
                            prefix,
                            value,
                            delimiter,
                        } => match delimiter.as_str() {
                            " " => {
                                parameters.push(format!("{}{}", prefix, key));
                                parameters.push(value.to_string());
                            },
                            _ => {
                                parameters.push(format!("{}{}{}{}", prefix, key, delimiter, value));
                            },
                        },
                        CLIParameter::Bool {
                            prefix,
                            value,
                        } => {
                            if value {
                                parameters.push(format!("{}{}", prefix, key));
                            }
                        },
                    }
                }

                parameters
            },
            Encoder::X265 {
                options, ..
            } => {
                let mut parameters = ["-".to_owned()].to_vec();
                let mut params =
                    Encoder::default_parameters(&self.base(), pass, first_pass_file_name);
                params.extend(options.clone());
                params.extend(CLIParameter::new_strings("-", " ", &[(
                    "o",
                    if pass.0 == pass.1 {
                        output_path
                    } else {
                        NULL_OUTPUT
                    },
                )]));
                // // Remove required parameters from options to avoid duplicates
                // params.remove("o");
                for (key, value) in params {
                    match value {
                        CLIParameter::String {
                            prefix,
                            value,
                            delimiter,
                        } => match delimiter.as_str() {
                            " " => {
                                parameters.push(format!("{}{}", prefix, key));
                                parameters.push(value);
                            },
                            _ => {
                                parameters.push(format!("{}{}{}{}", prefix, key, delimiter, value));
                            },
                        },
                        CLIParameter::Number {
                            prefix,
                            value,
                            delimiter,
                        } => match delimiter.as_str() {
                            " " => {
                                parameters.push(format!("{}{}", prefix, key));
                                parameters.push(value.to_string());
                            },
                            _ => {
                                parameters.push(format!("{}{}{}{}", prefix, key, delimiter, value));
                            },
                        },
                        CLIParameter::Bool {
                            prefix,
                            value,
                        } => {
                            if value {
                                parameters.push(format!("{}{}", prefix, key));
                            }
                        },
                    }
                }

                parameters
            },
            Encoder::VVenC {
                ..
            } => todo!(),
            Encoder::FFmpeg {
                options, ..
            } => {
                let mut parameters: Vec<String> = ["-i".to_owned(), "-".to_owned()].to_vec();
                // Remove required parameters from options to avoid duplicates
                let mut params =
                    Encoder::default_parameters(&self.base(), pass, first_pass_file_name);
                params.extend(options.clone());

                if let Some(codec) = params.remove("c:v") {
                    parameters.push("-c:v".to_owned());
                    parameters.push(codec.to_string_value());
                }
                for (key, value) in params {
                    match value {
                        CLIParameter::String {
                            prefix,
                            value,
                            delimiter,
                        } => match delimiter.as_str() {
                            " " => {
                                parameters.push(format!("{}{}", prefix, key));
                                parameters.push(value);
                            },
                            _ => {
                                parameters.push(format!("{}{}{}{}", prefix, key, delimiter, value));
                            },
                        },
                        CLIParameter::Number {
                            prefix,
                            value,
                            delimiter,
                        } => match delimiter.as_str() {
                            " " => {
                                parameters.push(format!("{}{}", prefix, key));
                                parameters.push(value.to_string());
                            },
                            _ => {
                                parameters.push(format!("{}{}{}{}", prefix, key, delimiter, value));
                            },
                        },
                        CLIParameter::Bool {
                            prefix,
                            value,
                        } => {
                            if value {
                                parameters.push(format!("{}{}", prefix, key));
                            }
                        },
                    }
                }

                parameters.push(output_path.to_owned());
                parameters
            },
        }
    }

    #[inline]
    pub fn encode_with_input(
        &self,
        input: &mut Input,
        output: &PathBuf,
        start_frame: usize,
        end_frame: usize,
        progress_tx: mpsc::Sender<EncodeProgress>,
    ) -> Result<EncoderResult> {
        match self.passes() {
            EncoderPasses::All(1) => Ok(self.encode_pass_with_input(
                input,
                output,
                start_frame,
                end_frame,
                (1, 1),
                progress_tx,
            )?),
            EncoderPasses::Specific(current, total) => Ok(self.encode_pass_with_input(
                input,
                output,
                start_frame,
                end_frame,
                (current, total),
                progress_tx,
            )?),
            EncoderPasses::All(total) => {
                let mut result = EncoderResult {
                    encoder:    self.base(),
                    parameters: self.parameters((1, total), output),
                    status:     std::process::ExitStatus::default(),
                    stdout:     StringOrBytes::from(Vec::new()),
                    stderr:     StringOrBytes::from(Vec::new()),
                };
                for pass in 1..total {
                    let progress_tx = progress_tx.clone();
                    result = self.encode_pass_with_input(
                        input,
                        output,
                        start_frame,
                        end_frame,
                        (pass, total),
                        progress_tx,
                    )?;
                }

                Ok(result)
            },
        }
    }

    #[inline]
    pub fn encode_pass_with_input(
        &self,
        input: &mut Input,
        output: &PathBuf,
        start_frame: usize,
        end_frame: usize,
        pass: (u8, u8),
        progress_tx: mpsc::Sender<EncodeProgress>,
    ) -> Result<EncoderResult> {
        // TODO:
        // - Report mapped errors
        // - Report CPU, memory, disk usage - Performance testing required
        // - Support cancellation

        static BUFFER_CAPACITY: usize = 128;
        let base_encoder = self.base();

        let mut system = sysinfo::System::new();
        let mut encoder = Command::new(self.executable())
            .args(self.parameters(pass, output))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let pid = encoder.id();

        // Data produced and consumed throughout
        let mut frames_decoded = 0;
        let frames_encoded = Arc::new(AtomicU64::new(0));
        let frames_encoded_clone = Arc::clone(&frames_encoded);
        let stderr_output = Arc::new(Mutex::new(String::with_capacity(BUFFER_CAPACITY)));
        let stderr_output_clone = Arc::clone(&stderr_output);
        let (frame_tx, frame_rx) = mpsc::channel();

        let y4m_header = input.y4m_header(Some(end_frame - start_frame))?;
        let mut stdin = encoder.stdin.take().expect("Encoder should have STDIN");
        let stderr = encoder.stderr.take().expect("Encoder should have STDERR");

        let encoder_stderr_thread = thread::spawn(move || -> Result<_> {
            let mut reader = BufReader::new(stderr);
            let mut buf = Vec::with_capacity(BUFFER_CAPACITY);

            loop {
                match reader.read_until(b'\r', &mut buf) {
                    Ok(0) => break,
                    Ok(_) => {
                        let line = buf.clone();
                        system.refresh_all();
                        let usage = system.process(sysinfo::Pid::from_u32(pid)).map_or_else(
                            || EncodeResourceUsage {
                                cpu:    0.0,
                                memory: 0,
                            },
                            |process| EncodeResourceUsage {
                                cpu:    process.cpu_usage(),
                                memory: process.memory(),
                            },
                        );

                        if let Ok(line) = simdutf8::basic::from_utf8(&line) {
                            // This needs to be done before parse_encoded_frames, as it potentially
                            // mutates the string - Not sure if this applies anymore -Boats
                            let mut stderr_output_lock =
                                stderr_output_clone.lock().expect("mutex should acquire lock");
                            stderr_output_lock.push_str(line);
                            stderr_output_lock.push('\n');

                            if let Some(latest_frame) =
                                Encoder::parse_encoded_frames_base(&base_encoder, line)
                            {
                                if latest_frame > frames_encoded_clone.load(Ordering::Relaxed) {
                                    frames_encoded_clone.store(latest_frame, Ordering::Relaxed);

                                    progress_tx
                                        .send(EncodeProgress {
                                            pass,
                                            frame: latest_frame as usize,
                                            usage,
                                        })
                                        .map_err(|_| {
                                            anyhow::Error::msg("Failed to send progress")
                                        })?;
                                }
                            }
                        }

                        buf.clear();
                    },
                    Err(e) => return Err(e.into()),
                }
            }

            Ok(())
        });

        let encoder_stdin_thread = thread::spawn(move || -> Result<()> {
            stdin.write_all(y4m_header.as_bytes())?;
            let mut frames_sent = 0;

            loop {
                if frames_sent >= (end_frame - start_frame) {
                    break;
                }

                let frame: Vec<u8> = frame_rx.recv()?;
                stdin.write_all(frame.as_bytes())?;
                frames_sent += 1;
            }

            stdin.flush()?;
            drop(stdin);
            Ok(())
        });

        // Decode frames in main thread
        let decode_start_time = Instant::now();
        for index in start_frame..end_frame {
            let frame = input.y4m_frame(index)?;
            frames_decoded += 1;
            frame_tx.send(frame.into_inner())?;
        }
        let decode_duration = decode_start_time.elapsed();

        // drop(stdin_tx);
        // drop(stderr_tx);
        encoder_stdin_thread
            .join()
            .map_err(|_| anyhow::Error::msg("Failed to join STDIN thread"))?
            .ok();
        encoder_stderr_thread
            .join()
            .map_err(|_| anyhow::Error::msg("Failed to join STDERR thread"))?
            .ok();

        let encoder_output = encoder.wait_with_output().expect("Encoder should finish");
        let encode_duration = decode_start_time.elapsed();

        let encoder_result = EncoderResult {
            encoder:    self.base(),
            parameters: self.parameters(pass, output),
            status:     encoder_output.status,
            stdout:     StringOrBytes::from(encoder_output.stdout),
            stderr:     stderr_output.lock().expect("mutex should acquire lock").clone().into(),
        };

        if !encoder_output.status.success() {
            return Err(encoder_result.into());
        }

        println!(
            "{}/{} frames decoded/encoded within {}/{} seconds",
            frames_decoded,
            // frames_encoded.lock().expect("mutex should acquire lock"),
            frames_encoded.load(Ordering::Relaxed),
            decode_duration.as_secs_f64(),
            encode_duration.as_secs_f64()
        );

        // println!(
        //     "STDOUT: {:?}",
        //     crate::broker::StringOrBytes::from(encoder_output.stdout)
        // );

        // println!("STDERR: {}", stderr_output);

        Ok(encoder_result)
    }

    #[inline]
    pub fn encode_with_stream(
        &self,
        frames_rx: crossbeam_channel::Receiver<Cursor<Vec<u8>>>,
        output: &PathBuf,
        progress_tx: mpsc::Sender<EncodeProgress>,
    ) -> Result<EncoderResult> {
        match self.passes() {
            EncoderPasses::All(1) => {
                progress_tx.send(EncodeProgress {
                    pass:  (1, 1),
                    frame: 0,
                    usage: EncodeResourceUsage::default(),
                })?;
                Ok(self.encode_pass_with_stream((1, 1), frames_rx, output, progress_tx)?)
            },
            EncoderPasses::Specific(current, total) => {
                progress_tx.send(EncodeProgress {
                    pass:  (current, total),
                    frame: 0,
                    usage: EncodeResourceUsage::default(),
                })?;
                Ok(self.encode_pass_with_stream(
                    (current, total),
                    frames_rx,
                    output,
                    progress_tx,
                )?)
            },
            EncoderPasses::All(total) => {
                let mut frame_senders: BTreeMap<u8, crossbeam_channel::Sender<Cursor<Vec<u8>>>> =
                    BTreeMap::new();
                let mut frame_receivers = BTreeMap::new();
                for pass in 1..=total {
                    let (frame_sender, frame_receiver) =
                        crossbeam_channel::unbounded::<Cursor<Vec<u8>>>();
                    frame_senders.insert(pass, frame_sender);
                    frame_receivers.insert(pass, frame_receiver);
                }
                let mut result = EncoderResult {
                    encoder:    self.base(),
                    parameters: self.parameters((1, total), output),
                    status:     std::process::ExitStatus::default(),
                    stdout:     StringOrBytes::from(Vec::new()),
                    stderr:     StringOrBytes::from(Vec::new()),
                };
                thread::spawn(move || -> Result<()> {
                    for frame in frames_rx {
                        for frame_sender in frame_senders.values() {
                            frame_sender.send(frame.clone())?;
                        }
                    }
                    Ok(())
                });
                for pass in 1..=total {
                    progress_tx.send(EncodeProgress {
                        pass:  (pass, total),
                        frame: 0,
                        usage: EncodeResourceUsage::default(),
                    })?;
                    let frames_rx = frame_receivers.remove(&pass).expect("receiver exists");
                    let progress_tx = progress_tx.clone();
                    result = self.encode_pass_with_stream(
                        (pass, total),
                        frames_rx,
                        output,
                        progress_tx,
                    )?;
                }
                Ok(result)
            },
        }
    }

    #[inline]
    pub fn encode_pass_with_stream(
        &self,
        pass: (u8, u8),
        frames_rx: crossbeam_channel::Receiver<Cursor<Vec<u8>>>,
        output: &PathBuf,
        progress_tx: mpsc::Sender<EncodeProgress>,
    ) -> Result<EncoderResult> {
        // TODO:
        // - Report mapped errors
        // - Report CPU, memory, disk usage - Done-ish
        // - Support cancellation

        static BUFFER_CAPACITY: usize = 128;
        let base_encoder = self.base();

        // println!("Encoding with params: {:?}", self.parameters(pass, output));
        // let mut system = sysinfo::System::new();
        let mut encoder = Command::new(self.executable())
            .args(self.parameters(pass, output))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        // let pid = encoder.id();

        // Data produced and consumed throughout
        let frames_encoded = Arc::new(AtomicU64::new(0));
        let frames_encoded_clone = Arc::clone(&frames_encoded);
        let stderr_output = Arc::new(Mutex::new(String::with_capacity(BUFFER_CAPACITY)));
        let stderr_output_clone = Arc::clone(&stderr_output);

        let mut stdin = encoder.stdin.take().expect("Encoder should have STDIN");
        let stderr = encoder.stderr.take().expect("Encoder should have STDERR");

        let encoder_stderr_thread = thread::spawn(move || -> Result<_> {
            let mut reader = BufReader::new(stderr);
            let mut buf = Vec::with_capacity(BUFFER_CAPACITY);

            loop {
                match reader.read_until(b'\r', &mut buf) {
                    Ok(0) => break,
                    Ok(_) => {
                        let line = buf.clone();
                        // let pid = sysinfo::Pid::from_u32(pid);
                        // let refresh_kind =
                        //     sysinfo::ProcessRefreshKind::nothing().with_cpu().with_memory();
                        // system.refresh_processes_specifics(
                        //     sysinfo::ProcessesToUpdate::Some(&[pid]),
                        //     true,
                        //     refresh_kind,
                        // );
                        // let usage = system.process(pid).map_or_else(
                        //     || EncodeResourceUsage {
                        //         cpu:    0.0,
                        //         memory: 0,
                        //     },
                        //     |process| EncodeResourceUsage {
                        //         cpu:    process.cpu_usage(),
                        //         memory: process.memory(),
                        //     },
                        // );
                        let usage = EncodeResourceUsage {
                            cpu:    0.0,
                            memory: 0,
                        };

                        if let Ok(line) = simdutf8::basic::from_utf8(&line) {
                            // This needs to be done before parse_encoded_frames, as it potentially
                            // mutates the string - Not sure if this applies anymore -Boats
                            let mut stderr_output_lock =
                                stderr_output_clone.lock().expect("mutex should acquire lock");
                            // println!("STDERR: {}", line);
                            stderr_output_lock.push_str(line);
                            stderr_output_lock.push('\n');

                            if let Some(latest_frame) =
                                Encoder::parse_encoded_frames_base(&base_encoder, line)
                            {
                                if latest_frame > frames_encoded_clone.load(Ordering::Relaxed) {
                                    frames_encoded_clone.store(latest_frame, Ordering::Relaxed);

                                    progress_tx
                                        .send(EncodeProgress {
                                            pass,
                                            frame: latest_frame as usize,
                                            usage,
                                        })
                                        .map_err(|_| {
                                            anyhow::Error::msg("Failed to send progress")
                                        })?;
                                }
                            }
                        }

                        buf.clear();
                    },
                    Err(e) => return Err(e.into()),
                }
            }

            Ok(())
        });

        let encoder_stdin_thread = thread::spawn(move || -> Result<()> {
            for frame in frames_rx {
                stdin.write_all(frame.into_inner().as_bytes())?;
            }

            stdin.flush()?;
            drop(stdin);
            Ok(())
        });

        let encoder_stdout_thread = thread::spawn(move || -> Result<_> {
            let encoder_output = encoder.wait_with_output().expect("Encoder should finish");
            Ok(encoder_output)
        });
        encoder_stdin_thread
            .join()
            .map_err(|_| anyhow::Error::msg("Failed to join STDIN thread"))?
            .ok();
        encoder_stderr_thread
            .join()
            .map_err(|_| anyhow::Error::msg("Failed to join STDERR thread"))?
            .ok();
        let encoder_output = encoder_stdout_thread
            .join()
            .map_err(|_| anyhow::Error::msg("Failed to join STDOUT thread"))??;

        let encoder_result = EncoderResult {
            encoder:    self.base(),
            parameters: self.parameters(pass, output),
            status:     encoder_output.status,
            stdout:     StringOrBytes::from(encoder_output.stdout),
            stderr:     stderr_output.lock().expect("mutex should acquire lock").clone().into(),
        };
        // if !encoder_output.status.success() {
        //     bail!(EncoderError::EncoderFailed(encoder_result.to_string()));
        // }

        Ok(encoder_result)
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
    pub fn total_passes(&self) -> u8 {
        match self.passes() {
            EncoderPasses::All(passes) => passes,
            EncoderPasses::Specific(_, _) => 1,
        }
    }

    #[inline]
    pub fn generate_photon_noise_table(
        &self,
        width: u32,
        height: u32,
        transfer_function: TransferFunction,
    ) -> Result<Option<(String, GrainTableSegment)>> {
        let photon_noise = match self {
            Encoder::AOM {
                photon_noise, ..
            } => photon_noise.as_ref(),
            Encoder::RAV1E {
                photon_noise, ..
            } => photon_noise.as_ref(),
            Encoder::SVTAV1 {
                photon_noise, ..
            } => photon_noise.as_ref(),
            _ => None,
        };
        if photon_noise.is_none() {
            return Ok(None);
        }
        let photon_noise = photon_noise.expect("photon noise should exist");
        let hash_name = photon_noise.hash_name();
        let mut params = generate_photon_noise_params(0, u64::MAX, NoiseGenArgs {
            iso_setting: photon_noise.iso,
            width: photon_noise.width.unwrap_or(width),
            height: photon_noise.height.unwrap_or(height),
            transfer_function,
            chroma_grain: photon_noise.chroma_iso.is_some_and(|c_iso| c_iso == photon_noise.iso),
            random_seed: None,
        });

        if let Some(chroma_iso) = photon_noise.chroma_iso {
            if chroma_iso != photon_noise.iso {
                let chroma_params = generate_photon_noise_params(0, u64::MAX, NoiseGenArgs {
                    iso_setting: chroma_iso,
                    width: photon_noise.width.unwrap_or(width),
                    height: photon_noise.height.unwrap_or(height),
                    transfer_function,
                    chroma_grain: true,
                    random_seed: None,
                });
                params.scaling_points_cr = chroma_params.scaling_points_cr;
                params.scaling_points_cb = chroma_params.scaling_points_cb;
            }
        }
        if let Some(cy) = &photon_noise.c_y {
            if cy.len() > 24 {
                anyhow::bail!("c_y must be at most 24 coefficients");
            }
            params.ar_coeffs_y = arrayvec::ArrayVec::<i8, 24>::from_iter(cy.iter().copied());
        }
        if let Some(ccb) = &photon_noise.ccb {
            if ccb.len() > 25 {
                anyhow::bail!("ccb must be at most 25 coefficients");
            }
            params.ar_coeffs_cb = arrayvec::ArrayVec::<i8, 25>::from_iter(ccb.iter().copied());
        }
        if let Some(ccr) = &photon_noise.ccr {
            if ccr.len() > 25 {
                anyhow::bail!("ccr must be at most 25 coefficients");
            }
            params.ar_coeffs_cr = arrayvec::ArrayVec::<i8, 25>::from_iter(ccr.iter().copied());
        }

        Ok(Some((hash_name, params)))
    }

    #[inline]
    pub fn apply_photon_noise_parameters(&mut self, table_path: &Path) -> Result<()> {
        match self {
            Encoder::AOM {
                options, ..
            } => {
                options.insert(
                    "film-grain-table".to_owned(),
                    CLIParameter::new_string("--", "=", &format!("{}", table_path.display())),
                );
            },
            Encoder::RAV1E {
                options, ..
            } => {
                options.insert(
                    "photon-noise-table".to_owned(),
                    CLIParameter::new_string("--", " ", &format!("{}", table_path.display())),
                );
            },
            Encoder::SVTAV1 {
                options, ..
            } => {
                options.insert(
                    "fgs-table".to_owned(),
                    CLIParameter::new_string("--", " ", &format!("{}", table_path.display())),
                );
            },
            _ => bail!(EncoderError::PhotonNoiseNotSupported(self.base())),
        }

        Ok(())
    }

    #[inline]
    pub fn parse_encoded_frames_base(encoder: &EncoderBase, line: &str) -> Option<u64> {
        match encoder {
            EncoderBase::AOM | EncoderBase::VPX => {
                cfg_if! {
                    if #[cfg(any(target_arch = "x86", target_arch = "x86_64"))] {
                        if is_x86_feature_detected!("sse4.1") && is_x86_feature_detected!("ssse3") {
                            // SAFETY: We verified that the CPU has the required feature set
                            return unsafe { parse_aom_vpx_frames_sse41(line.as_bytes()) };
                        }
                    }
                }

                parse_aom_vpx_frames(line)
            },
            EncoderBase::RAV1E => parse_rav1e_frames(line),
            EncoderBase::SVTAV1 => parse_svt_av1_frames(line),
            EncoderBase::X264 | EncoderBase::X265 => parse_x26x_frames(line),
            EncoderBase::VVenC => todo!(),
            EncoderBase::FFmpeg => todo!(),
        }
    }

    #[inline]
    pub fn parse_encoded_frames(&self, line: &str) -> Option<u64> {
        Self::parse_encoded_frames_base(&self.base(), line)
    }

    #[inline]
    pub fn output_extension_base(encoder: &EncoderBase) -> &'static str {
        match encoder {
            EncoderBase::AOM | EncoderBase::RAV1E | EncoderBase::VPX | EncoderBase::SVTAV1 => "ivf",
            EncoderBase::X264 => "264",
            EncoderBase::X265 => "hevc",
            EncoderBase::VVenC => todo!(),
            EncoderBase::FFmpeg => ".mkv",
        }
    }

    #[inline]
    pub fn output_extension(&self) -> &'static str {
        Self::output_extension_base(&self.base())
    }
}

impl Default for Encoder {
    #[inline]
    fn default() -> Self {
        Self::SVTAV1 {
            executable:   None,
            pass:         EncoderPasses::All(1),
            options:      HashMap::new(),
            photon_noise: None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct EncodeProgress {
    pub pass:  (u8, u8),
    pub frame: usize,
    pub usage: EncodeResourceUsage,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct EncodeResourceUsage {
    pub cpu:    f32,
    pub memory: u64,
}

#[derive(Debug, Error, Clone)]
pub struct EncoderResult {
    pub encoder:    EncoderBase,
    pub parameters: Vec<String>,
    pub status:     std::process::ExitStatus,
    pub stdout:     StringOrBytes,
    pub stderr:     StringOrBytes,
}

impl Display for EncoderResult {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Encoder ({} {}) exited with {}\nSTDOUT:\n{:#?}\nSTDERR:\n{:#?}",
            self.encoder,
            self.parameters.join(" "),
            self.status,
            self.stdout,
            self.stderr
        )
    }
}

#[derive(Debug, Clone, Error)]
pub enum EncoderError {
    #[error("Encoder failed: {0}")]
    EncoderFailed(EncoderResult),
    // #[error("encoder failed to start")]
    // EncoderFailedToStart,
    // #[error("encoder failed to finish")]
    // EncoderFailedToFinish,
    // #[error("encoder failed to encode frame")]
    // EncoderFailedToEncodeFrame,
    #[error("Encoder {0} does not support photon noise")]
    PhotonNoiseNotSupported(EncoderBase),
}
