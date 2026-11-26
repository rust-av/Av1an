use crate::utils::cancellation::Cancellation;
use std::{
    sync::{
        self, Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::SystemTime,
};

use thiserror::Error;

use crate::{
    core::{
        Condor,
        input::Input,
        sequence::{Sequence, SequenceCompletion, SequenceDetails, SequenceStatus, Status},
    },
    models::{
        Condor as CondorModel,
        sequence::{
            SequenceConfigHandler, SequenceDataHandler,
            noise_detector::{
                NoiseDetectorConfigHandler, NoiseDetectorData, NoiseDetectorDataHandler,
            },
        },
    },
    vapoursynth::{
        VapourSynthError, get_core,
        plugins::{
            MetricPluginFunction, PluginFunction,
            standard::{plane_stats::PlaneStats, splice::Splice, trim::Trim},
        },
    },
};

pub static DETAILS: SequenceDetails = SequenceDetails {
    name: "Noise Detector",
    description: "Measure the amount of noise of the video per scene",
    version: "0.0.1",
};

#[derive(Default)]
pub struct NoiseDetector {
    pub input: Option<Input>,
}

impl<Data, Config> Sequence<Data, Config> for NoiseDetector
where
    Data: SequenceDataHandler + NoiseDetectorDataHandler,
    Config: SequenceConfigHandler + NoiseDetectorConfigHandler,
{
    #[inline]
    fn details(&self) -> SequenceDetails {
        DETAILS
    }

    #[inline]
    fn validate(
        &mut self,
        _condor: &mut Condor<Data, Config>,
    ) -> anyhow::Result<((), Vec<anyhow::Error>)> {
        let warnings = vec![];

        Ok(((), warnings))
    }

    #[inline]
    fn initialize(
        &mut self,
        _condor: &mut Condor<Data, Config>,
        _progress_tx: sync::mpsc::Sender<SequenceStatus>,
        cancellation: &Cancellation,
    ) -> anyhow::Result<((), Vec<anyhow::Error>)> {
        super::check_sequence(cancellation, || Ok(((), Vec::new())))
    }

    #[inline]
    fn execute(
        &mut self,
        condor: &mut Condor<Data, Config>,
        progress_tx: sync::mpsc::Sender<SequenceStatus>,
        cancellation: Cancellation,
    ) -> anyhow::Result<((), Vec<anyhow::Error>)> {
        super::check_sequence(&cancellation, || {
            #[expect(deprecated, reason = "existing frame callbacks use an AtomicBool")]
            let cancelled = cancellation.legacy_flag();
            let warnings = vec![];
            let config = condor.sequence_config.noise_detector()?.clone().unwrap_or_default();
            let input_data_copy = condor.input.as_data();
            let input = self.input.as_mut().unwrap_or(&mut condor.input);

            // Owned, so it outlives the `&mut v_input` borrow taken below.
            let mut v_input_owned = input.as_vapoursynth_script()?;
            let v_input = v_input_owned.as_mut();
            let decoder = match input {
                Input::VapourSynth { decoder, .. } | Input::VapourSynthScript { decoder, .. } => {
                    decoder
                },
                Input::Video { .. } => v_input.expect("Video Input exists").decoder(),
            };
            let vapoursynth_decoder =
                decoder.get_vapoursynth_impl().expect("Decoder is VapourSynth");
            let env = &vapoursynth_decoder.env;
            let mut reference_node = vapoursynth_decoder.get_output(
                vapoursynth_decoder.get_output_index(),
                vapoursynth_decoder.get_node_modifier(),
            )?;
            let core = get_core(env)?;

            let mut denoised_node = reference_node.clone();

            for filter in &config.reference_filters {
                reference_node = filter.invoke_plugin_function(core, &reference_node)?;
            }
            for filter in &config.denoised_filters {
                denoised_node = filter.invoke_plugin_function(core, &denoised_node)?;
            }

            if cancelled.load(Ordering::Relaxed) {
                return Ok(((), warnings));
            }

            let pending: Vec<usize> = condor
                .scenes
                .iter()
                .enumerate()
                .filter_map(|(index, scene)| {
                    scene
                        .sequence_data
                        .get_noise_detection()
                        .is_ok_and(|detection| detection.is_none())
                        .then_some(index)
                })
                .collect();
            let total = condor.scenes.len() as u64;
            let base_completed = total - pending.len() as u64;

            if pending.is_empty() {
                let _ = progress_tx.send(SequenceStatus::Whole(Status::Completed {
                    id: Self::DETAILS.name.to_owned(),
                }));
                return Ok(((), warnings));
            }

            // Sample 1 frame in the middle of each scene
            let reference_node = {
                let frame_nodes: Vec<_> = pending
                    .iter()
                    .map(|&index| {
                        let scene = &condor.scenes[index];
                        let frame_index =
                            scene.start_frame + (scene.end_frame - scene.start_frame) / 2;
                        Trim {
                            first: Some(frame_index as u32),
                            last: Some(frame_index as u32),
                            ..Default::default()
                        }
                        .invoke(core, &reference_node)
                    })
                    .collect::<Result<Vec<_>, _>>()?;

                Splice::invoke(core, &frame_nodes)?
            };
            let denoised_node = {
                let frame_nodes: Vec<_> = pending
                    .iter()
                    .map(|&index| {
                        let scene = &condor.scenes[index];
                        let frame_index =
                            scene.start_frame + (scene.end_frame - scene.start_frame) / 2;
                        Trim {
                            first: Some(frame_index as u32),
                            last: Some(frame_index as u32),
                            ..Default::default()
                        }
                        .invoke(core, &denoised_node)
                    })
                    .collect::<Result<Vec<_>, _>>()?;

                Splice::invoke(core, &frame_nodes)?
            };

            let plane_stats_node = PlaneStats {
                clip_b_name: Some("denoised".to_owned()),
                plane: Some(0),
                prop: None,
            }
            .call(core, &reference_node, Some(&denoised_node))?;

            let (obsolete_progress_tx, _) = sync::mpsc::channel();

            // Accumulate scores here as frames complete possibly out of order
            let slots: Arc<Mutex<Vec<Option<f64>>>> =
                Arc::new(Mutex::new(vec![None; pending.len()]));
            let scored = Arc::new(AtomicU64::new(0));

            let frame_tx = progress_tx.clone();
            let frame_slots = Arc::clone(&slots);
            let frame_scored = Arc::clone(&scored);
            let frame_cancelled = Arc::clone(&cancelled);
            let pending_map = pending.clone();
            let res: Result<Vec<f64>, VapourSynthError> = PlaneStats::collect_frame_values(
                &plane_stats_node,
                obsolete_progress_tx,
                move |local: usize, noise: &f64| -> Result<(), VapourSynthError> {
                    let noise = *noise;
                    if let Some(slot) = frame_slots.lock().expect("noise slots lock").get_mut(local)
                    {
                        *slot = Some(noise);
                    }
                    let completed_now =
                        base_completed + frame_scored.fetch_add(1, Ordering::Relaxed) + 1;
                    let global = pending_map.get(local).copied().unwrap_or(local);
                    let _ = frame_tx.send(SequenceStatus::Subprocess {
                        parent: Status::Processing {
                            id: Self::DETAILS.name.to_owned(),
                            completion: SequenceCompletion::Custom {
                                name: Self::DETAILS.name.to_owned(),
                                completed: completed_now as f64,
                                total: total as f64,
                            },
                        },
                        child: Status::Processing {
                            id: "Noise".to_owned(),
                            completion: SequenceCompletion::SceneQuality {
                                index: global as u64,
                                quantizer: 0.0,
                                score: noise,
                                bitrate: 0.0,
                            },
                        },
                    });
                    if frame_cancelled.load(Ordering::Relaxed) {
                        return Err(PlaneStats::new_error("Cancelled".to_owned()));
                    }
                    Ok(())
                },
                move |frame| {
                    PlaneStats::PROPERTY_NAMES
                        .iter()
                        .find_map(|property_name| frame.props().get_float(property_name).ok())
                        .ok_or_else(|| {
                            PlaneStats::new_error(format!(
                                "Score not found on any of the following properties: {}",
                                PlaneStats::PROPERTY_NAMES.join(", ")
                            ))
                        })
                },
            );

            // Assign completed scores to scenes
            {
                let buf = slots.lock().expect("noise slots lock");
                for (local, slot) in buf.iter().enumerate() {
                    if let Some(noise) = slot {
                        let global = pending[local];
                        *condor.scenes[global].sequence_data.get_noise_detection_mut()? =
                            Some(NoiseDetectorData {
                                noise: *noise,
                                luminance: 0.0,
                                created_on: SystemTime::now(),
                            });
                    }
                }
            }
            let data = CondorModel {
                input: input_data_copy,
                output: condor.output.as_data(),
                encoder: condor.encoder.clone(),
                scenes: condor.scenes.clone(),
                sequence_config: condor.sequence_config.clone(),
            };
            (condor.save_callback)(data)?;

            if let Err(error) = res
                && !cancelled.load(Ordering::Relaxed)
            {
                return Err(error.into());
            }

            let _ = progress_tx.send(SequenceStatus::Whole(Status::Completed {
                id: Self::DETAILS.name.to_owned(),
            }));

            Ok(((), warnings))
        })
    }
}

impl NoiseDetector {
    pub const DETAILS: SequenceDetails = DETAILS;
}

#[derive(Debug, Error)]
pub enum NoiseDetectorError {
    #[error("Input must be VapourSynthScript")]
    InvalidInput,
}
