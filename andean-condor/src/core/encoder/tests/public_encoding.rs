use super::*;
use std::{fs, os::unix::fs::PermissionsExt};

const HEADER: &str = "YUV4MPEG2 C420 W64 H64 F30000:1001 Ip A0:0 XLENGTH=3\n";

// Catch the missing final pass at the process boundary, not by inspecting the loop.
// The fake consumes real Y4M and requires pass one's statistics before writing output.
const FAKE: &str = r#"#!/bin/sh
set -eu
argv=$(printf '%s\n' "$@")
pass=1 total=1 stats= output= records=
while [ "$#" -gt 0 ]; do
    case "$1" in
        --first-pass) pass=1; total=2; stats=$2; shift ;;
        --second-pass) pass=2; total=2; stats=$2; shift ;;
        --output) output=$2; shift ;;
        --public-encoding-records) records=$2; shift ;;
    esac
    shift
done
printf '%s/%s\n' "$pass" "$total" >> "$records/order"
printf '%s\n' "$argv" > "$records/pass$pass.argv"
cat > "$records/pass$pass.y4m"
[ ! -e "$records/scene0.ivf" ]
if [ "$pass/$total" = 1/2 ]; then
    [ "$output" = /dev/null ]
    printf first > "$stats"
    printf 'public encoding first 1/2\n'
else
    [ "$output" = "$records/scene0.ivf" ]
    if [ "$total" = 2 ]; then
        [ "$(cat "$stats")" = first ]
    fi
    printf final > "$output"
    printf 'public encoding final %s/%s\n' "$pass" "$total"
fi
"#;

#[test]
fn public_input_and_stream_execute_each_pass_once() {
    bounded("fake");
}

#[test]
fn installed_rav1e_public_two_pass_jobs_decode_exactly_three_frames() {
    bounded("native");
}

fn bounded(case: &str) {
    let temp = tempfile::tempdir().expect("fixture");
    let root = temp.path();
    fs::write(
        root.join("watchdog.rs"),
        include_str!("../../../../tests/core/encoder/fixtures/fake_encoder.rs"),
    )
    .expect("watchdog source");
    crate::core::encoder::tests::support::run_bounded(
        Command::new("rustc")
            .arg(root.join("watchdog.rs"))
            .arg("-o")
            .arg(root.join("watchdog")),
        &root.join("rustc.log"),
    );
    let cwd = root.join("cwd");
    fs::create_dir_all(&cwd).expect("isolated cwd");
    fs::write(cwd.join("scene0.stat"), "unrelated CWD sentinel").expect("sentinel");
    let log = crate::core::encoder::tests::support::run_bounded(
        Command::new(root.join("watchdog"))
            .arg("--encoder-fixture-watchdog")
            .arg(std::env::current_exe().expect("test binary"))
            .arg(root)
            .arg(case)
            .arg("core::encoder::tests::public_encoding::public_encoding_child")
            .current_dir(&cwd),
        &root.join("child.log"),
    );
    assert!(
        log.lines()
            .any(|line| line == "encoder fixture watchdog reaped 0 descendants; timeout=false")
    );
    assert_eq!(fs::read_dir(&cwd).expect("cwd listing").count(), 1);
    assert_eq!(
        fs::read_to_string(cwd.join("scene0.stat")).expect("sentinel"),
        "unrelated CWD sentinel"
    );
    eprint!("{log}");
}

pub(super) fn value<'a>(argv: &'a [String], key: &str) -> &'a str {
    &argv
        .windows(2)
        .find(|args| args[0] == key)
        .unwrap_or_else(|| panic!("missing {key}: {argv:?}"))[1]
}

pub(super) fn check_input(input: &mut Input) -> ClipInfo {
    let info = input.clip_info().expect("clip info");
    assert_eq!(info.num_frames, 5);
    assert_eq!(
        (*info.frame_rate.numer(), *info.frame_rate.denom()),
        (30000, 1001)
    );
    assert_eq!(input.y4m_header(Some(3)).expect("range header"), HEADER);
    info
}

pub(super) fn expected_y4m() -> Vec<u8> {
    let mut bytes = HEADER.as_bytes().to_vec();
    for luma in [48, 80, 112] {
        bytes.extend_from_slice(b"FRAME\n");
        bytes.extend(vec![luma; 64 * 64]);
        bytes.extend(vec![128; 64 * 64 / 2]);
    }
    bytes
}

pub(super) fn frames(input: &mut Input) -> crossbeam_channel::Receiver<Cursor<Vec<u8>>> {
    let (tx, rx) = crossbeam_channel::unbounded();
    let header = input.y4m_header(Some(3)).expect("stream header");
    let mut bytes = header.as_bytes().to_vec();
    tx.send(Cursor::new(header.into_bytes())).expect("header");
    let mut reader = input.frame_reader().expect("frame reader");
    for index in 1..4 {
        let frame = reader.y4m_frame(index).expect("stream frame");
        bytes.extend_from_slice(frame.get_ref());
        tx.send(frame).expect("frame");
    }
    assert_eq!(bytes, expected_y4m(), "nonzero-offset stream fixture");
    drop(tx);
    rx
}

#[test]
fn public_encoding_child() {
    let Some(root) = std::env::var_os("ENCODER_FIXTURE_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let native = std::env::var("ENCODER_FIXTURE_CASE").expect("case") == "native";
    let executable = if native {
        which::which("rav1e").expect("installed rav1e is required for public encoding")
    } else {
        let path = root.join("fake encoder");
        fs::write(&path, FAKE).expect("fake");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("executable");
        path
    };
    if native {
        let version = crate::core::encoder::tests::support::run_bounded(
            Command::new(&executable).arg("--version"),
            &root.join("version.log"),
        );
        eprintln!(
            "public encoding installed {}: {version}",
            executable.display()
        );
    }
    let script = root.join("five frames.vpy");
    fs::write(&script, "from vapoursynth import core, YUV420P8\ncore.std.Splice([core.std.BlankClip(width=64,height=64,length=1,format=YUV420P8,fpsnum=30000,fpsden=1001,color=[y,128,128]) for y in [16,48,80,112,144]]).set_output()\n").expect("finite VS input");
    let model = crate::models::input::Input::VapourSynthScript {
        source: crate::models::input::VapourSynthScriptSource::Path(script),
        variables: HashMap::new(),
        index: 0,
        filters: vec![],
        stream_concurrently: false,
    };
    let mut attempts = Vec::new();
    for total in if native { &[2][..] } else { &[1, 2][..] } {
        for api in ["input", "stream"] {
            let job = root.join(format!("{api}{total} space 映像"));
            fs::create_dir_all(&job).expect("independent job");
            let output = job.join("scene0.ivf");
            let mut encoder = Encoder::default_from_base(&EncoderBase::RAV1E, true);
            encoder.set_executable(Some(executable.clone()));
            encoder.set_append_color_data(false);
            *encoder.passes_mut().expect("passes") = EncoderPasses::All(*total);
            encoder.parameters_mut().extend(CLIParameter::new_numbers(
                "--",
                " ",
                &[("speed", 10.0), ("threads", 1.0), ("bitrate", 100.0)],
            ));
            if !native {
                encoder.parameters_mut().insert(
                    "public-encoding-records".into(),
                    CLIParameter::new_string("--", " ", job.to_str().expect("utf8")),
                );
            }
            let mut input = Input::from_data(&model).expect("actual input");
            let info = check_input(&mut input);
            let (progress, rx) = mpsc::channel();
            drop(rx);
            let result = if api == "input" {
                encoder.encode_with_input(&mut input, &output, 1, 4, progress)
            } else {
                encoder.encode_with_stream(frames(&mut input), &output, progress, &info)
            }
            .expect("successful public orchestration");
            assert!(result.status.success(), "{result}");
            check_input(&mut input);
            if !native {
                assert_eq!(
                    fs::read_to_string(job.join("order")).expect("pass order"),
                    if *total == 1 { "1/1\n" } else { "1/2\n2/2\n" },
                    "{api} All({total}) must execute each pass exactly once"
                );
                for pass in 1..=*total {
                    assert_eq!(
                        fs::read(job.join(format!("pass{pass}.y4m"))).expect("consumed Y4M"),
                        expected_y4m(),
                        "{api} pass {pass}: identical header, range, and bytes"
                    );
                    let argv: Vec<_> = fs::read_to_string(job.join(format!("pass{pass}.argv")))
                        .expect("actual argv")
                        .lines()
                        .map(str::to_owned)
                        .collect();
                    if pass == *total {
                        assert_eq!(argv, result.parameters, "return the final pass result");
                    } else {
                        assert_eq!(value(&argv, "--output"), NULL_OUTPUT);
                        assert_eq!(
                            value(&argv, "--first-pass"),
                            value(&result.parameters, "--second-pass")
                        );
                    }
                }
                assert_eq!(fs::read_to_string(&output).expect("final output"), "final");
                assert_eq!(
                    result.stdout.to_string(),
                    format!("public encoding final {total}/{total}\n")
                );
                eprintln!(
                    "public encoding {api} All({total}): exact order {}, final marker, identical 3-frame Y4M",
                    if *total == 1 {
                        "[(1,1)]"
                    } else {
                        "[(1,2),(2,2)]"
                    }
                );
            } else {
                let probe = crate::core::encoder::tests::support::run_bounded(
                    Command::new(which::which("ffprobe").expect("installed ffprobe required"))
                        .args([
                            "-v",
                            "error",
                            "-select_streams",
                            "v:0",
                            "-count_frames",
                            "-show_entries",
                            "stream=codec_name,r_frame_rate,nb_read_frames",
                            "-of",
                            "default=noprint_wrappers=1",
                        ])
                        .arg(&output)
                        .stdin(Stdio::null()),
                    &root.join(format!("{api}-probe.log")),
                );
                assert_eq!(
                    probe.trim(),
                    "codec_name=av1\nr_frame_rate=30000/1001\nnb_read_frames=3"
                );
                assert_eq!(
                    fs::read_dir(&job).expect("job listing").count(),
                    1,
                    "only final output remains"
                );
                eprintln!(
                    "public encoding {api} public All(2) final argv: {:?}\n{probe}",
                    result.parameters
                );
            }
            assert_eq!(
                value(&result.parameters, "--output"),
                output.to_str().expect("utf8")
            );
            if *total == 2 {
                let stats = PathBuf::from(value(&result.parameters, "--second-pass"));
                assert_eq!(stats.file_name().expect("stats basename"), "scene0.stat");
                assert_eq!(
                    stats.parent().expect("private directory").parent(),
                    Some(job.as_path())
                );
                assert!(
                    !stats.parent().expect("private directory").exists(),
                    "owned cleanup"
                );
                assert!(
                    !attempts.contains(&stats),
                    "independent API statistics contexts"
                );
                attempts.push(stats);
            }
        }
    }
}

mod failures {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};

    // Consume the whole finite input before failing: this isolates exit status from
    // encoder fixture writer errors. Distinct raw diagnostics identify the pass that actually failed.
    const FAKE: &str = r#"#!/bin/sh
set -eu
argv=$(printf '%s\n' "$@")
pass=1 total=1 stats= output= records= fail= code=
while [ "$#" -gt 0 ]; do
    case "$1" in
        --first-pass) pass=1; total=2; stats=$2; shift ;;
        --second-pass) pass=2; total=2; stats=$2; shift ;;
        --output) output=$2; shift ;;
        --encoding-failure-records) records=$2; shift ;;
        --encoding-failure-fail-pass) fail=$2; shift ;;
        --encoding-failure-code) code=$2; shift ;;
    esac
    shift
done
printf '%s/%s\n' "$pass" "$total" >> "$records/order"
printf '%s\n' "$argv" > "$records/pass$pass.argv"
cat > "$records/pass$pass.y4m"
if [ "$pass/$total" = 1/2 ]; then
    [ "$output" = /dev/null ]
    printf first > "$stats"
elif [ "$total" = 2 ]; then
    [ "$(cat "$stats")" = first ]
fi
printf 'encoding failure stdout %s/%s\nbad\377\r\nstdout tail %s' "$pass" "$total" "$pass"
printf 'encoded 1 frames\nencoded 3 frames\rE27 stderr %s/%s\nbad\376\r\nstderr tail %s' "$pass" "$total" "$pass" >&2
if [ -n "$stats" ]; then
    [ -d "${stats%/*}" ]
    [ "$(cat "$stats")" = first ]
    printf stats_alive=true > "$records/pass$pass.complete"
else
    printf no_stats > "$records/pass$pass.complete"
fi
[ "$pass" != "$fail" ] || exit "$code"
"#;

    #[test]
    fn public_encoding_preserves_pass_failures_and_raw_diagnostics() {
        let temp = tempfile::tempdir().expect("fixture");
        let root = temp.path();
        fs::write(
            root.join("watchdog.rs"),
            include_str!("../../../../tests/core/encoder/fixtures/fake_encoder.rs"),
        )
        .expect("watchdog source");
        crate::core::encoder::tests::support::run_bounded(
            Command::new("rustc")
                .arg(root.join("watchdog.rs"))
                .arg("-o")
                .arg(root.join("watchdog")),
            &root.join("rustc.log"),
        );
        let mut failures = Vec::new();
        for api in ["input", "stream"] {
            for case in ["first", "final", "single", "specific1", "specific2", "direct1", "direct2"]
            {
                let job = root.join(format!("{api}-{case} space 映像"));
                let cwd = job.join("cwd");
                fs::create_dir_all(&cwd).expect("independent job");
                fs::write(cwd.join("scene0.stat"), "CWD sentinel").expect("sentinel");
                let file = fs::File::create(job.join("child.log")).expect("log");
                let status = Command::new(root.join("watchdog"))
                .arg("--encoder-fixture-watchdog")
                .arg(std::env::current_exe().expect("test binary"))
                .arg(&job)
                .arg(format!("{api}-{case}"))
                .arg("core::encoder::tests::public_encoding::failures::public_encoding_failure_child")
                .current_dir(&cwd)
                .stdout(file.try_clone().expect("clone log"))
                .stderr(file)
                .status()
                .expect("bounded child");
                let log = fs::read_to_string(job.join("child.log")).expect("child log");
                if !status.success()
                    || !job.join("executed").exists()
                    || !job.join("verified").exists()
                    || !log.contains("test result: ok. 1 passed;")
                    || !log.lines().any(|line| {
                        line == "encoder fixture watchdog reaped 0 descendants; timeout=false"
                    })
                {
                    failures.push(format!("{api}-{case}: {status}\n{log}"));
                }
                assert_eq!(fs::read_dir(&cwd).expect("cwd listing").count(), 1);
                assert_eq!(
                    fs::read_to_string(cwd.join("scene0.stat")).expect("sentinel"),
                    "CWD sentinel"
                );
                eprint!("{log}");
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn public_encoding_failure_child() {
        let Some(root) = std::env::var_os("ENCODER_FIXTURE_ROOT") else {
            return;
        };
        let root = PathBuf::from(root);
        let case = std::env::var("ENCODER_FIXTURE_CASE").expect("case");
        let (api, case) = case.split_once('-').expect("api-case");
        let (passes, failing, code, order, executed): (_, _, _, _, &[u8]) = match case {
            "first" => (EncoderPasses::All(2), (1, 2), 42, "1/2\n", &[1]),
            "final" => (EncoderPasses::All(2), (2, 2), 43, "1/2\n2/2\n", &[1, 2]),
            "single" => (EncoderPasses::All(1), (1, 1), 44, "1/1\n", &[1]),
            "specific1" | "direct1" => (EncoderPasses::Specific(1, 2), (1, 2), 45, "1/2\n", &[1]),
            "specific2" | "direct2" => (EncoderPasses::Specific(2, 2), (2, 2), 46, "2/2\n", &[2]),
            _ => panic!("unknown case {case}"),
        };
        fs::write(root.join("executed"), format!("{api}-{case}")).expect("execution handshake");
        let executable = root.join("fake encoder");
        fs::write(&executable, FAKE).expect("fake");
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).expect("executable");
        let output = root.join("scene0.ivf");
        fs::write(&output, "preexisting final output").expect("sentinel");
        fs::write(root.join("unrelated.stat"), "unrelated stats").expect("sentinel");
        let mut encoder = Encoder::default_from_base(&EncoderBase::RAV1E, true);
        encoder.set_executable(Some(executable));
        encoder.set_append_color_data(false);
        *encoder.passes_mut().expect("passes") = passes;
        for (key, val) in [
            (
                "encoding-failure-records",
                root.to_str().expect("utf8").to_owned(),
            ),
            ("encoding-failure-fail-pass", failing.0.to_string()),
            ("encoding-failure-code", code.to_string()),
        ] {
            encoder
                .parameters_mut()
                .insert(key.into(), CLIParameter::new_string("--", " ", &val));
        }
        let direct = case.starts_with("direct");
        let stats = direct.then(|| PassStatistics::new(&output).expect("manual owned statistics"));
        if matches!(passes, EncoderPasses::Specific(2, 2)) {
            let base = stats
                .as_ref()
                .map_or_else(|| output.with_extension(""), |s| s.base().to_path_buf());
            fs::write(base.with_extension("stat"), "first").expect("manual pass one's stats");
        }
        let script = root.join("five frames.vpy");
        fs::write(&script, "from vapoursynth import core, YUV420P8\ncore.std.Splice([core.std.BlankClip(width=64,height=64,length=1,format=YUV420P8,fpsnum=30000,fpsden=1001,color=[y,128,128]) for y in [16,48,80,112,144]]).set_output()\n").expect("finite VS input");
        let mut input = Input::from_data(&crate::models::input::Input::VapourSynthScript {
            source: crate::models::input::VapourSynthScriptSource::Path(script),
            variables: HashMap::new(),
            index: 0,
            filters: vec![],
            stream_concurrently: false,
        })
        .expect("actual input");
        let info = check_input(&mut input);
        let (progress, rx) = mpsc::channel();
        let result = match (api, direct) {
            ("input", false) => encoder.encode_with_input(&mut input, &output, 1, 4, progress),
            ("input", true) => encoder.encode_pass_with_input(
                &mut input,
                &output,
                1,
                4,
                failing,
                progress,
                stats.as_ref(),
            ),
            ("stream", false) => {
                encoder.encode_with_stream(frames(&mut input), &output, progress, &info)
            },
            ("stream", true) => encoder.encode_pass_with_stream(
                failing,
                frames(&mut input),
                &output,
                progress,
                &info,
                stats.as_ref(),
            ),
            _ => panic!("unknown API {api}"),
        };
        assert_eq!(
            fs::read_to_string(root.join("order")).expect("pass order"),
            order,
            "no pass after failure"
        );
        let error =
            result.expect_err("nonzero pass status must be Err, never a prior/later success");
        let failed = error.downcast_ref::<EncoderResult>().expect("typed failing pass result");
        assert_eq!(failed.status.code(), Some(code));
        assert_eq!(failed.encoder, EncoderBase::RAV1E);
        let argv: Vec<_> = fs::read_to_string(root.join(format!("pass{}.argv", failing.0)))
            .expect("failing argv")
            .lines()
            .map(str::to_owned)
            .collect();
        assert_eq!(failed.parameters, argv);
        assert_eq!(
            value(&argv, "--output"),
            if failing.0 == failing.1 {
                output.to_str().expect("utf8")
            } else {
                NULL_OUTPUT
            }
        );
        let mut stdout =
            format!("encoding failure stdout {}/{}\nbad", failing.0, failing.1).into_bytes();
        stdout.extend_from_slice(b"\xff\r\nstdout tail ");
        stdout.extend_from_slice(failing.0.to_string().as_bytes());
        let mut stderr = format!(
            "encoded 1 frames\nencoded 3 frames\rE27 stderr {}/{}\nbad",
            failing.0, failing.1
        )
        .into_bytes();
        stderr.extend_from_slice(b"\xfe\r\nstderr tail ");
        stderr.extend_from_slice(failing.0.to_string().as_bytes());
        let StringOrBytes::Bytes(actual_stdout) = &failed.stdout else {
            panic!("raw stdout lost");
        };
        let StringOrBytes::Bytes(actual_stderr) = &failed.stderr else {
            panic!("raw stderr lost");
        };
        assert_eq!(actual_stdout, &stdout);
        assert_eq!(actual_stderr, &stderr);
        let mut expected_progress = Vec::new();
        let mut stats_path = None;
        for &pass in executed {
            assert_eq!(
                fs::read(root.join(format!("pass{pass}.y4m"))).expect("consumed input"),
                expected_y4m()
            );
            assert_eq!(
                fs::read_to_string(root.join(format!("pass{pass}.complete")))
                    .expect("completion before API return"),
                if failing.1 == 2 {
                    "stats_alive=true"
                } else {
                    "no_stats"
                }
            );
            if api == "stream" && !direct {
                expected_progress.push(((pass, failing.1), 0));
            }
            expected_progress.extend([((pass, failing.1), 1), ((pass, failing.1), 3)]);
            if failing.1 == 2 {
                let argv: Vec<_> = fs::read_to_string(root.join(format!("pass{pass}.argv")))
                    .expect("argv")
                    .lines()
                    .map(str::to_owned)
                    .collect();
                let path = PathBuf::from(value(
                    &argv,
                    if pass == 1 {
                        "--first-pass"
                    } else {
                        "--second-pass"
                    },
                ));
                if let Some(previous) = &stats_path {
                    assert_eq!(&path, previous, "same stats across passes");
                }
                stats_path = Some(path);
            }
        }
        assert_eq!(
            rx.try_iter().map(|p| (p.pass, p.frame)).collect::<Vec<_>>(),
            expected_progress,
            "all progress workers completed before return"
        );
        assert!(
            matches!(rx.try_recv(), Err(mpsc::TryRecvError::Disconnected)),
            "pass progress senders must already be dropped on return"
        );
        if let Some(path) = stats_path {
            if matches!(passes, EncoderPasses::All(_)) {
                assert!(
                    !path.parent().expect("owned directory").exists(),
                    "owned stats cleaned on error"
                );
            } else {
                assert_eq!(
                    fs::read_to_string(&path).expect("manual stats retained"),
                    "first"
                );
                drop(stats);
                if direct {
                    assert!(!path.parent().expect("owned directory").exists());
                } else {
                    assert!(path.exists(), "caller stats are never deleted");
                }
            }
        }
        assert!(
            !fs::read_dir(&root).expect("listing").any(|e| e
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .starts_with(".av1an-stats-"))
        );
        assert_eq!(
            fs::read_to_string(&output).expect("final sentinel"),
            "preexisting final output"
        );
        assert_eq!(
            fs::read_to_string(root.join("unrelated.stat")).expect("stats sentinel"),
            "unrelated stats"
        );
        fs::write(root.join("verified"), "all assertions executed")
            .expect("verification handshake");
        eprintln!(
            "encoding failure {api}-{case}: Err{code}, exact failing argv/raw stdout+stderr, order={executed:?}, progress={expected_progress:?}, stats alive through completion then owned cleanup/caller retention; sentinels intact"
        );
    }
}
