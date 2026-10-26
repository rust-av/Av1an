//! The `condor scenes` command surface: the actions with their keys and
//! settings, plus the errors that belong to scene editing itself.

use std::path::PathBuf;

use clap::{Subcommand, value_parser};
use thiserror::Error;

use crate::commands::{EncoderMethod, config::Toggle};

/// `condor scenes [INDEX] <action>`.
#[derive(Debug, Clone, Subcommand)]
pub enum ScenesAction {
    /// Print the addressed scenes, or one of their saved settings, as JSON.
    Info {
        /// Which setting to print; omit for the scenes themselves.
        #[command(subcommand)]
        key: Option<ScenesKey>,
    },
    /// Change one setting of the addressed scenes.
    Set {
        /// Which setting to change.
        #[command(subcommand)]
        setting: ScenesSet,
    },
    /// Reset the addressed scenes, or the whole list, to what detection would
    /// have written.
    Reset {
        /// Which setting to reset; omit to reset the scenes themselves.
        #[command(subcommand)]
        key: Option<ScenesKey>,
    },
    /// Remove the addressed scenes from the list.
    Delete,
    /// Insert a new scene spanning START..END.
    #[command(hide = true)]
    Add {
        /// First frame of the new scene.
        #[arg(value_name = "START")]
        start:   usize,
        /// Frame the new scene ends at (exclusive).
        #[arg(value_name = "END")]
        end:     usize,
        /// Put the new scene at the end of the list.
        #[arg(long, conflicts_with_all = ["prepend", "after"])]
        append:  bool,
        /// Put the new scene at the start of the list.
        #[arg(long, conflicts_with_all = ["append", "after"])]
        prepend: bool,
        /// Put the new scene directly after scene N.
        #[arg(long, value_name = "N", conflicts_with_all = ["append", "prepend"])]
        after:   Option<usize>,
    },
}

/// The settings `info` and `reset` address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Subcommand)]
pub enum ScenesKey {
    /// The scenes' frame ranges.
    Frames,
    /// The scenes' encoders and their parameters.
    Encoder,
    /// The scenes' sub-scenes.
    SubScenes,
    /// Scene-detection results: the scores and when they were computed.
    SceneDetection,
    /// Noise-detection results.
    NoiseDetection,
    /// Noise-scaling results.
    NoiseScaling,
    /// Parallel-encoder progress: `started_on`, `completed_on` and `bytes`.
    ParallelEncoder,
    /// Target-quality passes.
    TargetQuality,
    /// Quality-check results.
    QualityCheck,
    /// The zoning record: whether the Zoner has processed the scene list.
    /// Reset it to make the configured zones apply again.
    Zoner,
}

/// The settings `set` can change, applied to every addressed scene.
#[derive(Debug, Clone, Subcommand)]
pub enum ScenesSet {
    /// Frame the scene starts at (inclusive).
    Start {
        /// The first frame of the scene.
        #[arg(value_name = "Frame")]
        frame: usize,
    },
    /// Frame the scene ends at (exclusive).
    End {
        /// The frame after the scene's last one.
        #[arg(value_name = "Frame")]
        frame: usize,
    },
    /// Switch the scene's encoder; switching starts from that encoder's
    /// defaults. The flags below are applied after the switch, so one line
    /// sets an encoder and its settings at once.
    Encoder {
        /// The encoder to switch to.
        #[arg(value_name = "Encoder")]
        encoder:           EncoderMethod,
        /// Custom encoder binary path.
        #[arg(long, value_name = "Path")]
        executable:        Option<PathBuf>,
        /// Number of encoder passes.
        #[arg(long, value_parser = value_parser!(u8).range(1..), value_name = "Passes")]
        passes:            Option<u8>,
        /// Encoder parameters (`--crf 16 --tune 0`), merged into the encoder.
        #[arg(long, value_name = "Parameters", allow_hyphen_values = true)]
        params:            Option<String>,
        /// Photon Noise luma ISO.
        #[arg(long, value_name = "ISO")]
        iso:               Option<u32>,
        /// Photon Noise chroma ISO (`--iso` is required as well).
        #[arg(long, value_name = "ISO")]
        chroma_iso:        Option<u32>,
        /// FFmpeg filtergraph applied to the frames before they reach the
        /// encoder.
        #[arg(long, value_name = "Filter", allow_hyphen_values = true)]
        ffmpeg_filter:     Option<String>,
        /// Append the source's colour and HDR metadata to the encoder
        /// parameters (`on`/`off`); the default when omitted is `on`.
        #[arg(long, value_name = "on|off")]
        append_color_data: Option<Toggle>,
    },
    /// Custom encoder binary path for the scene.
    Executable {
        /// Path to the encoder binary.
        #[arg(value_name = "Path")]
        path: PathBuf,
    },
    /// Number of encoder passes for the scene.
    Passes {
        /// How many passes the encoder makes.
        #[arg(value_parser = value_parser!(u8).range(1..), value_name = "Passes")]
        value: u8,
    },
    /// Encoder parameters (`--crf 16 --tune 0`), merged into the scene's
    /// encoder.
    Params {
        /// The parameters to merge in.
        #[arg(value_name = "Parameters", allow_hyphen_values = true)]
        params: String,
    },
    /// Photon Noise ISO for the scene (`--chroma-noise` sets the chroma ISO).
    PhotonNoise {
        /// Luma ISO strength.
        #[arg(value_name = "ISO")]
        iso:          u32,
        /// Optional chroma ISO strength.
        #[arg(long, value_name = "ISO")]
        chroma_noise: Option<u32>,
    },
    /// FFmpeg filtergraph applied to the scene's frames before they reach the
    /// encoder.
    FfmpegFilter {
        /// The filtergraph to apply.
        #[arg(value_name = "Filter", allow_hyphen_values = true)]
        filter: String,
    },
    /// Whether the source's colour and HDR metadata is appended to the scene's
    /// encoder parameters. Bare `on` is the default when the value is omitted.
    AppendColorData {
        #[arg(value_name = "on|off")]
        value: Option<Toggle>,
    },
    /// Replace the scene's sub-scenes with `START-END` frame ranges; `none`
    /// clears them.
    SubScenes {
        /// The sub-scene ranges, e.g. `240-480`.
        #[arg(value_name = "Range")]
        ranges: Vec<String>,
    },
}

/// Everything that can make a scene edit unusable beyond the scene-list rules
/// in [`crate::scenes::ScenesError`] or the encoder rules in
/// [`EncodeConfigError`](crate::commands::handlers::encode::EncodeConfigError).
#[derive(Debug, Error)]
pub enum ScenesConfigError {
    #[error(
        "`{0}` has no default; it comes from the step that produced it — set `start`/`end` to \
         change the frame range, or re-detect with `condor detect-scenes`"
    )]
    NoDefault(&'static str),
    #[error("pass at least one START-END range, or `none` to clear the sub-scenes")]
    SubScenesNeedRanges,
    #[error(
        "which scenes? address them with an index: `condor scenes 0 set start 23`, or `condor \
         scenes all set start 23`"
    )]
    SetNeedsIndex,
    #[error(
        "which scenes? address them with an index: `condor scenes 1 delete`, or `condor scenes \
         all delete`"
    )]
    DeleteNeedsIndex,
    #[error(
        "--chroma-iso needs --iso: a chroma noise level only exists inside a Photon Noise table"
    )]
    ChromaIsoNeedsIso,
}

/// Where `condor scenes add` puts its new scene.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Placement {
    /// Sorted by start frame, so the list keeps ascending starts (the default).
    Sorted,
    /// At the end of the list.
    Append,
    /// At the start of the list.
    Prepend,
    /// Directly after scene N.
    After(usize),
}
