use std::{collections::HashMap, path::PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::common::{Chapters, Metadata, default_true};

/// Video/audio/subtitle overrides, keyed by mkvmerge track ID (not attachment ID).
/// On the main input, video ID 0 describes the encoded scenes instead.
///
/// `None` copies every track (the default). A [`MkvmergeTrack`] modifies the
/// track it is keyed to, or drops it when its `copy` flag is `false`.
pub type TrackMap = HashMap<u32, MkvmergeTrack>;

/// Attachment overrides in mkvmerge's independent attachment ID namespace.
/// A value with `path` adds a file instead of selecting a source ID.
pub type AttachmentMap = HashMap<u32, AttachmentTrack>;

/// `mkvmerge` output settings for the Scene Concatenator.
#[derive(Debug, Clone, Default, PartialEq, Serialize, JsonSchema)]
pub struct MkvmergeConfig {
    /// Video/audio/subtitle overrides. Main video ID 0 describes encoded scenes.
    #[serde(default)]
    pub tracks: Option<TrackMap>,
    /// Attachment overrides, keyed separately from video/audio/subtitle IDs.
    #[serde(default)]
    pub attachments: Option<AttachmentMap>,
    /// Chapter handling. `None` copies the source's chapters.
    #[serde(default)]
    pub chapters: Option<Chapters>,
    /// Global metadata/tags. `None` copies the source's global tags.
    #[serde(default)]
    pub metadata: Option<Metadata>,
    /// Additional files merged alongside the encoded scenes.
    #[serde(default)]
    pub extra_inputs: Vec<MkvmergeExtraInput>,
}

/// An additional source file merged into the output.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct MkvmergeExtraInput {
    pub path: PathBuf,
    /// Video/audio/subtitle overrides using this file's actual source IDs.
    #[serde(default)]
    pub tracks: Option<TrackMap>,
    /// Attachment overrides in this file's attachment ID namespace.
    #[serde(default)]
    pub attachments: Option<AttachmentMap>,
    /// Chapter handling for this file. `None` copies its chapters.
    #[serde(default)]
    pub chapters: Option<Chapters>,
    /// Metadata handling for this file. `None` copies its global tags.
    #[serde(default)]
    pub metadata: Option<Metadata>,
}

/// A track type and the parameters that modify it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum MkvmergeTrack {
    /// Boxed because a video track carries far more parameters than the other
    /// variants.
    Video(Box<VideoTrack>),
    Audio(AudioTrack),
    Subtitle(SubtitleTrack),
}

/// The kind of track an [`MkvmergeTrack`] describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MkvmergeTrackKind {
    Video,
    Audio,
    Subtitle,
}

impl MkvmergeTrack {
    /// Whether this track is copied to the output. Defaults to `true`.
    #[inline]
    #[must_use]
    pub fn copy(&self) -> bool {
        match self {
            Self::Video(track) => track.track.copy,
            Self::Audio(track) => track.track.copy,
            Self::Subtitle(track) => track.track.copy,
        }
    }

    /// Sets whether this track is copied to the output.
    #[inline]
    pub fn set_copy(&mut self, copy: bool) {
        match self {
            Self::Video(track) => track.track.copy = copy,
            Self::Audio(track) => track.track.copy = copy,
            Self::Subtitle(track) => track.track.copy = copy,
        }
    }

    /// An entry for `kind` that only sets the `copy` flag.
    ///
    /// Used to select or drop a source track without describing it any
    /// further.
    #[must_use]
    #[inline]
    pub fn plain(kind: MkvmergeTrackKind, copy: bool) -> Self {
        match kind {
            MkvmergeTrackKind::Video => Self::Video(Box::new(VideoTrack {
                track: TrackOptions {
                    copy,
                    ..TrackOptions::default()
                },
                ..VideoTrack::default()
            })),
            MkvmergeTrackKind::Audio => Self::Audio(AudioTrack {
                track: TrackOptions {
                    copy,
                    ..TrackOptions::default()
                },
                ..AudioTrack::default()
            }),
            MkvmergeTrackKind::Subtitle => Self::Subtitle(SubtitleTrack {
                track: TrackOptions {
                    copy,
                    ..TrackOptions::default()
                },
                ..SubtitleTrack::default()
            }),
        }
    }
}

// Reader-only legacy shape. Never pass through serde_json::Value: that would
// discard duplicate IDs before migration can reject an ambiguous definition.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum ReadTrack {
    Video(Box<VideoTrack>),
    Audio(AudioTrack),
    Subtitle(SubtitleTrack),
    Attachment(AttachmentTrack),
}

struct IdMap<T>(HashMap<u32, T>);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for IdMap<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor<T>(std::marker::PhantomData<T>);
        impl<'de, T: Deserialize<'de>> serde::de::Visitor<'de> for Visitor<T> {
            type Value = IdMap<T>;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a map of unique mkvmerge track or attachment IDs")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let mut entries = HashMap::new();
                while let Some((key, value)) = map.next_entry::<String, T>()? {
                    let id = key.parse::<u32>().map_err(|error| {
                        serde::de::Error::custom(format!(
                            "invalid mkvmerge track/attachment ID {key:?}: {error}"
                        ))
                    })?;
                    if entries.insert(id, value).is_some() {
                        return Err(serde::de::Error::custom(format!(
                            "duplicate mkvmerge track/attachment ID {id}"
                        )));
                    }
                }
                Ok(IdMap(entries))
            }
        }
        deserializer.deserialize_map(Visitor(std::marker::PhantomData))
    }
}

#[derive(Default, Deserialize)]
struct ReadSettings {
    #[serde(default)]
    tracks: Option<IdMap<ReadTrack>>,
    #[serde(default)]
    attachments: Option<IdMap<AttachmentTrack>>,
    #[serde(default)]
    chapters: Option<Chapters>,
    #[serde(default)]
    metadata: Option<Metadata>,
}

impl ReadSettings {
    fn migrate<E: serde::de::Error>(
        &mut self,
        context: &str,
    ) -> Result<(Option<TrackMap>, Option<AttachmentMap>), E> {
        let mut attachments = self.attachments.take().map(|map| map.0);
        let tracks = self.tracks.take().map(|map| {
            let mut tracks = TrackMap::new();
            for (id, track) in map.0 {
                let track = match track {
                    ReadTrack::Video(track) => MkvmergeTrack::Video(track),
                    ReadTrack::Audio(track) => MkvmergeTrack::Audio(track),
                    ReadTrack::Subtitle(track) => MkvmergeTrack::Subtitle(track),
                    ReadTrack::Attachment(track) => {
                        if attachments.get_or_insert_with(AttachmentMap::new).insert(id, track).is_some() {
                            return Err(E::custom(format!("{context}: attachment ID {id} collision between legacy tracks and attachments; keep one definition in attachments")));
                        }
                        continue;
                    },
                };
                tracks.insert(id, track);
            }
            Ok(tracks)
        }).transpose()?;
        Ok((tracks, attachments))
    }
}

impl<'de> Deserialize<'de> for MkvmergeConfig {
    #[inline]
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Reader {
            #[serde(flatten)]
            settings: ReadSettings,
            #[serde(default)]
            extra_inputs: Vec<MkvmergeExtraInput>,
        }
        let mut reader = Reader::deserialize(deserializer)?;
        let (tracks, attachments) = reader.settings.migrate("main input")?;
        Ok(Self {
            tracks,
            attachments,
            chapters: reader.settings.chapters,
            metadata: reader.settings.metadata,
            extra_inputs: reader.extra_inputs,
        })
    }
}

impl<'de> Deserialize<'de> for MkvmergeExtraInput {
    #[inline]
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Reader {
            path: PathBuf,
            #[serde(flatten)]
            settings: ReadSettings,
        }
        let mut reader = Reader::deserialize(deserializer)?;
        let (tracks, attachments) =
            reader.settings.migrate(&format!("extra input {}", reader.path.display()))?;
        Ok(Self {
            path: reader.path,
            tracks,
            attachments,
            chapters: reader.settings.chapters,
            metadata: reader.settings.metadata,
        })
    }
}

/// Options shared by video, audio, and subtitle tracks.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TrackOptions {
    /// Whether to copy this track. Defaults to `true`.
    #[serde(default = "default_true")]
    pub copy: bool,
    /// Track name (`--track-name`).
    #[serde(default)]
    pub name: Option<String>,
    /// ISO 639-2 language code (`--language`).
    #[serde(default)]
    pub language: Option<String>,
    /// Delay in milliseconds (`--sync`).
    #[serde(default)]
    pub delay: Option<i64>,
    /// Default-track flag (`--default-track-flag`).
    #[serde(default)]
    pub default_track: Option<bool>,
    /// Forced-display flag (`--forced-display-flag`).
    #[serde(default)]
    pub forced: Option<bool>,
    /// Track-enabled flag (`--track-enabled-flag`).
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Hearing-impaired flag (`--hearing-impaired-flag`).
    #[serde(default)]
    pub hearing_impaired: Option<bool>,
    /// Visual-impaired flag (`--visual-impaired-flag`).
    #[serde(default)]
    pub visual_impaired: Option<bool>,
    /// Text-descriptions flag (`--text-descriptions-flag`).
    #[serde(default)]
    pub text_descriptions: Option<bool>,
    /// Original-language flag (`--original-flag`).
    #[serde(default)]
    pub original: Option<bool>,
    /// Commentary flag (`--commentary-flag`).
    #[serde(default)]
    pub commentary: Option<bool>,
}

impl TrackOptions {
    /// mkvmerge arguments shared by track types, addressed to `id`.
    #[must_use]
    fn common_args(&self, id: u32) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(name) = &self.name {
            args.push("--track-name".to_owned());
            args.push(format!("{id}:{name}"));
        }
        if let Some(language) = &self.language {
            args.push("--language".to_owned());
            args.push(format!("{id}:{language}"));
        }
        if let Some(delay) = self.delay {
            args.push("--sync".to_owned());
            args.push(format!("{id}:{delay}"));
        }
        let flags = [
            ("--default-track-flag", self.default_track),
            ("--forced-display-flag", self.forced),
            ("--track-enabled-flag", self.enabled),
            ("--hearing-impaired-flag", self.hearing_impaired),
            ("--visual-impaired-flag", self.visual_impaired),
            ("--text-descriptions-flag", self.text_descriptions),
            ("--original-flag", self.original),
            ("--commentary-flag", self.commentary),
        ];
        for (option, value) in flags {
            if let Some(value) = value {
                args.push(option.to_owned());
                args.push(format!("{id}:{}", u8::from(value)));
            }
        }
        args
    }
}

/// Pixel cropping parameters, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Crop {
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

/// Display dimensions, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DisplayDimensions {
    pub width: u32,
    pub height: u32,
}

/// Horizontal/vertical chroma subsampling.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ChromaSubsample {
    pub horizontal: u32,
    pub vertical: u32,
}

/// Horizontal/vertical chroma siting.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ChromaSiting {
    pub horizontal: u32,
    pub vertical: u32,
}

/// CIE 1931 red/green/blue chromaticity coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ChromaticityCoordinates {
    pub red_x: f32,
    pub red_y: f32,
    pub green_x: f32,
    pub green_y: f32,
    pub blue_x: f32,
    pub blue_y: f32,
}

/// CIE 1931 white-point coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WhiteColorCoordinates {
    pub x: f32,
    pub y: f32,
}

/// A video track's parameters (`--cropping`, `--aspect-ratio`, color/HDR
/// metadata, ...).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct VideoTrack {
    #[serde(flatten)]
    pub track: TrackOptions,
    /// `--cropping`.
    #[serde(default)]
    pub crop: Option<Crop>,
    /// `--display-dimensions`.
    #[serde(default)]
    pub display_dimensions: Option<DisplayDimensions>,
    /// `--aspect-ratio` (a ratio, e.g. `16/9`). Mutually exclusive with
    /// `display_dimensions` and `aspect_ratio_factor`.
    #[serde(default)]
    pub aspect_ratio: Option<String>,
    /// `--aspect-ratio-factor`.
    #[serde(default)]
    pub aspect_ratio_factor: Option<String>,
    /// `--color-primaries`.
    #[serde(default)]
    pub color_primaries: Option<u32>,
    /// `--color-transfer-characteristics`.
    #[serde(default)]
    pub transfer_characteristics: Option<u32>,
    /// `--color-matrix-coefficients`.
    #[serde(default)]
    pub matrix_coefficients: Option<u32>,
    /// `--color-range`.
    #[serde(default)]
    pub color_range: Option<u32>,
    /// `--color-bits-per-channel`.
    #[serde(default)]
    pub color_bits_per_channel: Option<u32>,
    /// `--chroma-subsample`.
    #[serde(default)]
    pub chroma_subsample: Option<ChromaSubsample>,
    /// `--chroma-siting`.
    #[serde(default)]
    pub chroma_siting: Option<ChromaSiting>,
    /// `--max-content-light` (MaxCLL).
    #[serde(default)]
    pub max_content_light: Option<u32>,
    /// `--max-frame-light` (MaxFALL).
    #[serde(default)]
    pub max_frame_light: Option<u32>,
    /// `--max-luminance`.
    #[serde(default)]
    pub max_luminance: Option<f32>,
    /// `--min-luminance`.
    #[serde(default)]
    pub min_luminance: Option<f32>,
    /// `--chromaticity-coordinates`.
    #[serde(default)]
    pub chromaticity_coordinates: Option<ChromaticityCoordinates>,
    /// `--white-color-coordinates`.
    #[serde(default)]
    pub white_color_coordinates: Option<WhiteColorCoordinates>,
    /// `--stereo-mode`.
    #[serde(default)]
    pub stereo_mode: Option<String>,
    /// `--field-order`.
    #[serde(default)]
    pub field_order: Option<u32>,
}

impl VideoTrack {
    /// mkvmerge arguments for this track, addressed to `id`.
    #[must_use]
    #[inline]
    pub fn args(&self, id: u32) -> Vec<String> {
        let mut args = self.track.common_args(id);
        if let Some(crop) = self.crop {
            args.push("--cropping".to_owned());
            args.push(format!(
                "{id}:{},{},{},{}",
                crop.left, crop.top, crop.right, crop.bottom
            ));
        }
        if let Some(dimensions) = self.display_dimensions {
            args.push("--display-dimensions".to_owned());
            args.push(format!("{id}:{}x{}", dimensions.width, dimensions.height));
        }
        if let Some(ratio) = &self.aspect_ratio {
            args.push("--aspect-ratio".to_owned());
            args.push(format!("{id}:{ratio}"));
        }
        if let Some(factor) = &self.aspect_ratio_factor {
            args.push("--aspect-ratio-factor".to_owned());
            args.push(format!("{id}:{factor}"));
        }
        let color = [
            ("--color-primaries", self.color_primaries.map(u64::from)),
            (
                "--color-transfer-characteristics",
                self.transfer_characteristics.map(u64::from),
            ),
            (
                "--color-matrix-coefficients",
                self.matrix_coefficients.map(u64::from),
            ),
            ("--color-range", self.color_range.map(u64::from)),
            (
                "--color-bits-per-channel",
                self.color_bits_per_channel.map(u64::from),
            ),
            ("--max-content-light", self.max_content_light.map(u64::from)),
            ("--max-frame-light", self.max_frame_light.map(u64::from)),
        ];
        for (option, value) in color {
            if let Some(value) = value {
                args.push(option.to_owned());
                args.push(format!("{id}:{value}"));
            }
        }
        if let Some(subsample) = self.chroma_subsample {
            args.push("--chroma-subsample".to_owned());
            args.push(format!(
                "{id}:{},{}",
                subsample.horizontal, subsample.vertical
            ));
        }
        if let Some(siting) = self.chroma_siting {
            args.push("--chroma-siting".to_owned());
            args.push(format!("{id}:{},{}", siting.horizontal, siting.vertical));
        }
        if let Some(luminance) = self.max_luminance {
            args.push("--max-luminance".to_owned());
            args.push(format!("{id}:{luminance}"));
        }
        if let Some(luminance) = self.min_luminance {
            args.push("--min-luminance".to_owned());
            args.push(format!("{id}:{luminance}"));
        }
        if let Some(c) = self.chromaticity_coordinates {
            args.push("--chromaticity-coordinates".to_owned());
            args.push(format!(
                "{id}:{},{},{},{},{},{}",
                c.red_x, c.red_y, c.green_x, c.green_y, c.blue_x, c.blue_y
            ));
        }
        if let Some(w) = self.white_color_coordinates {
            args.push("--white-color-coordinates".to_owned());
            args.push(format!("{id}:{},{}", w.x, w.y));
        }
        if let Some(mode) = &self.stereo_mode {
            args.push("--stereo-mode".to_owned());
            args.push(format!("{id}:{mode}"));
        }
        if let Some(order) = self.field_order {
            args.push("--field-order".to_owned());
            args.push(format!("{id}:{order}"));
        }
        args
    }
}

/// An audio track's parameters.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AudioTrack {
    #[serde(flatten)]
    pub track: TrackOptions,
    /// `--aac-is-sbr`.
    #[serde(default)]
    pub aac_is_sbr: Option<bool>,
    /// `--reduce-to-core`.
    #[serde(default)]
    pub reduce_to_core: Option<bool>,
}

impl AudioTrack {
    /// mkvmerge arguments for this track, addressed to `id`.
    #[must_use]
    #[inline]
    pub fn args(&self, id: u32) -> Vec<String> {
        let mut args = self.track.common_args(id);
        if let Some(value) = self.aac_is_sbr {
            args.push("--aac-is-sbr".to_owned());
            args.push(format!("{id}:{}", u8::from(value)));
        }
        if self.reduce_to_core == Some(true) {
            args.push("--reduce-to-core".to_owned());
            args.push(id.to_string());
        }
        args
    }
}

/// Subtitle compression method (`--compression`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Compression {
    None,
    Zlib,
}

/// A subtitle track's parameters.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SubtitleTrack {
    #[serde(flatten)]
    pub track: TrackOptions,
    /// `--compression`.
    #[serde(default)]
    pub compression: Option<Compression>,
    /// `--sub-charset`.
    #[serde(default)]
    pub charset: Option<String>,
}

impl SubtitleTrack {
    /// mkvmerge arguments for this track, addressed to `id`.
    #[must_use]
    #[inline]
    pub fn args(&self, id: u32) -> Vec<String> {
        let mut args = self.track.common_args(id);
        if let Some(compression) = self.compression {
            let value = match compression {
                Compression::None => "none",
                Compression::Zlib => "zlib",
            };
            args.push("--compression".to_owned());
            args.push(format!("{id}:{value}"));
        }
        if let Some(charset) = &self.charset {
            args.push("--sub-charset".to_owned());
            args.push(format!("{id}:{charset}"));
        }
        args
    }
}

/// An attachment's parameters.
///
/// A `path` attaches a new file to the output (using the `attachment_*` fields
/// as its name, description, and MIME type). Without a `path` the entry only
/// participates in copy selection; mkvmerge cannot rename or re-describe an
/// attachment that is copied from a source file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AttachmentTrack {
    /// Whether to copy this attachment. Defaults to `true`.
    #[serde(default = "default_true")]
    pub copy: bool,
    /// A file to attach to the output.
    #[serde(default)]
    pub path: Option<PathBuf>,
    /// `--attachment-name`.
    #[serde(default)]
    pub attachment_name: Option<String>,
    /// `--attachment-description`.
    #[serde(default)]
    pub attachment_description: Option<String>,
    /// `--attachment-mime-type`.
    #[serde(default)]
    pub attachment_mime_type: Option<String>,
}

impl MkvmergeConfig {
    /// File-selection and track-modification arguments for the main input.
    ///
    /// Only the arguments that belong to the source file are returned; the
    /// encoded video track is handled by [`MkvmergeConfig::video_args`].
    #[must_use]
    #[inline]
    pub fn input_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        push_attachment_exclusions(&mut args, self.attachments.as_ref());
        let Some(tracks) = &self.tracks else {
            return args;
        };
        // The input's video is never copied — the encoded scenes replace it and
        // `--no-video` is applied to this source — so a `Video` entry (which
        // describes the *output* video track) must not emit a selection option
        // here. Only audio/subtitle selection applies to this track namespace.
        let (_, audio, subtitle) = Self::exclusions(tracks);
        push_exclusion(&mut args, "--audio-tracks", &audio);
        push_exclusion(&mut args, "--subtitle-tracks", &subtitle);

        for (id, track) in sorted_tracks(tracks) {
            if !track.copy() {
                continue;
            }
            match track {
                MkvmergeTrack::Video(_) => (),
                MkvmergeTrack::Audio(audio) => args.extend(audio.args(id)),
                MkvmergeTrack::Subtitle(subtitle) => args.extend(subtitle.args(id)),
            }
        }
        args
    }

    /// Track-modification arguments for the encoded video track.
    ///
    /// The video comes from the concatenated scene files, which mkvmerge sees
    /// as a single logical source whose only track is `0`.
    #[must_use]
    #[inline]
    pub fn video_args(&self) -> Vec<String> {
        let Some(tracks) = &self.tracks else {
            return Vec::new();
        };
        let mut args = Vec::new();
        for (_, track) in sorted_tracks(tracks) {
            if let MkvmergeTrack::Video(video) = track {
                if video.track.copy {
                    args.extend(video.args(0));
                } else {
                    args.push("--no-video".to_owned());
                }
            }
        }
        args
    }

    /// Arguments attaching new files to the output.
    #[must_use]
    #[inline]
    pub fn attachment_additions(&self) -> Vec<String> {
        let Some(attachments) = &self.attachments else {
            return Vec::new();
        };
        let mut args = Vec::new();
        for (_, attachment) in sorted_attachments(attachments) {
            args.extend(attachment.args());
        }
        args
    }

    /// Track IDs excluded because their entry sets `copy` to `false`.
    fn exclusions(tracks: &TrackMap) -> (Vec<u32>, Vec<u32>, Vec<u32>) {
        let mut video = Vec::new();
        let mut audio = Vec::new();
        let mut subtitle = Vec::new();
        for (id, track) in tracks {
            if track.copy() {
                continue;
            }
            match track {
                MkvmergeTrack::Video(_) => video.push(*id),
                MkvmergeTrack::Audio(_) => audio.push(*id),
                MkvmergeTrack::Subtitle(_) => subtitle.push(*id),
            }
        }
        video.sort_unstable();
        audio.sort_unstable();
        subtitle.sort_unstable();
        (video, audio, subtitle)
    }

    /// Validates the configuration, returning non-fatal warnings.
    ///
    /// # Errors
    ///
    /// Fails when the configuration describes something mkvmerge cannot honor.
    #[inline]
    pub fn validate(&self) -> anyhow::Result<Vec<anyhow::Error>> {
        let mut warnings = Vec::new();
        if let Some(tracks) = &self.tracks {
            let video_entries = tracks
                .iter()
                .filter(|(_, track)| matches!(track, MkvmergeTrack::Video(_)))
                .count();
            if video_entries > 1 {
                return Err(MkvmergeConfigError::MultipleVideoEntries.into());
            }
            for (id, track) in tracks {
                match track {
                    MkvmergeTrack::Video(_) if *id != 0 => {
                        return Err(MkvmergeConfigError::VideoTrackNotZero { id: *id }.into());
                    },
                    _ => (),
                }
            }
        }
        for attachments in self
            .attachments
            .iter()
            .chain(self.extra_inputs.iter().filter_map(|extra| extra.attachments.as_ref()))
        {
            for (id, attachment) in attachments {
                if attachment.path.is_none()
                    && (attachment.attachment_name.is_some()
                        || attachment.attachment_description.is_some()
                        || attachment.attachment_mime_type.is_some())
                {
                    warnings.push(MkvmergeConfigError::AttachmentFieldsIgnored { id: *id }.into());
                }
            }
        }
        for (index, extra) in self.extra_inputs.iter().enumerate() {
            if extra
                .chapters
                .as_ref()
                .is_some_and(|chapters| chapters.language.is_some() || chapters.charset.is_some())
            {
                warnings.push(MkvmergeConfigError::ExtraInputChaptersGlobal { index }.into());
            }
            if extra
                .metadata
                .as_ref()
                .is_some_and(|metadata| metadata.title.is_some() || !metadata.tags.is_empty())
            {
                warnings.push(MkvmergeConfigError::ExtraInputMetadataGlobal { index }.into());
            }
        }
        Ok(warnings)
    }
}

/// Everything that stops the mkvmerge output settings from being used as
/// written.
#[derive(Debug, Error)]
pub enum MkvmergeConfigError {
    #[error("only one video track entry is supported (the encoded video is a single track)")]
    MultipleVideoEntries,
    #[error("the video track entry must use track ID 0 (the encoded video), found {id}")]
    VideoTrackNotZero { id: u32 },
    #[error(
        "attachment track {id} sets a name/description/MIME type without a path; mkvmerge cannot \
         modify an attachment copied from a source, so these fields are ignored"
    )]
    AttachmentFieldsIgnored { id: u32 },
    #[error(
        "extra input {index} sets a chapter language/charset, which mkvmerge applies globally; \
         set them on the main input instead"
    )]
    ExtraInputChaptersGlobal { index: usize },
    #[error(
        "extra input {index} sets a title or tags, which apply to the whole output; only the main \
         input's metadata is written"
    )]
    ExtraInputMetadataGlobal { index: usize },
}

impl MkvmergeExtraInput {
    /// File-selection and track-modification arguments for this source file.
    #[must_use]
    #[inline]
    pub fn input_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(attachments) = &self.attachments {
            for (_, attachment) in sorted_attachments(attachments) {
                args.extend(attachment.args());
            }
        }
        push_attachment_exclusions(&mut args, self.attachments.as_ref());
        let Some(tracks) = &self.tracks else {
            return args;
        };
        let (video, audio, subtitle) = MkvmergeConfig::exclusions(tracks);
        push_exclusion(&mut args, "--video-tracks", &video);
        push_exclusion(&mut args, "--audio-tracks", &audio);
        push_exclusion(&mut args, "--subtitle-tracks", &subtitle);
        for (id, track) in sorted_tracks(tracks) {
            if !track.copy() {
                continue;
            }
            match track {
                MkvmergeTrack::Video(video) => args.extend(video.args(id)),
                MkvmergeTrack::Audio(audio) => args.extend(audio.args(id)),
                MkvmergeTrack::Subtitle(subtitle) => args.extend(subtitle.args(id)),
            }
        }
        args
    }
}

impl AttachmentTrack {
    /// Attachment arguments, emitted before the sources.
    #[must_use]
    fn args(&self) -> Vec<String> {
        if !self.copy {
            return Vec::new();
        }
        let Some(path) = &self.path else {
            return Vec::new();
        };
        let mut args = Vec::new();
        if let Some(name) = &self.attachment_name {
            args.push("--attachment-name".to_owned());
            args.push(name.clone());
        }
        if let Some(description) = &self.attachment_description {
            args.push("--attachment-description".to_owned());
            args.push(description.clone());
        }
        if let Some(mime_type) = &self.attachment_mime_type {
            args.push("--attachment-mime-type".to_owned());
            args.push(mime_type.clone());
        }
        args.push("--attach-file".to_owned());
        args.push(path.display().to_string());
        args
    }
}

fn sorted_attachments(attachments: &AttachmentMap) -> Vec<(u32, &AttachmentTrack)> {
    let mut entries: Vec<_> = attachments.iter().map(|(id, track)| (*id, track)).collect();
    entries.sort_unstable_by_key(|(id, _)| *id);
    entries
}

fn push_attachment_exclusions(args: &mut Vec<String>, attachments: Option<&AttachmentMap>) {
    let Some(attachments) = attachments else {
        return;
    };
    // New files do not identify a source attachment, even when copying is disabled.
    let ids: Vec<_> = sorted_attachments(attachments)
        .into_iter()
        .filter_map(|(id, track)| (!track.copy && track.path.is_none()).then_some(id))
        .collect();
    push_exclusion(args, "--attachments", &ids);
}

/// Sorts a track map into a deterministic ID order.
fn sorted_tracks(tracks: &TrackMap) -> Vec<(u32, &MkvmergeTrack)> {
    let mut ids = tracks.keys().copied().collect::<Vec<_>>();
    ids.sort_unstable();
    ids.into_iter()
        .filter_map(|id| tracks.get(&id).map(|track| (id, track)))
        .collect()
}

fn push_exclusion(args: &mut Vec<String>, option: &str, ids: &[u32]) {
    if ids.is_empty() {
        return;
    }
    args.push(option.to_owned());
    args.push(format!(
        "!{}",
        ids.iter().map(u32::to_string).collect::<Vec<_>>().join(",")
    ));
}

/// Renders global metadata as a Matroska tags XML document.
///
/// mkvmerge reads global tags from an XML file; this builds one containing
/// `title` (when set) and every entry in `tags`.
#[must_use]
#[inline]
pub fn tags_xml(metadata: &Metadata) -> String {
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE Tags SYSTEM \
         \"matroskatags.dtd\">\n<Tags>\n  <Tag>\n    <Targets/>\n",
    );
    let mut simple = |name: &str, value: &str| {
        xml.push_str("    <Simple>\n      <Name>");
        xml.push_str(&xml_escape(name));
        xml.push_str("</Name>\n      <String>");
        xml.push_str(&xml_escape(value));
        xml.push_str("</String>\n    </Simple>\n");
    };
    if let Some(title) = &metadata.title {
        simple("TITLE", title);
    }
    for (name, value) in &metadata.tags {
        simple(name, value);
    }
    xml.push_str("  </Tag>\n</Tags>\n");
    xml
}

fn xml_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    mod serialization;
    use super::*;

    fn config_from(json: &str) -> MkvmergeConfig {
        serde_json::from_str(json).expect("config should deserialize")
    }

    /// Track entries default to copying the track.
    #[test]
    fn copy_defaults_to_true() {
        let config = config_from(r#"{"tracks":{"1":{"type":"audio"}}}"#);
        let tracks = config.tracks.expect("tracks should be present");
        assert!(tracks[&1].copy(), "a track is copied unless it opts out");
    }

    /// The configured fields survive a round trip so saving a config does not
    /// silently drop them.
    #[test]
    fn track_options_round_trip() {
        let config = config_from(
            r#"{
                "tracks": {
                    "0": {"type":"video","language":"eng","crop":{"left":1,"top":2,"right":3,"bottom":4}},
                    "1": {"type":"audio","copy":false,"delay":250},
                    "2": {"type":"subtitle","compression":"zlib"}
                }
            }"#,
        );
        let written = serde_json::to_string(&config).expect("config should serialize");
        let reread: MkvmergeConfig =
            serde_json::from_str(&written).expect("written config should reload");
        let tracks = reread.tracks.expect("tracks should be present");
        assert_eq!(tracks.len(), 3);
        assert!(!tracks[&1].copy(), "an explicit false must survive");
    }

    /// Dropped tracks are turned into mkvmerge's reversed track list.
    #[test]
    fn exclusion_lists_use_the_reversed_form() {
        let config = config_from(
            r#"{"tracks":{"0":{"type":"audio","copy":false},"2":{"type":"subtitle","copy":false}}}"#,
        );
        let args = config.input_args();
        assert!(args.windows(2).any(|pair| pair == ["--audio-tracks", "!0"]));
        assert!(args.windows(2).any(|pair| pair == ["--subtitle-tracks", "!2"]));
    }

    /// The encoded video track is addressed as track 0 of the scene group.
    #[test]
    fn video_args_target_track_zero() {
        let config = config_from(
            r#"{"tracks":{"0":{"type":"video","color_primaries":9,"max_content_light":1000}}}"#,
        );
        let args = config.video_args();
        assert!(args.windows(2).any(|pair| pair == ["--color-primaries", "0:9"]));
        assert!(args.windows(2).any(|pair| pair == ["--max-content-light", "0:1000"]));
    }

    /// Only ID 0 is a meaningful video entry; anything else is user error.
    #[test]
    fn video_entry_must_use_track_zero() {
        let config = config_from(r#"{"tracks":{"3":{"type":"video"}}}"#);
        assert!(config.validate().is_err());
    }

    /// Dropping the video must not emit an input selection option that would
    /// re-enable video from the source; the video is dropped from the scene
    /// group instead.
    #[test]
    fn dropping_the_video_is_a_group_option_not_an_input_selection() {
        let config = config_from(r#"{"tracks":{"0":{"type":"video","copy":false}}}"#);
        assert!(
            !config.input_args().iter().any(|arg| arg == "--video-tracks"),
            "the input video is never copied, so no video selection option is emitted"
        );
        assert_eq!(config.video_args(), vec!["--no-video"]);
    }

    /// Global tags are written as escaped Matroska XML.
    #[test]
    fn tags_xml_escapes_values() {
        let mut metadata = Metadata {
            copy: true,
            title: Some("A & B".to_owned()),
            tags: HashMap::new(),
        };
        metadata.tags.insert("BPS".to_owned(), "<1000>".to_owned());
        let xml = tags_xml(&metadata);
        assert!(xml.contains("<Name>TITLE</Name>"));
        assert!(xml.contains("A &amp; B"));
        assert!(xml.contains("&lt;1000&gt;"));
    }
}
