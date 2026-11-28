//! Counts actual dispatcher workers; only the external codec is synthetic at scale.
use super::*;
pub(in crate::core::sequence::parallel_encoder) mod internal_integration {
    use super::*;
    use crate::{
        core::encoder::EncoderResult,
        models::encoder::{Encoder, EncoderBase, EncoderPasses},
        utils::semaphore::Semaphore,
    };
    use std::{
        collections::VecDeque,
        fs,
        io::Cursor,
        os::unix::fs::PermissionsExt,
        path::Path,
        process::Command,
        sync::{
            Condvar, Mutex, OnceLock,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
    };

    struct Probe {
        synthetic: bool,
        handshake: bool,
        failure: bool,
        save_failure: bool,
        workers: usize,
        spawned: AtomicUsize,
        live: AtomicUsize,
        maximum: AtomicUsize,
        headers: Mutex<Vec<usize>>,
        consumed: Mutex<Vec<usize>>,
        held: Mutex<(usize, bool, bool)>,
        changed: Condvar,
        admission: crossbeam_channel::Sender<Arc<Semaphore>>,
    }

    static PROBE: OnceLock<Probe> = OnceLock::new();

    #[derive(Default)]
    struct DisconnectState {
        worker_checked: bool,
        cancelled: bool,
        produced: bool,
        scheduled: bool,
        frame_send_error: bool,
    }

    struct Disconnect {
        case: String,
        root: std::path::PathBuf,
        state: Mutex<DisconnectState>,
        changed: Condvar,
    }

    static DISCONNECT: OnceLock<Disconnect> = OnceLock::new();

    pub(in crate::core::sequence::parallel_encoder) fn worker_received() {
        if let Some(p) = DISCONNECT.get().filter(|p| p.case == "cancel-before-worker") {
            let state = p.state.lock().expect("state");
            drop(
                p.changed
                    .wait_while(state, |s| !s.cancelled)
                    .expect("cancel before worker check"),
            );
        }
    }

    pub(in crate::core::sequence::parallel_encoder) fn worker_admitted() {
        if let Some(p) = DISCONNECT.get() {
            p.state.lock().expect("state").worker_checked = true;
            p.changed.notify_all();
        }
    }

    pub(in crate::core::sequence::parallel_encoder) fn after_handoff(cancelled: &AtomicBool) {
        let Some(p) = DISCONNECT.get().filter(|p| p.case.starts_with("cancel-")) else {
            return;
        };
        let state = p.state.lock().expect("state");
        let mut state = p
            .changed
            .wait_while(state, |s| p.case == "cancel-handoff" && !s.worker_checked)
            .expect("worker passed stop check");
        cancelled.store(true, Ordering::SeqCst);
        state.cancelled = true;
        p.changed.notify_all();
    }

    pub(in crate::core::sequence::parallel_encoder) fn before_header() {
        if let Some(p) = DISCONNECT
            .get()
            .filter(|p| p.case == "codec-disconnect" || p.case.starts_with("decode-primary"))
        {
            // The real process has closed stdin before its writer sees this header.
            while !p.root.join("stdin-closed").exists() {
                std::thread::yield_now();
            }
        }
    }

    pub(in crate::core::sequence::parallel_encoder) fn before_frames(stop: &lifecycle::Stop) {
        if let Some(p) = DISCONNECT
            .get()
            .filter(|p| p.case == "codec-disconnect" || p.case.starts_with("decode-primary"))
        {
            while !p.root.join("consumer-closed").exists() {
                std::thread::yield_now();
            }
            if p.case == "decode-primary-stopped" {
                stop.stop();
            }
        }
    }

    pub(in crate::core::sequence::parallel_encoder) fn consumer_closed(point: &str) {
        let Some(p) = DISCONNECT.get().filter(|p| {
            (point == "encoder"
                && (p.case == "codec-disconnect" || p.case.starts_with("decode-primary")))
                || (point == "filter" && p.case == "filter-disconnect")
        }) else {
            return;
        };
        // Delay only error notification, after real codec/filter receivers and
        // helpers have closed. Let the actual producer observe its own send error.
        fs::write(p.root.join("consumer-closed"), "receivers closed").expect("closed handshake");
        let state = p.state.lock().expect("state");
        drop(
            p.changed
                .wait_while(state, |s| !s.scheduled)
                .expect("producer failure collected before worker notification"),
        );
    }

    pub(in crate::core::sequence::parallel_encoder) fn producer_returned(result: &Result<()>) {
        if let Some(p) = DISCONNECT.get() {
            let mut state = p.state.lock().expect("state");
            state.produced = true;
            state.frame_send_error = result
                .as_ref()
                .is_err_and(|err| err.is::<crossbeam_channel::SendError<Cursor<Vec<u8>>>>());
            p.changed.notify_all();
        }
    }

    pub(in crate::core::sequence::parallel_encoder) fn scheduled_finished() {
        if let Some(p) = DISCONNECT.get() {
            let mut state = p.state.lock().expect("state");
            assert!(state.produced || p.case.starts_with("cancel-"));
            state.scheduled = true;
            p.changed.notify_all();
        }
    }

    pub(in crate::core::sequence::parallel_encoder) struct WorkerCount(&'static Probe);

    pub(in crate::core::sequence::parallel_encoder) fn worker_started() -> Option<WorkerCount> {
        let probe = PROBE.get()?;
        probe.spawned.fetch_add(1, Ordering::SeqCst);
        let live = probe.live.fetch_add(1, Ordering::SeqCst) + 1;
        probe.maximum.fetch_max(live, Ordering::SeqCst);
        Some(WorkerCount(probe))
    }

    impl Drop for WorkerCount {
        fn drop(&mut self) {
            self.0.live.fetch_sub(1, Ordering::SeqCst);
        }
    }

    pub(in crate::core::sequence::parallel_encoder) fn before_admission(
        stop: &lifecycle::Stop,
        task: &Task,
    ) {
        let Some(p) = PROBE.get().filter(|p| p.handshake && task.index == p.workers) else {
            return;
        };
        let held = p.held.lock().expect("held workers");
        drop(p.changed.wait_while(held, |h| h.0 != p.workers).expect("all workers busy"));
        p.admission.send(Arc::clone(&stop.workers)).expect("controller");
    }

    pub(in crate::core::sequence::parallel_encoder) fn header(task: &Task) {
        let Some(p) = PROBE.get() else { return };
        if p.handshake && task.index >= p.workers {
            assert!(
                p.held.lock().expect("held").1,
                "queued header decoded while all workers busy"
            );
            assert!(!p.failure, "queued header decoded after failure");
        }
        p.headers.lock().expect("headers")[task.index] += 1;
    }

    pub(in crate::core::sequence::parallel_encoder) fn before_joins() {
        if let Some(p) = PROBE.get().filter(|p| p.save_failure) {
            p.held.lock().expect("held").2 = true;
            p.changed.notify_all();
        }
    }

    pub(in crate::core::sequence::parallel_encoder) fn consume(
        task: &Task,
        frames: &crossbeam_channel::Receiver<Cursor<Vec<u8>>>,
        temp: &Path,
        stop: &lifecycle::Stop,
    ) -> Option<Result<EncoderResult>> {
        let p = PROBE.get().filter(|p| p.synthetic)?;
        Some((|| {
            let bytes: Vec<_> = frames.iter().map(Cursor::into_inner).collect();
            assert_eq!(bytes.len(), 2, "one real VS header and one real frame");
            let header = std::str::from_utf8(&bytes[0]).expect("Y4M header");
            assert!(header.starts_with("YUV4MPEG2 "));
            for field in ["W64", "H64", "F24:1"] {
                assert!(
                    header.split_whitespace().any(|token| token == field),
                    "{header}"
                );
            }
            assert_eq!(&bytes[1][..6], b"FRAME\n");
            assert_eq!(bytes[1].len(), 6 + 64 * 64 * 3 / 2);
            p.consumed.lock().expect("identities")[task.index] += 1;
            if p.handshake && task.index < p.workers {
                let mut held = p.held.lock().expect("held");
                held.0 += 1;
                p.changed.notify_all();
                if (p.failure || p.save_failure) && task.index != 0 {
                    drop(held);
                    let _ = stop.receiver.recv();
                } else {
                    drop(
                        p.changed
                            .wait_while(held, |h| !h.1 || (p.save_failure && !h.2))
                            .expect("release"),
                    );
                }
                if p.failure && task.index == 0 {
                    anyhow::bail!("dispatch active scene {} failure", task.original_index);
                }
            }
            fs::write(temp, task.original_index.to_string())?;
            Ok(EncoderResult {
                encoder: EncoderBase::X264,
                parameters: vec![task.original_index.to_string()],
                status: std::process::ExitStatus::default(),
                stdout: Vec::new().into(),
                stderr: Vec::new().into(),
            })
        })())
    }

    fn dispatch_cases(test: &str, cases: &[&str]) {
        if let Ok(case) = std::env::var("ENCODER_FIXTURE_CASE") {
            let case = case.strip_prefix("dispatch-").expect("dispatch selection");
            assert!(
                cases.contains(&case),
                "dispatch case belongs to this contract"
            );
            check_dispatch_case();
            println!("dispatch {case}: assertions completed");
            return;
        }
        let temp = tempfile::tempdir().expect("fixtures");
        let root = temp.path();
        fs::write(
            root.join("watchdog.rs"),
            include_str!("../../../../../tests/core/encoder/fixtures/fake_encoder.rs"),
        )
        .expect("watchdog");
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
        for case in cases {
            let job = root.join(case);
            fs::create_dir_all(&job).expect("job");
            let log = fs::File::create(job.join("child.log")).expect("log");
            let status = Command::new(root.join("watchdog"))
            .arg("--encoder-fixture-watchdog")
            .arg(std::env::current_exe().expect("test binary"))
            .arg(&job)
            .arg(format!("dispatch-{case}"))
            .arg(format!("core::sequence::parallel_encoder::tests::dispatch::internal_integration::{test}"))
            .stdout(log.try_clone().expect("log clone"))
            .stderr(log)
            .status()
            .expect("watchdog");
            let log = fs::read_to_string(job.join("child.log")).expect("child log");
            if !status.success()
                || !job.join("verified").exists()
                || !log.contains("test result: ok. 1 passed;")
                || !log.contains(&format!("dispatch {case}: assertions completed"))
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

    fn check_dispatch_case() {
        let Ok(case) = std::env::var("ENCODER_FIXTURE_CASE") else {
            return;
        };
        let Some(case) = case.strip_prefix("dispatch-") else {
            return;
        };
        let root =
            std::path::PathBuf::from(std::env::var_os("ENCODER_FIXTURE_ROOT").expect("root"));
        fs::write(root.join("executed"), case).expect("execution handshake");
        let (workers, count) = match case {
            "count1" => (1, 2000),
            "count3" => (3, 2000),
            "excess" => (8, 2),
            "empty" => (3, 0),
            "save" => (3, 3),
            "admission" | "failure" => (3, 4),
            "public1" => (1, 5),
            "public3" => (3, 5),
            "public8" => (8, 5),
            "codec-disconnect"
            | "filter-disconnect"
            | "cancel-handoff"
            | "cancel-before-worker"
            | "decode-primary"
            | "decode-primary-stopped" => (1, 2),
            _ => panic!("unknown case"),
        };
        let disconnect = case.ends_with("disconnect")
            || case.starts_with("cancel-")
            || case.starts_with("decode-primary");
        let synthetic = !case.starts_with("public") && !disconnect;
        let handshake = matches!(case, "admission" | "failure" | "save");
        let failure = case == "failure";
        let save_failure = case == "save";
        let (admission, admitted) = crossbeam_channel::bounded(1);
        assert!(
            PROBE
                .set(Probe {
                    synthetic,
                    handshake,
                    failure,
                    save_failure,
                    workers: usize::from(workers),
                    spawned: AtomicUsize::new(0),
                    live: AtomicUsize::new(0),
                    maximum: AtomicUsize::new(0),
                    headers: Mutex::new(vec![0; count]),
                    consumed: Mutex::new(vec![0; count]),
                    held: Mutex::new((0, false, false)),
                    changed: Condvar::new(),
                    admission,
                })
                .is_ok()
        );
        let p = PROBE.get().expect("probe");
        if disconnect {
            disconnect_child(&root, case, p);
            fs::write(
                root.join("verified"),
                "actual consumer diagnostic/cancel/decode checks passed",
            )
            .expect("verified");
            return;
        }
        let mut input = shutdown::internal_integration::input(&root, false);
        assert!(
            !input.streams_concurrently(),
            "real nonstream VS dispatcher"
        );
        let fake = root.join("codec");
        fs::write(&fake, "#!/bin/sh\nset -eu\nout=\nwhile [ \"$#\" -gt 0 ]; do\nif [ \"$1\" = -o ]; then shift; out=$1; fi\nshift\ndone\ncat >/dev/null\nprintf owned >\"$out\"\n").expect("fake codec");
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).expect("executable");
        let tasks: VecDeque<_> = (0..count)
            .map(|position| {
                // Non-handshake cases deliberately dispatch in reverse result-slot order.
                let index = if handshake {
                    position
                } else {
                    count - 1 - position
                };
                let mut encoder = Encoder::default_from_base(&EncoderBase::X264, true);
                encoder.set_append_color_data(false);
                encoder.set_executable(Some(fake.clone()));
                *encoder.passes_mut().expect("passes") = EncoderPasses::All(1);
                Task {
                    index,
                    original_index: 7 + index * 3,
                    frame_indices: vec![0],
                    sub_scenes: None,
                    encoder,
                    output: root.join(format!("{index}.264")),
                }
            })
            .collect();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut callbacks = vec![0; count];
        let mut on_results = |batch: Vec<task::PublishedResult>| -> Result<()> {
            for r in batch.into_iter().map(|published| published.result) {
                assert!(r.result.status.success());
                callbacks[(r.scene - 7) / 3] += 1;
            }
            if save_failure && callbacks[0] == 1 && callbacks[1] == 0 && callbacks[2] == 0 {
                anyhow::bail!("dispatch persistence failure with last batch active");
            }
            Ok(())
        };
        let (results_tx, results_rx) = crossbeam_channel::unbounded();
        let mut stream = task::ResultStream {
            results_tx,
            results_rx: &results_rx,
            finished_scenes: Arc::new(Semaphore::new(0)),
            on_results: &mut on_results,
        };
        let result = std::thread::scope(|s| {
            let controller = handshake.then(|| {
                s.spawn(|| {
                    if save_failure {
                        let held = p.held.lock().expect("held");
                        let mut held =
                            p.changed.wait_while(held, |h| h.0 != 3).expect("all workers busy");
                        held.1 = true;
                        p.changed.notify_all();
                        return;
                    }
                    let pool =
                        admitted.recv().expect("queued admission attempted with all workers held");
                    let blocked = pool.wait_for_waiter();
                    let mut held = p.held.lock().expect("held");
                    assert_eq!(held.0, 3, "all workers really occupied");
                    let headers = p.headers.lock().expect("headers")[3];
                    held.1 = true;
                    p.changed.notify_all();
                    drop(held);
                    assert!(
                        blocked,
                        "scheduler must block on worker admission, before queued decode"
                    );
                    assert_eq!(headers, 0, "queued header untouched at admission handshake");
                })
            });
            let result = if synthetic {
                ParallelEncoder::encode_tasks_with_sender(
                    &mut input,
                    workers,
                    tasks,
                    tx,
                    Arc::new(AtomicBool::new(false)).into(),
                    &mut stream,
                    ParallelEncoderOptions {
                        max_tries: 1,
                        validate_frame_count: false,
                        ..Default::default()
                    }, // Fixed-pool one-attempt control.
                )
            } else {
                ParallelEncoder::encode_tasks(
                    &mut input,
                    workers,
                    tasks,
                    tx,
                    Arc::new(AtomicBool::new(false)).into(),
                    ParallelEncoderOptions {
                        max_tries: 1,
                        validate_frame_count: false,
                        ..Default::default()
                    },
                )
            };
            if let Some(controller) = controller {
                controller.join().expect("controller joined");
            }
            result
        });
        let spawned = p.spawned.load(Ordering::SeqCst);
        let maximum = p.maximum.load(Ordering::SeqCst);
        eprintln!(
            "dispatch {case}: tasks={count}, configured={workers}, spawned={spawned}, max_live={maximum}, live_after_join={}",
            p.live.load(Ordering::SeqCst)
        );
        assert_eq!(
            p.live.load(Ordering::SeqCst),
            0,
            "all actual worker guards dropped"
        );
        assert!(
            spawned <= usize::from(workers).min(count),
            "one fixed bounded worker set, not per-scene threads: {spawned}"
        );
        assert_eq!(
            spawned,
            usize::from(workers).min(count),
            "no zero-worker false pass"
        );
        assert!(maximum <= spawned && (count == 0 || maximum > 0));
        if save_failure {
            let error = result.expect_err("callback failure observed before any blocking join");
            assert!(
                format!("{error:#}")
                    .contains("dispatch persistence failure with last batch active")
            );
            assert_eq!(
                callbacks,
                [1, 1, 1],
                "no failed-batch retry; active peers still drained"
            );
            for index in 0..3 {
                assert_eq!(
                    fs::read_to_string(root.join(format!("{index}.264"))).expect("active output"),
                    (7 + index * 3).to_string()
                );
            }
        } else if failure {
            let error = result.expect_err("active failure closes admission");
            assert!(format!("{error:#}").contains("dispatch active scene 7 failure"));
            assert_eq!(*p.headers.lock().expect("headers"), [1, 1, 1, 0]);
            assert_eq!(*p.consumed.lock().expect("consumed"), [1, 1, 1, 0]);
            assert_eq!(
                callbacks,
                [0, 1, 1, 0],
                "successful active peers still persist after stop"
            );
            assert!(!root.join("0.264").exists() && !root.join("3.264").exists());
        } else {
            let results = result.expect("all tasks completed");
            assert_eq!(results.len(), count);
            for (index, result) in results.into_iter().enumerate() {
                let result = result.expect("each result slot filled exactly once");
                assert_eq!(
                    result.scene,
                    7 + index * 3,
                    "sparse scene identity vs result slot"
                );
                assert!(result.result.status.success());
                let expected = if synthetic {
                    result.scene.to_string()
                } else {
                    "owned".to_owned()
                };
                assert_eq!(
                    fs::read_to_string(root.join(format!("{index}.264"))).expect("final output"),
                    expected
                );
                assert_eq!(result.bytes, expected.len() as u64);
            }
            assert_eq!(*p.headers.lock().expect("headers"), vec![1; count]);
            if synthetic {
                assert_eq!(*p.consumed.lock().expect("consumed"), vec![1; count]);
                assert_eq!(
                    callbacks,
                    vec![1; count],
                    "each successful task persisted exactly once"
                );
            }
        }
        let _: Vec<_> = rx.try_iter().collect();
        assert!(
            matches!(
                rx.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Disconnected)
            ),
            "all progress senders dropped"
        );
        assert!(
            !fs::read_dir(&root).expect("files").any(|e| e
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .contains(".temp."))
        );
        fs::write(
            root.join("verified"),
            "bounded actual dispatch, identities and joins checked",
        )
        .expect("verified");
    }

    fn disconnect_child(root: &Path, case: &str, p: &Probe) {
        use crate::{
            core::{encoder::EncoderProcessError, input::Input},
            models::input::{Input as InputModel, VapourSynthScriptSource},
        };
        assert!(
            DISCONNECT
                .set(Disconnect {
                    case: case.to_owned(),
                    root: root.to_owned(),
                    state: Mutex::new(DisconnectState::default()),
                    changed: Condvar::new(),
                })
                .is_ok()
        );
        let early = case == "codec-disconnect" || case.starts_with("decode-primary");
        let codec = root.join("codec");
        let body = if early {
            "exec 0<&-\nprintf 'dispatch early codec diagnostic' >&2\nprintf closed > \"$ENCODER_FIXTURE_ROOT/stdin-closed\"\nexit 42\n"
        } else if case == "filter-disconnect" {
            "cat > \"$ENCODER_FIXTURE_ROOT/input.y4m\"\nprintf owned > \"$out\"\n"
        } else {
            "cat > \"$ENCODER_FIXTURE_ROOT/input.y4m\"\nif [ ! -s \"$ENCODER_FIXTURE_ROOT/input.y4m\" ]; then printf 'dispatch empty admitted input' >&2; exit 84; fi\nprintf owned > \"$out\"\n"
        };
        fs::write(&codec, format!("#!/bin/sh\nset -eu\nprintf 'started\\n' >> \"$ENCODER_FIXTURE_ROOT/codec-starts\"\nout=\nwhile [ \"$#\" -gt 0 ]; do\nif [ \"$1\" = -o ]; then shift; out=$1; fi\nshift\ndone\n{body}")).expect("finite codec");
        fs::set_permissions(&codec, fs::Permissions::from_mode(0o755)).expect("codec executable");
        let script = root.join("disconnect.vpy");
        let eval = if case.starts_with("decode-primary") {
            "    if n == 1: raise RuntimeError('dispatch actual VS frame decode failure')\n"
                .to_owned()
        } else if case == "filter-disconnect" {
            format!(
                "    if n == 32:\n        Path({:?}).write_text('producer has frames remaining')\n        while not Path({:?}).exists(): pass\n",
                root.join("frame-blocked"),
                root.join("consumer-closed")
            )
        } else {
            String::new()
        };
        fs::write(&script, format!("import vapoursynth as vs\nfrom pathlib import Path\ncore = vs.core\nclip = core.std.BlankClip(width=64, height=64, length=96, format=vs.YUV420P8, fpsnum=24, fpsden=1)\ndef gate(n):\n{eval}    return clip\ncore.std.FrameEval(clip, gate).set_output()\n")).expect("real VS script");
        let mut input = Input::from_data(&InputModel::VapourSynthScript {
            source: VapourSynthScriptSource::Path(script),
            variables: Default::default(),
            index: 0,
            filters: vec![],
            stream_concurrently: false,
        })
        .expect("actual nonstream VS");
        assert!(!input.streams_concurrently());
        let tasks = (0..2)
            .map(|index| {
                let mut encoder = Encoder::default_from_base(&EncoderBase::X264, true);
                encoder.set_append_color_data(false);
                encoder.set_executable(Some(codec.clone()));
                *encoder.passes_mut().expect("passes") = EncoderPasses::All(1);
                if case == "filter-disconnect" {
                    encoder.set_ffmpeg_filter(Some("dispatch_missing_filter".to_owned()));
                }
                Task {
                    index,
                    original_index: 7 + index * 3,
                    frame_indices: if case.starts_with("decode-primary") {
                        vec![1]
                    } else {
                        (0..96).collect()
                    },
                    sub_scenes: None,
                    encoder,
                    output: root.join(format!("{index}.264")),
                }
            })
            .collect();
        fs::write(root.join("unrelated.stat"), "stats sentinel").expect("stats sentinel");
        fs::write(root.join("preexisting.264"), "final sentinel").expect("final sentinel");
        let (tx, rx) = std::sync::mpsc::channel();
        let result = ParallelEncoder::encode_tasks(
            &mut input,
            1,
            tasks,
            tx,
            Arc::new(AtomicBool::new(false)).into(),
            ParallelEncoderOptions {
                max_tries: 1,
                validate_frame_count: false,
                ..Default::default()
            },
        );
        let state = DISCONNECT.get().expect("race probe").state.lock().expect("state");
        if case.starts_with("cancel-") {
            let results =
                result.expect("graceful handoff cancellation is not an empty-codec failure");
            assert_eq!(results.len(), 2);
            assert!(results[1].is_none());
            if case == "cancel-handoff" {
                assert_eq!(
                    results[0].as_ref().expect("active admitted job finishes").scene,
                    7
                );
                assert_eq!(
                    fs::read(root.join("0.264")).expect("active output"),
                    b"owned"
                );
                let bytes = fs::read(root.join("input.y4m")).expect("nonempty real codec input");
                assert!(bytes.starts_with(b"YUV4MPEG2 "));
                assert_eq!(bytes.windows(6).filter(|w| *w == b"FRAME\n").count(), 96);
            } else {
                assert!(
                    results[0].is_none(),
                    "worker observing stop before consume skips cleanly"
                );
            }
        } else {
            let error = result.expect_err("actual consumer/decode failure");
            let diagnostic = format!("{error:#}");
            assert!(
                !error.is::<crossbeam_channel::SendError<Cursor<Vec<u8>>>>(),
                "consumer/decode diagnostic must beat frame disconnect: {diagnostic}"
            );
            if case.starts_with("decode-primary") {
                assert!(
                    !state.frame_send_error,
                    "actual decoder failure, not a channel fault"
                );
                assert!(
                    diagnostic.contains("dispatch actual VS frame decode failure"),
                    "{diagnostic}"
                );
                assert!(
                    diagnostic.contains("Scene 7 frame production"),
                    "{diagnostic}"
                );
            } else {
                assert!(
                    state.frame_send_error,
                    "real producer observed a typed frame SendError before worker notification"
                );
                if case == "codec-disconnect" {
                    assert!(
                        error.downcast_ref::<EncoderProcessError>().is_some(),
                        "real STDIN worker error retained: {diagnostic}"
                    );
                    let encoded = error
                        .chain()
                        .find_map(|e| e.downcast_ref::<EncoderResult>())
                        .expect("typed encoder result retained");
                    assert_eq!(encoded.status.code(), Some(42));
                    assert_eq!(
                        encoded.stderr.to_string(),
                        "dispatch early codec diagnostic"
                    );
                } else {
                    assert!(
                        root.join("frame-blocked").exists(),
                        "producer held with frames remaining"
                    );
                    assert!(
                        diagnostic.contains("FFmpeg filter failed while encoding scene 7")
                            && diagnostic.contains("dispatch_missing_filter"),
                        "{diagnostic}"
                    );
                }
            }
            eprintln!(
                "dispatch {case}: real frame_send={}, returned primary: {diagnostic}",
                state.frame_send_error
            );
            assert!(!root.join("0.264").exists(), "failed scene not finalized");
        }
        assert_eq!(p.spawned.load(Ordering::SeqCst), 1);
        assert_eq!(p.maximum.load(Ordering::SeqCst), 1);
        assert_eq!(p.live.load(Ordering::SeqCst), 0, "actual worker joined");
        assert_eq!(
            p.headers.lock().expect("headers")[1],
            0,
            "queued scene never decoded"
        );
        assert!(!root.join("1.264").exists());
        let starts = fs::read_to_string(root.join("codec-starts")).unwrap_or_default();
        assert_eq!(
            starts.lines().count(),
            usize::from(case != "cancel-before-worker"),
            "no queued/empty codec starts"
        );
        let _: Vec<_> = rx.try_iter().collect();
        assert!(
            matches!(
                rx.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Disconnected)
            ),
            "all progress relays joined"
        );
        assert!(!fs::read_dir(root).expect("files").any(|e| {
            let name = e.expect("entry").file_name().to_string_lossy().into_owned();
            name.contains(".temp.") || name.starts_with(".av1an-")
        }));
        assert_eq!(
            fs::read_to_string(root.join("unrelated.stat")).expect("sentinel"),
            "stats sentinel"
        );
        assert_eq!(
            fs::read_to_string(root.join("preexisting.264")).expect("sentinel"),
            "final sentinel"
        );
        eprintln!(
            "dispatch {case}: spawned/max/live=1/1/0, all joined, queued header=0, sentinels retained"
        );
    }

    macro_rules! dispatch_contract {
    ($name:ident, [$($case:literal),+ $(,)?]) => {
        #[test]
        fn $name() { dispatch_cases(stringify!($name), &[$($case),+]); }
    };
}

    dispatch_contract!(
        dispatcher_spawns_only_the_configured_bounded_worker_set,
        ["count1", "count3", "excess", "empty", "public1", "public3", "public8"]
    );
    dispatch_contract!(
        dispatcher_blocks_before_decoding_a_queued_header,
        ["admission"]
    );
    dispatch_contract!(
        worker_failure_closes_admission_but_persists_successful_active_peers,
        ["failure", "save"]
    );
    dispatch_contract!(
        cancelled_handoffs_are_not_consumed_by_unadmitted_workers,
        ["cancel-handoff", "cancel-before-worker"]
    );
    dispatch_contract!(
        consumer_disconnect_retains_the_primary_decoder_or_codec_error,
        [
            "codec-disconnect",
            "filter-disconnect",
            "decode-primary",
            "decode-primary-stopped"
        ]
    );
}
