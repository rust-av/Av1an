use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::Result;
use av1an_core::{
    condor::{
        core::{
            input::{DecoderError, Input, ModifyNode},
            output::Output,
            processors::{parallel_encoder::ParallelEncoder, scene_detector::SceneDetector},
            Condor,
            SaveCallback,
        },
        data::{
            encoding::Encoder,
            input::{Input as InputData, VapourSynthImportMethod},
            output::Output as OutputData,
            processing::{
                parallel_encoder::{ParallelEncoderConfig, ParallelEncoderProcessing},
                scene_detection::{
                    SceneDetectionData,
                    SceneDetectionDataHandler,
                    SceneDetectionMethod,
                    SceneDetectionProcessing,
                    SceneDetectorConfig,
                    DEFAULT_MAX_SCENE_LENGTH_SECONDS,
                },
                BaseProcessDataTrait,
                BaseProcessingTrait,
                BaseProcessorConfigTrait,
            },
            Condor as CondorData,
        },
    },
    vs::{
        plugins::{
            bestsource::VideoSource,
            dgdecodenv::DGSource,
            ffms2::Source,
            lsmash::LWLibavSource,
            resize::Scaler,
        },
        script_builder::{script::VapourSynthScript, VapourSynthPluginScript},
        vapoursynth_filters::VapourSynthFilter,
    },
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Configuration {
    pub condor:            CondorData<CliProcessData, CliProcessorConfig>,
    // Duplicated in case Condor instantiates a VapourSynthScript Input
    pub input:             PathBuf,
    pub temp:              PathBuf,
    pub input_filters:     Vec<VapourSynthFilter>,
    pub scd_input_filters: Vec<VapourSynthFilter>,
    // pub tq_input_filters:  Vec<VapourSynthFilter>,
}

impl Configuration {
    #[inline]
    pub fn new(input: &Path, output: &Path, temp: &Path) -> Result<Self> {
        let input_data = InputData::VapourSynth {
            path:          input.to_path_buf(),
            import_method: VapourSynthImportMethod::BestSource {
                index: None
            },
            cache_path:    None,
        };
        println!("Indexing input...");
        let mut input_instance = Input::from_data(&input_data)?;
        let clip_info = input_instance.clip_info()?;
        let fps = *clip_info.frame_rate.numer() as f64 / *clip_info.frame_rate.denom() as f64;

        let configuration = Self {
            condor: CondorData {
                input: input_data,
                output: OutputData {
                    path: output.to_path_buf(),
                    concatenation_method: av1an_core::ConcatMethod::MKVMerge,
                    tags: HashMap::new(),
                    video_tags: HashMap::new(),
                },
                encoder: Encoder::default(),
                scenes: Vec::new(),
                processor_config: CliProcessorConfig {
                    scene_detection: SceneDetectorConfig {
                        input: None,
                        method: av1an_core::condor::data::processing::scene_detection::SceneDetectionMethod::AVSceneChange { minimum_length: fps.round() as usize, maximum_length: DEFAULT_MAX_SCENE_LENGTH_SECONDS as usize * fps.round() as usize, method: av1an_core::ScenecutMethod::Standard },
                    },
                    parallel_encoder: ParallelEncoderConfig {
                        workers: 1,
                        scenes_directory: temp.join("scenes"),
                        input: None,
                        encoder: None
                    },
                },
            },
            input: input.to_path_buf(),
            temp: temp.to_path_buf(),
            input_filters: Vec::from(&[VapourSynthFilter::Resize { scaler: Some(Scaler::Bicubic), width: None, height: None, format: Some(av1an_core::ffmpeg::FFPixelFormat::YUV420P10LE) }]),
            scd_input_filters: Vec::new(),
        };

        Ok(configuration)
    }

    #[inline]
    pub fn save(&self, path: &Path) -> Result<()> {
        let mut buffer = vec![];
        let formatter = serde_json::ser::PrettyFormatter::with_indent(b"    ");
        let mut serializer = serde_json::Serializer::with_formatter(&mut buffer, formatter);
        self.serialize(&mut serializer)?;
        std::fs::write(path, buffer)?;
        Ok(())
    }

    #[inline]
    pub fn save_data(data: &Configuration, path: &Path) -> Result<()> {
        let mut buffer = vec![];
        let formatter = serde_json::ser::PrettyFormatter::with_indent(b"    ");
        let mut serializer = serde_json::Serializer::with_formatter(&mut buffer, formatter);
        data.serialize(&mut serializer)?;
        std::fs::write(path, buffer)?;
        Ok(())
    }

    #[inline]
    pub fn load(config_path: &Path) -> Result<Option<Configuration>> {
        if !config_path.exists() {
            return Ok(None);
        }
        let data = std::fs::read_to_string(config_path)
            .map_err(|_| ConfigError::ConfigLoadError(config_path.to_path_buf()))?;
        let data = serde_json::from_str(&data)
            .map_err(|_| ConfigError::ConfigLoadError(config_path.to_path_buf()))?;

        Ok(Some(data))
    }

    #[inline]
    pub fn instantiate_condor(
        &self,
        save_callback: SaveCallback<CliProcessData, CliProcessorConfig>,
    ) -> Result<Condor<CliProcessData, CliProcessorConfig>> {
        // let input = Self::instantiate_input_with_filters(&self.condor.input,
        // &self.input_filters)?;
        let input = {
            if matches!(&self.condor.input, InputData::Video { .. }) {
                Input::from_video(&self.condor.input)?
            } else if self.input_filters.iter().any(|filter| filter.is_script_only()) {
                Self::instantiate_input_with_filters(&self.condor.input, &self.input_filters)?
            } else {
                let filters = self.input_filters.clone();
                let node_modifier: ModifyNode = Box::new(move |core, node| {
                    let mut node = node.expect("node exists");
                    for filter in &filters {
                        node = filter.invoke_plugin_function(core, &node).map_err(|e| {
                            DecoderError::VapoursynthScriptError {
                                cause: e.to_string(),
                            }
                        })?;
                    }

                    Ok(node)
                });
                Input::from_vapoursynth(&self.condor.input, Some(node_modifier))?
            }
        };
        let output = Output::new(&self.condor.output)?;

        let condor = Condor {
            input,
            output,
            encoder: self.condor.encoder.clone(),
            scenes: self.condor.scenes.clone(),
            processor_config: self.condor.processor_config.clone(),
            save_callback,
        };

        Ok(condor)
    }

    #[inline]
    pub fn instantiate_input_with_filters(
        input_data: &InputData,
        filters: &[VapourSynthFilter],
    ) -> Result<Input> {
        let input = {
            match input_data {
                InputData::Video {
                    ..
                } => Input::from_video(input_data)?,
                InputData::VapourSynth {
                    path,
                    import_method,
                    cache_path,
                } => {
                    const SCRIPT_OUTPUT_INDEX: u8 = 0;
                    const SCRIPT_NODE_NAME: &str = "clip";
                    let mut script = VapourSynthScript::default();
                    let script = {
                        let (dec_import_lines, dec_lines) = match import_method {
                            VapourSynthImportMethod::LSMASHWorks {
                                ..
                            } => LWLibavSource::new(path)
                                .generate_script(SCRIPT_NODE_NAME.to_owned())?,
                            VapourSynthImportMethod::DGDecNV {
                                ..
                            } => {
                                DGSource::new(path).generate_script(SCRIPT_NODE_NAME.to_owned())?
                            },
                            VapourSynthImportMethod::FFMS2 {
                                ..
                            } => Source::new(path).generate_script(SCRIPT_NODE_NAME.to_owned())?,
                            VapourSynthImportMethod::BestSource {
                                ..
                            } => VideoSource::new(path)
                                .generate_script(SCRIPT_NODE_NAME.to_owned())?,
                        };
                        if let Some(dec_import_lines) = dec_import_lines {
                            script.add_imports(dec_import_lines);
                        }
                        script.add_lines(dec_lines);
                        for filter in filters {
                            let (import_lines, filter_lines) =
                                filter.generate_script(SCRIPT_NODE_NAME.to_owned())?;

                            if let Some(import_lines) = import_lines {
                                script.add_imports(import_lines);
                            }
                            script.add_lines(filter_lines);
                        }

                        script.outputs.insert(SCRIPT_OUTPUT_INDEX, SCRIPT_NODE_NAME.to_owned());
                        script
                    };
                    let script_input_data = InputData::VapourSynthScript {
                        source:    av1an_core::condor::data::input::VapourSynthScriptSource::Text(
                            script.to_string(),
                        ),
                        variables: HashMap::new(),
                        index:     SCRIPT_OUTPUT_INDEX,
                    };

                    Input::from_vapoursynth(&script_input_data, None)?
                },
                InputData::VapourSynthScript {
                    source,
                    variables,
                    index,
                } => Input::from_data(input_data)?,
            }
        };

        Ok(input)
    }
}

pub struct CliProcessing
where
    Self: BaseProcessingTrait + SceneDetectionProcessing + ParallelEncoderProcessing, /* 
                                                                                       * + TargetQualityProcessing
                                                                                       * + QualityCheckProcessing, */
{
    pub scene_detection:  SceneDetector,
    pub parallel_encoder: ParallelEncoder,
    // pub target_quality:  Option<TargetQuality>,
}

impl Default for CliProcessing {
    #[inline]
    fn default() -> Self {
        Self {
            scene_detection:  SceneDetector::default(),
            parallel_encoder: ParallelEncoder::default(),
            // target_quality:  None,
        }
    }
}

impl BaseProcessingTrait for CliProcessing {
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliProcessorConfig
where
    Self: BaseProcessorConfigTrait,
{
    pub scene_detection:  SceneDetectorConfig,
    pub parallel_encoder: ParallelEncoderConfig,
    // pub target_quality:  Option<TargetQualityConfig>,
}

impl Default for CliProcessorConfig {
    #[inline]
    fn default() -> Self {
        Self {
            scene_detection:  SceneDetectorConfig::default(),
            parallel_encoder: ParallelEncoderConfig::default(),
            // target_quality:  None,
        }
    }
}

impl BaseProcessorConfigTrait for CliProcessorConfig {
}

impl SceneDetectionProcessing for CliProcessing {
    #[inline]
    fn get_scene_detection_method(&self) -> Result<SceneDetectionMethod> {
        Ok(self.scene_detection.method)
    }

    #[inline]
    fn get_scene_detection_method_mut(&mut self) -> Result<&mut SceneDetectionMethod> {
        Ok(&mut self.scene_detection.method)
    }

    #[inline]
    fn get_scene_detection_input_mut(&mut self) -> Result<&mut Option<Input>> {
        Ok(&mut self.scene_detection.input)
    }

    #[inline]
    fn set_scene_detection_input(&mut self, input: Input) -> Result<()> {
        self.scene_detection.input = Some(input);
        Ok(())
    }
}

impl ParallelEncoderProcessing for CliProcessing {
    fn get_workers(&self) -> Result<u8> {
        Ok(self.parallel_encoder.workers)
    }

    fn get_workers_mut(&mut self) -> Result<&mut u8> {
        Ok(&mut self.parallel_encoder.workers)
    }
}

// impl TargetQualityProcessing for DefaultProcessing {
//     #[inline]
//     fn target_quality_mut(&mut self) -> Result<&mut Option<TargetQuality>> {
//         Ok(&mut self.target_quality)
//     }
// }

// impl QualityCheckProcessing for DefaultProcessing {
//     #[inline]
//     fn metric(&self) -> Result<target_quality::QualityMetric> {
//         todo!()
//     }

//     #[inline]
//     fn metric_mut(&mut self) -> Result<&mut target_quality::QualityMetric> {
//         todo!()
//     }

//     #[inline]
//     fn input(&self) -> Result<Option<crate::condor::core::input::Input>> {
//         todo!()
//     }

//     #[inline]
//     fn input_mut(&mut self) -> Result<&mut
// Option<crate::condor::core::input::Input>> {         todo!()
//     }
// }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliProcessData
where
    Self: BaseProcessDataTrait + SceneDetectionDataHandler, /* 
                                                             * + TargetQualityDataHandler
                                                             * + QualityCheckDataHandler, */
{
    pub scene_detection: SceneDetectionData,
    // pub target_quality:  TargetQualityData,
    // pub quality_check:   QualityCheckData,
}

impl BaseProcessDataTrait for CliProcessData {
}

impl Default for CliProcessData {
    #[inline]
    fn default() -> Self {
        Self {
            scene_detection: SceneDetectionData::default(),
            // target_quality:  TargetQualityData::default(),
            // quality_check:   QualityCheckData::default(),
        }
    }
}

impl SceneDetectionDataHandler for CliProcessData {
    #[inline]
    fn get_scene_detection(&self) -> Result<SceneDetectionData> {
        Ok(self.scene_detection.clone())
    }

    #[inline]
    fn get_scene_detection_mut(&mut self) -> Result<&mut SceneDetectionData> {
        Ok(&mut self.scene_detection)
    }
}

// impl TargetQualityDataHandler for DefaultProcessData {
//     #[inline]
//     fn get_passes(&self) -> Result<&Vec<QualityPass>> {
//         Ok(&self.target_quality.passes)
//     }

//     #[inline]
//     fn get_passes_mut(&mut self) -> Result<&mut Vec<QualityPass>> {
//         Ok(&mut self.target_quality.passes)
//     }
// }

// impl QualityCheckDataHandler for DefaultProcessData {
//     #[inline]
//     fn quality(&self) -> Result<&QualityPass> {
//         Ok(&self.quality_check.quality)
//     }

//     #[inline]
//     fn quality_mut(&mut self) -> Result<&mut QualityPass> {
//         Ok(&mut self.quality_check.quality)
//     }
// }

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Failed to load config file: {0}")]
    ConfigLoadError(PathBuf),
}
