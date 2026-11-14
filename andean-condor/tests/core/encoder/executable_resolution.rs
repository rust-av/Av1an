use super::*;
use andean_condor::core::{
    encoder::{Container, EncoderCapability},
    input::Input,
};
use std::{collections::HashMap, io::Cursor, path::PathBuf, sync::mpsc};
use std::{fs, os::unix::ffi::OsStringExt, os::unix::fs::PermissionsExt, time::Duration};

const CUSTOM: &str = r#"#!/bin/sh
set -eu
case "${1-}" in
    --version) printf 'x264 executable resolution\n' ;;
    *) /bin/cat > "$EXECUTABLE_RESOLUTION_ROOT/consumed" ;;
esac
printf custom >> "$EXECUTABLE_RESOLUTION_ROOT/custom"
"#;
const STOCK: &str = "#!/bin/sh\nprintf stock > \"$EXECUTABLE_RESOLUTION_ROOT/stock\"\nif [ \"$EXECUTABLE_RESOLUTION_CASE\" = default ]; then /bin/cat >/dev/null; fi\nexit 0\n";

fn executable(path: &Path, source: &str) {
    fs::write(path, source).expect("fixture executable");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("permissions");
}

#[test]
fn explicit_encoder_paths_resolve_without_stock_fallback() {
    resolution_cases(
        "explicit_encoder_paths_resolve_without_stock_fallback",
        &["path", "absolute", "relative", "nonutf8", "default"],
    );
}

#[test]
fn invalid_encoder_paths_fail_before_input_or_progress() {
    resolution_cases(
        "invalid_encoder_paths_fail_before_input_or_progress",
        &[
            "missing",
            "missing-absolute",
            "missing-relative",
            "nonexec",
            "directory",
            "nul",
            "input",
            "input-pass",
            "stream",
            "stream-pass",
            "nonexec-input",
            "nonexec-stream-pass",
            "directory-input-pass",
            "nul-stream",
            "two-input",
            "two-stream",
        ],
    );
}

#[test]
fn missing_optional_encoder_queries_return_no_capability() {
    resolution_cases(
        "missing_optional_encoder_queries_return_no_capability",
        &["optional"],
    );
}

#[test]
fn encoder_spawn_errors_preserve_the_requested_os_path() {
    resolution_cases(
        "encoder_spawn_errors_preserve_the_requested_os_path",
        &["spawn-input", "spawn-stream"],
    );
}

// Re-execution isolates PATH/CWD and bounds regressions that receive before validation.
fn resolution_cases(test: &str, cases: &[&str]) {
    if std::env::var_os("EXECUTABLE_RESOLUTION_ROOT").is_some() {
        let selected = std::env::var("EXECUTABLE_RESOLUTION_CASE").expect("case");
        assert!(
            cases.contains(&selected.as_str()),
            "selected executable case belongs to this contract"
        );
        assert_requested_encoder_path();
        println!("encoder path {selected}: assertions completed");
        return;
    }
    let temp = tempfile::tempdir().expect("fixture");
    let mut failures = Vec::new();
    for &case in cases {
        let root = temp.path().join(case);
        let bin = root.join("bin");
        fs::create_dir_all(&bin).expect("bin");
        executable(&bin.join("renamed encoder 映像"), CUSTOM);
        executable(&bin.join("x264"), STOCK);
        executable(&bin.join("invalid-format"), "not an executable format\n");
        executable(
            &bin.join(std::ffi::OsString::from_vec(b"encoder-\xff".to_vec())),
            CUSTOM,
        );
        fs::write(bin.join("nonexec"), CUSTOM).expect("nonexecutable file");
        fs::create_dir_all(bin.join("directory")).expect("directory");
        let log_path = root.join("child.log");
        let log = fs::File::create(&log_path).expect("log");
        let mut child = Command::new(std::env::current_exe().expect("test binary"))
            .args([
                "--exact",
                &format!("core::encoder::executable_resolution::{test}"),
                "--nocapture",
            ])
            .env("PATH", &bin)
            .env("EXECUTABLE_RESOLUTION_ROOT", &root)
            .env("EXECUTABLE_RESOLUTION_CASE", case)
            .current_dir(&root)
            .stdout(log.try_clone().expect("log clone"))
            .stderr(log)
            .spawn()
            .expect("child");
        let start = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait().expect("wait") {
                break Some(status);
            }
            if start.elapsed() > Duration::from_secs(10) {
                child.kill().expect("kill blocked child");
                child.wait().expect("reap blocked child");
                break None;
            }
            thread::sleep(Duration::from_millis(10));
        };
        let log = fs::read_to_string(log_path).expect("log");
        if !status.is_some_and(|status| status.success())
            || !log.contains("1 passed; 0 failed")
            || !log.contains(&format!("encoder path {case}: assertions completed"))
        {
            failures.push(format!("{case}: {status:?}\n{log}"));
        }
        print!("{log}");
        if case != "default" && root.join("stock").exists() {
            failures.push(format!("{case}: stock fallback ran"));
        }
        if case != "spawn-input" && root.join("decoded").exists() {
            failures.push(format!("{case}: input decoded before executable rejection"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn input() -> Input {
    Input::from_data(&andean_condor::models::input::Input::VapourSynthScript {
        source: andean_condor::models::input::VapourSynthScriptSource::Text(
            "from vapoursynth import core, YUV420P8\nimport os\nclip = core.std.BlankClip(width=64,height=64,length=1,format=YUV420P8)\ndef decode(n):\n    open(os.path.join(os.environ['EXECUTABLE_RESOLUTION_ROOT'], 'decoded'), 'w').close()\n    return clip\ncore.std.FrameEval(clip, decode).set_output()\n".into(),
        ),
        variables: HashMap::new(),
        index: 0,
        filters: vec![],
        stream_concurrently: false,
    }).expect("blank input")
}

#[allow(
    clippy::unnecessary_debug_formatting,
    reason = "Check escaped OS paths, including NUL, in diagnostics"
)]
fn assert_requested_encoder_path() {
    let Some(root) = std::env::var_os("EXECUTABLE_RESOLUTION_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let case = std::env::var("EXECUTABLE_RESOLUTION_CASE").expect("case");
    let requested = match case.as_str() {
        "path" => PathBuf::from("renamed encoder 映像"),
        "absolute" => root.join("bin/renamed encoder 映像"),
        "relative" => PathBuf::from("./bin/renamed encoder 映像"),
        "nonutf8" => root.join("bin").join(std::ffi::OsString::from_vec(b"encoder-\xff".to_vec())),
        "missing-absolute" => root.join("bin/executable-resolution-missing-custom"),
        "missing-relative" => PathBuf::from("./bin/executable-resolution-missing-custom"),
        "nonexec" | "nonexec-input" | "nonexec-stream-pass" => root.join("bin/nonexec"),
        "directory" | "directory-input-pass" => root.join("bin/directory"),
        "nul" | "nul-stream" => PathBuf::from("bad\0encoder"),
        "spawn-input" | "spawn-stream" => root.join("bin/invalid-format"),
        _ => PathBuf::from("executable-resolution-missing-custom"),
    };
    let mut encoder = Encoder::default_from_base(&EncoderBase::X264, false);
    if case != "default" {
        encoder.set_executable(Some(requested.clone()));
    }
    let (progress, reports) = mpsc::channel();
    if matches!(
        case.as_str(),
        "path" | "absolute" | "relative" | "nonutf8" | "default"
    ) {
        encoder.validate().expect("requested executable resolves");
        if case != "default" {
            assert_eq!(encoder.executable().as_os_str(), requested.as_os_str());
            let resolved = encoder.resolve_executable().expect("resolved binary");
            assert_eq!(
                fs::canonicalize(resolved).expect("resolved file"),
                fs::canonicalize(if case == "nonutf8" {
                    requested
                } else {
                    root.join("bin/renamed encoder 映像")
                })
                .expect("fixture file")
            );
            assert_eq!(
                encoder.version_text().as_deref(),
                Some("executable resolution")
            );
            assert!(encoder.supports_capability(EncoderCapability::Zoning));
            assert_eq!(encoder.container(), Some(Container::AnnexB));
        }
        let (sender, frames) = crossbeam_channel::unbounded();
        sender
            .send(Cursor::new(
                b"finite executable resolution input\n".to_vec(),
            ))
            .expect("frame");
        drop(sender);
        encoder
            .encode_with_stream(frames, &root.join("out.264"), progress, &super::clip_info())
            .expect("custom spawn");
        if case != "default" {
            assert_eq!(
                fs::read(root.join("consumed")).expect("consumed input"),
                b"finite executable resolution input\n"
            );
            assert!(root.join("custom").exists());
        } else {
            assert!(root.join("stock").exists());
        }
        return;
    }
    if case == "optional" {
        assert!(!encoder.supports_capability(EncoderCapability::Zoning));
        assert_eq!(encoder.container(), None);
        assert_eq!(encoder.version_text(), None);
        return;
    }
    let result = if matches!(
        case.as_str(),
        "missing" | "missing-absolute" | "missing-relative" | "nonexec" | "directory" | "nul"
    ) {
        encoder.validate()
    } else {
        if case.starts_with("two-") {
            *encoder.passes_mut().expect("passes") = EncoderPasses::All(2);
        }
        let output = root.join("out.264");
        if case.contains("input") {
            let mut input = input();
            if case.ends_with("input-pass") || case == "spawn-input" {
                encoder.encode_pass_with_input(&mut input, &output, 0, 1, (1, 1), progress, None)
            } else {
                encoder.encode_with_input(&mut input, &output, 0, 1, progress)
            }
        } else {
            let (sender, frames) = crossbeam_channel::unbounded();
            // An open, empty receiver proves rejection precedes channel waiting.
            let result = if case.ends_with("stream-pass") || case == "spawn-stream" {
                encoder.encode_pass_with_stream(
                    (1, 1),
                    frames,
                    &output,
                    progress,
                    &super::clip_info(),
                    None,
                )
            } else {
                encoder.encode_with_stream(frames, &output, progress, &super::clip_info())
            };
            drop(sender);
            result
        }
        .map(|_| ())
    };
    let error = format!("{:#}", result.expect_err("custom executable must fail"));
    assert!(error.contains("x264"), "{error}");
    assert!(error.contains(&format!("{requested:?}")), "{error}");
    assert!(
        error.contains(if case.starts_with("spawn-") {
            "spawn"
        } else {
            "resolv"
        }),
        "{error}"
    );
    assert!(reports.try_recv().is_err(), "preflight emitted progress");
    if !case.starts_with("spawn-") {
        assert!(!encoder.supports_capability(EncoderCapability::Zoning));
        assert_eq!(encoder.container(), None);
        assert_eq!(encoder.version_text(), None);
    }
    assert!(
        !fs::read_dir(root).expect("listing").any(|e| {
            e.expect("entry").file_name().to_string_lossy().starts_with(".av1an-stats-")
        })
    );
}
