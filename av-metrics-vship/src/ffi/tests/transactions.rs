//! Removing any native-call guard must expose a gated entry or overlapping C regions.
#![cfg(unix)]

use super::*;
use crate::{VideoFormat, VshipConfig, VshipMetric, VshipScorer};
use std::{
    fs::File,
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};
use v_frame::{chroma::ChromaSubsampling, frame::FrameBuilder};

#[path = "../../../tests/common/mod.rs"]
mod common;

#[test]
fn device_count_failure_holds_native_transaction_through_short_diagnostic() {
    transaction_cases(
        "device_count_failure_holds_native_transaction_through_short_diagnostic",
        0,
    );
}

#[test]
fn device_info_failure_holds_native_transaction_through_short_diagnostic() {
    transaction_cases(
        "device_info_failure_holds_native_transaction_through_short_diagnostic",
        1,
    );
}

#[test]
fn gpu_admission_failure_holds_native_transaction_through_short_diagnostic() {
    transaction_cases(
        "gpu_admission_failure_holds_native_transaction_through_short_diagnostic",
        2,
    );
}

#[test]
fn handler_init_failure_holds_native_transaction_without_adopting_output() {
    transaction_cases(
        "handler_init_failure_holds_native_transaction_without_adopting_output",
        3,
    );
}

#[test]
fn metric_compute_failure_holds_native_transaction_and_frees_owned_handler() {
    transaction_cases(
        "metric_compute_failure_holds_native_transaction_and_frees_owned_handler",
        4,
    );
}

#[test]
fn history_reset_failure_holds_native_transaction_and_frees_handler() {
    transaction_cases(
        "history_reset_failure_holds_native_transaction_and_frees_handler",
        5,
    );
}

#[test]
fn score_reset_failure_holds_native_transaction_and_frees_handler() {
    transaction_cases(
        "score_reset_failure_holds_native_transaction_and_frees_handler",
        6,
    );
}

#[test]
fn handler_free_failure_is_serialized_and_frees_exact_handle_once() {
    transaction_cases(
        "handler_free_failure_is_serialized_and_frees_exact_handle_once",
        7,
    );
}

#[test]
fn short_error_translation_itself_holds_the_native_transaction() {
    isolated(
        "short_error_translation_itself_holds_the_native_transaction",
        &["8/17/2".into()],
    );
}

#[test]
fn concurrent_thread_local_scorers_share_one_default_probe_and_free_distinct_handles() {
    isolated(
        "concurrent_thread_local_scorers_share_one_default_probe_and_free_distinct_handles",
        &["lifecycle-concurrent".into()],
    );
}

#[test]
fn score_callback_reenters_public_api_outside_native_lock() {
    isolated(
        "score_callback_reenters_public_api_outside_native_lock",
        &["lifecycle-callback".into()],
    );
}

#[test]
fn poisoned_native_call_lock_still_allows_drop_and_later_calls() {
    isolated(
        "poisoned_native_call_lock_still_allows_drop_and_later_calls",
        &["lifecycle-poison".into()],
    );
}

#[test]
fn explicit_init_and_first_default_lookup_complete_without_lock_inversion() {
    isolated(
        "explicit_init_and_first_default_lookup_complete_without_lock_inversion",
        &["order".into()],
    );
}

fn transaction_cases(test: &str, kind: u32) {
    let mut scenarios = Vec::new();
    for status in [17, 777, -123] {
        for metric in if matches!(kind, 3 | 4 | 7) {
            0..3
        } else {
            2..3
        } {
            scenarios.push(format!("{kind}/{status}/{metric}"));
        }
    }
    isolated(test, &scenarios);
}

fn isolated(test: &str, scenarios: &[String]) {
    if let Ok(scenario) = std::env::var("VSHIP_TRANSACTION_FIXTURE_CHILD") {
        assert!(
            scenarios.contains(&scenario),
            "case belongs to this contract"
        );
        child(&scenario);
        println!("native transaction {scenario}: assertions completed");
        return;
    }
    let module = module_path!().split_once("::").expect("test crate prefix").1;
    let exact = format!("{module}::{test}");
    let dir = tempfile::tempdir().expect("fixture directory");
    let library = dir.path().join(if cfg!(target_os = "macos") {
        "libvship.dylib"
    } else {
        "libvship.so"
    });
    let out = Command::new(std::env::var_os("CC").unwrap_or_else(|| "cc".into()))
        .args(["-std=c11", "-shared", "-fPIC", "-Wall", "-Wextra", "-Werror"])
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/native_call.c"
        ))
        .arg("-o")
        .arg(&library)
        .output()
        .expect("C compiler required");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let mut failures = Vec::new();
    for (i, scenario) in scenarios.iter().enumerate() {
        // File-backed output cannot fill a pipe while the watchdog waits.
        let output = dir.path().join(format!("child-{i}.log"));
        let stdout = File::create(&output).expect("output file");
        let stderr = stdout.try_clone().expect("stderr");
        let mut process = Command::new(std::env::current_exe().expect("executable"))
            .args(["--exact", &exact, "--nocapture"])
            .env("VSHIP_TRANSACTION_FIXTURE_CHILD", scenario)
            .env("VSHIP_TRANSACTION_FIXTURE_LIBRARY", &library)
            .env("VSHIP_PLUGIN_PATH", dir.path())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .expect("child");
        let start = Instant::now();
        let status = loop {
            if let Some(status) = process.try_wait().expect("status") {
                break Some(status);
            }
            if start.elapsed() > Duration::from_secs(30) {
                process.kill().expect("kill timed-out child");
                process.wait().expect("reap timed-out child");
                break None;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let text = std::fs::read_to_string(output).expect("child output");
        if status.is_some_and(|s| s.success())
            && text.contains("1 passed; 0 failed")
            && text.contains(&format!(
                "native transaction {scenario}: assertions completed"
            ))
        {
            print!("{text}");
        } else {
            failures.push(format!("{scenario}: {status:?}\n{text}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn format() -> VideoFormat {
    VideoFormat {
        width: 32,
        height: 32,
        bit_depth: 8,
        chroma_sampling: ChromaSubsampling::Yuv420,
        color_range: None,
        color_primaries: None,
        color_transfer_characteristic: None,
        color_space: None,
        chroma_location: None,
    }
}

fn new(metric: VshipMetric) -> Result<VshipScorer, VshipError> {
    VshipScorer::new(
        VshipConfig::default().with_metric(metric).with_gpu_id(0),
        format(),
        format(),
        None,
    )
}

fn assert_error(result: Result<(), VshipError>, kind: u32, status: i32) {
    let function = [
        "Vship_GetDeviceCount",
        "Vship_GetDeviceInfo",
        "Vship_GPUFullCheck",
        "Vship_InitHandler",
        "Vship_ComputeHandler",
        "Vship_Reset",
        "Vship_ResetScore",
    ][kind as usize];
    let message = if status == 17 {
        "short status 17".to_owned()
    } else {
        format!("libvship error {status}")
    };
    match result.expect_err("controlled native failure") {
        VshipError::MetricUnavailable { reason, .. } if kind == 3 => {
            assert_eq!(
                reason,
                format!("libvship call `{function}` failed with status {status}: {message}")
            );
        },
        VshipError::CallFailed {
            function: f,
            status: s,
            message: m,
        } => {
            assert_eq!((f, s, m), (function, status, message));
        },
        error => panic!("unexpected error {error:?}"),
    }
}

fn child(scenario: &str) {
    // SAFETY: pinned-header test exports with exact signatures; mapping outlives workers.
    let library = unsafe {
        libloading::Library::new(
            std::env::var_os("VSHIP_TRANSACTION_FIXTURE_LIBRARY").expect("library"),
        )
    }
    .expect("fixture mapping");
    // SAFETY: both functions only operate on atomic fixture state and validate indices.
    let (count, set) = unsafe {
        (
            *library
                .get::<unsafe extern "C" fn(u32) -> u32>(b"transaction_count\0")
                .expect("counter"),
            *library
                .get::<unsafe extern "C" fn(u32, i32)>(b"transaction_set\0")
                .expect("control"),
        )
    };
    // SAFETY: all call sites use the fixture's defined indices, while mapped.
    let count = |i| unsafe { count(i) };
    // SAFETY: controls are atomic, with indices bounded below.
    let set = |i, value| unsafe { set(i, value) };
    let api = VshipApi::load().expect("production API");
    if scenario.starts_with("lifecycle-") {
        lifecycle(api, count, set, scenario);
        return;
    }
    if scenario == "order" {
        lock_order(count, set);
        return;
    }
    let values = scenario
        .split('/')
        .map(|s| s.parse::<i32>().expect("integer"))
        .collect::<Vec<_>>();
    let (kind, status, metric) = (
        values[0] as u32,
        values[1],
        VshipMetric::all()[values[2] as usize],
    );
    let (ready_tx, ready_rx) = mpsc::channel();
    let (go_tx, go_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        // !Send scorers are created, used and destroyed on their own worker.
        let mut scorer = (4..=7).contains(&kind).then(|| new(metric).expect("worker scorer"));
        ready_tx.send(()).expect("ready");
        go_rx.recv().expect("start");
        let result = match kind {
            0 => api.device_count().map(|_| ()),
            1 => api.device_info(0).map(|_| ()),
            2 | 8 => api.gpu_full_check(0),
            3 => new(metric).map(drop),
            4 => {
                let frame = FrameBuilder::new(32, 32, ChromaSubsampling::Yuv420, 8)
                    .build::<u8>()
                    .expect("frame");
                scorer.as_mut().expect("scorer").submit_pair(&frame, &frame).map(|_| ())
            },
            5 => scorer.as_mut().expect("scorer").reset_temporal(),
            6 => scorer.as_mut().expect("scorer").reset_score(),
            7 => {
                drop(scorer.take());
                Ok(())
            },
            _ => unreachable!(),
        };
        if kind != 7 {
            assert_error(result, if kind == 8 { 2 } else { kind }, status);
        }
        drop(scorer);
    });
    ready_rx.recv().expect("worker prepared");
    set(if kind == 8 { 2 } else { kind }, status);
    let challenger_kind = u32::from(kind != 1);
    set(challenger_kind, -123);
    set(8, kind as i32);
    go_tx.send(()).expect("start worker");
    while count(11) == 0 {
        std::thread::yield_now();
    }
    // Deterministic ownership evidence at every gated export, including the
    // short diagnostic: this supplements the bounded overlap counter below.
    let guarded = matches!(
        NATIVE_CALLS.try_lock(),
        Err(std::sync::TryLockError::WouldBlock)
    );
    let (attempt_tx, attempt_rx) = mpsc::channel();
    let challenger = std::thread::spawn(move || {
        attempt_tx.send(()).expect("attempt");
        let result = if kind == 1 {
            api.device_count().map(|_| ())
        } else {
            api.device_info(2).map(|_| ())
        };
        assert_error(result, challenger_kind, -123);
    });
    attempt_rx.recv().expect("challenger scheduled");
    // Gate holds the first actual native entry; bounded yields expose an unguarded
    // challenger without asserting elapsed time or invoking racy C++ code.
    for _ in 0..20_000 {
        if count(10) != 0 {
            break;
        }
        std::thread::yield_now();
    }
    set(9, 1);
    worker.join().expect("worker");
    challenger.join().expect("challenger");
    println!(
        "native transaction {scenario}: overlaps={}, calls={:?}, live={}, freed-mask={:#x}; 20000-yield scheduling bound",
        count(10),
        (0..10).map(count).collect::<Vec<_>>(),
        count(12),
        count(13)
    );
    assert_eq!(
        count(10),
        0,
        "global-mutating native entry regions overlapped"
    );
    assert!(
        guarded,
        "gated native entry/diagnostic did not hold the shared call guard"
    );
    assert_eq!(count(12), 0, "all adopted handles freed, including errors");
    assert_eq!(count(9), 0, "short diagnostics never copy global details");
    assert_eq!(
        count(8),
        u32::from(status == 17),
        "unknown statuses skip enum translation"
    );
    let owned = u32::from((4..=7).contains(&kind));
    assert_eq!(
        count(7),
        owned,
        "failed init is not adopted; owned handles free once"
    );
    assert_eq!(count(13), owned, "exact owned handle identity freed once");
    let mut expected = [0; 10];
    expected[challenger_kind as usize] += 1;
    expected[if kind == 8 { 2 } else { kind } as usize] += 1;
    if kind == 3 || owned != 0 {
        expected[2] += 1;
    }
    if owned != 0 {
        expected[3] += 1;
    }
    if (4..=6).contains(&kind) {
        expected[7] += 1;
    }
    expected[8] = u32::from(status == 17);
    assert_eq!(
        (0..10).map(count).collect::<Vec<_>>(),
        expected,
        "exact native call inventory"
    );
}

fn lifecycle(
    api: &'static VshipApi,
    count: impl Fn(u32) -> u32,
    _set: impl Fn(u32, i32),
    scenario: &str,
) {
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(6));
    let workers = (0..6)
        .map(|i| {
            let barrier = std::sync::Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                assert!(crate::is_available());
                let metric = VshipMetric::all()[i % 3];
                let mut scorer = VshipScorer::new(
                    VshipConfig::default().with_metric(metric),
                    format(),
                    format(),
                    None,
                )
                .expect("independent scorer");
                let frame = FrameBuilder::new(32, 32, ChromaSubsampling::Yuv420, 8)
                    .build::<u8>()
                    .expect("frame");
                assert_eq!(
                    scorer.submit_pair(&frame, &frame).expect("score").value(metric, true),
                    Some(42.0)
                );
                scorer.reset_temporal().expect("history");
                scorer.reset_score().expect("score reset");
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        worker.join().expect("success worker");
    }
    assert_eq!(
        (count(0), count(1), count(2)),
        (1, 2, 1),
        "concurrent default probes once"
    );
    assert_eq!((count(3), count(4), count(7), count(12)), (6, 6, 6, 0));
    assert_eq!(count(13), 0x3f, "six distinct handlers freed once");
    assert_eq!(api.version_string(), "5.1.2");
    println!(
        "native transaction lifecycle: six thread-local scorers, markers=42, init/compute/free=6/6/6, default count/info/probe=1/2/1"
    );
    if scenario == "lifecycle-concurrent" {
        return;
    }

    let clip = common::SyntheticClip::generate(
        32,
        32,
        8,
        ChromaSubsampling::Yuv420,
        1,
        common::Distortion::Mild,
    );
    let mut scorer = new(VshipMetric::Cvvdp).expect("callback scorer");
    let mut callbacks = 0;
    let scores = scorer
        .score_decoders::<u8>(&mut clip.decoder(), &mut clip.decoder(), |i, score| {
            assert_eq!((i, score.score), (0, Some(42.0)));
            assert!(
                NATIVE_CALLS.try_lock().is_ok(),
                "user callback outside native lock"
            );
            assert!(crate::is_available());
            drop(new(VshipMetric::Butteraugli).expect("reentrant callback constructor"));
            api.device_count().expect("reentrant callback probe");
            callbacks += 1;
        })
        .expect("decoder scoring");
    assert_eq!((callbacks, scores.len()), (1, 1));
    if scenario == "lifecycle-callback" {
        return;
    }

    // Test-only panic inside the SAME production transaction used by the safe
    // APIs; no user callbacks are accepted there. The actual C call completes.
    let panicked = std::panic::catch_unwind(|| {
        api.call("Vship_GetDeviceCount", || {
            let mut n = 0;
            // SAFETY: writable integer, exact pinned fixture signature.
            assert_eq!(unsafe { (api.get_device_count)(&raw mut n) }, 0);
            assert_eq!(n, 3);
            panic!("native transaction deliberate unwind");
        })
    });
    assert!(panicked.is_err() && NATIVE_CALLS.is_poisoned());
    drop(scorer); // Pre-panic owned handle must still be released, without a second panic.
    assert_eq!(api.device_count().expect("poison recovery probe"), 3);
    for metric in VshipMetric::all() {
        drop(new(metric).expect("poison recovery init/free"));
    }
    assert_eq!(
        (count(3), count(7), count(12), count(13)),
        (11, 11, 0, 0x7ff)
    );
    println!(
        "native transaction callback reentry and poison recovery: init/free=11/11, live=0, freed-mask=0x7ff"
    );
}

fn lock_order(count: impl Fn(u32) -> u32, set: impl Fn(u32, i32)) {
    set(8, 3); // Explicit init holds native lock before the FIRST default lookup.
    let explicit = std::thread::spawn(|| drop(new(VshipMetric::Cvvdp).expect("explicit init")));
    while count(11) == 0 {
        std::thread::yield_now();
    }
    let (tx, rx) = mpsc::channel();
    let default = std::thread::spawn(move || {
        tx.send(()).expect("default start");
        assert!(crate::is_available());
        drop(
            VshipScorer::new(VshipConfig::default(), format(), format(), None)
                .expect("default init"),
        );
    });
    rx.recv().expect("default scheduled");
    for _ in 0..20_000 {
        std::thread::yield_now();
    }
    set(9, 1);
    explicit.join().expect("explicit finishes without default cache reentry");
    default.join().expect("default finishes cache -> native");
    assert_eq!(
        (count(0), count(1), count(2), count(3), count(7), count(12)),
        (1, 2, 2, 2, 2, 0)
    );
    println!(
        "native transaction lock order: explicit init vs first default OnceLock/cache lookup joined; init/free=2/2"
    );
}
