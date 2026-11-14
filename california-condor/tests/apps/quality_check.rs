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
