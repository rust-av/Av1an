use std::path::PathBuf;

use anyhow::{bail, Result};
use av1an_core::{
    condor::data::{
        encoding::{photon_noise::PhotonNoise, Encoder, EncoderBase, EncoderPasses},
        input::{ImportMethod, Input as InputData, VapourSynthImportMethod},
    },
    vs::vapoursynth_filters::VapourSynthFilter,
    ConcatMethod,
};

use crate::{
    commands::DecoderMethod,
    configuration::Configuration,
    utils::parameter_parser::EncoderParamsParser,
    CondorCliError,
    DEFAULT_CONFIG_PATH,
    DEFAULT_TEMP_PATH,
};

#[allow(clippy::too_many_arguments)]
pub fn start_handler(
    config_path: Option<PathBuf>,
    temp_path: Option<PathBuf>,
    input_path: Option<PathBuf>,
    output_path: Option<PathBuf>,
    decoder: Option<&DecoderMethod>,
    filters: Option<Vec<VapourSynthFilter>>,
    concat: Option<ConcatMethod>,
    workers: Option<u8>,
    encoder: Option<EncoderBase>,
    passes: Option<u8>,
    params: Option<String>,
    photon_noise: Option<u32>,
    skip_benchmark: bool,
) -> Result<(Configuration, PathBuf)> {
    if config_path.as_ref().is_some_and(|p| !p.exists())
        && (input_path.is_none() || output_path.is_none())
    {
        bail!(CondorCliError::NoConfigOrInputOrOutput);
    }
    let config_path =
        path_abs::PathAbs::new(config_path.unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_PATH)))?
            .as_path()
            .to_path_buf();

    let mut configuration = {
        if config_path.exists() {
            Configuration::load(&config_path)
                .map_err(|_| CondorCliError::ConfigLoadError(config_path.clone()))?
                .expect("Config should exist")
        } else {
            if input_path.is_none() || output_path.is_none() {
                bail!(CondorCliError::NoConfigOrInputOrOutput);
            }
            let input = input_path.clone().expect("Input should be Some");
            let output = output_path.clone().expect("Output should be Some");
            let cwd = std::env::current_dir()?;
            let temp = path_abs::PathAbs::new(
                temp_path.clone().unwrap_or_else(|| cwd.join(DEFAULT_TEMP_PATH)),
            )?
            .as_path()
            .to_path_buf();
            Configuration::new(&input, &output, &temp)?
        }
    };

    if let Some(temp) = temp_path {
        configuration.temp = temp;
    }
    if let Some(decoder) = &decoder {
        let existing_input_path = match configuration.condor.input {
            InputData::Video {
                path, ..
            } => Some(path),
            InputData::VapourSynth {
                path, ..
            } => Some(path),
            InputData::VapourSynthScript {
                ..
            } => input_path.clone(),
        };
        if existing_input_path.is_none() {
            bail!(CondorCliError::DecoderWithoutInput);
        }
        let existing_input_path = existing_input_path.expect("Input path should be Some");
        match decoder {
            DecoderMethod::FFMS2 => {
                configuration.condor.input = InputData::Video {
                    path:          existing_input_path,
                    import_method: ImportMethod::FFMS2 {},
                };
            },
            vs_decoders => {
                configuration.condor.input = InputData::VapourSynth {
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
        if let Some(decoder_method) = &decoder {
            configuration.condor.input = match decoder_method {
                DecoderMethod::FFMS2 => InputData::Video {
                    path:          input,
                    import_method: ImportMethod::FFMS2 {},
                },
                vs_decoders => InputData::VapourSynth {
                    path:          input,
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
                },
            }
        } else {
            configuration.condor.input = InputData::VapourSynth {
                path:          input,
                import_method: VapourSynthImportMethod::BestSource {
                    index: None
                },
                cache_path:    None,
            };
        }
    }
    if let Some(filters) = filters {
        configuration.input_filters = filters;
    }
    if let Some(output) = output_path {
        configuration.condor.output.path = output;
    }
    if let Some(concat) = concat {
        configuration.condor.output.concatenation_method = concat;
    }
    if let Some(workers) = workers {
        configuration.condor.processor_config.parallel_encoder.workers = workers;
    }
    if let Some(encoder) = encoder {
        let options = encoder.default_parameters();
        let pass = encoder.default_passes();
        let photon_noise = photon_noise.map(|iso| PhotonNoise {
            iso,
            chroma_iso: None,
            width: None,
            height: None,
            c_y: None,
            ccb: None,
            ccr: None,
        });
        configuration.condor.encoder = match encoder {
            av1an_core::condor::data::encoding::EncoderBase::AOM => Encoder::AOM {
                executable: None,
                pass,
                options,
                photon_noise,
            },
            av1an_core::condor::data::encoding::EncoderBase::RAV1E => Encoder::RAV1E {
                executable: None,
                pass,
                options,
                photon_noise,
            },
            av1an_core::condor::data::encoding::EncoderBase::VPX => Encoder::VPX {
                executable: None,
                pass,
                options,
            },
            av1an_core::condor::data::encoding::EncoderBase::SVTAV1 => Encoder::SVTAV1 {
                executable: None,
                pass,
                options,
                photon_noise,
            },
            av1an_core::condor::data::encoding::EncoderBase::X264 => Encoder::X264 {
                executable: None,
                pass,
                options,
            },
            av1an_core::condor::data::encoding::EncoderBase::X265 => Encoder::X265 {
                executable: None,
                pass,
                options,
            },
            av1an_core::condor::data::encoding::EncoderBase::VVenC => Encoder::VVenC {
                executable: None,
                pass,
                options,
            },
            av1an_core::condor::data::encoding::EncoderBase::FFmpeg => Encoder::FFmpeg {
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

    configuration.save(&config_path)?;

    Ok((configuration, config_path))
}
