use std::{
    panic,
    path::{Path, PathBuf},
    process,
};

use anyhow::Result;
use av1an_core::condor::core::Condor;
use clap::Parser;
use thiserror::Error;
use tracing::{debug, info, level_filters::LevelFilter};

use crate::{
    commands::{
        config::config_sub_handler,
        detect_scenes::detect_scenes_handler,
        init::init_handler,
        start::start_handler,
        Commands,
        CondorCli,
    },
    configuration::Configuration,
    logging::init_logging,
    tui::{run_parallel_encoder_tui, run_scene_concatenator_tui, run_scene_detection_tui},
};

mod apps;
mod commands;
mod components;
mod configuration;
mod logging;
mod tui;
mod utils;

pub const DEFAULT_CONFIG_PATH: &str = "./condor.json";
pub const DEFAULT_TEMP_PATH: &str = "./temp";
pub const DEFAULT_LOG_PATH: &str = "./logs/condor.log";

fn main() -> anyhow::Result<()> {
    let orig_hook = panic::take_hook();
    // Catch panics in child threads
    panic::set_hook(Box::new(move |panic_info| {
        orig_hook(panic_info);
        process::exit(1);
    }));
    run()
}

fn run() -> anyhow::Result<()> {
    let cli = CondorCli::parse();
    let cwd = std::env::current_dir()?;
    let config_path = cli.config_file;
    let logs = cli.logs.unwrap_or_else(|| cwd.join(DEFAULT_LOG_PATH));
    init_logging(LevelFilter::INFO, &logs, LevelFilter::DEBUG)?;
    // TODO: hash input file name and use it as the temp folder path

    match cli.command {
        Commands::Init {
            input,
            output,
            temp,
            decoder,
            concat,
            workers,
        } => {
            init_handler(config_path, input, output, temp, decoder, concat, workers)?;
        },
        Commands::Config {
            subcommand,
        } => {
            config_sub_handler(config_path, subcommand)?;
        },
        Commands::DetectScenes {
            method,
            min_scene_seconds,
            max_scene_seconds,
        } => {
            detect_scenes_handler(config_path, method, min_scene_seconds, max_scene_seconds)?;

            // run_condor_tui("Scene Detection")?
        },
        Commands::Benchmark {
            threshold,
            max_memory,
        } => {
            todo!();
            // run_condor_tui("Benchmarking")?;
        },
        Commands::Start {
            temp,
            input,
            decoder,
            filters,
            output,
            concat,
            workers,
            encoder,
            passes,
            params,
            photon_noise,
            skip_benchmark,
        } => {
            let (configuration, save_file) = start_handler(
                config_path,
                temp,
                input,
                output,
                decoder.as_ref(),
                filters,
                concat,
                workers,
                encoder,
                passes,
                params,
                photon_noise,
                skip_benchmark,
            )?;

            // let config_copy = configuration.clone();
            run_condor_tui(&configuration, &save_file)?;
        },
        Commands::Clean {
            all,
        } => {
            todo!();
        },
    }

    Ok(())
}

#[tracing::instrument(skip_all)]
pub fn run_condor_tui(configuration: &Configuration, save_file: &Path) -> Result<()> {
    let config_copy = configuration.clone();
    let save_file_copy = save_file.to_path_buf();
    debug!("Instantiating Condor with {:?}", {
        // Remove scenes to reduce log spam
        let mut config = configuration.clone();
        config.condor.scenes = Vec::new();
        config
    });
    let mut condor: Condor<configuration::CliProcessData, configuration::CliProcessorConfig> =
        configuration.instantiate_condor(Box::new(move |data| {
            let mut config = config_copy.clone();
            config.condor = data;
            Configuration::save(&config, &save_file_copy)?;
            Ok(())
        }))?;

    let cancellation_token = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let scenes_directory = &configuration.temp.join("scenes");

    let cancelled = || {
        if cancellation_token.load(std::sync::atomic::Ordering::Relaxed) {
            info!("Condor cancelled. Exiting...");
            return true;
        }
        false
    };

    run_scene_detection_tui(
        &mut condor,
        &configuration.scd_input_filters,
        std::sync::Arc::clone(&cancellation_token),
    )?;
    if cancelled() {
        return Ok(());
    }

    // run_benchmarker_tui(
    //     &mut condor,
    //     std::sync::Arc::clone(&cancelled),
    // )?;
    // if cancelled() {
    //     return Ok(());
    // }

    // run_grain_analyzer_tui(
    //     &mut condor,
    //     std::sync::Arc::clone(&cancelled),
    // )?;
    // if cancelled() {
    //     return Ok(());
    // }

    // run_target_quality_tui(
    //     &mut condor,
    //     std::sync::Arc::clone(&cancelled),
    // )?;
    // if cancelled() {
    //     return Ok(());
    // }

    run_parallel_encoder_tui(
        &mut condor,
        &configuration.input_filters,
        scenes_directory,
        std::sync::Arc::clone(&cancellation_token),
    )?;
    if cancelled() {
        return Ok(());
    }

    // run_quality_normalizer_tui(
    //     &mut condor,
    //     scenes_directory,
    //     std::sync::Arc::clone(&cancelled),
    // )?;
    // if cancelled() {
    //     return Ok(());
    // }

    run_scene_concatenator_tui(
        &mut condor,
        scenes_directory,
        std::sync::Arc::clone(&cancellation_token),
    )?;
    if cancelled() {
        return Ok(());
    }

    // run_quality_analyzer_tui(
    //     &mut condor,
    //     scenes_directory,
    //     std::sync::Arc::clone(&cancellation_token),
    // )?;
    // if cancelled() {
    //     return Ok(());
    // }

    info!(
        "Condor has landed. Output: {}",
        condor.output.path.display()
    );
    info!("Have a nice day!");

    Ok(())
}

#[derive(Debug, Error)]
pub enum CondorCliError {
    #[error("Cannot initialize over an existing config file: {0}")]
    ConfigFileAlreadyExists(PathBuf),
    #[error("No config file found at: {0}")]
    ConfigFileNotFound(PathBuf),
    #[error("Failed to load config file: {0}")]
    ConfigLoadError(PathBuf),
    #[error("Cannot start without a config file or without input and output paths")]
    NoConfigOrInputOrOutput,
    #[error("Cannot set Decoder without a valid Input path")]
    DecoderWithoutInput,
}
