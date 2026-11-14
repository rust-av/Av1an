//! Losing max-tries parsing/forwarding/default preservation must fail here.
use super::*;
use clap::Parser;

#[test]
fn cli_max_tries_accepts_large_positive_budget_on_root_init_encode_and_set() {
    for args in [
        vec!["condor", "--max-tries", "300"],
        vec!["condor", "init", "--max-tries", "300"],
        vec!["condor", "encode", "--max-tries", "300"],
        vec!["condor", "encode", "set", "max-tries", "300"],
    ] {
        CondorCli::try_parse_from(&args).unwrap_or_else(|e| panic!("{args:?}: {e}"));
    }
}

#[test]
fn cli_max_tries_zero_is_rejected_on_root_init_encode_and_set() {
    for args in [
        vec!["condor", "--max-tries", "0"],
        vec!["condor", "init", "--max-tries", "0"],
        vec!["condor", "encode", "--max-tries", "0"],
        vec!["condor", "encode", "set", "max-tries", "0"],
    ] {
        assert!(CondorCli::try_parse_from(&args).is_err(), "{args:?}");
    }
}

#[test]
fn cli_max_tries_property_supports_info_and_reset() {
    for action in ["info", "reset"] {
        CondorCli::try_parse_from(["condor", "encode", action, "max-tries"]).expect("CRUD");
    }
}

#[test]
fn offline_handlers_crud_preserve_and_validate_budget() {
    use crate::{
        commands::{
            config::{ConfigAction, ConfigCommand},
            handlers::{
                encode::{self, EncodeCrud, EncodeKey, EncodeSet},
                init::init_handler,
                start::start_handler,
            },
        },
        configuration::Configuration,
        test_helpers::{TestVideo, default_config},
    };
    let temp = tempfile::tempdir().expect("job");
    let path = temp.path().join("condor.json");
    let video = TestVideo {
        path: temp.path().join("missing-source.vpy"),
        width: 64,
        height: 64,
        frames: 3,
        fps_rational: (24, 1),
        format: andean_condor::ffmpeg::FFPixelFormat::YUV420P,
        scenes: vec![],
    };
    let mut config = default_config(&video, &temp.path().join("out.mkv"), temp.path());
    config.condor.sequence_config.parallel_encoder.max_tries = 7;
    // Offline edits must not resolve this binary or open the nonexistent source.
    config.condor.encoder.set_executable(Some(temp.path().join("missing-encoder")));
    config.save(&path).expect("save model");
    let run_encode = |value| {
        encode::encode_handler(
            Some(&path),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            value,
        )
    };
    assert_eq!(
        run_encode(None)
            .expect("omission")
            .0
            .condor
            .sequence_config
            .parallel_encoder
            .max_tries,
        7
    );
    assert_eq!(
        run_encode(Some(300))
            .expect("explicit")
            .0
            .condor
            .sequence_config
            .parallel_encoder
            .max_tries,
        300
    );
    let before = fs::read(&path).expect("before");
    assert!(run_encode(Some(0)).is_err());
    assert_eq!(fs::read(&path).expect("atomic"), before);
    // Root/start also configures scene detection, whose existing contract reads clip info.
    fs::write(&video.path, "from vapoursynth import core, YUV420P8\ncore.std.BlankClip(width=64,height=64,length=3,format=YUV420P8,fpsnum=24,fpsden=1).set_output()\n").expect("root input");
    let mut config = Configuration::load(&path).expect("load root model").expect("exists");
    config.condor.input = andean_condor::models::input::Input::VapourSynthScript {
        source: andean_condor::models::input::VapourSynthScriptSource::Path(video.path.clone()),
        variables: Default::default(),
        index: 0,
        filters: vec![],
        stream_concurrently: false,
    };
    config.save(&path).expect("root model");
    let run_start = |value| {
        start_handler(
            Some(&path),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            value,
            false,
        )
    };
    assert_eq!(
        run_start(None)
            .expect("root omission")
            .0
            .condor
            .sequence_config
            .parallel_encoder
            .max_tries,
        300
    );
    assert_eq!(
        run_start(Some(2))
            .expect("root explicit")
            .0
            .condor
            .sequence_config
            .parallel_encoder
            .max_tries,
        2
    );
    let before = fs::read(&path).expect("before root zero");
    assert!(run_start(Some(0)).is_err());
    assert_eq!(fs::read(&path).expect("root atomic"), before);
    let mut invalid =
        serde_json::to_value(Configuration::load(&path).expect("load").expect("exists"))
            .expect("JSON");
    invalid["condor"]["sequence_config"]["parallel_encoder"]["max_tries"] = 0.into();
    let invalid = serde_json::to_vec(&invalid).expect("saved zero");
    for root in [false, true] {
        fs::write(&path, &invalid).expect("saved zero config");
        let run = |value| {
            if root {
                run_start(value)
            } else {
                run_encode(value)
            }
        };
        assert!(
            run(None)
                .expect_err("omission cannot repair saved zero")
                .to_string()
                .contains("max_tries")
        );
        assert_eq!(fs::read(&path).expect("unchanged zero"), invalid);
        assert_eq!(
            run(Some(2))
                .expect("explicit positive budget replaces saved zero")
                .0
                .condor
                .sequence_config
                .parallel_encoder
                .max_tries,
            2
        );
        assert_eq!(
            Configuration::load(&path)
                .expect("persisted")
                .expect("exists")
                .condor
                .sequence_config
                .parallel_encoder
                .max_tries,
            2
        );
    }
    encode::config(
        Some(&path),
        &ConfigAction::Set {
            setting: EncodeSet::MaxTries { value: 9 },
        },
    )
    .expect("set");
    let mut config = Configuration::load(&path).expect("load").expect("exists");
    assert_eq!(
        EncodeCrud::render(&config, Some(&EncodeKey::MaxTries)).expect("info"),
        9
    );
    let before = serde_json::to_vec(&config).expect("snapshot");
    assert!(EncodeCrud::apply(&mut config, &EncodeSet::MaxTries { value: 0 }).is_err());
    assert_eq!(serde_json::to_vec(&config).expect("unchanged"), before);
    encode::config(
        Some(&path),
        &ConfigAction::Reset {
            key: Some(EncodeKey::MaxTries),
        },
    )
    .expect("reset");
    let mut config = Configuration::load(&path).expect("load").expect("exists");
    assert_eq!(config.condor.sequence_config.parallel_encoder.max_tries, 3);
    config.condor.sequence_config.parallel_encoder.max_tries = 11;
    EncodeCrud::restore(&mut config, None).expect("section reset");
    assert_eq!(config.condor.sequence_config.parallel_encoder.max_tries, 3);
    config.condor.sequence_config.parallel_encoder.max_tries = 0;
    fs::remove_file(&video.path).expect("zero must not open source");
    assert!(config.validate_encoder_settings().is_err());
    assert!(
        config
            .instantiate_condor(Box::new(|_| Ok(())))
            .err()
            .expect("zero before backend IO")
            .to_string()
            .contains("max_tries")
    );
    assert!(config.save(&temp.path().join("must-not-save/config.json")).is_err());
    assert!(!temp.path().join("must-not-save").exists());
    let mut json = serde_json::to_value(&config).expect("invalid saved JSON");
    json["condor"]["sequence_config"]["parallel_encoder"]["max_tries"] = 0.into();
    fs::write(&path, serde_json::to_vec(&json).expect("JSON")).expect("external edit");
    assert!(run_encode(None).expect_err("saved zero").to_string().contains("max_tries"));
    assert!(
        run_start(None)
            .expect_err("saved root zero before source validation")
            .to_string()
            .contains("max_tries")
    );
    let init = temp.path().join("init.json");
    assert!(
        init_handler(
            Some(&init),
            None,
            &video.path,
            &temp.path().join("out.mkv"),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            true,
            None,
            None,
            Some(0),
            false,
        )
        .expect_err("init zero before backend IO")
        .to_string()
        .contains("max_tries")
    );
    assert!(!init.exists());
}

use std::fs;
