//! CPU process/publication checks, not licensed DGDecNV decode acceptance.
#![cfg(test)]
use super::*;
use crate::{
    utils::cancellation::{Cancellation, Cancelled},
    vapoursynth::plugins::dgdecodenv::DGIndexFailure,
};
use std::{
    fs,
    path::{Path, PathBuf},
};

// Vendor's one-GOP HEVC example, not an invented complete frame-table grammar:
// https://forum.doom9.org/showthread.php?t=175328 (videoh, post #2).
const SAMPLE: &[u8] = include_bytes!("../../../../vapoursynth/plugins/dgindexnv_sample.dgi");

#[path = "../../../../../tests/core/input/vapoursynth/fixtures/indexing_support.rs"]
mod fixture;
use fixture::{assert_clean, run_index_cases};

pub(crate) fn before_publish(path: &Path, token: &Cancellation) {
    match std::env::var("VS_INDEX_CASE").as_deref() {
        Ok("publish-pre") => {
            token.request().expect("graceful");
            token.request().expect("force");
        },
        Ok("publish-sentinel") => fs::write(path, "caller-owned").expect("race sentinel"),
        Ok("publish-valid") => fs::write(path, [SAMPLE, b"winning concurrent result\n"].concat())
            .expect("valid racing winner"),
        _ => {},
    }
}

pub(crate) fn after_stdout_read() -> anyhow::Result<()> {
    if std::env::var("VS_INDEX_CASE").as_deref() == Ok("read-error") {
        return Err(std::io::Error::other("injected stdout read error").into());
    }
    Ok(())
}

pub(crate) fn after_stderr_read() {
    assert_ne!(
        std::env::var("VS_INDEX_CASE").as_deref(),
        Ok("join-panic"),
        "injected stderr worker panic"
    );
}

pub(crate) fn after_publish(token: &Cancellation) {
    if std::env::var("VS_INDEX_CASE").as_deref() == Ok("publish-post") {
        token.request().expect("graceful");
        token.request().expect("force");
    }
}

fn assert_index_publication_case() {
    if std::env::var("VS_INDEX_CASE").as_deref() != Ok("failure-retry") {
        fixture::assert_index_case();
        return;
    }
    let Some(root) = std::env::var_os("VS_INDEX_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let case = std::env::var("VS_INDEX_CASE").expect("case");
    fs::write(root.join("selected"), "yes").expect("selected");
    let source = root.join("source space 映像.mkv");
    fs::write(&source, "media placeholder").expect("source");
    let cache = source.with_extension("dgi");
    fs::write(root.join("sentinel"), "caller-owned").expect("sentinel");
    let method = VapourSynthImportMethod::DGDecNV {
        dgindexnv_executable: Some(root.join("fake")),
    };
    let token = Cancellation::default();
    if case == "failure-retry" {
        let error = source_path(&source, &method, None, &Cancellation::default())
            .expect_err("exit 42 cannot publish a partial index");
        let failed = error.downcast_ref::<DGIndexFailure>().expect("typed status/raw diagnostics");
        assert_eq!(failed.status.expect("status").code(), Some(42));
        assert!(
            matches!(&failed.stdout, crate::core::encoder::StringOrBytes::Bytes(bytes) if bytes == b"index stdout\xff\n")
        );
        assert!(
            matches!(&failed.stderr, crate::core::encoder::StringOrBytes::Bytes(bytes) if bytes == b"index stderr\xfe\n")
        );
        let context = format!("{error:#}");
        for path in [&source, &cache, &root.join("fake")] {
            assert!(context.contains(path.to_str().expect("path")), "{context}");
        }
        assert!(!cache.exists(), "failure must be retryable");
        assert_clean(&root);
        fs::write(root.join("succeed"), "yes").expect("retry");
        let data = InputModel::VapourSynth {
            path: source.clone(),
            import_method: method.clone(),
            cache_path: None,
            filters: vec![],
        };
        let mut starts = 0;
        index_source(
            &data,
            |progress| {
                if matches!(
                    progress,
                    OpenProgress::Indexing {
                        current: 0,
                        total: 0
                    }
                ) {
                    starts += 1;
                }
            },
            &token,
        )
        .expect("owning input pre-index retry");
        assert_eq!(starts, 1);
        assert_eq!(
            source_path(&source, &method, None, &Cancellation::default()).expect("retry succeeds"),
            cache
        );
        assert_eq!(
            source_path(&source, &method, None, &Cancellation::default()).expect("valid reuse"),
            cache
        );
        let (_, lines) = source_script(&source, &method, None, &Cancellation::default())
            .expect("source-options generated planner");
        assert_eq!(
            super::source_options::recorded(lines),
            serde_json::json!([{"source": cache.display().to_string()}])
        );
        assert_eq!(
            fs::read_to_string(root.join("invocations")).expect("invocations"),
            "run\nrun\n"
        );
    }
    fixture::assert_finished(&root, &case, &source, &cache);
}

mod internal_integration {
    use super::*;

    macro_rules! index_cases {
        ($name:ident, [$($case:literal),+ $(,)?]) => {
            #[test]
            fn $name() {
                run_index_cases(concat!("core::input::vapoursynth::tests::indexing::internal_integration::",stringify!($name)), &[$($case),+], assert_index_publication_case);
            }
        };
    }

    index_cases!(
        dg_index_failure_is_retryable_without_publishing_partial_files,
        ["failure-retry"]
    );
    index_cases!(
        dg_index_publication_preserves_concurrent_winners,
        ["publish-sentinel", "publish-valid"]
    );
    index_cases!(
        dg_index_reader_failures_join_before_return,
        ["read-error", "join-panic"]
    );
    index_cases!(
        dg_index_force_at_publication_preserves_only_committed_files,
        ["publish-pre", "publish-post"]
    );
}
