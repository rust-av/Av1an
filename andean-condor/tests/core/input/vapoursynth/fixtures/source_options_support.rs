//! Owned read-only source and explicit-cache fixtures shared by source-option tests.
use super::{HashMap, Lines, VapourSynthScript};
use std::{fs, process::Command};

pub(super) const PYTHON_LITERAL_STRINGS: &[&str] = &[
    "",
    "single' and double\" quotes",
    "back\\slash",
    "literal\\n\\t\\u1234\\x41",
    "line\ncarriage\rtab\t",
    "trailing\\",
    "映像🦀 e\u{301}\u{2028}\u{2029}",
    "\0\u{1}\u{8}\u{c}\u{1f}",
    "\"+str(marker.append('executed'))+\"",
    "str(marker.append('executed'))",
];

pub(super) fn recorded(lines: Lines) -> serde_json::Value {
    let mut script = VapourSynthScript::default();
    script.add_lines(lines);
    script.outputs.insert(0, "clip".to_owned());
    recorded_script(&script.to_string(), &HashMap::new())
}

pub(super) fn recorded_script(
    script: &str,
    variables: &HashMap<String, String>,
) -> serde_json::Value {
    let script_file = tempfile::NamedTempFile::new().expect("generated script");
    fs::write(script_file.path(), script).expect("write generated script");
    let output = run(Command::new("timeout").args(["10s","python3","-c",r#"
import json, sys, types
calls = []
class Clip:
    def set_output(self, *args, **kwargs): pass
def source(**kwargs):
    calls.append(kwargs)
    return Clip()
core = types.SimpleNamespace(**{n: types.SimpleNamespace(**{f: source}) for n, f in [('ffms2','Source'), ('bs','VideoSource'), ('lsmas','LWLibavSource'), ('dgdecodenv','DGSource')]})
sys.modules['vapoursynth'] = types.SimpleNamespace(core=core)
exec(compile(open(sys.argv[1]).read(), sys.argv[1], 'exec'), json.loads(sys.argv[2]))
print(json.dumps(calls))
"#]).arg(script_file.path()).arg(serde_json::to_string(variables).expect("script variables")));
    serde_json::from_slice(&output).expect("recorded generated kwargs")
}

pub(super) fn run(command: &mut Command) -> Vec<u8> {
    // File-backed output plus the caller's timeout avoids pipe deadlocks.
    let stdout = tempfile::NamedTempFile::new().expect("stdout");
    let stderr = tempfile::NamedTempFile::new().expect("stderr");
    let status = command
        .stdin(std::process::Stdio::null())
        .stdout(stdout.reopen().expect("stdout handle"))
        .stderr(stderr.reopen().expect("stderr handle"))
        .status()
        .expect("source-options requires ffmpeg, python3 and timeout on PATH");
    assert!(
        status.success(),
        "{command:?}: {}",
        fs::read_to_string(stderr.path()).expect("diagnostic")
    );
    fs::read(stdout.path()).expect("output")
}

pub(super) struct Fixture {
    directory: tempfile::TempDir,
    pub(super) source: std::path::PathBuf,
    original: Vec<u8>,
}

impl Fixture {
    pub(super) fn new() -> Self {
        Self::with_source_name("two tracks.mkv")
    }

    pub(super) fn with_source_name(name: &str) -> Self {
        let directory = tempfile::tempdir().expect("source-options fixture");
        let parent = directory.path().join("read only 映像");
        fs::create_dir_all(&parent).expect("source directory");
        let source = parent.join(name);
        run(Command::new("timeout")
            .args(["30s", "ffmpeg", "-nostdin", "-v", "error", "-y"])
            .args(["-f", "lavfi", "-i", "nullsrc=s=32x16:r=24,geq=lum=37:cb=83:cr=201"])
            .args(["-f", "lavfi", "-i", "nullsrc=s=32x16:r=24,geq=lum=219:cb=41:cr=159"])
            .args([
                "-map",
                "0:v",
                "-map",
                "1:v",
                "-frames:v",
                "100",
                "-c:v",
                "ffv1",
                "-threads",
                "1",
            ])
            .arg(&source));
        let original = fs::read(&source).expect("source bytes");
        let fixture = Self {
            directory,
            source,
            original,
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&fixture.source, fs::Permissions::from_mode(0o444))
                .expect("read-only file");
            fs::set_permissions(&parent, fs::Permissions::from_mode(0o555))
                .expect("read-only parent");
            let probe = parent.join("write probe");
            match fs::write(&probe, b"permission probe") {
                Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                    eprintln!("source-options readonly enforced: PermissionDenied ({parent:?})");
                },
                Ok(()) => {
                    fs::remove_file(&probe).expect("remove exact permission probe");
                    eprintln!(
                        "source-options readonly not-enforced/root capability: removed {probe:?}; continuing full assertions"
                    );
                },
                Err(error) => {
                    panic!("source-options permission probe {probe:?} failed unexpectedly: {error}")
                },
            }
        }
        fixture
    }

    pub(super) fn cache(&self, name: &str) -> std::path::PathBuf {
        let directory = self.directory.path().join(name);
        fs::create_dir_all(&directory).expect("writable cache directory");
        directory.join("explicit cache 索引")
    }

    pub(super) fn unchanged(&self) {
        assert_eq!(fs::read(&self.source).expect("source bytes"), self.original);
        assert_eq!(
            fs::read_dir(self.source.parent().expect("parent"))
                .expect("source listing")
                .count(),
            1,
            "no indexes beside read-only source"
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                self.source.parent().expect("parent"),
                fs::Permissions::from_mode(0o755),
            )
            .expect("restore parent for cleanup");
            fs::set_permissions(&self.source, fs::Permissions::from_mode(0o644))
                .expect("restore source for cleanup");
        }
    }
}

pub(super) fn expected_frame(track: Option<u8>, width: usize) -> Vec<u8> {
    let [y, u, v] = if track == Some(1) {
        [219, 41, 159]
    } else {
        [37, 83, 201]
    };
    [b"FRAME\n".to_vec(), vec![y; width * 16], vec![u; width * 4], vec![v; width * 4]].concat()
}
