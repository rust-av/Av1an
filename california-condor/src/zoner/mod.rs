//! The CLI-only Zoner sequence.
//!
//! A [`Zone`](zone::Zone) is a frame range plus the encoder settings and scene
//! length cap its scenes take. Zones reach Condor through
//! [`ZonerConfig`], written by the *--zones* flag on
//! [`av1an`](../../commands/condor.md) or by the
//! [`zone`](../../commands/zone.md) command from zone strings, JSON, JSON
//! lines, or text files.
//!
//! The work is split three ways: [`zone`] holds the data model and the encoder
//! application, [`parse`] reads zones from strings and files, and this module
//! owns the configuration, the validation and the [`Sequence`]
//! implementation. Application and validation are plain functions over scenes
//! so they can be tested without an open input.
//!
//! The pipeline applies zones after Scene Detector. A compact original baseline
//! and structured applied definitions make unchanged passes idempotent and
//! changed/removed overrides reversible. Unknown legacy baselines and unsafe
//! rebases refuse without overwriting scene edits or deleting files.

use std::sync::{self, Arc, atomic::AtomicBool};

use andean_condor::{
    core::{
        Condor,
        sequence::{Sequence, SequenceDetails, SequenceStatus},
    },
    models::{
        encoder::Encoder,
        scene::{Scene, SubScene},
    },
};
use anyhow::{Context, Result, bail};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::{
    configuration::{CliSequenceConfig, CliSequenceData},
    scenes::invalidate_changed_results,
    zoner::zone::zone_label,
};

pub mod parse;
pub mod zone;

pub use parse::{load_zones, parse_zone, parse_zones};
pub use zone::{
    Zone, ZoneEncoder, ZoneEncoderMerge, ZoneOptions, ZoneOptionsBody, ZonePhotonNoise,
    ZonePhotonNoiseBody,
};

pub static DETAILS: SequenceDetails = SequenceDetails {
    name: "Zoner",
    description: "Overrides the encoder of the scenes inside frame ranges.",
    version: "0.0.1",
};

/// `condor.sequence_config.zoner` in `condor.json`.
/// Unknown keys are rejected; an omitted `zones` list defaults to empty.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ZonerConfig {
    /// The overrides, applied in start-frame order.
    #[serde(default)]
    pub zones: Vec<Zone>,
}

/// `condor.scenes[].sequence_data.zoner` in `condor.json`.
///
/// The legacy pass flag and, on the first scene, the proof needed for safe rebase.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ZonerData {
    /// Whether the Zoner has processed this scene list. Alone this flag cannot
    /// prove which definitions ran or what original settings they replaced.
    #[serde(default)]
    pub zoned: bool,
    /// One compact list-level record, stored on the first scene only. Absence
    /// on legacy zoned lists is not evidence of an original encoder baseline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifecycle: Option<ZoneLifecycle>,
}

/// Original and last-applied scene definitions, without any recursive results.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ZoneLifecycle {
    /// Zoning algorithm, not a result certificate: 0 (including missing) is
    /// whole-scene zoning; 1 partitions at exact half-open boundaries.
    #[serde(default)]
    pub algorithm_revision: u32,
    pub baseline: Vec<SceneDefinition>,
    pub zones: Vec<Zone>,
    pub applied: Vec<SceneDefinition>,
}

const ALGORITHM_REVISION: u32 = 1;

/// Only the fields a zoning pass may change; never copies probe histories.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SceneDefinition {
    pub start_frame: usize,
    pub end_frame: usize,
    pub sub_scenes: Option<Vec<SubScene>>,
    pub encoder: Encoder,
}

impl From<&Scene<CliSequenceData>> for SceneDefinition {
    fn from(scene: &Scene<CliSequenceData>) -> Self {
        Self {
            start_frame: scene.start_frame,
            end_frame: scene.end_frame,
            sub_scenes: scene.sub_scenes.clone(),
            encoder: scene.encoder.clone(),
        }
    }
}

fn definitions(scenes: &[Scene<CliSequenceData>]) -> Vec<SceneDefinition> {
    scenes.iter().map(SceneDefinition::from).collect()
}

fn baseline_scenes(baseline: &[SceneDefinition]) -> Vec<Scene<CliSequenceData>> {
    baseline
        .iter()
        .map(|definition| Scene {
            start_frame: definition.start_frame,
            end_frame: definition.end_frame,
            sub_scenes: definition.sub_scenes.clone(),
            encoder: definition.encoder.clone(),
            sequence_data: CliSequenceData::default(),
        })
        .collect()
}

#[derive(Default)]
pub struct Zoner {}

impl Zoner {
    pub const DETAILS: SequenceDetails = DETAILS;

    /// Validates and applies the configured zones.
    ///
    /// Unchanged current-algorithm definitions do not apply or save again.
    /// Old-algorithm or changed/removed definitions rebuild from the original
    /// baseline or refuse an unsafe rebase. If old/new algorithms produce the
    /// same recorded definitions, only the revision tag is published; live
    /// downstream edits and results remain untouched.
    pub fn run(condor: &mut Condor<CliSequenceData, CliSequenceConfig>) -> Result<()> {
        let mut zoner = Self::default();

        let (_, warnings) = zoner.validate(condor)?;
        log_warnings(&warnings);

        let (progress_tx, _progress_rx) = sync::mpsc::channel();
        let (_, warnings) = zoner.execute(condor, progress_tx, Arc::new(AtomicBool::new(false)))?;
        log_warnings(&warnings);

        Ok(())
    }
}

impl Sequence<CliSequenceData, CliSequenceConfig> for Zoner {
    #[inline]
    fn details(&self) -> SequenceDetails {
        DETAILS
    }

    fn validate(
        &mut self,
        condor: &mut Condor<CliSequenceData, CliSequenceConfig>,
    ) -> Result<((), Vec<anyhow::Error>)> {
        let zones = configured_zones(&condor.sequence_config);
        if !zones.is_empty() {
            validate_zones(zones, condor.input.clip_info()?.num_frames)?;
        }

        Ok(((), Vec::new()))
    }

    #[inline]
    fn initialize(
        &mut self,
        _condor: &mut Condor<CliSequenceData, CliSequenceConfig>,
        _progress_tx: sync::mpsc::Sender<SequenceStatus>,
    ) -> Result<((), Vec<anyhow::Error>)> {
        Ok(((), Vec::new()))
    }

    fn execute(
        &mut self,
        condor: &mut Condor<CliSequenceData, CliSequenceConfig>,
        _progress_tx: sync::mpsc::Sender<SequenceStatus>,
        _cancelled: Arc<AtomicBool>,
    ) -> Result<((), Vec<anyhow::Error>)> {
        let zones = configured_zones(&condor.sequence_config);
        let lifecycle =
            condor.scenes.first().and_then(|s| s.sequence_data.zoner.lifecycle.as_ref());
        if lifecycle.is_none()
            && condor
                .scenes
                .iter()
                .any(|s| s.sequence_data.zoner.zoned || s.sequence_data.zoner.lifecycle.is_some())
        {
            bail!(
                "Zoner: legacy or edited zoned scenes have no proven original baseline/applied definitions; preserve edits and reset/redetect the whole scene list in an empty work directory, or use a new job. Resetting metadata does not delete physical chunks"
            );
        }
        if zones.is_empty() && lifecycle.is_none() {
            return Ok(((), Vec::new()));
        }
        if condor.scenes.is_empty() {
            bail!("no scenes to zone; run Scene Detector first");
        }
        let num_frames = condor.input.clip_info()?.num_frames;
        validate_zones(zones, num_frames)?;
        if let Some(state) = lifecycle {
            if state.algorithm_revision > ALGORITHM_REVISION {
                bail!(
                    "Zoner: unsupported algorithm revision {}; use a compatible Condor version or a separate new job; existing configuration and files are preserved",
                    state.algorithm_revision
                );
            }
            if state.algorithm_revision == ALGORITHM_REVISION && state.zones == zones {
                info!(
                    "Zoner: the scenes already record a zoning pass; nothing to apply (definitions unchanged)"
                );
                return Ok(((), Vec::new()));
            }
            if state.algorithm_revision == 0 && state.zones == zones && !state.baseline.is_empty() {
                let mut expected = baseline_scenes(&state.baseline);
                apply_zones_inner(zones, &mut expected, num_frames, false);
                if definitions(&expected) == state.applied {
                    // No scene rewrite is needed, even if downstream producers
                    // changed the live encoder. This tag certifies no results.
                    let mut revised = state.clone();
                    revised.algorithm_revision = ALGORITHM_REVISION;
                    let mut candidate = condor.scenes.clone();
                    candidate[0].sequence_data.zoner.lifecycle = Some(revised);
                    let mut staged = condor.as_data();
                    staged.scenes = candidate.clone();
                    (condor.save_callback)(staged)?;
                    condor.scenes = candidate;
                    info!("Zoner: equivalent zoning algorithms; updated algorithm revision only");
                    return Ok(((), Vec::new()));
                }
            }
            if state.applied != definitions(&condor.scenes) {
                bail!(
                    "Zoner: scene definitions changed after zoning; cannot safely rebase over manual or derived encoder edits. Preserve edits and reset/redetect the whole scene list in an empty work directory, or use a new job"
                );
            }
        }
        let baseline = lifecycle.map_or_else(
            || definitions(&condor.scenes),
            |state| state.baseline.clone(),
        );
        let mut candidate = if lifecycle.is_none() {
            condor.scenes.clone()
        } else {
            baseline_scenes(&baseline)
        };
        let mut warnings = apply_zones(zones, &mut candidate, num_frames);
        // Keep unaffected records exactly. For changed definitions, retain only
        // the matching range's data until shared invalidation withdraws it.
        if lifecycle.is_some() {
            for (index, scene) in candidate.iter_mut().enumerate() {
                let same_range = |before: &&Scene<CliSequenceData>| {
                    before.start_frame == scene.start_frame
                        && before.end_frame == scene.end_frame
                        && before.sub_scenes == scene.sub_scenes
                };
                if let Some(before) = condor
                    .scenes
                    .get(index)
                    .filter(same_range)
                    .or_else(|| condor.scenes.iter().find(same_range))
                {
                    scene.sequence_data = before.sequence_data.clone();
                }
            }
        }
        warnings.extend(invalidate_changed_results(
            &condor.scenes,
            &mut candidate,
            &condor.sequence_config,
            &condor.input.as_data(),
            "zoning",
        )?);
        let applied = definitions(&candidate);
        for scene in &mut candidate {
            scene.sequence_data.zoner = ZonerData {
                zoned: true,
                lifecycle: None,
            };
        }
        candidate[0].sequence_data.zoner.lifecycle = Some(ZoneLifecycle {
            algorithm_revision: ALGORITHM_REVISION,
            baseline,
            zones: zones.to_vec(),
            applied,
        });
        // Publish the complete candidate without mutating caller state. The CLI
        // callback validates the whole Configuration under its outer JobLock.
        let mut staged = condor.as_data();
        staged.scenes = candidate.clone();
        (condor.save_callback)(staged)?;
        condor.scenes = candidate;

        Ok(((), warnings))
    }
}

fn configured_zones(config: &CliSequenceConfig) -> &[Zone] {
    config.zoner.as_ref().map_or(&[], |zoner| &zoner.zones)
}

/// Checks every zone against an input of `num_frames` and that no two zones
/// overlap, which would make the settings a scene takes depend on their order.
pub fn validate_zones(zones: &[Zone], num_frames: usize) -> Result<()> {
    let mut ranges = Vec::with_capacity(zones.len());
    for (index, zone) in zones.iter().enumerate() {
        let (start, end) = zone.resolve(num_frames);
        zone.validate(num_frames).with_context(|| zone_label(index, start, end))?;
        ranges.push((index, start, end));
    }
    ranges.sort_by_key(|(_, start, _)| *start);

    for pair in ranges.windows(2) {
        let (index, start, end) = pair[0];
        let (next_index, next_start, next_end) = pair[1];
        if next_start < end {
            bail!(
                "{} overlaps {}; zones must not overlap",
                zone_label(index, start, end),
                zone_label(next_index, next_start, next_end)
            );
        }
    }

    Ok(())
}

/// Applies the zones to `scenes` in start-frame order.
///
/// Partition at all resolved zone boundaries before applying settings and caps.
/// Original cuts, order and gaps survive; outside pieces keep their encoder.
/// Callers validate zones first and own result invalidation/publication.
#[inline]
pub fn apply_zones(
    zones: &[Zone],
    scenes: &mut Vec<Scene<CliSequenceData>>,
    num_frames: usize,
) -> Vec<anyhow::Error> {
    apply_zones_inner(zones, scenes, num_frames, true)
}

// Equivalence checks use this exact pipeline without reporting hypothetical edits.
fn apply_zones_inner(
    zones: &[Zone],
    scenes: &mut Vec<Scene<CliSequenceData>>,
    num_frames: usize,
    report: bool,
) -> Vec<anyhow::Error> {
    let mut warnings = Vec::new();
    let initial_scenes = scenes.len();
    let mut total_changes = 0usize;
    if report {
        info!(
            "Zoner: applying {} zones to {initial_scenes} scenes",
            zones.len()
        );
    }
    let mut ordered = zones
        .iter()
        .enumerate()
        .map(|(index, zone)| {
            let (start, end) = zone.resolve(num_frames);

            (index, zone, start, end)
        })
        .collect::<Vec<_>>();
    ordered.sort_by_key(|(_, _, start, _)| *start);

    let boundaries =
        ordered.iter().flat_map(|(_, _, start, end)| [*start, *end]).collect::<Vec<_>>();
    *scenes = scenes.drain(..).flat_map(|scene| zone::partition(scene, &boundaries)).collect();
    if report && scenes.len() != initial_scenes {
        info!(
            "Zoner: zone boundaries added {} scenes ({initial_scenes} -> {} partitions) before overrides and caps",
            scenes.len() - initial_scenes,
            scenes.len()
        );
    }

    for (index, zone, start, end) in ordered {
        let mut zoned = Vec::with_capacity(scenes.len());
        let mut matched = 0usize;
        let mut modified = 0usize;
        let mut split = 0usize;
        let mut added = 0usize;
        let label = zone_label(index, start, end);
        for scene in scenes.drain(..) {
            if scene.end_frame <= start || scene.start_frame >= end {
                zoned.push(scene);
            } else {
                matched += 1;
                let scene_start = scene.start_frame;
                let scene_end = scene.end_frame;
                let original_encoder = scene.encoder.clone();
                let applied = zone.apply(index, scene, &mut warnings);
                let encoder_changed = applied[0].encoder != original_encoder;
                let was_split = applied.len() > 1;
                if encoder_changed || was_split {
                    modified += 1;
                }
                if report && encoder_changed {
                    info!(
                        "Zoner: {label} modified encoder settings for scene \
                         {scene_start}..{scene_end} ({} -> {})",
                        original_encoder.base(),
                        applied[0].encoder.base()
                    );
                }
                if was_split {
                    split += 1;
                    added += applied.len() - 1;
                    if report {
                        let ranges = applied
                            .iter()
                            .map(|scene| (scene.start_frame, scene.end_frame))
                            .collect::<Vec<_>>();
                        info!(
                            "Zoner: {label} split scene {scene_start}..{scene_end} into {} scenes ({} \
                             added), frame ranges: {ranges:?}",
                            applied.len(),
                            applied.len() - 1
                        );
                    }
                }
                zoned.extend(applied);
            }
        }
        *scenes = zoned;
        total_changes += modified;

        if report {
            info!(
                "Zoner: {label} matched {matched} scenes, modified {modified}, split {split}, added \
                 {added} ({} unchanged)",
                matched - modified
            );
        }
        if matched == 0 {
            warnings.push(anyhow::anyhow!(
                "{} matched no scenes",
                zone_label(index, start, end)
            ));
        }
    }

    if report {
        info!(
            "Zoner finished: {} zones, {total_changes} scene changes across zones, {} scenes added \
             ({initial_scenes} -> {} total), {} warnings",
            zones.len(),
            scenes.len() - initial_scenes,
            scenes.len(),
            warnings.len()
        );
    }

    warnings
}

/// Logs sequence warnings.
fn log_warnings(warnings: &[anyhow::Error]) {
    for warning in warnings {
        warn!("{warning}");
    }
}

#[cfg(test)]
mod tests {
    use andean_condor::models::{
        encoder::{Encoder, EncoderBase, cli_parameter::CLIParameter},
        scene::Scene,
    };

    use super::{
        zone::{ZoneEncoder, ZoneEncoderMerge, ZoneOptions, ZoneOptionsBody},
        *,
    };
    use crate::configuration::CliSequenceData;

    fn scenes() -> Vec<Scene<CliSequenceData>> {
        vec![scene(0, 100), scene(100, 200), scene(200, 400)]
    }

    fn scene(start_frame: usize, end_frame: usize) -> Scene<CliSequenceData> {
        Scene {
            start_frame,
            end_frame,
            sub_scenes: None,
            encoder: Encoder::default_from_base(&EncoderBase::SVTAV1, false),
            sequence_data: CliSequenceData::default(),
        }
    }

    /// A zone replacing the encoder of `start..end` with x265.
    fn replace(start_frame: Option<usize>, end_frame: Option<usize>) -> Zone {
        Zone {
            start_frame,
            end_frame,
            maximum_frames: None,
            encoder: Some(ZoneEncoder::Replace(Encoder::default_from_base(
                &EncoderBase::X265,
                false,
            ))),
        }
    }

    #[test]
    fn exact_boundary_partition_table() {
        use EncoderBase::{SVTAV1 as S, X265 as X};
        // Literal oracles catch whole-scene overrides, lost cuts/gaps and inclusive ends.
        let cases = [
            (
                vec![(0, 100)],
                vec![(Some(25), Some(75))],
                vec![(0, 25, S), (25, 75, X), (75, 100, S)],
            ),
            (
                vec![(0, 100)],
                vec![(Some(75), Some(100)), (Some(0), Some(25)), (Some(25), Some(75))],
                vec![(0, 25, X), (25, 75, X), (75, 100, X)],
            ),
            (
                vec![(0, 100)],
                vec![(None, Some(25)), (Some(75), None)],
                vec![(0, 25, X), (25, 75, S), (75, 100, X)],
            ),
            (
                vec![(0, 25), (25, 75), (75, 100)],
                vec![(Some(25), Some(75))],
                vec![(0, 25, S), (25, 75, X), (75, 100, S)],
            ),
            (
                vec![(10, 40), (60, 90)],
                vec![(Some(25), Some(75))],
                vec![(10, 25, S), (25, 40, X), (60, 75, X), (75, 90, S)],
            ),
            (vec![(0, 25)], vec![(Some(25), Some(75))], vec![(0, 25, S)]),
            (
                vec![(75, 100), (0, 25)],
                vec![(None, None)],
                vec![(75, 100, X), (0, 25, X)],
            ),
        ];
        for (ranges, bounds, expected) in cases {
            let mut actual = ranges.iter().map(|&(a, b)| scene(a, b)).collect();
            let zones = bounds.into_iter().map(|(a, b)| replace(a, b)).collect::<Vec<_>>();
            validate_zones(&zones, 100).expect("valid disjoint zones");
            let warnings = apply_zones(&zones, &mut actual, 100);
            assert_eq!(warnings.len(), usize::from(expected == vec![(0, 25, S)]));
            assert_eq!(
                actual
                    .iter()
                    .map(|s| (s.start_frame, s.end_frame, s.encoder.base()))
                    .collect::<Vec<_>>(),
                expected
            );
            for frame in 0..100 {
                assert_eq!(
                    actual.iter().filter(|s| s.start_frame <= frame && frame < s.end_frame).count(),
                    ranges.iter().filter(|&&(a, b)| a <= frame && frame < b).count(),
                    "frame {frame}"
                );
            }
        }
    }

    #[test]
    fn caps_follow_boundaries_and_clip_crossing_subscenes() {
        let mut original = scene(0, 100);
        original.sub_scenes = Some(vec![SubScene {
            start_frame: 20,
            end_frame: 80,
        }]);
        let mut zone = replace(Some(25), Some(75));
        zone.maximum_frames = Some(20);
        let mut actual = vec![original.clone()];
        assert!(apply_zones(&[zone.clone()], &mut actual, 100).is_empty());
        let expected = vec![(0, 25), (25, 41), (41, 58), (58, 75), (75, 100)];
        assert_eq!(
            actual.iter().map(|s| (s.start_frame, s.end_frame)).collect::<Vec<_>>(),
            expected
        );
        assert_eq!(
            actual
                .iter()
                .map(|s| s
                    .sub_scenes
                    .as_ref()
                    .expect("clipped subscene")
                    .iter()
                    .map(|s| (s.start_frame, s.end_frame))
                    .collect::<Vec<_>>())
                .collect::<Vec<_>>(),
            vec![vec![(20, 25)], vec![(25, 41)], vec![(41, 58)], vec![(58, 75)], vec![(75, 80)]]
        );
        assert_eq!(actual[0].encoder, original.encoder);
        assert_eq!(actual[4].encoder, original.encoder);
        assert!(actual[1..4].iter().all(|s| s.encoder.base() == EncoderBase::X265));
        for frame in 0..100 {
            assert_eq!(
                actual.iter().filter(|s| s.start_frame <= frame && frame < s.end_frame).count(),
                1
            );
        }
        // The public direct boundary must share the partition contract, not leak overrides.
        let direct = zone.apply(0, original, &mut Vec::new());
        assert_eq!(definitions(&direct), definitions(&actual));
    }

    #[test]
    fn only_frames_inside_a_zone_are_zoned() {
        let mut scenes = scenes();

        let warnings = apply_zones(&[replace(Some(50), Some(200))], &mut scenes, 400);

        assert!(warnings.is_empty(), "every zone matches");
        assert_eq!(
            scenes.len(),
            4,
            "the interior boundary splits the first scene"
        );
        assert_eq!(
            scenes[0].encoder.base(),
            EncoderBase::SVTAV1,
            "frames before 50 stay outside"
        );
        assert_eq!(
            scenes[1].encoder.base(),
            EncoderBase::X265,
            "frame 50 is inside"
        );
        assert_eq!(
            scenes[2].encoder.base(),
            EncoderBase::X265,
            "frame 199 is inside"
        );
        assert_eq!(
            scenes[3].encoder.base(),
            EncoderBase::SVTAV1,
            "frame 200 is outside the exclusive end"
        );
    }

    #[test]
    fn unset_bounds_reach_the_input_edges() {
        let mut scenes = scenes();

        let warnings = apply_zones(&[replace(None, None)], &mut scenes, 400);

        assert!(warnings.is_empty());
        assert!(
            scenes.iter().all(|scene| scene.encoder.base() == EncoderBase::X265),
            "an unbounded zone covers every scene"
        );
    }

    #[test]
    fn a_zone_matching_nothing_warns() {
        let mut scenes = vec![scene(0, 100)];

        let warnings = apply_zones(&[replace(Some(300), Some(400))], &mut scenes, 400);

        assert_eq!(warnings.len(), 1, "the unmatched zone is reported");
        assert!(
            warnings[0].to_string().contains("matched no scenes"),
            "warning names the problem: {}",
            warnings[0]
        );
        assert_eq!(scenes[0].encoder.base(), EncoderBase::SVTAV1);
    }

    /// A zone merging `crf` into the scenes of `start..end`.
    fn crf(start_frame: usize, end_frame: usize, crf: f64) -> Zone {
        Zone {
            start_frame: Some(start_frame),
            end_frame: Some(end_frame),
            maximum_frames: None,
            encoder: Some(ZoneEncoder::Merge(ZoneEncoderMerge {
                base: Some(EncoderBase::SVTAV1),
                options: Some(ZoneOptions::Merge(ZoneOptionsBody::Map(
                    CLIParameter::new_numbers("--", " ", &[("crf", crf)]),
                ))),
                ..Default::default()
            })),
        }
    }

    #[test]
    fn zones_apply_in_start_frame_order() {
        let mut scenes = vec![scene(0, 300)];

        // Disjoint zones cannot overwrite each other's frames, even out of order.
        let warnings = apply_zones(&[crf(200, 400, 30.0), crf(0, 100, 23.0)], &mut scenes, 400);

        assert!(warnings.is_empty(), "both zones match");
        assert_eq!(
            scenes
                .iter()
                .map(|s| (
                    s.start_frame,
                    s.end_frame,
                    s.encoder.parameters().get("crf").cloned()
                ))
                .collect::<Vec<_>>(),
            vec![
                (0, 100, Some(CLIParameter::new_number("--", " ", 23.0))),
                (100, 200, Some(CLIParameter::new_number("--", " ", 25.0))),
                (200, 300, Some(CLIParameter::new_number("--", " ", 30.0)))
            ]
        );
    }

    #[test]
    fn a_zone_caps_the_length_of_the_scenes_it_matches() {
        let mut scenes = scenes();
        let zone = Zone {
            start_frame: Some(200),
            end_frame: None,
            maximum_frames: Some(100),
            encoder: None,
        };

        let warnings = apply_zones(&[zone], &mut scenes, 400);

        assert!(warnings.is_empty());
        assert_eq!(scenes.len(), 4, "the 200-frame scene becomes two");
        assert_eq!((scenes[3].start_frame, scenes[3].end_frame), (300, 400));
    }

    #[test]
    fn validation_rejects_overlapping_zones() {
        let zones = vec![replace(Some(0), Some(250)), replace(Some(200), Some(300))];

        let error = validate_zones(&zones, 400).expect_err("the zones overlap");

        assert!(
            error.to_string().contains("zones must not overlap"),
            "error explains the overlap: {error}"
        );
    }

    #[test]
    fn validation_accepts_adjacent_and_disjoint_zones() {
        let adjacent = vec![replace(Some(0), Some(200)), replace(Some(200), Some(300))];
        assert!(
            validate_zones(&adjacent, 400).is_ok(),
            "touching ranges do not overlap"
        );

        let disjoint = vec![replace(Some(300), Some(400)), replace(Some(0), Some(100))];
        assert!(
            validate_zones(&disjoint, 400).is_ok(),
            "out-of-order zones are fine"
        );
    }

    #[test]
    fn validation_names_the_invalid_zone() {
        let zones = vec![replace(Some(0), Some(200)), replace(Some(100), Some(100))];

        let error = validate_zones(&zones, 400).expect_err("the second zone is empty");

        assert!(
            error.to_string().starts_with("zone 2"),
            "error names the zone: {error}"
        );
    }

    #[test]
    fn no_zones_change_nothing() {
        let mut scenes = scenes();

        let warnings = apply_zones(&[], &mut scenes, 400);

        assert!(warnings.is_empty());
        assert_eq!(scenes.len(), 3);
    }
}
