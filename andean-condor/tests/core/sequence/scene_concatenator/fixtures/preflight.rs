//! Native inputs and child-local tool seams shared by public and private checks.
use super::*;

pub(super) fn run(command: &mut Command) -> Vec<u8> {
    let output = command.output().expect("required native fixture tool");
    assert!(
        output.status.success(),
        "{command:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

pub(super) fn chunks(root: &Path) -> Vec<PathBuf> {
    ["black", "white"]
        .into_iter()
        .enumerate()
        .map(|(index, color)| {
            let path = root.join(format!("chunk {index} 'quoted'.mkv"));
            run(Command::new("ffmpeg")
                .args([
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    &format!("color={color}:size=32x16:rate=24"),
                    "-frames:v",
                    "2",
                    "-c:v",
                    "ffv1",
                ])
                .arg(&path));
            path
        })
        .collect()
}

pub(super) fn ancillary_source(root: &Path) -> PathBuf {
    let source = root.join("source.mkv");
    std::fs::write(
        root.join("sub.srt"),
        "1\n00:00:00,000 --> 00:00:00,100\nX01 subtitle\n",
    )
    .expect("subtitle");
    std::fs::write(
        root.join("attachment.txt"),
        b"concat-preflight attachment identity",
    )
    .expect("attachment");
    std::fs::write(root.join("metadata.txt"), ";FFMETADATA1\ntitle=concat-preflight title\ncomment=concat-preflight global tag\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=0\nEND=100\ntitle=concat-preflight chapter\n").expect("chapters");
    run(Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=size=32x16:rate=24",
            "-f",
            "lavfi",
            "-i",
            "sine=sample_rate=48000",
            "-i",
        ])
        .arg(root.join("sub.srt"))
        .arg("-i")
        .arg(root.join("metadata.txt"))
        .args([
            "-map",
            "0:v",
            "-map",
            "1:a",
            "-map",
            "2:s",
            "-map_metadata",
            "3",
            "-map_chapters",
            "3",
            "-t",
            "0.2",
            "-c:v",
            "ffv1",
            "-c:a",
            "pcm_s16le",
            "-c:s",
            "srt",
            "-metadata:s:a:0",
            "language=eng",
            "-metadata:s:a:0",
            "title=concat-preflight audio name",
            "-attach",
        ])
        .arg(root.join("attachment.txt"))
        .args(["-metadata:s:t:0", "mimetype=text/plain"])
        .arg(&source));
    source
}

#[cfg(unix)]
pub(super) fn isolated(module: &str, test: &str, cases: &[&str], check: impl FnOnce(&Path, &str)) {
    use std::os::unix::fs::PermissionsExt;
    if let Some(root) = std::env::var_os("CONCAT_PREFLIGHT_ROOT") {
        let case = std::env::var("CONCAT_PREFLIGHT_CASE").expect("case");
        assert!(
            cases.contains(&case.as_str()),
            "case belongs to this contract"
        );
        check(Path::new(&root), &case);
        println!("preflight-case={case}: assertions completed");
        return;
    }
    let python = which::which("python3").expect("required Python fixture tool");
    let ffmpeg = which::which("ffmpeg").expect("required FFmpeg");
    let ffprobe = which::which("ffprobe").expect("required FFprobe");
    let mkvmerge = which::which("mkvmerge").expect("required mkvmerge");
    let fixtures = tempfile::tempdir().expect("native fixtures");
    let scenes = chunks(fixtures.path());
    let source = ancillary_source(fixtures.path());
    let module = module.split_once("::").expect("test crate prefix").1;
    let exact = format!("{module}::{test}");
    for &case in cases {
        let dir = tempfile::tempdir().expect("isolated case");
        let root = dir.path();
        std::fs::copy(&source, root.join("source 'quoted'.mkv")).expect("source");
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).expect("private bin");
        let script = format!(
            r#"#!{}
import os,sys,json,subprocess,time
from pathlib import Path
r=Path(os.environ['CONCAT_PREFLIGHT_ROOT']); case=os.environ['CONCAT_PREFLIGHT_CASE']
tool=Path(sys.argv[0]).name; args=sys.argv[1:]
real=os.environ['CONCAT_PREFLIGHT_REAL_'+tool.upper()]
source=str(r/"source 'quoted'.mkv")
def hold():
 p=subprocess.Popen([sys.executable,'-c','import time; time.sleep(30)'])
 (r/'pids').write_text(str(os.getpid())+' '+str(p.pid))
 (r/'active').write_text('ready')
 while True: time.sleep(0.01)
if tool=='mkvmerge' and '--identify' in args and args[-1]==source:
 if case=='malformed-identify': print('{{'); sys.exit(0)
 if case=='identify-status': print('{{"container":{{"recognized":false,"supported":false}},"errors":[]}}'); sys.exit(2)
 if case=='identify-errors': print('{{"container":{{"recognized":true,"supported":true}},"errors":["fixture error"]}}'); sys.exit(1)
 if case=='identify-warning':
  result=subprocess.run([real,*args],capture_output=True); sys.stdout.buffer.write(result.stdout); sys.exit(1)
 print('{{"container":{{"recognized":false,"supported":false}},"errors":[]}}'); sys.exit(0)
if tool=='ffprobe' and args[-1]==source:
 if case=='leader-exit':
  data=subprocess.check_output([real,*args])
  p=subprocess.Popen([sys.executable,'-c','import time; time.sleep(6)'])
  (r/'pids').write_text(str(os.getpid())+' '+str(p.pid))
  sys.stdout.buffer.write(data); sys.exit(0)
 if case=='force-probe': hold()
 if case=='probe-failure': sys.stderr.write('fixture probe failure'); sys.exit(1)
 if case=='malformed-probe': print('{{}}'); sys.exit(0)
 if case=='probe-overflow': sys.stdout.write(' '*(4*1024*1024+1)); sys.stdout.flush(); hold()
 if case=='stderr-overflow': sys.stderr.write('x'*(64*1024+1)); sys.stderr.flush(); hold()
 if case in ('metadata-only','chapter-only','unsupported-type'):
  data=json.loads(subprocess.check_output([real,*args]))
  if case=='unsupported-type': data['streams'][1]['codec_type']='data'
  else: data['streams']=data['streams'][:1]
  if case=='chapter-only': data['format']['tags']={{}}
  print(json.dumps(data)); sys.exit(0)
if tool=='ffmpeg':
 if case=='force-remux': Path(args[-1]).write_bytes(b'partial source'); hold()
 if case=='empty-remux': Path(args[-1]).write_bytes(b''); sys.exit(0)
 if case=='missing-remux': sys.exit(0)
 if case=='invalid-remux': Path(args[-1]).write_bytes(b'not a Matroska file'); sys.exit(0)
 if case=='track-loss': args=args[:-1]+['-map','-0:2',args[-1]]
 if case=='codec-refusal': sys.stderr.write('codec cannot be copied into Matroska'); sys.exit(1)
 if case=='metadata-loss': args=args[:-1]+['-metadata','title=lost',args[-1]]
os.execv(real,[real,*args])
"#,
            python.display()
        );
        for tool in ["ffmpeg", "ffprobe", "mkvmerge"] {
            if (case == "missing-probe" && tool == "ffprobe")
                || (case == "missing-identify" && tool == "mkvmerge")
            {
                continue;
            }
            let path = bin.join(tool);
            std::fs::write(&path, &script).expect("fixture executable");
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
                .expect("executable");
        }
        let result = Command::new(std::env::current_exe().expect("test binary"))
            .args(["--exact", &exact, "--nocapture"])
            .env("CONCAT_PREFLIGHT_ROOT", root)
            .env("CONCAT_PREFLIGHT_CASE", case)
            .env("CONCAT_PREFLIGHT_SCENE0", &scenes[0])
            .env("CONCAT_PREFLIGHT_SCENE1", &scenes[1])
            .env("CONCAT_PREFLIGHT_REAL_FFMPEG", &ffmpeg)
            .env("CONCAT_PREFLIGHT_REAL_FFPROBE", &ffprobe)
            .env("CONCAT_PREFLIGHT_REAL_MKVMERGE", &mkvmerge)
            .env("PATH", bin)
            .output()
            .expect("isolated owning backend");
        let stdout = String::from_utf8_lossy(&result.stdout);
        assert!(
            result.status.success()
                && stdout.contains("1 passed; 0 failed")
                && stdout.contains(&format!("preflight-case={case}: assertions completed")),
            "{case}/{exact}: {stdout}\n{}",
            String::from_utf8_lossy(&result.stderr)
        );
        eprintln!("concat-preflight isolated {case}: {stdout}");
    }
}
