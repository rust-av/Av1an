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
            parallel_encode::{
                ParallelEncodeConfig,
                ParallelEncodeData,
                ParallelEncodeDataHandler,
                DEFAULT_MAX_SCENE_LENGTH_SECONDS,
            },
            scene_concatenate::SceneConcatenateConfig,
            scene_detect::{
                SceneDetectConfig,
                SceneDetectData,
                SceneDetectDataHandler,
                SceneDetectionMethod,
                ScenecutMethod,
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
    pub fn new(input: &Path, output: &Path, temp: &Path) -> Result<Self> {
        let input_data = InputModel::VapourSynth {
            path:          input.to_path_buf(),
            import_method: VapourSynthImportMethod::BestSource {
                index: None
            },
            cache_path:    None,
        };
        info!("Indexing input...");
        let mut input_instance = Input::from_data(&input_data)?;
        let clip_info = input_instance.clip_info()?;
        let fps = *clip_info.frame_rate.numer() as f64 / *clip_info.frame_rate.denom() as f64;

        let mut configuration = Self {
            condor:            CondorModel {
                input:            input_data,
                output:           OutputModel {
                    path:       output.to_path_buf(),
                    tags:       HashMap::new(),
                    video_tags: HashMap::new(),
                },
                encoder:          Encoder::default(),
                scenes:           Vec::new(),
                sequence_config: CliSequenceConfig {
                    scene_detection:     SceneDetectConfig {
                        input:  None,
                        method: SceneDetectionMethod::AVSceneChange {
                            minimum_length: fps.round() as usize,
                            maximum_length: DEFAULT_MAX_SCENE_LENGTH_SECONDS as usize
                                * fps.round() as usize,
                            method:         ScenecutMethod::Standard,
                        },
                    },
                    parallel_encoder:    ParallelEncodeConfig {
                        workers:          1,
                        scenes_directory: temp.join("scenes"),
                        input:            None,
                        encoder:          None,
                    },
                    scene_concatenation: SceneConcatenateConfig::default(),
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
            processor_config: self.condor.sequence_config.clone(),
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliSequenceConfig
where
    Self: SequenceConfigHandler,
{
    pub scene_detection:     SceneDetectConfig,
    pub parallel_encoder:    ParallelEncodeConfig,
    pub scene_concatenation: SceneConcatenateConfig,
    // pub target_quality:  Option<TargetQualityConfig>,
}

impl Default for CliSequenceConfig {
    #[inline]
    fn default() -> Self {
        Self {
            scene_detection:     SceneDetectConfig::default(),
            parallel_encoder:    ParallelEncodeConfig::default(),
            scene_concatenation: SceneConcatenateConfig::default(),
            // target_quality:  None,
        }
    }
}

impl SequenceConfigHandler for CliSequenceConfig {
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliSequenceData
where
    Self: SequenceDataHandler + SceneDetectDataHandler + ParallelEncodeDataHandler, /* 
                                                                                     * + TargetQualityDataHandler
                                                                                     * + QualityCheckDataHandler, */
{
    pub scene_detection:  SceneDetectData,
    pub parallel_encoder: ParallelEncodeData, /* pub target_quality:  TargetQualityData,
                                               * pub quality_check:   QualityCheckData, */
}

impl SequenceDataHandler for CliSequenceData {
}

impl Default for CliSequenceData {
    #[inline]
    fn default() -> Self {
        Self {
            scene_detection:  SceneDetectData::default(),
            parallel_encoder: ParallelEncodeData::default(),
            // target_quality:  TargetQualityData::default(),
            // quality_check:   QualityCheckData::default(),
        }
    }
}

impl SceneDetectDataHandler for CliSequenceData {
    #[inline]
    fn get_scene_detection(&self) -> Result<&SceneDetectData> {
        Ok(&self.scene_detection)
    }

    #[inline]
    fn get_scene_detection_mut(&mut self) -> Result<&mut SceneDetectData> {
        Ok(&mut self.scene_detection)
    }
}

impl ParallelEncodeDataHandler for CliSequenceData {
    fn get_parallel_encode(&self) -> Result<&ParallelEncodeData> {
        Ok(&self.parallel_encoder)
    }

    fn get_parallel_encode_mut(&mut self) -> Result<&mut ParallelEncodeData> {
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
