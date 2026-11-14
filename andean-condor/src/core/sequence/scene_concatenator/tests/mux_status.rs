//! Exercise the owning mux backend, not just its exit-code classifier.
use super::*;
use std::{
    os::unix::fs::PermissionsExt,
    time::{Duration, Instant},
};

#[derive(Clone, Default)]
struct Events(Arc<Mutex<Vec<(tracing::Level, String)>>>);

impl tracing::Subscriber for Events {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        let mut fields = String::new();
        event.record(
            &mut |field: &tracing::field::Field, value: &dyn std::fmt::Debug| {
                use std::fmt::Write;
                write!(fields, "{field}={value:?} ").expect("event fields");
            },
        );
        self.0.lock().expect("events").push((*event.metadata().level(), fields));
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

#[test]
fn owning_mux_status_artifact_and_warning_matrix() {
    let python = which::which("python3").expect("required Python fixture tool");
    let mut cases = Vec::new();
    for phase in ["final", "group", "grouped-final"] {
        for status in ["1", "0", "2", "signal"] {
            for artifact in ["valid", "missing", "empty", "directory"] {
                cases.push((phase, status, artifact, "normal"));
            }
        }
    }
    cases.push(("both", "1", "valid", "normal"));
    #[cfg(target_os = "linux")]
    {
        cases.push(("final", "1", "valid", "force"));
        cases.push(("final", "1", "preserve", "force-existing"));
    }
    let mut failures = Vec::new();
    for (phase, status, artifact, cancellation) in cases {
        let dir = tempfile::tempdir().expect("isolated mux case");
        let root = dir.path();
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).expect("private bin");
        let script = bin.join("mkvmerge");
        std::fs::write(
            &script,
            format!(
                r#"#!{}
import os,sys,json,signal
from pathlib import Path
r=Path(os.environ['MUX_STATUS_ROOT'])
options=Path(sys.argv[1][1:]); args=json.loads(options.read_text())
assert args[0]=='-o' and '[' in args and args[-1]==']'
out=Path(args[1]); phase='final' if options.name=='options.json' else 'group'
with (r/'calls').open('a') as f: f.write(phase+'\n')
target=os.environ['MUX_STATUS_PHASE']
selected=target=='both' or phase==('final' if target=='grouped-final' else target)
code=os.environ['MUX_STATUS_STATUS'] if selected else '0'
artifact=os.environ['MUX_STATUS_ARTIFACT'] if selected else 'valid'
if artifact=='valid': out.write_bytes(b'completed mux fixture')
elif artifact=='empty': out.write_bytes(b'')
elif artifact=='directory': out.mkdir()
print('Progress: 100%\r',end='')
print('mux-status '+phase+' stdout: check track timestamps',flush=True)
print('mux-status '+phase+' stderr: check source timecodes',file=sys.stderr,flush=True)
if code=='signal': os.kill(os.getpid(),signal.SIGTERM)
sys.exit(int(code))
"#,
                python.display()
            ),
        )
        .expect("finite mux fixture");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))
            .expect("executable");
        let log_path = root.join("child.log");
        let log = File::create(&log_path).expect("child log");
        let mut command = Command::new(std::env::current_exe().expect("test binary"));
        command
            .args([
                "--exact",
                "core::sequence::scene_concatenator::tests::mux_status::mux_status_child",
                "--nocapture",
            ])
            .env("MUX_STATUS_ROOT", root)
            .env("MUX_STATUS_PHASE", phase)
            .env("MUX_STATUS_STATUS", status)
            .env("MUX_STATUS_ARTIFACT", artifact)
            .env("MUX_STATUS_CANCEL", cancellation)
            .env("PATH", bin)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone().expect("log clone")))
            .stderr(Stdio::from(log));
        if cancellation.starts_with("force") {
            command
                .env("PROCESS_LIFECYCLE_ROOT", root)
                .env("PROCESS_LIFECYCLE_CASE", "commit-mkvmerge-pre");
        }
        let token = Cancellation::default();
        let mut child = OwnedProcess::spawn(&mut command, &token).expect("owned isolated test");
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut timed_out = false;
        let result = loop {
            if let Some(result) = child.try_wait().expect("poll isolated test") {
                break result;
            }
            if Instant::now() >= deadline {
                timed_out = true;
                child.kill().expect("watchdog kills owned test");
                break child.wait().expect("watchdog reaps owned test");
            }
            thread::park_timeout(Duration::from_millis(10));
        };
        let log = std::fs::read_to_string(log_path).expect("isolated diagnostics");
        let label = format!("{phase}/{status}/{artifact}/{cancellation}");
        if timed_out || !result.success() || !log.contains("1 passed; 0 failed") {
            failures.push(format!("{label}: timeout={timed_out}\n{log}"));
        } else {
            eprintln!("mux-status {label}: owning backend passed");
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn mux_status_child() {
    use std::os::unix::process::ExitStatusExt;
    let Some(root) = std::env::var_os("MUX_STATUS_ROOT").map(PathBuf::from) else {
        return;
    };
    let phase = std::env::var("MUX_STATUS_PHASE").expect("phase");
    let status = std::env::var("MUX_STATUS_STATUS").expect("status");
    let artifact = std::env::var("MUX_STATUS_ARTIFACT").expect("artifact");
    let cancellation = std::env::var("MUX_STATUS_CANCEL").expect("cancellation");
    let output = root.join("committed");
    if cancellation == "force-existing" {
        std::fs::write(&output, b"caller-owned sentinel").expect("sentinel");
    }
    // 513 distinct paths exercise production grouping without a configurable group-size seam.
    let scenes = (0..if phase == "final" { 1 } else { 513 })
        .map(|index| {
            let path = root.join(format!("scene {index} 'quoted'.mkv"));
            std::fs::write(&path, b"owned scene fixture").expect("nonempty mux input");
            path
        })
        .collect::<Vec<_>>();
    let (tx, _rx) = sync::mpsc::channel();
    let token = Cancellation::default();
    let events = Events::default();
    let result = tracing::subscriber::with_default(events.clone(), || {
        SceneConcatenator::mkvmerge_cancel(
            &root,
            &output,
            &scenes,
            None,
            Ratio::new(24, 1),
            None,
            &tx,
            &token,
        )
    });
    let completed_status = status == "0" || status == "1";
    let valid_artifact = artifact == "valid" || artifact == "preserve";
    let success = completed_status && valid_artifact && cancellation == "normal";
    let calls = std::fs::read_to_string(root.join("calls")).expect("actual mux invocations");
    let expected_calls = if phase == "final" {
        "final\n"
    } else if phase == "group" && !success {
        "group\n"
    } else {
        "group\ngroup\nfinal\n"
    };
    assert_eq!(
        calls, expected_calls,
        "a failed group must prevent the final mux"
    );
    let events = events.0.lock().expect("captured events");
    let warnings = events
        .iter()
        .filter(|(level, _)| *level == tracing::Level::WARN)
        .map(|(_, fields)| fields)
        .collect::<Vec<_>>();
    if status == "1" {
        let count = if phase == "both" {
            3
        } else if phase == "group" && success {
            2
        } else {
            1
        };
        assert_eq!(
            warnings.len(),
            count,
            "warning exit must be surfaced, not discarded: {events:?}"
        );
        for fields in warnings {
            let warning_phase = if fields.contains("phase=\"group\"") {
                "group"
            } else {
                "final"
            };
            assert!(
                fields.contains(&format!("phase=\"{warning_phase}\"")),
                "{fields}"
            );
            assert!(fields.contains("status=exit status: 1"), "{fields}");
            assert!(
                fields.contains(&format!(
                    "mux-status {warning_phase} stdout: check track timestamps"
                )),
                "{fields}"
            );
            assert!(
                fields.contains(&format!(
                    "mux-status {warning_phase} stderr: check source timecodes"
                )),
                "{fields}"
            );
        }
        assert!(
            !events.iter().any(|(level, _)| *level == tracing::Level::ERROR),
            "warning-only status is not an error"
        );
    } else {
        assert!(
            warnings.is_empty(),
            "only status 1 is a completed-with-warnings mux"
        );
    }
    if success {
        result.expect("0/1 with a regular nonempty artifact completes");
        assert_eq!(
            std::fs::read(&output).expect("final mux"),
            b"completed mux fixture"
        );
    } else {
        let error = result.expect_err("failed status, invalid artifact or force cannot complete");
        if cancellation.starts_with("force") {
            assert_eq!(
                error.downcast_ref::<crate::utils::cancellation::Cancelled>(),
                Some(&crate::utils::cancellation::Cancelled::Forced)
            );
            if cancellation == "force-existing" {
                assert_eq!(
                    std::fs::read(&output).expect("caller output survives"),
                    b"caller-owned sentinel"
                );
            } else {
                assert!(
                    !output.exists(),
                    "force removes an uncommitted owned output"
                );
            }
        } else if !completed_status {
            let Some(SceneConcatenatorError::MkvmergeFailed {
                status: actual,
                stdout,
                stderr,
            }) = error.downcast_ref()
            else {
                panic!("typed mux failure: {error:#}");
            };
            if status == "signal" {
                assert_eq!(actual.signal(), Some(libc::SIGTERM));
            } else {
                assert_eq!(actual.code(), Some(2));
            }
            let failed_phase = if phase == "group" { "group" } else { "final" };
            assert!(
                stdout.contains(&format!(
                    "mux-status {failed_phase} stdout: check track timestamps"
                )),
                "{stdout}"
            );
            assert!(
                stderr.contains(&format!(
                    "mux-status {failed_phase} stderr: check source timecodes"
                )),
                "{stderr}"
            );
        } else {
            let text = format!("{error:#}");
            assert!(
                text.contains("output")
                    && text.contains(if phase == "group" { "group" } else { "final" }),
                "{text}"
            );
        }
    }
    println!(
        "mux-status phase={phase} status={status} artifact={artifact} cancellation={cancellation}: checked result, typed diagnostics, warnings and group/final admission"
    );
}
