use super::*;
use andean_condor::core::{encoder::EncoderResult, input::Input};
use std::{collections::HashMap, io::Cursor, path::PathBuf, sync::mpsc};

fn encoder(base: EncoderBase) -> Encoder {
    let mut encoder = Encoder::default_from_base(&base, true);
    *encoder.passes_mut().expect("passes") = EncoderPasses::All(2);
    if base == EncoderBase::SVTAV1 {
        encoder.parameters_mut().extend(CLIParameter::new_numbers(
            "--",
            " ",
            &[("rc", 1.0), ("keyint", 48.0)],
        ));
    }
    if base == EncoderBase::VVenC {
        encoder.parameters_mut().extend(CLIParameter::new_numbers(
            "--",
            " ",
            &[("bitrate", 1000.0)],
        ));
    }
    encoder
}

fn value(argv: &[String], key: &str) -> String {
    argv.iter()
        .enumerate()
        .find_map(|(index, arg)| {
            arg.strip_prefix(&format!("{key}="))
                .map(str::to_owned)
                .or_else(|| (arg == key).then(|| argv[index + 1].clone()))
        })
        .unwrap_or_else(|| panic!("missing {key}: {argv:?}"))
}

#[test]
fn owned_attempts_are_disjoint_and_cleanup_only_their_private_directory() {
    let temp = tempfile::tempdir().expect("fixture");
    let mut bases = Vec::new();
    let mut contexts = Vec::new();
    for job in ["job one 映像", "job two 映像"] {
        let parent = temp.path().join(job);
        std::fs::create_dir_all(&parent).expect("job");
        let output = parent.join("scene0.ivf");
        let sentinel = parent.join("scene0.log");
        std::fs::write(&sentinel, "unrelated").expect("sentinel");
        for _ in 0..2 {
            let stats = PassStatistics::new(&output).expect("attempt");
            assert_eq!(
                stats.base().parent().expect("private dir").parent(),
                Some(parent.as_path())
            );
            assert_eq!(stats.base().file_name().expect("scene base"), "scene0");
            for suffix in [
                ".log",
                ".stat",
                ".json",
                "_analysis.dat",
                ".log.mbtree",
                ".log.mbtree.temp",
                ".log.temp",
                ".log.cutree",
                ".log.cutree.temp",
            ] {
                std::fs::write(format!("{}{suffix}", stats.base().display()), "owned")
                    .expect("sidecar");
            }
            assert!(!bases.contains(&stats.base().to_path_buf()));
            bases.push(stats.base().to_path_buf());
            contexts.push(stats);
        }
    }
    let last_owner = contexts[0].clone();
    drop(contexts);
    assert!(bases[0].parent().expect("dir").exists());
    drop(last_owner);
    for base in bases {
        assert!(!base.parent().expect("dir").exists());
    }
    for job in ["job one 映像", "job two 映像"] {
        assert_eq!(
            std::fs::read_to_string(temp.path().join(job).join("scene0.log")).expect("sentinel"),
            "unrelated"
        );
    }
    let external = temp.path().join("caller stats");
    std::fs::write(&external, "caller-owned").expect("external");
    drop(PassStatistics::external(&external).expect("external context"));
    assert_eq!(
        std::fs::read_to_string(external).expect("retained"),
        "caller-owned"
    );
}

#[test]
fn invalid_statistics_output_paths_return_errors_without_panicking() {
    for path in [Path::new("/"), Path::new(""), Path::new("bad\0name")] {
        assert!(PassStatistics::new(path).is_err());
        assert!(
            encoder(EncoderBase::AOM)
                .parameters_with_output((1, 2), path, &super::clip_info())
                .is_err()
        );
    }
    let temp = tempfile::tempdir().expect("fixture");
    let file = temp.path().join("not a directory");
    std::fs::write(&file, "sentinel").expect("file");
    let error = PassStatistics::new(&file.join("scene0.ivf")).expect_err("bad parent");
    assert!(
        error.to_string().contains("Creating pass statistics"),
        "{error:#}"
    );
}

#[cfg(unix)]
mod process {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt, time::Duration};

    // Finite fake: exercise real process argv, pass sharing, and final usage without
    // depending on pending rav1e/SVT CLI fixes or absent AVM/VVenC installations.
    const FAKE: &str = r#"#!/bin/sh
set -eu
argv=$(printf '%s\n' "$@")
pass=0 stats= output= records= fail=0
while [ "$#" -gt 0 ]; do
    case "$1" in
        --pass) pass=$2; shift ;;
        --stats) stats=$2; shift ;;
        -o) output=$2; shift ;;
        --pass-statistics-records) records=$2; shift ;;
        --pass-statistics-fail) fail=$2; shift ;;
    esac
    shift
done
cat > /dev/null
id=${stats%/*}; id=${id##*/}
if [ "$pass" = 1 ]; then
    printf first > "$stats"
    printf tree > "$stats.mbtree"
    printf '%s\n' "$argv" > "$records/$id.pass1"
elif [ "$pass" = 2 ]; then
    [ "$(cat "$stats")" = first ]
    [ "$(cat "$stats.mbtree")" = tree ]
    printf '%s\n' "$argv" > "$records/$id.pass2"
    [ "$fail" = 0 ] || exit 42
    printf final > "$output"
fi
"#;

    fn frames() -> crossbeam_channel::Receiver<Cursor<Vec<u8>>> {
        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(Cursor::new(b"finite input\n".to_vec())).expect("frame");
        drop(tx);
        rx
    }

    fn fake(root: &Path) -> Encoder {
        let mut encoder = encoder(EncoderBase::X264);
        encoder.set_executable(Some(root.join("fake encoder")));
        encoder.parameters_mut().insert(
            "pass-statistics-records".into(),
            CLIParameter::new_string("--", " ", root.join("records").to_str().expect("utf8")),
        );
        encoder
    }

    fn listing(path: &Path) -> Vec<PathBuf> {
        let mut entries: Vec<_> =
            fs::read_dir(path).expect("listing").map(|e| e.expect("entry").path()).collect();
        entries.sort();
        entries
    }

    #[test]
    fn concurrent_encoding_attempts_use_disjoint_job_local_statistics() {
        statistics_case(
            "concurrent_encoding_attempts_use_disjoint_job_local_statistics",
            "concurrent",
        );
    }

    #[test]
    fn manual_passes_share_statistics_until_the_owner_drops() {
        statistics_case(
            "manual_passes_share_statistics_until_the_owner_drops",
            "manual",
        );
    }

    #[test]
    fn encoding_failures_remove_owned_statistics_without_touching_caller_files() {
        statistics_case(
            "encoding_failures_remove_owned_statistics_without_touching_caller_files",
            "failure",
        );
    }

    #[test]
    fn avm_single_pass_allocates_no_statistics() {
        statistics_case("avm_single_pass_allocates_no_statistics", "single");
    }

    fn statistics_case(test: &str, case: &str) {
        if std::env::var_os("PASS_STATISTICS_DIR").is_some() {
            assert_eq!(std::env::var("PASS_STATISTICS_CASE").expect("case"), case);
            assert_statistics_ownership(case);
            println!("statistics {case}: assertions completed");
            return;
        }
        let temp = tempfile::tempdir().expect("fixture");
        let root = temp.path();
        fs::write(root.join("fake encoder"), FAKE).expect("fake");
        fs::set_permissions(root.join("fake encoder"), fs::Permissions::from_mode(0o755))
            .expect("executable");
        for dir in ["readonly cwd", "records", "job one 映像", "job two 映像"] {
            fs::create_dir_all(root.join(dir)).expect("fixture dir");
        }
        for job in ["job one 映像", "job two 映像"] {
            fs::write(root.join(job).join("scene0.log"), "preexisting").expect("caller sentinel");
        }
        let cwd = root.join("readonly cwd");
        fs::write(cwd.join("scene0.log"), "CWD sentinel").expect("sentinel");
        let before = listing(&cwd);
        fs::set_permissions(&cwd, fs::Permissions::from_mode(0o555)).expect("read-only CWD");
        let log = fs::File::create(root.join("child.log")).expect("log");
        let mut child = Command::new(std::env::current_exe().expect("test binary"))
            .args([
                "--exact",
                &format!("core::encoder::statistics::process::{test}"),
                "--nocapture",
            ])
            .env("PASS_STATISTICS_DIR", root)
            .env("PASS_STATISTICS_CASE", case)
            .current_dir(&cwd)
            .stdout(log.try_clone().expect("log clone"))
            .stderr(log)
            .spawn()
            .expect("child");
        let start = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait().expect("wait") {
                break status;
            }
            if start.elapsed() > Duration::from_secs(20) {
                child.kill().expect("kill");
                child.wait().expect("reap");
                fs::set_permissions(&cwd, fs::Permissions::from_mode(0o755))
                    .expect("restore fixture before timeout panic");
                panic!(
                    "bounded pass statistics process timed out: {}",
                    fs::read_to_string(root.join("child.log")).expect("log")
                );
            }
            thread::sleep(Duration::from_millis(10));
        };
        fs::set_permissions(&cwd, fs::Permissions::from_mode(0o755)).expect("restore fixture");
        let log = fs::read_to_string(root.join("child.log")).expect("log");
        assert!(
            status.success() && log.contains("1 passed; 0 failed"),
            "{log}"
        );
        assert!(
            log.contains(&format!("statistics {case}: assertions completed")),
            "selected ownership assertions completed: {log}"
        );
        eprint!("{log}");
        assert_eq!(listing(&cwd), before);
        assert_eq!(
            fs::read_to_string(cwd.join("scene0.log")).expect("sentinel"),
            "CWD sentinel"
        );
        for job in ["job one 映像", "job two 映像"] {
            assert_eq!(
                fs::read_to_string(root.join(job).join("scene0.log")).expect("caller sentinel"),
                "preexisting"
            );
        }
    }

    fn assert_statistics_ownership(case: &str) {
        let Some(root) = std::env::var_os("PASS_STATISTICS_DIR") else {
            return;
        };
        let root = PathBuf::from(root);
        // A privileged runner may bypass chmod. Keep all no-spill assertions either way.
        match fs::File::create("permission probe") {
            Ok(file) => {
                drop(file);
                fs::remove_file("permission probe").expect("remove probe");
                eprintln!(
                    "pass statistics CWD permission bypass (root/capability); absolute paths and no-spill checks still enforced"
                );
            },
            Err(error) => {
                assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
                eprintln!("pass statistics read-only CWD permission enforced");
            },
        }
        let encoder = fake(&root);
        let (progress, _rx) = mpsc::channel();
        if case == "concurrent" {
            let results = thread::scope(|scope| {
                let mut handles = Vec::new();
                for job in ["job one 映像", "job two 映像"] {
                    let output = root.join(job).join("scene0.264");
                    fs::write(output.with_extension("log"), "preexisting").expect("sentinel");
                    for _ in 0..2 {
                        let encoder = &encoder;
                        let output = output.clone();
                        let progress = progress.clone();
                        handles.push(scope.spawn(move || {
                            encoder
                                .encode_with_stream(
                                    frames(),
                                    &output,
                                    progress,
                                    &super::super::clip_info(),
                                )
                                .expect("two passes")
                        }));
                    }
                }
                handles.into_iter().map(|h| h.join().expect("thread")).collect::<Vec<_>>()
            });
            let mut seen = Vec::new();
            for result in results {
                assert!(result.status.success(), "{result}");
                let stats = value(&result.parameters, "--stats");
                assert!(!seen.contains(&stats), "simultaneous attempt collision");
                let path = Path::new(&stats);
                let dir = path.parent().expect("dir");
                assert!(!dir.exists(), "automatic statistics not cleaned");
                let id = dir.file_name().expect("attempt id");
                let record = root.join("records").join(id);
                for pass in ["pass1", "pass2"] {
                    let argv: Vec<String> = fs::read_to_string(record.with_extension(pass))
                        .expect("argv record")
                        .lines()
                        .map(str::to_owned)
                        .collect();
                    assert_eq!(value(&argv, "--stats"), stats);
                    if pass == "pass2" {
                        assert_eq!(argv, result.parameters);
                    }
                }
                seen.push(stats);
            }
            for job in ["job one 映像", "job two 映像"] {
                let parent = root.join(job);
                assert_eq!(
                    fs::read_to_string(parent.join("scene0.264")).expect("final output"),
                    "final"
                );
                assert_eq!(
                    fs::read_to_string(parent.join("scene0.log")).expect("sentinel"),
                    "preexisting"
                );
                assert_eq!(listing(&parent).len(), 2, "only output and sentinel remain");
            }
        } else if case == "manual" {
            let output = root.join("job one 映像/manual.264");
            let stats = PassStatistics::new(&output).expect("manual context");
            let mut manual = encoder.clone();
            for pass in [1, 2] {
                *manual.passes_mut().expect("passes") = EncoderPasses::Specific(pass, 2);
                let result = manual
                    .encode_pass_with_stream(
                        (pass, 2),
                        frames(),
                        &output,
                        progress.clone(),
                        &super::super::clip_info(),
                        Some(&stats),
                    )
                    .expect("manual pass");
                assert!(result.status.success(), "{result}");
                assert_eq!(
                    value(&result.parameters, "--stats"),
                    format!("{}.log", stats.base().display())
                );
                assert!(
                    PathBuf::from(format!("{}.log", stats.base().display())).exists(),
                    "must survive both calls"
                );
            }
            let owned_dir = stats.base().parent().expect("dir").to_path_buf();
            drop(stats);
            assert!(!owned_dir.exists());

            fs::write(root.join("blank.vpy"), "from vapoursynth import core, YUV420P8\ncore.std.BlankClip(width=64,height=64,length=1,format=YUV420P8).set_output()\n").expect("blank script");
            let model = andean_condor::models::input::Input::VapourSynthScript {
                source: andean_condor::models::input::VapourSynthScriptSource::Path(
                    root.join("blank.vpy"),
                ),
                variables: HashMap::new(),
                index: 0,
                filters: vec![],
                stream_concurrently: false,
            };
            let mut input = Input::from_data(&model).expect("blank input");
            let output = root.join("job one 映像/manual input.264");
            let stats = PassStatistics::new(&output).expect("input context");
            for pass in [1, 2] {
                *manual.passes_mut().expect("passes") = EncoderPasses::Specific(pass, 2);
                let result = manual
                    .encode_pass_with_input(
                        &mut input,
                        &output,
                        0,
                        1,
                        (pass, 2),
                        progress.clone(),
                        Some(&stats),
                    )
                    .expect("manual input pass");
                assert!(result.status.success());
                assert_eq!(
                    value(&result.parameters, "--stats"),
                    format!("{}.log", stats.base().display())
                );
            }
            let owned_dir = stats.base().parent().expect("dir").to_path_buf();
            assert_eq!(
                fs::read_to_string(&output).expect("final input output"),
                "final"
            );
            drop(stats);
            assert!(!owned_dir.exists());

            // The no-context Specific wrapper is deliberately caller-owned, not a new
            // temporary directory per call. Independent calls must retain pass one's data.
            let output = root.join("job two 映像/manual.264");
            for pass in [1, 2] {
                *manual.passes_mut().expect("passes") = EncoderPasses::Specific(pass, 2);
                let result = manual
                    .encode_with_stream(
                        frames(),
                        &output,
                        progress.clone(),
                        &super::super::clip_info(),
                    )
                    .expect("Specific wrapper");
                assert!(result.status.success(), "{result}");
            }
            assert!(output.with_extension("log").exists());

            let caller_stats = root.join("job two 映像/caller stats.log");
            let mut overridden = encoder;
            overridden.parameters_mut().insert(
                "stats".into(),
                CLIParameter::new_string("--", " ", caller_stats.to_str().expect("utf8")),
            );
            let result = overridden
                .encode_with_stream(frames(), &output, progress, &super::super::clip_info())
                .expect("caller path override");
            assert!(result.status.success());
            assert_eq!(
                value(&result.parameters, "--stats"),
                caller_stats.to_str().expect("utf8")
            );
            assert_eq!(
                fs::read_to_string(&caller_stats).expect("caller stats survive cleanup"),
                "first"
            );
            assert!(PathBuf::from(format!("{}.mbtree", caller_stats.display())).exists());
        } else if case == "failure" {
            let mut failing = encoder.clone();
            failing.parameters_mut().insert(
                "pass-statistics-fail".into(),
                CLIParameter::new_number("--", " ", 1.0),
            );
            let output = root.join("job one 映像/failure.264");
            let error = failing
                .encode_with_stream(
                    frames(),
                    &output,
                    progress.clone(),
                    &super::super::clip_info(),
                )
                .expect_err("failed pass status propagates");
            let result = error.downcast_ref::<EncoderResult>().expect("failed pass diagnostics");
            assert_eq!(result.status.code(), Some(42));
            assert!(
                !Path::new(&value(&result.parameters, "--stats")).parent().expect("dir").exists()
            );

            fs::set_permissions(root.join("fake encoder"), fs::Permissions::from_mode(0o644))
                .expect("make spawn fail");
            let before = listing(output.parent().expect("parent"));
            assert!(
                encoder
                    .encode_with_stream(frames(), &output, progress, &super::super::clip_info())
                    .is_err()
            );
            assert_eq!(
                listing(output.parent().expect("parent")),
                before,
                "spawn failure cleans private directory"
            );
        } else {
            assert_eq!(case, "single");
            let output = root.join("job one 映像/failure.264");
            // Single-pass AVM must not allocate statistics or imply native two-pass support.
            fs::write(
                root.join("single-pass encoder"),
                "#!/bin/sh\ncat >/dev/null\n",
            )
            .expect("single-pass fake");
            fs::set_permissions(
                root.join("single-pass encoder"),
                fs::Permissions::from_mode(0o755),
            )
            .expect("executable");
            let mut avm = Encoder::default_from_base(&EncoderBase::AVM, true);
            avm.set_executable(Some(root.join("single-pass encoder")));
            let before = listing(output.parent().expect("parent"));
            let (progress, _rx) = mpsc::channel();
            let result = avm
                .encode_with_stream(frames(), &output, progress, &super::super::clip_info())
                .expect("AVM single-pass frontend");
            assert!(result.status.success());
            assert!(!result.parameters.iter().any(|arg| arg.starts_with("--fpf")));
            assert_eq!(listing(output.parent().expect("parent")), before);
        }
    }
}
