use std::path::{Path, PathBuf};

use crate::utils::cancellation::Cancellation;
use anyhow::{Context, Result};
use serde::Serialize;

use crate::{
    core::{
        input::Input,
        output::Output,
        sequence::{SequenceDetails, SequenceStatus, Sequences},
    },
    models::{
        Condor as CondorModel,
        encoder::Encoder,
        scene::Scene,
        sequence::{
            DefaultSequenceConfig, DefaultSequenceData, SequenceConfigHandler, SequenceDataHandler,
        },
    },
};

#[cfg(test)]
mod tests;

pub mod encoder;
pub mod input;
pub mod output;
pub mod process;
pub mod sequence;
// pub mod scene;

pub type SaveCallback<Data, Config> = Box<dyn Fn(CondorModel<Data, Config>) -> Result<()>>;

pub struct Condor<Data, Config>
where
    Data: SequenceDataHandler,
    Config: SequenceConfigHandler,
{
    pub input: Input,
    pub output: Output,
    pub encoder: Encoder,
    pub scenes: Vec<Scene<Data>>,
    pub sequence_config: Config,
    pub save_callback: SaveCallback<Data, Config>,
}

impl<Data, Config> Condor<Data, Config>
where
    Data: SequenceDataHandler,
    Config: SequenceConfigHandler,
{
    #[inline]
    pub fn new(
        input: Input,
        output: Output,
        encoder: Encoder,
        scenes: Vec<Scene<Data>>,
        processor_config: Option<Config>,
        save_callback: SaveCallback<Data, Config>,
    ) -> Self {
        Self {
            input,
            output,
            encoder,
            scenes,
            sequence_config: processor_config.unwrap_or_default(),
            save_callback,
        }
    }

    #[inline]
    pub fn as_data(&self) -> CondorModel<Data, Config> {
        CondorModel {
            input: self.input.as_data(),
            output: self.output.as_data(),
            encoder: self.encoder.clone(),
            scenes: self.scenes.clone(),
            sequence_config: self.sequence_config.clone(),
        }
    }

    #[inline]
    pub fn save(&self) -> Result<Option<CondorModel<Data, Config>>> {
        let data = CondorModel {
            input: self.input.as_data(),
            output: self.output.as_data(),
            encoder: self.encoder.clone(),
            scenes: self.scenes.clone(),
            sequence_config: self.sequence_config.clone(),
        };

        (self.save_callback)(data.clone())?;
        // Self::save_data(save_file, &data)?;

        Ok(Some(data))
    }

    #[inline]
    pub fn save_data(save_file: &Path, data: &CondorModel<Data, Config>) -> Result<()> {
        let save_file_temp = temp_save_path(save_file);
        let mut buffer = vec![];
        let formatter = serde_json::ser::PrettyFormatter::with_indent(b"    ");
        let mut serializer = serde_json::Serializer::with_formatter(&mut buffer, formatter);
        data.serialize(&mut serializer)?;
        std::fs::write(&save_file_temp, buffer)?;
        std::fs::rename(&save_file_temp, save_file)?;

        Ok(())
    }
}

/// The temporary file [`CondorModel::save_data`] writes before renaming onto
/// `save_file`.
///
/// `Path::with_extension` replaces the existing extension rather than appending
/// to it, so the temp name is `{stem}.temp.{ext}`. A leading dot in the
/// replacement extension would keep the original dot and produce
/// `condor..temp.json`.
fn temp_save_path(save_file: &Path) -> PathBuf {
    save_file.extension().map_or_else(
        || save_file.with_extension("temp"),
        |extension| save_file.with_extension(format!("temp.{}", extension.to_string_lossy())),
    )
}

pub trait AndeanCondor<Data = DefaultSequenceData, Config = DefaultSequenceConfig>
where
    Data: SequenceDataHandler,
    Config: SequenceConfigHandler,
{
    fn condor(&self) -> &Condor<Data, Config>;
    fn condor_mut(&mut self) -> &mut Condor<Data, Config>;
    fn validate(&mut self) -> Result<((), SequenceWarnings)>;
    fn load(save_file_path: &Path) -> Result<Option<CondorModel<Data, Config>>>;
    fn process_one(
        &mut self,
        processor_index: Option<usize>,
        progress_event_tx: std::sync::mpsc::Sender<SequenceProgressEvent>,
        cancellation: Cancellation,
    ) -> Result<((), SequenceWarningsTuple)>;
    fn process_all(
        &mut self,
        progress_event_tx: std::sync::mpsc::Sender<SequenceProgressEvent>,
        cancellation: Cancellation,
    ) -> Result<Vec<((), SequenceWarningsTuple)>>;
}

pub struct DefaultAndeanCondor<Data = DefaultSequenceData, Config = DefaultSequenceConfig>
where
    Data: SequenceDataHandler,
    Config: SequenceConfigHandler,
{
    pub condor: Condor<Data, Config>,
    pub sequences: Sequences<Data, Config>,
}

impl AndeanCondor<DefaultSequenceData, DefaultSequenceConfig>
    for DefaultAndeanCondor<DefaultSequenceData, DefaultSequenceConfig>
{
    #[inline]
    fn condor(&self) -> &Condor<DefaultSequenceData, DefaultSequenceConfig> {
        &self.condor
    }

    #[inline]
    fn condor_mut(&mut self) -> &mut Condor<DefaultSequenceData, DefaultSequenceConfig> {
        &mut self.condor
    }

    #[inline]
    fn load(
        save_file_path: &Path,
    ) -> Result<Option<CondorModel<DefaultSequenceData, DefaultSequenceConfig>>> {
        let data = match std::fs::read_to_string(save_file_path) {
            Ok(data) => data,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "Failed to read saved Condor model from {}",
                        save_file_path.display()
                    )
                });
            },
        };
        let data = serde_json::from_str(&data).with_context(|| {
            format!(
                "Failed to deserialize malformed or incompatible saved Condor model from {}",
                save_file_path.display()
            )
        })?;
        Ok(Some(data))
    }

    #[inline]
    fn process_one(
        &mut self,
        processor_index: Option<usize>,
        progress_event_tx: std::sync::mpsc::Sender<SequenceProgressEvent>,
        cancelled: Cancellation,
    ) -> Result<((), SequenceWarningsTuple)> {
        cancelled.check()?;
        let index = processor_index.unwrap_or(0);
        let sequence_count = self.sequences.len();
        let processor = self.sequences.get_mut(index).with_context(|| {
            format!("Sequence index {index} is out of range (only {sequence_count} sequences)")
        })?;
        let details = processor.details();

        // Processors could have their own Inputs, Outputs, Encoders, and Scenes
        // that should be validated
        let (_, validation_warnings) = processor.validate(&mut self.condor)?;
        cancelled.check()?;
        // Processors could have their own Inputs that should be initialized
        let (progress_tx, progress_rx) = std::sync::mpsc::channel();
        let event_tx = progress_event_tx.clone();
        let initialization_progress_thread = std::thread::spawn(move || -> Result<()> {
            for progress in progress_rx {
                let event = SequenceProgressEvent {
                    sequence_type: SequenceType::Initialization,
                    index,
                    details,
                    progress,
                };
                event_tx.send(event)?;
            }
            Ok(())
        });
        let initialized = processor.initialize(&mut self.condor, progress_tx, &cancelled);
        let relayed = encoder::pipes::join_worker(
            initialization_progress_thread,
            "Joining initialization progress relay",
        );
        let (_, initialization_warnings) = initialized?;
        relayed?;
        cancelled.check()?;
        // Finally, process the Processor (But who processes the process
        // processor?)
        let (progress_tx, progress_rx) = std::sync::mpsc::channel();
        let process_progress_thread = std::thread::spawn(move || -> Result<()> {
            for progress in progress_rx {
                let event = SequenceProgressEvent {
                    sequence_type: SequenceType::Processing,
                    index,
                    details,
                    progress,
                };
                progress_event_tx.send(event)?;
            }
            Ok(())
        });
        let executed = processor.execute(&mut self.condor, progress_tx, cancelled.clone());
        let relayed =
            encoder::pipes::join_worker(process_progress_thread, "Joining sequence progress relay");
        let (_, process_warnings) = executed?;
        cancelled.check_forced()?;
        relayed?;

        Ok((
            (),
            (
                validation_warnings,
                initialization_warnings,
                process_warnings,
            ),
        ))
    }

    #[inline]
    fn process_all(
        &mut self,
        progress_event_tx: std::sync::mpsc::Sender<SequenceProgressEvent>,
        cancelled: Cancellation,
    ) -> Result<Vec<((), SequenceWarningsTuple)>> {
        let mut processor_warnings_tuples = Vec::with_capacity(self.sequences.len());
        for i in 0..self.sequences.len() {
            cancelled.check_forced()?;
            if cancelled.is_cancelled() {
                break;
            }
            let progress_event_tx = progress_event_tx.clone();
            let (_, (validation_warnings, initialization_warnings, process_warnings)) =
                self.process_one(Some(i), progress_event_tx, cancelled.clone())?;

            processor_warnings_tuples.push((
                (),
                (
                    validation_warnings,
                    initialization_warnings,
                    process_warnings,
                ),
            ));
        }

        Ok(processor_warnings_tuples)
    }

    #[inline]
    fn validate(&mut self) -> Result<((), SequenceWarnings)> {
        let mut warnings = vec![];
        // Validate inputs, outputs, encoders, scenes, and processors

        Input::validate(&self.condor.input.as_data())?;
        Output::validate(&self.condor.output.as_data())?;
        self.condor.encoder.validate()?;
        for scene in &self.condor.scenes {
            scene.encoder.validate()?;
        }

        // Processors could have their own Inputs, Outputs, Encoders, and Scenes
        // that should be validated
        for processor in &mut self.sequences {
            let (_, process_warnings) = processor.validate(&mut self.condor)?;

            warnings.extend(process_warnings);
        }

        Ok(((), warnings))
    }
}

pub type SequenceWarnings = Vec<anyhow::Error>;
pub type SequenceWarningsTuple = (SequenceWarnings, SequenceWarnings, SequenceWarnings);

pub struct SequenceProgressEvent {
    pub sequence_type: SequenceType,
    pub index: usize,
    pub details: SequenceDetails,
    pub progress: SequenceStatus,
}

pub enum SequenceType {
    Validation,
    Initialization,
    Processing,
}
