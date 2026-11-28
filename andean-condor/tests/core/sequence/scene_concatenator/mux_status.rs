use super::*;
use crate::core::support::process::wait_with_timeout;
use andean_condor::utils::process::OwnedProcess;

#[path = "fixtures/mux_status.rs"]
mod fixture;

#[test]
fn mkvmerge_warning_exit_publishes_valid_artifacts_and_reports_diagnostics() {
    fixture::isolated(
        module_path!(),
        "mkvmerge_warning_exit_publishes_valid_artifacts_and_reports_diagnostics",
        &[
            ("final", "1", "valid", "normal"),
            ("group", "1", "valid", "normal"),
            ("grouped-final", "1", "valid", "normal"),
            ("both", "1", "valid", "normal"),
        ],
    );
}

#[test]
fn mkvmerge_success_exit_publishes_valid_group_and_final_artifacts() {
    fixture::isolated(
        module_path!(),
        "mkvmerge_success_exit_publishes_valid_group_and_final_artifacts",
        &[
            ("final", "0", "valid", "normal"),
            ("group", "0", "valid", "normal"),
            ("grouped-final", "0", "valid", "normal"),
        ],
    );
}

#[test]
fn mkvmerge_failed_or_signalled_mux_returns_original_status_and_diagnostics() {
    let mut cases = Vec::new();
    for phase in ["final", "group", "grouped-final"] {
        for status in ["2", "signal"] {
            for artifact in ["valid", "missing", "empty", "directory"] {
                cases.push((phase, status, artifact, "normal"));
            }
        }
    }
    fixture::isolated(
        module_path!(),
        "mkvmerge_failed_or_signalled_mux_returns_original_status_and_diagnostics",
        &cases,
    );
}

#[test]
fn mkvmerge_rejects_missing_empty_or_directory_output_even_after_success_status() {
    let mut cases = Vec::new();
    for phase in ["final", "group", "grouped-final"] {
        for status in ["0", "1"] {
            for artifact in ["missing", "empty", "directory"] {
                cases.push((phase, status, artifact, "normal"));
            }
        }
    }
    fixture::isolated(
        module_path!(),
        "mkvmerge_rejects_missing_empty_or_directory_output_even_after_success_status",
        &cases,
    );
}
