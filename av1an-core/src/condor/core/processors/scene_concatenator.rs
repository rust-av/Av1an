#[cfg(windows)]
use std::path::Path;
use std::{
    error::Error,
    fs::File,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{self, atomic::AtomicBool, Arc},
};

use anyhow::{bail, Context, Result};
use av_format::rational::{Ratio, Rational64};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    condor::{
        data::processing::{BaseProcessDataTrait, BaseProcessingTrait, BaseProcessorConfigTrait},
        core::{
            input::Input,
            processors::{
                parallel_encoder::ParallelEncoder,
                ProcessStatus,
                Processor,
                ProcessorDetails,
            },
            Condor,
        },
    },
    ConcatMethod,
};

static DETAILS: ProcessorDetails = ProcessorDetails {
    name:        "Scene Concatenator",
    description: "Concatenates encoded scenes into a single output file",
    version:     "0.0.1",
};

pub struct SceneConcatenator {
    pub method:           Option<ConcatMethod>,
    pub scenes_directory: PathBuf,
}

impl<Processing, ProcessData, ProcessorConfig> Processor<Processing, ProcessData, ProcessorConfig>
    for SceneConcatenator
where
    Processing: BaseProcessingTrait,
    ProcessData: BaseProcessDataTrait,
    ProcessorConfig: BaseProcessorConfigTrait,
{
    #[inline]
    fn details(&self) -> ProcessorDetails {
        DETAILS
    }

    #[inline]
    fn validate(
        &mut self,
        condor: &mut Condor<ProcessData, ProcessorConfig>,
    ) -> Result<((), Vec<Box<dyn Error>>)> {
        let method = self.method.unwrap_or(condor.output.concatenation_method);
        match method {
            ConcatMethod::MKVMerge => {
                if which::which("mkvmerge").is_err() {
                    bail!(SceneConcatenatorError::MKVMergeNotInstalled);
                }
            },
            ConcatMethod::FFmpeg => {
                if which::which("ffmpeg").is_err() {
                    bail!(SceneConcatenatorError::FFmpegNotInstalled);
                }
            },
            ConcatMethod::Ivf => todo!(),
        }

        Ok(((), vec![]))
    }

    #[inline]
    fn initialize(
        &mut self,
        condor: &mut Condor<ProcessData, ProcessorConfig>,
        progress_tx: sync::mpsc::Sender<ProcessStatus>,
    ) -> Result<((), Vec<Box<dyn Error>>)> {
        let mut warnings: Vec<Box<dyn Error>> = vec![];

        if !self.scenes_directory.exists() {
            bail!(SceneConcatenatorError::ScenesDirectoryMissing {
                path: self.scenes_directory.clone(),
            });
        }
        if !self.scenes_directory.is_dir() {
            bail!(SceneConcatenatorError::ScenesDirectoryInvalid {
                path: self.scenes_directory.clone(),
            });
        }

        if !self.scratch_directory().exists() {
            std::fs::create_dir_all(self.scratch_directory())?;
        }

        let scene_files = condor
            .scenes
            .iter()
            .enumerate()
            .map(|(index, scene)| {
                let path = self.scenes_directory.join(format!(
                    "{}.{}",
                    ParallelEncoder::scene_id(index),
                    scene.encoder.output_extension()
                ));
                let exists = path.exists();

                (index, path, exists)
            })
            .filter(|(_, _, exists)| !*exists)
            .collect::<Vec<_>>();

        if !scene_files.is_empty() {
            warnings.push(Box::new(SceneConcatenatorError::SceneFilesMissing {
                scenes: scene_files.iter().map(|(index, _, _)| *index).collect(),
            }));
        }

        Ok(((), warnings))
    }

    #[inline]
    fn process(
        &mut self,
        condor: &mut Condor<ProcessData, ProcessorConfig>,
        progress_tx: sync::mpsc::Sender<ProcessStatus>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<((), Vec<Box<dyn Error>>)> {
        let warnings: Vec<Box<dyn Error>> = vec![];

        let method = self.method.unwrap_or(condor.output.concatenation_method);
        let framerate = condor.input.clip_info()?.frame_rate;
        let input_path = {
            match &condor.input {
                Input::Video {
                    path, ..
                }
                | Input::VapourSynth {
                    path, ..
                } => Some(path.as_path()),
                Input::VapourSynthScript {
                    ..
                } => None, // May be invalid/Optional in the future
            }
        };
        let scenes = condor
            .scenes
            .iter()
            .enumerate()
            .map(|(index, scene)| {
                let path = self.scenes_directory.join(format!(
                    "{}.{}",
                    ParallelEncoder::scene_id(index),
                    scene.encoder.output_extension()
                ));
                let exists = path.exists();

                (index, path, exists)
            })
            .filter(|(_, _, exists)| *exists)
            .collect::<Vec<_>>();

        let scene_paths = scenes.iter().map(|(_, path, _)| path.clone()).collect::<Vec<_>>();

        match method {
            ConcatMethod::MKVMerge => {
                self.mkvmerge(&condor.output.path, &scene_paths, input_path, framerate)?;
            },
            ConcatMethod::FFmpeg => self.ffmpeg(&condor.output.path, &scene_paths)?,
            ConcatMethod::Ivf => todo!(),
        };

        Ok(((), warnings))
    }
}

impl SceneConcatenator {
    pub const DETAILS: ProcessorDetails = DETAILS;
    #[inline]
    pub fn new(scenes_directory: &Path) -> Self {
        Self {
            scenes_directory: scenes_directory.to_path_buf(),
            method:           None,
        }
    }

    #[inline]
    pub fn mkvmerge(
        &self,
        output: &Path,
        scene_paths: &[PathBuf],
        input: Option<&Path>,
        duration: Ratio<i64>,
    ) -> Result<()> {
        const MAXIMUM_CHUNKS_PER_MERGE: usize = 100;
        // mkvmerge does not accept UNC paths on Windows
        #[cfg(windows)]
        fn fix_path<P: AsRef<Path>>(p: P) -> String {
            const UNC_PREFIX: &str = r#"\\?\"#;

            let p = p.as_ref().display().to_string();
            p.strip_prefix(UNC_PREFIX).map_or_else(
                || p.clone(),
                |path| {
                    path.strip_prefix("UNC")
                        .map_or_else(|| path.to_string(), |p2| format!("\\{p2}"))
                },
            )
        }

        #[cfg(not(windows))]
        fn fix_path<P: AsRef<Path>>(p: P) -> String {
            p.as_ref().display().to_string()
        }

        let scratch_directory = self.scratch_directory();
        let fixed_output = fix_path(output);
        let fixed_input = input.map(fix_path);

        let chunk_groups: Vec<Vec<PathBuf>> = scene_paths
            .chunks(MAXIMUM_CHUNKS_PER_MERGE)
            .map(|chunk| chunk.to_vec())
            .collect();

        for (group_index, chunk_group) in chunk_groups.iter().enumerate() {
            let group_options_path = scratch_directory.join(format!("{group_index:05}.json"));
            let group_output_path = scratch_directory.join(format!("{group_index:05}.mkv"));
            let group_output_path = fix_path(&group_output_path);

            let group_options = MKVMergeOptions::new(
                &group_output_path,
                &chunk_group.iter().map(fix_path).collect::<Vec<_>>(),
                None,
                None,
            );
            group_options.write_to_disk(&group_options_path)?;

            let mut group_cmd = Command::new("mkvmerge");
            group_cmd.current_dir(&self.scenes_directory);
            group_cmd.arg(format!("@./Scene Concatenator/{group_index:05}.json"));

            let group_out =
                group_cmd.output().with_context(|| "Failed to concatenate with mkvmerge")?;

            if !group_out.status.success() {
                bail!(SceneConcatenatorError::MkvmergeFailed {
                    status: group_out.status,
                });
            }
        }

        let options_path = scratch_directory.join("options.json");
        let chunk_group_options_names = chunk_groups
            .iter()
            .enumerate()
            .map(|(index, _)| format!("{index:05}.mkv"))
            .collect::<Vec<_>>();
        let options = MKVMergeOptions::new(
            &fixed_output,
            &chunk_group_options_names,
            fixed_input.as_deref(),
            Some(duration),
        );
        options.write_to_disk(&options_path)?;

        let mut cmd = Command::new("mkvmerge");
        cmd.current_dir(&scratch_directory);
        cmd.arg("@./options.json");
        let out = cmd.output().with_context(|| "Failed to concatenate with mkvmerge")?;

        if !out.status.success() {
            bail!(SceneConcatenatorError::MkvmergeFailed {
                status: out.status
            });
        }

        Ok(())
    }

    #[inline]
    pub fn ffmpeg(&self, output: &Path, scene_paths: &[PathBuf]) -> Result<()> {
        let scratch_directory = self.scenes_directory.join("Scene Concatenator");
        let concat_file_path = scratch_directory.join("concat.txt");
        let concat_file = {
            let mut contents = String::with_capacity(24 * scene_paths.len());

            for scene_path in scene_paths {
                let fixed_path = scene_path
                    .display()
                    .to_string()
                    .replace('\\', r"\\")
                    .replace(' ', r"\ ")
                    .replace('\'', r"\'");
                contents.push_str("file ");
                contents.push_str(&fixed_path);
                contents.push('\n');
            }

            contents
        };
        File::create(&concat_file_path)?.write_all(concat_file.as_bytes())?;

        let mut cmd = Command::new("ffmpeg");

        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        cmd.args(["-y", "-hide_banner", "-loglevel", "error", "-f", "concat", "-safe", "0", "-i"]);
        cmd.arg(concat_file_path);
        // todo: copy from input -i
        cmd.args(["-map", "0"]);
        // copy from input -i
        // cmd.args(["-map", "1", "-map", "-1:v"]);
        cmd.args(["-c", "copy"]);
        cmd.arg(output);

        let out = cmd.output().with_context(|| "Failed to concatenate with ffmpeg")?;

        if !out.status.success() {
            bail!(SceneConcatenatorError::FfmpegFailed {
                status: out.status
            });
        }

        Ok(())
    }

    pub(crate) fn scratch_directory(&self) -> PathBuf {
        self.scenes_directory.join("Scene Concatenator")
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct MKVMergeOptions {
    output:           String,
    audio:            Option<String>,
    default_duration: Option<String>,
    chunks:           Vec<String>,
}

impl MKVMergeOptions {
    pub fn new(
        output: &str,
        chunks: &[String],
        audio: Option<&str>,
        default_duration: Option<Rational64>,
    ) -> Self {
        let default_duration = default_duration
            .map(|output_fps| format!("0:{}/{}fps", output_fps.numer(), output_fps.denom()));

        MKVMergeOptions {
            output: output.to_string(),
            audio: audio.map(|a| a.to_string()),
            default_duration,
            chunks: chunks.to_vec(),
        }
    }

    pub fn write_to_disk(&self, path: &Path) -> Result<()> {
        let args = self.generate_args();
        let mut file = File::create(path)?;
        file.write_all(serde_json::to_string_pretty(&args)?.as_bytes())?;
        Ok(())
    }

    pub fn generate_args(&self) -> Vec<&str> {
        let mut args = vec!["-o", &self.output];
        if let Some(audio) = &self.audio {
            args.push("--no-video");
            args.push(audio);
        }
        if let Some(default_duration) = &self.default_duration {
            args.push("--default-duration");
            args.push(default_duration);
        }
        args.push("[");
        for chunk in &self.chunks {
            args.push(chunk);
        }
        args.push("]");
        args
    }
}

#[derive(Debug, Error)]
pub enum SceneConcatenatorError {
    #[error("mkvmerge not installed")]
    MKVMergeNotInstalled,
    #[error("FFmpeg not installed")]
    FFmpegNotInstalled,
    #[error("Missing scene files: {scenes:?}")]
    SceneFilesMissing { scenes: Vec<usize> },
    #[error("Missing scenes directory: {path}")]
    ScenesDirectoryMissing { path: PathBuf },
    #[error("Scenes directory is not a directory: {path}")]
    ScenesDirectoryInvalid { path: PathBuf },
    #[error("Failed to concatenate with mkvmerge: {status}")]
    MkvmergeFailed { status: std::process::ExitStatus },
    #[error("Failed to concatenate with ffmpeg: {status}")]
    FfmpegFailed { status: std::process::ExitStatus },
}
