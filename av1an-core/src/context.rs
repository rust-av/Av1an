use std::{
    borrow::Cow,
    cmp::{self, Reverse},
    convert::TryInto,
    ffi::OsString,
    fs::{self, File},
    io::Write,
    iter,
    path::{Path, PathBuf},
    process::{exit, Stdio},
    sync::{
        atomic::{self, AtomicBool, AtomicUsize},
        mpsc,
        Arc,
    },
    thread::available_parallelism,
};

use anyhow::Context;
use av1_grain::TransferFunction;
use av_decoders::VapoursynthDecoder;
use colored::*;
use itertools::Itertools;
use num_traits::cast::ToPrimitive;
use rand::{prelude::SliceRandom, rng};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::ChildStderr,
};
use tracing::{debug, error, info, warn};

use crate::{
    broker::{Broker, EncoderCrash},
    chunk::Chunk,
    concat::{self, ConcatMethod},
    create_dir,
    determine_workers,
    ffmpeg::{compose_ffmpeg_pipe, get_num_frames},
    get_done,
    init_done,
    into_vec,
    metrics::vmaf,
    progress_bar::{
        finish_progress_bar,
        inc_bar,
        inc_mp_bar,
        init_multi_progress_bar,
        init_progress_bar,
        reset_bar_at,
        reset_mp_bar_at,
        set_audio_size,
        update_mp_chunk,
        update_mp_msg,
        update_progress_bar_estimates,
    },
    read_chunk_queue,
    save_chunk_queue,
    scenes::{Scene, SceneFactory, ZoneOptions},
    settings::{EncodeArgs, InputPixelFormat},
    split::segment,
    vapoursynth::create_vs_file,
    zones::parse_zones,
    ChunkMethod,
    ChunkOrdering,
    DashMap,
    DoneJson,
    Input,
    Verbosity,
};

#[derive(Debug)]
pub struct Av1anContext {
    pub frames:               usize,
    pub vs_script:            Option<PathBuf>,
    pub args:                 EncodeArgs,
    pub(crate) scene_factory: SceneFactory,
}

impl Av1anContext {
    #[tracing::instrument(level = "debug")]
    pub fn new(mut args: EncodeArgs) -> anyhow::Result<Self> {
        args.validate()?;

        let mut this = Self {
            frames: args.input.clip_info()?.num_frames,
            vs_script: None,
            args,
            scene_factory: SceneFactory::new(),
        };
        this.initialize()?;
        Ok(this)
    }

    /// Initialize logging routines and create temporary directories
    #[tracing::instrument(level = "debug")]
    fn initialize(&mut self) -> anyhow::Result<()> {
        ffmpeg::init()?;
        ffmpeg::util::log::set_level(ffmpeg::util::log::level::Level::Fatal);

        if !self.args.resume && Path::new(&self.args.temp).is_dir() {
            fs::remove_dir_all(&self.args.temp).with_context(|| {
                format!(
                    "Failed to remove temporary directory {temp}",
                    temp = self.args.temp
                )
            })?;
        }

        create_dir!(Path::new(&self.args.temp))?;
        create_dir!(Path::new(&self.args.temp).join("split"))?;
        create_dir!(Path::new(&self.args.temp).join("encode"))?;

        debug!("temporary directory: {temp}", temp = &self.args.temp);

        let done_path = Path::new(&self.args.temp).join("done.json");
        let done_json_exists = done_path.exists();
        let chunks_json_exists = Path::new(&self.args.temp).join("chunks.json").exists();

        if self.args.resume {
            match (done_json_exists, chunks_json_exists) {
                // both files exist, so there is no problem
                (true, true) => {},
                (false, true) => {
                    info!(
                        "resume was set but done.json does not exist in temporary directory {temp}",
                        temp = self.args.temp
                    );
                    self.args.resume = false;
                },
                (true, false) => {
                    info!(
                        "resume was set but chunks.json does not exist in temporary directory \
                         {temp}",
                        temp = self.args.temp
                    );
                    self.args.resume = false;
                },
                (false, false) => {
                    info!(
                        "resume was set but neither chunks.json nor done.json exist in temporary \
                         directory {temp}",
                        temp = self.args.temp
                    );
                    self.args.resume = false;
                },
            }
        }

        if self.args.resume && done_json_exists {
            let done = fs::read_to_string(done_path)
                .with_context(|| "Failed to read contents of done.json")?;
            let done: DoneJson =
                serde_json::from_str(&done).with_context(|| "Failed to parse done.json")?;
            self.frames = done.frames.load(atomic::Ordering::Relaxed);

            // frames need to be recalculated in this case
            if self.frames == 0 {
                self.frames = self.args.input.clip_info()?.num_frames;
                done.frames.store(self.frames, atomic::Ordering::Relaxed);
            }

            init_done(done);
        } else {
            init_done(DoneJson {
                frames:     AtomicUsize::new(0),
                done:       DashMap::new(),
                audio_done: AtomicBool::new(false),
            });

            let mut done_file = File::create(&done_path).unwrap();
            done_file.write_all(serde_json::to_string(get_done())?.as_bytes())?;
        };

        Ok(())
    }

    #[tracing::instrument(skip(self))]
    #[inline]
    pub fn encode_file(&mut self) -> anyhow::Result<()> {
        let initial_frames =
            get_done().done.iter().map(|ref_multi| ref_multi.frames).sum::<usize>();

        // Technically we should check if the vapoursynth cache file exists rather than
        // !self.resume, but the code still works if we are resuming and the
        // cache file doesn't exist (as it gets generated when vspipe is first
        // called), so it's not worth adding all the extra complexity.
        if (self.args.input.is_vapoursynth()
            || (self.args.input.is_video()
                && matches!(
                    self.args.chunk_method,
                    ChunkMethod::LSMASH
                        | ChunkMethod::FFMS2
                        | ChunkMethod::DGDECNV
                        | ChunkMethod::BESTSOURCE
                )))
            && !self.args.resume
        {
            // Create the VapourSynth script file and store the path to it
            self.vs_script = Some(match &self.args.input {
                Input::VapourSynth {
                    path, ..
                } => path.clone(),
                Input::Video {
                    path, ..
                } => {
                    let (script_path, _) = create_vs_file(
                        &self.args.temp,
                        path,
                        self.args.chunk_method,
                        self.args.sc_downscale_height,
                        self.args.sc_pix_format,
                        self.args.scaler.clone(),
                    )?;
                    script_path
                },
            });
            let variables_map = self.args.input.as_vspipe_args_hashmap()?;
            let decoder = match &self.args.input {
                Input::VapourSynth {
                    path, ..
                } => {
                    let mut dec = VapoursynthDecoder::from_file(path)?;
                    dec.set_variables(variables_map)?;
                    av_scenechange::Decoder::from_decoder_impl(
                        av_decoders::DecoderImpl::Vapoursynth(dec),
                    )?
                },
                Input::Video {
                    ..
                } => av_scenechange::Decoder::from_script(
                    &self.args.input.as_script_text(
                        self.args.sc_downscale_height,
                        self.args.sc_pix_format,
                        Some(self.args.scaler.clone()),
                    )?,
                    Some(variables_map),
                )?,
            };
            // Getting the details will evaluate the script
            // and produce the VapourSynth cache file
            decoder.get_video_details();
        }

        let clip_info = self.args.input.clip_info()?;
        let res = clip_info.resolution;
        let fps_ratio = clip_info.frame_rate;
        let fps = fps_ratio.to_f64().unwrap();
        let format = clip_info.format_info;
        let tfc = clip_info.transfer_function_params_adjusted(&self.args.video_params);
        info!(
            "Input: {}x{} @ {:.3} fps, {}, {}",
            res.0,
            res.1,
            fps,
            match format {
                InputPixelFormat::VapourSynth {
                    bit_depth,
                } => format!("{bit_depth} BPC"),
                InputPixelFormat::FFmpeg {
                    format,
                } => format!("{format:?}"),
            },
            match tfc {
                TransferFunction::SMPTE2084 => "HDR",
                TransferFunction::BT1886 => "SDR",
            }
        );

        let splits = self.split_routine()?.to_vec();

        if self.args.sc_only {
            debug!("scene detection only");

            if let Err(e) = fs::remove_dir_all(&self.args.temp) {
                warn!("Failed to delete temp directory: {e}");
            }

            exit(0);
        }

        let (chunk_queue, total_chunks) = self.load_or_gen_chunk_queue(&splits)?;

        let mut chunks_done = 0;
        if self.args.resume {
            chunks_done = get_done().done.len();
            info!(
                "encoding resumed with {}/{} chunks completed ({} remaining)",
                chunks_done,
                total_chunks,
                chunk_queue.len()
            );
        }

        crossbeam_utils::thread::scope(|s| -> anyhow::Result<()> {
            // vapoursynth audio is currently unsupported
            let audio_thread = if self.args.input.is_video()
                && (!self.args.resume || !get_done().audio_done.load(atomic::Ordering::SeqCst))
            {
                let input = self.args.input.as_video_path();
                let temp = self.args.temp.as_str();
                let audio_params = self.args.audio_params.as_slice();
                Some(s.spawn(move |_| {
                    let audio_output = crate::ffmpeg::encode_audio(input, temp, audio_params);
                    get_done().audio_done.store(true, atomic::Ordering::SeqCst);

                    let progress_file = Path::new(temp).join("done.json");
                    let mut progress_file = File::create(progress_file).unwrap();
                    progress_file
                        .write_all(serde_json::to_string(get_done()).unwrap().as_bytes())
                        .unwrap();

                    if let Some(ref audio_output) = audio_output {
                        let audio_size = audio_output.metadata().unwrap().len();
                        set_audio_size(audio_size);
                    }

                    audio_output.is_some()
                }))
            } else {
                None
            };

            if self.args.workers == 0 {
                self.args.workers = determine_workers(&self.args)? as usize;
            }
            self.args.workers = cmp::min(self.args.workers, chunk_queue.len());

            info!(
                "\n{}{} {} {}{} {} {}{} {} {}{} {}\n{}: {}",
                "Q".green().bold(),
                "ueue".green(),
                format!("{len}", len = chunk_queue.len()).green().bold(),
                "W".blue().bold(),
                "orkers".blue(),
                format!("{workers}", workers = self.args.workers).blue().bold(),
                "E".purple().bold(),
                "ncoder".purple(),
                format!("{encoder}", encoder = self.args.encoder).purple().bold(),
                "P".purple().bold(),
                "asses".purple(),
                format!("{passes}", passes = self.args.passes).purple().bold(),
                "Params".bold(),
                self.args.video_params.join(" ").dimmed()
            );

            if self.args.verbosity == Verbosity::Normal {
                init_progress_bar(
                    self.frames as u64,
                    initial_frames as u64,
                    Some((chunks_done as u32, total_chunks as u32)),
                );
                reset_bar_at(initial_frames as u64);
            } else if self.args.verbosity == Verbosity::Verbose {
                init_multi_progress_bar(
                    self.frames as u64,
                    self.args.workers,
                    initial_frames as u64,
                    (chunks_done as u32, total_chunks as u32),
                );
                reset_mp_bar_at(initial_frames as u64);
            }

            if chunks_done > 0 {
                update_progress_bar_estimates(
                    fps,
                    self.frames,
                    self.args.verbosity,
                    (chunks_done as u32, total_chunks as u32),
                );
            }

            let broker = Broker {
                chunk_queue,
                project: self,
            };

            let (tx, rx) = mpsc::channel();
            let handle = s.spawn(|_| {
                broker.encoding_loop(tx, self.args.set_thread_affinity, total_chunks as u32);
            });

            // Queue::encoding_loop only sends a message if there was an error (meaning a
            // chunk crashed) more than MAX_TRIES. So, we have to explicitly
            // exit the program if that happens.
            if rx.recv().is_ok() {
                exit(1);
            }

            handle.join().unwrap();

            finish_progress_bar();

            // TODO add explicit parameter to concatenation functions to control whether
            // audio is also muxed in
            let _audio_output_exists =
                audio_thread.is_some_and(|audio_thread| audio_thread.join().unwrap());

            debug!(
                "encoding finished, concatenating with {concat}",
                concat = self.args.concat
            );

            match self.args.concat {
                ConcatMethod::Ivf => {
                    concat::ivf(
                        &Path::new(&self.args.temp).join("encode"),
                        self.args.output_file.as_ref(),
                    )?;
                },
                ConcatMethod::MKVMerge => {
                    concat::mkvmerge(
                        self.args.temp.as_ref(),
                        self.args.output_file.as_ref(),
                        self.args.encoder,
                        total_chunks,
                        if self.args.ignore_frame_mismatch {
                            info!(
                                "`--ignore-frame-mismatch` set. Don't force output FPS, as an FPS \
                                 changing filter might have been applied."
                            );
                            None
                        } else {
                            debug!(
                                "`--ignore-frame-mismatch` not set. Forcing output FPS to \
                                 {fps_ratio} with mkvmerge."
                            );
                            Some(fps_ratio)
                        },
                    )?;
                },
                ConcatMethod::FFmpeg => {
                    concat::ffmpeg(self.args.temp.as_ref(), self.args.output_file.as_ref())?;
                },
            }

            if self.args.vmaf || self.args.target_quality.is_some() {
                let vmaf_res = if let Some(ref tq) = self.args.target_quality {
                    if tq.vmaf_res == "inputres" {
                        let inputres = self.args.input.clip_info()?.resolution;
                        format!("{width}x{height}", width = inputres.0, height = inputres.1)
                    } else {
                        tq.vmaf_res.clone()
                    }
                } else {
                    self.args.vmaf_res.clone()
                };

                let vmaf_model = self.args.vmaf_path.as_deref().or_else(|| {
                    self.args.target_quality.as_ref().and_then(|tq| tq.model.as_deref())
                });
                let vmaf_scaler = "bicubic";
                let vmaf_filter = self.args.vmaf_filter.as_deref().or_else(|| {
                    self.args.target_quality.as_ref().and_then(|tq| tq.vmaf_filter.as_deref())
                });

                if self.args.vmaf {
                    let vmaf_threads = available_parallelism().map_or(1, std::num::NonZero::get);

                    if let Err(e) = vmaf::plot(
                        self.args.output_file.as_ref(),
                        &self.args.input,
                        vmaf_model,
                        &vmaf_res,
                        vmaf_scaler,
                        1,
                        vmaf_filter,
                        vmaf_threads,
                    ) {
                        error!("VMAF calculation failed with error: {e}");
                    }
                }
            }

            if !Path::new(&self.args.output_file).exists() {
                warn!(
                    "Concatenation failed for unknown reasons! Temp folder will not be deleted: \
                     {temp}",
                    temp = self.args.temp
                );
            } else if !self.args.keep {
                if let Err(e) = fs::remove_dir_all(&self.args.temp) {
                    warn!("Failed to delete temp directory: {e}");
                }
            }

            Ok(())
        })
        .unwrap()?;

        Ok(())
    }

    #[tracing::instrument(level = "debug")]
    fn read_queue_files(source_path: &Path) -> anyhow::Result<Vec<PathBuf>> {
        let mut queue_files = fs::read_dir(source_path)
            .with_context(|| {
                format!("Failed to read queue files from source path {source_path:?}")
            })?
            .map(|res| res.map(|e| e.path()))
            .collect::<Result<Vec<_>, _>>()?;

        queue_files.retain(|file| {
            file.is_file() && matches!(file.extension().map(|ext| ext == "mkv"), Some(true))
        });
        concat::sort_files_by_filename(&mut queue_files);

        Ok(queue_files)
    }

    /// Returns the number of frames encoded if crashed, to reset the progress
    /// bar.
    #[inline]
    pub fn create_pipes(
        &self,
        chunk: &Chunk,
        current_pass: u8,
        worker_id: usize,
        padding: usize,
    ) -> Result<(), (Box<EncoderCrash>, u64)> {
        update_mp_chunk(worker_id, chunk.index, padding);

        let fpf_file = Path::new(&chunk.temp)
            .join("split")
            .join(format!("{name}_fpf", name = chunk.name()));

        let video_params = chunk.video_params.clone();

        let mut enc_cmd = if chunk.passes == 1 {
            chunk.encoder.compose_1_1_pass(video_params, chunk.output())
        } else if current_pass == 1 {
            chunk.encoder.compose_1_2_pass(video_params, fpf_file.to_str().unwrap())
        } else {
            chunk
                .encoder
                .compose_2_2_pass(video_params, fpf_file.to_str().unwrap(), chunk.output())
        };

        if let Some(per_shot_target_quality_cq) = chunk.tq_cq {
            enc_cmd = chunk.encoder.man_command(enc_cmd, per_shot_target_quality_cq as usize);
        }

        let rt = tokio::runtime::Builder::new_current_thread().enable_io().build().unwrap();

        let (source_pipe_stderr, ffmpeg_pipe_stderr, enc_output, enc_stderr, frame) =
            rt.block_on(async {
                let mut source_pipe = if let [source, args @ ..] = &*chunk.source_cmd {
                    let mut command = tokio::process::Command::new(source);
                    for arg in chunk.input.as_vspipe_args_vec().unwrap() {
                        command.args(["-a", &arg]);
                    }
                    command
                        .args(args)
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped())
                        .spawn()
                        .unwrap()
                } else {
                    unreachable!()
                };

                let source_pipe_stdout: Stdio =
                    source_pipe.stdout.take().unwrap().try_into().unwrap();

                let source_pipe_stderr = source_pipe.stderr.take().unwrap();

                // converts the pixel format
                let create_ffmpeg_pipe = |pipe_from: Stdio, source_pipe_stderr: ChildStderr| {
                    let ffmpeg_pipe = compose_ffmpeg_pipe(
                        self.args.ffmpeg_filter_args.as_slice(),
                        self.args.output_pix_format.format,
                    );

                    let mut ffmpeg_pipe = if let [ffmpeg, args @ ..] = &*ffmpeg_pipe {
                        tokio::process::Command::new(ffmpeg)
                            .args(args)
                            .stdin(pipe_from)
                            .stdout(Stdio::piped())
                            .stderr(Stdio::piped())
                            .spawn()
                            .unwrap()
                    } else {
                        unreachable!()
                    };

                    let ffmpeg_pipe_stdout: Stdio =
                        ffmpeg_pipe.stdout.take().unwrap().try_into().unwrap();
                    let ffmpeg_pipe_stderr = ffmpeg_pipe.stderr.take().unwrap();
                    (
                        ffmpeg_pipe_stdout,
                        source_pipe_stderr,
                        Some(ffmpeg_pipe_stderr),
                    )
                };

                let (y4m_pipe, source_pipe_stderr, mut ffmpeg_pipe_stderr) =
                    if self.args.ffmpeg_filter_args.is_empty() {
                        match &self.args.input_pix_format {
                            InputPixelFormat::FFmpeg {
                                format,
                            } => {
                                if self.args.output_pix_format.format == *format {
                                    (source_pipe_stdout, source_pipe_stderr, None)
                                } else {
                                    create_ffmpeg_pipe(source_pipe_stdout, source_pipe_stderr)
                                }
                            },
                            InputPixelFormat::VapourSynth {
                                bit_depth,
                            } => {
                                if self.args.output_pix_format.bit_depth == *bit_depth {
                                    (source_pipe_stdout, source_pipe_stderr, None)
                                } else {
                                    create_ffmpeg_pipe(source_pipe_stdout, source_pipe_stderr)
                                }
                            },
                        }
                    } else {
                        create_ffmpeg_pipe(source_pipe_stdout, source_pipe_stderr)
                    };

                let mut source_reader = BufReader::new(source_pipe_stderr).lines();
                let ffmpeg_reader =
                    ffmpeg_pipe_stderr.take().map(|stderr| BufReader::new(stderr).lines());

                let pipe_stderr = Arc::new(parking_lot::Mutex::new(String::with_capacity(128)));
                let p_stdr2 = Arc::clone(&pipe_stderr);

                let ffmpeg_stderr = if ffmpeg_reader.is_some() {
                    Some(Arc::new(parking_lot::Mutex::new(String::with_capacity(
                        128,
                    ))))
                } else {
                    None
                };

                let f_stdr2 = ffmpeg_stderr.clone();

                tokio::spawn(async move {
                    while let Some(line) = source_reader.next_line().await.unwrap() {
                        p_stdr2.lock().push_str(&line);
                        p_stdr2.lock().push('\n');
                    }
                });
                if let Some(mut ffmpeg_reader) = ffmpeg_reader {
                    let f_stdr2 = f_stdr2.unwrap();
                    tokio::spawn(async move {
                        while let Some(line) = ffmpeg_reader.next_line().await.unwrap() {
                            f_stdr2.lock().push_str(&line);
                            f_stdr2.lock().push('\n');
                        }
                    });
                }

                let mut enc_pipe = if let [encoder, args @ ..] = &*enc_cmd {
                    tokio::process::Command::new(encoder)
                        .args(args)
                        .stdin(y4m_pipe)
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped())
                        .spawn()
                        .unwrap()
                } else {
                    unreachable!()
                };

                let mut frame = 0;

                let mut reader = BufReader::new(enc_pipe.stderr.take().unwrap());

                let mut buf = Vec::with_capacity(128);
                let mut enc_stderr = String::with_capacity(128);

                while let Ok(read) = reader.read_until(b'\r', &mut buf).await {
                    if read == 0 {
                        break;
                    }

                    if let Ok(line) = simdutf8::basic::from_utf8_mut(&mut buf) {
                        if self.args.verbosity == Verbosity::Verbose && !line.contains('\n') {
                            update_mp_msg(worker_id, line.trim().to_string());
                        }
                        // This needs to be done before parse_encoded_frames, as it potentially
                        // mutates the string
                        enc_stderr.push_str(line);
                        enc_stderr.push('\n');

                        if current_pass == chunk.passes {
                            if let Some(new) = chunk.encoder.parse_encoded_frames(line) {
                                if new > frame {
                                    if self.args.verbosity == Verbosity::Normal {
                                        inc_bar(new - frame);
                                    } else if self.args.verbosity == Verbosity::Verbose {
                                        inc_mp_bar(new - frame);
                                    }
                                    frame = new;
                                }
                            }
                        }
                    }

                    buf.clear();
                }

                let enc_output = enc_pipe.wait_with_output().await.unwrap();

                let source_pipe_stderr = pipe_stderr.lock().clone();
                let ffmpeg_pipe_stderr = ffmpeg_stderr.map(|x| x.lock().clone());
                (
                    source_pipe_stderr,
                    ffmpeg_pipe_stderr,
                    enc_output,
                    enc_stderr,
                    frame,
                )
            });

        if !enc_output.status.success() {
            return Err((
                Box::new(EncoderCrash {
                    exit_status:        enc_output.status,
                    source_pipe_stderr: source_pipe_stderr.into(),
                    ffmpeg_pipe_stderr: ffmpeg_pipe_stderr.map(Into::into),
                    stderr:             enc_stderr.into(),
                    stdout:             enc_output.stdout.into(),
                }),
                frame,
            ));
        }

        if current_pass == chunk.passes {
            let encoded_frames = get_num_frames(chunk.output().as_ref());

            let err_str = match encoded_frames {
                Ok(encoded_frames)
                    if !chunk.ignore_frame_mismatch && encoded_frames != chunk.frames() =>
                {
                    Some(format!(
                        "FRAME MISMATCH: chunk {index}: {encoded_frames}/{expected} \
                         (actual/expected frames)",
                        index = chunk.index,
                        expected = chunk.frames()
                    ))
                },
                Err(error) => Some(format!(
                    "FAILED TO COUNT FRAMES: chunk {index}: {error}",
                    index = chunk.index
                )),
                _ => None,
            };

            if let Some(err_str) = err_str {
                return Err((
                    Box::new(EncoderCrash {
                        exit_status:        enc_output.status,
                        source_pipe_stderr: source_pipe_stderr.into(),
                        ffmpeg_pipe_stderr: ffmpeg_pipe_stderr.map(Into::into),
                        stderr:             enc_stderr.into(),
                        stdout:             err_str.into(),
                    }),
                    frame,
                ));
            }
        }

        Ok(())
    }

    fn create_encoding_queue(&self, scenes: &[Scene]) -> anyhow::Result<Vec<Chunk>> {
        let mut chunks = match &self.args.input {
            Input::Video {
                ..
            } => match self.args.chunk_method {
                ChunkMethod::FFMS2
                | ChunkMethod::LSMASH
                | ChunkMethod::DGDECNV
                | ChunkMethod::BESTSOURCE => {
                    let vs_script = self.vs_script.as_ref().unwrap().as_path();
                    self.create_video_queue_vs(scenes, vs_script)
                },
                ChunkMethod::Hybrid => self.create_video_queue_hybrid(scenes)?,
                ChunkMethod::Select => self.create_video_queue_select(scenes),
                ChunkMethod::Segment => self.create_video_queue_segment(scenes)?,
            },
            Input::VapourSynth {
                path, ..
            } => self.create_video_queue_vs(scenes, path.as_path()),
        };

        match self.args.chunk_order {
            ChunkOrdering::LongestFirst => {
                chunks.sort_unstable_by_key(|chunk| Reverse(chunk.frames()));
            },
            ChunkOrdering::ShortestFirst => {
                chunks.sort_unstable_by_key(Chunk::frames);
            },
            ChunkOrdering::Sequential => {
                // Already in order
            },
            ChunkOrdering::Random => {
                chunks.shuffle(&mut rng());
            },
        }

        Ok(chunks)
    }

    // If we are not resuming, then do scene detection. Otherwise: get scenes from
    // scenes.json and return that.
    fn split_routine(&mut self) -> anyhow::Result<&[Scene]> {
        let scene_file = self.args.scenes.as_ref().map_or_else(
            || Cow::Owned(Path::new(&self.args.temp).join("scenes.json")),
            |path| Cow::Borrowed(path.as_path()),
        );
        if scene_file.exists() && (self.args.scenes.is_some() || self.args.resume) {
            self.scene_factory = SceneFactory::from_scenes_file(&scene_file)?;
        } else {
            let zones = parse_zones(&self.args, self.frames)?;
            self.scene_factory.compute_scenes(&self.args, &zones)?;
        }
        self.frames = self.scene_factory.get_frame_count();
        self.scene_factory.write_scenes_to_file(scene_file)?;
        self.scene_factory.get_split_scenes()
    }

    fn create_select_chunk(
        &self,
        index: usize,
        src_path: &Path,
        start_frame: usize,
        end_frame: usize,
        frame_rate: f64,
        overrides: Option<ZoneOptions>,
    ) -> anyhow::Result<Chunk> {
        assert!(
            start_frame < end_frame,
            "Can't make a chunk with <= 0 frames!"
        );

        let ffmpeg_gen_cmd: Vec<OsString> = into_vec![
            "ffmpeg",
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            src_path,
            "-vf",
            format!(
                r"select=between(n\,{start}\,{end})",
                start = start_frame,
                end = end_frame - 1
            ),
            "-pix_fmt",
            self.args.output_pix_format.format.descriptor().unwrap().name(),
            "-strict",
            "-1",
            "-f",
            "yuv4mpegpipe",
            "-",
        ];

        let output_ext = self.args.encoder.output_extension();

        let mut chunk = Chunk {
            temp: self.args.temp.clone(),
            index,
            input: Input::Video {
                path:         src_path.to_path_buf(),
                temp:         self.args.temp.clone(),
                chunk_method: ChunkMethod::Select,
            },
            source_cmd: ffmpeg_gen_cmd,
            output_ext: output_ext.to_owned(),
            start_frame,
            end_frame,
            frame_rate,
            video_params: overrides.as_ref().map_or_else(
                || self.args.video_params.clone(),
                |ovr| ovr.video_params.clone(),
            ),
            passes: overrides.as_ref().map_or(self.args.passes, |ovr| ovr.passes),
            encoder: overrides.as_ref().map_or(self.args.encoder, |ovr| ovr.encoder),
            noise_size: self.args.photon_noise_size,
            tq_cq: None,
            ignore_frame_mismatch: self.args.ignore_frame_mismatch,
        };
        chunk.apply_photon_noise_args(
            overrides.map_or(self.args.photon_noise, |ovr| ovr.photon_noise),
            self.args.chroma_noise,
        )?;
        if let Some(ref tq) = self.args.target_quality {
            tq.per_shot_target_quality_routine(
                &mut chunk,
                None,
                self.args.vapoursynth_plugins.as_ref(),
            )?;
        }
        Ok(chunk)
    }

    fn create_vs_chunk(
        &self,
        index: usize,
        vs_script: &Path,
        scene: &Scene,
        frame_rate: f64,
    ) -> anyhow::Result<Chunk> {
        // the frame end boundary is actually a frame that should be included in the
        // next chunk
        let frame_end = scene.end_frame - 1;

        let vspipe_cmd_gen: Vec<OsString> = into_vec![
            "vspipe",
            vs_script,
            "-c",
            "y4m",
            "-",
            "-s",
            scene.start_frame.to_string(),
            "-e",
            frame_end.to_string(),
        ];

        let output_ext = self.args.encoder.output_extension();

        let mut chunk = Chunk {
            temp: self.args.temp.clone(),
            index,
            input: Input::VapourSynth {
                path:        vs_script.to_path_buf(),
                vspipe_args: self.args.input.as_vspipe_args_vec()?,
                script_text: self.args.input.as_script_text(
                    self.args.sc_downscale_height,
                    self.args.sc_pix_format,
                    Some(self.args.scaler.clone()),
                )?,
            },
            source_cmd: vspipe_cmd_gen,
            output_ext: output_ext.to_owned(),
            start_frame: scene.start_frame,
            end_frame: scene.end_frame,
            frame_rate,
            video_params: scene.zone_overrides.as_ref().map_or_else(
                || self.args.video_params.clone(),
                |ovr| ovr.video_params.clone(),
            ),
            passes: scene.zone_overrides.as_ref().map_or(self.args.passes, |ovr| ovr.passes),
            encoder: scene.zone_overrides.as_ref().map_or(self.args.encoder, |ovr| ovr.encoder),
            noise_size: scene.zone_overrides.as_ref().map_or(self.args.photon_noise_size, |ovr| {
                (ovr.photon_noise_width, ovr.photon_noise_height)
            }),
            tq_cq: None,
            ignore_frame_mismatch: self.args.ignore_frame_mismatch,
        };
        chunk.apply_photon_noise_args(
            scene
                .zone_overrides
                .as_ref()
                .map_or(self.args.photon_noise, |ovr| ovr.photon_noise),
            scene
                .zone_overrides
                .as_ref()
                .map_or(self.args.chroma_noise, |ovr| ovr.chroma_noise),
        )?;
        Ok(chunk)
    }

    fn create_video_queue_vs(&self, scenes: &[Scene], vs_script: &Path) -> Vec<Chunk> {
        let frame_rate = self.args.input.clip_info().unwrap().frame_rate.to_f64().unwrap();
        let chunk_queue: Vec<Chunk> = scenes
            .iter()
            .enumerate()
            .map(|(index, scene)| {
                self.create_vs_chunk(index, vs_script, scene, frame_rate).unwrap()
            })
            .collect();

        chunk_queue
    }

    fn create_video_queue_select(&self, scenes: &[Scene]) -> Vec<Chunk> {
        let input = self.args.input.as_video_path();
        let frame_rate = self.args.input.clip_info().unwrap().frame_rate.to_f64().unwrap();

        let chunk_queue: Vec<Chunk> = scenes
            .iter()
            .enumerate()
            .map(|(index, scene)| {
                self.create_select_chunk(
                    index,
                    input,
                    scene.start_frame,
                    scene.end_frame,
                    frame_rate,
                    scene.zone_overrides.clone(),
                )
                .unwrap()
            })
            .collect();

        chunk_queue
    }

    fn create_video_queue_segment(&self, scenes: &[Scene]) -> anyhow::Result<Vec<Chunk>> {
        let input = self.args.input.as_video_path();
        let frame_rate = self.args.input.clip_info()?.frame_rate.to_f64().unwrap();

        debug!("Splitting video");
        segment(
            input,
            &self.args.temp,
            &scenes.iter().skip(1).map(|scene| scene.start_frame).collect::<Vec<usize>>(),
        );
        debug!("Splitting done");

        let source_path = Path::new(&self.args.temp).join("split");
        let queue_files = Self::read_queue_files(&source_path)?;

        assert!(
            !queue_files.is_empty(),
            "Error: No files found in temp/split, probably splitting not working"
        );

        let chunk_queue: Vec<Chunk> = queue_files
            .iter()
            .enumerate()
            .map(|(index, file)| {
                self.create_chunk_from_segment(
                    index,
                    file.as_path().to_str().unwrap(),
                    frame_rate,
                    scenes[index].zone_overrides.clone(),
                )
                .unwrap()
            })
            .collect();

        Ok(chunk_queue)
    }

    fn create_video_queue_hybrid(&self, scenes: &[Scene]) -> anyhow::Result<Vec<Chunk>> {
        let input = self.args.input.as_video_path();
        let frame_rate = self.args.input.clip_info()?.frame_rate.to_f64().unwrap();

        let keyframes = crate::ffmpeg::get_keyframes(input).unwrap();

        let to_split: Vec<usize> = keyframes
            .iter()
            .filter(|kf| scenes.iter().any(|scene| scene.start_frame == **kf))
            .copied()
            .collect();

        debug!("Segmenting video");
        segment(input, &self.args.temp, &to_split[1..]);
        debug!("Segment done");

        let source_path = Path::new(&self.args.temp).join("split");
        let queue_files = Self::read_queue_files(&source_path)?;

        let kf_list = to_split.iter().copied().chain(iter::once(self.frames)).tuple_windows();

        let mut segments = Vec::with_capacity(scenes.len());
        for (file, (x, y)) in queue_files.iter().zip(kf_list) {
            for s in scenes {
                let s0 = s.start_frame;
                let s1 = s.end_frame;
                if s0 >= x && s1 <= y && s0 < s1 {
                    segments.push((file.as_path(), (s0 - x, s1 - x, s)));
                }
            }
        }

        let chunk_queue: Vec<Chunk> = segments
            .iter()
            .enumerate()
            .map(|(index, &(file, (start, end, scene)))| {
                self.create_select_chunk(
                    index,
                    file,
                    start,
                    end,
                    frame_rate,
                    scene.zone_overrides.clone(),
                )
                .unwrap()
            })
            .collect();

        Ok(chunk_queue)
    }

    #[tracing::instrument(level = "debug")]
    fn create_chunk_from_segment(
        &self,
        index: usize,
        file: &str,
        frame_rate: f64,
        overrides: Option<ZoneOptions>,
    ) -> anyhow::Result<Chunk> {
        let ffmpeg_gen_cmd: Vec<OsString> = into_vec![
            "ffmpeg",
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            file.to_owned(),
            "-strict",
            "-1",
            "-pix_fmt",
            self.args.output_pix_format.format.descriptor().unwrap().name(),
            "-f",
            "yuv4mpegpipe",
            "-",
        ];

        let output_ext = self.args.encoder.output_extension();

        let num_frames = get_num_frames(Path::new(file))?;

        let mut chunk = Chunk {
            temp: self.args.temp.clone(),
            input: Input::Video {
                path:         PathBuf::from(file),
                temp:         self.args.temp.clone(),
                chunk_method: ChunkMethod::Segment,
            },
            source_cmd: ffmpeg_gen_cmd,
            output_ext: output_ext.to_owned(),
            index,
            start_frame: 0,
            end_frame: num_frames,
            frame_rate,
            video_params: overrides.as_ref().map_or_else(
                || self.args.video_params.clone(),
                |ovr| ovr.video_params.clone(),
            ),
            passes: overrides.as_ref().map_or(self.args.passes, |ovr| ovr.passes),
            encoder: overrides.as_ref().map_or(self.args.encoder, |ovr| ovr.encoder),
            noise_size: self.args.photon_noise_size,
            tq_cq: None,
            ignore_frame_mismatch: self.args.ignore_frame_mismatch,
        };
        chunk.apply_photon_noise_args(
            overrides.map_or(self.args.photon_noise, |ovr| ovr.photon_noise),
            self.args.chroma_noise,
        )?;
        Ok(chunk)
    }

    /// Returns unfinished chunks and number of total chunks
    fn load_or_gen_chunk_queue(&self, splits: &[Scene]) -> anyhow::Result<(Vec<Chunk>, usize)> {
        if self.args.resume {
            let mut chunks = read_chunk_queue(self.args.temp.as_ref())?;
            let num_chunks = chunks.len();

            let done = get_done();

            // only keep the chunks that are not done
            chunks.retain(|chunk| !done.done.contains_key(&chunk.name()));

            Ok((chunks, num_chunks))
        } else {
            let chunks = self.create_encoding_queue(splits)?;
            let num_chunks = chunks.len();
            save_chunk_queue(&self.args.temp, &chunks)?;
            Ok((chunks, num_chunks))
        }
    }
}
