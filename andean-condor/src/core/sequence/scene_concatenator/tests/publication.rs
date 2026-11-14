//! Final-owner policy tests; fake mux output is not native codec evidence.
use super::*;
use crate::utils::cancellation::Cancelled;

type StageCheck = Box<dyn FnOnce(&Path)>;
type PersistCheck = Box<dyn FnOnce(&Path, &Path)>;
thread_local! {
    static BEFORE_COMMIT: std::cell::RefCell<Option<StageCheck>> = const { std::cell::RefCell::new(None) };
    static BEFORE_PERSIST: std::cell::RefCell<Option<PersistCheck>> = const { std::cell::RefCell::new(None) };
    static DURING_PAYLOAD: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

pub(crate) fn before_commit(staged: &Path) {
    if let Some(check) = BEFORE_COMMIT.with_borrow_mut(Option::take) {
        check(staged);
    }
}

pub(super) fn observe_commit(check: impl FnOnce(&Path) + 'static) {
    BEFORE_COMMIT.with_borrow_mut(|hook| *hook = Some(Box::new(check)));
}

pub(crate) fn before_persist(staged: &Path, destination: &Path) {
    if let Some(check) = BEFORE_PERSIST.with_borrow_mut(Option::take) {
        check(staged, destination);
    }
}

pub(super) fn remove_stage_before_persist() {
    BEFORE_PERSIST.with_borrow_mut(|hook| {
        *hook = Some(Box::new(|staged, _| {
            std::fs::remove_file(staged).expect("remove only owned stage after its shape check");
        }));
    });
}

pub(crate) fn during_payload(staged: &Path, token: &Cancellation) {
    if let Some(force) = DURING_PAYLOAD.replace(None) {
        assert!(std::fs::metadata(staged).expect("partial IVF").len() > 32);
        token.request().expect("graceful mid-record");
        if force {
            token.request().expect("force mid-record");
        }
    }
}

fn ivf_job(root: &Path, output: &Path) -> Condor<DefaultSequenceData, DefaultSequenceConfig> {
    let mut job = scene_list::job(root, output, 1);
    // Opaque framed payloads exercise cancellation, not native codec decodability.
    let mut bytes =
        b"DKIF\0\0\x20\0AV01\x20\0\x10\0\x18\0\0\0\x01\0\0\0\x02\0\0\0\0\0\0\0".to_vec();
    for timestamp in [0u64, 1] {
        bytes.extend_from_slice(&16384u32.to_le_bytes());
        bytes.extend_from_slice(&timestamp.to_le_bytes());
        bytes.extend_from_slice(&[0xAB; 16384]);
    }
    std::fs::write(root.join("00000.ivf"), bytes).expect("framed IVF fixture");
    crate::core::sequence::tests::support::current_encoding_fixture(&mut job);
    job
}

#[test]
fn ivf_mid_record_graceful_finishes_but_force_interrupts_without_publication() {
    for force in [false, true] {
        let dir = tempfile::tempdir().expect("IVF cancellation fixture");
        let output = dir.path().join("final.ivf");
        std::fs::write(&output, b"old final bytes").expect("old final");
        let mut job = ivf_job(dir.path(), &output);
        let original = std::fs::read(dir.path().join("00000.ivf")).expect("original chunk");
        let finished = Arc::new(AtomicBool::new(false));
        let observed = Arc::clone(&finished);
        BEFORE_COMMIT.with_borrow_mut(|hook| {
            *hook = Some(Box::new(move |staged| {
                let bytes = std::fs::read(staged).expect("finished admitted IVF");
                assert_eq!(bytes, original);
                observed.store(true, std::sync::atomic::Ordering::SeqCst);
            }));
        });
        DURING_PAYLOAD.set(Some(force));
        let (tx, rx) = sync::mpsc::channel();
        let error = SceneConcatenator::default()
            .execute_cancel(&mut job, tx, Cancellation::default())
            .expect_err("cancelled final Sequence");
        BEFORE_COMMIT.with_borrow_mut(|hook| {
            hook.take();
        });
        assert_eq!(
            error.downcast_ref::<Cancelled>(),
            Some(&if force {
                Cancelled::Forced
            } else {
                Cancelled::Graceful
            })
        );
        assert_eq!(finished.load(std::sync::atomic::Ordering::SeqCst), !force);
        assert_final(&output, true);
        assert_eq!(completed(&rx), 0);
        assert_clean(dir.path());
    }
}

#[test]
fn sequence_post_last_admission_race_uses_atomic_noclobber() {
    for overwrite in [false, true] {
        let dir = tempfile::tempdir().expect("publication race fixture");
        let output = dir.path().join("final.ivf");
        let mut job = ivf_job(dir.path(), &output);
        job.output.overwrite = overwrite;
        let original = std::fs::read(dir.path().join("00000.ivf")).expect("chunk");
        let reached = Arc::new(AtomicBool::new(false));
        let observed = Arc::clone(&reached);
        BEFORE_PERSIST.with_borrow_mut(|hook| {
            *hook = Some(Box::new(move |staged, destination| {
                assert!(!destination.exists(), "absent through last admission");
                assert!(std::fs::metadata(staged).expect("finished stage").len() > 0);
                std::fs::write(destination, b"racer owns final")
                    .expect("only inject owned file creation");
                observed.store(true, std::sync::atomic::Ordering::SeqCst);
            }));
        });
        let (tx, rx) = sync::mpsc::channel();
        let result =
            SceneConcatenator::default().execute_cancel(&mut job, tx, Cancellation::default());
        assert!(reached.load(std::sync::atomic::Ordering::SeqCst));
        if overwrite {
            result.expect("explicit consent may replace the late file");
            assert_eq!(std::fs::read(&output).expect("final"), original);
            assert_eq!(completed(&rx), 1);
        } else {
            let error = result.expect_err("atomic refusal, not readmission refusal");
            assert_eq!(
                error
                    .downcast_ref::<tempfile::PathPersistError>()
                    .expect("original atomic persist cause")
                    .error
                    .kind(),
                std::io::ErrorKind::AlreadyExists
            );
            assert_eq!(std::fs::read(&output).expect("racer"), b"racer owns final");
            assert_eq!(completed(&rx), 0);
            drop(error); // PathPersistError retains ownership of the failed staging path.
        }
        assert_clean(dir.path());
    }
}

#[test]
fn cancellation_before_lock_withdraws_but_after_commit_does_not() {
    for before in [false, true] {
        let dir = tempfile::tempdir().expect("linearization fixture");
        let output = dir.path().join("final.ivf");
        let mut job = ivf_job(dir.path(), &output);
        let token = Cancellation::default();
        if before {
            let token = token.clone();
            BEFORE_COMMIT.with_borrow_mut(|hook| {
                *hook = Some(Box::new(move |_| {
                    token.request().expect("request outside publication lock");
                }));
            });
        }
        let (tx, rx) = sync::mpsc::channel();
        let result = SceneConcatenator::default().execute_cancel(&mut job, tx, token.clone());
        if before {
            assert_eq!(
                result.expect_err("cancel before lock").downcast_ref::<Cancelled>(),
                Some(&Cancelled::Graceful)
            );
            assert!(!output.exists());
            assert_eq!(completed(&rx), 0);
        } else {
            result.expect("commit wins");
            token.request().expect("graceful after commit");
            token.request().expect("force after commit");
            assert_eq!(completed(&rx), 1);
            assert_eq!(
                std::fs::read(&output).expect("final"),
                std::fs::read(dir.path().join("00000.ivf")).expect("chunk")
            );
        }
        assert_clean(dir.path());
    }
}

#[test]
fn native_all_three_final_backends_exact_order_and_publication() {
    scene_list::native_complete_lists_keep_order_and_frame_count_and_require_current_provenance();
}

#[cfg(target_os = "linux")]
#[test]
fn final_owner_mux_cancellation_and_artifact_matrix() {
    use std::{
        os::unix::fs::PermissionsExt,
        time::{Duration, Instant},
    };
    let python = which::which("python3").expect("required Python fixture CLI");
    let mut failures = Vec::new();
    for backend in ["ffmpeg", "mkvmerge"] {
        for case in ["graceful", "force", "failure", "missing", "empty", "directory", "symlink"] {
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
                command.args(["--exact", "core::sequence::scene_concatenator::tests::publication::final_publication_child", "--nocapture"])
                    .env("FINAL_PUBLICATION_ROOT", root).env("FINAL_PUBLICATION_CASE", case).env("FINAL_PUBLICATION_BACKEND", backend)
                    .env("FINAL_PUBLICATION_EXISTING", existing.to_string()).env("PATH", bin)
                    .stdin(Stdio::null()).stdout(Stdio::from(log.try_clone().expect("log clone")))
                    .stderr(Stdio::from(log));
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
                        child.kill().expect("kill only owned test");
                        break child.wait().expect("reap owned test");
                    }
                    thread::park_timeout(Duration::from_millis(10));
                };
                let log = std::fs::read_to_string(log_path).expect("diagnostics");
                let label = format!("{backend}/{case}/existing={existing}");
                if timed_out || !status.success() || !log.contains("1 passed;") {
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
#[test]
fn final_publication_child() {
    use std::time::{Duration, Instant};
    let Some(root) = std::env::var_os("FINAL_PUBLICATION_ROOT").map(PathBuf::from) else {
        return;
    };
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
    crate::core::sequence::tests::support::current_encoding_fixture(&mut job);
    let saved_config = serde_json::to_value(&job.sequence_config).expect("config snapshot");
    let scratch = SceneConcatenator::scratch_directory(&root);
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
    concat.initialize(&mut job, tx.clone()).expect("initialize");
    let result = concat.execute_cancel(&mut job, tx, token);
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
