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

use crate::{
    commands::{
        DecoderMethod, EncoderMethod,
        config::{ConfigAction, ConfigCommand, InputOverride, PropertyChange, Toggle},
        handlers::{
            apply_input_filters, apply_stream_concurrently,
            config::{edit, ensure_override_requested},
            configure_input, configure_temp, load_configuration, warn_missing_executable,
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
    max_tries: Option<u32>,
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
        max_tries,
    )?;

    configuration.validate_encoder_settings()?;
    configuration.admit_results()?;
    configuration.save(&config_path)?;
    if encoder.is_some()
        || passes.is_some()
        || params.is_some()
        || photon_noise.is_some()
        || chroma_noise.is_some()
        || ffmpeg_filter.is_some()
    {
        warn_existing_scene_encoders(&configuration);
    }

    Ok((configuration, config_path))
}

/// Global edits are defaults for future scenes, never implicit scene resets.
pub(super) fn warn_existing_scene_encoders(configuration: &Configuration) {
    if !configuration.condor.scenes.is_empty() {
        tracing::warn!(
            "Global encoder defaults updated for future scenes. Existing scenes unchanged; to copy the global encoder into them explicitly, run `av1an scenes all reset encoder`. Saved results/files are not cleaned or recomputed."
        );
    }
}

#[allow(clippy::too_many_arguments)]
pub fn configure_parallel_encoder(
    configuration: &mut Configuration,
    input_path: Option<&Path>,
    decoder: Option<&DecoderMethod>,
    filters: Option<&[VapourSynthFilter]>,
    vs_args: Option<&[String]>,
    workers: Option<u8>,
    max_tries: Option<u32>,
) -> Result<()> {
    let mut candidate = configuration.condor.sequence_config.parallel_encoder.clone();
    if let Some(value) = max_tries {
        candidate.max_tries = value;
    }
    if let Some(workers) = workers {
        candidate.workers = Some(workers);
    }
    candidate.validate()?;
    let mut main_input = configuration.condor.input.clone();

    if input_path.is_some() || decoder.is_some() || vs_args.is_some() {
        let mut source = candidate.input.as_ref().unwrap_or(&main_input).clone();
        if filters.is_some() {
            // Replaced filters must not fail conversion to the requested decoder.
            source.set_filters(Vec::new())?;
        }
        candidate.input = Some(configure_input(
            configuration,
            &source,
            input_path,
            decoder,
            vs_args,
            None,
            None,
        )?);
    }
    // Filter the effective encoder input; an override must not alter the root reference.
    if let Some(filters) = filters {
        apply_input_filters(candidate.input.as_mut().unwrap_or(&mut main_input), filters)?;
    }

    // Publish inputs and execution settings only after every fallible conversion succeeds.
    configuration.condor.input = main_input;
    configuration.condor.sequence_config.parallel_encoder = candidate;

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
    let mut candidate = method.map_or_else(
        || encoder.clone(),
        |method| Encoder::default_from_base(&method.as_encoder_base(), false),
    );
    let base = candidate.base();
    if let Some(passes) = passes {
        let Some(encoder_passes) = candidate.passes_mut() else {
            bail!(EncodeConfigError::PassesNotSupported(base));
        };
        *encoder_passes = EncoderPasses::All(passes);
    }
    if let Some(params) = params {
        let parameters = EncoderParamsParser::parse_string(params)?;
        if base == EncoderBase::SVTAV1
            && !parameters.contains_key("crf")
            && (parameters.contains_key("cqp")
                || parameters.contains_key("qp")
                || parameters.contains_key("q")
                || parameters
                    .get("aq-mode")
                    .is_some_and(|p| matches!(p.to_string_value().as_str(), "0" | "1"))
                || parameters.get("rc").is_some_and(|p| {
                    matches!(
                        p.to_string_value().as_str(),
                        "1" | "2" | "vbr" | "cbr" | "cqp"
                    )
                }))
            && candidate.parameters().get("crf") == base.default_parameters().get("crf")
        {
            // An explicit mode change replaces only the stock CRF, not custom quality settings.
            candidate.parameters_mut().remove("crf");
        }
        candidate.parameters_mut().extend(parameters);
    }
    if photon_noise.is_some() || chroma_noise.is_some() {
        if !base.supports_photon_noise() {
            bail!(EncodeConfigError::PhotonNoiseNotSupported(base));
        }
        let mut noise = if let Some(iso) = photon_noise {
            PhotonNoise {
                iso,
                chroma_iso: chroma_noise,
                width: None,
                height: None,
                c_y: None,
                ccb: None,
                ccr: None,
            }
        } else {
            candidate.photon_noise().cloned().ok_or(EncodeConfigError::ChromaWithoutLuma)?
        };
        if let Some(chroma) = chroma_noise {
            noise.chroma_iso = Some(chroma);
        }
        candidate.set_photon_noise(Some(noise));
    }
    if let Some(ffmpeg_filter) = ffmpeg_filter {
        if base == EncoderBase::FFmpeg {
            bail!(EncodeConfigError::FfmpegFilterNotSupported(base));
        }
        candidate.set_ffmpeg_filter(Some(ffmpeg_filter.to_owned()));
    }
    if let Some(append_color_data) = append_color_data {
        candidate.set_append_color_data(append_color_data);
    }
    candidate.validate_settings()?;
    *encoder = candidate;
    Ok(())
}

/// `info` and `reset` keys of `sequence_config.parallel_encoder` and `encoder`.
#[derive(Debug, Clone, Subcommand)]
pub enum EncodeKey {
    /// Total attempts per scene including the initial attempt.
    MaxTries,
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

/// Settings `av1an encode set` can change.
#[derive(Debug, Clone, Subcommand)]
pub enum EncodeSet {
    /// Total attempts per scene including the initial attempt.
    MaxTries {
        #[arg(value_parser = value_parser!(u32).range(1..))]
        value: u32,
    },
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
    /// Custom binary path or PATH name (`reset executable` restores the stock name).
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
        iso: u32,
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

/// The action triple attached to `av1an encode`.
pub type EncodeAction = ConfigAction<EncodeKey, EncodeSet>;

/// Everything that can make a Parallel Encoder or Encoder setting unusable.
pub use andean_condor::models::encoder::EncoderSettingsError as EncodeConfigError;

/// The `av1an encode` configuration section.
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
            Some(EncodeKey::MaxTries) => serde_json::to_value(parallel.max_tries)?,
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
            EncodeSet::MaxTries { value } => {
                let mut candidate = configuration.condor.sequence_config.parallel_encoder.clone();
                candidate.max_tries = *value;
                candidate.validate()?;
                configuration.condor.sequence_config.parallel_encoder = candidate;
                PropertyChange::new("max-tries", value)
            },
            EncodeSet::Workers { value } => {
                configuration.condor.sequence_config.parallel_encoder.workers = Some(*value);
                PropertyChange::new(
                    "workers",
                    configuration.condor.sequence_config.parallel_encoder.workers,
                )
            },
            EncodeSet::Input { args } => {
                Self::apply_input(configuration, args)?;
                PropertyChange::new(
                    "input",
                    &configuration.condor.sequence_config.parallel_encoder.input,
                )
            },
            EncodeSet::ScenesDirectory { path } => {
                configuration.condor.sequence_config.parallel_encoder.scenes_directory =
                    path.clone();
                PropertyChange::new(
                    "scenes-directory",
                    &configuration.condor.sequence_config.parallel_encoder.scenes_directory,
                )
            },
            EncodeSet::Encoder { encoder } => {
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
            EncodeSet::Executable { path } => {
                warn_missing_executable(path);
                let mut candidate = configuration.condor.encoder.clone();
                candidate.set_executable(Some(path.clone()));
                candidate.validate_settings()?;
                configuration.condor.encoder = candidate;
                PropertyChange::new(
                    "executable",
                    configuration.condor.encoder.custom_executable(),
                )
            },
            EncodeSet::Passes { value } => {
                configure_encoder(
                    &mut configuration.condor.encoder,
                    None,
                    Some(*value),
                    None,
                    None,
                    None,
                    None,
                    None,
                )?;
                PropertyChange::new("passes", configuration.condor.encoder.passes())
            },
            EncodeSet::Params { params } => {
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
            EncodeSet::PhotonNoise { iso, chroma_noise } => {
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
            EncodeSet::FfmpegFilter { filter } => {
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
            EncodeSet::AppendColorData { value } => {
                let enabled = value.unwrap_or(Toggle::On).enabled();
                configure_encoder(
                    &mut configuration.condor.encoder,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some(enabled),
                )?;
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
        let mut candidate = configuration.condor.encoder.clone();
        let (property, value) = match key {
            None => {
                // `scenes_directory` is derived from --temp; keep it in place.
                let parallel = &mut configuration.condor.sequence_config.parallel_encoder;
                parallel.workers = None;
                parallel.max_tries = ParallelEncoderConfig::default().max_tries;
                parallel.input = None;
                candidate = Encoder::default();
                (
                    "section",
                    json!({
                        "parallel_encoder": serde_json::to_value(&configuration.condor.sequence_config.parallel_encoder)?,
                        "encoder": serde_json::to_value(&candidate)?,
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
            Some(EncodeKey::MaxTries) => {
                let value = ParallelEncoderConfig::default().max_tries;
                configuration.condor.sequence_config.parallel_encoder.max_tries = value;
                ("max-tries", serde_json::to_value(value)?)
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
                candidate = Encoder::default();
                ("encoder", serde_json::to_value(&candidate)?)
            },
            Some(EncodeKey::Executable) => {
                candidate.set_executable(None);
                (
                    "executable",
                    serde_json::to_value(candidate.custom_executable())?,
                )
            },
            Some(EncodeKey::Passes) => {
                let base = candidate.base();
                if let Some(passes) = candidate.passes_mut() {
                    *passes = base.default_passes();
                }
                ("passes", serde_json::to_value(candidate.passes())?)
            },
            Some(EncodeKey::Params) => {
                let base = candidate.base();
                *candidate.parameters_mut() = base.default_parameters();
                ("params", serde_json::to_value(candidate.parameters())?)
            },
            Some(EncodeKey::PhotonNoise) => {
                candidate.set_photon_noise(None);
                (
                    "photon-noise",
                    serde_json::to_value(candidate.photon_noise())?,
                )
            },
            Some(EncodeKey::FfmpegFilter) => {
                candidate.set_ffmpeg_filter(None);
                (
                    "ffmpeg-filter",
                    serde_json::to_value(candidate.ffmpeg_filter())?,
                )
            },
            Some(EncodeKey::AppendColorData) => {
                candidate.set_append_color_data(true);
                (
                    "append-color-data",
                    serde_json::to_value(candidate.append_color_data())?,
                )
            },
        };
        if !matches!(
            key,
            Some(
                EncodeKey::Workers
                    | EncodeKey::MaxTries
                    | EncodeKey::Input
                    | EncodeKey::ScenesDirectory
            )
        ) {
            candidate.validate_settings()?;
            configuration.condor.encoder = candidate;
        }
        Ok(Some(PropertyChange {
            property: property.to_owned(),
            value,
        }))
    }
}

/// `av1an encode info|set|reset`.
pub fn config(config_path: Option<&Path>, action: &EncodeAction) -> Result<()> {
    edit::<EncodeCrud>(config_path, action)?;
    let encoder_edit = match action {
        ConfigAction::Info { .. } => false,
        ConfigAction::Set { setting } => !matches!(
            setting,
            EncodeSet::MaxTries { .. }
                | EncodeSet::Workers { .. }
                | EncodeSet::Input { .. }
                | EncodeSet::ScenesDirectory { .. }
        ),
        ConfigAction::Reset { key } => !matches!(
            key,
            Some(
                EncodeKey::MaxTries
                    | EncodeKey::Workers
                    | EncodeKey::Input
                    | EncodeKey::ScenesDirectory
            )
        ),
    };
    if encoder_edit {
        warn_existing_scene_encoders(&load_configuration(config_path)?.0);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

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
        test_helpers::{check_basic_config, default_config, get_test_video},
        utils::hash_path::hash_path,
    };

    #[test]
    fn e19_avm_explicit_noise_edits_fail_atomically_offline() {
        for method in [None, Some(EncoderMethod::AVM)] {
            for (noise, chroma) in [(Some(800), None), (None, Some(400)), (Some(800), Some(400))] {
                let mut encoder = Encoder::default_from_base(&EncoderBase::AVM, false);
                encoder.set_executable(Some("/missing/e19-avmenc".into()));
                let before = encoder.clone();
                let error = configure_encoder(
                    &mut encoder,
                    method.as_ref(),
                    Some(1),
                    Some("--qp=40"),
                    noise,
                    chroma,
                    None,
                    Some(false),
                )
                .expect_err("unsupported AVM Photon Noise");
                assert_eq!(
                    error.downcast_ref::<EncodeConfigError>(),
                    Some(&EncodeConfigError::PhotonNoiseNotSupported(
                        EncoderBase::AVM
                    ))
                );
                assert_eq!(encoder, before, "failed candidate must not be committed");
            }
        }
    }

    #[test]
    fn e19_avm_crud_preserves_saved_config_and_repairs_legacy_noise_offline() {
        use crate::test_helpers::TestVideo;
        let temp = tempfile::tempdir().expect("configuration fixture");
        let video = TestVideo {
            path: temp.path().join("missing-source.mkv"),
            width: 64,
            height: 64,
            frames: 1,
            fps_rational: (24, 1),
            format: andean_condor::ffmpeg::FFPixelFormat::YUV420P,
            scenes: vec![],
        };
        let mut configuration = default_config(&video, &temp.path().join("out.mkv"), temp.path());
        configuration.condor.encoder = Encoder::default_from_base(&EncoderBase::AVM, false);
        configuration
            .condor
            .encoder
            .set_executable(Some(temp.path().join("missing-avmenc")));
        let path = temp.path().join("condor.json");
        configuration.save(&path).expect("saved config");
        let before = fs::read(&path).expect("original bytes");
        let original = serde_json::to_value(&configuration).expect("original config");
        let set = EncodeSet::PhotonNoise {
            iso: 800,
            chroma_noise: Some(400),
        };
        let error = EncodeCrud::apply(&mut configuration, &set).expect_err("atomic in-memory edit");
        assert_eq!(
            error.downcast_ref::<EncodeConfigError>(),
            Some(&EncodeConfigError::PhotonNoiseNotSupported(
                EncoderBase::AVM
            ))
        );
        assert_eq!(
            serde_json::to_value(&configuration).expect("unchanged config"),
            original
        );
        let error = config(Some(&path), &ConfigAction::Set { setting: set })
            .expect_err("atomic saved edit");
        assert_eq!(
            error.downcast_ref::<EncodeConfigError>(),
            Some(&EncodeConfigError::PhotonNoiseNotSupported(
                EncoderBase::AVM
            ))
        );
        assert_eq!(fs::read(&path).expect("saved bytes"), before);

        configuration.condor.encoder.set_photon_noise(Some(PhotonNoise {
            iso: 800,
            chroma_iso: Some(400),
            width: None,
            height: None,
            c_y: None,
            ccb: None,
            ccr: None,
        }));
        configuration.save(&path).expect("legacy saved request");
        let legacy = fs::read(&path).expect("legacy bytes");
        let error = encode_handler(
            Some(&path),
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
        .expect_err("loaded noise must not silently run");
        assert_eq!(
            error.downcast_ref::<EncodeConfigError>(),
            Some(&EncodeConfigError::PhotonNoiseNotSupported(
                EncoderBase::AVM
            ))
        );
        assert_eq!(fs::read(&path).expect("unchanged legacy bytes"), legacy);
        config(
            Some(&path),
            &ConfigAction::Reset {
                key: Some(EncodeKey::PhotonNoise),
            },
        )
        .expect("clear legacy noise");
        let repaired = Configuration::load(&path).expect("load").expect("saved repair");
        assert!(repaired.condor.encoder.photon_noise().is_none());
        assert_eq!(repaired.condor.encoder.validate_settings(), Ok(()));
        config(
            Some(&path),
            &ConfigAction::Set {
                setting: EncodeSet::Params {
                    params: "--film-grain-table=caller-filmgrn2.tbl".into(),
                },
            },
        )
        .expect("raw native option stays available");
        let raw = Configuration::load(&path).expect("load").expect("saved raw option");
        assert_eq!(
            raw.condor.encoder.parameters()["film-grain-table"].to_string_value(),
            "caller-filmgrn2.tbl"
        );
        configuration.save(&path).expect("restore legacy fixture");
        config(
            Some(&path),
            &ConfigAction::Set {
                setting: EncodeSet::Encoder {
                    encoder: EncoderMethod::AOM,
                },
            },
        )
        .expect("switch encoder to repair legacy settings");
        let switched = Configuration::load(&path).expect("load").expect("saved switch");
        assert_eq!(switched.condor.encoder.base(), EncoderBase::AOM);
        assert_eq!(switched.condor.encoder.validate_settings(), Ok(()));
        assert_eq!(
            fs::read_dir(temp.path()).expect("no indexing or side effects").count(),
            1
        );
    }

    #[test]
    fn c04_invalid_edits_are_atomic() {
        for (method, passes, params, noise, chroma, filter) in [
            (Some(EncoderMethod::X264), None, None, Some(100), None, None),
            (Some(EncoderMethod::FFmpeg), Some(1), None, None, None, None),
            (
                Some(EncoderMethod::FFmpeg),
                None,
                None,
                None,
                None,
                Some("hflip"),
            ),
            (None, None, None, None, Some(100), None),
            (None, Some(3), None, None, None, None),
            (None, None, Some("--crf NaN"), None, None, None),
            (Some(EncoderMethod::SVTAV1), Some(2), None, None, None, None),
            (
                Some(EncoderMethod::SVTAV1),
                None,
                Some("--rc 1 --crf 25"),
                None,
                None,
                None,
            ),
        ] {
            let mut encoder = Encoder::default_from_base(&EncoderBase::AOM, false);
            let before = encoder.clone();
            let result = configure_encoder(
                &mut encoder,
                method.as_ref(),
                passes,
                params,
                noise,
                chroma,
                filter,
                Some(false),
            );
            assert!(
                result.is_err(),
                "accepted {method:?}/{passes:?}/{params:?}/{noise:?}/{chroma:?}/{filter:?}"
            );
            assert_eq!(encoder, before, "failed edit changed the original");
        }
    }

    #[test]
    fn c04_switch_retains_explicit_controls_and_chroma_updates_luma() {
        let mut encoder = Encoder::default_from_base(&EncoderBase::X264, false);
        configure_encoder(
            &mut encoder,
            Some(&EncoderMethod::RAV1E),
            Some(2),
            Some("--speed 10"),
            Some(0),
            Some(0),
            Some("hflip"),
            Some(false),
        )
        .expect("supported switch");
        assert_eq!(encoder.base(), EncoderBase::RAV1E);
        assert_eq!(encoder.passes(), EncoderPasses::All(2));
        assert_eq!(encoder.speed().as_deref(), Some("10"));
        assert_eq!(encoder.ffmpeg_filter(), Some("hflip"));
        assert!(!encoder.append_color_data());
        let mut noise = encoder.photon_noise().expect("luma").clone();
        noise.c_y = Some(vec![1, 2, 3, 4]);
        noise.ccb = Some(vec![5; 5]);
        noise.ccr = Some(vec![-5; 5]);
        noise.chroma_iso = None;
        encoder.set_photon_noise(Some(noise.clone()));
        configure_encoder(&mut encoder, None, None, None, None, Some(0), None, None)
            .expect("chroma update");
        noise.chroma_iso = Some(0);
        assert_eq!(encoder.photon_noise(), Some(&noise));
        let before = encoder.clone();
        configure_encoder(&mut encoder, None, None, None, None, Some(70), None, None)
            .expect_err("independent chroma ISO is unsupported");
        assert_eq!(encoder, before, "invalid chroma edit remains transactional");
    }

    #[test]
    fn c04_explicit_svt_mode_change_replaces_only_stock_crf() {
        for params in
            ["--rc 1 --tbr 1000 --keyint 48", "--rc vbr --tbr 1M --keyint 2s", "--cqp 25.25"]
        {
            let mut encoder = Encoder::default_from_base(&EncoderBase::SVTAV1, false);
            configure_encoder(
                &mut encoder,
                None,
                None,
                Some(params),
                None,
                None,
                None,
                None,
            )
            .expect("supported explicit mode");
            assert!(!encoder.parameters().contains_key("crf"));
        }
        for params in ["--rc 1 --crf 25", "--crf 25 --rc 1", "--rc 2 --cqp 25", "--crf 25 --cqp 25"]
        {
            let mut encoder = Encoder::default_from_base(&EncoderBase::SVTAV1, false);
            let before = encoder.clone();
            configure_encoder(
                &mut encoder,
                None,
                None,
                Some(params),
                None,
                None,
                None,
                None,
            )
            .expect_err("explicit conflict");
            assert_eq!(encoder, before);
        }
    }

    #[test]
    fn c04_f6_explicit_quality_choices_replace_stock_not_custom_crf() {
        for params in [
            "--qp 32",
            "-q 32 --aq-mode 0",
            "--rc cqp",
            "--aq-mode 0",
            "--aq-mode 1",
            "--cqp 25.25 --rc cqp",
            "--qp 32 --rc cqp",
            "--rc cqp --qp 32",
        ] {
            let mut encoder = Encoder::default_from_base(&EncoderBase::SVTAV1, false);
            configure_encoder(
                &mut encoder,
                None,
                None,
                Some(params),
                None,
                None,
                None,
                None,
            )
            .expect("deterministic explicit choice");
            assert!(
                !encoder.parameters().contains_key("crf"),
                "stock CRF overrides {params}"
            );
        }
        for params in [
            "--crf 25 --aq-mode 0",
            "--aq-mode 0 --crf 25",
            "--crf 25 --qp 32",
            "--qp 32 --crf 25",
            "--cqp 25 --aq-mode 2",
            "--rc cqp --aq-mode 2",
            "--rc crf --cqp 25",
            "--qp 25 -q 32",
        ] {
            let mut encoder = Encoder::default_from_base(&EncoderBase::SVTAV1, false);
            let before = encoder.clone();
            let error = configure_encoder(
                &mut encoder,
                None,
                None,
                Some(params),
                None,
                None,
                None,
                None,
            )
            .expect_err("explicit conflict");
            assert!(
                matches!(
                    error.downcast_ref::<EncodeConfigError>(),
                    Some(EncodeConfigError::SvtMode(_))
                ),
                "{params}: {error:#}"
            );
            assert_eq!(encoder, before);
        }
        let mut encoder = Encoder::default_from_base(&EncoderBase::SVTAV1, false);
        encoder.set_quantizer(20.0);
        let before = encoder.clone();
        let error = configure_encoder(
            &mut encoder,
            None,
            None,
            Some("--qp 32"),
            None,
            None,
            None,
            None,
        )
        .expect_err("custom quality must not disappear");
        assert!(
            matches!(
                error.downcast_ref::<EncodeConfigError>(),
                Some(EncodeConfigError::SvtMode(_))
            ),
            "{error:#}"
        );
        assert_eq!(encoder, before);
        configure_encoder(
            &mut encoder,
            None,
            None,
            Some("--qp 20 --aq-mode 2"),
            None,
            None,
            None,
            None,
        )
        .expect("compatible explicit controls");
        assert_eq!(encoder.quantizer(), Some(20.0));
    }

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
            None,
            false,
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
            top: Some(140),
            bottom: Some(140),
            left: None,
            right: None,
        }];
        let custom_vs_args = vec!["method=target quality".to_string()];

        let mut expected_config = default_config(&test_video, &output, &temp_abs);
        expected_config.condor.encoder = Encoder::default_from_base(&EncoderBase::RAV1E, false);
        expected_config.condor.encoder.parameters_mut().insert(
            "speed".to_owned(),
            CLIParameter::Number {
                prefix: "--".to_owned(),
                delimiter: " ".to_owned(),
                value: 10.0,
            },
        );
        if let Some(encoder_passes) = expected_config.condor.encoder.passes_mut() {
            *encoder_passes = EncoderPasses::All(2);
        }
        expected_config.condor.encoder.set_photon_noise(Some(PhotonNoise {
            iso: 1600,
            chroma_iso: Some(1600),
            width: None,
            height: None,
            c_y: None,
            ccb: None,
            ccr: None,
        }));
        expected_config.condor.sequence_config.parallel_encoder = ParallelEncoderConfig {
            // Explicit override filters must not replace the root reference's filters.
            input: Some(Input::VapourSynth {
                path: input_abs.clone(),
                import_method: VapourSynthImportMethod::DGDecNV {
                    dgindexnv_executable: None,
                },
                cache_path: None,
                filters: custom_filters.clone(),
            }),
            workers: Some(2),
            scenes_directory: temp_abs.join("scenes"),
            validate_frame_count: true,
            max_tries: 3,
        };
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
            None,
            false,
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
            Some(1600),
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

    /// Filter-only edits reach the saved override without replacing the root reference.
    #[test]
    fn c08_encode_handler_filters_target_existing_override() {
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
            width: Some(960),
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
            None,
            false,
        )
        .expect("init_handler should succeed");

        let (mut config, _) =
            load_configuration(Some(&config_path)).expect("load_configuration should succeed");
        config.condor.sequence_config.parallel_encoder.input = Some(Input::VapourSynth {
            path: input_abs,
            import_method: VapourSynthImportMethod::DGDecNV {
                dgindexnv_executable: None,
            },
            cache_path: None,
            filters: vec![],
        });
        let mut expected = config.clone();
        if let Some(Input::VapourSynth { filters, .. }) =
            &mut expected.condor.sequence_config.parallel_encoder.input
        {
            *filters = custom_filters.clone();
        }
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
            None,
        )
        .expect("encode_handler should succeed");

        assert_eq!(
            serde_json::to_value(&config).expect("actual configuration"),
            serde_json::to_value(&expected).expect("only override filters replaced"),
        );
    }
}
