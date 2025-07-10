use std::{fmt::Write, time::Duration};

use indicatif::{
    HumanBytes,
    HumanDuration,
    MultiProgress,
    ProgressBar,
    ProgressDrawTarget,
    ProgressState,
    ProgressStyle,
};
use once_cell::sync::OnceCell;

use crate::{get_done, util::printable_base10_digits, Verbosity};

const PROGRESS_CHARS: &str = if cfg!(windows) {
    "█▓▒░  "
} else {
    "█▉▊▋▌▍▎▏  "
};

const INDICATIF_PROGRESS_TEMPLATE: &str = "{elapsed_precise:.bold} \
                                           {prefix}▐{wide_bar:.blue/white.dim}▌ {percent:.bold} \
                                           {pos} ({fps:.bold}, eta {fixed_eta}{msg})";

const INDICATIF_SC_SPINNER_TEMPLATE: &str =
    "{elapsed_precise:.bold} [{wide_bar:.blue/white.dim}]  {pos} frames ({fps:.bold})";

static PROGRESS_BAR: OnceCell<ProgressBar> = OnceCell::new();
static AUDIO_BYTES: OnceCell<u64> = OnceCell::new();

pub fn set_audio_size(val: u64) {
    AUDIO_BYTES.get_or_init(|| val);
}

pub fn get_audio_size() -> u64 {
    *AUDIO_BYTES.get().unwrap_or(&0u64)
}

fn pretty_progress_style(resume_frames: u64) -> ProgressStyle {
    ProgressStyle::default_bar()
        .template(INDICATIF_PROGRESS_TEMPLATE)
        .unwrap()
        .with_key("fps", move |state: &ProgressState, w: &mut dyn Write| {
            let resume_pos = if state.pos() < resume_frames {
                resume_frames
            } else {
                state.pos() - resume_frames
            };
            if resume_pos == 0 || state.elapsed().as_secs_f32() < f32::EPSILON {
                write!(w, "0 fps").unwrap();
            } else {
                let fps = resume_pos as f32 / state.elapsed().as_secs_f32();
                if fps < 1.0 {
                    write!(w, "{:.2} s/fr", 1.0 / fps).unwrap();
                } else {
                    write!(w, "{fps:.2} fps").unwrap();
                }
            }
        })
        .with_key(
            "fixed_eta",
            move |state: &ProgressState, w: &mut dyn Write| {
                let resume_pos = if state.pos() < resume_frames {
                    resume_frames
                } else {
                    state.pos() - resume_frames
                };
                if resume_pos == 0 || state.elapsed().as_secs_f32() < f32::EPSILON {
                    write!(w, "unknown").unwrap();
                } else {
                    let spf = state.elapsed().as_secs_f32() / resume_pos as f32;
                    let remaining = state.len().unwrap_or(0) - state.pos();
                    write!(
                        w,
                        "{:#}",
                        HumanDuration(Duration::from_secs_f32(spf * remaining as f32))
                    )
                    .unwrap();
                }
            },
        )
        .with_key("pos", |state: &ProgressState, w: &mut dyn Write| {
            write!(w, "{}/{}", state.pos(), state.len().unwrap_or(0)).unwrap();
        })
        .with_key("percent", |state: &ProgressState, w: &mut dyn Write| {
            write!(w, "{:>3.0}%", state.fraction() * 100_f32).unwrap();
        })
        .progress_chars(PROGRESS_CHARS)
}

fn spinner_style(resume_frames: u64) -> ProgressStyle {
    ProgressStyle::default_spinner()
        .template(INDICATIF_SC_SPINNER_TEMPLATE)
        .unwrap()
        .with_key("fps", move |state: &ProgressState, w: &mut dyn Write| {
            let resume_pos = if state.pos() < resume_frames {
                resume_frames
            } else {
                state.pos() - resume_frames
            };
            if resume_pos == 0 || state.elapsed().as_secs_f32() < f32::EPSILON {
                write!(w, "0 fps").unwrap();
            } else {
                let fps = resume_pos as f32 / state.elapsed().as_secs_f32();
                if fps < 1.0 {
                    write!(w, "{:.2} s/fr", 1.0 / fps).unwrap();
                } else {
                    write!(w, "{fps:.2} fps",).unwrap();
                }
            }
        })
        .with_key("pos", |state: &ProgressState, w: &mut dyn Write| {
            write!(w, "{}", state.pos()).unwrap();
        })
        .progress_chars(PROGRESS_CHARS)
}

/// Initialize progress bar
/// Enables steady 100 ms tick
pub fn init_progress_bar(len: u64, resume_frames: u64, chunks: Option<(u32, u32)>) {
    let pb = if len > 0 {
        PROGRESS_BAR
            .get_or_init(|| ProgressBar::new(len).with_style(pretty_progress_style(resume_frames)))
    } else {
        // Avoid showing `xxx/0` if we don't know the length yet.
        // Affects scenechange progress.
        PROGRESS_BAR.get_or_init(|| ProgressBar::new(len).with_style(spinner_style(resume_frames)))
    };
    pb.set_draw_target(ProgressDrawTarget::stderr());
    pb.enable_steady_tick(Duration::from_millis(100));
    pb.reset();
    pb.reset_eta();
    pb.reset_elapsed();
    pb.set_position(0);
    if let Some((done, chunks)) = chunks {
        pb.set_prefix(format!("[{done}/{chunks} Chunks] "));
    }
}

pub fn convert_to_progress(resume_frames: u64) {
    if let Some(pb) = PROGRESS_BAR.get() {
        pb.set_style(pretty_progress_style(resume_frames));
    }
}

pub fn inc_bar(inc: u64) {
    if let Some(pb) = PROGRESS_BAR.get() {
        pb.inc(inc);
    }
}

pub fn dec_bar(dec: u64) {
    if let Some(pb) = PROGRESS_BAR.get() {
        pb.set_position(pb.position().saturating_sub(dec));
    }

    if let Some((_, pbs)) = MULTI_PROGRESS_BAR.get() {
        let pb = pbs.last().unwrap();
        pb.set_position(pb.position().saturating_sub(dec));
    }
}

pub fn update_bar_info(kbps: f64, est_size: HumanBytes, chunks: Option<(u32, u32)>) {
    if let Some(pb) = PROGRESS_BAR.get() {
        pb.set_message(format!(", {kbps:.1} Kbps, est. {est_size}"));
        if let Some((done, chunks)) = chunks {
            pb.set_prefix(format!("[{done}/{chunks} Chunks] "));
        }
    }
}

pub fn set_pos(pos: u64) {
    if let Some(pb) = PROGRESS_BAR.get() {
        pb.set_position(pos);
    }
}

pub fn finish_progress_bar() {
    if let Some(pb) = PROGRESS_BAR.get() {
        pb.finish();
    }

    if let Some((_, pbs)) = MULTI_PROGRESS_BAR.get() {
        for pb in pbs {
            pb.finish();
        }
    }
}

static MULTI_PROGRESS_BAR: OnceCell<(MultiProgress, Vec<ProgressBar>)> = OnceCell::new();

pub fn set_len(len: u64) {
    let pb = PROGRESS_BAR.get().unwrap();
    pb.set_length(len);
}

pub fn reset_bar_at(pos: u64) {
    if let Some(pb) = PROGRESS_BAR.get() {
        pb.reset();
        pb.set_position(pos);
        pb.reset_eta();
        pb.reset_elapsed();
    }
}

pub fn reset_mp_bar_at(pos: u64) {
    if let Some((_, pbs)) = MULTI_PROGRESS_BAR.get() {
        if let Some(pb) = pbs.last() {
            pb.reset();
            pb.set_position(pos);
            pb.reset_eta();
            pb.reset_elapsed();
        }
    }
}

pub fn init_multi_progress_bar(len: u64, workers: usize, resume_frames: u64, chunks: (u32, u32)) {
    MULTI_PROGRESS_BAR.get_or_init(|| {
        let mpb = MultiProgress::new();

        let mut pbs = Vec::new();

        let digits = printable_base10_digits(chunks.1 as usize) as usize;

        for _ in 1..=workers {
            let pb = ProgressBar::hidden().with_style(
                ProgressStyle::default_spinner().template("{prefix:.dim} {msg}").unwrap(),
            );
            pb.set_prefix(format!("[Idle  {digits:digits$}]"));
            pbs.push(mpb.add(pb));
        }

        let pb = ProgressBar::hidden();
        pb.set_style(pretty_progress_style(resume_frames));
        pb.enable_steady_tick(Duration::from_millis(100));
        pb.reset_elapsed();
        pb.reset_eta();
        pb.set_position(0);
        pb.set_length(len);
        pb.reset();
        pb.set_prefix(format!(
            "[{done}/{total} Chunks] ",
            done = chunks.0,
            total = chunks.1
        ));
        pbs.push(mpb.add(pb));

        mpb.set_draw_target(ProgressDrawTarget::stderr());

        (mpb, pbs)
    });
}

pub fn update_mp_chunk(worker_idx: usize, chunk: usize, padding: usize) {
    if let Some((_, pbs)) = MULTI_PROGRESS_BAR.get() {
        pbs[worker_idx].set_prefix(format!("[Chunk {chunk:>padding$}]"));
    }
}

pub fn update_mp_msg(worker_idx: usize, msg: String) {
    if let Some((_, pbs)) = MULTI_PROGRESS_BAR.get() {
        pbs[worker_idx].set_message(msg);
    }
}

pub fn inc_mp_bar(inc: u64) {
    if let Some((_, pbs)) = MULTI_PROGRESS_BAR.get() {
        pbs.last().unwrap().inc(inc);
    }
}

pub fn update_mp_bar_info(kbps: f64, est_size: HumanBytes, chunks: (u32, u32)) {
    if let Some((_, pbs)) = MULTI_PROGRESS_BAR.get() {
        let pb = pbs.last().unwrap();
        pb.set_message(format!(", {kbps:.1} Kbps, est. {est_size}"));
        pb.set_prefix(format!(
            "[{done}/{total} Chunks] ",
            done = chunks.0,
            total = chunks.1
        ));
    }
}

pub fn update_progress_bar_estimates(
    frame_rate: f64,
    total_frames: usize,
    verbosity: Verbosity,
    chunks: (u32, u32),
) {
    let completed_frames: usize =
        get_done().done.iter().map(|ref_multi| ref_multi.value().frames).sum();
    if completed_frames == 0 {
        // avoid division by 0
        return;
    }

    let total_size: u64 = get_done()
        .done
        .iter()
        .map(|ref_multi| ref_multi.value().size_bytes)
        .sum::<u64>();
    let seconds_completed = completed_frames as f64 / frame_rate;
    let kbps = total_size as f64 * 8. / 1000. / seconds_completed;
    let progress = completed_frames as f64 / total_frames as f64;

    let audio_size_byte = get_audio_size();

    let est_size = total_size as f64 / progress + audio_size_byte as f64;
    if verbosity == Verbosity::Normal {
        update_bar_info(kbps, HumanBytes(est_size as u64), Some(chunks));
    } else if verbosity == Verbosity::Verbose {
        update_mp_bar_info(kbps, HumanBytes(est_size as u64), chunks);
    }
}
