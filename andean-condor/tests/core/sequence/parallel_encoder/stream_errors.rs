//! Public native streams; each scenario runs in one exact bounded child.
use andean_condor::{
    core::{
        Condor,
        encoder::{EncoderProcessError, EncoderResult, StringOrBytes},
        input::Input,
        sequence::{
            Sequence, SequenceCompletion, SequenceStatus, Status,
            parallel_encoder::{
                ParallelEncoder, ParallelEncoderError, ParallelEncoderOptions, Task,
            },
        },
    },
    models::{
        encoder::{Encoder, EncoderBase, cli_parameter::CLIParameter},
        input::{ImportMethod, Input as InputModel},
    },
    utils::cancellation::Cancellation,
};
use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command, sync::Arc};

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
fn native_stream_decode_failure_stays_primary_and_retains_independent_codec_failure() {
    stream_cases(
        "native_stream_decode_failure_stays_primary_and_retains_independent_codec_failure",
        &["decode", "both"],
    );
}

#[test]
fn native_stream_codec_or_filter_failure_suppresses_induced_send_fallout() {
    stream_cases(
        "native_stream_codec_or_filter_failure_suppresses_induced_send_fallout",
        &["early", "badflag", "badfilter", "filterearly", "execute"],
    );
}

#[test]
fn healthy_native_streams_publish_each_scene_without_leaking_progress_senders() {
    stream_cases(
        "healthy_native_streams_publish_each_scene_without_leaking_progress_senders",
        &["valid", "validcodec"],
    );
}

fn stream_cases(test: &str, cases: &[&str]) {
    if std::env::var_os("ENCODER_FIXTURE_ROOT").is_some() {
        let case = std::env::var("ENCODER_FIXTURE_CASE").expect("case");
        let case = case.strip_prefix("stream-errors-").expect("stream failures selection");
        assert!(
            cases.contains(&case),
            "stream case belongs to this contract"
        );
        check_stream_case();
        println!("stream-errors {case}: assertions completed");
        return;
    }
    let dir = tempfile::tempdir().expect("fixture");
    let root = dir.path();
    fs::write(
        root.join("watchdog.rs"),
        include_str!("../../encoder/fixtures/fake_encoder.rs"),
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
    for case in cases {
        let job = root.join(case);
        fs::create_dir_all(&job).expect("job");
        let output = Command::new(root.join("watchdog"))
            .arg("--encoder-fixture-watchdog")
            .arg(std::env::current_exe().expect("test binary"))
            .arg(&job)
            .arg(format!("stream-errors-{case}"))
            .arg(format!(
                "core::sequence::parallel_encoder::stream_errors::{test}"
            ))
            .output()
            .expect("supervisor");
        let text = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        eprintln!("{text}");
        if !output.status.success()
            || !job.join("verified").exists()
            || !text.contains("test result: ok. 1 passed;")
            || !text
                .lines()
                .any(|line| line == "encoder fixture watchdog reaped 0 descendants; timeout=false")
            || !text.contains(&format!("stream-errors {case}: assertions completed"))
        {
            failures.push(format!("{case}: {text}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

pub(super) fn native_input(root: &Path) -> Input {
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

fn check_stream_case() {
    let Some(root) = std::env::var_os("ENCODER_FIXTURE_ROOT") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let case = std::env::var("ENCODER_FIXTURE_CASE").expect("case");
    let case = case.strip_prefix("stream-errors-").expect("stream failures selection");
    fs::write(root.join("encoder"), ENCODER).expect("encoder");
    fs::set_permissions(root.join("encoder"), fs::Permissions::from_mode(0o755))
        .expect("executable");
    let mut input = native_input(&root);
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
        use andean_condor::{
            core::output::Output,
            models::{
                scene::Scene,
                sequence::{DefaultSequenceConfig, DefaultSequenceData},
            },
        };
        let encoder = encoder(&root, "early");
        let mut config = DefaultSequenceConfig::default();
        config.parallel_encoder.workers = Some(1);
        config.parallel_encoder.max_tries = 1;
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
        let result = ParallelEncoder::default().execute(&mut condor, tx, Cancellation::default());
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
        ParallelEncoder::encode_tasks(
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
            !error.is::<andean_condor::utils::cancellation::Cancelled>(),
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
                    matches!(&process.stderr,StringOrBytes::String(value) if value == "stream failures invalid encoder flag\n")
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
