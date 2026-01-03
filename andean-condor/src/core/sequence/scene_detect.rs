use std::{
    error::Error,
    sync::{
        self,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
        Mutex,
    },
};

use anyhow::{Ok, Result};
use av_scenechange::{detect_scene_changes, DetectionOptions};
use thiserror::Error;
use tracing::{debug, trace};

use crate::{
    core::{
        input::Input,
        sequence::{Sequence, SequenceCompletion, SequenceDetails, SequenceStatus, Status},
        Condor,
    },
    models::{
        scene::Scene,
        sequence::{
            scene_detect::{
                SceneDetectDataHandler,
                SceneDetectionMethod,
                ScenecutMethod,
                ScenecutScore,
                DEFAULT_MAX_SCENE_LENGTH_SECONDS,
            },
            SequenceConfigHandler,
            SequenceDataHandler,
        },
    },
};

static DETAILS: SequenceDetails = SequenceDetails {
    name:        "Scene Detector",
    description: "Detect scene changes",
    version:     "0.0.1-A",
};

pub struct SceneDetector {
    pub method: SceneDetectionMethod,
    pub input:  Option<Input>,
}

impl<DataHandler, ConfigHandler> Sequence<DataHandler, ConfigHandler> for SceneDetector
where
    DataHandler: SequenceDataHandler + SceneDetectDataHandler,
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
    ) -> Result<((), Vec<Box<dyn Error>>)> {
        if let Some(input) = &self.input {
            Input::validate(&input.as_data())?;
        }

        Ok(((), vec![]))
    }

    #[inline]
    fn initialize(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        progress_tx: sync::mpsc::Sender<SequenceStatus>,
    ) -> Result<((), Vec<Box<dyn Error>>)> {
        let mut warnings: Vec<Box<dyn Error>> = vec![];

        // Getting clip_info may be a long running process, so do it after data/config
        // validation but before execution
        if let Some(input) = &mut self.input {
            progress_tx.send(SequenceStatus::Whole(Status::Processing {
                id:         Self::DETAILS.name.to_owned(),
                completion: SequenceCompletion::Custom {
                    name:      "Indexing Input".to_owned(),
                    completed: 0.0,
                    total:     1.0,
                },
            }))?;
            let input_frames = input.clip_info()?.num_frames;
            progress_tx.send(SequenceStatus::Whole(Status::Completed {
                id: Self::DETAILS.name.to_owned(),
            }))?;
            let condor_input_frames = condor.input.clip_info()?.num_frames;
            if input_frames != condor_input_frames {
                warnings.push(Box::new(SceneDetectorError::InputFrameMismatch(
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
    ) -> Result<((), Vec<Box<dyn Error>>)> {
        let warnings: Vec<Box<dyn Error>> = vec![];
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

            if matches!(self.method, SceneDetectionMethod::AVSceneChange { .. }) {
                trace!(
                    "Skipping {} frames by decoding with av_decoders",
                    last_scene.end_frame
                );
                let bit_depth = input.clip_info()?.format_info.as_bit_depth()?;
                let start_time = std::time::Instant::now();
                for index in 0..last_scene.end_frame {
                    if !cancelled.load(Ordering::Relaxed) {
                        if bit_depth > 8 {
                            input.decoder().read_video_frame::<u16>()?;
                        } else {
                            input.decoder().read_video_frame::<u8>()?;
                        }
                        progress_tx.send(SequenceStatus::Whole(Status::Processing {
                            id:         DETAILS.name.to_owned(),
                            completion: SequenceCompletion::Frames {
                                completed: index as u64,
                                total:     frames as u64,
                            },
                        }))?;
                    }
                }
                trace!(
                    "Skipping {} frames took {} ms",
                    last_scene.end_frame,
                    start_time.elapsed().as_millis()
                );
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
                            id:         DETAILS.name.to_owned(),
                            completion: SequenceCompletion::Frames {
                                completed: (last_frame + frames_analyzed) as u64,
                                total:     frames as u64,
                            },
                        }))
                        .expect("failed to send progres");
                    if keyframes == keyframes_count.load(Ordering::Relaxed) {
                        return;
                    }
                    keyframes_count.fetch_add(1, Ordering::Relaxed);
                    let mut scenes_lock = scenes.lock().expect("mutex should acquire lock");
                    let start = previous_end.load(Ordering::Relaxed);
                    let end = last_frame + (frames_analyzed - 1);
                    previous_end.store(end, Ordering::Relaxed);
                    trace!("New Scene detected: {} - {}", start, end);
                    scenes_lock.push(Scene {
                        start_frame: start,
                        end_frame:   end,
                        sub_scenes:  None,
                        encoder:     condor.encoder.clone(),
                        processing:  DataHandler::default(),
                    });
                    progress_tx
                        .send(SequenceStatus::Whole(Status::Processing {
                            id:         DETAILS.name.to_owned(),
                            completion: SequenceCompletion::Custom {
                                name:      "new-scene".to_owned(),
                                completed: start as f64,
                                total:     end as f64,
                            },
                        }))
                        .expect("failed to send progress");

                    let mut data = condor_data.clone();
                    data.scenes = scenes_lock.clone();
                    (condor.save_callback)(data).expect("failed to save data");
                };

                let results = if input.clip_info()?.format_info.as_bit_depth()? > 8 {
                    detect_scene_changes::<u16>(input.decoder(), options, None, Some(&cb))
                } else {
                    detect_scene_changes::<u8>(input.decoder(), options, None, Some(&cb))
                }?;

                for (start_frame, end_frame) in
                    itertools::Itertools::tuple_windows(results.scene_changes.iter().copied())
                {
                    let start_frame = start_frame + last_frame;
                    let end_frame = end_frame + last_frame;
                    let scene_scores = results
                        .scores
                        .iter()
                        .filter(|(frame_index, _score)| {
                            *frame_index >= &start_frame && *frame_index < &end_frame
                        })
                        .map(|(frame_index, score)| {
                            (*frame_index, ScenecutScore::from_scenecutresult(score))
                        })
                        .collect();

                    let mut scene = Scene {
                        encoder: condor.encoder.clone(),
                        start_frame,
                        end_frame,
                        sub_scenes: None,
                        processing: DataHandler::default(),
                    };

                    // Update with non-default scenecut scores
                    scene.processing.get_scene_detection_mut()?.scenecut_scores =
                        Some(scene_scores);

                    condor.scenes.push(scene);
                }

                if let Some(last_scene) = condor.scenes.last()
                    && last_scene.end_frame < frames
                {
                    // Insert final scene
                    let scene = Scene {
                        encoder:     condor.encoder.clone(),
                        start_frame: last_scene.end_frame,
                        end_frame:   frames,
                        sub_scenes:  None,
                        processing:  DataHandler::default(),
                    };
                    condor.scenes.push(scene.clone());
                    progress_tx.send(SequenceStatus::Whole(Status::Processing {
                        id:         DETAILS.name.to_owned(),
                        completion: SequenceCompletion::Custom {
                            name:      "new-scene".to_owned(),
                            completed: scene.start_frame as f64,
                            total:     scene.end_frame as f64,
                        },
                    }))?;
                    progress_tx.send(SequenceStatus::Whole(Status::Processing {
                        id:         DETAILS.name.to_owned(),
                        completion: SequenceCompletion::Frames {
                            completed: frames as u64,
                            total:     frames as u64,
                        },
                    }))?;
                }

                condor.save()?;
            },
            SceneDetectionMethod::None {
                maximum_length, ..
            } => {
                debug!(
                    "Scene Detection Disabled. Splitting into {} frame chunks",
                    maximum_length
                );
                let mut scenes_vec = Vec::new();
                let mut prev_end = condor.scenes.last().map_or(0, |scene| scene.end_frame);
                while prev_end < frames {
                    let end = (prev_end + maximum_length).min(frames);
                    let scene = Scene {
                        start_frame: prev_end,
                        end_frame:   end,
                        sub_scenes:  None,
                        encoder:     condor.encoder.clone(),
                        processing:  DataHandler::default(),
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
            // started_on: None,
            // completed_on: None,
        }
    }

    #[inline]
    pub fn with_input(input: Input, method: Option<SceneDetectionMethod>) -> Result<Self> {
        let mut sd = Self {
            method: method.unwrap_or_default(),
            input:  Some(input),
            // started_on:   None,
            // completed_on: None,
        };

        // Set maximum scene length in frames based on input framerate
        if method.is_none()
            && let Some(input) = &mut sd.input
        {
            let fps_ratio: av_format::rational::Ratio<i64> = input.clip_info()?.frame_rate;
            let fps = *fps_ratio.numer() as f64 / *fps_ratio.denom() as f64;
            let max_scene_length_frames =
                (fps * DEFAULT_MAX_SCENE_LENGTH_SECONDS as f64).round() as u64;

            if let SceneDetectionMethod::AVSceneChange {
                minimum_length,
                method,
                ..
            } = sd.method
            {
                sd.method = SceneDetectionMethod::AVSceneChange {
                    minimum_length,
                    maximum_length: max_scene_length_frames as usize,
                    method,
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
