//! Measures where fmetrics spends its time, and how it scales across threads.
//!
//! Three numbers, because one number cannot answer the question a caller has:
//!
//! - **end-to-end** — decode and score a clip pair the way the scorer is
//!   actually driven. Comparable against another engine's end-to-end figure.
//! - **library-only** — score pre-converted frames, so decode and conversion
//!   are out of the timed region. This is what says whether the library itself
//!   parallelises; end-to-end cannot, because decode does not.
//! - **conversion-only** — the YUV to RGB conversion on its own. Roughly half
//!   of fmetrics' per-frame cost, so it cannot be inferred from the other two.
//!
//! ```sh
//! # Everything, on one thread and across several.
//! cargo run --release -p av-metrics-fmetrics --example bench -- ref.mkv dist.mkv
//!
//! # Through 12 threads, stopping after 200 frames per thread.
//! cargo run --release -p av-metrics-fmetrics --example bench -- ref.mkv dist.mkv 12 200
//! ```
//!
//! Scores are printed alongside the timings throughout, because a speedup that
//! changes the numbers is not a speedup: a shared workspace corrupts fmetrics'
//! scratch silently rather than failing.

use std::{sync::Arc, time::Instant};

use av_decoders::Decoder;
use av_metrics_fmetrics::{
    ColorInfo, FmetricsConfig, FmetricsMetric, FmetricsScorer, PlaneSet, RgbView, is_available,
    yuv_to_rgb,
};

/// One decoded pair, converted once up front so the timed region allocates
/// nothing.
struct Prepared {
    reference: Vec<u8>,
    distorted: Vec<u8>,
    width: usize,
    height: usize,
}

/// The error type every fallible step here returns.
///
/// Concrete rather than boxed, because a boxed `dyn Error` is not `Send` and
/// these closures run on worker threads.
type Error = Box<dyn std::error::Error + Send + Sync>;

impl Prepared {
    /// Score one pair, doing no work beyond the library call.
    fn score(&self, scorer: &FmetricsScorer) -> Result<f64, Error> {
        let reference = RgbView::new(&self.reference, self.width, self.height, false)?;
        let distorted = RgbView::new(&self.distorted, self.width, self.height, false)?;
        Ok(scorer
            .submit_rgb(&reference, &distorted)?
            .require(FmetricsMetric::Ssimulacra2)?)
    }
}

/// Decode and score `reference_path` against `distorted_path`, timing the two
/// separately.
fn end_to_end(reference_path: &str, distorted_path: &str, threads: u32) -> Result<(), Error> {
    let mut reference = Decoder::from_file(reference_path)?;
    let mut distorted = Decoder::from_file(distorted_path)?;
    let details = *reference.get_video_details();
    let color = ColorInfo::from_details(&details)?;
    let distorted_color = ColorInfo::from_details(distorted.get_video_details())?;
    let scorer = FmetricsScorer::new(
        FmetricsConfig::new(FmetricsMetric::Ssimulacra2).with_threads(threads),
        color,
        distorted_color,
    )?;

    let mut decode_time = std::time::Duration::ZERO;
    let mut score_time = std::time::Duration::ZERO;
    let mut scores = Vec::new();

    loop {
        let started = Instant::now();
        let reference_frame = reference.read_video_frame::<u8>();
        let distorted_frame = distorted.read_video_frame::<u8>();
        let (Ok(reference_frame), Ok(distorted_frame)) = (reference_frame, distorted_frame) else {
            // `av-decoders` signals end of stream through its error type.
            break;
        };
        decode_time += started.elapsed();

        let started = Instant::now();
        let reference_planes = PlaneSet::from_frame(&reference_frame)?;
        let distorted_planes = PlaneSet::from_frame(&distorted_frame)?;
        let score = scorer.submit_pair(&reference_planes, &distorted_planes)?;
        score_time += started.elapsed();

        scores.push(score.require(FmetricsMetric::Ssimulacra2)?);
    }

    let frames = scores.len();
    if frames == 0 {
        return Err("no frames were decoded".into());
    }
    let n = frames as f64;

    println!("## end-to-end (decode + convert + score), threads={threads}");
    println!("  {} frames", frames);
    println!(
        "  decode_ms_per_frame={:.1} score_ms_per_frame={:.1} total_ms_per_frame={:.1}",
        decode_time.as_secs_f64() / n * 1000.0,
        score_time.as_secs_f64() / n * 1000.0,
        (decode_time + score_time).as_secs_f64() / n * 1000.0,
    );
    println!(
        "  first={:.4} mean={:.4} last={:.4}",
        scores[0],
        scores.iter().sum::<f64>() / n,
        scores[frames - 1],
    );

    Ok(())
}

/// Time the conversion alone, on one pair at a time.
///
/// One pair at a time because holding many decoded frames alive is a different
/// measurement, and a much larger footprint than the scorer ever uses.
fn conversion_only(reference_path: &str, distorted_path: &str) -> Result<(), Error> {
    let mut reference = Decoder::from_file(reference_path)?;
    let mut distorted = Decoder::from_file(distorted_path)?;
    let details = *reference.get_video_details();
    let color = ColorInfo::from_details(&details)?;
    let peak = color.max_sample()?;
    let distorted_color = ColorInfo::from_details(distorted.get_video_details())?;
    let distorted_peak = distorted_color.max_sample()?;

    let mut sink = 0u64;
    let mut pairs = 0usize;
    let started = Instant::now();

    loop {
        let (Ok(a), Ok(b)) = (
            reference.read_video_frame::<u8>(),
            distorted.read_video_frame::<u8>(),
        ) else {
            break;
        };
        let (Ok(a), Ok(b)) = (PlaneSet::from_frame(&a), PlaneSet::from_frame(&b)) else {
            break;
        };

        let x = yuv_to_rgb(&a, &color, peak)?;
        let y = yuv_to_rgb(&b, &distorted_color, distorted_peak)?;
        for sample in x.as_bytes().iter().chain(y.as_bytes().iter()) {
            sink = sink.wrapping_mul(31).wrapping_add(u64::from(*sample));
        }
        pairs += 1;
    }

    if pairs == 0 {
        return Err("no frames were decoded".into());
    }

    println!("## conversion only (both sides of a pair)");
    println!("  pairs={pairs}");
    println!(
        "  ms_per_frame_pair={:.2}",
        started.elapsed().as_secs_f64() / pairs as f64 * 1000.0,
    );
    // The checksum keeps the conversion from being optimised away and changes
    // if the conversion's output does.
    println!("  checksum={sink}");

    Ok(())
}

/// Score pre-converted frames from `threads` threads, reporting throughput.
///
/// Every thread scores every frame, so each does the same total work as a
/// serial run and wall time cannot fall below it however well the library
/// scales. What parallelism buys is more work per second, so `serial /
/// parallel` would be capped at 1.0 by construction and would hide the effect
/// being measured.
fn library_only(
    reference_path: &str,
    distorted_path: &str,
    threads: usize,
    wanted: usize,
) -> Result<(), Error> {
    let color;
    let distorted_color;
    let peak;
    let prepared;
    {
        let mut reference = Decoder::from_file(reference_path)?;
        let mut distorted = Decoder::from_file(distorted_path)?;
        let details = *reference.get_video_details();
        color = ColorInfo::from_details(&details)?;
        peak = color.max_sample()?;
        distorted_color = ColorInfo::from_details(distorted.get_video_details())?;
        let distorted_peak = distorted_color.max_sample()?;

        // Deliberately outside every timed region below: this is our cost, not
        // the library's.
        let (width, height) = (details.width, details.height);
        let mut frames = Vec::with_capacity(wanted);
        for _ in 0..wanted {
            let (Ok(a), Ok(b)) = (
                reference.read_video_frame::<u8>(),
                distorted.read_video_frame::<u8>(),
            ) else {
                break;
            };
            let (Ok(a), Ok(b)) = (PlaneSet::from_frame(&a), PlaneSet::from_frame(&b)) else {
                break;
            };
            frames.push(Prepared {
                reference: yuv_to_rgb(&a, &color, peak)?.as_bytes().to_vec(),
                distorted: yuv_to_rgb(&b, &distorted_color, distorted_peak)?.as_bytes().to_vec(),
                width,
                height,
            });
        }
        prepared = Arc::new(frames);
    }

    let frames = prepared.len();
    if frames == 0 {
        return Err("no frames decoded".into());
    }

    let scorer = Arc::new(FmetricsScorer::new(
        FmetricsConfig::new(FmetricsMetric::Ssimulacra2).with_threads(threads as u32),
        color,
        distorted_color,
    )?);

    // A serial pass first: both the reference score and the wall time the
    // parallel runs are compared against.
    let started = Instant::now();
    let serial: Vec<f64> =
        prepared.iter().map(|frame| frame.score(&scorer)).collect::<Result<_, _>>()?;
    let serial_wall = started.elapsed().as_secs_f64();

    // Per-call latency is recorded alongside wall time: unchanged latency with
    // falling throughput means the calls are queueing, while latency rising
    // with thread count means they are contending for a shared resource.
    let (parallel, identical, worst_call_ms) = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                let scorer = Arc::clone(&scorer);
                let prepared = Arc::clone(&prepared);
                scope.spawn(move || {
                    let started = Instant::now();
                    let mut scores = Vec::with_capacity(prepared.len());
                    let mut worst = 0.0f64;
                    for frame in prepared.iter() {
                        let call = Instant::now();
                        scores.push(frame.score(&scorer)?);
                        worst = worst.max(call.elapsed().as_secs_f64() * 1000.0);
                    }
                    Ok::<_, Error>((started.elapsed().as_secs_f64(), scores, worst))
                })
            })
            .collect();

        let mut slowest = 0.0f64;
        let mut all_match = true;
        let mut worst_latency = 0.0f64;
        for handle in handles {
            let (elapsed, scores, worst) = handle.join().expect("worker")?;
            slowest = slowest.max(elapsed);
            // Bit-identical, not approximately equal: a difference would mean
            // the library's results depend on which workspace ran
            // them.
            all_match &= scores.iter().zip(&serial).all(|(a, b)| a.to_bits() == b.to_bits());
            worst_latency = worst_latency.max(worst);
        }
        Ok::<_, Error>((slowest, all_match, worst_latency))
    })?;

    let n = frames as f64;
    let throughput_speedup = ((n * threads as f64) / parallel) / (n / serial_wall);

    println!("## library only (no decode, no conversion), threads={threads}");
    println!("  frames_per_thread={frames} identical_to_serial={identical}");
    println!(
        "  serial_ms_per_frame={:.1} worst_call_ms={worst_call_ms:.1}",
        serial_wall / n * 1000.0,
    );
    println!(
        "  throughput_speedup={throughput_speedup:.2}x scale_efficiency={:.0}%",
        100.0 * throughput_speedup / threads as f64,
    );

    Ok(())
}

fn main() -> Result<(), Error> {
    let mut args = std::env::args().skip(1);
    let usage = "usage: bench <reference> <distorted> [threads] [frames]";
    let reference_path = args.next().ok_or(usage)?;
    let distorted_path = args.next().ok_or(usage)?;
    let threads: usize = args
        .next()
        .map_or(Ok(1), |value| value.parse())
        .map_err(|_| "threads must be a number")?;
    let frames: usize = args
        .next()
        .map_or(Ok(96), |value| value.parse())
        .map_err(|_| "frames must be a number")?;

    if !is_available() {
        eprintln!("fmetrics is not available; set FMETRICS_LIB_PATH or install it");
        std::process::exit(2);
    }

    println!(
        "fmetrics {}, {} cores",
        av_metrics_fmetrics::fmetrics_version().unwrap_or_else(|| "unknown".to_owned()),
        std::thread::available_parallelism().map_or(0, |n| n.get()),
    );

    // End-to-end first: if it fails, there is no point reporting finer numbers
    // for a run that did not happen.
    end_to_end(&reference_path, &distorted_path, threads as u32)?;
    println!();
    conversion_only(&reference_path, &distorted_path)?;
    println!();
    library_only(&reference_path, &distorted_path, threads, frames)?;

    Ok(())
}
