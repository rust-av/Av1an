use super::*;

fn assert_published(outcome: &final_mux::MuxOutcome) {
    outcome.result.as_ref().expect("successful mux and publication");
    assert!(
        outcome.completed,
        "successful publication reports completion"
    );
    assert_ne!(
        std::fs::read(outcome.output()).expect("replacement"),
        outcome.previous,
        "successful mux replaces the distinct one-frame output"
    );
    let probe = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-count_frames",
            "-show_entries",
            "stream=nb_read_frames",
            "-of",
            "csv=p=0",
        ])
        .arg(outcome.output())
        .output()
        .expect("required FFprobe CLI");
    assert!(probe.status.success());
    assert_eq!(String::from_utf8_lossy(&probe.stdout).trim(), "2");
}

#[test]
fn successful_mux_replaces_existing_output_at_configured_destination() {
    final_mux::isolated(
        module_path!(),
        "successful_mux_replaces_existing_output_at_configured_destination",
        "native",
        |root| {
            let outcome = final_mux::execute(root, "native");
            assert_published(&outcome);
            outcome.assert_common();
        },
    );
}

#[test]
fn successful_mux_publishes_new_output_without_overwrite_permission() {
    final_mux::isolated(
        module_path!(),
        "successful_mux_publishes_new_output_without_overwrite_permission",
        "native-new",
        |root| {
            let outcome = final_mux::execute(root, "native-new");
            assert_published(&outcome);
            outcome.assert_common();
        },
    );
}

#[test]
fn failed_mux_preserves_existing_output_and_exit_status() {
    final_mux::isolated(
        module_path!(),
        "failed_mux_preserves_existing_output_and_exit_status",
        "failure",
        |root| {
            let outcome = final_mux::execute(root, "failure");
            let error = outcome.result.as_ref().expect_err("mux fails");
            let Some(SceneConcatenatorError::FfmpegFailed { status, .. }) =
                error.downcast_ref::<SceneConcatenatorError>()
            else {
                panic!("typed FFmpeg failure: {error:#}");
            };
            assert_eq!(status.code(), Some(42));
            outcome.assert_preserved(&outcome.previous);
            outcome.assert_common();
        },
    );
}

#[test]
fn late_destination_race_preserves_competing_output() {
    final_mux::isolated(
        module_path!(),
        "late_destination_race_preserves_competing_output",
        "race",
        |root| {
            let outcome = final_mux::execute(root, "race");
            outcome.assert_preserved(b"racer owns final");
            outcome.assert_common();
        },
    );
}

#[test]
fn late_source_alias_refuses_publication_without_changing_source() {
    final_mux::isolated(
        module_path!(),
        "late_source_alias_refuses_publication_without_changing_source",
        "alias",
        |root| {
            let outcome = final_mux::execute(root, "alias");
            outcome.assert_preserved(b"protected original bytes");
            outcome.assert_common();
        },
    );
}

#[test]
fn forced_mux_cancellation_does_not_finish_or_publish_output() {
    final_mux::isolated(
        module_path!(),
        "forced_mux_cancellation_does_not_finish_or_publish_output",
        "force",
        |root| {
            let outcome = final_mux::execute(root, "force");
            assert_eq!(
                outcome.result.as_ref().expect_err("forced mux").downcast_ref::<Cancelled>(),
                Some(&Cancelled::Forced)
            );
            assert!(
                !root.join("mux-finished").exists(),
                "force stops the active mux"
            );
            outcome.assert_preserved(&outcome.previous);
            outcome.assert_common();
        },
    );
}

#[test]
fn graceful_mux_cancellation_finishes_active_work_without_publication() {
    final_mux::isolated(
        module_path!(),
        "graceful_mux_cancellation_finishes_active_work_without_publication",
        "graceful",
        |root| {
            let outcome = final_mux::execute(root, "graceful");
            assert_eq!(
                outcome.result.as_ref().expect_err("graceful mux").downcast_ref::<Cancelled>(),
                Some(&Cancelled::Graceful)
            );
            assert!(
                root.join("mux-finished").exists(),
                "graceful finishes the admitted mux"
            );
            outcome.assert_preserved(&outcome.previous);
            outcome.assert_common();
        },
    );
}
