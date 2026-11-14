//! Every case runs the public owner or its precise owning seam in an isolated process.
pub(crate) use internal_integration::{
    after_process_registration, before_output_commit, before_process_registration,
    before_stream_return, observe_reap, output_drop,
};

mod internal_integration {
    use super::super::*;
    use crate::core::{
        Condor,
        output::Output,
        sequence::{
            Sequence,
            ffmpeg_filter::FfmpegFilter,
            parallel_encoder::{ParallelEncoder, Task},
            scene_concatenator::SceneConcatenator,
            serial_encoder::SerialEncoder,
            zone_encoder::{ZoneEncoder, ZonePlan},
        },
    };
    use crate::models::{
        input::{ImportMethod, Input as InputModel, VapourSynthScriptSource},
        scene::Scene,
        sequence::{DefaultSequenceConfig, DefaultSequenceData},
    };
    use std::{fs, io};

    macro_rules! lifecycle_cases {
    ($name:ident, [$($case:literal),+ $(,)?]) => {
        #[test]
        fn $name() { run_lifecycle_cases(stringify!($name), &[$($case),+]); }
    };
}

    lifecycle_cases!(
        forced_scene_cancellation_reaps_encoders_before_removing_outputs,
        [
            "force-vs",
            "mid-production",
            "force-native",
            "stream-idle",
            "replay-full",
            "serial",
            "serial-native",
            "zone"
        ]
    );
    lifecycle_cases!(
        forced_cancellation_with_full_producer_retains_raw_diagnostics,
        ["stdin-full"]
    );
    lifecycle_cases!(
        graceful_scene_cancellation_finishes_active_work_and_skips_queued_work,
        ["graceful-vs", "graceful-native", "graceful-sigint"]
    );
    lifecycle_cases!(
        registration_races_reap_the_child_before_spawn_returns,
        ["registration", "filter-spawn-race"]
    );
    lifecycle_cases!(
        force_at_stream_dispatch_return_preserves_completed_output,
        ["stream-idle-return"]
    );
    lifecycle_cases!(
        owned_process_drop_reaps_the_process_tree,
        ["guard-drop", "guard-unwind", "after-completion"]
    );
    lifecycle_cases!(
        lost_process_ownership_disarms_later_signals,
        ["lost-wait", "lost-kill"]
    );
    lifecycle_cases!(
        unsupported_sigchld_autoreap_is_rejected_before_spawn,
        ["autoreap-ignore", "autoreap-nocldwait"]
    );
    lifecycle_cases!(
        forced_process_exit_reaps_a_leader_that_changed_groups,
        ["leader-changes-group"]
    );
    lifecycle_cases!(
        forced_process_exit_reaps_descendants_after_the_leader_exits,
        ["leader-exits"]
    );
    lifecycle_cases!(
        forced_stream_cancellation_preserves_existing_caller_output,
        ["capture", "encoder-existing"]
    );
    lifecycle_cases!(
        forced_filter_cancellation_unblocks_empty_and_full_queues,
        ["filter-empty", "filter-full"]
    );
    lifecycle_cases!(
        forced_mux_and_index_processes_remove_only_owned_artifacts,
        ["dg", "ffmpeg", "ffmpeg-existing", "mkvmerge", "mkv-group"]
    );
    lifecycle_cases!(forced_zone_probe_reaps_its_process_tree, ["zone-probe"]);
    lifecycle_cases!(
        forced_ffms_indexing_does_not_publish_a_partial_cache,
        ["ffms-index"]
    );
    lifecycle_cases!(
        pre_spawn_cancellation_never_starts_an_encoder,
        ["pre-spawn"]
    );
    lifecycle_cases!(
        waiter_failure_keeps_statistics_alive_through_process_reaping,
        ["stats-error", "stats-unwind"]
    );
    lifecycle_cases!(
        forced_output_commit_preserves_only_completed_artifacts,
        [
            "commit-input-pre",
            "commit-input-post",
            "commit-stream-pre",
            "commit-stream-post",
            "commit-pass-input-pre",
            "commit-pass-input-post",
            "commit-pass-stream-pre",
            "commit-pass-stream-post",
            "commit-all2-pre",
            "commit-all2-post",
            "commit-all2-between",
            "commit-ffmpeg-pre",
            "commit-ffmpeg-post",
            "commit-mkvmerge-pre",
            "commit-mkvmerge-post",
            "commit-ivf-pre",
            "commit-ivf-post",
            "commit-dg-pre",
            "commit-dg-post",
            "commit-ffms-pre",
            "commit-ffms-post"
        ]
    );

    fn run_lifecycle_cases(test: &str, cases: &[&str]) {
        if std::env::var_os("PROCESS_LIFECYCLE_ROOT").is_some() {
            let case = std::env::var("PROCESS_LIFECYCLE_CASE").expect("case");
            assert!(
                cases.contains(&case.as_str()),
                "selected lifecycle case belongs to this contract"
            );
            assert_lifecycle_case();
            println!("lifecycle {case}: assertions completed");
            return;
        }
        let dir = tempfile::tempdir().expect("fixture");
        fs::write(
            dir.path().join("fake.rs"),
            include_str!("../../../../tests/core/encoder/fixtures/lifecycle_process.rs"),
        )
        .expect("source");
        crate::core::encoder::tests::support::run_bounded(
            Command::new("rustc")
                .arg(dir.path().join("fake.rs"))
                .arg("-o")
                .arg(dir.path().join("fake")),
            &dir.path().join("rustc.log"),
        );
        let mut failures = Vec::new();
        for case in cases {
            let root = dir.path().join(case);
            fs::create_dir_all(&root).expect("case directory");
            // Reuse an immutable binary while the unfiltered suite forks/execs in parallel.
            std::os::unix::fs::symlink(dir.path().join("fake"), root.join("fake"))
                .expect("fixture executable");
            let output = Command::new(root.join("fake"))
                .arg("supervise")
                .arg(std::env::current_exe().expect("test binary"))
                .arg(&root)
                .arg(case)
                .arg(format!(
                    "core::encoder::tests::process_lifecycle::internal_integration::{test}"
                ))
                .output()
                .unwrap_or_else(|error| panic!("bounded supervisor {case}: {error}"));
            let log = format!(
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            eprintln!("{log}");
            if !output.status.success()
                || !log.contains("test result: ok. 1 passed;")
                || !log.contains(&format!("lifecycle {case}: assertions completed"))
                || !log.lines().any(|line| {
                    line == format!("process lifecycle case={case} selected=true rescued=0")
                })
            {
                failures.push(format!("{case}: {log}"));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    fn wait_file(root: &Path, name: &str) {
        while !root.join(name).exists() {
            thread::yield_now();
        }
    }

    fn registration_fixture_root(command: &Command) -> Option<PathBuf> {
        let root = PathBuf::from(std::env::var_os("PROCESS_LIFECYCLE_ROOT")?);
        (command.get_program() == root.join("fake")).then_some(root)
    }

    pub(crate) fn before_process_registration(command: &Command, pid: u32) {
        if let Some(root) = registration_fixture_root(command)
            && matches!(
                std::env::var("PROCESS_LIFECYCLE_CASE").as_deref(),
                Ok("mid-production" | "stream-idle" | "serial" | "serial-native")
            )
        {
            fs::write(
                root.join(format!("registration-blocked-{pid}")),
                "before register",
            )
            .expect("registration barrier");
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            while !root.join(format!("registration-release-{pid}")).exists() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "release registration barrier"
                );
                thread::sleep(std::time::Duration::from_millis(1));
            }
        }
    }

    pub(crate) fn after_process_registration(command: &Command, pid: u32) {
        if let Some(root) = registration_fixture_root(command) {
            // The child can publish readiness before spawn returns; only this parent knows registration.
            fs::write(root.join(format!("registered-{pid}")), "registered")
                .expect("post-registration handshake");
        }
    }

    pub(crate) fn before_stream_return(cancellation: &Cancellation) {
        if std::env::var("PROCESS_LIFECYCLE_CASE").as_deref() == Ok("stream-idle-return") {
            cancellation.request().expect("graceful after all workers joined");
            cancellation.request().expect("force before owning dispatch returns");
        }
    }

    thread_local! {
        static OUTPUT_DROPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
        static OUTPUT_COMMITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    fn output_case(path: &Path) -> Option<(String, usize)> {
        let case = std::env::var("PROCESS_LIFECYCLE_CASE").ok()?;
        let root = PathBuf::from(std::env::var_os("PROCESS_LIFECYCLE_ROOT")?);
        if !case.starts_with("commit-") || path != root.join("committed") {
            return None;
        }
        let nth = if case.starts_with("commit-all2") {
            3
        } else if case.starts_with("commit-input-") || case.starts_with("commit-stream-") {
            2
        } else {
            1
        };
        Some((case, nth))
    }

    pub(crate) fn before_output_commit(path: &Path, cancellation: &Cancellation) {
        if let Some((case, nth)) = output_case(path) {
            let count = OUTPUT_COMMITS.get() + 1;
            OUTPUT_COMMITS.set(count);
            if case.ends_with("-pre") && count == nth {
                cancellation.request().expect("graceful immediately before commit");
                cancellation.request().expect("force immediately before commit");
            }
        }
    }

    pub(crate) fn output_drop(path: &Path, cancellation: &Cancellation) {
        if let Some((case, nth)) = output_case(path) {
            let count = OUTPUT_DROPS.get() + 1;
            OUTPUT_DROPS.set(count);
            if (case.ends_with("-post") && count == nth)
                || (case.ends_with("-pre") && OUTPUT_COMMITS.get() == 0 && count == nth)
                || (case.ends_with("-between") && count == 1)
            {
                cancellation.request().expect("graceful at output Drop seam");
                cancellation.request().expect("force at output Drop seam");
            }
        }
    }

    pub(crate) fn observe_reap(pid: u32) {
        let Ok(case) = std::env::var("PROCESS_LIFECYCLE_CASE") else {
            return;
        };
        if !case.starts_with("stats-") {
            return;
        }
        let root = PathBuf::from(std::env::var_os("PROCESS_LIFECYCLE_ROOT").expect("root"));
        assert_eq!(
            fs::read_to_string(root.join("leader"))
                .expect("leader")
                .parse::<u32>()
                .expect("PID"),
            pid
        );
        let stats = fs::read_to_string(root.join("stats-dir")).expect("stats directory");
        fs::write(
            root.join("stats-through-reap"),
            Path::new(&stats).is_dir().to_string(),
        )
        .expect("lifetime observation");
    }

    fn input(root: &Path, native: bool) -> Input {
        if native {
            let path = root.join("input.y4m");
            let mut bytes = b"YUV4MPEG2 W64 H64 F24:1 Ip A1:1 C420jpeg\n".to_vec();
            for _ in 0..2 {
                bytes.extend_from_slice(b"FRAME\n");
                bytes.extend(vec![48; 64 * 64 * 3 / 2]);
            }
            fs::write(&path, bytes).expect("native input");
            Input::from_video(&InputModel::Video {
                path,
                import_method: ImportMethod::FFMS2 {
                    index: None,
                    cache_path: Some(root.join("input.ffindex")),
                },
                filters: vec![],
            })
            .expect("native FFMS2")
        } else {
            let path = root.join("input.vpy");
            let script = if std::env::var("PROCESS_LIFECYCLE_CASE").as_deref()
                == Ok("mid-production")
            {
                format!(
                    "import os,time\nfrom vapoursynth import core,YUV420P8\nc=core.std.BlankClip(width=64,height=64,length=9,format=YUV420P8)\ndef mark(n):\n if n==3:\n  open({:?},'w').write('frame3')\n  deadline=time.monotonic()+8\n  while not os.path.exists({:?}) and time.monotonic()<deadline: pass\n return c\ncore.std.FrameEval(c,mark).set_output()\n",
                    root.join("frame-k").to_str().expect("path"),
                    root.join("release-frame").to_str().expect("path")
                )
            } else {
                format!(
                    "from vapoursynth import core, YUV420P8\nc=core.std.BlankClip(width=64,height=64,length=2,format=YUV420P8)\ndef mark(n):\n if n==1: open({:?},'w').write('decoded')\n return c\ncore.std.FrameEval(c,mark).set_output()\n",
                    root.join("queued-decoded").to_str().expect("path")
                )
            };
            fs::write(&path, script).expect("script");
            Input::from_data(&InputModel::VapourSynthScript {
                source: VapourSynthScriptSource::Path(path),
                variables: HashMap::new(),
                index: 0,
                filters: vec![],
                stream_concurrently: false,
            })
            .expect("VS input")
        }
    }

    fn encoder(root: &Path, multipass: bool) -> Encoder {
        let mut encoder = Encoder::default_from_base(&EncoderBase::X264, true);
        encoder.set_executable(Some(root.join("fake")));
        encoder.set_append_color_data(false);
        encoder.set_quantizer(23.0);
        if multipass {
            *encoder.passes_mut().expect("passes") = EncoderPasses::All(2);
        }
        encoder
    }

    fn reap(root: &Path) {
        if let Ok(pid) = fs::read_to_string(root.join("grandchild")) {
            let pid: i32 = pid.parse().expect("grandchild PID");
            let mut status = 0;
            // SAFETY: Only the known adopted fixture grandchild is reaped. Its finite
            // emergency lifetime exceeds the supervisor bound; natural exit cannot pass.
            let reaped = unsafe { libc::waitpid(pid, &mut status, 0) };
            assert_eq!(
                reaped, pid,
                "grandchild must be dead, not killed by supervisor"
            );
            assert!(libc::WIFSIGNALED(status));
            assert_eq!(libc::WTERMSIG(status), libc::SIGKILL);
        }
        if let Ok(pid) = fs::read_to_string(root.join("leader")) {
            let pid: i32 = pid.parse().expect("leader PID");
            // SAFETY: Signal zero checks existence only; it cannot kill an unrelated process.
            let exists = unsafe { libc::kill(pid, 0) };
            assert_eq!(exists, -1, "direct child must be reaped");
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ESRCH)
            );
        }
        assert_eq!(
            fs::read(root.join("sentinel")).expect("sentinel"),
            b"caller-owned"
        );
        assert!(
            !fs::read_dir(root).expect("listing").any(|e| e
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .starts_with(".av1an-stats-"))
        );
        assert!(
            !fs::read_dir(root).expect("serial scratch listing").any(|e| e
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .starts_with(".av1an-serial-"))
        );
    }

    fn assert_lifecycle_case() {
        let Some(root) = std::env::var_os("PROCESS_LIFECYCLE_ROOT") else {
            return;
        };
        let root = PathBuf::from(root);
        let case = std::env::var("PROCESS_LIFECYCLE_CASE").expect("case");
        fs::write(root.join("selected"), "yes").expect("handshake");
        fs::write(root.join("sentinel"), "caller-owned").expect("sentinel");
        if case == "encoder-existing" {
            fs::write(root.join("out.264"), "caller-owned").expect("existing output");
        }
        if case == "ffmpeg-existing" {
            fs::write(root.join("out.mkv"), "caller-owned").expect("existing output");
        }
        // SAFETY: This test runs alone in a supervisor-owned process. Adopt only its
        // fixture grandchildren, and ignore terminal-style SIGINT in the test group.
        unsafe {
            assert_eq!(libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0), 0);
            libc::signal(libc::SIGINT, libc::SIG_IGN);
        }
        let cancellation = Cancellation::default();
        if case.starts_with("autoreap-") {
            // SAFETY: Only this isolated test process's SIGCHLD disposition is changed.
            let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
            action.sa_sigaction = if case == "autoreap-ignore" {
                libc::SIG_IGN
            } else {
                libc::SIG_DFL
            };
            action.sa_flags = if case == "autoreap-nocldwait" {
                libc::SA_NOCLDWAIT
            } else {
                0
            };
            // SAFETY: Both structures and the signal set are valid; no sibling test runs here.
            let mut previous: libc::sigaction = unsafe { std::mem::zeroed() };
            // SAFETY: Initialize the mask before installing the local test disposition.
            assert_eq!(unsafe { libc::sigemptyset(&mut action.sa_mask) }, 0);
            assert_eq!(
                // SAFETY: Save and change only this subprocess's SIGCHLD disposition.
                unsafe { libc::sigaction(libc::SIGCHLD, &action, &mut previous) },
                0
            );
            let mut command = Command::new(root.join("fake"));
            command.arg("--complete");
            let result = OwnedProcess::spawn(&mut command, &cancellation);
            assert_eq!(
                // SAFETY: Restore the saved disposition before any guard/error assertion can unwind.
                unsafe { libc::sigaction(libc::SIGCHLD, &previous, std::ptr::null_mut()) },
                0
            );
            let error = result.err().expect("unsupported auto-reap rejected before spawn");
            assert_eq!(
                error.downcast_ref::<io::Error>().expect("typed OS error").kind(),
                io::ErrorKind::Unsupported
            );
            reap(&root);
            return;
        }
        if case.starts_with("lost-") {
            let mut command = Command::new(root.join("fake"));
            command.arg("--complete");
            let mut process =
                OwnedProcess::spawn(&mut command, &cancellation).expect("owned child");
            let pid = i32::try_from(process.id()).expect("PID");
            // SAFETY: Deliberately violate ownership for this one known direct child, in isolation.
            assert_eq!(unsafe { libc::waitpid(pid, std::ptr::null_mut(), 0) }, pid);
            let result = if case == "lost-wait" {
                process.wait().map(|_| ())
            } else {
                process.kill()
            };
            let error = result.expect_err("known lost ownership must be reported, not signalled");
            assert_eq!(
                error.downcast_ref::<io::Error>().expect("typed OS error").raw_os_error(),
                Some(libc::ECHILD)
            );
            assert!(
                !process.is_armed(),
                "ECHILD must disarm Drop and later force signals"
            );
            drop(process);
            cancellation.request().expect("graceful after ownership loss");
            cancellation.request().expect("disarmed force after ownership loss");
            reap(&root);
            return;
        }
        if case == "leader-changes-group" {
            let mut command = Command::new(root.join("fake"));
            command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
            let mut process =
                OwnedProcess::spawn(&mut command, &cancellation).expect("owned child");
            wait_file(&root, "group-changed");
            cancellation.request().expect("graceful");
            cancellation.request().expect("force group and changed-group direct child");
            let status = process.wait().expect("reap direct child promptly");
            use std::os::unix::process::ExitStatusExt;
            assert_eq!(status.signal(), Some(libc::SIGKILL));
            reap(&root);
            return;
        }
        if case.starts_with("stats-") {
            let stats = PassStatistics::new(&root.join("out.264")).expect("owned statistics");
            fs::write(
                root.join("stats-dir"),
                stats.base().parent().expect("parent").to_str().expect("path"),
            )
            .expect("stats handshake");
            let mut command = Command::new(root.join("fake"));
            command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
            let process =
                OwnedProcess::spawn(&mut command, &cancellation).expect("real waiter child");
            wait_file(&root, "grandchild");
            let (reap_tx, waiter) = spawn_waiter(process, Some(stats));
            if case == "stats-unwind" {
                reap_tx.send(()).expect("drained");
            }
            drop(reap_tx);
            let result = waiter.join();
            if case == "stats-unwind" {
                assert!(result.is_err());
            } else {
                assert!(result.expect("waiter error, not panic").is_err());
            }
            assert_eq!(
                fs::read_to_string(root.join("stats-through-reap")).expect("actual reap observed"),
                "true",
                "statistics must outlive child Drop/reap on error and unwind"
            );
            reap(&root);
            return;
        }
        if case == "stream-idle-return" {
            let input = input(&root, true);
            let encoder = encoder(&root, false);
            let mut config = DefaultSequenceConfig::default();
            config.parallel_encoder.scenes_directory = root.clone();
            config.parallel_encoder.workers = Some(1);
            config.parallel_encoder.max_tries = 1; // Preserve the original direct-stream cancellation control.
            config.parallel_encoder.validate_frame_count = false; // Finite ownership markers, not bitstreams.
            let callbacks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let saved = Arc::clone(&callbacks);
            let mut condor = Condor::new(
                input,
                Output {
                    path: root.join("unused"),
                    overwrite: false,
                    tags: HashMap::new(),
                    video_tags: HashMap::new(),
                },
                encoder.clone(),
                vec![Scene {
                    start_frame: 0,
                    end_frame: 1,
                    sub_scenes: None,
                    encoder,
                    sequence_data: DefaultSequenceData::default(),
                }],
                Some(config),
                Box::new(move |_| {
                    saved.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }),
            );
            let (tx, _rx) = mpsc::channel();
            let result = ParallelEncoder::default().execute_cancel(&mut condor, tx, cancellation);
            assert_eq!(
                result
                    .expect_err("force at idle dispatch return is not success")
                    .downcast_ref::<Cancelled>(),
                Some(&Cancelled::Forced)
            );
            assert_eq!(
                callbacks.load(Ordering::SeqCst),
                1,
                "no empty success callback after force"
            );
            assert!(
                root.join("00000.264").exists(),
                "already committed active work survives"
            );
            assert!(condor.scenes[0].sequence_data.parallel_encoder.bytes.is_some());
            reap(&root);
            return;
        }
        if case == "guard-drop" || case == "guard-unwind" || case == "after-completion" {
            let completed = case == "after-completion";
            let cleaned = std::panic::catch_unwind(|| {
                let mut command = Command::new(root.join("fake"));
                command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
                if completed {
                    command.arg("--complete");
                }
                let mut process =
                    OwnedProcess::spawn(&mut command, &cancellation).expect("owned child");
                if completed {
                    assert!(process.wait().expect("normal reap").success());
                    cancellation.request().expect("graceful after reap");
                    cancellation.request().expect("force after reap is disarmed");
                } else {
                    wait_file(&root, "grandchild");
                    assert_ne!(case, "guard-unwind", "deliberate guard unwind");
                }
            });
            assert_eq!(cleaned.is_err(), case == "guard-unwind");
            reap(&root);
            return;
        }
        if case == "ffms-index" {
            let path = root.join("finite.y4m");
            let mut bytes = b"YUV4MPEG2 W64 H64 F24:1 Ip A1:1 C420jpeg\n".to_vec();
            for _ in 0..32 {
                bytes.extend_from_slice(b"FRAME\n");
                bytes.extend(vec![48; 64 * 64 * 3 / 2]);
            }
            fs::write(&path, bytes).expect("native source");
            let cache = root.join("interrupted.ffindex");
            let data = InputModel::Video {
                path,
                import_method: ImportMethod::FFMS2 {
                    index: None,
                    cache_path: Some(cache.clone()),
                },
                filters: vec![],
            };
            let mut observed = false;
            let result = Input::ensure_indexed_cancel(
                &data,
                |progress| {
                    if matches!(progress, crate::core::input::OpenProgress::Indexing { .. })
                        && !observed
                    {
                        observed = true;
                        cancellation.request().expect("graceful native index");
                        cancellation.request().expect("force native index");
                    }
                },
                &cancellation,
            );
            assert!(observed, "real FFMS2 indexing callback ran");
            assert_eq!(
                result.err().expect("native indexing force").downcast_ref::<Cancelled>(),
                Some(&Cancelled::Forced)
            );
            assert!(!cache.exists(), "interrupted new index cannot be reused");
            reap(&root);
            return;
        }
        if case == "pre-spawn" {
            cancellation.request().expect("graceful");
            let (_tx, rx) = crossbeam_channel::bounded(1);
            let (progress, _) = mpsc::channel();
            assert_eq!(
                encoder(&root, true)
                    .encode_with_stream_cancel(
                        rx,
                        &root.join("out.264"),
                        progress,
                        &crate::core::encoder::tests::clip_info(),
                        &cancellation
                    )
                    .expect_err("graceful cannot admit a fresh scene")
                    .downcast_ref::<Cancelled>(),
                Some(&Cancelled::Graceful)
            );
            cancellation.request().expect("forced");
            let (_tx, rx) = crossbeam_channel::bounded(1);
            let (progress, _) = mpsc::channel();
            assert!(
                encoder(&root, true)
                    .encode_with_stream_cancel(
                        rx,
                        &root.join("out.264"),
                        progress,
                        &crate::core::encoder::tests::clip_info(),
                        &cancellation
                    )
                    .expect_err("pre-spawn force")
                    .is::<Cancelled>()
            );
            assert!(!root.join("leader").exists());
            reap(&root);
            return;
        }
        // All PATH mutation precedes controller/relay threads in this isolated process.
        for name in ["ffmpeg", "mkvmerge"] {
            std::os::unix::fs::symlink(root.join("fake"), root.join(name))
                .expect("fixture launcher");
        }
        let path = std::env::join_paths(std::iter::once(root.clone()).chain(
            std::env::split_paths(&std::env::var_os("PATH").expect("PATH")),
        ))
        .expect("PATH");
        // SAFETY: No other test runs in this process, and no threads have been started.
        unsafe {
            std::env::set_var("PATH", path);
        }
        if case.starts_with("commit-") {
            let output = root.join("committed");
            let (progress, _rx) = mpsc::channel();
            let result: Result<()> =
                if case.starts_with("commit-ffmpeg") || case.starts_with("commit-mkvmerge") {
                    fs::create_dir_all(root.join("Scene Concatenator")).expect("scratch");
                    let scenes = vec![root.join("scene.264")];
                    fs::write(&scenes[0], b"owned scene fixture").expect("nonempty mux input");
                    if case.starts_with("commit-ffmpeg") {
                        SceneConcatenator::ffmpeg_cancel(
                            &root,
                            &output,
                            &scenes,
                            None,
                            1,
                            av_format::rational::Ratio::new(24, 1),
                            None,
                            &progress,
                            &cancellation,
                        )
                    } else {
                        SceneConcatenator::mkvmerge_cancel(
                            &root,
                            &output,
                            &scenes,
                            None,
                            av_format::rational::Ratio::new(24, 1),
                            None,
                            &progress,
                            &cancellation,
                        )
                    }
                } else if case.starts_with("commit-ivf") {
                    let source = root.join("source.ivf");
                    let mut bytes =
                        b"DKIF\0\0\x20\0AV01\x08\0\x08\0\x18\0\0\0\x01\0\0\0\x01\0\0\0\0\0\0\0"
                            .to_vec();
                    bytes.extend_from_slice(&1_u32.to_le_bytes());
                    bytes.extend_from_slice(&0_u64.to_le_bytes());
                    bytes.push(0x12);
                    fs::write(&source, bytes).expect("finite IVF");
                    SceneConcatenator::ivf_cancel(&output, &[source], &progress, &cancellation)
                } else if case.starts_with("commit-dg") {
                    fs::write(root.join("source"), "fixture source").expect("source");
                    crate::vapoursynth::plugins::dgdecodenv::DGSource::index_video_cancel(
                        &root.join("source"),
                        Some(&output),
                        Some(&root.join("fake")),
                        None,
                        &cancellation,
                    )
                    .map(|_| ())
                } else if case.starts_with("commit-ffms") {
                    let _input = input(&root, true);
                    let data = InputModel::Video {
                        path: root.join("input.y4m"),
                        import_method: ImportMethod::FFMS2 {
                            index: None,
                            cache_path: Some(output.clone()),
                        },
                        filters: vec![],
                    };
                    Input::ensure_indexed_cancel(&data, |_| {}, &cancellation).map(|_| ())
                } else {
                    let encoder = encoder(&root, case.starts_with("commit-all2"));
                    let (progress, _rx) = mpsc::channel::<EncodeProgress>();
                    if case.starts_with("commit-input-") || case.starts_with("commit-pass-input-") {
                        let mut input = input(&root, false);
                        if case.starts_with("commit-pass-input") {
                            encoder.encode_pass_with_input_cancel(
                                &mut input,
                                &output,
                                0,
                                1,
                                (1, 1),
                                progress,
                                None,
                                &cancellation,
                            )
                        } else {
                            encoder.encode_with_input_cancel(
                                &mut input,
                                &output,
                                0,
                                1,
                                progress,
                                &cancellation,
                            )
                        }
                    } else {
                        let (tx, rx) = crossbeam_channel::bounded(1);
                        tx.send(Cursor::new(vec![0; 64])).expect("finite stream");
                        drop(tx);
                        if case.starts_with("commit-pass-stream") {
                            encoder.encode_pass_with_stream_cancel(
                                (1, 1),
                                rx,
                                &output,
                                progress,
                                &crate::core::encoder::tests::clip_info(),
                                None,
                                &cancellation,
                            )
                        } else {
                            encoder.encode_with_stream_cancel(
                                rx,
                                &output,
                                progress,
                                &crate::core::encoder::tests::clip_info(),
                                &cancellation,
                            )
                        }
                    }
                    .map(|_| ())
                };
            assert!(
                cancellation.is_forced(),
                "actual completion/Drop seam selected"
            );
            if case.ends_with("-post") {
                if case.starts_with("commit-ffms") {
                    assert_eq!(
                        result
                            .expect_err("opening after index commit sees force")
                            .downcast_ref::<Cancelled>(),
                        Some(&Cancelled::Forced)
                    );
                } else {
                    result.expect("force after successful unit commit cannot undo success");
                }
                assert!(
                    output.exists(),
                    "successfully committed artifact survives a later request"
                );
            } else {
                assert_eq!(
                    result
                        .expect_err("force before unit commit is cancellation")
                        .downcast_ref::<Cancelled>(),
                    Some(&Cancelled::Forced)
                );
                assert!(!output.exists(), "incomplete owned artifact removed");
            }
            if case.ends_with("-between") {
                assert_eq!(
                    fs::read_to_string(root.join("passes")).expect("passes"),
                    "1\n"
                );
            }
            reap(&root);
            return;
        }
        let root2 = root.clone();
        let signal = cancellation.clone();
        let kind = case.clone();
        let controller = thread::spawn(move || {
            if kind == "capture" {
                while !fs::read_dir(&root2).expect("listing").any(|e| {
                    e.expect("entry").file_name().to_string_lossy().starts_with(".av1an-stats-")
                }) {
                    thread::yield_now();
                }
            } else if kind.starts_with("graceful") {
                wait_file(&root2, "active");
            } else {
                wait_file(&root2, "grandchild");
            }
            if kind == "stdin-full" {
                wait_file(&root2, "queued");
            }
            if kind == "mid-production" {
                wait_file(&root2, "frame-k");
            }
            if kind == "filter-full" {
                wait_file(&root2, "full-output");
            }
            if [
                "force-vs",
                "mid-production",
                "force-native",
                "stdin-full",
                "stream-idle",
                "replay-full",
                "serial",
                "serial-native",
                "zone",
            ]
            .contains(&kind.as_str())
            {
                let pid: u32 = fs::read_to_string(root2.join("leader"))
                    .expect("correct leader PID")
                    .parse()
                    .expect("PID");
                let registered = format!("registered-{pid}");
                if ["mid-production", "stream-idle", "serial", "serial-native"]
                    .contains(&kind.as_str())
                {
                    wait_file(&root2, &format!("registration-blocked-{pid}"));
                    assert!(
                        !root2.join(&registered).exists(),
                        "child readiness precedes registration"
                    );
                    assert!(
                        !signal.is_forced(),
                        "readiness alone must not force diagnostic stage"
                    );
                    fs::write(
                        root2.join(format!("registration-release-{pid}")),
                        "register now",
                    )
                    .expect("release registration barrier");
                }
                wait_file(&root2, &registered);
            }
            if kind == "leader-exits" {
                let pid: u32 =
                    fs::read_to_string(root2.join("leader")).expect("PID").parse().expect("PID");
                loop {
                    // SAFETY: Observe only our still-owned direct child, without reaping it.
                    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
                    // SAFETY: Observe the known direct child into a valid siginfo, without reaping.
                    let observed = unsafe {
                        libc::waitid(
                            libc::P_PID,
                            pid,
                            &mut info,
                            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                        )
                    };
                    assert_eq!(observed, 0);
                    // SAFETY: Successful waitid initializes the SIGCHLD union member.
                    if unsafe { info.si_pid() } != 0 {
                        break;
                    }
                    thread::yield_now();
                }
            }
            signal.request().expect("graceful");
            if kind.starts_with("graceful") {
                if kind == "graceful-sigint" {
                    // SAFETY: The supervisor gave this test its own group; managed children lead other groups.
                    assert_eq!(unsafe { libc::kill(0, libc::SIGINT) }, 0);
                }
                fs::write(root2.join("release"), "finish active scene").expect("release");
            } else {
                signal.request().expect("force");
                if kind == "mid-production" {
                    fs::write(root2.join("release-frame"), "forced")
                        .expect("release finite callback");
                }
            }
        });
        let mut filter_spawn_failed = false;
        let result: Result<()> = if case.starts_with("filter") {
            let (_tx, rx) = crossbeam_channel::bounded(1);
            match FfmpegFilter::spawn_cancel(
                rx,
                1,
                "null",
                Arc::new(std::sync::Mutex::new(None)),
                &cancellation,
            ) {
                Err(error) => {
                    // Force can win registration; still join the controller and reap below.
                    filter_spawn_failed = true;
                    Err(error)
                },
                Ok(mut stage) => {
                    let output = stage.take_receiver().expect("receiver");
                    if case == "filter-full" {
                        while output.is_empty() {
                            thread::yield_now();
                        }
                        fs::write(root.join("full-output"), "bounded output full")
                            .expect("full handshake");
                        assert!(cancellation.forced().recv().is_err());
                    } else {
                        let _ = output.recv();
                    }
                    drop(output);
                    stage.check()
                },
            }
        } else if case == "dg" {
            fs::write(root.join("source"), "finite fixture source").expect("source");
            crate::vapoursynth::plugins::dgdecodenv::DGSource::index_video_cancel(
                &root.join("source"),
                Some(&root.join("out.dgi")),
                Some(&root.join("fake")),
                None,
                &cancellation,
            )
            .map(|_| ())
        } else if case.starts_with("ffmpeg") || case.starts_with("mkv") {
            fs::create_dir_all(root.join("Scene Concatenator")).expect("scratch");
            let scenes = vec![root.join("scene.264"); if case == "mkv-group" { 513 } else { 1 }];
            fs::write(&scenes[0], b"owned scene fixture").expect("nonempty mux input");
            let (tx, _rx) = mpsc::channel();
            if case.starts_with("ffmpeg") {
                SceneConcatenator::ffmpeg_cancel(
                    &root,
                    &root.join("out.mkv"),
                    &scenes,
                    None,
                    1,
                    av_format::rational::Ratio::new(24, 1),
                    None,
                    &tx,
                    &cancellation,
                )
            } else {
                SceneConcatenator::mkvmerge_cancel(
                    &root,
                    &root.join("out.mkv"),
                    &scenes,
                    None,
                    av_format::rational::Ratio::new(24, 1),
                    None,
                    &tx,
                    &cancellation,
                )
            }
        } else if ["capture", "stream-idle", "encoder-existing", "stdin-full", "replay-full"]
            .contains(&case.as_str())
        {
            let (tx, rx) = crossbeam_channel::bounded(1);
            let full = case == "stdin-full" || case == "replay-full";
            let _keep_alive = (!full).then(|| tx.clone());
            let producer = full.then(|| {
                let replay = case == "replay-full";
                let root = root.clone();
                thread::spawn(move || {
                    tx.send(Cursor::new(vec![1; 16 * 1024 * 1024])).expect("first chunk");
                    if !replay {
                        tx.send(Cursor::new(vec![2; 1024])).expect("queued chunk");
                        fs::write(root.join("queued"), "yes").expect("queue handshake");
                        assert!(tx.send(Cursor::new(vec![3; 1024])).is_err());
                    }
                })
            });
            let (progress, _rx) = mpsc::channel();
            let result = encoder(&root, case == "capture" || case == "replay-full")
                .encode_with_stream_cancel(
                    rx,
                    &root.join("out.264"),
                    progress,
                    &crate::core::encoder::tests::clip_info(),
                    &cancellation,
                )
                .map(|_| ());
            if let Some(producer) = producer {
                producer.join().expect("bounded producer joined");
            }
            result
        } else {
            let mut input = input(&root, case.ends_with("native"));
            let encoder = encoder(&root, case.starts_with("graceful"));
            let scenes = (0..2)
                .map(|i| Scene {
                    start_frame: if case == "mid-production" { i * 8 } else { i },
                    end_frame: if case == "mid-production" && i == 0 {
                        8
                    } else if case == "mid-production" {
                        9
                    } else {
                        i + 1
                    },
                    sub_scenes: None,
                    encoder: encoder.clone(),
                    sequence_data: DefaultSequenceData::default(),
                })
                .collect();
            let mut config = DefaultSequenceConfig::default();
            config.parallel_encoder.scenes_directory = root.clone();
            config.parallel_encoder.workers = Some(1);
            config.parallel_encoder.max_tries = 1; // Mid-production handshakes require direct streaming, not capture.
            config.parallel_encoder.validate_frame_count = false; // Finite ownership markers, not bitstreams.
            let saved = root.join("saved.json");
            let (progress, _rx) = mpsc::channel();
            if case == "zone" || case == "zone-probe" {
                let tasks: Vec<_> = (0..2)
                    .map(|i| Task {
                        original_index: i,
                        index: i,
                        frame_indices: vec![i],
                        sub_scenes: None,
                        encoder: encoder.clone(),
                        output: root.join(format!("{i}.264")),
                    })
                    .collect();
                let plan = ZonePlan::try_build_cancel(&tasks, &cancellation);
                plan.and_then(|plan| {
                    ZoneEncoder::encode_tasks_cancel(
                        &mut input,
                        tasks.into(),
                        plan,
                        progress,
                        cancellation,
                    )
                })
                .map(|_| ())
            } else {
                let mut condor = Condor::new(
                    input,
                    Output {
                        path: root.join("unused"),
                        overwrite: false,
                        tags: HashMap::new(),
                        video_tags: HashMap::new(),
                    },
                    encoder,
                    scenes,
                    Some(config),
                    Box::new(move |data| {
                        fs::write(&saved, serde_json::to_vec(&data)?)?;
                        Ok(())
                    }),
                );
                let result = if case.starts_with("serial") {
                    SerialEncoder::new(&root).execute_cancel(&mut condor, progress, cancellation)
                } else {
                    ParallelEncoder::default().execute_cancel(&mut condor, progress, cancellation)
                };
                if case.starts_with("graceful") {
                    assert_eq!(
                        fs::read_to_string(root.join("passes")).expect("passes"),
                        "1\n2\n"
                    );
                    assert!(root.join("00000.264").exists());
                    assert!(condor.scenes[0].sequence_data.parallel_encoder.bytes.is_some());
                    assert!(condor.scenes[1].sequence_data.parallel_encoder.bytes.is_none());
                    assert!(root.join("saved.json").exists());
                } else {
                    assert!(
                        condor.scenes.iter().all(|scene| scene
                            .sequence_data
                            .parallel_encoder
                            .bytes
                            .is_none())
                    );
                    assert!(
                        !root.join("saved.json").exists(),
                        "no success callback for forced scenes"
                    );
                }
                result.map(|_| ())
            }
        };
        controller.join().expect("controller joined");
        if case.starts_with("graceful") {
            result.expect("active All2 completes");
            assert!(!root.join("queued-decoded").exists());
        } else {
            let error = result.expect_err("force is not success");
            assert_eq!(error.downcast_ref::<Cancelled>(), Some(&Cancelled::Forced));
            if [
                "force-vs",
                "mid-production",
                "force-native",
                "stdin-full",
                "stream-idle",
                "replay-full",
                "serial",
                "serial-native",
                "zone",
            ]
            .contains(&case.as_str())
            {
                assert!(
                    error.chain().any(|cause| cause.is::<EncoderResult>()),
                    "forced encoder diagnostics/status must remain typed: {error:#}"
                );
            }
            if ["mid-production", "stdin-full", "serial", "serial-native"].contains(&case.as_str())
            {
                let diagnostic = error
                    .chain()
                    .find_map(|cause| cause.downcast_ref::<EncoderResult>())
                    .expect("mid-production typed encoder diagnostic");
                let bytes = |value: &StringOrBytes| match value {
                    StringOrBytes::String(value) => value.as_bytes().to_vec(),
                    StringOrBytes::Bytes(value) => value.clone(),
                };
                assert_eq!(
                    bytes(&diagnostic.stdout),
                    b"process lifecycle mid stdout\xff\n"
                );
                assert_eq!(
                    bytes(&diagnostic.stderr),
                    b"process lifecycle mid stderr\xfe\n"
                );
                if case == "mid-production" || case == "stdin-full" {
                    assert!(error.chain().any(|cause| cause.is::<EncoderProcessError>()));
                }
            }
            assert!(!root.join("00000.264").exists());
            if case == "encoder-existing" {
                assert_eq!(
                    fs::read(root.join("out.264")).expect("existing output"),
                    b"caller-owned"
                );
            } else {
                assert!(!root.join("out.264").exists());
            }
            assert!(!root.join("out.dgi").exists());
            if case == "ffmpeg-existing" {
                assert_eq!(
                    fs::read(root.join("out.mkv")).expect("existing output"),
                    b"caller-owned"
                );
            } else {
                assert!(!root.join("out.mkv").exists());
            }
        }
        assert!(!root.join("00001.264").exists());
        assert!(!root.join("zone-pass.temp.264").exists());
        assert!(!root.join("Scene Concatenator/00000.mkv").exists());
        reap(&root);
        if case == "filter-spawn-race" {
            assert!(
                filter_spawn_failed,
                "force won construction, not stage.check()"
            );
        }
    }
}
