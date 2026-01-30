use std::{
    error::Error,
    io::Cursor,
    path::{Path, PathBuf},
    sync::{self, atomic::AtomicBool, Arc, Mutex},
    thread,
    time::Instant,
};

use anyhow::Result;
use thiserror::Error;

use crate::{
    core::{
        encoder::EncodeProgress,
        input::Input,
        sequence::{Sequence, SequenceDetails, SequenceStatus},
        Condor,
    },
    models::{
        encoder::Encoder,
        sequence::{
            scene_detector::SceneDetectorDataHandler,
            SequenceConfigHandler,
            SequenceDataHandler,
        },
    },
};

static DETAILS: SequenceDetails = SequenceDetails {
    name:        "Serial Encoder",
    description: "Encodes every scene linearly with no threading or parallelism.",
    version:     "0.0.1",
};

pub struct SerialEncoder {
    pub input:            Option<Input>,
    pub encoder:          Option<Encoder>,
    pub scenes_directory: PathBuf,
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
    ) -> Result<((), Vec<Box<dyn Error>>)> {
        let warnings: Vec<Box<dyn Error>> = vec![];

        let encoder = self.encoder.as_ref().map_or(&condor.encoder, |e| e);
        if let Some(input) = &self.input {
            Input::validate(&input.as_data())?;
        }

        encoder.validate()?;

        // Ensure all the scene encoders are validated
        for scene in &condor.scenes {
            scene.encoder.validate()?;
        }

        // TODO: Validate all scenes output the same codec

        Ok(((), warnings))
    }

    #[inline]
    fn initialize(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        progress_tx: sync::mpsc::Sender<SequenceStatus>,
    ) -> Result<((), Vec<Box<dyn Error>>)> {
        let mut warnings: Vec<Box<dyn Error>> = vec![];

        // Ensure scenes is not empty
        if condor.scenes.is_empty() {
            warnings.push(Box::new(SerialEncoderError::NoScenes));
        }
        // let encoder = self.encoder.as_ref().map_or(&condor.encoder, |e| e);
        if let Some(input) = &mut self.input {
            // Initialize input by getting clip_info. For VapourSynth inputs, this may begin
            // a lengthy caching process, hence the separation between validate and
            // initialize.
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
    ) -> Result<((), Vec<Box<dyn Error>>)> {
        let mut warnings: Vec<Box<dyn Error>> = vec![];
        if condor.scenes.is_empty() {
            warnings.push(Box::new(SerialEncoderError::NoScenes));
            return Ok(((), warnings));
        }
        let encoder = self.encoder.as_ref().map_or(&condor.encoder, |e| e);
        let input: &mut Input = self.input.as_mut().map_or(&mut condor.input, |i| i);

        let total_frames: usize = condor.scenes.iter().map(|s| s.end_frame - s.start_frame).sum();
        let start_time = Instant::now();
        let (progress_tx, progress_rx) = sync::mpsc::channel::<EncodeProgress>();
        let progress_reports = Arc::new(Mutex::new(Vec::new()));

        let progress_thread = thread::spawn(move || -> Result<()> {
            for progress in progress_rx.iter() {
                let mut reports = progress_reports.lock().expect("mutex should acquire lock");
                reports.push(progress);

                let now = Instant::now();
                let elapsed = now - start_time;
                let frames_encoded = reports.len();
                let fps = frames_encoded as f64 / elapsed.as_secs_f64();
                // let eta = elapsed * (total_frames as f64 / frames_encoded as f64) - elapsed;
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

                // Expect a report for every frame
                if reports.len() == total_frames {
                    break;
                }
            }

            Ok(())
        });

        for (index, scene) in &mut condor.scenes.iter_mut().enumerate() {
            let scene_encoder = scene.encoder.clone();
            let progress_tx_clone = progress_tx.clone();
            let scene_id = SerialEncoder::scene_id(index);
            let extension = encoder.output_extension();
            let output = self.scenes_directory.join(format!("{}.{}", scene_id, extension));

            let (scene_progress_tx, scene_progress_rx) = sync::mpsc::channel();
            // let (frames_tx, frames_rx) = sync::mpsc::channel();
            let (frames_tx, frames_rx) = crossbeam_channel::unbounded();

            let scene_progress_thread = thread::spawn(move || -> Result<()> {
                for progress in scene_progress_rx {
                    progress_tx_clone.send(progress)?;
                }

                Ok(())
            });

            let encoder_thread = thread::spawn(move || -> Result<()> {
                scene_encoder.encode_with_stream(frames_rx, &output, scene_progress_tx)?;

                Ok(())
            });

            let y4m_header = input.y4m_header(Some(scene.end_frame - scene.start_frame))?;
            frames_tx.send(Cursor::new(Vec::from(y4m_header.as_bytes())))?;
            let frame_indices = (scene.start_frame..scene.end_frame).collect::<Vec<_>>();
            input.y4m_frames(frames_tx, &frame_indices)?;

            let _ = encoder_thread
                .join()
                .map_err(|_| anyhow::anyhow!("Failed to join encoder thread"))?;
            let _ = scene_progress_thread
                .join()
                .map_err(|_| anyhow::anyhow!("Failed to join scene thread"))?;
        }

        let _ = progress_thread
            .join()
            .map_err(|_| anyhow::anyhow!("Failed to join progress report thread"))?;

        Ok(((), warnings))
    }
}

impl SerialEncoder {
    pub const DETAILS: SequenceDetails = DETAILS;

    #[inline]
    pub fn new(scenes_directory: &Path) -> Self {
        SerialEncoder {
            input:            None,
            encoder:          None,
            scenes_directory: scenes_directory.to_path_buf(),
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
