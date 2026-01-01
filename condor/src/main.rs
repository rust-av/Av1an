use std::{
    panic,
    path::{Path, PathBuf},
    process,
};

use anyhow::Result;
use av1an_core::condor::core::Condor;
use clap::Parser;
use thiserror::Error;

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
    tui::{run_parallel_encoder_tui, run_scene_concatenator_tui, run_scene_detection_tui},
};

mod commands;
mod components;
mod configuration;
mod tui;
mod utils;
mod apps;

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
    // TODO: Set up tracing

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

pub fn run_condor_tui(configuration: &Configuration, save_file: &Path) -> Result<()> {
    let config_copy = configuration.clone();
    let save_file_copy = save_file.to_path_buf();
    let mut condor: Condor<configuration::CliProcessData, configuration::CliProcessorConfig> =
        configuration.instantiate_condor(Box::new(move |data| {
            let mut config = config_copy.clone();
            config.condor = data;
            Configuration::save(&config, &save_file_copy)?;
            Ok(())
        }))?;

    let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let scenes_directory = &configuration.temp.join("scenes");

    // let ctrlc_cancelled = std::sync::Arc::clone(&cancelled);
    // ctrlc::set_handler(move || {
    //     println!("Ctrl-C Crate: pressed");
    //     let already_cancelled = ctrlc_cancelled.swap(true,
    // std::sync::atomic::Ordering::SeqCst);     if already_cancelled {
    //         println!("Force quit Condor");
    //         process::exit(0);
    //     }
    // })
    // .expect("Error setting Ctrl-C handler");

    run_scene_detection_tui(
        &mut condor,
        &configuration.scd_input_filters,
        std::sync::Arc::clone(&cancelled),
    )?;
    if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
        return Ok(());
    }
    run_parallel_encoder_tui(
        &mut condor,
        &configuration.input_filters,
        scenes_directory,
        std::sync::Arc::clone(&cancelled),
    )?;
    if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
        return Ok(());
    }
    run_scene_concatenator_tui(
        &mut condor,
        scenes_directory,
        std::sync::Arc::clone(&cancelled),
    )?;
    if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
        return Ok(());
    }

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
