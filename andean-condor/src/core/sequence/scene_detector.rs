use std::{
    collections::BTreeMap,
    sync::{
        self, Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use anyhow::{Ok, Result, bail};
use av_scenechange::{DetectionOptions, ScenecutResult, detect_scene_changes};
use thiserror::Error;
use tracing::{debug, trace};

use crate::{
    core::{
        Condor,
        input::Input,
        sequence::{Sequence, SequenceCompletion, SequenceDetails, SequenceStatus, Status},
    },
    models::{
        scene::Scene,
        sequence::{
            SequenceConfigHandler, SequenceDataHandler,
            scene_detector::{
                DEFAULT_MAX_SCENE_LENGTH_SECONDS, SceneDetectionMethod, SceneDetectorData,
                SceneDetectorDataHandler, ScenecutMethod, ScenecutScore,
            },
        },
    },
};

pub static DETAILS: SequenceDetails = SequenceDetails {
    name: "Scene Detector",
    description: "Detect scene changes",
    version: "0.0.1-A",
};

mod transnetv2;

pub struct SceneDetector {
    pub method: SceneDetectionMethod,
    pub input: Option<Input>,
}

impl<DataHandler, ConfigHandler> Sequence<DataHandler, ConfigHandler> for SceneDetector
where
    DataHandler: SequenceDataHandler + SceneDetectorDataHandler,
    ConfigHandler: SequenceConfigHandler,
{
    #[inline]
    fn details(&self) -> SequenceDetails {
        DETAILS
    }

    #[inline]
    fn validate(
        &mut self,
        _condor: &mut Condor<DataHandler, ConfigHandler>,
    ) -> Result<((), Vec<anyhow::Error>)> {
        if let Some(input) = &self.input {
            Input::validate(&input.as_data())?;
        }
        self.method.validate_lengths()?;

        Ok(((), vec![]))
    }

    #[inline]
    fn initialize(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        progress_tx: sync::mpsc::Sender<SequenceStatus>,
    ) -> Result<((), Vec<anyhow::Error>)> {
        let mut warnings = vec![];

        // Getting clip_info may be a long running process, so do it after
        // data/config validation but before execution
        if let Some(input) = &mut self.input {
            progress_tx.send(SequenceStatus::Whole(Status::Processing {
                id: Self::DETAILS.name.to_owned(),
                completion: SequenceCompletion::Custom {
                    name: "Indexing Input".to_owned(),
                    completed: 0.0,
                    total: 1.0,
                },
            }))?;
            let input_frames = input.clip_info()?.num_frames;
            progress_tx.send(SequenceStatus::Whole(Status::Completed {
                id: Self::DETAILS.name.to_owned(),
            }))?;
            let condor_input_frames = condor.input.clip_info()?.num_frames;
            if input_frames != condor_input_frames {
                warnings.push(anyhow::Error::new(SceneDetectorError::InputFrameMismatch(
                    condor_input_frames,
                    input_frames,
                )));
            }
        }

        Ok(((), warnings))
    }

    #[inline]
    fn execute(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        progress_tx: sync::mpsc::Sender<SequenceStatus>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<((), Vec<anyhow::Error>)> {
        let warnings = vec![];
        let condor_data = condor.as_data();
        let input = self.input.as_mut().unwrap_or(&mut condor.input);
        let frames = input.clip_info()?.num_frames;

        // Skip if already completed
        if let Some(last_scene) = condor.scenes.last() {
            if last_scene.end_frame == frames {
                debug!("All scenes already detected");
                progress_tx.send(SequenceStatus::Whole(Status::Completed {
                    id: DETAILS.name.to_owned(),
                }))?;
                return Ok(((), warnings));
            }

            if matches!(
                self.method,
                SceneDetectionMethod::AVSceneChange { .. }
                    | SceneDetectionMethod::TransNetV2 { .. }
            ) {
                trace!("Skipping {} frames by seeking", last_scene.end_frame);
                input.decoder().seek_to_frame(last_scene.end_frame).map_err(|_| {
                    SceneDetectorError::SceneDetectionFailed("Failed to seek to frame".to_owned())
                })?;
                progress_tx.send(SequenceStatus::Whole(Status::Processing {
                    id: DETAILS.name.to_owned(),
                    completion: SequenceCompletion::Frames {
                        completed: last_scene.end_frame as u64,
                        total: frames as u64,
                    },
                }))?;
                trace!("Skipping {} frames took {} ms", last_scene.end_frame, 0);
                if cancelled.load(Ordering::Relaxed) {
                    return Ok(((), warnings));
                }
            }
        }

        match self.method {
            SceneDetectionMethod::AVSceneChange {
                minimum_length,
                maximum_length,
                method,
                save_scores,
            } => {
                let options = DetectionOptions {
                    min_scenecut_distance: Some(minimum_length),
                    analysis_speed: match method {
                        ScenecutMethod::Fast => av_scenechange::SceneDetectionSpeed::Fast,
                        ScenecutMethod::Standard => av_scenechange::SceneDetectionSpeed::Standard,
                    },
                    max_scenecut_distance: Some(maximum_length),
                    ..Default::default()
                };

                debug!(
                    "Detecting scenes with AVSceneChange {} with lengths {} - {}",
                    method, minimum_length, maximum_length
                );

                let last_frame =
                    condor.scenes.last().map(|scene| scene.end_frame).unwrap_or_default();
                let previous_end = AtomicUsize::new(last_frame);
                let keyframes_count = AtomicUsize::new(1);
                let scenes = sync::Arc::new(Mutex::new(condor.scenes.clone()));
                let cb = |frames_analyzed, keyframes| {
                    progress_tx
                        .send(SequenceStatus::Whole(Status::Processing {
                            id: DETAILS.name.to_owned(),
                            completion: SequenceCompletion::Frames {
                                completed: (last_frame + frames_analyzed) as u64,
                                total: frames as u64,
                            },
                        }))
                        .expect("failed to send progres");
                    if keyframes == keyframes_count.load(Ordering::Relaxed) {
                        return;
                    }
                    keyframes_count.fetch_add(1, Ordering::Relaxed);
                    let mut scenes_lock: sync::MutexGuard<'_, Vec<Scene<DataHandler>>> =
                        scenes.lock().expect("mutex should acquire lock");
                    let start = previous_end.load(Ordering::Relaxed);
                    let end = last_frame + (frames_analyzed - 1);
                    previous_end.store(end, Ordering::Relaxed);
                    trace!("New Scene detected: {} - {}", start, end);
                    scenes_lock.push(Scene {
                        start_frame: start,
                        end_frame: end,
                        sub_scenes: None,
                        encoder: condor.encoder.clone(),
                        sequence_data: DataHandler::default(),
                    });
                    progress_tx
                        .send(SequenceStatus::Whole(Status::Processing {
                            id: DETAILS.name.to_owned(),
                            completion: SequenceCompletion::Custom {
                                name: "new-scene".to_owned(),
                                completed: start as f64,
                                total: end as f64,
                            },
                        }))
                        .expect("failed to send progress");

                    let mut data = condor_data.clone();
                    data.scenes = scenes_lock.clone();
                    (condor.save_callback)(data).expect("failed to save data");
                };

                let results = if input.clip_info()?.format_info.as_bit_depth()? > 8 {
                    detect_scene_changes::<u16>(
                        input.decoder(),
                        options,
                        None,
                        Some(&cb),
                        Some(Arc::clone(&cancelled)),
                    )
                } else {
                    detect_scene_changes::<u8>(
                        input.decoder(),
                        options,
                        None,
                        Some(&cb),
                        Some(Arc::clone(&cancelled)),
                    )
                }?;

                let scores = &results.scores;
                append_detected_scenes(
                    condor,
                    &results.scene_changes,
                    last_frame,
                    frames,
                    &progress_tx,
                    |start, end, data| {
                        if save_scores {
                            data.scenecut_scores =
                                Some(scenecut_scores(scores, last_frame, start, end));
                        }
                    },
                    &cancelled,
                )?;
                if cancelled.load(Ordering::Relaxed) {
                    return Ok(((), warnings));
                }
            },
            SceneDetectionMethod::TransNetV2 {
                minimum_length,
                maximum_length,
                ref model_path,
                save_scores,
            } => {
                let last_frame =
                    condor.scenes.last().map(|scene| scene.end_frame).unwrap_or_default();
                debug!(
                    "Detecting scenes with TransNetV2 with lengths {} - {}",
                    minimum_length, maximum_length
                );
                let (cuts, predictions) = transnetv2::detect_cuts(
                    input,
                    frames,
                    last_frame,
                    minimum_length,
                    maximum_length,
                    model_path.as_deref(),
                    &progress_tx,
                    &cancelled,
                )?;
                let mut starts = Vec::with_capacity(cuts.len() + 1);
                starts.push(0);
                starts.extend(cuts);
                append_detected_scenes(
                    condor,
                    &starts,
                    last_frame,
                    frames,
                    &progress_tx,
                    |start, end, data| {
                        if save_scores {
                            data.transnetv2_scores =
                                Some(transnetv2_scores(&predictions, last_frame, start, end));
                        }
                    },
                    &cancelled,
                )?;
                if cancelled.load(Ordering::Relaxed) {
                    return Ok(((), warnings));
                }
            },
            SceneDetectionMethod::None { maximum_length, .. } => {
                debug!(
                    "Scene Detection Disabled. Splitting into {} frame chunks",
                    maximum_length
                );
                let mut scenes_vec = Vec::new();
                let mut prev_end = condor.scenes.last().map_or(0, |scene| scene.end_frame);
                while prev_end < frames {
                    // A zero length leaves `end` equal to `prev_end` forever, and a
                    // length near `usize::MAX` overflows. Validation rejects the first;
                    // saturating addition and this guard also cover direct callers.
                    let end = prev_end.saturating_add(maximum_length).min(frames);
                    if end <= prev_end {
                        bail!("scene detection maximum length must be at least one frame");
                    }
                    let scene = Scene {
                        start_frame: prev_end,
                        end_frame: end,
                        sub_scenes: None,
                        encoder: condor.encoder.clone(),
                        sequence_data: DataHandler::default(),
                    };
                    scenes_vec.push(scene);
                    prev_end = end;
                }

                condor.scenes.extend(scenes_vec);
                condor.save()?;
            },
        };
        progress_tx.send(SequenceStatus::Whole(Status::Completed {
            id: DETAILS.name.to_owned(),
        }))?;

        Ok(((), warnings))
    }
}

impl SceneDetector {
    pub const DETAILS: SequenceDetails = DETAILS;

    #[inline]
    pub fn new(method: SceneDetectionMethod) -> Self {
        Self {
            method,
            input: None,
        }
    }

    #[inline]
    pub fn with_input(input: Input, method: Option<SceneDetectionMethod>) -> Result<Self> {
        let method_was_given = method.is_some();
        let mut sd = Self {
            method: method.unwrap_or_default(),
            input: Some(input),
        };

        // Set maximum scene length in frames based on input framerate
        if !method_was_given && let Some(input) = &mut sd.input {
            let fps_ratio: av_format::rational::Ratio<i64> = input.clip_info()?.frame_rate;
            let fps = *fps_ratio.numer() as f64 / *fps_ratio.denom() as f64;
            let max_scene_length_frames =
                (fps * DEFAULT_MAX_SCENE_LENGTH_SECONDS as f64).round() as u64;

            if let SceneDetectionMethod::AVSceneChange {
                minimum_length,
                method,
                save_scores,
                ..
            } = sd.method
            {
                sd.method = SceneDetectionMethod::AVSceneChange {
                    minimum_length,
                    maximum_length: max_scene_length_frames as usize,
                    method,
                    save_scores,
                };
            }
        }

        Ok(sd)
    }
}

impl Default for SceneDetector {
    #[inline]
    fn default() -> Self {
        Self::new(SceneDetectionMethod::default())
    }
}

#[derive(Debug, Error)]
pub enum SceneDetectorError {
    #[error("Frame mismatch between Condor Input and Scene Detector Input: {0} != {1}")]
    InputFrameMismatch(usize, usize),
    #[error("Scene detection failed: {0}")]
    SceneDetectionFailed(String),
}

/// Per-frame scenecut scores for the absolute frame range `[start, end)`.
/// `scores` is keyed relative to this run, which resumed from `last_frame`.
fn scenecut_scores(
    scores: &BTreeMap<usize, ScenecutResult>,
    last_frame: usize,
    start: usize,
    end: usize,
) -> BTreeMap<usize, ScenecutScore> {
    scores
        .iter()
        .filter_map(|(frame_index, score)| {
            let frame = last_frame + frame_index;
            (start..end)
                .contains(&frame)
                .then_some((frame, ScenecutScore::from_scenecutresult(score)))
        })
        .collect()
}

/// Per-frame TransNetV2 probabilities for the absolute frame range
/// `[start, end)`. `predictions` is indexed relative to `last_frame`.
fn transnetv2_scores(
    predictions: &[f32],
    last_frame: usize,
    start: usize,
    end: usize,
) -> BTreeMap<usize, f32> {
    (start..end)
        .filter_map(|frame| predictions.get(frame - last_frame).map(|&score| (frame, score)))
        .collect()
}

/// An absolute `(start, end)` frame range appended as a scene.
type SceneRange = (usize, usize);

/// The absolute frame ranges a detection run appends: one per consecutive pair
/// in `starts` (scene starts relative to `last_frame`, always beginning with
/// 0), plus a final range covering the remainder up to `frames`. A cancelled
/// run has no tail range: the run stops where it stopped analysing, so frames
/// past that point are never persisted as a scene.
fn scene_ranges(
    starts: &[usize],
    last_frame: usize,
    frames: usize,
    cancelled: bool,
) -> (Vec<SceneRange>, Option<SceneRange>) {
    let interior = itertools::Itertools::tuple_windows(starts.iter().copied())
        .map(|(start, end)| (start + last_frame, end + last_frame))
        .collect();
    let last_end = starts.last().map_or(0, |&start| start + last_frame);
    let tail = (!cancelled && last_end < frames).then_some((last_end, frames));
    (interior, tail)
}

/// Appends the scenes described by `starts`, completes coverage up to `frames`,
/// reports progress, and saves. Shared by the AVSceneChange and TransNetV2
/// arms; `record_scores` writes per-scene scores onto the scene's detection
/// data when the method asked for them.
fn append_detected_scenes<DataHandler, ConfigHandler>(
    condor: &mut Condor<DataHandler, ConfigHandler>,
    starts: &[usize],
    last_frame: usize,
    frames: usize,
    progress_tx: &sync::mpsc::Sender<SequenceStatus>,
    mut record_scores: impl FnMut(usize, usize, &mut SceneDetectorData),
    cancelled: &Arc<AtomicBool>,
) -> Result<()>
where
    DataHandler: SequenceDataHandler + SceneDetectorDataHandler,
    ConfigHandler: SequenceConfigHandler,
{
    let cancelled = cancelled.load(Ordering::Relaxed);
    let (interior, tail) = scene_ranges(starts, last_frame, frames, cancelled);

    for (start_frame, end_frame) in interior {
        let mut scene = Scene {
            encoder: condor.encoder.clone(),
            start_frame,
            end_frame,
            sub_scenes: None,
            sequence_data: DataHandler::default(),
        };
        record_scores(
            start_frame,
            end_frame,
            scene.sequence_data.get_scene_detection_mut()?,
        );
        condor.scenes.push(scene);
    }

    if cancelled {
        // Persist partial progress so a later run resumes from here.
        condor.save()?;
        return Ok(());
    }

    // No scene cuts detected: cover the remainder with a single scene.
    if let Some((start_frame, end_frame)) = tail {
        let scene = Scene {
            encoder: condor.encoder.clone(),
            start_frame,
            end_frame,
            sub_scenes: None,
            sequence_data: DataHandler::default(),
        };
        condor.scenes.push(scene.clone());
        progress_tx.send(SequenceStatus::Whole(Status::Processing {
            id: DETAILS.name.to_owned(),
            completion: SequenceCompletion::Custom {
                name: "new-scene".to_owned(),
                completed: scene.start_frame as f64,
                total: scene.end_frame as f64,
            },
        }))?;
    }
    progress_tx.send(SequenceStatus::Whole(Status::Processing {
        id: DETAILS.name.to_owned(),
        completion: SequenceCompletion::Frames {
            completed: frames as u64,
            total: frames as u64,
        },
    }))?;

    condor.save()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scenecut_result(inter_cost: f64) -> ScenecutResult {
        ScenecutResult {
            inter_cost,
            imp_block_cost: 0.0,
            backward_adjusted_cost: 0.0,
            forward_adjusted_cost: 0.0,
            threshold: 0.0,
        }
    }

    /// Regression: score indices are relative to the run, which resumes from
    /// `last_frame`; the stored keys must be absolute frame numbers.
    #[test]
    fn resumed_run_keys_scenecut_scores_by_absolute_frame() {
        let scores = BTreeMap::from([
            (0, scenecut_result(1.0)),
            (1, scenecut_result(2.0)),
            (2, scenecut_result(3.0)),
        ]);

        let stored = scenecut_scores(&scores, 100, 101, 103);

        assert_eq!(stored.keys().copied().collect::<Vec<_>>(), vec![101, 102]);
        assert_eq!(stored[&102].inter_cost, 3.0);
    }

    /// Regression: TransNetV2 predictions are indexed from the resumed frame,
    /// so stored keys must be offset by `last_frame`.
    #[test]
    fn resumed_run_keys_transnetv2_scores_by_absolute_frame() {
        let predictions = vec![0.1f32, 0.2, 0.3, 0.4];

        let stored = transnetv2_scores(&predictions, 100, 100, 104);

        assert_eq!(
            stored.keys().copied().collect::<Vec<_>>(),
            vec![100, 101, 102, 103]
        );
        assert_eq!(stored[&102], 0.3);

        // A scene starting later keeps the same absolute keys.
        let later = transnetv2_scores(&predictions, 100, 101, 103);
        assert_eq!(later.keys().copied().collect::<Vec<_>>(), vec![101, 102]);
    }

    /// Regression: a cancelled run must not persist a scene over frames it did
    /// not analyse, so it gets no tail range even though the clip continues.
    #[test]
    fn cancelled_detection_ranges_cover_only_the_analyzed_prefix() {
        // No cut was found before the run was cancelled.
        let (interior, tail) = scene_ranges(&[0], 100, 1000, true);
        assert!(interior.is_empty());
        assert_eq!(
            tail, None,
            "a cancelled run must not cover unanalysed frames"
        );

        // Cuts within the analysed prefix still persist, but nothing beyond it.
        let (interior, tail) = scene_ranges(&[0, 50, 100], 200, 1000, true);
        assert_eq!(interior, vec![(200, 250), (250, 300)]);
        assert_eq!(tail, None);
    }

    /// The same run, completed: the tail scene covers the rest of the clip.
    #[test]
    fn completed_run_covers_the_remainder() {
        let (interior, tail) = scene_ranges(&[0, 50, 100], 200, 1000, false);
        assert_eq!(interior, vec![(200, 250), (250, 300)]);
        assert_eq!(tail, Some((300, 1000)));

        // The whole clip is covered exactly once, with no gaps or overlaps.
        let mut ranges = interior;
        ranges.extend(tail);
        assert_eq!(ranges.first().map(|range| range.0), Some(200));
        assert_eq!(ranges.last().map(|range| range.1), Some(1000));
        assert!(ranges.windows(2).all(|pair| pair[0].1 == pair[1].0));
    }
}
