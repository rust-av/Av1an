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
    process::Command,
    thread,
};

// Vendor's one-GOP HEVC example, not an invented complete frame-table grammar:
// https://forum.doom9.org/showthread.php?t=175328 (videoh, post #2).
const SAMPLE: &[u8] = include_bytes!("../../../../vapoursynth/plugins/dgindexnv_sample.dgi");

#[test]
fn footer_markers_reuse_in_either_order_with_numeric_validation() {
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
        let result = DGSource::index_video(&source, Some(&cache), Some(&executable), None, None);
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
    let error = DGSource::index_video(&source, Some(&source), None, None, None)
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
    let error = DGSource::index_video(&project, Some(&alternate), None, None, None)
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

fn run_index_cases(test: &str, cases: &[&str]) {
    if std::env::var_os("VS_INDEX_ROOT").is_some() {
        let case = std::env::var("VS_INDEX_CASE").expect("case");
        assert!(
            cases.contains(&case.as_str()),
            "selected indexing case belongs to this contract"
        );
        assert_index_publication_case();
        println!("dg-index {case}: assertions completed");
        return;
    }
    let dir = tempfile::tempdir().expect("fixture");
    fs::write(
        dir.path().join("fake.rs"),
        include_str!("../../../../../tests/core/encoder/fixtures/lifecycle_process.rs"),
    )
    .expect("fixture source");
    let status = Command::new("rustc")
        .arg(dir.path().join("fake.rs"))
        .arg("-o")
        .arg(dir.path().join("fake"))
        .status()
        .expect("rustc");
    assert!(status.success());
    for case in cases {
        let root = dir.path().join(case);
        fs::create_dir_all(&root).expect("case");
        fs::write(root.join("sample.dgi"), SAMPLE).expect("sample");
        std::os::unix::fs::symlink(dir.path().join("fake"), root.join("fake")).expect("executable");
        let output = Command::new(root.join("fake"))
            .arg("supervise-index-publication")
            .arg(std::env::current_exe().expect("test binary"))
            .arg(&root)
            .arg(case)
            .arg(format!(
                "core::input::vapoursynth::tests::indexing::internal_integration::{test}"
            ))
            .output()
            .expect("supervisor");
        let log = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        eprintln!("{log}");
        assert!(output.status.success(), "{case}: {log}");
        assert!(
            log.contains("test result: ok. 1 passed;"),
            "exact child selected: {log}"
        );
        assert!(
            log.contains(&format!("dg-index {case}: assertions completed")),
            "index assertions executed: {log}"
        );
        assert!(
            log.contains(&format!(
                "process lifecycle case={case} selected=true rescued=0"
            )),
            "{log}"
        );
    }
}

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

fn wait_file(root: &Path, name: &str) {
    while !root.join(name).exists() {
        thread::yield_now();
    }
}

fn assert_clean(root: &Path) {
    assert!(
        !fs::read_dir(root).expect("listing").any(|entry| entry
            .expect("entry")
            .file_name()
            .to_string_lossy()
            .starts_with(".av1an-dgi-")),
        "owned temp directory must be gone after readers and child join"
    );
    assert_eq!(
        fs::read(root.join("sentinel")).expect("sentinel"),
        b"caller-owned"
    );
}

fn assert_argv(root: &Path, source: &Path, cache: &Path) {
    for entry in fs::read_dir(root).expect("listing") {
        let path = entry.expect("entry").path();
        if !path.file_name().expect("name").to_string_lossy().starts_with("argv-") {
            continue;
        }
        let text = fs::read_to_string(path).expect("argv");
        let argv: Vec<_> = text.lines().collect();
        assert_eq!(
            &argv[..4],
            &["-h", "-i", source.to_str().expect("source"), "-o"]
        );
        assert_eq!(argv.len(), 5);
        let output = Path::new(argv[4]);
        assert!(output.is_absolute());
        assert_ne!(output, cache, "never give the final path to the indexer");
        assert_eq!(output.extension().expect("extension"), "dgi");
        assert_eq!(
            output.parent().expect("owned directory").parent(),
            cache.parent()
        );
        assert!(!output.exists(), "private output removed after completion");
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
            fn $name() { run_index_cases(stringify!($name), &[$($case),+]); }
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
