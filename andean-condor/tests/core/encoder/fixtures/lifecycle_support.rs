//! Test-only lifecycle fixtures shared by public API and private-hook contracts.
use super::*;

pub(super) fn install_launchers(root: &Path) {
    for name in ["ffmpeg", "mkvmerge"] {
        std::os::unix::fs::symlink(root.join("fake"), root.join(name)).expect("fixture launcher");
    }
    let path = std::env::join_paths(std::iter::once(root.to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").expect("PATH")),
    ))
    .expect("PATH");
    // SAFETY: This isolated child has not started controller or relay threads yet.
    unsafe {
        std::env::set_var("PATH", path);
    }
}

pub(super) fn assert_scene_case(root: &Path, case: &str, cancellation: Cancellation) {
    let root2 = root.to_path_buf();
    let signal = cancellation.clone();
    let kind = case.to_owned();
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
            if ["mid-production", "stream-idle", "serial", "serial-native"].contains(&kind.as_str())
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
                // SAFETY: Observe only this still-owned direct child without reaping it.
                let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
                // SAFETY: The known child and valid siginfo are observed using WNOWAIT.
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
                // SAFETY: Only this supervisor-owned group is signalled; managed children lead other groups.
                assert_eq!(unsafe { libc::kill(0, libc::SIGINT) }, 0);
            }
            fs::write(root2.join("release"), "finish active scene").expect("release");
        } else {
            signal.request().expect("force");
            if kind == "mid-production" {
                fs::write(root2.join("release-frame"), "forced").expect("release finite callback");
            }
        }
    });
    let mut filter_spawn_failed = false;
    let result: Result<()> = if case.starts_with("filter") {
        let (_tx, rx) = crossbeam_channel::bounded(1);
        match FfmpegFilter::spawn(
            rx,
            1,
            "null",
            Arc::new(std::sync::Mutex::new(None)),
            &cancellation,
        ) {
            Err(error) => {
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
        DGSource::index_video(
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
            SceneConcatenator::ffmpeg(
                root,
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
            SceneConcatenator::mkvmerge(
                root,
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
        .contains(&case)
    {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let full = case == "stdin-full" || case == "replay-full";
        let _keep_alive = (!full).then(|| tx.clone());
        let producer = full.then(|| {
            let replay = case == "replay-full";
            let root = root.to_path_buf();
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
        let result = encoder(root, case == "capture" || case == "replay-full")
            .encode_with_stream(
                rx,
                &root.join("out.264"),
                progress,
                &super::clip_info(),
                &cancellation,
            )
            .map(|_| ());
        if let Some(producer) = producer {
            producer.join().expect("bounded producer joined");
        }
        result
    } else {
        let mut input = input(root, case.ends_with("native"));
        let encoder = encoder(root, case.starts_with("graceful"));
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
        config.parallel_encoder.scenes_directory = root.to_path_buf();
        config.parallel_encoder.workers = Some(1);
        config.parallel_encoder.max_tries = 1; // Registration handshakes require direct streaming.
        config.parallel_encoder.validate_frame_count = false; // Ownership markers are not bitstreams.
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
            let plan = ZonePlan::try_build(&tasks, &cancellation);
            plan.and_then(|plan| {
                ZoneEncoder::encode_tasks(
                    &mut input,
                    tasks.into(),
                    plan,
                    progress,
                    cancellation,
                    true,
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
                SerialEncoder::new(root).execute(&mut condor, progress, cancellation)
            } else {
                ParallelEncoder::default().execute(&mut condor, progress, cancellation)
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
        .contains(&case)
        {
            assert!(
                error.chain().any(|cause| cause.is::<EncoderResult>()),
                "forced encoder diagnostics/status must remain typed: {error:#}"
            );
        }
        if ["mid-production", "stdin-full", "serial", "serial-native"].contains(&case) {
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
    reap(root);
    if case == "filter-spawn-race" {
        assert!(
            filter_spawn_failed,
            "force won construction, not stage.check()"
        );
    }
}

pub(super) fn prepare_case() -> (PathBuf, String, Cancellation) {
    let root = PathBuf::from(std::env::var_os("PROCESS_LIFECYCLE_ROOT").expect("fixture root"));
    let case = std::env::var("PROCESS_LIFECYCLE_CASE").expect("case");
    fs::write(root.join("selected"), "yes").expect("handshake");
    fs::write(root.join("sentinel"), "caller-owned").expect("sentinel");
    if case == "encoder-existing" {
        fs::write(root.join("out.264"), "caller-owned").expect("existing output");
    }
    if case == "ffmpeg-existing" {
        fs::write(root.join("out.mkv"), "caller-owned").expect("existing output");
    }
    // SAFETY: This supervisor-owned child adopts only fixture grandchildren and ignores terminal SIGINT.
    unsafe {
        assert_eq!(libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0), 0);
        libc::signal(libc::SIGINT, libc::SIG_IGN);
    }
    (root, case, Cancellation::default())
}

pub(super) fn wait_file(root: &Path, name: &str) {
    while !root.join(name).exists() {
        thread::yield_now();
    }
}

pub(super) fn input(root: &Path, native: bool) -> Input {
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
        let script = if std::env::var("PROCESS_LIFECYCLE_CASE").as_deref() == Ok("mid-production") {
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

pub(super) fn encoder(root: &Path, multipass: bool) -> Encoder {
    let mut encoder = Encoder::default_from_base(&EncoderBase::X264, true);
    encoder.set_executable(Some(root.join("fake")));
    encoder.set_append_color_data(false);
    encoder.set_quantizer(23.0);
    if multipass {
        *encoder.passes_mut().expect("passes") = EncoderPasses::All(2);
    }
    encoder
}

pub(super) fn reap(root: &Path) {
    if let Ok(pid) = fs::read_to_string(root.join("grandchild")) {
        let pid: i32 = pid.parse().expect("grandchild PID");
        let mut status = 0;
        // SAFETY: Reap only this known adopted child. Its lifetime exceeds the supervisor bound.
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
        // SAFETY: Signal zero observes this fixture PID without killing anything.
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
