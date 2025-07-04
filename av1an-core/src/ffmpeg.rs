use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use av_format::rational::Rational64;
use ffmpeg::{
    color::TransferCharacteristic,
    format::{context::Input, input, Pixel},
    media::Type as MediaType,
    Error::StreamNotFound,
    Stream,
};
use path_abs::{PathAbs, PathInfo};
use tracing::warn;

use crate::{into_array, into_vec, ClipInfo, InputPixelFormat};

#[inline]
pub fn compose_ffmpeg_pipe<S: Into<String>>(
    params: impl IntoIterator<Item = S>,
    pix_format: Pixel,
) -> Vec<String> {
    let mut p: Vec<String> =
        into_vec!["ffmpeg", "-y", "-hide_banner", "-loglevel", "error", "-i", "-",];

    p.extend(params.into_iter().map(Into::into));

    p.extend(into_array![
        "-pix_fmt",
        pix_format.descriptor().unwrap().name(),
        "-strict",
        "-1",
        "-f",
        "yuv4mpegpipe",
        "-"
    ]);

    p
}

#[inline]
pub fn get_clip_info(source: &Path) -> Result<ClipInfo, ffmpeg::Error> {
    let mut ictx = input(source)?;
    let input = ictx.streams().best(MediaType::Video).ok_or(StreamNotFound)?;
    let video_stream_index = input.index();

    Ok(ClipInfo {
        format_info:              InputPixelFormat::FFmpeg {
            format: get_pixel_format(&input)?,
        },
        frame_rate:               frame_rate(&input)?,
        resolution:               resolution(&input)?,
        transfer_characteristics: match transfer_characteristics(&input)? {
            TransferCharacteristic::SMPTE2084 => av1_grain::TransferFunction::SMPTE2084,
            _ => av1_grain::TransferFunction::BT1886,
        },
        num_frames:               num_frames(&mut ictx, video_stream_index)?,
    })
}

/// Get frame count using FFmpeg
#[inline]
pub fn get_num_frames(source: &Path) -> Result<usize, ffmpeg::Error> {
    let mut ictx = input(source)?;
    let input = ictx.streams().best(MediaType::Video).ok_or(StreamNotFound)?;
    let video_stream_index = input.index();
    num_frames(&mut ictx, video_stream_index)
}

fn num_frames(ictx: &mut Input, video_stream_index: usize) -> Result<usize, ffmpeg::Error> {
    Ok(ictx
        .packets()
        .filter_map(Result::ok)
        .filter(|(stream, _)| stream.index() == video_stream_index)
        .count())
}

fn frame_rate(input: &Stream) -> Result<Rational64, ffmpeg::Error> {
    let rate = input.avg_frame_rate();
    Ok(Rational64::new(
        rate.numerator() as i64,
        rate.denominator() as i64,
    ))
}

fn get_pixel_format(input: &Stream) -> Result<Pixel, ffmpeg::Error> {
    let decoder = ffmpeg::codec::context::Context::from_parameters(input.parameters())?
        .decoder()
        .video()?;

    Ok(decoder.format())
}

fn resolution(input: &Stream) -> Result<(u32, u32), ffmpeg::Error> {
    let decoder = ffmpeg::codec::context::Context::from_parameters(input.parameters())?
        .decoder()
        .video()?;

    Ok((decoder.width(), decoder.height()))
}

fn transfer_characteristics(input: &Stream) -> Result<TransferCharacteristic, ffmpeg::Error> {
    let decoder = ffmpeg::codec::context::Context::from_parameters(input.parameters())?
        .decoder()
        .video()?;

    Ok(decoder.color_transfer_characteristic())
}

/// Returns vec of all keyframes
#[tracing::instrument(level = "debug")]
pub fn get_keyframes(source: &Path) -> Result<Vec<usize>, ffmpeg::Error> {
    let mut ictx = input(source)?;
    let input = ictx.streams().best(MediaType::Video).ok_or(StreamNotFound)?;
    let video_stream_index = input.index();

    let kfs = ictx
        .packets()
        .filter_map(Result::ok)
        .filter(|(stream, _)| stream.index() == video_stream_index)
        .map(|(_, packet)| packet)
        .enumerate()
        .filter(|(_, packet)| packet.is_key())
        .map(|(i, _)| i)
        .collect::<Vec<_>>();

    if kfs.is_empty() {
        return Ok(vec![0]);
    };

    Ok(kfs)
}

/// Returns true if input file have audio in it
#[must_use]
#[inline]
pub fn has_audio(file: &Path) -> bool {
    let ictx = input(file).unwrap();
    ictx.streams().best(MediaType::Audio).is_some()
}

/// Encodes the audio using FFmpeg, blocking the current thread.
///
/// This function returns `Some(output)` if the audio exists and the audio
/// successfully encoded, or `None` otherwise.
#[must_use]
#[inline]
pub fn encode_audio<S: AsRef<OsStr>>(
    input: impl AsRef<Path> + std::fmt::Debug,
    temp: impl AsRef<Path> + std::fmt::Debug,
    audio_params: &[S],
) -> Option<PathBuf> {
    let input = input.as_ref();
    let temp = temp.as_ref();

    if has_audio(input) {
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

        let output = encode_audio.output().unwrap();

        if !output.status.success() {
            warn!("FFmpeg failed to encode audio!\n{output:#?}\nParams: {encode_audio:?}");
            return None;
        }

        Some(audio_file)
    } else {
        None
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
