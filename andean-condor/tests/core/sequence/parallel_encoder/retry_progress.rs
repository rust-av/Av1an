//! Public retry, progress and rate outcomes; boundary-only hooks remain in lib tests.
use crate::core::support::provenance::current_encoding_fixture;
use andean_condor as condor;
use condor::core::{
    Condor,
    input::Input,
    sequence::{
        Sequence, SequenceCompletion, SequenceStatus, Status,
        parallel_encoder::{ParallelEncoder, ParallelEncoderError, ParallelEncoderOptions, Task},
    },
};
use std::sync::Arc;

#[path = "fixtures/retry_progress_support.rs"]
mod fixture;
#[path = "fixtures/probe_rate_support.rs"]
mod rate_fixture;

fn input(root: &std::path::Path, native: bool) -> Input {
    if native {
        super::stream_errors::native_input(root)
    } else {
        use condor::models::input::{Input as InputModel, VapourSynthScriptSource};
        let script = root.join("finite.vpy");
        std::fs::write(&script,"from vapoursynth import core, YUV420P8\ncore.std.BlankClip(width=64,height=64,length=96,format=YUV420P8,fpsnum=24,fpsden=1).set_output()\n").expect("VS script");
        Input::from_data(&InputModel::VapourSynthScript {
            source: VapourSynthScriptSource::Path(script),
            variables: std::collections::HashMap::new(),
            index: 0,
            filters: vec![],
            stream_concurrently: false,
        })
        .expect("real nonstream input")
    }
}

macro_rules! contract {
    ($name:ident, rate_cases, [$($kind:literal),+ $(,)?]) => {
        #[test]
        fn $name() { rate_fixture::rate_cases(concat!("core::sequence::parallel_encoder::retry_progress::",stringify!($name)), &[$($kind),+]); }
    };
    ($name:ident, $run:ident, [$($kind:literal),+ $(,)?]) => {
        #[test]
        fn $name() { fixture::$run(concat!("core::sequence::parallel_encoder::retry_progress::",stringify!($name)), &[$($kind),+]); }
    };
}

contract!(
    encoded_probe_rate_uses_integer_or_fractional_source_frame_duration,
    rate_cases,
    ["integer", "fractional"]
);
contract!(
    invalid_rate_or_empty_selection_refuses_before_native_work,
    rate_cases,
    ["invalid"]
);
contract!(
    empty_or_cancelled_encode_admission_does_not_launch_a_codec,
    rate_cases,
    ["no-work"]
);
contract!(
    zone_encode_results_report_bits_per_second,
    rate_cases,
    ["zone"]
);

contract!(
    failed_encoding_attempts_replay_identical_input_and_cleanup_before_retry,
    retry_cases,
    ["once", "filter", "filter-only", "all2", "missing", "zero", "wrong", "direct"]
);
contract!(
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
contract!(
    graceful_or_existing_cancellation_prevents_new_retry_admission,
    retry_cases,
    ["all2-graceful", "cancelled"]
);
contract!(
    successful_retries_allow_healthy_and_queued_peers_to_finish,
    retry_cases,
    ["peers"]
);
contract!(
    source_preflight_save_and_publication_failures_are_not_retried,
    retry_cases,
    ["budget0", "preflight", "source", "save", "publication"]
);

contract!(
    sparse_or_quiet_encoder_progress_is_reconciled_at_artifact_commit,
    progress_cases,
    ["quiet", "sparse"]
);
contract!(
    failed_attempt_progress_is_rolled_back_without_removing_peer_credit,
    progress_cases,
    ["once", "all2", "exhaust1", "exhaust-wrong"]
);
contract!(
    retry_commit_and_resume_emit_each_scene_completion_once,
    progress_cases,
    ["resume", "resumed"]
);
contract!(
    failed_publication_never_credits_uncommitted_frames,
    progress_cases,
    ["publication"]
);
