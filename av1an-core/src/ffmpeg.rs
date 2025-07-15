use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    str::FromStr,
};

use anyhow::bail;
use av_format::rational::Rational64;
use path_abs::{PathAbs, PathInfo};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::{into_array, into_vec, ClipInfo, InputPixelFormat};

#[inline]
pub fn compose_ffmpeg_pipe<S: Into<String>>(
    params: impl IntoIterator<Item = S>,
    pix_format: FFPixelFormat,
) -> Vec<String> {
    let mut p: Vec<String> =
        into_vec!["ffmpeg", "-y", "-hide_banner", "-loglevel", "error", "-i", "-",];

    p.extend(params.into_iter().map(Into::into));

    p.extend(into_array![
        "-pix_fmt",
        pix_format.to_pix_fmt_string(),
        "-strict",
        "-1",
        "-f",
        "yuv4mpegpipe",
        "-"
    ]);

    p
}

#[derive(Debug, Clone, Deserialize)]
struct FfProbeInfo {
    pub streams: Vec<FfProbeStreamInfo>,
}

#[derive(Debug, Clone, Deserialize)]
struct FfProbeStreamInfo {
    pub width:          u32,
    pub height:         u32,
    pub pix_fmt:        String,
    pub color_transfer: Option<String>,
    pub avg_frame_rate: String,
    pub nb_frames:      Option<usize>,
}

#[inline]
pub fn get_clip_info(source: &Path) -> anyhow::Result<ClipInfo> {
    let output = Command::new("ffprobe")
        .arg("-v")
        .arg("quiet")
        .arg("-select_streams")
        .arg("v:0")
        .arg("-print_format")
        .arg("json")
        .arg("-show_entries")
        .arg("stream=width,height,pix_fmt,avg_frame_rate,nb_frames,color_transfer")
        .arg(source)
        .output()?
        .stdout;
    let ffprobe_info: FfProbeInfo = serde_json::from_slice(&output)?;
    let stream_info = ffprobe_info
        .streams
        .first()
        .ok_or_else(|| anyhow::anyhow!("no video streams found in source file"))?;

    Ok(ClipInfo {
        format_info:              InputPixelFormat::FFmpeg {
            format: FFPixelFormat::from_str(&stream_info.pix_fmt)?,
        },
        frame_rate:               parse_frame_rate(&stream_info.avg_frame_rate)?,
        resolution:               (stream_info.width, stream_info.height),
        transfer_characteristics: match stream_info.color_transfer.as_deref() {
            Some("smpte2084") => av1_grain::TransferFunction::SMPTE2084,
            _ => av1_grain::TransferFunction::BT1886,
        },
        num_frames:               match stream_info.nb_frames {
            Some(nb_frames) => nb_frames,
            None => get_num_frames(source)?,
        },
    })
}

/// Get frame count using FFmpeg
#[inline]
pub fn get_num_frames(source: &Path) -> anyhow::Result<usize> {
    let output = Command::new("ffprobe")
        .arg("-v")
        .arg("error")
        .arg("-select_streams")
        .arg("v:0")
        .arg("-count_packets")
        .arg("-show_entries")
        .arg("stream=nb_read_packets")
        .arg("-print_format")
        .arg("csv=p=0")
        .arg(source)
        .output()?
        .stdout;
    match String::from_utf8_lossy(&output).trim().parse::<usize>() {
        Ok(x) if x > 0 => Ok(x),
        _ => {
            // If we got empty output or a 0 frame count, try using the slower
            // but more reliable method
            get_num_frames_slow(source)
        },
    }
}

/// Slower but more reliable frame count method
fn get_num_frames_slow(source: &Path) -> anyhow::Result<usize> {
    let output = Command::new("ffprobe")
        .arg("-v")
        .arg("error")
        .arg("-count_frames")
        .arg("-select_streams")
        .arg("v:0")
        .arg("-show_entries")
        .arg("stream=nb_read_frames")
        .arg("-print_format")
        .arg("default=noprint_wrappers=1:nokey=1")
        .arg(source)
        .output()?
        .stdout;
    Ok(String::from_utf8_lossy(&output).trim().parse::<usize>()?)
}

fn parse_frame_rate(rate: &str) -> anyhow::Result<Rational64> {
    let (numer, denom) = rate
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!("failed to parse frame rate from ffprobe output"))?;
    Ok(Rational64::new(
        numer.parse::<i64>()?,
        denom.parse::<i64>()?,
    ))
}

#[derive(Debug, Clone, Deserialize)]
struct FfProbeKeyframesData {
    pub frames: Vec<FfProbeKeyframeFrame>,
}

#[derive(Debug, Clone, Deserialize)]
struct FfProbeKeyframeFrame {
    // 0 or 1
    pub key_frame: u8,
}

/// Returns vec of all keyframes
#[tracing::instrument(level = "debug")]
pub fn get_keyframes(source: &Path) -> anyhow::Result<Vec<usize>> {
    // This is slow because it has to iterate through the whole video,
    // but it is the best suggestion that reliably worked
    // since not all codecs code "coded_picture_number" into the frames.
    let output = Command::new("ffprobe")
        .arg("-v")
        .arg("quiet")
        .arg("-print_format")
        .arg("json")
        .arg("-show_frames")
        .arg("-select_streams")
        .arg("v:0")
        .arg("-show_entries")
        .arg("frame=key_frame")
        .arg(source)
        .output()?
        .stdout;
    let frames = serde_json::from_slice::<FfProbeKeyframesData>(&output)?.frames;
    Ok(frames
        .into_iter()
        .enumerate()
        .filter_map(|(i, frame)| if frame.key_frame > 0 { Some(i) } else { None })
        .collect())
}

/// Returns true if input file have audio in it
#[inline]
pub fn has_audio(file: &Path) -> anyhow::Result<bool> {
    let output = Command::new("ffprobe")
        .arg("-v")
        .arg("error")
        .arg("-select_streams")
        .arg("a")
        .arg("-show_entries")
        .arg("stream=index")
        .arg("-of")
        .arg("csv=p=0")
        .arg(file)
        .output()?
        .stdout;
    let output = String::from_utf8_lossy(&output);
    Ok(!output.trim().is_empty())
}

/// Encodes the audio using FFmpeg, blocking the current thread.
///
/// This function returns `Some(output)` if the audio exists and the audio
/// successfully encoded, or `None` otherwise.
#[inline]
pub fn encode_audio<S: AsRef<OsStr>>(
    input: impl AsRef<Path> + std::fmt::Debug,
    temp: impl AsRef<Path> + std::fmt::Debug,
    audio_params: &[S],
) -> anyhow::Result<Option<PathBuf>> {
    let input = input.as_ref();
    let temp = temp.as_ref();

    if has_audio(input)? {
        let audio_file = Path::new(temp).join("audio.mkv");
        let mut encode_audio = Command::new("ffmpeg");

        encode_audio.stdout(Stdio::piped());
        encode_audio.stderr(Stdio::piped());

        encode_audio.args(["-y", "-hide_banner", "-loglevel", "error"]);
        encode_audio.args(["-i", input.to_str().unwrap()]);
        encode_audio.args(["-map_metadata", "0"]);
        encode_audio.args(["-map", "0", "-c", "copy", "-vn", "-dn"]);

        encode_audio.args(audio_params);
        encode_audio.arg(&audio_file);

        let output = encode_audio.output()?;

        if !output.status.success() {
            warn!("FFmpeg failed to encode audio!\n{output:#?}\nParams: {encode_audio:?}");
            return Ok(None);
        }

        Ok(Some(audio_file))
    } else {
        Ok(None)
    }
}

/// Escapes paths in ffmpeg filters if on windows
#[inline]
pub fn escape_path_in_filter(path: impl AsRef<Path>) -> String {
    if cfg!(windows) {
        PathAbs::new(path.as_ref())
      .unwrap()
      .to_str()
      .unwrap()
      // This is needed because of how FFmpeg handles absolute file paths on Windows.
      // https://stackoverflow.com/questions/60440793/how-can-i-use-windows-absolute-paths-with-the-movie-filter-on-ffmpeg
      .replace('\\', "/")
      .replace(':', r"\\:")
    } else {
        PathAbs::new(path.as_ref()).unwrap().to_str().unwrap().to_string()
    }
    .replace('[', r"\[")
    .replace(']', r"\]")
    .replace(',', "\\,")
}

/// Pixel formats supported by ffmpeg
#[allow(non_camel_case_types)]
#[derive(Eq, PartialEq, Copy, Clone, Debug, Serialize, Deserialize)]
pub enum FFPixelFormat {
    GBRP,
    GBRP10LE,
    GBRP12L,
    GBRP12LE,
    GRAY10LE,
    GRAY12L,
    GRAY12LE,
    GRAY8,
    NV12,
    NV16,
    NV20LE,
    NV21,
    YUV420P,
    YUV420P10LE,
    YUV420P12LE,
    YUV422P,
    YUV422P10LE,
    YUV422P12LE,
    YUV440P,
    YUV440P10LE,
    YUV440P12LE,
    YUV444P,
    YUV444P10LE,
    YUV444P12LE,
    YUVA420P,
    YUVJ420P,
    YUVJ422P,
    YUVJ444P,
}

impl FFPixelFormat {
    /// The string to be used with ffmpeg's `-pix_fmt` argument.
    #[inline]
    pub fn to_pix_fmt_string(&self) -> &'static str {
        match self {
            FFPixelFormat::GBRP => "gbrp",
            FFPixelFormat::GBRP10LE => "gbrp10le",
            FFPixelFormat::GBRP12L => "gbrp12l",
            FFPixelFormat::GBRP12LE => "gbrp12le",
            FFPixelFormat::GRAY10LE => "gray10le",
            FFPixelFormat::GRAY12L => "gray12l",
            FFPixelFormat::GRAY12LE => "gray12le",
            FFPixelFormat::GRAY8 => "gray",
            FFPixelFormat::NV12 => "nv12",
            FFPixelFormat::NV16 => "nv16",
            FFPixelFormat::NV20LE => "nv20le",
            FFPixelFormat::NV21 => "nv21",
            FFPixelFormat::YUV420P => "yuv420p",
            FFPixelFormat::YUV420P10LE => "yuv420p10le",
            FFPixelFormat::YUV420P12LE => "yuv420p12le",
            FFPixelFormat::YUV422P => "yuv422p",
            FFPixelFormat::YUV422P10LE => "yuv422p10le",
            FFPixelFormat::YUV422P12LE => "yuv422p12le",
            FFPixelFormat::YUV440P => "yuv440p",
            FFPixelFormat::YUV440P10LE => "yuv440p10le",
            FFPixelFormat::YUV440P12LE => "yuv440p12le",
            FFPixelFormat::YUV444P => "yuv444p",
            FFPixelFormat::YUV444P10LE => "yuv444p10le",
            FFPixelFormat::YUV444P12LE => "yuv444p12le",
            FFPixelFormat::YUVA420P => "yuva420p",
            FFPixelFormat::YUVJ420P => "yuvj420p",
            FFPixelFormat::YUVJ422P => "yuvj422p",
            FFPixelFormat::YUVJ444P => "yuvj444p",
        }
    }
}

impl FromStr for FFPixelFormat {
    type Err = anyhow::Error;

    #[inline]
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "gbrp" => FFPixelFormat::GBRP,
            "gbrp10le" => FFPixelFormat::GBRP10LE,
            "gbrp12l" => FFPixelFormat::GBRP12L,
            "gbrp12le" => FFPixelFormat::GBRP12LE,
            "gray10le" => FFPixelFormat::GRAY10LE,
            "gray12l" => FFPixelFormat::GRAY12L,
            "gray12le" => FFPixelFormat::GRAY12LE,
            "gray" => FFPixelFormat::GRAY8,
            "nv12" => FFPixelFormat::NV12,
            "nv16" => FFPixelFormat::NV16,
            "nv20le" => FFPixelFormat::NV20LE,
            "nv21" => FFPixelFormat::NV21,
            "yuv420p" => FFPixelFormat::YUV420P,
            "yuv420p10le" => FFPixelFormat::YUV420P10LE,
            "yuv420p12le" => FFPixelFormat::YUV420P12LE,
            "yuv422p" => FFPixelFormat::YUV422P,
            "yuv422p10le" => FFPixelFormat::YUV422P10LE,
            "yuv422p12le" => FFPixelFormat::YUV422P12LE,
            "yuv440p" => FFPixelFormat::YUV440P,
            "yuv440p10le" => FFPixelFormat::YUV440P10LE,
            "yuv440p12le" => FFPixelFormat::YUV440P12LE,
            "yuv444p" => FFPixelFormat::YUV444P,
            "yuv444p10le" => FFPixelFormat::YUV444P10LE,
            "yuv444p12le" => FFPixelFormat::YUV444P12LE,
            "yuva420p" => FFPixelFormat::YUVA420P,
            "yuvj420p" => FFPixelFormat::YUVJ420P,
            "yuvj422p" => FFPixelFormat::YUVJ422P,
            "yuvj444p" => FFPixelFormat::YUVJ444P,
            s => bail!("Unsupported pixel format string: {s}"),
        })
    }
}
