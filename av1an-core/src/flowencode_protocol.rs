use std::io::{self, Write};

use num_traits::cast::ToPrimitive;
use serde::Serialize;

use crate::{Chunk, ClipInfo, EncodeArgs, Encoder, TargetMetric};

pub const FLOWENCODE_PROTOCOL_VERSION: i32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressFormat {
    Human,
    Jsonl,
}

#[derive(Serialize)]
pub struct Capabilities<'a> {
    pub protocol: i32,
    pub backend_version: &'a str,
    pub supported_metrics: &'a [&'a str],
    pub supported_encoders: &'a [&'a str],
    pub interp_methods: &'a [&'a str],
    pub probing_stats: &'a [&'a str],
}

#[derive(Serialize)]
struct Event<'a, T: Serialize> {
    #[serde(rename = "type")]
    event_type: &'a str,
    ts: String,
    #[serde(flatten)]
    payload: T,
}

#[derive(Serialize)]
struct RunStarted<'a> {
    protocol: i32,
    backend_version: &'a str,
    encoder: &'a str,
    target_metric: &'a str,
    target_quality: Option<(f64, f64)>,
}

#[derive(Serialize)]
struct InputProbed<'a> {
    input_kind: &'a str,
    width: u32,
    height: u32,
    total_frames: usize,
    fps: String,
}

#[derive(Serialize)]
struct ChunkPlan {
    chunk_count: u32,
}

#[derive(Serialize)]
struct EncodeProgress {
    fraction_done: u32,
    fraction_total: u32,
    chunk_index: Option<usize>,
}

#[derive(Serialize)]
struct RunFinished {
    exit_code: i32,
}

#[derive(Serialize)]
struct RunFailed<'a> {
    exit_code: i32,
    message: &'a str,
}

pub fn capabilities() -> Capabilities<'static> {
    Capabilities {
        protocol: FLOWENCODE_PROTOCOL_VERSION,
        backend_version: env!("CARGO_PKG_VERSION"),
        supported_metrics: &[
            "vmaf",
            "ssimulacra2",
            "butteraugli-inf",
            "butteraugli-3",
            "xpsnr",
            "xpsnr-weighted",
        ],
        supported_encoders: &["x264", "x265", "svt-av1"],
        interp_methods: &[
            "linear",
            "quadratic",
            "natural",
            "pchip",
            "catmull",
            "akima",
            "cubic-polynomial",
        ],
        probing_stats: &[
            "auto",
            "mean",
            "median",
            "harmonic",
            "percentile",
            "standard-deviation",
            "mode",
            "minimum",
            "maximum",
            "root-mean-square",
        ],
    }
}

fn emit<T: Serialize>(format: ProgressFormat, event_type: &'static str, payload: T) {
    if format != ProgressFormat::Jsonl {
        return;
    }

    let event = Event {
        event_type,
        ts: chrono::Utc::now().to_rfc3339(),
        payload,
    };

    if let Ok(line) = serde_json::to_string(&event) {
        let mut stdout = io::stdout().lock();
        let _ = stdout.write_all(line.as_bytes());
        let _ = stdout.write_all(b"\n");
        let _ = stdout.flush();
    }
}

fn encoder_name(encoder: Encoder) -> &'static str {
    match encoder {
        Encoder::x264 => "x264",
        Encoder::x265 => "x265",
        Encoder::svt_av1 => "svt-av1",
        Encoder::aom => "aom",
        Encoder::rav1e => "rav1e",
        Encoder::vpx => "vpx",
    }
}

fn metric_name(metric: TargetMetric) -> &'static str {
    match metric {
        TargetMetric::VMAF => "vmaf",
        TargetMetric::SSIMULACRA2 => "ssimulacra2",
        TargetMetric::ButteraugliINF => "butteraugli-inf",
        TargetMetric::Butteraugli3 => "butteraugli-3",
        TargetMetric::XPSNR => "xpsnr",
        TargetMetric::XPSNRWeighted => "xpsnr-weighted",
    }
}

pub fn emit_run_started(args: &EncodeArgs) {
    emit(
        args.progress_format,
        "run_started",
        RunStarted {
            protocol: FLOWENCODE_PROTOCOL_VERSION,
            backend_version: env!("CARGO_PKG_VERSION"),
            encoder: encoder_name(args.encoder),
            target_metric: metric_name(args.target_quality.metric),
            target_quality: args.target_quality.target,
        },
    );
}

pub fn emit_input_probed(args: &EncodeArgs, clip_info: &ClipInfo) {
    emit(
        args.progress_format,
        "input_probed",
        InputProbed {
            input_kind: if args.input.is_vapoursynth() {
                "vapoursynth"
            } else {
                "video"
            },
            width: clip_info.resolution.0,
            height: clip_info.resolution.1,
            total_frames: clip_info.num_frames,
            fps: format!("{:.3}", clip_info.frame_rate.to_f64().unwrap_or_default()),
        },
    );
}

pub fn emit_chunk_plan(progress_format: ProgressFormat, chunk_count: usize) {
    emit(
        progress_format,
        "chunk_plan",
        ChunkPlan {
            chunk_count: chunk_count as u32,
        },
    );
}

pub fn emit_chunk_progress(
    progress_format: ProgressFormat,
    completed_chunks: usize,
    total_chunks: usize,
    chunk: Option<&Chunk>,
) {
    emit(
        progress_format,
        "encode_progress",
        EncodeProgress {
            fraction_done: completed_chunks as u32,
            fraction_total: total_chunks as u32,
            chunk_index: chunk.map(|value| value.index),
        },
    );
}

pub fn emit_run_completed(progress_format: ProgressFormat) {
    emit(progress_format, "run_completed", RunFinished { exit_code: 0 });
}

pub fn emit_run_failed(progress_format: ProgressFormat, exit_code: i32, message: &str) {
    emit(
        progress_format,
        "run_failed",
        RunFailed {
            exit_code,
            message,
        },
    );
}
