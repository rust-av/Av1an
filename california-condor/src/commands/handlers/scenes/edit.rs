//! The mutations `condor scenes` applies to the scene list: frame and
//! encoder edits, resets, removals and insertions.

use andean_condor::models::{
    encoder::{Encoder, EncoderBase, EncoderPasses},
    scene::{Scene, SubScene},
    sequence::{
        parallel_encoder::ParallelEncoderData,
        quality_check::QualityCheckData,
        scene_detector::SceneDetectorData,
        target_quality::TargetQualityData,
    },
};
use anyhow::{Result, bail};
use serde_json::Value;
use tracing::warn;

use super::{
    action::{Placement, ScenesConfigError, ScenesKey, ScenesSet},
    report::frame_range,
};
use crate::{
    commands::{
        config::Toggle,
        handlers::{
            encode::{EncodeConfigError, configure_encoder},
            warn_missing_path,
        },
    },
    configuration::{CliSequenceData, Configuration},
    scenes::{
        ScenesError,
        overlapping_scenes,
        parse_sub_scene_range,
        require_scene,
        require_scene_mut,
        set_end,
        set_start,
        sorted_insert_index,
        validate_sub_scenes,
    },
    zoner::ZonerData,
};

/// Applies one `set` property to one scene, returning the property's name and
/// the value it left behind.
pub(super) fn apply_set(
    configuration: &mut Configuration,
    index: usize,
    set: &ScenesSet,
) -> Result<(&'static str, Value)> {
    let scenes = &mut configuration.condor.scenes;

    Ok(match set {
        ScenesSet::Start {
            frame,
        } => {
            let overlaps = set_start(scenes, index, *frame)?;
            warn_about_overlaps(scenes, index, &overlaps);
            ("start_frame", frame_range(require_scene(scenes, index)?))
        },
        ScenesSet::End {
            frame,
        } => {
            let overlaps = set_end(scenes, index, *frame)?;
            warn_about_overlaps(scenes, index, &overlaps);
            ("end_frame", frame_range(require_scene(scenes, index)?))
        },
        ScenesSet::Encoder {
            encoder,
            executable,
            passes,
            params,
            iso,
            chroma_iso,
            ffmpeg_filter,
            append_color_data,
        } => {
            if chroma_iso.is_some() && iso.is_none() {
                bail!(ScenesConfigError::ChromaIsoNeedsIso);
            }
            // Capability is checked against the encoder the scene is about to
            // have, and before anything is written, so an unsupported flag
            // never reaches the file.
            let base = encoder.as_encoder_base();
            if passes.is_some() && Encoder::default_from_base(&base, false).passes_mut().is_none() {
                bail!(EncodeConfigError::PassesNotSupported(base));
            }
            if iso.is_some() && !base.supports_photon_noise() {
                bail!(EncodeConfigError::PhotonNoiseNotSupported(base));
            }
            if ffmpeg_filter.is_some() && base == EncoderBase::FFmpeg {
                bail!(EncodeConfigError::FfmpegFilterNotSupported(base));
            }
            if let Some(path) = executable {
                warn_missing_path("encoder executable", path);
            }
            let scene = require_scene_mut(scenes, index)?;
            configure_encoder(
                &mut scene.encoder,
                Some(encoder),
                *passes,
                params.as_deref(),
                *iso,
                *chroma_iso,
                ffmpeg_filter.as_deref(),
                None,
            )?;
            if let Some(toggle) = append_color_data {
                scene.encoder.set_append_color_data(toggle.enabled());
            }
            if let Some(path) = executable {
                scene.encoder.set_executable(Some(path.clone()));
            }
            ("encoder", serde_json::to_value(&scene.encoder)?)
        },
        ScenesSet::Executable {
            path,
        } => {
            warn_missing_path("encoder executable", path);
            let scene = require_scene_mut(scenes, index)?;
            scene.encoder.set_executable(Some(path.clone()));
            (
                "executable",
                serde_json::to_value(scene.encoder.custom_executable())?,
            )
        },
        ScenesSet::Passes {
            value,
        } => {
            let scene = require_scene_mut(scenes, index)?;
            let base = scene.encoder.base();
            let Some(passes) = scene.encoder.passes_mut() else {
                bail!(EncodeConfigError::PassesNotSupported(base));
            };
            *passes = EncoderPasses::All(*value);
            ("passes", serde_json::to_value(scene.encoder.passes())?)
        },
        ScenesSet::Params {
            params,
        } => {
            let scene = require_scene_mut(scenes, index)?;
            configure_encoder(
                &mut scene.encoder,
                None,
                None,
                Some(params),
                None,
                None,
                None,
                None,
            )?;
            ("params", serde_json::to_value(scene.encoder.parameters())?)
        },
        ScenesSet::PhotonNoise {
            iso,
            chroma_noise,
        } => {
            let scene = require_scene_mut(scenes, index)?;
            let base = scene.encoder.base();
            if !base.supports_photon_noise() {
                bail!(EncodeConfigError::PhotonNoiseNotSupported(base));
            }
            configure_encoder(
                &mut scene.encoder,
                None,
                None,
                None,
                Some(*iso),
                *chroma_noise,
                None,
                None,
            )?;
            (
                "photon-noise",
                serde_json::to_value(scene.encoder.photon_noise())?,
            )
        },
        ScenesSet::FfmpegFilter {
            filter,
        } => {
            let scene = require_scene_mut(scenes, index)?;
            let base = scene.encoder.base();
            if base == EncoderBase::FFmpeg {
                bail!(EncodeConfigError::FfmpegFilterNotSupported(base));
            }
            configure_encoder(
                &mut scene.encoder,
                None,
                None,
                None,
                None,
                None,
                Some(filter),
                None,
            )?;
            (
                "ffmpeg-filter",
                serde_json::to_value(scene.encoder.ffmpeg_filter())?,
            )
        },
        ScenesSet::AppendColorData {
            value,
        } => {
            let enabled = value.unwrap_or(Toggle::On).enabled();
            let scene = require_scene_mut(scenes, index)?;
            scene.encoder.set_append_color_data(enabled);
            (
                "append-color-data",
                serde_json::to_value(scene.encoder.append_color_data())?,
            )
        },
        ScenesSet::SubScenes {
            ranges,
        } => {
            let sub_scenes = parse_sub_scenes(ranges)?;
            let scene = require_scene_mut(scenes, index)?;
            validate_sub_scenes(scene, sub_scenes.as_deref().unwrap_or_default())?;
            scene.sub_scenes = sub_scenes;
            ("sub-scenes", serde_json::to_value(&scene.sub_scenes)?)
        },
    })
}

/// Resets one scene to what detection would have written: the configuration's
/// encoder and no sub-scenes. A freshly detected scene carries its own encoder
/// and no sub-scenes, while its frame range and results are detection's output
/// and stay untouched.
pub(super) fn reset_scene(
    configuration: &mut Configuration,
    index: usize,
    global: &Encoder,
) -> Result<Value> {
    let scene = require_scene_mut(&mut configuration.condor.scenes, index)?;
    scene.encoder = global.clone();
    scene.sub_scenes = None;

    Ok(serde_json::to_value(&*scene)?)
}

/// Resets one key of one scene, returning the property's name and the value it
/// left behind.
pub(super) fn reset_key(
    configuration: &mut Configuration,
    index: usize,
    key: ScenesKey,
    global: &Encoder,
) -> Result<(&'static str, Value)> {
    let scene = require_scene_mut(&mut configuration.condor.scenes, index)?;

    Ok(match key {
        ScenesKey::Frames => bail!(ScenesConfigError::NoDefault("frames")),
        ScenesKey::Encoder => {
            scene.encoder = global.clone();
            ("encoder", serde_json::to_value(&scene.encoder)?)
        },
        ScenesKey::SubScenes => {
            scene.sub_scenes = None;
            ("sub-scenes", serde_json::to_value(&scene.sub_scenes)?)
        },
        ScenesKey::SceneDetection => {
            scene.sequence_data.scene_detection = SceneDetectorData::default();
            (
                "scene-detection",
                serde_json::to_value(&scene.sequence_data.scene_detection)?,
            )
        },
        ScenesKey::NoiseDetection => {
            scene.sequence_data.noise_detection = None;
            (
                "noise-detection",
                serde_json::to_value(&scene.sequence_data.noise_detection)?,
            )
        },
        ScenesKey::NoiseScaling => {
            scene.sequence_data.noise_scaling = None;
            (
                "noise-scaling",
                serde_json::to_value(&scene.sequence_data.noise_scaling)?,
            )
        },
        ScenesKey::ParallelEncoder => {
            scene.sequence_data.parallel_encoder = ParallelEncoderData::default();
            (
                "parallel-encoder",
                serde_json::to_value(&scene.sequence_data.parallel_encoder)?,
            )
        },
        ScenesKey::TargetQuality => {
            scene.sequence_data.target_quality = TargetQualityData::default();
            (
                "target-quality",
                serde_json::to_value(&scene.sequence_data.target_quality)?,
            )
        },
        ScenesKey::QualityCheck => {
            scene.sequence_data.quality_check = QualityCheckData::default();
            (
                "quality-check",
                serde_json::to_value(&scene.sequence_data.quality_check)?,
            )
        },
        ScenesKey::Zoner => {
            scene.sequence_data.zoner = ZonerData::default();
            ("zoner", serde_json::to_value(&scene.sequence_data.zoner)?)
        },
    })
}

/// Removes every addressed scene, returning them in the order they were
/// addressed.
pub(super) fn remove_scenes(
    configuration: &mut Configuration,
    indices: &[usize],
) -> Result<Vec<Value>> {
    let mut removed = Vec::with_capacity(indices.len());
    // Back to front, so removing one never shifts an index still to come.
    for &index in indices.iter().rev() {
        let scene = configuration.condor.scenes.remove(index);
        removed.push(serde_json::to_value(scene)?);
    }
    removed.reverse();

    Ok(removed)
}

/// Inserts a new scene, returning the index it took.
///
/// The scene is like a newly detected one: the configuration's encoder, no
/// sub-scenes, and no saved results.
pub(super) fn add_scene(
    configuration: &mut Configuration,
    start: usize,
    end: usize,
    placement: Placement,
) -> Result<usize> {
    if start >= end {
        bail!(ScenesError::EmptyScene {
            start,
            end
        });
    }
    let encoder = configuration.condor.encoder.clone();
    let scenes = &mut configuration.condor.scenes;
    let position = match placement {
        Placement::Sorted => sorted_insert_index(scenes, start),
        Placement::Append => scenes.len(),
        Placement::Prepend => 0,
        Placement::After(after) => {
            require_scene(scenes, after)?;
            after + 1
        },
    };
    scenes.insert(position, Scene {
        start_frame: start,
        end_frame: end,
        sub_scenes: None,
        encoder,
        sequence_data: CliSequenceData::default(),
    });
    let overlaps = overlapping_scenes(scenes, position, start, end);
    warn_about_overlaps(scenes, position, &overlaps);

    Ok(position)
}

/// Warns about each scene an edit now shares frames with.
fn warn_about_overlaps(scenes: &[Scene<CliSequenceData>], index: usize, overlaps: &[usize]) {
    let Some(scene) = scenes.get(index) else {
        return;
    };
    for &other in overlaps {
        let Some(other_scene) = scenes.get(other) else {
            continue;
        };
        warn!(
            "scene {index} ({}..{}) overlaps scene {other} ({}..{}); the pipeline would encode \
             the shared frames twice",
            scene.start_frame, scene.end_frame, other_scene.start_frame, other_scene.end_frame
        );
    }
}

/// Parses `set sub-scenes` ranges: `none` clears them, otherwise every value is
/// a `START-END` frame range checked against its scene before writing.
fn parse_sub_scenes(ranges: &[String]) -> Result<Option<Vec<SubScene>>> {
    if ranges.len() == 1 && ranges[0].eq_ignore_ascii_case("none") {
        return Ok(None);
    }
    if ranges.is_empty() {
        bail!(ScenesConfigError::SubScenesNeedRanges);
    }
    ranges
        .iter()
        .map(|range| parse_sub_scene_range(range).map_err(Into::into))
        .collect::<Result<Vec<_>>>()
        .map(Some)
}
