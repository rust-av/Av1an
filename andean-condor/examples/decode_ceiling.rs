//! Measures the pure decode ceiling for a probe pass, with no encoder involved.
//!
//! Three strategies are compared over the same frame set:
//!   1. `y4m_frame`   - one frame at a time, as Zone Encoder feeds its encoder
//!   2. `y4m_frames`  - streamed through a bounded channel, as Parallel Encoder
//!      does
//!   3. `frame_sources` - VapourSynth-driven concurrent readers
//!
//! Each strategy discards the frames as fast as it can produce them, so the
//! number reported is the decode limit rather than an encode.
//!
//! Usage: cargo run --release --example decode_ceiling -- <input>
//! <frame_indices...>

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use andean_condor::{
    core::input::Input,
    models::input::{Input as InputModel, VapourSynthImportMethod},
    vapoursynth::vapoursynth_filters::VapourSynthFilter,
};

/// Reports frames per second for one timed run.
struct Timing {
    label: String,
    frames: usize,
    taken: Duration,
}

impl Timing {
    fn fps(&self) -> f64 {
        self.frames as f64 / self.taken.as_secs_f64()
    }
}

fn report(timing: &Timing) {
    println!(
        "DECODE {label:<14} frames={frames} ms={ms:.1} fps={fps:.2}",
        label = timing.label,
        frames = timing.frames,
        ms = timing.taken.as_secs_f64() * 1000.0,
        fps = timing.fps(),
    );
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let Some(input_path) = args.next().map(PathBuf::from) else {
        eprintln!("usage: decode_ceiling <input> <frame_indices...>");
        std::process::exit(2);
    };
    let indices = args
        .map(|value| value.parse::<usize>().expect("frame index is a number"))
        .collect::<Vec<_>>();
    if indices.is_empty() {
        eprintln!("no frame indices given");
        std::process::exit(2);
    }

    let model = InputModel::VapourSynth {
        path: input_path,
        import_method: VapourSynthImportMethod::FFMS2 { index: None },
        cache_path: None,
        filters: vec![VapourSynthFilter::Resize {
            scaler: None,
            width: None,
            height: None,
            format: Some(andean_condor::ffmpeg::FFPixelFormat::YUV420P10LE),
        }],
    };

    // 1. One frame at a time, exactly as Zone Encoder does it.
    let mut input = Input::from_data(&model)?;
    input.clip_info()?;
    let started = Instant::now();
    let mut bytes = 0usize;
    for index in &indices {
        bytes += input.y4m_frame(*index)?.get_ref().len();
    }
    report(&Timing {
        label: "y4m_frame".to_owned(),
        frames: indices.len(),
        taken: started.elapsed(),
    });
    println!("  (produced {bytes} bytes)");

    // 2. Streamed through a bounded channel, as Parallel Encoder does.
    let mut input = Input::from_data(&model)?;
    input.clip_info()?;
    let started = Instant::now();
    let (sender, receiver) = crossbeam_channel::bounded::<std::io::Cursor<Vec<u8>>>(8);
    let consumer = std::thread::spawn(move || {
        let mut count = 0usize;
        while receiver.recv().is_ok() {
            count += 1;
        }
        count
    });
    input.y4m_frames(
        sender,
        &indices,
        &andean_condor::utils::cancellation::Cancellation::default(),
    )?;
    let streamed = consumer.join().expect("consumer thread should join");
    report(&Timing {
        label: "y4m_frames".to_owned(),
        frames: streamed,
        taken: started.elapsed(),
    });

    // 3. Concurrent readers, the fastest path the library offers.
    let mut input = Input::from_data(&model)?;
    input.clip_info()?;
    let started = Instant::now();
    let workers = 8usize;
    let sources = input.frame_sources(workers)?;
    // Split the indices so each worker decodes its own contiguous share, which
    // is how a real pass distributes scenes across workers.
    let per_worker = indices.len().div_ceil(workers);
    let counted = std::thread::scope(|scope| -> usize {
        let mut counted = 0usize;
        // Joined one at a time rather than through a collected handle vector:
        // each worker is spawned before its own join, so the consumer thread
        // always terminates and no handle has to be held.
        for (worker, source) in sources.into_iter().enumerate() {
            let share = indices
                [worker * per_worker..((worker + 1) * per_worker).min(indices.len())]
                .to_vec();
            counted += scope
                .spawn(move || {
                    if share.is_empty() {
                        return 0usize;
                    }
                    let mut reader = source.open().expect("frame source opens");
                    let (sender, receiver) = crossbeam_channel::bounded(8);
                    let consumer = std::thread::spawn(move || {
                        let mut count = 0usize;
                        while receiver.recv().is_ok() {
                            count += 1;
                        }
                        count
                    });
                    reader
                        .y4m_frames(
                            &sender,
                            &share,
                            8,
                            &andean_condor::utils::cancellation::Cancellation::default(),
                        )
                        .expect("frame source reads");
                    // `y4m_frames` borrows the sender, so it has to be dropped
                    // here; otherwise the consumer waits
                    // for a disconnect that never comes.
                    drop(sender);
                    consumer.join().expect("consumer thread should join")
                })
                .join()
                .expect("reader thread should join");
        }
        counted
    });
    report(&Timing {
        label: "sources(8)".to_owned(),
        frames: counted,
        taken: started.elapsed(),
    });

    Ok(())
}
