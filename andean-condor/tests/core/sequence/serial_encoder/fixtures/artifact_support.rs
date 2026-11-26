//! Public serial regressions: accepting progress/status without a private validated
//! artifact, or overwriting a final path, must fail these checks.
use super::*;
use condor::{
    core::{
        encoder::{EncoderResult, StringOrBytes},
        output::Output,
    },
    models::{
        encoder::{Encoder, EncoderBase, EncoderPasses, cli_parameter::CLIParameter},
        input::{ImportMethod, Input as InputModel, VapourSynthScriptSource},
        scene::Scene,
        sequence::{DefaultSequenceConfig, DefaultSequenceData},
    },
    utils::cancellation::{Cancellation, Cancelled},
};
use std::{collections::HashMap, fs, os::unix::fs::PermissionsExt, process::Command};

const CODEC: &str = r#"#!/bin/sh
set -eu
out= root= pass=1
while [ "$#" -gt 0 ]; do
 case "$1" in -o) out=$2; shift ;; --serial-encoder-root) root=$2; shift ;; --pass) pass=$2; shift ;; esac
 shift
done
printf '%s\n' "$out" >> "$root/starts"
cat >/dev/null
if [ "$ENCODER_FIXTURE_CASE" = graceful ] && [ "$pass" = 1 ]; then
 printf ready > "$root/active"
 while [ ! -f "$root/release" ]; do :; done
fi
case "$ENCODER_FIXTURE_CASE" in
 missing|optout-missing) exit 0 ;;
 zero|optout-zero) : > "$out" ;;
 corrupt) printf corrupt > "$out" ;;
 second) if [ -f "$root/first" ]; then printf corrupt > "$out"; else cp "$root/fixture" "$out"; touch "$root/first"; fi ;;
 *) cp "$root/fixture" "$out" ;;
esac
case "$ENCODER_FIXTURE_CASE" in
 race) printf caller-owned > "$root/scenes/00000.264"; printf 'raw\377stdout'; printf 'raw\376stderr' >&2 ;;
 status|optout-status) printf 'raw\377stdout' ; printf 'raw\376stderr' >&2; exit 42 ;;
 probe-force|wrong) printf 'raw\377stdout' ; printf 'raw\376stderr' >&2 ;;
esac
printf done > "$root/codec-done"
"#;

pub(super) fn serial_cases(test: &str, cases: &[&str]) {
    if let Ok(case) = std::env::var("ENCODER_FIXTURE_CASE") {
        assert!(
            cases.contains(&case.as_str()),
            "serial case belongs to this contract"
        );
        serial_artifact_child();
        println!(
            "serial-encoder native={} case={case}: assertions completed",
            std::env::var("SERIAL_ENCODER_NATIVE").expect("input kind")
        );
        return;
    }
    let temp = tempfile::tempdir().expect("fixture");
    let root = temp.path();
    fs::write(
        root.join("watchdog.rs"),
        include_str!("../../../encoder/fixtures/fake_encoder.rs"),
    )
    .expect("watchdog");
    assert!(
        Command::new("rustc")
            .arg(root.join("watchdog.rs"))
            .arg("-o")
            .arg(root.join("watchdog"))
            .status()
            .expect("rustc")
            .success()
    );
    let mut failures = Vec::new();
    for native in [false, true] {
        for &case in cases {
            let job = root.join(format!("{native}-{case}"));
            fs::create_dir_all(job.join("scenes")).expect("job");
            let output = Command::new(root.join("watchdog"))
                .arg("--encoder-fixture-watchdog")
                .arg(std::env::current_exe().expect("test binary"))
                .arg(&job)
                .arg(case)
                .arg(test)
                .env("SERIAL_ENCODER_NATIVE", if native { "1" } else { "0" })
                .output()
                .expect("bounded child");
            let text = format!(
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            eprintln!("serial-encoder native={native} case={case}\n{text}");
            if !output.status.success()
                || !job.join("verified").exists()
                || !text.contains("test result: ok. 1 passed;")
                || !text.contains(&format!(
                    "serial-encoder native={} case={case}: assertions completed",
                    u8::from(native)
                ))
                || !text
                    .lines()
                    .any(|l| l == "encoder fixture watchdog reaped 0 descendants; timeout=false")
            {
                failures.push(format!("native={native} {case}: {text}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn input(root: &Path, native: bool) -> Input {
    if native {
        let path = root.join("input.y4m");
        let mut bytes = b"YUV4MPEG2 W64 H64 F24:1 Ip A1:1 C420jpeg\n".to_vec();
        for _ in 0..3 {
            bytes.extend_from_slice(b"FRAME\n");
            bytes.extend(vec![48; 64 * 64 * 3 / 2]);
        }
        fs::write(&path, bytes).expect("native source");
        Input::from_video(&InputModel::Video {
            path,
            import_method: ImportMethod::FFMS2 {
                index: None,
                cache_path: Some(root.join("input.ffindex")),
            },
            filters: vec![],
        })
        .expect("FFMS2")
    } else {
        let path = root.join("input.vpy");
        fs::write(&path, "from vapoursynth import core,YUV420P8\ncore.std.BlankClip(width=64,height=64,length=3,format=YUV420P8).set_output()\n").expect("VS source");
        Input::from_data(&InputModel::VapourSynthScript {
            source: VapourSynthScriptSource::Path(path),
            variables: HashMap::new(),
            index: 0,
            filters: vec![],
            stream_concurrently: false,
        })
        .expect("VS")
    }
}

fn serial_artifact_child() {
    let Some(root) = std::env::var_os("ENCODER_FIXTURE_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let case = std::env::var("ENCODER_FIXTURE_CASE").expect("case");
    let native = std::env::var("SERIAL_ENCODER_NATIVE").as_deref() == Ok("1");
    let frames = if case.ends_with("wrong") { "2" } else { "3" };
    let fixture = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=s=64x64:r=24",
            "-frames:v",
            frames,
            "-c:v",
            "libx264",
            "-threads",
            "1",
            "-f",
            "h264",
        ])
        .arg(root.join("fixture"))
        .output()
        .expect("fixture");
    assert!(fixture.status.success(), "{fixture:?}");
    if case == "truncated" {
        fs::OpenOptions::new()
            .write(true)
            .open(root.join("fixture"))
            .expect("fixture")
            .set_len(32)
            .expect("truncate");
    }
    fs::write(root.join("codec"), CODEC).expect("codec");
    fs::set_permissions(root.join("codec"), fs::Permissions::from_mode(0o755)).expect("executable");
    let mut encoder = Encoder::default_from_base(&EncoderBase::X264, true);
    encoder.set_append_color_data(false);
    encoder.set_executable(Some(root.join("codec")));
    encoder.parameters_mut().insert(
        "serial-encoder-root".into(),
        CLIParameter::new_string("--", " ", root.to_str().expect("path")),
    );
    if case == "real" {
        encoder.set_executable(Some(which::which("x264").expect("x264")));
        encoder.parameters_mut().remove("serial-encoder-root");
        encoder
            .parameters_mut()
            .insert("demuxer".into(), CLIParameter::new_string("--", " ", "y4m"));
        encoder
            .parameters_mut()
            .insert("threads".into(), CLIParameter::new_number("--", " ", 1.0));
    }
    if case == "graceful" {
        *encoder.passes_mut().expect("passes") = EncoderPasses::All(2);
    }
    let ranges = match case.as_str() {
        "zero-range" => vec![(0, 0)],
        "reversed-range" => vec![(2, 1)],
        "decode" => vec![(0, 4)],
        "second" | "graceful" => vec![(0, 3), (0, 3)],
        _ => vec![(0, 3)],
    };
    let scenes = ranges
        .into_iter()
        .map(|(start_frame, end_frame)| Scene {
            start_frame,
            end_frame,
            sub_scenes: None,
            encoder: encoder.clone(),
            sequence_data: DefaultSequenceData::default(),
        })
        .collect();
    let mut condor = Condor::<DefaultSequenceData, DefaultSequenceConfig>::new(
        input(&root, native),
        Output {
            path: root.join("unused"),
            overwrite: false,
            tags: HashMap::new(),
            video_tags: HashMap::new(),
        },
        encoder,
        scenes,
        None,
        Box::new(|_| Ok(())),
    );
    if case.starts_with("specific-first") {
        *condor.scenes[0].encoder.passes_mut().expect("manual scene pass") =
            EncoderPasses::Specific(1, 2);
        let scenes = root.join("scenes");
        for (name, bytes) in [
            ("00000.log", b"caller statistics".as_slice()),
            ("00000.log.mbtree", b"caller mbtree"),
            ("caller.264", b"caller output"),
        ] {
            fs::write(scenes.join(name), bytes).expect("caller sentinel");
        }
        let mut serial = SerialEncoder::new(&scenes);
        serial.validate_frame_count = !case.ends_with("optout");
        let validated = serial.validate(&mut condor);
        let (tx, rx) = sync::mpsc::channel();
        drop(rx);
        let executed = serial.execute(&mut condor, tx, Cancellation::default());
        assert!(
            !root.join("starts").exists(),
            "non-final Specific must fail before codec launch: validate={validated:?}, execute={executed:?}"
        );
        for result in [validated, executed] {
            let error = result.expect_err("serial rejects non-final Specific before any work");
            let text = error.to_string();
            for expected in [
                "Serial scene 00000",
                "pass 1 of 2",
                "Encoder::encode_pass_with_stream",
                "PassStatistics",
            ] {
                assert!(text.contains(expected), "{text}");
            }
        }
        let info = match &condor.input {
            Input::Video { clip_info, .. }
            | Input::VapourSynth { clip_info, .. }
            | Input::VapourSynthScript { clip_info, .. } => clip_info,
        };
        assert!(
            info.is_none(),
            "manual-pass rejection must precede backend consumption"
        );
        assert_eq!(
            fs::read(scenes.join("00000.log")).expect("stats preserved"),
            b"caller statistics"
        );
        assert_eq!(
            fs::read(scenes.join("00000.log.mbtree")).expect("mbtree preserved"),
            b"caller mbtree"
        );
        assert_eq!(
            fs::read(scenes.join("caller.264")).expect("output preserved"),
            b"caller output"
        );
        assert_eq!(
            fs::read_dir(&scenes).expect("scene listing").count(),
            3,
            "no private output/statistics or final admitted"
        );
        fs::write(
            root.join("verified"),
            b"non-final Specific rejected before work",
        )
        .expect("verified");
        return;
    }
    if case.starts_with("preflight-") {
        fs::create_dir_all(root.join("empty-path")).expect("empty PATH");
        // SAFETY: Isolated child before any worker or controller threads exist.
        unsafe {
            std::env::set_var("PATH", root.join("empty-path"));
        }
        if case != "preflight-probe" {
            condor.encoder.set_executable(Some(root.join("missing-executable")));
        }
        if case == "preflight-settings" {
            condor
                .encoder
                .parameters_mut()
                .insert("crf".into(), CLIParameter::new_number("--", " ", f64::NAN));
        }
        let mut serial = SerialEncoder::new(&root.join("must-not-create"));
        for direct in [false, true] {
            let (tx, _rx) = sync::mpsc::channel();
            let result = if direct {
                serial.execute(&mut condor, tx, Cancellation::default())
            } else {
                serial.validate(&mut condor)
            };
            let error =
                result.expect_err("preflight must reject before clip_info, scratch, or launch");
            if case == "preflight-settings" {
                assert!(
                    error.is::<condor::models::encoder::EncoderSettingsError>(),
                    "{error:#}"
                );
            } else {
                assert!(
                    format!("{error:#}").contains(if case == "preflight-probe" {
                        "ffprobe"
                    } else {
                        "missing-executable"
                    }),
                    "{error:#}"
                );
            }
            assert!(!root.join("must-not-create").exists());
            assert!(!root.join("starts").exists());
            let info = match &condor.input {
                Input::Video { clip_info, .. }
                | Input::VapourSynth { clip_info, .. }
                | Input::VapourSynthScript { clip_info, .. } => clip_info,
            };
            assert!(
                info.is_none(),
                "preflight must not ask the backend for clip info"
            );
        }
        if case == "preflight-probe" {
            serial.validate_frame_count = false;
            serial
                .validate(&mut condor)
                .expect("count-only optout does not require ffprobe");
        }
        fs::write(root.join("verified"), b"preflight checked").expect("verified");
        return;
    }
    if case == "optout-no-probe" {
        fs::create_dir_all(root.join("bin")).expect("bin without ffprobe");
        for tool in ["cat", "cp"] {
            std::os::unix::fs::symlink(
                which::which(tool).expect("fixture tool"),
                root.join("bin").join(tool),
            )
            .expect("fixture tool link");
        }
        // SAFETY: Isolated child, before any worker or controller threads start.
        unsafe {
            std::env::set_var("PATH", root.join("bin"));
        }
    }
    let final_path = root.join("scenes/00000.264");
    fs::write(root.join("scenes/sentinel"), b"caller-owned").expect("sentinel");
    if case == "existing" {
        fs::write(&final_path, b"caller-owned").expect("existing final");
    }
    let cancellation = Cancellation::default();
    if case == "forced-before" {
        cancellation.request().expect("graceful");
        cancellation.request().expect("force");
    }
    // A fake *probe*, not a fake validator: assert the public owner keeps final
    // invisible until the real artifact validator has joined its process.
    if case == "private" || case == "probe-force" {
        fs::create_dir_all(root.join("bin")).expect("bin");
        let probe = root.join("bin/ffprobe");
        fs::write(&probe, format!("#!/bin/sh\nset -eu\nroot='{}'\ntest -f \"$root/codec-done\"\ntest ! -f \"$root/scenes/00000.264\"\nprintf ready > \"$root/probe-ready\"\nwhile [ ! -f \"$root/release\" ]; do :; done\nprintf '{{\"streams\":[{{\"nb_read_frames\":\"3\"}}]}}'\n",root.display())).expect("probe");
        fs::set_permissions(probe, fs::Permissions::from_mode(0o755)).expect("probe executable");
        // SAFETY: Isolated child, before any controller/worker threads start.
        unsafe {
            std::env::set_var(
                "PATH",
                format!(
                    "{}:{}",
                    root.join("bin").display(),
                    std::env::var("PATH").expect("PATH")
                ),
            );
        }
    }
    let controller = matches!(case.as_str(), "graceful" | "private" | "probe-force").then(|| {
        let root = root.clone();
        let signal = cancellation.clone();
        let kind = case.clone();
        thread::spawn(move || {
            let marker = if kind == "graceful" {
                "active"
            } else {
                "probe-ready"
            };
            let deadline = Instant::now() + std::time::Duration::from_secs(5);
            while !root.join(marker).exists() {
                assert!(Instant::now() < deadline, "handshake {marker} missing");
                thread::yield_now();
            }
            assert!(!root.join("scenes/00000.264").exists(), "no early final");
            if kind != "private" {
                signal.request().expect("graceful");
            }
            if kind == "probe-force" {
                signal.request().expect("force probe");
            }
            fs::write(root.join("release"), b"finish").expect("release");
        })
    });
    let (tx, rx) = sync::mpsc::channel();
    drop(rx);
    let mut serial = SerialEncoder::new(&root.join("scenes"));
    if case.starts_with("optout-") {
        serial.validate_frame_count = false;
    }
    let result = serial.execute(&mut condor, tx, cancellation.clone());
    if let Some(controller) = controller {
        controller.join().expect("controller joined");
    }
    if matches!(
        case.as_str(),
        "valid" | "real" | "private" | "graceful" | "optout-wrong" | "optout-no-probe"
    ) {
        result.expect("validated serial output");
        assert!(final_path.metadata().expect("published final").len() > 0);
        if case == "graceful" {
            assert_eq!(
                fs::read_to_string(root.join("starts")).expect("passes").lines().count(),
                2
            );
        }
        cancellation.request().expect("after publication");
        cancellation.request().expect("force after publication");
        assert!(final_path.exists(), "committed work survives cancellation");
    } else {
        let error = result.expect_err("invalid/interrupted serial output cannot succeed");
        if case.ends_with("status") || matches!(case.as_str(), "probe-force" | "wrong" | "race") {
            let diagnostic = error.downcast_ref::<EncoderResult>().expect("typed native status");
            assert_eq!(
                diagnostic.status.code(),
                Some(if case.ends_with("status") { 42 } else { 0 })
            );
            let bytes = |value: &StringOrBytes| match value {
                StringOrBytes::String(s) => s.as_bytes().to_vec(),
                StringOrBytes::Bytes(b) => b.clone(),
            };
            assert_eq!(bytes(&diagnostic.stdout), b"raw\xffstdout");
            assert_eq!(bytes(&diagnostic.stderr), b"raw\xfestderr");
        }
        if case == "forced-before" || case == "probe-force" {
            assert_eq!(error.downcast_ref::<Cancelled>(), Some(&Cancelled::Forced));
        }
        if case == "probe-force" {
            assert!(
                error
                    .to_string()
                    .starts_with("Serial scene 00000 failed: forced cancellation requested"),
                "forced validation keeps the scene headline too: {error}"
            );
        }
        if case == "wrong" || case == "race" {
            let plain = format!("{error}");
            let detailed = format!("{error:#}");
            let headline = if case == "wrong" {
                "Serial scene 00000 failed: decoded frame count 2 does not match expected 3"
            } else {
                "Serial scene 00000 failed: File exists"
            };
            assert!(
                plain.starts_with(headline),
                "plain Display must lead with the actual failure, not codec success: {plain}"
            );
            assert!(detailed.starts_with(headline), "{detailed}");
            for diagnostic in ["Encoder (x264", "STDOUT:", "STDERR:"] {
                assert_eq!(
                    detailed.matches(diagnostic).count(),
                    1,
                    "no copied full diagnostic chain: {detailed}"
                );
            }
            assert_eq!(
                detailed
                    .matches(if case == "wrong" {
                        "Validating encoded artifact "
                    } else {
                        "Publishing serial scene "
                    })
                    .count(),
                1,
                "actual cause context appears once: {detailed}"
            );
            if case == "wrong" {
                assert!(
                    error
                        .root_cause()
                        .to_string()
                        .starts_with("decoded frame count 2 does not match expected 3")
                );
            } else {
                assert!(
                    error.chain().any(|cause| {
                        cause
                            .downcast_ref::<std::io::Error>()
                            .is_some_and(|error| error.kind() == std::io::ErrorKind::AlreadyExists)
                    }),
                    "typed original no-clobber cause retained: {detailed}"
                );
            }
        }
        if case == "existing" || case == "race" {
            assert_eq!(
                fs::read(&final_path).expect("sentinel final"),
                b"caller-owned"
            );
        } else if case == "second" {
            assert_eq!(
                fs::read(&final_path).expect("first committed"),
                fs::read(root.join("fixture")).expect("fixture")
            );
        } else {
            assert!(
                !final_path.exists(),
                "failed final must not exist: {error:#}"
            );
        }
        if case == "feed-panic" {
            assert!(
                format!("{error:#}").contains("serial-encoder deliberate feed panic"),
                "{error:#}"
            );
            assert!(
                root.join("codec-done").exists(),
                "panic path joins the happy encoder before cleanup/return"
            );
        }
        if case == "encoder-panic" || case == "relay-panic" {
            assert!(format!("{error:#}").contains("panicked"), "{error:#}");
        }
        if case.ends_with("range") || case == "forced-before" {
            assert!(
                !root.join("starts").exists(),
                "no codec for unadmitted/empty work"
            );
        }
    }
    assert!(!root.join("scenes/00001.264").exists());
    assert_eq!(
        fs::read(root.join("scenes/sentinel")).expect("caller sentinel"),
        b"caller-owned"
    );
    assert!(
        fs::read_dir(root.join("scenes")).expect("scene listing").all(|e| {
            let name = e.expect("entry").file_name();
            matches!(name.to_str(), Some("sentinel" | "00000.264"))
        }),
        "private artifacts must be gone after joins"
    );
    fs::write(root.join("verified"), b"public child assertions passed").expect("verified");
}
