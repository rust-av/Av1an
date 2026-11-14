use andean_condor::{
    core::{AndeanCondor, DefaultAndeanCondor},
    models::{
        Condor as CondorModel,
        encoder::Encoder,
        input::{Input as InputModel, VapourSynthScriptSource},
        sequence::{DefaultSequenceConfig, DefaultSequenceData},
    },
};
use std::fs;

#[test]
fn load_malformed_models_preserves_bytes_and_serde_causes() {
    for (bytes, category) in [
        (&b"{\"input\":"[..], serde_json::error::Category::Eof),
        (&b"{\"input\":42}"[..], serde_json::error::Category::Syntax),
    ] {
        let dir = tempfile::tempdir().expect("temporary save directory");
        let path = dir.path().join("saved-model.json");
        fs::write(&path, bytes).expect("write malformed save");
        let error = DefaultAndeanCondor::load(&path).expect_err("reject malformed model");
        let context = error.to_string();
        assert!(context.contains(&path.display().to_string()), "{error:#}");
        assert!(context.contains("saved Condor model"), "{error:#}");
        assert!(context.contains("malformed") && context.contains("incompatible"));
        assert_eq!(
            error.downcast_ref::<serde_json::Error>().expect("serde cause").classify(),
            category,
            "{error:#}"
        );
        assert_eq!(fs::read(&path).expect("unchanged save"), bytes);
        assert_eq!(fs::read_dir(dir.path()).expect("save directory").count(), 1);
    }
}

#[test]
fn load_current_and_optional_field_omissions_without_opening_source() {
    let dir = tempfile::tempdir().expect("temporary save directory");
    let missing_source = dir.path().join("nonexistent-source.vpy");
    let model: CondorModel<DefaultSequenceData, DefaultSequenceConfig> = CondorModel {
        input: InputModel::VapourSynthScript {
            source: VapourSynthScriptSource::Path(missing_source.clone()),
            variables: Default::default(),
            index: 0,
            filters: vec![],
            stream_concurrently: false,
        },
        output: andean_condor::models::output::Output {
            path: dir.path().join("nonexistent-output.mkv"),
            overwrite: true,
            tags: Default::default(),
            video_tags: Default::default(),
        },
        encoder: Encoder::FFmpeg {
            executable: None,
            options: Default::default(),
            append_color_data: false,
        },
        scenes: vec![],
        sequence_config: Default::default(),
    };
    for omit_optional in [false, true] {
        let mut value = serde_json::to_value(&model).expect("serialize model");
        if omit_optional {
            value["output"].as_object_mut().expect("output object").remove("overwrite");
            value["input"]["VapourSynthScript"]
                .as_object_mut()
                .expect("input object")
                .remove("stream_concurrently");
            value["encoder"]["FFmpeg"]
                .as_object_mut()
                .expect("encoder object")
                .remove("append_color_data");
        }
        let bytes = serde_json::to_vec(&value).expect("serialize save");
        let path = dir.path().join("saved-model.json");
        fs::write(&path, &bytes).expect("write save");
        let loaded = DefaultAndeanCondor::load(&path)
            .expect("load compatible save")
            .expect("existing model");
        assert_eq!(loaded.input.path(), Some(missing_source.as_path()));
        assert_eq!(loaded.output.path, model.output.path);
        assert_eq!(loaded.output.overwrite, !omit_optional);
        assert!(
            matches!(loaded.input, InputModel::VapourSynthScript { stream_concurrently, .. } if stream_concurrently == omit_optional)
        );
        assert!(
            matches!(loaded.encoder, Encoder::FFmpeg { append_color_data, .. } if append_color_data == omit_optional)
        );
        assert_eq!(fs::read(&path).expect("unchanged save"), bytes);
        assert_eq!(fs::read_dir(dir.path()).expect("save directory").count(), 1);
    }
}

#[test]
fn missing_saved_model_returns_none_without_creating_files() {
    let dir = tempfile::tempdir().expect("temporary save directory");
    let missing = dir.path().join("missing.json");
    assert!(DefaultAndeanCondor::load(&missing).expect("missing save").is_none());
    assert_eq!(fs::read_dir(dir.path()).expect("save directory").count(), 0);
}

#[test]
fn saved_model_read_errors_preserve_io_causes_and_original_files() {
    let dir = tempfile::tempdir().expect("temporary save directory");
    let directory = dir.path().join("directory.json");
    fs::create_dir_all(&directory).expect("directory in place of save");
    let non_utf8 = dir.path().join("non-utf8.json");
    fs::write(&non_utf8, [0xff]).expect("write invalid UTF-8");
    for path in [&directory, &non_utf8] {
        let expected = fs::read_to_string(path).expect_err("unreadable as text");
        let error = DefaultAndeanCondor::load(path).expect_err("return read error");
        assert!(
            error.to_string().contains(&path.display().to_string()),
            "{error:#}"
        );
        let cause = error.downcast_ref::<std::io::Error>().expect("real I/O cause");
        assert_eq!(cause.kind(), expected.kind());
        assert_eq!(cause.raw_os_error(), expected.raw_os_error());
        assert!(error.downcast_ref::<serde_json::Error>().is_none());
    }
    assert_eq!(
        fs::read(&non_utf8).expect("unchanged invalid UTF-8"),
        [0xff]
    );
    assert_eq!(
        fs::read_dir(&directory).expect("unchanged directory").count(),
        0
    );
    assert_eq!(fs::read_dir(dir.path()).expect("save directory").count(), 2);
}
