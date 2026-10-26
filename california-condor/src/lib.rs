use std::{panic, path::Path, process};

use andean_condor::core::Condor;
use anyhow::Result;
use clap::{CommandFactory, FromArgMatches};
use tracing::{debug, info, level_filters::LevelFilter};

use crate::{
    commands::{
        Commands,
        CondorCli,
        handlers::{
            benchmarker::benchmarker_handler,
            concatenate::concatenate_handler,
            detect_noise::detect_noise_handler,
            detect_scenes::detect_scenes_handler,
            encode::encode_handler,
            init::init_handler,
            optimize_bitrate::optimize_bitrate_handler,
            quality_check::quality_check_handler,
            scale_noise::scale_noise_handler,
            scale_speed::scale_speed_handler,
            start::start_handler,
            target_quality::target_quality_handler,
            zone::{configure_zoner, zone_handler},
        },
        help_text::process_command_tree,
        version::print_version,
    },
    configuration::Configuration,
    logging::init_logging,
    tui::{
        run_benchmarker_tui,
        run_bitrate_optimizer_tui,
        run_noise_detector_tui,
        run_noise_scaler_tui,
        run_parallel_encoder_tui,
        run_quality_check_tui,
        run_scene_concatenator_tui,
        run_scene_detector_tui,
        run_speed_scaler_tui,
        run_target_quality_tui,
    },
    zoner::Zoner,
};

pub mod apps;
pub mod commands;
pub mod components;
pub mod configuration;
pub mod logging;
pub mod scenes;
#[cfg(feature = "screenshots")]
pub mod screenshot;
pub mod theme;
pub mod tui;
pub mod utils;
pub mod zoner;

#[doc(hidden)]
pub mod test_helpers;

pub const DEFAULT_CONFIG_PATH: &str = "./condor.json";
pub const DEFAULT_LOG_PATH: &str = "./logs/condor.log";

/// Entry point used by both `main()` binary and externally (e.g., tests).
pub fn entry_point() -> anyhow::Result<()> {
    let orig_hook = panic::take_hook();
    panic::set_hook(Box::new(move |panic_info| {
        orig_hook(panic_info);
        process::exit(1);
    }));
    run()
}

fn run() -> anyhow::Result<()> {
    let mut cmd = CondorCli::command();
    cmd = process_command_tree(cmd);
    let matches = cmd.get_matches_mut();
    let mut cli = CondorCli::from_arg_matches(&matches).map_err(|err| err.exit())?;
    cli.normalize_zone_flags();
    cli.reject_indexed_add(&mut cmd);
    commands::set_verbose(cli.verbose);

    if cli.version {
        return print_version(cli.verbose);
    }

    let cwd = std::env::current_dir()?;
    let config_path = cli.config_file;
    let logs = cli.logs.unwrap_or_else(|| cwd.join(DEFAULT_LOG_PATH));
    let set_logs = |log_path: &Path| {
        init_logging(
            LevelFilter::INFO,
            log_path,
            if cli.verbose {
                LevelFilter::TRACE
            } else {
                LevelFilter::DEBUG
            },
        )
    };
    set_logs(&logs)?;
    let temp = cli.temp;

    match cli.command {
        Some(Commands::Init {
            edit,
            input,
            output,
            decoder,
            filters,
            vs_args,
            concat,
            discard,
            workers,
            encoder,
            params,
            photon_noise,
            disable_color_data,
            target_metric,
            target,
        }) => {
            if let Some(edit) = edit {
                return commands::handlers::init::config(config_path.as_deref(), &edit);
            }
            let (input, output) =
                commands::handlers::init::require_paths(input.as_deref(), output.as_deref())?;
            init_handler(
                config_path.as_deref(),
                temp.as_deref(),
                input,
                output,
                decoder.as_ref(),
                filters.as_deref(),
                vs_args.as_deref(),
                concat.as_ref(),
                discard.as_deref(),
                workers,
                encoder.as_ref(),
                params.as_deref(),
                photon_noise,
                !disable_color_data,
                target_metric.as_ref(),
                target,
            )?;
        },
        Some(Commands::DetectScenes {
            edit,
            input,
            decoder,
            filters,
            vs_args,
            method,
            min_scene_seconds,
            max_scene_seconds,
        }) => {
            if let Some(edit) = edit {
                return commands::handlers::detect_scenes::config(config_path.as_deref(), &edit);
            }
            let (configuration, save_file) = detect_scenes_handler(
                config_path.as_deref(),
                temp.as_deref(),
                input.as_deref(),
                decoder.as_ref(),
                filters.as_deref(),
                vs_args.as_deref(),
                method.as_ref(),
                min_scene_seconds,
                max_scene_seconds,
            )?;

            run_scene_detector(&configuration, &save_file)?;
        },
        Some(Commands::Benchmark {
            edit,
            threshold,
            max_memory,
        }) => {
            if let Some(edit) = edit {
                return commands::handlers::benchmarker::config(config_path.as_deref(), &edit);
            }
            let (configuration, save_file) = benchmarker_handler(
                config_path.as_deref(),
                temp.as_deref(),
                threshold,
                max_memory,
            )?;

            run_benchmarker(&configuration, &save_file)?;
        },
        Some(Commands::Zone {
            zones,
            dry_run,
            ..
        }) => {
            let (configuration, save_file) = zone_handler(config_path.as_deref(), &zones)?;
            if dry_run {
                info!(
                    "Zoner dry-run: previewing planned changes; no configuration or outputs will \
                     be saved"
                );
                let mut condor = configuration.instantiate_condor(Box::new(|_| Ok(())))?;
                Zoner::run(&mut condor)?;
                info!("Zoner dry-run complete: no configuration changes were saved");
            } else {
                run_zoner(&configuration, &save_file)?;
            }
        },
        Some(Commands::Scenes {
            index,
            action,
        }) => {
            // The index comes first and is optional: without an action the
            // addressed scenes are printed, which is what a bare
            // `condor scenes` performs for the whole list.
            return commands::handlers::scenes::config(
                config_path.as_deref(),
                index.as_ref(),
                action.as_ref(),
            );
        },
        Some(Commands::DetectNoise {
            edit,
            input,
            decoder,
            vs_args,
            reference_filters,
            denoised_filters,
        }) => {
            if let Some(edit) = edit {
                return commands::handlers::detect_noise::config(config_path.as_deref(), &edit);
            }
            let (configuration, save_file) = detect_noise_handler(
                config_path.as_deref(),
                input.as_deref(),
                decoder.as_ref(),
                vs_args.as_deref(),
                reference_filters.as_deref(),
                denoised_filters.as_deref(),
            )?;

            run_noise_detector(&configuration, &save_file)?;
        },
        Some(Commands::ScaleNoise {
            edit,
            threshold,
            minimum_scaler,
            maximum_scaler,
            scale_chroma,
        }) => {
            if let Some(edit) = edit {
                return commands::handlers::scale_noise::config(config_path.as_deref(), &edit);
            }
            let (configuration, save_file) = scale_noise_handler(
                config_path.as_deref(),
                threshold,
                minimum_scaler,
                maximum_scaler,
                scale_chroma,
            )?;

            run_noise_scaler(&configuration, &save_file)?;
        },
        Some(Commands::TargetQuality {
            edit,
            input,
            decoder,
            filters,
            vs_args,
            params,
            metric,
            target,
            minimum_quantizer,
            maximum_quantizer,
            profile,
        }) => {
            if let Some(edit) = edit {
                return commands::handlers::target_quality::config(config_path.as_deref(), &edit);
            }
            let (configuration, save_file) = target_quality_handler(
                config_path.as_deref(),
                temp.as_deref(),
                input.as_deref(),
                decoder.as_ref(),
                filters.as_deref(),
                vs_args.as_deref(),
                params.as_deref(),
                metric.as_ref(),
                target,
                minimum_quantizer,
                maximum_quantizer,
                profile.as_ref(),
            )?;

            run_target_quality(&configuration, &save_file)?;
        },
        Some(Commands::OptimizeBitrate {
            edit,
            sigma_threshold,
        }) => {
            if let Some(edit) = edit {
                return commands::handlers::optimize_bitrate::config(config_path.as_deref(), &edit);
            }
            let (configuration, save_file) =
                optimize_bitrate_handler(config_path.as_deref(), sigma_threshold)?;

            run_bitrate_optimizer(&configuration, &save_file)?;
        },
        Some(Commands::ScaleSpeed {
            edit,
            quantizers,
            speeds,
        }) => {
            if let Some(edit) = edit {
                return commands::handlers::scale_speed::config(config_path.as_deref(), &edit);
            }
            let (configuration, save_file) = scale_speed_handler(
                config_path.as_deref(),
                quantizers.as_deref(),
                speeds.as_deref(),
            )?;

            run_speed_scaler(&configuration, &save_file)?;
        },
        Some(Commands::Encode {
            edit,
            input,
            decoder,
            filters,
            vs_args,
            workers,
            encoder,
            passes,
            params,
            photon_noise,
            chroma_noise,
            ffmpeg_filter,
        }) => {
            if let Some(edit) = edit {
                return commands::handlers::encode::config(config_path.as_deref(), &edit);
            }
            let (configuration, save_file) = encode_handler(
                config_path.as_deref(),
                temp.as_deref(),
                input.as_deref(),
                decoder.as_ref(),
                filters.as_deref(),
                vs_args.as_deref(),
                workers,
                encoder.as_ref(),
                passes,
                params.as_deref(),
                photon_noise,
                chroma_noise,
                ffmpeg_filter.as_deref(),
            )?;

            run_encoder(&configuration, &save_file)?;
        },
        Some(Commands::Concatenate {
            edit,
            method,
            discard,
        }) => {
            if let Some(edit) = edit {
                return commands::handlers::concatenate::config(config_path.as_deref(), &edit);
            }
            let (configuration, save_file) = concatenate_handler(
                config_path.as_deref(),
                temp.as_deref(),
                method.as_ref(),
                discard.as_deref(),
            )?;

            run_concatenator(&configuration, &save_file)?;
        },
        Some(Commands::QualityCheck {
            edit,
            input,
            decoder,
            filters,
            vs_args,
            metric,
            profile,
        }) => {
            if let Some(edit) = edit {
                return commands::handlers::quality_check::config(config_path.as_deref(), &edit);
            }
            let (configuration, save_file) = quality_check_handler(
                config_path.as_deref(),
                temp.as_deref(),
                input.as_deref(),
                decoder.as_ref(),
                filters.as_deref(),
                vs_args.as_deref(),
                metric.as_ref(),
                profile.as_ref(),
            )?;

            run_quality_check(&configuration, &save_file)?;
        },
        Some(Commands::Clean {
            all: _,
        }) => {
            todo!();
        },
        None => {
            let (mut configuration, save_file) = start_handler(
                config_path.as_deref(),
                temp.as_deref(),
                cli.input.as_deref(),
                cli.scd_input.as_deref(),
                cli.tq_input.as_deref(),
                cli.output.as_deref(),
                cli.decoder.as_ref(),
                cli.scd_decoder.as_ref(),
                cli.tq_decoder.as_ref(),
                cli.filters.as_deref(),
                cli.scd_filters.as_deref(),
                cli.tq_filters.as_deref(),
                cli.vs_args.as_deref(),
                cli.scd_vs_args.as_deref(),
                cli.tq_vs_args.as_deref(),
                cli.concat.as_ref(),
                cli.discard.as_deref(),
                cli.workers,
                cli.encoder.as_ref(),
                cli.passes,
                cli.params.as_deref(),
                cli.tq_params.as_deref(),
                cli.photon_noise,
                cli.chroma_noise,
                cli.ffmpeg_filter.as_deref(),
                cli.disable_color_data.then_some(false),
                cli.target_metric.as_ref(),
                cli.target,
                cli.minimum_quantizer,
                cli.maximum_quantizer,
                cli.target_profile.as_ref(),
            )?;

            if let Some(zones) = &cli.zones {
                configure_zoner(&mut configuration, zones)?;
                configuration.save(&save_file)?;
            }

            run_condor(&configuration, &save_file, cli.skip_scd)?;
        },
    }

    Ok(())
}

/// Instantiates a `Condor` that writes every save back to `save_file`.
///
/// The sequence runners below differ only in the sequence they hand the
/// `Condor` to.
fn instantiate_saved_condor(
    configuration: &Configuration,
    save_file: &Path,
) -> Result<Condor<configuration::CliSequenceData, configuration::CliSequenceConfig>> {
    let config_copy = configuration.clone();
    let save_file_copy = save_file.to_path_buf();
    debug!("Instantiating Condor with {:?}", {
        // Remove scenes to reduce log spam
        let mut config = configuration.clone();
        config.condor.scenes = Vec::new();
        config
    });

    configuration.instantiate_condor(Box::new(move |data| {
        let mut config = config_copy.clone();
        config.condor = data;
        Configuration::save(&config, &save_file_copy)?;
        Ok(())
    }))
}

#[tracing::instrument(skip_all)]
pub fn run_condor(configuration: &Configuration, save_file: &Path, skip_scd: bool) -> Result<()> {
    let mut condor = instantiate_saved_condor(configuration, save_file)?;

    let cancellation_token = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

    let cancelled = || {
        if cancellation_token.load(std::sync::atomic::Ordering::Relaxed) {
            info!("Condor cancelled. Exiting...");
            return true;
        }
        false
    };

    if !skip_scd {
        run_scene_detector_tui(&mut condor, std::sync::Arc::clone(&cancellation_token))?;
        if cancelled() {
            return Ok(());
        }
    }

    if configuration
        .condor
        .sequence_config
        .zoner
        .as_ref()
        .is_some_and(|zoner| !zoner.zones.is_empty())
    {
        Zoner::run(&mut condor)?;
        if cancelled() {
            return Ok(());
        }
    }

    if configuration.condor.sequence_config.noise_detector.is_some() {
        run_noise_detector_tui(&mut condor, std::sync::Arc::clone(&cancellation_token))?;
        if cancelled() {
            return Ok(());
        }

        run_noise_scaler_tui(&mut condor, std::sync::Arc::clone(&cancellation_token))?;
        if cancelled() {
            return Ok(());
        }
    }

    if configuration.condor.sequence_config.parallel_encoder.workers.is_none() {
        run_benchmarker_tui(&mut condor, std::sync::Arc::clone(&cancellation_token))?;
        if cancelled() {
            return Ok(());
        }
    }

    if configuration.condor.sequence_config.target_quality.is_some() {
        run_target_quality_tui(&mut condor, std::sync::Arc::clone(&cancellation_token))?;
        if cancelled() {
            return Ok(());
        }

        if configuration
            .condor
            .sequence_config
            .bitrate_optimizer
            .bitrate_sigma_threshold
            .is_some()
        {
            run_bitrate_optimizer_tui(&mut condor, std::sync::Arc::clone(&cancellation_token))?;
            if cancelled() {
                return Ok(());
            }
        }
    }

    if configuration.condor.sequence_config.speed_scaler.speed_quantizers.len() >= 2 {
        run_speed_scaler_tui(&mut condor, std::sync::Arc::clone(&cancellation_token))?;
        if cancelled() {
            return Ok(());
        }
    }

    run_parallel_encoder_tui(&mut condor, std::sync::Arc::clone(&cancellation_token))?;
    if cancelled() {
        return Ok(());
    }

    run_scene_concatenator_tui(&mut condor, std::sync::Arc::clone(&cancellation_token))?;
    if cancelled() {
        return Ok(());
    }

    if configuration.condor.sequence_config.quality_check.is_some() {
        run_quality_check_tui(&mut condor, std::sync::Arc::clone(&cancellation_token))?;
        if cancelled() {
            return Ok(());
        }
    }

    info!(
        "Condor has landed. Output: {}",
        condor.output.path.display()
    );
    info!("Have a nice day!");

    Ok(())
}

#[tracing::instrument(skip_all)]
pub fn run_scene_detector(configuration: &Configuration, save_file: &Path) -> Result<()> {
    let mut condor = instantiate_saved_condor(configuration, save_file)?;

    run_scene_detector_tui(
        &mut condor,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )?;

    Ok(())
}

/// Applies the configured zones to the scenes already in the configuration,
/// without a TUI.
#[tracing::instrument(skip_all)]
pub fn run_zoner(configuration: &Configuration, save_file: &Path) -> Result<()> {
    let mut condor = instantiate_saved_condor(configuration, save_file)?;

    Zoner::run(&mut condor)?;

    Ok(())
}

#[tracing::instrument(skip_all)]
pub fn run_benchmarker(configuration: &Configuration, save_file: &Path) -> Result<()> {
    let mut condor = instantiate_saved_condor(configuration, save_file)?;

    run_benchmarker_tui(
        &mut condor,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )?;

    Ok(())
}

#[tracing::instrument(skip_all)]
pub fn run_noise_detector(configuration: &Configuration, save_file: &Path) -> Result<()> {
    let mut condor = instantiate_saved_condor(configuration, save_file)?;

    run_noise_detector_tui(
        &mut condor,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )?;

    Ok(())
}

#[tracing::instrument(skip_all)]
pub fn run_noise_scaler(configuration: &Configuration, save_file: &Path) -> Result<()> {
    let mut condor = instantiate_saved_condor(configuration, save_file)?;

    run_noise_scaler_tui(
        &mut condor,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )?;

    Ok(())
}

#[tracing::instrument(skip_all)]
pub fn run_target_quality(configuration: &Configuration, save_file: &Path) -> Result<()> {
    let mut condor = instantiate_saved_condor(configuration, save_file)?;

    run_target_quality_tui(
        &mut condor,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )?;

    Ok(())
}

#[tracing::instrument(skip_all)]
pub fn run_bitrate_optimizer(configuration: &Configuration, save_file: &Path) -> Result<()> {
    let mut condor = instantiate_saved_condor(configuration, save_file)?;

    run_bitrate_optimizer_tui(
        &mut condor,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )?;

    Ok(())
}

#[tracing::instrument(skip_all)]
pub fn run_speed_scaler(configuration: &Configuration, save_file: &Path) -> Result<()> {
    let mut condor = instantiate_saved_condor(configuration, save_file)?;

    run_speed_scaler_tui(
        &mut condor,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )?;

    Ok(())
}

#[tracing::instrument(skip_all)]
pub fn run_encoder(configuration: &Configuration, save_file: &Path) -> Result<()> {
    let mut condor = instantiate_saved_condor(configuration, save_file)?;

    run_parallel_encoder_tui(
        &mut condor,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )?;

    Ok(())
}

#[tracing::instrument(skip_all)]
pub fn run_concatenator(configuration: &Configuration, save_file: &Path) -> Result<()> {
    let mut condor = instantiate_saved_condor(configuration, save_file)?;

    run_scene_concatenator_tui(
        &mut condor,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )?;

    Ok(())
}

#[tracing::instrument(skip_all)]
pub fn run_quality_check(configuration: &Configuration, save_file: &Path) -> Result<()> {
    let mut condor = instantiate_saved_condor(configuration, save_file)?;

    run_quality_check_tui(
        &mut condor,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )?;

    Ok(())
}
