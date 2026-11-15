use std::path::{Path, PathBuf};

use andean_condor::{
    core::sequence::target_quality::TargetQuality,
    models::{
        encoder::{Encoder, EncoderBase},
        sequence::{
            scene_concatenator::SceneConcatenatorConfig, target_quality::TargetQualityConfig,
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
        ConcatenationMethod, CondorCliError, DecoderMethod, DiscardCategory, EncoderMethod,
        QualityMetric as QualityMetricBase,
        config::{ConfigAction, ConfigCommand, InputOverride, PropertyChange},
        handlers::{
            apply_input_filters,
            concatenate::{apply_discard, configure_concatenate},
            config::{edit, ensure_override_requested},
            configure_root_input, configure_temp,
            encode::{configure_encoder, configure_parallel_encoder},
            target_quality::{
                TargetQualityCrud, configure_target_quality, require_target_quality_target,
            },
            warn_missing_executable, warn_unused_vs_args,
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
    max_tries: Option<u32>,
    overwrite: bool,
) -> Result<()> {
    init_handler_cancel(
        config_path,
        temp_path,
        input_path,
        output_path,
        decoder,
        filters,
        vs_args,
        concatenator,
        discard,
        workers,
        encoder,
        params,
        photon_noise,
        append_color_data,
        target_metric,
        target,
        max_tries,
        overwrite,
        &Default::default(),
    )
}

/// Initialization source/index opening with caller-owned cancellation.
#[allow(clippy::too_many_arguments)]
pub fn init_handler_cancel(
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
    max_tries: Option<u32>,
    overwrite: bool,
    cancellation: &andean_condor::utils::cancellation::Cancellation,
) -> Result<()> {
    cancellation.check()?;
    anyhow::ensure!(
        max_tries != Some(0),
        "max_tries must be at least 1 total attempt"
    );
    let cwd = std::env::current_dir()?;
    andean_condor::core::output::admit_output(output_path, &[input_path.into()], overwrite, false)?;
    let input = cwd.join(input_path);
    let output = cwd.join(output_path);
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

    require_target_quality_target(false, target, target_metric.is_some())?;
    // Reject encoder flags before opening/indexing the source to build a new configuration.
    let mut selected_encoder = Encoder::default_from_base(&EncoderBase::SVTAV1, false);
    configure_encoder(
        &mut selected_encoder,
        encoder,
        None,
        params,
        photon_noise,
        None,
        None,
        Some(append_color_data),
    )?;
    let mut configuration =
        Configuration::new_cancel(&input, &output, temp_path, vs_args, decoder, cancellation)?;
    configuration.condor.output.overwrite = overwrite;
    configuration.condor.encoder = selected_encoder;
    // A video input never consumes script arguments; say so instead of
    // dropping them silently.
    warn_unused_vs_args(&configuration.condor.input, vs_args);

    configure_temp(&mut configuration, temp_path)?;
    if let Some(filters) = filters {
        apply_input_filters(&mut configuration.condor.input, filters)?;
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
    configuration.validate_output(false)?;
    apply_discard(&mut configuration, discard.unwrap_or_default())?;

    configuration.validate_encoder_settings()?;
    configuration.save(&config_path)?;

    info!(
        "Initialized Condor configuration at: {}",
        config_path.display()
    );
    info!(
        "Run \"av1an\" to start, or \"av1an init info\" / \"av1an init set\" to adjust the \
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
    /// Saved consent to replace an existing distinct final output.
    Overwrite,
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

/// Settings `av1an init set` can change.
#[derive(Debug, Clone, Subcommand)]
pub enum InitSet {
    /// Save or revoke consent to replace a distinct final output (true/false).
    Overwrite {
        #[arg(action = clap::ArgAction::Set)]
        value: bool,
    },
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
        encoder: Option<EncoderMethod>,
        /// Custom encoder binary path.
        #[arg(long, value_name = "Path")]
        executable: Option<PathBuf>,
        /// Encoder parameters (`--crf 16 --tune 0`), exactly like *--params*.
        #[arg(long, value_name = "Parameters", allow_hyphen_values = true)]
        params: Option<String>,
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

/// The action triple attached to `av1an init`.
pub type InitAction = ConfigAction<InitKey, InitSet>;

/// Everything that can make a top-level setting unusable.
#[derive(Debug, Error)]
pub enum InitConfigError {
    #[error(
        "av1an init needs INPUT and OUTPUT to create a configuration, or one of `info`, `set` or \
         `reset` to edit an existing one"
    )]
    PathsRequired,
    #[error("provide at least one of --encoder, --executable, --params or --photon-noise")]
    EncoderOptionsRequired,
    #[error("target must be a finite score")]
    TargetNotFinite,
    #[error("{0} has no default to reset to; set it instead")]
    NoDefault(&'static str),
    #[error("provide a property to reset, e.g. `av1an init reset encoder`")]
    ResetNeedsProperty,
}

/// The INPUT and OUTPUT an `av1an init` run needs.
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
        configure_root_input(configuration, args)
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
            Some(InitKey::Overwrite) => serde_json::to_value(condor.output.overwrite)?,
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
            InitSet::Overwrite { value } => {
                configuration.condor.output.overwrite = *value;
                PropertyChange::new("overwrite", value)
            },
            InitSet::Input { args } => {
                Self::apply_input(configuration, args)?;
                PropertyChange::new("input", &configuration.condor.input)
            },
            InitSet::Output { path } => {
                let mut candidate = configuration.clone();
                candidate.condor.output.path = path.clone();
                candidate.validate_output(false)?;
                *configuration = candidate;
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
                    warn_missing_executable(executable);
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
            InitSet::Workers { value } => {
                configuration.condor.sequence_config.parallel_encoder.workers = Some(*value);
                PropertyChange::new(
                    "workers",
                    configuration.condor.sequence_config.parallel_encoder.workers,
                )
            },
            InitSet::Concatenator { method } => {
                configure_concatenate(configuration, Some(method))?;
                PropertyChange::new(
                    "concatenator",
                    &configuration.condor.sequence_config.scene_concatenator,
                )
            },
            InitSet::TargetMetric { metric } => {
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
            InitSet::Target { value } => {
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
            InitKey::Overwrite => {
                configuration.condor.output.overwrite = false;
                Ok(Some(PropertyChange::new("overwrite", false)?))
            },
            // The input and output paths are what `init` exists to capture;
            // there is no default worth writing over them.
            InitKey::Input => Err(InitConfigError::NoDefault("input").into()),
            InitKey::Output => Err(InitConfigError::NoDefault("output").into()),
            InitKey::Encoder => {
                let candidate = Encoder::default();
                candidate.validate_settings()?;
                configuration.condor.encoder = candidate;
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

/// `av1an init info|set|reset`.
pub fn config(config_path: Option<&Path>, action: &InitAction) -> Result<()> {
    edit::<InitCrud>(config_path, action)?;
    if matches!(
        action,
        ConfigAction::Set {
            setting: InitSet::Encoder { .. }
        } | ConfigAction::Reset {
            key: Some(InitKey::Encoder)
        }
    ) {
        super::encode::warn_existing_scene_encoders(&super::load_configuration(config_path)?.0);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::assert_matches;

    use andean_condor::models::input::Input;

    use super::*;
    use crate::{
        commands::DecoderMethod, test_helpers::default_config, utils::hash_path::hash_path,
    };

    /// `av1an init` with only INPUT parses, because `init` also carries the
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
    fn empty_script_to_video_edit_does_not_reuse_script_output_index() {
        use crate::commands::{
            config::{ConfigCommand, InputOverride},
            handlers::init::{InitCrud, InitSet},
        };
        use crate::test_helpers::TestVideo;
        use andean_condor::models::input::{ImportMethod, VapourSynthScriptSource};

        let directory = std::env::temp_dir().join("condor-empty-source-edit-model-only");
        let source = directory.join("a.vpy");
        let video = TestVideo {
            path: source.clone(),
            width: 32,
            height: 16,
            frames: 12,
            fps_rational: (24, 1),
            format: andean_condor::ffmpeg::FFPixelFormat::YUV420P,
            scenes: vec![(0, 12)],
        };
        let mut configuration = default_config(
            &video,
            &directory.join("out.mkv"),
            &directory.join(hash_path(&source)),
        );
        configuration.condor.input = Input::VapourSynthScript {
            source: VapourSynthScriptSource::Path(source),
            variables: [("keep".into(), "yes".into())].into(),
            index: 0,
            filters: vec![],
            stream_concurrently: false,
        };
        configuration.condor.scenes = video.mock_scenes(&configuration.condor.encoder);
        let data = &mut configuration.condor.scenes[0].sequence_data;
        data.parallel_encoder.started_on = Some(10);
        data.parallel_encoder.completed_on = Some(20);
        data.parallel_encoder.bytes = Some(1234);
        data.quality_check.quality.scores = vec![91.5, 92.5];
        data.target_quality.passes.push(data.quality_check.quality.clone());
        configuration.condor.scenes.clear();
        if let Input::VapourSynthScript { index, .. } = &mut configuration.condor.input {
            *index = 7;
        }
        let mut args = InputOverride {
            path: Some(directory.join("offline.mkv")),
            decoder: None,
            filters: None,
            vs_args: None,
            stream_concurrently: None,
        };
        args.decoder = Some(DecoderMethod::FFMS2);
        InitCrud::apply(&mut configuration, &InitSet::Input { args })
            .expect("empty cross-kind edit");
        assert!(
            matches!(
                configuration.condor.input,
                Input::Video {
                    import_method: ImportMethod::FFMS2 {
                        index: None,
                        cache_path: None
                    },
                    ..
                }
            ),
            "new source must not inherit output 7 or default script output 0 as a video track index"
        );
    }
}
