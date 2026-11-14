//! Public native streaming regressions; the encoder fixture supervisor bounds every child.
use super::*;
use crate::{
    core::encoder::{EncoderProcessError, EncoderResult, StringOrBytes},
    models::encoder::{Encoder, EncoderBase, cli_parameter::CLIParameter},
    utils::cancellation::Cancellation,
};
use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command};

const ENCODER: &str = r#"#!/bin/sh
set -eu
out= root= bad=false
while [ "$#" -gt 0 ]; do
    case "$1" in
        -o) out=$2; shift ;;
        --stream-errors-root) root=$2; shift ;;
        --stream-errors-bad-flag) bad=true ;;
    esac
    shift
done
printf started >> "$root/starts"
printf partial > "$out"
if [ "$bad" = true ]; then
    printf 'stream failures invalid encoder flag\n' >&2
    exit 42
fi
cat > "$root/consumed"
if [ "$ENCODER_FIXTURE_CASE" = stream-errors-both ]; then
    printf 'stream failures independent codec failure\n' >&2
    exit 42
fi
printf complete > "$out"
"#;

#[test]
fn public_stream_failures_retain_primary_and_secondary_causes() {
    let dir = tempfile::tempdir().expect("fixture");
    let root = dir.path();
    fs::write(
        root.join("watchdog.rs"),
        include_str!("../../../../../tests/core/encoder/fixtures/fake_encoder.rs"),
    )
    .expect("supervisor source");
    let compiled = Command::new("rustc")
        .arg(root.join("watchdog.rs"))
        .arg("-o")
        .arg(root.join("watchdog"))
        .output()
        .expect("rustc");
    assert!(compiled.status.success(), "{compiled:?}");
    let mut failures = Vec::new();
    for case in [
        "early",
        "badflag",
        "decode",
        "both",
        "badfilter",
        "filterearly",
        "valid",
        "validcodec",
        "execute",
    ] {
        let job = root.join(case);
        fs::create_dir_all(&job).expect("job");
        let output = Command::new(root.join("watchdog"))
            .arg("--encoder-fixture-watchdog")
            .arg(std::env::current_exe().expect("test binary"))
            .arg(&job)
            .arg(format!("stream-errors-{case}"))
            .arg("core::sequence::parallel_encoder::tests::stream_errors::public_stream_child")
            .output()
            .expect("supervisor");
        let text = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if !output.status.success()
            || !job.join("verified").exists()
            || !text.contains("test result: ok. 1 passed;")
            || !text
                .lines()
                .any(|line| line == "encoder fixture watchdog reaped 0 descendants; timeout=false")
        {
            failures.push(format!("{case}: {text}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn result_in(error: &anyhow::Error) -> Option<&EncoderResult> {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<EncoderResult>())
        .or_else(|| match error.downcast_ref::<ParallelEncoderError>() {
            Some(ParallelEncoderError::EncoderFailed { result, .. }) => Some(result),
            _ => None,
        })
}

fn encoder(root: &Path, case: &str) -> Encoder {
    let mut encoder = Encoder::default_from_base(&EncoderBase::X264, true);
    encoder.set_append_color_data(false);
    if matches!(case, "badflag" | "decode" | "badfilter" | "validcodec") {
        encoder.set_executable(Some(which::which("x264").expect("installed x264")));
        encoder
            .parameters_mut()
            .insert("demuxer".into(), CLIParameter::new_string("--", " ", "y4m"));
    } else {
        encoder.set_executable(Some(root.join("encoder")));
        encoder.parameters_mut().insert(
            "stream-errors-root".into(),
            CLIParameter::new_string("--", " ", root.to_str().expect("path")),
        );
    }
    if matches!(case, "early" | "badflag" | "filterearly") {
        encoder.parameters_mut().insert(
            "stream-errors-bad-flag".into(),
            CLIParameter::new_bool("--", true),
        );
    }
    if case == "badfilter" {
        encoder.set_ffmpeg_filter(Some("stream_missing_filter".into()));
    } else if case == "filterearly" {
        encoder.set_ffmpeg_filter(Some("null".into()));
    }
    encoder
}

#[test]
fn public_stream_child() {
    let Some(root) = std::env::var_os("ENCODER_FIXTURE_ROOT") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let case = std::env::var("ENCODER_FIXTURE_CASE").expect("case");
    let case = case.strip_prefix("stream-errors-").expect("stream failures selection");
    fs::write(root.join("encoder"), ENCODER).expect("encoder");
    fs::set_permissions(root.join("encoder"), fs::Permissions::from_mode(0o755))
        .expect("executable");
    let mut input = shutdown::input(&root, true);
    assert!(input.streams_concurrently());
    let scenes = root.join("scenes");
    fs::create_dir_all(&scenes).expect("scenes");
    fs::write(scenes.join("unrelated.stat"), "caller-owned").expect("sentinel");
    let tasks = (0..2)
        .map(|index| Task {
            index,
            original_index: index + 7,
            frame_indices: if matches!(case, "decode" | "both") {
                vec![0, 9999]
            } else {
                (0..96).collect()
            },
            sub_scenes: None,
            encoder: encoder(&root, case),
            output: scenes.join(format!("{index}.264")),
        })
        .collect();
    let (tx, rx) = std::sync::mpsc::channel();
    let result = if case == "execute" {
        use crate::{
            core::output::Output,
            models::{
                scene::Scene,
                sequence::{DefaultSequenceConfig, DefaultSequenceData},
            },
        };
        let encoder = encoder(&root, "early");
        let mut config = DefaultSequenceConfig::default();
        config.parallel_encoder.workers = Some(1);
        config.parallel_encoder.max_tries = 1; // One-attempt causal diagnostic control.
        config.parallel_encoder.scenes_directory = scenes.clone();
        let callbacks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let saved = Arc::clone(&callbacks);
        let mut condor = Condor::new(
            input,
            Output {
                path: root.join("unused"),
                overwrite: false,
                tags: Default::default(),
                video_tags: Default::default(),
            },
            encoder.clone(),
            std::iter::repeat_with(|| Scene {
                start_frame: 0,
                end_frame: 96,
                sub_scenes: None,
                encoder: encoder.clone(),
                sequence_data: DefaultSequenceData::default(),
            })
            .take(2)
            .collect(),
            Some(config),
            Box::new(move |_| {
                saved.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            }),
        );
        let result =
            ParallelEncoder::default().execute_cancel(&mut condor, tx, Cancellation::default());
        assert!(
            condor.scenes.iter().all(|scene| {
                let data = &scene.sequence_data.parallel_encoder;
                data.bytes.is_none() && data.started_on.is_none() && data.completed_on.is_none()
            }),
            "no failed/queued completion metadata"
        );
        assert_eq!(
            callbacks.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "no success callback"
        );
        result.map(|_| Vec::new())
    } else {
        ParallelEncoder::encode_tasks_with_options(
            &mut input,
            1,
            tasks,
            tx,
            Cancellation::default(),
            ParallelEncoderOptions {
                max_tries: 1,
                validate_frame_count: case != "valid",
            },
        )
    };
    let statuses: Vec<_> = rx.try_iter().collect();
    assert!(
        matches!(
            rx.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        ),
        "all progress relays joined"
    );
    if case == "valid" || case == "validcodec" {
        assert_eq!(
            result
                .expect("healthy pipeline")
                .iter()
                .flatten()
                .map(|r| r.scene)
                .collect::<Vec<_>>(),
            [7, 8]
        );
        if case == "valid" {
            assert_eq!(
                fs::read(scenes.join("0.264")).expect("final output"),
                b"complete"
            );
        } else {
            assert!(scenes.join("0.264").metadata().expect("actual codec output").len() > 0);
        }
    } else {
        let error = result.expect_err("unusable stream");
        let text = format!("{error:#}");
        assert!(
            text.contains(if case == "execute" {
                "Scene 0"
            } else {
                "Scene 7"
            }),
            "{text}"
        );
        assert!(
            !error.is::<crate::utils::cancellation::Cancelled>(),
            "ordinary failure is not cancellation"
        );
        assert!(!statuses.iter().any(|status| matches!(status, SequenceStatus::Whole(Status::Processing { completion: SequenceCompletion::Custom { name, .. }, .. }) if name == "size")), "no success notification");
        assert!(!scenes.join("0.264").exists() && !scenes.join("1.264").exists());
        assert_eq!(
            fs::read_dir(&scenes).expect("cleanup").count(),
            1,
            "no temp/stats/filter artifacts"
        );
        if matches!(case, "decode" | "both") {
            assert!(
                text.contains("Scene 7 frame production"),
                "actual streaming decode operation: {text}"
            );
            assert!(
                text.contains("FFMS2 could not decode frame 9999")
                    && text.contains("Out of bounds"),
                "returned decoder context: {text}"
            );
            assert!(
                error.root_cause().to_string().contains("FFMS2 could not decode frame 9999"),
                "decoder cause stays primary: {text}"
            );
            if case == "both" {
                assert!(
                    text.contains("stream failures independent codec failure")
                        && text.contains("42"),
                    "both genuine failures returned: {text}"
                );
            }
        } else if case == "badfilter" {
            assert!(text.contains("stream_missing_filter"), "{text}");
        } else {
            let codec = result_in(&error).expect("original typed codec status");
            assert_eq!(
                codec.status.code(),
                Some(if case == "badflag" { 255 } else { 42 })
            );
            let marker = if case == "badflag" {
                "unrecognized option"
            } else {
                "stream failures invalid encoder flag"
            };
            assert!(codec.stderr.to_string().contains(marker), "{text}");
            assert!(
                !text.contains("sending on a disconnected channel"),
                "producer fallout must not add diagnostic noise: {text}"
            );
            let codec_position = text.find("Encoder (").expect("encoder diagnostic");
            for symptom in
                ["Broken pipe", "Writing encoder STDIN", "sending on a disconnected channel"]
            {
                if let Some(position) = text.find(symptom) {
                    assert!(
                        codec_position < position,
                        "codec must lead induced symptoms: {text}"
                    );
                }
            }
            if case == "early" {
                for label in ["Scene 7 consumer", "Scene 7 encoder"] {
                    assert_eq!(
                        text.matches(label).count(),
                        1,
                        "one operation context: {text}"
                    );
                }
                let process = error
                    .downcast_ref::<EncoderProcessError>()
                    .expect("actual broken pipe aggregate");
                assert!(process.failures.iter().any(|error| {
                    error
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|error| error.kind() == std::io::ErrorKind::BrokenPipe)
                }));
                assert!(
                    matches!(&process.stderr, StringOrBytes::String(value) if value == "stream failures invalid encoder flag\n")
                );
            }
        }
        if !matches!(case, "badflag" | "decode" | "badfilter") {
            assert_eq!(
                fs::read_to_string(root.join("starts")).expect("one scene started"),
                "started",
                "no queued scene or retry"
            );
        }
        eprintln!("stream failures {case}: {text}");
    }
    assert_eq!(
        fs::read(scenes.join("unrelated.stat")).expect("sentinel"),
        b"caller-owned"
    );
    fs::write(root.join("verified"), "public stream and cleanup checked").expect("verification");
}

#[test]
fn joined_priority_retains_primary_types_and_returned_secondary_context() {
    use crate::utils::cancellation::Cancelled;
    use std::{io, os::unix::process::ExitStatusExt};
    let codec = || EncoderResult {
        encoder: EncoderBase::X264,
        parameters: vec!["--bad-flag".into()],
        status: std::process::ExitStatus::from_raw(42 << 8),
        stdout: b"stdout\xff".to_vec().into(),
        stderr: "stream failures codec cause".to_owned().into(),
    };
    let encoded = || {
        anyhow::Error::from(ParallelEncoderError::EncoderFailed {
            scene: 7,
            result: codec(),
        })
    };
    let decode = || {
        anyhow::Error::from(io::Error::new(
            io::ErrorKind::InvalidData,
            "stream failures decoder cause",
        ))
        .context("Scene 7 frame production")
    };
    let disconnected = || {
        anyhow::Error::from(crossbeam_channel::SendError(std::io::Cursor::new(vec![
            0_u8,
        ])))
    };
    let merged = streaming::joined_scene_result(Err(encoded()), Err(disconnected()))
        .expect_err("codec primary");
    assert!(merged.downcast_ref::<ParallelEncoderError>().is_some());
    assert_eq!(
        result_in(&merged).expect("typed codec").status.code(),
        Some(42)
    );
    assert!(!format!("{merged:#}").contains("sending on a disconnected channel"));

    let merged =
        streaming::joined_scene_result(Err(encoded()), Err(decode())).expect_err("decode primary");
    assert_eq!(
        merged.downcast_ref::<io::Error>().expect("original typed decoder cause").kind(),
        io::ErrorKind::InvalidData
    );
    assert_eq!(
        merged.root_cause().to_string(),
        "stream failures decoder cause"
    );
    let text = format!("{merged:#}");
    assert!(text.starts_with("Scene 7 frame production"), "{text}");
    assert!(
        text.contains("stream failures decoder cause")
            && text.contains("stream failures codec cause"),
        "{text}"
    );
    assert!(text.contains("Scene 7") && text.contains("42"), "{text}");

    let forced = anyhow::Error::from(codec()).context(Cancelled::Forced);
    let baseline = [format!("{forced:#}"), format!("{forced:?}")];
    let merged =
        streaming::joined_scene_result(Err(forced), Err(decode())).expect_err("force primary");
    assert_eq!(merged.downcast_ref::<Cancelled>(), Some(&Cancelled::Forced));
    assert_eq!(
        result_in(&merged).expect("original forced diagnostic").status.code(),
        Some(42)
    );
    assert!(merged.to_string().contains("stream failures decoder cause"));
    for (baseline, rendered) in
        baseline.into_iter().zip([format!("{merged:#}"), format!("{merged:?}")])
    {
        assert_eq!(
            rendered.matches("stream failures codec cause").count(),
            baseline.matches("stream failures codec cause").count(),
            "forced primary is not copied into context: {rendered}"
        );
        assert!(
            rendered.contains("Scene 7 frame production")
                && rendered.contains("stream failures decoder cause"),
            "genuine producer failure retained under force: {rendered}"
        );
    }

    let merged = streaming::joined_scene_result(Ok(codec()), Err(decode()))
        .expect_err("source without consumer failure");
    assert!(merged.is::<io::Error>());
    assert!(
        streaming::joined_scene_result(Err(encoded()), Ok(()))
            .expect_err("codec alone")
            .is::<ParallelEncoderError>()
    );
    assert!(streaming::joined_scene_result(Ok(codec()), Ok(())).is_ok());
}

#[test]
fn fallout_keeps_anyhow_alternate_and_debug_consumer_baseline() {
    use crate::utils::cancellation::Cancelled;
    use std::{io, os::unix::process::ExitStatusExt};
    let mut differences = Vec::new();
    for kind in ["send", "graceful", "forced"] {
        let producer = match kind {
            "send" => {
                anyhow::Error::from(crossbeam_channel::SendError(std::io::Cursor::new(vec![
                    0_u8,
                ])))
            },
            "graceful" => Cancelled::Graceful.into(),
            _ => Cancelled::Forced.into(),
        }
        .context("Scene 7 frame production");
        let codec = EncoderResult {
            encoder: EncoderBase::X264,
            parameters: vec!["--bad-flag".into()],
            status: std::process::ExitStatus::from_raw(42 << 8),
            stdout: "stream failures cause-chain unique stdout marker".to_owned().into(),
            stderr: "stream failures cause-chain unique stderr marker".to_owned().into(),
        };
        let consumer = anyhow::Error::from(EncoderProcessError {
            stdout: codec.stdout.clone(),
            stderr: codec.stderr.clone(),
            result: Some(codec),
            failures: vec![
                anyhow::Error::from(io::Error::from(io::ErrorKind::BrokenPipe))
                    .context("Writing encoder STDIN"),
            ],
        })
        .context("Scene 7 consumer");
        let baseline = [format!("{consumer:#}"), format!("{consumer:?}")];
        let original_root = consumer.root_cause().to_string();
        let merged = streaming::joined_scene_result(Err(consumer), Err(producer))
            .expect_err("original consumer failure");
        assert!(merged.is::<EncoderProcessError>());
        assert_eq!(
            result_in(&merged).expect("typed codec").status.code(),
            Some(42)
        );
        assert_eq!(merged.root_cause().to_string(), original_root);
        for ((style, baseline), rendered) in ["alternate", "Debug"]
            .into_iter()
            .zip(baseline)
            .zip([format!("{merged:#}"), format!("{merged:?}")])
        {
            for marker in [
                "stream failures cause-chain unique stdout marker",
                "stream failures cause-chain unique stderr marker",
            ] {
                let before = baseline.matches(marker).count();
                let after = rendered.matches(marker).count();
                assert!(before > 0, "non-vacuous consumer baseline");
                if after != before {
                    differences.push(format!(
                        "{kind}/{style}: {marker}: baseline={before}, joined={after}"
                    ));
                }
            }
            if rendered != baseline {
                differences.push(format!(
                    "{kind}/{style}: fallout changed original consumer rendering"
                ));
            }
        }
    }
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}

#[test]
fn genuine_secondary_keeps_nested_primary_cause_once_in_anyhow_rendering() {
    use std::{io, os::unix::process::ExitStatusExt};
    let producer = anyhow::Error::from(io::Error::new(
        io::ErrorKind::InvalidData,
        "stream failures cause-chain unique inner decode cause",
    ))
    .context("stream failures cause-chain decode operation")
    .context("Scene 7 frame production");
    let baseline = [format!("{producer:#}"), format!("{producer:?}")];
    let consumer = anyhow::Error::from(EncoderResult {
        encoder: EncoderBase::X264,
        parameters: vec![],
        status: std::process::ExitStatus::from_raw(42 << 8),
        stdout: "stream failures cause-chain secondary stdout marker".to_owned().into(),
        stderr: "stream failures cause-chain secondary stderr marker".to_owned().into(),
    })
    .context("Scene 7 consumer");
    let merged = streaming::joined_scene_result(Err(consumer), Err(producer))
        .expect_err("genuine double failure");
    assert_eq!(
        merged.downcast_ref::<io::Error>().expect("original typed source").kind(),
        io::ErrorKind::InvalidData
    );
    assert_eq!(
        merged.root_cause().to_string(),
        "stream failures cause-chain unique inner decode cause"
    );
    assert!(merged.to_string().starts_with("Scene 7 frame production"));
    let mut differences = Vec::new();
    for ((style, baseline), rendered) in ["alternate", "Debug"]
        .into_iter()
        .zip(baseline)
        .zip([format!("{merged:#}"), format!("{merged:?}")])
    {
        for marker in [
            "stream failures cause-chain secondary stdout marker",
            "stream failures cause-chain secondary stderr marker",
            "Scene 7 consumer",
            "stream failures cause-chain decode operation",
        ] {
            assert!(
                rendered.contains(marker),
                "{style}: both genuine diagnostics returned: {rendered}"
            );
        }
        let before = baseline
            .matches("stream failures cause-chain unique inner decode cause")
            .count();
        let after = rendered
            .matches("stream failures cause-chain unique inner decode cause")
            .count();
        assert_eq!(before, 1, "literal source cause baseline");
        if after != before {
            differences.push(format!(
                "{style}: inner decode cause baseline={before}, joined={after}"
            ));
        }
    }
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}
