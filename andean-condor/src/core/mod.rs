use std::{
    error::Error,
    path::Path,
    sync::{atomic::AtomicBool, Arc},
};

use anyhow::Result;
use serde::Serialize;

use crate::{
    core::{
        input::Input,
        output::Output,
        sequence::{SequenceDetails, SequenceStatus, Sequences},
    },
    models::{
        encoder::Encoder,
        scene::Scene,
        sequence::{
            DefaultSequenceConfig,
            DefaultSequenceData,
            SequenceConfigHandler,
            SequenceDataHandler,
        },
        Condor as CondorModel,
    },
};

pub mod encoder;
pub mod input;
pub mod output;
pub mod sequence;
// pub mod scene;

pub type SaveCallback<Data, Config> = Box<dyn Fn(CondorModel<Data, Config>) -> Result<()>>;

pub struct Condor<Data, Config>
where
    Data: SequenceDataHandler,
    Config: SequenceConfigHandler,
{
    pub input:            Input,
    pub output:           Output,
    pub encoder:          Encoder,
    pub scenes:           Vec<Scene<Data>>,
    pub processor_config: Config,
    pub save_callback:    SaveCallback<Data, Config>,
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
            processor_config: processor_config.unwrap_or_default(),
            save_callback,
        }
    }

    #[inline]
    pub fn as_data(&self) -> CondorModel<Data, Config> {
        CondorModel {
            input:           self.input.as_data(),
            output:          self.output.as_data(),
            encoder:         self.encoder.clone(),
            scenes:          self.scenes.clone(),
            sequence_config: self.processor_config.clone(),
        }
    }

    #[inline]
    pub fn save(&self) -> Result<Option<CondorModel<Data, Config>>> {
        let data = CondorModel {
            input:           self.input.as_data(),
            output:          self.output.as_data(),
            encoder:         self.encoder.clone(),
            scenes:          self.scenes.clone(),
            sequence_config: self.processor_config.clone(),
        };

        (self.save_callback)(data.clone())?;
        // Self::save_data(save_file, &data)?;

        Ok(Some(data))
    }

    #[inline]
    pub fn save_data(save_file: &Path, data: &CondorModel<Data, Config>) -> Result<()> {
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

pub trait AndeanCondor<Data = DefaultSequenceData, Config = DefaultSequenceConfig>
where
    Data: SequenceDataHandler,
    Config: SequenceConfigHandler,
{
    fn condor(&self) -> &Condor<Data, Config>;
    fn condor_mut(&mut self) -> &mut Condor<Data, Config>;
    fn validate(&mut self) -> Result<((), Vec<Box<dyn Error>>)>;
    fn load(save_file_path: &Path) -> Result<Option<CondorModel<Data, Config>>>;
    fn process_one(
        &mut self,
        processor_index: Option<usize>,
        progress_event_tx: std::sync::mpsc::Sender<SequenceProgressEvent>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<((), SequenceWarningsTuple)>;
    fn process_all(
        &mut self,
        progress_event_tx: std::sync::mpsc::Sender<SequenceProgressEvent>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Vec<((), SequenceWarningsTuple)>>;
}

pub struct DefaultAndeanCondor<Data = DefaultSequenceData, Config = DefaultSequenceConfig>
where
    Data: SequenceDataHandler,
    Config: SequenceConfigHandler,
{
    pub condor:    Condor<Data, Config>,
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
        progress_event_tx: std::sync::mpsc::Sender<SequenceProgressEvent>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<((), SequenceWarningsTuple)> {
        let index = processor_index.unwrap_or(0);
        let processor = &mut self.sequences[index];
        let details = processor.details();

        // Processors could have their own Inputs, Outputs, Encoders, and Scenes that
        // should be validated
        let (_, validation_warnings) = processor.validate(&mut self.condor)?;
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
        let (_, initialization_warnings) = processor.initialize(&mut self.condor, progress_tx)?;
        let _ = initialization_progress_thread
            .join()
            .expect("progress event thread should join");
        // Finally, process the Processor (But who processes the process processor?)
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
        let (_, process_warnings) = processor.execute(&mut self.condor, progress_tx, cancelled)?;
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
        progress_event_tx: std::sync::mpsc::Sender<SequenceProgressEvent>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Vec<((), SequenceWarningsTuple)>> {
        let mut processor_warnings_tuples = Vec::with_capacity(self.sequences.len());
        for i in 0..self.sequences.len() {
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
        for processor in &mut self.sequences {
            let (_, process_warnings) = processor.validate(&mut self.condor)?;

            warnings.extend(process_warnings);
        }

        Ok(((), warnings))
    }
}

pub type SequenceWarnings = Vec<Box<dyn Error>>;
pub type SequenceWarningsTuple = (SequenceWarnings, SequenceWarnings, SequenceWarnings);

pub struct SequenceProgressEvent {
    pub sequence_type: SequenceType,
    pub index:         usize,
    pub details:       SequenceDetails,
    pub progress:      SequenceStatus,
}

pub enum SequenceType {
    Validation,
    Initialization,
    Processing,
}
#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, HashMap},
        path::PathBuf,
        sync::{atomic::AtomicBool, Arc},
    };

    use av_decoders::ModifyNode;
    use vapoursynth::format::PresetFormat;

    use crate::{
        core::{
            input::Input,
            output::Output,
            sequence::{
                parallel_encoder::ParallelEncoder,
                quality_check::QualityCheck,
                scene_concatenator::SceneConcatenator,
                scene_detect::SceneDetector,
                target_quality::TargetQuality,
                Sequence,
                SequenceCompletion,
                SequenceStatus,
                Status,
            },
            AndeanCondor,
            Condor,
            DefaultAndeanCondor,
            SequenceProgressEvent,
            SequenceType,
        },
        models::{
            encoder::{
                cli_parameter::CLIParameter,
                photon_noise::PhotonNoise,
                Encoder,
                EncoderBase,
            },
            input::{Input as InputModel, VapourSynthImportMethod, VapourSynthScriptSource},
            output::Output as OutputModel,
            scene::Scene,
            sequence::{
                scene_concatenate::ConcatMethod,
                scene_detect::SceneDetectConfig,
                DefaultSequenceConfig,
                DefaultSequenceData,
            },
        },
        vapoursynth::{
            plugins::{
                dgdecodenv::DGSource,
                rescale::{ArtCNNModel, BorderHandling, Doubler, RescaleBuilder, VSJETKernel},
                resize::bilinear::Bilinear,
            },
            script_builder::{script::VapourSynthScript, VapourSynthPluginScript},
        },
    };

    #[test]
    fn instantiate_condor() {
        let input_data = InputModel::VapourSynth {
            path:          PathBuf::from("C:/Condor/sample.mkv"),
            import_method: VapourSynthImportMethod::DGDecNV {
                dgindexnv_executable: None,
            },
            cache_path:    None,
        };
        let mut scd_script = VapourSynthScript::default();
        let scd_script = {
            let (_import_lines, dg_lines) = DGSource::new(&PathBuf::from("C:/Condor/OP.dgi"))
                .generate_script("clip".to_owned())
                .expect("generated script");
            let (_import_lines, bilinear_lines) = Bilinear {
                width: Some(960),
                height: Some(540),
                ..Default::default()
            }
            .generate_script("clip".to_owned())
            .expect("generated script");
            scd_script.add_lines(dg_lines);
            scd_script.add_lines(bilinear_lines);
            let mut output_lines = BTreeMap::new();
            output_lines.insert(0, "clip".to_owned());
            scd_script.add_outputs(output_lines);
            scd_script
        };
        // println!("Generated Script: {}", scd_script);
        let scd_input_data = InputModel::VapourSynthScript {
            // source:    VapourSynthScriptSource::Path(PathBuf::from("C:/Condor/loadscript.vpy")),
            source:    VapourSynthScriptSource::Text(scd_script.to_string()),
            index:     0,
            variables: HashMap::new(),
        };
        // let modify_node_scd: ModifyNode = Box::new(|core, node| {
        //     let node = node.expect("node exists");
        //     let bilinear = Bilinear {
        //         width: Some(960),
        //         height: Some(540),
        //         // format: Some(PresetFormat::YUV420P10),
        //         ..Default::default()
        //     };
        //     let node = bilinear.invoke(core, &node).expect("Video downscaled");
        //     Ok(node)
        // });
        let modify_node: ModifyNode = Box::new(|core, node| {
            let node = node.expect("node exists");
            let bilinear = Bilinear {
                // width: Some(1280),
                // height: Some(720),
                format: Some(PresetFormat::YUV420P10),
                ..Default::default()
            };
            let node: vapoursynth::prelude::Node<'_> =
                bilinear.invoke(core, &node).expect("Video downscaled");

            Ok(node)
        });
        DGSource::index_video(&PathBuf::from("C:/Condor/sample.dgi"), None, None)
            .expect("indexed video");
        let input = Input::from_vapoursynth(&input_data, Some(modify_node)).expect("Input exists");
        let scd_input = Input::from_vapoursynth(&scd_input_data, None).expect("Input exists");
        // let scd_input = Input::from_vapoursynth(&scd_input_data,
        // Some(modify_node_scd)).expect("Input exists");
        let scenes_directory = PathBuf::from("C:/Condor/scenes");
        let output_data = OutputModel {
            path:       PathBuf::from("C:/Condor/test.mkv"),
            tags:       HashMap::new(),
            video_tags: HashMap::new(),
        };
        let output = Output::new(&output_data).expect("Output is correct");
        let mut svt_options = EncoderBase::SVTAV1.default_parameters();
        svt_options.extend(CLIParameter::new_numbers("--", " ", &[
            ("keyint", 0.0),
            ("preset", 8.0),
            ("crf", 35.0),
            ("lp", 6.0),
            ("tune", 2.0),
            // ("invalid-test", 12.0),
        ]));
        let encoder = Encoder::SVTAV1 {
            executable:   None,
            pass:         EncoderBase::SVTAV1.default_passes(),
            options:      svt_options.clone(),
            photon_noise: None,
        };

        let scd = SceneDetector::with_input(scd_input, None).expect("Scene Detector is correct");
        let scd_method = scd.method;
        let sequences: Vec<Box<dyn Sequence<DefaultSequenceData, DefaultSequenceConfig>>> = vec![
            Box::new(scd) as Box<dyn Sequence<DefaultSequenceData, DefaultSequenceConfig>>,
            Box::new(TargetQuality::default())
                as Box<dyn Sequence<DefaultSequenceData, DefaultSequenceConfig>>,
            Box::new(ParallelEncoder::new(2, &scenes_directory))
                as Box<dyn Sequence<DefaultSequenceData, DefaultSequenceConfig>>,
            Box::new(SceneConcatenator::new(
                &scenes_directory,
                ConcatMethod::MKVMerge,
            )) as Box<dyn Sequence<DefaultSequenceData, DefaultSequenceConfig>>,
            Box::new(QualityCheck::default())
                as Box<dyn Sequence<DefaultSequenceData, DefaultSequenceConfig>>,
        ];

        let scenes = {
            let ranges = [
                (0, 270),
                (270, 578),
                (578, 767),
                (767, 952),
                (952, 1306),
                (1306, 1366),
                (1366, 1579),
                (1579, 1802),
                (1802, 1826),
                (1826, 1899),
                (1899, 1927),
                (1927, 2011),
                (2011, 2072),
                (2072, 2162),
            ];
            let mut scenes: Vec<Scene<DefaultSequenceData>> = vec![];
            for (index, (start, end)) in ranges.iter().enumerate() {
                let encoder = Encoder::SVTAV1 {
                    executable:   None,
                    pass:         EncoderBase::SVTAV1.default_passes(),
                    options:      svt_options.clone(),
                    photon_noise: Some(PhotonNoise {
                        iso:        (index as u32).saturating_mul(100000),
                        chroma_iso: Some((index as u32).saturating_mul(100000)),
                        width:      None,
                        height:     None,
                        c_y:        None,
                        ccb:        None,
                        ccr:        None,
                    }),
                };
                scenes.push(Scene {
                    encoder,
                    start_frame: *start,
                    end_frame: *end,
                    sub_scenes: None,
                    processing: DefaultSequenceData::default(),
                });
            }
            scenes
        };

        let condor = Condor {
            input,
            output,
            encoder,
            scenes,
            save_callback: Box::new(|data| {
                Condor::save_data(&PathBuf::from("C:/Condor/condor.json"), &data)?;
                Ok(())
            }),
            processor_config: DefaultSequenceConfig {
                scene_detection: SceneDetectConfig {
                    input:  Some(scd_input_data),
                    method: scd_method,
                },
                ..Default::default()
            },
        };

        let mut andean_condor = DefaultAndeanCondor {
            condor,
            sequences,
        };

        let (event_tx, event_rx) = std::sync::mpsc::channel::<SequenceProgressEvent>();
        let scd_details = SceneDetector::DETAILS;
        let encoder_details = ParallelEncoder::DETAILS;
        let progress_thread = std::thread::spawn(move || -> anyhow::Result<()> {
            let mut last_print_time = std::time::Instant::now();
            for event in event_rx {
                // Scene Detector
                if event.details.name == scd_details.name {
                    match event.sequence_type {
                        SequenceType::Validation => (),
                        SequenceType::Initialization => {
                            if let SequenceStatus::Whole(status) = &event.progress {
                                match status {
                                    Status::Processing {
                                        completion, ..
                                    } => {
                                        #[allow(clippy::collapsible_match)]
                                        if let SequenceCompletion::Custom {
                                            name,
                                            completed,
                                            total,
                                        } = completion
                                        {
                                            println!(
                                                "Scene Detector Initialization: {} - {}/{}",
                                                name,
                                                completed + 1.0,
                                                total
                                            );
                                        }
                                    },
                                    Status::Completed {
                                        ..
                                    } => {
                                        println!("Scene Detector Initialization: Completed");
                                    },
                                    _ => (),
                                };
                            }
                        },
                        SequenceType::Processing => {
                            if let SequenceStatus::Whole(status) = &event.progress {
                                match status {
                                    Status::Processing {
                                        completion, ..
                                    } => {
                                        #[allow(clippy::collapsible_match)]
                                        if let SequenceCompletion::Frames {
                                            completed,
                                            total,
                                        } = completion
                                        {
                                            if last_print_time.elapsed().as_millis() > 1000 {
                                                print!(
                                                    "\rScene Detector: {}/{}",
                                                    completed + 1,
                                                    total
                                                );
                                                last_print_time = std::time::Instant::now();
                                            }
                                        }
                                    },
                                    Status::Completed {
                                        ..
                                    } => {
                                        println!("\nScene Detector: Completed");
                                    },
                                    _ => (),
                                }
                            };
                        },
                        // _ => (),
                    }
                }
                // Parallel Encoder
                if event.details.name == encoder_details.name {
                    match event.sequence_type {
                        SequenceType::Validation => (),
                        SequenceType::Initialization => (),
                        SequenceType::Processing => match event.progress {
                            SequenceStatus::Whole(status) => match status {
                                Status::Processing {
                                    id,
                                    completion,
                                } => {
                                    if id == encoder_details.name {
                                        if let SequenceCompletion::Frames {
                                            completed,
                                            total,
                                        } = completion
                                        {
                                            print!("\rParallel Encoder: {}/{}", completed, total);
                                        }
                                    }
                                },
                                Status::Completed {
                                    ..
                                } => {
                                    println!("\nParallel Encoder Done");
                                },
                                _ => unimplemented!(),
                            },
                            _ => (),
                        },
                    }
                }
            }
            Ok(())
        });

        let cancelled = Arc::new(AtomicBool::new(false));
        let cancelled_clone = Arc::clone(&cancelled);
        let ct_thread = std::thread::spawn(move || -> anyhow::Result<()> {
            std::thread::sleep(std::time::Duration::from_secs(13));
            // cancelled_clone.store(true, std::sync::atomic::Ordering::Relaxed);
            Ok(())
        });

        let results = andean_condor
            .process_all(event_tx, cancelled)
            .expect("SuperCondor finished Processing");
        let _ = progress_thread.join().expect("Progress thread joined");
        let _ = ct_thread.join().expect("CT thread joined");
        for (index, ((), (validation_warnings, initialization_warnings, process_warnings))) in
            results.iter().enumerate()
        {
            let no_warnings = validation_warnings.is_empty()
                && initialization_warnings.is_empty()
                && process_warnings.is_empty();
            println!(
                "PROCESS: {} - {} COMPLETED{}",
                index,
                andean_condor.sequences[index].details().name,
                if no_warnings { "" } else { " WITH WARNINGS:" }
            );
            for warning in validation_warnings {
                println!("VALIDATION: {}", warning);
            }
            for warning in initialization_warnings {
                println!("INITIALIZATION: {}", warning);
            }
            for warning in process_warnings {
                println!("PROCESSING: {}", warning);
            }
        }
    }

}
