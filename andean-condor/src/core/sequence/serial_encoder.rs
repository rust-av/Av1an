use std::{
    io::Cursor,
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    sync::{self, Arc, Mutex, atomic::AtomicBool},
    thread,
    time::Instant,
};

use anyhow::{Context, Result, ensure};
use thiserror::Error;

use crate::{
    core::{
        Condor,
        encoder::{EncodeProgress, cancellation_result, pipes::join_worker},
        input::Input,
        sequence::{Sequence, SequenceDetails, SequenceStatus, parallel_encoder::artifact},
    },
    models::{
        encoder::{Encoder, EncoderPasses},
        sequence::{
            SequenceConfigHandler, SequenceDataHandler, scene_detector::SceneDetectorDataHandler,
        },
    },
    utils::cancellation::Cancelled,
};

pub static DETAILS: SequenceDetails = SequenceDetails {
    name: "Serial Encoder",
    description: "Encodes every scene linearly with no threading or parallelism.",
    version: "0.0.1",
};

/// Publishes complete scenes, not cross-call manual-pass statistics. Use
/// [`Encoder::encode_pass_with_stream`] with an explicit
/// [`PassStatistics`](crate::core::encoder::PassStatistics) for caller-managed passes.
pub struct SerialEncoder {
    pub input: Option<Input>,
    pub encoder: Option<Encoder>,
    pub scenes_directory: PathBuf,
    /// Require the decoded output count to match the scene range (default true).
    /// Disabling this never disables successful-status/nonempty-file checks.
    pub validate_frame_count: bool,
}

impl<DataHandler, ConfigHandler> Sequence<DataHandler, ConfigHandler> for SerialEncoder
where
    DataHandler: SequenceDataHandler + SceneDetectorDataHandler,
    ConfigHandler: SequenceConfigHandler,
{
    #[inline]
    fn details(&self) -> SequenceDetails {
        DETAILS
    }

    #[inline]
    fn validate(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
    ) -> Result<((), Vec<anyhow::Error>)> {
        let warnings = vec![];

        let encoder = self.encoder.as_ref().map_or(&condor.encoder, |e| e);
        encoder.validate()?;

        // Ensure all the scene encoders are validated
        for (index, scene) in condor.scenes.iter().enumerate() {
            scene.encoder.validate()?;
            if let EncoderPasses::Specific(current, total) = scene.encoder.passes() {
                ensure!(
                    current == total,
                    "Serial scene {} cannot run non-final Specific pass {current} of {total}; use All({total}) for complete scenes, or Encoder::encode_pass_with_stream with one explicit PassStatistics retained across direct passes",
                    Self::scene_id(index)
                );
            }
            ensure!(
                scene.start_frame < scene.end_frame,
                "Serial scenes require a nonempty increasing frame range"
            );
        }

        artifact::preflight(self.validate_frame_count)?;
        if let Some(input) = &self.input {
            Input::validate(&input.as_data())?;
        }

        // TODO: Validate all scenes output the same codec

        Ok(((), warnings))
    }

    #[inline]
    fn initialize(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        _progress_tx: sync::mpsc::Sender<SequenceStatus>,
    ) -> Result<((), Vec<anyhow::Error>)> {
        let mut warnings = vec![];

        // Ensure scenes is not empty
        if condor.scenes.is_empty() {
            warnings.push(anyhow::Error::new(SerialEncoderError::NoScenes));
        }
        // let encoder = self.encoder.as_ref().map_or(&condor.encoder, |e| e);
        if let Some(input) = &mut self.input {
            // Initialize input by getting clip_info. For VapourSynth inputs,
            // this may begin a lengthy caching process, hence the
            // separation between validate and initialize.
            input.clip_info()?;
        }

        if !self.scenes_directory.exists() {
            std::fs::create_dir_all(&self.scenes_directory)?;
        }

        Ok(((), warnings))
    }

    #[inline]
    fn execute(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        progress_tx: sync::mpsc::Sender<SequenceStatus>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<((), Vec<anyhow::Error>)> {
        self.execute_cancel(condor, progress_tx, cancelled.into())
    }

    #[inline]
    fn execute_cancel(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        _progress_tx: sync::mpsc::Sender<SequenceStatus>,
        cancellation: crate::utils::cancellation::Cancellation,
    ) -> Result<((), Vec<anyhow::Error>)> {
        cancellation.check_forced()?;
        if cancellation.is_cancelled() {
            return Ok(((), Vec::new()));
        }
        let mut warnings = vec![];
        if condor.scenes.is_empty() {
            warnings.push(anyhow::Error::new(SerialEncoderError::NoScenes));
            return Ok(((), warnings));
        }
        // Direct execution has the same pure preflight as the sequence pipeline.
        self.validate(condor)?;
        let input: &mut Input = self.input.as_mut().map_or(&mut condor.input, |i| i);
        let clip_info = input.clip_info()?;
        let total_frames: usize = condor.scenes.iter().map(|s| s.end_frame - s.start_frame).sum();
        let start_time = Instant::now();
        let (progress_tx, progress_rx) = sync::mpsc::channel::<EncodeProgress>();
        let progress_reports = Arc::new(Mutex::new(Vec::new()));

        let progress_thread = thread::Builder::new()
            .name("serial-reports".into())
            .spawn(move || -> Result<()> {
                for progress in progress_rx.iter() {
                    let mut reports = progress_reports.lock().expect("mutex should acquire lock");
                    reports.push(progress);

                    let now = Instant::now();
                    let elapsed = now - start_time;
                    let frames_encoded = reports.len();
                    let fps = frames_encoded as f64 / elapsed.as_secs_f64();
                    // let eta = elapsed * (total_frames as f64 / frames_encoded as
                    // f64) - elapsed;
                    let cpu_usage = {
                        let usage: Vec<f64> = reports
                            .iter()
                            .filter(|p| {
                                p.usage.cpu > 0.0 // Ignore empty reports
                            })
                            .map(|p| p.usage.cpu as f64)
                            .collect();
                        let count = usage.len() as f64;
                        usage.iter().sum::<f64>() / count
                    };
                    let memory_usage = {
                        let usage: Vec<f64> = reports
                            .iter()
                            .filter(|p| {
                                p.usage.memory > 0 // Ignore empty reports
                            })
                            .map(|p| p.usage.memory as f64)
                            .collect();
                        let count = usage.len() as f64;
                        usage.iter().sum::<f64>() / count
                    };

                    println!(
                        "Progress: {}/{} frames, {:.2} fps | CPU: {:.2}%, Memory: {:.2} MB",
                        frames_encoded,
                        total_frames,
                        fps,
                        cpu_usage,
                        memory_usage / 1024. / 1024.
                    );
                    // "Progress: {}/{} frames, {:.2} fps, {:.2} s remaining",
                    // frames_encoded, total_frames, fps, eta.as_secs_f64()
                }

                Ok(())
            })
            .context("Starting serial progress collector")?;

        let scenes_result = serial_catch(|| -> Result<()> {
            for (index, scene) in &mut condor.scenes.iter_mut().enumerate() {
                cancellation.check_forced()?;
                if cancellation.is_cancelled() {
                    break;
                }
                let active = cancellation.active();
                let scene_encoder = scene.encoder.clone();
                let progress_tx_clone = progress_tx.clone();
                let scene_id = SerialEncoder::scene_id(index);
                let extension = scene_encoder.base().output_extension();
                let output = self.scenes_directory.join(format!("{}.{}", scene_id, extension));
                artifact::protect_existing(&output)?;
                let attempt = tempfile::Builder::new()
                    .prefix(".av1an-serial-")
                    .tempdir_in(&self.scenes_directory)
                    .context("Creating private serial scene directory")?;
                // Only this exclusive attempt directory grants FFmpeg scratch
                // overwrite, never the separately protected published scene path.
                let candidate = tempfile::TempPath::try_from_path(
                    attempt.path().join(format!("{scene_id}.{extension}")),
                )?;
                let private_output = candidate.to_path_buf();
                let y4m_header = input.y4m_header(Some(scene.end_frame - scene.start_frame))?;

                let (scene_progress_tx, scene_progress_rx) = sync::mpsc::channel();
                // let (frames_tx, frames_rx) = sync::mpsc::channel();
                let (frames_tx, frames_rx) = crossbeam_channel::unbounded();

                let scene_progress_thread = thread::Builder::new()
                    .name("serial-relay".into())
                    .spawn(move || -> Result<()> {
                        #[cfg(all(test, target_os = "linux"))]
                        tests::internal_integration::worker("relay");
                        relay_progress(scene_progress_rx, progress_tx_clone);
                        Ok(())
                    })
                    .context("Starting serial progress relay")?;

                let encode_cancellation = active.clone();
                let encoder_thread =
                    thread::Builder::new().name("serial-encoder".into()).spawn(move || {
                        #[cfg(all(test, target_os = "linux"))]
                        tests::internal_integration::worker("encoder");
                        scene_encoder.encode_owned_with_stream(
                            frames_rx,
                            &private_output,
                            scene_progress_tx,
                            &clip_info,
                            &encode_cancellation,
                        )
                    });
                let encoder_thread = match encoder_thread {
                    Ok(worker) => worker,
                    Err(error) => {
                        let relayed = join_worker(
                            scene_progress_thread,
                            "Joining serial relay after encoder setup failure",
                        );
                        return serial_results([
                            Err(error).context("Starting serial encoder worker"),
                            relayed,
                        ]);
                    },
                };

                let feed = serial_catch(|| -> Result<()> {
                    #[cfg(all(test, target_os = "linux"))]
                    tests::internal_integration::worker("feed");
                    frames_tx.send(Cursor::new(Vec::from(y4m_header.as_bytes())))?;
                    let frame_indices = (scene.start_frame..scene.end_frame).collect::<Vec<_>>();
                    input.y4m_frames(frames_tx, &frame_indices, &active)
                });
                let encoded = join_worker(encoder_thread, "Joining serial scene encoder");
                let relayed =
                    join_worker(scene_progress_thread, "Joining serial scene progress relay");
                // Collect every join before propagating any error or removing scratch.
                let mut diagnostic = None;
                let encoded = encoded.and_then(|result| {
                    if result.status.success() {
                        diagnostic = Some(result);
                        Ok(())
                    } else {
                        Err(result.into())
                    }
                });
                let completed = (|| -> Result<()> {
                    cancellation_result(serial_results([encoded, feed, relayed]), &active)?;
                    artifact::validate(
                        &candidate,
                        scene.end_frame - scene.start_frame,
                        self.validate_frame_count,
                        &active,
                    )?;
                    // persist_noclobber is atomic and cannot replace even a target
                    // created during encoding; publish linearizes this commit with Force.
                    active.publish(|| {
                        candidate
                            .persist_noclobber(&output)
                            .map_err(anyhow::Error::from)
                            .with_context(|| {
                                format!("Publishing serial scene {}", output.display())
                            })
                    })?;
                    Ok(())
                })();
                // A later feed/probe/commit failure must not erase the native
                // status/bytes. Typed context preserves the original cause too.
                completed.map_err(|error| match diagnostic {
                    Some(result) => {
                        let headline =
                            format!("Serial scene {scene_id} failed: {}", error.root_cause());
                        error.context(result).context(headline)
                    },
                    None => error,
                })?;
            }
            Ok(())
        });

        // Reports are observations, not one-per-frame; only channel EOF ends collection.
        drop(progress_tx);
        let reported = join_worker(progress_thread, "Joining serial progress report worker");
        let result = serial_results([scenes_result, reported]);
        // Keep an already-typed Forced error's scene headline; a fresh force
        // after successful publication still reports cancellation without rollback.
        if result
            .as_ref()
            .is_err_and(|error| error.downcast_ref::<Cancelled>() == Some(&Cancelled::Forced))
        {
            result?;
        } else {
            cancellation_result(result, &cancellation)?;
        }

        Ok(((), warnings))
    }
}

fn relay_progress(
    source: sync::mpsc::Receiver<EncodeProgress>,
    destination: sync::mpsc::Sender<EncodeProgress>,
) {
    let mut destination = Some(destination);
    for progress in source {
        if let Some(sender) = &destination
            && sender.send(progress).is_err()
        {
            destination = None;
        }
    }
}

fn serial_results(results: impl IntoIterator<Item = Result<()>>) -> Result<()> {
    let mut error: Option<anyhow::Error> = None;
    for result in results {
        if let Err(next) = result {
            error = Some(match error {
                Some(previous) => previous.context(format!("Additional serial failure: {next:#}")),
                None => next,
            });
        }
    }
    error.map_or(Ok(()), Err)
}

fn serial_catch<T>(work: impl FnOnce() -> Result<T>) -> Result<T> {
    catch_unwind(AssertUnwindSafe(work)).unwrap_or_else(|panic| {
        let message = panic
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
            .unwrap_or("non-string panic");
        Err(crate::utils::CaughtPanic {
            operation: "Serial scene worker".into(),
            message: message.into(),
        }
        .into())
    })
}

impl SerialEncoder {
    pub const DETAILS: SequenceDetails = DETAILS;

    #[inline]
    pub fn new(scenes_directory: &Path) -> Self {
        SerialEncoder {
            input: None,
            encoder: None,
            scenes_directory: scenes_directory.to_path_buf(),
            validate_frame_count: true,
        }
    }

    #[inline]
    pub fn scene_id(index: usize) -> String {
        format!("{:>05}", index)
    }
}

#[derive(Debug, Clone, Copy, Error)]
pub enum SerialEncoderError {
    #[error("No scenes found")]
    NoScenes,
}

#[cfg(test)]
mod tests;
