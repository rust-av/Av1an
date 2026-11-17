use super::*;

#[test]
fn forced_spool_replay_unblocks_a_full_consumer_queue() {
    let cancellation = Cancellation::default();
    let (tx, rx) = bounded(1);
    tx.send(Cursor::new(vec![1; 1024 * 1024])).expect("spool input");
    drop(tx);
    let mut calls = 0;
    let error = encode_cancel(
        Cursor::new(Vec::new()),
        rx,
        Path::new("unused"),
        2,
        &cancellation,
        |_, frames| -> Result<()> {
            calls += 1;
            while frames.len() != 2 {
                thread::yield_now();
            }
            cancellation.request()?;
            cancellation.request()?;
            Err(Cancelled::Forced.into())
        },
    )
    .expect_err("force unblocks the bounded replay producer");
    assert_eq!(calls, 1);
    assert!(error.is::<Cancelled>());
}

#[test]
fn force_between_spool_passes_prevents_the_next_pass() {
    let cancellation = Cancellation::default();
    let (tx, rx) = bounded(1);
    tx.send(Cursor::new(vec![1; 32])).expect("spool input");
    drop(tx);
    let mut calls = 0;
    let error = encode_cancel(
        Cursor::new(Vec::new()),
        rx,
        Path::new("unused"),
        2,
        &cancellation,
        |pass, frames| {
            calls += 1;
            assert_eq!(pass, 1, "force at pass transition must prevent pass 2");
            assert_eq!(
                frames.into_iter().map(|f| f.into_inner().len()).sum::<usize>(),
                32
            );
            cancellation.request()?;
            cancellation.request()?;
            Ok(())
        },
    )
    .expect_err("force checked before the next pass");
    assert_eq!(calls, 1);
    assert!(error.is::<Cancelled>());
}

#[cfg(target_os = "linux")]
pub(super) mod runtime {
    pub(in crate::core::encoder::spool) use internal_integration::{
        observe_capture, observe_flush,
    };

    mod internal_integration {
        use crate::core::encoder::*;
        use std::{cell::RefCell, fs, os::unix::fs::PermissionsExt};

        thread_local! {
            static CAPTURE: RefCell<Option<mpsc::Sender<&'static str>>> = const { RefCell::new(None) };
            static FINISHED: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
        }

        pub(in crate::core::encoder::spool) fn observe_flush() {
            FINISHED.with(|hook| {
                if let Some(path) = hook.borrow_mut().take() {
                    fs::write(path, "capture flushed before process launch")
                        .expect("flush handshake");
                }
            });
        }

        pub(in crate::core::encoder::spool) fn observe_capture() {
            CAPTURE.with(|hook| {
                if let Some(tx) = hook.borrow_mut().take() {
                    tx.send("capture").expect("capture observer");
                }
            });
        }

        const FAKE: &str = r#"#!/bin/sh
set -eu
pass=1 records= mode=
while [ "$#" -gt 0 ]; do
    case "$1" in
        --first-pass) pass=1; shift ;;
        --second-pass) pass=2; shift ;;
        --spool-fixture-records) records=$2; shift ;;
        --spool-fixture-mode) mode=$2; shift ;;
    esac
    shift
done
printf '%s\n' "$pass" >> "$records/order"
if [ "$mode" = early ]; then
    printf 'early stdout\377'
    printf 'early stderr\376' >&2
    exit 42
fi
if [ "$mode" = capture ]; then
    [ -f "$records/captured" ]
    [ -f "$records/flushed" ]
    cat > "$records/pass$pass.y4m"
else
    IFS= read -r header
    printf '%s\n' "$header" > "$records/header"
    printf 'encoded 1 frames\n' >&2
    cat > "$records/tail"
fi
"#;

        fn encoder(root: &Path, mode: &str, passes: EncoderPasses) -> Encoder {
            let executable = root.join("fake encoder");
            fs::write(&executable, FAKE).expect("fake");
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o755))
                .expect("executable");
            let mut encoder = Encoder::default_from_base(&EncoderBase::RAV1E, true);
            encoder.set_executable(Some(executable));
            encoder.set_append_color_data(false);
            *encoder.passes_mut().expect("passes") = passes;
            for (key, value) in [
                ("spool-fixture-records", root.to_str().expect("utf8")),
                ("spool-fixture-mode", mode),
            ] {
                encoder
                    .parameters_mut()
                    .insert(key.into(), CLIParameter::new_string("--", " ", value));
            }
            encoder
        }

        #[test]
        fn multipass_stream_capture_finishes_before_identical_replays() {
            spool_cases(
                "multipass_stream_capture_finishes_before_identical_replays",
                &["capture"],
            );
        }

        #[test]
        fn single_and_specific_pass_stream_without_spooling() {
            spool_cases(
                "single_and_specific_pass_stream_without_spooling",
                &["single", "specific"],
            );
        }

        #[test]
        fn spool_io_failures_join_workers_and_close_owned_files() {
            spool_cases(
                "spool_io_failures_join_workers_and_close_owned_files",
                &["early", "write", "flush", "seek", "read", "drop", "parent"],
            );
        }

        #[test]
        fn spool_capture_and_replay_bound_outstanding_buffers() {
            spool_cases(
                "spool_capture_and_replay_bound_outstanding_buffers",
                &["bounds"],
            );
        }

        fn spool_cases(test: &str, cases: &[&str]) {
            if std::env::var_os("ENCODER_FIXTURE_ROOT").is_some() {
                let case = std::env::var("ENCODER_FIXTURE_CASE").expect("case");
                assert!(
                    cases.contains(&case.as_str()),
                    "selected spool case belongs to this contract"
                );
                assert_spool_capture_or_failure();
                println!("spool {case}: assertions completed");
                return;
            }
            let temp = tempfile::tempdir().expect("fixture");
            let root = temp.path();
            fs::write(
                root.join("watchdog.rs"),
                include_str!("../../../../tests/core/encoder/fixtures/fake_encoder.rs"),
            )
            .expect("watchdog source");
            crate::core::encoder::tests::support::run_bounded(
                Command::new("rustc")
                    .arg(root.join("watchdog.rs"))
                    .arg("-o")
                    .arg(root.join("watchdog")),
                &root.join("rustc.log"),
            );
            let mut failures = Vec::new();
            for case in cases {
                let job = root.join(case);
                fs::create_dir_all(&job).expect("job");
                let file = fs::File::create(job.join("child.log")).expect("log");
                let status = Command::new(root.join("watchdog"))
                    .arg("--encoder-fixture-watchdog")
                    .arg(std::env::current_exe().expect("test binary"))
                    .arg(&job)
                    .arg(case)
                    .arg(format!(
                        "core::encoder::spool::tests::runtime::internal_integration::{test}"
                    ))
                    .stdout(file.try_clone().expect("clone log"))
                    .stderr(file)
                    .status()
                    .expect("watchdog");
                let log = fs::read_to_string(job.join("child.log")).expect("child log");
                if !status.success()
                    || !job.join("verified").exists()
                    || !log.contains("test result: ok. 1 passed;")
                    || !log.contains(&format!("spool {case}: assertions completed"))
                    || !log.lines().any(|s| {
                        s == "encoder fixture watchdog reaped 0 descendants; timeout=false"
                    })
                {
                    failures.push(format!("{case}: {status}\n{log}"));
                }
                eprint!("{log}");
            }
            assert!(failures.is_empty(), "{}", failures.join("\n"));
        }

        fn assert_spool_capture_or_failure() {
            let Some(root) = std::env::var_os("ENCODER_FIXTURE_ROOT") else {
                return;
            };
            let root = PathBuf::from(root);
            let case = std::env::var("ENCODER_FIXTURE_CASE").expect("case");
            if matches!(
                case.as_str(),
                "bounds" | "write" | "flush" | "seek" | "read" | "drop" | "parent"
            ) {
                io_case(&root, &case);
                fs::write(root.join("verified"), "I/O assertions executed")
                    .expect("verification handshake");
                return;
            }
            let passes = match case.as_str() {
                "single" => EncoderPasses::All(1),
                "specific" => EncoderPasses::Specific(1, 2),
                _ => EncoderPasses::All(2),
            };
            let mode = if case == "specific" || case == "single" {
                "stream"
            } else {
                &case
            };
            let encoder = encoder(&root, mode, passes);
            let output = root.join("scene0.ivf");
            fs::write(&output, "caller output").expect("sentinel");
            fs::write(root.join("scene0.stat"), "caller stats").expect("sentinel");
            let (tx, rx) = crossbeam_channel::bounded(2);
            let (progress, progress_rx) = mpsc::channel::<EncodeProgress>();
            let (event, events) = mpsc::channel();
            let first_progress = event.clone();
            let forward = thread::spawn(move || {
                for p in progress_rx {
                    if p.frame == 0 {
                        let _ = first_progress.send("progress");
                    }
                    if p.frame == 1 {
                        let _ = first_progress.send("consumed");
                    }
                }
            });
            let job_output = output.clone();
            let encode = thread::spawn(move || {
                CAPTURE.with(|hook| *hook.borrow_mut() = Some(event));
                FINISHED
                    .with(|hook| *hook.borrow_mut() = Some(job_output.with_file_name("flushed")));
                encoder.encode_with_stream(
                    rx,
                    &job_output,
                    progress,
                    &crate::core::encoder::tests::clip_info(),
                )
            });
            let header = b"YUV4MPEG2 W64 H64 F24:1 Ip A0:0 C420\n";
            let mut header_message = Cursor::new(header.to_vec());
            header_message.set_position(7);
            tx.send(header_message).expect("header with nonzero cursor position");
            let mut first = None;
            if case == "capture" {
                first = Some(events.recv().expect("capture or premature pass progress"));
                // Bounded caller generates each message lazily; no all-frames fixture Vec.
                for index in 0..64u8 {
                    let mut bytes = b"FRAME tag\n".to_vec();
                    bytes.extend(std::iter::repeat_n(index, 1024 * 1024));
                    if tx.send(Cursor::new(bytes)).is_err() {
                        break;
                    }
                }
                fs::write(root.join("captured"), "EOF next").expect("EOF handshake");
            } else if case == "early" {
                for _ in 0..16 {
                    if tx.send(Cursor::new(vec![37; 1024 * 1024])).is_err() {
                        break;
                    }
                }
            } else {
                while events.recv().expect("real stdin consumption before EOF") != "consumed" {}
                tx.send(Cursor::new(b"FRAME\nliteral\0\xff".to_vec())).expect("tail");
            }
            drop(tx);
            let result = encode.join().expect("encode joined");
            forward.join().expect("progress joined");
            if case == "capture" {
                assert_eq!(
                    first,
                    Some("capture"),
                    "pass one must wait for complete spool capture, not fan out"
                );
                result.expect("two passes");
                assert_eq!(
                    fs::read_to_string(root.join("order")).expect("order"),
                    "1\n2\n"
                );
                for pass in [1, 2] {
                    use std::io::Read;
                    let mut file =
                        fs::File::open(root.join(format!("pass{pass}.y4m"))).expect("pass bytes");
                    let mut got = vec![0; header.len()];
                    file.read_exact(&mut got).expect("header");
                    assert_eq!(got, header);
                    let mut payload = vec![0; 1024 * 1024];
                    for index in 0..64u8 {
                        let mut control = [0; 10];
                        file.read_exact(&mut control).expect("FRAME control");
                        assert_eq!(&control, b"FRAME tag\n");
                        file.read_exact(&mut payload).expect("payload");
                        assert!(payload.iter().all(|b| *b == index));
                    }
                    assert_eq!(file.read(&mut payload).expect("EOF"), 0);
                }
            } else if case == "early" {
                let error = result.expect_err("failed pass");
                let failed = error
                    .downcast_ref::<EncoderProcessError>()
                    .expect("writer failure with original result");
                assert_eq!(
                    failed.result.as_ref().expect("status").status.code(),
                    Some(42)
                );
                assert!(
                    matches!(&failed.stdout, StringOrBytes::Bytes(b) if b == b"early stdout\xff")
                );
                assert!(
                    matches!(&failed.stderr, StringOrBytes::Bytes(b) if b == b"early stderr\xfe")
                );
                assert_eq!(
                    fs::read_to_string(root.join("order")).expect("order"),
                    "1\n"
                );
            } else {
                result.expect("fully streaming pass");
                assert!(
                    !root.join("flushed").exists(),
                    "single/specific pass must never spool"
                );
                assert_eq!(fs::read(root.join("header")).expect("header"), header);
                assert_eq!(
                    fs::read(root.join("tail")).expect("tail"),
                    b"FRAME\nliteral\0\xff"
                );
            }
            assert!(!fs::read_dir(&root).expect("listing").any(|e| {
                e.expect("entry").file_name().to_string_lossy().starts_with(".av1an-stats-")
            }));
            assert_eq!(
                fs::read_to_string(output).expect("output sentinel"),
                "caller output"
            );
            assert_eq!(
                fs::read_to_string(root.join("scene0.stat")).expect("stats sentinel"),
                "caller stats"
            );
            fs::write(root.join("verified"), "joined and checked").expect("verification handshake");
            eprintln!(
                "spool-fixture {case}: capture ordering / byte equality / streaming handshake / early-failure joins checked"
            );
        }

        use std::{
            io::{self, Read, Seek, SeekFrom},
            os::unix::fs::MetadataExt,
        };

        struct FaultFile {
            file: fs::File,
            fault: String,
            calls: usize,
        }

        impl Write for FaultFile {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                if self.fault == "write" {
                    self.calls += 1;
                    if self.calls > 1 {
                        return Err(io::Error::from_raw_os_error(28));
                    }
                    return self.file.write(&buf[..buf.len().min(7)]);
                }
                self.file.write(buf)
            }
            fn flush(&mut self) -> io::Result<()> {
                if self.fault == "flush" {
                    return Err(io::Error::from_raw_os_error(28));
                }
                self.file.flush()
            }
        }

        impl Read for FaultFile {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                if self.fault == "read" {
                    self.calls += 1;
                    if self.calls > 2 {
                        return Err(io::Error::other("injected spool read failure"));
                    }
                }
                self.file.read(buf)
            }
        }

        impl Seek for FaultFile {
            fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
                if self.fault == "seek" {
                    return Err(io::ErrorKind::InvalidInput.into());
                }
                self.file.seek(pos)
            }
        }

        fn assert_no_file_owner(identity: (u64, u64)) {
            // Compare inode identity, not an FD number that another parallel test could reuse.
            let owners = fs::read_dir("/proc/self/fd")
                .expect("Linux FD listing")
                .filter_map(|entry| fs::metadata(entry.ok()?.path()).ok())
                .filter(|m| (m.dev(), m.ino()) == identity)
                .count();
            assert_eq!(
                owners, 0,
                "all anonymous spool handles must be closed before return"
            );
        }

        fn io_case(root: &Path, case: &str) {
            if case == "bounds" {
                bounded_buffers(root);
                return;
            }
            let encoder = encoder(root, "stream", EncoderPasses::All(2));
            let output = root.join("scene0.ivf");
            fs::write(&output, "caller output").expect("sentinel");
            fs::write(root.join("scene0.stat"), "caller statistics").expect("sentinel");
            if case == "parent" {
                let not_directory = root.join("not-directory");
                fs::write(&not_directory, "caller file").expect("non-directory parent");
                for parent in [root.join("missing"), not_directory.clone(), root.join("readonly")] {
                    if parent.ends_with("readonly") {
                        fs::create_dir_all(&parent).expect("readonly parent");
                        fs::set_permissions(&parent, fs::Permissions::from_mode(0o500))
                            .expect("permissions");
                        // Root/capabilities can bypass chmod; probe the operation preflight uses.
                        match tempfile::tempdir_in(&parent) {
                            Ok(probe) => {
                                probe.close().expect("remove permission probe");
                                fs::set_permissions(&parent, fs::Permissions::from_mode(0o700))
                                    .expect("restore permissions for cleanup");
                                eprintln!(
                                    "spool parent: read-only permissions bypassed; missing and non-directory checks remain enforced"
                                );
                                continue;
                            },
                            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {},
                            Err(error) => {
                                fs::set_permissions(&parent, fs::Permissions::from_mode(0o700))
                                    .expect("restore permissions for cleanup");
                                panic!("unexpected read-only parent probe error: {error}");
                            },
                        }
                    }
                    let (tx, rx) = crossbeam_channel::bounded(1);
                    tx.send(Cursor::new(b"not consumed".to_vec())).expect("input");
                    // A preflight regression must fail an assertion, not wait forever for EOF.
                    drop(tx);
                    let peek = rx.clone();
                    let settings = format!("{encoder:?}");
                    let result = encoder.encode_with_stream(
                        rx,
                        &parent.join("out.ivf"),
                        mpsc::channel().0,
                        &crate::core::encoder::tests::clip_info(),
                    );
                    if parent.ends_with("readonly") {
                        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700))
                            .expect("restore permissions for cleanup");
                    }
                    let error = result.expect_err("bad output parent must fail before input/child");
                    assert!(error.to_string().contains("Creating pass statistics"));
                    assert_eq!(peek.len(), 1, "preflight must not consume input");
                    assert_eq!(settings, format!("{encoder:?}"));
                    if parent.is_dir() {
                        assert_eq!(fs::read_dir(parent).expect("no artifacts").count(), 0);
                    }
                }
                assert_eq!(
                    fs::read_to_string(not_directory).expect("non-directory sentinel"),
                    "caller file"
                );
                assert!(!root.join("order").exists(), "no child on parent error");
                return;
            }
            let stats = PassStatistics::new(&output).expect("owned stats");
            let directory = stats.base().parent().expect("stats parent").to_path_buf();
            let file = spool::create(&directory, &output).expect("actual anonymous spool");
            let meta = file.metadata().expect("file identity");
            assert_eq!(
                meta.nlink(),
                0,
                "production Linux spool is anonymous/unlinked"
            );
            let identity = (meta.dev(), meta.ino());
            let (tx, rx) = crossbeam_channel::bounded(2);
            let producer = thread::spawn(move || {
                tx.send(Cursor::new(
                    b"YUV4MPEG2 W64 H64 F24:1 Ip A0:0 C420\n".to_vec(),
                ))
                .expect("header");
                for _ in 0..16 {
                    if tx.send(Cursor::new(vec![37; 1024 * 1024])).is_err() {
                        break;
                    }
                }
            });
            let mut launched = 0;
            let error = spool::encode(
                FaultFile {
                    file,
                    fault: case.to_owned(),
                    calls: 0,
                },
                rx,
                &output,
                2,
                |pass, rx| {
                    launched += 1;
                    if case == "drop" {
                        drop(rx);
                        anyhow::bail!("local consumer cancelled");
                    }
                    encoder.encode_pass_with_stream(
                        (pass, 2),
                        rx,
                        &output,
                        mpsc::channel().0,
                        &crate::core::encoder::tests::clip_info(),
                        Some(&stats),
                    )
                },
            )
            .expect_err("I/O faults cannot produce usable output");
            producer.join().expect("bounded input producer joined on capture failure too");
            assert_eq!(launched, usize::from(matches!(case, "read" | "drop")));
            if case == "drop" {
                assert_eq!(
                    error.to_string(),
                    "local consumer cancelled",
                    "do not mask original consumer failure with SendError"
                );
                // Named-file ownership also cleans up when the consumer disconnects.
                let named =
                    tempfile::NamedTempFile::new_in(&directory).expect("named ownership fixture");
                let path = named.path().to_path_buf();
                let (tx, rx) = crossbeam_channel::bounded(1);
                tx.send(Cursor::new(vec![1; 1024 * 1024])).expect("input");
                drop(tx);
                let error = spool::encode(named, rx, &output, 2, |_, rx| -> Result<()> {
                    drop(rx);
                    anyhow::bail!("cancelled")
                })
                .expect_err("consumer drop");
                assert_eq!(error.to_string(), "cancelled");
                assert!(
                    !path.exists(),
                    "named spool removed after joined replay consumer failure"
                );
                // Local owner drop requires no new force-cancellation API.
                let local = spool::create(&directory, &output).expect("local owner");
                let m = local.metadata().expect("identity");
                drop(local);
                assert_no_file_owner((m.dev(), m.ino()));
            } else {
                let operation = match case {
                    "write" => "Writing",
                    "flush" => "Flushing",
                    "seek" => "Seeking",
                    _ => "Reading",
                };
                let message = error.to_string();
                assert!(
                    message.contains(operation)
                        && message.contains("Y4M replay spool")
                        && message.contains("scene0.ivf"),
                    "{error:#}"
                );
                let io = error.downcast_ref::<io::Error>().expect("typed original I/O failure");
                if matches!(case, "write" | "flush") {
                    assert_eq!(io.raw_os_error(), Some(28));
                } else {
                    assert_eq!(
                        io.kind(),
                        if case == "seek" {
                            io::ErrorKind::InvalidInput
                        } else {
                            io::ErrorKind::Other
                        }
                    );
                }
            }
            assert_no_file_owner(identity);
            drop(stats);
            assert!(
                !directory.exists(),
                "private attempt area removed on failure"
            );
            if case == "read" {
                assert_eq!(
                    fs::read_to_string(root.join("order")).expect("one child"),
                    "1\n"
                );
            } else {
                assert!(
                    !root.join("order").exists(),
                    "capture/seek failure cannot launch an encoder"
                );
            }
            assert_eq!(
                fs::read_to_string(&output).expect("sentinel"),
                "caller output"
            );
            assert_eq!(
                fs::read_to_string(root.join("scene0.stat")).expect("sentinel"),
                "caller statistics"
            );
            eprintln!(
                "spool-fixture {case}: typed/contextual failure, launched={launched}, producer joined, zero anonymous FD owners, owned directory removed, sentinels unchanged"
            );
        }

        fn bounded_buffers(root: &Path) {
            struct CaptureFile {
                file: fs::File,
                gate: Option<mpsc::Receiver<()>>,
                entered: mpsc::Sender<()>,
                completed: Arc<AtomicU64>,
            }
            impl Write for CaptureFile {
                fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                    if let Some(gate) = self.gate.take() {
                        self.entered.send(()).expect("capture entered");
                        gate.recv().expect("capture released");
                    }
                    self.file.write_all(bytes)?;
                    self.completed.fetch_add(1, Ordering::SeqCst);
                    Ok(bytes.len())
                }
                fn flush(&mut self) -> io::Result<()> {
                    self.file.flush()
                }
            }
            impl Read for CaptureFile {
                fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
                    self.file.read(bytes)
                }
            }
            impl Seek for CaptureFile {
                fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
                    self.file.seek(pos)
                }
            }
            let made = Arc::new(AtomicU64::new(0));
            let completed = Arc::new(AtomicU64::new(0));
            let peak = Arc::new(AtomicU64::new(0));
            let (tx, rx) = crossbeam_channel::bounded(2);
            let peek = rx.clone();
            let (allocated, allocations) = mpsc::channel();
            let (made_by_producer, done_by_producer, producer_peak) =
                (Arc::clone(&made), Arc::clone(&completed), Arc::clone(&peak));
            let producer = thread::spawn(move || {
                for _ in 0..64 {
                    let bytes = vec![37; 1024 * 1024];
                    let count = made_by_producer.fetch_add(1, Ordering::SeqCst) + 1;
                    producer_peak.fetch_max(
                        count - done_by_producer.load(Ordering::SeqCst),
                        Ordering::SeqCst,
                    );
                    allocated.send(()).expect("allocation observed");
                    tx.send(Cursor::new(bytes)).expect("bounded capture");
                }
            });
            let (entered, capture_entered) = mpsc::channel();
            let (release, gate) = mpsc::channel();
            let output = root.join("out.ivf");
            let file = spool::create(root, &output).expect("spool");
            let identity = file.metadata().expect("spool identity");
            let file = CaptureFile {
                file,
                gate: Some(gate),
                entered,
                completed: Arc::clone(&completed),
            };
            let captured = Arc::clone(&completed);
            let capture = thread::spawn(move || {
                spool::encode(file, rx, &output, 2, |_, rx| {
                    assert_eq!(
                        captured.load(Ordering::SeqCst),
                        64,
                        "all messages written before replay"
                    );
                    let mut bytes = 0;
                    for chunk in rx {
                        assert!(chunk.get_ref().len() <= 64 * 1024);
                        assert!(chunk.get_ref().iter().all(|b| *b == 37));
                        bytes += chunk.get_ref().len();
                    }
                    assert_eq!(bytes, 64 * 1024 * 1024);
                    Ok(())
                })
                .expect("bounded capture and two replays");
            });
            capture_entered.recv().expect("first write blocked");
            for _ in 0..4 {
                allocations.recv().expect("actual allocated frame");
            }
            assert_eq!(made.load(Ordering::SeqCst), 4);
            assert_eq!(completed.load(Ordering::SeqCst), 0);
            assert_eq!(
                peek.len(),
                2,
                "two queued + one capture + one blocked caller allocation"
            );
            release.send(()).expect("release write");
            producer.join().expect("producer joined");
            capture.join().expect("capture/replay joined");
            assert_no_file_owner((identity.dev(), identity.ino()));
            assert_eq!(made.load(Ordering::SeqCst), 64);
            assert_eq!(completed.load(Ordering::SeqCst), 64);
            assert_eq!(
                peak.load(Ordering::SeqCst),
                4,
                "outstanding frame allocations are bounded independently of stream length"
            );
            drop(peek);

            struct ReplayFile {
                file: fs::File,
                reads: mpsc::Sender<()>,
                count: Arc<AtomicU64>,
                interrupted: bool,
            }
            impl Read for ReplayFile {
                fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
                    if self.interrupted {
                        self.interrupted = false;
                        return Err(io::ErrorKind::Interrupted.into());
                    }
                    let n = self.file.read(bytes)?;
                    if n != 0 {
                        self.count.fetch_add(1, Ordering::SeqCst);
                        self.reads.send(()).expect("read observed");
                    }
                    Ok(n)
                }
            }
            impl Seek for ReplayFile {
                fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
                    self.file.seek(pos)
                }
            }
            let mut file = spool::create(root, &root.join("replay.ivf")).expect("spool");
            for _ in 0..32 {
                file.write_all(&vec![37; 64 * 1024]).expect("finite spool");
            }
            let (reads, observed) = mpsc::channel();
            let count = Arc::new(AtomicU64::new(0));
            let mut file = ReplayFile {
                file,
                reads,
                count: Arc::clone(&count),
                interrupted: true,
            };
            spool::replay(&mut file, &root.join("replay.ivf"), |rx| {
                let first = rx.recv().expect("first writer-held chunk");
                assert_eq!(
                    rx.capacity(),
                    Some(2),
                    "replay must be bounded even while the consumer is stopped"
                );
                for _ in 0..4 {
                    observed.recv().expect("four actual read buffers");
                }
                assert_eq!(
                    count.load(Ordering::SeqCst),
                    4,
                    "producer cannot read a fifth buffer while send is blocked"
                );
                assert_eq!(rx.len(), 2);
                assert_eq!(first.get_ref().len(), 64 * 1024);
                drop(first);
                let mut chunks = 1;
                for chunk in rx {
                    assert_eq!(chunk.get_ref().len(), 64 * 1024);
                    assert!(chunk.get_ref().iter().all(|b| *b == 37));
                    chunks += 1;
                }
                assert_eq!(chunks, 32);
                Ok(())
            })
            .expect("replay joins");
            assert_eq!(count.load(Ordering::SeqCst), 32);
            eprintln!(
                "spool-fixture bounds: capture=64 x 1MiB lazy messages, peak outstanding=4 (caller2 + capture1 + producer1); replay=32 x 64KiB, blocked outstanding=4 (queue2 + writer1 + producer1), total replay bytes=2MiB; both 64MiB passes exact"
            );
        }
    }
}
