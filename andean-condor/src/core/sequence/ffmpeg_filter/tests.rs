use super::*;

/// Minimal Y4M stream: header plus `frames` solid-black 8-bit 4:2:0
/// frames at 8x8.
fn y4m_stream(frames: usize) -> Vec<u8> {
    const HEADER: &[u8] = b"YUV4MPEG2 W8 H8 F1:1 Ip A1:1 C420mpeg2\n";
    const FRAME_HEADER: &[u8] = b"FRAME\n";
    let mut stream = HEADER.to_vec();
    for _ in 0..frames {
        stream.extend_from_slice(FRAME_HEADER);
        stream.extend_from_slice(&[16_u8; 8 * 8]); // Y
        stream.extend_from_slice(&[128_u8; 4 * 4]); // U
        stream.extend_from_slice(&[128_u8; 4 * 4]); // V
    }
    stream
}

/// Feeds `stream` through `graph` and returns all output bytes plus the
/// stage's `check()` result.
fn run_filter(graph: &str, stream: Vec<u8>) -> (Vec<u8>, Result<()>) {
    let (input_tx, input_rx) = crossbeam_channel::bounded(2);
    let failure = Arc::new(Mutex::new(None));
    let mut stage =
        FfmpegFilter::spawn(input_rx, 2, graph, failure).expect("FfmpegFilter should spawn");
    let output_rx = stage.take_receiver().expect("sole receiver");

    let producer = thread::spawn(move || {
        // Feed one chunk per frame to mirror window-streamed producers.
        let split = stream
            .windows(6)
            .position(|window| window == b"FRAME\n")
            .expect("stream should have frames");
        input_tx.send(Cursor::new(stream[..split].to_vec())).ok();
        let mut rest = &stream[split..];
        while !rest.is_empty() {
            let end = rest[6..]
                .windows(6)
                .position(|window| window == b"FRAME\n")
                .map_or(rest.len(), |offset| offset + 6);
            input_tx.send(Cursor::new(rest[..end].to_vec())).ok();
            rest = &rest[end..];
        }
    });

    let mut output = Vec::new();
    for chunk in output_rx {
        output.extend_from_slice(chunk.get_ref());
    }
    producer.join().expect("producer thread should join");
    (output, stage.check())
}

#[test]
fn passes_frames_through_unchanged() {
    if which::which("ffmpeg").is_err() {
        eprintln!("skipping passes_frames_through_unchanged: ffmpeg not in PATH");
        return;
    }
    let frames = 8;
    let (output, result) = run_filter("null", y4m_stream(frames));
    result.expect("the null filter should not fail");
    let output = String::from_utf8_lossy(&output);
    assert!(
        output.starts_with("YUV4MPEG2"),
        "output starts with a Y4M header"
    );
    let frame_count = output.matches("FRAME\n").count();
    assert_eq!(frame_count, frames, "passthrough preserves the frame count");
}

#[test]
fn crop_filter_updates_dimensions() {
    if which::which("ffmpeg").is_err() {
        eprintln!("skipping crop_filter_updates_dimensions: ffmpeg not in PATH");
        return;
    }
    let (output, result) = run_filter("crop=6:6", y4m_stream(8));
    result.expect("the crop filter should not fail");
    let output = String::from_utf8_lossy(&output);
    assert!(
        output.starts_with("YUV4MPEG2"),
        "output starts with a Y4M header"
    );
    assert!(
        output.contains("W6 H6"),
        "FFmpeg regenerates the header with the cropped dimensions"
    );
}

#[test]
fn reports_invalid_filter_failure() {
    if which::which("ffmpeg").is_err() {
        eprintln!("skipping reports_invalid_filter_failure: ffmpeg not in PATH");
        return;
    }
    let (output, result) = run_filter("definitely_not_a_filter", y4m_stream(8));
    let message = result.expect_err("an invalid filter should fail the stage");
    assert!(output.is_empty(), "a failed filter produces no frames");
    let message = message.to_string();
    assert!(
        message.contains("definitely_not_a_filter"),
        "failure message includes FFmpeg's stderr: {message}"
    );
}

#[test]
fn check_reports_missing_ffmpeg() {
    let (input_tx, input_rx) = crossbeam_channel::bounded(2);
    drop(input_tx);
    let result = FfmpegFilter::spawn(input_rx, 2, "null", Arc::new(Mutex::new(None)));
    match result {
        Ok(stage) => drop(stage),
        Err(err) => assert!(
            err.to_string().contains("not found in PATH"),
            "spawn error names the PATH lookup: {err}"
        ),
    }
}

#[cfg(target_os = "linux")]
pub(super) mod runtime {
    //! Removing sole receiver ownership or encoder-side filter cleanup must hang
    //! these bounded fixtures; the external watchdog reaps even a failing baseline.
    use super::*;
    use crate::{
        core::{
            Condor,
            encoder::EncoderResult,
            input::Input,
            output::Output,
            sequence::{
                Sequence, SequenceStatus,
                parallel_encoder::{ParallelEncoder, Task},
                zone_encoder::{ZoneEncoder, ZonePlan},
            },
        },
        models::{
            encoder::{Encoder, EncoderBase, cli_parameter::CLIParameter},
            input::{ImportMethod, Input as InputModel},
            scene::Scene,
            sequence::{DefaultSequenceConfig, DefaultSequenceData},
        },
    };
    use std::{
        collections::HashMap,
        fs,
        os::unix::fs::PermissionsExt,
        path::Path,
        time::{Duration, Instant},
    };

    fn thread_count() -> usize {
        fs::read_dir("/proc/self/task").expect("threads").count()
    }

    // Linux can wake a native join before unlinking the exiting task from /proc.
    // This observation barrier runs after owned joins; it cannot replace them.
    fn wait_for_thread_count(
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
    fn thread_census_waits_for_transition() {
        let mut observations = [3, 3, 2].into_iter();
        assert_eq!(
            wait_for_thread_count(2, Duration::from_secs(5), || observations
                .next()
                .expect("census"))
            .expect("observe teardown after logical join"),
            2
        );
        assert!(
            observations.next().is_none(),
            "must observe the transition, not skip the census"
        );
    }

    #[test]
    fn filter_shutdown_joins_workers_and_preserves_primary_errors() {
        let temp = tempfile::tempdir().expect("fixture");
        let root = temp.path();
        fs::write(
            root.join("watchdog.rs"),
            include_str!("../../encoder/tests/fixtures/fake_encoder.rs"),
        )
        .expect("watchdog source");
        let compiled = Command::new("rustc")
            .arg(root.join("watchdog.rs"))
            .arg("-o")
            .arg(root.join("watchdog"))
            .output()
            .expect("rustc");
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let mut failures = Vec::new();
        for case in [
            "first-drop",
            "first-check",
            "blocked-output",
            "late-check",
            "controls",
            "relay-faults",
            "native-invalid",
            "native-status",
            "native-preflight",
            "native-spawn",
            "native-filter",
            "zone-invalid",
            "zone-filter",
        ] {
            let job = root.join(case);
            fs::create_dir_all(&job).expect("job");
            let log = fs::File::create(job.join("child.log")).expect("log");
            let status = Command::new(root.join("watchdog"))
                .arg("--encoder-fixture-watchdog")
                .arg(std::env::current_exe().expect("test binary"))
                .arg(&job)
                .arg(case)
                .arg("core::sequence::ffmpeg_filter::tests::runtime::filter_lifecycle_child")
                .stdout(log.try_clone().expect("clone log"))
                .stderr(log)
                .status()
                .expect("watchdog");
            let log = fs::read_to_string(job.join("child.log")).expect("log");
            if !status.success()
                || !job.join("executed").exists()
                || !job.join("verified").exists()
                || !log.contains("test result: ok. 1 passed;")
                || !log
                    .lines()
                    .any(|l| l == "encoder fixture watchdog reaped 0 descendants; timeout=false")
            {
                failures.push(format!("{case}: {status}\n{log}"));
            }
            eprint!("{log}");
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    fn native_input(root: &Path) -> Input {
        let path = root.join("96 frames.y4m");
        let mut bytes = b"YUV4MPEG2 W128 H128 F24:1 Ip A1:1 C420jpeg\n".to_vec();
        for _ in 0..96 {
            bytes.extend_from_slice(b"FRAME\n");
            bytes.extend(vec![48; 128 * 128]);
            bytes.extend(vec![128; 128 * 128 / 2]);
        }
        fs::write(&path, bytes).expect("finite source larger than the channel window");
        let mut input = Input::from_video(&InputModel::Video {
            path,
            import_method: ImportMethod::FFMS2 {
                index: None,
                cache_path: Some(root.join("native.ffindex")),
            },
            filters: vec![],
        })
        .expect("native FFMS2");
        assert!(
            input.streams_concurrently(),
            "exercise actual streaming path"
        );
        assert_eq!(input.clip_info().expect("clip").num_frames, 96);
        input
    }

    fn direct_drop(case: &str) {
        let before = thread_count();
        let (tx, rx) = crossbeam_channel::bounded(1);
        let mut stage =
            FfmpegFilter::spawn(rx, 1, "null", Arc::new(Mutex::new(None))).expect("filter");
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
            let mut stage =
                FfmpegFilter::spawn(rx, 1, graph, Arc::new(Mutex::new(None))).expect("real FFmpeg");
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
        let mut stage =
            FfmpegFilter::spawn(rx, 1, "null", Arc::new(Mutex::new(None))).expect("filter");
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

    fn native_failure(root: &Path, case: &str) {
        let mut input = native_input(root);
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
        if case == "native-status" {
            let path = root.join("early encoder");
            fs::write(
                &path,
                "#!/bin/sh\nprintf 'filter-lifecycle early encoder diagnostic' >&2\nexit 42\n",
            )
            .expect("early status fixture");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("executable");
            encoder.set_executable(Some(path));
        }
        let scenes_dir = root.join("scenes");
        fs::create_dir_all(&scenes_dir).expect("scenes");
        let (progress_tx, progress_rx) = std::sync::mpsc::channel();
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let error = if case.starts_with("zone") {
            let tasks = vec![Task {
                original_index: 7,
                index: 0,
                frame_indices: (0..96).collect(),
                sub_scenes: None,
                encoder,
                output: scenes_dir.join("00007.264"),
            }];
            let plan = ZonePlan::try_build(&tasks).expect("native x264 zoning capability");
            ZoneEncoder::encode_tasks(&mut input, tasks.into(), plan, progress_tx, cancelled)
                .expect_err("zone filter/encoder failure")
        } else if matches!(case, "native-preflight" | "native-spawn") {
            let executable = root.join("unspawnable encoder");
            if case == "native-spawn" {
                fs::write(
                    &executable,
                    format!("#!{}\n", root.join("missing-interpreter").display()),
                )
                .expect("script");
                fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).expect("mode");
            }
            encoder.set_executable(Some(executable));
            ParallelEncoder::encode_tasks(
                &mut input,
                1,
                [Task {
                    original_index: 7,
                    index: 0,
                    frame_indices: (0..96).collect(),
                    sub_scenes: None,
                    encoder,
                    output: scenes_dir.join("00007.264"),
                }]
                .into(),
                progress_tx,
                cancelled,
            )
            .expect_err("early failure must unblock inline producer")
        } else {
            let saved = Arc::new(Mutex::new(0));
            let observed = Arc::clone(&saved);
            let mut config = DefaultSequenceConfig::default();
            config.parallel_encoder.workers = Some(1);
            config.parallel_encoder.scenes_directory = scenes_dir.clone();
            let mut condor = Condor::<DefaultSequenceData, DefaultSequenceConfig>::new(
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
                    end_frame: 96,
                    sub_scenes: None,
                    encoder,
                    sequence_data: DefaultSequenceData::default(),
                }],
                Some(config),
                Box::new(move |data| {
                    if data.scenes.iter().any(|s| {
                        s.sequence_data.parallel_encoder.bytes.is_some()
                            || s.sequence_data.parallel_encoder.completed_on.is_some()
                    }) {
                        *observed.lock().expect("saved") += 1;
                    }
                    Ok(())
                }),
            );
            let error = ParallelEncoder::default()
                .execute(&mut condor, progress_tx, cancelled)
                .expect_err("filter/encoder failure");
            assert_eq!(
                *saved.lock().expect("saved"),
                0,
                "no failed completion saved"
            );
            assert!(
                condor.scenes.iter().all(|s| s.sequence_data.parallel_encoder.bytes.is_none()
                    && s.sequence_data.parallel_encoder.completed_on.is_none())
            );
            error
        };
        let diagnostic = format!("{error:#}");
        match case {
            "native-filter" | "zone-filter" => assert!(
                diagnostic.contains("lifecycle_missing_filter"),
                "original FFmpeg stderr: {diagnostic}"
            ),
            "native-invalid" | "native-status" | "zone-invalid" => {
                let result = error.chain().find_map(|e| e.downcast_ref::<EncoderResult>()).or_else(|| {
                match error.downcast_ref::<crate::core::sequence::parallel_encoder::ParallelEncoderError>() {
                    Some(crate::core::sequence::parallel_encoder::ParallelEncoderError::EncoderFailed { result, .. }) => Some(result),
                    _ => None,
                }
            }).expect("typed original encoder result");
                assert!(!result.status.success());
                if case == "native-status" {
                    assert_eq!(result.status.code(), Some(42));
                    assert_eq!(
                        result.stderr.to_string(),
                        "filter-lifecycle early encoder diagnostic"
                    );
                } else {
                    assert!(
                        result.stderr.to_string().contains("filter-lifecycle-invalid-option"),
                        "{}",
                        result.stderr
                    );
                }
                assert!(
                    !diagnostic.contains("FFmpeg filter failed"),
                    "intentional abort must not mask encoder: {diagnostic}"
                );
            },
            "native-spawn" => assert!(diagnostic.contains("Could not spawn"), "{diagnostic}"),
            "native-preflight" => {
                assert!(diagnostic.contains("unspawnable encoder"), "{diagnostic}");
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

    #[test]
    fn filter_lifecycle_child() {
        let Some(root) = std::env::var_os("ENCODER_FIXTURE_ROOT") else {
            return;
        };
        let root = std::path::PathBuf::from(root);
        let case = std::env::var("ENCODER_FIXTURE_CASE").expect("case");
        fs::write(root.join("executed"), &case).expect("handshake");
        if case == "controls" {
            controls();
        } else if case == "relay-faults" {
            relay_faults();
        } else if case.starts_with("native") || case.starts_with("zone") {
            native_failure(&root, &case);
        } else {
            direct_drop(&case);
        }
        fs::write(root.join("verified"), "all assertions ran").expect("verification handshake");
    }
}
