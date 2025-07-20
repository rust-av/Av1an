use av_format::rational::Rational64;

use super::*;

#[test]
fn mkvmerge_options_json_no_audio() {
    let result = mkvmerge_options_json(
        &["00000.ivf".to_string(), "00001.ivf".to_string()],
        "output.mkv",
        None,
        Some(Rational64::new(30, 1)),
    )
    .expect("options call should succeed");
    assert_eq!(
        result,
        r#"["-o", "output.mkv", "--default-duration", "0:30/1fps", "[", "00000.ivf", "00001.ivf","]"]"#
    );
}

#[test]
fn mkvmerge_options_json_with_audio() {
    let result = mkvmerge_options_json(
        &["00000.ivf".to_string(), "00001.ivf".to_string()],
        "output.mkv",
        Some("audio.mkv"),
        Some(Rational64::new(30, 1)),
    )
    .expect("options call should succeed");
    assert_eq!(
        result,
        r#"["-o", "output.mkv", "audio.mkv", "--default-duration", "0:30/1fps", "[", "00000.ivf", "00001.ivf","]"]"#
    );
}
