use std::{
    cmp::max,
    collections::{hash_map::DefaultHasher, HashMap},
    fs::{self, read_to_string, File},
    hash::{Hash, Hasher},
    io::Write,
    path::{Path, PathBuf},
    string::ToString,
    sync::atomic::{AtomicBool, AtomicUsize},
    thread::available_parallelism,
    time::Instant,
};

use ::ffmpeg::format::Pixel;
use ::vapoursynth::{api::API, map::OwnedMap};
use anyhow::{bail, Context};
use av1_grain::TransferFunction;
use av_format::rational::Rational64;
use chunk::Chunk;
use dashmap::DashMap;
use once_cell::sync::{Lazy, OnceCell};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use strum::{Display, EnumString, FromRepr, IntoStaticStr};

pub use crate::{
    concat::ConcatMethod,
    context::Av1anContext,
    encoder::Encoder,
    settings::{EncodeArgs, InputPixelFormat, PixelFormat},
    target_quality::{InterpolationMethod, TargetQuality},
    util::read_in_dir,
};
use crate::{progress_bar::finish_progress_bar, vapoursynth::generate_loadscript_text};

mod broker;
mod chunk;
mod concat;
mod context;
mod encoder;
pub mod ffmpeg;
mod metrics {
    pub mod butteraugli;
    pub mod statistics;
    pub mod vmaf;
    pub mod xpsnr;
}
mod interpol;
mod parse;
mod progress_bar;
mod scene_detect;
mod scenes;
mod settings;
mod split;
mod target_quality;
mod util;
pub mod vapoursynth;
mod zones;

static CLIP_INFO_CACHE: Lazy<Mutex<HashMap<CacheKey, ClipInfo>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct CacheKey {
    input:          Input,
    vs_script_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub enum Input {
    VapourSynth {
        path:        PathBuf,
        vspipe_args: Vec<String>,
        script_text: String,
    },
    Video {
        path:        PathBuf,
        script_text: Option<String>,
    },
}

impl Input {
    #[inline]
    pub fn new<P: AsRef<Path> + Into<PathBuf>>(
        path: P,
        vspipe_args: Vec<String>,
        temp: &str,
        chunk_method: ChunkMethod,
        scene_detection_downscale_height: Option<usize>,
        scene_detection_pixel_format: Option<Pixel>,
        scene_detection_scaler: String,
    ) -> anyhow::Result<Self> {
        if let Some(ext) = path.as_ref().extension() {
            if ext == "py" || ext == "vpy" {
                let input_path = path.into();
                let script_text = read_to_string(input_path.clone())?;
                Ok(Self::VapourSynth {
                    path: input_path.clone(),
                    vspipe_args,
                    script_text,
                })
            } else {
                let input_path = path.into();
                Ok(Self::Video {
                    path:        input_path.clone(),
                    script_text: match chunk_method {
                        ChunkMethod::LSMASH
                        | ChunkMethod::FFMS2
                        | ChunkMethod::DGDECNV
                        | ChunkMethod::BESTSOURCE => Some(
                            generate_loadscript_text(
                                temp,
                                &input_path,
                                chunk_method,
                                scene_detection_downscale_height,
                                scene_detection_pixel_format,
                                scene_detection_scaler,
                            )
                            .unwrap(),
                        ),
                        _ => None,
                    },
                })
            }
        } else {
            let input_path = path.into();
            Ok(Self::Video {
                path:        input_path.clone(),
                script_text: match chunk_method {
                    ChunkMethod::LSMASH
                    | ChunkMethod::FFMS2
                    | ChunkMethod::DGDECNV
                    | ChunkMethod::BESTSOURCE => Some(
                        generate_loadscript_text(
                            temp,
                            &input_path,
                            chunk_method,
                            scene_detection_downscale_height,
                            scene_detection_pixel_format,
                            scene_detection_scaler,
                        )
                        .unwrap(),
                    ),
                    _ => None,
                },
            })
        }
    }

    /// Returns a reference to the inner path, panicking if the input is not an
    /// `Input::Video`.
    #[inline]
    pub fn as_video_path(&self) -> &Path {
        match &self {
            Input::Video {
                path, ..
            } => path.as_ref(),
            Input::VapourSynth {
                ..
            } => {
                panic!("called `Input::as_video_path()` on an `Input::VapourSynth` variant")
            },
        }
    }

    /// Returns a reference to the inner path, panicking if the input is not an
    /// `Input::VapourSynth`.
    #[inline]
    pub fn as_vapoursynth_path(&self) -> &Path {
        match &self {
            Input::VapourSynth {
                path, ..
            } => path.as_ref(),
            Input::Video {
                ..
            } => {
                panic!("called `Input::as_vapoursynth_path()` on an `Input::Video` variant")
            },
        }
    }

    /// Returns a reference to the inner path regardless of whether `self` is
    /// `Video` or `VapourSynth`.
    ///
    /// The caller must ensure that the input type is being properly handled.
    /// This method should not be used unless the code is TRULY agnostic of the
    /// input type!
    #[inline]
    pub fn as_path(&self) -> &Path {
        match &self {
            Input::Video {
                path, ..
            }
            | Input::VapourSynth {
                path, ..
            } => path.as_ref(),
        }
    }

    /// Returns a reference to the inner script text, panicking if the input is
    /// does not have script text.
    #[inline]
    pub fn as_script_text(&self) -> &String {
        match &self {
            Input::VapourSynth {
                script_text, ..
            } => script_text,
            Input::Video {
                script_text, ..
            } => {
                if let Some(text) = script_text {
                    text
                } else {
                    panic!(
                        "called `Input::as_script_text()` on an `Input::Video` variant with no \
                         script text"
                    )
                }
            },
        }
    }

    #[inline]
    pub const fn is_video(&self) -> bool {
        matches!(&self, Input::Video { .. })
    }

    #[inline]
    pub const fn is_vapoursynth(&self) -> bool {
        matches!(&self, Input::VapourSynth { .. })
    }

    #[inline]
    pub fn clip_info(&self, vs_script_path: Option<PathBuf>) -> anyhow::Result<ClipInfo> {
        const FAIL_MSG: &str = "Failed to get number of frames for input video";

        let mut cache = CLIP_INFO_CACHE.lock();
        let key = CacheKey {
            input:          self.clone(),
            vs_script_path: vs_script_path.clone(),
        };
        let cached = cache.get(&key);
        if let Some(cached) = cached {
            return Ok(*cached);
        }

        let info = match &self {
            Input::Video {
                path, ..
            } if vs_script_path.is_none() => {
                ffmpeg::get_clip_info(path.as_path()).context(FAIL_MSG)?
            },
            path => vapoursynth::get_clip_info(
                vs_script_path.as_deref().unwrap_or(path.as_path()),
                self.as_vspipe_args_map()?,
            )
            .context(FAIL_MSG)?,
        };
        cache.insert(key, info);
        Ok(info)
    }

    /// Calculates tiles from resolution
    /// Don't convert tiles to encoder specific representation
    /// Default video without tiling is 1,1
    /// Return number of horizontal and vertical tiles
    #[inline]
    pub fn calculate_tiles(&self) -> (u32, u32) {
        match self.clip_info(None).map(|info| info.resolution) {
            Ok((h, v)) => {
                // tile range 0-1440 pixels
                let horizontal = max((h - 1) / 720, 1);
                let vertical = max((v - 1) / 720, 1);

                (horizontal, vertical)
            },
            _ => (1, 1),
        }
    }

    /// Returns the vector of arguments passed to the vspipe python environment
    /// If the input is not a vapoursynth script, the vector will be empty.
    #[inline]
    pub fn as_vspipe_args_vec(&self) -> Result<Vec<String>, anyhow::Error> {
        match self {
            Input::VapourSynth {
                vspipe_args, ..
            } => Ok(vspipe_args.to_owned()),
            Input::Video {
                ..
            } => Ok(vec![]),
        }
    }

    /// Creates and returns an OwnedMap of the arguments passed to the vspipe
    /// python environment If the input is not a vapoursynth script, the map
    /// will be empty.
    #[inline]
    pub fn as_vspipe_args_map(&self) -> Result<OwnedMap<'static>, anyhow::Error> {
        let mut args_map = OwnedMap::new(API::get().unwrap());

        for arg in self.as_vspipe_args_vec()? {
            let split: Vec<&str> = arg.split_terminator('=').collect();
            if args_map.set_data(split[0], split[1].as_bytes()).is_err() {
                bail!("Failed to split vspipe arguments");
            };
        }

        Ok(args_map)
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy)]
struct DoneChunk {
    frames:     usize,
    size_bytes: u64,
}

/// Concurrent data structure for keeping track of the finished chunks in an
/// encode
#[derive(Debug, Deserialize, Serialize)]
struct DoneJson {
    frames:     AtomicUsize,
    done:       DashMap<String, DoneChunk>,
    audio_done: AtomicBool,
}

static DONE_JSON: OnceCell<DoneJson> = OnceCell::new();

// once_cell::sync::Lazy cannot be used here due to Lazy<T> not implementing
// Serialize or Deserialize, we need to get a reference directly to the global
// data
fn get_done() -> &'static DoneJson {
    DONE_JSON.get().unwrap()
}

fn init_done(done: DoneJson) -> &'static DoneJson {
    DONE_JSON.get_or_init(|| done)
}

#[inline]
pub fn list_index(params: &[impl AsRef<str>], is_match: fn(&str) -> bool) -> Option<usize> {
    assert!(!params.is_empty(), "received empty list of parameters");

    params.iter().enumerate().find_map(|(idx, s)| {
        if is_match(s.as_ref()) {
            Some(idx)
        } else {
            None
        }
    })
}

#[derive(Serialize, Deserialize, Debug, EnumString, IntoStaticStr, Display, Clone)]
pub enum SplitMethod {
    #[strum(serialize = "av-scenechange")]
    AvScenechange,
    #[strum(serialize = "none")]
    None,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, EnumString, IntoStaticStr, Display)]
pub enum ScenecutMethod {
    #[strum(serialize = "fast")]
    Fast,
    #[strum(serialize = "standard")]
    Standard,
}

#[derive(PartialEq, Eq, Copy, Clone, Serialize, Deserialize, Debug, EnumString, IntoStaticStr)]
pub enum ChunkMethod {
    #[strum(serialize = "select")]
    Select,
    #[strum(serialize = "hybrid")]
    Hybrid,
    #[strum(serialize = "segment")]
    Segment,
    #[strum(serialize = "ffms2")]
    FFMS2,
    #[strum(serialize = "lsmash")]
    LSMASH,
    #[strum(serialize = "dgdecnv")]
    DGDECNV,
    #[strum(serialize = "bestsource")]
    BESTSOURCE,
}

#[derive(
    PartialEq, Eq, Copy, Clone, Serialize, Deserialize, Debug, Display, EnumString, IntoStaticStr,
)]
pub enum ChunkOrdering {
    #[strum(serialize = "long-to-short")]
    LongestFirst,
    #[strum(serialize = "short-to-long")]
    ShortestFirst,
    #[strum(serialize = "sequential")]
    Sequential,
    #[strum(serialize = "random")]
    Random,
}

#[derive(
    PartialEq,
    Eq,
    Copy,
    Clone,
    Serialize,
    Deserialize,
    Debug,
    Display,
    EnumString,
    IntoStaticStr,
    Hash,
)]
pub enum VmafFeature {
    #[strum(serialize = "default")]
    Default,
    #[strum(serialize = "weighted")]
    Weighted,
    #[strum(serialize = "neg")]
    Neg,
    #[strum(serialize = "motionless")]
    Motionless,
    #[strum(serialize = "uhd")]
    Uhd,
}

#[derive(
    PartialEq, Eq, Copy, Clone, Serialize, Deserialize, Debug, Display, EnumString, IntoStaticStr,
)]
pub enum TargetMetric {
    #[strum(serialize = "vmaf")]
    VMAF,
    #[strum(serialize = "ssimulacra2")]
    SSIMULACRA2,
    #[strum(serialize = "butteraugli-inf")]
    ButteraugliINF,
    #[strum(serialize = "butteraugli-3")]
    Butteraugli3,
    #[strum(serialize = "xpsnr")]
    XPSNR,
    #[strum(serialize = "xpsnr-weighted")]
    XPSNRWeighted,
}

/// Determine the optimal number of workers for an encoder
#[inline]
pub fn determine_workers(args: &EncodeArgs) -> anyhow::Result<u64> {
    let res = args.input.clip_info(None)?.resolution;
    let tiles = args.tiles;
    let megapixels = (res.0 * res.1) as f64 / 1e6;
    // encoder memory and chunk_method memory usage scales with resolution
    // (megapixels), approximately linearly. Expressed as GB/Megapixel
    let cm_ram = match args.chunk_method {
        ChunkMethod::FFMS2 | ChunkMethod::LSMASH | ChunkMethod::BESTSOURCE => 0.3,
        ChunkMethod::DGDECNV => 0.3,
        ChunkMethod::Hybrid | ChunkMethod::Select | ChunkMethod::Segment => 0.1,
    };
    let enc_ram = match args.encoder {
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
    let cpu_threads = match args.encoder {
        Encoder::aom => 4,
        Encoder::rav1e => ((tiles.0 * tiles.1) as f32 * 0.7).ceil() as u64,
        Encoder::svt_av1 => 6,
        Encoder::vpx => 3,
        Encoder::x264 | Encoder::x265 => 8,
    };
    // memory usage scales with pixel format, expressed as a multiplier of memory
    // usage. Roughly the same behavior was observed accross all encoders.
    let pix_mult = match args.output_pix_format.format {
        Pixel::YUV444P | Pixel::YUV444P10LE | Pixel::YUV444P12LE => 1.5,
        Pixel::YUV422P | Pixel::YUV422P10LE | Pixel::YUV422P12LE => 1.25,
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

#[inline]
pub fn hash_path(path: &Path) -> String {
    let mut s = DefaultHasher::new();
    path.hash(&mut s);
    format!("{:x}", s.finish())[..7].to_string()
}

fn save_chunk_queue(temp: &str, chunk_queue: &[Chunk]) -> anyhow::Result<()> {
    let mut file = File::create(Path::new(temp).join("chunks.json"))
        .with_context(|| "Failed to create chunks.json file")?;

    file
    // serializing chunk_queue as json should never fail, so unwrap is OK here
    .write_all(serde_json::to_string(&chunk_queue).unwrap().as_bytes())
    .with_context(|| format!("Failed to write serialized chunk_queue data to {:?}", &file))?;

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verbosity {
    Verbose,
    Normal,
    Quiet,
}

fn read_chunk_queue(temp: &Path) -> anyhow::Result<Vec<Chunk>> {
    let file = Path::new(temp).join("chunks.json");

    let contents = fs::read_to_string(&file)
        .with_context(|| format!("Failed to read chunk queue file {:?}", &file))?;

    Ok(serde_json::from_str(&contents)?)
}

#[derive(
    Serialize, Deserialize, Debug, EnumString, IntoStaticStr, Display, Clone, Copy, FromRepr,
)]
pub enum ProbingSpeed {
    #[strum(serialize = "veryslow")]
    VerySlow = 0,
    #[strum(serialize = "slow")]
    Slow = 1,
    #[strum(serialize = "medium")]
    Medium = 2,
    #[strum(serialize = "fast")]
    Fast = 3,
    #[strum(serialize = "veryfast")]
    VeryFast = 4,
}

#[derive(Serialize, Deserialize, Debug, EnumString, IntoStaticStr, Display, Clone)]
pub enum ProbingStatisticName {
    #[strum(serialize = "mean")]
    Mean = 0,
    #[strum(serialize = "median")]
    Median = 1,
    #[strum(serialize = "harmonic")]
    Harmonic = 2,
    #[strum(serialize = "percentile")]
    Percentile = 3,
    #[strum(serialize = "standard-deviation")]
    StandardDeviation = 4,
    #[strum(serialize = "mode")]
    Mode = 5,
    #[strum(serialize = "minimum")]
    Minimum = 6,
    #[strum(serialize = "maximum")]
    Maximum = 7,
    #[strum(serialize = "root-mean-square")]
    RootMeanSquare = 8,
    #[strum(serialize = "auto")]
    Automatic = 9,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ProbingStatistic {
    pub name:  ProbingStatisticName,
    pub value: Option<f64>,
}

#[derive(Debug, Clone, Copy)]
pub struct ClipInfo {
    pub num_frames:               usize,
    pub format_info:              InputPixelFormat,
    pub frame_rate:               Rational64,
    pub resolution:               (u32, u32),
    /// This is overly simplified because we currently only use it for photon
    /// noise gen, which only supports two transfer functions
    pub transfer_characteristics: TransferFunction,
}

impl ClipInfo {
    #[inline]
    pub fn transfer_function_params_adjusted(&self, enc_params: &[String]) -> TransferFunction {
        if enc_params.iter().any(|p| {
            let p = p.to_ascii_lowercase();
            p == "pq" || p.ends_with("=pq") || p.ends_with("smpte2084")
        }) {
            return TransferFunction::SMPTE2084;
        }
        if enc_params.iter().any(|p| {
            let p = p.to_ascii_lowercase();
            // If the user specified an SDR transfer characteristic, assume they want to
            // encode to SDR.
            p.ends_with("bt709")
                || p.ends_with("bt.709")
                || p.ends_with("bt601")
                || p.ends_with("bt.601")
                || p.contains("smpte240")
                || p.contains("smpte170")
        }) {
            return TransferFunction::BT1886;
        }
        self.transfer_characteristics
    }
}
