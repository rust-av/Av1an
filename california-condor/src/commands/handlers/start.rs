use std::path::{Path, PathBuf};

use andean_condor::{
    models::input::Input as InputModel,
    vapoursynth::vapoursynth_filters::VapourSynthFilter,
};
use anyhow::{Result, bail};
use tracing::{debug, error, trace, warn};

use crate::{
    DEFAULT_CONFIG_PATH,
    commands::{
        ConcatenationMethod,
        CondorCliError,
        DecoderMethod,
        DiscardCategory,
        EncoderMethod,
        QualityMetric,
        QualityProfile,
        handlers::{
            concatenate::{apply_discard, configure_concatenate},
            configure_input,
            configure_temp,
            detect_scenes::configure_scene_detector,
            encode::{configure_encoder, configure_parallel_encoder},
            target_quality::configure_target_quality,
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
) -> Result<(Configuration, PathBuf)> {
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
            let input = path_abs::PathAbs::new(input_path)?.as_path().to_path_buf();
            let output = path_abs::PathAbs::new(output_path)?.as_path().to_path_buf();
            debug!("TEMP: {temp:?}", temp = temp_path);
            Configuration::new(&input, &output, temp_path, vs_args, decoder)?
        }
    };

    configure_temp(&mut configuration, temp_path)?;
    if let Some(output) = output_path {
        let output = path_abs::PathAbs::new(output)?.as_path().to_path_buf();
        configuration.condor.output.path = output;
    }

    let existing_input = configuration.condor.input.clone();
    configuration.condor.input = configure_input(
        &configuration,
        &existing_input,
        input_path,
        decoder,
        vs_args,
        None,
        None,
    )?;
    if let Some(filters) = filters {
        let unsupported = InputModel::unsupported_filters(filters);
        configuration.condor.input.set_filters(filters.to_vec());
        for filter in unsupported {
            warn!(
                "{filter} needs VapourSynth and was dropped: a native FFMS2 input can only \
                 convert bit depth, chroma and resolution"
            );
        }
    }
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
    configure_scene_detector(
        &mut configuration,
        scd_input_path,
        scd_decoder,
        scd_filters,
        scd_vs_args,
        None,
        None,
        None,
    )?;
    configure_parallel_encoder(&mut configuration, None, None, None, None, workers)?;
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
    configure_concatenate(&mut configuration, concatenator)?;
    apply_discard(&mut configuration, discard.unwrap_or_default())?;

    if !config_already_existed {
        debug!("Saving new Configuration to {}", config_path.display());
    }
    configuration.save(&config_path)?;

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
                        ProbeStatistic,
                        ProbeStrategy,
                        QualityMetric,
                        SubsetProbeLength,
                        SubsetProbePosition,
                        TargetQualityProbing,
                    },
                },
            },
        },
        vapoursynth::{plugins::resize::Scaler, vapoursynth_filters::VapourSynthFilter},
    };
    use serial_test::serial;

    use super::*;
    use crate::{
        commands::{
            ConcatenationMethod,
            DecoderMethod,
            EncoderMethod,
            QualityMetric as QualityMetricBase,
        },
        test_helpers::{
            CwdGuard,
            check_benchmarker,
            check_bitrate_optimizer,
            check_encoder,
            check_input,
            check_noise_detector,
            check_noise_scaler,
            check_output,
            check_parallel_encoder,
            check_scene_concatenator,
            check_scene_detector,
            check_speed_scaler,
            check_target_quality,
            default_config,
            get_test_video,
            vapoursynth_script,
        },
        utils::hash_path::hash_path,
    };

    #[serial(needs_cwd)]
    #[test]
    fn start_creates_default_config() {
        let test_video = get_test_video();
        let input_abs = path_abs::PathAbs::new(&test_video.path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let temp = tempfile::tempdir().expect("temp directory");
        let temp_abs = path_abs::PathAbs::new(temp.path().join(hash_path(&input_abs)))
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let output = temp.path().join("out.mkv");
        let output_abs = path_abs::PathAbs::new(&output)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let config_path = temp.path().join("condor.json");
        let config_path_abs = path_abs::PathAbs::new(&config_path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();

        // Must run inside the temp directory so the default config path resolves there
        let _cwd_guard = CwdGuard::set(temp.path());

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
            top:    Some(42),
            bottom: Some(42),
            left:   None,
            right:  None,
        }];
        let custom_scd_filters = vec![VapourSynthFilter::Crop {
            top:    None,
            bottom: None,
            left:   Some(100),
            right:  Some(100),
        }];
        let custom_tq_filters = vec![VapourSynthFilter::Resize {
            scaler: Some(Scaler::Lanczos),
            width:  Some(960),
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
            path:          input_abs.clone(),
            import_method: ImportMethod::FFMS2 {
                index:      None,
                cache_path: None,
            },
            // `--filters` replaces the default conversion, and a native FFMS2
            // input cannot crop, so nothing remains.
            filters:       Vec::new(),
        };
        expected_config.condor.encoder = Encoder::X265 {
            executable:        None,
            pass:              EncoderPasses::All(3),
            options:           custom_encoder_parameters,
            ffmpeg_filter:     None,
            append_color_data: true,
        };
        expected_config.condor.sequence_config.scene_detector.input = Some(Input::VapourSynth {
            path:          input_abs.clone(),
            import_method: VapourSynthImportMethod::BestSource {
                index: None
            },
            cache_path:    None,
            filters:       custom_scd_filters.clone(),
        });
        expected_config.condor.sequence_config.parallel_encoder.workers = Some(6);
        let mut tq_encoder_parameters = EncoderBase::X265.default_parameters();
        tq_encoder_parameters.insert(
            "preset".to_owned(),
            CLIParameter::new_string("--", " ", "ultrafast"),
        );
        expected_config.condor.sequence_config.target_quality = Some(TargetQualityConfig {
            input: Some(Input::VapourSynth {
                path:          input_abs.clone(),
                import_method: VapourSynthImportMethod::FFMS2 {
                    index: None
                },
                cache_path:    None,
                filters:       custom_tq_filters.clone(),
            }),
            metric: QualityMetric::XPSNR {
                target_range: (39.0, 41.0),
                resolution:   None,
            },
            quantizer_range: (4, 35),
            probing: TargetQualityProbing {
                encoder_options: Some(tq_encoder_parameters),
                statistic:       ProbeStatistic::Percentile(10.0),
                strategy:        ProbeStrategy::Whole,
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
            Some(3),
            Some("--preset slow --crf 18"),
            Some("--preset ultrafast"),
            Some(404),
            Some(404),
            None,
            None,
            Some(&QualityMetricBase::XPSNR),
            Some(40.0),
            Some(4),
            Some(35),
            Some(&QualityProfile::Slow),
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
            top:    Some(42),
            bottom: Some(42),
            left:   None,
            right:  None,
        }];
        let custom_scd_filters = vec![VapourSynthFilter::Crop {
            top:    None,
            bottom: None,
            left:   Some(100),
            right:  Some(100),
        }];
        let custom_tq_filters = vec![VapourSynthFilter::Resize {
            scaler: Some(Scaler::Lanczos),
            width:  Some(960),
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
            path:          input_abs.clone(),
            import_method: ImportMethod::FFMS2 {
                index:      None,
                cache_path: None,
            },
            // `--filters` replaces the default conversion, and a native FFMS2
            // input cannot crop, so nothing remains.
            filters:       Vec::new(),
        };
        expected_config.condor.encoder = Encoder::X265 {
            executable:        None,
            pass:              EncoderPasses::All(3),
            options:           custom_encoder_parameters,
            ffmpeg_filter:     None,
            append_color_data: true,
        };
        expected_config.condor.sequence_config.scene_detector.input = Some(Input::VapourSynth {
            path:          input_abs.clone(),
            import_method: VapourSynthImportMethod::BestSource {
                index: None
            },
            cache_path:    None,
            filters:       custom_scd_filters.clone(),
        });
        expected_config.condor.sequence_config.parallel_encoder.workers = Some(6);
        let mut tq_encoder_parameters = EncoderBase::X265.default_parameters();
        tq_encoder_parameters.insert(
            "preset".to_owned(),
            CLIParameter::new_string("--", " ", "ultrafast"),
        );
        expected_config.condor.sequence_config.target_quality = Some(TargetQualityConfig {
            input: Some(Input::VapourSynth {
                path:          input_abs.clone(),
                import_method: VapourSynthImportMethod::FFMS2 {
                    index: None
                },
                cache_path:    None,
                filters:       custom_tq_filters.clone(),
            }),
            metric: QualityMetric::XPSNR {
                target_range: (39.0, 41.0),
                resolution:   None,
            },
            quantizer_range: (4, 35),
            probing: TargetQualityProbing {
                encoder_options: Some(tq_encoder_parameters),
                statistic:       ProbeStatistic::Percentile(10.0),
                strategy:        ProbeStrategy::Whole,
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
            Some(3),
            Some("--preset slow --crf 18"),
            Some("--preset ultrafast"),
            Some(404),
            Some(404),
            None,
            None,
            Some(&QualityMetricBase::XPSNR),
            Some(40.0),
            Some(4),
            Some(35),
            Some(&QualityProfile::Slow),
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
        expected_config.condor.input = Input::VapourSynthScript {
            source:              VapourSynthScriptSource::Path(script_input_abs),
            variables:           custom_variables,
            index:               0,
            filters:             custom_filters.clone(),
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
            executable:        None,
            pass:              EncoderPasses::All(1),
            options:           custom_encoder_parameters.clone(),
            ffmpeg_filter:     None,
            append_color_data: true,
        };
        if let Some(ref mut tq) = expected_config.condor.sequence_config.target_quality {
            tq.metric = QualityMetric::CVVDP {
                target_range:      (9.4, 9.6),
                resolution:        None,
                display_model:     None,
                resize_to_display: None,
                disable_temporal:  None,
                gpu_id:            None,
            };
            tq.probing.statistic = ProbeStatistic::RootMeanSquare;
            tq.probing.strategy = ProbeStrategy::Subset {
                position: SubsetProbePosition::Middle,
                length:   SubsetProbeLength::Percentage(25.0),
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
