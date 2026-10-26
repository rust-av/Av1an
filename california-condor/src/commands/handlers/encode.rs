use std::path::{Path, PathBuf};

use andean_condor::{
    models::{
        encoder::{Encoder, EncoderBase, EncoderPasses, photon_noise::PhotonNoise},
        sequence::parallel_encoder::ParallelEncoderConfig,
    },
    vapoursynth::vapoursynth_filters::VapourSynthFilter,
};
use anyhow::{Result, bail};
use clap::{Subcommand, value_parser};
use serde_json::{Value, json};
use thiserror::Error;

use crate::{
    commands::{
        DecoderMethod,
        EncoderMethod,
        config::{ConfigAction, ConfigCommand, InputOverride, PropertyChange, Toggle},
        handlers::{
            apply_input_filters,
            apply_stream_concurrently,
            config::{edit, ensure_override_requested},
            configure_input,
            configure_temp,
            load_configuration,
            warn_missing_path,
        },
    },
    configuration::Configuration,
    utils::parameter_parser::EncoderParamsParser,
};

#[allow(clippy::too_many_arguments)]
pub fn encode_handler(
    config_path: Option<&Path>,
    temp_path: Option<&Path>,
    input_path: Option<&Path>,
    decoder: Option<&DecoderMethod>,
    filters: Option<&[VapourSynthFilter]>,
    vs_args: Option<&[String]>,
    workers: Option<u8>,
    encoder: Option<&EncoderMethod>,
    passes: Option<u8>,
    params: Option<&str>,
    photon_noise: Option<u32>,
    chroma_noise: Option<u32>,
    ffmpeg_filter: Option<&str>,
) -> Result<(Configuration, PathBuf)> {
    let (mut configuration, config_path) = load_configuration(config_path)?;

    configure_temp(&mut configuration, temp_path)?;
    configure_encoder(
        &mut configuration.condor.encoder,
        encoder,
        passes,
        params,
        photon_noise,
        chroma_noise,
        ffmpeg_filter,
        None,
    )?;
    configure_parallel_encoder(
        &mut configuration,
        input_path,
        decoder,
        filters,
        vs_args,
        workers,
    )?;

    configuration.save(&config_path)?;

    Ok((configuration, config_path))
}

#[allow(clippy::too_many_arguments)]
pub fn configure_parallel_encoder(
    configuration: &mut Configuration,
    input_path: Option<&Path>,
    decoder: Option<&DecoderMethod>,
    filters: Option<&[VapourSynthFilter]>,
    vs_args: Option<&[String]>,
    workers: Option<u8>,
) -> Result<()> {
    // `--filters` describes the input being encoded, not the parallel encoder's own
    // override, so it always applies to the main input. Applying it first lets an
    // override built below inherit the filters via `adopt_filters`.
    if let Some(filters) = filters {
        apply_input_filters(&mut configuration.condor.input, filters);
    }

    if input_path.is_some() || decoder.is_some() || vs_args.is_some() {
        let existing_input = configuration
            .condor
            .sequence_config
            .parallel_encoder
            .input
            .clone()
            .unwrap_or_else(|| configuration.condor.input.clone());
        let input = configure_input(
            configuration,
            &existing_input,
            input_path,
            decoder,
            vs_args,
            None,
            None,
        )?;
        configuration.condor.sequence_config.parallel_encoder.input = Some(input);
    }

    if let Some(workers) = workers {
        configuration.condor.sequence_config.parallel_encoder.workers = Some(workers);
    }

    Ok(())
}

/// Applies encoder settings to `encoder`: the configuration's global
/// encoder for `encode`/`init`, or one scene's own encoder for
/// [`super::scenes`].
///
/// Switching the method starts from that encoder's defaults; passes,
/// merged parameters, Photon Noise, the FFmpeg filter and colour metadata
/// appending are written after the rebuild so a switch cannot silently drop
/// them.
#[allow(clippy::too_many_arguments)]
pub fn configure_encoder(
    encoder: &mut Encoder,
    method: Option<&EncoderMethod>,
    passes: Option<u8>,
    params: Option<&str>,
    photon_noise: Option<u32>,
    chroma_noise: Option<u32>,
    ffmpeg_filter: Option<&str>,
    append_color_data: Option<bool>,
) -> Result<()> {
    if let Some(method) = method {
        *encoder = Encoder::default_from_base(&method.as_encoder_base(), false);
    }
    if let Some(passes) = passes
        && let Some(encoder_passes) = encoder.passes_mut()
    {
        *encoder_passes = EncoderPasses::All(passes);
    }
    if let Some(params) = params {
        let parameters = EncoderParamsParser::parse_string(params)?;
        encoder.parameters_mut().extend(parameters);
    }
    if let Some(iso) = photon_noise {
        // TODO: Support chroma noise only
        encoder.set_photon_noise(Some(PhotonNoise {
            iso,
            chroma_iso: chroma_noise,
            width: None,
            height: None,
            c_y: None,
            ccb: None,
            ccr: None,
        }));
    }
    if let Some(ffmpeg_filter) = ffmpeg_filter {
        encoder.set_ffmpeg_filter(Some(ffmpeg_filter.to_owned()));
    }
    if let Some(append_color_data) = append_color_data {
        encoder.set_append_color_data(append_color_data);
    }

    Ok(())
}

/// `info` and `reset` keys of `sequence_config.parallel_encoder` and `encoder`.
#[derive(Debug, Clone, Subcommand)]
pub enum EncodeKey {
    /// How many encoder processes run at once.
    Workers,
    /// The input Parallel Encoder reads instead of the main input.
    Input,
    /// The directory the encoded scenes are written to.
    ScenesDirectory,
    /// The encoder and its parameters.
    Encoder,
    /// Custom encoder binary path.
    Executable,
    /// Number of encoder passes.
    Passes,
    /// Encoder parameters (`--crf 16 --tune 0`).
    Params,
    /// Photon Noise ISO table settings.
    PhotonNoise,
    /// FFmpeg filtergraph applied to the frames before they reach the encoder.
    FfmpegFilter,
    /// Whether the source's colour and HDR metadata is appended to the encoder
    /// parameters.
    AppendColorData,
}

/// Settings `condor encode set` can change.
#[derive(Debug, Clone, Subcommand)]
pub enum EncodeSet {
    /// How many encoder processes run at once.
    Workers {
        #[arg(value_parser = value_parser!(u8).range(1..), value_name = "Workers")]
        value: u8,
    },
    /// Read from this input instead of the main input.
    ///
    /// Builds the same input the *--input*, *--decoder*, *--filters* and
    /// *--vs-args* flags of `encode` would build.
    Input {
        #[command(flatten)]
        args: InputOverride,
    },
    /// The directory the encoded scenes are written to.
    ScenesDirectory {
        #[arg(value_name = "Path")]
        path: PathBuf,
    },
    /// The encoder to use; switching encoders starts from that encoder's
    /// defaults.
    Encoder {
        #[arg(value_name = "Encoder")]
        encoder: EncoderMethod,
    },
    /// Custom encoder binary path (`reset executable` restores PATH lookup).
    Executable {
        #[arg(value_name = "Path")]
        path: PathBuf,
    },
    /// Number of encoder passes.
    Passes {
        #[arg(value_parser = value_parser!(u8).range(1..), value_name = "Passes")]
        value: u8,
    },
    /// Encoder parameters (`--crf 16 --tune 0`), merged into the current
    /// encoder's options.
    Params {
        #[arg(value_name = "Parameters", allow_hyphen_values = true)]
        params: String,
    },
    /// Photon Noise ISO applied to every scene (`--chroma-noise` sets the
    /// chroma ISO).
    PhotonNoise {
        /// Luma ISO strength.
        #[arg(value_name = "ISO")]
        iso:          u32,
        /// Optional chroma ISO strength.
        #[arg(long, value_name = "ISO")]
        chroma_noise: Option<u32>,
    },
    /// FFmpeg filtergraph applied to the frames before they reach the encoder.
    FfmpegFilter {
        #[arg(value_name = "Filter", allow_hyphen_values = true)]
        filter: String,
    },
    /// Whether the source's colour and HDR metadata is appended to the encoder
    /// parameters.
    ///
    /// Bare `on` is the default when the value is omitted.
    AppendColorData {
        #[arg(value_name = "on|off")]
        value: Option<Toggle>,
    },
}

/// The action triple attached to `condor encode`.
pub type EncodeAction = ConfigAction<EncodeKey, EncodeSet>;

/// Everything that can make a Parallel Encoder or Encoder setting unusable.
#[derive(Debug, Error)]
pub enum EncodeConfigError {
    #[error("the {0} encoder is single-pass only and does not take a pass count")]
    PassesNotSupported(EncoderBase),
    #[error("the {0} encoder does not support Photon Noise")]
    PhotonNoiseNotSupported(EncoderBase),
    #[error(
        "the {0} encoder does not take --ffmpeg-filter; pass `-vf` through its parameters instead"
    )]
    FfmpegFilterNotSupported(EncoderBase),
}

/// The `condor encode` configuration section.
pub struct EncodeCrud;

impl EncodeCrud {
    /// Applies one `set input` override to the parallel encoder.
    fn apply_input(configuration: &mut Configuration, args: &InputOverride) -> Result<()> {
        ensure_override_requested(args)?;
        if let Some(path) = args.path.as_deref() {
            warn_missing_path("input path", path);
        }
        configure_parallel_encoder(
            configuration,
            args.path.as_deref(),
            args.decoder.as_ref(),
            args.filters.as_deref(),
            args.vs_args.as_deref(),
            None,
        )?;
        if let Some(toggle) = args.stream_concurrently {
            if configuration.condor.sequence_config.parallel_encoder.input.is_none() {
                let existing = configuration.condor.input.clone();
                let input =
                    configure_input(configuration, &existing, None, None, None, None, None)?;
                configuration.condor.sequence_config.parallel_encoder.input = Some(input);
            }
            if let Some(input) =
                configuration.condor.sequence_config.parallel_encoder.input.as_mut()
            {
                apply_stream_concurrently(input, Some(toggle));
            }
        }
        Ok(())
    }
}

impl ConfigCommand for EncodeCrud {
    type Key = EncodeKey;
    type Set = EncodeSet;

    fn subject() -> &'static str {
        "Encode settings"
    }

    fn render(configuration: &Configuration, key: Option<&Self::Key>) -> Result<Value> {
        let parallel = &configuration.condor.sequence_config.parallel_encoder;
        let encoder = &configuration.condor.encoder;
        Ok(match key {
            None => json!({
                "parallel_encoder": serde_json::to_value(parallel)?,
                "encoder": serde_json::to_value(encoder)?,
            }),
            Some(EncodeKey::Workers) => serde_json::to_value(parallel.workers)?,
            Some(EncodeKey::Input) => serde_json::to_value(&parallel.input)?,
            Some(EncodeKey::ScenesDirectory) => serde_json::to_value(&parallel.scenes_directory)?,
            Some(EncodeKey::Encoder) => serde_json::to_value(encoder)?,
            Some(EncodeKey::Executable) => serde_json::to_value(encoder.custom_executable())?,
            Some(EncodeKey::Passes) => serde_json::to_value(encoder.passes())?,
            Some(EncodeKey::Params) => serde_json::to_value(encoder.parameters())?,
            Some(EncodeKey::PhotonNoise) => serde_json::to_value(encoder.photon_noise())?,
            Some(EncodeKey::FfmpegFilter) => serde_json::to_value(encoder.ffmpeg_filter())?,
            Some(EncodeKey::AppendColorData) => serde_json::to_value(encoder.append_color_data())?,
        })
    }

    fn apply(configuration: &mut Configuration, set: &Self::Set) -> Result<PropertyChange> {
        match set {
            EncodeSet::Workers {
                value,
            } => {
                configuration.condor.sequence_config.parallel_encoder.workers = Some(*value);
                PropertyChange::new(
                    "workers",
                    configuration.condor.sequence_config.parallel_encoder.workers,
                )
            },
            EncodeSet::Input {
                args,
            } => {
                Self::apply_input(configuration, args)?;
                PropertyChange::new(
                    "input",
                    &configuration.condor.sequence_config.parallel_encoder.input,
                )
            },
            EncodeSet::ScenesDirectory {
                path,
            } => {
                configuration.condor.sequence_config.parallel_encoder.scenes_directory =
                    path.clone();
                PropertyChange::new(
                    "scenes-directory",
                    &configuration.condor.sequence_config.parallel_encoder.scenes_directory,
                )
            },
            EncodeSet::Encoder {
                encoder,
            } => {
                configure_encoder(
                    &mut configuration.condor.encoder,
                    Some(encoder),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                )?;
                PropertyChange::new("encoder", &configuration.condor.encoder)
            },
            EncodeSet::Executable {
                path,
            } => {
                warn_missing_path("encoder executable", path);
                configuration.condor.encoder.set_executable(Some(path.clone()));
                PropertyChange::new(
                    "executable",
                    configuration.condor.encoder.custom_executable(),
                )
            },
            EncodeSet::Passes {
                value,
            } => {
                let base = configuration.condor.encoder.base();
                let Some(passes) = configuration.condor.encoder.passes_mut() else {
                    bail!(EncodeConfigError::PassesNotSupported(base));
                };
                *passes = EncoderPasses::All(*value);
                PropertyChange::new("passes", configuration.condor.encoder.passes())
            },
            EncodeSet::Params {
                params,
            } => {
                configure_encoder(
                    &mut configuration.condor.encoder,
                    None,
                    None,
                    Some(params),
                    None,
                    None,
                    None,
                    None,
                )?;
                PropertyChange::new("params", configuration.condor.encoder.parameters())
            },
            EncodeSet::PhotonNoise {
                iso,
                chroma_noise,
            } => {
                let base = configuration.condor.encoder.base();
                if !base.supports_photon_noise() {
                    bail!(EncodeConfigError::PhotonNoiseNotSupported(base));
                }
                configure_encoder(
                    &mut configuration.condor.encoder,
                    None,
                    None,
                    None,
                    Some(*iso),
                    *chroma_noise,
                    None,
                    None,
                )?;
                PropertyChange::new("photon-noise", configuration.condor.encoder.photon_noise())
            },
            EncodeSet::FfmpegFilter {
                filter,
            } => {
                let base = configuration.condor.encoder.base();
                if base == EncoderBase::FFmpeg {
                    bail!(EncodeConfigError::FfmpegFilterNotSupported(base));
                }
                configure_encoder(
                    &mut configuration.condor.encoder,
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some(filter),
                    None,
                )?;
                PropertyChange::new(
                    "ffmpeg-filter",
                    configuration.condor.encoder.ffmpeg_filter(),
                )
            },
            EncodeSet::AppendColorData {
                value,
            } => {
                let enabled = value.unwrap_or(Toggle::On).enabled();
                configuration.condor.encoder.set_append_color_data(enabled);
                PropertyChange::new(
                    "append-color-data",
                    configuration.condor.encoder.append_color_data(),
                )
            },
        }
    }

    fn restore(
        configuration: &mut Configuration,
        key: Option<&Self::Key>,
    ) -> Result<Option<PropertyChange>> {
        let (property, value) = match key {
            None => {
                // `scenes_directory` is derived from --temp; keep it in place.
                let parallel = &mut configuration.condor.sequence_config.parallel_encoder;
                parallel.workers = None;
                parallel.input = None;
                configuration.condor.encoder = Encoder::default();
                (
                    "section",
                    json!({
                        "parallel_encoder": serde_json::to_value(&configuration.condor.sequence_config.parallel_encoder)?,
                        "encoder": serde_json::to_value(&configuration.condor.encoder)?,
                    }),
                )
            },
            Some(EncodeKey::Workers) => {
                configuration.condor.sequence_config.parallel_encoder.workers = None;
                (
                    "workers",
                    serde_json::to_value(
                        configuration.condor.sequence_config.parallel_encoder.workers,
                    )?,
                )
            },
            Some(EncodeKey::Input) => {
                configuration.condor.sequence_config.parallel_encoder.input = None;
                (
                    "input",
                    serde_json::to_value(
                        &configuration.condor.sequence_config.parallel_encoder.input,
                    )?,
                )
            },
            Some(EncodeKey::ScenesDirectory) => {
                let defaults = ParallelEncoderConfig::default();
                configuration.condor.sequence_config.parallel_encoder.scenes_directory =
                    defaults.scenes_directory;
                (
                    "scenes-directory",
                    serde_json::to_value(
                        &configuration.condor.sequence_config.parallel_encoder.scenes_directory,
                    )?,
                )
            },
            Some(EncodeKey::Encoder) => {
                configuration.condor.encoder = Encoder::default();
                (
                    "encoder",
                    serde_json::to_value(&configuration.condor.encoder)?,
                )
            },
            Some(EncodeKey::Executable) => {
                configuration.condor.encoder.set_executable(None);
                (
                    "executable",
                    serde_json::to_value(configuration.condor.encoder.custom_executable())?,
                )
            },
            Some(EncodeKey::Passes) => {
                let base = configuration.condor.encoder.base();
                if let Some(passes) = configuration.condor.encoder.passes_mut() {
                    *passes = base.default_passes();
                }
                (
                    "passes",
                    serde_json::to_value(configuration.condor.encoder.passes())?,
                )
            },
            Some(EncodeKey::Params) => {
                let base = configuration.condor.encoder.base();
                *configuration.condor.encoder.parameters_mut() = base.default_parameters();
                (
                    "params",
                    serde_json::to_value(configuration.condor.encoder.parameters())?,
                )
            },
            Some(EncodeKey::PhotonNoise) => {
                configuration.condor.encoder.set_photon_noise(None);
                (
                    "photon-noise",
                    serde_json::to_value(configuration.condor.encoder.photon_noise())?,
                )
            },
            Some(EncodeKey::FfmpegFilter) => {
                configuration.condor.encoder.set_ffmpeg_filter(None);
                (
                    "ffmpeg-filter",
                    serde_json::to_value(configuration.condor.encoder.ffmpeg_filter())?,
                )
            },
            Some(EncodeKey::AppendColorData) => {
                configuration.condor.encoder.set_append_color_data(true);
                (
                    "append-color-data",
                    serde_json::to_value(configuration.condor.encoder.append_color_data())?,
                )
            },
        };
        Ok(Some(PropertyChange {
            property: property.to_owned(),
            value,
        }))
    }
}

/// `condor encode info|set|reset`.
pub fn config(config_path: Option<&Path>, action: &EncodeAction) -> Result<()> {
    edit::<EncodeCrud>(config_path, action)
}

#[cfg(test)]
mod tests {
    use andean_condor::{
        models::{
            encoder::cli_parameter::CLIParameter,
            input::{Input, VapourSynthImportMethod},
            sequence::parallel_encoder::ParallelEncoderConfig,
        },
        vapoursynth::plugins::resize::Scaler,
    };

    use super::*;
    use crate::{
        commands::handlers::init::init_handler,
        test_helpers::{check_basic_config, check_input, default_config, get_test_video},
        utils::hash_path::hash_path,
    };

    #[test]
    fn encode_default_config() {
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

        let expected_config = default_config(&test_video, &output, &temp_abs);

        init_handler(
            // Simulate default directory to avoid changing CWD
            Some(&config_path),
            Some(&temp.path().join(hash_path(&input_abs))),
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
        let (config, found_config_path) = encode_handler(
            // Simulate default directory to avoid changing CWD
            Some(&config_path),
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
        .expect("encode_handler should succeed");

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
    fn encode_custom_config() {
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
        let custom_filters = vec![VapourSynthFilter::Crop {
            top:    Some(140),
            bottom: Some(140),
            left:   None,
            right:  None,
        }];
        let custom_vs_args = vec!["method=target quality".to_string()];

        let mut expected_config = default_config(&test_video, &output, &temp_abs);
        expected_config.condor.encoder = Encoder::default_from_base(&EncoderBase::RAV1E, false);
        expected_config.condor.encoder.parameters_mut().insert(
            "speed".to_owned(),
            CLIParameter::Number {
                prefix:    "--".to_owned(),
                delimiter: " ".to_owned(),
                value:     10.0,
            },
        );
        if let Some(encoder_passes) = expected_config.condor.encoder.passes_mut() {
            *encoder_passes = EncoderPasses::All(2);
        }
        expected_config.condor.encoder.set_photon_noise(Some(PhotonNoise {
            iso:        1600,
            chroma_iso: Some(400),
            width:      None,
            height:     None,
            c_y:        None,
            ccb:        None,
            ccr:        None,
        }));
        expected_config.condor.sequence_config.parallel_encoder = ParallelEncoderConfig {
            // `--filters` describes the input being encoded, so it applies to
            // `condor.input` and the override inherits it.
            input:            Some(Input::VapourSynth {
                path:          input_abs.clone(),
                import_method: VapourSynthImportMethod::DGDecNV {
                    dgindexnv_executable: None,
                },
                cache_path:    None,
                filters:       custom_filters.clone(),
            }),
            workers:          Some(2),
            scenes_directory: temp_abs.join("scenes"),
        };
        if let Input::VapourSynth {
            filters: main_filters,
            ..
        } = &mut expected_config.condor.input
        {
            *main_filters = custom_filters.clone();
        }
        // immutable shadow
        let expected_config = expected_config;

        init_handler(
            // Simulate default directory to avoid changing CWD
            Some(&config_path),
            Some(&temp.path().join(hash_path(&input_abs))),
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
        let (config, found_config_path) = encode_handler(
            // Simulate default directory to avoid changing CWD
            Some(&config_path),
            None,
            Some(&test_video.path),
            Some(&DecoderMethod::DGDecodeNV),
            Some(&custom_filters),
            Some(&custom_vs_args),
            Some(2),
            Some(&EncoderMethod::RAV1E),
            Some(2),
            Some("--speed 10"),
            Some(1600),
            Some(400),
            None,
        )
        .expect("encode_handler should succeed");

        assert_eq!(
            found_config_path,
            config_path_abs,
            "config path is {}",
            config_path_abs.display()
        );
        check_basic_config(&config, &expected_config);
        assert!(config.condor.scenes.is_empty(), "scenes is empty");
    }

    /// `--filters` describes the input being encoded, so it must reach
    /// `condor.input` even when the config already carries a
    /// `parallel_encoder.input` override.
    #[test]
    fn encode_filters_target_main_input_with_override_present() {
        let test_video = get_test_video();
        let input_abs = path_abs::PathAbs::new(&test_video.path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let temp = tempfile::tempdir().expect("temp directory");
        let output = temp.path().join("out.mkv");
        let config_path = temp.path().join("condor.json");
        let custom_filters = vec![VapourSynthFilter::Resize {
            scaler: Some(Scaler::Point),
            width:  Some(960),
            height: Some(540),
            format: None,
        }];

        init_handler(
            Some(&config_path),
            Some(&temp.path().join(hash_path(&input_abs))),
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

        // The filters belong to the main input; an existing override keeps
        // its own.
        let (mut config, _) =
            load_configuration(Some(&config_path)).expect("load_configuration should succeed");
        config.condor.sequence_config.parallel_encoder.input = Some(Input::VapourSynth {
            path:          input_abs,
            import_method: VapourSynthImportMethod::DGDecNV {
                dgindexnv_executable: None,
            },
            cache_path:    None,
            filters:       vec![],
        });
        let override_before = config.condor.sequence_config.parallel_encoder.input.clone();
        config.save(&config_path).expect("save should succeed");

        let (config, _) = encode_handler(
            Some(&config_path),
            None,
            None,
            None,
            Some(&custom_filters),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("encode_handler should succeed");

        assert_eq!(
            config.condor.input.vapoursynth_filters(),
            custom_filters,
            "main input receives the filters"
        );
        check_input(
            config.condor.sequence_config.parallel_encoder.input.as_ref(),
            override_before.as_ref(),
            "parallel encoder input",
        );
    }
}
