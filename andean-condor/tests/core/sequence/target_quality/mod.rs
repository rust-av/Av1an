use crate::core::support::provenance::current_target_fixture;
use andean_condor as condor;
mod execute;
#[cfg(unix)]
use condor::core::input::Input;
#[cfg(unix)]
use condor::{
    metrics::Engine,
    models::sequence::{
        provenance::MetricBackend,
        target_quality::types::{MeasurementError, QualityMetric, QualityPass},
    },
};
#[path = "fixtures/probe_support.rs"]
mod probe_fixture;
#[cfg(unix)]
use probe_fixture::executable;
use probe_fixture::{fixture, probe, tasks};

#[test]
fn actual_probe_retains_bps_in_serialized_history() {
    let (_temp, mut input, encoder, config, directory) = fixture();
    let completed = probe(
        1,
        &directory,
        &config,
        &mut input,
        &tasks(&encoder, &directory, 1),
    )
    .expect("real encode/metric pass");
    for task in completed {
        // Each scene selects two frames at 24 FPS: twelve scenes/second, eight bits/byte.
        let bytes = std::fs::metadata(&task.output).expect("real output").len();
        let expected = bytes as f64 * 96.0;
        assert!((task.passes[0].bitrate - expected).abs() < 1e-8);
        let saved = serde_json::to_value(&task.passes).expect("persistable history");
        assert_eq!(saved[0]["bitrate"].as_f64(), Some(task.passes[0].bitrate));
        assert_eq!(task.passes[0].scores.len(), 2);
    }
}

#[cfg(unix)]
fn check_probe_cleanup(phase: &str) {
    assert!(["collision", "boundaries"].contains(&phase));
    let (temp, mut input, mut encoder, config, directory) = fixture();
    let script = temp.path().join("encoder");
    executable(
        &script,
        "#!/bin/sh\nprintf spawned > \"$0.calls\"\nexit 1\n",
    );
    encoder.set_executable(Some(script.clone()));
    let tasks = tasks(&encoder, &directory, 1);
    std::fs::create_dir_all(&tasks[0].output).expect("directory collision");
    let sentinel = tasks[0].output.join("caller-data");
    std::fs::write(&sentinel, b"preserve").expect("sentinel");
    let error = probe(1, &directory, &config, &mut input, &tasks).expect_err("cleanup must fail");
    eprintln!("probe-resume cleanup owning-path error: {error:#}");
    assert!(
        format!("{error:#}").contains(&tasks[0].output.display().to_string()),
        "{error:#}"
    );
    assert!(
        !script.with_extension("calls").exists(),
        "cleanup must precede any encoder spawn"
    );
    assert_eq!(
        std::fs::read(sentinel).expect("preserved directory"),
        b"preserve"
    );
    if phase == "collision" {
        return;
    }
    let mut escaped = tasks.clone();
    escaped[1].output = temp.path().join("source.mkv");
    assert!(probe(1, &directory, &config, &mut input, &escaped).is_err());
    assert!(
        escaped[1].output.exists(),
        "never delete outside the owned pass"
    );
    let detached = temp.path().join("caller-directory");
    std::fs::rename(directory.join("1"), &detached).expect("move scratch");
    std::os::unix::fs::symlink(&detached, directory.join("1")).expect("pass-dir link");
    assert!(probe(1, &directory, &config, &mut input, &tasks).is_err());
    assert_eq!(
        std::fs::read(detached.join("00000.mkv/caller-data")).expect("linked directory preserved"),
        b"preserve"
    );
    assert!(!script.with_extension("calls").exists());
}

#[cfg(unix)]
#[test]
fn probe_cleanup_failure_reports_output_path_before_any_encoder_spawn() {
    check_probe_cleanup("collision");
}

#[cfg(unix)]
#[test]
fn probe_cleanup_refuses_escaped_or_symlinked_output_directories_without_deleting_targets() {
    check_probe_cleanup("boundaries");
}

#[cfg(unix)]
#[test]
fn cleanup_never_deletes_an_input_even_inside_pass_scratch() {
    let (temp, mut input, mut encoder, config, directory) = fixture();
    let script = temp.path().join("encoder");
    executable(
        &script,
        "#!/bin/sh\nprintf spawned > \"$0.calls\"\nexit 1\n",
    );
    encoder.set_executable(Some(script.clone()));
    let tasks = tasks(&encoder, &directory, 1);
    std::fs::copy(temp.path().join("source.mkv"), &tasks[0].output).expect("source inside scratch");
    if let Input::Video { path, .. } = &mut input {
        *path = directory.join("1/../1/00000.mkv");
    }
    let before = std::fs::read(&tasks[0].output).expect("source bytes");
    assert!(probe(1, &directory, &config, &mut input, &tasks).is_err());
    assert_eq!(
        std::fs::read(&tasks[0].output).expect("source must be preserved"),
        before
    );
    assert!(!script.with_extension("calls").exists());
}

#[cfg(unix)]
fn check_complete_probe_reuse(phase: &str) {
    assert!(["reuse", "invalid", "empty"].contains(&phase));
    let (temp, mut input, mut encoder, config, directory) = fixture();
    let script = temp.path().join("encoder");
    executable(
        &script,
        "#!/bin/sh\nprintf spawned > \"$0.calls\"\nexit 1\n",
    );
    encoder.set_executable(Some(script.clone()));
    let mut tasks = tasks(&encoder, &directory, 1);
    // Synthetic valid reuse records, not owning-producer certification.
    for task in &mut tasks {
        task.passes.push(QualityPass {
            provenance: None,
            backend: Some(MetricBackend::current(Engine::Vmaf)),
            quantizer: 26.0,
            scores: vec![75.0, 76.0],
            bitrate: 100.0,
            started_on: 12,
            completed_on: 34,
        });
    }
    std::fs::remove_file(temp.path().join("source.mkv")).expect("no retained source");
    for available in [true, false] {
        if !available {
            std::fs::remove_file(&script).expect("unavailable encoder");
        }
        let before = format!("{tasks:?}");
        let completed = probe(1, &directory, &config, &mut input, &tasks)
            .expect("complete validated history is sufficient");
        assert_eq!(format!("{completed:?}"), before);
        assert!(!script.with_extension("calls").exists());
        eprintln!(
            "probe-resume complete reuse: records unchanged, missing outputs/source, encoder available={available}, no spawn"
        );
    }
    if phase == "reuse" {
        return;
    }
    let mut invalid = tasks.clone();
    invalid[1].passes[0].scores.pop();
    assert!(
        probe(1, &directory, &config, &mut input, &invalid)
            .expect_err("strict history")
            .is::<MeasurementError>()
    );
    let mut invalid_config = config.clone();
    *invalid_config.metric.target_range_mut() = (f64::NAN, 75.0);
    assert!(probe(1, &directory, &invalid_config, &mut input, &tasks).is_err());
    let mut temporal = config.clone();
    temporal.metric = QualityMetric::CVVDP {
        target_range: (0.0, 1.0),
        resolution: None,
        display_model: None,
        resize_to_display: None,
        disable_temporal: None,
        gpu_id: None,
    };
    let mut invalid_selection = tasks.clone();
    invalid_selection[0].frame_indices.reverse();
    let error = probe(1, &directory, &temporal, &mut input, &invalid_selection)
        .expect_err("pure temporal selection validation still precedes reuse");
    assert!(
        format!("{error:#}").contains("strictly increasing"),
        "{error:#}"
    );
    if phase == "invalid" {
        return;
    }
    assert!(probe(0, &directory, &config, &mut input, &tasks).is_err());
    assert!(
        probe(1, &directory, &config, &mut input, &[])
            .expect("empty no-work")
            .is_empty()
    );
}

#[cfg(unix)]
#[test]
fn complete_probe_pass_reuses_valid_records_when_source_outputs_and_encoder_are_missing() {
    check_complete_probe_reuse("reuse");
}

#[cfg(unix)]
#[test]
fn complete_probe_reuse_still_rejects_invalid_history_config_or_temporal_selection() {
    check_complete_probe_reuse("invalid");
}

#[cfg(unix)]
#[test]
fn empty_probe_pass_is_a_noop_but_zero_pass_ordinal_is_invalid() {
    check_complete_probe_reuse("empty");
}

#[cfg(unix)]
#[test]
fn incomplete_pass_reencodes_current_quantizers_and_replaces_partial_records() {
    let (temp, mut input, encoder, config, directory) = fixture();
    let mut tasks = probe(
        1,
        &directory,
        &config,
        &mut input,
        &tasks(&encoder, &directory, 1),
    )
    .expect("verified prefix");
    let previous = tasks
        .iter()
        .map(|task| serde_json::to_value(&task.passes).expect("history"))
        .collect::<Vec<_>>();
    for task in &mut tasks {
        task.output = directory.join("2").join(format!("{:05}.mkv", task.original_index));
        task.encoder.set_quantizer(28.0);
    }
    let partial = probe(2, &directory, &config, &mut input, &tasks[..1])
        .expect("one scene has a current record");
    tasks[0].passes = partial[0].passes.clone();
    std::fs::write(&tasks[0].output, b"stale q28 marker").expect("abandoned output");
    let outside = temp.path().join("caller-directory");
    std::fs::create_dir_all(&outside).expect("outside directory");
    std::fs::write(outside.join("caller-data"), b"outside sentinel").expect("outside");
    std::os::unix::fs::symlink(&outside, &tasks[1].output).expect("stale output symlink");
    let unrelated = directory.join("2/unrelated.txt");
    std::fs::write(&unrelated, b"not a declared output").expect("caller neighbor");
    for (task, q) in tasks.iter_mut().zip([30.0, 34.0]) {
        task.encoder.set_quantizer(q);
    }
    let completed = probe(2, &directory, &config, &mut input, &tasks).expect("whole pass replay");
    eprintln!(
        "probe-resume mixed replay: {:?}",
        completed
            .iter()
            .map(|task| (
                task.original_index,
                task.passes.iter().map(|pass| pass.quantizer).collect::<Vec<_>>()
            ))
            .collect::<Vec<_>>()
    );
    for (index, task) in completed.iter().enumerate() {
        assert_eq!(
            task.passes.len(),
            2,
            "replace current record rather than appending a duplicate"
        );
        assert_eq!(
            serde_json::to_value(&task.passes[..1]).expect("prefix"),
            previous[index]
        );
        assert_eq!(task.passes[1].quantizer, [30.0, 34.0][index]);
        assert_eq!(task.passes[1].scores.len(), 2);
        assert!(task.passes[1].scores.iter().all(|score| score.is_finite()));
        assert!(!task.output.symlink_metadata().expect("fresh artifact").file_type().is_symlink());
        assert_ne!(
            std::fs::read(&task.output).expect("fresh output"),
            b"stale q28 marker"
        );
    }
    assert_eq!(
        std::fs::read(outside.join("caller-data")).expect("symlink directory target preserved"),
        b"outside sentinel"
    );
    assert_eq!(
        std::fs::read(unrelated).expect("caller data preserved"),
        b"not a declared output"
    );
    tasks[1].passes.clear();
    let before = std::fs::read(&tasks[0].output).expect("output");
    assert!(
        probe(2, &directory, &config, &mut input, &tasks).is_err(),
        "missing earlier prefix must not be invented"
    );
    assert_eq!(
        std::fs::read(&tasks[0].output).expect("not deleted"),
        before
    );
}
