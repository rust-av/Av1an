use super::*;
use serde_json::{Value, json};

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
fn ffmpeg_args_count_copied_streams_before_indexing_new_attachments() {
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
fn ffmpeg_args_index_only_retained_tracks_within_each_stream_type() {
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
fn ffmpeg_args_map_extra_and_attachment_only_sources_without_an_original_input() {
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
fn ffmpeg_args_reject_missing_or_mistyped_source_ids_with_input_context() {
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
