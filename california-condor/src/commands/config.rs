//! Shared plumbing for the `info` / `set` / `reset` configuration subcommands.
//!
//! Every Sequence command attaches a [`ConfigAction`] to its clap definition,
//! so its settings can be read, changed and reset without running it:
//!
//! ```text
//! > av1an detect-scenes info
//! > av1an detect-scenes set method avscenechange --minimum-frames 20 --maximum-frames 280 --save-scores
//! > av1an detect-scenes reset method
//! ```
//!
//! `K` lists the properties `info` and `reset` accept, `S` the properties `set`
//! accepts together with their values and flags. Both are subcommand enums, so
//! every level of `--help` shows exactly what can be changed and how. The
//! per-command `K` and `S` enums live next to the handler that applies them
//! (see [`crate::commands::handlers`]); this module keeps only the pieces every
//! command shares.
//!
//! Each command's section is described by one [`ConfigCommand`] implementation,
//! so the `info` / `set` / `reset` driver itself is written once.

use std::path::PathBuf;

use andean_condor::{
    models::sequence::target_quality::types::{
        ProbeStatistic, ProbeStrategy, SubsetProbeLength, SubsetProbePosition,
    },
    vapoursynth::vapoursynth_filters::VapourSynthFilter,
};
use clap::{Subcommand, ValueEnum};
use serde_json::Value;
use thiserror::Error;

use crate::{commands::DecoderMethod, configuration::Configuration};

/// The `info` / `set` / `reset` triple every Sequence command exposes over one
/// configuration section.
///
/// `K` keys the read and reset operations, `S` carries the values and flags of
/// the write operation.
#[derive(Debug, Subcommand)]
pub enum ConfigAction<K, S>
where
    K: Subcommand + Clone,
    S: Subcommand + Clone,
{
    /// Print the saved settings as JSON.
    Info {
        /// Which setting to print; omit for the whole section.
        #[command(subcommand)]
        key: Option<K>,
    },
    /// Change a saved setting.
    Set {
        /// Which setting to change.
        #[command(subcommand)]
        setting: S,
    },
    /// Reset a saved setting, or the whole section, to its default.
    Reset {
        /// Which setting to reset; omit for the whole section.
        #[command(subcommand)]
        key: Option<K>,
    },
}

/// A yes/no choice for settings that are a plain switch.
///
/// Written as a value so a setting can always be turned in both directions
/// without a second, negating flag: `on` (or its aliases) switches it on, `off`
/// (or its aliases) switches it off. A flag that carries this value may also
/// accept no value at all, which means `on`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Toggle {
    /// Turn the setting on.
    #[value(
        name = "on",
        alias = "enable",
        alias = "enabled",
        alias = "yes",
        alias = "true",
        alias = "1"
    )]
    On,
    /// Turn the setting off.
    #[value(
        name = "off",
        alias = "disable",
        alias = "disabled",
        alias = "no",
        alias = "false",
        alias = "0"
    )]
    Off,
}

impl Toggle {
    /// Whether this choice means "on".
    #[inline]
    pub fn enabled(self) -> bool {
        matches!(self, Toggle::On)
    }
}

impl From<bool> for Toggle {
    #[inline]
    fn from(value: bool) -> Self {
        if value { Toggle::On } else { Toggle::Off }
    }
}

/// Flags shared by every `set input` property: they build an input override
/// exactly the way the run flags of the owning command do.
#[derive(Debug, Clone, clap::Args)]
pub struct InputOverride {
    /// Path to a video or a VapourSynth script.
    #[arg(long, short('i'), value_name = "Path")]
    pub path: Option<PathBuf>,
    /// Decoding plugin for a video input.
    #[arg(long, value_name = "Decoder")]
    pub decoder: Option<DecoderMethod>,
    /// Conversion applied to the decoded frames.
    #[arg(long, value_name = "Filter")]
    pub filters: Option<Vec<VapourSynthFilter>>,
    /// VapourSynth script arguments (`key=value`); scripts only.
    #[arg(long, value_name = "Arguments")]
    pub vs_args: Option<Vec<String>>,
    /// Let several encoders pull frames from a VapourSynth script at once.
    ///
    /// Bare `--stream-concurrently` means on; `--stream-concurrently off`
    /// decodes a whole scene ahead of its encoders instead. Scripts only.
    #[arg(long, value_name = "on|off", num_args = 0..=1, default_missing_value = "on")]
    pub stream_concurrently: Option<Toggle>,
}

impl InputOverride {
    /// Whether the override sets nothing at all.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.path.is_none()
            && self.decoder.is_none()
            && self.filters.is_none()
            && self.vs_args.is_none()
            && self.stream_concurrently.is_none()
    }
}

/// Flags shared by every `set strategy` property: they build the probing
/// strategy the same way in `target-quality` and `quality-check`.
#[derive(Debug, Clone, clap::Args)]
pub struct StrategyArgs {
    /// The probing strategy to use.
    #[arg(value_name = "KIND")]
    pub kind: ProbeStrategyKind,
    /// Frames between probes; required by *skip*.
    #[arg(value_name = "Frames")]
    pub skip: Option<u32>,
    /// Where in the scene to probe; used by *subset*.
    #[arg(long, value_name = "Position")]
    pub position: Option<SubsetPosition>,
    /// Subset length in frames; used by *subset*.
    #[arg(long, value_name = "Frames")]
    pub length_frames: Option<u32>,
    /// Subset length as a finite percentage (0 < percent <= 100), at least one frame.
    #[arg(long, value_name = "Percent")]
    pub length_percent: Option<f64>,
}

/// Flags shared by every `set statistic` property: they build the probe
/// statistic the same way in `target-quality` and `quality-check`.
#[derive(Debug, Clone, clap::Args)]
pub struct StatisticArgs {
    /// The statistic to reduce the probe scores with.
    #[arg(value_name = "KIND")]
    pub kind: StatisticKind,
    /// The value required by *percentile* (`0..=100`) or
    /// *standard-deviation-distance* (`SIGMA`).
    #[arg(value_name = "VALUE", allow_hyphen_values = true)]
    pub value: Option<f64>,
    /// The percentile (`0..=100`) for *percentile*.
    #[arg(long, value_name = "Percent")]
    pub percentile: Option<f64>,
    /// The standard deviation distance for *standard-deviation-distance*.
    #[arg(long, value_name = "Sigma", allow_hyphen_values = true)]
    pub sigma: Option<f64>,
}

/// The strategies `set strategy` can choose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ProbeStrategyKind {
    /// Probe every frame of the scene.
    Whole,
    /// Probe one frame every FRAMES frames.
    Skip,
    /// Probe a contiguous part of the scene.
    Subset,
}

/// Where `set strategy subset` probes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SubsetPosition {
    Start,
    Middle,
    End,
}

/// The statistics `set statistic` can choose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum StatisticKind {
    Mean,
    Median,
    Harmonic,
    /// One value at the PERCENTILE (`0..=100`) of the scores.
    Percentile,
    /// The mean shifted by SIGMA standard deviations.
    StandardDeviationDistance,
    Mode,
    Minimum,
    Maximum,
    RootMeanSquare,
}

/// Every way a `set strategy` or `set statistic` value can be malformed.
#[derive(Debug, Error)]
pub enum ProbeError {
    #[error("the whole strategy takes no values")]
    WholeStrategyTakesNoValues,
    #[error("the whole strategy takes no flags")]
    WholeStrategyTakesNoFlags,
    #[error("the skip strategy needs a FRAMES value")]
    SkipStrategyNeedsFrames,
    #[error("the skip strategy's FRAMES value must be positive")]
    SkipStrategyNotPositive,
    #[error("the skip strategy takes no other flags")]
    SkipStrategyTakesOnlyFrames,
    #[error("the subset strategy does not take a FRAMES value")]
    SubsetStrategyTakesNoSkip,
    #[error("--length-frames must be positive")]
    SubsetLengthNotPositive,
    #[error("--length-percent must be finite and within 0 < percent <= 100")]
    SubsetLengthPercentOutOfRange,
    #[error("provide either --length-frames or --length-percent, not both")]
    SubsetLengthAmbiguous,
    #[error("the {0} statistic takes no values")]
    StatisticTakesNoValues(&'static str),
    #[error("the percentile statistic needs a PERCENT value (or --percentile PERCENT)")]
    PercentileNeedsValue,
    #[error("percentile must be within 0..=100")]
    PercentileOutOfRange,
    #[error("provide the percentile once, either positionally or with --percentile")]
    PercentileDuplicate,
    #[error("--sigma belongs to standard-deviation-distance")]
    SigmaBelongsToOtherStatistic,
    #[error("the standard-deviation-distance statistic needs a SIGMA value (or --sigma SIGMA)")]
    StandardDeviationNeedsValue,
    #[error("sigma must be a finite number")]
    SigmaNotFinite,
    #[error("provide sigma once, either positionally or with --sigma")]
    SigmaDuplicate,
    #[error("--percentile belongs to percentile")]
    PercentileBelongsToOtherStatistic,
}

impl StrategyArgs {
    /// Builds the probing strategy these values describe.
    pub fn build(&self) -> Result<ProbeStrategy, ProbeError> {
        let takes_flags = self.position.is_some()
            || self.length_frames.is_some()
            || self.length_percent.is_some();
        match self.kind {
            ProbeStrategyKind::Whole => {
                if self.skip.is_some() {
                    return Err(ProbeError::WholeStrategyTakesNoValues);
                }
                if takes_flags {
                    return Err(ProbeError::WholeStrategyTakesNoFlags);
                }
                Ok(ProbeStrategy::Whole)
            },
            ProbeStrategyKind::Skip => {
                let skip = self.skip.ok_or(ProbeError::SkipStrategyNeedsFrames)?;
                if (ProbeStrategy::Skip { skip }).validate().is_err() {
                    return Err(ProbeError::SkipStrategyNotPositive);
                }
                if takes_flags {
                    return Err(ProbeError::SkipStrategyTakesOnlyFrames);
                }
                Ok(ProbeStrategy::Skip { skip })
            },
            ProbeStrategyKind::Subset => {
                if self.skip.is_some() {
                    return Err(ProbeError::SubsetStrategyTakesNoSkip);
                }
                let length = match (self.length_frames, self.length_percent) {
                    (Some(frames), None) => SubsetProbeLength::Frames(frames),
                    (None, Some(percent)) => SubsetProbeLength::Percentage(percent),
                    (None, None) => SubsetProbeLength::default(),
                    (Some(_), Some(_)) => return Err(ProbeError::SubsetLengthAmbiguous),
                };
                let strategy = ProbeStrategy::Subset {
                    position: self.position.map_or(SubsetProbePosition::Middle, |position| {
                        match position {
                            SubsetPosition::Start => SubsetProbePosition::Start,
                            SubsetPosition::Middle => SubsetProbePosition::Middle,
                            SubsetPosition::End => SubsetProbePosition::End,
                        }
                    }),
                    length,
                };
                strategy.validate().map_err(|_| match length {
                    SubsetProbeLength::Frames(_) => ProbeError::SubsetLengthNotPositive,
                    SubsetProbeLength::Percentage(_) => ProbeError::SubsetLengthPercentOutOfRange,
                })?;
                Ok(strategy)
            },
        }
    }
}

impl StatisticArgs {
    /// Builds the probe statistic these values describe.
    pub fn build(&self) -> Result<ProbeStatistic, ProbeError> {
        let takes_values =
            self.value.is_some() || self.percentile.is_some() || self.sigma.is_some();
        match self.kind {
            StatisticKind::Mean => {
                self.reject_flags(takes_values, "mean")?;
                Ok(ProbeStatistic::Mean)
            },
            StatisticKind::Median => {
                self.reject_flags(takes_values, "median")?;
                Ok(ProbeStatistic::Median)
            },
            StatisticKind::Harmonic => {
                self.reject_flags(takes_values, "harmonic")?;
                Ok(ProbeStatistic::Harmonic)
            },
            StatisticKind::Mode => {
                self.reject_flags(takes_values, "mode")?;
                Ok(ProbeStatistic::Mode)
            },
            StatisticKind::Minimum => {
                self.reject_flags(takes_values, "minimum")?;
                Ok(ProbeStatistic::Minimum)
            },
            StatisticKind::Maximum => {
                self.reject_flags(takes_values, "maximum")?;
                Ok(ProbeStatistic::Maximum)
            },
            StatisticKind::RootMeanSquare => {
                self.reject_flags(takes_values, "root-mean-square")?;
                Ok(ProbeStatistic::RootMeanSquare)
            },
            StatisticKind::Percentile => {
                if self.sigma.is_some() {
                    return Err(ProbeError::SigmaBelongsToOtherStatistic);
                }
                if self.value.is_some() && self.percentile.is_some() {
                    return Err(ProbeError::PercentileDuplicate);
                }
                let percentile =
                    self.value.or(self.percentile).ok_or(ProbeError::PercentileNeedsValue)?;
                if !percentile.is_finite() || !(0.0..=100.0).contains(&percentile) {
                    return Err(ProbeError::PercentileOutOfRange);
                }
                Ok(ProbeStatistic::Percentile(percentile))
            },
            StatisticKind::StandardDeviationDistance => {
                if self.percentile.is_some() {
                    return Err(ProbeError::PercentileBelongsToOtherStatistic);
                }
                if self.value.is_some() && self.sigma.is_some() {
                    return Err(ProbeError::SigmaDuplicate);
                }
                let sigma =
                    self.value.or(self.sigma).ok_or(ProbeError::StandardDeviationNeedsValue)?;
                if !sigma.is_finite() {
                    return Err(ProbeError::SigmaNotFinite);
                }
                Ok(ProbeStatistic::StandardDeviationDistance { sigma })
            },
        }
    }

    /// Rejects a statistic that was handed a value it cannot use.
    fn reject_flags(&self, takes_values: bool, name: &'static str) -> Result<(), ProbeError> {
        if takes_values {
            return Err(ProbeError::StatisticTakesNoValues(name));
        }
        Ok(())
    }
}

/// What one `set` or `reset` actually changed, for the feedback printed after
/// the configuration is saved.
#[derive(Debug, Clone)]
pub struct PropertyChange {
    /// The property that changed, e.g. `method` — for list-addressed edits
    /// such as `av1an scenes` it may name its item, e.g. `scene 3 params`.
    pub property: String,
    /// The property's value after the change, in the shape `info` prints.
    pub value: Value,
}

impl PropertyChange {
    /// A change to `property`, whose new value is `value`.
    pub fn new<T: serde::Serialize>(property: impl Into<String>, value: T) -> anyhow::Result<Self> {
        let property = property.into();
        Ok(Self {
            property,
            value: serde_json::to_value(value)?,
        })
    }
}

/// One command's configuration section, editable through `info`, `set` and
/// `reset`.
///
/// Implementing this per command keeps the load / validate / save / warn driver
/// in one place while every property's own logic stays next to the handler that
/// owns it.
pub trait ConfigCommand {
    /// The properties `info` and `reset` accept.
    type Key: Subcommand + Clone;
    /// The `set` subcommand, carrying one property's values.
    type Set: Subcommand + Clone;

    /// Name used in the staleness warning, e.g. `Scene detection settings`.
    fn subject() -> &'static str;

    /// The value of `key`, or of the whole section when `key` is `None`.
    fn render(configuration: &Configuration, key: Option<&Self::Key>) -> anyhow::Result<Value>;

    /// Applies `set` and reports what changed. Must validate before returning
    /// so a rejected edit never reaches the configuration file.
    fn apply(configuration: &mut Configuration, set: &Self::Set) -> anyhow::Result<PropertyChange>;

    /// Restores `key` (or the whole section) to its default and reports the
    /// restored value; `None` when there was nothing to reset.
    fn restore(
        configuration: &mut Configuration,
        key: Option<&Self::Key>,
    ) -> anyhow::Result<Option<PropertyChange>>;

    /// Extra line printed after a reset, when the reset has a useful follow-up.
    fn reset_hint(key: Option<&Self::Key>) -> Option<String> {
        let _ = key;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{ProbeStrategyKind, StrategyArgs};

    #[test]
    fn quality_strategy_positive_percentage_up_to_hundred_is_accepted() {
        let args = StrategyArgs {
            kind: ProbeStrategyKind::Subset,
            skip: None,
            position: None,
            length_frames: None,
            length_percent: None,
        };
        for percent in [f64::MIN_POSITIVE, 25.0, 100.0] {
            assert!(
                StrategyArgs {
                    length_percent: Some(percent),
                    ..args
                }
                .build()
                .is_ok()
            );
        }
    }
}
