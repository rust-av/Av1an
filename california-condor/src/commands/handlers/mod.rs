use std::path::{Path, PathBuf};

use andean_condor::{
    models::input::{
        ImportMethod, Input as InputModel, VapourSynthImportMethod, VapourSynthScriptSource,
    },
    vapoursynth::vapoursynth_filters::VapourSynthFilter,
};
use anyhow::{Context, Result, bail, ensure};
use tracing::{debug, error, warn};

use crate::{
    DEFAULT_CONFIG_PATH,
    commands::{
        CondorCliError, DecoderMethod,
        config::{InputOverride, Toggle},
    },
    configuration::{ConfigError, Configuration},
    utils::hash_path::hash_path,
};

pub mod benchmarker;
pub mod concatenate;
pub mod config;
pub mod detect_noise;
pub mod detect_scenes;
pub mod encode;
pub mod init;
pub mod optimize_bitrate;
pub mod quality_check;
pub mod scale_noise;
pub mod scale_speed;
pub mod scenes;
pub mod start;
pub mod target_quality;
pub mod zone;

/// Loads a snapshot without locking. Direct transaction callers must acquire
/// [`crate::configuration::JobLock`] before this call, pass its resolved path,
/// and retain it through handler edits, execution, save callbacks and joins.
pub fn load_configuration(config_path: Option<&Path>) -> Result<(Configuration, PathBuf)> {
    if let Some(config_path) = config_path
        && !config_path.exists()
    {
        let err = CondorCliError::ConfigFileNotFound(config_path.to_path_buf());
        error!("{}", err);
        bail!(err);
    }
    let config_path =
        path_abs::PathAbs::new(config_path.unwrap_or_else(|| Path::new(DEFAULT_CONFIG_PATH)))?
            .as_path()
            .to_path_buf();
    if !config_path.exists() {
        let err = CondorCliError::NoConfig;
        error!("{}", err);
        bail!(err);
    }

    let configuration = {
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
    };

    Ok((configuration, config_path))
}

/// Changes an empty job's temp root without moving files or opening its source.
/// Equivalent roots are no-ops; populated jobs and foreign destinations refuse atomically.
pub fn configure_temp(configuration: &mut Configuration, temp_path: Option<&Path>) -> Result<()> {
    let Some(temp_path) = temp_path else {
        return Ok(());
    };
    let temp = path_abs::PathAbs::new(temp_path)
        .with_context(|| format!("normalizing temp directory `{}`", temp_path.display()))?
        .as_path()
        .to_path_buf();
    if normalized_path(&configuration.temp, "temp directory")?
        == normalized_path(temp_path, "temp directory")?
    {
        // Keep the saved representation, custom paths and history even for populated aliases.
        return Ok(());
    }

    let refusal = || {
        format!(
            "cannot change temp directory from `{}` to `{}`: saved scenes or nonempty work directory; start a new job with a new --config-file; --temp does not move files",
            configuration.temp.display(),
            temp.display()
        )
    };
    ensure!(configuration.condor.scenes.is_empty(), "{}", refusal());
    let sequence = &configuration.condor.sequence_config;
    let mut directories = [
        sequence.parallel_encoder.scenes_directory.clone(),
        sequence.scene_concatenator.scenes_directory.clone(),
    ];
    for directory in &directories {
        ensure!(
            !work_directory_populated(directory)?,
            "{} (`{}`)",
            refusal(),
            directory.display()
        );
    }
    let old_scenes = normalized_path(&configuration.temp.join("scenes"), "work directory")?;
    let new_scenes = temp.join("scenes");
    let mut rebased = [false; 2];
    for (index, directory) in directories.iter_mut().enumerate() {
        if directory.as_os_str().is_empty()
            || normalized_path(directory, "work directory")? == old_scenes
        {
            *directory = new_scenes.clone();
            rebased[index] = true;
        }
    }
    if rebased.contains(&true) {
        ensure!(
            !work_directory_populated(&new_scenes)?,
            "{} (destination `{}` already contains work)",
            refusal(),
            new_scenes.display()
        );
    }
    for ((directory, rebased), command) in
        directories.iter().zip(rebased).zip(["encode", "concatenate"])
    {
        if !rebased {
            warn!(
                "Preserving custom {command} scenes directory `{}`; --temp does not move those files; use `av1an {command} set scenes-directory PATH` to change it",
                directory.display()
            );
        }
    }
    // All filesystem checks precede publication; Target Quality derives probes from the encoder root.
    let [encoder, concat] = directories;
    let sequence = &mut configuration.condor.sequence_config;
    sequence.parallel_encoder.scenes_directory = encoder;
    sequence.scene_concatenator.scenes_directory = concat;
    configuration.temp = temp;
    Ok(())
}

fn root_source_path<'a>(input: &'a InputModel, fallback: &'a Path) -> &'a Path {
    match input {
        InputModel::Video { path, .. } | InputModel::VapourSynth { path, .. } => path,
        InputModel::VapourSynthScript { source, .. } => match source {
            VapourSynthScriptSource::Path(path) => path,
            VapourSynthScriptSource::Text(_) => fallback,
        },
    }
}

fn normalized_path(path: &Path, what: &str) -> Result<PathBuf> {
    match std::fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => path_abs::PathAbs::new(path)
            .map(|path| path.as_path().to_path_buf())
            .with_context(|| format!("normalizing offline {what} `{}`", path.display())),
        Err(error) => {
            Err(error).with_context(|| format!("normalizing {what} `{}`", path.display()))
        },
    }
}

fn work_directory_populated(path: &Path) -> Result<bool> {
    // An unset directory is not the process's current working directory.
    if path.as_os_str().is_empty() {
        return Ok(false);
    }
    let mut entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("inspecting work directory `{}`", path.display()));
        },
    };
    entries
        .next()
        .transpose()
        .map(|entry| entry.is_some())
        .with_context(|| format!("reading work directory `{}`", path.display()))
}

/// Atomically edits the job's root source without opening it or deleting work.
/// A different normalized path requires no scenes and empty known work directories;
/// stage overrides deliberately use the unrestricted `configure_input` builder instead.
pub(crate) fn configure_root_input(
    configuration: &mut Configuration,
    args: &InputOverride,
) -> Result<()> {
    let old_path = root_source_path(&configuration.condor.input, &configuration.input);
    let replacement = args
        .path
        .as_deref()
        .map(|path| -> Result<_> {
            Ok((
                normalized_path(old_path, "input")?,
                normalized_path(path, "input")?,
                path_abs::PathAbs::new(path)
                    .with_context(|| format!("normalizing input path `{}`", path.display()))?
                    .as_path()
                    .to_path_buf(),
            ))
        })
        .transpose()?;
    let changed = replacement.as_ref().is_some_and(|(old, new, _)| old != new);
    if changed {
        let sequence = &configuration.condor.sequence_config;
        let refusal = || {
            format!(
                "cannot change job input from `{}` to `{}`: saved scenes or nonempty work directory; use a new --config-file for a different source",
                old_path.display(),
                args.path.as_ref().expect("explicit replacement").display()
            )
        };
        ensure!(configuration.condor.scenes.is_empty(), "{}", refusal());
        for directory in [
            &sequence.parallel_encoder.scenes_directory,
            &sequence.scene_concatenator.scenes_directory,
        ] {
            ensure!(
                !work_directory_populated(directory)?,
                "{} (`{}`)",
                refusal(),
                directory.display()
            );
        }
    }

    let existing = &configuration.condor.input;
    // Path-only equivalent edits must not rebuild inline scripts or lose decoder caches.
    let mut input = if !changed && args.decoder.is_none() && args.vs_args.is_none() {
        existing.clone()
    } else if !changed
        && matches!(
            existing,
            InputModel::VapourSynthScript {
                source: VapourSynthScriptSource::Text(_),
                ..
            }
        )
    {
        let mut input = existing.clone();
        if let InputModel::VapourSynthScript { variables, .. } = &mut input
            && let Some(args) = &args.vs_args
        {
            *variables = args
                .iter()
                .map(|arg| {
                    let (key, value) = arg.split_once('=').unwrap_or((arg, ""));
                    (key.to_owned(), value.to_owned())
                })
                .collect();
        }
        input
    } else {
        configure_input(
            configuration,
            existing,
            replacement.as_ref().filter(|_| changed).map(|(_, _, new)| new.as_path()),
            args.decoder.as_ref(),
            args.vs_args.as_deref(),
            None,
            None,
        )?
    };
    if let (
        InputModel::VapourSynth {
            import_method: old_method,
            cache_path: old_cache,
            ..
        },
        InputModel::VapourSynth {
            import_method,
            cache_path,
            ..
        },
    ) = (existing, &mut input)
    {
        if std::mem::discriminant(old_method) == std::mem::discriminant(import_method) {
            *cache_path = old_cache.clone();
        }
        if let (
            VapourSynthImportMethod::DGDecNV {
                dgindexnv_executable: old,
            },
            VapourSynthImportMethod::DGDecNV {
                dgindexnv_executable,
            },
        ) = (old_method, import_method)
        {
            *dgindexnv_executable = old.clone();
        }
    }
    if changed {
        // Reset the resulting kind: script output 0 is not a video track selection.
        match &mut input {
            InputModel::Video {
                import_method: ImportMethod::FFMS2 { index, cache_path },
                ..
            } => {
                *index = None;
                *cache_path = None;
            },
            InputModel::VapourSynth {
                import_method,
                cache_path,
                ..
            } => {
                *cache_path = None;
                match import_method {
                    VapourSynthImportMethod::LSMASHWorks { index }
                    | VapourSynthImportMethod::FFMS2 { index }
                    | VapourSynthImportMethod::BestSource { index } => *index = None,
                    VapourSynthImportMethod::DGDecNV { .. } => {},
                }
            },
            InputModel::VapourSynthScript { index, .. } => *index = 0,
        }
    }
    if let Some(filters) = args.filters.as_deref() {
        apply_input_filters(&mut input, filters)?;
    }
    apply_stream_concurrently(&mut input, args.stream_concurrently);
    if let Some(path) = args.path.as_deref() {
        warn_missing_path("input path", path);
    }
    let mut temp = configuration.temp.clone();
    if let Some((old_identity, _, new)) = replacement.as_ref().filter(|_| changed) {
        // Fresh init hashes lexical absolute paths; also recognize already-canonical auto roots.
        let old_lexical = path_abs::PathAbs::new(old_path)
            .with_context(|| format!("normalizing previous input path `{}`", old_path.display()))?;
        if temp.file_name() == Some(std::ffi::OsStr::new(&hash_path(old_lexical.as_path())))
            || temp.file_name() == Some(std::ffi::OsStr::new(&hash_path(old_identity)))
        {
            temp.set_file_name(hash_path(new));
        }
    }

    // Publish only after all fallible model/filter conversions have succeeded.
    if args.path.is_some() {
        configuration.input = root_source_path(&input, &configuration.input).to_path_buf();
    }
    configuration.condor.input = input;
    if changed {
        let old_scenes = configuration.temp.join("scenes");
        let sequence = &mut configuration.condor.sequence_config;
        for directory in [
            &mut sequence.parallel_encoder.scenes_directory,
            &mut sequence.scene_concatenator.scenes_directory,
        ] {
            if directory.as_os_str().is_empty() || *directory == old_scenes {
                *directory = temp.join("scenes");
            }
        }
        configuration.temp = temp;
    }
    Ok(())
}

/// Rebuilds an input model without opening or indexing its source.
/// Invalid native format conversions return an error without modifying the original.
pub fn configure_input(
    configuration: &Configuration,
    existing_input: &InputModel,
    input_path: Option<&Path>,
    decoder: Option<&DecoderMethod>,
    vs_args: Option<&[String]>,
    index: Option<u8>,
    cache_path: Option<&Path>,
) -> Result<InputModel> {
    let (existing_input_path, existing_decoder, existing_vs_args, existing_index, existing_cache) =
        match existing_input {
            InputModel::Video {
                path,
                import_method,
                ..
            } => match import_method {
                ImportMethod::FFMS2 { index, cache_path } => (
                    path,
                    Some(DecoderMethod::FFMS2),
                    None,
                    *index,
                    cache_path.as_deref(),
                ),
            },
            InputModel::VapourSynth {
                path,
                import_method,
                ..
            } => match import_method {
                VapourSynthImportMethod::LSMASHWorks { index } => {
                    (path, Some(DecoderMethod::LSMASHWorks), None, *index, None)
                },
                VapourSynthImportMethod::DGDecNV { .. } => {
                    (path, Some(DecoderMethod::DGDecodeNV), None, None, None)
                },
                VapourSynthImportMethod::FFMS2 { index } => {
                    (path, Some(DecoderMethod::VSFFMS2), None, *index, None)
                },
                VapourSynthImportMethod::BestSource { index } => {
                    (path, Some(DecoderMethod::BestSource), None, *index, None)
                },
            },
            InputModel::VapourSynthScript {
                source,
                variables,
                index,
                ..
            } => match source {
                VapourSynthScriptSource::Path(path) => {
                    (path, None, Some(variables.clone()), Some(*index), None)
                },
                VapourSynthScriptSource::Text(_) => (
                    &configuration.input,
                    None,
                    Some(variables.clone()),
                    Some(*index),
                    None,
                ),
            },
        };

    let existing_vs_args: Option<Vec<String>> = existing_vs_args
        .map(|args| args.iter().map(|(key, value)| format!("{}={}", key, value)).collect());

    // Only a script has this knob, and the rebuild below cannot know it, so it
    // is read separately rather than folded into the tuple above.
    let existing_stream_concurrently = match existing_input {
        InputModel::VapourSynthScript {
            stream_concurrently,
            ..
        } => Some(*stream_concurrently),
        InputModel::Video { .. } | InputModel::VapourSynth { .. } => None,
    };

    let mut input = Configuration::new_input_model(
        path_abs::PathAbs::new(input_path.unwrap_or(existing_input_path))?.as_path(),
        decoder.or(existing_decoder.as_ref()),
        vs_args.or(existing_vs_args.as_deref()),
        index.or(existing_index),
        cache_path.or(existing_cache),
    )?;
    warn_unused_vs_args(&input, vs_args);
    // The input is rebuilt from its path and decoder, which drops its filters,
    // so carry them over.
    input.adopt_filters(existing_input)?;
    if let Some(stream_concurrently) = existing_stream_concurrently
        && let InputModel::VapourSynthScript {
            stream_concurrently: rebuilt,
            ..
        } = &mut input
    {
        *rebuilt = stream_concurrently;
    }
    Ok(input)
}

/// A human-readable description of an input's resolved kind, for diagnostics.
pub fn input_kind_label(input: &InputModel) -> &'static str {
    match input {
        InputModel::Video {
            import_method: ImportMethod::FFMS2 { .. },
            ..
        } => "native FFMS2 video input",
        InputModel::VapourSynth {
            import_method: VapourSynthImportMethod::BestSource { .. },
            ..
        } => "BestSource video input",
        InputModel::VapourSynth {
            import_method: VapourSynthImportMethod::FFMS2 { .. },
            ..
        } => "VapourSynth FFMS2 video input",
        InputModel::VapourSynth {
            import_method: VapourSynthImportMethod::LSMASHWorks { .. },
            ..
        } => "L-SMASH-Works video input",
        InputModel::VapourSynth {
            import_method: VapourSynthImportMethod::DGDecNV { .. },
            ..
        } => "DGDecNV video input",
        InputModel::VapourSynthScript { .. } => "VapourSynth script input",
    }
}

/// Warns that a script-only flag was ignored for an input that cannot use it.
///
/// VapourSynth script arguments and concurrent streaming only reach a
/// `.vpy`/`.py` input; a video or an indexed source has no script environment
/// to pass them to, so the flag would otherwise be dropped without a word.
pub fn warn_script_flag_ignored(flag: &str, input: &InputModel) {
    let kind = input_kind_label(input);
    warn!(
        "{flag} applies only to VapourSynth script (.vpy/.py) inputs and was ignored for this \
         {kind}; drop {flag} or use a script input"
    );
}

/// Warns when `--vs-args` was given for an input that cannot use it.
pub fn warn_unused_vs_args(input: &InputModel, vs_args: Option<&[String]>) {
    let given = vs_args.is_some_and(|args| !args.is_empty());
    if given && !matches!(input, InputModel::VapourSynthScript { .. }) {
        warn_script_flag_ignored("--vs-args", input);
    }
}

/// Applies `--stream-concurrently` to `input`, or warns when `input` is not a
/// VapourSynth script.
pub fn apply_stream_concurrently(input: &mut InputModel, toggle: Option<Toggle>) {
    let Some(toggle) = toggle else {
        return;
    };
    if let InputModel::VapourSynthScript {
        stream_concurrently,
        ..
    } = input
    {
        *stream_concurrently = toggle.enabled();
    } else {
        warn_script_flag_ignored("--stream-concurrently", input);
    }
}

/// Warns when `path` does not exist yet on disk.
///
/// Editing a configuration does not open the file immediately, so a missing
/// path is accepted with a warning rather than rejected.
pub fn warn_missing_path(what: &str, path: &Path) {
    if !path.exists() {
        warn!(
            "{what} `{}` does not exist; it will be needed when the step runs",
            path.display()
        );
    }
}

/// Warns about missing explicit paths, leaving bare executable names for PATH
/// lookup when encoding starts. Offline configuration edits never probe binaries.
pub fn warn_missing_executable(path: &Path) {
    if path.file_name() != Some(path.as_os_str()) {
        warn_missing_path("encoder executable", path);
    }
}

/// Builds the override input an [`InputOverride`] describes, warning if a
/// supplied path does not exist yet.
///
/// The caller has already rejected an override that sets no flag
/// ([`ensure_override_requested`](super::config::ensure_override_requested)),
/// so this always builds one.
pub fn apply_input_override(
    configuration: &Configuration,
    existing_input: &InputModel,
    args: &InputOverride,
) -> Result<InputModel> {
    if let Some(path) = args.path.as_deref() {
        warn_missing_path("input path", path);
    }
    let mut input = configure_input(
        configuration,
        existing_input,
        args.path.as_deref(),
        args.decoder.as_ref(),
        args.vs_args.as_deref(),
        None,
        None,
    )?;
    if let Some(filters) = args.filters.as_deref() {
        apply_input_filters(&mut input, filters)?;
    }
    apply_stream_concurrently(&mut input, args.stream_concurrently);
    Ok(input)
}

/// Replaces `input`'s filters, warning about any its decoder cannot run.
///
/// # Errors
/// An unsupported explicit native format or invalid dimension leaves the input unchanged.
pub fn apply_input_filters(input: &mut InputModel, filters: &[VapourSynthFilter]) -> Result<()> {
    let unsupported = InputModel::unsupported_filters(filters);
    input.set_filters(filters.to_vec())?;
    for filter in unsupported {
        warn!(
            "{filter} needs VapourSynth and was dropped: a native FFMS2 input can only convert \
             bit depth, chroma and resolution"
        );
    }
    Ok(())
}

/// Builds a sequence's override input, or `None` when no override flag is set.
///
/// Filters count as an override, so passing only filters still gives the
/// sequence its own input. Writing them to the main input instead would replace
/// its filters, losing the conversions the encoder depends on.
pub fn configure_override_input(
    configuration: &Configuration,
    existing_input: &InputModel,
    input_path: Option<&Path>,
    decoder: Option<&DecoderMethod>,
    filters: Option<&[VapourSynthFilter]>,
    vs_args: Option<&[String]>,
) -> Result<Option<InputModel>> {
    if input_path.is_none() && decoder.is_none() && filters.is_none() && vs_args.is_none() {
        return Ok(None);
    }

    let mut input = configure_input(
        configuration,
        existing_input,
        input_path,
        decoder,
        vs_args,
        None,
        None,
    )?;
    if let Some(filters) = filters {
        apply_input_filters(&mut input, filters)?;
    }

    Ok(Some(input))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_tq_and_qc_profiles_select_middle_quarter_with_rms() {
        use crate::{
            commands::QualityProfile,
            test_helpers::{TestVideo, default_config},
        };
        use andean_condor::models::{
            input::ImportMethod,
            sequence::target_quality::{TargetQualityConfig, types::ProbeStatistic},
        };
        let root = std::env::temp_dir().join("condor-profile-model-only");
        let video = TestVideo {
            path: root.join("must-not-open.mkv"),
            width: 32,
            height: 32,
            frames: 100,
            fps_rational: (24, 1),
            format: andean_condor::ffmpeg::FFPixelFormat::YUV420P,
            scenes: vec![(0, 50), (50, 100)],
        };
        let mut configuration =
            default_config(&video, &root.join("output.mkv"), &root.join("scratch"));
        configuration.condor.scenes = video.mock_scenes(&configuration.condor.encoder);
        configuration.condor.input = InputModel::Video {
            path: video.path,
            import_method: ImportMethod::FFMS2 {
                index: Some(0),
                cache_path: Some(root.join("forbidden.ffindex")),
            },
            filters: vec![],
        };
        configuration.condor.sequence_config.target_quality = Some(TargetQualityConfig::default());
        configuration.condor.sequence_config.quality_check = Some(Default::default());
        target_quality::configure_target_quality(
            &mut configuration,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(&QualityProfile::Standard),
        )
        .expect("TQ profile");
        quality_check::configure_quality_check(
            &mut configuration,
            None,
            None,
            None,
            None,
            None,
            Some(&QualityProfile::Standard),
        )
        .expect("QC profile");
        let tq = configuration.condor.sequence_config.target_quality.as_ref().expect("TQ");
        let qc = configuration.condor.sequence_config.quality_check.as_ref().expect("QC");
        for strategy in [&tq.probing.strategy, &qc.strategy] {
            assert_eq!(
                strategy.frame_indices(0, 100).expect("selection"),
                (38..63).collect::<Vec<_>>()
            );
        }
        assert_eq!(tq.probing.statistic, ProbeStatistic::RootMeanSquare);
        assert_eq!(qc.statistic, ProbeStatistic::RootMeanSquare);
    }
}
