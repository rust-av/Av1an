use crate::core::support::provenance::current_encoding_fixture;
use andean_condor as condor;
use condor::{
    core::{
        input::Input,
        sequence::{
            SequenceCompletion, SequenceStatus, Status,
            parallel_encoder::{ParallelEncoderError, Task},
            zone_encoder::{DifferingField, ZoneIneligible, ZonePlan},
        },
    },
    models::encoder::{Encoder, EncoderBase, EncoderPasses, cli_parameter::CLIParameter},
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
};

#[path = "fixtures/runtime_support.rs"]
mod fixture;

#[test]
fn x264_probe_selections_remain_valid_in_final_per_scene_encodes() {
    fixture::native_probe_parity(
        "core::sequence::zone_encoder::x264_probe_selections_remain_valid_in_final_per_scene_encodes",
        &["x264"],
    );
}

#[test]
fn zone_capable_svt_probe_selections_remain_valid_in_final_per_scene_encodes() {
    fixture::native_probe_parity(
        "core::sequence::zone_encoder::zone_capable_svt_probe_selections_remain_valid_in_final_per_scene_encodes",
        &["svt"],
    );
}

#[test]
fn x265_probes_match_independent_final_quality_regardless_of_zone_eligibility() {
    fixture::native_probe_parity(
        "core::sequence::zone_encoder::x265_probes_match_independent_final_quality_regardless_of_zone_eligibility",
        &["x265"],
    );
}

macro_rules! failure_contract {
    ($name:ident, [$($case:literal),+ $(,)?]) => {
        #[test]
        fn $name() {
            fixture::sequence_failures(concat!("core::sequence::zone_encoder::",stringify!($name)), &[$($case),+], |_,_,_,_| unreachable!("private zone selectors cannot enter a public contract"));
        }
    };
}

failure_contract!(
    parallel_scene_failure_retains_original_status_and_preserves_resumed_peer,
    ["parallel-vs1", "parallel-vs2", "parallel-native1", "parallel-native2"]
);
failure_contract!(
    serial_scene_failure_retains_original_status_and_owned_partial_policy,
    ["serial"]
);
