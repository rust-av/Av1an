use std::path::{Path, PathBuf};

use andean_condor::{
    core::sequence::target_quality::TargetQuality,
    models::{
        encoder::Encoder,
        sequence::{
            scene_concatenator::SceneConcatenatorConfig,
            target_quality::TargetQualityConfig,
        },
    },
    vapoursynth::vapoursynth_filters::VapourSynthFilter,
};
use anyhow::{Result, bail};
use clap::{Subcommand, value_parser};
use serde_json::{Value, json};
use thiserror::Error;
use tracing::{error, info, warn};

use crate::{
    DEFAULT_CONFIG_PATH,
    commands::{
        ConcatenationMethod,
        CondorCliError,
        DecoderMethod,
        DiscardCategory,
        EncoderMethod,
        QualityMetric as QualityMetricBase,
        config::{ConfigAction, ConfigCommand, InputOverride, PropertyChange},
        handlers::{
            apply_input_filters,
            apply_stream_concurrently,
            concatenate::{apply_discard, configure_concatenate},
            config::{edit, ensure_override_requested},
            configure_input,
            configure_temp,
            encode::{configure_encoder, configure_parallel_encoder},
            target_quality::{TargetQualityCrud, configure_target_quality},
            warn_missing_path,
            warn_unused_vs_args,
        },
    },
    configuration::Configuration,
};

#[allow(clippy::too_many_arguments)]
pub fn init_handler(
    config_path: Option<&Path>,
    temp_path: Option<&Path>,
    input_path: &Path,
    output_path: &Path,
    decoder: Option<&DecoderMethod>,
    filters: Option<&[VapourSynthFilter]>,
    vs_args: Option<&[String]>,
    concatenator: Option<&ConcatenationMethod>,
    discard: Option<&[DiscardCategory]>,
    workers: Option<u8>,
    encoder: Option<&EncoderMethod>,
    params: Option<&str>,
    photon_noise: Option<u32>,
    append_color_data: bool,
    target_metric: Option<&QualityMetricBase>,
    target: Option<f64>,
) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let input = path_abs::PathAbs::new(input_path)?.as_path().to_path_buf();
    let output = path_abs::PathAbs::new(output_path)?.as_path().to_path_buf();
    let config_path = path_abs::PathAbs::new(
        config_path.map_or_else(|| cwd.join(DEFAULT_CONFIG_PATH), |p| p.to_path_buf()),
    )?
    .as_path()
    .to_path_buf();

    if config_path.exists() {
        let err = CondorCliError::ConfigFileAlreadyExists(config_path);
        error!("{}", err);
        bail!(err);
    }

    let mut configuration = Configuration::new(&input, &output, temp_path, vs_args, decoder)?;
    // A video input never consumes script arguments; say so instead of
    // dropping them silently.
    warn_unused_vs_args(&configuration.condor.input, vs_args);

    configure_temp(&mut configuration, temp_path)?;
    if let Some(filters) = filters {
        apply_input_filters(&mut configuration.condor.input, filters);
    }
    configure_encoder(
        &mut configuration.condor.encoder,
        encoder,
        None,
        params,
        photon_noise,
        None,
        None,
        Some(append_color_data),
    )?;
    configure_parallel_encoder(&mut configuration, None, None, None, None, workers)?;
    configure_target_quality(
        &mut configuration,
        None,
        None,
        None,
        None,
        None,
        target_metric,
        target,
        None,
        None,
        None,
    )?;
    configure_concatenate(&mut configuration, concatenator)?;
    apply_discard(&mut configuration, discard.unwrap_or_default())?;

    configuration.save(&config_path)?;

    info!(
        "Initialized Condor configuration at: {}",
        config_path.display()
    );
    info!(
        "Run \"condor\" to start, or \"condor init info\" / \"condor init set\" to adjust the \
         top-level settings."
    );

    Ok(())
}

/// `info` and `reset` keys of the top-level `condor` settings.
#[derive(Debug, Clone, Subcommand)]
pub enum InitKey {
    /// The input video or VapourSynth script.
    Input,
    /// The file the pipeline writes.
    Output,
    /// The encoder and its parameters.
    Encoder,
    /// How many encoder processes run at once.
    Workers,
    /// The tool used to join the encoded scenes.
    Concatenator,
    /// The quality metric Target Quality aims for.
    TargetMetric,
    /// The score Target Quality aims for.
    Target,
}

/// Settings `condor init set` can change.
#[derive(Debug, Clone, Subcommand)]
pub enum InitSet {
    /// The input video or VapourSynth script.
    ///
    /// Builds the same input the *--decoder*, *--filters* and *--vs-args* flags
    /// of `init` would build; omitted flags keep the current input's settings.
    Input {
        #[command(flatten)]
        args: InputOverride,
    },
    /// The file the pipeline writes.
    Output {
        #[arg(value_name = "Path")]
        path: PathBuf,
    },
    /// The encoder and its parameters.
    ///
    /// Provide at least one of the flags; omitted ones keep their current
    /// value.
    Encoder {
        /// The encoder to use.
        #[arg(long, value_name = "Encoder")]
        encoder:      Option<EncoderMethod>,
        /// Custom encoder binary path.
        #[arg(long, value_name = "Path")]
        executable:   Option<PathBuf>,
        /// Encoder parameters (`--crf 16 --tune 0`), exactly like *--params*.
        #[arg(long, value_name = "Parameters", allow_hyphen_values = true)]
        params:       Option<String>,
        /// Photon Noise ISO applied to every scene.
        #[arg(long, value_name = "ISO")]
        photon_noise: Option<u32>,
    },
    /// How many encoder processes run at once.
    Workers {
        #[arg(value_parser = value_parser!(u8).range(1..), value_name = "Workers")]
        value: u8,
    },
    /// The tool used to join the encoded scenes.
    ///
    /// Keeping the current tool keeps its settings; switching to another tool
    /// starts from that tool's defaults.
    Concatenator {
        #[arg(value_name = "Method")]
        method: ConcatenationMethod,
    },
    /// The quality metric Target Quality aims for.
    ///
    /// Switching the metric resets its target range; use *set target* to aim
    /// for a score afterwards.
    TargetMetric {
        #[arg(value_name = "Metric")]
        metric: QualityMetricBase,
    },
    /// The score Target Quality aims for.
    Target {
        #[arg(value_name = "Score")]
        value: f64,
    },
}

/// The action triple attached to `condor init`.
pub type InitAction = ConfigAction<InitKey, InitSet>;

/// Everything that can make a top-level setting unusable.
#[derive(Debug, Error)]
pub enum InitConfigError {
    #[error(
        "condor init needs INPUT and OUTPUT to create a configuration, or one of `info`, `set` or \
         `reset` to edit an existing one"
    )]
    PathsRequired,
    #[error("provide at least one of --encoder, --executable, --params or --photon-noise")]
    EncoderOptionsRequired,
    #[error("target must be a finite score")]
    TargetNotFinite,
    #[error("{0} has no default to reset to; set it instead")]
    NoDefault(&'static str),
    #[error("provide a property to reset, e.g. `condor init reset encoder`")]
    ResetNeedsProperty,
}

/// The INPUT and OUTPUT a `condor init` run needs.
///
/// They are optional at parse time because `init` also carries the `info`,
/// `set` and `reset` subcommands, so the check lives here instead of in Clap.
pub fn require_paths<'a>(
    input: Option<&'a Path>,
    output: Option<&'a Path>,
) -> Result<(&'a Path, &'a Path)> {
    let (Some(input), Some(output)) = (input, output) else {
        bail!(InitConfigError::PathsRequired);
    };
    Ok((input, output))
}

/// The top-level `condor` settings.
pub struct InitCrud;

impl InitCrud {
    /// Materializes the Target Quality section so a metric or target edit has
    /// somewhere to land.
    fn target_quality_mut(configuration: &mut Configuration) -> &mut TargetQualityConfig {
        if configuration.condor.sequence_config.target_quality.is_none() {
            configuration.condor.sequence_config.target_quality = Some(TargetQualityConfig {
                quantizer_range: TargetQuality::default_quantizer_range(
                    &configuration.condor.encoder.base(),
                ),
                ..Default::default()
            });
        }
        configuration
            .condor
            .sequence_config
            .target_quality
            .as_mut()
            .expect("target quality was just created")
    }

    /// Applies one `set input` override to the root input.
    fn apply_input(configuration: &mut Configuration, args: &InputOverride) -> Result<()> {
        ensure_override_requested(args)?;
        if let Some(path) = args.path.as_deref() {
            warn_missing_path("input path", path);
        }
        let existing = configuration.condor.input.clone();
        let mut input = configure_input(
            configuration,
            &existing,
            args.path.as_deref(),
            args.decoder.as_ref(),
            args.vs_args.as_deref(),
            None,
            None,
        )?;
        if let Some(filters) = args.filters.as_deref() {
            apply_input_filters(&mut input, filters);
        }
        apply_stream_concurrently(&mut input, args.stream_concurrently);
        configuration.condor.input = input;
        if let Some(path) = args.path.as_deref() {
            configuration.input = path.to_path_buf();
        }
        Ok(())
    }
}

impl ConfigCommand for InitCrud {
    type Key = InitKey;
    type Set = InitSet;

    fn subject() -> &'static str {
        "Top-level settings"
    }

    fn render(configuration: &Configuration, key: Option<&Self::Key>) -> Result<Value> {
        let condor = &configuration.condor;
        Ok(match key {
            None => json!({
                "input": serde_json::to_value(&condor.input)?,
                "output": serde_json::to_value(&condor.output)?,
                "encoder": serde_json::to_value(&condor.encoder)?,
                "workers": serde_json::to_value(condor.sequence_config.parallel_encoder.workers)?,
                "concatenator": serde_json::to_value(&condor.sequence_config.scene_concatenator)?,
                "target_quality": serde_json::to_value(&condor.sequence_config.target_quality)?,
            }),
            Some(InitKey::Input) => serde_json::to_value(&condor.input)?,
            Some(InitKey::Output) => serde_json::to_value(&condor.output)?,
            Some(InitKey::Encoder) => serde_json::to_value(&condor.encoder)?,
            Some(InitKey::Workers) => {
                serde_json::to_value(condor.sequence_config.parallel_encoder.workers)?
            },
            Some(InitKey::Concatenator) => {
                serde_json::to_value(&condor.sequence_config.scene_concatenator)?
            },
            Some(InitKey::TargetMetric) => serde_json::to_value(
                condor.sequence_config.target_quality.as_ref().map(|quality| &quality.metric),
            )?,
            Some(InitKey::Target) => serde_json::to_value(
                condor
                    .sequence_config
                    .target_quality
                    .as_ref()
                    .map(|quality| quality.metric.target_range()),
            )?,
        })
    }

    fn apply(configuration: &mut Configuration, set: &Self::Set) -> Result<PropertyChange> {
        match set {
            InitSet::Input {
                args,
            } => {
                Self::apply_input(configuration, args)?;
                PropertyChange::new("input", &configuration.condor.input)
            },
            InitSet::Output {
                path,
            } => {
                configuration.condor.output.path = path.clone();
                PropertyChange::new("output", &configuration.condor.output)
            },
            InitSet::Encoder {
                encoder,
                executable,
                params,
                photon_noise,
            } => {
                if encoder.is_none()
                    && executable.is_none()
                    && params.is_none()
                    && photon_noise.is_none()
                {
                    return Err(InitConfigError::EncoderOptionsRequired.into());
                }
                if let Some(executable) = executable.as_deref() {
                    warn_missing_path("encoder executable", executable);
                }
                configure_encoder(
                    &mut configuration.condor.encoder,
                    encoder.as_ref(),
                    None,
                    params.as_deref(),
                    *photon_noise,
                    None,
                    None,
                    None,
                )?;
                if let Some(executable) = executable {
                    configuration.condor.encoder.set_executable(Some(executable.clone()));
                }
                PropertyChange::new("encoder", &configuration.condor.encoder)
            },
            InitSet::Workers {
                value,
            } => {
                configuration.condor.sequence_config.parallel_encoder.workers = Some(*value);
                PropertyChange::new(
                    "workers",
                    configuration.condor.sequence_config.parallel_encoder.workers,
                )
            },
            InitSet::Concatenator {
                method,
            } => {
                configure_concatenate(configuration, Some(method))?;
                PropertyChange::new(
                    "concatenator",
                    &configuration.condor.sequence_config.scene_concatenator,
                )
            },
            InitSet::TargetMetric {
                metric,
            } => {
                Self::target_quality_mut(configuration);
                configure_target_quality(
                    configuration,
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some(metric),
                    None,
                    None,
                    None,
                    None,
                )?;
                PropertyChange::new(
                    "target-metric",
                    &Self::target_quality_mut(configuration).metric,
                )
            },
            InitSet::Target {
                value,
            } => {
                if !value.is_finite() {
                    return Err(InitConfigError::TargetNotFinite.into());
                }
                Self::target_quality_mut(configuration);
                configure_target_quality(
                    configuration,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some(*value),
                    None,
                    None,
                    None,
                )?;
                PropertyChange::new(
                    "target",
                    Self::target_quality_mut(configuration).metric.target_range(),
                )
            },
        }
    }

    fn restore(
        configuration: &mut Configuration,
        key: Option<&Self::Key>,
    ) -> Result<Option<PropertyChange>> {
        let Some(key) = key else {
            return Err(InitConfigError::ResetNeedsProperty.into());
        };
        match key {
            // The input and output paths are what `init` exists to capture;
            // there is no default worth writing over them.
            InitKey::Input => Err(InitConfigError::NoDefault("input").into()),
            InitKey::Output => Err(InitConfigError::NoDefault("output").into()),
            InitKey::Encoder => {
                configuration.condor.encoder = Encoder::default();
                Ok(Some(PropertyChange::new(
                    "encoder",
                    &configuration.condor.encoder,
                )?))
            },
            InitKey::Workers => {
                configuration.condor.sequence_config.parallel_encoder.workers = None;
                Ok(Some(PropertyChange::new(
                    "workers",
                    configuration.condor.sequence_config.parallel_encoder.workers,
                )?))
            },
            InitKey::Concatenator => {
                let defaults = SceneConcatenatorConfig::default();
                let concat = &mut configuration.condor.sequence_config.scene_concatenator;
                // `scenes_directory` is derived from --temp; keep it in place.
                concat.method = defaults.method;
                concat.output = None;
                Ok(Some(PropertyChange::new("concatenator", &*concat)?))
            },
            InitKey::TargetMetric | InitKey::Target => {
                let Some(quality) = configuration.condor.sequence_config.target_quality.as_mut()
                else {
                    warn!("Target Quality is not configured; nothing to reset.");
                    return Ok(None);
                };
                match key {
                    InitKey::TargetMetric => {
                        let defaults = TargetQualityConfig::default();
                        quality.metric = defaults.metric;
                        Ok(Some(PropertyChange::new("target-metric", &quality.metric)?))
                    },
                    _ => {
                        TargetQualityCrud::reset_target_range(&mut quality.metric);
                        Ok(Some(PropertyChange::new(
                            "target",
                            quality.metric.target_range(),
                        )?))
                    },
                }
            },
        }
    }
}

/// `condor init info|set|reset`.
pub fn config(config_path: Option<&Path>, action: &InitAction) -> Result<()> {
    edit::<InitCrud>(config_path, action)
}

#[cfg(test)]
mod tests {
    use std::assert_matches;

    use andean_condor::{
        core::sequence::target_quality::TargetQuality,
        models::{
            encoder::{
                Encoder,
                EncoderBase,
                EncoderPasses,
                cli_parameter::CLIParameter,
                photon_noise::PhotonNoise,
            },
            input::{Input, VapourSynthImportMethod},
            sequence::{
                scene_concatenator::ConcatMethod,
                target_quality::{TargetQualityConfig, types::QualityMetric},
            },
        },
    };
    use serial_test::serial;

    use super::*;
    use crate::{
        commands::{
            ConcatenationMethod,
            DecoderMethod,
            EncoderMethod,
            QualityMetric as QualityMetricBase,
            handlers::load_configuration,
        },
        test_helpers::{CwdGuard, check_basic_config, default_config, get_test_video},
        utils::hash_path::hash_path,
    };

    #[serial(needs_cwd)]
    #[test]
    fn init_creates_default_config() {
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
        let config_path = temp.path().join("condor.json");
        let config_path_abs = path_abs::PathAbs::new(&config_path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();

        // Must run inside the temp directory so the default config path resolves there
        let _cwd_guard = CwdGuard::set(temp.path());

        let expected_config = default_config(&test_video, &output, &temp_abs);

        init_handler(
            None, // defaults to ./condor.json
            None, // defaults to directory 7-character hash of input file
            &test_video.path,
            &output,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            true,
            None,
            None,
        )
        .expect("init_handler should succeed");

        assert!(config_path.exists(), "config file exists");
        let (config, found_config_path) =
            load_configuration(Some(&config_path)).expect("config loads");
        assert_eq!(
            found_config_path,
            config_path_abs,
            "config path is {}",
            config_path_abs.display()
        );
        check_basic_config(&config, &expected_config);
        assert!(config.condor.scenes.is_empty(), "scenes is empty");
    }

    #[test]
    fn init_creates_custom_config() {
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
        let config_path = temp.path().join("custom-condor.json");
        let config_path_abs = path_abs::PathAbs::new(&config_path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let custom_filters = vec![VapourSynthFilter::Crop {
            top:    Some(2),
            bottom: Some(2),
            left:   None,
            right:  None,
        }];
        let custom_vs_args = vec!["key=value".to_string()];
        let mut custom_encoder_parameters = EncoderBase::AOM.default_parameters();
        custom_encoder_parameters.insert(
            "cpu-used".to_owned(),
            CLIParameter::new_number("--", "=", 7.0),
        );

        let mut expected_config = default_config(&test_video, &output, &temp_abs);
        expected_config.condor.input = Input::VapourSynth {
            path:          input_abs,
            import_method: VapourSynthImportMethod::FFMS2 {
                index: None
            },
            cache_path:    None,
            filters:       custom_filters.clone(),
        };
        expected_config.condor.encoder = Encoder::AOM {
            executable:        None,
            pass:              EncoderPasses::All(2),
            options:           custom_encoder_parameters,
            photon_noise:      Some(PhotonNoise {
                iso:        4800,
                chroma_iso: None,
                width:      None,
                height:     None,
                c_y:        None,
                ccb:        None,
                ccr:        None,
            }),
            ffmpeg_filter:     None,
            append_color_data: true,
        };
        expected_config.condor.sequence_config.parallel_encoder.workers = Some(2);
        expected_config.condor.sequence_config.target_quality = Some(TargetQualityConfig {
            input: None,
            metric: QualityMetric::SSIMULACRA2 {
                target_range: (74.0, 76.0),
                resolution:   None,
                threads:      None,
                gpu_id:       None,
            },
            quantizer_range: TargetQuality::default_quantizer_range(&EncoderBase::AOM),
            ..Default::default()
        });
        expected_config.condor.sequence_config.scene_concatenator.method =
            ConcatMethod::FFmpeg(Default::default());
        // immutable shadow
        let expected_config = expected_config;

        init_handler(
            Some(&config_path),
            Some(&temp.path().join("custom-temp")),
            &test_video.path,
            &output,
            Some(&DecoderMethod::VSFFMS2),
            Some(&custom_filters),
            Some(&custom_vs_args),
            Some(&ConcatenationMethod::FFmpeg),
            None,
            Some(2),
            Some(&EncoderMethod::AOM),
            Some("--cpu-used=7"),
            Some(4800),
            true,
            Some(&QualityMetricBase::SSIMULACRA2),
            Some(75.0),
        )
        .expect("init_handler should succeed");

        assert!(config_path.exists(), "config file exists");
        let (config, found_config_path) =
            load_configuration(Some(&config_path)).expect("config loads");
        assert_eq!(
            found_config_path,
            config_path_abs,
            "config path is {}",
            config_path_abs.display()
        );
        check_basic_config(&config, &expected_config);
        assert!(config.condor.scenes.is_empty(), "scenes is empty");
    }

    /// `--disable-color-data` writes `append_color_data: false` into the
    /// encoder the configuration starts with.
    #[test]
    fn init_disable_color_data_turns_appending_off() {
        let test_video = get_test_video();
        let temp = tempfile::tempdir().expect("temp directory");
        let output = temp.path().join("out.mkv");
        let config_path = temp.path().join("condor.json");

        init_handler(
            Some(&config_path),
            None,
            &test_video.path,
            &output,
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
            None,
            None,
        )
        .expect("init_handler should succeed");

        let (config, _) = load_configuration(Some(&config_path)).expect("config loads");
        assert!(
            !config.condor.encoder.append_color_data(),
            "append_color_data is false"
        );
    }

    /// `condor init` with only INPUT parses, because `init` also carries the
    /// `info`/`set`/`reset` subcommands, so the missing OUTPUT must be caught
    /// before anything is created.
    #[test]
    fn init_requires_both_paths() {
        let input = Path::new("in.mkv");
        let output = Path::new("out.mkv");

        assert_matches!(
            require_paths(None, None)
                .expect_err("no paths at all")
                .downcast::<InitConfigError>(),
            Ok(InitConfigError::PathsRequired),
            "neither path is an error"
        );
        assert_matches!(
            require_paths(Some(input), None)
                .expect_err("output is missing")
                .downcast::<InitConfigError>(),
            Ok(InitConfigError::PathsRequired),
            "a missing output is an error"
        );
        assert_matches!(
            require_paths(None, Some(output))
                .expect_err("input is missing")
                .downcast::<InitConfigError>(),
            Ok(InitConfigError::PathsRequired),
            "a missing input is an error"
        );
        assert_eq!(
            require_paths(Some(input), Some(output)).expect("both paths are present"),
            (input, output),
            "both paths are returned unchanged"
        );
    }

    #[test]
    fn init_input_not_found() {
        let test_video = std::env::temp_dir().join("not_found.mkv");
        let input_abs = path_abs::PathAbs::new(&test_video)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let temp = tempfile::tempdir().expect("temp directory");
        let output = temp.path().join("out.mkv");
        let config_path = temp.path().join("condor.json");

        let result = init_handler(
            // Simulate default directory to avoid changing CWD and conflicting with other parallel
            // tests
            Some(&config_path),
            Some(&temp.path().join(hash_path(&input_abs))),
            &test_video,
            &output,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            true,
            None,
            None,
        );
        assert_matches!(result, Err(_), "init_handler should fail");
        assert!(!config_path.exists(), "config file does not exist");
    }
}
