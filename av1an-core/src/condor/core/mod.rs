use std::{
    error::Error,
    path::Path,
    sync::{atomic::AtomicBool, Arc},
};

use anyhow::Result;
use serde::Serialize;

use crate::condor::{
    core::{
        input::Input,
        output::Output,
        processors::{ProcessStatus, ProcessorDetails, Processors},
    },
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

pub type SaveCallback<ProcessData, ProcessorConfig> =
    Box<dyn Fn(CondorData<ProcessData, ProcessorConfig>) -> Result<()>>;

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
    pub save_callback:    SaveCallback<ProcessData, ProcessorConfig>,
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
        save_callback: SaveCallback<ProcessData, ProcessorConfig>,
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
        let data = CondorData {
            input:            self.input.as_data(),
            output:           self.output.as_data(),
            encoder:          self.encoder.clone(),
            scenes:           self.scenes.clone(),
            processor_config: self.processor_config.clone(),
        };

        (self.save_callback)(data.clone())?;
        // Self::save_data(save_file, &data)?;

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
        condor::{
            core::{
                input::Input,
                output::Output,
                processors::{
                    parallel_encoder::ParallelEncoder,
                    quality_check::QualityCheck,
                    scene_concatenator::SceneConcatenator,
                    scene_detector::SceneDetector,
                    target_quality::TargetQuality,
                    ProcessCompletion,
                    ProcessStatus,
                    Processor,
                    Status,
                },
                Condor,
                CondorInstance,
                ProcessProgressEvent,
                ProcessType,
                SuperCondor,
            },
            data::{
                encoding::{
                    cli_parameter::CLIParameter,
                    photon_noise::PhotonNoise,
                    Encoder,
                    EncoderBase,
                },
                input::{Input as InputData, VapourSynthImportMethod, VapourSynthScriptSource},
                output::Output as OutputData,
                processing::{
                    scene_detection::SceneDetectorConfig,
                    DefaultProcessData,
                    DefaultProcessing,
                    DefaultProcessorConfig,
                },
                scene::Scene,
            },
        },
        vs::{
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
        let input_data = InputData::VapourSynth {
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
        let scd_input_data = InputData::VapourSynthScript {
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
        DGSource::index_video(&PathBuf::from("C:/Condor/OP.dgi"), None, None)
            .expect("indexed video");
        let input = Input::from_vapoursynth(&input_data, Some(modify_node)).expect("Input exists");
        let scd_input = Input::from_vapoursynth(&scd_input_data, None).expect("Input exists");
        // let scd_input = Input::from_vapoursynth(&scd_input_data,
        // Some(modify_node_scd)).expect("Input exists");
        let scenes_directory = PathBuf::from("C:/Condor/scenes");
        let output_data = OutputData {
            path:                 PathBuf::from("C:/Condor/test.mkv"),
            concatenation_method: crate::ConcatMethod::MKVMerge,
            tags:                 HashMap::new(),
            video_tags:           HashMap::new(),
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
        let processors: Vec<
            Box<dyn Processor<DefaultProcessing, DefaultProcessData, DefaultProcessorConfig>>,
        > = vec![
            Box::new(scd)
                as Box<
                    dyn Processor<DefaultProcessing, DefaultProcessData, DefaultProcessorConfig>,
                >,
            Box::new(TargetQuality::default())
                as Box<
                    dyn Processor<DefaultProcessing, DefaultProcessData, DefaultProcessorConfig>,
                >,
            Box::new(ParallelEncoder::new(2, &scenes_directory))
                as Box<
                    dyn Processor<DefaultProcessing, DefaultProcessData, DefaultProcessorConfig>,
                >,
            Box::new(SceneConcatenator::new(&scenes_directory))
                as Box<
                    dyn Processor<DefaultProcessing, DefaultProcessData, DefaultProcessorConfig>,
                >,
            Box::new(QualityCheck::default())
                as Box<
                    dyn Processor<DefaultProcessing, DefaultProcessData, DefaultProcessorConfig>,
                >,
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
            let mut scenes: Vec<Scene<DefaultProcessData>> = vec![];
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
                    processing: DefaultProcessData::default(),
                });
            }
            scenes
        };

        let condor = Condor {
            input,
            output,
            encoder,
            scenes,
            // save_file: Some(PathBuf::from("C:/Condor/condor.json")),
            save_callback: Box::new(|data| {
                Condor::save_data(&PathBuf::from("C:/Condor/condor.json"), &data)?;
                Ok(())
            }),
            processor_config: DefaultProcessorConfig {
                scene_detection: SceneDetectorConfig {
                    input:  Some(scd_input_data),
                    method: scd_method,
                },
                ..Default::default()
            },
        };

        let mut instance = SuperCondor {
            condor,
            processing: processors,
        };

        let (event_tx, event_rx) = std::sync::mpsc::channel::<ProcessProgressEvent>();
        let scd_details = SceneDetector::DETAILS;
        let encoder_details = ParallelEncoder::DETAILS;
        let progress_thread = std::thread::spawn(move || -> anyhow::Result<()> {
            let scd_pb = indicatif::ProgressBar::new(0);
            scd_pb.set_style(
                indicatif::ProgressStyle::default_bar()
                    .template(&format!(
                        "{} - {{elapsed}} {{wide_bar}} [{{pos}}/{{len}}] ({{percent:.2}}%) {{eta}}",
                        scd_details.name
                    ))
                    .expect("failed to create indicatif progress bar"),
            );
            let mut scd_pb_initialized = false;
            let enc_mpb = indicatif::MultiProgress::new();
            let enc_main_pb = enc_mpb.add(indicatif::ProgressBar::new(0));
            enc_main_pb.set_style(
                indicatif::ProgressStyle::default_bar()
                    .template(&format!(
                        "{} - {{elapsed}} {{wide_bar}} [{{pos}}/{{len}}] ({{percent:.2}}%) {{eta}}",
                        encoder_details.name
                    ))
                    .expect("failed to create indicatif progress bar"),
            );
            let mut enc_mpb_initialized = false;
            let mut enc_pb_map = BTreeMap::new();
            for event in event_rx {
                // Scene Detector
                if event.details.name == scd_details.name {
                    match event.process_type {
                        ProcessType::Validation => (),
                        ProcessType::Initialization => {
                            if let ProcessStatus::Whole(status) = &event.progress {
                                match status {
                                    Status::Processing {
                                        completion, ..
                                    } => {
                                        #[allow(clippy::collapsible_match)]
                                        if let ProcessCompletion::Custom {
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
                        ProcessType::Processing => {
                            if let ProcessStatus::Whole(status) = &event.progress {
                                match status {
                                    Status::Processing {
                                        completion, ..
                                    } => {
                                        #[allow(clippy::collapsible_match)]
                                        if let ProcessCompletion::Frames {
                                            completed,
                                            total,
                                        } = completion
                                        {
                                            if !scd_pb_initialized {
                                                scd_pb.set_length(*total);
                                                scd_pb_initialized = true;
                                            }
                                            scd_pb.set_position(*completed);
                                        }
                                    },
                                    Status::Completed {
                                        ..
                                    } => {
                                        scd_pb.finish();
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
                    match event.process_type {
                        ProcessType::Validation => (),
                        ProcessType::Initialization => (),
                        ProcessType::Processing => {
                            match event.progress {
                                ProcessStatus::Whole(status) => match status {
                                    Status::Processing {
                                        id,
                                        completion,
                                    } => {
                                        if id == encoder_details.name {
                                            if let ProcessCompletion::Frames {
                                                completed,
                                                total,
                                            } = completion
                                            {
                                                if !enc_mpb_initialized {
                                                    println!(
                                                        "Initializing multi progress bar with \
                                                         {}/{}",
                                                        completed, total
                                                    );
                                                    enc_main_pb.set_length(total);
                                                    enc_mpb_initialized = true;
                                                }
                                                enc_main_pb.set_position(completed);
                                            }
                                        }
                                    },
                                    Status::Completed {
                                        id,
                                    } => {
                                        if id == encoder_details.name {
                                            enc_main_pb.finish();
                                        }
                                    },
                                    _ => unimplemented!(),
                                },
                                ProcessStatus::Subprocess {
                                    child, ..
                                } => match child {
                                    Status::Processing {
                                        id,
                                        completion,
                                    } => {
                                        if let ProcessCompletion::PassFrames {
                                            passes,
                                            frames,
                                        } = completion
                                        {
                                            let (current_pass, total_passes) = passes;
                                            let (current_frame, total_frames) = frames;
                                            let pb =
                                                enc_pb_map.entry(id.clone()).or_insert_with(|| {
                                                    let pb = enc_mpb.insert_before(
                                                        &enc_main_pb,
                                                        indicatif::ProgressBar::new(total_frames),
                                                    );
                                                    pb.set_style(
                                                        indicatif::ProgressStyle::default_bar()
                                                            .template(&format!(
                                                                "Scene {} - Pass {}/{} - \
                                                                 {{elapsed}} {{wide_bar}} \
                                                                 [{{pos}}/{{len}}] \
                                                                 ({{percent:.2}}%) {{eta}}",
                                                                id, current_pass, total_passes
                                                            ))
                                                            .expect("template is valid"),
                                                    );
                                                    pb
                                                });
                                            if current_frame == 0 {
                                                // New pass, reset progress bar with new template
                                                pb.reset();
                                                pb.set_style(
                                                    indicatif::ProgressStyle::default_bar()
                                                        .template(&format!(
                                                            "Scene {} - Pass {}/{} - {{elapsed}} \
                                                             {{wide_bar}} [{{pos}}/{{len}}] \
                                                             ({{percent:.2}}%) {{eta}}",
                                                            id, current_pass, total_passes
                                                        ))
                                                        .expect("template is valid"),
                                                );
                                            }
                                            pb.set_position(current_frame);
                                            if current_pass == total_passes
                                                && current_frame == total_frames
                                            {
                                                pb.finish_and_clear();
                                            }
                                        }
                                    },
                                    Status::Completed {
                                        id,
                                    } => {
                                        if let Some(pb) = enc_pb_map.remove(&id) {
                                            pb.finish_and_clear();
                                        }
                                    },
                                    _ => unimplemented!(),
                                },
                            }
                        },
                        // _ => (),
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

        let results = instance
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
                instance.processing[index].details().name,
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

    #[test]
    fn rescale() {
        let video_path = PathBuf::from("C:/Condor/OP.mkv");
        let cache_path = PathBuf::from("C:/Condor/OP.dgi");

        let mut script = VapourSynthScript::default();
        let (_import_lines, dg_lines) = DGSource::new(&PathBuf::from("C:/Condor/OP.dgi"))
            .generate_script("clip".to_owned())
            .expect("generated dgdecnv script");
        let (_import_lines, _bilinear_lines) = Bilinear {
            width: Some(960),
            height: Some(540),
            ..Default::default()
        }
        .generate_script("clip".to_owned())
        .expect("generated bilinear script");
        let rescale_plugin = RescaleBuilder {
            width:            1280.0,
            height:           720.0,
            base_width:       None,
            base_height:      None,
            descale_kernel:   VSJETKernel::Bilinear {
                linear:          false,
                border_handling: BorderHandling::Repeat,
            },
            doubler:          Doubler::ArtCNN(ArtCNNModel::C4F32),
            downscale_kernel: VSJETKernel::Hermite {
                linear:          true,
                border_handling: BorderHandling::Mirror,
            },
            mode:             None,
        };
        let (rescale_imports, rescale_lines) = rescale_plugin
            .generate_script("clip".to_owned())
            .expect("generated rescale script");
        if let Some(rescale_import_lines) = rescale_imports {
            script.add_imports(rescale_import_lines);
        }
        script.add_lines(dg_lines);
        // script.add_lines(bilinear_lines);
        script.add_lines(rescale_lines);
        let mut output_lines = BTreeMap::new();
        output_lines.insert(0, "clip".to_owned());
        script.add_outputs(output_lines);
        println!("SCRIPT: \n{}", script);
        let input_data = InputData::VapourSynthScript {
            source:    VapourSynthScriptSource::Text(script.to_string()),
            index:     0,
            variables: HashMap::new(),
        };
        DGSource::index_video(&video_path, Some(&cache_path), None).expect("indexed video");
        let mut input = Input::from_vapoursynth(&input_data, None).expect("got input");
        let vs_decoder = input.decoder();
        // let mut vs_decoder =
        //     av_decoders::Decoder::from_decoder_impl(av_decoders::DecoderImpl::Vapoursynth(
        //         av_decoders::VapoursynthDecoder::new().expect("got vs decoder"),
        //     ))
        //     .expect("got decoder");
        // let env = vapoursynth::vsscript::Environment::new().expect("got vs
        // environment");
        let env = vs_decoder.get_vapoursynth_env().expect("got vs environment");
        let _core = env.get_core().expect("got vapoursynth core");
        // DGSource::index_video(&PathBuf::from("C:/Condor/OP.mkv"), None, None)
        //     .expect("indexed video");
        // let node = dg_plugin.invoke(core);
        // assert!(node.is_ok());
    }
}
