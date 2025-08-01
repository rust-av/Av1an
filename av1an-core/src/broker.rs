use std::{
    fmt::{Debug, Display},
    fs::File,
    io::Write,
    path::Path,
    process::ExitStatus,
    sync::{
        atomic::{AtomicU8, Ordering},
        mpsc::Sender,
        Arc,
    },
    thread::available_parallelism,
};

use anyhow::bail;
use cfg_if::cfg_if;
use smallvec::SmallVec;
use thiserror::Error;
use tracing::{debug, error, warn};

use crate::{
    context::Av1anContext,
    finish_progress_bar,
    get_done,
    progress_bar::{
        dec_bar,
        inc_mp_bar,
        update_mp_chunk,
        update_mp_msg,
        update_progress_bar_estimates,
    },
    util::printable_base10_digits,
    Chunk,
    DoneChunk,
    Instant,
};

#[derive(Debug)]
pub struct Broker<'a> {
    pub chunk_queue: Vec<Chunk>,
    pub project:     &'a Av1anContext,
}

#[derive(Clone)]
pub enum StringOrBytes {
    String(String),
    Bytes(Vec<u8>),
}

impl Debug for StringOrBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::String(s) => {
                if f.alternate() {
                    f.write_str(&textwrap::indent(s, "        "))?; // 8 spaces
                } else {
                    f.write_str(s)?;
                }
            },
            Self::Bytes(b) => write!(f, "raw bytes: {b:?}")?,
        }

        Ok(())
    }
}

impl From<Vec<u8>> for StringOrBytes {
    fn from(bytes: Vec<u8>) -> Self {
        #[expect(
            clippy::option_if_let_else,
            reason = "https://github.com/rust-lang/rust-clippy/issues/15142"
        )]
        if let Ok(res) = simdutf8::basic::from_utf8(&bytes) {
            Self::String(res.to_string())
        } else {
            Self::Bytes(bytes)
        }
    }
}

impl From<String> for StringOrBytes {
    fn from(s: String) -> Self {
        Self::String(s)
    }
}

#[derive(Error, Debug)]
pub struct EncoderCrash {
    pub exit_status:        ExitStatus,
    pub stdout:             StringOrBytes,
    pub stderr:             StringOrBytes,
    pub source_pipe_stderr: StringOrBytes,
    pub ffmpeg_pipe_stderr: Option<StringOrBytes>,
}

impl Display for EncoderCrash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "encoder crashed: {}\nstdout:\n{:#?}\nstderr:\n{:#?}\nsource pipe stderr:\n{:#?}",
            self.exit_status, self.stdout, self.stderr, self.source_pipe_stderr,
        )?;

        if let Some(ffmpeg_pipe_stderr) = &self.ffmpeg_pipe_stderr {
            write!(f, "\nffmpeg pipe stderr:\n{ffmpeg_pipe_stderr:#?}")?;
        }

        Ok(())
    }
}

impl Broker<'_> {
    /// Main encoding loop. set_thread_affinity may be ignored if the value is
    /// invalid.
    #[tracing::instrument(skip(self))]
    pub fn encoding_loop(
        self,
        tx: Sender<()>,
        set_thread_affinity: Option<usize>,
        total_chunks: u32,
    ) -> anyhow::Result<()> {
        if !self.chunk_queue.is_empty() {
            let (sender, receiver) = crossbeam_channel::bounded(self.chunk_queue.len());

            for chunk in &self.chunk_queue {
                sender.send(chunk.clone())?;
            }
            drop(sender);

            crossbeam_utils::thread::scope(|s| {
                let terminations_requested = Arc::new(AtomicU8::new(0));
                let terminations_requested_clone = Arc::clone(&terminations_requested);
                ctrlc::set_handler(move || {
                    let count = terminations_requested_clone.fetch_add(1, Ordering::SeqCst) + 1;
                    if count == 1 {
                        error!("Shutting down. Waiting for current workers to finish...");
                    } else {
                        error!("Shutting down all workers...");
                    }
                })
                .expect("should set ctrlc handler");

                let consumers: Vec<_> = (0..self.project.args.workers)
                    .map(|idx| (receiver.clone(), &self, idx, Arc::clone(&terminations_requested)))
                    .map(|(rx, queue, worker_id, terminations_requested)| {
                        let tx = tx.clone();
                        s.spawn(move |_| {
                            cfg_if! {
                                if #[cfg(any(target_os = "linux", target_os = "windows"))] {
                                    if let Some(threads) = set_thread_affinity {
                                        if threads == 0 {
                                            warn!("Ignoring set_thread_affinity: Requested 0 threads");
                                        } else {
                                            match available_parallelism() {
                                                Ok(parallelism) => {
                                                    let available_threads = parallelism.get();
                                                    let mut cpu_set = SmallVec::<[usize; 16]>::new();
                                                    let start_thread = (threads * worker_id) % available_threads;
                                                    cpu_set.extend((start_thread..start_thread + threads).map(|t| t % available_threads));
                                                    if let Err(e) = affinity::set_thread_affinity(&cpu_set) {
                                                        warn!("Failed to set thread affinity for worker {worker_id}: {e}");
                                                    }
                                                },
                                                Err(e) => {
                                                    warn!("Failed to get thread count: {e}. Thread affinity will not be set");
                                                }
                                            }
                                        }
                                    }
                                }
                            }

                            while let Ok(mut chunk) = rx.recv() {
                                if terminations_requested.load(Ordering::SeqCst) == 0 {
                                    if let Err(e) = queue.encode_chunk(&mut chunk, worker_id, &terminations_requested, total_chunks) {
                                            error!("[chunk {index}] {e}", index = chunk.index);
                                        tx.send(()).expect("should send successfully");
                                        return Err(());
                                    }
                                }
                            }
                            Ok(())
                        })
                    })
                    .collect();
                for consumer in consumers {
                    consumer.join().expect("consumer should join successfully").ok();
                }

                if terminations_requested.load(Ordering::SeqCst) > 0 {
                    tx.send(()).expect("should send successfully");
                }
            })
            .expect("thread should spawn successfully");

            finish_progress_bar();
        }

        Ok(())
    }

    #[tracing::instrument(skip(self, chunk, terminations_requested), fields(chunk_index = format!("{:>05}", chunk.index)))]
    fn encode_chunk(
        &self,
        chunk: &mut Chunk,
        worker_id: usize,
        terminations_requested: &Arc<AtomicU8>,
        total_chunks: u32,
    ) -> anyhow::Result<()> {
        let st_time = Instant::now();

        // we display the index, so we need to subtract 1 to get the max index
        let padding = printable_base10_digits(self.chunk_queue.len() - 1) as usize;
        update_mp_chunk(worker_id, chunk.index, padding);

        if let Some((min, max)) = chunk.target_quality.target {
            update_mp_msg(
                worker_id,
                format!(
                    "Targeting {metric} Quality: {min}-{max}",
                    metric = chunk.target_quality.metric,
                    min = min,
                    max = max
                ),
            );
            for r#try in 1..=self.project.args.max_tries {
                let res = chunk.target_quality.per_shot_target_quality(
                    chunk,
                    Some(worker_id),
                    self.project.args.vapoursynth_plugins,
                );
                match res {
                    Ok(cq) => {
                        chunk.tq_cq = Some(cq);
                        break;
                    },
                    Err(e) => {
                        if r#try >= self.project.args.max_tries {
                            bail!(
                                "Target Quality failed after {} tries on chunk {}:\n{}",
                                r#try,
                                chunk.index,
                                e
                            );
                        }
                    },
                }
            }

            if chunk.target_quality.params_copied
                && chunk.tq_cq.is_some()
                && chunk.target_quality.probing_rate == 1
                && self.project.args.ffmpeg_filter_args.is_empty()
                && chunk.proxy.is_none()
            {
                let optimal_q = chunk.tq_cq.expect("tq_cq is some");
                let extension = match self.project.args.encoder {
                    crate::encoder::Encoder::x264 => "264",
                    crate::encoder::Encoder::x265 => "hevc",
                    _ => "ivf",
                };
                let probe_file = std::path::Path::new(&self.project.args.temp).join("split").join(
                    format!("v_{index:05}_{optimal_q}.{extension}", index = chunk.index),
                );

                if probe_file.exists() {
                    let encode_dir = std::path::Path::new(&self.project.args.temp).join("encode");
                    std::fs::create_dir_all(&encode_dir)?;
                    let output_file =
                        encode_dir.join(format!("{index:05}.{extension}", index = chunk.index));
                    std::fs::copy(&probe_file, &output_file)?;

                    inc_mp_bar(chunk.frames() as u64);

                    let progress_file = Path::new(&self.project.args.temp).join("done.json");
                    get_done().done.insert(chunk.name(), DoneChunk {
                        frames:     chunk.frames(),
                        size_bytes: output_file.metadata()?.len(),
                    });

                    let mut progress_file = File::create(progress_file)?;
                    progress_file.write_all(serde_json::to_string(get_done())?.as_bytes())?;

                    update_progress_bar_estimates(
                        chunk.frame_rate,
                        self.project.frames,
                        self.project.args.verbosity,
                        (get_done().done.len() as u32, total_chunks),
                    );

                    return Ok(());
                }
            }
        }

        if terminations_requested.load(Ordering::SeqCst) > 0 {
            bail!(
                "Termination requested after Target Quality. Skipping chunk {}",
                chunk.index
            );
        }

        // space padding at the beginning to align with "finished chunk"
        debug!(
            " started chunk {index:05}: {frames} frames",
            index = chunk.index,
            frames = chunk.frames()
        );

        let passes = chunk.passes;
        for current_pass in 1..=passes {
            for r#try in 1..=self.project.args.max_tries {
                let res = self.project.create_pipes(chunk, current_pass, worker_id, padding);
                if let Err((e, frames)) = res {
                    dec_bar(frames);

                    // If user presses CTRL+C more than once, do not let the worker finish
                    if terminations_requested.load(Ordering::SeqCst) > 1 {
                        bail!(
                            "Termination requested after Worker restart. Skipping chunk {}",
                            chunk.index
                        );
                    }

                    if r#try == self.project.args.max_tries {
                        bail!(
                            "[chunk {index}] encoder failed {tries} times, shutting down worker: \
                             {e}",
                            index = chunk.index,
                            tries = self.project.args.max_tries
                        );
                    }
                    // avoids double-print of the error message as both a WARN and ERROR,
                    // since `Broker::encoding_loop` will print the error message as well
                    warn!(
                        "Encoder failed (on chunk {index}):\n{e}",
                        index = chunk.index
                    );
                } else {
                    break;
                }
            }
        }

        let enc_time = st_time.elapsed();
        let fps = chunk.frames() as f64 / enc_time.as_secs_f64();

        let progress_file = Path::new(&self.project.args.temp).join("done.json");
        get_done().done.insert(chunk.name(), DoneChunk {
            frames:     chunk.frames(),
            size_bytes: Path::new(&chunk.output())
                .metadata()
                .expect("Unable to get size of finished chunk")
                .len(),
        });

        let mut progress_file = File::create(progress_file)?;
        progress_file.write_all(serde_json::to_string(get_done())?.as_bytes())?;

        update_progress_bar_estimates(
            chunk.frame_rate,
            self.project.frames,
            self.project.args.verbosity,
            (get_done().done.len() as u32, total_chunks),
        );

        debug!(
            "finished chunk {index:05}: {frames} frames, {fps:.2} fps, took {enc_time:.2?}",
            index = chunk.index,
            frames = chunk.frames()
        );

        Ok(())
    }
}
