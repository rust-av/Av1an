use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use andean_condor::{
    core::{
        input::{DecoderError, Input, ModifyNode},
        output::Output,
        Condor,
        SaveCallback,
    },
    ffmpeg::FFPixelFormat,
    models::{
        encoder::{Encoder, EncoderBase},
        input::{Input as InputModel, VapourSynthImportMethod, VapourSynthScriptSource},
        output::Output as OutputModel,
        sequence::{
            benchmarker::{BenchmarkerConfig, BenchmarkerConfigHandler},
            parallel_encoder::{
                ParallelEncoderConfig,
                ParallelEncoderConfigHandler,
                ParallelEncoderData,
                ParallelEncoderDataHandler,
            },
            scene_concatenator::{SceneConcatenatorConfig, SceneConcatenatorConfigHandler},
            scene_detector::{
                SceneDetectionMethod,
                SceneDetectorConfig,
                SceneDetectorData,
                SceneDetectorDataHandler,
                ScenecutMethod,
                DEFAULT_MAX_SCENE_LENGTH_SECONDS,
            },
            SequenceConfigHandler,
            SequenceDataHandler,
        },
        Condor as CondorModel,
    },
    vapoursynth::{
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
use anyhow::Result;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::info;

use crate::commands::DecoderMethod;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Configuration {
    pub condor:            CondorModel<CliSequenceData, CliSequenceConfig>,
    // Duplicated in case Condor instantiates a VapourSynthScript Input
    pub input:             PathBuf,
    pub temp:              PathBuf,
    pub input_filters:     Vec<VapourSynthFilter>,
    pub scd_input_filters: Vec<VapourSynthFilter>,
    // pub tq_input_filters:  Vec<VapourSynthFilter>,
}

impl Configuration {
    #[inline]
    pub fn new(
        input: &Path,
        output: &Path,
        temp: &Path,
        vs_args: Option<&[String]>,
    ) -> Result<Self> {
        let input_data = Self::new_input_model(input, Some(&DecoderMethod::BestSource), vs_args)?;
        info!("Indexing input...");
        let mut input_instance = Input::from_data(&input_data)?;
        let clip_info = input_instance.clip_info()?;
        let fps = *clip_info.frame_rate.numer() as f64 / *clip_info.frame_rate.denom() as f64;

        let scenes_directory = temp.join("scenes");

        let mut configuration = Self {
            condor:            CondorModel {
                input:           input_data,
                output:          OutputModel {
                    path:       output.to_path_buf(),
                    tags:       HashMap::new(),
                    video_tags: HashMap::new(),
                },
                encoder:         Encoder::default(),
                scenes:          Vec::new(),
                sequence_config: CliSequenceConfig {
                    scene_detection:     SceneDetectorConfig {
                        input:  None,
                        method: SceneDetectionMethod::AVSceneChange {
                            minimum_length: fps.round() as usize,
                            maximum_length: DEFAULT_MAX_SCENE_LENGTH_SECONDS as usize
                                * fps.round() as usize,
                            method:         ScenecutMethod::Standard,
                        },
                    },
                    benchmarker:         BenchmarkerConfig::default(),
                    parallel_encoder:    ParallelEncoderConfig {
                        workers:          None,
                        scenes_directory: scenes_directory.clone(),
                        input:            None,
                    },
                    scene_concatenation: SceneConcatenatorConfig::new(&scenes_directory),
                },
            },
            input:             input.to_path_buf(),
            temp:              temp.to_path_buf(),
            input_filters:     Vec::from(&[VapourSynthFilter::Resize {
                scaler: Some(Scaler::Bicubic),
                width:  None,
                height: None,
                format: Some(FFPixelFormat::YUV420P10LE),
            }]),
            scd_input_filters: Vec::new(),
        };

        *configuration.condor.encoder.parameters_mut() = EncoderBase::SVTAV1.default_parameters();

        Ok(configuration)
    }

    #[inline]
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        Self::save_data(self, path)?;
        Ok(())
    }

    #[inline]
    pub fn save_data(data: &Configuration, path: &Path) -> Result<(), ConfigError> {
        let mut buffer = vec![];
        let formatter = serde_json::ser::PrettyFormatter::with_indent(b"    ");
        let mut serializer = serde_json::Serializer::with_formatter(&mut buffer, formatter);
        data.serialize(&mut serializer).map_err(ConfigError::Serialize)?;
        let directory = path.parent();
        if let Some(directory) = directory {
            std::fs::create_dir_all(directory).map_err(ConfigError::Save)?;
        }
        std::fs::write(path, buffer).map_err(ConfigError::Save)?;
        Ok(())
    }

    #[inline]
    pub fn load(config_path: &Path) -> Result<Option<Configuration>, ConfigError> {
        if !config_path.exists() {
            return Ok(None);
        }
        let data = std::fs::read_to_string(config_path)
            .map_err(|_| ConfigError::Load(config_path.to_path_buf()))?;
        let data = serde_json::from_str(&data)
            .map_err(|_| ConfigError::Load(config_path.to_path_buf()))?;

        Ok(Some(data))
    }

    #[inline]
    pub fn instantiate_condor(
        &self,
        save_callback: SaveCallback<CliSequenceData, CliSequenceConfig>,
    ) -> Result<Condor<CliSequenceData, CliSequenceConfig>> {
        // let input = Self::instantiate_input_with_filters(&self.condor.input,
        // &self.input_filters)?;
        let input = {
            if matches!(&self.condor.input, InputModel::Video { .. }) {
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
            sequence_config: self.condor.sequence_config.clone(),
            save_callback,
        };

        Ok(condor)
    }

    #[inline]
    pub fn instantiate_input_with_filters(
        input_data: &InputModel,
        filters: &[VapourSynthFilter],
    ) -> Result<Input> {
        let input = {
            match input_data {
                InputModel::Video {
                    ..
                } => Input::from_video(input_data)?,
                InputModel::VapourSynth {
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
                    let script_input_data = InputModel::VapourSynthScript {
                        source:    VapourSynthScriptSource::Text(script.to_string()),
                        variables: HashMap::new(),
                        index:     SCRIPT_OUTPUT_INDEX,
                    };

                    Input::from_vapoursynth(&script_input_data, None)?
                },
                InputModel::VapourSynthScript {
                    source,
                    variables,
                    index,
                } => Input::from_data(input_data)?,
            }
        };

        Ok(input)
    }

    #[inline]
    pub fn new_input_model(
        input: &Path,
        decoder: Option<&DecoderMethod>,
        vs_args: Option<&[String]>,
    ) -> Result<InputModel> {
        let input_model = match decoder {
            Some(DecoderMethod::FFMS2) => InputModel::Video {
                path:          input.to_path_buf(),
                import_method: andean_condor::models::input::ImportMethod::FFMS2 {},
            },
            Some(method) => match method {
                DecoderMethod::FFMS2 => unreachable!(),
                DecoderMethod::BestSource => Self::new_vs_input_model(
                    input,
                    Some(VapourSynthImportMethod::BestSource {
                        index: None
                    }),
                    vs_args,
                )?,
                DecoderMethod::DGDecodeNV => Self::new_vs_input_model(
                    input,
                    Some(VapourSynthImportMethod::DGDecNV {
                        dgindexnv_executable: None,
                    }),
                    vs_args,
                )?,
                DecoderMethod::LSMASHWorks => Self::new_vs_input_model(
                    input,
                    Some(VapourSynthImportMethod::LSMASHWorks {
                        index: None
                    }),
                    vs_args,
                )?,
                DecoderMethod::VSFFMS2 => Self::new_vs_input_model(
                    input,
                    Some(VapourSynthImportMethod::FFMS2 {
                        index: None
                    }),
                    vs_args,
                )?,
            },
            None => Self::new_vs_input_model(
                input,
                Some(VapourSynthImportMethod::BestSource {
                    index: None
                }),
                vs_args,
            )?,
        };

        Ok(input_model)
    }

    #[inline]
    pub fn new_vs_input_model(
        input: &Path,
        decoder: Option<VapourSynthImportMethod>,
        vs_args: Option<&[String]>,
    ) -> Result<InputModel> {
        let input_is_script = input
            .extension()
            .map(|s| s.to_str())
            .is_some_and(|s| s.is_some_and(|extension| matches!(extension, "vpy" | "py")));
        let input_data = if input_is_script {
            let variables = vs_args.map_or_else(HashMap::new, |vs_args| {
                vs_args
                    .iter()
                    .map(|arg| {
                        let (key, value) = arg.split_once('=').unwrap_or((arg, ""));
                        (key.to_string(), value.to_string())
                    })
                    .collect()
            });
            InputModel::VapourSynthScript {
                source: VapourSynthScriptSource::Path(input.to_path_buf()),
                variables,
                index: 0,
            }
        } else {
            InputModel::VapourSynth {
                path:          input.to_path_buf(),
                import_method: decoder.map_or(
                    VapourSynthImportMethod::BestSource {
                        index: None
                    },
                    |decoder| decoder,
                ),
                cache_path:    None,
            }
        };

        Ok(input_data)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliSequenceConfig
where
    Self: SequenceConfigHandler
        + BenchmarkerConfigHandler
        + ParallelEncoderConfigHandler
        + SceneConcatenatorConfigHandler,
{
    pub scene_detection:     SceneDetectorConfig,
    pub benchmarker:         BenchmarkerConfig,
    pub parallel_encoder:    ParallelEncoderConfig,
    pub scene_concatenation: SceneConcatenatorConfig,
    // pub target_quality:  Option<TargetQualityConfig>,
}

impl Default for CliSequenceConfig {
    #[inline]
    fn default() -> Self {
        Self {
            scene_detection:     SceneDetectorConfig::default(),
            benchmarker:         BenchmarkerConfig::default(),
            parallel_encoder:    ParallelEncoderConfig::default(),
            scene_concatenation: SceneConcatenatorConfig::default(),
            // target_quality:  None,
        }
    }
}

impl SequenceConfigHandler for CliSequenceConfig {
}

impl BenchmarkerConfigHandler for CliSequenceConfig {
    fn benchmarker(&self) -> Result<&BenchmarkerConfig> {
        Ok(&self.benchmarker)
    }

    fn benchmarker_mut(&mut self) -> Result<&mut BenchmarkerConfig> {
        Ok(&mut self.benchmarker)
    }
}

impl ParallelEncoderConfigHandler for CliSequenceConfig {
    fn parallel_encoder(&self) -> Result<&ParallelEncoderConfig> {
        Ok(&self.parallel_encoder)
    }

    fn parallel_encoder_mut(&mut self) -> Result<&mut ParallelEncoderConfig> {
        Ok(&mut self.parallel_encoder)
    }
}

impl SceneConcatenatorConfigHandler for CliSequenceConfig {
    fn scene_concatenator(&self) -> Result<&SceneConcatenatorConfig> {
        Ok(&self.scene_concatenation)
    }

    fn scene_concatenator_mut(&mut self) -> Result<&mut SceneConcatenatorConfig> {
        Ok(&mut self.scene_concatenation)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliSequenceData
where
    Self: SequenceDataHandler + SceneDetectorDataHandler + ParallelEncoderDataHandler, /* 
                                                                                        * + TargetQualityDataHandler
                                                                                        * + QualityCheckDataHandler, */
{
    pub scene_detection:  SceneDetectorData,
    pub parallel_encoder: ParallelEncoderData, /* pub target_quality:  TargetQualityData,
                                                * pub quality_check:   QualityCheckData, */
}

impl SequenceDataHandler for CliSequenceData {
}

impl Default for CliSequenceData {
    #[inline]
    fn default() -> Self {
        Self {
            scene_detection:  SceneDetectorData::default(),
            parallel_encoder: ParallelEncoderData::default(),
            // target_quality:  TargetQualityData::default(),
            // quality_check:   QualityCheckData::default(),
        }
    }
}

impl SceneDetectorDataHandler for CliSequenceData {
    #[inline]
    fn get_scene_detection(&self) -> Result<&SceneDetectorData> {
        Ok(&self.scene_detection)
    }

    #[inline]
    fn get_scene_detection_mut(&mut self) -> Result<&mut SceneDetectorData> {
        Ok(&mut self.scene_detection)
    }
}

impl ParallelEncoderDataHandler for CliSequenceData {
    fn get_parallel_encoder(&self) -> Result<&ParallelEncoderData> {
        Ok(&self.parallel_encoder)
    }

    fn get_parallel_encoder_mut(&mut self) -> Result<&mut ParallelEncoderData> {
        Ok(&mut self.parallel_encoder)
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
    Load(PathBuf),
    #[error("Failed to serialize config file: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("Failed to save config file: {0}")]
    Save(#[from] std::io::Error),
}
