use std::path::PathBuf;

use andean_condor::core::input::Input;
use anyhow::{bail, Result};

use crate::{
    commands::SceneDetectionMethod,
    configuration::Configuration,
    CondorCliError,
    DEFAULT_CONFIG_PATH,
};

pub fn detect_scenes_handler(
    config_path: Option<PathBuf>,
    method: Option<SceneDetectionMethod>,
    min_scene_seconds: Option<usize>,
    max_scene_seconds: Option<usize>,
) -> Result<()> {
    let config_path =
        path_abs::PathAbs::new(config_path.unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_PATH)))?
            .as_path()
            .to_path_buf();

    if !config_path.exists() {
        bail!(CondorCliError::ConfigFileNotFound(config_path));
    }

    let config = Configuration::load(&config_path)
        .map_err(|_| CondorCliError::ConfigLoadError(config_path.clone()))?;
    let mut config = config.expect("config should exist");

    println!("Indexing input...");
    let mut input = Input::from_data(&config.condor.input)?;
    let clip_info = input.clip_info()?;
    let fps = *clip_info.frame_rate.numer() as f64 / *clip_info.frame_rate.denom() as f64;

    let previous_method = config.condor.sequence_config.scene_detection.method;
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
        config.condor.sequence_config.scene_detection.method = new_method;
    }
    config
        .condor
        .sequence_config
        .scene_detection
        .method
        .set_minimum_length(min_scene_frames)?;
    config
        .condor
        .sequence_config
        .scene_detection
        .method
        .set_maximum_length(max_scene_frames)?;

    config.save(&config_path)?;

    Ok(())
}
