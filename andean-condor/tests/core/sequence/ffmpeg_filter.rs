use std::{
    io::Cursor,
    process::Command,
    sync::{Arc, Mutex},
    thread,
};

use andean_condor::core::sequence::ffmpeg_filter::FfmpegFilter;
#[cfg(target_os = "linux")]
use andean_condor::{
    core::input::Input,
    models::input::{ImportMethod, Input as InputModel},
};
use anyhow::Result;

#[cfg(target_os = "linux")]
#[path = "fixtures/filter_lifecycle.rs"]
mod lifecycle_fixture;

#[cfg(target_os = "linux")]
mod parallel_failures {
    use super::*;
    use andean_condor::{
        core::{
            Condor,
            encoder::EncoderResult,
            output::Output,
            sequence::{
                Sequence, SequenceStatus,
                parallel_encoder::{ParallelEncoder, ParallelEncoderError, Task},
            },
        },
        models::{
            encoder::{Encoder, EncoderBase, cli_parameter::CLIParameter},
            scene::Scene,
            sequence::{DefaultSequenceConfig, DefaultSequenceData},
        },
    };
    use std::{collections::HashMap, fs, os::unix::fs::PermissionsExt, path::Path};

    #[test]
    fn filter_failures_preserve_encoder_diagnostics_and_clear_outputs() {
        lifecycle_fixture::isolated(
            module_path!(),
            "filter_failures_preserve_encoder_diagnostics_and_clear_outputs",
            &["native-invalid", "native-status", "native-filter"],
            assert_failure,
        );
    }

    #[test]
    fn filter_preflight_or_spawn_failure_unblocks_producer() {
        lifecycle_fixture::isolated(
            module_path!(),
            "filter_preflight_or_spawn_failure_unblocks_producer",
            &["native-preflight", "native-spawn"],
            assert_failure,
        );
    }

    fn assert_failure(root: &Path, case: &str) {
        let mut input = lifecycle_fixture::native_input(root);
        let mut encoder = Encoder::default_from_base(&EncoderBase::X264, true);
        encoder.set_append_color_data(false);
        encoder.set_quantizer(23.0);
        encoder.set_ffmpeg_filter(Some(
            if case.ends_with("filter") {
                "lifecycle_missing_filter"
            } else {
                "null"
            }
            .to_owned(),
        ));
        if case.ends_with("invalid") {
            encoder.parameters_mut().insert(
                "filter-lifecycle-invalid-option".into(),
                CLIParameter::new_bool("--", true),
            );
        }
        if case == "native-status" {
            let path = root.join("early encoder");
            fs::write(
                &path,
                "#!/bin/sh\nprintf 'filter-lifecycle early encoder diagnostic' >&2\nexit 42\n",
            )
            .expect("early status fixture");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("executable");
            encoder.set_executable(Some(path));
        }
        let scenes_dir = root.join("scenes");
        fs::create_dir_all(&scenes_dir).expect("scenes");
        let (progress_tx, progress_rx) = std::sync::mpsc::channel();
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let error = if matches!(case, "native-preflight" | "native-spawn") {
            let executable = root.join("unspawnable encoder");
            if case == "native-spawn" {
                fs::write(
                    &executable,
                    format!("#!{}\n", root.join("missing-interpreter").display()),
                )
                .expect("script");
                fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).expect("mode");
            }
            encoder.set_executable(Some(executable));
            ParallelEncoder::encode_tasks(
                &mut input,
                1,
                [Task {
                    original_index: 7,
                    index: 0,
                    frame_indices: (0..96).collect(),
                    sub_scenes: None,
                    encoder,
                    output: scenes_dir.join("00007.264"),
                }]
                .into(),
                progress_tx,
                cancelled.into(),
                andean_condor::core::sequence::parallel_encoder::ParallelEncoderOptions::default(),
            )
            .expect_err("early failure must unblock inline producer")
        } else {
            let saved = Arc::new(Mutex::new(0));
            let observed = Arc::clone(&saved);
            let mut config = DefaultSequenceConfig::default();
            config.parallel_encoder.workers = Some(1);
            config.parallel_encoder.scenes_directory = scenes_dir.clone();
            let mut condor = Condor::<DefaultSequenceData, DefaultSequenceConfig>::new(
                input,
                Output {
                    path: root.join("unused"),
                    overwrite: false,
                    tags: HashMap::new(),
                    video_tags: HashMap::new(),
                },
                encoder.clone(),
                vec![Scene {
                    start_frame: 0,
                    end_frame: 96,
                    sub_scenes: None,
                    encoder,
                    sequence_data: DefaultSequenceData::default(),
                }],
                Some(config),
                Box::new(move |data| {
                    if data.scenes.iter().any(|s| {
                        s.sequence_data.parallel_encoder.bytes.is_some()
                            || s.sequence_data.parallel_encoder.completed_on.is_some()
                    }) {
                        *observed.lock().expect("saved") += 1;
                    }
                    Ok(())
                }),
            );
            let error = ParallelEncoder::default()
                .execute(&mut condor, progress_tx, cancelled.into())
                .expect_err("filter/encoder failure");
            assert_eq!(
                *saved.lock().expect("saved"),
                0,
                "no failed completion saved"
            );
            assert!(
                condor.scenes.iter().all(|s| s.sequence_data.parallel_encoder.bytes.is_none()
                    && s.sequence_data.parallel_encoder.completed_on.is_none())
            );
            error
        };
        let diagnostic = format!("{error:#}");
        match case {
            "native-filter" => assert!(
                diagnostic.contains("lifecycle_missing_filter"),
                "original FFmpeg stderr: {diagnostic}"
            ),
            "native-invalid" | "native-status" => {
                let result = error
                    .chain()
                    .find_map(|e| e.downcast_ref::<EncoderResult>())
                    .or_else(|| match error.downcast_ref::<ParallelEncoderError>() {
                        Some(ParallelEncoderError::EncoderFailed { result, .. }) => Some(result),
                        _ => None,
                    })
                    .expect("typed original encoder result");
                assert!(!result.status.success());
                if case == "native-status" {
                    assert_eq!(result.status.code(), Some(42));
                    assert_eq!(
                        result.stderr.to_string(),
                        "filter-lifecycle early encoder diagnostic"
                    );
                } else {
                    assert!(
                        result.stderr.to_string().contains("filter-lifecycle-invalid-option"),
                        "{}",
                        result.stderr
                    );
                }
                assert!(
                    !diagnostic.contains("FFmpeg filter failed"),
                    "intentional abort must not mask encoder: {diagnostic}"
                );
            },
            "native-spawn" => assert!(diagnostic.contains("Could not spawn"), "{diagnostic}"),
            "native-preflight" => {
                assert!(diagnostic.contains("unspawnable encoder"), "{diagnostic}");
            },
            _ => unreachable!(),
        }
        assert_eq!(
            fs::read_dir(&scenes_dir).expect("outputs").count(),
            0,
            "no failed output or statistics survive"
        );
        let events: Vec<_> = progress_rx.try_iter().collect();
        assert!(
            matches!(
                progress_rx.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Disconnected)
            ),
            "all progress relays joined"
        );
        assert!(!events.iter().any(|e| matches!(e, SequenceStatus::Whole(andean_condor::core::sequence::Status::Processing { completion: andean_condor::core::sequence::SequenceCompletion::Custom { name, .. }, .. }) if name == "size")));
        eprintln!(
            "filter-lifecycle {case}: 96 frames/window 8; original diagnostic retained: {diagnostic}"
        );
    }
}

fn y4m_stream(frames: usize) -> Vec<u8> {
    let mut stream = b"YUV4MPEG2 W8 H8 F1:1 Ip A1:1 C420mpeg2\n".to_vec();
    for _ in 0..frames {
        stream.extend_from_slice(b"FRAME\n");
        stream.extend_from_slice(&[16_u8; 8 * 8]);
        stream.extend_from_slice(&[128_u8; 4 * 4]);
        stream.extend_from_slice(&[128_u8; 4 * 4]);
    }
    stream
}

fn run_filter(graph: &str, stream: Vec<u8>) -> (Vec<u8>, Result<()>) {
    let (input_tx, input_rx) = crossbeam_channel::bounded(2);
    let failure = Arc::new(Mutex::new(None));
    let mut stage = FfmpegFilter::spawn(
        input_rx,
        2,
        graph,
        failure,
        &andean_condor::utils::cancellation::Cancellation::default(),
    )
    .expect("FfmpegFilter should spawn");
    let output_rx = stage.take_receiver().expect("sole receiver");
    let producer = thread::spawn(move || {
        // Feed one chunk per frame to mirror window-streamed producers.
        let split = stream
            .windows(6)
            .position(|window| window == b"FRAME\n")
            .expect("stream should have frames");
        input_tx.send(Cursor::new(stream[..split].to_vec())).ok();
        let mut rest = &stream[split..];
        while !rest.is_empty() {
            let end = rest[6..]
                .windows(6)
                .position(|window| window == b"FRAME\n")
                .map_or(rest.len(), |offset| offset + 6);
            input_tx.send(Cursor::new(rest[..end].to_vec())).ok();
            rest = &rest[end..];
        }
    });
    let mut output = Vec::new();
    for chunk in output_rx {
        output.extend_from_slice(chunk.get_ref());
    }
    producer.join().expect("producer thread should join");
    (output, stage.check())
}

#[test]
fn null_filter_preserves_y4m_header_and_frame_count() {
    if which::which("ffmpeg").is_err() {
        eprintln!("skipping null_filter_preserves_y4m_header_and_frame_count: ffmpeg not in PATH");
        return;
    }
    let frames = 8;
    let (output, result) = run_filter("null", y4m_stream(frames));
    result.expect("the null filter should not fail");
    let output = String::from_utf8_lossy(&output);
    assert!(
        output.starts_with("YUV4MPEG2"),
        "output starts with a Y4M header"
    );
    assert_eq!(
        output.matches("FRAME\n").count(),
        frames,
        "passthrough preserves the frame count"
    );
}

#[test]
fn crop_filter_updates_dimensions() {
    if which::which("ffmpeg").is_err() {
        eprintln!("skipping crop_filter_updates_dimensions: ffmpeg not in PATH");
        return;
    }
    let (output, result) = run_filter("crop=6:6", y4m_stream(8));
    result.expect("the crop filter should not fail");
    let output = String::from_utf8_lossy(&output);
    assert!(
        output.starts_with("YUV4MPEG2"),
        "output starts with a Y4M header"
    );
    assert!(
        output.contains("W6 H6"),
        "FFmpeg regenerates the header with the cropped dimensions"
    );
}

#[test]
fn reports_invalid_filter_failure() {
    if which::which("ffmpeg").is_err() {
        eprintln!("skipping reports_invalid_filter_failure: ffmpeg not in PATH");
        return;
    }
    let (output, result) = run_filter("definitely_not_a_filter", y4m_stream(8));
    let message = result.expect_err("an invalid filter should fail the stage");
    assert!(output.is_empty(), "a failed filter produces no frames");
    let message = message.to_string();
    assert!(
        message.contains("definitely_not_a_filter"),
        "failure message includes FFmpeg's stderr: {message}"
    );
}

#[test]
fn spawn_reports_path_lookup_context_when_ffmpeg_is_unavailable() {
    if std::env::var_os("FILTER_MISSING_FFMPEG").is_none() {
        let empty = tempfile::tempdir().expect("empty executable search path");
        let module = module_path!().split_once("::").expect("test crate prefix").1;
        let exact =
            format!("{module}::spawn_reports_path_lookup_context_when_ffmpeg_is_unavailable");
        let child = Command::new(std::env::current_exe().expect("test binary"))
            .args(["--exact", &exact, "--nocapture"])
            .env("FILTER_MISSING_FFMPEG", "1")
            .env("PATH", empty.path())
            .output()
            .expect("child-local missing-tool check");
        let stdout = String::from_utf8_lossy(&child.stdout);
        assert!(
            child.status.success()
                && stdout.contains("1 passed; 0 failed")
                && stdout.contains("missing FFmpeg lookup asserted"),
            "{stdout}\n{}",
            String::from_utf8_lossy(&child.stderr)
        );
        return;
    }
    let (input_tx, input_rx) = crossbeam_channel::bounded(2);
    drop(input_tx);
    let Err(error) = FfmpegFilter::spawn(
        input_rx,
        2,
        "null",
        Arc::new(Mutex::new(None)),
        &andean_condor::utils::cancellation::Cancellation::default(),
    ) else {
        panic!("FFmpeg cannot be found in an empty PATH");
    };
    assert!(
        error.to_string().contains("not found in PATH"),
        "spawn error names the PATH lookup: {error}"
    );
    println!("missing FFmpeg lookup asserted");
}
