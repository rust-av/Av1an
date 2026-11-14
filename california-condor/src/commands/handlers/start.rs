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

#[cfg(test)]
mod tests {
    use std::{assert_matches, collections::HashMap, fs};

    use andean_condor::{
        ffmpeg::FFPixelFormat,
        models::{
            encoder::{Encoder, EncoderBase, EncoderPasses, cli_parameter::CLIParameter},
            input::{ImportMethod, Input, VapourSynthImportMethod, VapourSynthScriptSource},
            sequence::{
                scene_concatenator::ConcatMethod,
                target_quality::{
                    TargetQualityConfig,
                    types::{
                        ProbeStatistic, ProbeStrategy, QualityMetric, SubsetProbeLength,
                        SubsetProbePosition, TargetQualityProbing,
                    },
                },
            },
        },
        vapoursynth::{plugins::resize::Scaler, vapoursynth_filters::VapourSynthFilter},
    };

    use super::*;
    use crate::{
        commands::{
            ConcatenationMethod, DecoderMethod, EncoderMethod, QualityMetric as QualityMetricBase,
        },
        test_helpers::{
            check_benchmarker, check_bitrate_optimizer, check_encoder, check_input,
            check_noise_detector, check_noise_scaler, check_output, check_parallel_encoder,
            check_scene_concatenator, check_scene_detector, check_speed_scaler,
            check_target_quality, default_config, get_test_video, isolated_default_path_test,
            vapoursynth_script,
        },
        utils::hash_path::hash_path,
    };

    #[test]
    fn start_without_config_creates_defaults_in_isolated_child() {
        isolated_default_path_test(
            "commands::handlers::start::tests::start_without_config_creates_defaults_in_isolated_child",
            start_creates_default_config,
        );
    }

    fn start_creates_default_config(temp: &Path) {
        let test_video = get_test_video();
        let input_abs = path_abs::PathAbs::new(&test_video.path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let temp_abs = path_abs::PathAbs::new(temp.join(hash_path(&input_abs)))
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let output = temp.join("out.mkv");
        let output_abs = path_abs::PathAbs::new(&output)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let config_path = temp.join("condor.json");
        let config_path_abs = path_abs::PathAbs::new(&config_path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();

        let expected_config = default_config(&test_video, &output, &temp_abs);

        let (config, found_config_path) = start_handler(
            None, // defaults to ./condor.json
            None, // defaults to directory 7-character hash of input file
            Some(&test_video.path),
            None,
            None,
            Some(&output),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            false,
        )
        .expect("start_handler should succeed");

        assert_eq!(
            found_config_path,
            config_path_abs,
            "config path is {}",
            config_path_abs.display()
        );
        assert_eq!(config.temp, temp_abs, "temp path is {}", temp_abs.display());
        assert_eq!(
            config.input,
            input_abs,
            "input path is {}",
            input_abs.display()
        );
        assert_eq!(
            config.condor.output.path,
            expected_config.condor.output.path,
            "output path is {}",
            output_abs.display()
        );
        check_input(
            Some(&config.condor.input),
            Some(&expected_config.condor.input),
            "input",
        );
        check_output(&config.condor.output, &expected_config.condor.output);
        assert!(config.condor.scenes.is_empty(), "scenes is empty");
        check_scene_detector(
            &config.condor.sequence_config.scene_detector,
            &expected_config.condor.sequence_config.scene_detector,
        );
        check_encoder(
            &config.condor.encoder,
            &expected_config.condor.encoder,
            "encoder",
        );
        check_benchmarker(
            &config.condor.sequence_config.benchmarker,
            &expected_config.condor.sequence_config.benchmarker,
        );
        check_noise_detector(
            config.condor.sequence_config.noise_detector.as_ref(),
            expected_config.condor.sequence_config.noise_detector.as_ref(),
        );
        check_noise_scaler(
            config.condor.sequence_config.noise_scaler.as_ref(),
            expected_config.condor.sequence_config.noise_scaler.as_ref(),
        );
        check_target_quality(
            config.condor.sequence_config.target_quality.as_ref(),
            expected_config.condor.sequence_config.target_quality.as_ref(),
        );
        check_bitrate_optimizer(
            &config.condor.sequence_config.bitrate_optimizer,
            &expected_config.condor.sequence_config.bitrate_optimizer,
        );
        check_speed_scaler(
            &config.condor.sequence_config.speed_scaler,
            &expected_config.condor.sequence_config.speed_scaler,
        );
        check_parallel_encoder(
            &config.condor.sequence_config.parallel_encoder,
            &expected_config.condor.sequence_config.parallel_encoder,
        );
        check_scene_concatenator(
            &config.condor.sequence_config.scene_concatenator,
            &expected_config.condor.sequence_config.scene_concatenator,
        );
    }

    #[test]
    fn start_creates_custom_config() {
        let test_video = get_test_video();
        let input_abs = path_abs::PathAbs::new(&test_video.path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let temp = tempfile::tempdir().expect("temp directory");
        let temp_abs = path_abs::PathAbs::new(temp.path().join("custom-temp"))
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let output = temp.path().join("out.mkv");
        let output_abs = path_abs::PathAbs::new(&output)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let config_path = temp.path().join("custom-condor.json");
        let config_path_abs = path_abs::PathAbs::new(&config_path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let custom_filters = vec![VapourSynthFilter::Crop {
            top: Some(42),
            bottom: Some(42),
            left: None,
            right: None,
        }];
        let custom_scd_filters = vec![VapourSynthFilter::Crop {
            top: None,
            bottom: None,
            left: Some(100),
            right: Some(100),
        }];
        let custom_tq_filters = vec![VapourSynthFilter::Resize {
            scaler: Some(Scaler::Lanczos),
            width: Some(960),
            height: Some(540),
            format: Some(FFPixelFormat::YUV444P10LE),
        }];
        let custom_vs_args = vec!["key=value".to_string()];
        let mut custom_encoder_parameters = EncoderBase::X265.default_parameters();
        custom_encoder_parameters.insert(
            "preset".to_owned(),
            CLIParameter::new_string("--", " ", "slow"),
        );
        custom_encoder_parameters
            .insert("crf".to_owned(), CLIParameter::new_number("--", " ", 18.0));

        let mut expected_config = default_config(&test_video, &output, &temp_abs);
        expected_config.condor.input = Input::Video {
            path: input_abs.clone(),
            import_method: ImportMethod::FFMS2 {
                index: None,
                cache_path: None,
            },
            // `--filters` replaces the default conversion, and a native FFMS2
            // input cannot crop, so nothing remains.
            filters: Vec::new(),
        };
        expected_config.condor.encoder = Encoder::X265 {
            executable: None,
            pass: EncoderPasses::All(2),
            options: custom_encoder_parameters,
            ffmpeg_filter: None,
            append_color_data: true,
        };
        expected_config.condor.sequence_config.scene_detector.input = Some(Input::VapourSynth {
            path: input_abs.clone(),
            import_method: VapourSynthImportMethod::BestSource { index: None },
            cache_path: None,
            filters: custom_scd_filters.clone(),
        });
        expected_config.condor.sequence_config.parallel_encoder.workers = Some(6);
        let mut tq_encoder_parameters = EncoderBase::X265.default_parameters();
        tq_encoder_parameters.insert(
            "preset".to_owned(),
            CLIParameter::new_string("--", " ", "ultrafast"),
        );
        expected_config.condor.sequence_config.target_quality = Some(TargetQualityConfig {
            input: Some(Input::VapourSynth {
                path: input_abs.clone(),
                import_method: VapourSynthImportMethod::FFMS2 { index: None },
                cache_path: None,
                filters: custom_tq_filters.clone(),
            }),
            metric: QualityMetric::XPSNR {
                target_range: (39.0, 41.0),
                resolution: None,
            },
            quantizer_range: (4, 35),
            probing: TargetQualityProbing {
                encoder_options: Some(tq_encoder_parameters),
                statistic: ProbeStatistic::Percentile(10.0),
                strategy: ProbeStrategy::Whole,
            },
            ..Default::default()
        });
        expected_config.condor.sequence_config.scene_concatenator.method = ConcatMethod::Ivf;
        // immutable shadow
        let expected_config = expected_config;

        let (config, found_config_path) = start_handler(
            Some(&config_path),
            Some(&temp.path().join("custom-temp")),
            Some(&test_video.path),
            Some(&test_video.path),
            Some(&test_video.path),
            Some(&output),
            Some(&DecoderMethod::FFMS2),
            Some(&DecoderMethod::BestSource),
            Some(&DecoderMethod::VSFFMS2),
            Some(&custom_filters),
            Some(&custom_scd_filters),
            Some(&custom_tq_filters),
            Some(&custom_vs_args.clone()),
            Some(&custom_vs_args.clone()),
            Some(&custom_vs_args),
            Some(&ConcatenationMethod::Ivf),
            None,
            Some(6),
            Some(&EncoderMethod::X265),
            Some(2),
            Some("--preset slow --crf 18"),
            Some("--preset ultrafast"),
            None,
            None,
            None,
            None,
            Some(&QualityMetricBase::XPSNR),
            Some(40.0),
            Some(4),
            Some(35),
            Some(&QualityProfile::Slow),
            None,
            false,
        )
        .expect("start_handler should succeed");

        assert_eq!(
            found_config_path,
            config_path_abs,
            "config path is {}",
            config_path_abs.display()
        );
        assert_eq!(config.temp, temp_abs, "temp path is {}", temp_abs.display());
        assert_eq!(
            config.input,
            input_abs,
            "input path is {}",
            input_abs.display()
        );
        assert_eq!(
            config.condor.output.path,
            expected_config.condor.output.path,
            "output path is {}",
            output_abs.display()
        );
        check_input(
            Some(&config.condor.input),
            Some(&expected_config.condor.input),
            "input",
        );
        check_output(&config.condor.output, &expected_config.condor.output);
        assert!(config.condor.scenes.is_empty(), "scenes is empty");
        check_scene_detector(
            &config.condor.sequence_config.scene_detector,
            &expected_config.condor.sequence_config.scene_detector,
        );
        check_encoder(
            &config.condor.encoder,
            &expected_config.condor.encoder,
            "encoder",
        );
        check_benchmarker(
            &config.condor.sequence_config.benchmarker,
            &expected_config.condor.sequence_config.benchmarker,
        );
        check_noise_detector(
            config.condor.sequence_config.noise_detector.as_ref(),
            expected_config.condor.sequence_config.noise_detector.as_ref(),
        );
        check_noise_scaler(
            config.condor.sequence_config.noise_scaler.as_ref(),
            expected_config.condor.sequence_config.noise_scaler.as_ref(),
        );
        check_target_quality(
            config.condor.sequence_config.target_quality.as_ref(),
            expected_config.condor.sequence_config.target_quality.as_ref(),
        );
        check_bitrate_optimizer(
            &config.condor.sequence_config.bitrate_optimizer,
            &expected_config.condor.sequence_config.bitrate_optimizer,
        );
        check_speed_scaler(
            &config.condor.sequence_config.speed_scaler,
            &expected_config.condor.sequence_config.speed_scaler,
        );
        check_parallel_encoder(
            &config.condor.sequence_config.parallel_encoder,
            &expected_config.condor.sequence_config.parallel_encoder,
        );
        check_scene_concatenator(
            &config.condor.sequence_config.scene_concatenator,
            &expected_config.condor.sequence_config.scene_concatenator,
        );
    }

    #[test]
    fn start_updates_custom_config() {
        let test_video = get_test_video();
        let input_abs = path_abs::PathAbs::new(&test_video.path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let temp = tempfile::tempdir().expect("temp directory");
        let temp_abs = path_abs::PathAbs::new(temp.path().join("custom-temp"))
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let output = temp.path().join("out.mkv");
        let output_abs = path_abs::PathAbs::new(&output)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let config_path = temp.path().join("custom-condor.json");
        let config_path_abs = path_abs::PathAbs::new(&config_path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let custom_filters = vec![VapourSynthFilter::Crop {
            top: Some(42),
            bottom: Some(42),
            left: None,
            right: None,
        }];
        let custom_scd_filters = vec![VapourSynthFilter::Crop {
            top: None,
            bottom: None,
            left: Some(100),
            right: Some(100),
        }];
        let custom_tq_filters = vec![VapourSynthFilter::Resize {
            scaler: Some(Scaler::Lanczos),
            width: Some(960),
            height: Some(540),
            format: Some(FFPixelFormat::YUV444P10LE),
        }];
        let custom_vs_args = vec!["key=value".to_string()];
        let mut custom_encoder_parameters = EncoderBase::X265.default_parameters();
        custom_encoder_parameters.insert(
            "preset".to_owned(),
            CLIParameter::new_string("--", " ", "slow"),
        );
        custom_encoder_parameters
            .insert("crf".to_owned(), CLIParameter::new_number("--", " ", 18.0));

        let mut expected_config = default_config(&test_video, &output, &temp_abs);
        expected_config.condor.input = Input::Video {
            path: input_abs.clone(),
            import_method: ImportMethod::FFMS2 {
                index: None,
                cache_path: None,
            },
            // `--filters` replaces the default conversion, and a native FFMS2
            // input cannot crop, so nothing remains.
            filters: Vec::new(),
        };
        expected_config.condor.encoder = Encoder::X265 {
            executable: None,
            pass: EncoderPasses::All(2),
            options: custom_encoder_parameters,
            ffmpeg_filter: None,
            append_color_data: true,
        };
        expected_config.condor.sequence_config.scene_detector.input = Some(Input::VapourSynth {
            path: input_abs.clone(),
            import_method: VapourSynthImportMethod::BestSource { index: None },
            cache_path: None,
            filters: custom_scd_filters.clone(),
        });
        expected_config.condor.sequence_config.parallel_encoder.workers = Some(6);
        let mut tq_encoder_parameters = EncoderBase::X265.default_parameters();
        tq_encoder_parameters.insert(
            "preset".to_owned(),
            CLIParameter::new_string("--", " ", "ultrafast"),
        );
        expected_config.condor.sequence_config.target_quality = Some(TargetQualityConfig {
            input: Some(Input::VapourSynth {
                path: input_abs.clone(),
                import_method: VapourSynthImportMethod::FFMS2 { index: None },
                cache_path: None,
                filters: custom_tq_filters.clone(),
            }),
            metric: QualityMetric::XPSNR {
                target_range: (39.0, 41.0),
                resolution: None,
            },
            quantizer_range: (4, 35),
            probing: TargetQualityProbing {
                encoder_options: Some(tq_encoder_parameters),
                statistic: ProbeStatistic::Percentile(10.0),
                strategy: ProbeStrategy::Whole,
            },
            ..Default::default()
        });
        expected_config.condor.sequence_config.scene_concatenator.method = ConcatMethod::Ivf;
        // immutable shadow
        let expected_config = expected_config;

        let (config, found_config_path) = start_handler(
            Some(&config_path),
            Some(&temp.path().join("custom-temp")),
            Some(&test_video.path),
            Some(&test_video.path),
            Some(&test_video.path),
            Some(&output),
            Some(&DecoderMethod::FFMS2),
            Some(&DecoderMethod::BestSource),
            Some(&DecoderMethod::VSFFMS2),
            Some(&custom_filters),
            Some(&custom_scd_filters),
            Some(&custom_tq_filters),
            Some(&custom_vs_args.clone()),
            Some(&custom_vs_args.clone()),
            Some(&custom_vs_args),
            Some(&ConcatenationMethod::Ivf),
            None,
            Some(6),
            Some(&EncoderMethod::X265),
            Some(2),
            Some("--preset slow --crf 18"),
            Some("--preset ultrafast"),
            None,
            None,
            None,
            None,
            Some(&QualityMetricBase::XPSNR),
            Some(40.0),
            Some(4),
            Some(35),
            Some(&QualityProfile::Slow),
            None,
            false,
        )
        .expect("start_handler should succeed");

        assert_eq!(
            found_config_path,
            config_path_abs,
            "config path is {}",
            config_path_abs.display()
        );
        assert_eq!(config.temp, temp_abs, "temp path is {}", temp_abs.display());
        assert_eq!(
            config.input,
            input_abs,
            "input path is {}",
            input_abs.display()
        );
        assert_eq!(
            config.condor.output.path,
            expected_config.condor.output.path,
            "output path is {}",
            output_abs.display()
        );
        check_input(
            Some(&config.condor.input),
            Some(&expected_config.condor.input),
            "input",
        );
        check_output(&config.condor.output, &expected_config.condor.output);
        assert!(config.condor.scenes.is_empty(), "scenes is empty");
        check_scene_detector(
            &config.condor.sequence_config.scene_detector,
            &expected_config.condor.sequence_config.scene_detector,
        );
        check_encoder(
            &config.condor.encoder,
            &expected_config.condor.encoder,
            "encoder",
        );
        check_benchmarker(
            &config.condor.sequence_config.benchmarker,
            &expected_config.condor.sequence_config.benchmarker,
        );
        check_noise_detector(
            config.condor.sequence_config.noise_detector.as_ref(),
            expected_config.condor.sequence_config.noise_detector.as_ref(),
        );
        check_noise_scaler(
            config.condor.sequence_config.noise_scaler.as_ref(),
            expected_config.condor.sequence_config.noise_scaler.as_ref(),
        );
        check_target_quality(
            config.condor.sequence_config.target_quality.as_ref(),
            expected_config.condor.sequence_config.target_quality.as_ref(),
        );
        check_bitrate_optimizer(
            &config.condor.sequence_config.bitrate_optimizer,
            &expected_config.condor.sequence_config.bitrate_optimizer,
        );
        check_speed_scaler(
            &config.condor.sequence_config.speed_scaler,
            &expected_config.condor.sequence_config.speed_scaler,
        );
        check_parallel_encoder(
            &config.condor.sequence_config.parallel_encoder,
            &expected_config.condor.sequence_config.parallel_encoder,
        );
        check_scene_concatenator(
            &config.condor.sequence_config.scene_concatenator,
            &expected_config.condor.sequence_config.scene_concatenator,
        );
        drop(config);
        drop(found_config_path);

        let script_input = temp.path().join("condor-test-script.vpy");
        let script_input_abs = path_abs::PathAbs::new(&script_input)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let vpy_script = vapoursynth_script(&test_video, Some(&custom_filters));
        // Save the VapourSynth script to the temp directory
        fs::write(&script_input, vpy_script).expect("write should succeed");

        // mutable shadow
        let mut expected_config = expected_config;
        let mut custom_variables = HashMap::new();
        custom_variables.insert("key".to_owned(), "value".to_owned());
        expected_config.input = script_input_abs.clone();
        expected_config.condor.input = Input::VapourSynthScript {
            source: VapourSynthScriptSource::Path(script_input_abs),
            variables: custom_variables,
            index: 0,
            filters: custom_filters.clone(),
            stream_concurrently: true,
        };
        let mut custom_encoder_parameters = EncoderBase::X264.default_parameters();
        custom_encoder_parameters.insert(
            "preset".to_owned(),
            CLIParameter::new_string("--", " ", "medium"),
        );
        custom_encoder_parameters
            .insert("crf".to_owned(), CLIParameter::new_number("--", " ", 12.0));
        expected_config.condor.encoder = Encoder::X264 {
            executable: None,
            pass: EncoderPasses::All(1),
            options: custom_encoder_parameters.clone(),
            ffmpeg_filter: None,
            append_color_data: true,
        };
        if let Some(ref mut tq) = expected_config.condor.sequence_config.target_quality {
            tq.metric = QualityMetric::CVVDP {
                target_range: (9.4, 9.6),
                resolution: None,
                display_model: None,
                resize_to_display: None,
                disable_temporal: None,
                gpu_id: None,
            };
            tq.probing.statistic = ProbeStatistic::RootMeanSquare;
            tq.probing.strategy = ProbeStrategy::Subset {
                position: SubsetProbePosition::Middle,
                length: SubsetProbeLength::Percentage(25.0),
            };
            tq.quantizer_range.1 = 25;
        }
        expected_config.condor.sequence_config.parallel_encoder.workers = Some(12);
        expected_config.condor.sequence_config.scene_concatenator.method =
            ConcatMethod::MKVMerge(Default::default());
        // immutable shadow
        let expected_config = expected_config;

        let (config, found_config_path) = start_handler(
            Some(&config_path),
            Some(&temp.path().join("custom-temp")),
            Some(&script_input),
            Some(&test_video.path),
            Some(&test_video.path),
            Some(&output),
            None,
            None,
            None,
            Some(&custom_filters),
            None,
            None,
            Some(&custom_vs_args),
            None,
            None,
            Some(&ConcatenationMethod::MkvMerge),
            None,
            Some(12),
            Some(&EncoderMethod::X264),
            Some(1),
            Some("--preset medium --crf 12"),
            None,
            None,
            None,
            None,
            None,
            Some(&QualityMetricBase::CVVDP),
            Some(9.5),
            Some(4),
            Some(25),
            Some(&QualityProfile::Standard),
            None,
            false,
        )
        .expect("start_handler should succeed");

        assert!(config_path.exists(), "config file exists");

        assert_eq!(
            found_config_path,
            config_path_abs,
            "config path is {}",
            config_path_abs.display()
        );
        assert_eq!(config.temp, temp_abs, "temp path is {}", temp_abs.display());
        assert_eq!(
            config.input,
            expected_config.input,
            "input path is {}",
            expected_config.input.display()
        );
        assert_eq!(
            config.condor.output.path,
            expected_config.condor.output.path,
            "output path is {}",
            output_abs.display()
        );
        check_input(
            Some(&config.condor.input),
            Some(&expected_config.condor.input),
            "input",
        );
        check_output(&config.condor.output, &expected_config.condor.output);
        assert!(config.condor.scenes.is_empty(), "scenes is empty");
        check_scene_detector(
            &config.condor.sequence_config.scene_detector,
            &expected_config.condor.sequence_config.scene_detector,
        );
        check_encoder(
            &config.condor.encoder,
            &expected_config.condor.encoder,
            "encoder",
        );
        check_benchmarker(
            &config.condor.sequence_config.benchmarker,
            &expected_config.condor.sequence_config.benchmarker,
        );
        check_noise_detector(
            config.condor.sequence_config.noise_detector.as_ref(),
            expected_config.condor.sequence_config.noise_detector.as_ref(),
        );
        check_noise_scaler(
            config.condor.sequence_config.noise_scaler.as_ref(),
            expected_config.condor.sequence_config.noise_scaler.as_ref(),
        );
        check_target_quality(
            config.condor.sequence_config.target_quality.as_ref(),
            expected_config.condor.sequence_config.target_quality.as_ref(),
        );
        check_bitrate_optimizer(
            &config.condor.sequence_config.bitrate_optimizer,
            &expected_config.condor.sequence_config.bitrate_optimizer,
        );
        check_speed_scaler(
            &config.condor.sequence_config.speed_scaler,
            &expected_config.condor.sequence_config.speed_scaler,
        );
        check_parallel_encoder(
            &config.condor.sequence_config.parallel_encoder,
            &expected_config.condor.sequence_config.parallel_encoder,
        );
        check_scene_concatenator(
            &config.condor.sequence_config.scene_concatenator,
            &expected_config.condor.sequence_config.scene_concatenator,
        );
    }

    /// `--disable-color-data` writes `append_color_data: false` into the
    /// encoder the configuration starts with.
    #[test]
    fn start_disable_color_data_turns_appending_off() {
        let test_video = get_test_video();
        let temp = tempfile::tempdir().expect("temp directory");
        let output = temp.path().join("out.mkv");
        let config_path = temp.path().join("condor.json");

        let (config, _) = start_handler(
            Some(&config_path),
            None,
            Some(&test_video.path),
            None,
            None,
            Some(&output),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(false),
            None,
            None,
            None,
            None,
            None,
            None,
            false,
        )
        .expect("start_handler should succeed");

        assert!(
            !config.condor.encoder.append_color_data(),
            "append_color_data is false"
        );
    }

    #[test]
    fn start_input_not_found() {
        // let test_video = get_test_video();
        let invalid_input = std::env::temp_dir().join("not_found.mkv");
        let input_abs = path_abs::PathAbs::new(&invalid_input)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let temp = tempfile::tempdir().expect("temp directory");
        let output = temp.path().join("out.mkv");
        let config_path = temp.path().join("condor.json");

        let result = start_handler(
            Some(&config_path),
            Some(&temp.path().join(hash_path(&input_abs))),
            Some(&invalid_input),
            None,
            None,
            Some(&output),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            false,
        );
        assert_matches!(result, Err(_), "start_handler should fail");
        assert!(!config_path.exists(), "config file does not exist");
        // TODO: Validate inputs exist
        // let result = start_handler(
        //     Some(&config_path),
        //     Some(&temp.path().join(hash_path(&input_abs))),
        //     Some(&test_video.path),
        //     Some(&invalid_input),
        //     None,
        //     Some(&output),
        //     None,
        //     None,
        //     None,
        //     None,
        //     None,
        //     None,
        //     None,
        //     None,
        //     None,
        //     None,
        //     None,
        //     None,
        //     None,
        //     None,
        //     None,
        //     None,
        //     None,
        //     None,
        //     None,
        //     None,
        //     None,
        //     None,
        // );
        // assert_matches!(result, Err(_), "start_handler should fail");
        // assert!(!config_path.exists(), "config file does not exist");

        // let result = start_handler(
        //     Some(&config_path),
        //     Some(&temp.path().join(hash_path(&input_abs))),
        //     Some(&test_video.path),
        //     None,
        //     Some(&invalid_input),
        //     Some(&output),
        //     None,
        //     None,
        //     None,
        //     None,
        //     None,
        //     None,
        //     None,
        //     None,
        //     None,
        //     None,
        //     None,
        //     None,
        //     None,
        //     None,
        //     None,
        //     None,
        //     None,
        //     None,
        //     None,
        //     None,
        //     None,
        //     None,
        // );
        // assert_matches!(result, Err(_), "start_handler should fail");
        // assert!(!config_path.exists(), "config file does not exist");
    }
}
