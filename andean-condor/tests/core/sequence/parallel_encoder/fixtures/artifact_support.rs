use super::*;

pub(super) mod saved_artifacts {
    //! Removing publication/resume validation must fail these bounded public-path checks.
    use super::*;
    use condor::{
        core::output::Output,
        models::{
            encoder::{Encoder, EncoderBase, cli_parameter::CLIParameter},
            scene::Scene,
            sequence::{DefaultSequenceConfig, DefaultSequenceData},
        },
        utils::cancellation::Cancellation,
    };
    use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command, sync::Mutex};

    const ENCODER: &str = r#"#!/bin/sh
set -eu
out= root=
while [ "$#" -gt 0 ]; do
    case "$1" in
        -o) out=$2; shift ;;
        --artifact-validation-root) root=$2; shift ;;
    esac
    shift
done
printf started >> "$root/starts"
cat > /dev/null
case "$ENCODER_FIXTURE_CASE" in
    *wrong*|*duplicate*) printf '3/3 frames\r3/3 frames\r3/3 frames\r' >&2 ;;
    *sparse*) printf '1/3 frames\r' >&2 ;;
esac
case "$ENCODER_FIXTURE_CASE" in
    *missing*) exit 0 ;;
    *zero*) : > "$out" ;;
    *corrupt*) printf corrupt > "$out" ;;
    *) cp "$root/fixture" "$out" ;;
esac
case "$ENCODER_FIXTURE_CASE" in artifact-validation-native-status|artifact-validation-vs-status|*optout-status) exit 42 ;; esac
"#;

    pub(crate) fn artifact_cases(test: &str, cases: &[&str]) {
        if let Ok(case) = std::env::var("ENCODER_FIXTURE_CASE") {
            let rest = case.strip_prefix("artifact-validation-").expect("artifact selector");
            let (path, case) = rest.split_once('-').expect("path-case");
            assert!(["native", "vs"].contains(&path) && cases.contains(&case));
            saved_artifact_validation_child();
            println!("artifact-validation {rest}: assertions completed");
            return;
        }
        let dir = tempfile::tempdir().expect("fixture");
        let root = dir.path();
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
            for &case in cases {
                let completed = format!("artifact-validation {path}-{case}: assertions completed");
                let case = format!("artifact-validation-{path}-{case}");
                let job = root.join(&case);
                fs::create_dir_all(job.join("bin")).expect("job bin");
                if case.contains("probe-") {
                    let probe = job.join("bin/ffprobe");
                    fs::write(&probe, r#"#!/bin/sh
set -eu
root=$(dirname "$(dirname "$0")")
printf '%s' "$$" > "$root/probe-started"
case "$ENCODER_FIXTURE_CASE" in
    *probe-force) printf ready > "$root/probe-ready"; exec sleep 30 ;;
    *probe-json) printf 'not json' ;;
    *probe-count) printf '{"streams":[{"nb_read_frames":"N/A"}]}' ;;
    *probe-status) printf '{"streams":[{"nb_read_frames":"3"}]}'; exit 42 ;;
    *probe-error) printf '{"streams":[{"nb_read_frames":"3"}]}'; printf 'decoder corruption' >&2 ;;
    *probe-flood) dd if=/dev/zero bs=65536 count=2 2>/dev/null; dd if=/dev/zero bs=65536 count=2 >&2 2>/dev/null ;;
esac
"#).expect("probe control");
                    fs::set_permissions(probe, fs::Permissions::from_mode(0o755))
                        .expect("probe executable");
                }
                let output = Command::new(root.join("watchdog"))
                    .arg("--encoder-fixture-watchdog")
                    .arg(std::env::current_exe().expect("test binary"))
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
                eprintln!("{text}");
                if !output.status.success()
                    || !job.join("verified").exists()
                    || !text.contains("test result: ok. 1 passed;")
                    || !text.contains(&completed)
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

    fn fixture(root: &Path, codec: &str, container: &str, frames: usize) {
        let mut command = Command::new("ffmpeg");
        command
            .args(["-v", "error", "-f", "lavfi", "-i", "color=s=64x64:r=24", "-frames:v"])
            .arg(frames.to_string())
            .args(["-c:v", codec, "-threads", "1"]);
        if codec == "libx265" {
            command.args(["-x265-params", "pools=1:frame-threads=1"]);
        }
        let output = command
            .args(["-f", container])
            .arg(root.join("fixture"))
            .output()
            .expect("ffmpeg fixture");
        assert!(output.status.success(), "{output:?}");
    }

    fn saved_artifact_validation_child() {
        let Some(root) = std::env::var_os("ENCODER_FIXTURE_ROOT") else {
            return;
        };
        let root = std::path::PathBuf::from(root);
        let case = std::env::var("ENCODER_FIXTURE_CASE").expect("case");
        let rest = case
            .strip_prefix("artifact-validation-")
            .expect("artifact validation selection");
        let (path, case) = rest.split_once('-').expect("path-case");
        let resume = case.starts_with("resume-");
        let optout = case.starts_with("optout-") || case == "filter-optout";
        let kind = case
            .strip_prefix("resume-")
            .or_else(|| case.strip_prefix("optout-"))
            .unwrap_or(case);
        let (codec, container) = match kind {
            "av1" => ("librav1e", "ivf"),
            "vp9" => ("libvpx-vp9", "ivf"),
            "webm" => ("libvpx-vp9", "webm"),
            "mkv" => ("libx264", "matroska"),
            "hevc" => ("libx265", "hevc"),
            _ => ("libx264", "h264"),
        };
        fixture(
            &root,
            codec,
            container,
            if kind == "wrong" && !resume { 2 } else { 3 },
        );
        if matches!(kind, "av1" | "vp9") {
            use std::io::{Seek, Write};
            let mut file =
                fs::OpenOptions::new().write(true).open(root.join("fixture")).expect("IVF");
            file.seek(std::io::SeekFrom::Start(24)).expect("frame header");
            file.write_all(&999_u32.to_le_bytes())
                .expect("lying header must not determine count");
        }
        if kind == "truncated" {
            let file =
                fs::OpenOptions::new().write(true).open(root.join("fixture")).expect("fixture");
            file.set_len(32).expect("truncate");
        }
        fs::write(root.join("encoder"), ENCODER).expect("encoder");
        fs::set_permissions(root.join("encoder"), fs::Permissions::from_mode(0o755))
            .expect("executable");
        let input = input(&root, path == "native");
        let index_before = fs::read(root.join("native.ffindex")).ok();
        let scenes = root.join("scenes");
        fs::create_dir_all(&scenes).expect("scenes");
        fs::write(scenes.join("sentinel"), "caller-owned").expect("sentinel");
        let output = scenes.join("00000.264");
        if kind == "api-existing" {
            fs::write(&output, "caller-owned existing chunk").expect("existing public API output");
        }
        let mut encoder = Encoder::default_from_base(&EncoderBase::X264, true);
        encoder.set_append_color_data(false);
        encoder.set_executable(Some(root.join("encoder")));
        encoder.parameters_mut().insert(
            "artifact-validation-root".into(),
            CLIParameter::new_string("--", " ", root.to_str().expect("path")),
        );
        if kind.starts_with("filter-") {
            encoder.set_executable(Some(which::which("x264").expect("native x264")));
            encoder.parameters_mut().remove("artifact-validation-root");
            encoder
                .parameters_mut()
                .insert("demuxer".into(), CLIParameter::new_string("--", " ", "y4m"));
            encoder.set_ffmpeg_filter(Some("select=not(mod(n\\,2))".into()));
        }
        let data = DefaultSequenceData::default();
        let mut config = DefaultSequenceConfig::default();
        config.parallel_encoder.workers = Some(1);
        config.parallel_encoder.scenes_directory = scenes.clone();
        config.parallel_encoder.validate_frame_count = !optout;
        let saved = Arc::new(Mutex::new(Vec::new()));
        let snapshots = Arc::clone(&saved);
        let fail_save = case == "resume-save-error";
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
                end_frame: 4,
                sub_scenes: None,
                encoder,
                sequence_data: data,
            }],
            Some(config),
            Box::new(move |data| {
                snapshots
                    .lock()
                    .expect("saved")
                    .push(data.scenes[0].sequence_data.parallel_encoder.clone());
                if fail_save {
                    anyhow::bail!("artifact validation save failed");
                }
                Ok(())
            }),
        );
        if resume && kind != "bare" {
            let callback = std::mem::replace(&mut condor.save_callback, Box::new(|_| Ok(())));
            // SAFETY: this entry runs in its own isolated subprocess, with no active
            // workers during either environment change.
            unsafe { std::env::set_var("ENCODER_FIXTURE_CASE", "artifact-validation-seed-valid") };
            let (tx, _rx) = std::sync::mpsc::channel();
            ParallelEncoder::default()
                .execute(&mut condor, tx, Cancellation::default())
                .expect("owning publication of a genuinely valid chunk before corruption");
            // SAFETY: the isolated child's seed encoder and relays have all joined.
            unsafe {
                std::env::set_var(
                    "ENCODER_FIXTURE_CASE",
                    format!("artifact-validation-{path}-{case}"),
                );
            };
            condor.save_callback = callback;
            fs::remove_file(root.join("starts")).expect("clear only seed invocation marker");
            condor.scenes[0].sequence_data.parallel_encoder.bytes = Some(999999);
            match kind {
                "missing" => fs::remove_file(&output).expect("remove certified chunk"),
                "zero" => fs::write(&output, []).expect("truncate certified chunk"),
                "corrupt" => fs::write(&output, b"corrupt").expect("corrupt certified chunk"),
                "wrong" => {
                    fs::remove_file(root.join("fixture")).expect("replace fixture");
                    fixture(&root, codec, container, 2);
                    fs::copy(root.join("fixture"), &output).expect("replace with wrong count");
                },
                _ => (),
            }
        } else if resume {
            fs::copy(root.join("fixture"), &output).expect("unclaimed bare chunk");
        }
        let original = fs::read(&output).ok();
        let prior_data = condor.scenes[0].sequence_data.parallel_encoder.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let token = Cancellation::default();
        let result = std::thread::scope(|scope| {
            let force = (kind == "probe-force").then(|| {
                scope.spawn(|| {
                    while !root.join("probe-ready").exists() {
                        std::thread::yield_now();
                    }
                    token.request().expect("graceful");
                    token.request().expect("force");
                })
            });
            let result = if kind == "api-existing" {
                ParallelEncoder::encode_tasks(
                    &mut condor.input,
                    1,
                    vec![Task {
                        original_index: 0,
                        index: 0,
                        frame_indices: vec![1, 2, 3],
                        sub_scenes: None,
                        encoder: condor.encoder.clone(),
                        output: output.clone(),
                    }]
                    .into(),
                    tx,
                    token.clone(),
                    condor::core::sequence::parallel_encoder::ParallelEncoderOptions::default(),
                )
                .map(|_| ((), vec![]))
            } else {
                ParallelEncoder::default().execute(&mut condor, tx, token.clone())
            };
            if let Some(force) = force {
                force.join().expect("force controller");
            }
            result
        });
        let statuses: Vec<_> = rx.try_iter().collect();
        assert!(
            matches!(
                rx.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Disconnected)
            ),
            "all relays joined"
        );
        let data = &condor.scenes[0].sequence_data.parallel_encoder;
        if matches!(
            kind,
            "valid"
                | "av1"
                | "vp9"
                | "webm"
                | "mkv"
                | "hevc"
                | "filter-optout"
                | "sparse"
                | "duplicate"
        ) {
            result.expect("eligible artifact");
            assert_eq!(data.bytes, Some(output.metadata().expect("output").len()));
            assert!(data.started_on.is_some() && data.completed_on.is_some());
            assert_eq!(statuses.iter().filter(|s| matches!(s, SequenceStatus::Whole(Status::Processing { completion: SequenceCompletion::Custom { name, .. }, .. }) if matches!(name.as_str(), "size" | "resumed-size"))).count(), 1, "exactly one eligibility/size event");
            if kind == "filter-optout" {
                let counted = Command::new("ffprobe")
                    .args([
                        "-v",
                        "error",
                        "-count_frames",
                        "-select_streams",
                        "v:0",
                        "-show_entries",
                        "stream=nb_read_frames",
                        "-of",
                        "json",
                    ])
                    .arg(&output)
                    .output()
                    .expect("independent filtered count");
                assert!(counted.status.success() && counted.stderr.is_empty());
                let json: serde_json::Value =
                    serde_json::from_slice(&counted.stdout).expect("count JSON");
                assert_eq!(
                    json["streams"][0]["nb_read_frames"], "2",
                    "intentional select filter genuinely changes count"
                );
            }
            if resume {
                assert!(
                    !root.join("starts").exists(),
                    "no encoder for eligible resume"
                );
                assert_eq!(
                    (data.started_on, data.completed_on),
                    (prior_data.started_on, prior_data.completed_on),
                    "retain real history"
                );
                assert_eq!(
                    fs::read(root.join("native.ffindex")).ok(),
                    index_before,
                    "resume validation never creates/changes an index"
                );
                let saved = saved.lock().expect("saved");
                assert_eq!(
                    saved.len(),
                    1,
                    "correct stale byte count through existing save path once"
                );
                assert_eq!(saved[0].bytes, data.bytes);
            }
        } else {
            let error = result.expect_err("ineligible artifact must not succeed");
            if kind == "probe-force" {
                assert_eq!(
                    error.downcast_ref::<condor::utils::cancellation::Cancelled>(),
                    Some(&condor::utils::cancellation::Cancelled::Forced)
                );
                let pid = fs::read_to_string(root.join("probe-started")).expect("owned probe pid");
                let pid = pid.parse::<u32>().expect("pid");
                assert!(
                    !Path::new(&format!("/proc/{pid}")).exists(),
                    "direct silent probe reaped before return, not cleaned up by test"
                );
            }
            let text = format!("{error:#}");
            if kind.starts_with("probe-") {
                assert!(
                    root.join("probe-started").exists(),
                    "selected actual validation child"
                );
                let diagnostic = match kind {
                    "probe-json" => "Malformed ffprobe",
                    "probe-count" => "missing/invalid decoded frame count",
                    "probe-status" => "exit status: 42",
                    "probe-error" => "decoder corruption",
                    "probe-flood" => "output exceeded",
                    _ => "forced cancellation requested",
                };
                assert!(text.contains(diagnostic), "{text}");
            }
            if kind == "save-error" {
                assert!(
                    text.contains("Saving artifact eligibility metadata"),
                    "{text}"
                );
            } else {
                assert!(text.to_lowercase().contains("scene 0"), "{text}");
            }
            let withheld_history =
                resume && matches!(kind, "corrupt" | "wrong" | "missing" | "zero");
            if withheld_history {
                assert_eq!(
                    (data.bytes, data.started_on, data.completed_on),
                    (
                        prior_data.bytes,
                        prior_data.started_on,
                        prior_data.completed_on
                    ),
                    "withhold current eligibility without destroying repairable historical records"
                );
            } else if kind == "save-error" {
                assert_eq!(
                    data.bytes,
                    Some(output.metadata().expect("validated output").len())
                );
                assert_eq!(data.provenance, prior_data.provenance);
                assert!(text.contains("artifact validation save failed"), "{text}");
            } else {
                assert!(
                    data.bytes.is_none()
                        && data.started_on.is_none()
                        && data.completed_on.is_none(),
                    "no false metadata"
                );
            }
            assert!(!statuses.iter().any(|s| matches!(s, SequenceStatus::Whole(Status::Processing { completion: SequenceCompletion::Custom { name, .. }, .. }) if matches!(name.as_str(), "size" | "resumed-size"))), "no success-size event");
            assert!(
                !statuses.iter().any(|s| matches!(
                    s,
                    SequenceStatus::Whole(Status::Completed { .. })
                        | SequenceStatus::Subprocess {
                            child: Status::Completed { .. },
                            ..
                        }
                )),
                "progress cannot complete an invalid artifact"
            );
            if resume || kind == "api-existing" {
                if resume {
                    assert!(!root.join("starts").exists(), "fail closed, not overwrite");
                }
                assert_eq!(fs::read(&output).ok(), original, "preserve user artifact");
                if kind != "save-error" {
                    assert!(
                        text.contains("new job") && text.contains("00000.264"),
                        "actionable: {text}"
                    );
                }
                if resume && kind != "bare" {
                    let saved = saved.lock().expect("saved");
                    if withheld_history {
                        assert!(
                            saved.is_empty(),
                            "no irreversible withdrawal for probe/count failure"
                        );
                    } else {
                        assert!(
                            !saved.is_empty(),
                            "validated stale byte correction reaches the failing save"
                        );
                        assert!(
                            saved
                                .iter()
                                .all(|d| d.bytes == data.bytes
                                    && d.completed_on == data.completed_on)
                        );
                    }
                }
            } else {
                assert!(!output.exists(), "no final artifact");
                assert!(
                    saved.lock().expect("saved").is_empty(),
                    "no failed completion callback"
                );
            }
        }
        assert_eq!(
            fs::read(scenes.join("sentinel")).expect("sentinel"),
            b"caller-owned"
        );
        assert_eq!(
            fs::read_dir(&scenes).expect("cleanup").count(),
            1 + usize::from(output.exists()),
            "no owned temps, stats or probe sidecars"
        );
        fs::write(root.join("verified"), "artifact and metadata checked").expect("verified");
    }
}

pub(super) mod resume_policy {
    //! Fixed-work policy and publication regressions; no throughput comparisons.
    use super::*;
    use condor::{
        core::{
            output::Output,
            sequence::{benchmarker::Benchmarker, target_quality::TargetQuality},
        },
        models::{
            encoder::{Encoder, EncoderBase, cli_parameter::CLIParameter},
            scene::Scene,
            sequence::{
                DefaultSequenceConfig, DefaultSequenceData,
                scene_concatenator::ConcatMethod,
                target_quality::{
                    TargetQualityConfig,
                    types::{QualityMetric, VmafFeature},
                },
            },
        },
        utils::cancellation::Cancellation,
    };
    use std::{
        fs,
        os::unix::fs::{PermissionsExt, symlink},
        path::Path,
        process::Command,
        sync::Mutex,
    };

    #[derive(Clone, Default, serde::Serialize)]
    struct Config {
        #[serde(flatten)]
        inner: DefaultSequenceConfig,
        benchmarker: condor::models::sequence::benchmarker::BenchmarkerConfig,
    }
    impl std::ops::Deref for Config {
        type Target = DefaultSequenceConfig;
        fn deref(&self) -> &Self::Target {
            &self.inner
        }
    }
    impl std::ops::DerefMut for Config {
        fn deref_mut(&mut self) -> &mut Self::Target {
            &mut self.inner
        }
    }
    impl condor::models::sequence::SequenceConfigHandler for Config {}
    impl condor::models::sequence::benchmarker::BenchmarkerConfigHandler for Config {
        fn benchmarker(
            &self,
        ) -> anyhow::Result<&condor::models::sequence::benchmarker::BenchmarkerConfig> {
            Ok(&self.benchmarker)
        }
        fn benchmarker_mut(
            &mut self,
        ) -> anyhow::Result<&mut condor::models::sequence::benchmarker::BenchmarkerConfig> {
            Ok(&mut self.benchmarker)
        }
    }
    impl condor::models::sequence::parallel_encoder::ParallelEncoderConfigHandler for Config {
        fn parallel_encoder(
            &self,
        ) -> anyhow::Result<&condor::models::sequence::parallel_encoder::ParallelEncoderConfig>
        {
            Ok(&self.inner.parallel_encoder)
        }
        fn parallel_encoder_mut(
            &mut self,
        ) -> anyhow::Result<&mut condor::models::sequence::parallel_encoder::ParallelEncoderConfig>
        {
            Ok(&mut self.inner.parallel_encoder)
        }
    }
    impl condor::models::sequence::target_quality::TargetQualityConfigHandler for Config {
        fn target_quality(&self) -> anyhow::Result<&Option<TargetQualityConfig>> {
            Ok(&self.inner.target_quality)
        }
        fn target_quality_mut(&mut self) -> anyhow::Result<&mut Option<TargetQualityConfig>> {
            Ok(&mut self.inner.target_quality)
        }
    }
    impl condor::models::sequence::scene_concatenator::SceneConcatenatorConfigHandler for Config {
        fn protected_sources(&self) -> Vec<std::path::PathBuf> {
            condor::models::sequence::scene_concatenator::SceneConcatenatorConfigHandler::protected_sources(&self.inner)
        }

        fn scene_concatenator(
            &self,
        ) -> anyhow::Result<&condor::models::sequence::scene_concatenator::SceneConcatenatorConfig>
        {
            Ok(&self.inner.scene_concatenator)
        }
        fn scene_concatenator_mut(
            &mut self,
        ) -> anyhow::Result<
            &mut condor::models::sequence::scene_concatenator::SceneConcatenatorConfig,
        > {
            Ok(&mut self.inner.scene_concatenator)
        }
    }

    const CODEC: &str = r#"#!/bin/sh
out= previous=
for argument in "$@"; do
    if [ "$previous" = -o ]; then out=$argument; fi
    previous=$argument
done
if [ -n "$out" ] && [ "$out" != /dev/null ]; then
    printf '%s\n' "$out" >> "$ENCODER_FIXTURE_ROOT/starts"
    case "$ENCODER_FIXTURE_CASE" in
        *-zero) cat >/dev/null; : > "$out"; exit 0 ;;
        *-status) cat >/dev/null; printf partial > "$out"; exit 42 ;;
    esac
fi
"$ARTIFACT_VALIDATION_X264" "$@"
status=$?
if [ -n "$out" ] && [ "$out" != /dev/null ] && [ -f "$out" ]; then cp "$out" "$ENCODER_FIXTURE_ROOT/encoded"; fi
exit "$status"
"#;

    pub(crate) fn policy_cases(test: &str, cases: &[&str]) {
        if let Ok(case) = std::env::var("ENCODER_FIXTURE_CASE") {
            let case = case.strip_prefix("resume-policy-").expect("policy selector");
            assert!(
                cases.contains(&case),
                "policy case belongs to this contract"
            );
            resume_policy_child();
            println!("resume-policy {case}: assertions completed");
            return;
        }
        let dir = tempfile::tempdir().expect("fixture");
        fs::write(
            dir.path().join("watchdog.rs"),
            include_str!("../../../encoder/fixtures/fake_encoder.rs"),
        )
        .expect("supervisor");
        assert!(
            Command::new("rustc")
                .arg(dir.path().join("watchdog.rs"))
                .arg("-o")
                .arg(dir.path().join("watchdog"))
                .status()
                .expect("rustc")
                .success()
        );
        let mut failures = vec![];
        for &case in cases {
            let job = dir.path().join(case);
            fs::create_dir_all(job.join("bin")).expect("bin");
            for tool in ["ffmpeg", "cat", "cp", "x264", "sleep"] {
                symlink(
                    which::which(tool).expect("installed fixture tool"),
                    job.join("bin").join(tool),
                )
                .expect("isolated PATH");
            }
            if matches!(case, "worker-optout" | "worker-strict")
                || case.starts_with("observer-")
                || case == "resume-policy"
            {
                symlink(
                    which::which("ffprobe").expect("installed ffprobe"),
                    job.join("bin/ffprobe"),
                )
                .expect("probe");
            } else if case.starts_with("resume-") {
                fs::write(
                job.join("bin/ffprobe"),
                r#"#!/bin/sh
printf probe >> "$ENCODER_FIXTURE_ROOT/probes"
if [ -f "$ENCODER_FIXTURE_ROOT/probe-bad" ]; then printf '{"streams":[{"nb_read_frames":"N/A"}]}'; exit 0; fi
if [ "$ENCODER_FIXTURE_CASE" = resume-policy-resume-graceful ]; then
    printf ready > "$ENCODER_FIXTURE_ROOT/probe-active"
    for attempt in 1 2 3 4 5; do
        [ -f "$ENCODER_FIXTURE_ROOT/release-probe" ] && break
        sleep 1
    done
    [ -f "$ENCODER_FIXTURE_ROOT/release-probe" ] || exit 124
fi
exec "$ARTIFACT_VALIDATION_FFPROBE" "$@"
"#,
            )
            .expect("probe control");
                fs::set_permissions(job.join("bin/ffprobe"), fs::Permissions::from_mode(0o755))
                    .expect("probe executable");
            }
            let output = Command::new(dir.path().join("watchdog"))
                .arg("--encoder-fixture-watchdog")
                .arg(std::env::current_exe().expect("test binary"))
                .arg(&job)
                .arg(format!("resume-policy-{case}"))
                .arg(test)
                .env("PATH", job.join("bin"))
                .env(
                    "ARTIFACT_VALIDATION_X264",
                    which::which("x264").expect("x264"),
                )
                .env(
                    "ARTIFACT_VALIDATION_FFPROBE",
                    which::which("ffprobe").expect("ffprobe"),
                )
                .output()
                .expect("bounded child");
            let text = format!(
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            eprintln!("{text}");
            if !output.status.success()
                || !job.join("verified").exists()
                || !text.contains("test result: ok. 1 passed;")
                || !text.contains(&format!("resume-policy {case}: assertions completed"))
                || !text.lines().any(|line| {
                    line == "encoder fixture watchdog reaped 0 descendants; timeout=false"
                })
            {
                failures.push(format!("{case}: {text}"));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    fn size(status: &SequenceStatus) -> bool {
        matches!(status, SequenceStatus::Whole(Status::Processing { completion: SequenceCompletion::Custom { name, .. }, .. }) if matches!(name.as_str(), "size" | "resumed-size"))
    }

    fn resume_policy_child() {
        let Some(root) = std::env::var_os("ENCODER_FIXTURE_ROOT") else {
            return;
        };
        let root = std::path::PathBuf::from(root);
        let case = std::env::var("ENCODER_FIXTURE_CASE").expect("case");
        let case = case.strip_prefix("resume-policy-").expect("resume-policy selection");
        fs::write(root.join("codec"), CODEC).expect("codec");
        fs::set_permissions(root.join("codec"), fs::Permissions::from_mode(0o755))
            .expect("executable");
        let mut encoder = Encoder::default_from_base(&EncoderBase::X264, true);
        encoder.set_executable(Some(root.join("codec")));
        encoder.set_append_color_data(false);
        encoder.set_quantizer(20.0);
        encoder
            .parameters_mut()
            .insert("demuxer".into(), CLIParameter::new_string("--", " ", "y4m"));
        encoder.parameters_mut().insert(
            "preset".into(),
            CLIParameter::new_string("--", " ", "ultrafast"),
        );
        encoder
            .parameters_mut()
            .insert("bframes".into(), CLIParameter::new_number("--", " ", 0.0));
        if case.starts_with("worker-") {
            encoder.set_ffmpeg_filter(Some("select=not(mod(n\\,2))".into()));
        }
        let input = input(&root, case != "observer-vs");
        let scenes_dir = root.join("scenes");
        fs::create_dir_all(scenes_dir.join("Benchmarker")).expect("scratch");
        fs::write(scenes_dir.join("Benchmarker/sentinel"), "caller-owned")
            .expect("scratch sentinel");
        let mut config = Config::default();
        config.parallel_encoder.scenes_directory = scenes_dir.clone();
        config.parallel_encoder.workers = if case.starts_with("worker-") {
            None
        } else {
            Some(1)
        };
        config.parallel_encoder.validate_frame_count = !case.ends_with("optout")
            && !matches!(
                case,
                "worker-zero" | "worker-status" | "tq-individual-zero" | "tq-zone-status"
            );
        if case.starts_with("tq-") || case == "preflight-tq" {
            config.target_quality = Some(TargetQualityConfig {
                maximum_probes: 1,
                metric: QualityMetric::VMAF {
                    target_range: (-1000.0, 1000.0),
                    resolution: None,
                    scaler: "bicubic".into(),
                    threads: 1,
                    model: None,
                    features: vec![VmafFeature::Default],
                },
                ..Default::default()
            });
            config.scene_concatenator.method = ConcatMethod::FFmpeg(Default::default());
        }
        let saved = Arc::new(Mutex::new(vec![]));
        let snapshots = Arc::clone(&saved);
        let save_path = root.join("saved-job.json");
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
                start_frame: 0,
                end_frame: 3,
                sub_scenes: None,
                encoder,
                sequence_data: DefaultSequenceData::default(),
            }],
            Some(config),
            Box::new(move |data| {
                fs::write(&save_path, serde_json::to_vec(&data)?)?;
                snapshots.lock().expect("snapshots").push(
                    data.scenes
                        .iter()
                        .map(|s| s.sequence_data.parallel_encoder.clone())
                        .collect::<Vec<_>>(),
                );
                Ok(())
            }),
        );
        let token = Cancellation::default();
        let (tx, rx) = std::sync::mpsc::channel();
        if case.starts_with("preflight-") {
            let result = match case {
                "preflight-worker" => Benchmarker::default().validate(&mut condor).map(|_| ()),
                "preflight-tq" => TargetQuality::default().validate(&mut condor).map(|_| ()),
                "preflight-tq-api" => TargetQuality::probe_pass(
                    1,
                    &scenes_dir,
                    &TargetQualityConfig::default(),
                    &mut condor.input,
                    None,
                    1,
                    &ConcatMethod::FFmpeg(Default::default()),
                    &[condor::core::sequence::target_quality::Task {
                        original_index: 0,
                        frame_indices: vec![0, 1, 2],
                        encoder: condor.encoder.clone(),
                        output: scenes_dir.join("new.264"),
                        passes: vec![],
                    }],
                    tx,
                    &token,
                    condor::core::sequence::parallel_encoder::ParallelEncoderOptions::default(),
                )
                .map(|_| ()),
                "preflight-parallel" | "preflight-encoder" => {
                    if case == "preflight-encoder" {
                        condor.scenes[0].encoder.set_executable(Some(root.join("missing-encoder")));
                    }
                    ParallelEncoder::encode_tasks(
                        &mut condor.input,
                        1,
                        vec![Task {
                            index: 0,
                            original_index: 0,
                            frame_indices: vec![0, 1, 2],
                            sub_scenes: None,
                            encoder: condor.scenes[0].encoder.clone(),
                            output: scenes_dir.join("new.264"),
                        }]
                        .into(),
                        tx,
                        token,
                        condor::core::sequence::parallel_encoder::ParallelEncoderOptions::default(),
                    )
                    .map(|_| ())
                },
                _ => {
                    use condor::core::sequence::zone_encoder::{ZoneEncoder, ZonePlan};
                    let task = Task {
                        index: 0,
                        original_index: 0,
                        frame_indices: vec![0, 1, 2],
                        sub_scenes: None,
                        encoder: condor.encoder.clone(),
                        output: scenes_dir.join("new.264"),
                    };
                    let plan = ZonePlan::try_build(std::slice::from_ref(&task), &token)
                        .expect("real zone plan");
                    ZoneEncoder::encode_tasks(
                        &mut condor.input,
                        vec![task].into(),
                        plan,
                        tx,
                        token,
                        true,
                    )
                    .map(|_| ())
                },
            };
            let text = format!("{:#}", result.expect_err("early prerequisite failure"));
            assert!(
                text.contains(if case == "preflight-encoder" {
                    "missing-encoder"
                } else {
                    "ffprobe"
                }),
                "{text}"
            );
            assert!(
                !root.join("starts").exists(),
                "preflight before actual encode"
            );
        } else if case.starts_with("worker-") {
            Benchmarker::default()
                .validate(&mut condor)
                .expect("stage prerequisites with configured count policy");
            let result = Benchmarker::default().execute(&mut condor, tx, token);
            if case.ends_with("optout") {
                result.expect("saved optout forwarded through default workers discovery");
                assert!(
                    root.join("fixed-batch").exists(),
                    "fixed single functional batch, no thresholds"
                );
                assert_eq!(
                    fs::read_to_string(root.join("starts")).expect("started").lines().count(),
                    1
                );
                let path = fs::read_to_string(root.join("starts")).expect("placement");
                assert!(
                    Path::new(path.trim()).starts_with(scenes_dir.join("Benchmarker")),
                    "job-local trial ownership: {path}"
                );
                assert!(!Path::new(path.trim()).exists(), "owned trial cleaned");
                assert_eq!(
                    fs::read(scenes_dir.join("Benchmarker/sentinel")).expect("sentinel"),
                    b"caller-owned"
                );
                let counted = Command::new(
                    std::env::var_os("ARTIFACT_VALIDATION_FFPROBE").expect("real probe"),
                )
                .args([
                    "-v",
                    "error",
                    "-count_frames",
                    "-show_entries",
                    "stream=nb_read_frames",
                    "-of",
                    "json",
                ])
                .arg(root.join("encoded"))
                .output()
                .expect("independent actual count");
                assert!(counted.status.success() && counted.stderr.is_empty());
                let count: serde_json::Value =
                    serde_json::from_slice(&counted.stdout).expect("count JSON");
                assert_eq!(
                    count["streams"][0]["nb_read_frames"], "2",
                    "fixed work filter genuinely changes count 3 to 2"
                );
            } else {
                let error = result.expect_err("strict count/status/nonempty remain mandatory");
                assert!(
                    !root.join("fixed-batch").exists(),
                    "failed batch not completed"
                );
                assert!(
                    format!("{error:#}").contains(match case {
                        "worker-strict" => "decoded frame count",
                        "worker-zero" => "nonempty regular file",
                        _ => "42",
                    }),
                    "{error:#}"
                );
            }
        } else if case.starts_with("tq-") {
            if case.starts_with("tq-individual-") {
                let mut other = condor.scenes[0].clone();
                other.start_frame = 3;
                other.end_frame = 6;
                other.encoder.parameters_mut().insert(
                    "preset".into(),
                    CLIParameter::new_string("--", " ", "veryfast"),
                );
                condor.scenes.push(other);
            } else {
                let mut other = condor.scenes[0].clone();
                other.start_frame = 3;
                other.end_frame = 6;
                condor.scenes.push(other);
            }
            let target_dir = scenes_dir.join("Target Quality");
            fs::create_dir_all(target_dir.join("1/Scene Concatenator"))
                .expect("pass/concat scratch");
            TargetQuality::default()
                .validate(&mut condor)
                .expect("optout requires no ffprobe, normal stage prerequisites remain");
            let result = TargetQuality::default().execute(&mut condor, tx, token);
            if case.ends_with("optout") {
                result.expect("configured optout forwarded to actual TQ encode and scoring");
                assert!(
                    condor.scenes.iter().all(|s| s.sequence_data.target_quality.passes.len() == 1),
                    "real scored pass"
                );
                let paths = fs::read_to_string(root.join("starts")).expect("actual encodes");
                // x264 remains zone-eligible, but TQ must probe each scene independently.
                assert_eq!(
                    paths.lines().count(),
                    2,
                    "actual x264 independent-scene dispatch selected"
                );
            } else {
                let error = result.expect_err("count optout cannot waive status/nonempty");
                assert!(
                    format!("{error:#}").contains(if case.ends_with("zero") {
                        "nonempty regular file"
                    } else {
                        "42"
                    }),
                    "{error:#}"
                );
                assert!(
                    condor.scenes.iter().all(|s| s.sequence_data.target_quality.passes.is_empty())
                );
            }
        } else if case.starts_with("resume-") {
            let final_path = scenes_dir.join("00000.264");
            let frames = if case == "resume-policy" { "2" } else { "3" };
            assert!(
                Command::new("ffmpeg")
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
                        "-f",
                        "h264"
                    ])
                    .arg(&final_path)
                    .status()
                    .expect("real artifact")
                    .success()
            );
            let data = &mut condor.scenes[0].sequence_data.parallel_encoder;
            data.bytes = Some(final_path.metadata().expect("bytes").len());
            data.started_on = Some(10);
            data.completed_on = Some(20);
            current_encoding_fixture(&mut condor);
            let data = &condor.scenes[0].sequence_data.parallel_encoder;
            let history = serde_json::to_value(data).expect("history");
            let saved_job = serde_json::to_vec(&condor.as_data()).expect("saved historical job");
            fs::write(root.join("saved-job.json"), &saved_job)
                .expect("persist history before validation");
            if case == "resume-graceful" {
                let other = condor.scenes[0].clone();
                condor.scenes.push(other);
                fs::copy(&final_path, scenes_dir.join("00001.264")).expect("second candidate");
                current_encoding_fixture(&mut condor);
                let resume_histories = condor
                    .scenes
                    .iter()
                    .map(|scene| {
                        serde_json::to_value(&scene.sequence_data.parallel_encoder)
                            .expect("current per-path history")
                    })
                    .collect::<Vec<_>>();
                let mut queued = condor.scenes[0].clone();
                queued.sequence_data = Default::default();
                condor.scenes.push(queued);
                fs::write(
                    root.join("saved-job.json"),
                    serde_json::to_vec(&condor.as_data()).expect("three-scene history"),
                )
                .expect("persist all resume candidates");
                std::thread::scope(|scope| {
                    let control_root = &root;
                    let control_token = token.clone();
                    let cancel = scope.spawn(move || {
                    let first = rx.recv().expect("validation progress before probe");
                    assert!(matches!(&first, SequenceStatus::Whole(Status::Processing { completion: SequenceCompletion::Custom { name, completed: 0.0, total: 3.0 }, .. }) if name == "validating"), "explicit candidate progress: {first:?}");
                    while !control_root.join("probe-active").exists() { std::thread::yield_now(); }
                    control_token.request().expect("graceful during first active candidate");
                    fs::write(control_root.join("release-probe"), "finish active probe").expect("release");
                    rx.iter().collect::<Vec<_>>()
                });
                    ParallelEncoder::default()
                        .execute(&mut condor, tx, token.clone())
                        .expect("graceful stops after finite active candidate");
                    let events = cancel.join().expect("progress observer");
                    assert_eq!(
                        events.iter().filter(|s| size(s)).count(),
                        1,
                        "only first validated candidate eligible"
                    );
                });
                assert_eq!(
                    fs::read_to_string(root.join("probes")).expect("probe count"),
                    "probe",
                    "no future candidate admitted"
                );
                assert!(
                    !root.join("starts").exists(),
                    "no new encode after Graceful"
                );
                for (scene, expected) in condor.scenes[..2].iter().zip(&resume_histories) {
                    assert_eq!(
                        serde_json::to_value(&scene.sequence_data.parallel_encoder)
                            .expect("history"),
                        *expected
                    );
                }
            } else {
                if case == "resume-probe" {
                    fs::write(root.join("probe-bad"), "transient").expect("probe fault");
                }
                let error = ParallelEncoder::default()
                    .execute(&mut condor, tx, token.clone())
                    .expect_err("first check fails closed");
                assert!(format!("{error:#}").contains(if case == "resume-policy" {
                    "decoded frame count"
                } else {
                    "missing/invalid"
                }));
                assert!(!rx.iter().any(|s| size(&s)), "withhold current eligibility");
                assert_eq!(
                    serde_json::to_value(&condor.scenes[0].sequence_data.parallel_encoder)
                        .expect("history"),
                    history,
                    "recoverable failure must not erase history"
                );
                assert!(
                    saved.lock().expect("saved").is_empty(),
                    "no destructive save"
                );
                assert_eq!(
                    fs::read(root.join("saved-job.json")).expect("persisted history"),
                    saved_job,
                    "recoverable rejection preserves the actual saved job"
                );
                if case == "resume-policy" {
                    condor.sequence_config.parallel_encoder.validate_frame_count = false;
                    fs::write(
                        root.join("saved-job.json"),
                        serde_json::to_vec(&condor.as_data()).expect("repaired config"),
                    )
                    .expect("save explicit policy repair");
                } else {
                    fs::remove_file(root.join("probe-bad")).expect("restore healthy probe");
                }
                let (tx, rx) = std::sync::mpsc::channel();
                ParallelEncoder::default()
                    .execute(&mut condor, tx, token)
                    .expect("same job recovers after policy/tool repair");
                assert_eq!(rx.iter().filter(size).count(), 1);
                assert_eq!(
                    serde_json::to_value(&condor.scenes[0].sequence_data.parallel_encoder)
                        .expect("history"),
                    history
                );
                assert!(
                    !root.join("starts").exists(),
                    "retry reuses historical artifact, no encoder"
                );
            }
        } else {
            std::thread::scope(|scope| {
                let observer = scope.spawn(|| {
                    while !root.join("renamed").exists() {
                        std::thread::yield_now();
                    }
                    drop(rx);
                    fs::write(root.join("observer-dropped"), "real receiver disconnected")
                        .expect("observer handshake");
                });
                ParallelEncoder::default()
                    .execute(&mut condor, tx, token)
                    .expect("optional observer cannot invalidate publication");
                observer.join().expect("observer joined");
            });
            let data = &condor.scenes[0].sequence_data.parallel_encoder;
            assert_eq!(
                data.bytes,
                Some(scenes_dir.join("00000.264").metadata().expect("published chunk").len())
            );
            assert!(data.started_on.is_some() && data.completed_on.is_some());
            let persisted: serde_json::Value = serde_json::from_slice(
                &fs::read(root.join("saved-job.json")).expect("persisted completed result"),
            )
            .expect("saved JSON");
            assert_eq!(
                persisted["scenes"][0]["sequence_data"]["parallel_encoder"]["bytes"],
                data.bytes.expect("completed bytes")
            );
            assert!(
                saved.lock().expect("saved").iter().any(|batch| batch[0].bytes == data.bytes),
                "published result persisted"
            );
        }
        assert!(
            !scenes_dir.join("00000.temp.264").exists(),
            "consumer/probe/relay owners released"
        );
        fs::write(
            root.join("verified"),
            "resume-policy functional boundary assertions passed",
        )
        .expect("verified");
    }
}
