use std::path::{Path, PathBuf};

use andean_condor::models::{
    input::Input,
    sequence::scene_concatenator::{
        Chapters,
        ConcatMethod,
        FfmpegConfig,
        FfmpegTrack,
        FfmpegTrackKind,
        FfmpegTrackMap,
        Metadata,
        MkvmergeConfig,
        MkvmergeTrack,
        MkvmergeTrackKind,
        SceneConcatenatorConfig,
        TrackMap,
    },
};
use anyhow::Result;
use clap::Subcommand;
use serde_json::Value;
use thiserror::Error;
use tracing::{debug, warn};

use crate::{
    commands::{
        ConcatenationMethod,
        DiscardCategory,
        config::{ConfigAction, ConfigCommand, PropertyChange},
        handlers::{config::edit, configure_temp, load_configuration},
    },
    configuration::Configuration,
    utils::track_probe::{self, ProbedSource, TrackKind},
};

#[allow(clippy::too_many_arguments)]
pub fn concatenate_handler(
    config_path: Option<&Path>,
    temp_path: Option<&Path>,
    method: Option<&ConcatenationMethod>,
    discard: Option<&[DiscardCategory]>,
) -> Result<(Configuration, PathBuf)> {
    let (mut configuration, config_path) = load_configuration(config_path)?;

    configure_temp(&mut configuration, temp_path)?;
    configure_concatenate(&mut configuration, method)?;
    apply_discard(&mut configuration, discard.unwrap_or_default())?;

    configuration.save(&config_path)?;

    Ok((configuration, config_path))
}

pub fn configure_concatenate(
    configuration: &mut Configuration,
    method: Option<&ConcatenationMethod>,
) -> Result<()> {
    if let Some(concat) = method {
        // Switching methods keeps that method's existing settings; changing to
        // a different method starts from its defaults.
        let existing = configuration.condor.sequence_config.scene_concatenator.method.clone();
        let concat = match concat {
            ConcatenationMethod::MkvMerge => match existing {
                ConcatMethod::MKVMerge(config) => ConcatMethod::MKVMerge(config),
                _ => ConcatMethod::MKVMerge(MkvmergeConfig::default()),
            },
            ConcatenationMethod::FFmpeg => match existing {
                ConcatMethod::FFmpeg(config) => ConcatMethod::FFmpeg(config),
                _ => ConcatMethod::FFmpeg(FfmpegConfig::default()),
            },
            ConcatenationMethod::Ivf => ConcatMethod::Ivf,
        };
        configuration.condor.sequence_config.scene_concatenator.method = concat;
    }

    Ok(())
}

/// Everything that can make a `--discard` request unusable.
///
/// The two warnings do not fail the run: the flag is a convenience, so a
/// request that cannot be honored leaves the configuration as it was and the
/// reason is reported instead.
#[derive(Debug, Error)]
pub enum ConcatenateConfigError {
    #[error(
        "--discard has no effect with the ivf method, which never carries source tracks, chapters \
         or metadata"
    )]
    IvfMethodIgnoresDiscard,
    #[error(
        "--discard needs a file input to inspect, and this configuration's input is a VapourSynth \
         script; nothing was discarded"
    )]
    ScriptInputNotProbed,
    #[error("discarding {summary} of {path}", path = .path.display())]
    TracksDiscarded { path: PathBuf, summary: String },
}

/// Records the tracks `--discard` names as `copy: false` entries, so the
/// concatenator leaves them out of the output.
///
/// Nothing is probed when `discard` is empty, and the IVF method ignores the
/// flag because it never carries source tracks in the first place.
///
/// # Errors
///
/// Fails when the input cannot be probed, which would otherwise silently keep
/// the tracks the flag asked to drop.
pub fn apply_discard(configuration: &mut Configuration, discard: &[DiscardCategory]) -> Result<()> {
    if discard.is_empty() {
        return Ok(());
    }
    let method = configuration.condor.sequence_config.scene_concatenator.method.clone();
    if matches!(method, ConcatMethod::Ivf) {
        warn!("{}", ConcatenateConfigError::IvfMethodIgnoresDiscard);
        return Ok(());
    }

    let Some(input) = discard_input(configuration) else {
        warn!("{}", ConcatenateConfigError::ScriptInputNotProbed);
        return Ok(());
    };

    let with_ffmpeg_streams = matches!(method, ConcatMethod::FFmpeg(_));
    let source = track_probe::probe(&input, with_ffmpeg_streams)?;
    let summary = track_probe::summarize(&source);
    debug!("{}", ConcatenateConfigError::TracksDiscarded {
        path: input,
        summary,
    });

    match method {
        ConcatMethod::MKVMerge(mut mkvmerge) => {
            discard_mkvmerge(&mut mkvmerge, &source, discard);
            configuration.condor.sequence_config.scene_concatenator.method =
                ConcatMethod::MKVMerge(mkvmerge);
        },
        ConcatMethod::FFmpeg(mut ffmpeg) => {
            discard_ffmpeg(&mut ffmpeg, &source, discard);
            configuration.condor.sequence_config.scene_concatenator.method =
                ConcatMethod::FFmpeg(ffmpeg);
        },
        ConcatMethod::Ivf => (),
    }

    Ok(())
}

/// The file `--discard` probes: the configured input, when it is something
/// mkvmerge can identify.
fn discard_input(configuration: &Configuration) -> Option<PathBuf> {
    match &configuration.condor.input {
        Input::Video {
            path, ..
        }
        | Input::VapourSynth {
            path, ..
        } => Some(path.clone()),
        // A script's own tracks depend on the script, which mkvmerge cannot
        // read, and the frame server has no container tracks of its own.
        Input::VapourSynthScript {
            ..
        } => None,
    }
}

/// Adds the discard entries to the mkvmerge settings.
///
/// mkvmerge's map is an override map, so only the dropped tracks are written;
/// everything else keeps being copied by default.
fn discard_mkvmerge(
    config: &mut MkvmergeConfig,
    source: &ProbedSource,
    discard: &[DiscardCategory],
) {
    for category in discard {
        match category {
            DiscardCategory::Audio => drop_mkvmerge_tracks(config, source, TrackKind::Audio),
            DiscardCategory::Subtitle => drop_mkvmerge_tracks(config, source, TrackKind::Subtitle),
            DiscardCategory::Attachment => {
                drop_mkvmerge_tracks(config, source, TrackKind::Attachment);
            },
            // The video entry describes the *output* track, so it is always
            // ID 0 rather than a probed source track.
            DiscardCategory::Video => drop_output_video_mkvmerge(config),
            DiscardCategory::Chapters => {
                config.chapters.get_or_insert_with(Chapters::default).copy = false;
            },
            DiscardCategory::Metadata => {
                config.metadata.get_or_insert_with(Metadata::default).copy = false;
            },
        }
    }
}

/// Marks every probed track of `kind` as not copied.
fn drop_mkvmerge_tracks(config: &mut MkvmergeConfig, source: &ProbedSource, kind: TrackKind) {
    let track_kind = match kind {
        TrackKind::Video => MkvmergeTrackKind::Video,
        TrackKind::Audio => MkvmergeTrackKind::Audio,
        TrackKind::Subtitle => MkvmergeTrackKind::Subtitle,
        TrackKind::Attachment => MkvmergeTrackKind::Attachment,
    };
    for id in source.mkvmerge_ids(kind) {
        let id = u32::try_from(id).unwrap_or(u32::MAX);
        config
            .tracks
            .get_or_insert_with(TrackMap::new)
            .entry(id)
            .and_modify(|track| track.set_copy(false))
            .or_insert_with(|| MkvmergeTrack::plain(track_kind, false));
    }
}

/// Drops the encoded video from the mkvmerge output.
fn drop_output_video_mkvmerge(config: &mut MkvmergeConfig) {
    let tracks = config.tracks.get_or_insert_with(TrackMap::new);
    // Validation only allows one video entry, at ID 0, so an existing one is
    // the entry to drop rather than a second one alongside it.
    if let Some(existing) =
        tracks.values_mut().find(|track| matches!(track, MkvmergeTrack::Video(_)))
    {
        existing.set_copy(false);
    } else {
        tracks.insert(0, MkvmergeTrack::plain(MkvmergeTrackKind::Video, false));
    }
}

/// Adds the discard entries to the FFmpeg settings.
///
/// FFmpeg's map is a selection map: a stream that is absent is simply not
/// mapped, so writing a map from scratch means listing every kept stream.
/// When the map is absent it is filled from the probe, because otherwise adding
/// one `copy: false` entry would drop every stream that was not listed.
fn discard_ffmpeg(config: &mut FfmpegConfig, source: &ProbedSource, discard: &[DiscardCategory]) {
    let dropped = |kind: TrackKind| match kind {
        TrackKind::Audio => discard.contains(&DiscardCategory::Audio),
        TrackKind::Subtitle => discard.contains(&DiscardCategory::Subtitle),
        TrackKind::Attachment => discard.contains(&DiscardCategory::Attachment),
        // The output video is the encoded one, not a source stream.
        TrackKind::Video => false,
    };

    let drops_source_streams = source
        .tracks
        .iter()
        .any(|track| track.kind != TrackKind::Video && dropped(track.kind));
    // Any entry written into an absent map narrows the selection, so the map
    // has to be filled with every kept stream first — including when only the
    // video entry is added.
    let writes_tracks = drops_source_streams || discard.contains(&DiscardCategory::Video);
    if writes_tracks && config.tracks.is_none() {
        let filled = source
            .tracks
            .iter()
            .filter(|track| track.kind != TrackKind::Video)
            .filter_map(|track| {
                let index = track.ffmpeg_index?;
                let kind = ffmpeg_kind(track.kind);
                Some((index, FfmpegTrack::plain(kind, true)))
            })
            .collect::<FfmpegTrackMap>();
        config.tracks = Some(filled);
    }

    if let Some(tracks) = config.tracks.as_mut() {
        for track in source.tracks.iter().filter(|track| dropped(track.kind)) {
            let Some(index) = track.ffmpeg_index else {
                continue;
            };
            let kind = ffmpeg_kind(track.kind);
            tracks
                .entry(index)
                .and_modify(|track| track.set_copy(false))
                .or_insert_with(|| FfmpegTrack::plain(kind, false));
        }
    }

    // The encoded video is the output's only video stream; dropping it leaves
    // an audio/subtitle-only file.
    if discard.contains(&DiscardCategory::Video) {
        let tracks = config.tracks.get_or_insert_with(FfmpegTrackMap::new);
        match tracks.values_mut().find(|track| matches!(track, FfmpegTrack::Video(_))) {
            Some(existing) => existing.set_copy(false),
            None => {
                tracks.insert(0, FfmpegTrack::plain(FfmpegTrackKind::Video, false));
            },
        }
    }

    if discard.contains(&DiscardCategory::Chapters) {
        config.chapters.get_or_insert_with(Chapters::default).copy = false;
    }
    if discard.contains(&DiscardCategory::Metadata) {
        config.metadata.get_or_insert_with(Metadata::default).copy = false;
    }
}

/// The FFmpeg track kind corresponding to a probed track kind.
fn ffmpeg_kind(kind: TrackKind) -> FfmpegTrackKind {
    match kind {
        TrackKind::Video => FfmpegTrackKind::Video,
        TrackKind::Audio => FfmpegTrackKind::Audio,
        TrackKind::Subtitle => FfmpegTrackKind::Subtitle,
        TrackKind::Attachment => FfmpegTrackKind::Attachment,
    }
}

/// `info` and `reset` keys of `sequence_config.scene_concatenator`.
#[derive(Debug, Clone, Subcommand)]
pub enum ConcatenateKey {
    /// The tool used to join the encoded scenes.
    Method,
    /// The file the concatenated video is written to.
    Output,
    /// The directory the encoded scenes are read from.
    ScenesDirectory,
}

/// Settings `condor concatenate set` can change.
#[derive(Debug, Clone, Subcommand)]
pub enum ConcatenateSet {
    /// The tool used to join the encoded scenes.
    ///
    /// Keeping the current tool keeps its settings; switching to another tool
    /// starts from that tool's defaults.
    Method {
        #[arg(value_name = "Method")]
        method: ConcatenationMethod,
    },
    /// The file the concatenated video is written to.
    Output {
        #[arg(value_name = "Path")]
        path: PathBuf,
    },
    /// The directory the encoded scenes are read from.
    ScenesDirectory {
        #[arg(value_name = "Path")]
        path: PathBuf,
    },
}

/// The action triple attached to `condor concatenate`.
pub type ConcatenateAction = ConfigAction<ConcatenateKey, ConcatenateSet>;

/// The `condor concatenate` configuration section.
pub struct ConcatenateCrud;

impl ConfigCommand for ConcatenateCrud {
    type Key = ConcatenateKey;
    type Set = ConcatenateSet;

    fn subject() -> &'static str {
        "Concatenation settings"
    }

    fn render(configuration: &Configuration, key: Option<&Self::Key>) -> Result<Value> {
        let concat = &configuration.condor.sequence_config.scene_concatenator;
        Ok(match key {
            None => serde_json::to_value(concat)?,
            Some(ConcatenateKey::Method) => serde_json::to_value(&concat.method)?,
            Some(ConcatenateKey::Output) => serde_json::to_value(&concat.output)?,
            Some(ConcatenateKey::ScenesDirectory) => {
                serde_json::to_value(&concat.scenes_directory)?
            },
        })
    }

    fn apply(configuration: &mut Configuration, set: &Self::Set) -> Result<PropertyChange> {
        match set {
            ConcatenateSet::Method {
                method,
            } => {
                configure_concatenate(configuration, Some(method))?;
                PropertyChange::new(
                    "method",
                    &configuration.condor.sequence_config.scene_concatenator.method,
                )
            },
            ConcatenateSet::Output {
                path: output,
            } => {
                configuration.condor.sequence_config.scene_concatenator.output =
                    Some(output.clone());
                PropertyChange::new(
                    "output",
                    &configuration.condor.sequence_config.scene_concatenator.output,
                )
            },
            ConcatenateSet::ScenesDirectory {
                path: directory,
            } => {
                configuration.condor.sequence_config.scene_concatenator.scenes_directory =
                    directory.clone();
                PropertyChange::new(
                    "scenes-directory",
                    &configuration.condor.sequence_config.scene_concatenator.scenes_directory,
                )
            },
        }
    }

    fn restore(
        configuration: &mut Configuration,
        key: Option<&Self::Key>,
    ) -> Result<Option<PropertyChange>> {
        let defaults = SceneConcatenatorConfig::default();
        let concat = &mut configuration.condor.sequence_config.scene_concatenator;
        let (property, value) = match key {
            // `scenes_directory` is derived from --temp, so resetting the whole
            // section restores the method and output but keeps it in place.
            None => {
                concat.method = defaults.method;
                concat.output = None;
                ("section", serde_json::to_value(&*concat)?)
            },
            Some(ConcatenateKey::Method) => {
                concat.method = defaults.method;
                ("method", serde_json::to_value(&concat.method)?)
            },
            Some(ConcatenateKey::Output) => {
                concat.output = None;
                ("output", serde_json::to_value(&concat.output)?)
            },
            Some(ConcatenateKey::ScenesDirectory) => {
                concat.scenes_directory = defaults.scenes_directory;
                (
                    "scenes-directory",
                    serde_json::to_value(&concat.scenes_directory)?,
                )
            },
        };
        Ok(Some(PropertyChange {
            property: property.to_owned(),
            value,
        }))
    }
}

/// `condor concatenate info|set|reset`.
pub fn config(config_path: Option<&Path>, action: &ConcatenateAction) -> Result<()> {
    edit::<ConcatenateCrud>(config_path, action)
}

#[cfg(test)]
mod tests {
    use std::assert_matches;

    use andean_condor::models::sequence::scene_concatenator::{AudioTrack, TrackOptions};

    use super::*;
    use crate::{
        commands::handlers::init::init_handler,
        test_helpers::{check_basic_config, default_config, get_test_video},
        utils::hash_path::hash_path,
    };

    #[test]
    fn detect_scenes_default_config() {
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
        let (config, found_config_path) = concatenate_handler(
            // Simulate default directory to avoid changing CWD
            Some(&config_path),
            None,
            None,
            None,
        )
        .expect("concatenate_handler should succeed");

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
    fn concatenate_custom_config() {
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

        let mut expected_config = default_config(&test_video, &output, &temp_abs);
        expected_config.condor.sequence_config.scene_concatenator.method =
            ConcatMethod::FFmpeg(Default::default());
        expected_config.condor.sequence_config.scene_concatenator.scenes_directory =
            temp_abs.join("scenes");
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
        let (config, found_config_path) = concatenate_handler(
            // Simulate default directory to avoid changing CWD
            Some(&config_path),
            Some(&temp.path().join(hash_path(&input_abs))),
            Some(&ConcatenationMethod::FFmpeg),
            None,
        )
        .expect("concatanate_handler should succeed");

        assert_eq!(
            found_config_path,
            config_path_abs,
            "config path is {}",
            config_path_abs.display()
        );
        check_basic_config(&config, &expected_config);
        assert!(config.condor.scenes.is_empty(), "scenes is empty");
    }

    /// A source with one video, two audio, one subtitle, and one attachment,
    /// with mkvmerge IDs and (deliberately mismatched) FFmpeg stream indices —
    /// mkvmerge counts attachments in its own ID space.
    fn probed_source() -> ProbedSource {
        let track = |kind, mkvmerge_id, ffmpeg_index| crate::utils::track_probe::ProbedTrack {
            kind,
            mkvmerge_id: Some(mkvmerge_id),
            ffmpeg_index,
        };
        ProbedSource {
            tracks: vec![
                track(TrackKind::Video, 0, Some(0)),
                track(TrackKind::Audio, 1, Some(1)),
                track(TrackKind::Audio, 2, Some(2)),
                track(TrackKind::Subtitle, 3, Some(3)),
                track(TrackKind::Attachment, 1, Some(4)),
            ],
        }
    }

    #[test]
    fn discard_nothing_leaves_every_track_alone() {
        let source = probed_source();
        let mut mkvmerge = MkvmergeConfig::default();
        discard_mkvmerge(&mut mkvmerge, &source, &[]);
        assert!(mkvmerge.tracks.is_none(), "no map is created");
        assert!(
            mkvmerge.chapters.is_none(),
            "no chapters section is created"
        );
        assert!(
            mkvmerge.metadata.is_none(),
            "no metadata section is created"
        );

        let mut ffmpeg = FfmpegConfig::default();
        discard_ffmpeg(&mut ffmpeg, &source, &[]);
        assert!(ffmpeg.tracks.is_none(), "no map is created");
        assert!(ffmpeg.chapters.is_none(), "no chapters section is created");
        assert!(ffmpeg.metadata.is_none(), "no metadata section is created");
    }

    /// mkvmerge's map is an override map, so only dropped tracks are written.
    #[test]
    fn discard_audio_only_writes_the_audio_tracks_for_mkvmerge() {
        let source = probed_source();
        let mut config = MkvmergeConfig::default();
        discard_mkvmerge(&mut config, &source, &[DiscardCategory::Audio]);

        let tracks = config.tracks.as_ref().expect("a track map is created");
        assert_eq!(tracks.len(), 2, "only the two audio tracks are written");
        for id in [1, 2] {
            let track = tracks.get(&id).expect("audio track is present");
            assert!(!track.copy(), "mkvmerge track {id} is dropped");
            assert_matches!(track, MkvmergeTrack::Audio(_));
        }
        assert!(config.chapters.is_none(), "chapters are untouched");
    }

    /// An existing entry keeps the settings it already had.
    #[test]
    fn discard_audio_keeps_existing_track_settings_for_mkvmerge() {
        let source = probed_source();
        let mut config = MkvmergeConfig {
            tracks: Some(TrackMap::from([(
                1,
                MkvmergeTrack::Audio(AudioTrack {
                    track: TrackOptions {
                        copy: true,
                        name: Some("Commentary".to_owned()),
                        language: Some("eng".to_owned()),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
            )])),
            ..Default::default()
        };
        discard_mkvmerge(&mut config, &source, &[DiscardCategory::Audio]);

        let tracks = config.tracks.as_ref().expect("the track map is kept");
        let existing = tracks.get(&1).expect("the existing audio track is kept");
        assert!(!existing.copy(), "the existing track is dropped");
        let MkvmergeTrack::Audio(options) = existing else {
            panic!("the entry stays an audio track");
        };
        assert_eq!(options.track.name.as_deref(), Some("Commentary"));
        assert_eq!(options.track.language.as_deref(), Some("eng"));
        assert!(tracks.get(&2).is_some(), "the other audio track is added");
    }

    #[test]
    fn discard_video_targets_track_zero_for_mkvmerge() {
        let source = probed_source();
        let mut config = MkvmergeConfig::default();
        discard_mkvmerge(&mut config, &source, &[DiscardCategory::Video]);

        let tracks = config.tracks.as_ref().expect("a track map is created");
        assert_eq!(tracks.len(), 1, "only the video track is written");
        let video = tracks.get(&0).expect("the video track is ID 0");
        assert_matches!(video, MkvmergeTrack::Video(_));
        assert!(!video.copy(), "the video is dropped");
    }

    #[test]
    fn discard_chapters_and_metadata_sets_copy_false() {
        let source = probed_source();
        let mut mkvmerge = MkvmergeConfig::default();
        discard_mkvmerge(&mut mkvmerge, &source, &[
            DiscardCategory::Chapters,
            DiscardCategory::Metadata,
        ]);
        assert!(!mkvmerge.chapters.expect("chapters are created").copy);
        assert!(!mkvmerge.metadata.expect("metadata is created").copy);

        let mut ffmpeg = FfmpegConfig::default();
        discard_ffmpeg(&mut ffmpeg, &source, &[
            DiscardCategory::Chapters,
            DiscardCategory::Metadata,
        ]);
        assert!(!ffmpeg.chapters.expect("chapters are created").copy);
        assert!(!ffmpeg.metadata.expect("metadata is created").copy);
    }

    /// FFmpeg's map is a selection map, so dropping a stream from an unset map
    /// has to keep every other stream mapped.
    #[test]
    fn discard_audio_fills_the_ffmpeg_selection() {
        let source = probed_source();
        let mut config = FfmpegConfig::default();
        discard_ffmpeg(&mut config, &source, &[DiscardCategory::Audio]);

        let tracks = config.tracks.as_ref().expect("a track map is created");
        assert_eq!(tracks.len(), 4, "every source stream is listed");
        for index in [1, 2] {
            assert!(!tracks[&index].copy(), "stream {index} is dropped");
        }
        assert!(tracks[&3].copy(), "the subtitle is kept");
        assert!(tracks[&4].copy(), "the attachment is kept");
        assert!(!tracks.contains_key(&0), "the encoded video is not listed");
    }

    /// An existing selection is only patched with the stream the flag names.
    #[test]
    fn discard_subtitle_patches_an_existing_ffmpeg_selection() {
        let source = probed_source();
        let mut config = FfmpegConfig {
            tracks: Some(FfmpegTrackMap::from([
                (2, FfmpegTrack::plain(FfmpegTrackKind::Audio, true)),
                (3, FfmpegTrack::plain(FfmpegTrackKind::Subtitle, true)),
            ])),
            ..Default::default()
        };
        discard_ffmpeg(&mut config, &source, &[DiscardCategory::Subtitle]);

        let tracks = config.tracks.as_ref().expect("the track map is kept");
        assert_eq!(tracks.len(), 2, "no stream is added to the selection");
        assert!(tracks[&2].copy(), "the audio track is untouched");
        assert!(!tracks[&3].copy(), "the subtitle is dropped");
    }

    #[test]
    fn discard_video_targets_stream_zero_for_ffmpeg() {
        let source = probed_source();
        let mut config = FfmpegConfig::default();
        discard_ffmpeg(&mut config, &source, &[DiscardCategory::Video]);

        let tracks = config.tracks.as_ref().expect("a track map is created");
        assert_eq!(
            tracks.len(),
            5,
            "the source streams are listed so only the video is dropped: {tracks:?}"
        );
        assert_matches!(tracks[&0], FfmpegTrack::Video(_));
        assert!(!tracks[&0].copy(), "the video is dropped");
        for index in [1, 2, 3, 4] {
            assert!(tracks[&index].copy(), "stream {index} is kept");
        }
    }

    /// The IVF method has no track settings, and a VapourSynth script has no
    /// container to probe, so both are reported and skipped instead of failing.
    #[test]
    fn apply_discard_skips_the_ivf_method() {
        let test_video = get_test_video();
        let temp = tempfile::tempdir().expect("temp directory");
        let output = temp.path().join("out.mkv");
        let temp_abs = path_abs::PathAbs::new(temp.path())
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let mut config = default_config(&test_video, &output, &temp_abs);
        config.condor.sequence_config.scene_concatenator.method = ConcatMethod::Ivf;

        apply_discard(&mut config, &[DiscardCategory::Audio])
            .expect("applying --discard to the ivf method should succeed");

        assert!(
            matches!(
                config.condor.sequence_config.scene_concatenator.method,
                ConcatMethod::Ivf
            ),
            "the method is unchanged"
        );
    }
}
