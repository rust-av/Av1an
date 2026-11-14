//! Scene-list policy shared by every command that edits saved scenes.
//!
//! `condor.scenes` is written by Scene Detector and edited in place by the
//! Zoner ([`crate::zoner`]) and by *av1an scenes*
//! ([`crate::commands::handlers::scenes`]). Both need the same answers about
//! the list itself — which scenes an index argument addresses, whether a
//! frame range is sane, which scenes an edit overlaps, whether sub-scene
//! ranges are sane, and which saved results or files a change makes stale —
//! so those rules live here instead of being spelled twice. Zone parsing and
//! zone application stay in [`crate::zoner`]: zones are a declarative policy
//! applied on every run, scene edits are direct changes to what detection
//! saved.

use std::{fs, str::FromStr};

use andean_condor::{
    core::sequence::{parallel_encoder::ParallelEncoder, target_quality::DETAILS},
    models::scene::{Scene, SubScene},
};
use anyhow::{Context, Result};
use thiserror::Error;

use crate::configuration::{CliSequenceConfig, CliSequenceData};

/// Every way a scene-list edit can be rejected.
#[derive(Debug, Error)]
pub enum ScenesError {
    #[error("no scenes configured; run `av1an detect-scenes` first")]
    NoScenes,
    #[error(
        "scene index {index} does not exist; the configuration holds {count} scenes (0..{count})"
    )]
    NoSuchScene { index: usize, count: usize },
    #[error("scene {index} would be empty: start frame {frame} is not before its end frame {end}")]
    FrameNotBeforeEnd {
        index: usize,
        frame: usize,
        end: usize,
    },
    #[error("scene {index} would be empty: end frame {frame} is not after its start frame {start}")]
    FrameNotAfterStart {
        index: usize,
        frame: usize,
        start: usize,
    },
    #[error("scene range {start}..{end} is empty")]
    EmptyScene { start: usize, end: usize },
    #[error(
        "`{text}` is not a scene index: use a number (`1`), a range (`0-4`), a list \
         (`1-2,8-10,16`), or `-1`/`all`/`every` for every scene"
    )]
    SelectionFormat { text: String },
    #[error("scene range {start}-{end} runs backwards: write the lower index first")]
    SelectionRangeReversed { start: usize, end: usize },
    #[error("sub-scene range {start}..{end} is empty")]
    EmptySubScene { start: usize, end: usize },
    #[error("sub-scene range {start}..{end} lies outside scene {scene_start}..{scene_end}")]
    SubSceneOutsideScene {
        start: usize,
        end: usize,
        scene_start: usize,
        scene_end: usize,
    },
    #[error(
        "sub-scene range {start}..{end} overlaps or precedes the range ending at {previous_end}; \
         ranges must ascend"
    )]
    SubScenesOutOfOrder {
        start: usize,
        end: usize,
        previous_end: usize,
    },
    #[error("sub-scene range `{text}` must be written as START-END, e.g. `240-480`")]
    SubSceneRangeFormat { text: String },
}

/// The scene at `index`, or an error naming the list's size.
#[inline]
pub fn require_scene(
    scenes: &[Scene<CliSequenceData>],
    index: usize,
) -> Result<&Scene<CliSequenceData>, ScenesError> {
    scenes.get(index).ok_or(ScenesError::NoSuchScene {
        index,
        count: scenes.len(),
    })
}

/// The mutable scene at `index`, or an error naming the list's size.
#[inline]
pub fn require_scene_mut(
    scenes: &mut [Scene<CliSequenceData>],
    index: usize,
) -> Result<&mut Scene<CliSequenceData>, ScenesError> {
    let count = scenes.len();
    scenes.get_mut(index).ok_or(ScenesError::NoSuchScene { index, count })
}

/// Which scenes an index argument addresses.
///
/// Written as a single index (`1`), an inclusive range (`0-4`), a list of
/// either (`1-2,8-10,16`), or `-1`/`all`/`every` for the whole list. Each part
/// is kept as written — a single index is the range `N-N` — and only
/// [`SceneSelection::resolve`] turns them into indices, against a list whose
/// size is known: a range reaching past the end is rejected instead of
/// expanded, however far it reaches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SceneSelection {
    /// Every scene in the list.
    All,
    /// The `(start, end)` ranges as written, both inclusive and ascending by
    /// start.
    Ranges(Vec<(usize, usize)>),
}

impl SceneSelection {
    /// Resolves the selection against a list of `count` scenes, naming the
    /// lowest index that does not exist.
    pub fn resolve(&self, count: usize) -> Result<Vec<usize>, ScenesError> {
        match self {
            Self::All => Ok((0..count).collect()),
            Self::Ranges(ranges) => {
                let mut indices: Vec<usize> = Vec::new();
                for &(start, end) in ranges {
                    if end >= count {
                        return Err(ScenesError::NoSuchScene {
                            index: start.max(count),
                            count,
                        });
                    }
                    // The ranges ascend, so whatever the previous one already
                    // covered is skipped rather than repeated.
                    let start = start.max(indices.last().map_or(0, |last| last + 1));
                    if start <= end {
                        indices.extend(start..=end);
                    }
                }
                Ok(indices)
            },
        }
    }
}

impl FromStr for SceneSelection {
    type Err = ScenesError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let text = text.trim();
        if text.eq_ignore_ascii_case("all") || text.eq_ignore_ascii_case("every") || text == "-1" {
            return Ok(Self::All);
        }
        let format = || ScenesError::SelectionFormat {
            text: text.to_owned(),
        };
        let mut ranges = Vec::new();
        for part in text.split(',') {
            let part = part.trim();
            ranges.push(match part.split_once('-') {
                Some((start, end)) => {
                    let start: usize = start.trim().parse().map_err(|_| format())?;
                    let end: usize = end.trim().parse().map_err(|_| format())?;
                    if start > end {
                        return Err(ScenesError::SelectionRangeReversed { start, end });
                    }
                    (start, end)
                },
                None => {
                    let index: usize = part.parse().map_err(|_| format())?;
                    (index, index)
                },
            });
        }
        // Ascending by start, so `resolve` can merge them in one pass.
        ranges.sort_unstable();

        Ok(Self::Ranges(ranges))
    }
}

/// Renders resolved indices the way they are written on the command line,
/// collapsing runs: `0-4`, `1,3`, `1-2,8-10,16`.
pub fn format_indices(indices: &[usize]) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut remaining = indices.iter().copied().peekable();
    while let Some(start) = remaining.next() {
        let mut end = start;
        while remaining.peek().is_some_and(|&next| next == end + 1) {
            end = remaining.next().expect("the peeked index");
        }
        parts.push(if start == end {
            format!("{start}")
        } else {
            format!("{start}-{end}")
        });
    }

    parts.join(",")
}

/// Sets scene `index`'s start frame, clipping its sub-scenes to the new
/// range.
///
/// The frame must leave the scene non-empty. The list does not have to stay
/// contiguous, so an edit may open a gap to, or overlap, the scenes around
/// it; the returned indices name the scenes the new range overlaps.
pub fn set_start(
    scenes: &mut [Scene<CliSequenceData>],
    index: usize,
    frame: usize,
) -> Result<Vec<usize>, ScenesError> {
    let end = require_scene(scenes, index)?.end_frame;
    if frame >= end {
        return Err(ScenesError::FrameNotBeforeEnd { index, frame, end });
    }
    let scene = require_scene_mut(scenes, index)?;
    scene.start_frame = frame;
    scene.sub_scenes = sub_scenes_within(scene.sub_scenes.as_deref(), frame, end);

    Ok(overlapping_scenes(scenes, index, frame, end))
}

/// Sets scene `index`'s end frame, clipping its sub-scenes to the new range.
///
/// The frame must leave the scene non-empty, and like [`set_start`] the
/// returned indices name the scenes the new range overlaps.
pub fn set_end(
    scenes: &mut [Scene<CliSequenceData>],
    index: usize,
    frame: usize,
) -> Result<Vec<usize>, ScenesError> {
    let start = require_scene(scenes, index)?.start_frame;
    if frame <= start {
        return Err(ScenesError::FrameNotAfterStart {
            index,
            frame,
            start,
        });
    }
    let scene = require_scene_mut(scenes, index)?;
    scene.end_frame = frame;
    scene.sub_scenes = sub_scenes_within(scene.sub_scenes.as_deref(), start, frame);

    Ok(overlapping_scenes(scenes, index, start, frame))
}

/// The scenes other than `index` whose frame range shares a frame with
/// `start`..`end`.
pub fn overlapping_scenes(
    scenes: &[Scene<CliSequenceData>],
    index: usize,
    start: usize,
    end: usize,
) -> Vec<usize> {
    scenes
        .iter()
        .enumerate()
        .filter(|(other, scene)| {
            *other != index && start < scene.end_frame && scene.start_frame < end
        })
        .map(|(other, _)| other)
        .collect()
}

/// Where a scene starting at `start_frame` belongs when the list stays sorted
/// by start frame ascending; the end of the list when it comes after every
/// other scene. A start equal to an existing scene's goes before it.
pub fn sorted_insert_index(scenes: &[Scene<CliSequenceData>], start_frame: usize) -> usize {
    scenes
        .iter()
        .position(|scene| scene.start_frame >= start_frame)
        .unwrap_or(scenes.len())
}

/// Clips intersecting sub-scenes to their new parent's half-open range.
#[inline]
pub fn sub_scenes_within(
    sub_scenes: Option<&[SubScene]>,
    start: usize,
    end: usize,
) -> Option<Vec<SubScene>> {
    let sub_scenes = sub_scenes?;
    let within = sub_scenes
        .iter()
        .filter_map(|sub_scene| {
            let start_frame = sub_scene.start_frame.max(start);
            let end_frame = sub_scene.end_frame.min(end);
            (start_frame < end_frame).then_some(SubScene {
                start_frame,
                end_frame,
            })
        })
        .collect::<Vec<_>>();

    (!within.is_empty()).then_some(within)
}

/// Parses `START-END` into a sub-scene range.
///
/// The ranges themselves are checked against their scene by
/// [`validate_sub_scenes`] before anything is written.
pub fn parse_sub_scene_range(text: &str) -> Result<SubScene, ScenesError> {
    let format = || ScenesError::SubSceneRangeFormat {
        text: text.to_owned(),
    };
    let (start, end) = text.split_once('-').ok_or_else(format)?;
    let start_frame = start.trim().parse().map_err(|_| format())?;
    let end_frame = end.trim().parse().map_err(|_| format())?;

    Ok(SubScene {
        start_frame,
        end_frame,
    })
}

/// Checks replacement sub-scenes against the scene that will hold them:
/// each range must be non-empty and inside its scene, and the ranges must
/// ascend without overlapping.
pub fn validate_sub_scenes(
    scene: &Scene<CliSequenceData>,
    sub_scenes: &[SubScene],
) -> Result<(), ScenesError> {
    let mut previous_end: Option<usize> = None;
    for sub_scene in sub_scenes {
        let (start, end) = (sub_scene.start_frame, sub_scene.end_frame);
        if start >= end {
            return Err(ScenesError::EmptySubScene { start, end });
        }
        if start < scene.start_frame || end > scene.end_frame {
            return Err(ScenesError::SubSceneOutsideScene {
                start,
                end,
                scene_start: scene.start_frame,
                scene_end: scene.end_frame,
            });
        }
        if let Some(previous_end) = previous_end
            && start < previous_end
        {
            return Err(ScenesError::SubScenesOutOfOrder {
                start,
                end,
                previous_end,
            });
        }
        previous_end = Some(end);
    }

    Ok(())
}

/// Saved results and existing files that a change to the scene list can
/// stale, as warnings; empty when the change touches nothing that matters.
///
/// Outputs are named by scene index, so a change can stale later scenes'
/// files even when their own encoder and frame range were not modified.
/// `reason` names the change ("zoning", "scene edits") so the warnings read
/// correctly wherever they are reported from.
pub fn stale_result_warnings(
    original: &[Scene<CliSequenceData>],
    current: &[Scene<CliSequenceData>],
    config: &CliSequenceConfig,
    reason: &str,
) -> Result<Vec<anyhow::Error>> {
    Ok(inspect_changes(original, current, config, reason)?.2)
}

/// Changed old/new filenames, including every shifted index. Result-only
/// resets deliberately do not count as scene edits.
pub fn affected_indices(
    original: &[Scene<CliSequenceData>],
    current: &[Scene<CliSequenceData>],
) -> Vec<usize> {
    (0..original.len().max(current.len()))
        .filter(|&index| match (original.get(index), current.get(index)) {
            (Some(before), Some(after)) => {
                !same_mapping(before, after) || before.encoder != after.encoder
            },
            _ => true,
        })
        .collect()
}

fn same_mapping(before: &Scene<CliSequenceData>, after: &Scene<CliSequenceData>) -> bool {
    before.start_frame == after.start_frame
        && before.end_frame == after.end_frame
        && before.sub_scenes == after.sub_scenes
}

/// Stage withdrawn results and the persisted PE barrier before publishing the
/// candidate. No physical files are removed or certified by this operation.
pub fn invalidate_changed_results(
    original: &[Scene<CliSequenceData>],
    current: &mut [Scene<CliSequenceData>],
    config: &CliSequenceConfig,
    input: &andean_condor::models::input::Input,
    reason: &str,
) -> Result<Vec<anyhow::Error>> {
    prepare_noise(original, current, config, input)?;
    let (blocked, old_claims, warnings) = inspect_changes(original, current, config, reason)?;
    for index in affected_indices(original, current) {
        let Some(scene) = current.get_mut(index) else {
            continue;
        };
        // ponytail: O(n²) range matching; use an interval lookup if scene lists
        // become huge. File-only old claims must follow split/moved ranges too.
        let inherited_artifact = old_claims.iter().any(|&old_index| {
            original.get(old_index).is_some_and(|before| {
                before.start_frame < scene.end_frame && scene.start_frame < before.end_frame
            })
        });
        if !original.iter().any(|before| same_mapping(before, scene)) {
            scene.sequence_data.scene_detection = Default::default();
        }
        let data = &mut scene.sequence_data;
        let already_invalid = data.parallel_encoder.invalidated;
        data.target_quality = Default::default();
        data.parallel_encoder = Default::default();
        data.parallel_encoder.invalidated =
            already_invalid || blocked.contains(&index) || inherited_artifact;
        data.quality_check = Default::default();
    }
    Ok(warnings)
}

fn prepare_noise(
    original: &[Scene<CliSequenceData>],
    current: &mut [Scene<CliSequenceData>],
    config: &CliSequenceConfig,
    input: &andean_condor::models::input::Input,
) -> Result<()> {
    use andean_condor::models::sequence::noise_scaler::scaling_context;
    let source = serde_json::to_value(input)?;
    let detector = serde_json::to_value(&config.noise_detector)?;
    // Input/detector settings govern the whole maximum-noise population, even
    // unscaled peers. Original proofs survive explicit staged scaling resets.
    let source_changed = original
        .iter()
        .chain(current.iter())
        .filter_map(|scene| scene.sequence_data.noise_scaling.as_ref()?.baseline.as_ref())
        .any(|baseline| {
            baseline.context.get("input") != Some(&source)
                || baseline.context.get("detector") != Some(&detector)
        });
    let retained_noise = current
        .iter()
        .map(|scene| {
            (!source_changed
                && original.iter().any(|before| {
                    same_mapping(before, scene)
                        && before.sequence_data.noise_detection
                            == scene.sequence_data.noise_detection
                }))
            .then(|| scene.sequence_data.noise_detection.clone())
            .flatten()
        })
        .collect::<Vec<_>>();
    let maximum_noise = retained_noise.iter().flatten().map(|data| data.noise).reduce(f64::max);
    let old_maximum = original
        .iter()
        .filter_map(|s| s.sequence_data.noise_detection.as_ref().map(|d| d.noise))
        .reduce(f64::max);
    let mut restored = Vec::new();
    for (index, scene) in current.iter().enumerate() {
        let before = original
            .get(index)
            .filter(|before| same_mapping(before, scene))
            .or_else(|| original.iter().find(|before| same_mapping(before, scene)));
        let record = scene
            .sequence_data
            .noise_scaling
            .as_ref()
            .or_else(|| before.and_then(|s| s.sequence_data.noise_scaling.as_ref()));
        let Some(record) = record else { continue };
        // An untouched legacy peer is not newly certified or reset by this edit.
        if record.baseline.is_none()
            && old_maximum == maximum_noise
            && before.is_some_and(|before| {
                before.encoder == scene.encoder
                    && before.sequence_data.noise_detection == retained_noise[index]
                    && before.sequence_data.noise_scaling == scene.sequence_data.noise_scaling
            })
        {
            continue;
        }
        let valid = before.is_some()
            && scene.sequence_data.noise_scaling.is_some()
            && record
                .baseline
                .as_ref()
                .is_some_and(|baseline| scene.encoder.photon_noise() == Some(&baseline.scaled))
            && record.baseline.as_ref().is_some_and(|baseline| {
                scaling_context(
                    input,
                    config.noise_detector.as_ref(),
                    config.noise_scaler.as_ref(),
                    scene,
                    retained_noise[index].as_ref(),
                    maximum_noise,
                ) == baseline.context
            });
        if !valid {
            let mut encoder = scene.encoder.clone();
            record.restore(&mut encoder, index)?;
            restored.push((index, encoder.photon_noise().cloned()));
        }
    }
    // No candidate changes until every required restoration has been proven.
    for (scene, noise) in current.iter_mut().zip(retained_noise) {
        scene.sequence_data.noise_detection = noise;
    }
    for (index, photon) in restored {
        current[index].encoder.set_photon_noise(photon);
        current[index].sequence_data.noise_scaling = None;
    }
    Ok(())
}

fn withdrawn_noise(before: &Scene<CliSequenceData>, current: &[Scene<CliSequenceData>]) -> bool {
    (before.sequence_data.noise_detection.is_some()
        && !current.iter().any(|after| {
            same_mapping(before, after)
                && before.sequence_data.noise_detection == after.sequence_data.noise_detection
        }))
        || (before.sequence_data.noise_scaling.is_some()
            && !current.iter().any(|after| {
                same_mapping(before, after)
                    && before.encoder.photon_noise() == after.encoder.photon_noise()
                    && before.sequence_data.noise_scaling == after.sequence_data.noise_scaling
            }))
}

fn saved_result_kinds(data: &CliSequenceData) -> Vec<&'static str> {
    let mut kinds = Vec::new();
    if !data.target_quality.passes.is_empty()
        || data.target_quality.selection.is_some()
        || data.target_quality.speed_transition.is_some()
    {
        kinds.push("target-quality results");
    }
    if data.parallel_encoder.invalidated
        || data.parallel_encoder.provenance.is_some()
        || data.parallel_encoder.started_on.is_some()
        || data.parallel_encoder.completed_on.is_some()
        || data.parallel_encoder.bytes.is_some()
    {
        kinds.push("encode results");
    }
    if !data.quality_check.quality.scores.is_empty()
        || data.quality_check.quality.started_on != 0
        || data.quality_check.quality.completed_on != 0
        || data.quality_check.quality.provenance.is_some()
        || data.quality_check.quality.backend.is_some()
    {
        kinds.push("quality-check results");
    }
    kinds
}

fn inspect_changes(
    original: &[Scene<CliSequenceData>],
    current: &[Scene<CliSequenceData>],
    config: &CliSequenceConfig,
    reason: &str,
) -> Result<(Vec<usize>, Vec<usize>, Vec<anyhow::Error>)> {
    let mut warnings = Vec::new();
    let mut blocked = Vec::new();
    let mut old_claims = Vec::new();
    let probe_directory = config.parallel_encoder.scenes_directory.join(DETAILS.name);
    let mut probe_passes = Vec::new();
    // Only inspect files if the change touched the scene layout or settings.
    let affected = affected_indices(original, current);
    if affected.is_empty() {
        return Ok((blocked, old_claims, warnings));
    }
    if probe_directory.is_dir() {
        for entry in fs::read_dir(&probe_directory)
            .with_context(|| format!("failed to inspect {}", probe_directory.display()))?
        {
            let path = entry?.path();
            if path.is_dir() {
                probe_passes.push(path);
            }
        }
    }

    for index in affected {
        let before = original.get(index);
        let after = current.get(index);
        let scene = before.or(after).expect("an existing scene at this index");
        let mut saved_results =
            before.map_or_else(Vec::new, |s| saved_result_kinds(&s.sequence_data));
        if before.is_some_and(|s| withdrawn_noise(s, current)) {
            saved_results.push("noise results");
        }
        if !saved_results.is_empty() {
            old_claims.push(index);
        }
        if let Some(scene) = after {
            for kind in saved_result_kinds(&scene.sequence_data) {
                if !saved_results.contains(&kind) {
                    saved_results.push(kind);
                }
            }
        }
        if !saved_results.is_empty() {
            blocked.push(index);
            warnings.push(anyhow::anyhow!(
                "scene index {index} ({}..{}) has saved {} that may be stale after {reason}; \
                  saved records are cleared and encoding is blocked until explicit reset in an empty work directory or a new job",
                scene.start_frame,
                scene.end_frame,
                saved_results.join(", ")
            ));
        }

        let mut files = Vec::new();
        for scene in before.into_iter().chain(after) {
            for directory in [
                &config.parallel_encoder.scenes_directory,
                &config.scene_concatenator.scenes_directory,
            ]
            .into_iter()
            .chain(probe_passes.iter())
            {
                let path = directory.join(format!(
                    "{}.{}",
                    ParallelEncoder::scene_id(index),
                    scene.encoder.output_extension()
                ));
                let present = match path.symlink_metadata() {
                    Ok(_) => true,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                    Err(error) => {
                        return Err(error)
                            .with_context(|| format!("failed to inspect {}", path.display()));
                    },
                };
                if present && !files.contains(&path) {
                    files.push(path);
                }
                if present
                    && before.is_some_and(|old| {
                        old.encoder.output_extension() == scene.encoder.output_extension()
                    })
                    && !old_claims.contains(&index)
                {
                    old_claims.push(index);
                }
            }
        }
        for path in files {
            if !blocked.contains(&index) {
                blocked.push(index);
            }
            warnings.push(anyhow::anyhow!(
                "scene index {index} has an existing encode/probe file {} that may be stale after \
                 {reason}; file was left untouched; start a new job with a different --config-file, \
                 a distinct empty --temp directory, and a distinct output",
                path.display()
            ));
        }
    }

    Ok((blocked, old_claims, warnings))
}

#[cfg(test)]
mod tests {
    use andean_condor::models::{
        encoder::{Encoder, EncoderBase},
        scene::SubScene,
    };

    use super::*;

    fn scene(start_frame: usize, end_frame: usize) -> Scene<CliSequenceData> {
        Scene {
            start_frame,
            end_frame,
            sub_scenes: None,
            encoder: Encoder::default_from_base(&EncoderBase::SVTAV1, false),
            sequence_data: CliSequenceData::default(),
        }
    }

    fn scenes() -> Vec<Scene<CliSequenceData>> {
        vec![scene(0, 121), scene(121, 240), scene(240, 480)]
    }

    fn sub_scene(start_frame: usize, end_frame: usize) -> SubScene {
        SubScene {
            start_frame,
            end_frame,
        }
    }

    #[test]
    fn scene_start_edit_keeps_neighbors_clips_subscenes_and_reports_no_overlap_for_a_gap() {
        let mut scenes = scenes();
        scenes[0].sub_scenes = Some(vec![sub_scene(10, 80), sub_scene(80, 121)]);

        let overlaps = set_start(&mut scenes, 1, 200).expect("the start frame moves");

        assert_eq!(
            scenes
                .iter()
                .map(|scene| (scene.start_frame, scene.end_frame))
                .collect::<Vec<_>>(),
            vec![(0, 121), (200, 240), (240, 480)],
            "the scene starts later and the neighbours are untouched"
        );
        assert!(
            overlaps.is_empty(),
            "opening a gap overlaps nothing: {overlaps:?}"
        );

        set_start(&mut scenes, 0, 50).expect("the start frame moves");
        assert_eq!(
            scenes[0].sub_scenes.as_deref(),
            Some(&[sub_scene(50, 80), sub_scene(80, 121)][..]),
            "the sub-scene crossing the new start is clipped"
        );
    }

    #[test]
    fn setting_a_cut_into_a_neighbour_reports_every_overlap() {
        let mut scenes = scenes();

        let overlaps = set_end(&mut scenes, 0, 300).expect("the end frame moves");

        assert_eq!(overlaps, vec![1, 2], "scenes 1 and 2 are overlapped");
        assert_eq!((scenes[0].start_frame, scenes[0].end_frame), (0, 300));

        let mut grown = self::scenes();
        let overlaps = set_start(&mut grown, 2, 100).expect("the start frame moves");
        assert_eq!(overlaps, vec![0, 1], "scenes 0 and 1 are overlapped");
    }

    #[test]
    fn frame_edits_that_would_empty_a_scene_are_rejected() {
        let mut scenes = scenes();

        let error = set_start(&mut scenes, 1, 240).expect_err("the scene would be empty");
        assert_eq!(
            error.to_string(),
            "scene 1 would be empty: start frame 240 is not before its end frame 240"
        );
        let error = set_end(&mut scenes, 1, 121).expect_err("the scene would be empty");
        assert_eq!(
            error.to_string(),
            "scene 1 would be empty: end frame 121 is not after its start frame 121"
        );
        let error = set_start(&mut scenes, 9, 10).expect_err("there are three scenes");
        assert!(
            error.to_string().contains("scene index 9 does not exist"),
            "{error}"
        );
        assert_eq!(
            scenes
                .iter()
                .map(|scene| (scene.start_frame, scene.end_frame))
                .collect::<Vec<_>>(),
            vec![(0, 121), (121, 240), (240, 480)],
            "a rejected edit leaves the list alone"
        );
    }

    #[test]
    fn scene_selections_keep_the_ranges_they_were_written_as() {
        assert_eq!(
            SceneSelection::from_str("1").expect("a single index"),
            SceneSelection::Ranges(vec![(1, 1)]),
            "a single index is the range it names"
        );
        assert_eq!(
            SceneSelection::from_str("0-4").expect("a range"),
            SceneSelection::Ranges(vec![(0, 4)]),
            "ranges are inclusive"
        );
        assert_eq!(
            SceneSelection::from_str("3,1-2,1").expect("a list"),
            SceneSelection::Ranges(vec![(1, 1), (1, 2), (3, 3)]),
            "the parts are sorted, not expanded"
        );
        assert_eq!(
            SceneSelection::from_str("1-2,8-10,16").expect("a mixed list"),
            SceneSelection::Ranges(vec![(1, 2), (8, 10), (16, 16)])
        );
        for sentinel in ["-1", "all", "every", "ALL"] {
            assert_eq!(
                SceneSelection::from_str(sentinel).expect("a sentinel"),
                SceneSelection::All,
                "{sentinel} addresses every scene"
            );
        }
    }

    #[test]
    fn scene_selections_resolve_to_ascending_duplicate_free_indices() {
        assert_eq!(
            SceneSelection::All.resolve(3).expect("three scenes"),
            vec![0, 1, 2]
        );
        assert!(
            SceneSelection::All.resolve(0).expect("no scenes").is_empty(),
            "an empty list has nothing to address"
        );
        assert_eq!(
            SceneSelection::from_str("2")
                .expect("a number")
                .resolve(3)
                .expect("three scenes"),
            vec![2]
        );
        assert_eq!(
            SceneSelection::from_str("3,1-2,1")
                .expect("a list")
                .resolve(4)
                .expect("four scenes"),
            vec![1, 2, 3],
            "repeated and unordered parts resolve once each"
        );
        assert_eq!(
            SceneSelection::from_str("1-3,2-4")
                .expect("overlapping ranges")
                .resolve(5)
                .expect("five scenes"),
            vec![1, 2, 3, 4],
            "overlapping ranges merge without repeating"
        );
    }

    #[test]
    fn scene_selections_reject_bad_syntax_and_missing_scenes() {
        let error = SceneSelection::from_str("scene 1").expect_err("not an index");
        assert!(
            error.to_string().contains("is not a scene index"),
            "{error}"
        );
        let error = SceneSelection::from_str("1,").expect_err("a trailing comma");
        assert!(
            error.to_string().contains("is not a scene index"),
            "{error}"
        );
        let error = SceneSelection::from_str("4-0").expect_err("the range runs backwards");
        assert_eq!(
            error.to_string(),
            "scene range 4-0 runs backwards: write the lower index first"
        );
        let error = SceneSelection::from_str("1,7")
            .expect("a valid list")
            .resolve(3)
            .expect_err("there are three scenes");
        assert_eq!(
            error.to_string(),
            "scene index 7 does not exist; the configuration holds 3 scenes (0..3)"
        );
    }

    #[test]
    fn a_range_past_the_end_is_rejected_without_being_expanded() {
        let error = SceneSelection::from_str(&format!("0-{}", usize::MAX))
            .expect("a range of any width parses")
            .resolve(5)
            .expect_err("only five scenes exist");
        assert_eq!(
            error.to_string(),
            "scene index 5 does not exist; the configuration holds 5 scenes (0..5)",
            "the bound is checked, not walked"
        );
        let error = SceneSelection::from_str("3-9")
            .expect("a range")
            .resolve(5)
            .expect_err("the range runs past the end");
        assert_eq!(
            error.to_string(),
            "scene index 5 does not exist; the configuration holds 5 scenes (0..5)",
            "a partly valid range names the first index that is missing"
        );
    }

    #[test]
    fn format_indices_collapses_contiguous_indices_and_keeps_gaps_comma_separated() {
        assert_eq!(format_indices(&[]), "");
        assert_eq!(format_indices(&[1]), "1");
        assert_eq!(format_indices(&[0, 1, 2, 3, 4]), "0-4");
        assert_eq!(format_indices(&[1, 3]), "1,3");
        assert_eq!(format_indices(&[1, 2, 8, 9, 10, 16]), "1-2,8-10,16");
    }

    #[test]
    fn a_new_scene_sorts_by_start_frame() {
        assert_eq!(
            sorted_insert_index(&scenes(), 0),
            0,
            "the 0 frame comes first"
        );
        assert_eq!(
            sorted_insert_index(&scenes(), 121),
            1,
            "before an equal start"
        );
        assert_eq!(sorted_insert_index(&scenes(), 200), 2);
        assert_eq!(
            sorted_insert_index(&scenes(), 9_999),
            3,
            "the end of the list"
        );
    }

    #[test]
    fn missing_scene_indices_name_the_list_size() {
        let scenes = scenes();

        let error = require_scene(&scenes, 3).expect_err("there are only three scenes");
        assert_eq!(
            error.to_string(),
            "scene index 3 does not exist; the configuration holds 3 scenes (0..3)"
        );
    }

    #[test]
    fn sub_scene_ranges_must_be_non_empty_inside_the_scene_and_ordered() {
        let scene = scene(100, 500);

        assert!(validate_sub_scenes(&scene, &[sub_scene(100, 500)]).is_ok());
        let error =
            validate_sub_scenes(&scene, &[sub_scene(200, 200)]).expect_err("the range is empty");
        assert!(error.to_string().contains("is empty"), "{error}");
        let error =
            validate_sub_scenes(&scene, &[sub_scene(99, 200)]).expect_err("it starts too early");
        assert!(
            error.to_string().contains("lies outside scene 100..500"),
            "{error}"
        );
        let error = validate_sub_scenes(&scene, &[sub_scene(300, 400), sub_scene(200, 250)])
            .expect_err("the ranges do not ascend");
        assert!(error.to_string().contains("must ascend"), "{error}");
    }

    #[test]
    fn sub_scene_ranges_parse_as_hyphenated_frames() {
        assert_eq!(
            parse_sub_scene_range("240-480").expect("the range parses"),
            sub_scene(240, 480)
        );
        let error = parse_sub_scene_range("240..480").expect_err("no hyphen");
        assert!(error.to_string().contains("START-END"), "{error}");
        let error = parse_sub_scene_range("x-480").expect_err("not a number");
        assert!(error.to_string().contains("START-END"), "{error}");
    }
}
