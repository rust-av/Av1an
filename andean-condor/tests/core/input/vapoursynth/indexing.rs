//! CPU index validation, not licensed DGDecNV decode acceptance.
use andean_condor::utils::cancellation::{Cancellation, Cancelled};
use andean_condor::vapoursynth::plugins::dgdecodenv::DGSource;
use std::fs;

const SAMPLE: &[u8] = include_bytes!("../../../../src/vapoursynth/plugins/dgindexnv_sample.dgi");

#[path = "fixtures/indexing_support.rs"]
mod fixture;

macro_rules! index_cases {
    ($name:ident, [$($case:literal),+ $(,)?]) => {
        #[test]
        fn $name() {
            fixture::run_index_cases(concat!("core::input::vapoursynth::indexing::",stringify!($name)), &[$($case),+], fixture::assert_index_case);
        }
    };
}

index_cases!(
    dg_index_candidates_are_validated_before_publication,
    ["missing", "zero", "garbage", "truncated", "directory", "symlink"]
);
index_cases!(
    dg_index_existing_artifacts_are_validated_without_spawning,
    ["existing"]
);
index_cases!(dg_index_spawn_failure_leaves_no_partial_cache, ["spawn"]);
index_cases!(
    dg_index_force_reaps_owned_processes_before_cleanup,
    ["force-retry"]
);
index_cases!(
    dg_index_graceful_cancellation_finishes_admitted_work,
    ["graceful"]
);
index_cases!(
    concurrent_dg_indexers_publish_one_complete_cache,
    ["concurrent"]
);
index_cases!(
    dg_index_observer_panics_return_after_reader_and_process_join,
    ["observer-panic"]
);

#[test]
fn dg_index_reuse_accepts_either_valid_numeric_footer_order() {
    let directory = tempfile::tempdir().expect("footer fixture");
    let source = directory.path().join("source.mkv");
    let cache = directory.path().join("index.dgi");
    let executable = directory.path().join("must-not-run");
    fs::write(&source, b"original media").expect("source");
    for (footer, valid) in [
        ("CODED 50\nORDER 0\n", true),
        ("ORDER 0\nCODED 50\n", true),
        ("CODED 0\nORDER 0\n", false),
        ("ORDER 0\nCODED 0\n", false),
        ("CODED no\nORDER 0\n", false),
        ("ORDER 0\nCODED no\n", false),
        ("CODED 50\nORDER no\n", false),
        ("ORDER no\nCODED 50\n", false),
        ("CODED 50\n", false),
        ("ORDER 0\n", false),
    ] {
        let bytes = format!("DGIndexNV\n{footer}");
        fs::write(&cache, &bytes).expect("structural fixture");
        let result = DGSource::index_video(
            &source,
            Some(&cache),
            Some(&executable),
            None,
            &andean_condor::utils::cancellation::Cancellation::default(),
        );
        assert_eq!(result.is_ok(), valid, "footer {footer:?}: {result:?}");
        if valid {
            assert_eq!(result.expect("reuse without spawning"), cache);
        }
        assert_eq!(fs::read(&cache).expect("preserved index"), bytes.as_bytes());
    }
    assert_eq!(
        fs::read(source).expect("preserved media"),
        b"original media"
    );
}

#[test]
fn media_cache_alias_guidance_never_suggests_removing_original() {
    let directory = tempfile::tempdir().expect("alias fixture");
    let source = directory.path().join("source.mkv");
    fs::write(&source, b"original media").expect("source");
    let error = DGSource::index_video(
        &source,
        Some(&source),
        None,
        None,
        &andean_condor::utils::cancellation::Cancellation::default(),
    )
    .expect_err("media is not an index");
    assert_eq!(
        fs::read(source).expect("preserved source"),
        b"original media"
    );
    let guidance = format!("{error:#}");
    assert!(guidance.contains("preserved unchanged"), "{guidance}");
    assert!(
        !guidance.contains("remove"),
        "must not advise removing original media: {guidance}"
    );
    assert!(guidance.contains("input/index path"), "{guidance}");
}

#[test]
fn direct_project_guidance_selects_valid_input_not_ignored_cache_override() {
    let directory = tempfile::tempdir().expect("direct project fixture");
    let project = directory.path().join("invalid.dgi");
    let alternate = directory.path().join("alternate.dgi");
    fs::write(&project, b"invalid original project").expect("project");
    fs::write(&alternate, SAMPLE).expect("valid alternate");
    let error = DGSource::index_video(
        &project,
        Some(&alternate),
        None,
        None,
        &andean_condor::utils::cancellation::Cancellation::default(),
    )
    .expect_err("direct project must be validated, not replaced by cache alias");
    assert_eq!(
        fs::read(project).expect("preserved project"),
        b"invalid original project"
    );
    assert_eq!(fs::read(alternate).expect("preserved alternate"), SAMPLE);
    let guidance = format!("{error:#}");
    assert!(guidance.contains("valid project or input"), "{guidance}");
    assert!(
        !guidance.contains("new cache"),
        "cache override is ignored for direct DGI: {guidance}"
    );
    assert!(!guidance.contains("remove"), "{guidance}");
}
