//! Every case runs the public owner or its precise owning seam in an isolated process.
pub(crate) use internal_integration::{
    after_process_registration, before_output_commit, before_process_registration,
    before_stream_return, observe_reap, output_drop,
};

use super::*;
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
use crate::vapoursynth::plugins::dgdecodenv::DGSource;
use std::{fs, io};

#[path = "../../../../tests/core/encoder/fixtures/lifecycle_support.rs"]
mod fixture;

mod internal_integration {
    use super::fixture::{encoder, input, reap, wait_file};
    use super::*;

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
        registration_races_reap_the_child_before_spawn_returns,
        ["registration", "filter-spawn-race"]
    );
    lifecycle_cases!(
        force_at_stream_dispatch_return_preserves_completed_output,
        ["stream-idle-return"]
    );
    lifecycle_cases!(
        lost_process_ownership_disarms_later_signals,
        ["lost-wait", "lost-kill"]
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

    fn assert_lifecycle_case() {
        let (root, case, cancellation) = fixture::prepare_case();
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
        fixture::install_launchers(&root);
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
        fixture::assert_scene_case(&root, &case, cancellation);
    }
}
