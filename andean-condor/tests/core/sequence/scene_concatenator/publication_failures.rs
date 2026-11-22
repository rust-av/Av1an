//! Final-owner policy tests; fake mux output is not native codec evidence.
use super::*;
use andean_condor::{
    core::process::OwnedProcess, models::sequence::scene_concatenator::MkvmergeConfig,
};
use std::fs::File;

#[cfg(target_os = "linux")]
#[test]
fn graceful_final_mux_finishes_active_work_without_publishing() {
    publication_cases(
        "graceful_final_mux_finishes_active_work_without_publishing",
        &["graceful"],
    );
}

#[cfg(target_os = "linux")]
#[test]
fn forced_final_mux_reaps_owned_descendants_and_preserves_output() {
    publication_cases(
        "forced_final_mux_reaps_owned_descendants_and_preserves_output",
        &["force"],
    );
}

#[cfg(target_os = "linux")]
#[test]
fn failed_final_mux_preserves_original_status_diagnostics_and_output() {
    publication_cases(
        "failed_final_mux_preserves_original_status_diagnostics_and_output",
        &["failure"],
    );
}

#[cfg(target_os = "linux")]
#[test]
fn missing_empty_directory_or_symlink_mux_output_is_not_published() {
    publication_cases(
        "missing_empty_directory_or_symlink_mux_output_is_not_published",
        &["missing", "empty", "directory", "symlink"],
    );
}

#[cfg(target_os = "linux")]
fn publication_cases(test: &str, cases: &[&str]) {
    use std::{os::unix::fs::PermissionsExt, time::Duration};
    if std::env::var_os("FINAL_PUBLICATION_ROOT").is_some() {
        let case = std::env::var("FINAL_PUBLICATION_CASE").expect("case");
        assert!(
            cases.contains(&case.as_str()),
            "case belongs to this contract"
        );
        assert_failed_publication_preserves_output();
        println!("publication-case={case}: assertions completed");
        return;
    }
    let module = module_path!().split_once("::").expect("test crate prefix").1;
    let exact = format!("{module}::{test}");
    let python = which::which("python3").expect("required Python fixture CLI");
    let mut failures = Vec::new();
    for backend in ["ffmpeg", "mkvmerge"] {
        for case in cases {
            for existing in [false, true] {
                let dir = tempfile::tempdir().expect("isolated owning Sequence fixture");
                let root = dir.path();
                let bin = root.join("bin");
                std::fs::create_dir_all(&bin).expect("private PATH");
                let script = bin.join(backend);
                std::fs::write(&script, format!(r#"#!{}
import os,sys,time,json,subprocess
from pathlib import Path
r=Path(os.environ['FINAL_PUBLICATION_ROOT']); case=os.environ['FINAL_PUBLICATION_CASE']
args=sys.argv[1:]
if args[0].startswith('@'): args=json.loads(Path(args[0][1:]).read_text())
out=Path(args[args.index('-o')+1] if '-o' in args else args[-1])
assert out.parent.name.startswith('.condor-output-') and out.suffix=='.mkv'
(r/'staged').write_text(str(out))
out.write_bytes(b'partial mux output')
print('Progress: 50%\r',end='',flush=True)
print('fixture stderr',file=sys.stderr,flush=True)
child=subprocess.Popen([sys.executable,'-c','import time; time.sleep(8)'])
def identity(pid): return str(pid)+' '+Path('/proc/'+str(pid)+'/stat').read_text().rsplit(')',1)[1].split()[19]
(r/'pids').write_text(identity(os.getpid())+'\n'+identity(child.pid)+'\n')
(r/'active').write_text('ready')
deadline=time.monotonic()+8
while not (r/'release').exists() and time.monotonic()<deadline: time.sleep(0.005)
if not (r/'release').exists(): sys.exit(43)
child.terminate(); child.wait()
if case=='failure': sys.exit(42)
out.unlink()
if case=='empty': out.touch()
elif case=='directory': out.mkdir()
elif case=='symlink': out.symlink_to(r/'protected')
elif case!='missing': out.write_bytes(b'finished mux output')
(r/'finished').write_text('admitted mux finished')
"#, python.display())).expect("finite mux fixture");
                std::fs::set_permissions(script, std::fs::Permissions::from_mode(0o700))
                    .expect("executable");
                let log_path = root.join("child.log");
                let log = File::create(&log_path).expect("file-backed diagnostics");
                let mut command = Command::new(std::env::current_exe().expect("test binary"));
                command
                    .args(["--exact", &exact, "--nocapture"])
                    .env("FINAL_PUBLICATION_ROOT", root)
                    .env("FINAL_PUBLICATION_CASE", case)
                    .env("FINAL_PUBLICATION_BACKEND", backend)
                    .env("FINAL_PUBLICATION_EXISTING", existing.to_string())
                    .env("PATH", bin)
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
                    || !log.contains(&format!("publication-case={case}: assertions completed"))
                {
                    failures.push(format!("{label}: timeout={timed_out}\n{log}"));
                } else {
                    eprintln!("final-publication {label}: passed");
                }
                dir.close().expect("fixture cleanup");
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[cfg(target_os = "linux")]
fn assert_failed_publication_preserves_output() {
    use std::time::{Duration, Instant};
    let root = PathBuf::from(std::env::var_os("FINAL_PUBLICATION_ROOT").expect("isolated root"));
    // SAFETY: Only this isolated test process adopts its own killed fixture descendants.
    assert_eq!(unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1) }, 0);
    let case = std::env::var("FINAL_PUBLICATION_CASE").expect("case");
    let backend = std::env::var("FINAL_PUBLICATION_BACKEND").expect("backend");
    let existing = std::env::var("FINAL_PUBLICATION_EXISTING").expect("existing") == "true";
    let output = root.join("final.mkv");
    if existing {
        std::fs::write(&output, b"old final bytes").expect("old final");
    }
    let mut job = scene_list::job(&root, &root.join("fallback.mkv"), 1);
    job.sequence_config.scene_concatenator.output = Some(output.clone());
    job.sequence_config.scene_concatenator.method = if backend == "ffmpeg" {
        ConcatMethod::FFmpeg(FfmpegConfig::default())
    } else {
        ConcatMethod::MKVMerge(MkvmergeConfig::default())
    };
    let chunk = root.join("00000.ivf");
    std::fs::write(&chunk, b"encoded input sentinel").expect("chunk");
    std::fs::write(root.join("protected"), b"source sentinel").expect("protected source");
    job.sequence_config.scene_detector.input = Some(InputModel::VapourSynthScript {
        source: VapourSynthScriptSource::Path(root.join("protected")),
        variables: Default::default(),
        index: 0,
        filters: vec![],
        stream_concurrently: false,
    });
    // Synthetic current C14 stamp; these bytes test owner policy, not codec decodability.
    current_encoding_fixture(&mut job);
    let saved_config = serde_json::to_value(&job.sequence_config).expect("config snapshot");
    let scratch = scratch_directory(&root);
    std::fs::create_dir_all(&scratch).expect("caller scratch");
    std::fs::write(scratch.join("source.mkv"), b"caller scratch sentinel")
        .expect("scratch sentinel");
    let token = Cancellation::default();
    let controller = {
        let root = root.clone();
        let output = output.clone();
        let token = token.clone();
        let case = case.clone();
        thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !root.join("active").exists() && Instant::now() < deadline {
                thread::yield_now();
            }
            let active = root.join("active").exists();
            if active {
                assert_final(&output, existing);
                let staged =
                    std::fs::read_to_string(root.join("staged")).expect("owned backend output");
                assert_eq!(
                    std::fs::read(staged).expect("partial owned output"),
                    b"partial mux output"
                );
            }
            if !active || case == "force" || case == "graceful" {
                token.request().expect("first graceful request");
                if !active || case == "force" {
                    token.request().expect("force");
                }
            }
            assert!(active, "bounded mid-mux handshake");
            if case != "force" {
                std::fs::write(root.join("release"), b"finish admitted mux").expect("release");
            }
        })
    };
    let (tx, rx) = sync::mpsc::channel();
    let mut concat = SceneConcatenator::default();
    concat.validate(&mut job).expect("final preflight");
    concat
        .initialize(
            &mut job,
            tx.clone(),
            &andean_condor::utils::cancellation::Cancellation::default(),
        )
        .expect("initialize");
    let result = concat.execute(&mut job, tx, token);
    controller.join().expect("joined controller");
    // Reap only the fixture descendant, identified while its leader still owned it.
    let pids = std::fs::read_to_string(root.join("pids")).expect("owned identities");
    let identities = pids
        .lines()
        .map(|line| {
            let (pid, start) = line.split_once(' ').expect("pid/start");
            (pid.parse::<i32>().expect("pid"), start.to_owned())
        })
        .collect::<Vec<_>>();
    assert_eq!(identities.len(), 2, "exactly one leader and one descendant");
    if case == "force" {
        let mut status = 0;
        // SAFETY: Positive PID is the adopted fixture child, never a wildcard waiter.
        let reaped = unsafe { libc::waitpid(identities[1].0, &mut status, 0) };
        assert_eq!(reaped, identities[1].0);
        assert!(libc::WIFSIGNALED(status));
        assert_eq!(libc::WTERMSIG(status), libc::SIGKILL);
    }
    for (pid, start) in identities {
        if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            assert_ne!(
                stat.rsplit_once(')').expect("stat").1.split_whitespace().nth(19),
                Some(start.as_str()),
                "owned PID was not reaped"
            );
        }
    }
    let error = result.expect_err("cancelled/invalid mux must not publish");
    if case == "force" || case == "graceful" {
        assert_eq!(
            error.downcast_ref::<Cancelled>(),
            Some(&if case == "force" {
                Cancelled::Forced
            } else {
                Cancelled::Graceful
            })
        );
        assert_eq!(
            root.join("finished").exists(),
            case == "graceful",
            "graceful finishes active work, force interrupts it"
        );
    } else if case == "failure" {
        let status =
            match error.downcast_ref::<SceneConcatenatorError>().expect("typed backend failure") {
                SceneConcatenatorError::FfmpegFailed { status, stderr, .. }
                | SceneConcatenatorError::MkvmergeFailed { status, stderr, .. } => {
                    assert!(stderr.contains("fixture stderr"));
                    status
                },
                other => panic!("unexpected error: {other}"),
            };
        assert_eq!(status.code(), Some(42));
    } else {
        assert!(format!("{error:#}").contains("output"), "{error:#}");
    }
    assert_eq!(completed(&rx), 0);
    assert_final(&output, existing);
    assert!(!job.output.path.exists(), "effective override honored");
    assert_eq!(
        std::fs::read(chunk).expect("chunk intact"),
        b"encoded input sentinel"
    );
    assert_eq!(
        std::fs::read(root.join("protected")).expect("source intact"),
        b"source sentinel"
    );
    assert_eq!(
        std::fs::read(scratch.join("source.mkv")).expect("scratch intact"),
        b"caller scratch sentinel"
    );
    assert_eq!(
        serde_json::to_value(&job.sequence_config).expect("config"),
        saved_config
    );
    assert_clean(&root);
}

fn completed(rx: &sync::mpsc::Receiver<SequenceStatus>) -> usize {
    rx.try_iter()
        .filter(|s| matches!(s, SequenceStatus::Whole(Status::Completed { .. })))
        .count()
}

fn assert_final(path: &Path, existing: bool) {
    if existing {
        assert_eq!(
            std::fs::read(path).expect("old final preserved"),
            b"old final bytes"
        );
    } else {
        assert!(!path.exists(), "no final partial output");
    }
}

pub(super) fn assert_clean(root: &Path) {
    assert!(
        !std::fs::read_dir(root).expect("staging cleanup").any(|e| e
            .expect("entry")
            .file_name()
            .to_string_lossy()
            .starts_with(".condor-output-"))
    );
}
