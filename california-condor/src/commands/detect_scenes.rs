use std::path::{Path, PathBuf};

use andean_condor::{
    core::input::Input,
    models::input::{
        ImportMethod,
        Input as InputModel,
        VapourSynthImportMethod,
        VapourSynthScriptSource,
    },
    vapoursynth::vapoursynth_filters::VapourSynthFilter,
};
use anyhow::{bail, Result};
use tracing::{debug, error, trace};

use crate::{
    commands::{DecoderMethod, SceneDetectionMethod},
    configuration::{ConfigError, Configuration},
    CondorCliError,
    DEFAULT_CONFIG_PATH,
    DEFAULT_TEMP_PATH,
};

#[allow(clippy::too_many_arguments)]
pub fn detect_scenes_handler(
    config_path: Option<&Path>,
    input_path: Option<&Path>,
    decoder: Option<&DecoderMethod>,
    filters: Option<&[VapourSynthFilter]>,
    vs_args: Option<&[String]>,
    method: Option<&SceneDetectionMethod>,
    min_scene_seconds: Option<usize>,
    max_scene_seconds: Option<usize>,
) -> Result<(Configuration, PathBuf)> {
    if config_path.is_some_and(|p| !p.exists()) && input_path.is_none() {
        bail!(CondorCliError::NoConfigOrInput);
    }
    let config_path =
        path_abs::PathAbs::new(config_path.unwrap_or_else(|| Path::new(DEFAULT_CONFIG_PATH)))?
            .as_path()
            .to_path_buf();
    let config_already_existed = config_path.exists();

    let mut configuration = {
        if config_already_existed {
            debug!("Loading existing configuration");
            match Configuration::load(&config_path) {
                Ok(config) => config.expect("Config should exist"),
                Err(err) => match err {
                    ConfigError::Load(path) => {
                        let err = CondorCliError::ConfigLoadError(path);
                        error!("{}", err);
                        bail!(err);
                    },
                    _ => unreachable!("ConfigError should be LoadError"),
                },
            }
        } else {
            trace!("No existing configuration found");
            let input_path = input_path.ok_or_else(|| {
                let err = CondorCliError::NoConfigOrInput;
                error!("{}", err);
                err
            })?;
            debug!("Creating new temporary configuration");
            let input = path_abs::PathAbs::new(input_path)?.as_path().to_path_buf();
            // Won't be used
            let output = input.with_file_name(format!(
                "{}.mkv",
                input.file_stem().expect("input is a file").display()
            ));
            let output = path_abs::PathAbs::new(output)?.as_path().to_path_buf();
            let cwd = std::env::current_dir()?;
            let temp_path = cwd.join(DEFAULT_TEMP_PATH);
            let temp = path_abs::PathAbs::new(temp_path)?.as_path().to_path_buf();
            Configuration::new(&input, &output, &temp, vs_args)?
        }
    };

    if let Some(decoder) = &decoder {
        let existing_input_path = match configuration.condor.input {
            InputModel::Video {
                path, ..
            } => Some(path),
            InputModel::VapourSynth {
                path, ..
            } => Some(path),
            InputModel::VapourSynthScript {
                source, ..
            } => match source {
                VapourSynthScriptSource::Path(source_path) => Some(source_path),
                _ => input_path.map(|p| p.to_path_buf()), // Default to provided input path
            },
        };
        let existing_input_path = existing_input_path.ok_or_else(|| {
            let err = CondorCliError::DecoderWithoutInput;
            error!("{}", err);
            err
        })?;
        let existing_input_path =
            path_abs::PathAbs::new(existing_input_path)?.as_path().to_path_buf();
        match decoder {
            DecoderMethod::FFMS2 => {
                configuration.condor.input = InputModel::Video {
                    path:          existing_input_path,
                    import_method: ImportMethod::FFMS2 {},
                };
            },
            vs_decoders => {
                configuration.condor.input = InputModel::VapourSynth {
                    path:          existing_input_path,
                    import_method: match vs_decoders {
                        DecoderMethod::BestSource => VapourSynthImportMethod::BestSource {
                            index: None,
                        },
                        DecoderMethod::VSFFMS2 => VapourSynthImportMethod::FFMS2 {
                            index: None
                        },
                        DecoderMethod::LSMASHWorks => VapourSynthImportMethod::LSMASHWorks {
                            index: None,
                        },
                        DecoderMethod::DGDecodeNV => VapourSynthImportMethod::DGDecNV {
                            dgindexnv_executable: None,
                        },
                        DecoderMethod::FFMS2 => unreachable!(),
                    },
                    cache_path:    None,
                };
            },
        };
    }
    if let Some(input) = input_path {
        configuration.condor.input = Configuration::new_input_model(
            path_abs::PathAbs::new(input)?.as_path(),
            decoder,
            vs_args,
        )?;
    }
    if let Some(filters) = filters {
        configuration.input_filters = filters.to_vec();
    }

    let mut input = Input::from_data(&configuration.condor.input)?;
    let clip_info = input.clip_info()?;
    let fps = *clip_info.frame_rate.numer() as f64 / *clip_info.frame_rate.denom() as f64;

    let previous_method = configuration.condor.sequence_config.scene_detection.method;
    let min_scene_frames = min_scene_seconds.map_or_else(
        || previous_method.minimum_length(),
        |seconds| (fps * seconds as f64).round() as usize,
    );
    let max_scene_frames = max_scene_seconds.map_or_else(
        || previous_method.maximum_length(),
        |seconds| (fps * seconds as f64).round() as usize,
    );
    let new_method =
        method.map(|method| method.as_core_method(Some(min_scene_frames), Some(max_scene_frames)));
    if let Some(new_method) = new_method {
        configuration.condor.sequence_config.scene_detection.method = new_method;
    }
    configuration
        .condor
        .sequence_config
        .scene_detection
        .method
        .set_minimum_length(min_scene_frames)?;
    configuration
        .condor
        .sequence_config
        .scene_detection
        .method
        .set_maximum_length(max_scene_frames)?;

    configuration.save(&config_path)?;

    if !config_already_existed {
        debug!("Saving new Configuration to {}", config_path.display());
        configuration.save(&config_path)?;
    }

    Ok((configuration, config_path))
}
