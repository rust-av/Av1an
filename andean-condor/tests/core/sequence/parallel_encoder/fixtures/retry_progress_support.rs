//! Removing scene replay or retry isolation breaks these real public jobs.
use super::*;
use condor::{
    core::output::Output,
    models::{
        encoder::{Encoder, EncoderBase, EncoderPasses, cli_parameter::CLIParameter},
        scene::Scene,
        sequence::{DefaultSequenceConfig, DefaultSequenceData},
    },
    utils::cancellation::Cancellation,
};
use std::{fs, os::unix::fs::PermissionsExt, process::Command, sync::Mutex};

#[derive(Clone, Default)]
struct RetryWarnings(Arc<Mutex<Vec<String>>>);

struct WarningFields<'a>(&'a mut String);

impl tracing::field::Visit for WarningFields<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write as _;
        write!(self.0, "{}={value:?};", field.name()).expect("warning field");
    }
}

impl tracing::Subscriber for RetryWarnings {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        metadata.level() == &tracing::Level::WARN
            && metadata.target().ends_with("parallel_encoder::encode")
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        let mut text = String::new();
        event.record(&mut WarningFields(&mut text));
        self.0.lock().expect("retry warnings").push(text);
    }
}

fn scene_fds(scenes: &std::path::Path) -> Vec<std::path::PathBuf> {
    fs::read_dir("/proc/self/fd")
        .expect("open descriptors")
        .filter_map(|entry| {
            let path = entry.expect("descriptor").path();
            fs::read_link(&path)
                .ok()
                .filter(|target| target.starts_with(scenes))
                .map(|_| path)
        })
        .collect()
}

const ENCODER: &str = r#"#!/bin/sh
set -eu
out= root= stats= idle= pass=1
while [ "$#" -gt 0 ]; do
    case "$1" in
        -o) out=$2; shift ;;
        --stats) stats=$2; shift ;;
        --pass) pass=$2; shift ;;
        --retry-root) root=$2; shift ;;
        --retry-idle) idle=$2; shift ;;
    esac
    shift
done
n=0
if [ -f "$root/count" ]; then n=$(cat "$root/count"); fi
n=$((n+1))
printf '%s' "$n" > "$root/count"
printf started > "$root/started"
case "$ENCODER_FIXTURE_CASE" in
    *peer-exhaust)
        case "$root" in
            */peer-1) while [ ! -f "$ENCODER_FIXTURE_ROOT/stop-observed" ]; do :; done ;;
            */peer-2) exit 91 ;;
            *) while [ ! -f "$root/peer-1/started" ]; do :; done ;;
        esac ;;
esac
printf '%s\n' "$pass" >> "$root/passes"
printf '%s\n' "$out" >> "$root/outputs"
printf '%s\n' "$stats" >> "$root/stat-paths"
case "$ENCODER_FIXTURE_CASE" in
    *progress-quiet|*progress-resume) ;;
    *progress-sparse) printf '1 frames: 1 fps\r1 frames: 1 fps\r0 frames: 1 fps\r' >&2 ;;
    *progress-*) printf '1 frames: 1 fps\r3 frames: 1 fps\r3 frames: 1 fps\r2 frames: 1 fps\r' >&2 ;;
esac
if [ "$pass" = 1 ]; then test ! -f "$out"; fi
case "$ENCODER_FIXTURE_CASE" in
    *force|*force-child-first)
        sleep 30 &
        printf '%s' "$!" > "$root/grandchild"
        exec "$idle" --encoder-force-force-idle "$root" ;;
    *all2-graceful)
        if [ "$pass" = 1 ]; then
            printf '%s' "$$" > "$root/ready"
            while [ ! -f "$root/release" ]; do :; done
        fi ;;
esac
cat > "$root/input-$n"
if [ "$pass" = 1 ] && [ -n "$stats" ]; then
    test ! -f "$stats"
    printf '%s' "$n" > "$stats"
elif [ -n "$stats" ]; then
    test -s "$stats"
fi
case "$ENCODER_FIXTURE_CASE" in
    *all2) fail=2 ;;
    *) fail=1 ;;
esac
case "$ENCODER_FIXTURE_CASE" in
    *exhaust*) fail=99 ;;
    *filter-failure) fail=99 ;;
    *all2-graceful|*save|*publication|*filter-only) fail=0 ;;
    *progress-quiet|*progress-sparse|*progress-resume) fail=0 ;;
esac
case "$root" in */peer-*) fail=0 ;; esac
if [ "$n" -le "$fail" ] && { [ "$pass" = 2 ] || [ -z "$stats" ]; }; then
    case "$ENCODER_FIXTURE_CASE" in
        *missing) exit 0 ;;
        *zero) : > "$out"; exit 0 ;;
        *wrong) cp "$root/wrong" "$out"; exit 0 ;;
    esac
    printf partial > "$out"
    printf 'last-codec-diagnostic\377' >&2
    exit 42
fi
cp "$root/fixture" "$out"
printf 0 > "$root/codec-status-$n"
case "$ENCODER_FIXTURE_CASE" in *publication) printf caller-final > "$root/scenes/00000.264" ;; esac
"#;

fn read_pid(root: &std::path::Path, file: &str) -> i32 {
    let pid = fs::read_to_string(root.join(file))
        .expect("actual child PID")
        .parse()
        .expect("complete PID");
    assert!(pid > 0, "positive child PID");
    pid
}

pub(super) fn retry_cases(test: &str, kinds: &[&str]) {
    if let Ok(case) = std::env::var("ENCODER_FIXTURE_CASE") {
        let rest = case.strip_prefix("retry-").expect("retry case");
        let (path, kind) = rest.split_once('-').expect("retry path-kind");
        assert!(["native", "vs"].contains(&path) && kinds.contains(&kind));
        retry_progress_child();
        println!("retry {rest}: assertions completed");
        return;
    }
    let temp = tempfile::tempdir().expect("fixture");
    let root = temp.path();
    fs::write(
        root.join("watchdog.rs"),
        include_str!("../../../encoder/fixtures/fake_encoder.rs"),
    )
    .expect("supervisor");
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
    for path in ["native", "vs"] {
        for &kind in kinds {
            if path == "native" && kind == "capture-force" {
                continue;
            }
            let case = format!("retry-{path}-{kind}");
            let job = root.join(&case);
            fs::create_dir_all(&job).expect("job");
            fs::create_dir_all(job.join("bin")).expect("controlled filter directory");
            if kind == "filter-only" {
                let real = which::which("ffmpeg").expect("real filter executable");
                fs::write(
                    job.join("bin/ffmpeg"),
                    format!(
                        r#"#!/bin/sh
set -eu
root=$(dirname "$(dirname "$0")")
for arg in "$@"; do
    if [ "$arg" = -vf ]; then
        n=0
        if [ -f "$root/filter-count" ]; then n=$(cat "$root/filter-count"); fi
        n=$((n+1)); printf '%s' "$n" > "$root/filter-count"
        if [ "$n" = 1 ]; then
            {real:?} "$@"
            printf 'retry filter-only failure' >&2
            exit 42
        fi
    fi
done
exec {real:?} "$@"
"#
                    ),
                )
                .expect("controlled FFmpeg");
                fs::set_permissions(job.join("bin/ffmpeg"), fs::Permissions::from_mode(0o755))
                    .expect("executable filter");
            }
            let output = Command::new(root.join("watchdog"))
                .arg("--encoder-fixture-watchdog")
                .arg(std::env::current_exe().expect("binary"))
                .arg(&job)
                .arg(&case)
                .arg(test)
                .env(
                    "PATH",
                    format!(
                        "{}:{}",
                        job.join("bin").display(),
                        std::env::var("PATH").expect("PATH")
                    ),
                )
                .output()
                .expect("bounded child");
            let text = format!(
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            eprintln!("retry {case}: {text}");
            if !output.status.success()
                || !job.join("verified").exists()
                || !text.contains("test result: ok. 1 passed;")
                || !text.contains(&format!("retry {path}-{kind}: assertions completed"))
                || !text.lines().any(|line| {
                    line == "encoder fixture watchdog reaped 0 descendants; timeout=false"
                })
            {
                failures.push(format!("{case}: {text}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

pub(super) fn progress_cases(test: &str, kinds: &[&str]) {
    if let Ok(case) = std::env::var("ENCODER_FIXTURE_CASE") {
        let rest = case.strip_prefix("retry-").expect("progress case");
        let (path, kind) = rest.split_once('-').expect("progress path-kind");
        let kind = kind.strip_prefix("progress-").expect("progress selection");
        assert!(["native", "vs"].contains(&path) && kinds.contains(&kind));
        retry_progress_child();
        println!("retry {rest}: assertions completed");
        return;
    }
    let temp = tempfile::tempdir().expect("fixture");
    let root = temp.path();
    fs::write(
        root.join("watchdog.rs"),
        include_str!("../../../encoder/fixtures/fake_encoder.rs"),
    )
    .expect("supervisor");
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
    for path in ["native", "vs"] {
        for &kind in kinds {
            let case = format!("retry-{path}-progress-{kind}");
            let job = root.join(&case);
            fs::create_dir_all(&job).expect("job");
            let output = Command::new(root.join("watchdog"))
                .arg("--encoder-fixture-watchdog")
                .arg(std::env::current_exe().expect("binary"))
                .arg(&job)
                .arg(&case)
                .arg(test)
                .output()
                .expect("bounded child");
            let text = format!(
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            eprintln!("progress {case}: {text}");
            if !output.status.success()
                || !job.join("verified").exists()
                || !text.contains("test result: ok. 1 passed;")
                || !text.contains(&format!(
                    "retry {path}-progress-{kind}: assertions completed"
                ))
                || !text.lines().any(|line| {
                    line == "encoder fixture watchdog reaped 0 descendants; timeout=false"
                })
            {
                failures.push(format!("{case}: {text}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn retry_progress_child() {
    let Some(root) = std::env::var_os("ENCODER_FIXTURE_ROOT") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let case = std::env::var("ENCODER_FIXTURE_CASE").expect("case");
    let Some(rest) = case.strip_prefix("retry-") else {
        return;
    };
    let (path, kind) = rest.split_once('-').expect("path-kind");
    let progress = kind.starts_with("progress-");
    let kind = kind.strip_prefix("progress-").unwrap_or(kind);
    let child_first = kind == "force-child-first";
    let kind = if child_first { "force" } else { kind };
    let warnings = RetryWarnings::default();
    tracing::subscriber::set_global_default(warnings.clone()).expect("isolated child subscriber");
    assert_eq!(
        // SAFETY: This isolated child adopts only its own fixture descendants.
        unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) },
        0
    );
    let fixture = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=s=64x64:r=24",
            "-frames:v",
            "3",
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
    if kind.ends_with("wrong") {
        let fixture = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=s=64x64:r=24",
                "-frames:v",
                "2",
                "-c:v",
                "libx264",
                "-threads",
                "1",
                "-f",
                "h264",
            ])
            .arg(root.join("wrong"))
            .output()
            .expect("wrong fixture");
        assert!(fixture.status.success());
    }
    fs::write(root.join("encoder"), ENCODER).expect("encoder");
    fs::set_permissions(root.join("encoder"), fs::Permissions::from_mode(0o755))
        .expect("executable");
    let input = if kind == "capture-force" {
        use condor::models::input::{Input as InputModel, VapourSynthScriptSource};
        let script = root.join("capture.vpy");
        fs::write(&script, format!("import os,time\nfrom vapoursynth import core,YUV420P8\nc=core.std.BlankClip(width=64,height=64,length=4,format=YUV420P8,fpsnum=24,fpsden=1)\ndef hold(n):\n if n==3:\n  open({:?},'w').write('producer alive')\n  deadline=time.monotonic()+15\n  while not os.path.exists({:?}) and time.monotonic()<deadline: pass\n  if not os.path.exists({:?}): open({:?},'w').write('rescue')\n  open({:?},'w').write('producer released')\n return c\ncore.std.FrameEval(c,hold).set_output()\n", root.join("producer-held"), root.join("release-frame"), root.join("release-frame"), root.join("producer-rescued"), root.join("producer-released"))).expect("held producer script");
        Input::from_data(&InputModel::VapourSynthScript {
            source: VapourSynthScriptSource::Path(script),
            variables: Default::default(),
            index: 0,
            filters: vec![],
            stream_concurrently: false,
        })
        .expect("real nonstream source")
    } else {
        input(&root, path == "native")
    };
    let index_before = fs::read(root.join("native.ffindex")).ok();
    let scenes = root.join("scenes");
    fs::create_dir_all(&scenes).expect("scratch");
    fs::write(scenes.join("00000.temp.264"), b"caller-owned").expect("sentinel");
    let output = scenes.join("00000.264");
    let mut encoder = Encoder::default_from_base(&EncoderBase::X264, true);
    encoder.set_append_color_data(false);
    encoder.set_executable(Some(root.join("encoder")));
    encoder.parameters_mut().insert(
        "retry-root".into(),
        CLIParameter::new_string("--", " ", root.to_str().expect("UTF8")),
    );
    encoder.parameters_mut().insert(
        "retry-idle".into(),
        CLIParameter::new_string(
            "--",
            " ",
            root.parent().expect("fixture root").join("watchdog").to_str().expect("UTF8"),
        ),
    );
    if matches!(kind, "filter" | "filter-only") {
        encoder.set_ffmpeg_filter(Some("null".into()));
    }
    if kind == "filter-failure" {
        encoder.set_ffmpeg_filter(Some("retry_missing_filter".into()));
    }
    if kind.starts_with("all2") {
        *encoder.passes_mut().expect("passes") = EncoderPasses::All(2);
    }
    if kind == "preflight" {
        *encoder.passes_mut().expect("passes") = EncoderPasses::All(3);
    }
    let tries = kind
        .strip_prefix("exhaust")
        .map(|n| n.parse::<u32>().unwrap_or(2))
        .or_else(|| (kind == "filter-failure").then_some(3));
    let tries = tries.or_else(|| (kind == "direct-exhaust").then_some(3));
    let mut config = DefaultSequenceConfig::default();
    let mut json = serde_json::to_value(&config.parallel_encoder).expect("JSON");
    if let Some(tries) = tries {
        json["max_tries"] = tries.into();
    }
    if kind == "budget0" {
        json["max_tries"] = 0.into();
    }
    config.parallel_encoder = serde_json::from_value(json).expect("saved config");
    config.parallel_encoder.workers = Some(1);
    config.parallel_encoder.scenes_directory = scenes.clone();
    if progress && matches!(kind, "quiet" | "sparse") {
        config.parallel_encoder.max_tries = 1;
    }
    let peers = matches!(kind, "peers" | "peer-exhaust");
    if peers {
        config.parallel_encoder.workers = Some(2);
    }
    let saved = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&saved);
    let fail_save = kind == "save";
    let mut condor = Condor::new(
        input,
        Output {
            path: root.join("unused"),
            overwrite: false,
            tags: Default::default(),
            video_tags: Default::default(),
        },
        encoder.clone(),
        vec![Scene {
            start_frame: 1,
            end_frame: if kind == "source" { 98 } else { 4 },
            sub_scenes: None,
            encoder,
            sequence_data: DefaultSequenceData::default(),
        }],
        Some(config),
        Box::new(move |data| {
            observed
                .lock()
                .expect("snapshots")
                .push(data.scenes[0].sequence_data.parallel_encoder.clone());
            if fail_save {
                anyhow::bail!("retry result save failure");
            }
            Ok(())
        }),
    );
    if peers {
        for index in 1..=2 {
            let peer = root.join(format!("peer-{index}"));
            fs::create_dir_all(&peer).expect("peer input logs");
            fs::copy(root.join("fixture"), peer.join("fixture")).expect("peer output fixture");
            let mut encoder = condor.encoder.clone();
            encoder.parameters_mut().insert(
                "retry-root".into(),
                CLIParameter::new_string("--", " ", peer.to_str().expect("UTF8")),
            );
            condor.scenes.push(Scene {
                start_frame: 1,
                end_frame: 4,
                sub_scenes: None,
                encoder,
                sequence_data: DefaultSequenceData::default(),
            });
        }
    }
    if progress && matches!(kind, "resume" | "resumed") {
        fs::copy(root.join("fixture"), &output).expect("resumed artifact");
        let data = &mut condor.scenes[0].sequence_data.parallel_encoder;
        data.bytes = Some(1);
        data.started_on = Some(1);
        data.completed_on = Some(2);
        if kind == "resume" {
            let mut scene = condor.scenes[0].clone();
            scene.sequence_data = DefaultSequenceData::default();
            condor.scenes.push(scene);
        }
        current_encoding_fixture(&mut condor);
    }
    let (tx, rx) = std::sync::mpsc::channel();
    if kind == "budget0" {
        let scratch = root.join("must-not-create");
        condor.sequence_config.parallel_encoder.scenes_directory = scratch.clone();
        let error = ParallelEncoder::default()
            .initialize(
                &mut condor,
                tx.clone(),
                &condor::utils::cancellation::Cancellation::default(),
            )
            .expect_err("zero before initialization IO");
        assert!(error.to_string().contains("max_tries"));
        assert!(!scratch.exists(), "zero must not create scratch");
    }
    let token = Cancellation::default();
    if kind == "cancelled" {
        token.request().expect("already cancelled");
    }
    let result = std::thread::scope(|scope| {
        let controller = matches!(kind, "force" | "all2-graceful" | "capture-force").then(|| {
            scope.spawn(|| {
                if kind == "capture-force" {
                    while !root.join("producer-held").exists()
                        || !scene_fds(&scenes)
                            .iter()
                            .any(|fd| fd.metadata().is_ok_and(|metadata| metadata.len() > 0))
                    {
                        std::thread::yield_now();
                    }
                    assert!(
                        !root.join("count").exists(),
                        "default-budget capture precedes codec spawn"
                    );
                    token.request().expect("graceful while capturing");
                    token.request().expect("force capture wait");
                    while !root.join("stop-observed").exists() {
                        std::thread::yield_now();
                    }
                    assert!(
                        scene_fds(&scenes).is_empty(),
                        "capture closed its anonymous spool while producer is still alive"
                    );
                    assert!(!root.join("producer-released").exists());
                    fs::write(
                        root.join("release-frame"),
                        "release finite callback after capture stopped",
                    )
                    .expect("release producer");
                    return;
                }
                if progress && kind == "force" {
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
                    while !root.join("ready-opened").exists() {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "readiness writer reached seam"
                        );
                        std::thread::yield_now();
                    }
                    assert!(!root.join("ready").exists(), "open file is not readiness");
                    fs::write(
                        root.join("publish-ready"),
                        "observer checked unpublished PID",
                    )
                    .expect("release readiness writer");
                }
                while !root.join("ready").exists() {
                    std::thread::yield_now();
                }
                if child_first {
                    let leader = read_pid(&root, "leader");
                    let grandchild = read_pid(&root, "grandchild");
                    // SAFETY: Positive PID from this private fixture's complete handshake.
                    assert_eq!(unsafe { libc::kill(grandchild, libc::SIGKILL) }, 0);
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
                    loop {
                        let stat = fs::read_to_string(format!("/proc/{grandchild}/stat"))
                            .expect("dead grandchild stays unreaped until leader force");
                        let mut fields =
                            stat.rsplit_once(')').expect("stat comm").1.split_whitespace();
                        let state = fields.next().expect("state");
                        assert_eq!(
                            fields.next().expect("PPID").parse::<i32>().expect("PPID"),
                            leader
                        );
                        if state == "Z" {
                            break;
                        }
                        assert!(
                            std::time::Instant::now() < deadline,
                            "grandchild exits before leader force"
                        );
                        std::thread::yield_now();
                    }
                }
                if kind == "force" {
                    read_pid(&root, "leader");
                    read_pid(&root, "grandchild");
                }
                token.request().expect("first request");
                if kind == "force" {
                    token.request().expect("force request");
                }
                fs::write(root.join("release"), "continue").expect("release active attempt");
            })
        });
        let result =
            if kind.starts_with("direct") || (progress && matches!(kind, "quiet" | "sparse")) {
                let tasks = vec![Task {
                    original_index: 7,
                    index: 0,
                    frame_indices: vec![1, 2, 3],
                    sub_scenes: None,
                    encoder: condor.encoder.clone(),
                    output: output.clone(),
                }]
                .into();
                let result = if progress && matches!(kind, "quiet" | "sparse") {
                    ParallelEncoder::encode_tasks(
                        &mut condor.input,
                        1,
                        tasks,
                        tx,
                        token.clone(),
                        ParallelEncoderOptions {
                            max_tries: 1,
                            validate_frame_count: true,
                        },
                    )
                } else {
                    ParallelEncoder::encode_tasks(
                        &mut condor.input,
                        1,
                        tasks,
                        tx,
                        token.clone(),
                        condor::core::sequence::parallel_encoder::ParallelEncoderOptions::default(),
                    )
                };
                result.map(|results| {
                    assert_eq!(results.len(), 1);
                    assert_eq!(results[0].as_ref().expect("direct result").scene, 7);
                    ((), vec![])
                })
            } else {
                ParallelEncoder::default().execute(&mut condor, tx, token.clone())
            };
        if let Some(controller) = controller {
            controller.join().expect("controller");
        }
        result
    });
    let calls: u32 = fs::read_to_string(root.join("count"))
        .ok()
        .map_or(0, |s| s.parse().expect("count"));
    let events: Vec<_> = rx.try_iter().collect();
    if progress {
        let totals: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                SequenceStatus::Whole(Status::Processing {
                    completion: SequenceCompletion::Frames { completed, total },
                    ..
                }) => Some((*completed, *total)),
                _ => None,
            })
            .collect();
        let expected = match kind {
            "exhaust1" | "exhaust-wrong" | "force" | "publication" => 0,
            "peer-exhaust" => 3,
            "resume" => 6,
            _ => 3,
        };
        assert_eq!(
            totals.last().map_or(0, |v| v.0),
            expected,
            "usable final frames: {events:?}"
        );
        assert_eq!(
            totals.last().map(|v| v.1),
            Some(match kind {
                "peer-exhaust" => 9,
                "resume" => 6,
                _ => 3,
            })
        );
        assert!(totals.iter().all(|(done, total)| done <= total));
        let completed = events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    SequenceStatus::Subprocess {
                        child: Status::Completed { .. },
                        ..
                    }
                )
            })
            .count();
        assert_eq!(
            completed,
            usize::from(!matches!(
                kind,
                "exhaust1" | "exhaust-wrong" | "force" | "publication" | "resumed"
            ))
        );
        if kind == "once" {
            assert!(
                totals.windows(2).any(|pair| pair[0].0 > pair[1].0),
                "failed attempt rolls back before retry"
            );
        }
        if matches!(kind, "resume" | "resumed") {
            result.expect("partial resume");
            assert_eq!(calls, u32::from(kind == "resume"));
            assert!(
                events
                    .iter()
                    .any(|event| matches!(event, SequenceStatus::Whole(Status::Completed { .. })))
            );
            assert!(events.iter().any(|event| matches!(event, SequenceStatus::Whole(Status::Processing { completion: SequenceCompletion::Custom { name, .. }, .. }) if name == "resumed-size")));
            assert!(
                condor.scenes.iter().all(|scene| scene
                    .sequence_data
                    .parallel_encoder
                    .bytes
                    .is_some())
            );
            fs::write(root.join("verified"), "resume progress checked").expect("verified");
            return;
        }
    }
    if let Some(expected) = match kind {
        "once" | "exhaust2" | "filter-only" => Some(1),
        "exhaust3" => Some(2),
        "exhaust1" | "graceful" | "force-boundary" | "capture-force" | "worker-fault" => Some(0),
        _ => None,
    } {
        let warnings = warnings.0.lock().expect("captured retry warnings");
        assert_eq!(
            warnings.len(),
            expected,
            "one warning per retry, not terminal failure"
        );
        for (index, warning) in warnings.iter().enumerate() {
            assert!(
                warning.contains(&format!(
                    "Scene 0 attempt {}/{}",
                    index + 1,
                    tries.unwrap_or(3)
                )),
                "{warning}"
            );
            let diagnostics = if kind == "filter-only" {
                vec!["retry filter-only failure", "42"]
            } else {
                vec!["Encoder (", "42", "STDERR", "bytes:"]
            };
            for diagnostic in diagnostics {
                assert!(
                    warning.contains(diagnostic),
                    "original contextual diagnostic: {warning}"
                );
            }
        }
    }
    if peers {
        if kind == "peers" {
            result.expect("transient failure must not stop queued/healthy scenes");
            assert_eq!(calls, 2);
            assert!(
                condor.scenes.iter().all(|scene| scene
                    .sequence_data
                    .parallel_encoder
                    .bytes
                    .is_some())
            );
            assert!(
                root.join("peer-2/started").exists(),
                "queued work still admitted"
            );
        } else {
            let error = result.expect_err("scene zero exhausts exactly three attempts");
            assert_eq!(calls, 3);
            assert!(format!("{error:#}").contains("attempt 3/3"));
            assert!(condor.scenes[0].sequence_data.parallel_encoder.bytes.is_none());
            assert!(
                condor.scenes[1].sequence_data.parallel_encoder.bytes.is_some(),
                "successful active peer saved after terminal failure"
            );
            assert!(
                !root.join("peer-2/started").exists(),
                "queued work stopped only on exhaustion"
            );
            assert_eq!(
                saved.lock().expect("saved").len(),
                1,
                "active peer saved once"
            );
        }
        assert_eq!(
            events
                .iter()
                .filter(|s| matches!(
                    s,
                    SequenceStatus::Subprocess {
                        child: Status::Completed { .. },
                        ..
                    }
                ))
                .count(),
            if kind == "peers" { 3 } else { 1 }
        );
    } else if matches!(kind, "budget0" | "preflight" | "cancelled") {
        if kind == "cancelled" {
            result.expect("no scenes admitted");
        } else {
            result.expect_err("invalid config before input/spawn");
        }
        assert_eq!(calls, 0);
        assert!(saved.lock().expect("saved").is_empty());
        assert_eq!(
            fs::read(root.join("native.ffindex")).ok(),
            index_before,
            "no input consumption/index changes"
        );
    } else if matches!(
        kind,
        "source" | "save" | "publication" | "force" | "capture-force" | "worker-fault"
    ) {
        let error = result.expect_err("terminal failure");
        assert!(calls <= 1, "no retry for non-attempt failure: {error:#}");
        if kind == "source" {
            let cause = error.root_cause().to_string();
            assert!(
                cause.contains("96") && cause.to_lowercase().contains("frame"),
                "actual source cause, not EOF/send fallout: {error:#}"
            );
        }
        if kind == "worker-fault" {
            assert_eq!(calls, 0);
            assert!(format!("{error:#}").contains("parallel-shutdown worker fault"));
        }
        if kind == "capture-force" {
            assert_eq!(calls, 0);
            assert!(
                !root.join("leader-signal").exists(),
                "capture spawned no leader"
            );
            assert_eq!(
                error.downcast_ref::<condor::utils::cancellation::Cancelled>(),
                Some(&condor::utils::cancellation::Cancelled::Forced)
            );
            assert!(root.join("producer-released").exists());
            assert!(!root.join("producer-rescued").exists());
            assert!(
                scene_fds(&scenes).is_empty(),
                "no anonymous capture descriptors after joins"
            );
        }
        if kind == "force" {
            assert_eq!(calls, 1);
            assert_eq!(
                error.downcast_ref::<condor::utils::cancellation::Cancelled>(),
                Some(&condor::utils::cancellation::Cancelled::Forced)
            );
            let pid = read_pid(&root, "grandchild");
            let mut status = 0;
            // SAFETY: Reap the known adopted grandchild, never signal or rescue it.
            assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
            assert!(libc::WIFSIGNALED(status));
            assert_eq!(libc::WTERMSIG(status), libc::SIGKILL);
            for file in ["leader", "grandchild"] {
                let pid = read_pid(&root, file);
                assert!(
                    !std::path::Path::new(&format!("/proc/{pid}")).exists(),
                    "owned process reaped"
                );
            }
            let leader = read_pid(&root, "leader");
            assert_eq!(
                fs::read_to_string(root.join("leader-signal")).expect("known leader reap evidence"),
                format!("{leader} Some({})", libc::SIGKILL),
                "known leader SIGKILL after owner joins"
            );
        }
        if kind == "publication" {
            assert_eq!(fs::read(&output).expect("existing final"), b"caller-final");
        }
        if kind != "save" {
            assert!(saved.lock().expect("saved").is_empty());
        }
    } else if tries.is_some() || matches!(kind, "graceful" | "force-boundary") {
        let tries = tries.unwrap_or(1);
        let error = result.expect_err("exhausted");
        if kind == "graceful" {
            assert_eq!(
                error.downcast_ref::<condor::utils::cancellation::Cancelled>(),
                Some(&condor::utils::cancellation::Cancelled::Graceful)
            );
        }
        if kind == "force-boundary" {
            assert_eq!(
                error.downcast_ref::<condor::utils::cancellation::Cancelled>(),
                Some(&condor::utils::cancellation::Cancelled::Forced)
            );
        }
        assert_eq!(calls, tries, "total attempts, not extra retries");
        assert!(
            format!("{error:#}").contains(&format!(
                "attempt {tries}/{}",
                if matches!(kind, "graceful" | "force-boundary") {
                    3
                } else {
                    tries
                }
            )),
            "{error:#}"
        );
        if !kind.starts_with("exhaust-") {
            let result = match error.downcast_ref::<ParallelEncoderError>() {
                Some(ParallelEncoderError::EncoderFailed { result, .. }) => result,
                _ => error
                    .downcast_ref::<condor::core::encoder::EncoderProcessError>()
                    .expect("typed error")
                    .result
                    .as_ref()
                    .expect("status"),
            };
            assert_eq!(result.status.code(), Some(42));
            let expected: &[u8] = if progress {
                b"1 frames: 1 fps\r3 frames: 1 fps\r3 frames: 1 fps\r2 frames: 1 fps\rlast-codec-diagnostic\xff"
            } else {
                b"last-codec-diagnostic\xff"
            };
            assert!(
                matches!(&result.stderr, condor::core::encoder::StringOrBytes::Bytes(bytes) if bytes == expected)
            );
        }
        assert!(!output.exists());
        assert!(saved.lock().expect("saved").is_empty());
        assert!(condor.scenes[0].sequence_data.parallel_encoder.bytes.is_none());
    } else {
        result.expect("public job retries a transient scene failure");
        assert_eq!(
            calls,
            if kind == "all2" {
                4
            } else if progress && matches!(kind, "quiet" | "sparse") {
                1
            } else {
                2
            }
        );
        if kind == "filter-only" {
            assert_eq!(
                fs::read_to_string(root.join("filter-count")).expect("actual filter attempts"),
                "2"
            );
            for n in 1..=2 {
                assert_eq!(
                    fs::read(root.join(format!("codec-status-{n}")))
                        .expect("successful codec, failed filter only"),
                    b"0"
                );
            }
        }
        assert_eq!(
            fs::read(&output).expect("published"),
            fs::read(root.join("fixture")).expect("fixture")
        );
        assert_eq!(
            events
                .iter()
                .filter(|s| matches!(
                    s,
                    SequenceStatus::Subprocess {
                        child: Status::Completed { .. },
                        ..
                    }
                ))
                .count(),
            1
        );
        assert!(
            saved
                .lock()
                .expect("saved")
                .iter()
                .all(|d| d.bytes.is_some() && d.completed_on.is_some())
        );
        assert_eq!(
            saved.lock().expect("saved").len(),
            usize::from(kind != "direct" && !(progress && matches!(kind, "quiet" | "sparse"))),
            "Sequence persists once; the direct API leaves persistence to its caller"
        );
        let input = fs::read(root.join("input-1")).expect("literal scene input");
        let header_end = input.iter().position(|byte| *byte == b'\n').expect("Y4M header") + 1;
        let header: Vec<_> = std::str::from_utf8(&input[..header_end])
            .expect("header")
            .split_whitespace()
            .collect();
        for token in ["YUV4MPEG2", "W64", "H64", "F24:1"] {
            assert!(header.contains(&token));
        }
        assert_eq!(
            input.len() - header_end,
            3 * (6 + 6144),
            "three literal 64x64 4:2:0 frames, not empty replay"
        );
        for frame in input[header_end..].as_chunks::<{ 6 + 6144 }>().0 {
            assert_eq!(&frame[..6], b"FRAME\n");
            if path == "native" {
                assert!(frame[6..].iter().all(|byte| *byte == 48));
            }
        }
        for n in 2..=calls {
            assert_eq!(
                fs::read(root.join("input-1")).expect("input"),
                fs::read(root.join(format!("input-{n}"))).expect("replayed input")
            );
        }
        if kind == "all2" {
            assert_eq!(
                fs::read_to_string(root.join("passes")).expect("pass order"),
                "1\n2\n1\n2\n"
            );
        }
        if kind == "all2-graceful" {
            assert_eq!(
                fs::read_to_string(root.join("passes")).expect("active pass order"),
                "1\n2\n"
            );
        }
    }
    assert_eq!(
        fs::read(scenes.join("00000.temp.264")).expect("caller temp"),
        b"caller-owned"
    );
    assert_eq!(
        fs::read_dir(scenes).expect("scratch").count(),
        1 + if peers {
            if kind == "peers" { 3 } else { 1 }
        } else {
            usize::from(output.exists())
        },
        "no owned output/stats/spool leftovers"
    );
    assert!(
        matches!(
            rx.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        ),
        "all owners joined"
    );
    fs::write(root.join("verified"), "public job checked").expect("verified");
}
