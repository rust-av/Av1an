use super::*;
use crate as condor;
use crate::{
    core::sequence::tests::support::current_encoding_fixture, utils::cancellation::Cancellation,
};
use std::{fs, path::Path};

#[path = "../../../../../tests/core/sequence/parallel_encoder/fixtures/artifact_support.rs"]
#[allow(
    dead_code,
    reason = "shared fixture also contains public-only saved artifact drivers"
)]
mod fixture;

fn input(root: &Path, native: bool) -> Input {
    shutdown::internal_integration::input(root, native)
}

mod saved_artifacts {
    #[test]
    fn missing_frame_count_policy_defaults_to_strict_and_explicit_optout_roundtrips() {
        use crate::models::sequence::parallel_encoder::ParallelEncoderConfig;
        let old = r#"{"workers":1,"scenes_directory":"scenes","input":null}"#;
        let mut config: ParallelEncoderConfig = serde_json::from_str(old).expect("old config");
        assert!(config.validate_frame_count);
        config.validate_frame_count = false;
        let json = serde_json::to_value(config).expect("serialize policy");
        assert_eq!(json["validate_frame_count"], false);
        let config: ParallelEncoderConfig = serde_json::from_value(json).expect("persisted optout");
        assert!(!config.validate_frame_count);
        let schema = schemars::schema_for!(ParallelEncoderConfig);
        let json = serde_json::to_value(schema).expect("generated schema");
        assert_eq!(json["properties"]["validate_frame_count"]["default"], true);
        assert_eq!(
            json["properties"]["validate_frame_count"]["type"],
            "boolean"
        );
        assert!(
            !json["required"]
                .as_array()
                .expect("required")
                .iter()
                .any(|v| v == "validate_frame_count")
        );
    }
}

pub(in crate::core::sequence) mod resume_policy {
    pub(in crate::core::sequence) mod internal_integration {
        use super::super::*;

        pub(in crate::core::sequence) fn fixed_batch() -> Option<(usize, usize)> {
            std::env::var("ENCODER_FIXTURE_CASE")
                .is_ok_and(|case| case.starts_with("resume-policy-worker-"))
                .then_some((1, 3))
        }
        pub(in crate::core::sequence) fn finish_fixed_batch(token: &Cancellation) {
            if fixed_batch().is_some() {
                fs::write(
                    Path::new(&std::env::var_os("ENCODER_FIXTURE_ROOT").expect("root"))
                        .join("fixed-batch"),
                    "one batch, no threshold or throughput comparison",
                )
                .expect("batch handshake");
                token.request().expect("stop before another trial");
            }
        }
        pub(in crate::core::sequence::parallel_encoder) fn after_rename(output: &Path) {
            if std::env::var("ENCODER_FIXTURE_CASE")
                .is_ok_and(|case| case.starts_with("resume-policy-observer-"))
            {
                let root = std::path::PathBuf::from(
                    std::env::var_os("ENCODER_FIXTURE_ROOT").expect("root"),
                );
                assert!(output.is_file(), "real rename precedes receiver failure");
                fs::write(root.join("renamed"), "published").expect("rename handshake");
                while !root.join("observer-dropped").exists() {
                    std::thread::yield_now();
                }
            }
        }

        #[test]
        fn fixed_benchmarker_batch_obeys_frame_count_policy() {
            fixture::resume_policy::policy_cases(
                "core::sequence::parallel_encoder::tests::artifact_validation::resume_policy::internal_integration::fixed_benchmarker_batch_obeys_frame_count_policy",
                &[
                    "worker-optout",
                    "worker-no-probe-optout",
                    "worker-strict",
                    "worker-zero",
                    "worker-status",
                ],
            );
        }
        #[test]
        fn observer_disconnect_after_rename_does_not_erase_published_metadata() {
            fixture::resume_policy::policy_cases(
                "core::sequence::parallel_encoder::tests::artifact_validation::resume_policy::internal_integration::observer_disconnect_after_rename_does_not_erase_published_metadata",
                &["observer-native", "observer-vs"],
            );
        }
    }
}
