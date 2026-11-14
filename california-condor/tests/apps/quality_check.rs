use california::apps::TuiApp;
use california_condor as california;

#[path = "fixtures/quality_scenes.rs"]
mod quality_scenes;

#[test]
fn qc_headless_resume_reports_saved_and_final_live_scores_without_reemitting_saved_frame_events() {
    const TEST: &str = "quality_check::qc_headless_resume_reports_saved_and_final_live_scores_without_reemitting_saved_frame_events";
    if std::env::var_os("QC_RESUMED_REPORT_CHILD").is_some() {
        assert_eq!(std::env::var("QC_RESUMED_REPORT_CHILD").unwrap(), TEST);
        quality_scenes::run_resumed_quality_report();
        println!("QC_RESUMED_PUBLIC_ASSERTIONS_COMPLETED");
        return;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", TEST, "--nocapture"])
        .env("QC_RESUMED_REPORT_CHILD", TEST)
        .env("CONDOR_TEST_MODE", "1")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("1 passed; 0 failed;")
            && stdout.contains("QC_RESUMED_PUBLIC_ASSERTIONS_COMPLETED")
    );
    let row: Vec<_> = stdout
        .lines()
        .find(|line| line.starts_with("Per-Frame"))
        .expect("after_loop final report")
        .split_whitespace()
        .collect();
    assert_eq!(
        row,
        ["Per-Frame", "99.5000", "0.0000", "199.0000", "2.0000", "57.7343"]
    );
    let events: Vec<_> = stdout
        .lines()
        .filter_map(|line| line.strip_prefix("[Quality Check][Frame] "))
        .map(|json| serde_json::from_str::<serde_json::Value>(json).unwrap())
        .collect();
    assert_eq!(events.len(), 100);
    assert_eq!(
        events[0],
        serde_json::json!({"FrameScore":{"frame":100,"score":100.0}})
    );
    assert_eq!(
        events[99],
        serde_json::json!({"FrameScore":{"frame":199,"score":199.0}})
    );
    assert!(
        stdout.contains("[Quality Check][Scene] {\"SceneScore\":{\"scene\":1,\"score\":149.5}}")
    );
    eprintln!("{stdout}");
}

fn check_prefix_report(independent: bool, test: &str) {
    if let Ok(selected) = std::env::var("QC_PREFIX_REPORT_CHILD") {
        assert_eq!(selected, test);
        let mut app = quality_scenes::prefix_report_app(independent);
        // after_loop is the public report boundary; mark the saved history complete.
        app.cached_state_mut().frames_compared = app.cached_state().total_frames;
        app.after_loop();
        println!("QC_PREFIX_PUBLIC_ASSERTIONS_COMPLETED");
        return;
    }
    let output = std::process::Command::new(std::env::current_exe().expect("test executable"))
        .args(["--exact", test, "--nocapture"])
        .env("QC_PREFIX_REPORT_CHILD", test)
        .output()
        .expect("report child");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(
        stdout.contains("1 passed; 0 failed;")
            && stdout.contains("QC_PREFIX_PUBLIC_ASSERTIONS_COMPLETED")
    );
    assert!(
        stdout.contains("Per-Scene") && stdout.contains("5.0000"),
        "{stdout}"
    );
    assert_eq!(stdout.contains("Per-Frame"), independent, "{stdout}");
    eprintln!("{stdout}");
}

#[test]
fn qc_temporal_report_summarizes_scene_finals_without_per_frame_statistics() {
    check_prefix_report(
        false,
        "quality_check::qc_temporal_report_summarizes_scene_finals_without_per_frame_statistics",
    );
}

#[test]
fn qc_independent_frame_report_includes_per_frame_statistics() {
    check_prefix_report(
        true,
        "quality_check::qc_independent_frame_report_includes_per_frame_statistics",
    );
}

#[test]
fn qc_console_emits_submission_progress_before_source_indexed_scores() {
    const TEST: &str =
        "quality_check::qc_console_emits_submission_progress_before_source_indexed_scores";
    if let Ok(selected) = std::env::var("QC_SUBMISSION_PROGRESS_CHILD") {
        assert_eq!(selected, TEST);
        quality_scenes::map_submission_progress(true, |_, _, _| {});
        println!("QC_SUBMISSION_PUBLIC_ASSERTIONS_COMPLETED");
        return;
    }
    let output = quality_scenes::submission_output(TEST);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    eprintln!("QC submission progress mapper/console:\n{stdout}\n{stderr}");
    assert!(output.status.success(), "{stdout}\n{stderr}");
    assert!(
        stdout.contains("running 1 test") && stdout.contains("1 passed; 0 failed"),
        "exact child filter must execute one test"
    );
    assert!(stdout.contains("QC_SUBMISSION_PUBLIC_ASSERTIONS_COMPLETED"));
    let events: Vec<serde_json::Value> = stdout
        .lines()
        .filter_map(|line| {
            line.strip_prefix("[Quality Check][Compare] ")
                .or_else(|| line.strip_prefix("[Quality Check][Frame] "))
                .map(|json| serde_json::from_str(json).expect("actual console JSON"))
        })
        .collect();
    assert_eq!(
        events,
        [
            serde_json::json!({"CompareProgress":{"current_frame":0,"total_frames":1}}),
            serde_json::json!({"CompareProgress":{"current_frame":1,"total_frames":3}}),
            serde_json::json!({"CompareProgress":{"current_frame":2,"total_frames":3}}),
            serde_json::json!({"CompareProgress":{"current_frame":3,"total_frames":3}}),
            serde_json::json!({"FrameScore":{"frame":4,"score":90.0}}),
            serde_json::json!({"FrameScore":{"frame":7,"score":91.0}}),
            serde_json::json!({"FrameScore":{"frame":9,"score":92.0}}),
        ],
        "accepted progress precedes real source-indexed final scores without extra progress events"
    );
}
