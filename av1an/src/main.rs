use std::{
    io::{self, Write},
    panic,
    path::{Path, PathBuf},
    process,
    process::exit,
    thread::available_parallelism,
};

use ::ffmpeg::format::Pixel;
use anyhow::{anyhow, bail, ensure, Context};
use av1an_core::{
    ffmpeg,
    hash_path,
    init_logging,
    into_vec,
    read_in_dir,
    vapoursynth,
    Av1anContext,
    ChunkMethod,
    ChunkOrdering,
    ConcatMethod,
    EncodeArgs,
    Encoder,
    Input,
    InputPixelFormat,
    PixelFormat,
    ProbingSpeed,
    ProbingStatistic,
    ProbingStatisticName,
    ScenecutMethod,
    SplitMethod,
    TargetMetric,
    TargetQuality,
    Verbosity,
    VmafFeature,
    DEFAULT_LOG_LEVEL,
};
use clap::{value_parser, Parser};
use num_traits::cast::ToPrimitive;
use once_cell::sync::OnceCell;
use path_abs::{PathAbs, PathInfo};
use tracing::{instrument, level_filters::LevelFilter, warn};

fn main() -> anyhow::Result<()> {
    let orig_hook = panic::take_hook();
    // Catch panics in child threads
    panic::set_hook(Box::new(move |panic_info| {
        orig_hook(panic_info);
        process::exit(1);
    }));
    run()
}

// needs to be static, runtime allocated string to avoid evil hacks to
// concatenate non-trivial strings at compile-time
fn version() -> &'static str {
    fn get_vs_info() -> String {
        let isfound = |found: bool| if found { "Found" } else { "Not found" };
        format!(
            "\
* VapourSynth Plugins
  systems.innocent.lsmas : {}
  com.vapoursynth.ffms2  : {}
  com.vapoursynth.dgdecodenv : {}
  com.vapoursynth.bestsource : {}
  com.julek.plugin : {}
  com.julek.vszip : {}
  com.lumen.vship : {}",
            isfound(vapoursynth::is_lsmash_installed()),
            isfound(vapoursynth::is_ffms2_installed()),
            isfound(vapoursynth::is_dgdecnv_installed()),
            isfound(vapoursynth::is_bestsource_installed()),
            isfound(vapoursynth::is_julek_installed()),
            isfound(vapoursynth::is_vszip_installed()),
            isfound(vapoursynth::is_vship_installed())
        )
    }

    fn get_encoder_info() -> String {
        format!(
            "\
* Available Encoders
  aomenc  : {}
  SVT-AV1 : {}
  rav1e   : {}
  x264    : {}
  x265    : {}
  vpxenc  : {}",
            Encoder::aom.version_text().as_deref().unwrap_or("Not found"),
            Encoder::svt_av1.version_text().as_deref().unwrap_or("Not found"),
            Encoder::rav1e.version_text().as_deref().unwrap_or("Not found"),
            Encoder::x264.version_text().as_deref().unwrap_or("Not found"),
            Encoder::x265.version_text().as_deref().unwrap_or("Not found"),
            Encoder::vpx.version_text().as_deref().unwrap_or("Not found")
        )
    }

    static INSTANCE: OnceCell<String> = OnceCell::new();
    INSTANCE.get_or_init(|| {
        match (
            option_env!("VERGEN_GIT_SHA"),
            option_env!("VERGEN_CARGO_DEBUG"),
            option_env!("VERGEN_RUSTC_SEMVER"),
            option_env!("VERGEN_RUSTC_LLVM_VERSION"),
            option_env!("VERGEN_CARGO_TARGET_TRIPLE"),
            option_env!("VERGEN_GIT_COMMIT_DATE"),
        ) {
            (
                Some(git_hash),
                Some(cargo_debug),
                Some(rustc_ver),
                Some(llvm_ver),
                Some(target_triple),
                Some(commit_date),
            ) => {
                format!(
                    "{}-unstable (rev {}) ({})

* Compiler
  rustc {} (LLVM {})

* Target Triple
  {}

* Date Info
  Commit Date:  {}

{}

{}",
                    env!("CARGO_PKG_VERSION"),
                    git_hash,
                    if cargo_debug.parse::<bool>().unwrap() {
                        "Debug"
                    } else {
                        "Release"
                    },
                    rustc_ver,
                    llvm_ver,
                    target_triple,
                    commit_date,
                    get_vs_info(),
                    get_encoder_info()
                )
            },
            _ => format!(
                "\
{}

{}

{}",
                // only include the semver on a release (when git information isn't available)
                env!("CARGO_PKG_VERSION"),
                get_vs_info(),
                get_encoder_info()
            ),
        }
    })
}

/// Cross-platform command-line AV1 / VP9 / HEVC / H264 encoding framework with
/// per-scene quality encoding
#[derive(Parser, Debug)]
#[clap(name = "av1an", version = version())]
pub struct CliOpts {
    /// Input file to encode
    ///
    /// Can be a video or VapourSynth (.py, .vpy) script.
    #[clap(short, required = true)]
    pub input: Vec<PathBuf>,

    /// Video output file
    #[clap(short)]
    pub output_file: Option<PathBuf>,

    /// Temporary directory to use
    ///
    /// If not specified, the temporary directory name is a hash of the input
    /// file name.
    #[clap(long)]
    pub temp: Option<PathBuf>,

    /// Disable printing progress to the terminal
    #[clap(short, long, conflicts_with = "verbose")]
    pub quiet: bool,

    /// Print extra progress info and stats to terminal
    #[clap(long)]
    pub verbose: bool,

    /// Log file location under ./logs [default: ./logs/av1an.log]
    ///
    /// Must be a relative path. Prepending with ./logs is optional.
    #[clap(short, long)]
    pub log_file: Option<String>,

    /// Set log level for log file (does not affect command-line log level)
    ///
    /// error: Designates very serious errors.
    ///
    /// warn: Designates hazardous situations.
    ///
    /// info: Designates useful information.
    ///
    /// debug: Designates lower priority information.
    ///
    /// trace: Designates very low priority, often extremely verbose,
    /// information. Includes rav1e scenechange decision info.
    #[clap(long, default_value_t = DEFAULT_LOG_LEVEL, ignore_case = true)]
    // "off" is also an allowed value for LevelFilter but we just disable the user from setting it
    pub log_level: LevelFilter,

    /// Resume previous session from temporary directory
    #[clap(short, long)]
    pub resume: bool,

    /// Do not delete the temporary folder after encoding has finished
    #[clap(short, long)]
    pub keep: bool,

    /// Do not check if the encoder arguments specified by -v/--video-params are
    /// valid.
    #[clap(long)]
    pub force: bool,

    /// Do not include Av1an's default set of encoder parameters.
    #[clap(long)]
    pub no_defaults: bool,

    /// Overwrite output file, without confirmation
    #[clap(short = 'y')]
    pub overwrite: bool,

    /// Never overwrite output file, without confirmation
    #[clap(short = 'n', conflicts_with = "overwrite")]
    pub never_overwrite: bool,

    /// Maximum number of chunk restarts for an encode
    #[clap(long, default_value_t = 3, value_parser = value_parser!(u32).range(1..))]
    pub max_tries: u32,

    /// Number of workers to spawn [0 = automatic]
    #[clap(short, long, default_value_t = 0)]
    pub workers: usize,

    /// Pin each worker to a specific set of threads of this size (disabled by
    /// default)
    ///
    /// This is currently only supported on Linux and Windows, and does nothing
    /// on unsupported platforms. Leaving this option unspecified allows the
    /// OS to schedule all processes spawned.
    #[clap(long)]
    pub set_thread_affinity: Option<usize>,

    /// Scaler used for scene detection (if --sc-downscale-height XXXX is used)
    /// and VMAF calculation
    ///
    /// Valid scalers are based on the scalers available in ffmpeg, including
    /// lanczos[1-9] with [1-9] defining the width of the lanczos scaler.
    #[clap(long, default_value = "bicubic")]
    pub scaler: String,

    /// Pass python argument(s) to the script environment
    /// --vspipe-args "message=fluffy kittens" "head=empty"
    #[clap(long, num_args(0..))]
    pub vspipe_args: Vec<String>,

    /// File location for scenes
    #[clap(short, long, help_heading = "Scene Detection")]
    pub scenes: Option<PathBuf>,

    /// Run the scene detection only before exiting
    ///
    /// Requires a scene file with --scenes.
    #[clap(long, requires("scenes"), help_heading = "Scene Detection")]
    pub sc_only: bool,

    /// Method used to determine chunk boundaries
    ///
    /// "av-scenechange" uses an algorithm to analyze which frames of the video
    /// are the start of new scenes, while "none" disables scene detection
    /// entirely (and only relies on -x/--extra-split to
    /// add extra scenecuts).
    #[clap(long, default_value_t = SplitMethod::AvScenechange, help_heading = "Scene Detection")]
    pub split_method: SplitMethod,

    /// Scene detection algorithm to use for av-scenechange
    ///
    /// Standard: Most accurate, still reasonably fast. Uses a cost-based
    /// algorithm to determine keyframes.
    ///
    /// Fast: Very fast, but less accurate. Determines keyframes based on the
    /// raw difference between pixels.
    #[clap(long, default_value_t = ScenecutMethod::Standard, help_heading = "Scene Detection")]
    pub sc_method: ScenecutMethod,

    /// Optional downscaling for scene detection
    ///
    /// Specify as the desired maximum height to scale to (e.g. "720" to
    /// downscale to 720p — this will leave lower resolution content
    /// untouched). Downscaling improves scene detection speed but lowers
    /// accuracy, especially when scaling to very low resolutions.
    ///
    /// By default, no downscaling is performed.
    #[clap(long, help_heading = "Scene Detection")]
    pub sc_downscale_height: Option<usize>,

    /// Perform scene detection with this pixel format
    #[clap(long, help_heading = "Scene Detection")]
    pub sc_pix_format: Option<Pixel>,

    /// Maximum scene length
    ///
    /// When a scenecut is found whose distance to the previous scenecut is
    /// greater than the value specified by this option, one or more extra
    /// splits (scenecuts) are added. Set this option to 0 to disable adding
    /// extra splits.
    #[clap(short = 'x', long, help_heading = "Scene Detection")]
    pub extra_split: Option<usize>,

    /// Maximum scene length, in seconds
    ///
    /// If both frames and seconds are specified, then the number of frames will
    /// take priority.
    #[clap(long, default_value_t = 10.0, help_heading = "Scene Detection")]
    pub extra_split_sec: f64,

    /// Minimum number of frames for a scenecut
    #[clap(long, default_value_t = 24, help_heading = "Scene Detection")]
    pub min_scene_len: usize,

    /// Comma-separated list of frames to force as keyframes
    ///
    /// Can be useful for improving seeking with chapters, etc.
    /// Frame 0 will always be a keyframe and does not need to be specified
    /// here.
    #[clap(long, help_heading = "Scene Detection")]
    pub force_keyframes: Option<String>,

    /// Video encoder to use
    #[clap(short, long, default_value_t = Encoder::aom, help_heading = "Encoding")]
    pub encoder: Encoder,

    /// Parameters for video encoder
    ///
    /// These parameters are for the encoder binary directly, so the ffmpeg
    /// syntax cannot be used. For example, CRF is specified in ffmpeg via
    /// "-crf <CRF>", but the x264 binary takes this value with double
    /// dashes, as in "--crf <CRF>". See the --help output of each encoder for
    /// a list of valid options. This list of parameters will be merged into
    /// Av1an's default set of encoder parameters.
    #[clap(short, long, allow_hyphen_values = true, help_heading = "Encoding")]
    pub video_params: Option<String>,

    /// Number of encoder passes
    ///
    /// Since aom and vpx benefit from two-pass mode even with constant quality
    /// mode (unlike other encoders in which two-pass mode is used for more
    /// accurate VBR rate control), two-pass mode is used by default for
    /// these encoders.
    ///
    /// When using aom or vpx with RT mode (--rt), one-pass mode is always used
    /// regardless of the value specified by this flag (as RT mode in aom
    /// and vpx only supports one-pass encoding).
    #[clap(short, long, value_parser = value_parser!(u8).range(1..=2), help_heading = "Encoding")]
    pub passes: Option<u8>,

    /// Estimate tile count from source
    ///
    /// Worker estimation will consider tile count accordingly.
    #[clap(long, help_heading = "Encoding")]
    pub tile_auto: bool,

    /// FFmpeg filter options
    #[clap(
        short = 'f',
        long = "ffmpeg",
        allow_hyphen_values = true,
        help_heading = "Encoding"
    )]
    pub ffmpeg_filter_args: Option<String>,

    /// Audio encoding parameters (ffmpeg syntax)
    ///
    /// If not specified, "-c:a copy" is used.
    ///
    /// Do not use ffmpeg's -map syntax with this option. Instead, use the colon
    /// syntax with each parameter you specify.
    ///
    /// Subtitles are always copied by default.
    ///
    /// Example to encode all audio tracks with libopus at 128k:
    ///
    /// -a="-c:a libopus -b:a 128k"
    ///
    /// Example to encode the first audio track with libopus at 128k, and the
    /// second audio track with aac at 24k, where only the second track is
    /// downmixed to a single channel:
    ///
    /// -a="-c:a:0 libopus -b:a:0 128k -c:a:1 aac -ac:a:1 1 -b:a:1 24k"
    #[clap(short, long, allow_hyphen_values = true, help_heading = "Encoding")]
    pub audio_params: Option<String>,

    /// Ignore any detected mismatch between scene frame count and encoder frame
    /// count
    #[clap(long, help_heading = "Encoding")]
    pub ignore_frame_mismatch: bool,

    /// Method used for piping exact ranges of frames to the encoder
    ///
    /// Methods that require an external vapoursynth plugin:
    ///
    /// lsmash - Generally the best and most accurate method. Does not require
    /// intermediate files. Errors generally only occur if the input file
    /// itself is broken (for example, if the video bitstream is invalid in some
    /// way, video players usually try to recover from the errors as much as
    /// possible even if it results in visible artifacts, while lsmash will
    /// instead throw an error). Requires the lsmashsource vapoursynth
    /// plugin to be installed.
    ///
    /// ffms2 - Accurate and does not require intermediate files. Can sometimes
    /// have bizarre bugs that are not present in lsmash (that can
    /// cause artifacts in the piped output). Slightly faster than lsmash for
    /// y4m input. Requires the ffms2 vapoursynth plugin to be installed.
    ///
    /// dgdecnv - Very fast, but only decodes AVC, HEVC, MPEG-2, and VC1. Does
    /// not require intermediate files. Requires dgindexnv to be present in
    /// system path, NVIDIA GPU that support CUDA video decoding, and dgdecnv
    /// vapoursynth plugin to be installed.
    ///
    /// bestsource - Very slow but accurate. Linearly decodes input files, very
    /// slow. Does not require intermediate files, requires the BestSource
    /// vapoursynth plugin to be installed.
    ///
    /// Methods that only require ffmpeg:
    ///
    /// hybrid - Uses a combination of segment and select. Usually accurate but
    /// requires intermediate files (which can be large). Avoids
    /// decoding irrelevant frames by seeking to the first keyframe before the
    /// requested frame and decoding only a (usually very small)
    /// number of irrelevant frames until relevant frames are decoded and piped
    /// to the encoder.
    ///
    /// select - Extremely slow, but accurate. Does not require intermediate
    /// files. Decodes from the first frame to the requested frame,
    /// without skipping irrelevant frames (causing quadratic decoding
    /// complexity).
    ///
    /// segment - Create chunks based on keyframes in the source. Not frame
    /// exact, as it can only split on keyframes in the source.
    /// Requires intermediate files (which can be large).
    ///
    /// Default: lsmash (if available), otherwise ffms2 (if available),
    /// otherwise DGDecNV (if available), otherwise bestsource (if available),
    /// otherwise hybrid.
    #[clap(short = 'm', long, help_heading = "Encoding")]
    pub chunk_method: Option<ChunkMethod>,

    /// The order in which av1an will encode chunks
    ///
    /// Available methods:
    ///
    /// long-to-short - The longest chunks will be encoded first. This method
    /// results in the smallest amount of time with idle cores,
    /// as the encode will not be waiting on a very long chunk to finish at the
    /// end of the encode after all other chunks have finished.
    ///
    /// short-to-long - The shortest chunks will be encoded first.
    ///
    /// sequential - The chunks will be encoded in the order they appear in the
    /// video.
    ///
    /// random - The chunks will be encoded in a random order. This will provide
    /// a more accurate estimated filesize sooner in the encode.
    #[clap(long, default_value_t = ChunkOrdering::LongestFirst, help_heading = "Encoding")]
    pub chunk_order: ChunkOrdering,

    /// Generates a photon noise table and applies it using grain synthesis
    /// [strength: 0-64] (disabled by default)
    ///
    /// Photon noise tables are more visually pleasing than the film grain
    /// generated by aomenc, and provide a consistent level of grain
    /// regardless of the level of grain in the source. Strength values
    /// correlate to ISO values, e.g. 1 = ISO 100, and 64 = ISO 6400. This
    /// option currently only supports aomenc and rav1e.
    ///
    /// An encoder's grain synthesis will still work without using this option,
    /// by specifying the correct parameter to the encoder. However, the two
    /// should not be used together, and specifying this option will disable
    /// the encoder's internal grain synthesis.
    #[clap(long, help_heading = "Encoding")]
    pub photon_noise: Option<u8>,

    /// Adds chroma grain synthesis to the grain table generated by
    /// `--photon-noise`. (Default: false)
    #[clap(long, help_heading = "Encoding", requires = "photon_noise")]
    pub chroma_noise: bool,

    /// Manually set the width for the photon noise table.
    #[clap(long, help_heading = "Encoding")]
    pub photon_noise_width: Option<u32>,

    /// Manually set the height for the photon noise table.
    #[clap(long, help_heading = "Encoding")]
    pub photon_noise_height: Option<u32>,

    /// Determines method used for concatenating encoded chunks and audio into
    /// output file
    ///
    /// ffmpeg - Uses ffmpeg for concatenation. Unfortunately, ffmpeg sometimes
    /// produces files with partially broken audio seeking, so mkvmerge
    /// should generally be preferred if available. ffmpeg concatenation
    /// also produces broken files with the --enable-keyframe-filtering=2 option
    /// in aomenc, so it is disabled if that option is used. However, ffmpeg can
    /// mux into formats other than matroska (.mkv), such as WebM. To output
    /// WebM, use a .webm extension in the output file.
    ///
    /// mkvmerge - Generally the best concatenation method (as it does not have
    /// either of the aforementioned issues that ffmpeg has), but can only
    /// produce matroska (.mkv) files. Requires mkvmerge to be installed.
    ///
    /// ivf - Experimental concatenation method implemented in av1an itself to
    /// concatenate to an ivf file (which only supports VP8, VP9, and AV1,
    /// and does not support audio).
    #[clap(short, long, default_value_t = ConcatMethod::MKVMerge, help_heading = "Encoding")]
    pub concat: ConcatMethod,

    /// FFmpeg pixel format
    #[clap(long, default_value = "yuv420p10le", help_heading = "Encoding")]
    pub pix_format: Pixel,

    /// Path to a file specifying zones within the video with differing encoder
    /// settings.
    ///
    /// The zones file should include one zone per line,
    /// with each arg within a zone space-separated.
    /// No quotes or escaping are needed around the encoder args,
    /// as these are assumed to be the last argument.
    ///
    /// The zone args on each line should be in this order:
    ///
    /// ```
    /// start_frame end_frame encoder reset(opt) video_params
    /// ```
    ///
    /// For example:
    ///
    /// ```
    /// 136 169 aom --photon-noise 4 --cq-level=32
    /// 169 1330 rav1e reset -s 3 -q 42
    /// ```
    ///
    /// Example line 1 will encode frames 136-168 using aomenc
    /// with the argument `--cq-level=32` and enable av1an's `--photon-noise`
    /// option. Note that the end frame number is *exclusive*.
    /// The start and end frame will both be forced to be scenecuts.
    /// Additional scene detection will still be applied within the zones.
    /// `-1` can be used to refer to the last frame in the video.
    ///
    /// The default behavior as shown on line 1 is to preserve
    /// any options passed to `--video-params` or `--photon-noise`
    /// in av1an, and append or overwrite the additional zone settings.
    ///
    /// Example line 2 will encode frames 169-1329 using rav1e.
    /// The `reset` keyword instructs av1an to ignore any settings
    /// which affect the encoder, and use only the parameters from this zone.
    ///
    /// For segments where no zone is specified,
    /// the settings passed to av1an itself will be used.
    ///
    /// The video params which may be specified include any parameters
    /// that are allowed by the encoder, as well as the following av1an options:
    ///
    /// - `-x`/`--extra-split`
    /// - `--min-scene-len`
    /// - `--passes`
    /// - `--photon-noise` (aomenc/rav1e only)
    #[clap(long, help_heading = "Encoding", verbatim_doc_comment)]
    pub zones: Option<PathBuf>,

    /// Plot an SVG of the VMAF for the encode
    ///
    /// This option is independent of --target-quality, i.e. it can be used with
    /// or without it. The SVG plot is created in the same directory as the
    /// output file.
    #[clap(long, help_heading = "VMAF")]
    pub vmaf: bool,

    /// Path to VMAF model (used by --vmaf and --target-quality)
    ///
    /// If not specified, ffmpeg's default is used.
    #[clap(long, help_heading = "VMAF")]
    pub vmaf_path: Option<PathBuf>,

    /// Resolution used for VMAF calculation
    ///
    /// If set to inputres, the output video will be scaled to the resolution of
    /// the input video.
    #[clap(long, default_value = "1920x1080", help_heading = "VMAF")]
    pub vmaf_res: String,

    /// Resolution used for Target Quality metric calculation in the form of
    /// `widthxheight` where width and height are positive integers
    ///
    /// If not specified, the output video will be scaled to the resolution of
    /// the input video.
    #[clap(long, help_heading = "Target Quality")]
    pub probe_res: Option<String>,

    /// Number of threads to use for target quality VMAF calculation
    #[clap(long, help_heading = "VMAF")]
    pub vmaf_threads: Option<usize>,

    /// Filter applied to source at VMAF calcualation
    ///
    /// This option should be specified if the source is cropped, for example.
    #[clap(long, help_heading = "VMAF")]
    pub vmaf_filter: Option<String>,

    /// Target a metric score for encoding (disabled by default)
    ///
    /// For each chunk, target quality uses an algorithm to find the
    /// quantizer/crf needed to achieve a certain metric score.
    /// Target quality mode is much slower than normal encoding, but can improve
    /// the consistency of quality in some cases.
    ///
    /// The VMAF and SSIMULACRA2 score ranges are 0-100 (where 0 is the worst
    /// quality, and 100 is the best).
    ///
    /// The butteraugli score minimum is 0 as the best quality and increases as
    /// quality decreases towards infinity.
    ///
    /// The XPSNR score minimum is 0 as the worst quality and increases as
    /// quality increases towards infinity.
    ///
    /// Floating-point values are allowed for all metrics.
    #[clap(long, help_heading = "Target Quality")]
    pub target_quality: Option<f64>,
    /// The metric used for Target Quality mode
    ///
    /// vmaf - Requires FFmpeg with VMAF enabled.
    ///
    /// ssimulacra2 - Requires Vapoursynth-HIP or VapourSynth-Zig Image Process
    /// plugin. Also requires Chunk method to be set to "lsmash", "ffms2",
    /// "bestsource", or "dgdecnv".
    ///
    /// butteraugli-inf - Uses the Infinite-Norm value of butteraugli with a
    /// target intensity of 203 nits. Requires Vapoursynth-HIP or Julek
    /// plugin. Also requires Chunk method to be set to "lsmash", "ffms2",
    /// "bestsource", or "dgdecnv".
    ///
    /// butteraugli-3  - Uses the 3-Norm value of butteraugli with a target
    /// intensity of 203 nits. Requires Vapoursynth-HIP plugin. Also
    /// requires Chunk method to be set to "lsmash", "ffms2", "bestsource",
    /// or "dgdecnv".
    ///
    /// xpsnr -  Uses the minimum of Y, U, and V. Requires FFmpeg with XPSNR
    /// enabled when Probing Rate is unspecified or set to 1. When Probing Rate
    /// is specified higher than 1, the VapourSynth-Zig Image Process plugin
    /// version R7 or newer is required and the Chunk method must be set to
    /// "lsmash", "ffms2", "bestsource", or "dgdecnv".
    ///
    /// xpsnr-weighted - Uses weighted XPSNR based on this formula: `((4 * Y) +
    /// U + V) / 6`. Requires FFmpeg with XPSNR enabled when Probing Rate is
    /// unspecified or set to 1. When Probing Rate is specified higher than 1,
    /// the VapourSynth-Zig Image Process plugin version R7 or newer is required
    /// and the Chunk method must be set to "lsmash", "ffms2", "bestsource", or
    /// "dgdecnv".
    #[clap(long, default_value_t = TargetMetric::VMAF, help_heading = "Target Quality")]
    pub target_metric:  TargetMetric,
    /// Maximum number of probes allowed for target quality
    #[clap(long, default_value_t = 4, help_heading = "Target Quality")]
    pub probes:         u32,

    /// Only use every nth frame for VMAF calculation, while probing.
    ///
    /// WARNING: The resulting VMAF score might differ from if all the frames
    /// were used; usually it should be lower, which means to get the same
    /// quality you must also usually lower the --target-quality. Going higher
    /// than n=4 usually results in unusable scores, so this is disabled.
    #[clap(long, default_value_t = 1, value_parser = clap::value_parser!(u16).range(1..=4), help_heading = "Target Quality")]
    pub probing_rate: u16,

    /// Speed for probes. Lower speed for higher quality and accuracy
    ///
    /// Speeds:
    ///   veryfast
    ///   fast
    ///   medium
    ///   slow
    ///   veryslow
    ///
    /// If specified with `--probe-slow`, overrides the respective speed
    /// parameter (eg. "--cpu-used=", "--preset", etc.). Otherwise defaults to
    /// "veryfast".
    #[clap(long, help_heading = "Target Quality", ignore_case = true)]
    pub probing_speed: Option<ProbingSpeed>,

    /// Use encoding settings for probes specified by --video-params rather than
    /// faster, less accurate settings
    ///
    /// If it is specified, Probing Speed will override the respective
    /// speed parameter (eg. "--cpu-used=", "--preset", etc.)
    ///
    /// Note that this always performs encoding in one-pass mode, regardless of
    /// --passes.
    #[clap(long, help_heading = "Target Quality")]
    pub probe_slow: bool,

    /// Lower bound for target quality Q-search early exit
    ///
    /// If min_q is tested and the probe's VMAF score is lower than
    /// target_quality, the Q-search early exits and min_q is used for the
    /// chunk.
    ///
    /// If not specified, the default value is used (chosen per encoder).
    #[clap(long, help_heading = "Target Quality")]
    pub min_q: Option<u32>,

    /// Upper bound for target quality Q-search early exit
    ///
    /// If max_q is tested and the probe's VMAF score is higher than
    /// target_quality, the Q-search early exits and max_q is used for the
    /// chunk.
    ///
    /// If not specified, the default value is used (chosen per encoder).
    #[clap(long, help_heading = "Target Quality")]
    pub max_q: Option<u32>,

    #[rustfmt::skip]
    /// VMAF calculation features for target quality probing
    ///
    /// Available features:
    ///   default     - Standard VMAF calculation (baseline)
    ///   weighted    - Perceptual weighting using (4Y+1U+1V)/6 formula
    ///   neg         - No Enhancement Gain: isolates compression artifacts by subtracting enhancement gains (e.g., sharpening)
    ///   motionless  - Disable motion compensation (prevents score inflation)
    ///   uhd         - Use 4K optimized VMAF model: Overrides 'DEFAULT' but can be combined with 'NEG'.
    ///
    /// Multiple features can be combined:
    ///   --probing-vmaf-features weighted neg motionless
    ///   --probing-vmaf-features default motionless
    ///
    /// 'NEG' overrides 'DEFAULT' because it is a different model.
    ///
    #[clap(long, num_args = 0.., value_enum, help_heading = "Target Quality", verbatim_doc_comment)]
    pub probing_vmaf_features: Vec<VmafFeature>,
    #[rustfmt::skip]
    /// Statistical method for calculating target quality from sorted probe
    /// scores
    ///
    /// Available methods:
    ///   auto                       - Automatically choose the best method based on the target metric, the probing speed, and the quantizer
    ///   mean                       - Arithmetic mean (average)
    ///   median                     - Middle value
    ///   harmonic                   - Harmonic mean (emphasizes lower quality scores)
    ///   percentile=<FLOAT>         - Percentile of a specified value. Must be between 0.0 and 100.0
    ///   standard-deviation=<FLOAT> - Standard deviation distance from mean (σ) clamped by the minimum and maximum probe scores
    ///   mode                       - Most common integer-rounded value
    ///   minimum                    - Lowest quality value
    ///   maximum                    - Highest quality value
    ///   root-mean-square           - Root Mean Square (quadratic mean)
    ///
    /// Warning:
    ///   "root-mean-square" should only be used with inverse target metrics such as "butteraugli".
    ///   "harmonic" works as expected when there are no negative scores. Use with caution with target metrics such as "ssimulacra2".
    #[clap(long, default_value_t = String::from("auto"), help_heading = "Target Quality", verbatim_doc_comment)]
    pub probing_stat: String,
}

impl CliOpts {
    #[tracing::instrument(level = "debug")]
    pub fn target_quality_params(
        &self,
        temp_dir: String,
        video_params: Vec<String>,
        output_pix_format: Pixel,
    ) -> anyhow::Result<Option<TargetQuality>> {
        self.target_quality
            .map(|tq| {
                let (min, max) = self.encoder.get_default_cq_range();
                let min_q = self.min_q.unwrap_or(min as u32);
                let max_q = self.max_q.unwrap_or(max as u32);

                let probing_statistic = match self.probing_stat.to_lowercase().as_str() {
                    "auto" => ProbingStatistic {
                        name:  ProbingStatisticName::Automatic,
                        value: None,
                    },
                    "mean" => ProbingStatistic {
                        name:  ProbingStatisticName::Mean,
                        value: None,
                    },
                    "harmonic" => ProbingStatistic {
                        name:  ProbingStatisticName::Harmonic,
                        value: None,
                    },
                    "root-mean-square" => ProbingStatistic {
                        name:  ProbingStatisticName::RootMeanSquare,
                        value: None,
                    },
                    "median" => ProbingStatistic {
                        name:  ProbingStatisticName::Median,
                        value: None,
                    },
                    "mode" => ProbingStatistic {
                        name:  ProbingStatisticName::Mode,
                        value: None,
                    },
                    "minimum" => ProbingStatistic {
                        name:  ProbingStatisticName::Minimum,
                        value: None,
                    },
                    "maximum" => ProbingStatistic {
                        name:  ProbingStatisticName::Maximum,
                        value: None,
                    },
                    probe_statistic if probe_statistic.starts_with("percentile") => {
                        if probe_statistic.matches('=').count() != 1
                            || !probe_statistic.starts_with("percentile=")
                        {
                            return Err(anyhow!(
                                "Probing Statistic percentile must have a value between 0.0 and \
                                 100.0 set using \"=\" (eg. \"--probing-stat percentile=1\")"
                            ));
                        }
                        let value = probe_statistic
                            .split("=")
                            .last()
                            .and_then(|s| s.parse::<f64>().ok())
                            .and_then(|v| {
                                if (0.0..=100.0).contains(&v) {
                                    Some(v)
                                } else {
                                    None
                                }
                            })
                            .ok_or_else(|| {
                                anyhow!(
                                    "Probing Statistic percentile must be set to a value between \
                                     0 and 100"
                                )
                            })?;
                        ProbingStatistic {
                            name:  ProbingStatisticName::Percentile,
                            value: Some(value),
                        }
                    },
                    probe_statistic if probe_statistic.starts_with("standard-deviation") => {
                        if probe_statistic.matches('=').count() != 1
                            || !probe_statistic.starts_with("standard-deviation=")
                        {
                            return Err(anyhow!(
                                "Probing Statistic standard deviation must have a positive or \
                                 negative value set using \"=\" (eg. \"--probing-stat \
                                 standard-deviation=-0.25\")"
                            ));
                        }
                        let value = probe_statistic
                            .split('=')
                            .next_back()
                            .and_then(|s| s.parse::<f64>().ok())
                            .ok_or_else(|| {
                                anyhow!(
                                    "Probing Statistic standard deviation must have a value \
                                     appended"
                                )
                            })?;
                        ProbingStatistic {
                            name:  ProbingStatisticName::StandardDeviation,
                            value: Some(value),
                        }
                    },
                    _ => {
                        return Err(anyhow!("Unknown Probing Statistic: {}", self.probing_stat));
                    },
                };

                Ok(TargetQuality {
                    vmaf_res: self.vmaf_res.clone(),
                    probe_res: self.probe_res.clone(),
                    vmaf_scaler: self.scaler.clone(),
                    vmaf_filter: self.vmaf_filter.clone(),
                    vmaf_threads: self.vmaf_threads.unwrap_or_else(|| {
                        available_parallelism()
                            .expect("Unrecoverable: Failed to get thread count")
                            .get()
                    }),
                    model: self.vmaf_path.clone(),
                    probes: self.probes,
                    target: tq,
                    metric: self.target_metric,
                    min_q,
                    max_q,
                    encoder: self.encoder,
                    pix_format: output_pix_format,
                    temp: temp_dir.clone(),
                    workers: self.workers,
                    video_params: video_params.clone(),
                    vspipe_args: self.vspipe_args.clone(),
                    probe_slow: self.probe_slow,
                    probing_speed: self.probing_speed,
                    probing_rate: self.probing_rate as usize,
                    probing_vmaf_features: if self.probing_vmaf_features.is_empty() {
                        vec![VmafFeature::Default]
                    } else {
                        self.probing_vmaf_features.clone()
                    },
                    probing_statistic,
                })
            })
            .transpose()
    }
}

fn confirm(prompt: &str) -> io::Result<bool> {
    let mut buf = String::with_capacity(4);
    let mut stdout = io::stdout();
    let stdin = io::stdin();
    loop {
        stdout.write_all(prompt.as_bytes())?;
        stdout.flush()?;
        stdin.read_line(&mut buf)?;

        match buf.as_str().trim() {
            // allows enter to continue
            "y" | "Y" => break Ok(true),
            "n" | "N" | "" => break Ok(false),
            other => {
                println!("Sorry, response {other:?} is not understood.");
                buf.clear();
                continue;
            },
        }
    }
}

/// Given Folder and File path as inputs
/// Converts them all to file paths
/// Converting only depth 1 of Folder paths
pub(crate) fn resolve_file_paths(path: &Path) -> anyhow::Result<Box<dyn Iterator<Item = PathBuf>>> {
    // TODO: to validate file extensions
    // let valid_media_extensions = ["mkv", "mov", "mp4", "webm", "avi", "qt", "ts",
    // "m2t", "py", "vpy"];

    ensure!(
        path.exists(),
        "Input path {:?} does not exist. Please ensure you typed it properly and it has not been \
         moved.",
        path
    );

    if path.is_dir() {
        Ok(Box::new(read_in_dir(path)?))
    } else {
        Ok(Box::new(std::iter::once(path.to_path_buf())))
    }
}

/// Returns vector of Encode args ready to be fed to encoder
#[tracing::instrument(level = "debug")]
pub fn parse_cli(args: CliOpts) -> anyhow::Result<Vec<EncodeArgs>> {
    let input_paths = &*args.input;

    let mut inputs = Vec::new();
    for path in input_paths {
        inputs.extend(resolve_file_paths(path)?);
    }

    let mut valid_args: Vec<EncodeArgs> = Vec::with_capacity(inputs.len());

    for input in inputs {
        let temp = if let Some(path) = args.temp.as_ref() {
            path.to_str().unwrap().to_owned()
        } else {
            format!(".{}", hash_path(input.as_path()))
        };

        let chunk_method =
            args.chunk_method.unwrap_or_else(vapoursynth::best_available_chunk_method);
        let scaler = {
            let mut scaler = args.scaler.to_string().clone();
            let mut scaler_ext =
                "+accurate_rnd+full_chroma_int+full_chroma_inp+bitexact".to_string();
            if scaler.starts_with("lanczos") {
                for n in 1..=9 {
                    if scaler.ends_with(&n.to_string()) {
                        scaler_ext.push_str(&format!(":param0={}", &n.to_string()));
                        scaler = "lanczos".to_string();
                    }
                }
            }
            scaler.push_str(&scaler_ext);
            scaler
        };

        let input = Input::new(
            input,
            args.vspipe_args.clone(),
            temp.as_str(),
            chunk_method,
            args.sc_downscale_height,
            args.sc_pix_format,
            scaler.clone(),
        )?;

        let verbosity = if args.quiet {
            Verbosity::Quiet
        } else if args.verbose {
            Verbosity::Verbose
        } else {
            Verbosity::Normal
        };

        let video_params = if let Some(args) = args.video_params.as_ref() {
            shlex::split(args).ok_or_else(|| anyhow!("Failed to split video encoder arguments"))?
        } else {
            Vec::new()
        };
        let output_pix_format = PixelFormat {
            format:    args.pix_format,
            bit_depth: args.encoder.get_format_bit_depth(args.pix_format)?,
        };

        let target_quality = args.target_quality_params(
            temp.clone(),
            video_params.clone(),
            output_pix_format.format,
        )?;

        // TODO make an actual constructor for this
        let arg = EncodeArgs {
            log_file: if let Some(log_file) = args.log_file.as_ref() {
                let log_path = Path::new(log_file);
                if log_path.starts_with("/") || log_path.is_absolute() {
                    Err(anyhow!("Log file path must be relative"))?
                }
                let absolute_path = std::path::absolute(log_path).unwrap();
                let log_path = absolute_path
                    .strip_prefix(std::env::current_dir().unwrap())
                    .unwrap()
                    .strip_prefix("logs")
                    .unwrap_or(
                        absolute_path.strip_prefix(std::env::current_dir().unwrap()).unwrap(),
                    );
                log_path.to_path_buf()
            } else {
                Path::new("av1an.log").to_owned()
            },
            log_level: args.log_level,
            ffmpeg_filter_args: if let Some(args) = args.ffmpeg_filter_args.as_ref() {
                shlex::split(args)
                    .ok_or_else(|| anyhow!("Failed to split ffmpeg filter arguments"))?
            } else {
                Vec::new()
            },
            temp: temp.clone(),
            force: args.force,
            no_defaults: args.no_defaults,
            passes: if let Some(passes) = args.passes {
                passes
            } else {
                args.encoder.get_default_pass()
            },
            video_params: video_params.clone(),
            output_file: if let Some(path) = args.output_file.as_ref() {
                let path = PathAbs::new(path)?;

                if let Ok(parent) = path.parent() {
                    ensure!(parent.exists(), "Path to file {:?} is invalid", path);
                } else {
                    bail!("Failed to get parent directory of path: {:?}", path);
                }

                path.to_string_lossy().to_string()
            } else {
                format!(
                    "{}_{}.mkv",
                    input
                        .as_path()
                        .file_stem()
                        .unwrap_or_else(|| input.as_path().as_ref())
                        .to_string_lossy(),
                    args.encoder
                )
            },
            audio_params: if let Some(args) = args.audio_params.as_ref() {
                shlex::split(args)
                    .ok_or_else(|| anyhow!("Failed to split ffmpeg audio encoder arguments"))?
            } else {
                into_vec!["-c:a", "copy"]
            },
            chunk_method,
            chunk_order: args.chunk_order,
            concat: args.concat,
            encoder: args.encoder,
            extra_splits_len: match args.extra_split {
                Some(0) => None,
                Some(x) => Some(x),
                // Make sure it's at least 10 seconds, unless specified by user
                None => match input.frame_rate() {
                    Ok(fps) => Some((fps.to_f64().unwrap() * args.extra_split_sec) as usize),
                    Err(_) => Some(240_usize),
                },
            },
            photon_noise: args.photon_noise.and_then(|arg| if arg == 0 { None } else { Some(arg) }),
            photon_noise_size: (args.photon_noise_width, args.photon_noise_height),
            chroma_noise: args.chroma_noise,
            sc_pix_format: args.sc_pix_format,
            keep: args.keep,
            max_tries: args.max_tries as usize,
            min_scene_len: args.min_scene_len,
            input_pix_format: {
                match &input {
                    Input::Video {
                        path, ..
                    } => InputPixelFormat::FFmpeg {
                        format: ffmpeg::get_pixel_format(path.as_ref()).with_context(|| {
                            format!("FFmpeg failed to get pixel format for input video {path:?}")
                        })?,
                    },
                    Input::VapourSynth {
                        path, ..
                    } => InputPixelFormat::VapourSynth {
                        bit_depth: crate::vapoursynth::bit_depth(
                            path.as_ref(),
                            input.as_vspipe_args_map()?,
                        )
                        .with_context(|| {
                            format!("VapourSynth failed to get bit depth for input video {path:?}")
                        })?,
                    },
                }
            },
            input,
            output_pix_format,
            resume: args.resume,
            scenes: args.scenes.clone(),
            split_method: args.split_method.clone(),
            sc_method: args.sc_method,
            sc_only: args.sc_only,
            sc_downscale_height: args.sc_downscale_height,
            force_keyframes: parse_comma_separated_numbers(
                args.force_keyframes.as_deref().unwrap_or(""),
            )?,
            target_quality,
            vmaf: args.vmaf,
            vmaf_path: args.vmaf_path.clone(),
            vmaf_res: args.vmaf_res.clone(),
            probe_res: args.probe_res.clone(),
            vmaf_threads: args.vmaf_threads,
            vmaf_filter: args.vmaf_filter.clone(),
            verbosity,
            workers: args.workers,
            tiles: (1, 1), // default value; will be adjusted if tile_auto set
            tile_auto: args.tile_auto,
            set_thread_affinity: args.set_thread_affinity,
            zones: args.zones.clone(),
            scaler,
            ignore_frame_mismatch: args.ignore_frame_mismatch,
        };

        if !args.overwrite {
            // UGLY: taking first file for output file
            if let Some(path) = args.output_file.as_ref() {
                if path.exists()
                    && (args.never_overwrite
                        || !confirm(&format!(
                            "Output file {path:?} exists. Do you want to overwrite it? [y/N]: "
                        ))?)
                {
                    println!("Not overwriting, aborting.");
                    exit(0);
                }
            } else {
                let path: &Path = arg.output_file.as_ref();

                if path.exists()
                    && (args.never_overwrite
                        || !confirm(&format!(
                            "Default output file {path:?} exists. Do you want to overwrite it? \
                             [y/N]: "
                        ))?)
                {
                    println!("Not overwriting, aborting.");
                    exit(0);
                }
            }
        }

        valid_args.push(arg)
    }

    Ok(valid_args)
}

#[instrument]
pub fn run() -> anyhow::Result<()> {
    let cli_options = CliOpts::parse();
    let args = parse_cli(cli_options)?;
    let first_arg = args.first().unwrap();

    init_logging(
        match first_arg.verbosity {
            Verbosity::Quiet => LevelFilter::WARN,
            Verbosity::Normal => LevelFilter::INFO,
            Verbosity::Verbose => LevelFilter::INFO,
        },
        first_arg.log_file.clone(),
        first_arg.log_level,
    );

    for arg in args {
        Av1anContext::new(arg)?.encode_file()?;
    }

    Ok(())
}

fn parse_comma_separated_numbers(string: &str) -> anyhow::Result<Vec<usize>> {
    let mut result = Vec::new();

    let string = string.trim();
    if string.is_empty() {
        return Ok(result);
    }

    for val in string.split(',') {
        result.push(val.trim().parse()?);
    }
    Ok(result)
}
