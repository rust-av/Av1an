use crate::{
    encoder::{parse_svt_av1_version, Encoder},
    ffmpeg::FFPixelFormat,
};

#[test]
fn svt_av1_parsing() {
    let test_cases = [
        ("SVT-AV1 v0.8.7-333-g010c1881 (release)", Some((0, 8, 7))),
        ("SVT-AV1 v0.9.0-dirty (debug)", Some((0, 9, 0))),
        ("SVT-AV1 v1.2.0 (release)", Some((1, 2, 0))),
        ("SVT-AV1 v3.2.1 (release)", Some((3, 2, 1))),
        ("SVT-AV1 v3.2.11 (release)", Some((3, 2, 11))),
        ("SVT-AV1 v0.8.11 (release)", Some((0, 8, 11))),
        ("SVT-AV1 v0.8.11-333-g010c1881 (release)", Some((0, 8, 11))),
        ("invalid", None),
    ];

    for (s, ans) in test_cases {
        assert_eq!(parse_svt_av1_version(s.as_bytes()), ans);
    }
}

#[test]
fn ffmpeg_probe_uses_fps_mode_passthrough() {
    let (pipe, _) = Encoder::svt_av1.probe_cmd(
        "temp".to_owned(),
        0,
        30.0,
        FFPixelFormat::YUV420P10LE,
        2,
        1,
        None,
    );
    let pipe = pipe.expect("FFmpeg pipe should be present");

    assert!(pipe.windows(2).any(|args| args[0] == "-fps_mode" && args[1] == "passthrough"));
    assert!(!pipe.iter().any(|arg| arg == "-vsync"));
}
