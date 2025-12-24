use std::{
    error::Error,
    path::{Path, PathBuf},
    sync::{atomic::AtomicBool, Arc},
};

use anyhow::Result;
use serde::Serialize;

use crate::condor::{
    data::{
        encoding::Encoder,
        processing::{
            BaseProcessDataTrait,
            BaseProcessingTrait,
            BaseProcessorConfigTrait,
            DefaultProcessData,
            DefaultProcessing,
            DefaultProcessorConfig,
        },
        scene::Scene,
        Condor as CondorData,
    },
    core::{
        input::Input,
        output::Output,
        processors::{ProcessStatus, ProcessorDetails, Processors},
    },
};

pub mod encoder;
pub mod input;
pub mod output;
pub mod processors;
pub mod scene;

pub trait CondorInstance<
    Processing = DefaultProcessing,
    ProcessData = DefaultProcessData,
    ProcessorConfig = DefaultProcessorConfig,
> where
    ProcessorConfig: BaseProcessorConfigTrait,
    Processing: BaseProcessingTrait,
    ProcessData: BaseProcessDataTrait,
{
    fn condor(&self) -> &Condor<ProcessData, ProcessorConfig>;
    fn condor_mut(&mut self) -> &mut Condor<ProcessData, ProcessorConfig>;
    fn validate(&mut self) -> Result<((), Vec<Box<dyn Error>>)>;
    fn load(save_file_path: &Path) -> Result<Option<CondorData<ProcessData, ProcessorConfig>>>;
    fn process_one(
        &mut self,
        processor_index: Option<usize>,
        progress_event_tx: std::sync::mpsc::Sender<ProcessProgressEvent>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<((), ProcessorWarningsTuple)>;
    fn process_all(
        &mut self,
        progress_event_tx: std::sync::mpsc::Sender<ProcessProgressEvent>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Vec<((), ProcessorWarningsTuple)>>;
}

pub struct Condor<ProcessData, ProcessorConfig>
where
    ProcessData: BaseProcessDataTrait,
    ProcessorConfig: BaseProcessorConfigTrait,
{
    pub input:            Input,
    pub output:           Output,
    pub encoder:          Encoder,
    pub scenes:           Vec<Scene<ProcessData>>,
    pub processor_config: ProcessorConfig,
    pub save_file:        Option<PathBuf>,
}

impl<ProcessData, ProcessorConfig> Condor<ProcessData, ProcessorConfig>
where
    ProcessData: BaseProcessDataTrait,
    ProcessorConfig: BaseProcessorConfigTrait,
{
    #[inline]
    pub fn new(
        input: Input,
        output: Output,
        encoder: Encoder,
        scenes: Vec<Scene<ProcessData>>,
        processor_config: Option<ProcessorConfig>,
        save_file: Option<PathBuf>,
    ) -> Self {
        Self {
            input,
            output,
            encoder,
            scenes,
            processor_config: processor_config.unwrap_or_default(),
            save_file,
        }
    }

    #[inline]
    pub fn as_data(&self) -> CondorData<ProcessData, ProcessorConfig> {
        CondorData {
            input:            self.input.as_data(),
            output:           self.output.as_data(),
            encoder:          self.encoder.clone(),
            scenes:           self.scenes.clone(),
            processor_config: self.processor_config.clone(),
        }
    }

    #[inline]
    pub fn save(&self) -> Result<Option<CondorData<ProcessData, ProcessorConfig>>> {
        if self.save_file.is_none() {
            return Ok(None);
        }
        let save_file = self.save_file.as_ref().expect("save_file exists");
        let data = CondorData {
            input:            self.input.as_data(),
            output:           self.output.as_data(),
            encoder:          self.encoder.clone(),
            scenes:           self.scenes.clone(),
            processor_config: self.processor_config.clone(),
        };
        Self::save_data(save_file, &data)?;

        Ok(Some(data))
    }

    #[inline]
    pub fn save_data(
        save_file: &Path,
        data: &CondorData<ProcessData, ProcessorConfig>,
    ) -> Result<()> {
        let save_file_ext = save_file.extension();
        let save_file_temp = save_file_ext.map_or_else(
            || save_file.with_extension("temp"),
            |extension| save_file.with_extension(format!(".temp.{}", extension.to_string_lossy())),
        );
        let mut buffer = vec![];
        let formatter = serde_json::ser::PrettyFormatter::with_indent(b"    ");
        let mut serializer = serde_json::Serializer::with_formatter(&mut buffer, formatter);
        data.serialize(&mut serializer)?;
        std::fs::write(&save_file_temp, buffer)?;
        std::fs::rename(&save_file_temp, save_file)?;

        Ok(())
    }
}

pub struct SuperCondor<
    Processing = DefaultProcessing,
    ProcessData = DefaultProcessData,
    ProcessorConfig = DefaultProcessorConfig,
> where
    Processing: BaseProcessingTrait,
    ProcessData: BaseProcessDataTrait,
    ProcessorConfig: BaseProcessorConfigTrait,
{
    pub condor:     Condor<ProcessData, ProcessorConfig>,
    pub processing: Processors<Processing, ProcessData, ProcessorConfig>,
}

impl CondorInstance<DefaultProcessing, DefaultProcessData, DefaultProcessorConfig>
    for SuperCondor<DefaultProcessing, DefaultProcessData, DefaultProcessorConfig>
{
    #[inline]
    fn condor(&self) -> &Condor<DefaultProcessData, DefaultProcessorConfig> {
        &self.condor
    }

    #[inline]
    fn condor_mut(&mut self) -> &mut Condor<DefaultProcessData, DefaultProcessorConfig> {
        &mut self.condor
    }

    #[inline]
    fn load(
        save_file_path: &Path,
    ) -> Result<Option<CondorData<DefaultProcessData, DefaultProcessorConfig>>> {
        if !save_file_path.exists() {
            return Ok(None);
        }
        let data = std::fs::read_to_string(save_file_path)?;
        let data = serde_json::from_str(&data).expect("Failed to deserialize Condor data");
        Ok(Some(data))
    }

    #[inline]
    fn process_one(
        &mut self,
        processor_index: Option<usize>,
        progress_event_tx: std::sync::mpsc::Sender<ProcessProgressEvent>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<((), ProcessorWarningsTuple)> {
        let index = processor_index.unwrap_or(0);
        let processor = &mut self.processing[index];
        let details = processor.details();

        // Processors could have their own Inputs, Outputs, Encoders, and Scenes that
        // should be validated
        let (_, validation_warnings) = processor.validate(&mut self.condor)?;
        // Processors could have their own Inputs that should be initialized
        let (progress_tx, progress_rx) = std::sync::mpsc::channel();
        let event_tx = progress_event_tx.clone();
        let initialization_progress_thread = std::thread::spawn(move || -> Result<()> {
            for progress in progress_rx {
                let event = ProcessProgressEvent {
                    process_type: ProcessType::Initialization,
                    index,
                    details,
                    progress,
                };
                event_tx.send(event)?;
            }
            Ok(())
        });
        let (_, initialization_warnings) = processor.initialize(&mut self.condor, progress_tx)?;
        let _ = initialization_progress_thread
            .join()
            .expect("progress event thread should join");
        // Finally, process the Processor (But who processes the process processor?)
        let (progress_tx, progress_rx) = std::sync::mpsc::channel();
        let process_progress_thread = std::thread::spawn(move || -> Result<()> {
            for progress in progress_rx {
                let event = ProcessProgressEvent {
                    process_type: ProcessType::Processing,
                    index,
                    details,
                    progress,
                };
                progress_event_tx.send(event)?;
            }
            Ok(())
        });
        let (_, process_warnings) = processor.process(&mut self.condor, progress_tx, cancelled)?;
        let _ = process_progress_thread.join().expect("progress event thread should join");

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
        progress_event_tx: std::sync::mpsc::Sender<ProcessProgressEvent>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Vec<((), ProcessorWarningsTuple)>> {
        let mut processor_warnings_tuples = Vec::with_capacity(self.processing.len());
        for i in 0..self.processing.len() {
            if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            let progress_event_tx = progress_event_tx.clone();
            let (_, (validation_warnings, initialization_warnings, process_warnings)) =
                self.process_one(Some(i), progress_event_tx, Arc::clone(&cancelled))?;

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
    fn validate(&mut self) -> Result<((), Vec<Box<dyn Error>>)> {
        let mut warnings = vec![];
        // Validate inputs, outputs, encoders, scenes, and processors

        Input::validate(&self.condor.input.as_data())?;
        Output::validate(&self.condor.output.as_data())?;
        self.condor.encoder.validate()?;
        for scene in &self.condor.scenes {
            scene.encoder.validate()?;
        }

        // Processors could have their own Inputs, Outputs, Encoders, and Scenes that
        // should be validated
        for processor in &mut self.processing {
            let (_, process_warnings) = processor.validate(&mut self.condor)?;

            warnings.extend(process_warnings);
        }

        Ok(((), warnings))
    }
}

pub type ProcessorWarnings = Vec<Box<dyn Error>>;
pub type ProcessorWarningsTuple = (ProcessorWarnings, ProcessorWarnings, ProcessorWarnings);

pub struct ProcessProgressEvent {
    pub process_type: ProcessType,
    pub index:        usize,
    pub details:      ProcessorDetails,
    pub progress:     ProcessStatus,
}

pub enum ProcessType {
    Validation,
    Initialization,
    Processing,
}
