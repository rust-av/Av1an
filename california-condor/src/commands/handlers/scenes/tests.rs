use std::{path::PathBuf, time::SystemTime};

use andean_condor::{
    ffmpeg::FFPixelFormat,
    models::{
        encoder::{EncoderBase, EncoderPasses, cli_parameter::CLIParameter},
        scene::SubScene,
        sequence::{noise_detector::NoiseDetectorData, noise_scaler::NoiseScalerData},
    },
};
use serde_json::json;

use super::*;
use crate::{
    commands::EncoderMethod,
    configuration::Configuration,
    scenes::overlapping_scenes,
    test_helpers::{TestVideo, default_config},
};

fn test_video() -> TestVideo {
    TestVideo {
        path: PathBuf::from("video.mkv"),
        width: 1920,
        height: 1080,
        frames: 720,
        fps_rational: (24, 1),
        format: FFPixelFormat::YUV420P,
        scenes: vec![(0, 121), (121, 240), (240, 480), (480, 528), (528, 720)],
    }
}

/// A saved configuration whose scenes a detection run produced.
fn configuration(temp: &Path) -> Configuration {
    let video = test_video();
    let mut configuration = default_config(&video, &temp.join("out.mkv"), temp);
    configuration.condor.scenes = video.mock_scenes(&configuration.condor.encoder);

    configuration
}

fn model_configuration() -> Configuration {
    let root = std::env::temp_dir().join("condor-scene-model-only");
    let mut video = test_video();
    video.path = root.join("video.mkv");
    let mut configuration = default_config(&video, &root.join("out.mkv"), &root);
    configuration.condor.scenes = video.mock_scenes(&configuration.condor.encoder);
    configuration
}

fn ranges(configuration: &Configuration) -> Vec<(usize, usize)> {
    configuration
        .condor
        .scenes
        .iter()
        .map(|scene| (scene.start_frame, scene.end_frame))
        .collect()
}

fn x265() -> EncoderMethod {
    EncoderMethod::X265
}

mod internal_integration {
    use super::*;

    fn check_failed_edit_save(validation: bool) {
        let temp = tempfile::tempdir().expect("fixture");
        let path = temp.path().join("condor.json");
        let mut config = configuration(temp.path());
        config.condor.scenes[0].sequence_data.parallel_encoder.bytes = Some(123);
        config.save(&path).expect("original");
        let json = std::fs::read(&path).expect("original bytes");
        let before = config.condor.scenes.clone();
        config.condor.scenes[0].encoder.set_quantizer(19.0);
        let candidate = serde_json::to_value(&config).expect("candidate");
        let blocked = temp.path().join("not-a-file");
        std::fs::create_dir_all(&blocked).expect("publication barrier");
        std::fs::write(blocked.join("user-sentinel"), b"preserve").expect("sentinel");
        // Real Configuration::save stages a sibling then fails publishing over a directory.
        assert!(save_edit(&config, &blocked, &before).is_err());
        assert_eq!(
            serde_json::to_value(&config).expect("no mutation"),
            candidate
        );
        assert_eq!(std::fs::read(&path).expect("original JSON"), json);
        assert_eq!(
            std::fs::read(blocked.join("user-sentinel")).expect("sentinel"),
            b"preserve"
        );
        if !validation {
            return;
        }
        config.condor.sequence_config.parallel_encoder.max_tries = 0;
        assert!(save_edit(&config, &path, &before).is_err());
        assert_eq!(std::fs::read(&path).expect("failed validation"), json);
        assert!(
            temp.path().read_dir().expect("files").all(|e| !e
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .starts_with(".tmp"))
        );
    }

    #[test]
    fn scene_edit_publication_failure_preserves_caller_saved_bytes_and_directory_sentinel() {
        check_failed_edit_save(false);
    }

    #[test]
    fn scene_edit_invalid_retry_budget_preserves_saved_bytes_and_removes_staging_files() {
        check_failed_edit_save(true);
    }

    fn check_publication_proofs(publish: bool) {
        use crate::zoner::{Zoner, ZonerConfig, parse_zone};
        use andean_condor::{
            core::sequence::{Sequence, noise_scaler::NoiseScaler},
            models::{
                encoder::photon_noise::PhotonNoise, sequence::noise_scaler::NoiseScalerConfig,
            },
        };
        let temp = tempfile::tempdir().expect("fixture");
        let video = crate::test_helpers::get_test_video();
        let mut config = default_config(&video, &temp.path().join("out.mkv"), temp.path());
        config.condor.encoder.set_photon_noise(Some(PhotonNoise {
            iso: 801,
            chroma_iso: Some(403),
            width: None,
            height: None,
            c_y: None,
            ccb: None,
            ccr: None,
        }));
        config.condor.scenes = video.mock_scenes(&config.condor.encoder);
        for scene in &mut config.condor.scenes {
            scene.sequence_data.noise_detection = Some(NoiseDetectorData {
                noise: 0.6,
                luminance: 0.3,
                created_on: SystemTime::UNIX_EPOCH,
            });
        }
        config.condor.sequence_config.zoner = Some(ZonerConfig {
            zones: vec![parse_zone("0 121 svt-av1 \"--preset 3\"").expect("zone")],
        });
        config.condor.sequence_config.noise_scaler = Some(NoiseScalerConfig {
            threshold: 0.2,
            minimum_scaler: 0.5,
            maximum_scaler: 0.5,
            scale_chroma: true,
        });
        let mut job = config
            .instantiate_condor(Box::new(|_| Ok(())), &Default::default())
            .expect("native job");
        Zoner::run(&mut job, Default::default()).expect("owning original zoning proof");
        let (tx, _rx) = std::sync::mpsc::channel();
        NoiseScaler::default()
            .execute(
                &mut job,
                tx,
                andean_condor::utils::cancellation::Cancellation::default(),
            )
            .expect("owning scaling proof");
        config.condor = job.as_data();
        let path = temp.path().join("condor.json");
        config.save(&path).expect("original JSON");
        let json = std::fs::read(&path).expect("bytes");
        let before = config.condor.scenes.clone();
        let proof = serde_json::to_value(&before[0].sequence_data.zoner.lifecycle)
            .expect("original lifecycle");
        config.condor.scenes.remove(0);
        config.condor.scenes[0].end_frame -= 1;
        let candidate = serde_json::to_value(&config).expect("edited caller");
        let blocked = temp.path().join("not-a-file");
        std::fs::create_dir_all(&blocked).expect("publication barrier");
        std::fs::write(blocked.join("sentinel"), b"preserve").expect("sentinel");
        assert!(save_edit(&config, &blocked, &before).is_err());
        assert_eq!(
            serde_json::to_value(&config).expect("unchanged caller"),
            candidate
        );
        assert_eq!(std::fs::read(&path).expect("old JSON"), json);
        assert_eq!(
            std::fs::read(blocked.join("sentinel")).expect("preserved sentinel"),
            b"preserve"
        );
        if !publish {
            return;
        }
        save_edit(&config, &path, &before).expect("publish all staged changes");
        let saved = Configuration::load(&path).expect("load").expect("saved");
        assert_eq!(
            serde_json::to_value(&saved.condor.scenes[0].sequence_data.zoner.lifecycle)
                .expect("relocated"),
            proof
        );
        assert_eq!(
            saved.condor.scenes[0].encoder.photon_noise().expect("exact original ISO").iso,
            801
        );
        assert_eq!(
            config.condor.scenes[0].encoder.photon_noise().expect("caller still scaled").iso,
            401
        );
        assert!(saved.condor.scenes[0].sequence_data.noise_detection.is_none());
        assert!(saved.condor.scenes[0].sequence_data.noise_scaling.is_none());
    }

    #[test]
    fn scene_edit_failed_publication_preserves_zoning_anchor_saved_bytes_and_scaled_caller_iso() {
        check_publication_proofs(false);
    }

    #[test]
    fn scene_edit_successful_save_relocates_anchor_restores_original_iso_and_withdraws_noise_results()
     {
        check_publication_proofs(true);
    }

    #[test]
    fn scene_encoder_offline_edit_save_and_info_preserve_requested_executable_identity() {
        use crate::commands::{
            config::ConfigCommand,
            handlers::encode::{EncodeCrud, EncodeKey, EncodeSet},
        };

        let temp = tempfile::tempdir().expect("fixture");
        let mut configuration = configuration(temp.path());
        let requested = PathBuf::from("missing-custom-encoder");
        EncodeCrud::apply(
            &mut configuration,
            &EncodeSet::Executable {
                path: requested.clone(),
            },
        )
        .expect("offline global edit");
        assert_eq!(
            EncodeCrud::render(&configuration, Some(&EncodeKey::Executable)).expect("info"),
            json!("missing-custom-encoder")
        );
        apply_set(
            &mut configuration,
            0,
            &ScenesSet::Executable {
                path: requested.clone(),
            },
        )
        .expect("offline scene edit");
        apply_set(
            &mut configuration,
            1,
            &ScenesSet::Encoder {
                encoder: EncoderMethod::VVenC,
                executable: Some(PathBuf::from("./offline tools 映像/vvenc")),
                passes: None,
                params: None,
                iso: None,
                chroma_iso: None,
                ffmpeg_filter: None,
                append_color_data: None,
            },
        )
        .expect("offline native encoder override");
        let file = temp.path().join("condor.json");
        configuration.save(&file).expect("save");
        let saved = Configuration::load(&file).expect("load").expect("saved config");
        assert_eq!(saved.condor.encoder.executable(), requested);
        assert_eq!(saved.condor.scenes[0].encoder.executable(), requested);
        assert_eq!(
            saved.condor.scenes[1].encoder.executable().as_os_str(),
            std::ffi::OsStr::new("./offline tools 映像/vvenc")
        );
        let info = selection_value(&saved.condor.scenes, &[1], Some(&ScenesKey::Encoder), true)
            .expect("scene info");
        assert_eq!(
            info["VVenC"]["executable"],
            json!("./offline tools 映像/vvenc")
        );
    }
}

#[test]
fn setting_frames_moves_the_range_and_keeps_the_neighbours() {
    let mut configuration = model_configuration();

    let (name, value) = apply_set(&mut configuration, 0, &ScenesSet::Start { frame: 23 })
        .expect("the start frame moves");
    assert_eq!(name, "start_frame");
    assert_eq!(value, json!({"start_frame": 23, "end_frame": 121}));

    let (name, _) = apply_set(&mut configuration, 4, &ScenesSet::End { frame: 600 })
        .expect("the end frame moves");
    assert_eq!(name, "end_frame");
    assert_eq!(
        ranges(&configuration),
        vec![(23, 121), (121, 240), (240, 480), (480, 528), (528, 600)],
        "only the addressed scenes change"
    );
}

#[test]
fn scene_start_edit_accepts_overlap_and_names_the_neighbor() {
    let mut configuration = model_configuration();

    let (_, _) = apply_set(&mut configuration, 1, &ScenesSet::Start { frame: 50 })
        .expect("the start frame moves");

    assert_eq!(
        ranges(&configuration)[1],
        (50, 240),
        "the edit itself is accepted"
    );
    assert_eq!(
        overlapping_scenes(&configuration.condor.scenes, 1, 50, 240),
        vec![0],
        "the overlap the warning names"
    );
}

#[test]
fn a_frame_edit_that_would_empty_a_scene_is_rejected() {
    let mut configuration = model_configuration();

    let error = apply_set(&mut configuration, 1, &ScenesSet::Start { frame: 240 })
        .expect_err("the scene would be empty");
    assert!(
        error.to_string().contains("scene 1 would be empty"),
        "{error}"
    );
    assert_eq!(
        ranges(&configuration)[1],
        (121, 240),
        "a rejected edit leaves the scene alone"
    );
}

#[test]
fn encoder_edits_touch_only_their_scene() {
    let mut configuration = model_configuration();
    let global = configuration.condor.encoder.clone();

    apply_set(
        &mut configuration,
        2,
        &ScenesSet::Encoder {
            encoder: x265(),
            executable: None,
            passes: None,
            params: None,
            iso: None,
            chroma_iso: None,
            ffmpeg_filter: None,
            append_color_data: None,
        },
    )
    .expect("the encoder switches");
    apply_set(
        &mut configuration,
        2,
        &ScenesSet::Params {
            params: "crf=18".to_owned(),
        },
    )
    .expect("the parameter merges");
    apply_set(&mut configuration, 2, &ScenesSet::Passes { value: 2 }).expect("the passes change");

    assert_eq!(
        configuration.condor.scenes[2].encoder.base(),
        EncoderBase::X265
    );
    assert_eq!(
        configuration.condor.scenes[2].encoder.parameters().get("crf"),
        Some(&CLIParameter::new_number("", "=", 18.0)),
        "the parameter merged into the scene's encoder"
    );
    assert_eq!(
        configuration.condor.scenes[2].encoder.passes(),
        EncoderPasses::All(2)
    );
    assert!(
        configuration
            .condor
            .scenes
            .iter()
            .enumerate()
            .all(|(index, scene)| { index == 2 || scene.encoder == global }),
        "the other scenes keep the global encoder"
    );
}

#[test]
fn scene_encoder_edit_applies_codec_passes_parameters_noise_and_filter_together() {
    let mut configuration = model_configuration();

    let (name, value) = apply_set(
        &mut configuration,
        1,
        &ScenesSet::Encoder {
            encoder: EncoderMethod::SVTAV1,
            executable: None,
            passes: Some(1),
            params: Some("crf=18".to_owned()),
            iso: Some(800),
            chroma_iso: Some(800),
            ffmpeg_filter: Some("hflip".to_owned()),
            append_color_data: None,
        },
    )
    .expect("the encoder and its settings change at once");

    assert_eq!(name, "encoder");
    assert!(value.get("SVTAV1").is_some(), "the encoder object: {value}");
    let encoder = &configuration.condor.scenes[1].encoder;
    assert_eq!(encoder.base(), EncoderBase::SVTAV1);
    assert_eq!(encoder.passes(), EncoderPasses::All(1));
    assert_eq!(
        encoder.parameters().get("crf"),
        Some(&CLIParameter::new_number("", "=", 18.0))
    );
    let photon_noise = encoder.photon_noise().expect("Photon Noise was set");
    assert_eq!(photon_noise.iso, 800);
    assert_eq!(photon_noise.chroma_iso, Some(800));
    assert_eq!(encoder.ffmpeg_filter(), Some("hflip"));
}

#[test]
fn encoder_capability_rules_apply_to_the_new_encoder() {
    let mut configuration = model_configuration();

    let photon_noise = apply_set(
        &mut configuration,
        0,
        &ScenesSet::Encoder {
            encoder: x265(),
            executable: None,
            passes: None,
            params: None,
            iso: Some(800),
            chroma_iso: None,
            ffmpeg_filter: None,
            append_color_data: None,
        },
    )
    .expect_err("x265 has no Photon Noise table");
    assert!(
        photon_noise.to_string().contains("does not support Photon Noise"),
        "{photon_noise}"
    );
    assert_eq!(
        configuration.condor.scenes[0].encoder.base(),
        EncoderBase::SVTAV1,
        "a rejected switch leaves the encoder alone"
    );

    let passes = apply_set(
        &mut configuration,
        1,
        &ScenesSet::Encoder {
            encoder: EncoderMethod::FFmpeg,
            executable: None,
            passes: Some(2),
            params: None,
            iso: None,
            chroma_iso: None,
            ffmpeg_filter: None,
            append_color_data: None,
        },
    )
    .expect_err("FFmpeg is single-pass");
    assert!(passes.to_string().contains("single-pass only"), "{passes}");

    let chroma = apply_set(
        &mut configuration,
        1,
        &ScenesSet::Encoder {
            encoder: EncoderMethod::SVTAV1,
            executable: None,
            passes: None,
            params: None,
            iso: None,
            chroma_iso: Some(200),
            ffmpeg_filter: None,
            append_color_data: None,
        },
    )
    .expect_err("chroma ISO needs a luma ISO");
    assert!(
        chroma.to_string().contains("chroma Photon Noise requires luma Photon Noise"),
        "{chroma}"
    );
}

#[test]
fn scene_standalone_passes_filter_and_noise_edits_apply_with_vbr_pass_validation() {
    let mut configuration = model_configuration();

    apply_set(&mut configuration, 3, &ScenesSet::Passes { value: 2 })
        .expect_err("CRF cannot become two-pass");
    apply_set(
        &mut configuration,
        3,
        &ScenesSet::Params {
            params: "--rc 1 --tbr 1000 --keyint 48".into(),
        },
    )
    .expect("switch to VBR");
    apply_set(&mut configuration, 3, &ScenesSet::Passes { value: 2 }).expect("VBR passes change");
    apply_set(
        &mut configuration,
        0,
        &ScenesSet::FfmpegFilter {
            filter: "hflip".to_owned(),
        },
    )
    .expect("the filter applies");
    apply_set(
        &mut configuration,
        1,
        &ScenesSet::PhotonNoise {
            iso: 800,
            chroma_noise: Some(800),
        },
    )
    .expect("the Photon Noise table is set");

    assert_eq!(
        configuration.condor.scenes[3].encoder.passes(),
        EncoderPasses::All(2)
    );
    assert_eq!(
        configuration.condor.scenes[0].encoder.ffmpeg_filter(),
        Some("hflip")
    );
    assert_eq!(
        configuration.condor.scenes[1]
            .encoder
            .photon_noise()
            .expect("Photon Noise")
            .chroma_iso,
        Some(800)
    );
}

fn check_subscene_edit(phase: u8) {
    assert!((1..=3).contains(&phase));
    let mut configuration = model_configuration();

    apply_set(
        &mut configuration,
        1,
        &ScenesSet::SubScenes {
            ranges: vec!["121-180".to_owned(), "180-240".to_owned()],
        },
    )
    .expect("the sub-scenes replace");
    assert_eq!(
        configuration.condor.scenes[1].sub_scenes,
        Some(vec![
            SubScene {
                start_frame: 121,
                end_frame: 180,
            },
            SubScene {
                start_frame: 180,
                end_frame: 240,
            },
        ])
    );

    if phase == 1 {
        return;
    }
    let outside = apply_set(
        &mut configuration,
        1,
        &ScenesSet::SubScenes {
            ranges: vec!["0-50".to_owned()],
        },
    )
    .expect_err("the range leaves the scene");
    assert!(
        outside.to_string().contains("lies outside scene 121..240"),
        "{outside}"
    );

    if phase == 2 {
        return;
    }
    apply_set(
        &mut configuration,
        1,
        &ScenesSet::SubScenes {
            ranges: vec!["none".to_owned()],
        },
    )
    .expect("none clears");
    assert_eq!(configuration.condor.scenes[1].sub_scenes, None);
}

#[test]
fn scene_subscene_edit_replaces_ranges_inside_the_scene() {
    check_subscene_edit(1);
}

#[test]
fn scene_subscene_edit_rejects_ranges_outside_the_scene() {
    check_subscene_edit(2);
}

#[test]
fn scene_subscene_none_edit_clears_existing_ranges() {
    check_subscene_edit(3);
}

#[test]
fn frame_edits_clip_sub_scenes() {
    let mut configuration = model_configuration();
    apply_set(
        &mut configuration,
        1,
        &ScenesSet::SubScenes {
            ranges: vec!["121-180".to_owned(), "180-240".to_owned()],
        },
    )
    .expect("the sub-scenes replace");

    apply_set(&mut configuration, 1, &ScenesSet::Start { frame: 200 })
        .expect("the start frame moves");

    assert_eq!(
        configuration.condor.scenes[1].sub_scenes,
        Some(vec![SubScene {
            start_frame: 200,
            end_frame: 240,
        }]),
        "the sub-scenes are clipped to the new range"
    );
}

#[test]
fn scene_reset_restores_global_encoder_clears_subscenes_and_keeps_frame_range() {
    let mut configuration = model_configuration();
    let global = configuration.condor.encoder.clone();

    apply_set(
        &mut configuration,
        2,
        &ScenesSet::Encoder {
            encoder: x265(),
            executable: None,
            passes: None,
            params: None,
            iso: None,
            chroma_iso: None,
            ffmpeg_filter: None,
            append_color_data: None,
        },
    )
    .expect("the encoder switches");
    apply_set(
        &mut configuration,
        2,
        &ScenesSet::SubScenes {
            ranges: vec!["240-300".to_owned()],
        },
    )
    .expect("the sub-scenes replace");

    let (name, _) =
        reset_key(&mut configuration, 2, ScenesKey::Encoder, &global).expect("the encoder resets");
    assert_eq!(name, "encoder");
    assert_eq!(
        configuration.condor.scenes[2].encoder, global,
        "the scene's encoder is the configuration's again"
    );

    reset_key(&mut configuration, 2, ScenesKey::SubScenes, &global).expect("the sub-scenes reset");
    assert_eq!(configuration.condor.scenes[2].sub_scenes, None);

    apply_set(
        &mut configuration,
        0,
        &ScenesSet::Encoder {
            encoder: x265(),
            executable: None,
            passes: None,
            params: None,
            iso: None,
            chroma_iso: None,
            ffmpeg_filter: None,
            append_color_data: None,
        },
    )
    .expect("the encoder switches");
    let _ = reset_scene(&mut configuration, 0, &global).expect("the scene resets");
    assert_eq!(configuration.condor.scenes[0].encoder, global);
    assert_eq!(configuration.condor.scenes[0].sub_scenes, None);
    assert_eq!(
        ranges(&configuration)[0],
        (0, 121),
        "a scene reset keeps the frame range"
    );

    let error = reset_key(&mut configuration, 0, ScenesKey::Frames, &global)
        .expect_err("frames come from detection");
    assert!(error.to_string().contains("has no default"), "{error}");
}

#[test]
fn scene_result_resets_clear_measurements_but_preserve_zoning_proof_and_encoder() {
    let mut configuration = model_configuration();
    let global = configuration.condor.encoder.clone();
    let scene = &mut configuration.condor.scenes[0];
    scene.sequence_data.noise_detection = Some(NoiseDetectorData {
        noise: 1.0,
        luminance: 2.0,
        created_on: SystemTime::now(),
    });
    scene.sequence_data.noise_scaling = Some(NoiseScalerData {
        scaler: 1.5,
        ..Default::default()
    });
    scene.sequence_data.parallel_encoder.bytes = Some(123);
    scene.sequence_data.quality_check.quality.scores = vec![99.0];
    scene.sequence_data.target_quality.passes.push(Default::default());
    scene.sequence_data.zoner.zoned = true;

    for (key, name) in [
        (ScenesKey::NoiseDetection, "noise-detection"),
        (ScenesKey::NoiseScaling, "noise-scaling"),
        (ScenesKey::ParallelEncoder, "parallel-encoder"),
        (ScenesKey::TargetQuality, "target-quality"),
        (ScenesKey::QualityCheck, "quality-check"),
        (ScenesKey::SceneDetection, "scene-detection"),
    ] {
        let (changed, _) = reset_key(&mut configuration, 0, key, &global)
            .unwrap_or_else(|error| panic!("{name} resets: {error}"));
        assert_eq!(changed, name, "the reset is reported under its own name");
    }

    assert!(
        reset_key(&mut configuration, 0, ScenesKey::Zoner, &global).is_err(),
        "erasing unknown zoning proof must not authorize layering new overrides"
    );
    let data = &configuration.condor.scenes[0].sequence_data;
    assert!(data.noise_detection.is_none());
    assert!(data.noise_scaling.is_none());
    assert_eq!(data.parallel_encoder.bytes, None);
    assert!(data.target_quality.passes.is_empty());
    assert!(data.quality_check.quality.scores.is_empty());
    assert!(data.scene_detection.scenecut_scores.is_none());
    assert!(
        data.zoner.zoned,
        "zoning proof remains until the whole scene list is reset/redetected"
    );
    assert_eq!(
        configuration.condor.scenes[0].encoder, global,
        "resetting results leaves the encoder alone"
    );
}

#[test]
fn info_renders_the_selection_and_every_scene_part() {
    let mut configuration = model_configuration();
    configuration.condor.scenes[2].sequence_data.parallel_encoder.bytes = Some(9);

    let scenes = &configuration.condor.scenes;

    let all = selection_value(scenes, &(0..scenes.len()).collect::<Vec<_>>(), None, false)
        .expect("the list renders");
    assert_eq!(all.as_array().expect("an array").len(), 5);

    let one = selection_value(scenes, &[2], None, true).expect("one scene renders");
    assert_eq!(one["start_frame"], json!(240));

    let many = selection_value(scenes, &[0, 2], None, false).expect("two scenes render");
    assert_eq!(many.as_array().expect("an array").len(), 2);

    let frames =
        selection_value(scenes, &[2], Some(&ScenesKey::Frames), true).expect("frames render");
    assert_eq!(
        frames,
        json!({
            "start_frame": 240,
            "end_frame":   480,
        })
    );

    let encoder =
        selection_value(scenes, &[2], Some(&ScenesKey::Encoder), true).expect("encoder renders");
    assert!(
        encoder.get("SVTAV1").is_some(),
        "the encoder object: {encoder}"
    );

    let bytes = selection_value(scenes, &[2], Some(&ScenesKey::ParallelEncoder), true)
        .expect("progress renders");
    assert_eq!(bytes["bytes"], json!(9), "saved results are reportable");

    let sub_scenes = selection_value(scenes, &[2], Some(&ScenesKey::SubScenes), true)
        .expect("sub-scenes render");
    assert_eq!(sub_scenes, Value::Null);

    let error = selection_value(scenes, &[7], None, true).expect_err("there are five scenes");
    assert!(
        error.to_string().contains("scene index 7 does not exist"),
        "{error}"
    );
}

#[test]
fn deleting_scenes_reports_what_went_and_keeps_the_rest() {
    let mut configuration = model_configuration();

    let removed = remove_scenes(&mut configuration, &[0, 2]).expect("the scenes go");
    assert_eq!(removed.len(), 2, "both addressed scenes are reported");
    assert_eq!(removed[0]["start_frame"], json!(0));
    assert_eq!(removed[1]["start_frame"], json!(240));
    assert_eq!(
        ranges(&configuration),
        vec![(121, 240), (480, 528), (528, 720)],
        "the remaining scenes keep their order"
    );
}

fn check_scene_insertion(phase: u8) {
    assert!((1..=3).contains(&phase));
    let mut configuration = model_configuration();
    let global = configuration.condor.encoder.clone();

    let position =
        add_scene(&mut configuration, 121, 200, Placement::Sorted).expect("the scene is added");
    assert_eq!(position, 1, "an equal start goes first");
    assert_eq!(
        ranges(&configuration)[1],
        (121, 200),
        "the new scene took the slot"
    );
    assert_eq!(
        configuration.condor.scenes[1].encoder, global,
        "a new scene starts from the configuration's encoder"
    );

    if phase == 1 {
        return;
    }
    assert_eq!(
        add_scene(&mut configuration, 900, 1000, Placement::Append).expect("appended"),
        6
    );
    assert_eq!(
        add_scene(&mut configuration, 0, 10, Placement::Prepend).expect("prepended"),
        0
    );
    assert_eq!(
        add_scene(&mut configuration, 300, 400, Placement::After(4)).expect("placed after 4"),
        5
    );
    assert_eq!(
        ranges(&configuration),
        vec![
            (0, 10),
            (0, 121),
            (121, 200),
            (121, 240),
            (240, 480),
            (300, 400),
            (480, 528),
            (528, 720),
            (900, 1000),
        ],
        "every placement landed where it was asked to"
    );

    if phase == 2 {
        return;
    }
    let empty = add_scene(&mut configuration, 10, 10, Placement::Sorted)
        .expect_err("an empty range is rejected");
    assert!(empty.to_string().contains("is empty"), "{empty}");
    let missing = add_scene(&mut configuration, 10, 20, Placement::After(99))
        .expect_err("there is no scene 99");
    assert!(
        missing.to_string().contains("scene index 99 does not exist"),
        "{missing}"
    );
}

#[test]
fn scene_sorted_insert_precedes_equal_start_and_uses_global_encoder() {
    check_scene_insertion(1);
}

#[test]
fn scene_explicit_append_prepend_and_after_placements_are_honored() {
    check_scene_insertion(2);
}

#[test]
fn scene_add_empty_range_or_missing_after_index_returns_error() {
    check_scene_insertion(3);
}

#[test]
fn an_edit_without_scenes_names_the_missing_index() {
    let root = std::env::temp_dir().join("condor-empty-scenes-model-only");
    let mut video = test_video();
    video.path = root.join("video.mkv");
    let mut configuration = default_config(&video, &root.join("out.mkv"), &root);

    let error = apply_set(&mut configuration, 0, &ScenesSet::Start { frame: 5 })
        .expect_err("there are no scenes yet");
    assert!(
        error.to_string().contains("scene index 0 does not exist"),
        "{error}"
    );
}

#[test]
fn labels_name_one_scene_or_a_span() {
    assert_eq!(label(&[2]), "scene 2");
    assert_eq!(label(&[0, 1, 2, 3, 4]), "scenes 0-4");
    assert_eq!(label(&[1, 3]), "scenes 1,3");
}
