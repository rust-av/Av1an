use super::*;
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
    let scratch = SceneConcatenator::scratch_directory(root);
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
    SceneConcatenator::ffmpeg_cancel(
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
fn native_extra_audio_filter_uses_mapped_output_index() {
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
        std::fs::read(SceneConcatenator::scratch_directory(dir.path()).join("source.mkv"))
            .expect("sentinel"),
        b"scratch sentinel"
    );
    dir.close().expect("native fixture cleanup");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn native_copied_and_new_attachment_identity_and_drops() {
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
        std::fs::read(SceneConcatenator::scratch_directory(dir.path()).join("source.mkv"))
            .expect("sentinel"),
        b"scratch sentinel"
    );
    dir.close().expect("native fixture cleanup");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn supplied(streams: &[(u32, &str)]) -> Vec<source::Stream> {
    serde_json::from_value(json!(
        streams
            .iter()
            .map(|(index, kind)| json!({"index":index,"codec_type":kind}))
            .collect::<Vec<_>>()
    ))
    .expect("supplied probe")
}

fn args(
    config: Value,
    input: Option<&Path>,
    streams: &[Vec<source::Stream>],
) -> Result<Vec<String>> {
    let config = serde_json::from_value(config)?;
    Ok(ffmpeg_args(
        Path::new("no filesystem concat"),
        Path::new("no filesystem output"),
        input,
        Some(&config),
        streams,
    )?
    .into_iter()
    .map(|arg| arg.to_string_lossy().into_owned())
    .collect())
}

fn maps(args: &[String]) -> Vec<&str> {
    args.windows(2)
        .filter(|pair| pair[0] == "-map")
        .map(|pair| pair[1].as_str())
        .collect()
}

#[test]
fn pure_copy_all_counts_concrete_streams_and_appends_new_attachments() {
    let config = json!({"extra_inputs":[{"path":"nonexistent extra", "tracks":{
        "9":{"type":"attachment"},
        "8":{"type":"subtitle","codec":"ass"},
        "7":{"type":"audio","codec":"aac","filter":"volume=0.25", "parameters":{"-b:a:17":"123k"}},
        "900":{"type":"attachment","path":"new font", "attachment_mime_type":"font/ttf",
            "attachment_name":"new name","attachment_description":"new comment"},
        "901":{"type":"attachment","copy":false,"path":"dropped font","attachment_mime_type":"font/ttf"}
    }}]});
    let streams = [
        supplied(&[
            (6, "attachment"),
            (4, "audio"),
            (99, "unknown"),
            (3, "subtitle"),
            (0, "video"),
            (2, "audio"),
            (5, "data"),
        ]),
        supplied(&[(9, "attachment"), (8, "subtitle"), (7, "audio")]),
    ];
    let argv = args(
        config.clone(),
        Some(Path::new("nonexistent root")),
        &streams,
    )
    .expect("pure plan");
    assert_eq!(
        maps(&argv),
        ["0", "1:2", "1:4", "1:3", "1:6", "2:7", "2:8", "2:9"]
    );
    for pair in [
        ["-c:a:2", "aac"],
        ["-filter:a:2", "volume=0.25"],
        ["-c:s:1", "ass"],
        ["-b:a:17", "123k"],
        ["-metadata:s:t:2", "mimetype=font/ttf"],
        ["-metadata:s:t:2", "filename=new name"],
        ["-metadata:s:t:2", "comment=new comment"],
        ["-attach", "new font"],
    ] {
        assert!(
            argv.windows(2).any(|actual| actual == pair),
            "missing {pair:?}: {argv:?}"
        );
    }
    assert!(!argv.iter().any(|arg| arg == "dropped font" || arg.contains("?")));
    assert_eq!(
        argv,
        args(config, Some(Path::new("nonexistent root")), &streams).expect("repeat pure plan")
    );
}

#[test]
fn pure_selected_drops_do_not_consume_type_local_indices() {
    let argv = args(
        json!({"tracks":{
            "700":{"type":"video","copy":false,"codec":"libx264"},
            "1":{"type":"audio","copy":false},"2":{"type":"audio","codec":"flac"},
            "3":{"type":"subtitle","copy":false},"4":{"type":"subtitle","codec":"ass"},
            "5":{"type":"attachment","copy":false},"6":{"type":"attachment"},
            "888":{"type":"attachment","path":"new font","attachment_mime_type":"font/ttf"}
        },"extra_inputs":[{"path":"extra", "tracks":{
            "0":{"type":"audio","codec":"aac","filter":"volume=0.25"},
            "1":{"type":"subtitle","codec":"srt"},"2":{"type":"attachment"}
        }}]}),
        Some(Path::new("root")),
        &[
            supplied(&[
                (1, "audio"),
                (2, "audio"),
                (3, "subtitle"),
                (4, "subtitle"),
                (5, "attachment"),
                (6, "attachment"),
            ]),
            supplied(&[(0, "audio"), (1, "subtitle"), (2, "attachment")]),
        ],
    )
    .expect("selected plan");
    assert_eq!(maps(&argv), ["1:2", "1:4", "1:6", "2:0", "2:1", "2:2"]);
    for pair in [
        ["-c:a:0", "flac"],
        ["-c:a:1", "aac"],
        ["-filter:a:1", "volume=0.25"],
        ["-c:s:0", "ass"],
        ["-c:s:1", "srt"],
        ["-metadata:s:t:2", "mimetype=font/ttf"],
    ] {
        assert!(
            argv.windows(2).any(|actual| actual == pair),
            "missing {pair:?}"
        );
    }
    assert!(!argv.iter().any(|arg| arg.starts_with("-c:v")));
}

#[test]
fn pure_extra_only_empty_selection_and_attachment_only_inputs() {
    for (input, config, streams, expected) in [
        (
            None,
            json!({"extra_inputs":[{"path":"only extra","tracks":{"4":{"type":"audio","codec":"aac"}}}]}),
            vec![supplied(&[(4, "audio")])],
            vec!["0", "1:4"],
        ),
        (
            Some(Path::new("empty root")),
            json!({"tracks":{},"extra_inputs":[{"path":"attachment only"}]}),
            vec![supplied(&[]), supplied(&[(12, "attachment")])],
            vec!["0", "2:12"],
        ),
        (
            None,
            json!({"tracks":{"999":{"type":"attachment","path":"new","attachment_mime_type":"font/ttf"}}}),
            vec![],
            vec!["0"],
        ),
    ] {
        let argv = args(config, input, &streams).expect("pure valid sparse inputs");
        assert_eq!(maps(&argv), expected);
        if expected.last() == Some(&"1:4") {
            assert!(argv.windows(2).any(|pair| pair == ["-c:a:0", "aac"]));
        }
    }
}

#[test]
fn pure_invalid_id_type_and_probe_identity_are_contextual_errors() {
    for (tracks, streams, expected) in [
        (
            json!({"7":{"type":"audio"}}),
            supplied(&[(0, "audio")]),
            "stream 7: missing source stream",
        ),
        (
            json!({"7":{"type":"audio","copy":false}}),
            supplied(&[(0, "audio")]),
            "stream 7: missing source stream",
        ),
        (
            json!({"7":{"type":"audio"}}),
            supplied(&[(7, "subtitle")]),
            "stream 7: configured audio, probed subtitle",
        ),
        (
            Value::Null,
            supplied(&[(2, "audio"), (2, "audio")]),
            "stream 2: duplicate probe index",
        ),
        (
            Value::Null,
            supplied(&[(2, "")]),
            "stream 2: malformed probe type",
        ),
    ] {
        let error = args(
            json!({"tracks":tracks}),
            Some(Path::new("root identity")),
            &[streams],
        )
        .expect_err("bad source rejected");
        let error = format!("{error:#}");
        assert!(
            error.contains("root identity input 1") && error.contains(expected),
            "{error}"
        );
    }
    assert!(
        args(json!({"tracks":{"7":{"type":"audio"}}}), None, &[])
            .expect_err("missing main source")
            .to_string()
            .contains("main input stream 7")
    );
}

#[test]
fn native_source_validation_precedes_output_and_concat_mutation() {
    let dir = tempfile::tempdir().expect("fixture");
    let (source, chunk) = fixture(dir.path(), false);
    let original = std::fs::read(&source).expect("source bytes");
    let concat = SceneConcatenator::scratch_directory(dir.path()).join("concat.txt");
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
    SceneConcatenator::ffmpeg_cancel(
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
fn isolated_probe_failures_and_force_preserve_output_and_scratch() {
    use std::os::unix::fs::PermissionsExt;
    let python = which::which("python3").expect("required Python fixture tool");
    for case in [
        "malformed",
        "duplicate",
        "bad-type",
        "stdout-overflow",
        "stderr-overflow",
        "force",
    ] {
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
            .args([
                "--exact",
                "core::sequence::scene_concatenator::ffmpeg::ffmpeg_tracks::ffmpeg_probe_child",
                "--nocapture",
            ])
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
#[test]
fn ffmpeg_probe_child() {
    let Some(root) = std::env::var_os("FFMPEG_TRACKS_ROOT").map(PathBuf::from) else {
        return;
    };
    let case = std::env::var("FFMPEG_TRACKS_CASE").expect("case");
    let source = root.join("source 'quoted'.mkv");
    let scene = root.join("scene.mkv");
    let output = root.join("output.mkv");
    let scratch = SceneConcatenator::scratch_directory(&root);
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
    let result = SceneConcatenator::ffmpeg_cancel(
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
            error.downcast_ref::<crate::utils::cancellation::Cancelled>(),
            Some(&crate::utils::cancellation::Cancelled::Forced)
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
            super::super::tests::preflight::wait_for_owned_process_exit(pid);
        }
    }
    println!(
        "{case}: owning FFmpeg preflight error/cancel, bounded joined capture, source/output/concat sentinels checked"
    );
}
