//! Test-only DG supervisor and owned artifact assertions shared across boundaries.
use super::{Cancellation, Cancelled, DGSource, SAMPLE};
use std::{fs, path::Path, process::Command, thread};

pub(super) fn assert_finished(root: &Path, case: &str, source: &Path, cache: &Path) {
    let invocations = fs::read_to_string(root.join("invocations")).unwrap_or_default();
    let expected = match case {
        "existing" | "spawn" => "",
        "failure-retry" | "force-retry" | "concurrent" => "run\nrun\n",
        _ => "run\n",
    };
    assert_eq!(
        invocations, expected,
        "exact child execution; error cases must not pass without spawning"
    );
    assert_argv(root, source, cache);
    assert_clean(root);
}

pub(super) fn assert_index_case() {
    let root = std::path::PathBuf::from(std::env::var_os("VS_INDEX_ROOT").expect("root"));
    let case = std::env::var("VS_INDEX_CASE").expect("case");
    assert_ne!(
        case, "failure-retry",
        "source planner retry assertions remain private"
    );
    fs::write(root.join("selected"), "yes").expect("selected");
    let source = root.join("source space 映像.mkv");
    fs::write(&source, "media placeholder").expect("source");
    let cache = source.with_extension("dgi");
    fs::write(root.join("sentinel"), "caller-owned").expect("sentinel");
    let token = Cancellation::default();
    let index = |token: &Cancellation| {
        DGSource::index_video(&source, None, Some(&root.join("fake")), None, token)
    };
    if case == "existing" {
        let no_executable = root.join("must-not-run");
        let reuse = |path: &Path, cache| {
            DGSource::index_video(
                path,
                cache,
                Some(&no_executable),
                None,
                &Cancellation::default(),
            )
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
            // SAFETY: This exact child runs alone under the finite Linux supervisor.
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
        // SAFETY: Reap only this fixture's known adopted grandchild.
        assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
        assert!(libc::WIFSIGNALED(status));
        assert_eq!(libc::WTERMSIG(status), libc::SIGKILL);
        let leader: i32 =
            fs::read_to_string(root.join("leader")).expect("leader").parse().expect("PID");
        // SAFETY: Signal zero observes existence without signalling the process.
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
        let result = DGSource::index_video(
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
            DGSource::index_video(
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
    assert_finished(&root, &case, &source, &cache);
}

pub(super) fn run_index_cases(test: &str, cases: &[&str], assert_case: impl FnOnce()) {
    if std::env::var_os("VS_INDEX_ROOT").is_some() {
        let case = std::env::var("VS_INDEX_CASE").expect("case");
        assert!(
            cases.contains(&case.as_str()),
            "selected indexing case belongs to this contract"
        );
        assert_case();
        println!("dg-index {case}: assertions completed");
        return;
    }
    let dir = tempfile::tempdir().expect("fixture");
    fs::write(
        dir.path().join("fake.rs"),
        include_str!("../../../encoder/fixtures/lifecycle_process.rs"),
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
            .arg(test)
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

pub(super) fn wait_file(root: &Path, name: &str) {
    while !root.join(name).exists() {
        thread::yield_now();
    }
}

pub(super) fn assert_clean(root: &Path) {
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

pub(super) fn assert_argv(root: &Path, source: &Path, cache: &Path) {
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
