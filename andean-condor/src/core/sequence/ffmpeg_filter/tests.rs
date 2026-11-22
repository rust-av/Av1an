#[cfg(target_os = "linux")]
use super::*;
#[cfg(target_os = "linux")]
use crate::{
    core::input::Input,
    models::input::{ImportMethod, Input as InputModel},
};
#[cfg(target_os = "linux")]
#[path = "../../../../tests/core/sequence/fixtures/filter_lifecycle.rs"]
mod lifecycle_fixture;

#[cfg(target_os = "linux")]
#[test]
fn thread_census_waits_for_transition() {
    let mut observations = [3, 3, 2].into_iter();
    assert_eq!(
        internal_integration::wait_for_thread_count(2, std::time::Duration::from_secs(5), || {
            observations.next().expect("census")
        })
        .expect("observe teardown after logical join"),
        2
    );
    assert!(
        observations.next().is_none(),
        "must observe the transition, not skip the census"
    );
}

#[cfg(target_os = "linux")]
mod internal_integration {
    //! Removing sole receiver ownership or encoder-side filter cleanup must hang
    //! these bounded fixtures; the external watchdog reaps even a failing baseline.
    use super::*;
    use crate::{
        core::{
            encoder::EncoderResult,
            sequence::{
                SequenceStatus,
                parallel_encoder::Task,
                zone_encoder::{ZoneEncoder, ZonePlan},
            },
        },
        models::encoder::{Encoder, EncoderBase, cli_parameter::CLIParameter},
    };
    use std::{
        fs,
        path::Path,
        time::{Duration, Instant},
    };

    fn thread_count() -> usize {
        fs::read_dir("/proc/self/task").expect("threads").count()
    }

    // Linux can wake a native join before unlinking the exiting task from /proc.
    // This observation barrier runs after owned joins; it cannot replace them.
    pub(super) fn wait_for_thread_count(
        before: usize,
        timeout: Duration,
        mut count: impl FnMut() -> usize,
    ) -> Result<usize, String> {
        let deadline = Instant::now() + timeout;
        loop {
            let after = count();
            if after == before {
                return Ok(after);
            }
            if Instant::now() >= deadline {
                let tasks: Vec<_> = fs::read_dir("/proc/self/task")
                    .expect("threads")
                    .map(|entry| {
                        let entry = entry.expect("task");
                        (
                            entry.file_name(),
                            fs::read_to_string(entry.path().join("stat")),
                        )
                    })
                    .collect();
                return Err(format!(
                    "thread teardown deadline: before={before}, after={after}; TIDs/stat={tasks:?}"
                ));
            }
            thread::yield_now();
        }
    }

    #[test]
    fn dropping_filter_before_input_disconnects_upstream_and_joins_relays() {
        isolated_cases(
            "dropping_filter_before_input_disconnects_upstream_and_joins_relays",
            &["first-drop"],
        );
    }

    #[test]
    fn checking_filter_before_input_is_idempotent_and_joins_relays() {
        isolated_cases(
            "checking_filter_before_input_is_idempotent_and_joins_relays",
            &["first-check"],
        );
    }

    #[test]
    fn dropping_filter_with_blocked_output_unblocks_producer() {
        isolated_cases(
            "dropping_filter_with_blocked_output_unblocks_producer",
            &["blocked-output"],
        );
    }

    #[test]
    fn checking_filter_after_pipe_disconnect_treats_sigpipe_as_intentional_abort() {
        isolated_cases(
            "checking_filter_after_pipe_disconnect_treats_sigpipe_as_intentional_abort",
            &["late-check"],
        );
    }

    #[test]
    fn completed_filters_preserve_y4m_format_and_reap_workers() {
        isolated_cases(
            "completed_filters_preserve_y4m_format_and_reap_workers",
            &["controls"],
        );
    }

    #[test]
    fn filter_check_reports_all_relay_faults_with_bounded_stderr() {
        isolated_cases(
            "filter_check_reports_all_relay_faults_with_bounded_stderr",
            &["relay-faults"],
        );
    }

    #[test]
    fn zone_filter_failures_preserve_primary_diagnostics_and_clear_outputs() {
        isolated_cases(
            "zone_filter_failures_preserve_primary_diagnostics_and_clear_outputs",
            &["zone-invalid", "zone-filter"],
        );
    }

    fn isolated_cases(test: &str, cases: &[&str]) {
        lifecycle_fixture::isolated(module_path!(), test, cases, |root, case| match case {
            "controls" => controls(),
            "relay-faults" => relay_faults(),
            c if c.starts_with("zone") => zone_failure(root, c),
            c => direct_drop(c),
        });
    }

    fn direct_drop(case: &str) {
        let before = thread_count();
        let (tx, rx) = crossbeam_channel::bounded(1);
        let mut stage = FfmpegFilter::spawn(
            rx,
            1,
            "null",
            Arc::new(Mutex::new(None)),
            &crate::utils::cancellation::Cancellation::default(),
        )
        .expect("filter");
        let output = stage.take_receiver().expect("sole receiver");
        assert!(stage.take_receiver().is_err(), "handoff happens only once");
        if case.starts_with("first") {
            // No input/header/output has arrived; the upstream sender remains alive.
            drop(output);
            if case == "first-check" {
                stage.check().expect("intentional abort is not a filter failure");
                stage.check().expect("completion is idempotent");
                assert!(stage.child.try_wait().expect("child status").is_some());
                assert!(
                    stage.input_thread.is_none()
                        && stage.output_thread.is_none()
                        && stage.stderr_thread.is_none()
                );
            }
            drop(stage);
            assert!(
                tx.send(Cursor::new(vec![])).is_err(),
                "Drop must disconnect idle upstream"
            );
        } else {
            let producer = thread::spawn(move || {
                tx.send(Cursor::new(
                    b"YUV4MPEG2 W128 H128 F24:1 Ip A1:1 C420jpeg\n".to_vec(),
                ))
                .expect("header");
                let mut frame = b"FRAME\n".to_vec();
                frame.extend(vec![48; 128 * 128 * 3 / 2]);
                for _ in 0..4096 {
                    if tx.send(Cursor::new(frame.clone())).is_err() {
                        return;
                    }
                }
                panic!("consumer drop should disconnect a bounded producer");
            });
            output.recv().expect("real first output handshake");
            drop(output);
            if case == "late-check" {
                // Model a descheduled consumer: output disconnect closes the pipe
                // and FFmpeg fails before the owning thread gets to cleanup.
                while !stage.output_thread.as_ref().expect("relay").is_finished() {
                    thread::yield_now();
                }
                assert!(!stage.child.wait().expect("FFmpeg exit after pipe disconnect").success());
                stage.check().expect("expected SIGPIPE must not replace encoder diagnostics");
            }
            drop(stage);
            producer.join().expect("upstream unblocked and joined");
        }
        let after = wait_for_thread_count(before, Duration::from_secs(5), thread_count)
            .expect("all filter relay workers joined before Drop returns");
        eprintln!(
            "filter-lifecycle direct {case}: relay threads before={before}, after={after}; producer disconnected"
        );
    }

    fn controls() {
        let before = thread_count();
        for (graph, dimensions, chroma) in [
            ("null", "W8 H8", "C420"),
            ("crop=6:6", "W6 H6", "C420"),
            ("format=yuv420p10le", "W8 H8", "C420p10"),
        ] {
            let (tx, rx) = crossbeam_channel::bounded(1);
            let mut stream = b"YUV4MPEG2 W8 H8 F24:1 Ip A1:1 C420jpeg\n".to_vec();
            for _ in 0..12 {
                stream.extend_from_slice(b"FRAME\n");
                stream.extend_from_slice(&[48; 96]);
            }
            tx.send(Cursor::new(stream)).expect("input");
            drop(tx);
            let mut stage = FfmpegFilter::spawn(
                rx,
                1,
                graph,
                Arc::new(Mutex::new(None)),
                &crate::utils::cancellation::Cancellation::default(),
            )
            .expect("real FFmpeg");
            let pid = stage.child.id();
            let mut bytes = Vec::new();
            for chunk in stage.take_receiver().expect("sole receiver") {
                bytes.extend(chunk.into_inner());
            }
            stage.check().expect("natural filter completion");
            let header_end = bytes.iter().position(|&b| b == b'\n').expect("Y4M header");
            let header = std::str::from_utf8(&bytes[..header_end]).expect("ASCII header");
            assert!(
                header.starts_with("YUV4MPEG2 ")
                    && header.contains(dimensions)
                    && header.contains(chroma)
                    && header.contains("F24:1"),
                "{header}"
            );
            assert_eq!(bytes.windows(6).filter(|b| *b == b"FRAME\n").count(), 12);
            assert!(
                stage.input_thread.is_none()
                    && stage.output_thread.is_none()
                    && stage.stderr_thread.is_none()
            );
            assert!(
                !Path::new(&format!("/proc/{pid}")).exists(),
                "owned FFmpeg reaped"
            );
        }
        wait_for_thread_count(before, Duration::from_secs(5), thread_count)
            .expect("all naturally completed filter relays joined");
        eprintln!(
            "filter-lifecycle real FFmpeg null/crop/10-bit: 12 frames each, regenerated dimensions/chroma/fps; 3 relays joined per stage"
        );

        // Keep a real extra worker alive until the census has rejected it.
        let (release, wait) = crossbeam_channel::bounded::<()>(0);
        let (ready, started) = crossbeam_channel::bounded(1);
        let worker = thread::spawn(move || {
            ready
                .send(fs::read_link("/proc/thread-self").expect("native worker TID"))
                .expect("ready");
            let _ = wait.recv_timeout(Duration::from_secs(10));
        });
        let tid = started.recv_timeout(Duration::from_secs(5)).expect("live worker ready");
        let rejected = wait_for_thread_count(before, Duration::from_millis(25), thread_count);
        let still_live = !worker.is_finished();
        drop(release);
        worker.join().expect("negative-control worker released and joined");
        assert!(
            still_live,
            "negative control must stay LIVE through the census deadline"
        );
        let diagnostic =
            rejected.expect_err("a persistent LIVE worker must fail the bounded census");
        assert!(
            diagnostic.contains(tid.file_name().expect("TID").to_string_lossy().as_ref()),
            "{diagnostic}"
        );
        wait_for_thread_count(before, Duration::from_secs(5), thread_count)
            .expect("negative-control cleanup");
        eprintln!("thread census LIVE-worker negative control rejected and released: {diagnostic}");
    }

    fn relay_faults() {
        struct FaultyStderr {
            bytes: Cursor<Vec<u8>>,
            interrupted: bool,
        }
        impl Read for FaultyStderr {
            fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
                if !self.interrupted {
                    self.interrupted = true;
                    return Err(io::ErrorKind::Interrupted.into());
                }
                match self.bytes.read(out)? {
                    0 => Err(io::Error::other("filter-lifecycle stderr read fault")),
                    read => Ok(read),
                }
            }
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        let mut bytes = b"YUV4MPEG2 W8 H8 F24:1 Ip A1:1 C420jpeg\nFRAME\n".to_vec();
        bytes.extend_from_slice(&[48; 96]);
        tx.send(Cursor::new(bytes)).expect("input");
        drop(tx);
        let mut stage = FfmpegFilter::spawn(
            rx,
            1,
            "null",
            Arc::new(Mutex::new(None)),
            &crate::utils::cancellation::Cancellation::default(),
        )
        .expect("filter");
        for _ in stage.take_receiver().expect("receiver") {}
        // The real relays complete first. Inject only otherwise hard-to-trigger
        // pipe/join faults, not FFmpeg behavior or native acceptance results.
        stage
            .input_thread
            .take()
            .expect("input")
            .join()
            .expect("join")
            .expect("input completed");
        stage
            .output_thread
            .take()
            .expect("output")
            .join()
            .expect("join")
            .expect("output completed");
        stage
            .stderr_thread
            .take()
            .expect("stderr")
            .join()
            .expect("join")
            .expect("stderr completed");
        stage.input_thread = Some(thread::spawn(|| {
            Err(io::Error::other("filter-lifecycle input fault"))
        }));
        stage.output_thread = Some(thread::spawn(|| panic!("filter-lifecycle output panic")));
        let mut bytes = b"filter-lifecycle discarded prefix".to_vec();
        bytes.extend(vec![b'x'; 2 * STDERR_TAIL_LIMIT]);
        bytes.extend_from_slice(b"filter-lifecycle stderr tail");
        stage.stderr_thread = Some(spawn_stderr_drain(FaultyStderr {
            bytes: Cursor::new(bytes),
            interrupted: false,
        }));
        let error = stage.check().expect_err("all relay faults must be returned").to_string();
        assert!(
            error.contains("filter-lifecycle input fault")
                && error.contains("output relay panicked")
                && error.contains("filter-lifecycle stderr read fault"),
            "{error}"
        );
        assert!(
            error.contains("filter-lifecycle stderr tail"),
            "partial diagnostic tail must survive read failure: {error}"
        );
        assert!(
            !error.contains("filter-lifecycle discarded prefix"),
            "stderr retention stays bounded"
        );
        assert!(
            error.len() < STDERR_TAIL_LIMIT + 1024,
            "stderr tail stays bounded on error"
        );
        assert!(
            stage.input_thread.is_none()
                && stage.output_thread.is_none()
                && stage.stderr_thread.is_none()
        );
    }

    fn zone_failure(root: &Path, case: &str) {
        let mut input = lifecycle_fixture::native_input(root);
        let mut encoder = Encoder::default_from_base(&EncoderBase::X264, true);
        encoder.set_append_color_data(false);
        encoder.set_quantizer(23.0);
        encoder.set_ffmpeg_filter(Some(
            if case.ends_with("filter") {
                "lifecycle_missing_filter"
            } else {
                "null"
            }
            .to_owned(),
        ));
        if case.ends_with("invalid") {
            encoder.parameters_mut().insert(
                "filter-lifecycle-invalid-option".into(),
                CLIParameter::new_bool("--", true),
            );
        }
        let scenes_dir = root.join("scenes");
        fs::create_dir_all(&scenes_dir).expect("scenes");
        let (progress_tx, progress_rx) = std::sync::mpsc::channel();
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let tasks = vec![Task {
            original_index: 7,
            index: 0,
            frame_indices: (0..96).collect(),
            sub_scenes: None,
            encoder,
            output: scenes_dir.join("00007.264"),
        }];
        let plan =
            ZonePlan::try_build(&tasks, &crate::utils::cancellation::Cancellation::default())
                .expect("native x264 zoning capability");
        let error = ZoneEncoder::encode_tasks(
            &mut input,
            tasks.into(),
            plan,
            progress_tx,
            cancelled.into(),
            true,
        )
        .expect_err("zone filter/encoder failure");
        let diagnostic = format!("{error:#}");
        match case {
            "zone-filter" => assert!(
                diagnostic.contains("lifecycle_missing_filter"),
                "original FFmpeg stderr: {diagnostic}"
            ),
            "zone-invalid" => {
                let result = error.chain().find_map(|e| e.downcast_ref::<EncoderResult>()).or_else(|| {
                match error.downcast_ref::<crate::core::sequence::parallel_encoder::ParallelEncoderError>() {
                    Some(crate::core::sequence::parallel_encoder::ParallelEncoderError::EncoderFailed { result, .. }) => Some(result),
                    _ => None,
                }
            }).expect("typed original encoder result");
                assert!(!result.status.success());
                assert!(
                    result.stderr.to_string().contains("filter-lifecycle-invalid-option"),
                    "{}",
                    result.stderr
                );
                assert!(
                    !diagnostic.contains("FFmpeg filter failed"),
                    "intentional abort must not mask encoder: {diagnostic}"
                );
            },
            _ => unreachable!(),
        }
        assert_eq!(
            fs::read_dir(&scenes_dir).expect("outputs").count(),
            0,
            "no failed output or statistics survive"
        );
        let events: Vec<_> = progress_rx.try_iter().collect();
        assert!(
            matches!(
                progress_rx.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Disconnected)
            ),
            "all progress relays joined"
        );
        assert!(!events.iter().any(|e| matches!(e, SequenceStatus::Whole(crate::core::sequence::Status::Processing { completion: crate::core::sequence::SequenceCompletion::Custom { name, .. }, .. }) if name == "size")));
        eprintln!(
            "filter-lifecycle {case}: 96 frames/window 8; original diagnostic retained: {diagnostic}"
        );
    }
}
