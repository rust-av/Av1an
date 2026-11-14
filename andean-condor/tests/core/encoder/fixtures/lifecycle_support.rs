//! Test-only lifecycle fixtures shared by public API and private-hook contracts.
use super::*;

pub(super) fn prepare_case() -> (PathBuf, String, Cancellation) {
    let root = PathBuf::from(std::env::var_os("PROCESS_LIFECYCLE_ROOT").expect("fixture root"));
    let case = std::env::var("PROCESS_LIFECYCLE_CASE").expect("case");
    fs::write(root.join("selected"), "yes").expect("handshake");
    fs::write(root.join("sentinel"), "caller-owned").expect("sentinel");
    if case == "encoder-existing" {
        fs::write(root.join("out.264"), "caller-owned").expect("existing output");
    }
    if case == "ffmpeg-existing" {
        fs::write(root.join("out.mkv"), "caller-owned").expect("existing output");
    }
    // SAFETY: This supervisor-owned child adopts only fixture grandchildren and ignores terminal SIGINT.
    unsafe {
        assert_eq!(libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0), 0);
        libc::signal(libc::SIGINT, libc::SIG_IGN);
    }
    (root, case, Cancellation::default())
}

pub(super) fn wait_file(root: &Path, name: &str) {
    while !root.join(name).exists() {
        thread::yield_now();
    }
}

pub(super) fn input(root: &Path, native: bool) -> Input {
    if native {
        let path = root.join("input.y4m");
        let mut bytes = b"YUV4MPEG2 W64 H64 F24:1 Ip A1:1 C420jpeg\n".to_vec();
        for _ in 0..2 {
            bytes.extend_from_slice(b"FRAME\n");
            bytes.extend(vec![48; 64 * 64 * 3 / 2]);
        }
        fs::write(&path, bytes).expect("native input");
        Input::from_video(&InputModel::Video {
            path,
            import_method: ImportMethod::FFMS2 {
                index: None,
                cache_path: Some(root.join("input.ffindex")),
            },
            filters: vec![],
        })
        .expect("native FFMS2")
    } else {
        let path = root.join("input.vpy");
        let script = if std::env::var("PROCESS_LIFECYCLE_CASE").as_deref() == Ok("mid-production") {
            format!(
                "import os,time\nfrom vapoursynth import core,YUV420P8\nc=core.std.BlankClip(width=64,height=64,length=9,format=YUV420P8)\ndef mark(n):\n if n==3:\n  open({:?},'w').write('frame3')\n  deadline=time.monotonic()+8\n  while not os.path.exists({:?}) and time.monotonic()<deadline: pass\n return c\ncore.std.FrameEval(c,mark).set_output()\n",
                root.join("frame-k").to_str().expect("path"),
                root.join("release-frame").to_str().expect("path")
            )
        } else {
            format!(
                "from vapoursynth import core, YUV420P8\nc=core.std.BlankClip(width=64,height=64,length=2,format=YUV420P8)\ndef mark(n):\n if n==1: open({:?},'w').write('decoded')\n return c\ncore.std.FrameEval(c,mark).set_output()\n",
                root.join("queued-decoded").to_str().expect("path")
            )
        };
        fs::write(&path, script).expect("script");
        Input::from_data(&InputModel::VapourSynthScript {
            source: VapourSynthScriptSource::Path(path),
            variables: HashMap::new(),
            index: 0,
            filters: vec![],
            stream_concurrently: false,
        })
        .expect("VS input")
    }
}

pub(super) fn encoder(root: &Path, multipass: bool) -> Encoder {
    let mut encoder = Encoder::default_from_base(&EncoderBase::X264, true);
    encoder.set_executable(Some(root.join("fake")));
    encoder.set_append_color_data(false);
    encoder.set_quantizer(23.0);
    if multipass {
        *encoder.passes_mut().expect("passes") = EncoderPasses::All(2);
    }
    encoder
}

pub(super) fn reap(root: &Path) {
    if let Ok(pid) = fs::read_to_string(root.join("grandchild")) {
        let pid: i32 = pid.parse().expect("grandchild PID");
        let mut status = 0;
        // SAFETY: Reap only this known adopted child. Its lifetime exceeds the supervisor bound.
        let reaped = unsafe { libc::waitpid(pid, &mut status, 0) };
        assert_eq!(
            reaped, pid,
            "grandchild must be dead, not killed by supervisor"
        );
        assert!(libc::WIFSIGNALED(status));
        assert_eq!(libc::WTERMSIG(status), libc::SIGKILL);
    }
    if let Ok(pid) = fs::read_to_string(root.join("leader")) {
        let pid: i32 = pid.parse().expect("leader PID");
        // SAFETY: Signal zero observes this fixture PID without killing anything.
        let exists = unsafe { libc::kill(pid, 0) };
        assert_eq!(exists, -1, "direct child must be reaped");
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }
    assert_eq!(
        fs::read(root.join("sentinel")).expect("sentinel"),
        b"caller-owned"
    );
    assert!(
        !fs::read_dir(root).expect("listing").any(|e| e
            .expect("entry")
            .file_name()
            .to_string_lossy()
            .starts_with(".av1an-stats-"))
    );
    assert!(
        !fs::read_dir(root).expect("serial scratch listing").any(|e| e
            .expect("entry")
            .file_name()
            .to_string_lossy()
            .starts_with(".av1an-serial-"))
    );
}
