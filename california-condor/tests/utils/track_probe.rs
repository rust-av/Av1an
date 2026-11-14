use std::path::PathBuf;
#[cfg(unix)]
use std::{io, path::Path};

#[cfg(unix)]
use california_condor::utils::track_probe::TrackProbeError;
use california_condor::utils::track_probe::{TrackKind, probe, summarize};

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

#[cfg(unix)]
fn check_identification_admission(expected_modes: &[&str], test: &str) {
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
    if let Ok(case) = std::env::var("TRACK_PROBE_CASE") {
        let (_, _, _, expected) = cases.iter().find(|row| row.0 == case).expect("known probe case");
        assert!(
            expected_modes.contains(expected),
            "case belongs to this behavior"
        );
        assert_eq!(
            std::env::var("TRACK_PROBE_EXPECTED").expect("expected mode"),
            *expected
        );
        check_track_probe_result();
        println!("track probe {case}: assertions completed");
        return;
    }
    assert!(
        std::env::var_os("TRACK_PROBE_EXPECTED").is_none(),
        "mode without selected case"
    );
    let mut failures = Vec::new();
    for (case, json, status, expected) in cases {
        if !expected_modes.contains(&expected) {
            continue;
        }
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
            .args(["--exact", test, "--nocapture"])
            // The parent owns this cwd until the bounded child is reaped.
            .current_dir(directory.path())
            .env("PATH", directory.path())
            .env("TRACK_PROBE_CASE", case)
            .env("TRACK_PROBE_EXPECTED", expected)
            .env("TRACK_PROBE_CALLS", directory.path().join("calls"))
            .env("TRACK_PROBE_SOURCE", directory.path().join("source.mkv"))
            .timeout(std::time::Duration::from_secs(20))
            .assert();
        let output = assertion.get_output();
        let stdout = String::from_utf8_lossy(&output.stdout);
        if !output.status.success()
            || !stdout.contains("test result: ok. 1 passed; 0 failed;")
            || !stdout.contains(&format!("track probe {case}: assertions completed"))
        {
            failures.push(case);
            eprintln!(
                "track probe {case}: {} {}",
                stdout,
                String::from_utf8_lossy(&output.stderr)
            );
        }
        eprintln!("{stdout}");
        eprintln!("track probe {case}: {expected}");
    }
    assert!(
        failures.is_empty(),
        "track probe admission failures: {failures:?}"
    );
}

#[cfg(unix)]
#[test]
fn recognized_track_identification_preserves_native_ids_and_exact_identify_arguments() {
    check_identification_admission(
        &["accept"],
        "track_probe::recognized_track_identification_preserves_native_ids_and_exact_identify_arguments",
    );
}

#[cfg(unix)]
#[test]
fn empty_recognized_source_has_no_tracks() {
    check_identification_admission(
        &["empty"],
        "track_probe::empty_recognized_source_has_no_tracks",
    );
}

#[cfg(unix)]
#[test]
fn ffmpeg_fallback_builds_discard_selection_without_mkvmerge_ids() {
    check_identification_admission(
        &["fallback"],
        "track_probe::ffmpeg_fallback_builds_discard_selection_without_mkvmerge_ids",
    );
}

#[cfg(unix)]
#[test]
fn rejected_track_probe_preserves_typed_diagnostics_configuration_and_files() {
    check_identification_admission(
        &["process", "semantic-error", "container", "parse", "io", "ffprobe-parse"],
        "track_probe::rejected_track_probe_preserves_typed_diagnostics_configuration_and_files",
    );
}

#[cfg(unix)]
fn check_track_probe_result() {
    use andean_condor::models::sequence::scene_concatenator::ConcatMethod;
    use california_condor::{
        commands::{DiscardCategory, handlers::concatenate::apply_discard},
        test_helpers::{TestVideo, default_config},
    };
    let expected = std::env::var("TRACK_PROBE_EXPECTED").expect("selected probe mode");
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
        let calls = std::fs::read_to_string(std::env::var_os("TRACK_PROBE_CALLS").expect("calls"))
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
        let ConcatMethod::FFmpeg(ffmpeg) = &config.condor.sequence_config.scene_concatenator.method
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
                    assert!(reason.contains("recognized=") && reason.contains("supported=false"));
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

fn native_video_fixture(multitrack: bool) -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().expect("owned native media");
    let video = temp.path().join("video.mkv");
    let status = std::process::Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=size=32x32:rate=24",
            "-frames:v",
            "2",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-threads",
            "1",
        ])
        .arg(&video)
        .status()
        .expect("finite video fixture");
    assert!(status.success(), "finite video generation failed");
    if !multitrack {
        return (temp, video);
    }
    let subtitle = temp.path().join("captions.srt");
    let attachment = temp.path().join("font.ttf");
    std::fs::write(&subtitle, "1\n00:00:00,000 --> 00:00:00,080\nCaption\n").expect("subtitle");
    std::fs::write(&attachment, b"condor test attachment").expect("attachment");
    let path = temp.path().join("multitrack.mkv");
    let status = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&video)
        .args([
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=0.1",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=880:duration=0.1",
            "-i",
        ])
        .arg(&subtitle)
        .arg("-attach")
        .arg(&attachment)
        .args([
            "-metadata:s:t:0",
            "mimetype=font/ttf",
            "-metadata:s:t:0",
            "filename=font.ttf",
            "-map",
            "0:v",
            "-map",
            "1:a",
            "-map",
            "2:a",
            "-map",
            "3:s",
            "-c:v",
            "copy",
            "-c:a",
            "aac",
            "-c:s",
            "srt",
        ])
        .arg(&path)
        .status()
        .expect("finite multitrack fixture");
    assert!(status.success(), "finite multitrack generation failed");
    (temp, path)
}

#[test]
fn native_multitrack_probe_maps_track_ids_and_ffmpeg_stream_indices() {
    let (_temp, path) = native_video_fixture(true);
    let source = probe(&path, true).expect("native multitrack identification");
    assert_eq!(source.mkvmerge_ids(TrackKind::Video), [0]);
    assert_eq!(source.mkvmerge_ids(TrackKind::Audio), [1, 2]);
    assert_eq!(source.mkvmerge_ids(TrackKind::Subtitle), [3]);
    assert_eq!(source.mkvmerge_ids(TrackKind::Attachment), [1]);
    assert_eq!(source.ffmpeg_indices(TrackKind::Audio), [1, 2]);
    assert_eq!(source.ffmpeg_indices(TrackKind::Subtitle), [3]);
    assert_eq!(source.ffmpeg_indices(TrackKind::Attachment), [4]);
}

#[test]
fn native_video_only_probe_reports_no_ancillary_tracks() {
    let (_temp, path) = native_video_fixture(false);
    let source = probe(&path, true).expect("native video-only identification");
    assert_eq!(source.mkvmerge_ids(TrackKind::Video), [0]);
    assert_eq!(
        summarize(&source),
        "1 video, 0 audio, 0 subtitle, 0 attachment"
    );
}
