use std::{collections::HashMap, fs, io::Write as _, path::Path};

use andean_condor::models::encoder::cli_parameter::CLIParameter;
use california_condor::zoner::{
    ZonerConfig,
    parse::{load_zones, parse_zones},
    zone::{Zone, ZoneEncoder, ZoneEncoderMerge, ZoneOptions},
};

fn write(path: &Path, contents: &str) {
    let mut file = fs::File::create(path).expect("the test file is created");
    file.write_all(contents.as_bytes()).expect("the test file is written");
}

fn number(value: f64) -> CLIParameter {
    CLIParameter::new_number("--", " ", value)
}

fn merge(zone: &Zone) -> &ZoneEncoderMerge {
    match zone.encoder.as_ref().expect("the zone carries an encoder") {
        ZoneEncoder::Merge(merge) => merge,
        ZoneEncoder::Replace(_) => panic!("the zone merges rather than replaces"),
    }
}

fn options(merge: &ZoneEncoderMerge) -> HashMap<String, CLIParameter> {
    match merge.options.as_ref().expect("the zone carries options") {
        ZoneOptions::Merge(body) | ZoneOptions::Replace(body) => {
            body.parameters().expect("the parameters parse")
        },
    }
}

#[test]
fn text_zone_file_skips_comments_and_blank_lines_and_reads_each_definition() {
    let directory = tempfile::tempdir().expect("a temp directory");
    let path = directory.path().join("zones.txt");
    write(
        &path,
        "# a comment\n\n-1 200 svt-av1 \"--preset 3\"\n200 -1 replace x265 \"--crf 18\"\n",
    );
    let zones = load_zones(&path).expect("the zone file parses");
    assert_eq!(zones.len(), 2, "comments and blank lines are skipped");
    assert_eq!(zones[0].end_frame, Some(200));
    assert_eq!(zones[1].start_frame, Some(200));
    assert_eq!(zones[1].end_frame, None);
}

#[test]
fn invalid_text_zone_file_reports_filename_and_line_number() {
    let directory = tempfile::tempdir().expect("a temp directory");
    let path = directory.path().join("zones.txt");
    write(&path, "-1 200 svt-av1 \"--preset 3\"\n0 100 nope\n");
    let error = load_zones(&path).expect_err("the second line is invalid");
    assert!(
        error.to_string().contains("zones.txt line 2"),
        "error names the line: {error}"
    );
}

#[test]
fn json_zone_config_file_preserves_maximum_frames_and_encoder_options() {
    let directory = tempfile::tempdir().expect("a temp directory");
    let config_path = directory.path().join("zones.json");
    write(
        &config_path,
        r#"{"zones": [{"start_frame": null, "end_frame": 200, "maximum_frames": 100,
                "encoder": {"Merge": {"base": "SVTAV1", "options": {"Merge": "--preset 3"}}}}]}"#,
    );
    let zones = load_zones(&config_path).expect("the config-shaped file parses");
    assert_eq!(zones.len(), 1);
    assert_eq!(zones[0].maximum_frames, Some(100));
    assert_eq!(options(merge(&zones[0])).get("preset"), Some(&number(3.0)));
}

#[test]
fn json_zone_array_file_preserves_end_frame_without_inventing_an_encoder() {
    let directory = tempfile::tempdir().expect("a temp directory");
    let list_path = directory.path().join("list.json");
    write(
        &list_path,
        r#"[{"start_frame": 0, "end_frame": 200, "maximum_frames": null, "encoder": null}]"#,
    );
    let zones = load_zones(&list_path).expect("the array-shaped file parses");
    assert_eq!(zones.len(), 1);
    assert_eq!(zones[0].end_frame, Some(200));
    assert_eq!(zones[0].encoder, None);
}

#[test]
fn unknown_zone_json_keys_retain_filename_and_typed_serde_cause() {
    let directory = tempfile::tempdir().expect("a temp directory");
    let path = directory.path().join("typo.json");
    for (contents, key) in [
        (r#"{"zone": [{"end_frame": 200}]}"#, "zone"),
        (
            r#"{"zones": [{"end_frame": 200}], "unexpected": []}"#,
            "unexpected",
        ),
    ] {
        write(&path, contents);
        let error = load_zones(&path).expect_err("unknown top-level keys must not be ignored");
        let message = format!("{error:#}");
        assert!(message.contains("typo.json"), "{message}");
        assert!(
            message.contains(&format!("unknown field `{key}`")),
            "{message}"
        );
        assert!(error.downcast_ref::<serde_json::Error>().is_some());
        assert!(serde_json::from_str::<ZonerConfig>(contents).is_err());
    }
}

#[test]
fn valid_inline_zone_does_not_mask_a_later_invalid_json_file() {
    let directory = tempfile::tempdir().expect("a temp directory");
    let path = directory.path().join("typo.json");
    write(&path, r#"{"zone": [{"end_frame": 200}]}"#);
    let error = parse_zones(&["0 100 x265".to_owned(), path.display().to_string()])
        .expect_err("a valid earlier zone must not mask a bad file");
    let message = format!("{error:#}");
    assert!(message.contains("typo.json"), "{message}");
    assert!(message.contains("unknown field `zone`"), "{message}");
}

#[test]
fn empty_zone_json_array_or_config_file_loads_without_zones() {
    let directory = tempfile::tempdir().expect("a temp directory");
    let path = directory.path().join("zones.json");
    for contents in ["{}", r#"{"zones": []}"#, "[]"] {
        write(&path, contents);
        assert!(load_zones(&path).expect("empty zones are valid").is_empty());
        assert!(parse_zones(&[path.display().to_string()]).expect("empty file").is_empty());
    }
    assert!(
        serde_json::from_str::<ZonerConfig>("{}")
            .expect("default config")
            .zones
            .is_empty()
    );
}

#[test]
fn zone_json_omitted_optional_fields_keep_unset_bounds_and_encoder() {
    let directory = tempfile::tempdir().expect("a temp directory");
    let path = directory.path().join("zones.json");
    for (contents, end_frame) in [
        (r#"{"zones": [{}]}"#, None),
        ("[{}]", None),
        (r#"{"zones": [{"end_frame": 200}]}"#, Some(200)),
        (r#"[{"end_frame": 200}]"#, Some(200)),
    ] {
        write(&path, contents);
        let zones = load_zones(&path).expect("optional fields keep their defaults");
        assert_eq!(zones.len(), 1);
        assert_eq!(zones[0].start_frame, None);
        assert_eq!(zones[0].maximum_frames, None);
        assert_eq!(zones[0].encoder, None);
        assert_eq!(zones[0].end_frame, end_frame);
    }
}

#[test]
fn invalid_zone_json_retains_filename_and_typed_serde_cause() {
    let directory = tempfile::tempdir().expect("a temp directory");
    let path = directory.path().join("zones.json");
    for contents in ["{", "null", "true", r#"{"zones": false}"#, "[false]"] {
        write(&path, contents);
        let error = load_zones(&path).expect_err("invalid JSON or shapes must fail");
        assert!(format!("{error:#}").contains("zones.json"));
        assert!(error.downcast_ref::<serde_json::Error>().is_some());
    }
}

#[test]
fn json_lines_zone_file_reads_definitions_across_blank_lines() {
    let directory = tempfile::tempdir().expect("a temp directory");
    let path = directory.path().join("zones.jsonl");
    write(
        &path,
        "{\"start_frame\": 0, \"end_frame\": 200, \"maximum_frames\": 100, \"encoder\": \
             null}\n\n{\"start_frame\": 200, \"end_frame\": null, \"maximum_frames\": null, \
             \"encoder\": null}\n",
    );
    let zones = load_zones(&path).expect("the JSON lines file parses");
    assert_eq!(zones.len(), 2);
    assert_eq!(zones[1].start_frame, Some(200));
}

#[test]
fn missing_zone_file_error_names_the_requested_path() {
    let error = parse_zones(&["zones.json".to_owned()]).expect_err("the file does not exist");
    assert!(
        error.to_string().contains("zone file zones.json does not exist"),
        "error names the file: {error}"
    );
}

#[test]
fn zone_files_and_inline_definitions_preserve_argument_order() {
    let directory = tempfile::tempdir().expect("a temp directory");
    let path = directory.path().join("zones.txt");
    write(&path, "-1 200 svt-av1 \"--preset 3\"\n");
    let zones = parse_zones(&[path.display().to_string(), "200 -1 x265 \"--crf 18\"".to_owned()])
        .expect("both values parse");
    assert_eq!(zones.len(), 2);
    assert_eq!(zones[0].end_frame, Some(200));
    assert_eq!(zones[1].start_frame, Some(200));
}
