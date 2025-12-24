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

use crate::condor::{
    data::{
        processing::{
            scene_detection::{
                SceneDetectionDataHandler,
                SceneDetectionMethod,
                SceneDetectionProcessing,
                ScenecutScore,
                DEFAULT_MAX_SCENE_LENGTH_SECONDS,
            },
            BaseProcessDataTrait,
            BaseProcessingTrait,
            BaseProcessorConfigTrait,
        },
        scene::Scene,
    },
    core::{
        input::Input,
        processors::{ProcessCompletion, ProcessStatus, Processor, ProcessorDetails, Status},
        Condor,
    },
};

static DETAILS: ProcessorDetails = ProcessorDetails {
    name:        "Scene Detector",
    description: "Detect scene changes",
    version:     "0.0.1-A",
};

pub struct SceneDetector {
    pub method: SceneDetectionMethod,
    pub input:  Option<Input>,
}

impl<Processing, ProcessData, ProcessorConfig> Processor<Processing, ProcessData, ProcessorConfig>
    for SceneDetector
where
    Processing: BaseProcessingTrait + SceneDetectionProcessing,
    ProcessData: BaseProcessDataTrait + SceneDetectionDataHandler,
    ProcessorConfig: BaseProcessorConfigTrait,
{
    #[inline]
    fn details(&self) -> ProcessorDetails {
        DETAILS
    }

    #[inline]
    fn validate(
        &mut self,
        _condor: &mut Condor<ProcessData, ProcessorConfig>,
    ) -> Result<((), Vec<Box<dyn Error>>)> {
        if let Some(input) = &self.input {
            Input::validate(&input.as_data())?;
        }

        Ok(((), vec![]))
    }

    #[inline]
    fn initialize(
        &mut self,
        condor: &mut Condor<ProcessData, ProcessorConfig>,
        progress_tx: sync::mpsc::Sender<ProcessStatus>,
    ) -> Result<((), Vec<Box<dyn Error>>)> {
        let mut warnings: Vec<Box<dyn Error>> = vec![];

        // Getting clip_info may be a long running process, so do it after data/config
        // validation but before processing
        if let Some(input) = &mut self.input {
            progress_tx.send(ProcessStatus::Whole(Status::Processing {
                id:         Self::DETAILS.name.to_owned(),
                completion: ProcessCompletion::Custom {
                    name:      "Indexing Input".to_owned(),
                    completed: 0.0,
                    total:     1.0,
                },
            }))?;
            let input_frames = input.clip_info()?.num_frames;
            progress_tx.send(ProcessStatus::Whole(Status::Completed {
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
    fn process(
        &mut self,
        condor: &mut Condor<ProcessData, ProcessorConfig>,
        progress_tx: sync::mpsc::Sender<ProcessStatus>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<((), Vec<Box<dyn Error>>)> {
        let warnings: Vec<Box<dyn Error>> = vec![];
        let condor_data = condor.as_data();
        let input = self.input.as_mut().unwrap_or(&mut condor.input);
        let frames = input.clip_info()?.num_frames;

        // Skip if already completed
        if let Some(last_scene) = condor.scenes.last() {
            if last_scene.end_frame == frames {
                progress_tx.send(ProcessStatus::Whole(Status::Completed {
                    id: DETAILS.name.to_owned(),
                }))?;
                return Ok(((), warnings));
            }

            if matches!(self.method, SceneDetectionMethod::AVSceneChange { .. }) {
                // Advance the decoder to the last frame
                let bit_depth = input.clip_info()?.format_info.as_bit_depth()?;
                let start_time = std::time::Instant::now();
                for index in 0..last_scene.end_frame {
                    if !cancelled.load(Ordering::Relaxed) {
                        if bit_depth > 8 {
                            input.decoder().read_video_frame::<u16>()?;
                        } else {
                            input.decoder().read_video_frame::<u8>()?;
                        }
                        progress_tx.send(ProcessStatus::Whole(Status::Processing {
                            id:         DETAILS.name.to_owned(),
                            completion: ProcessCompletion::Frames {
                                completed: index as u64,
                                total:     frames as u64,
                            },
                        }))?;
                    }
                }
                // println!(
                //     "Skipping {} frames took {} ms",
                //     last_scene.end_frame,
                //     start_time.elapsed().as_millis()
                // );
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
                        crate::ScenecutMethod::Fast => av_scenechange::SceneDetectionSpeed::Fast,
                        crate::ScenecutMethod::Standard => {
                            av_scenechange::SceneDetectionSpeed::Standard
                        },
                    },
                    max_scenecut_distance: Some(maximum_length),
                    ..Default::default()
                };

                let last_frame =
                    condor.scenes.last().map(|scene| scene.end_frame).unwrap_or_default();
                let previous_end = AtomicUsize::new(last_frame);
                let keyframes_count = AtomicUsize::new(1);
                let scenes = sync::Arc::new(Mutex::new(condor.scenes.clone()));
                let cb = |frames_analyzed, keyframes| {
                    progress_tx
                        .send(ProcessStatus::Whole(Status::Processing {
                            id:         DETAILS.name.to_owned(),
                            completion: ProcessCompletion::Frames {
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
                    scenes_lock.push(Scene {
                        start_frame: start,
                        end_frame:   end,
                        sub_scenes:  None,
                        encoder:     condor.encoder.clone(),
                        processing:  ProcessData::default(),
                    });

                    if let Some(save_file) = condor.save_file.as_deref() {
                        let mut data = condor_data.clone();
                        data.scenes = scenes_lock.clone();
                        Condor::save_data(save_file, &data).expect("failed to save data");
                    }

                    // print!(
                    //     "\r[{}]: {} / {} frames analyzed, {} keyframes
                    // found",     DETAILS.name,
                    //     last_frame + frames_analyzed,
                    //     frames,
                    //     keyframes
                    // );
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
                        processing: ProcessData::default(),
                    };

                    // Update with non-default scenecut scores
                    scene.processing.get_scene_detection_mut()?.scenecut_scores =
                        Some(scene_scores);

                    condor.scenes.push(scene);
                }

                if let Some(last_scene) = condor.scenes.last() {
                    if last_scene.end_frame < frames {
                        // Insert final scene
                        let scene = Scene {
                            encoder:     condor.encoder.clone(),
                            start_frame: last_scene.end_frame,
                            end_frame:   frames,
                            sub_scenes:  None,
                            processing:  ProcessData::default(),
                        };
                        condor.scenes.push(scene);
                        progress_tx.send(ProcessStatus::Whole(Status::Processing {
                            id:         DETAILS.name.to_owned(),
                            completion: ProcessCompletion::Frames {
                                completed: frames as u64,
                                total:     frames as u64,
                            },
                        }))?;
                    }
                }

                condor.save()?;
            },
            SceneDetectionMethod::None {
                maximum_length, ..
            } => {
                let mut scenes_vec = Vec::new();
                let mut prev_end = condor.scenes.last().map_or(0, |scene| scene.end_frame);
                while prev_end < frames {
                    let end = (prev_end + maximum_length).min(frames);
                    let scene = Scene {
                        start_frame: prev_end,
                        end_frame:   end,
                        sub_scenes:  None,
                        encoder:     condor.encoder.clone(),
                        processing:  ProcessData::default(),
                    };
                    scenes_vec.push(scene);
                    prev_end = end;
                }

                condor.scenes.extend(scenes_vec);
                condor.save()?;
            },
        };
        progress_tx.send(ProcessStatus::Whole(Status::Completed {
            id: DETAILS.name.to_owned(),
        }))?;

        Ok(((), warnings))
    }
}

impl SceneDetector {
    pub const DETAILS: ProcessorDetails = DETAILS;
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
        if method.is_none() {
            if let Some(input) = &mut sd.input {
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

impl SceneDetectionProcessing for SceneDetector {
    #[inline]
    fn get_scene_detection_method(&self) -> Result<SceneDetectionMethod> {
        Ok(self.method)
    }

    #[inline]
    fn get_scene_detection_method_mut(&mut self) -> Result<&mut SceneDetectionMethod> {
        Ok(&mut self.method)
    }

    #[inline]
    fn get_scene_detection_input_mut(&mut self) -> Result<&mut Option<Input>> {
        Ok(&mut self.input)
    }

    #[inline]
    fn set_scene_detection_input(&mut self, input: Input) -> Result<()> {
        self.input = Some(input);
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum SceneDetectorError {
    #[error("Frame mismatch between Condor Input and Scene Detector Input: {0} != {1}")]
    InputFrameMismatch(usize, usize),
    #[error("Scene detection failed: {0}")]
    SceneDetectionFailed(String),
}
