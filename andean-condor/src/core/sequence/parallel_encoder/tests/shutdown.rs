//! Runtime shutdown checks: the supervisor kills/reaps a deadlocked baseline.
use super::*;
pub(in crate::core::sequence::parallel_encoder) mod internal_integration {
    use super::*;
    use crate::{
        core::encoder::{EncoderProcessError, EncoderResult},
        models::{
            encoder::{Encoder, EncoderBase, EncoderPasses, cli_parameter::CLIParameter},
            input::{ImportMethod, Input as InputModel, VapourSynthScriptSource},
        },
        utils::semaphore::Semaphore,
    };
    use std::{
        collections::HashMap, fs, os::unix::fs::PermissionsExt, path::Path, process::Command,
        sync::atomic::AtomicBool,
    };

    // Private subprocess-local seams: header/producer/size and panic faults are
    // injected. Missing/spawn/filter/rename/results/save failures use real APIs.
    pub(in crate::core::sequence::parallel_encoder) fn fault(
        point: &str,
        scene: usize,
    ) -> anyhow::Result<()> {
        let Ok(case) = std::env::var("ENCODER_FIXTURE_CASE") else {
            return Ok(());
        };
        let root = std::path::PathBuf::from(
            std::env::var_os("ENCODER_FIXTURE_ROOT").expect("fixture root"),
        );
        if case.starts_with("retry-") && case.ends_with("worker-fault") && point == "worker" {
            anyhow::bail!("parallel-shutdown worker fault");
        }
        if case.starts_with("vs-")
            && point == "header"
            && scene == 8
            && !matches!(
                case.as_str(),
                "vs-success1"
                    | "vs-success2"
                    | "vs-multipass"
                    | "vs-save-active"
                    | "vs-cancelled"
                    | "vs-zero"
            )
        {
            // Make scene 7 the failing owner, rather than assuming FIFO wakeups.
            while !root.join("started-7").exists() && !root.join("stop-observed").exists() {
                std::thread::yield_now();
            }
        }
        if point == "decode" {
            fs::write(root.join(format!("decoded-{scene}")), "admitted")?;
        }
        if scene != 7 {
            return Ok(());
        }
        let kind = case.split_once('-').expect("path-kind").1;
        if point == "open" && kind == "open" {
            fs::remove_file(root.join("finite.y4m"))?;
        } else if kind == point {
            anyhow::bail!("parallel-shutdown {point} fault");
        } else if kind == format!("{point}-panic") {
            panic!("parallel-shutdown {point} panic");
        }
        Ok(())
    }

    pub(in crate::core::sequence::parallel_encoder) fn stopped() {
        if let Some(root) = std::env::var_os("ENCODER_FIXTURE_ROOT") {
            fs::write(Path::new(&root).join("stop-observed"), "admission closed")
                .expect("stop handshake");
        }
    }

    pub(in crate::core::sequence::parallel_encoder) fn collected_error() {
        if std::env::var("ENCODER_FIXTURE_CASE")
            .is_ok_and(|case| case.starts_with("vs-persist-active"))
        {
            let root = std::env::var_os("ENCODER_FIXTURE_ROOT").expect("fixture root");
            fs::write(
                Path::new(&root).join("collected-error"),
                "original failure collected",
            )
            .expect("join handshake");
        }
    }

    pub(in crate::core::sequence::parallel_encoder) fn verify_pools(
        stop: &lifecycle::Stop,
        workers: usize,
    ) {
        let mut returned = 0;
        while stop.workers.try_acquire().is_some() {
            returned += 1;
        }
        assert_eq!(
            returned, workers,
            "every transferred permit returned exactly once after joins"
        );
    }

    const FAKE: &str = r#"#!/bin/sh
set -eu
out= stats= root= scene=
while [ "$#" -gt 0 ]; do
    case "$1" in
        -o) out=$2; shift ;;
        --stats) stats=$2; shift ;;
        --parallel-shutdown-root) root=$2; shift ;;
        --parallel-shutdown-scene) scene=$2; shift ;;
    esac
    shift
done
printf started > "$root/started-$scene"
case "$ENCODER_FIXTURE_CASE" in
    vs-persist-active*)
        if [ "$scene" = 7 ]; then
            while [ ! -f "$root/started-8" ]; do :; done
        elif [ "$scene" = 8 ]; then
            while [ ! -f "$root/stop-observed" ]; do :; done
            while [ ! -f "$root/collected-error" ]; do :; done
        fi ;;
esac
if [ "$ENCODER_FIXTURE_CASE" = native-early ] || [ "$ENCODER_FIXTURE_CASE" = vs-early ]; then
    printf 'parallel shutdown early diagnostic' >&2
    exit 42
fi
if [ "$ENCODER_FIXTURE_CASE" = native-save-active ] || [ "$ENCODER_FIXTURE_CASE" = vs-save-active ]; then
    if [ "$scene" = 7 ]; then
        while [ ! -f "$root/started-8" ]; do :; done
    else
        while [ ! -f "$root/stop-observed" ]; do :; done
    fi
fi
cat > "$root/input-$scene"
printf owned > "$out"
if [ -n "$stats" ]; then printf owned > "$stats"; fi
printf complete > "$root/complete-$scene"
case "$ENCODER_FIXTURE_CASE" in
    vs-persist-active*)
        if [ "$scene" = 7 ]; then
            printf 'parallel shutdown active peer failure' >&2
            exit 42
        fi ;;
esac
if [ "$ENCODER_FIXTURE_CASE" = native-status ] || [ "$ENCODER_FIXTURE_CASE" = vs-status ]; then
    printf 'parallel shutdown status diagnostic' >&2
    exit 42
fi
"#;

    fn shutdown_cases(test: &str, kinds: &[&str], extras: &[&str]) {
        if let Ok(case) = std::env::var("ENCODER_FIXTURE_CASE") {
            assert!(
                extras.contains(&case.as_str())
                    || ["vs", "native"]
                        .iter()
                        .any(|path| kinds.iter().any(|kind| case == format!("{path}-{kind}"))),
                "shutdown case belongs to this contract"
            );
            check_shutdown_case();
            println!("parallel-shutdown {case}: assertions completed");
            return;
        }
        let temp = tempfile::tempdir().expect("fixture");
        let root = temp.path();
        fs::write(
            root.join("watchdog.rs"),
            include_str!("../../../../../tests/core/encoder/fixtures/fake_encoder.rs"),
        )
        .expect("watchdog source");
        let compiled = Command::new("rustc")
            .arg(root.join("watchdog.rs"))
            .arg("-o")
            .arg(root.join("watchdog"))
            .output()
            .expect("rustc");
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let mut failures = Vec::new();
        for path in ["vs", "native"] {
            for kind in kinds {
                run_child(root, &format!("{path}-{kind}"), test, &mut failures);
            }
        }
        for case in extras {
            run_child(root, case, test, &mut failures);
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    fn run_child(root: &Path, case: &str, test: &str, failures: &mut Vec<String>) {
        let job = root.join(case);
        fs::create_dir_all(&job).expect("job");
        let log = fs::File::create(job.join("child.log")).expect("log");
        let status = Command::new(root.join("watchdog"))
            .arg("--encoder-fixture-watchdog")
            .arg(std::env::current_exe().expect("test binary"))
            .arg(&job)
            .arg(case)
            .arg(format!(
                "core::sequence::parallel_encoder::tests::shutdown::internal_integration::{test}"
            ))
            .stdout(log.try_clone().expect("log clone"))
            .stderr(log)
            .status()
            .expect("supervisor");
        let log = fs::read_to_string(job.join("child.log")).expect("log");
        if !status.success()
            || !job.join("executed").exists()
            || !job.join("verified").exists()
            || !log.contains("test result: ok. 1 passed;")
            || !log.contains(&format!("parallel-shutdown {case}: assertions completed"))
            || !log
                .lines()
                .any(|l| l == "encoder fixture watchdog reaped 0 descendants; timeout=false")
        {
            failures.push(format!("{case}: {status}\n{log}"));
        }
        eprint!("{log}");
    }

    pub(in crate::core::sequence::parallel_encoder) fn input(root: &Path, native: bool) -> Input {
        if native {
            let path = root.join("finite.y4m");
            let mut bytes = b"YUV4MPEG2 W64 H64 F24:1 Ip A1:1 C420jpeg\n".to_vec();
            for _ in 0..96 {
                bytes.extend_from_slice(b"FRAME\n");
                bytes.extend(vec![48; 64 * 64 * 3 / 2]);
            }
            fs::write(&path, bytes).expect("native input");
            Input::from_video(&InputModel::Video {
                path,
                import_method: ImportMethod::FFMS2 {
                    index: None,
                    cache_path: Some(root.join("native.ffindex")),
                },
                filters: vec![],
            })
            .expect("native FFMS2 input")
        } else {
            let script = root.join("finite.vpy");
            fs::write(&script, "from vapoursynth import core, YUV420P8\ncore.std.BlankClip(width=64,height=64,length=96,format=YUV420P8,fpsnum=24,fpsden=1).set_output()\n").expect("VS script");
            Input::from_data(&InputModel::VapourSynthScript {
                source: VapourSynthScriptSource::Path(script),
                variables: HashMap::new(),
                index: 0,
                filters: vec![],
                stream_concurrently: false,
            })
            .expect("real nonstream input")
        }
    }

    fn encoder(root: &Path, scene: usize, kind: &str) -> Encoder {
        let mut encoder = Encoder::default_from_base(&EncoderBase::X264, true);
        encoder.set_append_color_data(false);
        *encoder.passes_mut().expect("passes") =
            EncoderPasses::All(if kind == "multipass" { 2 } else { 1 });
        encoder.set_executable(Some(root.join(match kind {
            "missing" => "missing-encoder",
            "spawn" => "unspawnable",
            _ => "fake-encoder",
        })));
        for (name, value) in [
            (
                "parallel-shutdown-root",
                root.to_str().expect("UTF8").to_owned(),
            ),
            ("parallel-shutdown-scene", scene.to_string()),
        ] {
            encoder
                .parameters_mut()
                .insert(name.into(), CLIParameter::new_string("--", " ", &value));
        }
        if kind == "filter" {
            encoder.set_ffmpeg_filter(Some("shutdown_missing_filter".to_owned()));
        }
        // Exercise filter lifecycle consumer-owned cleanup on generic errors and panics too.
        if matches!(kind, "worker-panic" | "relay-panic" | "early") {
            encoder.set_ffmpeg_filter(Some("null".to_owned()));
        }
        encoder
    }

    fn metric_callbacks(input: &mut Input, fail: bool) {
        use crate::vapoursynth::plugins::{
            MetricPluginFunction, PluginFunction, standard::plane_stats::PlaneStats,
        };
        use std::sync::atomic::{AtomicUsize, Ordering};
        let vs = input.decoder().get_vapoursynth_impl().expect("real VS decoder");
        let node = vs
            .get_output(vs.get_output_index(), vs.get_node_modifier())
            .expect("real VS node");
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&calls);
        let (tx, rx) = std::sync::mpsc::channel();
        let values = PlaneStats::collect_frame_values(
            &node,
            tx,
            move |index, value| {
                assert_eq!(*value, 64);
                seen.fetch_add(1, Ordering::SeqCst);
                if fail && index == 0 {
                    return Err(PlaneStats::new_error(
                        "parallel-shutdown metric callback failure".to_owned(),
                    ));
                }
                Ok(())
            },
            |frame| Ok(frame.width(0)),
        );
        if fail {
            assert!(
                values
                    .expect_err("callback error preserved")
                    .to_string()
                    .contains("parallel-shutdown metric callback failure")
            );
            assert!(calls.load(Ordering::SeqCst) > 0);
        } else {
            assert_eq!(values.expect("raw callback semaphore drains"), vec![64; 96]);
            assert_eq!(calls.load(Ordering::SeqCst), 96);
        }
        let _: Vec<_> = rx.try_iter().collect();
        assert!(matches!(
            rx.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        ));
    }

    fn persist_success_after_worker_failure(root: &Path, input: Input, fail_save: bool) {
        use crate::{
            core::{Condor, output::Output, sequence::Sequence},
            models::{
                scene::Scene,
                sequence::{DefaultSequenceConfig, DefaultSequenceData},
            },
        };
        use std::sync::Mutex;

        fs::write(root.join("fake-encoder"), FAKE).expect("finite encoder fixture");
        fs::set_permissions(root.join("fake-encoder"), fs::Permissions::from_mode(0o755))
            .expect("executable");
        let scenes_dir = root.join("scenes");
        fs::create_dir_all(&scenes_dir).expect("scenes");
        fs::write(scenes_dir.join("unrelated.stat"), "sentinel").expect("stats sentinel");
        let scenes = (0..4)
            .map(|index| Scene {
                start_frame: 0,
                end_frame: 96,
                sub_scenes: None,
                encoder: encoder(root, index + 7, "success1"),
                sequence_data: DefaultSequenceData::default(),
            })
            .collect();
        let saved = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&saved);
        let mut config = DefaultSequenceConfig::default();
        config.parallel_encoder.workers = Some(2);
        config.parallel_encoder.max_tries = 1; // Preserve the single-attempt active-peer shutdown control.
        config.parallel_encoder.scenes_directory = scenes_dir.clone();
        config.parallel_encoder.validate_frame_count = false;
        let mut condor = Condor::<DefaultSequenceData, DefaultSequenceConfig>::new(
            input,
            Output {
                path: root.join("unused"),
                overwrite: false,
                tags: HashMap::new(),
                video_tags: HashMap::new(),
            },
            encoder(root, 7, "success1"),
            scenes,
            Some(config),
            Box::new(move |data| {
                observed.lock().expect("snapshots").push(
                    data.scenes
                        .iter()
                        .map(|scene| {
                            let metadata = &scene.sequence_data.parallel_encoder;
                            (metadata.bytes, metadata.started_on, metadata.completed_on)
                        })
                        .collect::<Vec<_>>(),
                );
                if fail_save {
                    anyhow::bail!("parallel-shutdown secondary save failure");
                }
                Ok(())
            }),
        );
        let (tx, rx) = std::sync::mpsc::channel();
        let error = ParallelEncoder::default()
            .execute(&mut condor, tx, Arc::new(AtomicBool::new(false)))
            .expect_err("peer failure returned only after all active jobs finish");
        let ParallelEncoderError::EncoderFailed { scene, result } = error
            .downcast_ref::<ParallelEncoderError>()
            .expect("original typed failure, not secondary callback error")
        else {
            panic!("wrong failure");
        };
        assert_eq!(*scene, 0);
        assert_eq!(result.status.code(), Some(42));
        assert_eq!(
            result.stderr.to_string(),
            "parallel shutdown active peer failure"
        );
        assert!(root.join("stop-observed").exists());
        assert!(
            root.join("complete-7").exists() && root.join("complete-8").exists(),
            "both actual subprocesses completed"
        );
        assert!(
            !root.join("started-9").exists() && !root.join("started-10").exists(),
            "no queued encoder starts after failure"
        );
        assert!(
            !root.join("decoded-2").exists(),
            "queued decode never admitted after failure"
        );
        assert_eq!(
            fs::read(scenes_dir.join("00001.264")).expect("successful active output"),
            b"owned"
        );
        for index in [0, 2, 3] {
            let metadata = &condor.scenes[index].sequence_data.parallel_encoder;
            assert!(
                metadata.bytes.is_none()
                    && metadata.started_on.is_none()
                    && metadata.completed_on.is_none(),
                "failed/queued scene {index} must not be completed"
            );
            assert!(!scenes_dir.join(format!("{index:05}.264")).exists());
        }
        let metadata = &condor.scenes[1].sequence_data.parallel_encoder;
        assert_eq!(
            metadata.provenance.is_some(),
            !fail_save,
            "failed callback withholds resume certification without dropping successful byte observations"
        );
        assert_eq!(
            metadata.bytes,
            Some(5),
            "successful active output must retain completion metadata after peer failure"
        );
        assert!(metadata.started_on.is_some() && metadata.completed_on.is_some());
        assert!(metadata.started_on <= metadata.completed_on);
        let snapshots = saved.lock().expect("saved snapshot");
        assert_eq!(
            snapshots.len(),
            1,
            "one successful batch; no failed-batch retry or empty callback"
        );
        assert_eq!(
            snapshots[0][1],
            (Some(5), metadata.started_on, metadata.completed_on)
        );
        for index in [0, 2, 3] {
            assert_eq!(snapshots[0][index], (None, None, None));
        }
        let _: Vec<_> = rx.try_iter().collect();
        assert!(
            matches!(
                rx.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Disconnected)
            ),
            "all progress relays joined"
        );
        assert_eq!(
            fs::read_dir(&scenes_dir).expect("only final output and sentinel").count(),
            2
        );
        assert_eq!(
            fs::read_to_string(scenes_dir.join("unrelated.stat")).expect("sentinel"),
            "sentinel"
        );
        eprintln!(
            "parallel shutdown execute active-success: bytes=5, start/completion saved; callback_error={fail_save}, calls=1, typed peer status=42, all joined"
        );
    }

    fn check_shutdown_case() {
        let Some(root) = std::env::var_os("ENCODER_FIXTURE_ROOT") else {
            return;
        };
        let root = std::path::PathBuf::from(root);
        let case = std::env::var("ENCODER_FIXTURE_CASE").expect("case");
        let (path, kind) = case.split_once('-').expect("path-kind");
        fs::write(root.join("executed"), &case).expect("execution handshake");
        let mut input = input(&root, path == "native");
        assert_eq!(input.streams_concurrently(), path == "native");
        assert_eq!(input.clip_info().expect("clip").num_frames, 96);
        if kind.starts_with("persist-active") {
            persist_success_after_worker_failure(&root, input, kind.ends_with("save-error"));
            fs::write(
                root.join("verified"),
                "execute persisted successful active work after stop",
            )
            .expect("verification");
            return;
        }
        if kind.starts_with("metric") {
            metric_callbacks(&mut input, kind == "metric-error");
            fs::write(
                root.join("verified"),
                "actual async metric callbacks completed",
            )
            .expect("verification");
            return;
        }
        for (name, text) in [
            ("fake-encoder", FAKE.to_owned()),
            (
                "unspawnable",
                format!("#!{}\n", root.join("missing-interpreter").display()),
            ),
        ] {
            fs::write(root.join(name), text).expect("encoder script");
            fs::set_permissions(root.join(name), fs::Permissions::from_mode(0o755))
                .expect("executable");
        }
        let scenes = root.join("scenes");
        fs::create_dir_all(&scenes).expect("outputs");
        fs::write(scenes.join("unrelated.stat"), "sentinel").expect("stats sentinel");
        fs::write(scenes.join("preexisting.264"), "preexisting final").expect("final sentinel");
        if kind == "rename" {
            fs::create_dir_all(scenes.join("0.264")).expect("rename obstruction");
            fs::write(scenes.join("0.264/sentinel"), "preexisting directory")
                .expect("directory sentinel");
        }
        let total = if kind == "save-active" { 8 } else { 4 };
        let tasks = (0..total)
            .map(|index| Task {
                index,
                original_index: index + 7,
                frame_indices: if kind == "decode" && path == "native" {
                    vec![9999]
                } else {
                    (0..96).collect()
                },
                sub_scenes: None,
                encoder: encoder(&root, index + 7, kind),
                output: scenes.join(format!("{index}.264")),
            })
            .collect();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut rx = Some(rx);
        if kind == "relay-error" {
            drop(rx.take());
        }
        let (mut results_tx, results_rx) = crossbeam_channel::unbounded();
        if kind == "results" {
            let (broken, broken_rx) = crossbeam_channel::unbounded();
            drop(broken_rx);
            results_tx = broken;
        }
        let mut saved = Vec::new();
        let mut on_results = |batch: Vec<ParallelEncoderResult>| -> Result<()> {
            assert!(batch.iter().all(|r| r.result.status.success() && r.bytes == 5));
            saved.extend(batch.iter().map(|r| r.scene));
            if kind == "save-active" {
                while !root.join("started-8").exists() {
                    std::thread::yield_now();
                }
                anyhow::bail!("parallel-shutdown persistence failure with active worker");
            }
            if kind == "save" {
                anyhow::bail!("parallel-shutdown persistence failure");
            }
            assert_ne!(kind, "save-panic", "parallel-shutdown persistence panic");
            Ok(())
        };
        let mut stream = task::ResultStream {
            results_tx,
            results_rx: &results_rx,
            finished_scenes: Arc::new(Semaphore::new(0)),
            on_results: &mut on_results,
        };
        let workers = if kind == "zero" {
            0
        } else if matches!(kind, "success2" | "save-active") {
            2
        } else {
            1
        };
        let cancelled = Arc::new(AtomicBool::new(kind == "cancelled"));
        let result = ParallelEncoder::encode_tasks_with_sender(
            &mut input,
            workers,
            tasks,
            tx,
            cancelled.into(),
            &mut stream,
            ParallelEncoderOptions {
                max_tries: 1,
                validate_frame_count: false,
            }, // One-attempt shutdown fixtures, not codec artifacts.
        );
        let success = matches!(kind, "success1" | "success2" | "multipass" | "cancelled");
        if success {
            let results = result.expect("all tasks returned");
            if kind == "cancelled" {
                assert!(results.iter().all(Option::is_none));
            } else {
                assert_eq!(
                    results.iter().flatten().map(|r| r.scene).collect::<Vec<_>>(),
                    [7, 8, 9, 10]
                );
                saved.sort_unstable();
                assert_eq!(saved, [7, 8, 9, 10], "no result/notification lost");
                for scene in 7..11 {
                    assert!(root.join(format!("complete-{scene}")).exists());
                }
            }
        } else {
            let error = result.expect_err("error returned, not panic/deadlock");
            let diagnostic = format!("{error:#}");
            match kind {
                "missing" => assert!(diagnostic.contains("missing-encoder"), "{diagnostic}"),
                "spawn" => assert!(diagnostic.contains("Could not spawn"), "{diagnostic}"),
                "filter" => assert!(
                    diagnostic.contains("shutdown_missing_filter"),
                    "{diagnostic}"
                ),
                "early" => {
                    let result = error
                        .chain()
                        .find_map(|e| e.downcast_ref::<EncoderResult>())
                        .or_else(|| match error.downcast_ref::<ParallelEncoderError>() {
                            Some(ParallelEncoderError::EncoderFailed { result, .. }) => {
                                Some(result)
                            },
                            _ => None,
                        })
                        .expect("typed early failure");
                    assert_eq!(result.status.code(), Some(42));
                    assert!(
                        result.stderr.to_string().contains("parallel shutdown early diagnostic")
                    );
                    assert!(
                        error.downcast_ref::<EncoderProcessError>().is_some(),
                        "actual broken input pipe: {diagnostic}"
                    );
                },
                "status" => {
                    let ParallelEncoderError::EncoderFailed { scene, result } =
                        error.downcast_ref::<ParallelEncoderError>().expect("typed status")
                    else {
                        panic!("wrong error");
                    };
                    assert_eq!(*scene, 7);
                    assert_eq!(result.status.code(), Some(42));
                    assert_eq!(
                        result.stderr.to_string(),
                        "parallel shutdown status diagnostic"
                    );
                },
                "save" | "save-active" => assert!(
                    diagnostic.contains("parallel-shutdown persistence failure"),
                    "{diagnostic}"
                ),
                "worker-panic" | "relay-panic" | "gate-panic" => assert!(
                    diagnostic.contains("panicked")
                        && diagnostic.contains("Scene 7")
                        && diagnostic.contains(&format!(
                            "parallel-shutdown {} panic",
                            kind.trim_end_matches("-panic")
                        )),
                    "{diagnostic}"
                ),
                "save-panic" => assert!(
                    diagnostic.contains("panicked")
                        && diagnostic.contains("parallel-shutdown persistence panic"),
                    "{diagnostic}"
                ),
                "schedule-panic" => assert!(
                    diagnostic.contains("Parallel worker setup panicked")
                        && diagnostic.contains("parallel-shutdown schedule panic"),
                    "{diagnostic}"
                ),
                "relay-error" => assert!(diagnostic.contains("progress relay"), "{diagnostic}"),
                "header" | "gate" | "produce" | "size" => assert!(
                    diagnostic.contains(&format!("parallel-shutdown {kind} fault")),
                    "{diagnostic}"
                ),
                "zero" => assert!(error.downcast_ref::<ParallelEncoderError>().is_some()),
                "rename" => {
                    assert_eq!(
                        error
                            .downcast_ref::<std::io::Error>()
                            .expect("real rename failure")
                            .raw_os_error(),
                        Some(21)
                    );
                    assert!(
                        root.join("complete-7").exists(),
                        "finite encoder really completed before rename"
                    );
                },
                "results" => assert!(diagnostic.contains("disconnected channel"), "{diagnostic}"),
                "open" => assert!(
                    diagnostic.contains("No video file found") && diagnostic.contains("finite.y4m"),
                    "{diagnostic}"
                ),
                "decode" if path == "native" => assert!(
                    diagnostic.contains("frame 9999") && diagnostic.contains("Out of bounds"),
                    "{diagnostic}"
                ),
                _ => assert!(!diagnostic.is_empty()),
            }
            if kind == "save-active" {
                assert!(
                    root.join("complete-8").exists(),
                    "active finite child joined before return"
                );
                assert!(
                    !root.join("started-10").exists(),
                    "two active jobs are held until stop is observed; no queued encoder starts after wakeup"
                );
                if path == "vs" {
                    assert!(
                        !root.join("decoded-9").exists(),
                        "callback failure stops admission before any queued decode"
                    );
                }
            } else if !matches!(kind, "save" | "save-panic") {
                assert!(
                    !root.join("started-8").exists(),
                    "workers=1: no queued encoder after first failure"
                );
            }
            if path == "vs" && !matches!(kind, "save" | "save-panic" | "save-active" | "zero") {
                assert!(
                    !root.join("decoded-8").exists(),
                    "workers=1: queued decode never admitted after failure"
                );
            }
            if !matches!(kind, "save" | "save-panic" | "save-active" | "results") {
                assert!(saved.is_empty(), "failed completion not saved");
            }
            eprintln!("parallel shutdown {case}: original error returned: {diagnostic}");
        }
        if let Some(rx) = rx {
            let _: Vec<_> = rx.try_iter().collect();
            assert!(
                matches!(
                    rx.try_recv(),
                    Err(std::sync::mpsc::TryRecvError::Disconnected)
                ),
                "all progress helpers joined"
            );
        }
        assert!(
            fs::read_dir(&scenes).expect("cleanup").all(|entry| {
                let name = entry.expect("entry").file_name().to_string_lossy().into_owned();
                !name.contains(".temp.") && !name.starts_with(".av1an-")
            }),
            "all owned temporary outputs/statistics/spools removed"
        );
        assert_eq!(
            fs::read_to_string(scenes.join("unrelated.stat")).expect("stats sentinel"),
            "sentinel"
        );
        assert_eq!(
            fs::read_to_string(scenes.join("preexisting.264")).expect("final sentinel"),
            "preexisting final"
        );
        if kind == "rename" {
            assert_eq!(
                fs::read_to_string(scenes.join("0.264/sentinel")).expect("directory sentinel"),
                "preexisting directory"
            );
        }
        fs::write(root.join("verified"), "all checks ran").expect("verification handshake");
    }

    macro_rules! shutdown_contract {
    ($name:ident, [$($kind:literal),* $(,)?], [$($extra:literal),* $(,)?]) => {
        #[test]
        fn $name() { shutdown_cases(stringify!($name), &[$($kind),*], &[$($extra),*]); }
    };
}

    shutdown_contract!(
        parallel_failures_join_active_workers_and_preserve_original_diagnostics,
        ["missing", "spawn", "filter", "early", "status", "rename", "relay-error"],
        []
    );
    shutdown_contract!(
        successful_active_peers_are_persisted_after_admission_stops,
        ["save-active"],
        ["vs-persist-active", "vs-persist-active-save-error"]
    );
    shutdown_contract!(
        private_worker_and_scheduler_faults_return_every_transferred_permit,
        [
            "size",
            "results",
            "save",
            "save-panic",
            "worker-panic",
            "relay-panic",
            "zero",
            "decode"
        ],
        [
            "vs-header",
            "vs-gate",
            "vs-gate-panic",
            "vs-schedule-panic",
            "native-open",
            "native-produce"
        ]
    );
    shutdown_contract!(
        raw_metric_callback_failures_drain_helpers_before_return,
        [],
        ["vs-metric", "vs-metric-error"]
    );
    shutdown_contract!(
        parallel_shutdown_joins_successful_and_cancelled_worker_sets,
        ["success1", "success2", "multipass", "cancelled"],
        []
    );
}
