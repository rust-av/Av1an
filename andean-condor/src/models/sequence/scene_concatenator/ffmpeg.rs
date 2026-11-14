use std::{collections::HashMap, path::PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::common::{Chapters, Metadata};

/// Stream overrides for one input file, keyed by the stream index in that file.
///
/// `None` copies every audio and subtitle stream (the default).
pub type FfmpegTrackMap = HashMap<u32, FfmpegTrack>;

/// FFmpeg output settings for the Scene Concatenator.
///
/// Setting `ffmpeg: {}` in the configuration adds the original input (and any
/// extra inputs) to the output; leaving it unset keeps the video-only
/// concatenation.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FfmpegConfig {
    /// Per-stream overrides for the main input. `None` copies all audio and
    /// subtitle streams.
    #[serde(default)]
    pub tracks: Option<FfmpegTrackMap>,
    /// Chapter handling. `None` copies the source's chapters.
    #[serde(default)]
    pub chapters: Option<Chapters>,
    /// Global metadata.
    #[serde(default)]
    pub metadata: Option<Metadata>,
    /// Additional files muxed alongside the encoded scenes.
    #[serde(default)]
    pub extra_inputs: Vec<FfmpegExtraInput>,
}

/// An additional source file muxed into the output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FfmpegExtraInput {
    pub path: PathBuf,
    /// Per-stream overrides for this file. `None` copies all its audio and
    /// subtitle streams.
    #[serde(default)]
    pub tracks: Option<FfmpegTrackMap>,
    /// Metadata handling for this file.
    #[serde(default)]
    pub metadata: Option<Metadata>,
}

/// A track type and its FFmpeg options.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum FfmpegTrack {
    Video(FfmpegTrackOptions),
    Audio(FfmpegTrackOptions),
    Subtitle(FfmpegTrackOptions),
    /// An attachment (FFmpeg's `t` stream type, mirrored from mkvmerge's
    /// `AttachmentTrack`).
    Attachment(FfmpegAttachmentTrack),
}

/// The kind of stream an [`FfmpegTrack`] describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfmpegTrackKind {
    Video,
    Audio,
    Subtitle,
    Attachment,
}

impl FfmpegTrack {
    /// Whether this track is copied to the output. Defaults to `true`.
    #[inline]
    #[must_use]
    pub fn copy(&self) -> bool {
        match self {
            Self::Video(options) | Self::Audio(options) | Self::Subtitle(options) => options.copy,
            Self::Attachment(attachment) => attachment.copy,
        }
    }

    /// Sets whether this track is copied to the output.
    #[inline]
    pub fn set_copy(&mut self, copy: bool) {
        match self {
            Self::Video(options) | Self::Audio(options) | Self::Subtitle(options) => {
                options.copy = copy;
            },
            Self::Attachment(attachment) => attachment.copy = copy,
        }
    }

    /// An entry for `kind` that only sets the `copy` flag.
    ///
    /// Used to select or drop a source stream without describing it any
    /// further.
    #[must_use]
    #[inline]
    pub fn plain(kind: FfmpegTrackKind, copy: bool) -> Self {
        let options = FfmpegTrackOptions {
            copy,
            codec: None,
            filter: None,
            parameters: HashMap::new(),
        };
        match kind {
            FfmpegTrackKind::Video => Self::Video(options),
            FfmpegTrackKind::Audio => Self::Audio(options),
            FfmpegTrackKind::Subtitle => Self::Subtitle(options),
            FfmpegTrackKind::Attachment => Self::Attachment(FfmpegAttachmentTrack {
                copy,
                path: None,
                attachment_name: None,
                attachment_description: None,
                attachment_mime_type: None,
            }),
        }
    }

    /// This track's codec/filter/parameter options, or `None` for an
    /// attachment (which carries no codec or filter).
    #[inline]
    #[must_use]
    pub fn options(&self) -> Option<&FfmpegTrackOptions> {
        match self {
            Self::Video(options) | Self::Audio(options) | Self::Subtitle(options) => Some(options),
            Self::Attachment(_) => None,
        }
    }

    /// This track's attachment parameters, or `None` for a non-attachment.
    #[inline]
    #[must_use]
    pub fn attachment(&self) -> Option<&FfmpegAttachmentTrack> {
        match self {
            Self::Attachment(attachment) => Some(attachment),
            _ => None,
        }
    }

    /// The FFmpeg stream type letter (`v`/`a`/`s`/`t`).
    #[inline]
    #[must_use]
    pub fn stream_type(&self) -> &'static str {
        match self {
            Self::Video(_) => "v",
            Self::Audio(_) => "a",
            Self::Subtitle(_) => "s",
            Self::Attachment(_) => "t",
        }
    }
}

/// An attachment's parameters.
///
/// A `path` attaches a new file to the output (using the `attachment_*` fields
/// as its name, description, and MIME type) via FFmpeg's `-attach`. Without a
/// `path` the entry only participates in copy selection; FFmpeg cannot rename
/// or re-describe an attachment copied from a source file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FfmpegAttachmentTrack {
    /// Whether to copy this attachment. Defaults to `true`.
    #[serde(default = "super::common::default_true")]
    pub copy: bool,
    /// A file to attach to the output.
    #[serde(default)]
    pub path: Option<PathBuf>,
    /// The attachment's file name in the output (`filename`).
    #[serde(default)]
    pub attachment_name: Option<String>,
    /// The attachment's description. FFmpeg attachments have no dedicated
    /// description element, so this is written as `comment` metadata.
    #[serde(default)]
    pub attachment_description: Option<String>,
    /// The attachment's MIME type (`mimetype`). Required when `path` is set.
    #[serde(default)]
    pub attachment_mime_type: Option<String>,
}

/// A single output stream's FFmpeg options.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FfmpegTrackOptions {
    /// Whether to copy this track. Defaults to `true`.
    #[serde(default = "super::common::default_true")]
    pub copy: bool,
    /// Output codec for this stream (`-c:a:N` and friends). `copy` keeps the
    /// source codec.
    #[serde(default)]
    pub codec: Option<String>,
    /// Filtergraph applied to this stream (`-filter:a:N` / `-filter:s:N`).
    ///
    /// Rejected for video tracks. Requires a non-`copy` codec, because a
    /// filtered stream has to be re-encoded.
    #[serde(default)]
    pub filter: Option<String>,
    /// Extra FFmpeg arguments, appended verbatim. Each key must start with
    /// `-` and must not override Condor's own arguments.
    #[serde(default)]
    pub parameters: HashMap<String, String>,
}

/// Options that would fight the arguments Condor builds itself.
///
/// Keys are compared with the leading dashes and case removed.
pub const FORBIDDEN_PARAMETERS: &[&str] = &[
    // Codec selection.
    "c",
    "codec",
    "acodec",
    "vcodec",
    "scodec",
    "c:a",
    "c:s",
    "c:v",
    "codec:a",
    "codec:s",
    "codec:v",
    // Stream mapping.
    "map",
    "map_metadata",
    "map_chapters",
    "map_channel",
    // Filtering.
    "af",
    "vf",
    "filter",
    "filter:a",
    "filter:v",
    "filter:s",
    "filter_complex",
    "filter_complex_script",
    // Input/output control.
    "i",
    "input",
    "y",
    "n",
    "f",
    "format",
    "safe",
    "stream_loop",
    // Stream selection.
    "vn",
    "an",
    "sn",
    "dn",
    "noaudio",
    "novideo",
    "nosubtitles",
    // Progress reporting.
    "abort_on",
    "progress",
    "stats",
    "stats_period",
];

/// Everything that stops the FFmpeg output settings from being used as
/// written.
#[derive(Debug, Error)]
pub enum FfmpegConfigError {
    #[error("FFmpeg has no chapter language/charset option; only the chapter copy flag is used")]
    ChapterLanguageIgnored,
    #[error(
        "FFmpeg extra input {path} sets metadata, which applies to the whole output; only the \
         main input's metadata is written",
        path = .path.display()
    )]
    ExtraInputMetadataIgnored { path: PathBuf },
    #[error("{input} {id}: an attachment with a path requires attachment_mime_type")]
    AttachmentMimeTypeMissing { input: String, id: u32 },
    #[error("{input} {id}: parameter key {key:?} must start with '-'")]
    ParameterKeyWithoutDash { input: String, id: u32, key: String },
    #[error(
        "{input} {id}: parameter \"{key}\" conflicts with an argument Condor sets; use the \
         codec/filter fields instead"
    )]
    ParameterKeyForbidden { input: String, id: u32, key: String },
    #[error("{input} {id}: parameter key {key:?} must not contain whitespace")]
    ParameterKeyWhitespace { input: String, id: u32, key: String },
    #[error("{input} {id}: filter must not be empty")]
    FilterEmpty { input: String, id: u32 },
    #[error(
        "{input} {id}: video tracks cannot be filtered; only audio and subtitle tracks support a \
         filter"
    )]
    VideoFilterUnsupported { input: String, id: u32 },
    #[error(
        "{input} {id}: a filter requires a non-copy codec, because a filtered stream must be \
         re-encoded"
    )]
    FilterRequiresReEncode { input: String, id: u32 },
}

impl FfmpegConfig {
    /// Validates the configuration.
    ///
    /// # Errors
    ///
    /// Fails when a parameter key cannot be passed safely, when a filter is
    /// applied to a video track, or when a filter would be applied to a copied
    /// (un-re-encoded) stream.
    #[inline]
    pub fn validate(&self) -> anyhow::Result<Vec<anyhow::Error>> {
        let mut warnings = Vec::new();
        if let Some(tracks) = &self.tracks {
            validate_tracks(tracks, "track")?;
        }
        if let Some(chapters) = &self.chapters
            && (chapters.language.is_some() || chapters.charset.is_some())
        {
            warnings.push(FfmpegConfigError::ChapterLanguageIgnored.into());
        }
        for extra in &self.extra_inputs {
            if let Some(tracks) = &extra.tracks {
                validate_tracks(tracks, &extra.path.display().to_string())?;
            }
            if extra.metadata.is_some() {
                warnings.push(
                    FfmpegConfigError::ExtraInputMetadataIgnored {
                        path: extra.path.clone(),
                    }
                    .into(),
                );
            }
        }
        Ok(warnings)
    }
}

fn validate_tracks(tracks: &FfmpegTrackMap, source: &str) -> anyhow::Result<()> {
    for (&id, track) in tracks {
        let input = || source.to_owned();
        // Attachments carry no codec, filter, or parameters; a new attachment
        // only needs a MIME type for FFmpeg to write it out.
        if let Some(attachment) = track.attachment() {
            if attachment.path.is_some() && attachment.attachment_mime_type.is_none() {
                return Err(
                    FfmpegConfigError::AttachmentMimeTypeMissing { input: input(), id }.into(),
                );
            }
            continue;
        }
        let Some(options) = track.options() else {
            continue;
        };
        for key in options.parameters.keys() {
            if !key.starts_with('-') {
                return Err(FfmpegConfigError::ParameterKeyWithoutDash {
                    input: input(),
                    id,
                    key: key.clone(),
                }
                .into());
            }
            let normalized = key.trim_start_matches('-').trim().to_ascii_lowercase();
            if FORBIDDEN_PARAMETERS.contains(&normalized.as_str()) {
                return Err(FfmpegConfigError::ParameterKeyForbidden {
                    input: input(),
                    id,
                    key: key.clone(),
                }
                .into());
            }
            if normalized.contains(char::is_whitespace) || normalized.contains('\n') {
                return Err(FfmpegConfigError::ParameterKeyWhitespace {
                    input: input(),
                    id,
                    key: key.clone(),
                }
                .into());
            }
        }
        if let Some(filter) = &options.filter {
            if filter.trim().is_empty() {
                return Err(FfmpegConfigError::FilterEmpty { input: input(), id }.into());
            }
            if matches!(track, FfmpegTrack::Video(_)) {
                return Err(
                    FfmpegConfigError::VideoFilterUnsupported { input: input(), id }.into(),
                );
            }
            let re_encodes = options
                .codec
                .as_deref()
                .is_some_and(|codec| !codec.eq_ignore_ascii_case("copy"));
            if !re_encodes {
                return Err(
                    FfmpegConfigError::FilterRequiresReEncode { input: input(), id }.into(),
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_from(json: &str) -> FfmpegConfig {
        serde_json::from_str(json).expect("config should deserialize")
    }

    /// Stream entries default to copying the stream.
    #[test]
    fn ffmpeg_track_json_defaults_to_copying() {
        let config = config_from(r#"{"tracks":{"1":{"type":"audio"}}}"#);
        let tracks = config.tracks.expect("tracks should be present");
        assert!(tracks[&1].copy());
    }

    /// A parameter that would fight Condor's own arguments is rejected.
    #[test]
    fn ffmpeg_track_parameters_reject_reserved_output_options() {
        let config = config_from(r#"{"tracks":{"1":{"type":"audio","parameters":{"-map":"0"}}}}"#);
        assert!(config.validate().is_err());
    }

    /// The leading dashes and case are ignored when matching forbidden keys.
    #[test]
    fn forbidden_parameter_matching_is_normalized() {
        let config =
            config_from(r#"{"tracks":{"1":{"type":"audio","parameters":{"-Codec:A":"aac"}}}}"#);
        assert!(config.validate().is_err());
    }

    /// Keys must be FFmpeg options, so a bare word is rejected.
    #[test]
    fn parameter_without_dash_is_rejected() {
        let config =
            config_from(r#"{"tracks":{"1":{"type":"audio","parameters":{"b:a":"128k"}}}}"#);
        assert!(config.validate().is_err());
    }

    /// A filter is accepted only on a re-encoded audio/subtitle stream; video
    /// tracks cannot be filtered at all.
    #[test]
    fn chapter_language_and_charset_only_warn() {
        let config =
            config_from(r#"{"chapters":{"copy":false,"language":"eng","charset":"UTF-8"}}"#);
        let warnings = config.validate().expect("only warnings are produced");
        assert_eq!(warnings.len(), 1, "one warning for the FFmpeg-only gap");
        assert!(!config.chapters.expect("chapters should be present").copy);
    }

    /// Video cannot be filtered, and filtering a copied stream is rejected.
    #[test]
    fn filter_only_applies_to_re_encoded_audio_or_subtitles() {
        let video =
            config_from(r#"{"tracks":{"0":{"type":"video","filter":"hflip","codec":"libx264"}}}"#);
        assert!(video.validate().is_err(), "video filtering is rejected");

        let copy_audio = config_from(r#"{"tracks":{"1":{"type":"audio","filter":"volume=0.5"}}}"#);
        assert!(
            copy_audio.validate().is_err(),
            "filtering a copied stream is rejected"
        );

        let filtered =
            config_from(r#"{"tracks":{"1":{"type":"audio","codec":"aac","filter":"volume=0.5"}}}"#);
        assert!(filtered.validate().is_ok(), "filtered audio is accepted");
    }
}
