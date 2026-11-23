use super::retry_progress::input;
use crate::core::support::provenance::current_encoding_fixture;
use andean_condor as condor;
use condor::core::{
    Condor,
    sequence::{
        Sequence, SequenceCompletion, SequenceStatus, Status,
        parallel_encoder::{ParallelEncoder, Task},
    },
};
use std::sync::Arc;

#[path = "fixtures/artifact_support.rs"]
mod fixture;

macro_rules! artifact_contract {
    ($name:ident, $module:ident::$run:ident, [$($case:literal),+ $(,)?]) => {
        #[test]
        fn $name() { fixture::$module::$run(concat!("core::sequence::parallel_encoder::artifact_validation::",stringify!($name)), &[$($case),+]); }
    };
}

artifact_contract!(
    new_parallel_outputs_require_successful_status_nonempty_file_and_decodable_frame_count,
    saved_artifacts::artifact_cases,
    [
        "missing",
        "zero",
        "corrupt",
        "truncated",
        "wrong",
        "status",
        "valid",
        "filter-strict",
        "av1",
        "vp9",
        "webm",
        "mkv",
        "hevc",
        "sparse",
        "duplicate"
    ]
);
artifact_contract!(
    saved_chunk_repair_reuses_the_worker_attempt_budget,
    saved_artifacts::artifact_cases,
    [
        "resume-valid",
        "resume-missing",
        "resume-zero",
        "resume-corrupt",
        "resume-wrong",
        "resume-exhaust",
        "resume-one",
        "resume-repair-save-error",
        "resume-save-error"
    ]
);
artifact_contract!(
    repair_leaves_unclaimed_unsafe_or_aliased_destinations_untouched,
    saved_artifacts::artifact_cases,
    [
        "resume-bare",
        "resume-symlink",
        "resume-directory",
        "resume-alias",
        "resume-moved",
        "resume-late"
    ]
);
artifact_contract!(
    frame_count_optout_preserves_status_and_nonempty_requirements,
    saved_artifacts::artifact_cases,
    ["optout-missing", "optout-zero", "optout-status", "filter-optout"]
);
artifact_contract!(
    artifact_probe_failures_and_force_do_not_publish_completion,
    saved_artifacts::artifact_cases,
    [
        "probe-force",
        "probe-json",
        "probe-count",
        "probe-status",
        "probe-error",
        "probe-flood"
    ]
);
artifact_contract!(
    parallel_encode_refuses_to_replace_existing_output,
    saved_artifacts::artifact_cases,
    ["api-existing"]
);

artifact_contract!(
    frame_count_policy_applies_to_target_quality_independent_scene_probes,
    resume_policy::policy_cases,
    ["tq-individual-optout", "tq-zone-optout", "tq-individual-zero", "tq-zone-status"]
);
artifact_contract!(
    missing_probe_or_encoder_is_rejected_at_preflight_before_work,
    resume_policy::policy_cases,
    [
        "preflight-worker",
        "preflight-tq",
        "preflight-tq-api",
        "preflight-parallel",
        "preflight-zone",
        "preflight-encoder"
    ]
);
artifact_contract!(
    failed_resume_validation_preserves_history_and_can_be_retried,
    resume_policy::policy_cases,
    ["resume-probe", "resume-probe-noisy"]
);
artifact_contract!(
    resume_reencodes_a_legacy_chunk_with_a_wrong_decoded_frame_count,
    resume_policy::policy_cases,
    ["resume-policy"]
);
artifact_contract!(
    graceful_resume_validation_does_not_admit_future_candidates,
    resume_policy::policy_cases,
    ["resume-graceful"]
);
artifact_contract!(
    resume_skips_decode_only_after_strict_verification_is_saved,
    resume_policy::policy_cases,
    ["resume-cache", "resume-cache-legacy", "resume-cache-optout"]
);
artifact_contract!(
    resume_persists_good_verification_when_a_later_probe_fails,
    resume_policy::policy_cases,
    ["resume-checkpoint-failure", "resume-reappeared"]
);
