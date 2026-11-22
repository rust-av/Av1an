use super::*;
use andean_condor::models::sequence::scene_concatenator::MkvmergeConfig;
use av_format::rational::Ratio;

#[path = "fixtures/preflight.rs"]
mod fixture;
use fixture::{ancillary_source, chunks, run};

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

fn mux(
    root: &Path,
    source: &Path,
    chunks: &[PathBuf],
    config: Option<&MkvmergeConfig>,
) -> Result<()> {
    let (tx, _rx) = sync::mpsc::channel();
    SceneConcatenator::mkvmerge(
        root,
        &root.join("out.mkv"),
        chunks,
        Some(source),
        Ratio::new(24, 1),
        config,
        &tx,
        &Cancellation::default(),
    )
}

fn probe(path: &Path) -> serde_json::Value {
    serde_json::from_slice(&run(Command::new("ffprobe")
        .args(["-v", "error", "-count_frames", "-show_streams", "-show_format", "-of", "json"])
        .arg(path)))
    .expect("native JSON")
}

#[test]
fn mkvmerge_omits_unsupported_y4m_source_and_preserves_concatenated_frame_order() {
    let dir = tempfile::tempdir().expect("fixture");
    let source = dir.path().join("source 'with spaces'.y4m");
    run(Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=red:size=32x16:rate=24",
            "-frames:v",
            "4",
        ])
        .arg(&source));
    let original = std::fs::read(&source).expect("source");
    mux(dir.path(), &source, &chunks(dir.path()), None).expect("Y4M must not reach mkvmerge");
    let output = dir.path().join("out.mkv");
    let data = probe(&output);
    assert_eq!(data["streams"].as_array().expect("streams").len(), 1);
    assert_eq!(data["streams"][0]["nb_read_frames"], "4");
    let pixels = run(Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&output)
        .args(["-f", "rawvideo", "-pix_fmt", "gray", "-"]));
    assert_eq!(pixels.len(), 32 * 16 * 4);
    assert!(pixels[..32 * 16 * 2].iter().all(|&pixel| pixel < 5));
    assert!(pixels[32 * 16 * 2..].iter().all(|&pixel| pixel > 250));
    assert_eq!(std::fs::read(source).expect("unchanged root"), original);
}

fn wmv_source(root: &Path) -> PathBuf {
    let source = root.join("source 'audio'.wmv");
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
            "sine=sample_rate=48000",
            "-t",
            "0.2",
            "-c:v",
            "wmv2",
            "-c:a",
            "wmav2",
            "-metadata",
            "title=concat-preflight preserved title",
            "-metadata:s:a:0",
            "language=eng",
        ])
        .arg(&source));
    source
}

#[test]
fn mkvmerge_adapts_wmv_audio_without_changing_packets_or_caller_scratch() {
    let dir = tempfile::tempdir().expect("fixture");
    let source = wmv_source(dir.path());
    let original = std::fs::read(&source).expect("source");
    let scratch = scratch_directory(dir.path());
    std::fs::create_dir_all(&scratch).expect("scratch");
    std::fs::write(scratch.join("source.mkv"), b"caller scratch sentinel").expect("sentinel");
    let scenes = chunks(dir.path());
    mux(dir.path(), &source, &scenes, None).expect("native ancillary adaptation");
    let output = dir.path().join("out.mkv");
    let data = probe(&output);
    let streams = data["streams"].as_array().expect("streams");
    let audio = streams
        .iter()
        .find(|stream| stream["codec_type"] == "audio")
        .expect("preserved audio");
    assert_eq!(audio["codec_name"], "wmav2");
    assert_eq!(audio["tags"]["language"], "eng");
    assert_eq!(
        data["format"]["tags"]["title"],
        "concat-preflight preserved title"
    );
    let packet_hashes = |path: &Path| {
        let data: serde_json::Value = serde_json::from_slice(&run(Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-select_streams",
                "a",
                "-show_packets",
                "-show_data_hash",
                "sha256",
                "-of",
                "json",
            ])
            .arg(path)))
        .expect("packet hashes");
        data["packets"]
            .as_array()
            .expect("packets")
            .iter()
            .map(|packet| packet["data_hash"].clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        packet_hashes(&source),
        packet_hashes(&output),
        "compressed audio bytes unchanged"
    );
    assert_eq!(std::fs::read(source).expect("source intact"), original);
    assert_eq!(
        std::fs::read(scratch.join("source.mkv")).expect("caller sentinel"),
        b"caller scratch sentinel"
    );
    assert!(
        !std::fs::read_dir(scratch).expect("scratch listing").any(|entry| entry
            .expect("entry")
            .file_name()
            .to_string_lossy()
            .starts_with(".condor-source-"))
    );
}

#[test]
fn mkvmerge_refuses_unprovable_track_mapping_after_wmv_adaptation() {
    let dir = tempfile::tempdir().expect("fixture");
    let source = wmv_source(dir.path());
    let original = std::fs::read(&source).expect("source");
    let scenes = chunks(dir.path());
    let output = dir.path().join("out.mkv");
    let config = serde_json::from_str(r#"{"tracks":{"1":{"type":"audio","language":"jpn"}}}"#)
        .expect("original ID settings");
    std::fs::write(&output, b"mapping refusal sentinel").expect("old output");
    let error = mux(
        dir.path(),
        &dir.path().join("source 'audio'.wmv"),
        &scenes,
        Some(&config),
    )
    .expect_err("source index 1 cannot be reused as remux mkvmerge ID 0");
    assert!(format!("{error:#}").contains("no provable mkvmerge"));
    assert_eq!(
        std::fs::read(output).expect("output sentinel"),
        b"mapping refusal sentinel"
    );
    assert_eq!(std::fs::read(source).expect("source intact"), original);
}

#[test]
fn mkvmerge_preserves_selected_track_ids_language_and_name_for_mkv_or_mp4_sources() {
    for extension in ["mkv", "mp4"] {
        let dir = tempfile::tempdir().expect("fixture");
        let source = dir.path().join(format!("known source.{extension}"));
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
                "-t",
                "0.2",
                "-c:v",
                "mpeg4",
                "-c:a",
                "aac",
            ])
            .arg(&source));
        let config = serde_json::from_str(r#"{"tracks":{"1":{"type":"audio","copy":false},"2":{"type":"audio","language":"jpn","name":"selected track"}}}"#).expect("settings");
        mux(dir.path(), &source, &chunks(dir.path()), Some(&config))
            .expect("recognized source retains IDs, including warning-only MP4 muxes");
        let data = probe(&dir.path().join("out.mkv"));
        let audio = data["streams"]
            .as_array()
            .expect("streams")
            .iter()
            .filter(|stream| stream["codec_type"] == "audio")
            .collect::<Vec<_>>();
        assert_eq!(audio.len(), 1);
        assert_eq!(audio[0]["tags"]["language"], "jpn");
        assert_eq!(audio[0]["tags"]["title"], "selected track");
    }
}

#[test]
fn mkvmerge_refuses_unprovable_ancillary_track_ids_before_mutating_output() {
    let dir = tempfile::tempdir().expect("fixture");
    let source = dir.path().join("root.y4m");
    run(Command::new("ffmpeg")
        .args(["-v", "error", "-f", "lavfi", "-i", "color=size=32x16:rate=24", "-frames:v", "2"])
        .arg(&source));
    let original = std::fs::read(&source).expect("root");
    let chunks = chunks(dir.path());
    for config in [
        serde_json::json!({"tracks":{"1":{"type":"audio","copy":true}}}),
        serde_json::json!({"extra_inputs":[{"path":source}]}),
    ] {
        std::fs::write(dir.path().join("out.mkv"), b"existing output sentinel").expect("sentinel");
        let config = serde_json::from_value(config).expect("config");
        let error = mux(dir.path(), &source, &chunks, Some(&config))
            .expect_err("no provable source ID namespace");
        assert!(
            format!("{error:#}").contains("use FFmpeg concat method"),
            "{error:#}"
        );
        assert_eq!(
            std::fs::read(dir.path().join("out.mkv")).expect("old output"),
            b"existing output sentinel"
        );
        assert_eq!(std::fs::read(&source).expect("root intact"), original);
    }
}

#[cfg(unix)]
#[test]
fn mkvmerge_adaptation_preserves_title_chapters_and_attachment_bytes() {
    preflight_cases(
        "mkvmerge_adaptation_preserves_title_chapters_and_attachment_bytes",
        &["adapt-content"],
    );
}

#[cfg(unix)]
#[test]
fn mkvmerge_identification_warning_allows_valid_source_tracks() {
    preflight_cases(
        "mkvmerge_identification_warning_allows_valid_source_tracks",
        &["identify-warning"],
    );
}

#[cfg(unix)]
#[test]
fn exited_probe_leader_does_not_wait_for_descendant_held_pipes() {
    preflight_cases(
        "exited_probe_leader_does_not_wait_for_descendant_held_pipes",
        &["leader-exit"],
    );
}

#[cfg(unix)]
#[test]
fn mkvmerge_preflight_rejects_failed_malformed_or_missing_identification() {
    preflight_cases(
        "mkvmerge_preflight_rejects_failed_malformed_or_missing_identification",
        &["identify-errors", "identify-status", "malformed-identify", "missing-identify"],
    );
}

#[cfg(unix)]
#[test]
fn mkvmerge_preflight_rejects_failed_malformed_or_missing_source_probe() {
    preflight_cases(
        "mkvmerge_preflight_rejects_failed_malformed_or_missing_source_probe",
        &["probe-failure", "malformed-probe", "missing-probe"],
    );
}

#[cfg(unix)]
#[test]
fn mkvmerge_preflight_limits_probe_stdout_and_stderr_capture() {
    preflight_cases(
        "mkvmerge_preflight_limits_probe_stdout_and_stderr_capture",
        &["probe-overflow", "stderr-overflow"],
    );
}

#[cfg(unix)]
#[test]
fn mkvmerge_adaptation_rejects_sources_without_supported_ancillary_streams() {
    preflight_cases(
        "mkvmerge_adaptation_rejects_sources_without_supported_ancillary_streams",
        &["metadata-only", "chapter-only", "unsupported-type"],
    );
}

#[cfg(unix)]
#[test]
fn mkvmerge_adaptation_rejects_missing_empty_or_unidentifiable_remux_output() {
    preflight_cases(
        "mkvmerge_adaptation_rejects_missing_empty_or_unidentifiable_remux_output",
        &["empty-remux", "missing-remux", "invalid-remux", "track-loss"],
    );
}

#[cfg(unix)]
#[test]
fn mkvmerge_adaptation_reports_codec_copy_refusal_and_preserves_output() {
    preflight_cases(
        "mkvmerge_adaptation_reports_codec_copy_refusal_and_preserves_output",
        &["codec-refusal"],
    );
}

#[cfg(unix)]
#[test]
fn mkvmerge_adaptation_rejects_remux_that_loses_source_metadata() {
    preflight_cases(
        "mkvmerge_adaptation_rejects_remux_that_loses_source_metadata",
        &["metadata-loss"],
    );
}

#[cfg(unix)]
#[test]
fn forced_mkvmerge_source_probe_preserves_output_and_cleans_owned_scratch() {
    preflight_cases(
        "forced_mkvmerge_source_probe_preserves_output_and_cleans_owned_scratch",
        &["force-probe"],
    );
}

#[cfg(unix)]
#[test]
fn forced_mkvmerge_source_remux_preserves_output_and_cleans_owned_scratch() {
    preflight_cases(
        "forced_mkvmerge_source_remux_preserves_output_and_cleans_owned_scratch",
        &["force-remux"],
    );
}

#[test]
fn mkvmerge_strips_or_replaces_global_metadata_without_losing_ancillary_track_metadata() {
    // Removing the owner's empty-tag filter breaks strip-only real muxes;
    // coupling copy=false to XML emission loses the explicitly requested values.
    let fixtures = tempfile::tempdir().expect("native fixtures");
    let source = ancillary_source(fixtures.path());
    let original = std::fs::read(&source).expect("source");
    let before = probe(&source);
    assert_eq!(before["format"]["tags"]["title"], "concat-preflight title");
    assert_eq!(
        before["format"]["tags"]["COMMENT"],
        "concat-preflight global tag"
    );
    let scenes = chunks(fixtures.path());
    for (case, metadata, expected_title, expected_tag) in [
        ("strip", serde_json::json!({"copy": false}), None, None),
        (
            "title",
            serde_json::json!({"copy": false, "title": "ancillary-metadata & <title> \"double\" 'single'"}),
            Some("ancillary-metadata & <title> \"double\" 'single'"),
            None,
        ),
        (
            "tags",
            serde_json::json!({"copy": false, "tags": {"ANCILLARY_METADATA_SPECIAL": "ancillary-metadata & <value> \"double\" 'single'"}}),
            None,
            Some("ancillary-metadata & <value> \"double\" 'single'"),
        ),
        (
            "empty-title",
            serde_json::json!({"copy": false, "title": ""}),
            Some(""),
            None,
        ),
    ] {
        let dir = tempfile::tempdir_in(fixtures.path()).expect("case");
        let config = serde_json::from_value(serde_json::json!({"metadata": metadata}))
            .expect("metadata config");
        let scratch = scratch_directory(dir.path());
        std::fs::create_dir_all(&scratch).expect("scratch");
        std::fs::write(scratch.join("source.mkv"), b"caller scratch sentinel").expect("sentinel");
        mux(dir.path(), &source, &scenes, Some(&config))
            .unwrap_or_else(|error| panic!("{case}: actual native final mux: {error:#}"));
        let output = dir.path().join("out.mkv");
        let data = probe(&output);
        let tags = &data["format"]["tags"];
        assert_eq!(
            tags["title"].as_str().or_else(|| tags["TITLE"].as_str()),
            expected_title,
            "{case}: {tags}"
        );
        assert_eq!(
            tags["ANCILLARY_METADATA_SPECIAL"].as_str(),
            expected_tag,
            "{case}: {tags}"
        );
        assert!(
            tags["COMMENT"].is_null(),
            "{case}: source global tag leaked"
        );
        let streams = data["streams"].as_array().expect("streams");
        assert_eq!(streams.len(), 4, "{case}: video/audio/subtitle/attachment");
        let audio = streams.iter().find(|stream| stream["codec_type"] == "audio").expect("audio");
        assert_eq!(audio["tags"]["language"], "eng");
        assert_eq!(audio["tags"]["title"], "concat-preflight audio name");
        let attachment = streams
            .iter()
            .find(|stream| stream["codec_type"] == "attachment")
            .expect("attachment");
        assert_eq!(attachment["tags"]["filename"], "attachment.txt");
        assert_eq!(attachment["tags"]["mimetype"], "text/plain");
        let options: Vec<String> = serde_json::from_slice(
            &std::fs::read(scratch.join("options.json")).expect("actual mux options"),
        )
        .expect("options");
        assert!(options.iter().any(|arg| arg == "--no-global-tags"));
        assert_eq!(
            options.iter().any(|arg| arg == "--global-tags"),
            case != "strip"
        );
        let xml = scratch.join("global-tags.xml");
        assert_eq!(xml.exists(), case != "strip");
        if case != "strip" {
            let xml =
                std::fs::read_to_string(xml).expect("generated XML consumed by native mkvmerge");
            let escaped = match case {
                "title" => {
                    "ancillary-metadata &amp; &lt;title&gt; &quot;double&quot; &apos;single&apos;"
                },
                "tags" => {
                    "ancillary-metadata &amp; &lt;value&gt; &quot;double&quot; &apos;single&apos;"
                },
                "empty-title" => "<String></String>",
                _ => unreachable!(),
            };
            assert!(xml.contains(escaped), "{case}: {xml}");
        }
        assert_eq!(std::fs::read(&source).expect("source intact"), original);
        assert_eq!(
            std::fs::read(scratch.join("source.mkv")).expect("sentinel"),
            b"caller scratch sentinel"
        );
        eprintln!(
            "ancillary-metadata native {case}: real final mux/probe, source COMMENT stripped (Segment title separate), requested values/stream tags/options/XML/sentinels checked"
        );
    }
}

#[cfg(unix)]
#[test]
fn mkvmerge_adapted_source_omits_global_tags_and_chapters_but_preserves_ancillary_tracks() {
    preflight_cases(
        "mkvmerge_adapted_source_omits_global_tags_and_chapters_but_preserves_ancillary_tracks",
        &["strip-global-and-chapters"],
    );
}

#[cfg(unix)]
fn preflight_cases(test: &str, cases: &[&str]) {
    fixture::isolated(module_path!(), test, cases, assert_source_preflight);
}

#[cfg(unix)]
fn assert_source_preflight(root: &Path, case: &str) {
    let root = root.to_owned();
    let source = root.join("source 'quoted'.mkv");
    let original = std::fs::read(&source).expect("source");
    let scenes = ["CONCAT_PREFLIGHT_SCENE0", "CONCAT_PREFLIGHT_SCENE1"]
        .map(|key| PathBuf::from(std::env::var_os(key).expect("scene")));
    let output = root.join("out.mkv");
    std::fs::write(&output, b"old output sentinel").expect("old output");
    let token = Cancellation::default();
    if case == "strip-global-and-chapters" {
        let config: MkvmergeConfig = serde_json::from_value(serde_json::json!({
            "metadata": {"copy": false}, "chapters": {"copy": false}
        }))
        .expect("global/chapter opt-out");
        let before = probe(&source);
        assert_eq!(before["format"]["tags"]["title"], "concat-preflight title");
        assert_eq!(
            before["format"]["tags"]["COMMENT"],
            "concat-preflight global tag"
        );
        let scratch = scratch_directory(&root);
        std::fs::create_dir_all(&scratch).expect("scratch");
        std::fs::write(scratch.join("source.mkv"), b"caller scratch sentinel").expect("sentinel");
        assert_eq!(
            std::fs::read(&output).expect("output sentinel"),
            b"old output sentinel"
        );
        mux(&root, &source, &scenes, Some(&config))
            .expect("strip-only actual adapter and native final mux");
        let data = probe(&output);
        let streams = data["streams"].as_array().expect("final streams");
        assert_eq!(streams.len(), 4);
        let audio = streams
            .iter()
            .find(|stream| stream["codec_type"] == "audio")
            .expect("final audio");
        assert_eq!(audio["tags"]["language"], "eng");
        assert_eq!(audio["tags"]["title"], "concat-preflight audio name");
        let attachment = streams
            .iter()
            .find(|stream| stream["codec_type"] == "attachment")
            .expect("final attachment");
        assert_eq!(attachment["tags"]["filename"], "attachment.txt");
        assert_eq!(attachment["tags"]["mimetype"], "text/plain");
        assert!(
            data["format"]["tags"]
                .as_object()
                .expect("final tags")
                .keys()
                .all(|key| key.eq_ignore_ascii_case("encoder") || key == "creation_time"),
            "final source globals stripped: {}",
            data["format"]["tags"]
        );
        let chapters: serde_json::Value = serde_json::from_slice(&run(Command::new("ffprobe")
            .args(["-v", "error", "-show_chapters", "-of", "json"])
            .arg(&output)))
        .expect("final chapters");
        assert!(chapters["chapters"].as_array().expect("chapters").is_empty());
        assert_eq!(
            run(Command::new("ffmpeg")
                .args(["-v", "error", "-dump_attachment:t:0", "pipe:1", "-i"])
                .arg(&output)
                .args(["-f", "null", "-"])),
            b"concat-preflight attachment identity"
        );
        let options: Vec<String> =
            serde_json::from_slice(&std::fs::read(scratch.join("options.json")).expect("options"))
                .expect("mux options");
        assert!(options.iter().any(|arg| arg == "--no-global-tags"));
        assert!(!options.iter().any(|arg| arg == "--global-tags"));
        assert!(!scratch.join("global-tags.xml").exists());
        assert!(
            !std::fs::read_dir(&scratch).expect("scratch listing").any(|entry| entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .starts_with(".condor-source-"))
        );
        assert_eq!(
            std::fs::read(scratch.join("source.mkv")).expect("caller scratch sentinel"),
            b"caller scratch sentinel"
        );
        assert_eq!(std::fs::read(source).expect("source intact"), original);
        println!(
            "strip-global-and-chapters: actual adapter AND native final mux/probe preserved audio language/name and attachment tags/bytes, removed source globals/chapters, omitted tag XML, kept source/scratch sentinels and cleaned owned scratch; only unsupported source identify is a fixture"
        );
        return;
    }
    let controller = case.starts_with("force-").then(|| {
        let root = root.clone();
        let token = token.clone();
        thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !root.join("active").exists() && std::time::Instant::now() < deadline {
                thread::yield_now();
            }
            let active = root.join("active").exists();
            token.request().expect("graceful");
            token.request().expect("force");
            assert!(active, "active owned process handshake");
        })
    });
    let (tx, _rx) = sync::mpsc::channel();
    let started = std::time::Instant::now();
    let result = SceneConcatenator::mkvmerge(
        &root,
        &output,
        &scenes,
        Some(&source),
        Ratio::new(24, 1),
        None,
        &tx,
        &token,
    );
    if let Some(controller) = controller {
        controller.join().expect("joined force controller");
    }
    if case == "leader-exit" {
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "exited leader must release descendant-held pipes without waiting for its sleep"
        );
    }
    if case == "adapt-content" || case == "identify-warning" || case == "leader-exit" {
        result.expect("real FFmpeg copy and native final mkvmerge");
        let data = probe(&output);
        assert_eq!(data["format"]["tags"]["title"], "concat-preflight title");
        assert_eq!(data["streams"].as_array().expect("streams").len(), 4);
        let chapters: serde_json::Value = serde_json::from_slice(&run(Command::new("ffprobe")
            .args(["-v", "error", "-show_chapters", "-of", "json"])
            .arg(&output)))
        .expect("chapters");
        assert_eq!(
            chapters["chapters"][0]["tags"]["title"],
            "concat-preflight chapter"
        );
        let attachment = run(Command::new("ffmpeg")
            .args(["-v", "error", "-dump_attachment:t:0", "pipe:1", "-i"])
            .arg(&output)
            .args(["-f", "null", "-"]));
        assert_eq!(attachment, b"concat-preflight attachment identity");
    } else {
        let error = result.expect_err("preflight must fail before final mux");
        let text = format!("{error:#}");
        match case {
            "force-probe" | "force-remux" => {
                assert_eq!(error.downcast_ref::<Cancelled>(), Some(&Cancelled::Forced));
            },
            "probe-overflow" | "stderr-overflow" => {
                assert!(text.contains("capture exceeded"), "{text}");
            },
            "codec-refusal" => assert!(
                text.contains("codec cannot be copied")
                    && text.contains("use FFmpeg concat method"),
                "{text}"
            ),
            other => {
                let expected = match other {
                    "identify-errors" => "identification errors",
                    "identify-status" => "failed (exit status: 2)",
                    "malformed-identify" => "Malformed mkvmerge identification",
                    "missing-identify" | "missing-probe" => "Running source preflight",
                    "probe-failure" => "fixture probe failure",
                    "malformed-probe" => "Malformed ffprobe source report",
                    "metadata-only" | "chapter-only" => "metadata/chapter-only source",
                    "unsupported-type" => "stream identity/type",
                    "empty-remux" | "missing-remux" => "empty/unrecognized source",
                    "invalid-remux" | "track-loss" => "cannot identify every remuxed",
                    "metadata-loss" => "did not verifiably preserve",
                    _ => panic!("unhandled case {other}"),
                };
                assert!(text.contains(expected), "{case}: {text}");
            },
        }
        assert_eq!(
            std::fs::read(output).expect("sentinel"),
            b"old output sentinel"
        );
    }
    assert_eq!(std::fs::read(source).expect("source intact"), original);
    let scratch = scratch_directory(&root);
    assert!(
        !scratch.exists()
            || !std::fs::read_dir(scratch).expect("scratch listing").any(|entry| entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .starts_with(".condor-source-"))
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
        "{case}: owning preflight, preservation/sentinels, readers/children and source scratch checked"
    );
}
