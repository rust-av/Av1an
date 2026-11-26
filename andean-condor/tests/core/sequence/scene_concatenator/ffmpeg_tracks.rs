use super::*;
use av_format::rational::Ratio;
use serde_json::{Value, json};

fn run(command: &mut Command) -> Vec<u8> {
    let output = command.output().expect("required native fixture tool");
    assert!(
        output.status.success(),
        "{command:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn probe(path: &Path) -> Value {
    serde_json::from_slice(&run(Command::new("ffprobe")
        .args(["-v", "error", "-show_streams", "-of", "json"])
        .arg(path)))
    .expect("native stream JSON")
}

fn hashes(path: &Path, selector: &str) -> Vec<Value> {
    let data: Value = serde_json::from_slice(&run(Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            selector,
            "-show_packets",
            "-show_data_hash",
            "sha256",
            "-of",
            "json",
        ])
        .arg(path)))
    .expect("packet JSON");
    let hashes = data["packets"]
        .as_array()
        .expect("packets")
        .iter()
        .map(|packet| packet["data_hash"].clone())
        .collect::<Vec<_>>();
    assert!(!hashes.is_empty());
    hashes
}

fn rms(path: &Path, selector: &str) -> f64 {
    let data = run(
        Command::new("ffmpeg").args(["-v", "error", "-i"]).arg(path).args([
            "-map",
            selector,
            "-f",
            "f32le",
            "-c:a",
            "pcm_f32le",
            "-",
        ]),
    );
    assert!(!data.is_empty());
    (data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|sample| f64::from(f32::from_le_bytes(*sample)).powi(2))
        .sum::<f64>()
        / (data.len() / 4) as f64)
        .sqrt()
}

fn fixture(root: &Path, attachment: bool) -> (PathBuf, PathBuf) {
    let subtitle = root.join("sub 'quoted'.srt");
    std::fs::write(
        &subtitle,
        "1\n00:00:00,000 --> 00:00:00,200\nX05 subtitle\n",
    )
    .expect("subtitle");
    let font = root.join("old 'font'.ttf");
    std::fs::write(&font, b"ffmpeg-tracks copied font bytes").expect("font");
    let source = root.join("source 'quoted'.mkv");
    let mut command = Command::new("ffmpeg");
    command
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=size=32x16:rate=24",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=880:sample_rate=48000",
            "-i",
        ])
        .arg(&subtitle)
        .args([
            "-map",
            "0:v",
            "-map",
            "1:a",
            "-map",
            "2:a",
            "-map",
            "3:s",
            "-map",
            "3:s",
            "-t",
            "0.25",
            "-c:v",
            "ffv1",
            "-c:a",
            "pcm_s16le",
            "-c:s",
            "srt",
            "-metadata:s:a:0",
            "language=eng",
            "-metadata:s:a:1",
            "language=jpn",
            "-metadata:s:s:0",
            "language=eng",
            "-metadata:s:s:1",
            "language=jpn",
            "-metadata:s:s:1",
            "title=second subtitle",
        ]);
    if attachment {
        command.arg("-attach").arg(&font).args([
            "-metadata:s:t:0",
            "mimetype=application/x-truetype-font",
            "-metadata:s:t:0",
            "filename=original font.ttf",
            "-metadata:s:t:0",
            "comment=original description",
        ]);
    }
    run(command.arg(&source));
    let chunk = root.join("encoded 'video'.mkv");
    run(Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&source)
        .args(["-map", "0:v:0", "-c", "copy"])
        .arg(&chunk));
    let scratch = scratch_directory(root);
    std::fs::create_dir_all(&scratch).expect("scratch");
    std::fs::write(scratch.join("source.mkv"), b"scratch sentinel").expect("sentinel");
    (source, chunk)
}

fn mux(
    root: &Path,
    source: &Path,
    chunk: &Path,
    output: &Path,
    config: &FfmpegConfig,
) -> Result<()> {
    let (tx, _rx) = sync::mpsc::channel();
    SceneConcatenator::ffmpeg(
        root,
        output,
        &[chunk.to_owned()],
        Some(source),
        6,
        Ratio::new(24, 1),
        Some(config),
        &tx,
        &Cancellation::default(),
    )
}

#[test]
fn ffmpeg_mux_applies_extra_audio_filters_after_all_retained_audio_tracks() {
    // Wildcard maps must count both original audios before configuring the extra.
    let dir = tempfile::tempdir().expect("fixture");
    let (source, chunk) = fixture(dir.path(), false);
    let original = std::fs::read(&source).expect("source bytes");
    let extra = dir.path().join("extra 'audio'.mka");
    run(Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=1320:sample_rate=48000",
            "-t",
            "0.25",
            "-c:a",
            "pcm_s16le",
        ])
        .arg(&extra));
    let extra_bytes = std::fs::read(&extra).expect("extra bytes");
    let mut failures = Vec::new();
    for (case, tracks, kept) in [
        ("copy-all", Value::Null, vec![0, 1]),
        (
            "drop-first",
            json!({"1":{"type":"audio","copy":false},"2":{"type":"audio"},
            "3":{"type":"subtitle","copy":false},"4":{"type":"subtitle"}}),
            vec![1],
        ),
    ] {
        let config = serde_json::from_value(json!({"tracks":tracks,"extra_inputs":[{"path":extra,
            "tracks":{"0":{"type":"audio","codec":"aac","filter":"volume=0.25"}}}]}))
        .expect("config");
        let output = dir.path().join(format!("{case}.mkv"));
        mux(dir.path(), &source, &chunk, &output, &config).expect("native audio mux");
        let data = probe(&output);
        let streams = data["streams"].as_array().expect("streams");
        let audio = streams
            .iter()
            .filter(|stream| stream["codec_type"] == "audio")
            .collect::<Vec<_>>();
        assert_eq!(audio.len(), kept.len() + 1);
        let filtered = rms(&output, &format!("0:a:{}", kept.len())) / rms(&extra, "0:a:0");
        let copies = kept.iter().enumerate().all(|(out, original)| {
            audio[out]["codec_name"] == "pcm_s16le"
                && hashes(&output, &format!("a:{out}")) == hashes(&source, &format!("a:{original}"))
                && audio[out]["tags"]["language"] == if *original == 0 { "eng" } else { "jpn" }
        });
        let correct =
            copies && audio[kept.len()]["codec_name"] == "aac" && (0.20..0.30).contains(&filtered);
        eprintln!(
            "ffmpeg-tracks native {case}: copied packet identity={copies}, codecs={:?}, extra RMS ratio={filtered}",
            audio.iter().map(|stream| &stream["codec_name"]).collect::<Vec<_>>()
        );
        if !correct {
            failures.push(format!("{case}: wrong copied audio/extra codec or actual filter effect (RMS ratio={filtered})"));
        }
        let subtitles = streams
            .iter()
            .filter(|stream| stream["codec_type"] == "subtitle")
            .collect::<Vec<_>>();
        assert_eq!(subtitles.len(), kept.len());
        assert_eq!(
            subtitles.last().expect("subtitle")["tags"]["language"],
            "jpn"
        );
        assert_eq!(
            subtitles.last().expect("subtitle")["tags"]["title"],
            "second subtitle"
        );
    }
    assert_eq!(std::fs::read(&source).expect("source intact"), original);
    assert_eq!(std::fs::read(extra).expect("extra intact"), extra_bytes);
    assert_eq!(
        std::fs::read(scratch_directory(dir.path()).join("source.mkv")).expect("sentinel"),
        b"scratch sentinel"
    );
    dir.close().expect("native fixture cleanup");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn ffmpeg_mux_preserves_retained_attachment_bytes_and_metadata_when_adding_or_dropping_tracks() {
    // Counting only new attachments overwrites the copied font's name/MIME/comment.
    let dir = tempfile::tempdir().expect("fixture");
    let (source, chunk) = fixture(dir.path(), true);
    let original = std::fs::read(&source).expect("source bytes");
    let new_font = dir.path().join("new 'font'.ttf");
    std::fs::write(&new_font, b"ffmpeg-tracks distinct new font bytes").expect("new font");
    let mut failures = Vec::new();
    for (case, tracks, copy_new) in [
        ("copy-all", Value::Null, true),
        (
            "explicit",
            json!({"5":{"type":"attachment"},"3":{"type":"subtitle","copy":false},"4":{"type":"subtitle","codec":"ass"}}),
            true,
        ),
        ("attachment-only", json!({"5":{"type":"attachment"}}), true),
        ("drop-new", json!({"5":{"type":"attachment"}}), false),
    ] {
        let config = serde_json::from_value(json!({"tracks":tracks,"extra_inputs":[{"path":source,
            "tracks":{"99":{"type":"attachment","copy":copy_new,"path":new_font,
                "attachment_name":"new font.ttf","attachment_mime_type":"font/ttf",
                "attachment_description":"new description"}}}]}))
        .expect("config");
        let output = dir.path().join(format!("{case}.mkv"));
        if let Err(error) = mux(dir.path(), &source, &chunk, &output, &config) {
            failures.push(format!("{case}: native mux: {error:#}"));
            continue;
        }
        let data = probe(&output);
        let streams = data["streams"].as_array().expect("streams");
        let attachments = streams
            .iter()
            .filter(|stream| stream["codec_type"] == "attachment")
            .collect::<Vec<_>>();
        let count = 1 + usize::from(copy_new);
        if attachments.len() != count {
            failures.push(format!(
                "{case}: expected {count} attachments: {attachments:?}"
            ));
        }
        for (index, (bytes, name, mime, comment)) in [
            (
                b"ffmpeg-tracks copied font bytes".as_slice(),
                "original font.ttf",
                "application/x-truetype-font",
                "original description",
            ),
            (
                b"ffmpeg-tracks distinct new font bytes".as_slice(),
                "new font.ttf",
                "font/ttf",
                "new description",
            ),
        ]
        .into_iter()
        .take(count)
        .enumerate()
        {
            let Some(attachment) = attachments.get(index) else {
                continue;
            };
            let tags = &attachment["tags"];
            if tags["filename"] != name || tags["mimetype"] != mime || tags["COMMENT"] != comment {
                failures.push(format!(
                    "{case} t:{index}: wrong attachment metadata {tags}"
                ));
            }
            let extracted = dir.path().join(format!("{case}-{index}.ttf"));
            run(Command::new("ffmpeg")
                .args(["-v", "error", &format!("-dump_attachment:t:{index}")])
                .arg(&extracted)
                .arg("-i")
                .arg(&output)
                .args(["-map", "0:v:0", "-c", "copy", "-f", "null", "-"]));
            assert_eq!(
                std::fs::read(extracted).expect("extracted bytes"),
                bytes,
                "{case} t:{index}"
            );
        }
        if case == "explicit" {
            let subtitles = streams
                .iter()
                .filter(|stream| stream["codec_type"] == "subtitle")
                .collect::<Vec<_>>();
            assert_eq!(subtitles.len(), 1);
            assert_eq!(subtitles[0]["codec_name"], "ass");
            assert_eq!(subtitles[0]["tags"]["language"], "jpn");
        }
        eprintln!(
            "ffmpeg-tracks native {case}: {} attachment streams, extracted bytes and distinct metadata checked",
            attachments.len()
        );
    }
    assert_eq!(std::fs::read(source).expect("source intact"), original);
    assert_eq!(
        std::fs::read(new_font).expect("new font intact"),
        b"ffmpeg-tracks distinct new font bytes"
    );
    assert_eq!(
        std::fs::read(scratch_directory(dir.path()).join("source.mkv")).expect("sentinel"),
        b"scratch sentinel"
    );
    dir.close().expect("native fixture cleanup");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn ffmpeg_mux_validates_selected_sources_before_mutation_but_does_not_probe_unused_sources() {
    let dir = tempfile::tempdir().expect("fixture");
    let (source, chunk) = fixture(dir.path(), false);
    let original = std::fs::read(&source).expect("source bytes");
    let concat = scratch_directory(dir.path()).join("concat.txt");
    let output = dir.path().join("output.mkv");
    for (tracks, expected) in [
        (
            json!({"777":{"type":"audio"}}),
            "stream 777: missing source stream",
        ),
        (
            json!({"1":{"type":"subtitle"}}),
            "stream 1: configured subtitle, probed audio",
        ),
    ] {
        std::fs::write(&concat, b"concat sentinel").expect("concat sentinel");
        std::fs::write(&output, b"output sentinel").expect("output sentinel");
        let config = serde_json::from_value(json!({"tracks":tracks})).expect("config");
        let error = mux(dir.path(), &source, &chunk, &output, &config)
            .expect_err("owning preflight refusal");
        let text = format!("{error:#}");
        assert!(
            text.contains(&source.display().to_string()) && text.contains(expected),
            "{text}"
        );
        assert_eq!(
            std::fs::read(&output).expect("output intact"),
            b"output sentinel"
        );
        assert_eq!(
            std::fs::read(&concat).expect("concat intact"),
            b"concat sentinel"
        );
    }
    // No config must never open or probe this nonexistent, unused source.
    let (tx, _rx) = sync::mpsc::channel();
    SceneConcatenator::ffmpeg(
        dir.path(),
        &output,
        &[chunk],
        Some(&dir.path().join("not a source")),
        6,
        Ratio::new(24, 1),
        None,
        &tx,
        &Cancellation::default(),
    )
    .expect("video-only ignores root");
    let data = probe(&output);
    assert_eq!(data["streams"].as_array().expect("streams").len(), 1);
    assert_eq!(data["streams"][0]["codec_type"], "video");
    assert_eq!(std::fs::read(source).expect("source intact"), original);
    dir.close().expect("fixture cleanup");
}

#[cfg(unix)]
#[test]
fn ffmpeg_source_probe_rejects_malformed_duplicate_or_mistyped_stream_reports() {
    probe_failure_cases(
        "ffmpeg_source_probe_rejects_malformed_duplicate_or_mistyped_stream_reports",
        &["malformed", "duplicate", "bad-type"],
    );
}

#[cfg(unix)]
#[test]
fn ffmpeg_source_probe_rejects_oversized_stdout_or_stderr_without_mutation() {
    probe_failure_cases(
        "ffmpeg_source_probe_rejects_oversized_stdout_or_stderr_without_mutation",
        &["stdout-overflow", "stderr-overflow"],
    );
}

#[cfg(unix)]
#[test]
fn forced_ffmpeg_source_probe_preserves_output_and_concat_scratch() {
    probe_failure_cases(
        "forced_ffmpeg_source_probe_preserves_output_and_concat_scratch",
        &["force"],
    );
}

#[cfg(unix)]
fn probe_failure_cases(test: &str, cases: &[&str]) {
    use std::os::unix::fs::PermissionsExt;
    if std::env::var_os("FFMPEG_TRACKS_ROOT").is_some() {
        let case = std::env::var("FFMPEG_TRACKS_CASE").expect("case");
        assert!(
            cases.contains(&case.as_str()),
            "case belongs to this contract"
        );
        assert_failed_source_probe_preserves_files();
        return;
    }
    let python = which::which("python3").expect("required Python fixture tool");
    let module = module_path!().split_once("::").expect("test crate prefix").1;
    let exact = format!("{module}::{test}");
    for case in cases {
        let dir = tempfile::tempdir().expect("isolated probe");
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&bin).expect("private PATH");
        let script = format!(
            r#"#!{}
import os,sys,json,subprocess,time
from pathlib import Path
r=Path(os.environ['FFMPEG_TRACKS_ROOT']); case=os.environ['FFMPEG_TRACKS_CASE']
if case=='force':
 p=subprocess.Popen([sys.executable,'-c','import time; time.sleep(30)'])
 (r/'pids').write_text(str(os.getpid())+' '+str(p.pid))
 (r/'active').write_text('ready')
 time.sleep(30)
elif case=='malformed': print('{{')
elif case=='stdout-overflow': sys.stdout.write('x'*(4*1024*1024+1))
elif case=='stderr-overflow': sys.stderr.write('x'*(64*1024+1))
else:
 streams=[dict(index=2,codec_type='audio')]*2 if case=='duplicate' else [dict(index=2,codec_type='')]
 print(json.dumps(dict(streams=streams,format={{}},chapters=[])))
"#,
            python.display()
        );
        let path = bin.join("ffprobe");
        std::fs::write(&path, script).expect("probe fixture");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).expect("executable");
        let result = Command::new(std::env::current_exe().expect("test binary"))
            .args(["--exact", &exact, "--nocapture"])
            .env("FFMPEG_TRACKS_ROOT", dir.path())
            .env("FFMPEG_TRACKS_CASE", case)
            .env("PATH", bin)
            .output()
            .expect("isolated owning preflight");
        assert!(
            result.status.success()
                && String::from_utf8_lossy(&result.stdout).contains("1 passed; 0 failed"),
            "{case}: {}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        eprintln!(
            "ffmpeg-tracks isolated {case}: {}",
            String::from_utf8_lossy(&result.stdout)
        );
        dir.close().expect("isolated fixture cleanup");
    }
}

#[cfg(unix)]
fn assert_failed_source_probe_preserves_files() {
    let root = PathBuf::from(std::env::var_os("FFMPEG_TRACKS_ROOT").expect("child root"));
    let case = std::env::var("FFMPEG_TRACKS_CASE").expect("case");
    let source = root.join("source 'quoted'.mkv");
    let scene = root.join("scene.mkv");
    let output = root.join("output.mkv");
    let scratch = scratch_directory(&root);
    std::fs::create_dir_all(&scratch).expect("scratch");
    let concat = scratch.join("concat.txt");
    for (path, bytes) in [
        (&source, b"source sentinel".as_slice()),
        (&output, b"output sentinel"),
        (&concat, b"concat sentinel"),
        (&scene, b"owned scene fixture"),
    ] {
        std::fs::write(path, bytes).expect("sentinel");
    }
    let cancellation = Cancellation::default();
    let controller = (case == "force").then(|| {
        let root = root.clone();
        let cancellation = cancellation.clone();
        thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !root.join("active").exists() && std::time::Instant::now() < deadline {
                thread::yield_now();
            }
            let active = root.join("active").exists();
            cancellation.request().expect("graceful");
            cancellation.request().expect("force");
            assert!(active, "owned probe handshake");
        })
    });
    let (tx, _rx) = sync::mpsc::channel();
    let result = SceneConcatenator::ffmpeg(
        &root,
        &output,
        std::slice::from_ref(&scene),
        Some(&source),
        6,
        Ratio::new(24, 1),
        Some(&FfmpegConfig::default()),
        &tx,
        &cancellation,
    );
    if let Some(controller) = controller {
        controller.join().expect("force controller joined");
    }
    let error = result.expect_err("source preflight must fail before concat scratch/output writes");
    if case == "force" {
        assert_eq!(
            error.downcast_ref::<andean_condor::utils::cancellation::Cancelled>(),
            Some(&andean_condor::utils::cancellation::Cancelled::Forced)
        );
    } else {
        let text = format!("{error:#}");
        let expected = match case.as_str() {
            "malformed" => "Malformed ffprobe source report",
            "duplicate" => "stream 2: duplicate probe index",
            "bad-type" => "stream 2: malformed probe type",
            _ => "capture exceeded",
        };
        assert!(
            text.contains(expected) && text.contains(&source.display().to_string()),
            "{text}"
        );
    }
    assert_eq!(
        std::fs::read(source).expect("source intact"),
        b"source sentinel"
    );
    assert_eq!(
        std::fs::read(output).expect("output intact"),
        b"output sentinel"
    );
    assert_eq!(
        std::fs::read(concat).expect("concat intact"),
        b"concat sentinel"
    );
    assert_eq!(
        std::fs::read(scene).expect("scene intact"),
        b"owned scene fixture"
    );
    #[cfg(target_os = "linux")]
    if root.join("pids").exists() {
        for pid in std::fs::read_to_string(root.join("pids"))
            .expect("owned PIDs")
            .split_whitespace()
        {
            wait_for_owned_process_exit(pid);
        }
    }
    println!(
        "{case}: owning FFmpeg preflight error/cancel, bounded joined capture, source/output/concat sentinels checked"
    );
}

#[test]
fn native_mov_timecode_is_omitted_and_audio_selection_is_preserved() {
    fn run(command: &mut Command) -> Vec<u8> {
        let output = command.output().expect("required native fixture tool");
        assert!(
            output.status.success(),
            "{command:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }
    fn probe(path: &Path) -> serde_json::Value {
        serde_json::from_slice(&run(Command::new("ffprobe")
            .args(["-v", "error", "-count_frames", "-show_streams", "-show_format", "-of", "json"])
            .arg(path)))
        .expect("native stream JSON")
    }
    let audio_hashes = |path: &Path, index: &str| {
        let data: serde_json::Value = serde_json::from_slice(&run(Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-select_streams",
                index,
                "-show_packets",
                "-show_data_hash",
                "sha256",
                "-of",
                "json",
            ])
            .arg(path)))
        .expect("native packet hashes");
        let hashes = data["packets"]
            .as_array()
            .expect("audio packets")
            .iter()
            .map(|packet| packet["data_hash"].as_str().expect("packet hash").to_owned())
            .collect::<Vec<_>>();
        assert!(!hashes.is_empty(), "fixture has audio packets");
        hashes
    };
    let dir = tempfile::tempdir().expect("fixture");
    let source = dir.path().join("source 'timecode'.mov");
    run(Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=size=32x16:rate=24",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=880:sample_rate=48000",
            "-map",
            "0:v",
            "-map",
            "1:a",
            "-map",
            "2:a",
            "-frames:v",
            "4",
            "-t",
            "0.2",
            "-c:v",
            "mpeg4",
            "-c:a",
            "pcm_s16le",
            "-timecode",
            "01:00:00:00",
            "-metadata:s:a:0",
            "language=eng",
            "-metadata:s:a:1",
            "language=jpn",
        ])
        .arg(&source));
    let original = std::fs::read(&source).expect("source bytes");
    let data = probe(&source);
    let streams = data["streams"].as_array().expect("source streams");
    assert_eq!(streams.len(), 4);
    assert_eq!(streams[0]["codec_type"], "video");
    assert_eq!(streams[1]["codec_type"], "audio");
    assert_eq!(streams[2]["codec_type"], "audio");
    assert_eq!(streams[3]["codec_type"], "data");
    assert_eq!(streams[3]["codec_tag_string"], "tmcd");
    eprintln!("ffmpeg-mapping source: video, eng audio, jpn audio, tmcd data verified by ffprobe");
    let chunk = dir.path().join("chunk 'video'.mkv");
    run(Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&source)
        .args(["-map", "0:v:0", "-c", "copy"])
        .arg(&chunk));
    let scratch = scratch_directory(dir.path());
    std::fs::create_dir_all(&scratch).expect("scratch");
    let sentinel = scratch.join("source.mkv");
    std::fs::write(&sentinel, b"caller scratch sentinel").expect("sentinel");
    let selected: FfmpegConfig = serde_json::from_str(
        r#"{"tracks":{"1":{"type":"audio","copy":false},"2":{"type":"audio"}}}"#,
    )
    .expect("explicit audio selection");
    let (tx, _rx) = sync::mpsc::channel();
    let mut failures = Vec::new();
    for (case, config, languages) in [
        ("copy-all", FfmpegConfig::default(), vec!["eng", "jpn"]),
        ("selected", selected, vec!["jpn"]),
    ] {
        let output = dir.path().join(format!("{case}.mkv"));
        let result = SceneConcatenator::ffmpeg(
            dir.path(),
            &output,
            std::slice::from_ref(&chunk),
            Some(&source),
            4,
            Ratio::new(24, 1),
            Some(&config),
            &tx,
            &Cancellation::default(),
        );
        if let Err(error) = result {
            failures.push(format!("{case}: {error:#}"));
            continue;
        }
        let data = probe(&output);
        assert!(data["format"]["format_name"].as_str().expect("format").contains("matroska"));
        let streams = data["streams"].as_array().expect("output streams");
        assert_eq!(
            streams.len(),
            1 + languages.len(),
            "{case}: no data or other streams"
        );
        let video = streams
            .iter()
            .filter(|stream| stream["codec_type"] == "video")
            .collect::<Vec<_>>();
        assert_eq!(video.len(), 1, "{case}: encoded video preserved");
        assert_eq!(video[0]["nb_read_frames"], "4");
        let audio = streams
            .iter()
            .filter(|stream| stream["codec_type"] == "audio")
            .collect::<Vec<_>>();
        assert_eq!(audio.len(), languages.len());
        for (index, (stream, language)) in audio.iter().zip(&languages).enumerate() {
            assert_eq!(stream["tags"]["language"], *language);
            let source_index = if *language == "eng" { "a:0" } else { "a:1" };
            assert_eq!(
                audio_hashes(&output, &format!("a:{index}")),
                audio_hashes(&source, source_index),
                "{case}: selected audio bytes preserved"
            );
        }
        assert!(!streams.iter().any(|stream| stream["codec_type"] == "data"));
        eprintln!(
            "ffmpeg-mapping {case}: native MKV, four video frames, {languages:?} audio packet hashes preserved, no data"
        );
    }
    assert_eq!(std::fs::read(&source).expect("source intact"), original);
    assert_eq!(
        std::fs::read(&sentinel).expect("scratch intact"),
        b"caller scratch sentinel"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    dir.close().expect("fixture and backend scratch cleaned");
}

#[cfg(target_os = "linux")]
fn wait_for_owned_process_exit(pid: &str) {
    let pid: u32 = pid.parse().expect("owned PID");
    assert!(pid > 0);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(stat) => {
                let (identity, fields) = stat.rsplit_once(')').expect("valid process stat");
                assert!(
                    identity.starts_with(&format!("{pid} (")),
                    "stat PID identity"
                );
                let state = fields.split_whitespace().next().expect("process state");
                assert!(
                    matches!(
                        state,
                        "R" | "S" | "D" | "Z" | "T" | "t" | "X" | "x" | "K" | "W" | "P" | "I"
                    ),
                    "invalid process state: {state}"
                );
                if state == "Z" {
                    return;
                }
            },
            // procfs can lose the task after open but before read (ESRCH rather than ENOENT).
            Err(error) if matches!(error.raw_os_error(), Some(libc::ENOENT | libc::ESRCH)) => {
                return;
            },
            Err(error) => panic!("reading owned process {pid}: {error}"),
        }
        // Pipe EOF precedes exit_notify; allow the killed grandchild to become a zombie.
        assert!(
            std::time::Instant::now() < deadline,
            "owned process {pid} remains live"
        );
        thread::sleep(std::time::Duration::from_millis(10));
    }
}
