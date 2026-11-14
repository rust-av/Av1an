//! Cross-QC/TQ private scoring matrix; public execute workflows have external owners.
#![cfg(unix)]
use super::support::native_scoring::{
    native_children, native_consumers, native_failure, sequential_decoders,
};
use crate::metrics::Engine;

mod internal_integration {
    use super::*;
    fn scene_cases(test: &str, phase: &str) {
        native_children(
            test,
            &[("scene-f-qc", ""), ("scene-v-qc", ""), ("scene-f-tq", ""), ("scene-v-tq", "")],
            || {
                let engine =
                    if std::env::var("TEMPORAL_FIXTURE_SCOPE").expect("scope").contains("-f-") {
                        Engine::Fmetrics
                    } else {
                        Engine::Vship
                    };
                if phase == "sequential" {
                    sequential_decoders(engine);
                } else {
                    native_consumers(engine, phase);
                }
            },
        );
    }

    macro_rules! scene_contract {
        ($name:ident, $phase:literal) => {
            #[test]
            fn $name() {
                scene_cases(
                    concat!(
                        "core::sequence::tests::native_scoring::internal_integration::",
                        stringify!($name)
                    ),
                    $phase,
                );
            }
        };
    }

    scene_contract!(
        native_scene_scoring_resets_temporal_sessions_between_scenes,
        "sessions"
    );
    scene_contract!(
        sequential_decoders_match_aligned_and_compacted_scene_scoring,
        "sequential"
    );
    scene_contract!(
        native_scene_callback_cancellation_prevents_later_contexts,
        "cancellation"
    );
    scene_contract!(
        invalid_or_empty_scene_selection_does_not_call_native_abi,
        "admission"
    );
    scene_contract!(
        independent_frame_scoring_reuses_one_session_and_reduces_each_scene,
        "independent"
    );
    scene_contract!(
        within_scene_gaps_follow_engine_temporal_capability_rules,
        "gaps"
    );
    scene_contract!(
        temporal_scene_reduction_uses_final_prefix_not_mean,
        "sessions"
    );
    scene_contract!(
        independent_frame_scene_reduction_uses_configured_statistic,
        "independent"
    );

    #[test]
    fn native_scoring_retains_failures_before_and_after_first_progress() {
        native_children(
            "core::sequence::tests::native_scoring::internal_integration::native_scoring_retains_failures_before_and_after_first_progress",
            &[
                ("scene-f-qc", "fp"),
                ("scene-v-qc", "vp"),
                ("scene-f-qc", "fp2"),
                ("scene-v-qc", "vp2"),
            ],
            || {
                let engine =
                    if std::env::var("TEMPORAL_FIXTURE_SCOPE").expect("scope").contains("-f-") {
                        Engine::Fmetrics
                    } else {
                        Engine::Vship
                    };
                let failure = std::env::var("TEMPORAL_FIXTURE_FAIL").expect("failure");
                native_failure(engine, &failure);
            },
        );
    }
}
