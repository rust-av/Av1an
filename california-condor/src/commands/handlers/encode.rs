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
    configuration.admit_results_for_encoding()?;
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
    use super::*;

    #[test]
    fn configure_parallel_encoder_zero_attempt_budget_preserves_configuration() {
        use crate::test_helpers::{TestVideo, default_config};
        use andean_condor::{
            ffmpeg::FFPixelFormat,
            models::{
                input::{ChromaSampling, Ffms2Filter, ImportMethod, Input},
                sequence::{
                    quality_check::QualityCheckConfig, target_quality::TargetQualityConfig,
                },
            },
        };

        let directory = std::env::temp_dir().join("condor-zero-attempt-model-only");
        let video = TestVideo {
            path: directory.join("root.mkv"),
            width: 32,
            height: 16,
            frames: 2,
            fps_rational: (24, 1),
            format: FFPixelFormat::YUV420P,
            scenes: vec![],
        };
        let mut configuration =
            default_config(&video, &directory.join("out.mkv"), &directory.join("work"));
        configuration.condor.input = Input::Video {
            path: video.path,
            import_method: ImportMethod::FFMS2 {
                index: Some(0),
                cache_path: Some(directory.join("root.ffindex")),
            },
            filters: vec![Ffms2Filter::OutputFormat {
                bit_depth: Some(10),
                chroma: Some(ChromaSampling::Yuv420),
                width: None,
                height: None,
            }],
        };
        let mut reference = configuration.condor.input.clone();
        if let Input::Video { path, .. } = &mut reference {
            *path = directory.join("metric.mkv");
        }
        configuration.condor.sequence_config.target_quality = Some(TargetQualityConfig {
            input: Some(reference.clone()),
            metric_input: Some(reference.clone()),
            ..Default::default()
        });
        configuration.condor.sequence_config.quality_check = Some(QualityCheckConfig {
            input: Some(reference),
            ..Default::default()
        });
        let before = serde_json::to_value(&configuration).expect("snapshot");
        assert!(
            configure_parallel_encoder(
                &mut configuration,
                None,
                None,
                None,
                None,
                Some(7),
                Some(0)
            )
            .is_err()
        );
        assert_eq!(
            serde_json::to_value(configuration).expect("invalid budget atomic"),
            before
        );
    }

    mod parameter_argv {
        use super::super::configure_encoder;
        use andean_condor::{
            core::input::{clip_info::ClipInfo, pixel_format::PixelFormat},
            models::encoder::{Encoder, EncoderBase},
        };

        fn clip_info() -> ClipInfo {
            ClipInfo {
                num_frames: 1,
                format_info: PixelFormat::VapourSynth { bit_depth: 8 },
                frame_rate: av_format::rational::Rational64::new(24, 1),
                resolution: (64, 64),
                color_range: None,
                color_primaries: None,
                color_transfer_characteristic: None,
                color_space: None,
                chroma_location: None,
                mastering_display: None,
                content_light_level: None,
            }
        }

        #[test]
        fn consecutive_flags_override_default_crf_in_encoder_argv() {
            let root = std::env::temp_dir().join("condor-encoder-argv-model-only");
            let output = root.join("out.mkv");
            let stats = root.join("stats");
            let mut encoder = Encoder::default_from_base(&EncoderBase::X265, false);
            encoder.set_append_color_data(false);
            let before = encoder
                .parameters_with_output((1, 1), &output, &clip_info(), Some(&stats))
                .expect("default argv");
            configure_encoder(
                &mut encoder,
                None,
                None,
                Some("--no-sao --crf 20"),
                None,
                None,
                None,
                None,
            )
            .expect("valid parameters");
            let argv = encoder
                .parameters_with_output((1, 1), &output, &clip_info(), Some(&stats))
                .expect("configured argv");
            assert_eq!(argv.len(), before.len() + 1, "{argv:?}");
            assert_eq!(argv.iter().filter(|arg| *arg == "--no-sao").count(), 1);
            assert_eq!(argv.iter().filter(|arg| *arg == "--crf").count(), 1);
            assert!(
                argv.windows(2).any(|pair| pair == ["--crf", "20"]),
                "{argv:?}"
            );
        }

        #[test]
        fn configured_parameters_reach_encoder_argv_without_resplitting() {
            let root = std::env::temp_dir().join("condor-encoder-argv-model-only");
            let output = root.join("out.mkv");
            let stats = root.join("stats");
            for (input, expected) in [
                (
                    r#"--key 'C:\my clips\source.y4m'"#,
                    vec![vec!["--key", r"C:\my clips\source.y4m"]],
                ),
                (
                    "--no-sao --crf 20",
                    vec![vec!["--no-sao"], vec!["--crf", "20"]],
                ),
                (
                    "--sharpness -2 --crf 20",
                    vec![vec!["--sharpness", "-2"], vec!["--crf", "20"]],
                ),
                ("-an -sn", vec![vec!["-an"], vec!["-sn"]]),
                ("--key=value", vec![vec!["--key=value"]]),
                ("--key=a=b", vec![vec!["--key=a=b"]]),
                (
                    "--key\tvalue\n--other\tword",
                    vec![vec!["--key", "value"], vec!["--other", "word"]],
                ),
                (r#"--key "a b""#, vec![vec!["--key", "a b"]]),
                (r#"--key="a b""#, vec![vec!["--key=a b"]]),
                (r#"--key "say \"hi\"""#, vec![vec!["--key", "say \"hi\""]]),
                (r#"--key """#, vec![vec!["--key", ""]]),
                ("--key=", vec![vec!["--key="]]),
                ("--key -0.5", vec![vec!["--key", "-0.5"]]),
                ("--key -1e-3", vec![vec!["--key", "-0.001"]]),
                ("--key=-literal", vec![vec!["--key=-literal"]]),
                ("crf=16 tune=0", vec![vec!["crf=16"], vec!["tune=0"]]),
                ("crf 16", vec![vec!["crf", "16"]]),
                ("-metadata:s:v 'a b'", vec![vec!["-metadata:s:v", "a b"]]),
                (
                    r#"--key '$HOME;$(echo literal)*'"#,
                    vec![vec!["--key", "$HOME;$(echo literal)*"]],
                ),
                (
                    "--key value # comment\n--other=word",
                    vec![vec!["--key", "value"], vec!["--other=word"]],
                ),
                ("", vec![]),
                (" \t\n ", vec![]),
                ("\r\n \t\u{b}", vec![]),
            ] {
                let mut encoder = Encoder::default_from_base(&EncoderBase::X265, false);
                encoder.parameters_mut().clear();
                configure_encoder(
                    &mut encoder,
                    None,
                    None,
                    Some(input),
                    None,
                    None,
                    None,
                    Some(false),
                )
                .expect("valid encoder parameters");
                let argv = encoder
                    .parameters_with_output((1, 1), &output, &clip_info(), Some(&stats))
                    .expect("actual emitted encoder argv");
                assert_eq!(
                    argv.len(),
                    3 + expected.iter().map(Vec::len).sum::<usize>(),
                    "{input:?}: {argv:?}"
                );
                assert_eq!(argv[0], "-");
                assert!(
                    argv.windows(2).any(|pair| pair == ["-o", output.to_str().expect("UTF-8")]),
                    "{argv:?}"
                );
                for tokens in expected {
                    assert!(
                        argv.windows(tokens.len()).any(|window| window == tokens),
                        "{input:?}: missing {tokens:?} in {argv:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn avm_explicit_noise_edits_fail_atomically_offline() {
        for method in [None, Some(EncoderMethod::AVM)] {
            for (noise, chroma) in [(Some(800), None), (None, Some(400)), (Some(800), Some(400))] {
                let mut encoder = Encoder::default_from_base(&EncoderBase::AVM, false);
                encoder.set_executable(Some("/missing/avmenc".into()));
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
    fn configure_encoder_invalid_controls_leave_the_original_encoder_unchanged() {
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
    fn malformed_parameters_leave_configure_encoder_unchanged() {
        for input in [
            "--key \"unfinished",
            "--key value\\",
            "--crf 20 --crf 21",
            "--ok=1 --",
            "--ok=1 trailing",
        ] {
            let mut encoder = Encoder::default_from_base(&EncoderBase::AOM, false);
            let before = encoder.clone();
            let error = configure_encoder(
                &mut encoder,
                Some(&EncoderMethod::X265),
                Some(1),
                Some(input),
                None,
                None,
                Some("hflip"),
                Some(false),
            )
            .expect_err("malformed parameters must not commit any edits");
            assert!(
                error.to_string().contains("Invalid parameter"),
                "{input:?}: {error:#}"
            );
            assert_eq!(encoder, before, "{input:?}");
        }
    }

    fn check_encoder_switch(phase: u8) {
        assert!((1..=3).contains(&phase));
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
        if phase == 1 {
            return;
        }
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
        if phase == 2 {
            return;
        }
        let before = encoder.clone();
        configure_encoder(&mut encoder, None, None, None, None, Some(70), None, None)
            .expect_err("independent chroma ISO is unsupported");
        assert_eq!(encoder, before, "invalid chroma edit remains transactional");
    }

    #[test]
    fn encoder_switch_retains_requested_passes_speed_filter_and_color_policy() {
        check_encoder_switch(1);
    }

    #[test]
    fn chroma_noise_edit_preserves_other_grain_fields() {
        check_encoder_switch(2);
    }

    #[test]
    fn invalid_chroma_noise_edit_leaves_encoder_unchanged() {
        check_encoder_switch(3);
    }

    #[test]
    fn explicit_svt_mode_change_replaces_stock_crf() {
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
    }

    #[test]
    fn conflicting_explicit_svt_modes_leave_encoder_unchanged() {
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

    fn check_quality_choices(phase: u8) {
        assert!((1..=4).contains(&phase));
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
        if phase == 1 {
            return;
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
        if phase == 2 {
            return;
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
        if phase == 3 {
            return;
        }
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
    fn explicit_svt_quality_choices_remove_stock_crf() {
        check_quality_choices(1);
    }

    #[test]
    fn conflicting_svt_quality_controls_return_typed_error_without_mutation() {
        check_quality_choices(2);
    }

    #[test]
    fn explicit_svt_quality_choice_cannot_discard_custom_crf() {
        check_quality_choices(3);
    }

    #[test]
    fn compatible_svt_quality_controls_preserve_custom_quantizer() {
        check_quality_choices(4);
    }
}
