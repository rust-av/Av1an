use std::path::Path;

use andean_condor::models::{
    input::{Input as InputModel, VapourSynthImportMethod},
    sequence::scene_concatenate::ConcatMethod,
};
use anyhow::{bail, Result};
use tracing::{error, info};

use crate::{configuration::Configuration, CondorCliError, DEFAULT_CONFIG_PATH, DEFAULT_TEMP_PATH};

pub fn init_handler(
    config_path: Option<&Path>,
    input_path: &Path,
    output_path: &Path,
    temp_path: Option<&Path>,
    vs_args: Option<&[String]>,
) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let input = path_abs::PathAbs::new(input_path)?.as_path().to_path_buf();
    let output = path_abs::PathAbs::new(output_path)?.as_path().to_path_buf();
    let config_path = path_abs::PathAbs::new(
        config_path.map_or_else(|| cwd.join(DEFAULT_CONFIG_PATH), |p| p.to_path_buf()),
    )?
    .as_path()
    .to_path_buf();
    let temp = path_abs::PathAbs::new(
        temp_path.map_or_else(|| cwd.join(DEFAULT_TEMP_PATH), |p| p.to_path_buf()),
    )?
    .as_path()
    .to_path_buf();

    if config_path.exists() {
        let err = CondorCliError::ConfigFileAlreadyExists(config_path);
        error!("{}", err);
        bail!(err);
    }

    let mut configuration = Configuration::new(&input, &output, &temp, vs_args)?;

    configuration.condor.input = InputModel::VapourSynth {
        path:          input,
        import_method: VapourSynthImportMethod::BestSource {
            index: None
        },
        cache_path:    None,
    };
    configuration.condor.sequence_config.scene_concatenation.method = ConcatMethod::MKVMerge;

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
