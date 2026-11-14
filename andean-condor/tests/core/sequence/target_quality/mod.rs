use andean_condor as condor;
#[cfg(unix)]
use condor::core::input::Input;
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
