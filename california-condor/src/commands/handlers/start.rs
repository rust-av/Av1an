use std::path::{Path, PathBuf};

use andean_condor::{
    models::encoder::{Encoder, EncoderBase},
    vapoursynth::vapoursynth_filters::VapourSynthFilter,
};
use anyhow::{Result, bail};
use tracing::{debug, error, trace};

use crate::{
    DEFAULT_CONFIG_PATH,
    commands::{
        ConcatenationMethod, CondorCliError, DecoderMethod, DiscardCategory, EncoderMethod,
        QualityMetric, QualityProfile,
        config::InputOverride,
        handlers::{
            concatenate::{apply_discard, configure_concatenate},
            configure_root_input, configure_temp,
            detect_scenes::configure_scene_detector_cancel,
            encode::{configure_encoder, configure_parallel_encoder, warn_existing_scene_encoders},
            target_quality::{configure_target_quality, require_target_quality_target},
        },
    },
    configuration::{ConfigError, Configuration},
};

#[allow(clippy::too_many_arguments)]
pub fn start_handler(
    config_path: Option<&Path>,
    temp_path: Option<&Path>,
    input_path: Option<&Path>,
    scd_input_path: Option<&Path>,
    tq_input_path: Option<&Path>,
    output_path: Option<&Path>,
    decoder: Option<&DecoderMethod>,
    scd_decoder: Option<&DecoderMethod>,
    tq_decoder: Option<&DecoderMethod>,
    filters: Option<&[VapourSynthFilter]>,
    scd_filters: Option<&[VapourSynthFilter]>,
    tq_filters: Option<&[VapourSynthFilter]>,
    vs_args: Option<&[String]>,
    scd_vs_args: Option<&[String]>,
    tq_vs_args: Option<&[String]>,
    concatenator: Option<&ConcatenationMethod>,
    discard: Option<&[DiscardCategory]>,
    workers: Option<u8>,
    encoder: Option<&EncoderMethod>,
    passes: Option<u8>,
    params: Option<&str>,
    tq_params: Option<&str>,
    photon_noise: Option<u32>,
    chroma_noise: Option<u32>,
    ffmpeg_filter: Option<&str>,
    append_color_data: Option<bool>,
    target_metric: Option<&QualityMetric>,
    target: Option<f64>,
    minimum_quantizer: Option<u8>,
    maximum_quantizer: Option<u8>,
    target_profile: Option<&QualityProfile>,
    max_tries: Option<u32>,
    overwrite: bool,
) -> Result<(Configuration, PathBuf)> {
    start_handler_cancel(
        config_path,
        temp_path,
        input_path,
        scd_input_path,
        tq_input_path,
        output_path,
        decoder,
        scd_decoder,
        tq_decoder,
        filters,
        scd_filters,
        tq_filters,
        vs_args,
        scd_vs_args,
        tq_vs_args,
        concatenator,
        discard,
        workers,
        encoder,
        passes,
        params,
        tq_params,
        photon_noise,
        chroma_noise,
        ffmpeg_filter,
        append_color_data,
        target_metric,
        target,
        minimum_quantizer,
        maximum_quantizer,
        target_profile,
        max_tries,
        overwrite,
        &Default::default(),
    )
}

/// Root startup, including preparatory opens, bound to the execution root.
#[allow(clippy::too_many_arguments)]
pub fn start_handler_cancel(
    config_path: Option<&Path>,
    temp_path: Option<&Path>,
    input_path: Option<&Path>,
    scd_input_path: Option<&Path>,
    tq_input_path: Option<&Path>,
    output_path: Option<&Path>,
    decoder: Option<&DecoderMethod>,
    scd_decoder: Option<&DecoderMethod>,
    tq_decoder: Option<&DecoderMethod>,
    filters: Option<&[VapourSynthFilter]>,
    scd_filters: Option<&[VapourSynthFilter]>,
    tq_filters: Option<&[VapourSynthFilter]>,
    vs_args: Option<&[String]>,
    scd_vs_args: Option<&[String]>,
    tq_vs_args: Option<&[String]>,
    concatenator: Option<&ConcatenationMethod>,
    discard: Option<&[DiscardCategory]>,
    workers: Option<u8>,
    encoder: Option<&EncoderMethod>,
    passes: Option<u8>,
    params: Option<&str>,
    tq_params: Option<&str>,
    photon_noise: Option<u32>,
    chroma_noise: Option<u32>,
    ffmpeg_filter: Option<&str>,
    append_color_data: Option<bool>,
    target_metric: Option<&QualityMetric>,
    target: Option<f64>,
    minimum_quantizer: Option<u8>,
    maximum_quantizer: Option<u8>,
    target_profile: Option<&QualityProfile>,
    max_tries: Option<u32>,
    overwrite: bool,
    cancellation: &andean_condor::utils::cancellation::Cancellation,
) -> Result<(Configuration, PathBuf)> {
    cancellation.check()?;
    anyhow::ensure!(
        max_tries != Some(0),
        "max_tries must be at least 1 total attempt"
    );
    if config_path.is_some_and(|p| !p.exists()) && (input_path.is_none() || output_path.is_none()) {
        let err = CondorCliError::NoConfigOrInputOrOutput;
        error!("{}", err);
        bail!(err);
    }
    let config_path =
        path_abs::PathAbs::new(config_path.unwrap_or_else(|| Path::new(DEFAULT_CONFIG_PATH)))?
            .as_path()
            .to_path_buf();
    let config_already_existed = config_path.exists();
    let tq_options_supplied = tq_input_path.is_some()
        || tq_decoder.is_some()
        || tq_filters.is_some()
        || tq_vs_args.is_some()
        || tq_params.is_some()
        || target_metric.is_some()
        || minimum_quantizer.is_some()
        || maximum_quantizer.is_some()
        || target_profile.is_some();
    if !config_already_existed {
        // New jobs must reject ignored options before Configuration::new opens the source.
        require_target_quality_target(false, target, tq_options_supplied)?;
    }

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
                    ConfigError::Parse(err) => {
                        let err = CondorCliError::ConfigParseError(err);
                        error!("{}", err);
                        bail!(err);
                    },
                    ConfigError::Serialize(err) => {
                        let err = CondorCliError::ConfigSerializeError(err);
                        error!("{}", err);
                        bail!(err);
                    },
                    ConfigError::Save(err) => {
                        let err = CondorCliError::ConfigSaveError(err);
                        error!("{}", err);
                        bail!(err);
                    },
                },
            }
        } else {
            trace!("No existing configuration found");
            let path_err = || {
                let err = CondorCliError::NoConfigOrInputOrOutput;
                error!("{}", err);
                err
            };
            let input_path = input_path.ok_or_else(path_err)?;
            let output_path = output_path.ok_or_else(path_err)?;
            debug!("Creating new configuration");
            let sources = [Some(input_path), scd_input_path, tq_input_path]
                .into_iter()
                .flatten()
                .map(Path::to_path_buf)
                .collect::<Vec<_>>();
            andean_condor::core::output::admit_output(output_path, &sources, overwrite, true)?;
            let cwd = std::env::current_dir()?;
            let input = cwd.join(input_path);
            let output = cwd.join(output_path);
            debug!("TEMP: {temp:?}", temp = temp_path);
            let mut selected_encoder = Encoder::default_from_base(&EncoderBase::SVTAV1, false);
            configure_encoder(
                &mut selected_encoder,
                encoder,
                passes,
                params,
                photon_noise,
                chroma_noise,
                ffmpeg_filter,
                append_color_data,
            )?;
            let mut configuration = Configuration::new_cancel(
                &input,
                &output,
                temp_path,
                vs_args,
                decoder,
                cancellation,
            )?;
            configuration.condor.encoder = selected_encoder;
            configuration
        }
    };

    if let (Some(_), Some(output)) = (
        output_path,
        configuration.condor.sequence_config.scene_concatenator.output.as_ref(),
    ) {
        bail!(
            "--output (-o) is shadowed by the saved concatenation output {}; run `av1an concatenate reset output` before choosing a root output, or `av1an concatenate set output PATH` to change the override",
            output.display()
        );
    }

    // Root admission precedes --temp/output edits, native work and all saves.
    configure_root_input(
        &mut configuration,
        &InputOverride {
            path: input_path.map(Path::to_path_buf),
            decoder: decoder.cloned(),
            filters: filters.map(<[_]>::to_vec),
            vs_args: vs_args.map(<[_]>::to_vec),
            stream_concurrently: None,
        },
    )?;
    configure_temp(&mut configuration, temp_path)?;
    if let Some(output) = output_path {
        let output = std::env::current_dir()?.join(output);
        configuration.condor.output.path = output;
    }
    if overwrite {
        configuration.condor.output.overwrite = true;
    }

    if config_already_existed {
        configure_encoder(
            &mut configuration.condor.encoder,
            encoder,
            passes,
            params,
            photon_noise,
            chroma_noise,
            ffmpeg_filter,
            append_color_data,
        )?;
    }
    configure_parallel_encoder(
        &mut configuration,
        None,
        None,
        None,
        None,
        workers,
        max_tries,
    )?;
    configuration.validate_encoder_settings()?;
    configure_concatenate(&mut configuration, concatenator)?;
    configuration.validate_output(true)?;
    let additional_sources = [input_path, scd_input_path, tq_input_path]
        .into_iter()
        .flatten()
        .map(Path::to_path_buf)
        .collect::<Vec<_>>();
    andean_condor::core::output::admit_output(
        configuration
            .condor
            .sequence_config
            .scene_concatenator
            .effective_output(&configuration.condor.output.path),
        &additional_sources,
        configuration.condor.output.overwrite,
        true,
    )?;
    require_target_quality_target(
        configuration.condor.sequence_config.target_quality.is_some(),
        target,
        tq_options_supplied,
    )?;
    configure_scene_detector_cancel(
        &mut configuration,
        scd_input_path,
        scd_decoder,
        scd_filters,
        scd_vs_args,
        None,
        None,
        None,
        cancellation,
    )?;
    // configure_benchmarker(
    //     &mut configuration,
    //     input_path.filter(|_| config_already_existed),
    //     decoder.filter(|_| config_already_existed),
    //     filters.filter(|_| config_already_existed),
    //     vs_args.filter(|_| config_already_existed),
    //     encoder,
    //     passes,
    //     params,
    //     threshold,
    //     max_memory,
    // )?;
    // configure_noise_detector(&mut configuration,
    // nd_input_path,
    // nd_vs_args)?; configure_noise_scaler(&mut
    // configuration, threshold, minimum_scaler, maximum_scaler, scale_chroma)?;
    configure_target_quality(
        &mut configuration,
        tq_input_path,
        tq_decoder,
        tq_filters,
        tq_vs_args,
        tq_params,
        target_metric,
        target,
        minimum_quantizer,
        maximum_quantizer,
        target_profile,
    )?;
    // configure_optimize_bitrate_handler(&mut configuration, sigma_threshold)?;
    // configure_scale_speed(&mut configuration, quantizers, speeds)?;
    configuration.validate_output(true)?;
    apply_discard(&mut configuration, discard.unwrap_or_default())?;

    if !config_already_existed {
        debug!("Saving new Configuration to {}", config_path.display());
    }
    configuration.admit_results()?;
    configuration.save(&config_path)?;
    if encoder.is_some()
        || passes.is_some()
        || params.is_some()
        || photon_noise.is_some()
        || chroma_noise.is_some()
        || ffmpeg_filter.is_some()
        || append_color_data.is_some()
    {
        warn_existing_scene_encoders(&configuration);
    }

    Ok((configuration, config_path))
}
