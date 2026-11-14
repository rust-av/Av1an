use crate::core::support::provenance::current_quality_fixture;
use andean_condor as condor;
#[path = "../fixtures/native_scoring.rs"]
mod native_fixture;
use native_fixture::{calls, clear, metric, native_children, y4m_fixture};
#[path = "fixtures/qc_support.rs"]
mod fixture;

macro_rules! output_contract {
    ($name:ident, $kind:literal) => {
        #[test]
        fn $name() {
            fixture::qc_output_case(
                concat!("core::sequence::quality_check::", stringify!($name)),
                $kind,
            );
        }
    };
}

output_contract!(qc_override_scores_distinct_native_output, "distinct");
output_contract!(qc_override_ignores_missing_root, "missing");
output_contract!(qc_override_ignores_invalid_root, "invalid");
output_contract!(
    qc_missing_effective_output_warns_without_scoring_root,
    "missing-override"
);
output_contract!(
    qc_without_output_override_scores_the_configured_root_output,
    "fallback"
);
output_contract!(
    qc_invalid_effective_output_reports_actual_error_without_save,
    "invalid-override"
);

#[test]
fn qc_execute_corrupt_last_scene_does_not_attach_save_or_complete() {
    fixture::corrupt_batch(
        "core::sequence::quality_check::qc_execute_corrupt_last_scene_does_not_attach_save_or_complete",
        None,
    );
}

#[test]
fn qc_rejects_tampered_certified_history_before_native_work() {
    fixture::check_qc_history(
        "core::sequence::quality_check::qc_rejects_tampered_certified_history_before_native_work",
        false,
        None,
    );
}

#[test]
fn qc_explicit_reset_remeasures_atomically_and_saves_only_a_valid_batch() {
    fixture::check_qc_history(
        "core::sequence::quality_check::qc_explicit_reset_remeasures_atomically_and_saves_only_a_valid_batch",
        true,
        None,
    );
}
