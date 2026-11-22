//! Shared owned mux fixture; private injection stays in its source-side caller.
use super::*;
use std::{
    os::unix::fs::PermissionsExt,
    time::{Duration, Instant},
};

pub(super) struct MuxOutcome {
    pub(super) root: PathBuf,
    pub(super) previous: Vec<u8>,
    pub(super) result: Result<()>,
    pub(super) completed: bool,
    job: Condor<DefaultSequenceData, DefaultSequenceConfig>,
    case: String,
}

impl MuxOutcome {
    pub(super) fn output(&self) -> PathBuf {
        self.root.join("out.mkv")
    }

    pub(super) fn assert_preserved(&self, expected: &[u8]) {
        assert!(
            self.result.is_err(),
            "failure or late conflict must reach the caller"
        );
        assert!(
            !self.completed,
            "failed publication must not report completion"
        );
        assert_eq!(
            std::fs::read(self.output()).expect("final preserved"),
            expected
        );
    }

    pub(super) fn assert_common(self) {
        // A persist error can retain a staging-path owner until the error is dropped.
        drop(self.result);
        assert_eq!(
            std::fs::read(self.root.join("protected")).expect("source preserved"),
            b"protected original bytes"
        );
        assert!(
            !self.job.output.path.exists(),
            "actual override destination is honored"
        );
        assert!(
            !std::fs::read_dir(&self.root).expect("cleanup listing").any(|entry| entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .starts_with(".condor-output-"))
        );
        if !self.case.starts_with("native") {
            assert_eq!(
                std::fs::read(self.root.join("partial-written")).expect("partial snapshot"),
                b"partial native output"
            );
            let staged = std::fs::read_to_string(self.root.join("actual-staging"))
                .expect("child destination");
            assert!(staged.contains(".condor-output-") && staged.ends_with("output.mkv"));
        }
    }
}

pub(super) fn execute(root: &Path, case: &str) -> MuxOutcome {
    let final_path = root.join("out.mkv");
    let reference = std::fs::read(root.join("reference.mkv")).expect("two-frame scene");
    let previous = std::fs::read(root.join("old.mkv")).expect("one-frame old output");
    if case != "race" && case != "native-new" {
        std::fs::write(&final_path, &previous).expect("old valid final");
    }
    std::fs::write(root.join("protected"), b"protected original bytes").expect("source");
    let mut job = concat_job(root, &root.join("fallback-must-not-be-written.mkv"));
    job.output.overwrite = case != "race" && case != "native-new";
    job.sequence_config.scene_concatenator.output = Some(final_path);
    job.sequence_config.scene_concatenator.method = ConcatMethod::FFmpeg(FfmpegConfig::default());
    job.sequence_config.scene_detector.input = Some(InputModel::VapourSynthScript {
        source: VapourSynthScriptSource::Path(root.join("protected")),
        variables: Default::default(),
        index: 0,
        filters: vec![],
        stream_concurrently: false,
    });
    job.scenes.push(Scene {
        start_frame: 0,
        end_frame: 2,
        sub_scenes: None,
        encoder: Default::default(),
        sequence_data: Default::default(),
    });
    let chunk = root.join(format!(
        "{}.{}",
        ParallelEncoder::scene_id(0),
        job.scenes[0].encoder.base().output_extension()
    ));
    std::fs::write(chunk, reference).expect("real encoded scene");
    job.sequence_config.parallel_encoder.scenes_directory = root.into();
    current_encoding_fixture(&mut job);
    let token = Cancellation::default();
    let controller = (case == "force" || case == "graceful").then(|| {
        let root = root.to_owned();
        let token = token.clone();
        let force = case == "force";
        thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !root.join("active").exists() && Instant::now() < deadline {
                thread::yield_now();
            }
            let observed = root.join("active").exists();
            token.request().expect("graceful");
            if force || !observed {
                token.request().expect("force");
            }
            if !force {
                std::fs::write(root.join("release"), b"finish admitted concat").expect("release");
            }
            assert!(observed, "bounded active process handshake");
        })
    });
    let mut concatenator = SceneConcatenator::default();
    let (tx, rx) = sync::mpsc::channel();
    concatenator.validate(&mut job).expect("final preflight");
    concatenator
        .initialize(&mut job, tx.clone(), &Default::default())
        .expect("owning scratch setup");
    let result = concatenator.execute(&mut job, tx, token).map(|_| ());
    if let Some(controller) = controller {
        controller.join().expect("controller joined");
    }
    let completed = rx
        .try_iter()
        .any(|status| matches!(status, SequenceStatus::Whole(Status::Completed { .. })));
    MuxOutcome {
        root: root.into(),
        previous,
        result,
        completed,
        job,
        case: case.into(),
    }
}

pub(super) fn isolated(module: &str, test: &str, case: &str, check: impl FnOnce(&Path)) {
    if let Some(root) = std::env::var_os("FINAL_OUTPUT_ROOT") {
        assert_eq!(std::env::var("FINAL_OUTPUT_CASE").expect("case"), case);
        check(Path::new(&root));
        println!("mux-case={case}: assertions completed");
        return;
    }
    let ffmpeg = which::which("ffmpeg").expect("required FFmpeg CLI");
    let python = which::which("python3").expect("required Python fixture CLI");
    let directory = tempfile::tempdir().expect("fixture");
    let root = std::fs::canonicalize(directory.path()).expect("root");
    let reference = root.join("reference.mkv");
    let output = Command::new(&ffmpeg)
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=size=32x16:rate=24",
            "-frames:v",
            "2",
            "-c:v",
            "ffv1",
            "-f",
            "matroska",
        ])
        .arg(&reference)
        .output()
        .expect("real fixture encoder");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let old = Command::new(&ffmpeg)
        .args(["-v", "error", "-i"])
        .arg(&reference)
        .args(["-frames:v", "1", "-c", "copy"])
        .arg(root.join("old.mkv"))
        .output()
        .expect("valid distinct old output");
    assert!(
        old.status.success(),
        "{}",
        String::from_utf8_lossy(&old.stderr)
    );
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).expect("bin");
    let script = bin.join("ffmpeg");
    std::fs::write(
        &script,
        format!(
            r#"#!{}
import os,sys,time
from pathlib import Path
r=Path(os.environ['FINAL_OUTPUT_ROOT'])
c=os.environ['FINAL_OUTPUT_CASE']
out=Path(sys.argv[-1])
(r/'actual-staging').write_text(str(out))
out.write_bytes(b'partial native output')
(r/'partial-written').write_bytes(out.read_bytes())
if c=='failure': sys.exit(42)
if c in ('force','graceful'):
 (r/'active').write_text('ready')
 deadline=time.monotonic()+8
 while not (r/'release').exists() and time.monotonic()<deadline: time.sleep(0.005)
 if not (r/'release').exists(): sys.exit(43)
if c=='race': (r/'out.mkv').write_bytes(b'racer owns final')
if c=='alias':
 (r/'out.mkv').unlink()
 os.link(r/'protected',r/'out.mkv')
out.write_bytes((r/'reference.mkv').read_bytes())
(r/'mux-finished').write_text('admitted mux finished')
"#,
            python.display()
        ),
    )
    .expect("finite mux process seam");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).expect("executable");
    let module = module.split_once("::").expect("test crate prefix").1;
    let exact = format!("{module}::{test}");
    let path = if case.starts_with("native") {
        ffmpeg.parent().expect("binary directory")
    } else {
        bin.as_path()
    };
    let child = Command::new(std::env::current_exe().expect("test executable"))
        .args(["--exact", &exact, "--nocapture"])
        .env("FINAL_OUTPUT_ROOT", &root)
        .env("FINAL_OUTPUT_CASE", case)
        .env("PATH", path)
        .stdin(Stdio::null())
        .output()
        .expect("isolated owning Sequence");
    let stdout = String::from_utf8_lossy(&child.stdout);
    assert!(
        child.status.success()
            && stdout.contains("1 passed; 0 failed")
            && stdout.contains(&format!("mux-case={case}: assertions completed")),
        "{case}/{exact}\n{stdout}\n{}",
        String::from_utf8_lossy(&child.stderr)
    );
}
