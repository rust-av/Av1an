use std::path::PathBuf;

use anyhow::{bail, Result};
use av1an_core::{
    condor::data::input::{ImportMethod, Input as InputData, VapourSynthImportMethod},
    ConcatMethod,
};
use tracing::{error, info};

use crate::{
    commands::DecoderMethod,
    configuration::Configuration,
    CondorCliError,
    DEFAULT_CONFIG_PATH,
    DEFAULT_TEMP_PATH,
};

pub fn init_handler(
    config_path: Option<PathBuf>,
    input_path: PathBuf,
    output_path: PathBuf,
    temp_path: Option<PathBuf>,
    decoder: DecoderMethod,
    concat: ConcatMethod,
    workers: Option<u8>,
) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let input = path_abs::PathAbs::new(input_path)?.as_path().to_path_buf();
    let output = path_abs::PathAbs::new(output_path)?.as_path().to_path_buf();
    let config_path =
        path_abs::PathAbs::new(config_path.unwrap_or_else(|| cwd.join(DEFAULT_CONFIG_PATH)))?
            .as_path()
            .to_path_buf();
    let temp = path_abs::PathAbs::new(temp_path.unwrap_or_else(|| cwd.join(DEFAULT_TEMP_PATH)))?
        .as_path()
        .to_path_buf();

    if config_path.exists() {
        let err = CondorCliError::ConfigFileAlreadyExists(config_path);
        error!("{}", err);
        bail!(err);
    }

    let mut configuration = Configuration::new(&input, &output, &temp)?;

    configuration.condor.input = match decoder {
        DecoderMethod::FFMS2 => InputData::Video {
            path:          input,
            import_method: ImportMethod::FFMS2 {},
        },
        vs_decoders => InputData::VapourSynth {
            path:          input,
            import_method: match vs_decoders {
                DecoderMethod::BestSource => VapourSynthImportMethod::BestSource {
                    index: None
                },
                DecoderMethod::VSFFMS2 => VapourSynthImportMethod::FFMS2 {
                    index: None
                },
                DecoderMethod::LSMASHWorks => VapourSynthImportMethod::LSMASHWorks {
                    index: None
                },
                DecoderMethod::DGDecodeNV => VapourSynthImportMethod::DGDecNV {
                    dgindexnv_executable: None,
                },
                DecoderMethod::FFMS2 => unreachable!(),
            },
            cache_path:    None,
        },
    };
    configuration.condor.output.concatenation_method = concat;
    if let Some(workers) = workers {
        configuration.condor.processor_config.parallel_encoder.workers = workers;
    }

    configuration.save(&config_path)?;

    info!(
        "Initialized Condor configuration at: {}",
        config_path.display()
    );
    info!(
        "Run \"condor start\" to start encoding or \"condor config\" to further modify the \
         configuration."
    );

    Ok(())
}
