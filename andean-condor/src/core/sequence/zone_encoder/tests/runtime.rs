//! Private forced ZonePlan failures; public probe/failure workflows live in tests/core.
use super::*;
use crate as condor;
use crate::core::sequence::tests::support::current_encoding_fixture;

#[path = "../../../../../tests/core/sequence/zone_encoder/fixtures/runtime_support.rs"]
#[allow(
    dead_code,
    reason = "shared test fixture also contains public-only native probe drivers"
)]
mod fixture;

mod internal_integration {
    use super::*;

    #[test]
    fn private_zone_spawn_or_worker_failure_joins_workers_and_cleans_zone_scratch() {
        fixture::sequence_failures(
            "core::sequence::zone_encoder::tests::runtime::internal_integration::private_zone_spawn_or_worker_failure_joins_workers_and_cleans_zone_scratch",
            &["zone", "zone-spawn", "zone-worker"],
            |input, tasks, tx, cancelled| {
                let plan = ZonePlan {
                    zones: vec![],
                    zone_option: "0,2,crf=23".to_owned(),
                    qpfile_content: "1 I\n2 I\n".to_owned(),
                    config_content: String::new(),
                };
                ZoneEncoder::encode_tasks(input, tasks, plan, tx, cancelled, true)
                    .expect_err("zone failure")
            },
        );
    }
}
