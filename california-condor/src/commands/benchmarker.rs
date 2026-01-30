use std::path::{Path, PathBuf};

use andean_condor::{
    models::{
        encoder::{Encoder, EncoderBase, EncoderPasses},
        input::{
            ImportMethod,
            Input as InputModel,
            VapourSynthImportMethod,
            VapourSynthScriptSource,
        },
    },
    vapoursynth::vapoursynth_filters::VapourSynthFilter,
};
use anyhow::{bail, Result};
use tracing::{debug, error, trace};

use crate::{
    commands::DecoderMethod,
    configuration::{ConfigError, Configuration},
    utils::parameter_parser::EncoderParamsParser,
    CondorCliError,
    DEFAULT_CONFIG_PATH,
    DEFAULT_TEMP_PATH,
};

#[allow(clippy::too_many_arguments)]
pub fn benchmarker_handler(
    config_path: Option<&Path>,
    temp_path: Option<&Path>,
    input_path: Option<&Path>,
    decoder: Option<&DecoderMethod>,
    filters: Option<&[VapourSynthFilter]>,
    vs_args: Option<&[String]>,
    encoder: Option<&EncoderBase>,
    passes: Option<u8>,
    params: Option<String>,
    threshold: Option<u8>,
    max_memory: Option<u32>,
) -> Result<(Configuration, PathBuf)> {
    if config_path.is_some_and(|p| !p.exists()) && input_path.is_none() {
        let err = CondorCliError::NoConfigOrInput;
        error!("{}", err);
        bail!(err);
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
            let path_err = || {
                let err = CondorCliError::NoConfigOrInput;
                error!("{}", err);
                err
            };
            let input_path = input_path.ok_or_else(path_err)?;
            debug!("Creating new configuration");
            let input = path_abs::PathAbs::new(input_path)?.as_path().to_path_buf();
            // Won't be used
            let output = input.with_file_name(format!(
                "{}.mkv",
                input.file_stem().expect("input is a file").display()
            ));
            let cwd = std::env::current_dir()?;
            let temp_path = temp_path.map(|p| p.to_path_buf());
            let temp =
                path_abs::PathAbs::new(temp_path.unwrap_or_else(|| cwd.join(DEFAULT_TEMP_PATH)))?
                    .as_path()
                    .to_path_buf();
            Configuration::new(&input, &output, &temp, vs_args)?
        }
    };

    if let Some(temp) = temp_path {
        configuration.temp = path_abs::PathAbs::new(temp)?.as_path().to_path_buf();
    }
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
    if let Some(encoder) = encoder {
        let options = encoder.default_parameters();
        let pass = encoder.default_passes();
        configuration.condor.encoder = match encoder {
            EncoderBase::AOM => Encoder::AOM {
                executable: None,
                pass,
                options,
                photon_noise: None,
            },
            EncoderBase::RAV1E => Encoder::RAV1E {
                executable: None,
                pass,
                options,
                photon_noise: None,
            },
            EncoderBase::VPX => Encoder::VPX {
                executable: None,
                pass,
                options,
            },
            EncoderBase::SVTAV1 => Encoder::SVTAV1 {
                executable: None,
                pass,
                options,
                photon_noise: None,
            },
            EncoderBase::X264 => Encoder::X264 {
                executable: None,
                pass,
                options,
            },
            EncoderBase::X265 => Encoder::X265 {
                executable: None,
                pass,
                options,
            },
            EncoderBase::VVenC => Encoder::VVenC {
                executable: None,
                pass,
                options,
            },
            EncoderBase::FFmpeg => Encoder::FFmpeg {
                executable: None,
                options,
            },
        }
    }
    if let Some(passes) = passes
        && let Some(encoder_passes) = configuration.condor.encoder.passes_mut()
    {
        *encoder_passes = EncoderPasses::All(passes);
    }
    if let Some(params) = params {
        let parameters = EncoderParamsParser::parse_string(&params);
        configuration.condor.encoder.parameters_mut().extend(parameters);
    }
    if let Some(threshold) = threshold {
        configuration.condor.sequence_config.benchmarker.threshold = threshold;
    }
    configuration.condor.sequence_config.benchmarker.max_memory = max_memory;

    if !config_already_existed {
        debug!("Saving new Configuration to {}", config_path.display());
        configuration.save(&config_path)?;
    }

    Ok((configuration, config_path))
}
