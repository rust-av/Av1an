#[cfg(test)]
mod tests;

use std::{
    fmt::{Display, Write as FmtWrite},
    fs::{self, DirEntry, File},
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
};

use anyhow::{anyhow, Context};
use av_format::{
    buffer::AccReader,
    demuxer::{Context as DemuxerContext, Event},
    muxer::{Context as MuxerContext, Writer},
    rational::Rational64,
};
use av_ivf::{demuxer::IvfDemuxer, muxer::IvfMuxer};
use path_abs::{PathAbs, PathInfo};
use serde::{Deserialize, Serialize};
use tracing::{debug, error, trace, warn};

use crate::{encoder::Encoder, util::read_in_dir};

#[derive(
    PartialEq,
    Eq,
    Copy,
    Clone,
    Serialize,
    Deserialize,
    Debug,
    strum::EnumString,
    strum::IntoStaticStr,
)]
pub enum ConcatMethod {
    #[strum(serialize = "mkvmerge")]
    MKVMerge,
    #[strum(serialize = "ffmpeg")]
    FFmpeg,
    #[strum(serialize = "ivf")]
    Ivf,
}

impl Display for ConcatMethod {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(<&'static str>::from(self))
    }
}

#[tracing::instrument(level = "debug")]
pub fn sort_files_by_filename(files: &mut [PathBuf]) {
    files.sort_unstable_by_key(|x| {
        // If the temp directory follows the expected format of 00000.ivf, 00001.ivf,
        // etc., then these unwraps will not fail
        x.file_stem()
            .expect("should have file stem")
            .to_string_lossy()
            .parse::<u32>()
            .expect("files should follow numeric pattern")
    });
}

#[tracing::instrument(level = "debug")]
pub fn ivf(input: &Path, out: &Path) -> anyhow::Result<()> {
    let mut files: Vec<PathBuf> = read_in_dir(input)?.collect();

    sort_files_by_filename(&mut files);

    assert!(!files.is_empty());

    let output = File::create(out)?;

    let mut muxer = MuxerContext::new(IvfMuxer::new(), Writer::new(output));

    let global_info = {
        let acc = AccReader::new(std::fs::File::open(&files[0])?);
        let mut demuxer = DemuxerContext::new(IvfDemuxer::new(), acc);

        demuxer.read_headers()?;

        // attempt to set the duration correctly
        let duration = demuxer.info.duration.unwrap_or(0)
            + files.iter().skip(1).try_fold(0u64, |sum, file| -> anyhow::Result<_> {
                let acc = AccReader::new(std::fs::File::open(file)?);
                let mut demuxer = DemuxerContext::new(IvfDemuxer::new(), acc);

                demuxer.read_headers()?;
                Ok(sum + demuxer.info.duration.unwrap_or(0))
            })?;

        let mut info = demuxer.info;
        info.duration = Some(duration);
        info
    };

    muxer.set_global_info(global_info)?;

    muxer.configure()?;
    muxer.write_header()?;

    let mut pos_offset: usize = 0;
    for file in &files {
        let mut last_pos: usize = 0;
        let input = std::fs::File::open(file)?;

        let acc = AccReader::new(input);

        let mut demuxer = DemuxerContext::new(IvfDemuxer::new(), acc);
        demuxer.read_headers()?;

        trace!("global info: {:#?}", demuxer.info);

        loop {
            match demuxer.read_event() {
                Ok(event) => match event {
                    Event::MoreDataNeeded(sz) => panic!("needed more data: {sz} bytes"),
                    Event::NewStream(s) => panic!("new stream: {s:?}"),
                    Event::NewPacket(mut packet) => {
                        if let Some(p) = packet.pos.as_mut() {
                            last_pos = *p;
                            *p += pos_offset;
                        }

                        trace!("received packet with pos: {:?}", packet.pos);
                        muxer.write_packet(Arc::new(packet))?;
                    },
                    Event::Continue => {
                        // do nothing
                    },
                    Event::Eof => {
                        trace!("EOF received.");
                        break;
                    },
                    _ => unimplemented!(),
                },
                Err(e) => {
                    error!("{:?}", e);
                    break;
                },
            }
        }
        pos_offset += last_pos + 1;
    }

    muxer.write_trailer()?;

    Ok(())
}

#[tracing::instrument(level = "debug")]
fn read_encoded_chunks(encode_dir: &Path) -> anyhow::Result<Vec<DirEntry>> {
    Ok(fs::read_dir(encode_dir)
        .with_context(|| {
            format!(
                "Failed to read encoded chunks from {}",
                encode_dir.display()
            )
        })?
        .collect::<Result<Vec<_>, _>>()?)
}

#[tracing::instrument(level = "debug")]
pub fn mkvmerge(
    temp_dir: &Path,
    output: &Path,
    encoder: Encoder,
    num_chunks: usize,
    output_fps: Option<Rational64>,
) -> anyhow::Result<()> {
    const MAXIMUM_CHUNKS_PER_MERGE: usize = 100;
    // mkvmerge does not accept UNC paths on Windows
    #[cfg(windows)]
    fn fix_path<P: AsRef<Path>>(p: P) -> String {
        const UNC_PREFIX: &str = r#"\\?\"#;

        let p = p.as_ref().display().to_string();
        p.strip_prefix(UNC_PREFIX).map_or_else(
            || p.clone(),
            |path| {
                path.strip_prefix("UNC")
                    .map_or_else(|| path.to_string(), |p2| format!("\\{p2}"))
            },
        )
    }

    #[cfg(not(windows))]
    fn fix_path<P: AsRef<Path>>(p: P) -> String {
        p.as_ref().display().to_string()
    }

    let audio_file = PathBuf::from(&temp_dir).join("audio.mkv");
    let audio_file = PathAbs::new(&audio_file)?;
    let audio_file = audio_file.as_path().exists().then(|| fix_path(audio_file));

    let encode_dir = PathBuf::from(temp_dir).join("encode");

    let output = PathAbs::new(output)?;

    assert!(num_chunks != 0);

    let num_chunk_groups = (num_chunks as f64 / MAXIMUM_CHUNKS_PER_MERGE as f64).ceil() as usize;
    let chunk_groups: Vec<Vec<String>> = (0..num_chunk_groups)
        .map(|group_index| {
            let start = group_index * MAXIMUM_CHUNKS_PER_MERGE;
            let end = (start + MAXIMUM_CHUNKS_PER_MERGE).min(num_chunks);
            (start..end)
                .map(|i| {
                    format!(
                        "{i:05}.{ext}",
                        ext = match encoder {
                            Encoder::x264 => "264",
                            Encoder::x265 => "hevc",
                            _ => "ivf",
                        }
                    )
                })
                .collect()
        })
        .collect();

    chunk_groups.iter().enumerate().try_for_each(|(group_index, chunk_group)| {
        let group_options_path =
            PathBuf::from(&temp_dir).join(format!("group_options_{group_index:05}.json"));
        let group_options_output_path = PathAbs::new(
            PathBuf::from(&temp_dir).join(format!("group_output_{group_index:05}.mkv")),
        )?;

        let group_options_json_contents = mkvmerge_options_json(
            chunk_group,
            &fix_path(group_options_output_path.to_string_lossy().as_ref()),
            None,
            output_fps,
        );

        let mut group_options_json = File::create(group_options_path)?;
        group_options_json.write_all(group_options_json_contents?.as_bytes())?;

        let mut group_cmd = Command::new("mkvmerge");
        group_cmd.current_dir(&encode_dir);
        group_cmd.arg(format!("@../group_options_{group_index:05}.json"));

        let group_out = group_cmd
            .output()
            .with_context(|| "Failed to execute mkvmerge command for concatenation")?;

        if !group_out.status.success() {
            return Err(anyhow::Error::msg(format!(
                "Failed to execute mkvmerge command for concatenation: {}",
                String::from_utf8_lossy(&group_out.stderr)
            )));
        }

        Ok(())
    })?;

    let chunk_group_options_names: Vec<String> = (0..num_chunk_groups)
        .map(|group_index| format!("group_output_{group_index:05}.mkv"))
        .collect();

    let options_path = PathBuf::from(&temp_dir).join("options.json");
    let options_json_contents = mkvmerge_options_json(
        &chunk_group_options_names,
        &fix_path(output.to_string_lossy().as_ref()),
        audio_file.as_deref(),
        output_fps,
    );

    let mut options_json = File::create(options_path)?;
    options_json.write_all(options_json_contents?.as_bytes())?;

    let mut cmd = Command::new("mkvmerge");
    cmd.current_dir(temp_dir);
    cmd.arg("@./options.json");

    let out = cmd
        .output()
        .with_context(|| "Failed to execute mkvmerge command for concatenation")?;

    if !out.status.success() {
        // TODO: make an EncoderCrash-like struct, but without all the other fields so
        // it can be used in a more broad scope than just for the pipe/encoder
        error!(
            "mkvmerge concatenation failed with output: {:#?}\ncommand: {:?}",
            out, cmd
        );
        return Err(anyhow!("mkvmerge concatenation failed"));
    }

    Ok(())
}

/// Create mkvmerge options.json
#[tracing::instrument(level = "debug")]
pub fn mkvmerge_options_json(
    chunks: &[String],
    output: &str,
    audio: Option<&str>,
    output_fps: Option<Rational64>,
) -> anyhow::Result<String> {
    let mut file_string = String::with_capacity(
        64 + output.len()
            + audio.map_or(0, |a| a.len() + 2)
            + chunks.iter().map(|s| s.len() + 4).sum::<usize>(),
    );
    write!(file_string, "[\"-o\", {output:?}")?;
    if let Some(audio) = audio {
        write!(file_string, ", {audio:?}")?;
    }
    if let Some(output_fps) = output_fps {
        write!(
            file_string,
            ", \"--default-duration\", \"0:{}/{}fps\", \"[\"",
            output_fps.numer(),
            output_fps.denom()
        )?;
    } else {
        file_string.push_str(", \"[\"");
    }
    for chunk in chunks {
        write!(file_string, ", \"{chunk}\"")?;
    }
    file_string.push_str(",\"]\"]");

    Ok(file_string)
}

/// Concatenates using ffmpeg (does not work with x265, and may have incorrect
/// FPS with vpx)
#[tracing::instrument(level = "debug")]
pub fn ffmpeg(temp: &Path, output: &Path) -> anyhow::Result<()> {
    fn write_concat_file(temp_folder: &Path) -> anyhow::Result<()> {
        let concat_file = temp_folder.join("concat");
        let encode_folder = temp_folder.join("encode");

        let mut files = read_encoded_chunks(&encode_folder)?;

        files.sort_by_key(DirEntry::path);

        let mut contents = String::with_capacity(24 * files.len());

        for i in files {
            writeln!(
                contents,
                "file {}",
                format!("{path}", path = i.path().display())
                    .replace('\\', r"\\")
                    .replace(' ', r"\ ")
                    .replace('\'', r"\'")
            )?;
        }

        let mut file = File::create(concat_file)?;
        file.write_all(contents.as_bytes())?;

        Ok(())
    }

    let temp = PathAbs::new(temp)?;
    let temp = temp.as_path();

    let concat = temp.join("concat");
    let concat_file = concat.to_string_lossy();

    write_concat_file(temp)?;

    let audio_file = {
        let file = temp.join("audio.mkv");
        (file.exists() && file.metadata().expect("file should have metadata").len() > 1000)
            .then_some(file)
    };

    let mut cmd = Command::new("ffmpeg");

    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    if let Some(file) = audio_file {
        cmd.args([
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
            &concat_file,
            "-i",
        ])
        .arg(file)
        .args(["-map", "0", "-map", "1", "-c", "copy"])
        .arg(output);
    } else {
        cmd.args([
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
            &concat_file,
        ])
        .args(["-map", "0", "-c", "copy"])
        .arg(output);
    }

    debug!("FFmpeg concat command: {:?}", cmd);

    let out = cmd
        .output()
        .with_context(|| "Failed to execute FFmpeg command for concatenation")?;

    if !out.status.success() {
        error!(
            "FFmpeg concatenation failed with output: {:#?}\ncommand: {:?}",
            out, cmd
        );
        return Err(anyhow!("FFmpeg concatenation failed"));
    }

    Ok(())
}
