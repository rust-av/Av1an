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
        path:         PathBuf::from("video.mkv"),
        width:        1920,
        height:       1080,
        frames:       720,
        fps_rational: (24, 1),
        format:       FFPixelFormat::YUV420P,
        scenes:       vec![(0, 121), (121, 240), (240, 480), (480, 528), (528, 720)],
    }
}

/// A saved configuration whose scenes a detection run produced.
fn configuration(temp: &Path) -> Configuration {
    let video = test_video();
    let mut configuration = default_config(&video, &temp.join("out.mkv"), temp);
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

#[test]
fn setting_frames_moves_the_range_and_keeps_the_neighbours() {
    let temp = tempfile::tempdir().expect("temp directory");
    let mut configuration = configuration(temp.path());

    let (name, value) = apply_set(&mut configuration, 0, &ScenesSet::Start {
        frame: 23
    })
    .expect("the start frame moves");
    assert_eq!(name, "start_frame");
    assert_eq!(value, json!({"start_frame": 23, "end_frame": 121}));

    let (name, _) = apply_set(&mut configuration, 4, &ScenesSet::End {
        frame: 600
    })
    .expect("the end frame moves");
    assert_eq!(name, "end_frame");
    assert_eq!(
        ranges(&configuration),
        vec![(23, 121), (121, 240), (240, 480), (480, 528), (528, 600)],
        "only the addressed scenes change"
    );
}

#[test]
fn setting_a_frame_into_a_neighbour_reports_the_overlap() {
    let temp = tempfile::tempdir().expect("temp directory");
    let mut configuration = configuration(temp.path());

    let (_, _) = apply_set(&mut configuration, 1, &ScenesSet::Start {
        frame: 50
    })
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
    let temp = tempfile::tempdir().expect("temp directory");
    let mut configuration = configuration(temp.path());

    let error = apply_set(&mut configuration, 1, &ScenesSet::Start {
        frame: 240
    })
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
    let temp = tempfile::tempdir().expect("temp directory");
    let mut configuration = configuration(temp.path());
    let global = configuration.condor.encoder.clone();

    apply_set(&mut configuration, 2, &ScenesSet::Encoder {
        encoder:           x265(),
        executable:        None,
        passes:            None,
        params:            None,
        iso:               None,
        chroma_iso:        None,
        ffmpeg_filter:     None,
        append_color_data: None,
    })
    .expect("the encoder switches");
    apply_set(&mut configuration, 2, &ScenesSet::Params {
        params: "crf=18".to_owned(),
    })
    .expect("the parameter merges");
    apply_set(&mut configuration, 2, &ScenesSet::Passes {
        value: 2
    })
    .expect("the passes change");

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
fn one_set_encoder_line_applies_every_flag() {
    let temp = tempfile::tempdir().expect("temp directory");
    let mut configuration = configuration(temp.path());

    let (name, value) = apply_set(&mut configuration, 1, &ScenesSet::Encoder {
        encoder:           EncoderMethod::SVTAV1,
        executable:        None,
        passes:            Some(2),
        params:            Some("crf=18".to_owned()),
        iso:               Some(800),
        chroma_iso:        Some(200),
        ffmpeg_filter:     Some("hflip".to_owned()),
        append_color_data: None,
    })
    .expect("the encoder and its settings change at once");

    assert_eq!(name, "encoder");
    assert!(value.get("SVTAV1").is_some(), "the encoder object: {value}");
    let encoder = &configuration.condor.scenes[1].encoder;
    assert_eq!(encoder.base(), EncoderBase::SVTAV1);
    assert_eq!(encoder.passes(), EncoderPasses::All(2));
    assert_eq!(
        encoder.parameters().get("crf"),
        Some(&CLIParameter::new_number("", "=", 18.0))
    );
    let photon_noise = encoder.photon_noise().expect("Photon Noise was set");
    assert_eq!(photon_noise.iso, 800);
    assert_eq!(photon_noise.chroma_iso, Some(200));
    assert_eq!(encoder.ffmpeg_filter(), Some("hflip"));
}

#[test]
fn encoder_capability_rules_apply_to_the_new_encoder() {
    let temp = tempfile::tempdir().expect("temp directory");
    let mut configuration = configuration(temp.path());

    let photon_noise = apply_set(&mut configuration, 0, &ScenesSet::Encoder {
        encoder:           x265(),
        executable:        None,
        passes:            None,
        params:            None,
        iso:               Some(800),
        chroma_iso:        None,
        ffmpeg_filter:     None,
        append_color_data: None,
    })
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

    let passes = apply_set(&mut configuration, 1, &ScenesSet::Encoder {
        encoder:           EncoderMethod::FFmpeg,
        executable:        None,
        passes:            Some(2),
        params:            None,
        iso:               None,
        chroma_iso:        None,
        ffmpeg_filter:     None,
        append_color_data: None,
    })
    .expect_err("FFmpeg is single-pass");
    assert!(passes.to_string().contains("single-pass only"), "{passes}");

    let chroma = apply_set(&mut configuration, 1, &ScenesSet::Encoder {
        encoder:           EncoderMethod::SVTAV1,
        executable:        None,
        passes:            None,
        params:            None,
        iso:               None,
        chroma_iso:        Some(200),
        ffmpeg_filter:     None,
        append_color_data: None,
    })
    .expect_err("chroma ISO needs a luma ISO");
    assert!(
        chroma.to_string().contains("--chroma-iso needs --iso"),
        "{chroma}"
    );
}

#[test]
fn the_standalone_overrides_keep_working() {
    let temp = tempfile::tempdir().expect("temp directory");
    let mut configuration = configuration(temp.path());

    apply_set(&mut configuration, 3, &ScenesSet::Passes {
        value: 2
    })
    .expect("the passes change");
    apply_set(&mut configuration, 0, &ScenesSet::FfmpegFilter {
        filter: "hflip".to_owned(),
    })
    .expect("the filter applies");
    apply_set(&mut configuration, 1, &ScenesSet::PhotonNoise {
        iso:          800,
        chroma_noise: Some(200),
    })
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
        Some(200)
    );
}

#[test]
fn sub_scenes_are_validated_against_their_scene() {
    let temp = tempfile::tempdir().expect("temp directory");
    let mut configuration = configuration(temp.path());

    apply_set(&mut configuration, 1, &ScenesSet::SubScenes {
        ranges: vec!["121-180".to_owned(), "180-240".to_owned()],
    })
    .expect("the sub-scenes replace");
    assert_eq!(
        configuration.condor.scenes[1].sub_scenes,
        Some(vec![
            SubScene {
                start_frame: 121,
                end_frame:   180,
            },
            SubScene {
                start_frame: 180,
                end_frame:   240,
            },
        ])
    );

    let outside = apply_set(&mut configuration, 1, &ScenesSet::SubScenes {
        ranges: vec!["0-50".to_owned()],
    })
    .expect_err("the range leaves the scene");
    assert!(
        outside.to_string().contains("lies outside scene 121..240"),
        "{outside}"
    );

    apply_set(&mut configuration, 1, &ScenesSet::SubScenes {
        ranges: vec!["none".to_owned()],
    })
    .expect("none clears");
    assert_eq!(configuration.condor.scenes[1].sub_scenes, None);
}

#[test]
fn frame_edits_clip_sub_scenes() {
    let temp = tempfile::tempdir().expect("temp directory");
    let mut configuration = configuration(temp.path());
    apply_set(&mut configuration, 1, &ScenesSet::SubScenes {
        ranges: vec!["121-180".to_owned(), "180-240".to_owned()],
    })
    .expect("the sub-scenes replace");

    apply_set(&mut configuration, 1, &ScenesSet::Start {
        frame: 200
    })
    .expect("the start frame moves");

    assert_eq!(
        configuration.condor.scenes[1].sub_scenes,
        Some(vec![SubScene {
            start_frame: 200,
            end_frame:   240,
        }]),
        "the sub-scenes are clipped to the new range"
    );
}

#[test]
fn resets_restore_what_a_detection_would_have_written() {
    let temp = tempfile::tempdir().expect("temp directory");
    let mut configuration = configuration(temp.path());
    let global = configuration.condor.encoder.clone();

    apply_set(&mut configuration, 2, &ScenesSet::Encoder {
        encoder:           x265(),
        executable:        None,
        passes:            None,
        params:            None,
        iso:               None,
        chroma_iso:        None,
        ffmpeg_filter:     None,
        append_color_data: None,
    })
    .expect("the encoder switches");
    apply_set(&mut configuration, 2, &ScenesSet::SubScenes {
        ranges: vec!["240-300".to_owned()],
    })
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

    apply_set(&mut configuration, 0, &ScenesSet::Encoder {
        encoder:           x265(),
        executable:        None,
        passes:            None,
        params:            None,
        iso:               None,
        chroma_iso:        None,
        ffmpeg_filter:     None,
        append_color_data: None,
    })
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
fn sequence_data_resets_clear_the_saved_results() {
    let temp = tempfile::tempdir().expect("temp directory");
    let mut configuration = configuration(temp.path());
    let global = configuration.condor.encoder.clone();
    let scene = &mut configuration.condor.scenes[0];
    scene.sequence_data.noise_detection = Some(NoiseDetectorData {
        noise:      1.0,
        luminance:  2.0,
        created_on: SystemTime::now(),
    });
    scene.sequence_data.noise_scaling = Some(NoiseScalerData {
        scaler: 1.5
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
        (ScenesKey::Zoner, "zoner"),
    ] {
        let (changed, _) = reset_key(&mut configuration, 0, key, &global)
            .unwrap_or_else(|error| panic!("{name} resets: {error}"));
        assert_eq!(changed, name, "the reset is reported under its own name");
    }

    let data = &configuration.condor.scenes[0].sequence_data;
    assert!(data.noise_detection.is_none());
    assert!(data.noise_scaling.is_none());
    assert_eq!(data.parallel_encoder.bytes, None);
    assert!(data.target_quality.passes.is_empty());
    assert!(data.quality_check.quality.scores.is_empty());
    assert!(data.scene_detection.scenecut_scores.is_none());
    assert!(
        !data.zoner.zoned,
        "the zoning record resets like every other sequence-data key"
    );
    assert_eq!(
        configuration.condor.scenes[0].encoder, global,
        "resetting results leaves the encoder alone"
    );
}

#[test]
fn info_renders_the_selection_and_every_scene_part() {
    let temp = tempfile::tempdir().expect("temp directory");
    let mut configuration = configuration(temp.path());
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
    let temp = tempfile::tempdir().expect("temp directory");
    let mut configuration = configuration(temp.path());

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

#[test]
fn adding_scenes_places_them_by_start_frame() {
    let temp = tempfile::tempdir().expect("temp directory");
    let mut configuration = configuration(temp.path());
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
fn an_edit_without_scenes_names_the_missing_index() {
    let temp = tempfile::tempdir().expect("temp directory");
    let video = test_video();
    let mut configuration = default_config(&video, &temp.path().join("out.mkv"), temp.path());

    let error = apply_set(&mut configuration, 0, &ScenesSet::Start {
        frame: 5
    })
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
