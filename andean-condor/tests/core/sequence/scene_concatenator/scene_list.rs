//! Admission must reject the whole requested list before any mux side effects.
use super::*;
use andean_condor::core::process::OwnedProcess;
use av_format::rational::Ratio;
use std::fs::File;

#[path = "fixtures/scene_list.rs"]
mod fixture;

fn direct(
    backend: &str,
    root: &Path,
    output: &Path,
    paths: &[PathBuf],
    token: &Cancellation,
) -> Result<()> {
    let (tx, _rx) = sync::mpsc::channel();
    let source = root.join("source");
    match backend {
        "ivf" => SceneConcatenator::ivf(output, paths, &tx, token),
        "ffmpeg" => SceneConcatenator::ffmpeg(
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
        "mkvmerge" => SceneConcatenator::mkvmerge(
            root,
            output,
            paths,
            Some(&source),
            Ratio::new(24, 1),
            None,
            &tx,
            token,
        ),
        _ => unreachable!("fixture backend"),
    }
}

#[cfg(unix)]
#[test]
fn concat_rejects_invalid_scene_lists_before_output_or_scratch_mutation() {
    use std::{os::unix::fs::PermissionsExt, time::Duration};
    if std::env::var_os("SCENE_LIST_ROOT").is_some() {
        assert_invalid_list_preserves_files();
        println!("invalid scene-list assertions completed");
        return;
    }
    let module = module_path!().split_once("::").expect("test crate prefix").1;
    let exact =
        format!("{module}::concat_rejects_invalid_scene_lists_before_output_or_scratch_mutation");
    let python = which::which("python3").expect("required fixture tool");
    let mut failures = Vec::new();
    for backend in ["ivf", "ffmpeg", "mkvmerge"] {
        for case in ["empty-list", "missing-middle", "empty-file", "directory", "unreadable"] {
            for existing in [false, true] {
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
                command
                    .args(["--exact", &exact, "--nocapture"])
                    .env("SCENE_LIST_ROOT", dir.path())
                    .env("SCENE_LIST_BACKEND", backend)
                    .env("SCENE_LIST_CASE", case)
                    .env("SCENE_LIST_EXISTING", existing.to_string())
                    .env("PATH", &bin)
                    .stdin(Stdio::null())
                    .stdout(Stdio::from(log.try_clone().expect("log clone")))
                    .stderr(Stdio::from(log));
                let cancellation = Cancellation::default();
                let mut child =
                    OwnedProcess::spawn(&mut command, &cancellation).expect("owned isolated test");
                let (status, timed_out) = crate::core::support::process::wait_with_timeout(
                    &mut child,
                    cancellation,
                    Duration::from_secs(20),
                );
                let log = std::fs::read_to_string(log_path).expect("diagnostics");
                let label = format!("{backend}/{case}/existing={existing}");
                if timed_out
                    || !status.success()
                    || !log.contains("1 passed;")
                    || !log.contains("invalid scene-list assertions completed")
                {
                    failures.push(format!("{label}: timeout={timed_out}\n{log}"));
                }
                dir.close().expect("fixture cleanup");
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[cfg(unix)]
fn assert_invalid_list_preserves_files() {
    use std::os::unix::fs::PermissionsExt;
    let root = PathBuf::from(std::env::var_os("SCENE_LIST_ROOT").expect("isolated root"));
    let backend = std::env::var("SCENE_LIST_BACKEND").expect("backend");
    let case = std::env::var("SCENE_LIST_CASE").expect("case");
    let existing = std::env::var("SCENE_LIST_EXISTING").expect("existing") == "true";
    let scratch = scratch_directory(&root);
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
            direct(&backend, &root, &output, &paths, &Cancellation::default())
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
    fixture::job(root, output, count)
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
    current_encoding_fixture(&mut job);
    let (tx, rx) = sync::mpsc::channel();
    let mut concat = SceneConcatenator::default();
    concat.validate(&mut job).expect("final admission");
    concat
        .initialize(
            &mut job,
            tx.clone(),
            &andean_condor::utils::cancellation::Cancellation::default(),
        )
        .expect("readable requested list");
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        concat.execute(&mut job, tx, Cancellation::default())
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
        current_encoding_fixture(&mut job);
        std::fs::remove_file(dir.path().join("00001.ivf"))
            .expect("missing middle after certification");
        let (tx, rx) = sync::mpsc::channel();
        let mut concat = SceneConcatenator::default();
        if concat
            .initialize(
                &mut job,
                tx.clone(),
                &andean_condor::utils::cancellation::Cancellation::default(),
            )
            .is_ok()
        {
            failures.push(format!(
                "count={count}: initialize accepted incomplete list"
            ));
        }
        if scratch_directory(dir.path()).exists() {
            failures.push(format!("count={count}: initialize created scratch"));
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            concat.execute(&mut job, tx, Cancellation::default())
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
            let error = direct(backend, dir.path(), &dir.path().join("out"), &[], &token)
                .expect_err("cancel before invalid list");
            assert_eq!(error.downcast_ref::<Cancelled>(), Some(&expected));
        }
        let mut job = job(dir.path(), &dir.path().join("final"), 0);
        let (tx, _rx) = sync::mpsc::channel();
        let error = SceneConcatenator::default()
            .execute(&mut job, tx, token)
            .expect_err("Sequence cancellation first");
        assert_eq!(error.downcast_ref::<Cancelled>(), Some(&expected));
    }
    assert_eq!(
        std::fs::read_dir(dir.path()).expect("no side effects").count(),
        0
    );
}

#[test]
fn direct_concat_preserves_caller_order_repeated_paths_and_frame_count() {
    let fixture = fixture::NativeScenes::new();
    let frames = &fixture.frames;
    let expected = [frames[2].as_slice(), &frames[0], &frames[1], &frames[2]].concat();
    for (index, method) in fixture::methods().iter().enumerate() {
        let output = fixture.concat_direct(index, method);
        assert_eq!(
            fixture::decoded(&output),
            expected,
            "direct {} order/count",
            method.extension()
        );
    }
    fixture.assert_unchanged();
}

#[test]
fn final_sequence_refuses_chunks_without_current_provenance_before_mux() {
    let mut fixture = fixture::NativeScenes::new();
    for (index, method) in fixture::methods().into_iter().enumerate() {
        let final_path = fixture.root().join(format!("uncertified-{index}.{}", method.extension()));
        fixture.job.sequence_config.scene_concatenator.method = method;
        fixture.job.sequence_config.scene_concatenator.output = Some(final_path.clone());
        let (tx, rx) = sync::mpsc::channel();
        let mut concat = SceneConcatenator::default();
        concat
            .initialize(
                &mut fixture.job,
                tx.clone(),
                &andean_condor::utils::cancellation::Cancellation::default(),
            )
            .expect("complete initialization");
        for scene in &mut fixture.job.scenes {
            scene.sequence_data.parallel_encoder.provenance = None;
        }
        assert!(concat.execute(&mut fixture.job, tx, Cancellation::default()).is_err());
        assert!(!final_path.exists());
        assert!(
            !rx.try_iter()
                .any(|s| matches!(s, SequenceStatus::Whole(Status::Completed { .. })))
        );
        fixture::assert_clean(fixture.root());
    }
    fixture.assert_unchanged();
}

#[test]
fn final_sequence_publishes_current_chunks_in_order_and_reports_completion_once() {
    let mut fixture = fixture::NativeScenes::new();
    for (index, method) in fixture::methods().into_iter().enumerate() {
        let (final_path, _) = fixture.prepare_final(index, method);
        let (tx, rx) = sync::mpsc::channel();
        let mut concat = SceneConcatenator::default();
        concat
            .initialize(
                &mut fixture.job,
                tx.clone(),
                &andean_condor::utils::cancellation::Cancellation::default(),
            )
            .expect("complete initialization");
        current_encoding_fixture(&mut fixture.job);
        let saved = serde_json::to_value(&fixture.job.sequence_config).expect("config snapshot");
        concat
            .execute(&mut fixture.job, tx, Cancellation::default())
            .expect("current saved-job-admission fixture admitted");
        assert_eq!(
            fixture::decoded(&final_path),
            fixture.frames.concat(),
            "Sequence order/count"
        );
        assert_eq!(
            rx.try_iter()
                .filter(|s| matches!(s, SequenceStatus::Whole(Status::Completed { .. })))
                .count(),
            1
        );
        assert!(
            !fixture.job.output.path.exists(),
            "effective output override"
        );
        assert_eq!(
            serde_json::to_value(&fixture.job.sequence_config).expect("config"),
            saved
        );
        fixture::assert_clean(fixture.root());
    }
    fixture.assert_unchanged();
}
