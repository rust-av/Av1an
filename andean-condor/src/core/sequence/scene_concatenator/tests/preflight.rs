use super::super::*;

#[cfg(unix)]
#[path = "../../../../../tests/core/sequence/scene_concatenator/fixtures/preflight.rs"]
mod fixture;

#[cfg(unix)]
#[test]
fn prepared_source_strips_globals_and_chapters_but_preserves_ancillary_tags_and_attachment_bytes() {
    fixture::isolated(
        module_path!(),
        "prepared_source_strips_globals_and_chapters_but_preserves_ancillary_tags_and_attachment_bytes",
        &["strip-global-and-chapters"],
        |root, _| {
            let source = root.join("source 'quoted'.mkv");
            let original = std::fs::read(&source).expect("source");
            let output = root.join("out.mkv");
            std::fs::write(&output, b"old output sentinel").expect("old output");
            let config: MkvmergeConfig = serde_json::from_value(serde_json::json!({
                "metadata": {"copy": false}, "chapters": {"copy": false}
            }))
            .expect("global/chapter opt-out");
            let probe = |path: &Path| -> serde_json::Value {
                serde_json::from_slice(&fixture::run(
                    Command::new("ffprobe")
                        .args([
                            "-v",
                            "error",
                            "-count_frames",
                            "-show_streams",
                            "-show_format",
                            "-of",
                            "json",
                        ])
                        .arg(path),
                ))
                .expect("native JSON")
            };
            let before = probe(&source);
            assert_eq!(before["format"]["tags"]["title"], "concat-preflight title");
            assert_eq!(
                before["format"]["tags"]["COMMENT"],
                "concat-preflight global tag"
            );
            let scratch = SceneConcatenator::scratch_directory(root);
            std::fs::create_dir_all(&scratch).expect("scratch");
            std::fs::write(scratch.join("source.mkv"), b"caller scratch sentinel")
                .expect("sentinel");
            let prepared = source::prepare(
                Some(&source),
                Some(&config),
                &scratch,
                &Cancellation::default(),
            )
            .expect("opting out of global tags must preserve ancillary track tags");
            let staged = prepared.path.as_ref().expect("owned ancillary source");
            let data = probe(staged);
            let streams = data["streams"].as_array().expect("ancillary streams");
            assert_eq!(streams.len(), 3);
            let audio =
                streams.iter().find(|stream| stream["codec_type"] == "audio").expect("audio");
            assert_eq!(audio["tags"]["language"], "eng");
            assert_eq!(audio["tags"]["title"], "concat-preflight audio name");
            let attachment = streams
                .iter()
                .find(|stream| stream["codec_type"] == "attachment")
                .expect("attachment");
            assert_eq!(attachment["tags"]["filename"], "attachment.txt");
            assert_eq!(attachment["tags"]["mimetype"], "text/plain");
            assert!(
                data["format"]["tags"]
                    .as_object()
                    .expect("generated format tags")
                    .keys()
                    .all(|key| key.eq_ignore_ascii_case("encoder")),
                "source global tags must be stripped: {}",
                data["format"]["tags"]
            );
            let chapters: serde_json::Value = serde_json::from_slice(&fixture::run(
                Command::new("ffprobe")
                    .args(["-v", "error", "-show_chapters", "-of", "json"])
                    .arg(staged),
            ))
            .expect("staged chapters");
            assert!(chapters["chapters"].as_array().expect("chapters").is_empty());
            let bytes = fixture::run(
                Command::new("ffmpeg")
                    .args(["-v", "error", "-dump_attachment:t:0", "pipe:1", "-i"])
                    .arg(staged)
                    .args(["-f", "null", "-"]),
            );
            assert_eq!(bytes, b"concat-preflight attachment identity");
            drop(prepared);
            assert_eq!(
                std::fs::read(output).expect("output sentinel"),
                b"old output sentinel"
            );
            assert_eq!(std::fs::read(source).expect("source intact"), original);
            assert_eq!(
                std::fs::read(scratch.join("source.mkv")).expect("caller scratch sentinel"),
                b"caller scratch sentinel"
            );
            assert!(
                !std::fs::read_dir(scratch).expect("scratch listing").any(|entry| entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".condor-source-"))
            );
        },
    );
}
