use std::{
    borrow::Cow,
    cmp::{self, Reverse},
    ffi::OsString,
    fs::{self, File},
    io::{BufRead, BufReader, Write},
    iter,
    path::{Path, PathBuf},
    process::{exit, ChildStderr, Command, Stdio},
    sync::{
        atomic::{self, AtomicBool, AtomicUsize},
        mpsc,
        Arc,
        Mutex,
    },
    thread::{self, available_parallelism},
};

use anyhow::{bail, ensure, Context, Result};
use av1_grain::TransferFunction;
use av_decoders::VapoursynthDecoder;
use colored::*;
use itertools::{chain, Itertools};
use num_traits::cast::ToPrimitive;
use rand::{prelude::SliceRandom, rng};
use tracing::{debug, error, info, warn};

use crate::{
    broker::{Broker, EncoderCrash},
    chunk::Chunk,
    concat::{self, ConcatMethod},
    create_dir,
    ffmpeg::{compose_ffmpeg_pipe, get_num_frames, FFPixelFormat},
    get_done,
    init_done,
    into_vec,
    metrics::{
        vmaf::{self, validate_libvmaf},
        xpsnr::validate_libxpsnr,
    },
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
    settings::{
        Av1anSettings,
        ChunkSettings,
        EncoderSettings,
        FfmpegSettings,
        InputOutputSettings,
        InputPixelFormat,
        ScenecutSettings,
        TargetQualitySettings,
    },
    split::segment,
    util::to_absolute_path,
    vapoursynth::{create_vs_file, get_vapoursynth_plugins, VSZipVersion, VapoursynthPlugins},
    zones::parse_zones,
    ChunkMethod,
    ChunkOrdering,
    DashMap,
    DoneJson,
    Encoder,
    Input,
    TargetMetric,
    Verbosity,
};

#[derive(Debug)]
pub struct Av1anContext {
    pub frames:               usize,
    pub vs_script:            Option<PathBuf>,
    pub vs_proxy_script:      Option<PathBuf>,
    pub encoder_settings:     EncoderSettings,
    pub io_settings:          InputOutputSettings,
    pub av1an_settings:       Av1anSettings,
    pub sc_settings:          ScenecutSettings,
    pub chunk_settings:       ChunkSettings,
    pub tq_settings:          TargetQualitySettings,
    pub ffmpeg_settings:      FfmpegSettings,
    pub(crate) scene_factory: SceneFactory,
    pub vapoursynth_plugins:  Option<VapoursynthPlugins>,
}

impl Av1anContext {
    #[tracing::instrument(level = "debug")]
    pub fn new(
        encoder_settings: EncoderSettings,
        io_settings: InputOutputSettings,
        av1an_settings: Av1anSettings,
        sc_settings: ScenecutSettings,
        chunk_settings: ChunkSettings,
        tq_settings: TargetQualitySettings,
        ffmpeg_settings: FfmpegSettings,
    ) -> Result<Self> {
        let mut this = Self {
            frames: io_settings.input.clip_info()?.num_frames,
            vs_script: None,
            vs_proxy_script: None,
            encoder_settings,
            io_settings,
            av1an_settings,
            sc_settings,
            chunk_settings,
            tq_settings,
            ffmpeg_settings,
            scene_factory: SceneFactory::new(),
            // Don't hard error, we can proceed if Vapoursynth isn't available
            vapoursynth_plugins: get_vapoursynth_plugins().ok(),
        };
        this.validate()?;
        this.initialize()?;
        Ok(this)
    }

    fn validate(&mut self) -> Result<()> {
        self.validate_av1an_settings()?;
        self.validate_io_settings()?;
        self.validate_encoder_settings()?;
        self.validate_scenecut_settings()?;
        self.validate_chunk_settings()?;
        self.validate_tq_settings()?;
        self.validate_ffmpeg_settings()?;

        Ok(())
    }

    fn validate_av1an_settings(&self) -> Result<()> {
        ensure!(self.av1an_settings.max_tries > 0);

        if self.av1an_settings.ignore_frame_mismatch {
            warn!(
                "The output video's frame count may differ, and target metric calculations may be \
                 incorrect"
            );
        }

        Ok(())
    }

    fn validate_io_settings(&self) -> Result<()> {
        ensure!(
            self.io_settings.input.as_path().exists(),
            "Input file {:?} does not exist!",
            self.io_settings.input
        );

        if let Some(proxy) = &self.io_settings.proxy {
            ensure!(
                proxy.as_path().exists(),
                "Proxy file {:?} does not exist!",
                proxy
            );

            // Frame count must match
            let input_frame_count = self.io_settings.input.clip_info()?.num_frames;
            let proxy_frame_count = proxy.clip_info()?.num_frames;

            ensure!(
                input_frame_count == proxy_frame_count,
                "Input and Proxy do not have the same number of frames! ({input_frame_count} != \
                 {proxy_frame_count})",
            );
        }

        Ok(())
    }

    fn validate_encoder_settings(&mut self) -> Result<()> {
        let encoder_bin = self.encoder_settings.encoder.bin();
        if which::which(encoder_bin).is_err() {
            bail!("Encoder {encoder_bin} not found. Is it installed in the system path?");
        }

        if let Some(strength) = self.encoder_settings.photon_noise {
            if strength > 64 {
                bail!("Valid strength values for photon noise are 0-64");
            }
            if ![Encoder::aom, Encoder::rav1e, Encoder::svt_av1]
                .contains(&self.encoder_settings.encoder)
            {
                bail!("Photon noise synth is only supported with aomenc, rav1e, and svt-av1");
            }
        }

        if matches!(self.encoder_settings.encoder, Encoder::aom | Encoder::vpx)
            && self.encoder_settings.passes != 1
            && self.encoder_settings.video_params.iter().any(|param| param == "--rt")
        {
            // --rt must be used with 1-pass mode
            self.encoder_settings.passes = 1;
        }

        if !self.av1an_settings.force {
            self.encoder_settings.validate_encoder_params();
            self.encoder_settings.check_rate_control();
        }

        Ok(())
    }

    fn validate_scenecut_settings(&self) -> Result<()> {
        Ok(())
    }

    fn validate_chunk_settings(&self) -> Result<()> {
        if self.chunk_settings.concat == ConcatMethod::Ivf
            && !matches!(
                self.encoder_settings.encoder,
                Encoder::rav1e | Encoder::aom | Encoder::svt_av1 | Encoder::vpx
            )
        {
            bail!(".ivf only supports VP8, VP9, and AV1");
        }

        if self.chunk_settings.concat == ConcatMethod::MKVMerge && which::which("mkvmerge").is_err()
        {
            if self.av1an_settings.sc_only {
                warn!(
                    "mkvmerge not found, but `--concat mkvmerge` was specified. Make sure to \
                     install mkvmerge or specify a different concatenation method (e.g. `--concat \
                     ffmpeg`) before encoding."
                );
            } else {
                bail!(
                    "mkvmerge not found, but `--concat mkvmerge` was specified. Is it installed \
                     in system path?"
                );
            }
        }

        if self.encoder_settings.encoder == Encoder::x265
            && self.chunk_settings.concat != ConcatMethod::MKVMerge
        {
            bail!(
                "mkvmerge is required for concatenating x265, as x265 outputs raw HEVC bitstream \
                 files without the timestamps correctly set, which FFmpeg cannot concatenate \
                 properly into a mkv file. Specify mkvmerge as the concatenation method by \
                 setting `--concat mkvmerge`."
            );
        }

        if self.encoder_settings.encoder == Encoder::vpx
            && self.chunk_settings.concat != ConcatMethod::MKVMerge
        {
            warn!(
                "mkvmerge is recommended for concatenating vpx, as vpx outputs with incorrect \
                 frame rates, which we can only resolve using mkvmerge. Specify mkvmerge as the \
                 concatenation method by setting `--concat mkvmerge`."
            );
        }

        match self.chunk_settings.chunk_method {
            ChunkMethod::LSMASH => ensure!(
                self.vapoursynth_plugins.is_some_and(|p| p.lsmash),
                "LSMASH is not installed, but it was specified as the chunk method"
            ),
            ChunkMethod::FFMS2 => ensure!(
                self.vapoursynth_plugins.is_some_and(|p| p.ffms2),
                "FFMS2 is not installed, but it was specified as the chunk method"
            ),
            ChunkMethod::DGDECNV => ensure!(
                self.vapoursynth_plugins.is_some_and(|p| p.dgdecnv)
                    && which::which("dgindexnv").is_ok(),
                "Either DGDecNV is not installed or DGIndexNV is not in system path, but it was \
                 specified as the chunk method"
            ),
            ChunkMethod::BESTSOURCE => ensure!(
                self.vapoursynth_plugins.is_some_and(|p| p.bestsource),
                "BestSource is not installed, but it was specified as the chunk method"
            ),
            ChunkMethod::Select => warn!(
                "It is not recommended to use the \"select\" chunk method, as it is very slow"
            ),
            _ => (),
        }

        if self.encoder_settings.encoder == Encoder::aom
            && self.chunk_settings.concat != ConcatMethod::MKVMerge
            && self
                .encoder_settings
                .video_params
                .iter()
                .any(|param| param == "--enable-keyframe-filtering=2")
        {
            bail!(
                "keyframe filtering mode 2 currently only works when using mkvmerge as the concat \
                 method"
            );
        }

        Ok(())
    }

    fn validate_tq_settings(&self) -> Result<()> {
        if let Some(vmaf_path) =
            &self.tq_settings.target_quality.as_ref().and_then(|tq| tq.model.as_ref())
        {
            ensure!(vmaf_path.exists());
        }

        if let Some(target_quality) = &self.tq_settings.target_quality {
            if self.io_settings.input.is_vapoursynth() {
                let input_absolute_path = to_absolute_path(self.io_settings.input.as_path())?;
                if !input_absolute_path.starts_with(std::env::current_dir()?) {
                    warn!(
                        "Target Quality with VapourSynth script file input not in current working \
                         directory. It is recommended to run in the same directory."
                    );
                }
            }

            match target_quality.metric {
                TargetMetric::VMAF => validate_libvmaf()?,
                TargetMetric::SSIMULACRA2 => {
                    ensure!(
                        self.vapoursynth_plugins.is_some_and(|p| p.vship)
                            || self
                                .vapoursynth_plugins
                                .is_some_and(|p| p.vszip != VSZipVersion::None),
                        "SSIMULACRA2 metric requires either Vapoursynth-HIP or VapourSynth Zig \
                         Image Process to be installed"
                    );
                    ensure!(
                        matches!(
                            self.chunk_settings.chunk_method,
                            ChunkMethod::LSMASH
                                | ChunkMethod::FFMS2
                                | ChunkMethod::BESTSOURCE
                                | ChunkMethod::DGDECNV
                        ),
                        "Chunk method must be lsmash, ffms2, bestsource, or dgdecnv for \
                         SSIMULACRA2"
                    );
                },
                TargetMetric::ButteraugliINF => {
                    ensure!(
                        self.vapoursynth_plugins.is_some_and(|p| p.vship)
                            || self.vapoursynth_plugins.is_some_and(|p| p.julek),
                        "Butteraugli metric requires either Vapoursynth-HIP or \
                         vapoursynth-julek-plugin to be installed"
                    );
                    ensure!(
                        matches!(
                            self.chunk_settings.chunk_method,
                            ChunkMethod::LSMASH
                                | ChunkMethod::FFMS2
                                | ChunkMethod::BESTSOURCE
                                | ChunkMethod::DGDECNV
                        ),
                        "Chunk method must be lsmash, ffms2, bestsource, or dgdecnv for \
                         Butteraugli"
                    );
                },
                TargetMetric::Butteraugli3 => {
                    ensure!(
                        self.vapoursynth_plugins.is_some_and(|p| p.vship),
                        "Butteraugli 3 Norm metric requires Vapoursynth-HIP plugin to be installed"
                    );
                    ensure!(
                        matches!(
                            self.chunk_settings.chunk_method,
                            ChunkMethod::LSMASH
                                | ChunkMethod::FFMS2
                                | ChunkMethod::BESTSOURCE
                                | ChunkMethod::DGDECNV
                        ),
                        "Chunk method must be lsmash, ffms2, bestsource, or dgdecnv for \
                         Butteraugli 3 Norm"
                    );
                },
                TargetMetric::XPSNR | TargetMetric::XPSNRWeighted => {
                    let metric_name = if target_quality.metric == TargetMetric::XPSNRWeighted {
                        "Weighted "
                    } else {
                        ""
                    };
                    if target_quality.probing_rate > 1 {
                        ensure!(
                            self.vapoursynth_plugins.is_some_and(|p| p.vszip == VSZipVersion::New),
                            format!(
                                "{metric_name}XPSNR metric with probing rate greater than 1 \
                                 requires VapourSynth-Zig Image Process R7 or newer to be \
                                 installed"
                            )
                        );
                        ensure!(
                            matches!(
                                self.chunk_settings.chunk_method,
                                ChunkMethod::LSMASH
                                    | ChunkMethod::FFMS2
                                    | ChunkMethod::BESTSOURCE
                                    | ChunkMethod::DGDECNV
                            ),
                            format!(
                                "Chunk method must be lsmash, ffms2, bestsource, or dgdecnv for \
                                 {metric_name}XPSNR with probing rate greater than 1"
                            )
                        );
                    } else {
                        validate_libxpsnr()?;
                    }
                },
            }

            if target_quality.probes < 4 {
                warn!("Target quality with less than 4 probes is experimental and not recommended");
            }

            if let Some(resolution) = &target_quality.probe_res {
                match resolution.split('x').collect::<Vec<&str>>().as_slice() {
                    [width_str, height_str] => {
                        match (width_str.parse::<u32>(), height_str.parse::<u32>()) {
                            (Ok(_width), Ok(_height)) => {},
                            _ => bail!("Failed to parse Probe Resolution"),
                        }
                    },
                    _ => bail!("Probe Resolution must be in the format widthxheight"),
                }
            }
        }

        Ok(())
    }

    fn validate_ffmpeg_settings(&self) -> Result<()> {
        if which::which("ffmpeg").is_err() {
            bail!("FFmpeg not found. Is it installed in system path?");
        }

        Ok(())
    }

    /// Initialize logging routines and create temporary directories
    #[tracing::instrument(level = "debug")]
    fn initialize(&mut self) -> Result<()> {
        if !self.av1an_settings.resume && Path::new(&self.io_settings.temp).is_dir() {
            fs::remove_dir_all(&self.io_settings.temp).with_context(|| {
                format!(
                    "Failed to remove temporary directory {temp}",
                    temp = self.io_settings.temp
                )
            })?;
        }

        create_dir!(Path::new(&self.io_settings.temp))?;
        create_dir!(Path::new(&self.io_settings.temp).join("split"))?;
        create_dir!(Path::new(&self.io_settings.temp).join("encode"))?;

        debug!("temporary directory: {temp}", temp = &self.io_settings.temp);

        let done_path = Path::new(&self.io_settings.temp).join("done.json");
        let done_json_exists = done_path.exists();
        let chunks_json_exists = Path::new(&self.io_settings.temp).join("chunks.json").exists();

        if self.av1an_settings.resume {
            match (done_json_exists, chunks_json_exists) {
                // both files exist, so there is no problem
                (true, true) => {},
                (false, true) => {
                    info!(
                        "resume was set but done.json does not exist in temporary directory {temp}",
                        temp = self.io_settings.temp
                    );
                    self.av1an_settings.resume = false;
                },
                (true, false) => {
                    info!(
                        "resume was set but chunks.json does not exist in temporary directory \
                         {temp}",
                        temp = self.io_settings.temp
                    );
                    self.av1an_settings.resume = false;
                },
                (false, false) => {
                    info!(
                        "resume was set but neither chunks.json nor done.json exist in temporary \
                         directory {temp}",
                        temp = self.io_settings.temp
                    );
                    self.av1an_settings.resume = false;
                },
            }
        }

        if self.av1an_settings.resume && done_json_exists {
            let done = fs::read_to_string(done_path)
                .with_context(|| "Failed to read contents of done.json")?;
            let done: DoneJson =
                serde_json::from_str(&done).with_context(|| "Failed to parse done.json")?;
            self.frames = done.frames.load(atomic::Ordering::Relaxed);

            // frames need to be recalculated in this case
            if self.frames == 0 {
                self.frames = self.io_settings.input.clip_info()?.num_frames;
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
    pub fn encode_file(&mut self) -> Result<()> {
        let tiles = self.encoder_settings.tiles.unwrap_or_else(|| self.input.calculate_tiles());
        let initial_frames =
            get_done().done.iter().map(|ref_multi| ref_multi.frames).sum::<usize>();

        // Create the VapourSynth script file and store the path to it and evaluate it
        let cache_vs_input = |vs_input: &Input| {
            let script_path = match vs_input {
                Input::VapourSynth {
                    path, ..
                } => path.clone(),
                Input::Video {
                    path,
                    is_proxy,
                    ..
                } => {
                    let (script_path, _) = create_vs_file(
                        &self.io_settings.temp,
                        path,
                        self.chunk_settings.chunk_method,
                        self.sc_settings.sc_downscale_height,
                        self.sc_settings.sc_pix_format,
                        &self.sc_settings.scaler,
                        *is_proxy,
                    )?;
                    script_path
                },
            };

            let variables_map = vs_input.as_vspipe_args_hashmap()?;
            let decoder = match vs_input {
                Input::VapourSynth {
                    path, ..
                } => {
                    let mut dec = VapoursynthDecoder::from_file(path)?;
                    dec.set_variables(variables_map)?;
                    av_scenechange::Decoder::from_decoder_impl(
                        av_decoders::DecoderImpl::Vapoursynth(dec),
                    )?
                },
                video_input => av_scenechange::Decoder::from_script(
                    &video_input.as_script_text(
                        self.sc_settings.sc_downscale_height,
                        self.sc_settings.sc_pix_format,
                        Some(self.sc_settings.scaler.as_str().into()),
                    )?,
                    Some(variables_map),
                )?,
            };
            // Getting the details will evaluate the script and produce the VapourSynth
            // cache file
            decoder.get_video_details();

            Ok::<PathBuf, anyhow::Error>(script_path)
        };

        // Technically we should check if the vapoursynth cache file exists rather than
        // !self.resume, but the code still works if we are resuming and the
        // cache file doesn't exist (as it gets generated when vspipe is first
        // called), so it's not worth adding all the extra complexity.
        if (self.io_settings.input.is_vapoursynth()
            || (self.io_settings.input.is_video()
                && matches!(
                    self.chunk_settings.chunk_method,
                    ChunkMethod::LSMASH
                        | ChunkMethod::FFMS2
                        | ChunkMethod::DGDECNV
                        | ChunkMethod::BESTSOURCE
                )))
            && !self.av1an_settings.resume
        {
            self.vs_script = Some(cache_vs_input(&self.io_settings.input)?);
        }
        if let Some(proxy) = &self.io_settings.proxy {
            if proxy.is_vapoursynth()
                || (proxy.is_video()
                    && matches!(
                        self.chunk_settings.chunk_method,
                        ChunkMethod::LSMASH
                            | ChunkMethod::FFMS2
                            | ChunkMethod::DGDECNV
                            | ChunkMethod::BESTSOURCE
                    )
                    && !self.av1an_settings.resume)
            {
                self.vs_proxy_script = Some(cache_vs_input(proxy)?);
            }
        }

        let clip_info = self.io_settings.input.clip_info()?;
        let res = clip_info.resolution;
        let fps_ratio = clip_info.frame_rate;
        let fps = fps_ratio.to_f64().unwrap();
        let format = clip_info.format_info;
        let tfc = clip_info.transfer_function_params_adjusted(&self.encoder_settings.video_params);
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

        if self.av1an_settings.sc_only {
            debug!("scene detection only");

            if let Err(e) = fs::remove_dir_all(&self.io_settings.temp) {
                warn!("Failed to delete temp directory: {e}");
            }

            exit(0);
        }

        let (chunk_queue, total_chunks) = self.load_or_gen_chunk_queue(&splits)?;

        let mut chunks_done = 0;
        if self.av1an_settings.resume {
            chunks_done = get_done().done.len();
            info!(
                "encoding resumed with {}/{} chunks completed ({} remaining)",
                chunks_done,
                total_chunks,
                chunk_queue.len()
            );
        }

        crossbeam_utils::thread::scope(|s| -> Result<()> {
            // vapoursynth audio is currently unsupported
            let audio_thread = if self.io_settings.input.is_video()
                && (!self.av1an_settings.resume
                    || !get_done().audio_done.load(atomic::Ordering::SeqCst))
            {
                let input = self.io_settings.input.as_video_path();
                let temp = self.io_settings.temp.as_str();
                let audio_params = self.ffmpeg_settings.audio_params.as_slice();
                Some(s.spawn(move |_| {
                    let audio_output =
                        crate::ffmpeg::encode_audio(input, temp, audio_params).unwrap();
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

            if self.av1an_settings.workers == 0 {
                self.av1an_settings.workers = self.determine_workers()? as usize;
            }
            self.av1an_settings.workers = cmp::min(self.av1an_settings.workers, chunk_queue.len());

            // TODO: Move this message and all progress stuff to CLI
            info!(
                "\n{}{} {} {}{} {} {}{} {} {}{} {}\n{}: {}",
                "Q".green().bold(),
                "ueue".green(),
                format!("{len}", len = chunk_queue.len()).green().bold(),
                "W".blue().bold(),
                "orkers".blue(),
                format!("{workers}", workers = self.av1an_settings.workers).blue().bold(),
                "E".purple().bold(),
                "ncoder".purple(),
                format!("{encoder}", encoder = self.encoder_settings.encoder).purple().bold(),
                "P".purple().bold(),
                "asses".purple(),
                format!("{passes}", passes = self.encoder_settings.passes).purple().bold(),
                "Params".bold(),
                self.encoder_settings.video_params.join(" ").dimmed()
            );

            if self.encoder_settings.verbosity == Verbosity::Normal {
                init_progress_bar(
                    self.frames as u64,
                    initial_frames as u64,
                    Some((chunks_done as u32, total_chunks as u32)),
                );
                reset_bar_at(initial_frames as u64);
            } else if self.encoder_settings.verbosity == Verbosity::Verbose {
                init_multi_progress_bar(
                    self.frames as u64,
                    self.encoder_settings.workers,
                    initial_frames as u64,
                    (chunks_done as u32, total_chunks as u32),
                );
                reset_mp_bar_at(initial_frames as u64);
            }

            if chunks_done > 0 {
                update_progress_bar_estimates(
                    fps,
                    self.frames,
                    self.encoder_settings.verbosity,
                    (chunks_done as u32, total_chunks as u32),
                );
            }

            let broker = Broker {
                chunk_queue,
                project: self,
            };

            let (tx, rx) = mpsc::channel();
            let handle = s.spawn(|_| {
                broker.encoding_loop(
                    tx,
                    self.av1an_settings.set_thread_affinity,
                    total_chunks as u32,
                );
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
                concat = self.chunk_settings.concat
            );

            match self.chunk_settings.concat {
                ConcatMethod::Ivf => {
                    concat::ivf(
                        &Path::new(&self.io_settings.temp).join("encode"),
                        self.io_settings.output_file.as_ref(),
                    )?;
                },
                ConcatMethod::MKVMerge => {
                    concat::mkvmerge(
                        self.io_settings.temp.as_ref(),
                        self.io_settings.output_file.as_ref(),
                        self.encoder_settings.encoder,
                        total_chunks,
                        if self.av1an_settings.ignore_frame_mismatch {
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
                    concat::ffmpeg(
                        self.io_settings.temp.as_ref(),
                        self.io_settings.output_file.as_ref(),
                    )?;
                },
            }

            if self.tq_settings.vmaf || self.tq_settings.target_quality.is_some() {
                let vmaf_res = if let Some(ref tq) = self.tq_settings.target_quality {
                    if tq.vmaf_res == "inputres" {
                        let inputres = self.io_settings.input.clip_info()?.resolution;
                        format!("{width}x{height}", width = inputres.0, height = inputres.1)
                    } else {
                        tq.vmaf_res.clone()
                    }
                } else {
                    self.tq_settings.vmaf_res.clone()
                };

                let vmaf_model = self.tq_settings.vmaf_path.as_deref().or_else(|| {
                    self.tq_settings.target_quality.as_ref().and_then(|tq| tq.model.as_deref())
                });
                let vmaf_scaler = "bicubic";
                let vmaf_filter = self.tq_settings.vmaf_filter.as_deref().or_else(|| {
                    self.tq_settings
                        .target_quality
                        .as_ref()
                        .and_then(|tq| tq.vmaf_filter.as_deref())
                });

                if self.tq_settings.vmaf {
                    let vmaf_threads = available_parallelism().map_or(1, std::num::NonZero::get);

                    if let Err(e) = vmaf::plot(
                        self.io_settings.output_file.as_ref(),
                        &self.io_settings.input,
                        vmaf_model,
                        &vmaf_res,
                        vmaf_scaler,
                        1,
                        vmaf_filter,
                        vmaf_threads,
                        self.tq_settings
                            .target_quality
                            .as_ref()
                            .map_or(&[], |tq| &tq.probing_vmaf_features),
                    ) {
                        error!("VMAF calculation failed with error: {e}");
                    }
                }
            }

            if !Path::new(&self.io_settings.output_file).exists() {
                warn!(
                    "Concatenation failed for unknown reasons! Temp folder will not be deleted: \
                     {temp}",
                    temp = self.io_settings.temp
                );
            } else if !self.av1an_settings.keep {
                if let Err(e) = fs::remove_dir_all(&self.io_settings.temp) {
                    warn!("Failed to delete temp directory: {e}");
                }
            }

            Ok(())
        })
        .unwrap()?;

        Ok(())
    }

    #[tracing::instrument(level = "debug")]
    fn read_queue_files(source_path: &Path) -> Result<Vec<PathBuf>> {
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

        let (source_pipe_stderr, ffmpeg_pipe_stderr, enc_output, enc_stderr, frame) =
            thread::scope(|scope| {
                let mut source_pipe = if let [source, args @ ..] = &*chunk.source_cmd {
                    let mut command = Command::new(source);
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

                let source_pipe_stdout: Stdio = source_pipe.stdout.take().unwrap().into();
                let source_pipe_stderr = source_pipe.stderr.take().unwrap();

                // converts the pixel format
                let create_ffmpeg_pipe = |pipe_from: Stdio, source_pipe_stderr: ChildStderr| {
                    let ffmpeg_pipe = compose_ffmpeg_pipe(
                        self.ffmpeg_settings.ffmpeg_filter_args.as_slice(),
                        self.ffmpeg_settings.output_pix_format.format,
                    );

                    let mut ffmpeg_pipe = if let [ffmpeg, args @ ..] = &*ffmpeg_pipe {
                        Command::new(ffmpeg)
                            .args(args)
                            .stdin(pipe_from)
                            .stdout(Stdio::piped())
                            .stderr(Stdio::piped())
                            .spawn()
                            .unwrap()
                    } else {
                        unreachable!()
                    };

                    let ffmpeg_pipe_stdout: Stdio = ffmpeg_pipe.stdout.take().unwrap().into();
                    let ffmpeg_pipe_stderr = ffmpeg_pipe.stderr.take().unwrap();
                    (
                        ffmpeg_pipe_stdout,
                        source_pipe_stderr,
                        Some(ffmpeg_pipe_stderr),
                    )
                };

                let (y4m_pipe, source_pipe_stderr, mut ffmpeg_pipe_stderr) =
                    if self.ffmpeg_settings.ffmpeg_filter_args.is_empty() {
                        match &self.io_settings.input_pix_format {
                            InputPixelFormat::FFmpeg {
                                format,
                            } => {
                                if self.io_settings.output_pix_format.format == *format {
                                    (source_pipe_stdout, source_pipe_stderr, None)
                                } else {
                                    create_ffmpeg_pipe(source_pipe_stdout, source_pipe_stderr)
                                }
                            },
                            InputPixelFormat::VapourSynth {
                                bit_depth,
                            } => {
                                if self.io_settings.output_pix_format.bit_depth == *bit_depth {
                                    (source_pipe_stdout, source_pipe_stderr, None)
                                } else {
                                    create_ffmpeg_pipe(source_pipe_stdout, source_pipe_stderr)
                                }
                            },
                        }
                    } else {
                        create_ffmpeg_pipe(source_pipe_stdout, source_pipe_stderr)
                    };

                let source_reader = BufReader::new(source_pipe_stderr);
                let ffmpeg_reader = ffmpeg_pipe_stderr.take().map(BufReader::new);

                let pipe_stderr = Arc::new(Mutex::new(String::with_capacity(128)));
                let p_stdr2 = Arc::clone(&pipe_stderr);

                let ffmpeg_stderr = if ffmpeg_reader.is_some() {
                    Some(Arc::new(Mutex::new(String::with_capacity(128))))
                } else {
                    None
                };

                let f_stdr2 = ffmpeg_stderr.clone();

                scope.spawn(move || {
                    for line in source_reader.lines() {
                        let mut lock = p_stdr2.lock().unwrap();
                        lock.push_str(&line.unwrap());
                        lock.push('\n');
                    }
                });
                if let Some(ffmpeg_reader) = ffmpeg_reader {
                    let f_stdr2 = f_stdr2.unwrap();
                    scope.spawn(move || {
                        for line in ffmpeg_reader.lines() {
                            let mut lock = f_stdr2.lock().unwrap();
                            lock.push_str(&line.unwrap());
                            lock.push('\n');
                        }
                    });
                }

                let mut enc_pipe = if let [encoder, args @ ..] = &*enc_cmd {
                    Command::new(encoder)
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

                while let Ok(read) = reader.read_until(b'\r', &mut buf) {
                    if read == 0 {
                        break;
                    }

                    // TODO: Move all progress bars to CLI
                    if let Ok(line) = simdutf8::basic::from_utf8_mut(&mut buf) {
                        if self.encoder_settings.verbosity == Verbosity::Verbose
                            && !line.contains('\n')
                        {
                            update_mp_msg(worker_id, line.trim().to_string());
                        }
                        // This needs to be done before parse_encoded_frames, as it potentially
                        // mutates the string
                        enc_stderr.push_str(line);
                        enc_stderr.push('\n');

                        if current_pass == chunk.passes {
                            if let Some(new) = chunk.encoder.parse_encoded_frames(line) {
                                if new > frame {
                                    if self.encoder_settings.verbosity == Verbosity::Normal {
                                        inc_bar(new - frame);
                                    } else if self.encoder_settings.verbosity == Verbosity::Verbose
                                    {
                                        inc_mp_bar(new - frame);
                                    }
                                    frame = new;
                                }
                            }
                        }
                    }

                    buf.clear();
                }

                let enc_output = enc_pipe.wait_with_output().unwrap();

                let source_pipe_stderr = pipe_stderr.lock().unwrap().clone();
                let ffmpeg_pipe_stderr = ffmpeg_stderr.map(|x| x.lock().unwrap().clone());
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

    fn create_encoding_queue(&self, scenes: &[Scene]) -> Result<Vec<Chunk>> {
        let mut chunks = match &self.io_settings.input {
            Input::Video {
                ..
            } => match self.chunk_settings.chunk_method {
                ChunkMethod::FFMS2
                | ChunkMethod::LSMASH
                | ChunkMethod::DGDECNV
                | ChunkMethod::BESTSOURCE => {
                    let vs_script = self.vs_script.as_ref().unwrap().as_path();
                    let vs_proxy_script = self.vs_proxy_script.as_deref();
                    self.create_video_queue_vs(scenes, vs_script, vs_proxy_script)
                },
                ChunkMethod::Hybrid => self.create_video_queue_hybrid(scenes)?,
                ChunkMethod::Select => self.create_video_queue_select(scenes),
                ChunkMethod::Segment => self.create_video_queue_segment(scenes)?,
            },
            Input::VapourSynth {
                path, ..
            } => {
                self.create_video_queue_vs(scenes, path.as_path(), self.vs_proxy_script.as_deref())
            },
        };

        match self.chunk_settings.chunk_order {
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
    fn split_routine(&mut self) -> Result<&[Scene]> {
        let scene_file = self.sc_settings.scenes.as_ref().map_or_else(
            || Cow::Owned(Path::new(&self.io_settings.temp).join("scenes.json")),
            |path| Cow::Borrowed(path.as_path()),
        );
        if scene_file.exists() && (self.sc_settings.scenes.is_some() || self.av1an_settings.resume)
        {
            self.scene_factory = SceneFactory::from_scenes_file(&scene_file)?;
        } else {
            let zones = parse_zones(&self.encoder_settings, self.frames)?;
            self.scene_factory.compute_scenes(&self.encoder_settings, &zones)?;
            self.scene_factory.write_scenes_to_file(scene_file)?;
        }
        self.frames = self.scene_factory.get_frame_count();
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
    ) -> Result<Chunk> {
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
            self.io_settings.output_pix_format.format.to_pix_fmt_string(),
            "-strict",
            "-1",
            "-f",
            "yuv4mpegpipe",
            "-",
        ];

        let output_ext = self.encoder_settings.encoder.output_extension();

        let mut chunk = Chunk {
            temp: self.io_settings.temp.clone(),
            index,
            input: Input::Video {
                path:         src_path.to_path_buf(),
                temp:         self.io_settings.temp.clone(),
                chunk_method: ChunkMethod::Select,
                is_proxy:     false,
            },
            proxy: self.io_settings.proxy.as_ref().map(|proxy| Input::Video {
                path:         proxy.as_path().to_path_buf(),
                temp:         self.io_settings.temp.clone(),
                chunk_method: ChunkMethod::Select,
                is_proxy:     true,
            }),
            source_cmd: ffmpeg_gen_cmd,
            proxy_cmd: None,
            output_ext: output_ext.to_owned(),
            start_frame,
            end_frame,
            frame_rate,
            video_params: overrides.as_ref().map_or_else(
                || self.encoder_settings.video_params.clone(),
                |ovr| ovr.video_params.clone(),
            ),
            passes: overrides.as_ref().map_or(self.encoder_settings.passes, |ovr| ovr.passes),
            encoder: overrides.as_ref().map_or(self.encoder_settings.encoder, |ovr| ovr.encoder),
            noise_size: self.encoder_settings.photon_noise_size,
            tq_cq: None,
            ignore_frame_mismatch: self.av1an_settings.ignore_frame_mismatch,
        };
        chunk.apply_photon_noise_args(
            overrides.map_or(self.encoder_settings.photon_noise, |ovr| ovr.photon_noise),
            self.encoder_settings.chroma_noise,
        )?;
        if let Some(ref tq) = self.tq_settings.target_quality {
            tq.per_shot_target_quality_routine(
                &mut chunk,
                None,
                self.vapoursynth_plugins.as_ref(),
            )?;
        }
        Ok(chunk)
    }

    fn create_vs_chunk(
        &self,
        index: usize,
        vs_script: &Path,
        vs_proxy_script: Option<&Path>,
        scene: &Scene,
        frame_rate: f64,
    ) -> Result<Chunk> {
        // the frame end boundary is actually a frame that should be included in the
        // next chunk
        let frame_end = scene.end_frame - 1;

        fn gen_vspipe_cmd(vs_script: &Path, scene_start: usize, scene_end: usize) -> Vec<OsString> {
            into_vec![
                "vspipe",
                vs_script,
                "-c",
                "y4m",
                "-",
                "-s",
                scene_start.to_string(),
                "-e",
                scene_end.to_string(),
            ]
        }

        let vspipe_cmd_gen = gen_vspipe_cmd(vs_script, scene.start_frame, frame_end);
        let vspipe_proxy_cmd_gen = vs_proxy_script
            .map(|vs_proxy_script| gen_vspipe_cmd(vs_proxy_script, scene.start_frame, frame_end));

        let output_ext = self.encoder_settings.encoder.output_extension();

        let mut chunk = Chunk {
            temp: self.io_settings.temp.clone(),
            index,
            input: Input::VapourSynth {
                path:        vs_script.to_path_buf(),
                vspipe_args: self.io_settings.input.as_vspipe_args_vec()?,
                script_text: self.io_settings.input.as_script_text(
                    self.sc_settings.sc_downscale_height,
                    self.sc_settings.sc_pix_format,
                    Some(self.sc_settings.scaler.as_str().into()),
                )?,
                is_proxy:    false,
            },
            proxy: if let Some(vs_proxy_script) = vs_proxy_script {
                Some(Input::VapourSynth {
                    path:        vs_proxy_script.to_path_buf(),
                    vspipe_args: self.io_settings.proxy.as_ref().unwrap().as_vspipe_args_vec()?,
                    script_text: self.io_settings.proxy.as_ref().unwrap().as_script_text(
                        self.sc_settings.sc_downscale_height,
                        self.sc_settings.sc_pix_format,
                        Some(self.sc_settings.scaler.as_str().into()),
                    )?,
                    is_proxy:    true,
                })
            } else {
                None
            },
            source_cmd: vspipe_cmd_gen,
            proxy_cmd: vspipe_proxy_cmd_gen,
            output_ext: output_ext.to_owned(),
            start_frame: scene.start_frame,
            end_frame: scene.end_frame,
            frame_rate,
            video_params: scene.zone_overrides.as_ref().map_or_else(
                || self.encoder_settings.video_params.clone(),
                |ovr| ovr.video_params.clone(),
            ),
            passes: scene
                .zone_overrides
                .as_ref()
                .map_or(self.encoder_settings.passes, |ovr| ovr.passes),
            encoder: scene
                .zone_overrides
                .as_ref()
                .map_or(self.encoder_settings.encoder, |ovr| ovr.encoder),
            noise_size: scene
                .zone_overrides
                .as_ref()
                .map_or(self.encoder_settings.photon_noise_size, |ovr| {
                    (ovr.photon_noise_width, ovr.photon_noise_height)
                }),
            tq_cq: None,
            ignore_frame_mismatch: self.av1an_settings.ignore_frame_mismatch,
        };
        chunk.apply_photon_noise_args(
            scene
                .zone_overrides
                .as_ref()
                .map_or(self.encoder_settings.photon_noise, |ovr| ovr.photon_noise),
            scene
                .zone_overrides
                .as_ref()
                .map_or(self.encoder_settings.chroma_noise, |ovr| ovr.chroma_noise),
        )?;
        Ok(chunk)
    }

    fn create_video_queue_vs(
        &self,
        scenes: &[Scene],
        vs_script: &Path,
        vs_proxy_script: Option<&Path>,
    ) -> Vec<Chunk> {
        let frame_rate = self.io_settings.input.clip_info().unwrap().frame_rate.to_f64().unwrap();
        let chunk_queue: Vec<Chunk> = scenes
            .iter()
            .enumerate()
            .map(|(index, scene)| {
                self.create_vs_chunk(index, vs_script, vs_proxy_script, scene, frame_rate)
                    .unwrap()
            })
            .collect();

        chunk_queue
    }

    fn create_video_queue_select(&self, scenes: &[Scene]) -> Vec<Chunk> {
        let input = self.io_settings.input.as_video_path();
        let frame_rate = self.io_settings.input.clip_info().unwrap().frame_rate.to_f64().unwrap();

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

    fn create_video_queue_segment(&self, scenes: &[Scene]) -> Result<Vec<Chunk>> {
        let input = self.io_settings.input.as_video_path();
        let frame_rate = self.io_settings.input.clip_info()?.frame_rate.to_f64().unwrap();

        debug!("Splitting video");
        segment(
            input,
            &self.io_settings.temp,
            &scenes.iter().skip(1).map(|scene| scene.start_frame).collect::<Vec<usize>>(),
        );
        debug!("Splitting done");

        let source_path = Path::new(&self.io_settings.temp).join("split");
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

    fn create_video_queue_hybrid(&self, scenes: &[Scene]) -> Result<Vec<Chunk>> {
        let input = self.io_settings.input.as_video_path();
        let frame_rate = self.io_settings.input.clip_info()?.frame_rate.to_f64().unwrap();

        let keyframes = crate::ffmpeg::get_keyframes(input).unwrap();

        let to_split: Vec<usize> = keyframes
            .iter()
            .filter(|kf| scenes.iter().any(|scene| scene.start_frame == **kf))
            .copied()
            .collect();

        debug!("Segmenting video");
        segment(input, &self.io_settings.temp, &to_split[1..]);
        debug!("Segment done");

        let source_path = Path::new(&self.io_settings.temp).join("split");
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
    ) -> Result<Chunk> {
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
            self.ffmpeg_settings.output_pix_format.format.to_pix_fmt_string(),
            "-f",
            "yuv4mpegpipe",
            "-",
        ];

        let output_ext = self.encoder_settings.encoder.output_extension();

        let num_frames = get_num_frames(Path::new(file))?;

        let mut chunk = Chunk {
            temp: self.io_settings.temp.clone(),
            input: Input::Video {
                path:         PathBuf::from(file),
                temp:         self.io_settings.temp.clone(),
                chunk_method: ChunkMethod::Segment,
                is_proxy:     false,
            },
            proxy: self.io_settings.proxy.as_ref().map(|proxy| Input::Video {
                path:         proxy.as_path().to_path_buf(),
                temp:         self.io_settings.temp.clone(),
                chunk_method: ChunkMethod::Segment,
                is_proxy:     true,
            }),
            source_cmd: ffmpeg_gen_cmd,
            proxy_cmd: None,
            output_ext: output_ext.to_owned(),
            index,
            start_frame: 0,
            end_frame: num_frames,
            frame_rate,
            video_params: overrides.as_ref().map_or_else(
                || self.encoder_settings.video_params.clone(),
                |ovr| ovr.video_params.clone(),
            ),
            passes: overrides.as_ref().map_or(self.encoder_settings.passes, |ovr| ovr.passes),
            encoder: overrides.as_ref().map_or(self.encoder_settings.encoder, |ovr| ovr.encoder),
            noise_size: self.encoder_settings.photon_noise_size,
            tq_cq: None,
            ignore_frame_mismatch: self.av1an_settings.ignore_frame_mismatch,
        };
        chunk.apply_photon_noise_args(
            overrides.map_or(self.encoder_settings.photon_noise, |ovr| ovr.photon_noise),
            self.encoder_settings.chroma_noise,
        )?;
        Ok(chunk)
    }

    /// Returns unfinished chunks and number of total chunks
    fn load_or_gen_chunk_queue(&self, splits: &[Scene]) -> Result<(Vec<Chunk>, usize)> {
        if self.av1an_settings.resume {
            let mut chunks = read_chunk_queue(self.io_settings.temp.as_ref())?;
            let num_chunks = chunks.len();

            let done = get_done();

            // only keep the chunks that are not done
            chunks.retain(|chunk| !done.done.contains_key(&chunk.name()));

            Ok((chunks, num_chunks))
        } else {
            let chunks = self.create_encoding_queue(splits)?;
            let num_chunks = chunks.len();
            save_chunk_queue(&self.io_settings.temp, &chunks)?;
            Ok((chunks, num_chunks))
        }
    }

    /// Determine the optimal number of workers for an encoder
    fn determine_workers(&self) -> anyhow::Result<u64> {
        let res = self.io_settings.input.clip_info()?.resolution;
        let tiles = self.encoder_settings.tiles;
        let megapixels = (res.0 * res.1) as f64 / 1e6;
        // encoder memory and chunk_method memory usage scales with resolution
        // (megapixels), approximately linearly. Expressed as GB/Megapixel
        let cm_ram = match self.chunk_settings.chunk_method {
            ChunkMethod::FFMS2 | ChunkMethod::LSMASH | ChunkMethod::BESTSOURCE => 0.3,
            ChunkMethod::DGDECNV => 0.3,
            ChunkMethod::Hybrid | ChunkMethod::Select | ChunkMethod::Segment => 0.1,
        };
        let enc_ram = match self.encoder_settings.encoder {
            Encoder::aom => 0.4,
            Encoder::rav1e => 0.7,
            Encoder::svt_av1 => 1.2,
            Encoder::vpx => 0.3,
            Encoder::x264 => 0.7,
            Encoder::x265 => 0.6,
        };
        // This is a rough estimate of how many cpu cores will be fully loaded by an
        // encoder worker. With rav1e, CPU usage scales with tiles, but not 1:1.
        // Other encoders don't seem to significantly scale CPU usage with tiles.
        // CPU threads/worker here is relative to default threading parameters, e.g. aom
        // will use 1 thread/worker if --threads=1 is set.
        let cpu_threads = match self.encoder_settings.encoder {
            Encoder::aom => 4,
            Encoder::rav1e => ((tiles.0 * tiles.1) as f32 * 0.7).ceil() as u64,
            Encoder::svt_av1 => 6,
            Encoder::vpx => 3,
            Encoder::x264 | Encoder::x265 => 8,
        };
        // memory usage scales with pixel format, expressed as a multiplier of memory
        // usage. Roughly the same behavior was observed accross all encoders.
        let pix_mult = match self.ffmpeg_settings.output_pix_format.format {
            FFPixelFormat::YUV444P | FFPixelFormat::YUV444P10LE | FFPixelFormat::YUV444P12LE => 1.5,
            FFPixelFormat::YUV422P | FFPixelFormat::YUV422P10LE | FFPixelFormat::YUV422P12LE => {
                1.25
            },
            _ => 1.0,
        };

        let mut system = sysinfo::System::new();
        system.refresh_memory();
        let cpu = available_parallelism()
            .expect("Unrecoverable: Failed to get thread count")
            .get() as u64;
        // sysinfo returns Bytes, convert to GB
        // use total instead of available, because av1an does not resize worker pool
        let ram_gb = system.total_memory() as f64 / 1e9;

        Ok(std::cmp::max(
            std::cmp::min(
                cpu / cpu_threads,
                (ram_gb / (megapixels * (enc_ram + cm_ram) * pix_mult)).round() as u64,
            ),
            1,
        ))
    }
}
