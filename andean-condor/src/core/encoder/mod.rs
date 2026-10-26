use std::{
    collections::{BTreeMap, HashMap},
    fmt::Display,
    io::{BufRead, BufReader, Cursor, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc::{self},
    },
    thread,
    time::Instant,
};

use anyhow::{Result, bail};
use av1_grain::{GrainTableSegment, NoiseGenArgs, TransferFunction, generate_photon_noise_params};
use cfg_if::cfg_if;
use nom::AsBytes;
use path_abs::PathInfo;
use thiserror::Error;
use tracing::info;

use crate::{
    core::input::{Input, clip_info::ClipInfo, color_range::ColorRange},
    models::encoder::{
        Encoder,
        EncoderBase,
        EncoderPasses,
        cli_parameter::CLIParameter,
        photon_noise::PhotonNoise,
    },
};

pub mod capability;
pub mod color_data;
pub mod parse;
pub mod string_or_bytes;
pub use capability::{Container, EncoderCapability};
pub use parse::*;
pub use string_or_bytes::StringOrBytes;

const NULL_OUTPUT: &str = if cfg!(windows) { "nul" } else { "/dev/null" };

impl Encoder {
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
            Encoder::AVM {
                executable, ..
            } => ("avmenc", executable),
            Encoder::X264 {
                executable, ..
            } => ("x264", executable),
            Encoder::X265 {
                executable, ..
            } => ("x265", executable),
            Encoder::VVenC {
                executable, ..
            } => ("vvencapp", executable),
            Encoder::FFmpeg {
                executable, ..
            } => ("ffmpeg", executable),
        };

        if let Some(executable) = executable
            && executable.exists()
        {
            return executable.to_str().expect("Executable name is not empty");
        }

        default_name
    }

    /// Checks if the encoder binary supports a specific capability,
    /// using the global capability cache to avoid repeated expensive probes.
    #[inline]
    pub fn supports_capability(&self, capability: EncoderCapability) -> bool {
        let Ok(executable_path) = which::which(self.executable()) else {
            return false;
        };

        capability::check_capability(self.base(), &executable_path, capability)
    }

    /// The container this encoder binary writes, or `None` when it cannot be
    /// split per scene (Matroska/WebM).
    ///
    /// A probe result is cached, so this is cheap after the first call.
    #[inline]
    pub fn container(&self) -> Option<Container> {
        let executable_path = which::which(self.executable()).ok()?;

        capability::encoder_container(self.base(), &executable_path)
    }

    #[inline]
    pub fn parameters_with_output(
        &self,
        pass: (u8, u8),
        output: &PathBuf,
        clip_info: &ClipInfo,
    ) -> Vec<String> {
        let output_path = output.to_str().expect("Output path should exist");
        let first_pass_file_name = output
            .file_stem()
            .expect("Output path is a file with an extension")
            .to_str()
            .expect("Output path is not empty");
        let pass_parameters = self.base().pass_parameters(pass, first_pass_file_name);
        let color_parameters = if self.append_color_data() {
            color_data::color_parameters(self.base(), clip_info)
        } else {
            HashMap::new()
        };

        match self {
            Encoder::AOM {
                options, ..
            } => {
                let mut parameters = ["-".to_owned()].to_vec();
                let mut params = pass_parameters;
                params.extend(color_parameters);
                params.extend(options.clone());
                params.extend(CLIParameter::new_strings("-", " ", &[(
                    "o",
                    if pass.0 == pass.1 {
                        output_path
                    } else {
                        NULL_OUTPUT
                    },
                )]));
                for (key, value) in params {
                    match value.to_string_pair(&key) {
                        (Some(prefixed_key), Some(separate_value)) => {
                            parameters.push(prefixed_key);
                            parameters.push(separate_value);
                        },
                        (Some(prefixed_and_delimited), None) => {
                            parameters.push(prefixed_and_delimited);
                        },
                        _ => (), // Boolean/Flag set to false, nothing to push
                    }
                }

                parameters
            },
            Encoder::RAV1E {
                options, ..
            } => {
                let mut parameters = ["-".to_owned()].to_vec();
                let mut params = pass_parameters;
                params.extend(color_parameters);
                params.extend(options.clone());
                params.extend(CLIParameter::new_strings("--", " ", &[(
                    "output",
                    if pass.0 == pass.1 {
                        output_path
                    } else {
                        NULL_OUTPUT
                    },
                )]));
                for (key, value) in params {
                    match value.to_string_pair(&key) {
                        (Some(prefixed_key), Some(separate_value)) => {
                            parameters.push(prefixed_key);
                            parameters.push(separate_value);
                        },
                        (Some(prefixed_and_delimited), None) => {
                            parameters.push(prefixed_and_delimited);
                        },
                        _ => (), // Boolean/Flag set to false, nothing to push
                    }
                }

                parameters
            },
            Encoder::VPX {
                options, ..
            } => {
                let mut parameters = ["-".to_owned()].to_vec();
                let mut params = pass_parameters;
                params.extend(color_parameters);
                params.extend(options.clone());
                params.extend(CLIParameter::new_strings("-", " ", &[(
                    "o",
                    if pass.0 == pass.1 {
                        output_path
                    } else {
                        NULL_OUTPUT
                    },
                )]));
                for (key, value) in params {
                    match value.to_string_pair(&key) {
                        (Some(prefixed_key), Some(separate_value)) => {
                            parameters.push(prefixed_key);
                            parameters.push(separate_value);
                        },
                        (Some(prefixed_and_delimited), None) => {
                            parameters.push(prefixed_and_delimited);
                        },
                        _ => (), // Boolean/Flag set to false, nothing to push
                    }
                }

                parameters
            },
            Encoder::SVTAV1 {
                options, ..
            } => {
                let mut parameters = Vec::new();
                let mut params = pass_parameters;
                params.extend(color_parameters);
                params.extend(options.clone());
                // Force progress 2. Progress 0-1 is unsupported
                params.extend(CLIParameter::new_numbers("--", " ", &[("progress", 2.0)]));
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
                for (key, value) in params {
                    match value.to_string_pair(&key) {
                        (Some(prefixed_key), Some(separate_value)) => {
                            parameters.push(prefixed_key);
                            parameters.push(separate_value);
                        },
                        (Some(prefixed_and_delimited), None) => {
                            parameters.push(prefixed_and_delimited);
                        },
                        _ => (), // Boolean/Flag set to false, nothing to push
                    }
                }

                parameters
            },
            Encoder::AVM {
                options, ..
            } => {
                let mut parameters = ["-".to_owned()].to_vec();
                let mut params = pass_parameters;
                params.extend(color_parameters);
                params.extend(options.clone());
                params.extend(CLIParameter::new_strings("-", " ", &[(
                    "o",
                    if pass.0 == pass.1 {
                        output_path
                    } else {
                        NULL_OUTPUT
                    },
                )]));
                for (key, value) in params {
                    match value.to_string_pair(&key) {
                        (Some(prefixed_key), Some(separate_value)) => {
                            parameters.push(prefixed_key);
                            parameters.push(separate_value);
                        },
                        (Some(prefixed_and_delimited), None) => {
                            parameters.push(prefixed_and_delimited);
                        },
                        _ => (), // Boolean/Flag set to false, nothing to push
                    }
                }

                parameters
            },
            Encoder::X264 {
                options, ..
            } => {
                let mut parameters = ["-".to_owned()].to_vec();
                let mut params = pass_parameters;
                params.extend(color_parameters);
                params.extend(options.clone());
                params.extend(CLIParameter::new_strings("-", " ", &[(
                    "o",
                    if pass.0 == pass.1 {
                        output_path
                    } else {
                        NULL_OUTPUT
                    },
                )]));
                for (key, value) in params {
                    match value.to_string_pair(&key) {
                        (Some(prefixed_key), Some(separate_value)) => {
                            parameters.push(prefixed_key);
                            parameters.push(separate_value);
                        },
                        (Some(prefixed_and_delimited), None) => {
                            parameters.push(prefixed_and_delimited);
                        },
                        _ => (), // Boolean/Flag set to false, nothing to push
                    }
                }

                parameters
            },
            Encoder::X265 {
                options, ..
            } => {
                let mut parameters = ["-".to_owned()].to_vec();
                let mut params = pass_parameters;
                params.extend(color_parameters);
                params.extend(options.clone());
                params.extend(CLIParameter::new_strings("-", " ", &[(
                    "o",
                    if pass.0 == pass.1 {
                        output_path
                    } else {
                        NULL_OUTPUT
                    },
                )]));
                for (key, value) in params {
                    match value.to_string_pair(&key) {
                        (Some(prefixed_key), Some(separate_value)) => {
                            parameters.push(prefixed_key);
                            parameters.push(separate_value);
                        },
                        (Some(prefixed_and_delimited), None) => {
                            parameters.push(prefixed_and_delimited);
                        },
                        _ => (), // Boolean/Flag set to false, nothing to push
                    }
                }

                parameters
            },
            Encoder::VVenC {
                options, ..
            } => {
                let mut parameters = ["-i".to_owned(), "-".to_owned()].to_vec();
                let mut params = pass_parameters;
                params.extend(color_parameters);
                params.extend(options.clone());
                params.extend(CLIParameter::new_strings("-", " ", &[(
                    "o",
                    if pass.0 == pass.1 {
                        output_path
                    } else {
                        NULL_OUTPUT
                    },
                )]));
                for (key, value) in params {
                    match value.to_string_pair(&key) {
                        (Some(prefixed_key), Some(separate_value)) => {
                            parameters.push(prefixed_key);
                            parameters.push(separate_value);
                        },
                        (Some(prefixed_and_delimited), None) => {
                            parameters.push(prefixed_and_delimited);
                        },
                        _ => (), // Boolean/Flag set to false, nothing to push
                    }
                }

                parameters
            },
            Encoder::FFmpeg {
                options, ..
            } => {
                let mut params = pass_parameters;
                params.extend(color_parameters);
                params.extend(options.clone());

                // FFmpeg re-derives the encoder context's colour properties
                // from each frame (`fftools/ffmpeg_enc.c`), so applying them
                // as output options is a no-op. They have to describe the
                // input instead, which also keeps a user override effective
                // because `params` already carries their value.
                let mut parameters = Vec::new();
                for key in color_data::ffmpeg_input_parameters() {
                    if let Some(value) = params.remove(*key)
                        && let (Some(prefixed_key), Some(separate_value)) =
                            value.to_string_pair(key)
                    {
                        parameters.push(prefixed_key);
                        parameters.push(separate_value);
                    }
                }

                parameters.push("-i".to_owned());
                parameters.push("-".to_owned());

                if let Some(codec) = params.remove("c:v") {
                    parameters.push("-c:v".to_owned());
                    parameters.push(codec.to_string_value());
                }
                for (key, value) in params {
                    match value.to_string_pair(&key) {
                        (Some(prefixed_key), Some(separate_value)) => {
                            parameters.push(prefixed_key);
                            parameters.push(separate_value);
                        },
                        (Some(prefixed_and_delimited), None) => {
                            parameters.push(prefixed_and_delimited);
                        },
                        _ => (), // Boolean/Flag set to false, nothing to push
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
                let clip_info = input.clip_info()?;
                let mut result = EncoderResult {
                    encoder:    self.base(),
                    parameters: self.parameters_with_output((1, total), output, &clip_info),
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
        let clip_info = input.clip_info()?;

        let mut system = sysinfo::System::new();
        let mut encoder = Command::new(self.executable())
            .args(self.parameters_with_output(pass, output, &clip_info))
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
                                && latest_frame > frames_encoded_clone.load(Ordering::Relaxed)
                            {
                                frames_encoded_clone.store(latest_frame, Ordering::Relaxed);

                                progress_tx
                                    .send(EncodeProgress {
                                        pass,
                                        frame: latest_frame as usize,
                                        usage,
                                    })
                                    .map_err(|_| anyhow::Error::msg("Failed to send progress"))?;
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
            parameters: self.parameters_with_output(pass, output, &clip_info),
            status:     encoder_output.status,
            stdout:     StringOrBytes::from(encoder_output.stdout),
            stderr:     stderr_output.lock().expect("mutex should acquire lock").clone().into(),
        };

        if !encoder_output.status.success() {
            return Err(encoder_result.into());
        }

        info!(
            "{}/{} frames decoded/encoded within {}/{} seconds",
            frames_decoded,
            // frames_encoded.lock().expect("mutex should acquire lock"),
            frames_encoded.load(Ordering::Relaxed),
            decode_duration.as_secs_f64(),
            encode_duration.as_secs_f64()
        );

        Ok(encoder_result)
    }

    #[inline]
    pub fn encode_with_stream(
        &self,
        frames_rx: crossbeam_channel::Receiver<Cursor<Vec<u8>>>,
        output: &PathBuf,
        progress_tx: mpsc::Sender<EncodeProgress>,
        clip_info: &ClipInfo,
    ) -> Result<EncoderResult> {
        match self.passes() {
            EncoderPasses::All(1) => {
                progress_tx.send(EncodeProgress {
                    pass:  (1, 1),
                    frame: 0,
                    usage: EncodeResourceUsage::default(),
                })?;
                Ok(self.encode_pass_with_stream(
                    (1, 1),
                    frames_rx,
                    output,
                    progress_tx,
                    clip_info,
                )?)
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
                    clip_info,
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
                    parameters: self.parameters_with_output((1, total), output, clip_info),
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
                        clip_info,
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
        clip_info: &ClipInfo,
    ) -> Result<EncoderResult> {
        // TODO:
        // - Report mapped errors
        // - Report CPU, memory, disk usage - Done-ish
        // - Support cancellation

        static BUFFER_CAPACITY: usize = 128;
        let base_encoder = self.base();
        let base_encoder_clone = base_encoder;

        // println!("Encoding with params: {:?}", self.parameters(pass, output));
        // let mut system = sysinfo::System::new();
        let mut encoder = Command::new(self.executable())
            .args(self.parameters_with_output(pass, output, clip_info))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        // let pid = encoder.id();

        // Data produced and consumed throughout
        let frames_encoded = Arc::new(AtomicU64::new(0));
        let frames_encoded_clone = Arc::clone(&frames_encoded);
        let stdout_frames_encoded_clone = Arc::clone(&frames_encoded);
        let stdout_output = Arc::new(Mutex::new(String::with_capacity(BUFFER_CAPACITY)));
        let stdout_output_clone = Arc::clone(&stdout_output);
        let stderr_output = Arc::new(Mutex::new(String::with_capacity(BUFFER_CAPACITY)));
        let stderr_output_clone = Arc::clone(&stderr_output);

        let mut stdin = encoder.stdin.take().expect("Encoder should have STDIN");
        let stdout = encoder.stdout.take().expect("Encoder should have STDOUT"); // For AVM progress only
        let stderr = encoder.stderr.take().expect("Encoder should have STDERR");

        let progress_tx_clone = progress_tx.clone();
        let encoder_stdout_thread = thread::spawn(move || -> Result<_> {
            if !matches!(base_encoder_clone, EncoderBase::AVM) {
                return Ok(());
            }

            let mut reader = BufReader::new(stdout);
            let mut buf = Vec::with_capacity(BUFFER_CAPACITY);
            let mut count = 0;

            loop {
                match reader.read_until(b'\r', &mut buf) {
                    Ok(0) => break,
                    Ok(_) => {
                        let line = buf.clone();
                        let usage = EncodeResourceUsage {
                            cpu:    0.0,
                            memory: 0,
                        };

                        if let Ok(line) = simdutf8::basic::from_utf8(&line) {
                            // This needs to be done before parse_encoded_frames, as it potentially
                            // mutates the string - Not sure if this applies anymore -Boats
                            let mut stdout_output_lock =
                                stdout_output_clone.lock().expect("mutex should acquire lock");
                            // println!("STDOUT: {}", line);
                            stdout_output_lock.push_str(line);
                            stdout_output_lock.push('\n');

                            // if let Some(latest_frame) =
                            //     Encoder::parse_encoded_frames_base(&base_encoder_clone, line)
                            //     && latest_frame
                            //         > stdout_frames_encoded_clone.load(Ordering::Relaxed)
                            // {
                            if line.contains("POC:") {
                                count += 1;
                                let latest_frame = count;
                                stdout_frames_encoded_clone.store(latest_frame, Ordering::Relaxed);

                                progress_tx_clone
                                    .send(EncodeProgress {
                                        pass,
                                        frame: latest_frame as usize,
                                        usage,
                                    })
                                    .map_err(|_| anyhow::Error::msg("Failed to send progress"))?;
                            }
                        }

                        buf.clear();
                    },
                    Err(e) => return Err(e.into()),
                }
            }

            Ok(())
        });

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
                                && latest_frame > frames_encoded_clone.load(Ordering::Relaxed)
                            {
                                frames_encoded_clone.store(latest_frame, Ordering::Relaxed);

                                progress_tx
                                    .send(EncodeProgress {
                                        pass,
                                        frame: latest_frame as usize,
                                        usage,
                                    })
                                    .map_err(|_| anyhow::Error::msg("Failed to send progress"))?;
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

        let encoder_output_thread = thread::spawn(move || -> Result<_> {
            let encoder_output = encoder.wait_with_output().expect("Encoder should finish");
            Ok(encoder_output)
        });
        encoder_stdin_thread
            .join()
            .map_err(|_| anyhow::Error::msg("Failed to join STDIN thread"))?
            .ok();
        encoder_stdout_thread
            .join()
            .map_err(|_| anyhow::Error::msg("Failed to join STDOUT thread"))?
            .ok();
        encoder_stderr_thread
            .join()
            .map_err(|_| anyhow::Error::msg("Failed to join STDERR thread"))?
            .ok();
        let encoder_output = encoder_output_thread
            .join()
            .map_err(|_| anyhow::Error::msg("Failed to join STDOUT thread"))??;

        let encoder_result = EncoderResult {
            encoder:    self.base(),
            parameters: self.parameters_with_output(pass, output, clip_info),
            status:     encoder_output.status,
            stdout:     StringOrBytes::from(encoder_output.stdout),
            stderr:     stderr_output.lock().expect("mutex should acquire lock").clone().into(),
        };

        Ok(encoder_result)
    }

    #[inline]
    pub fn set_photon_noise(&mut self, photon_noise: Option<PhotonNoise>) {
        match self {
            Encoder::AOM {
                photon_noise: pn, ..
            } => *pn = photon_noise,
            Encoder::RAV1E {
                photon_noise: pn, ..
            } => *pn = photon_noise,
            Encoder::SVTAV1 {
                photon_noise: pn, ..
            } => *pn = photon_noise,
            Encoder::AVM {
                photon_noise: pn, ..
            } => *pn = photon_noise,
            _ => {},
        }
    }

    #[inline]
    pub fn generate_photon_noise_table(
        &self,
        width: u32,
        height: u32,
        transfer_function: TransferFunction,
        color_range: Option<ColorRange>,
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
            full_range: matches!(color_range, Some(ColorRange::Full)),
        });

        if let Some(chroma_iso) = photon_noise.chroma_iso
            && chroma_iso != photon_noise.iso
        {
            let chroma_params = generate_photon_noise_params(0, u64::MAX, NoiseGenArgs {
                iso_setting: chroma_iso,
                width: photon_noise.width.unwrap_or(width),
                height: photon_noise.height.unwrap_or(height),
                transfer_function,
                chroma_grain: true,
                random_seed: None,
                full_range: matches!(color_range, Some(ColorRange::Full)),
            });
            params.scaling_points_cr = chroma_params.scaling_points_cr;
            params.scaling_points_cb = chroma_params.scaling_points_cb;
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
    pub fn quantizer(&self) -> Option<f64> {
        match self {
            Encoder::AOM {
                options, ..
            } => options.get("cq-level").and_then(|p| match p {
                CLIParameter::Number {
                    value, ..
                } => Some(*value),
                _ => None,
            }),
            Encoder::RAV1E {
                options, ..
            } => options.get("quantizer").and_then(|p| match p {
                CLIParameter::Number {
                    value, ..
                } => Some(*value),
                _ => None,
            }),
            Encoder::SVTAV1 {
                options, ..
            } => options.get("crf").and_then(|p| match p {
                CLIParameter::Number {
                    value, ..
                } => Some(*value),
                _ => None,
            }),
            Encoder::AVM {
                options, ..
            } => options.get("qp").and_then(|p| match p {
                CLIParameter::Number {
                    value, ..
                } => Some(*value),
                _ => None,
            }),
            Encoder::VPX {
                options, ..
            } => options.get("cq-level").and_then(|p| match p {
                CLIParameter::Number {
                    value, ..
                } => Some(*value),
                _ => None,
            }),
            Encoder::X264 {
                options, ..
            } => options.get("crf").and_then(|p| match p {
                CLIParameter::Number {
                    value, ..
                } => Some(*value),
                _ => None,
            }),
            Encoder::X265 {
                options, ..
            } => options.get("crf").and_then(|p| match p {
                CLIParameter::Number {
                    value, ..
                } => Some(*value),
                _ => None,
            }),
            Encoder::VVenC {
                options, ..
            } => options.get("qp").and_then(|p| match p {
                CLIParameter::Number {
                    value, ..
                } => Some(*value),
                _ => None,
            }),
            Encoder::FFmpeg {
                options, ..
            } => options.get("crf").and_then(|p| match p {
                CLIParameter::Number {
                    value, ..
                } => Some(*value),
                _ => None,
            }),
        }
    }

    #[inline]
    pub fn set_quantizer(&mut self, quantizer: f64) {
        match self {
            Encoder::AOM {
                options, ..
            } => options.insert(
                "cq-level".to_owned(),
                CLIParameter::new_number("--", "=", quantizer),
            ),
            Encoder::RAV1E {
                options, ..
            } => options.insert(
                "quantizer".to_owned(),
                CLIParameter::new_number("--", " ", quantizer),
            ),
            Encoder::SVTAV1 {
                options, ..
            } => options.insert(
                "crf".to_owned(),
                CLIParameter::new_number("--", " ", quantizer),
            ),
            Encoder::AVM {
                options, ..
            } => options.insert(
                "qp".to_owned(),
                CLIParameter::new_number("--", "=", quantizer),
            ),
            Encoder::VPX {
                options, ..
            } => options.insert(
                "cq-level".to_owned(),
                CLIParameter::new_number("--", "=", quantizer),
            ),
            Encoder::X264 {
                options, ..
            } => options.insert(
                "crf".to_owned(),
                CLIParameter::new_number("--", " ", quantizer),
            ),
            Encoder::X265 {
                options, ..
            } => options.insert(
                "crf".to_owned(),
                CLIParameter::new_number("--", " ", quantizer),
            ),
            Encoder::VVenC {
                options, ..
            } => options.insert(
                "qp".to_owned(),
                CLIParameter::new_number("--", " ", quantizer),
            ),
            Encoder::FFmpeg {
                options, ..
            } => options.insert(
                "crf".to_owned(),
                CLIParameter::new_number("-", " ", quantizer),
            ),
        };
    }

    #[inline]
    pub fn set_speed(&mut self, speed: i8) {
        match self {
            Encoder::AOM {
                options, ..
            } => options.insert(
                "cpu-used".to_owned(),
                CLIParameter::new_number("--", "=", speed as f64),
            ),
            Encoder::RAV1E {
                options, ..
            } => options.insert(
                "speed".to_owned(),
                CLIParameter::new_number("--", " ", speed as f64),
            ),
            Encoder::SVTAV1 {
                options, ..
            } => options.insert(
                "preset".to_owned(),
                CLIParameter::new_number("--", " ", speed as f64),
            ),
            Encoder::AVM {
                options, ..
            } => options.insert(
                "cpu-used".to_owned(),
                CLIParameter::new_number("--", "=", speed as f64),
            ),
            Encoder::VPX {
                options, ..
            } => options.insert(
                "cpu-used".to_owned(),
                CLIParameter::new_number("--", "=", speed as f64),
            ),
            Encoder::X264 {
                options, ..
            } => options.insert(
                "preset".to_owned(),
                CLIParameter::new_string("--", " ", match speed {
                    i8::MIN..=0 => "placebo",
                    1 => "veryslow",
                    2 => "slower",
                    3 => "slow",
                    4 => "medium",
                    5 => "fast",
                    6 => "faster",
                    7 => "veryfast",
                    8 => "superfast",
                    9..=i8::MAX => "ultrafast",
                }),
            ),
            Encoder::X265 {
                options, ..
            } => options.insert(
                "preset".to_owned(),
                CLIParameter::new_string("--", " ", match speed {
                    i8::MIN..=0 => "placebo",
                    1 => "veryslow",
                    2 => "slower",
                    3 => "slow",
                    4 => "medium",
                    5 => "fast",
                    6 => "faster",
                    7 => "veryfast",
                    8 => "superfast",
                    9..=i8::MAX => "ultrafast",
                }),
            ),
            Encoder::VVenC {
                options, ..
            } => options.insert(
                "preset".to_owned(),
                CLIParameter::new_string("--", " ", match speed {
                    i8::MIN..=0 => "slower",
                    1 => "slow",
                    2 => "medium",
                    3 => "fast",
                    4..=i8::MAX => "faster",
                }),
            ),
            Encoder::FFmpeg {
                options, ..
            } => options.insert(
                "preset".to_owned(),
                CLIParameter::new_number("-", " ", speed as f64),
            ),
        };
    }

    #[inline]
    pub fn speed(&self) -> Option<String> {
        match self {
            Encoder::AOM {
                options, ..
            } => options.get("cpu-used").and_then(|p| match p {
                CLIParameter::Number {
                    value, ..
                } => Some(value.to_string()),
                _ => None,
            }),
            Encoder::RAV1E {
                options, ..
            } => options.get("speed").and_then(|p| match p {
                CLIParameter::Number {
                    value, ..
                } => Some(value.to_string()),
                _ => None,
            }),
            Encoder::SVTAV1 {
                options, ..
            } => options.get("preset").and_then(|p| match p {
                CLIParameter::Number {
                    value, ..
                } => Some(value.to_string()),
                _ => None,
            }),
            Encoder::AVM {
                options, ..
            } => options.get("cpu-used").and_then(|p| match p {
                CLIParameter::Number {
                    value, ..
                } => Some(value.to_string()),
                _ => None,
            }),
            Encoder::VPX {
                options, ..
            } => options.get("cpu-used").and_then(|p| match p {
                CLIParameter::Number {
                    value, ..
                } => Some(value.to_string()),
                _ => None,
            }),
            Encoder::X264 {
                options, ..
            } => options.get("preset").and_then(|p| match p {
                CLIParameter::String {
                    value, ..
                } => Some(value.clone()),
                _ => None,
            }),
            Encoder::X265 {
                options, ..
            } => options.get("preset").and_then(|p| match p {
                CLIParameter::String {
                    value, ..
                } => Some(value.clone()),
                CLIParameter::Number {
                    value, ..
                } => Some(value.to_string()),
                _ => None,
            }),
            Encoder::VVenC {
                options, ..
            } => options.get("preset").and_then(|p| match p {
                CLIParameter::String {
                    value, ..
                } => Some(value.clone()),
                CLIParameter::Number {
                    value, ..
                } => Some(value.to_string()),
                _ => None,
            }),
            Encoder::FFmpeg {
                options, ..
            } => options.get("preset").and_then(|p| match p {
                CLIParameter::String {
                    value, ..
                } => Some(value.clone()),
                CLIParameter::Number {
                    value, ..
                } => Some(value.to_string()),
                _ => None,
            }),
        }
    }

    #[inline]
    pub fn parse_encoded_frames_base(encoder: &EncoderBase, line: &str) -> Option<u64> {
        match encoder {
            EncoderBase::AOM | EncoderBase::VPX | EncoderBase::AVM => {
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
            EncoderBase::VVenC => parse_vvc_frames(line),
            EncoderBase::FFmpeg => parse_ffmpeg_frames(line),
        }
    }

    #[inline]
    pub fn parse_encoded_frames(&self, line: &str) -> Option<u64> {
        Self::parse_encoded_frames_base(&self.base(), line)
    }

    #[inline]
    pub fn version_text(&self) -> Option<String> {
        match self {
            Encoder::AOM {
                ..
            } => {
                let result = Command::new(self.executable()).arg("--help").output().ok()?;
                let stdout = String::from_utf8_lossy(&result.stdout);
                let version_line = stdout.lines().find(|line| line.starts_with("    av1"))?;
                Some(
                    version_line
                        .split_once('-')
                        .expect("unexpected aom version string format")
                        .1
                        .replace("(default)", "")
                        .trim()
                        .to_string(),
                )
            },
            Encoder::RAV1E {
                ..
            } => {
                let result = Command::new(self.executable()).arg("--version").output().ok()?;
                let stdout = String::from_utf8_lossy(&result.stdout);
                let version_line = stdout.lines().find(|line| line.starts_with("rav1e"))?;
                Some(version_line.to_string())
            },
            Encoder::VPX {
                ..
            } => {
                let result = Command::new(self.executable()).arg("--help").output().ok()?;
                let stdout = String::from_utf8_lossy(&result.stdout);
                let version_line = stdout.lines().find(|line| line.starts_with("    vp9"))?;
                Some(
                    version_line
                        .split_once('-')
                        .expect("unexpected vpx version string format")
                        .1
                        .replace("(default)", "")
                        .trim()
                        .to_string(),
                )
            },
            Encoder::SVTAV1 {
                ..
            } => {
                let result = Command::new(self.executable()).arg("--version").output().ok()?;
                let stdout = String::from_utf8_lossy(&result.stdout);
                let version_line = stdout.lines().find(|line| line.starts_with("SVT-AV1"))?;
                Some(version_line.to_string())
            },
            Encoder::AVM {
                ..
            } => {
                let result = Command::new(self.executable()).arg("--help").output().ok()?;
                let stdout = String::from_utf8_lossy(&result.stdout);
                let version_line = stdout.lines().find(|line| line.starts_with("    av2"))?;
                Some(
                    version_line
                        .split_once('-')
                        .expect("unexpected avm version string format")
                        .1
                        .replace("(default)", "")
                        .trim()
                        .to_string(),
                )
            },
            Encoder::X264 {
                ..
            } => {
                let result = Command::new(self.executable()).arg("--version").output().ok()?;
                let stdout = String::from_utf8_lossy(&result.stdout);
                let version_line = stdout.lines().find(|line| line.starts_with("x264"))?;
                Some(
                    version_line
                        .split_once(" ")
                        .expect("unexpected x264 version string format")
                        .1
                        .trim()
                        .to_string(),
                )
            },
            Encoder::X265 {
                ..
            } => {
                let result = Command::new(self.executable()).arg("--version").output().ok()?;
                let stderr = String::from_utf8_lossy(&result.stderr);
                let version_line = stderr.lines().find(|line| line.starts_with("x265"))?;
                Some(
                    version_line
                        .split_once(':')
                        .expect("unexpected x265 version string format")
                        .1
                        .trim()
                        .to_string(),
                )
            },
            Encoder::VVenC {
                ..
            } => {
                let result = Command::new(self.executable()).arg("--version").output().ok()?;
                let stdout = String::from_utf8_lossy(&result.stdout);
                let version_line = stdout.lines().find(|line| line.starts_with("vvencapp"))?;
                Some(version_line.to_string())
            },
            Encoder::FFmpeg {
                ..
            } => {
                let result = Command::new(self.executable()).output().ok()?;
                let stderr = String::from_utf8_lossy(&result.stderr);
                let version_line = stderr.lines().find(|line| line.starts_with("ffmpeg"))?;
                version_line.split(' ').nth(2).map(String::from)
            },
        }
    }
}

impl Default for Encoder {
    #[inline]
    fn default() -> Self {
        Self::SVTAV1 {
            executable:        None,
            pass:              EncoderPasses::All(1),
            options:           HashMap::new(),
            photon_noise:      None,
            ffmpeg_filter:     None,
            append_color_data: true,
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

#[cfg(test)]
mod tests {
    use av_decoders::{ColorPrimaries, ColorSpace, TransferCharacteristic};

    use super::*;

    fn clip_info() -> ClipInfo {
        ClipInfo {
            num_frames:                    24,
            format_info:
                crate::core::input::pixel_format::PixelFormat::VapourSynth {
                    bit_depth: 10
                },
            frame_rate:                    av_format::rational::Rational64::new(24, 1),
            resolution:                    (1920, 1080),
            color_range:                   None,
            color_primaries:               Some(ColorPrimaries::BT2020),
            color_transfer_characteristic: Some(TransferCharacteristic::SMPTE2084),
            color_space:                   Some(ColorSpace::BT2020NCL),
            chroma_location:               None,
            mastering_display:             None,
            content_light_level:           None,
        }
    }

    fn parameters(encoder: &Encoder) -> Vec<String> {
        encoder.parameters_with_output((1, 1), &PathBuf::from("output.ivf"), &clip_info())
    }

    /// The value of `flag` in an argv vector built with a separate-value
    /// delimiter.
    fn value_of<'a>(parameters: &'a [String], flag: &str) -> Option<&'a str> {
        parameters
            .iter()
            .position(|argument| argument == flag)
            .and_then(|index| parameters.get(index + 1))
            .map(String::as_str)
    }

    #[test]
    fn colour_data_is_appended_by_default() {
        let encoder = Encoder::default_from_base(&EncoderBase::SVTAV1, true);
        let parameters = parameters(&encoder);

        assert_eq!(value_of(&parameters, "--color-primaries"), Some("bt2020"));
        assert_eq!(
            value_of(&parameters, "--transfer-characteristics"),
            Some("smpte2084")
        );
        assert_eq!(
            value_of(&parameters, "--matrix-coefficients"),
            Some("bt2020-ncl")
        );
    }

    #[test]
    fn disabling_colour_data_appends_nothing() {
        let mut encoder = Encoder::default_from_base(&EncoderBase::SVTAV1, true);
        encoder.set_append_color_data(false);
        let parameters = parameters(&encoder);

        assert_eq!(value_of(&parameters, "--color-primaries"), None);
        assert_eq!(value_of(&parameters, "--transfer-characteristics"), None);
        assert_eq!(value_of(&parameters, "--matrix-coefficients"), None);
    }

    #[test]
    fn a_user_option_overrides_the_appended_colour_data() {
        let mut encoder = Encoder::default_from_base(&EncoderBase::SVTAV1, true);
        encoder.parameters_mut().insert(
            "color-primaries".to_owned(),
            CLIParameter::new_string("--", " ", "bt709"),
        );

        let parameters = parameters(&encoder);
        assert_eq!(value_of(&parameters, "--color-primaries"), Some("bt709"));
        assert!(
            !parameters.iter().any(|argument| argument == "bt2020"),
            "the appended value must be replaced rather than duplicated"
        );
    }

    #[test]
    fn a_user_option_overrides_the_appended_hdr_metadata() {
        let mut encoder = Encoder::default_from_base(&EncoderBase::X265, true);
        encoder.parameters_mut().insert(
            "max-cll".to_owned(),
            CLIParameter::new_string("--", " ", "400,200"),
        );
        let clip_info = ClipInfo {
            content_light_level: Some(av_decoders::ContentLightLevel {
                max_cll:  1000,
                max_fall: 400,
            }),
            ..clip_info()
        };

        let parameters =
            encoder.parameters_with_output((1, 1), &PathBuf::from("output.hevc"), &clip_info);
        assert_eq!(value_of(&parameters, "--max-cll"), Some("400,200"));
    }

    #[test]
    fn colour_data_follows_the_pass_parameters() {
        let mut encoder = Encoder::default_from_base(&EncoderBase::SVTAV1, true);
        encoder
            .passes_mut()
            .expect("svt-av1 takes passes")
            .clone_from(&EncoderPasses::All(2));

        let parameters =
            encoder.parameters_with_output((1, 2), &PathBuf::from("output.ivf"), &clip_info());

        // A two-pass SVT-AV1 command carries `--pass 1`; the colour data is
        // merged after it, and both are present.
        assert_eq!(value_of(&parameters, "--pass"), Some("1"));
        assert_eq!(value_of(&parameters, "--color-primaries"), Some("bt2020"));
    }

    #[test]
    fn ffmpeg_colour_data_precedes_the_input() {
        let encoder = Encoder::default_from_base(&EncoderBase::FFmpeg, true);
        let parameters =
            encoder.parameters_with_output((1, 1), &PathBuf::from("output.mkv"), &ClipInfo {
                chroma_location: Some(av_decoders::ChromaLocation::Left),
                ..clip_info()
            });
        let input = parameters
            .iter()
            .position(|argument| argument == "-i")
            .expect("ffmpeg reads the Y4M stream from stdin");

        // `fftools/ffmpeg_enc.c` copies the colour properties out of every
        // frame, so these only survive as input options.
        for flag in ["-color_primaries", "-color_trc", "-colorspace", "-chroma_sample_location"] {
            let index = parameters
                .iter()
                .position(|argument| argument == flag)
                .unwrap_or_else(|| panic!("{flag} is missing"));
            assert!(index < input, "{flag} must precede -i");
        }
        assert_eq!(value_of(&parameters, "-color_primaries"), Some("bt2020"));
        assert_eq!(value_of(&parameters, "-color_trc"), Some("smpte2084"));
        assert_eq!(value_of(&parameters, "-colorspace"), Some("bt2020nc"));
    }

    #[test]
    fn a_user_option_overrides_the_appended_ffmpeg_colour_data() {
        let mut encoder = Encoder::default_from_base(&EncoderBase::FFmpeg, true);
        encoder.parameters_mut().insert(
            "color_primaries".to_owned(),
            CLIParameter::new_string("-", " ", "bt709"),
        );

        let parameters = parameters(&encoder);
        assert_eq!(value_of(&parameters, "-color_primaries"), Some("bt709"));
        assert!(
            !parameters.iter().any(|argument| argument == "bt2020"),
            "the appended value must be replaced rather than duplicated"
        );
    }
}
