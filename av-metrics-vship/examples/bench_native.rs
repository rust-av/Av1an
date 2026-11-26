//! Scores a reference/encode pair with libvship and reports timing and scores.
//!
//! A development harness for comparing the native path against FFVship and the
//! VapourSynth plugin on real content. Run with:
//!
//! ```text
//! cargo run --release -p av-metrics-vship --example bench_native -- \
//!     <reference> <encode> <metric> [target_or_auto] [handler_threads]
//! ```
//!
//! Prints one `METRIC=... SECONDS=... MEAN=...` line so a driver can compare it
//! against the other tools without parsing prose.
//! `handler_threads` is a stored legacy request; scoring always uses one handler.

use std::{hint::black_box, time::Instant};

use av_decoders::Decoder;
use av_metrics_vship::{
    DEFAULT_HANDLER_THREADS, PoolMethod, VideoFormat, VshipConfig, VshipMetric, VshipScorer,
    is_available,
};

fn main() {
    let mut args = std::env::args().skip(1);
    let reference_path = args.next().expect("a reference path is required");
    let encode_path = args.next().expect("an encode path is required");
    let metric_name = args.next().expect("a metric name is required");

    // libvship performs no geometry-equality check between the two sides: it
    // fails `Vship_InitHandler` with `DifferingInputType` unless both
    // colorspaces resolve to the same size. Scaling is therefore explicit,
    // and FFVship's behaviour -- always scale the encode to the source --
    // is the default here. Pass `none` to leave both sides unscaled.
    let target: Option<(u32, u32)> = match args.next().as_deref() {
        None | Some("auto") => None,
        Some("none") => Some((0, 0)),
        Some(value) => {
            let (width, height) = value.split_once('x').expect("target must be WIDTHxHEIGHT");
            Some((
                width.parse().expect("width must be a number"),
                height.parse().expect("height must be a number"),
            ))
        },
    };

    let handler_threads: u32 = args.next().map_or_else(
        || DEFAULT_HANDLER_THREADS,
        |value| value.parse().expect("handler count must be a number"),
    );

    let metric = match metric_name.to_ascii_uppercase().as_str() {
        "SSIMULACRA2" | "SSIMU2" => VshipMetric::Ssimulacra2,
        "BUTTERAUGLI" => VshipMetric::Butteraugli,
        "CVVDP" => VshipMetric::Cvvdp,
        other => panic!("unknown metric `{other}`"),
    };

    assert!(
        is_available(),
        "libvship is unavailable: set VSHIP_PLUGIN_PATH or VSSCRIPT_PATH"
    );

    let mut reference = Decoder::from_file(&reference_path).expect("open the reference");
    let mut distorted = Decoder::from_file(&encode_path).expect("open the encode");

    let reference_details = *reference.get_video_details();
    let distorted_details = *distorted.get_video_details();

    // `auto` scales the encode to the source, which is what FFVship does and
    // what makes a differently-sized encode scoreable at all.
    let target_resolution = match target {
        Some((0, 0)) => None,
        Some(size) => Some(size),
        None => Some((
            reference_details.width as u32,
            reference_details.height as u32,
        )),
    };

    let config = VshipConfig::default().with_metric(metric).with_handler_threads(handler_threads);

    // CVVDP's temporal filter is framerate-dependent, so a real frame rate must
    // be supplied. FFVship and the VapourSynth plugin both take it from the
    // source, and leaving it at 0 changes the result.
    let fps = args_fps();
    let config = if fps > 0.0 {
        config.with_fps(fps)
    } else {
        config
    };

    // CVVDP's display model implies a resolution of its own, and rescaling to
    // it is far cheaper than scoring the source frames at their native
    // size.
    let config = if std::env::var_os("BENCH_RESIZE_TO_DISPLAY").is_some() {
        config.with_resize_to_display(true)
    } else {
        config
    };

    let started = Instant::now();
    let mut scorer = VshipScorer::new(
        config,
        VideoFormat::from_details(&reference_details),
        VideoFormat::from_details(&distorted_details),
        target_resolution,
    )
    .expect("build scorer");
    let setup = started.elapsed();

    // Decoding is part of what is being compared, so it stays inside the timed
    // region by default. FFVship overlaps decode with GPU work, which is the
    // difference the harness is meant to expose, so the two can also be timed
    // separately.
    let decode_only = std::env::var_os("BENCH_DECODE_ONLY").is_some();

    let started = Instant::now();
    let scores = if decode_only {
        let mut frames = 0usize;
        while reference.read_video_frame::<u8>().is_ok()
            && distorted.read_video_frame::<u8>().is_ok()
        {
            frames += 1;
        }
        println!(
            "DECODE_ONLY FRAMES={frames} SECONDS={:.3}",
            started.elapsed().as_secs_f64()
        );
        Vec::new()
    } else {
        scorer
            .score_decoders::<u8>(&mut reference, &mut distorted, |_, _| {})
            .expect("score the clip")
    };
    let elapsed = started.elapsed();

    black_box(&scores);

    let mean = if decode_only {
        f64::NAN
    } else {
        VshipScorer::pool(&scores, metric, PoolMethod::Mean, true).expect("pool")
    };
    let frames = scores.len();
    let fps = frames as f64 / elapsed.as_secs_f64();

    // Per-frame values, so a run can be compared against FFVship's
    // `--live-score-output` line by line.
    if std::env::var_os("BENCH_DUMP_FRAMES").is_some() {
        for (index, score) in scores.iter().enumerate() {
            if let Some(value) = score.value(metric, true) {
                println!("FRAME {index} {value:.6}");
            }
        }
    }

    println!(
        "METRIC={} SECONDS={:.3} SETUP={:.3} FRAMES={} FPS={:.1} MEAN={:.6} \
         TARGET={target_resolution:?} BACKEND={:?} DEVICE={:?}",
        metric_name.to_ascii_uppercase(),
        elapsed.as_secs_f64(),
        setup.as_secs_f64(),
        frames,
        fps,
        mean,
        scorer.backend(),
        device_label(),
    );
}

/// The frame rate to hand libvship, from the environment.
///
/// CVVDP's temporal filter is framerate-dependent, and a rate of 0 is not the
/// same as "unset" there. Set `BENCH_FPS` to the source rate to match what
/// FFVship and the VapourSynth plugin use.
fn args_fps() -> f32 {
    std::env::var("BENCH_FPS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0.0)
}

/// The selected device's name, or `"unknown"` when it cannot be queried.
///
/// libvship reports the name as a fixed-size NUL-padded C array rather than a
/// Rust `String`, so it has to be decoded and trimmed here.
fn device_label() -> String {
    let Ok(info) = VshipScorer::device_info() else {
        return "unknown".to_owned();
    };

    let bytes: Vec<u8> = info
        .name
        .iter()
        .take_while(|&&byte| byte != 0)
        .map(|&byte| byte as u8)
        .collect();

    String::from_utf8_lossy(&bytes).into_owned()
}
