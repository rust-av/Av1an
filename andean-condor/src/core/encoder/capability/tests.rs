//! Catches shared/PID scratch, early cleanup, caller-path deletion and forced-cache poisoning.
mod internal_integration {
    use super::super::*;
    use crate::utils::cancellation::Cancelled;
    use std::{
        fs,
        os::unix::fs::{PermissionsExt, symlink},
        thread,
        time::{Duration, Instant},
    };

    #[test]
    fn concurrent_capability_probes_use_disjoint_private_scratch() {
        probe_cases(
            "concurrent_capability_probes_use_disjoint_private_scratch",
            &["overlap"],
            false,
        );
    }

    #[test]
    fn failed_capability_probes_reap_processes_and_remove_scratch() {
        probe_cases(
            "failed_capability_probes_reap_processes_and_remove_scratch",
            &["failure", "feed-failure", "spawn", "no-temp", "missing", "directory"],
            false,
        );
    }

    #[test]
    fn forced_capability_probe_does_not_cache_failure() {
        probe_cases(
            "forced_capability_probe_does_not_cache_failure",
            &["force"],
            false,
        );
    }

    #[test]
    fn graceful_capability_probe_finishes_with_private_scratch_alive() {
        probe_cases(
            "graceful_capability_probe_finishes_with_private_scratch_alive",
            &["graceful"],
            false,
        );
    }

    #[test]
    fn fileless_x264_probe_needs_no_scratch() {
        probe_cases("fileless_x264_probe_needs_no_scratch", &["no-temp"], true);
    }

    fn probe_cases(test: &str, selected_modes: &[&str], fileless: bool) {
        if std::env::var_os("CAPABILITY_PROBE_ROOT").is_some() {
            let mode = std::env::var("CAPABILITY_PROBE_MODE").expect("mode");
            let kind = std::env::var("CAPABILITY_PROBE_KIND").expect("kind");
            assert!(selected_modes.contains(&mode.as_str()) && (kind == "x264") == fileless);
            assert_probe_scratch_lifetime();
            println!("capability {kind}/{mode}: assertions completed");
            return;
        }
        let fixture = tempfile::tempdir().expect("fixture");
        fs::write(
            fixture.path().join("fake.rs"),
            include_str!("fixtures/probe.rs"),
        )
        .expect("source");
        crate::core::encoder::tests::support::run_bounded(
            Command::new("rustc")
                .arg(fixture.path().join("fake.rs"))
                .arg("-o")
                .arg(fixture.path().join("fake")),
            &fixture.path().join("rustc.log"),
        );
        let mut failures = Vec::new();
        let mut selected = 0;
        for kind in ["x265", "svt-zone", "svt-container", "x264"] {
            if (kind == "x264") != fileless {
                continue;
            }
            let modes: &[&str] = if kind == "x264" {
                &["no-temp"]
            } else {
                &["overlap", "failure", "force", "graceful", "feed-failure", "spawn", "no-temp"]
            };
            for mode in modes
                .iter()
                .copied()
                .chain((kind == "svt-container").then_some("missing"))
                .chain((kind == "svt-container").then_some("directory"))
            {
                if !selected_modes.contains(&mode) {
                    continue;
                }
                selected += 1;
                let root = fixture.path().join(format!("{kind}-{mode}"));
                fs::create_dir_all(root.join("tmp")).expect("isolated temp root");
                symlink(fixture.path().join("fake"), root.join("fake")).expect("executable");
                let result = std::panic::catch_unwind(|| {
                    let log = crate::core::encoder::tests::support::run_bounded(
                        Command::new(std::env::current_exe().expect("test binary"))
                            .args([
                                "--exact",
                                &format!(
                                    "core::encoder::capability::tests::internal_integration::{test}"
                                ),
                                "--nocapture",
                            ])
                            .env("CAPABILITY_PROBE_ROOT", &root)
                            .env("CAPABILITY_PROBE_KIND", kind)
                            .env("CAPABILITY_PROBE_MODE", mode)
                            .env("TMPDIR", root.join("tmp")),
                        &root.join("child.log"),
                    );
                    assert!(
                        log.contains("test result: ok. 1 passed;"),
                        "child was selected: {log}"
                    );
                    assert!(
                        log.contains(&format!("capability {kind}/{mode}: assertions completed")),
                        "probe assertions executed: {log}"
                    );
                    print!("{kind}/{mode}: {log}");
                });
                if result.is_err() {
                    failures.push(format!(
                        "{kind}/{mode}: {}",
                        fs::read_to_string(root.join("child.log")).expect("log")
                    ));
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
        assert!(selected > 0, "registered probe contract selected cases");
    }

    fn records(root: &Path) -> Vec<PathBuf> {
        fs::read_dir(root)
            .expect("records")
            .map(|entry| entry.expect("entry").path())
            .filter(|path| path.file_name().expect("name").to_string_lossy().starts_with("record-"))
            .collect()
    }

    fn scratch(record: &Path) -> PathBuf {
        PathBuf::from(fs::read_to_string(record.join("path")).expect("native path"))
    }

    fn probe(kind: &str, executable: &Path, cancellation: &Cancellation) -> anyhow::Result<bool> {
        if kind == "svt-container" {
            encoder_container(EncoderBase::SVTAV1, executable, cancellation)
                .map(|result| result == Some(Container::Ivf))
        } else {
            check_capability(
                match kind {
                    "x265" => EncoderBase::X265,
                    "svt-zone" => EncoderBase::SVTAV1,
                    _ => EncoderBase::X264,
                },
                executable,
                EncoderCapability::Zoning,
                cancellation,
            )
        }
    }

    fn assert_probe_scratch_lifetime() {
        let Some(root) = std::env::var_os("CAPABILITY_PROBE_ROOT") else {
            return;
        };
        let root = PathBuf::from(root);
        let kind = std::env::var("CAPABILITY_PROBE_KIND").expect("kind");
        let mode = std::env::var("CAPABILITY_PROBE_MODE").expect("mode");
        let tmp = root.join("tmp");
        let executable = root.join("fake");
        let cancellation = Cancellation::default();
        fs::write(root.join("mode"), &mode).expect("mode");
        // Only this subprocess uses this TMPDIR: no shared-root sentinels or global mutation.
        let mut sentinels = Vec::new();
        for suffix in ["zonefile", "config", "out"] {
            let prefix = if suffix == "out" { "container" } else { "zone" };
            let old = tmp.join(format!(
                "av1an-{prefix}-probe-{}.{suffix}",
                std::process::id()
            ));
            let target = root.join(format!("sentinel-{suffix}"));
            fs::write(&target, b"caller-owned").expect("sentinel");
            symlink(&target, &old).expect("old predictable symlink");
            sentinels.push((old, target));
        }
        if mode == "no-temp" {
            // A nonexistent parent deterministically fails directory creation, even as root.
            for (old, _) in &sentinels {
                fs::remove_file(old).expect("fixture link");
            }
            fs::remove_dir(&tmp).expect("fixture temp root");
            fs::write(root.join("release"), b"fileless probe").expect("release");
            assert_eq!(
                probe(&kind, &executable, &cancellation).expect("probe"),
                kind == "x264"
            );
            assert!(!tmp.exists());
            assert_eq!(records(&root).len(), usize::from(kind == "x264"));
            println!(
                "capability-probe {kind}/no-temp: creation failure leaves no scratch; X264 stays fileless"
            );
            return;
        }
        if mode == "spawn" {
            assert!(
                !probe(&kind, &root.join("missing-executable"), &cancellation)
                    .expect("spawn failure")
            );
        } else if mode == "feed-failure" {
            assert!(!probe(&kind, &executable, &cancellation).expect("closed native stdin"));
        } else {
            let count = if mode == "overlap" { 2 } else { 1 };
            let results = thread::scope(|scope| {
                let mut handles = Vec::new();
                for _ in 0..count {
                    handles.push(scope.spawn(|| probe(&kind, &executable, &cancellation)));
                }
                let deadline = Instant::now() + Duration::from_secs(3);
                let active = loop {
                    let active = records(&root);
                    if active.len() == count
                        && active.iter().all(|record| record.join("ready").exists())
                    {
                        break active;
                    }
                    assert!(
                        Instant::now() < deadline,
                        "both uncached probes must overlap before release"
                    );
                    thread::yield_now();
                };
                // Collect evidence while children are active; assert after release/join so RED is finite.
                let evidence: Vec<_> = active
                    .iter()
                    .map(|record| {
                        let path = scratch(record);
                        let parent = path.parent().expect("scratch parent").to_owned();
                        let private =
                            fs::metadata(&parent).expect("live parent").permissions().mode()
                                & 0o777
                                == 0o700;
                        let payload = fs::read(&path).ok();
                        (record.clone(), path, parent, private, payload)
                    })
                    .collect();
                if mode == "force" || mode == "graceful" {
                    cancellation.request().expect("graceful");
                }
                if mode == "force" {
                    cancellation.request().expect("forced");
                } else {
                    fs::write(root.join("release"), b"finish").expect("release");
                }
                let results: Vec<_> = handles
                    .into_iter()
                    .map(|handle| handle.join().expect("probe joined"))
                    .collect();
                for (record, path, parent, private, payload) in &evidence {
                    assert_ne!(
                        parent,
                        &tmp,
                        "native probe received shared-root scratch: {}",
                        path.display()
                    );
                    assert_eq!(parent.parent(), Some(tmp.as_path()));
                    assert!(private, "scratch must exclude other users");
                    println!(
                        "capability-probe native scratch={} private=true",
                        path.display()
                    );
                    if !matches!(mode.as_str(), "missing" | "directory") {
                        let expected = match kind.as_str() {
                            "x265" => b"0 --crf 30\n".to_vec(),
                            "svt-zone" => b"Zones : 0,0,30\nForceKeyFrames : 0f\n".to_vec(),
                            _ => format!(
                                "DKIF{}",
                                record
                                    .file_name()
                                    .expect("record")
                                    .to_string_lossy()
                                    .trim_start_matches("record-")
                            )
                            .into_bytes(),
                        };
                        assert_eq!(
                            payload.as_ref(),
                            Some(&expected),
                            "each active probe reads its own initialized payload"
                        );
                    }
                    if mode != "force" {
                        assert!(
                            record.join("alive-until-exit").exists(),
                            "scratch outlives native owner"
                        );
                    }
                    assert!(
                        !parent.exists(),
                        "scratch cleaned after native wait and output detection"
                    );
                    if kind == "svt-container" {
                        assert!(
                            fs::read_to_string(record.join("argv"))
                                .expect("argv")
                                .ends_with("--webm\n0")
                        );
                    }
                }
                if count == 2 {
                    assert_ne!(
                        evidence[0].2, evidence[1].2,
                        "same executable cache misses must have independent parents"
                    );
                }
                results
            });
            for result in results {
                if mode == "force" {
                    assert_eq!(
                        result.expect_err("force stays typed").downcast_ref::<Cancelled>(),
                        Some(&Cancelled::Forced)
                    );
                } else {
                    assert_eq!(
                        result.expect("probe"),
                        !matches!(mode.as_str(), "failure" | "missing" | "directory")
                    );
                }
            }
            if mode == "force" {
                fs::write(root.join("mode"), "success").expect("retry mode");
                fs::write(root.join("release"), b"retry").expect("release retry");
                assert!(
                    probe(&kind, &executable, &Cancellation::default())
                        .expect("force did not cache a failure")
                );
            } else if mode == "overlap" {
                assert!(
                    probe(&kind, &executable, &Cancellation::default()).expect("cached support")
                );
                assert_eq!(records(&root).len(), 2, "cache identity unchanged");
            }
        }
        for record in records(&root) {
            let path = scratch(&record);
            assert!(!path.exists(), "owned file cleaned");
            let pid = record
                .file_name()
                .expect("record")
                .to_string_lossy()
                .trim_start_matches("record-")
                .parse::<i32>()
                .expect("PID");
            assert_eq!(
                // SAFETY: Signal zero only observes this known fixture PID; it cannot kill anything.
                unsafe { libc::kill(pid, 0) },
                -1,
                "native child reaped, not rescued"
            );
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ESRCH)
            );
        }
        for (old, target) in sentinels {
            assert_eq!(
                fs::read_link(&old).expect("caller symlink must survive"),
                target
            );
            assert_eq!(fs::read(&old).expect("caller data"), b"caller-owned");
        }
        assert_eq!(
            fs::read_dir(&tmp).expect("scratch listing").count(),
            3,
            "only caller-owned links survive"
        );
        println!(
            "capability-probe {kind}/{mode}: owned scratch cleaned; native children reaped; sentinel links/data unchanged"
        );
    }
}
