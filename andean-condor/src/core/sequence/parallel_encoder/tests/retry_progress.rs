//! Retry-boundary cleanup and fault injection remain private integrations.
use super::*;
use crate as condor;
use crate::core::sequence::tests::support::current_encoding_fixture;
use crate::utils::cancellation::Cancellation;
use std::fs;

#[path = "../../../../../tests/core/sequence/parallel_encoder/fixtures/retry_progress_support.rs"]
mod fixture;

fn input(root: &std::path::Path, native: bool) -> Input {
    shutdown::internal_integration::input(root, native)
}

#[test]
fn max_tries_defaults_to_three_and_schema_requires_at_least_one_attempt() {
    use crate::models::sequence::parallel_encoder::ParallelEncoderConfig;
    let config: ParallelEncoderConfig =
        serde_json::from_str(r#"{"workers":1,"scenes_directory":"scenes","input":null}"#)
            .expect("old JSON");
    let json = serde_json::to_value(config).expect("saved");
    assert_eq!(json["max_tries"], 3);
    let schema =
        serde_json::to_value(schemars::schema_for!(ParallelEncoderConfig)).expect("schema");
    assert_eq!(schema["properties"]["max_tries"]["default"], 3);
    assert_eq!(schema["properties"]["max_tries"]["minimum"], 1);
}

pub(in crate::core::sequence::parallel_encoder) mod internal_integration {
    use super::*;

    pub(in crate::core::sequence::parallel_encoder) fn after_failure(token: &Cancellation) {
        let Ok(case) = std::env::var("ENCODER_FIXTURE_CASE") else {
            return;
        };
        if !case.starts_with("retry-") {
            return;
        }
        let root =
            std::path::PathBuf::from(std::env::var_os("ENCODER_FIXTURE_ROOT").expect("root"));
        for output in fs::read_to_string(root.join("outputs")).unwrap_or_default().lines() {
            if std::path::Path::new(output).starts_with(root.join("scenes")) {
                assert!(
                    !std::path::Path::new(output).exists(),
                    "failed attempt removed after joins"
                );
            }
        }
        for stats in fs::read_to_string(root.join("stat-paths"))
            .unwrap_or_default()
            .lines()
            .filter(|s| !s.is_empty())
        {
            assert!(
                !std::path::Path::new(stats).exists(),
                "failed statistics removed after reap"
            );
        }
        if case.ends_with("graceful") && !case.ends_with("all2-graceful") {
            token.request().expect("graceful at retry admission");
        }
        if case.ends_with("force-boundary") {
            token.request().expect("first request after joined failure");
            token.request().expect("force before replacement");
        }
    }

    macro_rules! retry_contract {
        ($name:ident, $run:ident, [$($kind:literal),+ $(,)?]) => {
            #[test]
            fn $name() { fixture::$run(concat!("core::sequence::parallel_encoder::tests::retry_progress::internal_integration::",stringify!($name)), &[$($kind),+]); }
        };
    }

    retry_contract!(
        failed_encoding_attempts_replay_identical_input_and_cleanup_before_retry,
        retry_cases,
        [
            "once",
            "filter",
            "filter-only",
            "all2",
            "missing",
            "zero",
            "wrong",
            "direct",
            "peers"
        ]
    );
    retry_contract!(
        retry_exhaustion_reports_the_last_original_codec_diagnostic,
        retry_cases,
        [
            "exhaust1",
            "exhaust2",
            "exhaust3",
            "exhaust-missing",
            "exhaust-zero",
            "exhaust-wrong",
            "filter-failure",
            "direct-exhaust"
        ]
    );
    retry_contract!(
        graceful_or_forced_cancellation_prevents_new_retry_admission,
        retry_cases,
        ["graceful", "force-boundary"]
    );
    retry_contract!(
        successful_active_peers_are_saved_when_another_scene_exhausts_retries,
        retry_cases,
        ["peer-exhaust"]
    );
    retry_contract!(
        private_retry_boundary_and_worker_faults_do_not_leak_owned_resources,
        retry_cases,
        ["capture-force", "worker-fault", "force"]
    );
    retry_contract!(
        failed_attempt_progress_is_rolled_back_without_removing_peer_credit,
        progress_cases,
        ["once", "all2", "exhaust1", "exhaust-wrong", "peer-exhaust"]
    );
    retry_contract!(
        forced_or_failed_publication_never_credits_uncommitted_frames,
        progress_cases,
        ["force", "force-child-first"]
    );
}
