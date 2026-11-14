#[cfg(test)]
mod job_lock {
    #[cfg(unix)]
    use std::fs::File;
    use std::{fs, io};

    #[cfg(unix)]
    use california_condor::configuration::Configuration;
    use california_condor::configuration::JobLock;

    #[cfg(unix)]
    fn fixture(directory: &std::path::Path) -> Configuration {
        use andean_condor::ffmpeg::FFPixelFormat;
        use california_condor::test_helpers::{TestVideo, default_config};
        let source = directory.join("source.vpy");
        fs::write(&source, "import vapoursynth as vs\nvs.core.std.BlankClip(width=32,height=16,length=2,format=vs.YUV420P8).set_output()\n").expect("tiny native source");
        let video = TestVideo {
            path: source.clone(),
            width: 32,
            height: 16,
            frames: 2,
            fps_rational: (24, 1),
            format: FFPixelFormat::YUV420P,
            scenes: vec![(0, 2)],
        };
        let mut config =
            default_config(&video, &directory.join("out.mkv"), &directory.join("work"));
        config.condor.input = Configuration::new_input_model(&source, None, None, None, None)
            .expect("script model without native open");
        config.condor.scenes = video.mock_scenes(&config.condor.encoder);
        config
    }

    #[cfg(unix)]
    #[test]
    fn symlink_aliases_resolve_to_one_job_lock_across_atomic_config_saves() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().expect("fixture");
        let base = fs::canonicalize(temp.path()).expect("base");
        let config = fixture(&base);
        fs::create_dir_all(base.join("real/child")).expect("parent");
        symlink("real/child", base.join("alias-parent")).expect("parent alias");
        let file = base.join("real/job.json");
        let guard = JobLock::acquire(&base.join("alias-parent/../job.json"))
            .expect("new path through symlink and dotdot");
        assert_eq!(guard.path(), file);
        config.save(guard.path()).expect("save under guard");
        symlink("real/job.json", base.join("alias.json")).expect("file alias");
        for alias in ["alias.json", "real/./job.json", "alias-parent/../job.json"] {
            assert!(
                JobLock::acquire(&base.join(alias))
                    .expect_err("same target")
                    .to_string()
                    .contains("another process")
            );
            config.save(&base.join(alias)).expect("low-level saves resolve same target");
            assert!(
                JobLock::acquire(&base.join(alias)).is_err(),
                "save must not split identity"
            );
        }
        assert!(
            fs::symlink_metadata(base.join("alias.json"))
                .expect("alias retained")
                .is_symlink()
        );
        drop(guard);
        let guard = JobLock::acquire(&base.join("alias.json")).expect("reopen same sidecar");
        assert_eq!(guard.path(), file);
    }

    #[test]
    fn job_lock_open_errors_fail_closed_with_path_context() {
        let temp = tempfile::tempdir().expect("fixture");
        let file = temp.path().join("job.json");
        fs::create_dir_all(temp.path().join("job.json.lock"))
            .expect("directory blocks sidecar open");
        let error = JobLock::acquire(&file).expect_err("no unprotected fallback");
        let message = format!("{error:#}");
        assert!(
            message.contains("opening job lock")
                && message.contains("job.json.lock")
                && message.contains("job.json"),
            "{message}"
        );
        assert!(!file.exists());
    }

    #[test]
    fn job_lock_missing_parent_returns_not_found_without_creating_directories_or_config() {
        let temp = tempfile::tempdir().expect("fixture");
        let file = temp.path().join("missing/deep/job.json");
        let result = JobLock::acquire(&file);
        assert!(
            !temp.path().join("missing").exists(),
            "generic acquisition must not create parents or sidecars"
        );
        let error = result.expect_err("caller must prepare a new job's parent deliberately");
        assert!(format!("{error:#}").contains("job.json.lock"));
        assert!(error.chain().any(|cause| {
            cause
                .downcast_ref::<io::Error>()
                .is_some_and(|error| error.kind() == io::ErrorKind::NotFound)
        }));
        fs::create_dir_all(file.parent().expect("parent")).expect("caller prepares parent");
        let guard = JobLock::acquire(&file).expect("explicit new job under an existing parent");
        assert_eq!(guard.path(), file);
        assert!(!file.exists(), "guard does not create the config");
        assert!(file.with_file_name("job.json.lock").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn job_lock_permission_refusal_matches_actual_sidecar_open_probe() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().expect("fixture");
        let file = temp.path().join("job.json");
        let sidecar = temp.path().join("job.json.lock");
        fs::write(&sidecar, b"").expect("persistent sidecar");
        fs::set_permissions(&sidecar, fs::Permissions::from_mode(0o000)).expect("deny access");
        let probe = File::options().read(true).write(true).open(&sidecar);
        let result = JobLock::acquire(&file);
        fs::set_permissions(&sidecar, fs::Permissions::from_mode(0o600))
            .expect("restore before assertions");
        match probe {
            Err(error) => {
                assert_eq!(
                    error.kind(),
                    io::ErrorKind::PermissionDenied,
                    "actual open probe"
                );
                let error = result.expect_err("denied opens cannot fall back");
                assert!(format!("{error:#}").contains("job.json.lock"));
                assert!(error.chain().any(|cause| {
                    cause
                        .downcast_ref::<io::Error>()
                        .is_some_and(|error| error.kind() == io::ErrorKind::PermissionDenied)
                }));
                eprintln!("job lock actual sidecar open denied; contextual refusal proved");
            },
            Ok(_) => {
                drop(result.expect("actual open allowed; kernel locking still required"));
                eprintln!(
                    "job lock chmod did not deny actual sidecar open; permission-denial proof unavailable on this host"
                );
            },
        }
        assert!(!file.exists());
    }
}
