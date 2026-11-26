//! Finite grouped-mux process seam; private commit injection is enabled only by its caller.
use super::*;
use std::{fs::File, os::unix::fs::PermissionsExt, time::Duration};

pub(super) fn isolated(module: &str, test: &str, cases: &[(&str, &str, &str, &str)]) {
    if std::env::var_os("MUX_STATUS_ROOT").is_some() {
        let values = [
            "MUX_STATUS_PHASE",
            "MUX_STATUS_STATUS",
            "MUX_STATUS_ARTIFACT",
            "MUX_STATUS_CANCEL",
        ]
        .map(|key| std::env::var(key).expect("case selector"));
        let selected = (
            values[0].as_str(),
            values[1].as_str(),
            values[2].as_str(),
            values[3].as_str(),
        );
        assert!(cases.contains(&selected), "case belongs to this contract");
        assert_mux_status();
        println!("mux-status assertions completed");
        return;
    }
    let module = module.split_once("::").expect("test crate prefix").1;
    let exact = format!("{module}::{test}");
    let python = which::which("python3").expect("required Python fixture tool");
    let mut failures = Vec::new();
    for &(phase, status, artifact, cancellation) in cases {
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
            .args(["--exact", &exact, "--nocapture"])
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
        let (result, timed_out) = wait_with_timeout(&mut child, token, Duration::from_secs(20));
        let log = std::fs::read_to_string(log_path).expect("isolated diagnostics");
        let label = format!("{phase}/{status}/{artifact}/{cancellation}");
        if timed_out
            || !result.success()
            || !log.contains("1 passed; 0 failed")
            || !log.contains("mux-status assertions completed")
        {
            failures.push(format!("{label}: timeout={timed_out}\n{log}"));
        } else {
            eprintln!("mux-status {label}: owning backend passed");
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[derive(Clone, Default)]
struct Events(std::sync::Arc<std::sync::Mutex<Vec<(tracing::Level, String)>>>);

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

fn assert_mux_status() {
    use av_format::rational::Ratio;
    use std::os::unix::process::ExitStatusExt;
    let root = PathBuf::from(std::env::var_os("MUX_STATUS_ROOT").expect("isolated root"));
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
        SceneConcatenator::mkvmerge(
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
            assert_eq!(error.downcast_ref::<Cancelled>(), Some(&Cancelled::Forced));
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
