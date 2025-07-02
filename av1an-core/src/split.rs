#[cfg(test)]
mod tests;

use std::{
    cmp::min,
    collections::BTreeMap,
    path::Path,
    process::{Command, Stdio},
    string::ToString,
};

use av_scenechange::ScenecutResult;

use crate::scenes::Scene;

pub fn segment(input: impl AsRef<Path>, temp: impl AsRef<Path>, segments: &[usize]) {
    let input = input.as_ref();
    let temp = temp.as_ref();
    let mut cmd = Command::new("ffmpeg");

    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    cmd.args(["-hide_banner", "-y", "-i"]);
    cmd.arg(input);
    cmd.args(["-map", "0:V:0", "-an", "-c", "copy", "-avoid_negative_ts", "1", "-vsync", "0"]);

    if segments.is_empty() {
        let split_path = Path::new(temp).join("split").join("0.mkv");
        let split_str = split_path.to_str().unwrap();
        cmd.arg(split_str);
    } else {
        let segments_to_string = segments.iter().map(ToString::to_string).collect::<Vec<String>>();
        let segments_joined = segments_to_string.join(",");

        cmd.args(["-f", "segment", "-segment_frames", &segments_joined]);
        let split_path = Path::new(temp).join("split").join("%05d.mkv");
        cmd.arg(split_path);
    }
    let out = cmd.output().unwrap();
    assert!(out.status.success(), "FFmpeg failed to segment: {out:#?}");
}

pub fn extra_splits(
    scenes: &[Scene],
    split_size: usize,
    scores: &BTreeMap<usize, ScenecutResult>,
) -> Vec<Scene> {
    if scores.is_empty() {
        // This is most likely to occur when the user specifies no scene detection
        simple_extra_splits(scenes, split_size)
    } else {
        enhanced_extra_splits(scenes, split_size, scores)
    }
}

/// This is a simple, cut right in the middle method for creating extra splits.
///
/// This function assumes that `scenes` is a contiguous and sorted list
fn simple_extra_splits(scenes: &[Scene], split_size: usize) -> Vec<Scene> {
    if cfg!(debug_assertions) {
        // In debug mode, validate that `scenes` is a contiguous and sorted list
        for window in scenes.windows(2) {
            assert_eq!(window[0].end_frame, window[1].start_frame);
        }
    }

    let mut new_scenes: Vec<Scene> = Vec::with_capacity(scenes.len());

    for scene in scenes {
        let distance = scene.end_frame - scene.start_frame;
        let split_size = scene
            .zone_overrides
            .as_ref()
            .map_or(split_size, |ovr| ovr.extra_splits_len.unwrap_or(usize::MAX));
        if distance > split_size {
            let additional_splits = (distance / split_size) + 1;
            for n in 1..additional_splits {
                let new_split = (distance as f64 * (n as f64 / additional_splits as f64)) as usize
                    + scene.start_frame;
                new_scenes.push(Scene {
                    start_frame: new_scenes
                        .last()
                        .map_or(scene.start_frame, |scene| scene.end_frame),
                    end_frame: new_split,
                    ..scene.clone()
                });
            }
        }
        new_scenes.push(Scene {
            start_frame: new_scenes.last().map_or(scene.start_frame, |scene| scene.end_frame),
            end_frame: scene.end_frame,
            ..scene.clone()
        });
    }

    new_scenes
}

/// This is an enhanced extra splits method that attempts to choose the optimal
/// location for the extra split based on the scenecut scores and the sizes
/// of the resulting scenes. It tries to avoid bad extra splits without
/// making one scene that is very small compared to the other.
///
/// This function assumes that `scenes` is a contiguous and sorted list
fn enhanced_extra_splits(
    scenes: &[Scene],
    split_size: usize,
    scores: &BTreeMap<usize, ScenecutResult>,
) -> Vec<Scene> {
    if cfg!(debug_assertions) {
        // In debug mode, validate that `scenes` is a contiguous and sorted list
        for window in scenes.windows(2) {
            assert_eq!(window[0].end_frame, window[1].start_frame);
        }
    }

    let mut new_scenes: Vec<Scene> = Vec::with_capacity(scenes.len());

    for scene in scenes {
        let mut distance = scene.end_frame - scene.start_frame;
        let split_size = scene
            .zone_overrides
            .as_ref()
            .map_or(split_size, |ovr| ovr.extra_splits_len.unwrap_or(usize::MAX));
        while distance > split_size {
            let minimum_split_count = distance / split_size;
            let middle_point = distance / (minimum_split_count + 1);

            // To avoid potentially harmful decisions, limit the search range
            let min_size = middle_point / 2;
            let max_size = min(split_size, middle_point + min_size);
            let range_size = max_size - min_size;
            debug_assert!(max_size >= min_size);
            debug_assert!(max_size <= split_size);

            let start_frame = new_scenes.last().map_or(scene.start_frame, |scene| scene.end_frame);

            // A list mapping split sizes to their scores
            let split_scores = (min_size..=max_size)
                .filter_map(|size| {
                    scores.get(&(start_frame + size)).map(|scores| {
                        let inter_score = scores.inter_cost / scores.threshold;
                        let distance_from_mid = (middle_point as i64 - size as i64).abs() as f64;
                        debug_assert!(distance_from_mid as usize <= range_size);
                        // Currently the weighting is linear, starting at 1.0x
                        // in the middle and going down to 0.5x at the ends
                        let distance_weighting = 1.0 - distance_from_mid / range_size as f64;
                        let weighted_score = inter_score * distance_weighting;
                        (size, weighted_score)
                    })
                })
                .collect::<Vec<(usize, f64)>>();
            let split_point = split_scores
                .iter()
                .max_by_key(|(_, score)| {
                    // This should be enough precision.
                    // It would be great if there was a `NonNaNF64` type that implemented `Ord`.
                    (*score * 10000.0).round() as u64
                })
                .expect("split scores is not empty")
                .0;

            let new_scene = Scene {
                start_frame,
                end_frame: start_frame + split_point,
                ..scene.clone()
            };
            distance = scene.end_frame - new_scene.end_frame;
            new_scenes.push(new_scene);
        }
        new_scenes.push(Scene {
            start_frame: new_scenes.last().map_or(scene.start_frame, |scene| scene.end_frame),
            end_frame: scene.end_frame,
            ..scene.clone()
        });
    }

    new_scenes
}
