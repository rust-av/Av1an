use crate::utils::cancellation::Cancellation;
use std::sync;

use anyhow::ensure;
use thiserror::Error;

use crate::{
    core::{
        Condor,
        sequence::{Sequence, SequenceDetails, SequenceStatus},
    },
    models::sequence::{
        SequenceConfigHandler, SequenceDataHandler, provenance,
        speed_scaler::SpeedScalerConfigHandler,
        target_quality::{
            ProbeEncoderTransition, TargetQualityConfigHandler, TargetQualityDataHandler,
        },
    },
    utils::interpolators::{linear, natural_cubic_spline},
};

pub static DETAILS: SequenceDetails = SequenceDetails {
    name: "Convex Hull",
    description: "Applies a speed based on the quantiizer per scene.",
    version: "0.0.1",
};

#[derive(Default)]
pub struct SpeedScaler {}

impl<Data, Config> Sequence<Data, Config> for SpeedScaler
where
    Data: SequenceDataHandler + TargetQualityDataHandler,
    Config: SequenceConfigHandler + SpeedScalerConfigHandler + TargetQualityConfigHandler,
{
    #[inline]
    fn details(&self) -> SequenceDetails {
        DETAILS
    }

    #[inline]
    fn validate(
        &mut self,
        condor: &mut Condor<Data, Config>,
    ) -> anyhow::Result<((), Vec<anyhow::Error>)> {
        let mut warnings = vec![];
        let config = condor.sequence_config.speed_scaler()?;
        config.validate(&condor.encoder)?;
        for scene in &condor.scenes {
            config.validate(&scene.encoder)?;
        }

        // Ensure scenes is not empty
        if condor.scenes.is_empty() {
            warnings.push(anyhow::Error::new(SpeedScalerError::ScenesEmpty));
        }
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
        _progress_tx: sync::mpsc::Sender<SequenceStatus>,
        cancellation: Cancellation,
    ) -> anyhow::Result<((), Vec<anyhow::Error>)> {
        super::check_sequence(&cancellation, || {
            let config = condor.sequence_config.speed_scaler()?;
            config.validate(&condor.encoder)?;
            let speed_quantizers = &config.speed_quantizers;
            if speed_quantizers.is_empty() || condor.scenes.is_empty() {
                return Ok(((), vec![]));
            }

            let mut sorted_speed_quantizers = speed_quantizers.clone();
            sorted_speed_quantizers.sort_by(|(_, q1), (_, q2)| q1.total_cmp(q2));
            let quantizers: Vec<_> = sorted_speed_quantizers.iter().map(|(_, q)| *q).collect();
            let speeds: Vec<_> =
                sorted_speed_quantizers.iter().map(|(s, _)| f64::from(*s)).collect();
            let min_speed = speeds.iter().copied().reduce(f64::min).expect("validated knots");
            let max_speed = speeds.iter().copied().reduce(f64::max).expect("validated knots");
            let last = quantizers.len() - 1;
            let mut edits = vec![];
            for (index, scene) in condor.scenes.iter().enumerate() {
                if let Some(quantizer) = scene.encoder.quantizer() {
                    ensure!(
                        quantizer.is_finite(),
                        "scene {index} quantizer must be finite"
                    );
                    config.validate(&scene.encoder)?;
                    let speed = if quantizer <= quantizers[0] {
                        speeds[0]
                    } else if quantizer >= quantizers[last] {
                        speeds[last]
                    } else if quantizers.len() == 2 {
                        linear(
                            &[quantizers[0], quantizers[1]],
                            &[speeds[0], speeds[1]],
                            quantizer,
                        )
                        .ok_or_else(|| anyhow::anyhow!("cannot interpolate scene {index} speed"))?
                    } else {
                        natural_cubic_spline(&quantizers, &speeds, quantizer).ok_or_else(|| {
                            anyhow::anyhow!("cannot interpolate scene {index} speed")
                        })?
                    };
                    ensure!(
                        speed.is_finite(),
                        "scene {index} interpolated speed must be finite"
                    );
                    let mut encoder = scene.encoder.clone();
                    // Preserve the natural curve, but never overshoot the configured speed range.
                    encoder.set_speed(speed.clamp(min_speed, max_speed).round() as i8);
                    encoder.validate_settings()?;
                    if encoder != scene.encoder {
                        edits.push((index, encoder));
                    }
                }
            }

            if edits.is_empty() {
                return Ok(((), vec![]));
            }
            // Stage the encoder and its producer-owned transition together; callbacks
            // cannot leave a certificate for a transition whose save failed.
            let mut staged = condor.scenes.clone();
            for (index, encoder) in edits {
                let scene = &condor.scenes[index];
                if condor.sequence_config.target_quality()?.is_some()
                    && !scene.sequence_data.get_target_quality()?.passes.is_empty()
                {
                    let before = provenance::probe_encoder_base(scene)?.clone();
                    staged[index].sequence_data.get_target_quality_mut()?.speed_transition =
                        Some(ProbeEncoderTransition {
                            before,
                            after: encoder.clone(),
                        });
                }
                staged[index].encoder = encoder;
            }
            let previous = std::mem::replace(&mut condor.scenes, staged);
            if let Err(error) = condor.save() {
                condor.scenes = previous;
                return Err(error);
            }

            Ok(((), vec![]))
        })
    }
}

impl SpeedScaler {
    pub const DETAILS: SequenceDetails = DETAILS;
}

#[derive(Debug, Error)]
pub enum SpeedScalerError {
    #[error("No Scenes found")]
    ScenesEmpty,
    #[error("At least 2 speed-quantizer pairs are required")]
    MinimumSpeedQuantizerPairsRequired,
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, io::Read, rc::Rc};

    use serde::Serialize;
    use serde_json::Value;

    use super::*;
    use crate::{
        core::{input::Input, output::Output},
        models::{
            encoder::{Encoder, EncoderBase},
            scene::{Scene, SubScene},
            sequence::speed_scaler::SpeedScalerConfig,
        },
    };

    #[derive(Clone, Default, Serialize)]
    struct Data(
        Vec<f64>,
        crate::models::sequence::target_quality::TargetQualityData,
    );
    impl SequenceDataHandler for Data {}
    impl TargetQualityDataHandler for Data {
        fn get_target_quality(
            &self,
        ) -> anyhow::Result<&crate::models::sequence::target_quality::TargetQualityData> {
            Ok(&self.1)
        }
        fn get_target_quality_mut(
            &mut self,
        ) -> anyhow::Result<&mut crate::models::sequence::target_quality::TargetQualityData>
        {
            Ok(&mut self.1)
        }
    }

    #[derive(Clone, Default, Serialize)]
    struct Config(
        SpeedScalerConfig,
        Option<crate::models::sequence::target_quality::TargetQualityConfig>,
    );
    impl SequenceConfigHandler for Config {}
    impl TargetQualityConfigHandler for Config {
        fn target_quality(
            &self,
        ) -> anyhow::Result<&Option<crate::models::sequence::target_quality::TargetQualityConfig>>
        {
            Ok(&self.1)
        }
        fn target_quality_mut(
            &mut self,
        ) -> anyhow::Result<&mut Option<crate::models::sequence::target_quality::TargetQualityConfig>>
        {
            Ok(&mut self.1)
        }
    }
    impl SpeedScalerConfigHandler for Config {
        fn speed_scaler(&self) -> anyhow::Result<&SpeedScalerConfig> {
            Ok(&self.0)
        }
        fn speed_scaler_mut(&mut self) -> anyhow::Result<&mut SpeedScalerConfig> {
            Ok(&mut self.0)
        }
    }

    type TestCondor = Condor<Data, Config>;
    type Saves = Rc<RefCell<Vec<Vec<u8>>>>;

    fn scene(base: EncoderBase, quantizer: Option<f64>) -> Scene<Data> {
        let mut encoder = Encoder::default_from_base(&base, true);
        encoder.set_executable(Some("speed-scaler-must-not-run".into()));
        encoder.set_speed(3);
        if let Some(q) = quantizer {
            encoder.set_quantizer(q);
        }
        Scene {
            start_frame: 12,
            end_frame: 34,
            sub_scenes: Some(vec![SubScene {
                start_frame: 12,
                end_frame: 34,
            }]),
            encoder,
            sequence_data: Data(vec![20.0, 40.0], Default::default()),
        }
    }

    fn fixture(points: Vec<(i8, f64)>, scenes: Vec<Scene<Data>>) -> (TestCondor, Saves) {
        // Header-only Rust decoder: any frame read or native encoder launch must fail.
        let reader: Box<dyn Read> = Box::new(&b"YUV4MPEG2 W2 H2 F24:1 Ip A1:1 C420\n"[..]);
        let decoder = av_decoders::Decoder::from_decoder_impl(av_decoders::DecoderImpl::Y4m(
            av_decoders::Y4mDecoder::new(reader).expect("Y4M header"),
        ))
        .expect("Rust decoder");
        let input = Input::Video {
            path: "speed-scaler-unused.y4m".into(),
            import_method: crate::models::input::ImportMethod::FFMS2 {
                index: None,
                cache_path: None,
            },
            filters: vec![],
            source_details: *decoder.get_video_details(),
            decoder,
            clip_info: None,
        };
        let saved = Saves::default();
        let callback = Rc::clone(&saved);
        let condor = Condor::new(
            input,
            Output {
                path: "speed-scaler-unused.mkv".into(),
                overwrite: false,
                tags: Default::default(),
                video_tags: Default::default(),
            },
            Encoder::default_from_base(&EncoderBase::AOM, true),
            scenes,
            Some(Config(
                SpeedScalerConfig {
                    speed_quantizers: points,
                },
                None,
            )),
            Box::new(move |data| {
                callback.borrow_mut().push(serde_json::to_vec(&data.scenes)?);
                Ok(())
            }),
        );
        (condor, saved)
    }

    fn run(condor: &mut TestCondor) -> anyhow::Result<((), Vec<anyhow::Error>)> {
        let (tx, _rx) = sync::mpsc::channel();
        SpeedScaler::default().execute(
            condor,
            tx,
            crate::utils::cancellation::Cancellation::default(),
        )
    }

    fn assert_speeds(points: Vec<(i8, f64)>, queries: &[f64], speeds: &[i8]) {
        let scenes = queries.iter().map(|q| scene(EncoderBase::SVTAV1, Some(*q))).collect();
        let (mut condor, saved) = fixture(points, scenes);
        let mut expected = condor.scenes.clone();
        for (scene, speed) in expected.iter_mut().zip(speeds) {
            scene.encoder.set_speed(*speed);
        }
        let result = run(&mut condor);
        let observed: Vec<_> = condor.scenes.iter().map(|s| s.encoder.speed()).collect();
        eprintln!("speed-scaler q={queries:?}: speeds={observed:?}; want={speeds:?}");
        result.expect("scaling succeeds");
        assert_eq!(
            serde_json::to_value(&condor.scenes).expect("scenes"),
            serde_json::to_value(&expected).expect("expected")
        );
        assert_eq!(saved.borrow().len(), 1, "one real save");
        let snapshot: Value = serde_json::from_slice(&saved.borrow()[0]).expect("saved scenes");
        assert_eq!(
            snapshot,
            serde_json::to_value(&expected).expect("expected snapshot")
        );
        run(&mut condor).expect("repeat succeeds");
        assert_eq!(saved.borrow().len(), 1, "unchanged repeat does not save");
    }

    #[test]
    fn two_points_interpolate_instead_of_using_an_endpoint() {
        assert_speeds(vec![(4, 20.0), (8, 40.0)], &[30.0, 39.0], &[6, 8]);
    }

    #[test]
    fn speed_scaling_sorts_fractional_knots_and_clamps_outside_queries() {
        assert_speeds(
            vec![(4, 40.5), (8, 20.5)],
            &[0.0, 20.5, 30.5, 39.5, 40.5, 60.0],
            &[8, 8, 6, 4, 4, 4],
        );
    }

    #[test]
    fn natural_speed_spline_rounds_estimates_and_clamps_overshoot() {
        // Natural spline at q30 is 4.875, not PCHIP's 3.75. At q10 it is -1.125.
        assert_speeds(
            vec![(0, 0.0), (0, 20.0), (12, 40.0)],
            &[10.0, 30.0],
            &[0, 5],
        );
        // Solving the natural boundary equations gives q25 = 4.8 (PCHIP 3.75).
        assert_speeds(
            vec![(0, 0.0), (0, 10.0), (0, 20.0), (12, 30.0)],
            &[15.0, 25.0],
            &[0, 5],
        );
        assert_speeds(vec![(8, 0.0), (8, 20.0), (0, 40.0)], &[10.0], &[8]);
    }

    #[test]
    fn loaded_invalid_knots_fail_validate_and_execute_without_save() {
        let mut failures = vec![];
        for points in [
            vec![(4, 20.0)],
            vec![(4, 20.0), (8, 20.0)],
            vec![(4, -0.0), (8, 0.0)],
            vec![(4, f64::NAN), (8, 40.0)],
            vec![(4, f64::INFINITY), (8, 40.0)],
            vec![(4, f64::NEG_INFINITY), (8, 40.0)],
        ] {
            let (mut condor, saved) =
                fixture(points.clone(), vec![scene(EncoderBase::AOM, Some(30.0))]);
            let before = serde_json::to_vec(&condor.scenes).expect("snapshot");
            let validation = SpeedScaler::default().validate(&mut condor);
            let execution = run(&mut condor);
            if validation.is_ok()
                || execution.is_ok()
                || serde_json::to_vec(&condor.scenes).expect("scenes") != before
                || !saved.borrow().is_empty()
            {
                failures.push(format!(
                    "{points:?}: validate={validation:?}, execute={execution:?}, saves={}",
                    saved.borrow().len()
                ));
            }
        }
        assert!(
            failures.is_empty(),
            "invalid loaded knots accepted or changed scenes: {failures:?}"
        );
    }

    #[test]
    fn later_invalid_scene_is_atomic_in_memory_and_saved_bytes() {
        for q in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let scenes =
                vec![scene(EncoderBase::AOM, Some(30.0)), scene(EncoderBase::AOM, Some(q))];
            let (mut condor, saved) = fixture(vec![(4, 20.0), (8, 40.0)], scenes);
            let before = serde_json::to_vec(&condor.scenes).expect("snapshot");
            assert!(
                run(&mut condor).is_err(),
                "nonfinite scene quantizer accepted: {q}"
            );
            assert_eq!(serde_json::to_vec(&condor.scenes).expect("scenes"), before);
            assert!(saved.borrow().is_empty());
        }
        let scenes =
            vec![scene(EncoderBase::AOM, Some(30.0)), scene(EncoderBase::RAV1E, Some(30.0))];
        let (mut condor, saved) = fixture(vec![(4, 20.0), (11, 40.0)], scenes);
        let before = serde_json::to_vec(&condor.scenes).expect("snapshot");
        assert!(
            run(&mut condor).is_err(),
            "invalid later RAV1E knot accepted"
        );
        assert_eq!(serde_json::to_vec(&condor.scenes).expect("scenes"), before);
        assert!(saved.borrow().is_empty());
    }

    #[test]
    fn nonfinite_estimate_is_rejected_without_save() {
        let (mut condor, saved) = fixture(
            vec![(4, 0.0), (8, f64::MIN_POSITIVE / 2.0), (4, 1.0)],
            vec![scene(EncoderBase::SVTAV1, Some(0.5))],
        );
        let before = serde_json::to_vec(&condor.scenes).expect("snapshot");
        assert!(
            run(&mut condor).is_err(),
            "overflowed spline estimate accepted"
        );
        assert_eq!(serde_json::to_vec(&condor.scenes).expect("scenes"), before);
        assert!(saved.borrow().is_empty());
    }

    #[test]
    fn save_error_does_not_leave_published_encoder_edits() {
        let (mut condor, saved) = fixture(
            vec![(4, 20.0), (8, 40.0)],
            vec![scene(EncoderBase::AOM, Some(30.0))],
        );
        let before = serde_json::to_vec(&condor.scenes).expect("snapshot");
        condor.save_callback = Box::new(|_| anyhow::bail!("speed-scaler failed atomic save"));
        let error = run(&mut condor).expect_err("save must fail");
        assert!(error.to_string().contains("speed-scaler failed atomic save"));
        assert_eq!(serde_json::to_vec(&condor.scenes).expect("scenes"), before);
        assert!(saved.borrow().is_empty());
    }

    #[test]
    fn disabled_empty_and_no_quantizer_do_not_save() {
        for (points, scenes) in [
            (vec![], vec![scene(EncoderBase::AOM, Some(30.0))]),
            (vec![(4, 20.0), (8, 40.0)], vec![]),
            (
                vec![(4, 20.0), (8, 40.0)],
                vec![scene(EncoderBase::AOM, None)],
            ),
        ] {
            let (mut condor, saved) = fixture(points, scenes);
            let before = serde_json::to_vec(&condor.scenes).expect("snapshot");
            run(&mut condor).expect("no work");
            assert_eq!(serde_json::to_vec(&condor.scenes).expect("scenes"), before);
            assert!(saved.borrow().is_empty());
        }
    }

    #[test]
    fn speed_scaling_uses_named_presets_and_rejects_out_of_range_codec_speeds() {
        for (base, points, query, expected) in [
            (EncoderBase::RAV1E, vec![(0, 0.0), (10, 40.0)], 20.0, "5"),
            (EncoderBase::SVTAV1, vec![(-1, 0.0), (13, 40.0)], 20.0, "6"),
            (
                EncoderBase::X264,
                vec![(-5, 0.0), (20, 40.0)],
                0.0,
                "placebo",
            ),
            (
                EncoderBase::X265,
                vec![(-5, 0.0), (20, 40.0)],
                40.0,
                "ultrafast",
            ),
            (
                EncoderBase::VVenC,
                vec![(-5, 0.0), (20, 40.0)],
                40.0,
                "faster",
            ),
        ] {
            let (mut condor, saved) = fixture(points, vec![scene(base, Some(query))]);
            run(&mut condor).expect("known valid settings");
            assert_eq!(condor.scenes[0].encoder.speed().as_deref(), Some(expected));
            assert_eq!(saved.borrow().len(), 1);
        }
        for (base, speed) in [
            (EncoderBase::RAV1E, -1),
            (EncoderBase::RAV1E, 11),
            (EncoderBase::SVTAV1, -2),
            (EncoderBase::SVTAV1, 14),
        ] {
            let (mut condor, saved) =
                fixture(vec![(4, 0.0), (speed, 40.0)], vec![scene(base, Some(20.0))]);
            let before = serde_json::to_vec(&condor.scenes).expect("snapshot");
            assert!(
                run(&mut condor).is_err(),
                "invalid {base:?} knot {speed} accepted"
            );
            assert_eq!(serde_json::to_vec(&condor.scenes).expect("scenes"), before);
            assert!(saved.borrow().is_empty());
        }
    }
}
