use andean_condor as condor;
use condor::core::{
    Condor,
    input::Input,
    sequence::{Sequence, serial_encoder::SerialEncoder},
};
use std::{
    path::{Path, PathBuf},
    sync, thread,
    time::Instant,
};
#[path = "fixtures/artifact_support.rs"]
mod fixture;

macro_rules! serial_contract {
    ($name:ident, [$($case:literal),+ $(,)?]) => {
        #[test]
        fn $name() { fixture::serial_cases(concat!("core::sequence::serial_encoder::",stringify!($name)), &[$($case),+]); }
    };
}

serial_contract!(
    serial_encoding_rejects_missing_empty_corrupt_or_wrong_count_outputs,
    [
        "missing",
        "zero",
        "corrupt",
        "truncated",
        "wrong",
        "status",
        "valid",
        "real",
        "second",
        "decode",
        "optout-wrong",
        "optout-no-probe",
        "optout-zero",
        "optout-status",
        "optout-missing"
    ]
);
serial_contract!(
    serial_encoding_preserves_existing_and_racing_outputs,
    ["existing", "race"]
);
serial_contract!(
    serial_preflight_refuses_invalid_settings_before_input_or_codec_work,
    [
        "zero-range",
        "reversed-range",
        "preflight-probe",
        "preflight-executable",
        "preflight-settings",
        "specific-first",
        "specific-first-optout"
    ]
);
serial_contract!(
    serial_cancellation_finishes_or_interrupts_owned_work_without_early_publication,
    ["forced-before", "graceful", "probe-force", "private"]
);
