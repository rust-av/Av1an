#[cfg(unix)]
use super::*;

#[cfg(unix)]
#[test]
fn persist_failure_after_mux_preserves_existing_output_and_cleans_staging() {
    final_mux::isolated(
        module_path!(),
        "persist_failure_after_mux_preserves_existing_output_and_cleans_staging",
        "publication",
        |root| {
            publication::remove_stage_before_persist();
            let outcome = final_mux::execute(root, "publication");
            let error = outcome.result.as_ref().expect_err("persist failure reaches caller");
            assert!(
                error.is::<tempfile::PathPersistError>(),
                "actual persist failure: {error:#}"
            );
            outcome.assert_preserved(&outcome.previous);
            outcome.assert_common();
        },
    );
}
