use andean_condor::{
    core::sequence::parallel_encoder::ParallelEncoder,
    models::{
        encoder::{Encoder, EncoderBase},
        scene::Scene,
    },
};
use california_condor::{
    configuration::{CliSequenceConfig, CliSequenceData},
    scenes::{affected_indices, invalidate_changed_results, stale_result_warnings},
};

fn scene(start_frame: usize, end_frame: usize) -> Scene<CliSequenceData> {
    Scene {
        start_frame,
        end_frame,
        sub_scenes: None,
        encoder: Encoder::default_from_base(&EncoderBase::SVTAV1, false),
        sequence_data: CliSequenceData::default(),
    }
}

fn scenes() -> Vec<Scene<CliSequenceData>> {
    vec![scene(0, 121), scene(121, 240), scene(240, 480)]
}

fn input() -> andean_condor::models::input::Input {
    andean_condor::models::input::Input::Video {
        path: "unused.mkv".into(),
        import_method: andean_condor::models::input::ImportMethod::FFMS2 {
            index: None,
            cache_path: None,
        },
        filters: vec![],
    }
}

fn check_result_invalidation(phase: u8) {
    assert!((1..=5).contains(&phase));
    use andean_condor::models::sequence::noise_detector::NoiseDetectorData;
    let temp = tempfile::tempdir().expect("fixture");
    let mut config = CliSequenceConfig::default();
    config.parallel_encoder.scenes_directory = temp.path().join("scenes");
    config.scene_concatenator.scenes_directory = temp.path().join("scenes");
    let mut original = scenes();
    for scene in &mut original {
        scene.sequence_data.scene_detection.transnetv2_scores =
            Some([(scene.start_frame, 0.5)].into());
        scene.sequence_data.noise_detection = Some(NoiseDetectorData {
            noise: 2.0,
            luminance: 30.0,
            created_on: std::time::UNIX_EPOCH,
        });
    }
    original[1].sequence_data.parallel_encoder.bytes = Some(123);
    let mut edited = original.clone();
    edited[1].encoder.set_quantizer(19.0);
    assert_eq!(affected_indices(&original, &edited), vec![1]);
    invalidate_changed_results(&original, &mut edited, &config, &input(), "test")
        .expect("invalidate");
    assert!(
        edited[1].sequence_data.noise_detection.is_some()
            && edited[1].sequence_data.noise_scaling.is_none()
    );
    assert!(edited[1].sequence_data.parallel_encoder.invalidated);
    assert_eq!(
        serde_json::to_value(&edited[1].sequence_data.scene_detection).expect("retained"),
        serde_json::to_value(&original[1].sequence_data.scene_detection).expect("original")
    );
    if phase == 1 {
        return;
    }
    let before_reset = edited.clone();
    edited[1].sequence_data.parallel_encoder = Default::default();
    assert!(
        invalidate_changed_results(&before_reset, &mut edited, &config, &input(), "reset")
            .expect("reset")
            .is_empty()
    );
    assert!(
        !edited[1].sequence_data.parallel_encoder.invalidated,
        "explicit reset is not a scene edit"
    );
    if phase == 2 {
        return;
    }
    let mut shifted = original.clone();
    shifted.remove(1);
    assert_eq!(affected_indices(&original, &shifted), vec![1, 2]);
    invalidate_changed_results(&original, &mut shifted, &config, &input(), "delete")
        .expect("shift");
    assert_eq!(
        serde_json::to_value(&shifted[1].sequence_data.scene_detection).expect("mapped"),
        serde_json::to_value(&original[2].sequence_data.scene_detection).expect("range data")
    );
    if phase == 3 {
        return;
    }
    let mut narrowed = original.clone();
    narrowed[1].end_frame = 239;
    invalidate_changed_results(&original, &mut narrowed, &config, &input(), "range")
        .expect("range");
    assert!(narrowed[1].sequence_data.scene_detection.transnetv2_scores.is_none());
    if phase == 4 {
        return;
    }
    let clean = scenes();
    let mut changed = clean.clone();
    changed[1].encoder.set_quantizer(19.0);
    invalidate_changed_results(&clean, &mut changed, &config, &input(), "empty job")
        .expect("empty edit");
    assert!(changed.iter().all(|s| !s.sequence_data.parallel_encoder.invalidated));
}

#[test]
fn encoder_edit_invalidates_saved_encode_but_keeps_raw_noise_and_same_range_detection() {
    check_result_invalidation(1);
}

#[test]
fn explicit_encode_result_reset_does_not_reinvalidate_the_scene() {
    check_result_invalidation(2);
}

#[test]
fn scene_deletion_maps_detection_scores_by_original_range() {
    check_result_invalidation(3);
}

#[test]
fn scene_range_edit_clears_detection_scores() {
    check_result_invalidation(4);
}

#[test]
fn empty_job_encoder_edit_does_not_create_invalidated_results() {
    check_result_invalidation(5);
}

#[test]
fn file_only_shift_blocks_both_occupied_filename_and_moved_source() {
    let temp = tempfile::tempdir().expect("fixture");
    let mut config = CliSequenceConfig::default();
    config.parallel_encoder.scenes_directory = temp.path().to_path_buf();
    config.scene_concatenator.scenes_directory = temp.path().to_path_buf();
    let original = scenes();
    std::fs::write(
        temp.path().join("00002.ivf"),
        b"unclaimed file must not be blessed",
    )
    .expect("old file");
    let mut shifted = original.clone();
    shifted.insert(1, scene(121, 180));
    invalidate_changed_results(&original, &mut shifted, &config, &input(), "insert")
        .expect("invalidate");
    assert!(
        shifted[2].sequence_data.parallel_encoder.invalidated,
        "old filename is occupied"
    );
    assert!(
        shifted[3].sequence_data.parallel_encoder.invalidated,
        "moved range inherited an invalidated old artifact even without saved records"
    );
    assert!(!shifted[0].sequence_data.parallel_encoder.invalidated);
}

#[test]
fn new_index_claim_is_not_misattributed_to_clean_old_range() {
    let temp = tempfile::tempdir().expect("fixture");
    let mut config = CliSequenceConfig::default();
    config.parallel_encoder.scenes_directory = temp.path().join("scenes");
    config.scene_concatenator.scenes_directory = temp.path().join("scenes");
    let mut original = scenes();
    original[1].sequence_data.parallel_encoder.bytes = Some(123);
    let mut shifted = original.clone();
    shifted.insert(1, scene(121, 180));
    invalidate_changed_results(&original, &mut shifted, &config, &input(), "insert")
        .expect("invalidate");
    assert!(shifted[1].sequence_data.parallel_encoder.invalidated);
    assert!(shifted[2].sequence_data.parallel_encoder.invalidated);
    assert!(
        !shifted[3].sequence_data.parallel_encoder.invalidated,
        "new index 2's inherited claim is not an old claim for clean range 240..480"
    );
}

fn check_stale_warnings(phase: u8) {
    assert!((1..=3).contains(&phase));
    let temp = tempfile::tempdir().expect("temp directory");
    let mut config = CliSequenceConfig::default();
    config.parallel_encoder.scenes_directory = temp.path().join("scenes");
    let mut original = scenes();
    original[1].sequence_data.parallel_encoder.bytes = Some(123);
    std::fs::create_dir_all(&config.parallel_encoder.scenes_directory).expect("scenes directory");
    for index in [1, 2] {
        std::fs::write(
            config
                .parallel_encoder
                .scenes_directory
                .join(format!("{}.ivf", ParallelEncoder::scene_id(index))),
            "existing encode",
        )
        .expect("encode fixture");
    }
    let unchanged = stale_result_warnings(&original, &original, &config, "scene edits")
        .expect("warnings compute");
    assert!(unchanged.is_empty(), "nothing changed: {unchanged:?}");
    if phase == 1 {
        return;
    }
    let mut moved = original.clone();
    moved[1].end_frame = 200;
    let warnings = stale_result_warnings(&original, &moved, &config, "scene edits").expect("ok");
    assert!(
        warnings.iter().any(|warning| {
            let warning = warning.to_string();
            warning.contains("scene index 1")
                && warning.contains("encode results")
                && warning.contains("may be stale after scene edits")
        }),
        "saved results warn with the given reason: {warnings:?}"
    );
    assert!(
        !warnings.iter().any(|warning| warning.to_string().contains("scene index 2")),
        "an untouched scene does not warn: {warnings:?}"
    );
    if phase == 2 {
        return;
    }
    let mut shortened = original.clone();
    shortened.pop();
    let warnings =
        stale_result_warnings(&original, &shortened, &config, "scene edits").expect("ok");
    assert!(
        warnings.iter().any(|warning| {
            let warning = warning.to_string();
            warning.contains("scene index 2") && warning.contains("may be stale after scene edits")
        }),
        "a missing scene warns about its file: {warnings:?}"
    );
}

#[test]
fn unchanged_scene_list_has_no_stale_result_warnings() {
    check_stale_warnings(1);
}

#[test]
fn moved_scene_cut_warns_only_for_changed_results_and_names_the_reason() {
    check_stale_warnings(2);
}

#[test]
fn deleted_scene_warns_about_its_existing_artifact() {
    check_stale_warnings(3);
}
