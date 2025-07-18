use std::{
    borrow::{Borrow, Cow},
    cmp::Ordering,
    collections::HashSet,
    path::{Path, PathBuf},
    process::{exit, Command},
};

use anyhow::bail;
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::{
    concat::ConcatMethod,
    encoder::Encoder,
    ffmpeg::FFPixelFormat,
    parse::valid_params,
    target_quality::TargetQuality,
    ChunkMethod,
    ChunkOrdering,
    Input,
    ScenecutMethod,
    SplitMethod,
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PixelFormat {
    pub format:    FFPixelFormat,
    pub bit_depth: usize,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum InputPixelFormat {
    VapourSynth { bit_depth: usize },
    FFmpeg { format: FFPixelFormat },
}

impl InputPixelFormat {
    #[inline]
    pub fn as_bit_depth(&self) -> anyhow::Result<usize> {
        match self {
            InputPixelFormat::VapourSynth {
                bit_depth,
            } => Ok(*bit_depth),
            InputPixelFormat::FFmpeg {
                ..
            } => Err(anyhow::anyhow!("failed to get bit depth; wrong input type")),
        }
    }

    #[inline]
    pub fn as_pixel_format(&self) -> anyhow::Result<FFPixelFormat> {
        match self {
            InputPixelFormat::VapourSynth {
                ..
            } => Err(anyhow::anyhow!("failed to get bit depth; wrong input type")),
            InputPixelFormat::FFmpeg {
                format,
            } => Ok(*format),
        }
    }
}

#[derive(Debug, Clone)]
pub struct InputOutputSettings {
    pub input:       Input,
    pub proxy:       Option<Input>,
    pub temp:        String,
    pub output_file: String,

    pub input_pix_format:  InputPixelFormat,
    pub output_pix_format: PixelFormat,
}

#[derive(Debug, Clone)]
pub struct Av1anSettings {
    pub resume:                bool,
    pub keep:                  bool,
    pub force:                 bool,
    pub ignore_frame_mismatch: bool,
    pub max_tries:             usize,
    pub sc_only:               bool,

    pub workers:             usize,
    pub set_thread_affinity: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct EncoderSettings {
    pub encoder:      Encoder,
    pub passes:       u8,
    pub video_params: Vec<String>,
    // tile (cols, rows) count; log2 will be applied later for specific encoders.
    // `None` implies automatically select optimal tile count.
    pub tiles:        Option<(u32, u32)>,

    pub photon_noise:      Option<u8>,
    // Width and Height
    pub photon_noise_size: (Option<u32>, Option<u32>),
    pub chroma_noise:      bool,

    pub zones: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct ScenecutSettings {
    pub scenes:              Option<PathBuf>,
    pub split_method:        SplitMethod,
    pub sc_pix_format:       Option<FFPixelFormat>,
    pub sc_method:           ScenecutMethod,
    pub sc_downscale_height: Option<usize>,
    pub scaler:              String,
    pub extra_splits_len:    Option<usize>,
    pub min_scene_len:       usize,
    pub force_keyframes:     Vec<usize>,
}

#[derive(Debug, Clone)]
pub struct ChunkSettings {
    pub chunk_method: ChunkMethod,
    pub chunk_order:  ChunkOrdering,
    pub concat:       ConcatMethod,
}

#[derive(Debug, Clone)]
pub struct TargetQualitySettings {
    pub target_quality: Option<TargetQuality>,
    // These only apply to the show VMAF option and not to TQ itself
    pub vmaf:           bool,
    pub vmaf_path:      Option<PathBuf>,
    pub vmaf_res:       String,
    pub probe_res:      Option<String>,
    pub vmaf_threads:   Option<usize>,
    pub vmaf_filter:    Option<String>,
}

#[derive(Debug, Clone)]
pub struct FfmpegSettings {
    pub ffmpeg_filter_args: Vec<String>,
    pub audio_params:       Vec<String>,
}

impl EncoderSettings {
    pub fn validate_encoder_params(&self) {
        let video_params: Vec<&str> = self
            .video_params
            .iter()
            .filter_map(|param| {
                if param.starts_with('-') && [Encoder::aom, Encoder::vpx].contains(&self.encoder) {
                    // These encoders require args to be passed using an equal sign,
                    // e.g. `--cq-level=30`
                    param.split('=').next()
                } else {
                    // The other encoders use a space, so we don't need to do extra splitting,
                    // e.g. `--crf 30`
                    None
                }
            })
            .collect();

        let help_text = {
            let [cmd, arg] = self.encoder.help_command();
            String::from_utf8(Command::new(cmd).arg(arg).output().unwrap().stdout).unwrap()
        };
        let valid_params = valid_params(&help_text, self.encoder);
        let invalid_params = invalid_params(&video_params, &valid_params);

        for wrong_param in &invalid_params {
            eprintln!(
                "'{}' isn't a valid parameter for {}",
                wrong_param, self.encoder,
            );
            if let Some(suggestion) = suggest_fix(wrong_param, &valid_params) {
                eprintln!("\tDid you mean '{suggestion}'?");
            }
        }

        if !invalid_params.is_empty() {
            println!("\nTo continue anyway, run av1an with '--force'");
            exit(1);
        }
    }

    /// Warns if rate control was not specified in encoder arguments
    pub fn check_rate_control(&self) {
        if self.encoder == Encoder::aom {
            if !self.video_params.iter().any(|f| Self::check_aom_encoder_mode(f)) {
                warn!("[WARN] --end-usage was not specified");
            }

            if !self.video_params.iter().any(|f| Self::check_aom_rate(f)) {
                warn!("[WARN] --cq-level or --target-bitrate was not specified");
            }
        }
    }

    fn check_aom_encoder_mode(s: &str) -> bool {
        const END_USAGE: &str = "--end-usage=";
        if s.len() <= END_USAGE.len() || !s.starts_with(END_USAGE) {
            return false;
        }

        s.as_bytes()[END_USAGE.len()..]
            .iter()
            .all(|&b| (b as char).is_ascii_alphabetic())
    }

    fn check_aom_rate(s: &str) -> bool {
        const CQ_LEVEL: &str = "--cq-level=";
        const TARGET_BITRATE: &str = "--target-bitrate=";

        if s.len() <= CQ_LEVEL.len() || !(s.starts_with(TARGET_BITRATE) || s.starts_with(CQ_LEVEL))
        {
            return false;
        }

        if s.starts_with(CQ_LEVEL) {
            s.as_bytes()[CQ_LEVEL.len()..].iter().all(|&b| (b as char).is_ascii_digit())
        } else {
            s.as_bytes()[TARGET_BITRATE.len()..]
                .iter()
                .all(|&b| (b as char).is_ascii_digit())
        }
    }
}

#[must_use]
pub(crate) fn invalid_params<'a>(
    params: &'a [&'a str],
    valid_options: &'a HashSet<Cow<'a, str>>,
) -> Vec<&'a str> {
    params
        .iter()
        .filter(|param| !valid_options.contains(Borrow::<str>::borrow(&**param)))
        .copied()
        .collect()
}

#[must_use]
pub(crate) fn suggest_fix<'a>(
    wrong_arg: &str,
    arg_dictionary: &'a HashSet<Cow<'a, str>>,
) -> Option<&'a str> {
    // Minimum threshold to consider a suggestion similar enough that it could be a
    // typo
    const MIN_THRESHOLD: f64 = 0.75;

    arg_dictionary
        .iter()
        .map(|arg| (arg, strsim::jaro_winkler(arg, wrong_arg)))
        .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(Ordering::Less))
        .and_then(|(suggestion, score)| {
            if score > MIN_THRESHOLD {
                Some(suggestion.borrow())
            } else {
                None
            }
        })
}

pub(crate) fn insert_noise_table_params(
    encoder: Encoder,
    video_params: &mut Vec<String>,
    table: &Path,
) -> anyhow::Result<()> {
    match encoder {
        Encoder::aom => {
            video_params.retain(|param| !param.starts_with("--denoise-noise-level="));
            video_params.push(format!("--film-grain-table={}", table.to_str().unwrap()));
        },
        Encoder::svt_av1 => {
            let film_grain_idx =
                video_params.iter().find_position(|param| param.as_str() == "--film-grain");
            if let Some((idx, _)) = film_grain_idx {
                video_params.remove(idx + 1);
                video_params.remove(idx);
            }
            video_params.push("--fgs-table".to_string());
            video_params.push(table.to_str().unwrap().to_string());
        },
        Encoder::rav1e => {
            let photon_noise_idx =
                video_params.iter().find_position(|param| param.as_str() == "--photon-noise");
            if let Some((idx, _)) = photon_noise_idx {
                video_params.remove(idx + 1);
                video_params.remove(idx);
            }
            video_params.push("--photon-noise-table".to_string());
            video_params.push(table.to_str().unwrap().to_string());
        },
        _ => bail!("This encoder does not support grain synth through av1an"),
    }

    Ok(())
}
