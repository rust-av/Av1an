use super::*;
use std::fs;

fn bytes(value: &StringOrBytes) -> &[u8] {
    match value {
        StringOrBytes::String(s) => s.as_bytes(),
        StringOrBytes::Bytes(b) => b,
    }
}

fn diagnostics(result: &EncoderResult, dropped_after_first: bool) {
    assert!(
        !result.parameters.iter().any(|parameter| parameter == "-y"),
        "non-FFmpeg argv unchanged"
    );
    let mut stdout = b"POC: 0\r\n".to_vec();
    if dropped_after_first {
        stdout.extend_from_slice(b"POC: 1\r\n");
    }
    stdout.extend(vec![b'o'; 512 * 1024]);
    stdout.extend_from_slice(b"\r\nbad\xff\rstdout tail");
    let mut stderr = b"1/4 frames\r\n".to_vec();
    if dropped_after_first {
        stderr.extend_from_slice(b"2/4 frames\r\n");
    }
    stderr.extend(vec![b'e'; 512 * 1024]);
    stderr.extend_from_slice(b"\r\nbad\xfe\rstderr tail");
    assert_eq!(bytes(&result.stdout).len(), stdout.len(), "stdout length");
    assert_eq!(bytes(&result.stderr).len(), stderr.len(), "stderr length");
    assert_eq!(bytes(&result.stdout), stdout);
    assert_eq!(bytes(&result.stderr), stderr);
}

#[test]
fn public_encoding_drains_diagnostics_and_joins_failures() {
    run_diagnostics_cases(&[
        "stream",
        "avm",
        "drop-after-first",
        "avm-drop-after-first",
        "input",
        "input-drop-after-first",
        "decode",
        "exit0",
        "exit42",
        "input-exit0",
        "input-exit42",
        "serial0",
        "serialfew",
        "serialmany",
        "serialmultipass",
        "serialdecode",
        "serialencode",
        "watchdog",
    ]);
}

#[test]
fn vvenc_public_input_and_stream_report_progress() {
    run_diagnostics_cases(&["vvenc-input", "vvenc-stream"]);
}

#[test]
fn owned_ffmpeg_rejects_stale_outputs_and_preserves_public_controls() {
    run_diagnostics_cases(&[
        "owned-ffmpeg-parallel-stale",
        "owned-ffmpeg-parallel-decoded-stale",
        "owned-ffmpeg-serial-stale",
        "owned-ffmpeg-parallel-raw-y",
        "owned-ffmpeg-parallel-raw-n",
        "owned-ffmpeg-parallel-raw-yn",
        "owned-ffmpeg-public",
    ]);
}

fn run_diagnostics_cases(cases: &[&str]) {
    let temp = tempfile::tempdir().expect("fixture");
    let root = temp.path();
    fs::write(
        root.join("fake.rs"),
        include_str!("fixtures/fake_encoder.rs"),
    )
    .expect("fixture source");
    crate::core::encoder::tests::support::run_bounded(
        Command::new("rustc").arg(root.join("fake.rs")).arg("-o").arg(root.join("fake")),
        &root.join("rustc.log"),
    );
    let mut failed = Vec::new();
    for &case in cases {
        let log_path = root.join(format!("{case}.log"));
        let file = fs::File::create(&log_path).expect("log");
        let status = Command::new(root.join("fake"))
            .arg("--encoder-fixture-watchdog")
            .arg(std::env::current_exe().expect("test binary"))
            .arg(root)
            .arg(case)
            .stdout(file.try_clone().expect("clone log"))
            .stderr(file)
            .status()
            .expect("bounded child");
        let log = fs::read_to_string(log_path).expect("child log");
        if case.starts_with("owned-ffmpeg-") {
            eprintln!("{case}: {status}\n{log}");
        }
        if case == "watchdog" {
            assert_eq!(
                status.code(),
                Some(124),
                "watchdog must bound a stuck process tree"
            );
            assert!(log.contains("reaped 1 descendants; timeout=true"), "{log}");
        } else if !status.success()
            || !log.contains("1 passed; 0 failed")
            || !log
                .lines()
                .any(|line| line == "encoder fixture watchdog reaped 0 descendants; timeout=false")
        {
            failed.push(format!("{case}: {status}\n{log}"));
        } else if let Some(expected) = match case {
            "serial0" => Some(1),
            "serialfew" => Some(4),
            "serialmany" => Some(9),
            "serialmultipass" => Some(18),
            _ => None,
        } {
            let actual = log.lines().filter(|s| s.starts_with("Progress: ")).count();
            if actual != expected {
                failed.push(format!(
                    "{case}: report count {actual}, expected {expected}"
                ));
            }
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

#[test]
fn diagnostics_child() {
    let Some(root) = std::env::var_os("ENCODER_FIXTURE_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let case = std::env::var("ENCODER_FIXTURE_CASE").expect("case");
    if case.starts_with("owned-ffmpeg-") {
        owned_ffmpeg_case(&root, &case);
        return;
    }
    if case.starts_with("vvenc-") {
        vvenc_case(&root, &case);
        return;
    }
    if case == "watchdog" {
        let mut descendant = Command::new(root.join("fake"))
            .arg("--encoder-fixture-stall")
            .spawn()
            .expect("stalled descendant");
        fs::write(root.join("watchdog-started"), "descendant started").expect("watchdog handshake");
        // Keep the handle alive without racing the watchdog's subreaper during group SIGKILL.
        thread::sleep(std::time::Duration::from_secs(30));
        descendant.wait().expect("fixture outlived watchdog deadline");
        panic!("watchdog did not stop the fixture tree");
    }
    let mut encoder = Encoder::default_from_base(
        &if case.starts_with("avm") {
            EncoderBase::AVM
        } else {
            EncoderBase::X264
        },
        true,
    );
    encoder.set_executable(Some(root.join("fake")));
    encoder.set_append_color_data(false);
    if case.starts_with("serial") {
        serial_case(&root, &case, encoder);
        return;
    }
    let (progress, rx) = mpsc::channel::<EncodeProgress>();
    let dropped_after_first = case.contains("drop-after-first");
    let observer = if dropped_after_first {
        let marker = root.join(format!("{case}.dropped"));
        encoder.parameters_mut().insert(
            "encoder-fixture-drop-marker".into(),
            CLIParameter::new_string("--", " ", marker.to_str().expect("utf8")),
        );
        Some(thread::spawn(move || {
            assert_eq!(rx.recv().expect("first progress").frame, 1);
            drop(rx);
            fs::write(marker, "observer dropped").expect("drop handshake");
        }))
    } else {
        drop(rx);
        None
    };
    let output = root.join(format!("{case}.264"));
    let early = case.contains("exit");
    if early {
        encoder.parameters_mut().insert(
            "encoder-fixture-exit".into(),
            CLIParameter::new_string("--", " ", case.rsplit("exit").next().expect("exit code")),
        );
    }
    let result = if case.starts_with("input") || case == "decode" {
        let script = root.join(format!("{case}.vpy"));
        let side = if early { 4096 } else { 64 };
        fs::write(&script, format!("from vapoursynth import core, YUV420P8\ncore.std.BlankClip(width={side},height={side},length=1,format=YUV420P8).set_output()\n")).expect("blank clip");
        let mut input = Input::from_data(&crate::models::input::Input::VapourSynthScript {
            source: crate::models::input::VapourSynthScriptSource::Path(script),
            variables: HashMap::new(),
            index: 0,
            filters: vec![],
            stream_concurrently: false,
        })
        .expect("input");
        if case == "decode" {
            *encoder.passes_mut().expect("passes") = EncoderPasses::All(2);
            encoder.parameters_mut().insert(
                "encoder-fixture-marker".into(),
                CLIParameter::new_string("--", " ", root.join("reaped").to_str().expect("utf8")),
            );
        }
        encoder.encode_with_input(
            &mut input,
            &output,
            0,
            if case == "decode" { 2 } else { 1 },
            progress,
        )
    } else {
        let (tx, frames) = crossbeam_channel::unbounded();
        tx.send(Cursor::new(vec![
            0;
            if early { 16 * 1024 * 1024 } else { 64 }
        ]))
        .expect("finite frames");
        drop(tx);
        if dropped_after_first {
            encoder.encode_pass_with_stream(
                (1, 1),
                frames,
                &output,
                progress,
                &crate::core::encoder::tests::clip_info(),
                None,
            )
        } else {
            encoder.encode_with_stream(
                frames,
                &output,
                progress,
                &crate::core::encoder::tests::clip_info(),
            )
        }
    };
    if case == "decode" || early {
        let error = result.expect_err("decode/writer failure must propagate after joining");
        let diagnostic = error
            .chain()
            .find_map(|e| e.downcast_ref::<EncoderResult>())
            .expect("recoverable complete encoder diagnostics");
        diagnostics(diagnostic, dropped_after_first);
        if early {
            assert!(format!("{error:#}").contains("STDIN"), "{error:#}");
            assert_eq!(
                diagnostic.status.code(),
                Some(if case.ends_with('0') { 0 } else { 42 })
            );
        } else {
            assert_eq!(
                fs::read_to_string(root.join("reaped")).expect("child reaped before return"),
                "stats_alive=true"
            );
            assert!(!fs::read_dir(&root).expect("listing").any(|e| {
                e.expect("entry").file_name().to_string_lossy().starts_with(".av1an-stats-")
            }));
        }
    } else {
        let result = result.unwrap_or_else(|error| {
            panic!(
                "optional observer cannot fail an encode: {}",
                error.to_string().lines().next().expect("error")
            )
        });
        assert!(result.status.success());
        diagnostics(&result, dropped_after_first);
    }
    if let Some(observer) = observer {
        observer.join().expect("observer joined");
    }
}

// Removing owned-only overwrite must break native stale encodes; extending
// it to public APIs must break the nonowned sentinel controls below.
fn owned_ffmpeg_case(root: &Path, case: &str) {
    use crate::{
        core::{
            Condor,
            output::Output,
            sequence::{
                Sequence,
                parallel_encoder::{ParallelEncoder, ParallelEncoderOptions, Task},
                serial_encoder::SerialEncoder,
            },
        },
        models::{
            input::{ImportMethod, Input as InputModel, VapourSynthScriptSource},
            scene::Scene,
            sequence::{DefaultSequenceConfig, DefaultSequenceData},
        },
        utils::cancellation::Cancellation,
    };
    use std::{collections::VecDeque, os::unix::fs::PermissionsExt};

    let job = root.join(case);
    fs::create_dir_all(&job).expect("isolated case");
    let root = job.as_path();
    let native = case != "owned-ffmpeg-parallel-decoded-stale";
    let source = root.join(if native { "input.y4m" } else { "input.vpy" });
    let source_bytes = if native {
        let mut data = b"YUV4MPEG2 W64 H64 F24:1 Ip A1:1 C420jpeg\n".to_vec();
        for _ in 0..2 {
            data.extend_from_slice(b"FRAME\n");
            data.extend(vec![48; 64 * 64 * 3 / 2]);
        }
        data
    } else {
        b"from vapoursynth import core,YUV420P8\ncore.std.BlankClip(width=64,height=64,length=2,format=YUV420P8).set_output()\n".to_vec()
    };
    fs::write(&source, &source_bytes).expect("source");
    let mut input = if native {
        Input::from_video(&InputModel::Video {
            path: source.clone(),
            import_method: ImportMethod::FFMS2 {
                index: None,
                cache_path: Some(root.join("input.ffindex")),
            },
            filters: vec![],
        })
    } else {
        Input::from_data(&InputModel::VapourSynthScript {
            source: VapourSynthScriptSource::Path(source.clone()),
            variables: HashMap::new(),
            index: 0,
            filters: vec![],
            stream_concurrently: false,
        })
    }
    .expect("real input");
    let mut encoder = Encoder::default_from_base(&EncoderBase::FFmpeg, true);
    encoder.set_append_color_data(false);
    encoder.parameters_mut().extend(CLIParameter::new_strings(
        "-",
        " ",
        &[("c:v", "ffv1"), ("f", "matroska"), ("v", "error")],
    ));
    let rejects_n = case.ends_with("raw-n") || case.ends_with("raw-yn");
    if case.contains("raw-") {
        for flag in case.rsplit('-').next().expect("raw options").chars() {
            encoder
                .parameters_mut()
                .insert(flag.to_string(), CLIParameter::new_bool("-", true));
        }
    }
    let wrapper = root.join("ffmpeg-wrapper");
    fs::write(&wrapper, format!(
        "#!/bin/sh\nset -eu\nfor out do :; done\nprintf '%s\\n' \"$@\" > '{}/argv-'$$\n{}exec '{}' \"$@\"\n",
        root.display(),
        if case.ends_with("stale") { "test -f \"$out\" || printf stale-owned-attempt > \"$out\"\n" } else { "" },
        which::which("ffmpeg").expect("ffmpeg").display(),
    )).expect("native wrapper");
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).expect("wrapper executable");
    encoder.set_executable(Some(wrapper));
    let old_final = root.join("user-final.mkv");
    fs::write(&old_final, b"existing user final").expect("old final");
    let info = input.clip_info().expect("clip info");
    if case == "owned-ffmpeg-public" {
        // A matching prefix, owned statistics, or a missing basename is not
        // output ownership. Exercise both public input/stream and pass APIs.
        let caller = root.join(".av1an-attempt-caller");
        fs::create_dir_all(&caller).expect("nonowned prefix");
        for manual in [false, true] {
            for decoded in [false, true] {
                for isolated in [false, true] {
                    let total = 1;
                    let output = caller.join(format!("{manual}-{decoded}-{isolated}.mkv"));
                    fs::write(&output, b"caller sentinel").expect("sentinel");
                    let stats = if isolated {
                        PassStatistics::new(&output)
                    } else {
                        PassStatistics::external(&output.with_extension("stats"))
                    }
                    .expect("statistics do not grant output ownership");
                    for parameters in [
                        encoder.parameters_with_output((total, total), &output, &info),
                        encoder.parameters_with_stats((total, total), &output, &info, stats.base()),
                        encoder.parameters_with_output(
                            (total, total),
                            &caller.join("missing.mkv"),
                            &info,
                        ),
                    ] {
                        assert!(!parameters.expect("public argv").iter().any(|p| p == "-y"));
                    }
                    let (tx, rx) = mpsc::channel();
                    drop(rx);
                    let result = if decoded {
                        if manual {
                            encoder.encode_pass_with_input(
                                &mut input,
                                &output,
                                0,
                                2,
                                (total, total),
                                tx,
                                Some(&stats),
                            )
                        } else {
                            encoder.encode_with_input(&mut input, &output, 0, 2, tx)
                        }
                    } else {
                        let (frames, rx) = crossbeam_channel::unbounded();
                        frames.send(Cursor::new(source_bytes.clone())).expect("Y4M stdin");
                        drop(frames);
                        if manual {
                            encoder.encode_pass_with_stream(
                                (total, total),
                                rx,
                                &output,
                                tx,
                                &info,
                                Some(&stats),
                            )
                        } else {
                            encoder.encode_with_stream(rx, &output, tx, &info)
                        }
                    };
                    // Some native FFmpeg builds return zero on overwrite refusal.
                    // Raw APIs preserve native status, not final-writer guarantees.
                    let diagnostic = match result {
                        Ok(result) => result,
                        Err(error) => error.downcast::<EncoderResult>().expect("native refusal"),
                    };
                    assert!(!diagnostic.parameters.iter().any(|p| p == "-y"));
                    let stderr = String::from_utf8_lossy(bytes(&diagnostic.stderr));
                    assert!(
                        stderr.contains("already exists")
                            && (stderr.contains("Exiting") || stderr.contains("Not overwriting")),
                        "{diagnostic:?}"
                    );
                    assert_eq!(fs::read(&output).expect("preserved"), b"caller sentinel");
                    eprintln!(
                        "public manual={manual} decoded={decoded} isolated_stats={isolated}: native status={} no generated -y; sentinel preserved",
                        diagnostic.status
                    );
                }
            }
        }
        // Explicit raw -y retains its existing low-level native meaning; it is
        // not C06 final-output consent and is not inserted into saved defaults.
        encoder.parameters_mut().insert("y".into(), CLIParameter::new_bool("-", true));
        let raw_output = caller.join("explicit-y.mkv");
        fs::write(&raw_output, b"explicit caller overwrite").expect("raw sentinel");
        let (tx, _rx) = mpsc::channel();
        let result = encoder
            .encode_with_input(&mut input, &raw_output, 0, 2, tx)
            .expect("explicit raw -y");
        assert_eq!(result.parameters.iter().filter(|p| *p == "-y").count(), 1);
        decoded_frames(&raw_output);
    } else {
        let scenes = root.join("scenes");
        fs::create_dir_all(&scenes).expect("scenes");
        if case.starts_with("owned-ffmpeg-serial-") {
            let scene = Scene {
                start_frame: 0,
                end_frame: 2,
                sub_scenes: None,
                encoder: encoder.clone(),
                sequence_data: DefaultSequenceData::default(),
            };
            let mut condor = Condor::<DefaultSequenceData, DefaultSequenceConfig>::new(
                input,
                Output {
                    path: old_final.clone(),
                    overwrite: false,
                    tags: HashMap::new(),
                    video_tags: HashMap::new(),
                },
                encoder,
                vec![scene],
                None,
                Box::new(|_| Ok(())),
            );
            let (tx, _rx) = mpsc::channel();
            let result = SerialEncoder::new(&scenes).execute_cancel(
                &mut condor,
                tx,
                Cancellation::default(),
            );
            eprintln!("serial result={result:?}");
            result.expect("owned serial FFmpeg output");
            decoded_frames(&scenes.join("00000.mkv"));
        } else {
            let tasks = (0..2)
                .map(|index| Task {
                    original_index: index,
                    index,
                    frame_indices: vec![0, 1],
                    sub_scenes: None,
                    encoder: encoder.clone(),
                    output: scenes.join(format!("{index}.mkv")),
                })
                .collect::<VecDeque<_>>();
            let (tx, _rx) = mpsc::channel();
            let result = ParallelEncoder::encode_tasks_with_options(
                &mut input,
                2,
                tasks,
                tx,
                Cancellation::default(),
                ParallelEncoderOptions {
                    max_tries: 1,
                    validate_frame_count: true,
                },
            );
            eprintln!("parallel result={result:?}");
            if rejects_n {
                let error = result.expect_err("owned -n conflict must fail before native launch");
                assert!(
                    format!("{error:#}")
                        .contains("-n conflicts with owned FFmpeg attempt overwrite"),
                    "{error:#}"
                );
            } else {
                let results = result.expect("owned parallel FFmpeg output");
                for (index, result) in results.into_iter().enumerate() {
                    let result = result.expect("published scene").result;
                    assert!(result.status.success());
                    assert_eq!(result.parameters.iter().filter(|p| *p == "-y").count(), 1);
                    decoded_frames(&scenes.join(format!("{index}.mkv")));
                }
            }
        }
        let mut paths = std::collections::BTreeMap::new();
        for entry in fs::read_dir(root).expect("native records") {
            let path = entry.expect("entry").path();
            if path.file_name().expect("name").to_string_lossy().starts_with("argv-") {
                let args = fs::read_to_string(path).expect("argv");
                eprintln!("native argv:\n{args}");
                let args = args.lines().collect::<Vec<_>>();
                assert_eq!(args.iter().filter(|p| **p == "-y").count(), 1);
                let output = *args.last().expect("output");
                assert_ne!(Path::new(output), source);
                assert_ne!(Path::new(output), old_final);
                assert!(
                    !Path::new(output).exists(),
                    "joined private output cleaned after publication"
                );
                *paths.entry(output.to_owned()).or_insert(0) += 1;
            }
        }
        assert_eq!(
            paths.len(),
            if rejects_n {
                0
            } else if case.starts_with("owned-ffmpeg-serial-") {
                1
            } else {
                2
            }
        );
        assert!(paths.values().all(|count| *count == 1));
        assert!(fs::read_dir(&scenes).expect("cleanup").all(|entry| {
            !entry.expect("entry").file_name().to_string_lossy().starts_with(".av1an-")
        }));
    }
    assert_eq!(
        fs::read(&old_final).expect("old final preserved"),
        b"existing user final"
    );
    assert_eq!(fs::read(&source).expect("source preserved"), source_bytes);
}

fn decoded_frames(output: &Path) {
    let probe = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-count_frames",
            "-show_entries",
            "stream=nb_read_frames",
            "-of",
            "csv=p=0",
        ])
        .arg(output)
        .output()
        .expect("real output decode");
    assert!(probe.status.success(), "{probe:?}");
    assert_eq!(String::from_utf8(probe.stdout).expect("count").trim(), "2");
    eprintln!("decoded {}: 2 frames", output.display());
}

fn vvenc_case(root: &Path, case: &str) {
    let mut encoder = Encoder::default_from_base(&EncoderBase::VVenC, true);
    encoder.set_executable(Some(root.join("fake")));
    encoder.set_append_color_data(false);
    encoder.parameters_mut().insert(
        "vvenc-fixture-progress".into(),
        CLIParameter::new_bool("--", true),
    );
    let (tx, rx) = mpsc::channel();
    let output = root.join(format!("{case}.266"));
    let input_path = case == "vvenc-input";
    let result = if input_path {
        let script = root.join("vvenc.vpy");
        fs::write(&script, "from vapoursynth import core, YUV420P8\ncore.std.BlankClip(width=64,height=64,length=1,format=YUV420P8).set_output()\n").expect("finite input");
        let mut input = Input::from_data(&crate::models::input::Input::VapourSynthScript {
            source: crate::models::input::VapourSynthScriptSource::Path(script),
            variables: HashMap::new(),
            index: 0,
            filters: vec![],
            stream_concurrently: false,
        }).expect("input");
        encoder.encode_with_input(&mut input, &output, 0, 1, tx)
    } else {
        let (frames, receiver) = crossbeam_channel::unbounded();
        frames.send(Cursor::new(vec![0; 64])).expect("finite stream");
        drop(frames);
        encoder.encode_with_stream(receiver, &output, tx, &crate::core::encoder::tests::clip_info())
    }.expect("public VVenC encode");
    assert!(result.status.success());
    assert_eq!(
        rx.into_iter().map(|p| (p.pass, p.frame)).collect::<Vec<_>>(),
        if input_path {
            vec![((1, 1), 60), ((1, 1), 90), ((1, 1), 120)]
        } else {
            vec![((1, 1), 0), ((1, 1), 60), ((1, 1), 90), ((1, 1), 120)]
        },
        "only increasing stdout stats, with the existing stream start event"
    );
    let mut stdout = b"noise frame= 700\r\nPOC: 0\nvvenc [info]: stats:  frame= 60 fps= 2.9 avg_fps= 2.9 bitrate= 29173.77 kbps avg_bitrate= 29173.77 kbps elapsed= 0\rvvenc [info]: stats: frame=bad fps= 999\nbad\xff\r\nvvenc [info]: stats: 75.0%% frame= 90/120 fps= 2.9 avg_fps= 2.9 bitrate= 29173.77 kbps avg_bitrate= 29173.77 kbps elapsed= 1\nvvenc [info]: stats: frame=90\rvvenc [info]: stats: frame=30\n".to_vec();
    stdout.extend(vec![b'o'; 512 * 1024]);
    stdout.extend_from_slice(
        b"\nvvenc [info]: stats: frame=120 fps= 2.9\rvvenc [info]: stats: frame=120 fps= 2.9",
    );
    let mut stderr = b"diagnostic\xfe\r\nvvenc [info]: stats: frame=900 fps= 999\n".to_vec();
    stderr.extend(vec![b'e'; 512 * 1024]);
    stderr.extend_from_slice(b"\r\nstderr tail");
    assert_eq!(bytes(&result.stdout), stdout);
    assert_eq!(bytes(&result.stderr), stderr);
}

fn serial_case(root: &Path, case: &str, mut encoder: Encoder) {
    use crate::{
        core::{
            Condor,
            output::Output,
            sequence::{Sequence, serial_encoder::SerialEncoder},
        },
        models::{
            scene::Scene,
            sequence::{DefaultSequenceConfig, DefaultSequenceData},
        },
    };
    let progress_count = if case == "serial0" {
        0
    } else if case == "serialfew" {
        1
    } else {
        8
    };
    encoder.parameters_mut().insert(
        "encoder-fixture-progress".into(),
        CLIParameter::new_number("--", " ", progress_count as f64),
    );
    let order = root.join(format!("{case}.order"));
    encoder.parameters_mut().insert(
        "encoder-fixture-order".into(),
        CLIParameter::new_string("--", " ", order.to_str().expect("utf8")),
    );
    if case == "serialmultipass" {
        *encoder.passes_mut().expect("passes") = EncoderPasses::All(2);
    }
    if case == "serialencode" {
        encoder.parameters_mut().insert(
            "encoder-fixture-exit".into(),
            CLIParameter::new_number("--", " ", 0.0),
        );
    }
    let script = root.join(format!("{case}.vpy"));
    let side = if case == "serialencode" { 4096 } else { 64 };
    fs::write(&script, format!("from vapoursynth import core, YUV420P8\ncore.std.BlankClip(width={side},height={side},length=4,format=YUV420P8).set_output()\n")).expect("serial input");
    let input = Input::from_data(&crate::models::input::Input::VapourSynthScript {
        source: crate::models::input::VapourSynthScriptSource::Path(script),
        variables: HashMap::new(),
        index: 0,
        filters: vec![],
        stream_concurrently: false,
    })
    .expect("serial input");
    let ranges = if case == "serialfew" {
        vec![(0, 2), (2, 4)]
    } else if case == "serialdecode" {
        vec![(0, 5)]
    } else if case == "serial0" {
        vec![(0, 4)]
    } else {
        vec![(0, 1)]
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
        input,
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
    let dir = root.join(case);
    fs::create_dir_all(&dir).expect("scenes");
    let mut serial = SerialEncoder::new(&dir);
    // These cases test EOF/join ownership, not decoder-backed bitstream counts.
    serial.validate_frame_count = false;
    let (tx, rx) = mpsc::channel();
    drop(rx);
    let result = serial.execute(
        &mut condor,
        tx,
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
    );
    if matches!(case, "serialdecode" | "serialencode") {
        assert!(
            result.is_err(),
            "inner encode/decode failure must propagate"
        );
        // Only a fully joined encoder can have finished writing this record.
        assert!(
            order.exists(),
            "error path must join encoder and relay before returning"
        );
        assert_eq!(
            fs::read_dir(&dir).expect("failed scene listing").count(),
            0,
            "joined failed scenes leave neither final files nor private scratch"
        );
    } else {
        assert!(result.is_ok(), "serial encode succeeds");
        let order = fs::read_to_string(order).expect("scene execution records");
        for output in order.lines().filter(|line| *line != "/dev/null") {
            let output = Path::new(output);
            assert!(
                output
                    .parent()
                    .expect("private parent")
                    .file_name()
                    .expect("directory")
                    .to_string_lossy()
                    .starts_with(".av1an-serial-")
            );
            assert!(!output.exists(), "private path is gone after publication");
            assert!(
                dir.join(output.file_name().expect("final basename")).exists(),
                "published final name retained"
            );
        }
        let names: Vec<_> = order
            .lines()
            .map(|line| Path::new(line).file_name().expect("scene").to_str().expect("utf8"))
            .collect();
        assert_eq!(
            names,
            if case == "serialfew" {
                vec!["00000.264", "00001.264"]
            } else if case == "serialmultipass" {
                vec!["null", "00000.264"]
            } else {
                vec!["00000.264"]
            }
        );
    }
}
