//! Admission must reject the whole requested list before any mux side effects.
use super::*;

fn direct(
    backend: &str,
    root: &Path,
    output: &Path,
    paths: &[PathBuf],
    token: &Cancellation,
    compatibility: bool,
) -> Result<()> {
    let (tx, _rx) = sync::mpsc::channel();
    let source = root.join("source");
    match (backend, compatibility) {
        ("ivf", false) => SceneConcatenator::ivf_cancel(output, paths, &tx, token),
        ("ivf", true) => SceneConcatenator::ivf(output, paths, &tx, &token.legacy_flag()),
        ("ffmpeg", false) => SceneConcatenator::ffmpeg_cancel(
            root,
            output,
            paths,
            Some(&source),
            paths.len() * 2,
            Ratio::new(24, 1),
            Some(&FfmpegConfig::default()),
            &tx,
            token,
        ),
        ("ffmpeg", true) => SceneConcatenator::ffmpeg(
            root,
            output,
            paths,
            Some(&source),
            paths.len() * 2,
            Ratio::new(24, 1),
            Some(&FfmpegConfig::default()),
            &tx,
            &token.legacy_flag(),
        ),
        ("mkvmerge", false) => SceneConcatenator::mkvmerge_cancel(
            root,
            output,
            paths,
            Some(&source),
            Ratio::new(24, 1),
            None,
            &tx,
            token,
        ),
        ("mkvmerge", true) => SceneConcatenator::mkvmerge(
            root,
            output,
            paths,
            Some(&source),
            Ratio::new(24, 1),
            None,
            &tx,
            &token.legacy_flag(),
        ),
        _ => unreachable!("fixture backend"),
    }
}

#[cfg(unix)]
#[test]
fn direct_invalid_lists_preserve_every_sentinel() {
    use std::{
        os::unix::fs::PermissionsExt,
        time::{Duration, Instant},
    };
    let python = which::which("python3").expect("required fixture tool");
    let mut failures = Vec::new();
    for backend in ["ivf", "ffmpeg", "mkvmerge"] {
        for case in ["empty-list", "missing-middle", "empty-file", "directory", "unreadable"] {
            for existing in [false, true] {
                for compatibility in [false, true] {
                    let dir = tempfile::tempdir().expect("isolated fixture");
                    let bin = dir.path().join("bin");
                    std::fs::create_dir_all(&bin).expect("private PATH");
                    for tool in ["ffprobe", "ffmpeg", "mkvmerge"] {
                        let script = bin.join(tool);
                        std::fs::write(&script, format!(
                            "#!{}\nimport os,sys\nfrom pathlib import Path\nPath(os.environ['SCENE_LIST_ROOT'],'tool-invoked').write_text(sys.argv[0])\nsys.exit(88)\n",
                            python.display()
                        )).expect("finite tool trap");
                        std::fs::set_permissions(script, std::fs::Permissions::from_mode(0o700))
                            .expect("executable");
                    }
                    let log_path = dir.path().join("child.log");
                    let log = File::create(&log_path).expect("child log");
                    let mut command = Command::new(std::env::current_exe().expect("test binary"));
                    command.args(["--exact", "core::sequence::scene_concatenator::tests::scene_list::scene_list_child", "--nocapture"])
                        .env("SCENE_LIST_ROOT", dir.path()).env("SCENE_LIST_BACKEND", backend).env("SCENE_LIST_CASE", case)
                        .env("SCENE_LIST_EXISTING", existing.to_string()).env("SCENE_LIST_COMPAT", compatibility.to_string())
                        .env("PATH", &bin).stdin(Stdio::null())
                        .stdout(Stdio::from(log.try_clone().expect("log clone"))).stderr(Stdio::from(log));
                    let mut child = OwnedProcess::spawn(&mut command, &Cancellation::default())
                        .expect("owned isolated test");
                    let deadline = Instant::now() + Duration::from_secs(20);
                    let mut timed_out = false;
                    let status = loop {
                        if let Some(status) = child.try_wait().expect("poll child") {
                            break status;
                        }
                        if Instant::now() >= deadline {
                            timed_out = true;
                            child.kill().expect("kill only owned test tree");
                            break child.wait().expect("reap owned test");
                        }
                        thread::park_timeout(Duration::from_millis(10));
                    };
                    let log = std::fs::read_to_string(log_path).expect("diagnostics");
                    let label =
                        format!("{backend}/{case}/existing={existing}/compat={compatibility}");
                    if timed_out || !status.success() || !log.contains("1 passed;") {
                        failures.push(format!("{label}: timeout={timed_out}\n{log}"));
                    }
                    dir.close().expect("fixture cleanup");
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[cfg(unix)]
#[test]
fn scene_list_child() {
    use std::os::unix::fs::PermissionsExt;
    let Some(root) = std::env::var_os("SCENE_LIST_ROOT").map(PathBuf::from) else {
        return;
    };
    let backend = std::env::var("SCENE_LIST_BACKEND").expect("backend");
    let case = std::env::var("SCENE_LIST_CASE").expect("case");
    let existing = std::env::var("SCENE_LIST_EXISTING").expect("existing") == "true";
    let compatibility = std::env::var("SCENE_LIST_COMPAT").expect("compat") == "true";
    let scratch = SceneConcatenator::scratch_directory(&root);
    std::fs::create_dir_all(&scratch).expect("scratch");
    let sentinels = [
        (root.join("source"), b"source sentinel".as_slice()),
        (scratch.join("concat.txt"), b"concat sentinel"),
        (scratch.join("options.json"), b"options sentinel"),
        (scratch.join("source.mkv"), b"caller scratch sentinel"),
    ];
    for (path, bytes) in &sentinels {
        std::fs::write(path, bytes).expect("sentinel");
    }
    let output = root.join("out");
    if existing {
        std::fs::write(&output, b"old output sentinel").expect("old output");
    }
    let mut paths = (0..3).map(|i| root.join(format!("scene-{i}"))).collect::<Vec<_>>();
    let ivf = b"DKIF\0\0\x20\0AV01\x08\0\x08\0\x18\0\0\0\x01\0\0\0\x01\0\0\0\0\0\0\0";
    for path in &paths {
        std::fs::write(path, ivf).expect("owned nonempty input");
    }
    match case.as_str() {
        "empty-list" => paths.clear(),
        "missing-middle" => std::fs::remove_file(&paths[1]).expect("missing middle"),
        "empty-file" => std::fs::write(&paths[1], b"").expect("empty middle"),
        "directory" => {
            std::fs::remove_file(&paths[1]).expect("replace middle");
            std::fs::create_dir_all(&paths[1]).expect("directory middle");
        },
        "unreadable" => std::fs::set_permissions(&paths[1], std::fs::Permissions::from_mode(0o0))
            .expect("unreadable middle"),
        _ => unreachable!("fixture case"),
    }
    let run_backend = if case == "unreadable" {
        match File::open(&paths[1]) {
            Ok(_) => {
                eprintln!(
                    "scene-list permission denial unavailable: mode-000 fixture is readable; backend not invoked"
                );
                false
            },
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => true,
            Err(error) => {
                std::fs::set_permissions(&paths[1], std::fs::Permissions::from_mode(0o600))
                    .expect("restore owned fixture permissions");
                panic!("Unexpected permission fixture probe error: {error}");
            },
        }
    } else {
        true
    };
    // The catch exposes the old IVF panic; a panic is never an acceptable refusal.
    let result = run_backend.then(|| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            direct(
                &backend,
                &root,
                &output,
                &paths,
                &Cancellation::default(),
                compatibility,
            )
        }))
    });
    if case == "unreadable" {
        std::fs::set_permissions(&paths[1], std::fs::Permissions::from_mode(0o600))
            .expect("restore owned fixture permissions");
    }
    if existing {
        assert_eq!(
            std::fs::read(&output).expect("output intact"),
            b"old output sentinel"
        );
    } else {
        assert!(!output.exists(), "invalid list must not create output");
    }
    for (path, bytes) in sentinels {
        assert_eq!(std::fs::read(path).expect("sentinel intact"), bytes);
    }
    assert!(
        !root.join("tool-invoked").exists(),
        "invalid list reached a native tool"
    );
    assert_eq!(
        std::fs::read_dir(&scratch).expect("scratch listing").count(),
        3
    );
    for (i, path) in paths.iter().enumerate().filter(|(i, _)| *i != 1) {
        assert_eq!(
            std::fs::read(path).expect("input preserved"),
            ivf,
            "scene {i}"
        );
    }
    let Some(result) = result else {
        assert_eq!(
            std::fs::read(&paths[1]).expect("readable input preserved"),
            ivf
        );
        return;
    };
    let error = result
        .expect("invalid list must not panic")
        .expect_err("invalid list must fail");
    let text = format!("{error:#}");
    if case == "empty-list" {
        assert!(text.contains("scene"), "{text}");
    } else {
        assert!(
            text.contains("scene 1") && text.contains(&paths[1].display().to_string()),
            "{text}"
        );
        match case.as_str() {
            "missing-middle" => assert_eq!(
                error.downcast_ref::<std::io::Error>().expect("original FS cause").kind(),
                std::io::ErrorKind::NotFound
            ),
            "unreadable" => assert_eq!(
                error.downcast_ref::<std::io::Error>().expect("original FS cause").kind(),
                std::io::ErrorKind::PermissionDenied
            ),
            _ => assert!(text.contains("nonempty regular file"), "{text}"),
        }
    }
}

pub(super) fn job(
    root: &Path,
    output: &Path,
    count: usize,
) -> Condor<DefaultSequenceData, DefaultSequenceConfig> {
    let mut job = concat_job(root, output);
    job.input = Input::from_data(&InputModel::VapourSynthScript {
        source: VapourSynthScriptSource::Text("import vapoursynth as vs\nvs.core.std.BlankClip(width=32,height=16,length=6,format=vs.YUV420P8).set_output()".into()),
        variables: Default::default(), index: 0, filters: vec![], stream_concurrently: false,
    }).expect("finite native input");
    job.sequence_config.parallel_encoder.scenes_directory = root.into();
    job.scenes = (0..count)
        .map(|index| Scene {
            start_frame: index * 2,
            end_frame: index * 2 + 2,
            sub_scenes: None,
            encoder: Default::default(),
            sequence_data: Default::default(),
        })
        .collect();
    job
}

#[test]
fn sequence_current_context_malformed_ivf_never_completes_or_replaces_final() {
    let dir = tempfile::tempdir().expect("Sequence framing fixture");
    let output = dir.path().join("final.ivf");
    std::fs::write(&output, b"old final sentinel").expect("old final");
    let mut job = job(dir.path(), &output, 1);
    let chunk = dir.path().join("00000.ivf");
    let mut bytes = b"DKIF\0\0\x20\0AV01\x20\0\x10\0\x18\0\0\0\x01\0\0\0\0\0\0\0\0\0\0\0".to_vec();
    bytes.extend_from_slice(&[1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xAB]);
    bytes.extend_from_slice(&[1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0]); // Missing second payload.
    std::fs::write(&chunk, &bytes).expect("malformed framing");
    // Synthetic current C14 stamp only: not a claim that a foreign encoder produced valid media.
    crate::core::sequence::tests::support::current_encoding_fixture(&mut job);
    let (tx, rx) = sync::mpsc::channel();
    let mut concat = SceneConcatenator::default();
    concat.validate(&mut job).expect("final admission");
    concat.initialize(&mut job, tx.clone()).expect("readable requested list");
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        concat.execute_cancel(&mut job, tx, Cancellation::default())
    }));
    let error = result
        .expect("owning Sequence must not panic")
        .expect_err("malformed IVF must reach caller");
    let text = format!("{error:#}");
    assert!(
        text.contains(&chunk.display().to_string()) && text.contains("record 1"),
        "{text}"
    );
    assert_eq!(
        error
            .downcast_ref::<std::io::Error>()
            .expect("original framing I/O cause")
            .kind(),
        std::io::ErrorKind::UnexpectedEof
    );
    assert!(
        !rx.try_iter()
            .any(|s| matches!(s, SequenceStatus::Whole(Status::Completed { .. })))
    );
    assert_eq!(
        std::fs::read(&output).expect("final preserved"),
        b"old final sentinel"
    );
    assert_eq!(std::fs::read(&chunk).expect("input preserved"), bytes);
    assert!(
        !std::fs::read_dir(dir.path()).expect("staging cleanup").any(|e| e
            .expect("entry")
            .file_name()
            .to_string_lossy()
            .starts_with(".condor-output-"))
    );
}

#[test]
fn sequence_initialization_and_bypassed_execution_reject_incomplete_lists() {
    let mut failures = Vec::new();
    for count in [0, 3] {
        let dir = tempfile::tempdir().expect("fixture");
        let output = dir.path().join("final.ivf");
        std::fs::write(&output, b"old final sentinel").expect("old final");
        let mut job = job(dir.path(), &output, count);
        for index in 0..3 {
            std::fs::write(
                dir.path().join(format!("{index:05}.ivf")),
                b"owned scene sentinel",
            )
            .expect("nonempty scene");
        }
        crate::core::sequence::tests::support::current_encoding_fixture(&mut job);
        std::fs::remove_file(dir.path().join("00001.ivf"))
            .expect("missing middle after certification");
        let (tx, rx) = sync::mpsc::channel();
        let mut concat = SceneConcatenator::default();
        if concat.initialize(&mut job, tx.clone()).is_ok() {
            failures.push(format!(
                "count={count}: initialize accepted incomplete list"
            ));
        }
        if SceneConcatenator::scratch_directory(dir.path()).exists() {
            failures.push(format!("count={count}: initialize created scratch"));
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            concat.execute_cancel(&mut job, tx, Cancellation::default())
        }));
        if !matches!(result, Ok(Err(_))) {
            failures.push(format!("count={count}: execute did not return an error"));
        }
        assert_eq!(
            std::fs::read(output).expect("final intact"),
            b"old final sentinel"
        );
        assert!(
            !rx.try_iter()
                .any(|s| matches!(s, SequenceStatus::Whole(Status::Completed { .. })))
        );
        assert!(!std::fs::read_dir(dir.path()).expect("listing").any(|e| {
            e.expect("entry").file_name().to_string_lossy().starts_with(".condor-output-")
        }));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn cancellation_still_precedes_list_admission() {
    use crate::utils::cancellation::Cancelled;
    let dir = tempfile::tempdir().expect("fixture");
    for force in [false, true] {
        let token = Cancellation::default();
        token.request().expect("graceful");
        if force {
            token.request().expect("force");
        }
        let expected = if force {
            Cancelled::Forced
        } else {
            Cancelled::Graceful
        };
        for backend in ["ivf", "ffmpeg", "mkvmerge"] {
            let error = direct(
                backend,
                dir.path(),
                &dir.path().join("out"),
                &[],
                &token,
                false,
            )
            .expect_err("cancel before invalid list");
            assert_eq!(error.downcast_ref::<Cancelled>(), Some(&expected));
        }
        let mut job = job(dir.path(), &dir.path().join("final"), 0);
        let (tx, _rx) = sync::mpsc::channel();
        let error = SceneConcatenator::default()
            .execute_cancel(&mut job, tx, token)
            .expect_err("Sequence cancellation first");
        assert_eq!(error.downcast_ref::<Cancelled>(), Some(&expected));
    }
    assert_eq!(
        std::fs::read_dir(dir.path()).expect("no side effects").count(),
        0
    );
}

fn native(command: &mut Command) -> Vec<u8> {
    let output = command.output().expect("required native fixture tool");
    assert!(
        output.status.success(),
        "{command:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn decoded(path: &Path) -> Vec<u8> {
    native(
        Command::new("ffmpeg").args(["-v", "error", "-i"]).arg(path).args([
            "-f",
            "rawvideo",
            "-pix_fmt",
            "yuv420p",
            "-fps_mode",
            "passthrough",
            "-",
        ]),
    )
}

#[test]
pub(super) fn native_complete_lists_keep_exact_order_count_and() {
    let dir = tempfile::tempdir().expect("native fixture");
    let mut job = job(dir.path(), &dir.path().join("final.ivf"), 3);
    let paths = (0..3).map(|i| dir.path().join(format!("{i:05}.ivf"))).collect::<Vec<_>>();
    for (path, color) in paths.iter().zip(["white", "black", "red"]) {
        native(
            Command::new("ffmpeg")
                .args(["-v", "error", "-f", "lavfi", "-i"])
                .arg(format!("color={color}:size=32x16:rate=24"))
                .args([
                    "-frames:v",
                    "2",
                    "-c:v",
                    "libaom-av1",
                    "-cpu-used",
                    "8",
                    "-crf",
                    "0",
                    "-g",
                    "1",
                ])
                .arg(path),
        );
    }
    let originals = paths
        .iter()
        .map(|p| std::fs::read(p).expect("native scene bytes"))
        .collect::<Vec<_>>();
    let frames = paths.iter().map(|p| decoded(p)).collect::<Vec<_>>();
    assert!(frames.iter().all(|f| f.len() == 32 * 16 * 3 / 2 * 2));
    assert!(
        frames.windows(2).all(|pair| pair[0] != pair[1]),
        "visibly distinct native scenes"
    );
    let scratch = SceneConcatenator::scratch_directory(dir.path());
    std::fs::create_dir_all(&scratch).expect("caller scratch");
    std::fs::write(scratch.join("sentinel"), b"caller scratch sentinel").expect("sentinel");
    std::fs::write(scratch.join("source.mkv"), b"caller fixed scratch sentinel").expect("sentinel");
    let protected = dir.path().join("source-config.vpy");
    std::fs::write(&protected, b"declared source config sentinel").expect("source config");
    job.sequence_config.scene_detector.input = Some(InputModel::VapourSynthScript {
        source: VapourSynthScriptSource::Path(protected.clone()),
        variables: Default::default(),
        index: 0,
        filters: vec![],
        stream_concurrently: false,
    });
    let (tx, _rx) = sync::mpsc::channel();
    // Direct order is caller-owned, including repeated paths, not sorted/deduplicated.
    let requested = [paths[2].clone(), paths[0].clone(), paths[1].clone(), paths[2].clone()];
    let expected = [frames[2].as_slice(), &frames[0], &frames[1], &frames[2]].concat();
    for (index, method) in [
        ConcatMethod::Ivf,
        ConcatMethod::FFmpeg(FfmpegConfig::default()),
        ConcatMethod::MKVMerge(MkvmergeConfig::default()),
    ]
    .into_iter()
    .enumerate()
    {
        let output = dir.path().join(format!("direct-{index}.{}", method.extension()));
        match &method {
            ConcatMethod::Ivf => {
                SceneConcatenator::ivf_cancel(&output, &requested, &tx, &Cancellation::default())
            },
            ConcatMethod::FFmpeg(_) => SceneConcatenator::ffmpeg_cancel(
                dir.path(),
                &output,
                &requested,
                None,
                8,
                Ratio::new(24, 1),
                None,
                &tx,
                &Cancellation::default(),
            ),
            ConcatMethod::MKVMerge(_) => SceneConcatenator::mkvmerge_cancel(
                dir.path(),
                &output,
                &requested,
                None,
                Ratio::new(24, 1),
                None,
                &tx,
                &Cancellation::default(),
            ),
        }
        .expect("complete direct concat");
        assert_eq!(
            decoded(&output),
            expected,
            "direct {} order/count",
            method.extension()
        );
        job.sequence_config.scene_concatenator.method = method;
        let final_path = dir.path().join(format!(
            "final-{index}.{}",
            job.sequence_config.scene_concatenator.method.extension()
        ));
        job.output.path = dir.path().join(format!("unused-fallback-{index}.mkv"));
        job.sequence_config.scene_concatenator.output = Some(final_path.clone());
        let (tx, rx) = sync::mpsc::channel();
        let rx = Arc::new(Mutex::new(rx));
        let mut concat = SceneConcatenator::default();
        concat.initialize(&mut job, tx.clone()).expect("complete initialization");
        // Shape admission is not saved-job eligibility. No stamp means no mux or Completed.
        for scene in &mut job.scenes {
            scene.sequence_data.parallel_encoder.provenance = None;
        }
        assert!(concat.execute_cancel(&mut job, tx.clone(), Cancellation::default()).is_err());
        assert!(!final_path.exists());
        assert!(
            !rx.lock()
                .expect("statuses")
                .try_iter()
                .any(|s| matches!(s, SequenceStatus::Whole(Status::Completed { .. })))
        );
        crate::core::sequence::tests::support::current_encoding_fixture(&mut job);
        // Real native chunks, synthetic current-context C14 stamp, not actual PE producer proof.
        let old = (index != 0).then(|| std::fs::read(&output).expect("distinct valid old mux"));
        if let Some(old) = &old {
            std::fs::write(&final_path, old).expect("old valid output with explicit consent");
        }
        let saved_config = serde_json::to_value(&job.sequence_config).expect("config snapshot");
        let destination = final_path.clone();
        let statuses = Arc::clone(&rx);
        let expected = frames.concat();
        let reached = Arc::new(AtomicBool::new(false));
        let observed = Arc::clone(&reached);
        publication::observe_commit(move |stage| {
            assert_eq!(
                std::fs::read(&destination).ok(),
                old,
                "no final output before commit"
            );
            assert_eq!(
                decoded(stage),
                expected,
                "complete native stage before publication"
            );
            assert!(
                !statuses
                    .lock()
                    .expect("statuses")
                    .try_iter()
                    .any(|s| matches!(s, SequenceStatus::Whole(Status::Completed { .. })))
            );
            observed.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        concat
            .execute_cancel(&mut job, tx, Cancellation::default())
            .expect("current saved-job-admission fixture admitted");
        assert_eq!(
            decoded(&final_path),
            frames.concat(),
            "Sequence order/count"
        );
        assert_eq!(
            rx.lock()
                .expect("statuses")
                .try_iter()
                .filter(|s| matches!(s, SequenceStatus::Whole(Status::Completed { .. })))
                .count(),
            1
        );
        assert!(reached.load(std::sync::atomic::Ordering::SeqCst));
        assert!(!job.output.path.exists(), "effective output override");
        assert_eq!(
            serde_json::to_value(&job.sequence_config).expect("config"),
            saved_config
        );
        publication::assert_clean(dir.path());
    }
    for (path, bytes) in paths.iter().zip(originals) {
        assert_eq!(std::fs::read(path).expect("input intact"), bytes);
    }
    assert_eq!(
        std::fs::read(scratch.join("sentinel")).expect("caller scratch intact"),
        b"caller scratch sentinel"
    );
    assert_eq!(
        std::fs::read(scratch.join("source.mkv")).expect("fixed scratch intact"),
        b"caller fixed scratch sentinel"
    );
    assert_eq!(
        std::fs::read(protected).expect("declared source intact"),
        b"declared source config sentinel"
    );
    dir.close().expect("native fixture cleanup");
}
