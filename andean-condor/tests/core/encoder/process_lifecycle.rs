use super::*;
use andean_condor::{
    core::{
        Condor,
        encoder::{EncoderProcessError, EncoderResult, StringOrBytes},
        input::Input,
        output::Output,
        process::OwnedProcess,
        sequence::{
            Sequence,
            ffmpeg_filter::FfmpegFilter,
            parallel_encoder::{ParallelEncoder, Task},
            scene_concatenator::SceneConcatenator,
            serial_encoder::SerialEncoder,
            zone_encoder::{ZoneEncoder, ZonePlan},
        },
    },
    models::{
        input::{ImportMethod, Input as InputModel, VapourSynthScriptSource},
        scene::Scene,
        sequence::{DefaultSequenceConfig, DefaultSequenceData},
    },
    utils::cancellation::{Cancellation, Cancelled},
    vapoursynth::plugins::dgdecodenv::DGSource,
};
use anyhow::Result;
use std::{
    collections::HashMap,
    fs, io,
    io::Cursor,
    path::PathBuf,
    sync::{Arc, mpsc},
};

#[path = "fixtures/lifecycle_support.rs"]
mod fixture;
use fixture::{encoder, reap, wait_file};

macro_rules! lifecycle_cases {
    ($name:ident, [$($case:literal),+ $(,)?]) => {
        #[test]
        fn $name() { run_lifecycle_cases(stringify!($name), &[$($case),+]); }
    };
}

lifecycle_cases!(
    graceful_scene_cancellation_finishes_active_work_and_skips_queued_work,
    ["graceful-vs", "graceful-native", "graceful-sigint"]
);
lifecycle_cases!(
    owned_process_drop_reaps_the_process_tree,
    ["guard-drop", "guard-unwind", "after-completion"]
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

fn run_lifecycle_cases(test: &str, cases: &[&str]) {
    if std::env::var_os("PROCESS_LIFECYCLE_ROOT").is_some() {
        let case = std::env::var("PROCESS_LIFECYCLE_CASE").expect("case");
        assert!(
            cases.contains(&case.as_str()),
            "selected lifecycle case belongs to this contract"
        );
        assert_public_lifecycle_case();
        println!("lifecycle {case}: assertions completed");
        return;
    }
    let dir = tempfile::tempdir().expect("fixture");
    fs::write(
        dir.path().join("fake.rs"),
        include_str!("fixtures/lifecycle_process.rs"),
    )
    .expect("source");
    super::support::run_bounded(
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
        std::os::unix::fs::symlink(dir.path().join("fake"), root.join("fake"))
            .expect("fixture executable");
        let output = Command::new(root.join("fake"))
            .arg("supervise")
            .arg(std::env::current_exe().expect("test binary"))
            .arg(&root)
            .arg(case)
            .arg(format!("core::encoder::process_lifecycle::{test}"))
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

fn assert_public_lifecycle_case() {
    let (root, case, cancellation) = fixture::prepare_case();
    if case.starts_with("autoreap-") {
        // SAFETY: Change only this isolated test child's SIGCHLD disposition.
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
            // SAFETY: Save and change only this child's SIGCHLD disposition.
            unsafe { libc::sigaction(libc::SIGCHLD, &action, &mut previous) },
            0
        );
        let mut command = Command::new(root.join("fake"));
        command.arg("--complete");
        let result = OwnedProcess::spawn(&mut command, &cancellation);
        assert_eq!(
            // SAFETY: Restore the saved disposition before any assertion can unwind.
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
    if case == "leader-changes-group" {
        let mut command = Command::new(root.join("fake"));
        command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        let mut process = OwnedProcess::spawn(&mut command, &cancellation).expect("owned child");
        wait_file(&root, "group-changed");
        cancellation.request().expect("graceful");
        cancellation.request().expect("force group and changed-group direct child");
        let status = process.wait().expect("reap direct child promptly");
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(libc::SIGKILL));
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
                if matches!(
                    progress,
                    andean_condor::core::input::OpenProgress::Indexing { .. }
                ) && !observed
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
                    &super::clip_info(),
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
                    &super::clip_info(),
                    &cancellation
                )
                .expect_err("pre-spawn force")
                .is::<Cancelled>()
        );
        assert!(!root.join("leader").exists());
        reap(&root);
        return;
    }
    fixture::install_launchers(&root);
    fixture::assert_scene_case(&root, &case, cancellation);
}
