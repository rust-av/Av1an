//! Reads the track layout of a source file so `--discard` can name the tracks
//! it drops.
//!
//! `mkvmerge --identify` is the primary probe because the mkvmerge output
//! settings are keyed by its track IDs. FFmpeg's settings are keyed by stream
//! index instead, which mkvmerge does not report, so the FFmpeg method also
//! probes with `ffprobe` and matches the two lists by position within each
//! track type.

use std::{
    io,
    path::{Path, PathBuf},
    process::ExitStatus,
};

use serde::Deserialize;
use thiserror::Error;
use tracing::debug;

/// The kinds of track [`ProbedSource`] records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackKind {
    Video,
    Audio,
    Subtitle,
    Attachment,
}

impl TrackKind {
    /// A human-readable name, used in messages.
    #[inline]
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Video => "video",
            Self::Audio => "audio",
            Self::Subtitle => "subtitle",
            Self::Attachment => "attachment",
        }
    }
}

/// One track or stream of the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbedTrack {
    pub kind: TrackKind,
    /// mkvmerge's track ID, absent when only `ffprobe` was available.
    pub mkvmerge_id: Option<u64>,
    /// FFmpeg's stream index, absent when `ffprobe` was not run.
    pub ffmpeg_index: Option<u32>,
}

/// Every track and stream of a source file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProbedSource {
    pub tracks: Vec<ProbedTrack>,
}

impl ProbedSource {
    /// mkvmerge's track IDs for `kind`, in container order.
    #[must_use]
    pub fn mkvmerge_ids(&self, kind: TrackKind) -> Vec<u64> {
        self.tracks
            .iter()
            .filter(|track| track.kind == kind)
            .filter_map(|track| track.mkvmerge_id)
            .collect()
    }

    /// FFmpeg's stream indices for `kind`, in container order.
    #[must_use]
    pub fn ffmpeg_indices(&self, kind: TrackKind) -> Vec<u32> {
        self.tracks
            .iter()
            .filter(|track| track.kind == kind)
            .filter_map(|track| track.ffmpeg_index)
            .collect()
    }
}

/// Probes `path`, reporting mkvmerge track IDs and, when `with_ffmpeg_streams`
/// is set, FFmpeg stream indices as well.
///
/// # Errors
///
/// Requires successful `ffprobe` output when `with_ffmpeg_streams` is set.
/// Otherwise requires recognized, supported mkvmerge identification without
/// errors, from a process exiting 0 (success) or 1 (warnings).
pub fn probe(path: &Path, with_ffmpeg_streams: bool) -> Result<ProbedSource, TrackProbeError> {
    let streams = if with_ffmpeg_streams {
        Some(ffprobe_streams(path)?)
    } else {
        None
    };

    let identified = match mkvmerge_identify(path) {
        Ok(identified) => identified,
        // FFmpeg's settings are keyed by stream index, so a source mkvmerge
        // cannot read is still usable with the FFmpeg method alone.
        Err(err) => match streams {
            Some(streams) => {
                debug!("{}", TrackProbeError::FfprobeFallback(Box::new(err)));
                return Ok(ProbedSource {
                    tracks: tracks_from_streams(&streams),
                });
            },
            None => return Err(err),
        },
    };

    let mut tracks = identified.tracks();
    if let Some(streams) = &streams {
        match_tracks_by_position(&mut tracks, streams);
    }

    let source = ProbedSource { tracks };
    debug!(
        "{}",
        TrackProbeError::Probed {
            path: path.to_path_buf(),
            summary: summarize(&source),
        }
    );

    Ok(source)
}

/// Builds a probed source from FFmpeg's streams alone.
fn tracks_from_streams(streams: &[FfprobeStream]) -> Vec<ProbedTrack> {
    streams
        .iter()
        .filter_map(|stream| {
            Some(ProbedTrack {
                kind: stream.kind()?,
                mkvmerge_id: None,
                ffmpeg_index: Some(stream.index),
            })
        })
        .collect()
}

/// A one-line description of a probed source, for logs and `--verbose`.
#[must_use]
pub fn summarize(source: &ProbedSource) -> String {
    let count = |kind| source.tracks.iter().filter(|track| track.kind == kind).count();
    format!(
        "{} video, {} audio, {} subtitle, {} attachment",
        count(TrackKind::Video),
        count(TrackKind::Audio),
        count(TrackKind::Subtitle),
        count(TrackKind::Attachment)
    )
}

/// Everything a probe can report.
///
/// The last two variants describe non-fatal outcomes and are logged instead.
#[derive(Debug, Error)]
pub enum TrackProbeError {
    #[error(
        "failed to run mkvmerge to inspect {path}: {source}. Install MKVToolNix, or drop \
         --discard",
        path = .path.display()
    )]
    MkvmergeNotRunnable { path: PathBuf, source: io::Error },
    #[error(
        "mkvmerge could not identify {path} ({status}): {source}; stdout: {stdout}; stderr: {stderr}",
        path = .path.display()
    )]
    MkvmergeIdentifyFailed {
        path: PathBuf,
        status: ExitStatus,
        stdout: String,
        stderr: String,
        source: serde_json::Error,
    },
    #[error(
        "mkvmerge rejected identification of {path} ({status}): {reason}; stdout: {stdout}; stderr: {stderr}",
        path = .path.display()
    )]
    MkvmergeRejected {
        path: PathBuf,
        status: ExitStatus,
        reason: String,
        stdout: String,
        stderr: String,
    },
    #[error(
        "failed to run ffprobe to inspect {path}: {source}. Install FFmpeg, or drop --discard",
        path = .path.display()
    )]
    FfprobeNotRunnable { path: PathBuf, source: io::Error },
    #[error("ffprobe could not inspect {path}: {stderr}", path = .path.display())]
    FfprobeFailed { path: PathBuf, stderr: String },
    #[error("unexpected ffprobe output for {path}", path = .path.display())]
    FfprobeOutputUnreadable {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("{0}; probing the FFmpeg streams only")]
    FfprobeFallback(Box<Self>),
    #[error("probed {path}: {summary}", path = .path.display())]
    Probed { path: PathBuf, summary: String },
}

/// Runs `mkvmerge --identify` on `path`.
fn mkvmerge_identify(path: &Path) -> Result<MkvmergeIdentify, TrackProbeError> {
    let output = std::process::Command::new("mkvmerge")
        .arg("--identify")
        .arg("--identification-format")
        .arg("json")
        .arg(path)
        .output()
        .map_err(|source| TrackProbeError::MkvmergeNotRunnable {
            path: path.to_path_buf(),
            source,
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let rejected = |reason| TrackProbeError::MkvmergeRejected {
        path: path.to_path_buf(),
        status: output.status,
        reason,
        stdout: stdout.clone(),
        stderr: stderr.clone(),
    };
    // Exit 1 allows warnings, not errors or an unrecognized/unsupported source.
    if !matches!(output.status.code(), Some(0 | 1)) {
        return Err(rejected("identification process failed".to_owned()));
    }
    let identified: MkvmergeIdentify = serde_json::from_str(&stdout).map_err(|source| {
        TrackProbeError::MkvmergeIdentifyFailed {
            path: path.to_path_buf(),
            status: output.status,
            stdout: stdout.clone(),
            stderr: stderr.clone(),
            source,
        }
    })?;
    if !identified.errors.is_empty() {
        return Err(rejected(identified.errors.join("; ")));
    }
    if !identified.container.recognized || !identified.container.supported {
        return Err(rejected(format!(
            "container recognized={}, supported={}",
            identified.container.recognized, identified.container.supported
        )));
    }
    Ok(identified)
}

/// Runs `ffprobe` on `path`.
fn ffprobe_streams(path: &Path) -> Result<Vec<FfprobeStream>, TrackProbeError> {
    let output = std::process::Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "stream=index,codec_type", "-of", "json"])
        .arg(path)
        .output()
        .map_err(|source| TrackProbeError::FfprobeNotRunnable {
            path: path.to_path_buf(),
            source,
        })?;

    if !output.status.success() {
        return Err(TrackProbeError::FfprobeFailed {
            path: path.to_path_buf(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }

    let probed: FfprobeOutput = serde_json::from_slice(&output.stdout).map_err(|source| {
        TrackProbeError::FfprobeOutputUnreadable {
            path: path.to_path_buf(),
            source,
        }
    })?;
    Ok(probed.streams)
}

/// Fills in each track's FFmpeg stream index by pairing the tracks of a type
/// with the streams of that type in order.
///
/// Both tools list tracks in container order, so the pairings hold as long as
/// the counts agree; a stream type neither tool reports on is left unmatched.
fn match_tracks_by_position(tracks: &mut [ProbedTrack], streams: &[FfprobeStream]) {
    for kind in [TrackKind::Video, TrackKind::Audio, TrackKind::Subtitle, TrackKind::Attachment] {
        let indices = streams
            .iter()
            .filter(|stream| stream.kind() == Some(kind))
            .map(|stream| stream.index);
        for (track, index) in tracks.iter_mut().filter(|track| track.kind == kind).zip(indices) {
            track.ffmpeg_index = Some(index);
        }
    }
}

impl FfprobeStream {
    /// This stream's track kind, or `None` for a stream type Condor does not
    /// select (data, unknown).
    fn kind(&self) -> Option<TrackKind> {
        match self.codec_type.as_deref() {
            Some("video") => Some(TrackKind::Video),
            Some("audio") => Some(TrackKind::Audio),
            Some("subtitle") => Some(TrackKind::Subtitle),
            Some("attachment") => Some(TrackKind::Attachment),
            _ => None,
        }
    }
}

impl MkvmergeIdentify {
    /// The tracks and attachments this identification reports.
    fn tracks(&self) -> Vec<ProbedTrack> {
        let tracks = self.tracks.iter().map(|track| ProbedTrack {
            kind: track.kind(),
            mkvmerge_id: Some(track.id),
            ffmpeg_index: None,
        });
        let attachments = self.attachments.iter().map(|attachment| ProbedTrack {
            kind: TrackKind::Attachment,
            mkvmerge_id: Some(attachment.id),
            ffmpeg_index: None,
        });
        tracks.chain(attachments).collect()
    }
}

/// `mkvmerge --identify --identification-format json`, reduced to what
/// `--discard` needs.
#[derive(Debug, Deserialize)]
struct MkvmergeIdentify {
    container: MkvmergeContainer,
    #[serde(default)]
    errors: Vec<String>,
    #[serde(default)]
    tracks: Vec<MkvmergeTrackInfo>,
    #[serde(default)]
    attachments: Vec<MkvmergeAttachmentInfo>,
}

#[derive(Debug, Deserialize)]
struct MkvmergeContainer {
    recognized: bool,
    supported: bool,
}

#[derive(Debug, Deserialize)]
struct MkvmergeTrackInfo {
    id: u64,
    #[serde(rename = "type")]
    kind: String,
}

impl MkvmergeTrackInfo {
    /// This track's kind. mkvmerge's `button` and future track types are
    /// reported as video so that `--discard video` accounts for them, matching
    /// how mkvmerge treats types it cannot otherwise place.
    fn kind(&self) -> TrackKind {
        match self.kind.as_str() {
            "audio" => TrackKind::Audio,
            "subtitles" => TrackKind::Subtitle,
            _ => TrackKind::Video,
        }
    }
}

#[derive(Debug, Deserialize)]
struct MkvmergeAttachmentInfo {
    id: u64,
}

#[derive(Debug, Deserialize)]
struct FfprobeOutput {
    #[serde(default)]
    streams: Vec<FfprobeStream>,
}

#[derive(Debug, Deserialize)]
struct FfprobeStream {
    index: u32,
    #[serde(default)]
    codec_type: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn fake_probe(directory: &Path, name: &str, json: &str, status: &str) {
        use std::os::unix::fs::PermissionsExt;
        // Only shell builtins: an isolated PATH cannot accidentally run a native tool.
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >> \"$TRACK_PROBE_CALLS\"\nprintf '%s\\n' '{json}'\nprintf '%s\\n' 'native diagnostic' >&2\n{status}\n"
        );
        let path = directory.join(name);
        std::fs::write(&path, script).expect("fake probe");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
            .expect("executable probe");
    }

    /// Re-exec keeps PATH local to a bounded child, never a process-global mutation.
    #[cfg(unix)]
    #[test]
    fn identification_admission_and_ffmpeg_fallback() {
        let valid = r#"{"container":{"recognized":true,"supported":true},"errors":[],"warnings":["recoverable warning"],"tracks":[{"id":0,"type":"video"},{"id":3,"type":"audio"}]}"#;
        let cases = [
            ("ok", valid, "exit 0", "accept"),
            ("warning", valid, "exit 1", "accept"),
            (
                "empty-recognized",
                r#"{"container":{"recognized":true,"supported":true},"errors":[]}"#,
                "exit 0",
                "empty",
            ),
            ("fatal-status", valid, "exit 2", "process"),
            (
                "error-status",
                r#"{"errors":["native identify error"]}"#,
                "exit 2",
                "process",
            ),
            (
                "error-zero",
                r#"{"container":{"recognized":true,"supported":true},"errors":["native identify error"]}"#,
                "exit 0",
                "semantic-error",
            ),
            (
                "error-warning",
                r#"{"container":{"recognized":true,"supported":true},"errors":["native identify error"]}"#,
                "exit 1",
                "semantic-error",
            ),
            (
                "unrecognized",
                r#"{"container":{"recognized":false,"supported":false},"errors":[]}"#,
                "exit 0",
                "container",
            ),
            (
                "unsupported",
                r#"{"container":{"recognized":true,"supported":false},"errors":[]}"#,
                "exit 1",
                "container",
            ),
            ("no-container", r#"{"errors":[]}"#, "exit 0", "parse"),
            (
                "malformed-container",
                r#"{"container":[],"errors":[]}"#,
                "exit 0",
                "parse",
            ),
            (
                "missing-flags",
                r#"{"container":{},"errors":[]}"#,
                "exit 0",
                "parse",
            ),
            (
                "absent-recognized",
                r#"{"container":{"supported":true},"errors":[]}"#,
                "exit 0",
                "parse",
            ),
            (
                "absent-supported",
                r#"{"container":{"recognized":true},"errors":[]}"#,
                "exit 0",
                "parse",
            ),
            (
                "malformed-flags",
                r#"{"container":{"recognized":"yes","supported":true},"errors":[]}"#,
                "exit 0",
                "parse",
            ),
            ("malformed-json", "{not json", "exit 0", "parse"),
            ("signal", valid, "kill -TERM $$", "process"),
            ("not-runnable", "", "exit 0", "io"),
            (
                "ffmpeg-fallback",
                r#"{"errors":["native identify error"]}"#,
                "exit 2",
                "fallback",
            ),
            (
                "ffmpeg-semantic-fallback",
                r#"{"container":{"recognized":false,"supported":false},"errors":[]}"#,
                "exit 0",
                "fallback",
            ),
            ("invalid-ffprobe", valid, "exit 0", "ffprobe-parse"),
        ];
        let mut failures = Vec::new();
        for (case, json, status, expected) in cases {
            let directory = tempfile::tempdir().expect("isolated probes");
            if expected != "io" {
                fake_probe(directory.path(), "mkvmerge", json, status);
            }
            fake_probe(
                directory.path(),
                "ffprobe",
                if expected == "ffprobe-parse" {
                    "{invalid"
                } else {
                    r#"{"streams":[{"index":0,"codec_type":"video"},{"index":7,"codec_type":"audio"},{"index":11,"codec_type":"subtitle"},{"index":15,"codec_type":"attachment"}]}"#
                },
                "exit 0",
            );
            let mut child = assert_cmd::Command::new(std::env::current_exe().expect("test binary"));
            let assertion = child
                .args(["--exact", "utils::track_probe::tests::track_probe_admission_child", "--nocapture"])
                // Other legacy tests change the parent's process-wide CWD.
                .current_dir(directory.path())
                .env("PATH", directory.path())
                .env("TRACK_PROBE_EXPECTED", expected)
                .env("TRACK_PROBE_CALLS", directory.path().join("calls"))
                .env("TRACK_PROBE_SOURCE", directory.path().join("source.mkv"))
                .timeout(std::time::Duration::from_secs(20))
                .assert();
            let output = assertion.get_output();
            if !output.status.success()
                || !String::from_utf8_lossy(&output.stdout).contains("test result: ok. 1 passed;")
            {
                failures.push(case);
                eprintln!(
                    "track probe {case}: {} {}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            eprintln!("track probe {case}: {expected}");
        }
        assert!(
            failures.is_empty(),
            "track probe admission failures: {failures:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn track_probe_admission_child() {
        use crate::{
            commands::{DiscardCategory, handlers::concatenate::apply_discard},
            test_helpers::{TestVideo, default_config},
        };
        use andean_condor::models::sequence::scene_concatenator::ConcatMethod;
        let Ok(expected) = std::env::var("TRACK_PROBE_EXPECTED") else {
            return;
        };
        let path = PathBuf::from(std::env::var_os("TRACK_PROBE_SOURCE").expect("source path"));
        std::fs::write(&path, b"source sentinel").expect("source");
        let with_ffmpeg = matches!(expected.as_str(), "fallback" | "ffprobe-parse");
        let result = probe(&path, with_ffmpeg);
        if expected == "empty" {
            assert!(
                result
                    .expect("admitted identification may omit ancillary lists")
                    .tracks
                    .is_empty()
            );
            return;
        }
        if expected == "accept" {
            let source = result.expect("recognized status 0/1 admitted");
            assert_eq!(source.mkvmerge_ids(TrackKind::Video), [0]);
            assert_eq!(source.mkvmerge_ids(TrackKind::Audio), [3]);
            let calls =
                std::fs::read_to_string(std::env::var_os("TRACK_PROBE_CALLS").expect("calls"))
                    .expect("probe invoked");
            assert_eq!(
                calls,
                format!(
                    "--identify\n--identification-format\njson\n{}\n",
                    path.display()
                )
            );
            return;
        }

        let output = path.with_file_name("out.mkv");
        std::fs::write(&output, b"output sentinel").expect("output");
        let video = TestVideo {
            path: path.clone(),
            width: 2,
            height: 2,
            frames: 1,
            fps_rational: (24, 1),
            format: andean_condor::ffmpeg::FFPixelFormat::YUV420P,
            scenes: vec![],
        };
        let mut config = default_config(&video, &output, path.parent().expect("parent"));
        if with_ffmpeg {
            config.condor.sequence_config.scene_concatenator.method =
                ConcatMethod::FFmpeg(Default::default());
        }
        if expected == "fallback" {
            let source = result.expect("FFmpeg fallback after mkvmerge rejection");
            assert_eq!(source.ffmpeg_indices(TrackKind::Video), [0]);
            assert_eq!(source.ffmpeg_indices(TrackKind::Audio), [7]);
            assert_eq!(source.ffmpeg_indices(TrackKind::Subtitle), [11]);
            assert_eq!(source.ffmpeg_indices(TrackKind::Attachment), [15]);
            assert!(source.tracks.iter().all(|track| track.mkvmerge_id.is_none()));
            apply_discard(&mut config, &[DiscardCategory::Audio]).expect("real discard");
            let ConcatMethod::FFmpeg(ffmpeg) =
                &config.condor.sequence_config.scene_concatenator.method
            else {
                panic!("FFmpeg")
            };
            let tracks = ffmpeg.tracks.as_ref().expect("selection created, not silently empty");
            assert_eq!(tracks.len(), 3);
            assert!(!tracks[&7].copy());
            assert!(tracks[&11].copy());
            assert!(tracks[&15].copy());
            return;
        }

        let error = result.expect_err("failed identification must not become an empty source");
        let message = error.to_string();
        assert!(message.contains(path.to_str().expect("path")), "{message}");
        match expected.as_str() {
            "parse" => {
                let TrackProbeError::MkvmergeIdentifyFailed {
                    path: actual,
                    source,
                    status,
                    stdout,
                    stderr,
                } = &error
                else {
                    panic!("{error:?}")
                };
                assert_eq!(actual, &path);
                assert_eq!(status.code(), Some(0));
                assert!(!stdout.is_empty());
                assert_eq!(stderr, "native diagnostic");
                assert!(source.is_data() || source.is_syntax() || source.is_eof());
                assert!(std::error::Error::source(&error).is_some());
            },
            "io" => assert!(
                matches!(error, TrackProbeError::MkvmergeNotRunnable { ref path, ref source } if path == &video.path && source.kind() == io::ErrorKind::NotFound)
            ),
            "ffprobe-parse" => assert!(matches!(
                error,
                TrackProbeError::FfprobeOutputUnreadable { .. }
            )),
            "semantic-error" | "container" | "process" => {
                let TrackProbeError::MkvmergeRejected {
                    path: actual,
                    status,
                    reason,
                    stdout,
                    stderr,
                } = &error
                else {
                    panic!("{error:?}")
                };
                assert_eq!(actual, &path);
                assert!(!stdout.is_empty());
                assert_eq!(stderr, "native diagnostic");
                assert!(
                    std::error::Error::source(&error).is_none(),
                    "no fabricated serde cause"
                );
                match expected.as_str() {
                    "semantic-error" => {
                        assert!(matches!(status.code(), Some(0 | 1)));
                        assert_eq!(reason, "native identify error");
                    },
                    "container" => {
                        assert!(matches!(status.code(), Some(0 | 1)));
                        assert!(
                            reason.contains("recognized=") && reason.contains("supported=false")
                        );
                    },
                    "process" => {
                        assert!(matches!(status.code(), Some(2) | None));
                        assert!(
                            message.contains(&status.to_string())
                                && message.contains("native diagnostic"),
                            "{message}"
                        );
                    },
                    _ => unreachable!(),
                }
            },
            _ => panic!("unknown child case {expected}"),
        }
        let before = serde_json::to_vec(&config).expect("snapshot");
        apply_discard(
            &mut config,
            &[DiscardCategory::Audio, DiscardCategory::Chapters, DiscardCategory::Metadata],
        )
        .expect_err("discard must stop before any mutation");
        assert_eq!(serde_json::to_vec(&config).expect("after"), before);
        assert_eq!(
            std::fs::read(&path).expect("source unchanged"),
            b"source sentinel"
        );
        assert_eq!(
            std::fs::read(&output).expect("output unchanged"),
            b"output sentinel"
        );
    }

    #[test]
    fn native_y4m_rejects_mkvmerge_and_uses_ffmpeg() {
        let temp = tempfile::tempdir().expect("native fixture");
        let path = temp.path().join("one-frame.y4m");
        let mut data = b"YUV4MPEG2 W2 H2 F24:1 Ip A1:1 C420jpeg\nFRAME\n".to_vec();
        data.extend([16, 16, 16, 16, 128, 128]);
        std::fs::write(&path, data).expect("tiny finite Y4M");
        let native = std::process::Command::new("mkvmerge")
            .args(["--identify", "--identification-format", "json"])
            .arg(&path)
            .output()
            .expect("installed mkvmerge is required");
        eprintln!(
            "track probe native Y4M: {} {}",
            native.status,
            String::from_utf8_lossy(&native.stdout)
        );
        let json: serde_json::Value = serde_json::from_slice(&native.stdout).expect("native JSON");
        assert_eq!(json["container"]["recognized"], false);
        assert_eq!(json["container"]["supported"], false);
        let error = probe(&path, false).expect_err("unrecognized is not a valid empty source");
        assert!(error.to_string().contains(path.to_str().expect("path")));
        let source = probe(&path, true).expect("native ffprobe fallback");
        assert_eq!(source.ffmpeg_indices(TrackKind::Video), [0]);
        assert!(source.mkvmerge_ids(TrackKind::Video).is_empty());
        assert_eq!(
            summarize(&source),
            "1 video, 0 audio, 0 subtitle, 0 attachment"
        );
    }

    #[test]
    fn native_multitrack_and_video_only_controls() {
        let path = crate::test_helpers::get_test_multitrack_video();
        let source = probe(&path, true).expect("native multitrack identification");
        assert_eq!(source.mkvmerge_ids(TrackKind::Video), [0]);
        assert_eq!(source.mkvmerge_ids(TrackKind::Audio), [1, 2]);
        assert_eq!(source.mkvmerge_ids(TrackKind::Subtitle), [3]);
        assert_eq!(source.mkvmerge_ids(TrackKind::Attachment), [1]);
        assert_eq!(source.ffmpeg_indices(TrackKind::Audio), [1, 2]);
        assert_eq!(source.ffmpeg_indices(TrackKind::Subtitle), [3]);
        assert_eq!(source.ffmpeg_indices(TrackKind::Attachment), [4]);
        let video = crate::test_helpers::get_test_video();
        let source = probe(&video.path, true).expect("native video-only identification");
        assert_eq!(source.mkvmerge_ids(TrackKind::Video), [0]);
        assert_eq!(
            summarize(&source),
            "1 video, 0 audio, 0 subtitle, 0 attachment"
        );
    }

    fn mkvmerge_identify_json() -> &'static str {
        r#"{
          "container": {"recognized": true, "supported": true},
          "errors": [],
          "attachments": [
            {"content_type": "font/ttf", "file_name": "font.ttf", "id": 1}
          ],
          "chapters": [],
          "global_tags": [{"num_entries": 1}],
          "identification_format_version": 20,
          "tracks": [
            {"codec": "AVC/H.264/MPEG-4p10", "id": 0, "type": "video"},
            {"codec": "AAC", "id": 1, "type": "audio"},
            {"codec": "SubRip/SRT", "id": 2, "type": "subtitles"},
            {"codec": "AAC", "id": 3, "type": "audio"}
          ]
        }"#
    }

    fn ffprobe_json() -> &'static str {
        r#"{
          "streams": [
            {"index": 0, "codec_type": "video"},
            {"index": 1, "codec_type": "audio"},
            {"index": 2, "codec_type": "subtitle"},
            {"index": 3, "codec_type": "attachment"},
            {"index": 4, "codec_type": "audio"},
            {"index": 5, "codec_type": "data"}
          ]
        }"#
    }

    fn probed() -> ProbedSource {
        let identified: MkvmergeIdentify =
            serde_json::from_str(mkvmerge_identify_json()).expect("identification parses");
        let FfprobeOutput { streams } =
            serde_json::from_str(ffprobe_json()).expect("ffprobe output parses");
        let mut tracks = identified.tracks();
        match_tracks_by_position(&mut tracks, &streams);
        ProbedSource { tracks }
    }

    /// mkvmerge's IDs come from its own list, attachments included.
    #[test]
    fn mkvmerge_ids_are_grouped_by_kind() {
        let source = probed();
        assert_eq!(source.mkvmerge_ids(TrackKind::Video), vec![0]);
        assert_eq!(source.mkvmerge_ids(TrackKind::Audio), vec![1, 3]);
        assert_eq!(source.mkvmerge_ids(TrackKind::Subtitle), vec![2]);
        assert_eq!(source.mkvmerge_ids(TrackKind::Attachment), vec![1]);
    }

    /// FFmpeg stream indices are matched by position within each type, so the
    /// attachment (index 3) does not shift the second audio stream (index 4).
    #[test]
    fn ffmpeg_indices_match_each_kind_without_attachments_shifting_audio() {
        let source = probed();
        assert_eq!(source.ffmpeg_indices(TrackKind::Video), vec![0]);
        assert_eq!(source.ffmpeg_indices(TrackKind::Audio), vec![1, 4]);
        assert_eq!(source.ffmpeg_indices(TrackKind::Subtitle), vec![2]);
        assert_eq!(source.ffmpeg_indices(TrackKind::Attachment), vec![3]);
    }

    /// Data streams are not selectable, so they never reach the probed list.
    #[test]
    fn data_streams_are_ignored() {
        let source = probed();
        assert!(
            source.tracks.iter().all(|track| track.ffmpeg_index != Some(5)),
            "the data stream is not probed"
        );
    }

    /// mkvmerge reports subtitle tracks as `subtitles` and its button tracks
    /// are treated as video.
    #[test]
    fn mkvmerge_subtitles_map_to_subtitle_and_button_tracks_to_video() {
        assert_eq!(
            MkvmergeTrackInfo {
                id: 1,
                kind: "subtitles".to_owned(),
            }
            .kind(),
            TrackKind::Subtitle
        );
        assert_eq!(
            MkvmergeTrackInfo {
                id: 2,
                kind: "button".to_owned(),
            }
            .kind(),
            TrackKind::Video
        );
    }

    /// Only some of the streams may have been probed by FFmpeg.
    #[test]
    fn an_unmatched_stream_leaves_the_index_unset() {
        let identified: MkvmergeIdentify =
            serde_json::from_str(mkvmerge_identify_json()).expect("identification parses");
        let mut tracks = identified.tracks();
        let FfprobeOutput { streams } = serde_json::from_str(
            r#"{"streams":[{"index":0,"codec_type":"video"},{"index":1,"codec_type":"audio"}]}"#,
        )
        .expect("ffprobe output parses");
        match_tracks_by_position(&mut tracks, &streams);
        let source = ProbedSource { tracks };
        assert_eq!(source.ffmpeg_indices(TrackKind::Audio), vec![1]);
        assert_eq!(
            source.ffmpeg_indices(TrackKind::Subtitle),
            Vec::<u32>::new()
        );
    }

    /// The summary counts every kind, so a log line can say what was found.
    #[test]
    fn summarize_counts_each_kind() {
        assert_eq!(
            summarize(&probed()),
            "1 video, 2 audio, 1 subtitle, 1 attachment"
        );
    }

    /// Without mkvmerge the FFmpeg streams alone still describe the source,
    /// since the FFmpeg settings are keyed by stream index.
    #[test]
    fn ffprobe_only_source_preserves_kind_indices_without_inventing_mkvmerge_ids() {
        let FfprobeOutput { streams } =
            serde_json::from_str(ffprobe_json()).expect("ffprobe output parses");
        let source = ProbedSource {
            tracks: tracks_from_streams(&streams),
        };
        assert_eq!(source.ffmpeg_indices(TrackKind::Video), vec![0]);
        assert_eq!(source.ffmpeg_indices(TrackKind::Audio), vec![1, 4]);
        assert_eq!(source.ffmpeg_indices(TrackKind::Subtitle), vec![2]);
        assert_eq!(source.ffmpeg_indices(TrackKind::Attachment), vec![3]);
        assert!(
            source.tracks.iter().all(|track| track.mkvmerge_id.is_none()),
            "no mkvmerge identification is available"
        );
        assert_eq!(
            summarize(&source),
            "1 video, 2 audio, 1 subtitle, 1 attachment"
        );
    }
}
