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
    thread,
};

// Vendor's one-GOP HEVC example, not an invented complete frame-table grammar:
// https://forum.doom9.org/showthread.php?t=175328 (videoh, post #2).
const SAMPLE: &[u8] = include_bytes!("../../../../vapoursynth/plugins/dgindexnv_sample.dgi");

#[path = "../../../../../tests/core/input/vapoursynth/fixtures/indexing_support.rs"]
mod fixture;
use fixture::{assert_argv, assert_clean, run_index_cases, wait_file};

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
    let index = |token: &Cancellation| {
        DGSource::index_video_cancel(&source, None, Some(&root.join("fake")), None, token)
    };
    if case == "failure-retry" {
        let error = source_path(&source, &method, None)
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
        index_source_cancel(
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
            source_path(&source, &method, None).expect("retry succeeds"),
            cache
        );
        assert_eq!(
            source_path(&source, &method, None).expect("valid reuse"),
            cache
        );
        let (_, lines) =
            source_script(&source, &method, None).expect("source-options generated planner");
        assert_eq!(
            super::source_options::recorded(lines),
            serde_json::json!([{"source": cache.display().to_string()}])
        );
        assert_eq!(
            fs::read_to_string(root.join("invocations")).expect("invocations"),
            "run\nrun\n"
        );
    } else if case == "existing" {
        let no_executable = root.join("must-not-run");
        let reuse = |path: &Path, cache| {
            DGSource::index_video(path, cache, Some(&no_executable), None, None)
        };
        for bytes in [
            Vec::new(),
            b"garbage".to_vec(),
            b"DGIndexNV\nCODED 50\nORDER ".to_vec(),
            b"DGIndexNV\nCODED 0\nORDER 0\n".to_vec(),
            b"DGIndexNV\nORDER 0\n".to_vec(),
            b"DGIndexNV\nCODED 50\nORDER no\n".to_vec(),
        ] {
            fs::write(&cache, &bytes).expect("invalid existing");
            let error = reuse(&source, None).expect_err("preserve invalid existing");
            assert!(
                format!("{error:#}").contains("preserved unchanged"),
                "actionable error: {error:#}"
            );
            assert_eq!(fs::read(&cache).expect("preserved"), bytes);
        }
        fs::remove_file(&cache).expect("fixture owns invalid file");
        fs::create_dir_all(&cache).expect("directory");
        assert!(reuse(&source, None).is_err());
        fs::remove_dir(&cache).expect("fixture directory");
        std::os::unix::fs::symlink(root.join("sentinel"), &cache).expect("symlink");
        assert!(reuse(&source, None).is_err());
        fs::remove_file(&cache).expect("fixture symlink");
        for bytes in [
            SAMPLE.to_vec(),
            [
                b"DGIndexNV another codec/version\r\nlegacy filename \xff\r\n".as_slice(),
                b"CODED 50\r\nORDER 0\r\n",
            ]
            .concat(),
        ] {
            fs::write(&cache, &bytes).expect("complete structural sample");
            assert_eq!(reuse(&source, None).expect("default reuse"), cache);
            assert_eq!(
                reuse(&source, Some(cache.as_path())).expect("explicit reuse"),
                cache
            );
            assert_eq!(
                reuse(&cache, Some(source.as_path()))
                    .expect("dgi is already the source, no alias overwrite"),
                cache
            );
            assert_eq!(fs::read(&cache).expect("unchanged"), bytes);
        }
        // A large sparse body catches accidental full-index slurping without a movie fixture.
        use std::io::{Seek, SeekFrom, Write};
        let mut large = fs::File::create(&cache).expect("large structural fixture");
        large.write_all(b"DGIndexNV\n").expect("header");
        large.seek(SeekFrom::Start(64 * 1024 * 1024)).expect("sparse frame body");
        large.write_all(b"\nCODED 50\nORDER 0\n").expect("footer");
        drop(large);
        assert_eq!(
            reuse(&source, None).expect("bounded header/tail read"),
            cache
        );
        assert_eq!(
            fs::read(&source).expect("source preserved"),
            b"media placeholder"
        );
        assert!(!root.join("invocations").exists());
    } else if case == "force-retry" {
        assert_eq!(
            // SAFETY: This exact child test runs alone under a finite Linux supervisor.
            unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) },
            0
        );
        let signal = token.clone();
        let controller = thread::spawn({
            let root = root.clone();
            move || {
                wait_file(&root, "grandchild");
                signal.request().expect("graceful");
                signal.request().expect("forced");
            }
        });
        let error = index(&token).expect_err("silent child with inherited pipes must be forced");
        controller.join().expect("controller");
        assert_eq!(error.downcast_ref::<Cancelled>(), Some(&Cancelled::Forced));
        let pid: i32 = fs::read_to_string(root.join("grandchild"))
            .expect("grandchild")
            .parse()
            .expect("PID");
        let mut status = 0;
        // SAFETY: Reap only this private fixture's adopted grandchild.
        assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
        assert!(libc::WIFSIGNALED(status));
        assert_eq!(libc::WTERMSIG(status), libc::SIGKILL);
        let leader: i32 =
            fs::read_to_string(root.join("leader")).expect("leader").parse().expect("PID");
        // SAFETY: Signal zero only observes existence.
        assert_eq!(unsafe { libc::kill(leader, 0) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
        assert!(!cache.exists());
        assert_clean(&root);
        fs::write(root.join("succeed"), "yes").expect("retry");
        assert_eq!(
            index(&Cancellation::default()).expect("fresh token retries"),
            cache
        );
    } else if case == "graceful" {
        let controller = thread::spawn({
            let root = root.clone();
            let token = token.clone();
            move || {
                wait_file(&root, "active");
                token.request().expect("graceful");
                fs::write(root.join("release"), "yes").expect("release");
            }
        });
        assert_eq!(
            index(&token).expect("admitted work completes during graceful"),
            cache
        );
        controller.join().expect("controller");
    } else if case == "concurrent" {
        thread::scope(|scope| {
            let a = scope.spawn(|| index(&Cancellation::default()));
            let b = scope.spawn(|| index(&Cancellation::default()));
            while fs::read_dir(&root)
                .expect("listing")
                .filter(|e| {
                    e.as_ref().expect("entry").file_name().to_string_lossy().starts_with("active-")
                })
                .count()
                < 2
            {
                thread::yield_now();
            }
            fs::write(root.join("release"), "yes").expect("release");
            assert_eq!(a.join().expect("a").expect("winner or valid reuse"), cache);
            assert_eq!(b.join().expect("b").expect("winner or valid reuse"), cache);
        });
        assert_eq!(fs::read(&cache).expect("one complete result"), SAMPLE);
    } else if case == "publish-post" {
        assert_eq!(
            index(&token).expect("committed index survives later force"),
            cache
        );
        assert!(token.is_forced());
        assert_eq!(fs::read(&cache).expect("committed"), SAMPLE);
    } else if case == "publish-valid" {
        assert_eq!(
            index(&token).expect("reuse a valid no-clobber winner"),
            cache
        );
        assert_eq!(
            fs::read(&cache).expect("winner not overwritten"),
            [SAMPLE, b"winning concurrent result\n"].concat()
        );
    } else if case == "observer-panic" {
        let result = DGSource::index_video_cancel(
            &source,
            None,
            Some(&root.join("fake")),
            Some(&mut |_, total| {
                assert_eq!(total, 0, "observer failed");
            }),
            &token,
        );
        assert!(
            result.is_err(),
            "callback panic must become an error after drain/reap"
        );
        assert!(!cache.exists());
    } else if case == "spawn" {
        assert!(
            DGSource::index_video_cancel(
                &source,
                None,
                Some(&root.join("missing-executable")),
                None,
                &token
            )
            .is_err()
        );
        assert!(!cache.exists());
    } else {
        let error = index(&token).expect_err("invalid/forced candidate cannot publish");
        if ["missing", "zero", "garbage", "truncated", "directory", "symlink"]
            .contains(&case.as_str())
        {
            assert!(
                format!("{error:#}").contains("Invalid DGIndexNV candidate"),
                "{error:#}"
            );
        }
        if case == "read-error" {
            assert!(
                format!("{error:#}").contains("injected stdout read error"),
                "{error:#}"
            );
        }
        if case == "join-panic" {
            assert!(
                format!("{error:#}").contains("Joining DGIndexNV stderr drain"),
                "{error:#}"
            );
        }
        if case == "publish-pre" {
            assert_eq!(error.downcast_ref::<Cancelled>(), Some(&Cancelled::Forced));
        }
        if case == "publish-sentinel" {
            assert_eq!(
                fs::read(&cache).expect("race sentinel preserved"),
                b"caller-owned"
            );
        } else {
            assert!(!cache.exists());
        }
    }
    let invocations = fs::read_to_string(root.join("invocations")).unwrap_or_default();
    let expected = match case.as_str() {
        "existing" | "spawn" => "",
        "failure-retry" | "force-retry" | "concurrent" => "run\nrun\n",
        _ => "run\n",
    };
    assert_eq!(
        invocations, expected,
        "exact child execution; error cases must not pass without spawning"
    );
    assert_argv(&root, &source, &cache);
    assert_clean(&root);
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
        dg_index_candidates_are_validated_before_publication,
        [
            "missing",
            "zero",
            "garbage",
            "truncated",
            "directory",
            "symlink",
            "existing",
            "spawn"
        ]
    );
    index_cases!(
        dg_index_force_reaps_owned_processes_before_cleanup,
        ["force-retry"]
    );
    index_cases!(
        dg_index_graceful_cancellation_finishes_admitted_work,
        ["graceful"]
    );
    index_cases!(
        dg_index_publication_preserves_concurrent_winners,
        ["publish-sentinel", "publish-valid", "concurrent"]
    );
    index_cases!(
        dg_index_reader_and_observer_failures_join_before_return,
        ["observer-panic", "read-error", "join-panic"]
    );
    index_cases!(
        dg_index_force_at_publication_preserves_only_committed_files,
        ["publish-pre", "publish-post"]
    );
}
