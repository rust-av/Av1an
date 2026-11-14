use super::*;

#[test]
fn mkvmerge_config_legacy_attachment_tracks_serialize_in_separate_namespaces() {
    let legacy: MkvmergeConfig = serde_json::from_str(r#"{"tracks":{"1":{"type":"attachment","copy":false,"path":"font.ttf","attachment_name":"exact name","attachment_description":"exact description","attachment_mime_type":"font/ttf"}},"extra_inputs":[{"path":"extra.mkv","tracks":{"1":{"type":"attachment"}}}]}"#).expect("unambiguous legacy");
    let canonical = serde_json::to_value(&legacy).expect("canonical write");
    assert!(
        canonical["tracks"].is_null() || canonical["tracks"].as_object().expect("map").is_empty()
    );
    assert_eq!(canonical["attachments"]["1"]["path"], "font.ttf");
    assert_eq!(canonical["attachments"]["1"]["copy"], false);
    assert_eq!(
        canonical["attachments"]["1"]["attachment_name"],
        "exact name"
    );
    assert_eq!(
        canonical["attachments"]["1"]["attachment_description"],
        "exact description"
    );
    assert_eq!(
        canonical["attachments"]["1"]["attachment_mime_type"],
        "font/ttf"
    );
    assert!(canonical["attachments"]["1"].get("type").is_none());
    assert_eq!(
        canonical["extra_inputs"][0]["attachments"]["1"]["copy"],
        true
    );
    assert_eq!(
        serde_json::from_value::<MkvmergeConfig>(canonical).expect("reload"),
        legacy
    );
}

#[test]
fn mkvmerge_config_duplicate_or_colliding_track_attachment_ids_are_rejected() {
    for text in [
        r#"{"tracks":{"1":{"type":"attachment"}},"attachments":{"1":{}}}"#,
        r#"{"tracks":{"1":{"type":"attachment"},"1":{"type":"audio"}}}"#,
        r#"{"tracks":{"1":{"type":"attachment"},"1":{"type":"attachment"}}}"#,
        r#"{"tracks":{"1":{"type":"audio"},"1":{"type":"attachment"}}}"#,
        r#"{"attachments":{"1":{}},"tracks":{"1":{"type":"attachment"}}}"#,
        r#"{"attachments":{"1":{},"1":{}}}"#,
        r#"{"extra_inputs":[{"path":"extra.mkv","tracks":{"1":{"type":"attachment"},"1":{"type":"audio"}}}]}"#,
        r#"{"extra_inputs":[{"path":"extra.mkv","tracks":{"1":{"type":"attachment"}},"attachments":{"1":{}}}]}"#,
    ] {
        let error = serde_json::from_str::<MkvmergeConfig>(text)
            .expect_err("ambiguous ID must not overwrite")
            .to_string();
        assert!(
            error.contains('1') && (error.contains("duplicate") || error.contains("collision")),
            "{error}"
        );
    }
}

#[test]
fn mkvmerge_schema_separates_attachment_and_regular_track_shapes() {
    let schema = serde_json::to_value(schemars::schema_for!(MkvmergeConfig)).expect("schema");
    assert_eq!(
        schema["properties"]["attachments"]["patternProperties"][r"^\d+$"]["$ref"],
        "#/$defs/AttachmentTrack"
    );
    assert!(schema["$defs"]["MkvmergeExtraInput"]["properties"].get("attachments").is_some());
    let track_shapes = schema["$defs"]["MkvmergeTrack"].to_string();
    assert!(track_shapes.contains("audio") && !track_shapes.contains("attachment"));
}

#[test]
fn mkvmerge_config_null_or_missing_maps_use_defaults_and_empty_maps_emit_no_input_args() {
    for text in ["{}", r#"{"tracks":null,"attachments":null}"#] {
        assert_eq!(
            serde_json::from_str::<MkvmergeConfig>(text).expect("defaults"),
            MkvmergeConfig::default()
        );
    }
    let empty: MkvmergeConfig = serde_json::from_str(r#"{"tracks":{},"attachments":{},"extra_inputs":[{"path":"extra","tracks":null,"attachments":{}}]}"#).expect("empty maps");
    assert!(empty.input_args().is_empty());
    assert!(empty.extra_inputs[0].input_args().is_empty());
    assert_eq!(
        serde_json::from_value::<MkvmergeConfig>(serde_json::to_value(&empty).expect("write"))
            .expect("reload"),
        empty
    );
}
