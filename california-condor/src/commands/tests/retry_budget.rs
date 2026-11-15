//! Losing max-tries parsing/forwarding/default preservation must fail here.
use super::*;
use clap::Parser;

#[test]
fn cli_max_tries_accepts_large_positive_budget_on_root_init_encode_and_set() {
    for args in [
        vec!["condor", "--max-tries", "300"],
        vec!["condor", "init", "--max-tries", "300"],
        vec!["condor", "encode", "--max-tries", "300"],
        vec!["condor", "encode", "set", "max-tries", "300"],
    ] {
        CondorCli::try_parse_from(&args).unwrap_or_else(|e| panic!("{args:?}: {e}"));
    }
}

#[test]
fn cli_max_tries_zero_is_rejected_on_root_init_encode_and_set() {
    for args in [
        vec!["condor", "--max-tries", "0"],
        vec!["condor", "init", "--max-tries", "0"],
        vec!["condor", "encode", "--max-tries", "0"],
        vec!["condor", "encode", "set", "max-tries", "0"],
    ] {
        assert!(CondorCli::try_parse_from(&args).is_err(), "{args:?}");
    }
}

#[test]
fn cli_max_tries_property_supports_info_and_reset() {
    for action in ["info", "reset"] {
        CondorCli::try_parse_from(["condor", "encode", action, "max-tries"]).expect("CRUD");
    }
}
