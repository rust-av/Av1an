use std::{
    ffi::OsString,
    fs::File,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{self, Arc, Mutex, atomic::AtomicBool},
    thread,
};

use crate::models::sequence::{
    parallel_encoder::{ParallelEncoderConfigHandler, ParallelEncoderDataHandler},
    provenance,
    target_quality::{TargetQualityConfigHandler, TargetQualityDataHandler},
};
use crate::{
    core::process::{InterruptedOutput, OwnedProcess},
    utils::cancellation::Cancellation,
};
use anyhow::{Context, Result, bail};
use av_format::rational::Ratio;
use thiserror::Error;
use tracing::error;

use crate::{
    core::{
        Condor,
        input::Input,
        sequence::{
            Sequence, SequenceCompletion, SequenceDetails, SequenceStatus, Status,
            parallel_encoder::ParallelEncoder,
        },
    },
    models::sequence::{
        SequenceConfigHandler, SequenceDataHandler,
        scene_concatenator::{
            Chapters, ConcatMethod, FfmpegConfig, FfmpegTrack, FfmpegTrackMap, FfmpegTrackOptions,
            Metadata, MkvmergeConfig, MkvmergeExtraInput, SceneConcatenatorConfigHandler,
            mkvmerge::tags_xml,
        },
    },
};

mod ffmpeg;
mod ivf;
mod mkvmerge;
mod source;

pub static DETAILS: SequenceDetails = SequenceDetails {
    name: "Scene Concatenator",
    description: "Concatenates encoded scenes into a single output file",
    version: "0.0.1",
};
#[derive(Default)]
pub struct SceneConcatenator {}
impl<DataHandler, ConfigHandler> Sequence<DataHandler, ConfigHandler> for SceneConcatenator
where
    DataHandler: SequenceDataHandler + ParallelEncoderDataHandler + TargetQualityDataHandler,
    ConfigHandler: SequenceConfigHandler
        + SceneConcatenatorConfigHandler
        + ParallelEncoderConfigHandler
        + TargetQualityConfigHandler,
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
        Self::admit_final(condor)?;
        let concatenator = condor.sequence_config.scene_concatenator()?;
        let mut warnings = Vec::new();
        match &concatenator.method {
            ConcatMethod::MKVMerge(mkvmerge) => {
                if which::which("mkvmerge").is_err() {
                    bail!(SceneConcatenatorError::MKVMergeNotInstalled);
                }
                warnings.extend(mkvmerge.validate()?);
            },
            ConcatMethod::FFmpeg(ffmpeg) => {
                if which::which("ffmpeg").is_err() {
                    bail!(SceneConcatenatorError::FFmpegNotInstalled);
                }
                let has_input = matches!(
                    condor.input,
                    Input::Video { .. } | Input::VapourSynth { .. }
                );
                if !has_input && ffmpeg.extra_inputs.is_empty() {
                    warnings.push(SceneConcatenatorError::NoFfmpegInputs.into());
                }
                warnings.extend(ffmpeg.validate()?);
            },
            ConcatMethod::Ivf => (),
        }

        Ok(((), warnings))
    }

    #[inline]
    fn initialize(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        _progress_tx: sync::mpsc::Sender<SequenceStatus>,
    ) -> Result<((), Vec<anyhow::Error>)> {
        let scenes_directory = &condor.sequence_config.scene_concatenator()?.scenes_directory;
        if !scenes_directory.exists() {
            bail!(SceneConcatenatorError::ScenesDirectoryMissing {
                path: scenes_directory.clone(),
            });
        }
        if !scenes_directory.is_dir() {
            bail!(SceneConcatenatorError::ScenesDirectoryInvalid {
                path: scenes_directory.clone(),
            });
        }
        let scene_paths = Self::scene_paths(scenes_directory, &condor.scenes);
        Self::validate_scene_paths(&scene_paths)?;
        let scratch_directory = Self::scratch_directory(scenes_directory.as_path());
        if !scratch_directory.exists() {
            std::fs::create_dir_all(scratch_directory)?;
        }

        Ok(((), vec![]))
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
        progress_tx: sync::mpsc::Sender<SequenceStatus>,
        cancelled: Cancellation,
    ) -> Result<((), Vec<anyhow::Error>)> {
        let cancelled = cancelled.admit()?;
        Self::admit_final(condor)?;
        let warnings = vec![];

        let root = condor.input.as_data();
        let pe = condor.sequence_config.parallel_encoder()?;
        let tq = condor.sequence_config.target_quality()?.as_ref();
        let input = pe.input.as_ref().unwrap_or(&root);
        let probe = provenance::probe_input(&root, pe, tq);
        let metric = tq.and_then(|c| c.metric_input.as_ref());
        let mut admission = provenance::Admission::default();
        let config = condor.sequence_config.scene_concatenator()?;
        let scene_paths = Self::scene_paths(&config.scenes_directory, &condor.scenes);
        for (index, (scene, path)) in condor.scenes.iter().zip(&scene_paths).enumerate() {
            admission.admit_encode(input, probe, metric, scene, tq, path, index)?;
            super::parallel_encoder::artifact::validate(
                path,
                scene.end_frame - scene.start_frame,
                false, // PE owns decode/count validation; concat checks only identity/nonempty.
                &cancelled,
            )?;
        }
        Self::validate_scene_paths(&scene_paths)?;

        let framerate = condor.input.clip_info()?.frame_rate;
        let input_path = {
            match &condor.input {
                Input::Video { path, .. } | Input::VapourSynth { path, .. } => Some(path.as_path()),
                Input::VapourSynthScript { .. } => None, // May be invalid/Optional in the future
            }
        };
        let destination = config.effective_output(&condor.output.path);
        let destination = std::env::current_dir()?.join(destination);
        // Native muxers can require a nonexistent path. A private owned sibling
        // directory reserves its namespace without an unlink/recreate race or prompt.
        let staging = tempfile::Builder::new()
            .prefix(".condor-output-")
            .tempdir_in(destination.parent().context("Output has no parent")?)?;
        let staged_path = staging.path().join(format!(
            "output.{}",
            destination
                .extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or_else(|| config.method.extension())
        ));
        let total_frames =
            condor.scenes.iter().map(|scene| scene.end_frame - scene.start_frame).sum();

        match &config.method {
            ConcatMethod::MKVMerge(mkvmerge) => {
                Self::mkvmerge_cancel(
                    &config.scenes_directory,
                    &staged_path,
                    &scene_paths,
                    input_path,
                    framerate,
                    Some(mkvmerge),
                    &progress_tx,
                    &cancelled,
                )?;
            },
            ConcatMethod::FFmpeg(ffmpeg) => {
                Self::ffmpeg_cancel(
                    &config.scenes_directory,
                    &staged_path,
                    &scene_paths,
                    input_path,
                    total_frames,
                    framerate,
                    Some(ffmpeg),
                    &progress_tx,
                    &cancelled,
                )?;
            },
            ConcatMethod::Ivf => {
                Self::ivf_cancel(&staged_path, &scene_paths, &progress_tx, &cancelled)?;
            },
        };

        #[cfg(test)]
        tests::publication::before_commit(&staged_path);
        cancelled.publish(|| {
            // Final output differs from active chunks: graceful may finish muxing, but
            // withdraws publication. Check under the transition lock, not before it.
            cancelled.check()?;
            // ponytail: shape only, not codec validity; mux/encode validation owns that.
            let metadata = std::fs::symlink_metadata(&staged_path).with_context(|| {
                format!("Inspecting final mux output {}", staged_path.display())
            })?;
            anyhow::ensure!(
                metadata.is_file() && metadata.len() > 0,
                "Final mux output is not a nonempty regular file: {}",
                staged_path.display()
            );
            Self::admit_final(condor)?;
            #[cfg(test)]
            tests::publication::before_persist(&staged_path, &destination);
            crate::core::output::persist_output(
                tempfile::TempPath::try_from_path(staged_path)?,
                &destination,
                condor.output.overwrite,
            )?;
            progress_tx.send(SequenceStatus::Whole(Status::Completed {
                id: DETAILS.name.to_owned(),
            }))?;
            Ok(())
        })?;

        Ok(((), warnings))
    }
}
impl SceneConcatenator {
    pub const DETAILS: SequenceDetails = DETAILS;

    /// Declared root/stage/mux sources and every configured encoded chunk, including missing ones.
    /// Inline script reads and custom inputs omitted by the config handler cannot be inferred.
    #[inline]
    pub fn protected_sources<Data, Config: SceneConcatenatorConfigHandler>(
        input: &crate::models::input::Input,
        config: &Config,
        scenes: &[crate::models::scene::Scene<Data>],
    ) -> Result<Vec<PathBuf>> {
        let concat = config.scene_concatenator()?;
        let mut paths = config.protected_sources();
        paths.extend(input.path().map(Path::to_path_buf));
        paths.extend(concat.mux_sources());
        paths.extend(Self::scene_paths(&concat.scenes_directory, scenes));
        Ok(paths)
    }

    fn scene_paths<Data>(
        scenes_directory: &Path,
        scenes: &[crate::models::scene::Scene<Data>],
    ) -> Vec<PathBuf> {
        scenes
            .iter()
            .enumerate()
            .map(|(index, scene)| {
                scenes_directory.join(format!(
                    "{}.{}",
                    ParallelEncoder::scene_id(index),
                    scene.encoder.base().output_extension()
                ))
            })
            .collect()
    }

    // ponytail: filesystem shape/readability only, not codec validity or C14 saved eligibility.
    fn validate_scene_paths(scene_paths: &[PathBuf]) -> Result<()> {
        anyhow::ensure!(
            !scene_paths.is_empty(),
            "Cannot concatenate an empty scene list"
        );
        for (index, path) in scene_paths.iter().enumerate() {
            let checked = || -> Result<()> {
                let metadata = path.metadata()?;
                anyhow::ensure!(
                    metadata.is_file() && metadata.len() > 0,
                    "not a nonempty regular file"
                );
                File::open(path)?.read_exact(&mut [0])?;
                Ok(())
            };
            checked().with_context(|| {
                format!("Validating requested scene {index}: {}", path.display())
            })?;
        }
        Ok(())
    }

    fn admit_final<
        Data: SequenceDataHandler,
        Config: SequenceConfigHandler + SceneConcatenatorConfigHandler,
    >(
        condor: &Condor<Data, Config>,
    ) -> Result<()> {
        let sources = Self::protected_sources(
            &condor.input.as_data(),
            &condor.sequence_config,
            &condor.scenes,
        )?;
        let config = condor.sequence_config.scene_concatenator()?;
        crate::core::output::admit_output(
            config.effective_output(&condor.output.path),
            &sources,
            condor.output.overwrite,
            true,
        )
    }

    #[inline]
    fn send_progress(progress_tx: &sync::mpsc::Sender<SequenceStatus>, percentage: f64) {
        if !percentage.is_finite() {
            return;
        }
        let _ = progress_tx.send(SequenceStatus::Whole(Status::Processing {
            id: DETAILS.name.to_owned(),
            completion: SequenceCompletion::Percentage(percentage.clamp(0.0, 100.0)),
        }));
    }

    pub(crate) fn scratch_directory(scenes_directory: &Path) -> PathBuf {
        scenes_directory.join("Scene Concatenator")
    }
}
#[derive(Debug, Error)]
pub enum SceneConcatenatorError {
    #[error("mkvmerge not installed")]
    MKVMergeNotInstalled,
    #[error("FFmpeg not installed")]
    FFmpegNotInstalled,
    #[error(
        "FFmpeg output settings configure no additional inputs; the output will contain only the \
         encoded video"
    )]
    NoFfmpegInputs,
    #[error("Missing scene files: {scenes:?}")]
    SceneFilesMissing { scenes: Vec<usize> },
    #[error("Missing scenes directory: {path}")]
    ScenesDirectoryMissing { path: PathBuf },
    #[error("Scenes directory is not a directory: {path}")]
    ScenesDirectoryInvalid { path: PathBuf },
    #[error("Failed to concatenate with mkvmerge: {status}\nSTDOUT:\n{stdout}\nSTDERR:\n{stderr}")]
    MkvmergeFailed {
        status: std::process::ExitStatus,
        stdout: String,
        stderr: String,
    },
    #[error("Failed to concatenate with ffmpeg: {status}\nSTDOUT:\n{stdout}\nSTDERR:\n{stderr}")]
    FfmpegFailed {
        status: std::process::ExitStatus,
        stdout: String,
        stderr: String,
    },
}

#[cfg(test)]
mod tests;
