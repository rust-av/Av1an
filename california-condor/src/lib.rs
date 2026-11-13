//! Direct Rust job transactions must acquire [`configuration::JobLock`] before
//! loading, use its resolved path, and retain it through handlers and all `run_*`
//! helpers (including persistence callbacks and joins). These low-level helpers
//! do not lock or make stale snapshots safe. The CLI owns that outer lifetime.

use std::path::Path;

use andean_condor::core::Condor;
use andean_condor::utils::cancellation::Cancellation;
use anyhow::{Context, Result};
use clap::{CommandFactory, FromArgMatches};
use tracing::{debug, info, level_filters::LevelFilter};

use crate::{
    commands::{
        Commands, CondorCli, CondorCliError,
        handlers::{
            benchmarker::benchmarker_handler,
            concatenate::concatenate_handler,
            detect_noise::detect_noise_handler,
            detect_scenes::detect_scenes_handler_cancel,
            encode::encode_handler,
            init::init_handler_cancel,
            optimize_bitrate::optimize_bitrate_handler,
            quality_check::quality_check_handler,
            scale_noise::scale_noise_handler,
            scale_speed::scale_speed_handler,
            start::start_handler_cancel,
            target_quality::target_quality_handler,
            zone::{configure_zoner, zone_handler},
        },
        help_text::process_command_tree,
        version::print_version,
    },
    configuration::{Configuration, JobLock, normalize_config_path},
    logging::init_logging,
    tui::{
        run_benchmarker_tui, run_bitrate_optimizer_tui, run_noise_detector_tui,
        run_noise_scaler_tui, run_parallel_encoder_tui, run_quality_check_tui,
        run_scene_concatenator_tui, run_scene_detector_tui, run_speed_scaler_tui,
        run_target_quality_tui,
    },
    zoner::Zoner,
};

pub mod apps;
pub mod commands;
pub mod components;
pub mod configuration;
pub mod interrupt;
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

#[cfg(test)]
type TestEntryRun = Box<dyn FnOnce() -> Result<()>>;

#[cfg(test)]
thread_local! {
    // Exercise the real entry hook without parsing libtest's argv or running encoders.
    static TEST_ENTRY_RUN: std::cell::RefCell<Option<TestEntryRun>> = const { std::cell::RefCell::new(None) };
}

/// Entry point used by both `main()` binary and externally (e.g., tests).
pub fn entry_point() -> anyhow::Result<()> {
    entry_point_cancel(interrupt::Interrupt::default())
}

/// Library entry with explicit cancellation; does not install OS handlers.
pub fn entry_point_cancel(cancellation: impl Into<interrupt::Interrupt>) -> anyhow::Result<()> {
    apps::install_panic_hook();
    let cancellation = cancellation.into();
    cancellation.finish(run(&cancellation))
}

/// Process frontend: preserve library errors, but do not print a caught panic's
/// payload again after the installed hook has reported it during this run.
pub fn cli_entry_point() -> std::process::ExitCode {
    let before = apps::panic_reports();
    let cancellation = interrupt::Interrupt::default();
    let result = (|| {
        let mut listener = interrupt::Listener::start(cancellation.clone())?;
        let result = entry_point_cancel(cancellation.clone());
        let joined = listener.join();
        match (result, joined) {
            (Err(error), Err(cleanup)) => {
                Err(error.context(format!("interrupt listener cleanup: {cleanup:#}")))
            },
            (Err(error), Ok(())) => Err(error),
            (Ok(()), result) => result,
        }
    })();
    match cancellation.finish(result) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            let _ = write_cli_error(
                &mut std::io::stderr().lock(),
                &error,
                apps::panic_reports() != before,
            );
            std::process::ExitCode::from(
                error.downcast_ref::<interrupt::Interrupted>().map_or(1, |cause| cause.0),
            )
        },
    }
}

fn contains_caught_panic(error: &anyhow::Error) -> bool {
    error.downcast_ref::<andean_condor::utils::CaughtPanic>().is_some()
        || error
            .downcast_ref::<tui::SecondaryDisplayFailure>()
            .is_some_and(|failure| contains_caught_panic(&failure.display))
        || error
            .downcast_ref::<andean_condor::core::encoder::EncoderProcessError>()
            .is_some_and(|failure| failure.failures.iter().any(contains_caught_panic))
}

fn write_cli_error(
    writer: &mut impl std::io::Write,
    error: &anyhow::Error,
    reported: bool,
) -> std::io::Result<()> {
    if !reported || !contains_caught_panic(error) {
        // Match Result's normal process diagnostic; ordinary errors are unchanged.
        return writeln!(writer, "Error: {error:?}");
    }
    if let Some(failure) = error.downcast_ref::<tui::SecondaryDisplayFailure>() {
        // The first source is the retained primary, not the secondary's Display
        // text (which deliberately includes the full library panic payload).
        if let Some(primary) = error.chain().nth(1) {
            write_cli_cause(writer, primary)?;
        }
        return write_cli_error(writer, &failure.display, true);
    }
    write_cli_cause(writer, error.as_ref())
}

fn write_cli_cause(
    writer: &mut impl std::io::Write,
    error: &(dyn std::error::Error + 'static),
) -> std::io::Result<()> {
    let mut cause = Some(error);
    while let Some(current) = cause {
        if let Some(panic) = current.downcast_ref::<andean_condor::utils::CaughtPanic>() {
            return writeln!(
                writer,
                "Error: {} panicked (diagnostic above)",
                panic.operation
            );
        }
        if let Some(failure) =
            current.downcast_ref::<andean_condor::core::encoder::EncoderProcessError>()
        {
            // Captured codec diagnostics and nonpanic failures must not disappear
            // just because a different pipe worker panicked.
            if let Some(result) = &failure.result {
                writeln!(writer, "{result}")?;
            } else {
                writeln!(
                    writer,
                    "Encoder status unavailable\nSTDOUT:\n{}\nSTDERR:\n{}",
                    failure.stdout, failure.stderr
                )?;
            }
            for failure in &failure.failures {
                write_cli_error(writer, failure, true)?;
            }
            return Ok(());
        }
        cause = current.source();
    }
    writeln!(writer, "Error: {error}")?;
    let mut cause = error.source();
    while let Some(current) = cause {
        writeln!(writer, "Caused by: {current}")?;
        cause = current.source();
    }
    Ok(())
}

fn run(cancellation: &interrupt::Interrupt) -> anyhow::Result<()> {
    #[cfg(test)]
    if let Some(run) = TEST_ENTRY_RUN.with_borrow_mut(Option::take) {
        return run();
    }
    let mut cmd = CondorCli::command();
    cmd = process_command_tree(cmd);
    let matches = cmd.get_matches_mut();
    CondorCli::admit_root_arguments(&matches, &mut cmd).map_err(|err| err.exit())?;
    let mut cli = CondorCli::from_arg_matches(&matches).map_err(|err| err.exit())?;
    cli.normalize_zone_flags();
    cli.reject_indexed_add(&mut cmd);
    cli.reject_unused_temp(&mut cmd);
    commands::set_verbose(cli.verbose);

    if cli.version {
        return print_version(cli.verbose);
    }

    let cwd = std::env::current_dir()?;
    // Help/version exit above; clean is deliberately offline. All other routes,
    // including info/scene reads, share one coherent whole-job guard policy.
    let job_lock = if matches!(cli.command, Some(Commands::Clean { .. })) {
        None
    } else {
        let selected = cli.config_file.as_deref().unwrap_or_else(|| Path::new(DEFAULT_CONFIG_PATH));
        let path = normalize_config_path(selected)
            .with_context(|| format!("normalizing job config `{}`", selected.display()))?;
        if matches!(cli.command, None | Some(Commands::Init { edit: None, .. })) {
            std::fs::create_dir_all(path.parent().context("job config needs a parent directory")?)
                .with_context(|| format!("preparing job config parent for `{}`", path.display()))?;
        } else {
            // Metadata-only admission avoids artifacts for typos/missing jobs, but
            // existing (even corrupt) JSON must not be read until after locking.
            match std::fs::metadata(&path) {
                Ok(_) => (),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Err(if cli.config_file.is_some() {
                        CondorCliError::ConfigFileNotFound(path)
                    } else {
                        CondorCliError::NoConfig
                    }
                    .into());
                },
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("inspecting job config `{}`", path.display()));
                },
            }
        }
        Some(JobLock::acquire(&path)?)
    };
    let config_path = job_lock.as_ref().map(|guard| guard.path().to_path_buf());
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
    let _logging_guard = set_logs(&logs)?;
    let temp = cli.temp;

    let result = (|| -> Result<()> {
        match cli.command {
            Some(Commands::Init {
                edit,
                overwrite,
                input,
                output,
                decoder,
                filters,
                vs_args,
                concat,
                discard,
                workers,
                max_tries,
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
                init_handler_cancel(
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
                    max_tries,
                    overwrite,
                    &cancellation.token,
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
                    return commands::handlers::detect_scenes::config(
                        config_path.as_deref(),
                        &edit,
                    );
                }
                let (configuration, save_file) = detect_scenes_handler_cancel(
                    config_path.as_deref(),
                    temp.as_deref(),
                    input.as_deref(),
                    decoder.as_ref(),
                    filters.as_deref(),
                    vs_args.as_deref(),
                    method.as_ref(),
                    min_scene_seconds,
                    max_scene_seconds,
                    &cancellation.token,
                )?;

                run_scene_detector_cancel(&configuration, &save_file, cancellation.clone())?;
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

                run_benchmarker_cancel(&configuration, &save_file, cancellation.clone())?;
            },
            Some(Commands::Zone { zones, dry_run, .. }) => {
                let (configuration, save_file) = zone_handler(config_path.as_deref(), &zones)?;
                if dry_run {
                    info!(
                        "Zoner dry-run: previewing planned changes; no configuration or outputs will \
                     be saved"
                    );
                    let mut condor = configuration
                        .instantiate_condor_cancel(Box::new(|_| Ok(())), &cancellation.token)?;
                    Zoner::run(&mut condor)?;
                    info!("Zoner dry-run complete: no configuration changes were saved");
                } else {
                    run_zoner_cancel(&configuration, &save_file, cancellation.clone())?;
                }
            },
            Some(Commands::Scenes { index, action }) => {
                // The index comes first and is optional: without an action the
                // addressed scenes are printed, which is what a bare
                // `av1an scenes` performs for the whole list.
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

                run_noise_detector_cancel(&configuration, &save_file, cancellation.clone())?;
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

                run_noise_scaler_cancel(&configuration, &save_file, cancellation.clone())?;
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
                    return commands::handlers::target_quality::config(
                        config_path.as_deref(),
                        &edit,
                    );
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

                run_target_quality_cancel(&configuration, &save_file, cancellation.clone())?;
            },
            Some(Commands::OptimizeBitrate {
                edit,
                sigma_threshold,
            }) => {
                if let Some(edit) = edit {
                    return commands::handlers::optimize_bitrate::config(
                        config_path.as_deref(),
                        &edit,
                    );
                }
                let (configuration, save_file) =
                    optimize_bitrate_handler(config_path.as_deref(), sigma_threshold)?;

                run_bitrate_optimizer_cancel(&configuration, &save_file, cancellation.clone())?;
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

                run_speed_scaler_cancel(&configuration, &save_file, cancellation.clone())?;
            },
            Some(Commands::Encode {
                edit,
                input,
                decoder,
                filters,
                vs_args,
                workers,
                max_tries,
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
                    max_tries,
                )?;

                run_encoder_cancel(&configuration, &save_file, cancellation.clone())?;
            },
            Some(Commands::Concatenate {
                edit,
                overwrite,
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
                    overwrite,
                )?;

                run_concatenator_cancel(&configuration, &save_file, cancellation.clone())?;
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
                    return commands::handlers::quality_check::config(
                        config_path.as_deref(),
                        &edit,
                    );
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

                run_quality_check_cancel(&configuration, &save_file, cancellation.clone())?;
            },
            Some(Commands::Clean { all: _ }) => {
                anyhow::bail!(
                    "cleanup is unavailable; no files removed. Start a new job with a different \
                 --config-file, a distinct empty --temp directory, and a distinct output."
                );
            },
            None => {
                let (mut configuration, save_file) = start_handler_cancel(
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
                    cli.max_tries,
                    cli.overwrite,
                    &cancellation.token,
                )?;

                if let Some(zones) = &cli.zones {
                    configure_zoner(&mut configuration, zones)?;
                    // Zoner publishes definitions and their scene/barrier state together.
                }

                run_condor_cancel(
                    &configuration,
                    &save_file,
                    cli.skip_scd,
                    cancellation.clone(),
                )?;
            },
        }

        Ok(())
    })();
    let result = cancellation.finish(result);
    debug!(result = ?result, "CLI job cleanup complete");
    result
}

/// Instantiates a `Condor` that writes every save back to `save_file`.
///
/// The sequence runners below differ only in the sequence they hand the
/// `Condor` to.
fn instantiate_saved_condor(
    configuration: &Configuration,
    save_file: &Path,
    cancellation: &Cancellation,
) -> Result<Condor<configuration::CliSequenceData, configuration::CliSequenceConfig>> {
    configuration.admit_results()?;
    instantiate_editable_condor(configuration, save_file, cancellation)
}

// Explicit scene edits may publish withdrawn results; constructing a runtime
// input is not permission for any consumer to bypass the persisted barrier.
fn instantiate_editable_condor(
    configuration: &Configuration,
    save_file: &Path,
    cancellation: &Cancellation,
) -> Result<Condor<configuration::CliSequenceData, configuration::CliSequenceConfig>> {
    let config_copy = configuration.clone();
    let save_file_copy = save_file.to_path_buf();
    debug!("Instantiating Condor with {:?}", {
        // Remove scenes to reduce log spam
        let mut config = configuration.clone();
        config.condor.scenes = Vec::new();
        config
    });

    configuration.instantiate_condor_cancel(
        Box::new(move |data| {
            let mut config = config_copy.clone();
            config.condor = data;
            config.validate_output(false)?;
            Configuration::save(&config, &save_file_copy)?;
            Ok(())
        }),
        cancellation,
    )
}

#[tracing::instrument(skip_all)]
pub fn run_condor(configuration: &Configuration, save_file: &Path, skip_scd: bool) -> Result<()> {
    run_condor_cancel(
        configuration,
        save_file,
        skip_scd,
        interrupt::Interrupt::default(),
    )
}

/// Runs all stages with one root, including source/override input opening.
pub fn run_condor_cancel(
    configuration: &Configuration,
    save_file: &Path,
    skip_scd: bool,
    cancellation: impl Into<interrupt::Interrupt>,
) -> Result<()> {
    let cancellation_token = cancellation.into();
    let mut condor = instantiate_saved_condor(configuration, save_file, &cancellation_token.token)?;

    if !skip_scd {
        run_scene_detector_tui(&mut condor, cancellation_token.clone())?;
        cancellation_token.token.check()?;
    }

    // Removing all definitions is still a zoning transition, not a skip.
    Zoner::run(&mut condor)?;
    cancellation_token.token.check()?;
    // A zoning publication may have withdrawn previously admitted results.
    // Refuse before noise/probe producers, not only at the final PE stage.
    for (index, scene) in condor.scenes.iter().enumerate() {
        scene.sequence_data.parallel_encoder.admit_scene(index)?;
    }

    if configuration.condor.sequence_config.noise_detector.is_some() {
        run_noise_detector_tui(&mut condor, cancellation_token.clone())?;
        cancellation_token.token.check()?;

        run_noise_scaler_tui(&mut condor, cancellation_token.clone())?;
        cancellation_token.token.check()?;
    }

    if configuration.condor.sequence_config.parallel_encoder.workers.is_none() {
        run_benchmarker_tui(&mut condor, cancellation_token.clone())?;
        cancellation_token.token.check()?;
    }

    if configuration.condor.sequence_config.target_quality.is_some() {
        run_target_quality_tui(&mut condor, cancellation_token.clone())?;
        cancellation_token.token.check()?;

        if configuration
            .condor
            .sequence_config
            .bitrate_optimizer
            .bitrate_sigma_threshold
            .is_some()
        {
            run_bitrate_optimizer_tui(&mut condor, cancellation_token.clone())?;
            cancellation_token.token.check()?;
        }
    }

    if configuration.condor.sequence_config.speed_scaler.speed_quantizers.len() >= 2 {
        run_speed_scaler_tui(&mut condor, cancellation_token.clone())?;
        cancellation_token.token.check()?;
    }

    run_parallel_encoder_tui(&mut condor, cancellation_token.clone())?;
    cancellation_token.token.check()?;

    run_scene_concatenator_tui(&mut condor, cancellation_token.clone())?;
    cancellation_token.token.check()?;

    if configuration.condor.sequence_config.quality_check.is_some() {
        run_quality_check_tui(&mut condor, cancellation_token.clone())?;
        cancellation_token.token.check()?;
    }

    info!(
        "Condor has landed. Output: {}",
        condor
            .sequence_config
            .scene_concatenator
            .effective_output(&condor.output.path)
            .display()
    );
    info!("Have a nice day!");

    Ok(())
}

#[tracing::instrument(skip_all)]
pub fn run_scene_detector(configuration: &Configuration, save_file: &Path) -> Result<()> {
    run_scene_detector_cancel(configuration, save_file, interrupt::Interrupt::default())
}

/// Standalone sequence with caller-owned cancellation; no OS registration.
pub fn run_scene_detector_cancel(
    configuration: &Configuration,
    save_file: &Path,
    cancellation: impl Into<interrupt::Interrupt>,
) -> Result<()> {
    let cancellation = cancellation.into();
    let mut condor = instantiate_saved_condor(configuration, save_file, &cancellation.token)?;
    cancellation.finish(run_scene_detector_tui(&mut condor, cancellation.clone()))
}

/// Applies the configured zones to the scenes already in the configuration,
/// without a TUI.
#[tracing::instrument(skip_all)]
pub fn run_zoner(configuration: &Configuration, save_file: &Path) -> Result<()> {
    run_zoner_cancel(configuration, save_file, interrupt::Interrupt::default())
}

/// Editable scene transaction with cancellation-aware source opening.
pub fn run_zoner_cancel(
    configuration: &Configuration,
    save_file: &Path,
    cancellation: impl Into<interrupt::Interrupt>,
) -> Result<()> {
    let cancellation = cancellation.into();
    let mut condor = instantiate_editable_condor(configuration, save_file, &cancellation.token)?;
    cancellation.finish(Zoner::run(&mut condor))
}

#[tracing::instrument(skip_all)]
pub fn run_benchmarker(configuration: &Configuration, save_file: &Path) -> Result<()> {
    run_benchmarker_cancel(configuration, save_file, interrupt::Interrupt::default())
}

/// Standalone sequence with caller-owned cancellation; no OS registration.
pub fn run_benchmarker_cancel(
    configuration: &Configuration,
    save_file: &Path,
    cancellation: impl Into<interrupt::Interrupt>,
) -> Result<()> {
    let cancellation = cancellation.into();
    let mut condor = instantiate_saved_condor(configuration, save_file, &cancellation.token)?;
    cancellation.finish(run_benchmarker_tui(&mut condor, cancellation.clone()))
}

#[tracing::instrument(skip_all)]
pub fn run_noise_detector(configuration: &Configuration, save_file: &Path) -> Result<()> {
    run_noise_detector_cancel(configuration, save_file, interrupt::Interrupt::default())
}

/// Standalone sequence with caller-owned cancellation; no OS registration.
pub fn run_noise_detector_cancel(
    configuration: &Configuration,
    save_file: &Path,
    cancellation: impl Into<interrupt::Interrupt>,
) -> Result<()> {
    let cancellation = cancellation.into();
    let mut condor = instantiate_saved_condor(configuration, save_file, &cancellation.token)?;
    cancellation.finish(run_noise_detector_tui(&mut condor, cancellation.clone()))
}

#[tracing::instrument(skip_all)]
pub fn run_noise_scaler(configuration: &Configuration, save_file: &Path) -> Result<()> {
    run_noise_scaler_cancel(configuration, save_file, interrupt::Interrupt::default())
}

/// Standalone sequence with caller-owned cancellation; no OS registration.
pub fn run_noise_scaler_cancel(
    configuration: &Configuration,
    save_file: &Path,
    cancellation: impl Into<interrupt::Interrupt>,
) -> Result<()> {
    let cancellation = cancellation.into();
    let mut condor = instantiate_saved_condor(configuration, save_file, &cancellation.token)?;
    cancellation.finish(run_noise_scaler_tui(&mut condor, cancellation.clone()))
}

#[tracing::instrument(skip_all)]
pub fn run_target_quality(configuration: &Configuration, save_file: &Path) -> Result<()> {
    run_target_quality_cancel(configuration, save_file, interrupt::Interrupt::default())
}

/// Standalone sequence with caller-owned cancellation; no OS registration.
pub fn run_target_quality_cancel(
    configuration: &Configuration,
    save_file: &Path,
    cancellation: impl Into<interrupt::Interrupt>,
) -> Result<()> {
    let cancellation = cancellation.into();
    let mut condor = instantiate_saved_condor(configuration, save_file, &cancellation.token)?;
    cancellation.finish(run_target_quality_tui(&mut condor, cancellation.clone()))
}

#[tracing::instrument(skip_all)]
pub fn run_bitrate_optimizer(configuration: &Configuration, save_file: &Path) -> Result<()> {
    run_bitrate_optimizer_cancel(configuration, save_file, interrupt::Interrupt::default())
}

/// Standalone sequence with caller-owned cancellation; no OS registration.
pub fn run_bitrate_optimizer_cancel(
    configuration: &Configuration,
    save_file: &Path,
    cancellation: impl Into<interrupt::Interrupt>,
) -> Result<()> {
    let cancellation = cancellation.into();
    let mut condor = instantiate_saved_condor(configuration, save_file, &cancellation.token)?;
    cancellation.finish(run_bitrate_optimizer_tui(&mut condor, cancellation.clone()))
}

#[tracing::instrument(skip_all)]
pub fn run_speed_scaler(configuration: &Configuration, save_file: &Path) -> Result<()> {
    run_speed_scaler_cancel(configuration, save_file, interrupt::Interrupt::default())
}

/// Standalone sequence with caller-owned cancellation; no OS registration.
pub fn run_speed_scaler_cancel(
    configuration: &Configuration,
    save_file: &Path,
    cancellation: impl Into<interrupt::Interrupt>,
) -> Result<()> {
    let cancellation = cancellation.into();
    let mut condor = instantiate_saved_condor(configuration, save_file, &cancellation.token)?;
    cancellation.finish(run_speed_scaler_tui(&mut condor, cancellation.clone()))
}

#[tracing::instrument(skip_all)]
pub fn run_encoder(configuration: &Configuration, save_file: &Path) -> Result<()> {
    run_encoder_cancel(configuration, save_file, interrupt::Interrupt::default())
}

/// Standalone sequence with caller-owned cancellation; no OS registration.
pub fn run_encoder_cancel(
    configuration: &Configuration,
    save_file: &Path,
    cancellation: impl Into<interrupt::Interrupt>,
) -> Result<()> {
    let cancellation = cancellation.into();
    let mut condor = instantiate_saved_condor(configuration, save_file, &cancellation.token)?;
    cancellation.finish(run_parallel_encoder_tui(&mut condor, cancellation.clone()))
}

#[tracing::instrument(skip_all)]
pub fn run_concatenator(configuration: &Configuration, save_file: &Path) -> Result<()> {
    run_concatenator_cancel(configuration, save_file, interrupt::Interrupt::default())
}

/// Standalone sequence with caller-owned cancellation; no OS registration.
pub fn run_concatenator_cancel(
    configuration: &Configuration,
    save_file: &Path,
    cancellation: impl Into<interrupt::Interrupt>,
) -> Result<()> {
    let cancellation = cancellation.into();
    let mut condor = instantiate_saved_condor(configuration, save_file, &cancellation.token)?;
    cancellation.finish(run_scene_concatenator_tui(
        &mut condor,
        cancellation.clone(),
    ))
}

#[tracing::instrument(skip_all)]
pub fn run_quality_check(configuration: &Configuration, save_file: &Path) -> Result<()> {
    run_quality_check_cancel(configuration, save_file, interrupt::Interrupt::default())
}

/// Standalone sequence with caller-owned cancellation; no OS registration.
pub fn run_quality_check_cancel(
    configuration: &Configuration,
    save_file: &Path,
    cancellation: impl Into<interrupt::Interrupt>,
) -> Result<()> {
    let cancellation = cancellation.into();
    let mut condor = instantiate_saved_condor(configuration, save_file, &cancellation.token)?;
    cancellation.finish(run_quality_check_tui(&mut condor, cancellation.clone()))
}
