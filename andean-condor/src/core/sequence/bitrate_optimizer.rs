use std::sync::{self, Arc, atomic::AtomicBool};

use anyhow::{Context, Result, ensure};
use thiserror::Error;
use tracing::debug;

use crate::{
    core::{
        Condor,
        sequence::{Sequence, SequenceDetails, SequenceStatus},
    },
    models::sequence::{
        SequenceConfigHandler, SequenceDataHandler,
        bitrate_optimizer::BitrateOptimizerConfigHandler, target_quality::TargetQualityDataHandler,
    },
};

pub static DETAILS: SequenceDetails = SequenceDetails {
    name: "Bitrate Optimizer",
    description: "Optimize scenes that exceed normal bitrate after targeting quality.",
    version: "0.0.1",
};

#[derive(Debug, Default)]
pub struct BitrateOptimizer {}

impl<DataHandler, ConfigHandler> Sequence<DataHandler, ConfigHandler> for BitrateOptimizer
where
    DataHandler: SequenceDataHandler + TargetQualityDataHandler,
    ConfigHandler: SequenceConfigHandler + BitrateOptimizerConfigHandler,
{
    #[inline]
    fn details(&self) -> SequenceDetails {
        DETAILS
    }

    #[inline]
    fn validate(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
    ) -> Result<((), Vec<anyhow::Error>)> {
        let warnings = vec![];

        if condor.scenes.is_empty() {
            return Err(anyhow::Error::new(BitrateOptimizerError::ScenesEmpty));
        }

        Ok(((), warnings))
    }

    #[inline]
    fn initialize(
        &mut self,
        _condor: &mut Condor<DataHandler, ConfigHandler>,
        _progress_tx: sync::mpsc::Sender<SequenceStatus>,
    ) -> Result<((), Vec<anyhow::Error>)> {
        let warnings = vec![];

        Ok(((), warnings))
    }

    #[inline]
    fn execute(
        &mut self,
        condor: &mut Condor<DataHandler, ConfigHandler>,
        _progress_tx: sync::mpsc::Sender<SequenceStatus>,
        _cancelled: Arc<AtomicBool>,
    ) -> Result<((), Vec<anyhow::Error>)> {
        let mut warnings = vec![];
        let Some(bitrate_sigma_threshold) =
            condor.sequence_config.bitrate_optimizer()?.bitrate_sigma_threshold
        else {
            return Ok(((), warnings));
        };

        if condor.scenes.is_empty() {
            warnings.push(anyhow::Error::new(BitrateOptimizerError::ScenesEmpty));
            return Ok(((), warnings));
        }

        let mut scene_values = Vec::new();
        for (index, scene) in condor.scenes.iter().enumerate() {
            let Some(quantizer) = scene.encoder.quantizer() else {
                continue;
            };
            ensure!(
                quantizer.is_finite(),
                "scene {index}: selected quantizer must be finite"
            );
            let Ok(tq) = scene.sequence_data.get_target_quality() else {
                continue;
            };
            // Resume histories may end at another q or contain an unmeasured attempt.
            let Some(pass) = tq
                .passes
                .iter()
                .rfind(|pass| pass.quantizer == quantizer && !pass.scores.is_empty())
            else {
                continue;
            };
            ensure!(
                pass.bitrate.is_finite() && pass.bitrate >= 0.0,
                "scene {index}: selected probe bitrate must be finite and nonnegative"
            );
            ensure!(
                pass.scores.iter().all(|score| score.is_finite()),
                "scene {index}: selected probe score must be finite"
            );
            scene_values.push((index, quantizer, pass.bitrate));
        }
        if scene_values.len() < 2 {
            return Ok(((), warnings));
        }

        let quantizer_average =
            scene_values.iter().map(|(_, q, _)| q).sum::<f64>() / scene_values.len() as f64;
        ensure!(
            quantizer_average.is_finite(),
            "quantizer average must be finite"
        );
        debug!("Average Quantizer: {}", quantizer_average);

        let bitrate_average =
            scene_values.iter().map(|(_, _, b)| b).sum::<f64>() / scene_values.len() as f64;
        ensure!(
            bitrate_average.is_finite(),
            "bitrate average must be finite"
        );
        let bitrate_variance = scene_values
            .iter()
            .map(|(_, _, bitrate)| (bitrate - bitrate_average).powi(2))
            .sum::<f64>()
            / scene_values.len() as f64;
        ensure!(
            bitrate_variance.is_finite(),
            "bitrate variance must be finite"
        );
        let bitrate_standard_deviation = bitrate_variance.sqrt();
        let bitrate_sigma_threshold =
            (bitrate_sigma_threshold as f64).mul_add(bitrate_standard_deviation, bitrate_average);
        ensure!(
            bitrate_sigma_threshold.is_finite(),
            "bitrate cutoff must be finite"
        );
        if bitrate_standard_deviation == 0.0 {
            return Ok(((), warnings));
        }

        let candidate = quantizer_average.round();
        let mut updates = Vec::new();
        for (index, quantizer, bitrate) in scene_values {
            if bitrate <= bitrate_sigma_threshold || quantizer >= candidate {
                continue;
            }
            // Validate every prospective encoder before publishing any scene changes.
            let mut encoder = condor.scenes[index].encoder.clone();
            encoder.set_quantizer(candidate);
            encoder.validate_settings().with_context(|| {
                format!("scene {index}: optimized quantizer {candidate} is invalid")
            })?;
            updates.push((index, encoder, bitrate));
        }
        if updates.is_empty() {
            return Ok(((), warnings));
        }

        for (index, encoder, bitrate) in updates {
            debug!(
                "Optimized Scene {}: Bitrate: {}({}), Quantizer: {}",
                index,
                bitrate,
                (bitrate - bitrate_average) / bitrate_standard_deviation,
                candidate,
            );
            condor.scenes[index].encoder = encoder;
        }

        condor.save()?;

        Ok(((), warnings))
    }
}

#[derive(Debug, Clone, Error)]
pub enum BitrateOptimizerError {
    #[error("No Scenes found")]
    ScenesEmpty,
    #[error("Previous Pass data not found")]
    PreviousPassDataNotFound,
    #[error("Failed to measure quality")]
    QualityMeasurementFailed,
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
            sequence::{
                DefaultSequenceConfig,
                target_quality::{TargetQualityData, types::QualityPass},
            },
        },
    };

    #[derive(Clone, Default, Serialize)]
    struct Data(Option<TargetQualityData>);

    impl SequenceDataHandler for Data {}

    impl TargetQualityDataHandler for Data {
        fn get_target_quality(&self) -> Result<&TargetQualityData> {
            self.0.as_ref().ok_or_else(|| anyhow::anyhow!("no target-quality data"))
        }

        fn get_target_quality_mut(&mut self) -> Result<&mut TargetQualityData> {
            self.0.as_mut().ok_or_else(|| anyhow::anyhow!("no target-quality data"))
        }
    }

    type TestCondor = Condor<Data, DefaultSequenceConfig>;
    type Snapshots = Rc<RefCell<Vec<Value>>>;

    fn measured(quantizer: f64, bitrate: f64) -> QualityPass {
        QualityPass {
            provenance: None,
            backend: None,
            quantizer,
            bitrate,
            scores: vec![80.0],
            started_on: 123,
            completed_on: 456,
        }
    }

    fn scene(base: EncoderBase, quantizer: f64, passes: Vec<QualityPass>) -> Scene<Data> {
        let mut encoder = Encoder::default_from_base(&base, false);
        encoder.set_executable(Some("bitrate-optimizer-must-not-run".into()));
        encoder.set_quantizer(quantizer);
        Scene {
            start_frame: 0,
            end_frame: 1,
            sub_scenes: Some(vec![SubScene {
                start_frame: 0,
                end_frame: 1,
            }]),
            encoder,
            sequence_data: Data(Some(TargetQualityData {
                passes,
                selection: None,
                speed_transition: None,
            })),
        }
    }

    fn fixture(mut scenes: Vec<Scene<Data>>) -> (TestCondor, Snapshots) {
        for (index, scene) in scenes.iter_mut().enumerate() {
            // Unequal durations must not change the equal-scene weighting.
            scene.start_frame = index * 100;
            scene.end_frame = scene.start_frame + index + 1;
        }
        // A header-only Rust Y4M decoder satisfies Condor without opening native input.
        let reader: Box<dyn Read> = Box::new(&b"YUV4MPEG2 W2 H2 F24:1 Ip A1:1 C420\n"[..]);
        let decoder = av_decoders::Decoder::from_decoder_impl(av_decoders::DecoderImpl::Y4m(
            av_decoders::Y4mDecoder::new(reader).expect("Y4M header"),
        ))
        .expect("Rust decoder");
        let input = Input::Video {
            path: "bitrate-optimizer-unused.y4m".into(),
            import_method: crate::models::input::ImportMethod::FFMS2 {
                index: None,
                cache_path: None,
            },
            filters: vec![],
            source_details: *decoder.get_video_details(),
            decoder,
            clip_info: None,
        };
        let snapshots = Snapshots::default();
        let saved = Rc::clone(&snapshots);
        let mut config = DefaultSequenceConfig::default();
        config.bitrate_optimizer.bitrate_sigma_threshold = Some(1);
        let condor = Condor::new(
            input,
            Output {
                path: "bitrate-optimizer-unused.mkv".into(),
                overwrite: false,
                tags: Default::default(),
                video_tags: Default::default(),
            },
            Encoder::default_from_base(&EncoderBase::AOM, false),
            scenes,
            Some(config),
            Box::new(move |data| {
                saved.borrow_mut().push(serde_json::to_value(data.scenes)?);
                Ok(())
            }),
        );
        (condor, snapshots)
    }

    fn run(condor: &mut TestCondor) -> Result<((), Vec<anyhow::Error>)> {
        let (tx, _rx) = sync::mpsc::channel();
        BitrateOptimizer::default().execute(condor, tx, Arc::new(AtomicBool::new(false)))
    }

    fn outliers() -> Vec<Scene<Data>> {
        [(20.0, 100.0, 50.0, 5000.0), (30.0, 100.0, 5.0, 1.0), (10.0, 1000.0, 60.0, 5.0)]
            .into_iter()
            .map(|(q, bitrate, other_q, other_bitrate)| {
                scene(
                    EncoderBase::AOM,
                    q,
                    vec![measured(q, bitrate), measured(other_q, other_bitrate)],
                )
            })
            .collect()
    }

    #[test]
    fn high_quantizer_outlier_is_unchanged_without_save() {
        let scenes = [(20.0, 100.0), (25.0, 100.0), (30.0, 100.0), (45.0, 1000.0)]
            .into_iter()
            .map(|(q, b)| scene(EncoderBase::AOM, q, vec![measured(q, b)]))
            .collect();
        let (mut condor, saved) = fixture(scenes);
        let before = serde_json::to_value(&condor.scenes).expect("snapshot");
        // Mean q30 must not lower the over-budget scene's selected q45.
        let (_, warnings) = run(&mut condor).expect("high-q outlier");
        let quantizers: Vec<_> =
            condor.scenes.iter().map(|scene| scene.encoder.quantizer()).collect();
        eprintln!(
            "quantizer-outlier high-q outlier: q={quantizers:?}, saves={}",
            saved.borrow().len()
        );
        assert!(warnings.is_empty());
        assert_eq!(
            serde_json::to_value(&condor.scenes).expect("unchanged"),
            before
        );
        assert!(saved.borrow().is_empty());
    }

    #[test]
    fn equal_or_quarter_step_above_mean_is_unchanged_without_save() {
        for selected in [30.0, 30.25] {
            let scenes = [(20.0, 100.0), (30.0, 100.0), (40.0, 100.0), (selected, 1000.0)]
                .into_iter()
                .map(|(q, b)| scene(EncoderBase::SVTAV1, q, vec![measured(q, b)]))
                .collect();
            let (mut condor, saved) = fixture(scenes);
            let before = serde_json::to_value(&condor.scenes).expect("snapshot");
            // Both means round to q30; an accepted quarter step must not round down.
            let (_, warnings) = run(&mut condor).expect("equal or quarter-step outlier");
            eprintln!(
                "quantizer-outlier selected q{selected}: q={:?}, saves={}",
                condor.scenes[3].encoder.quantizer(),
                saved.borrow().len()
            );
            assert!(warnings.is_empty());
            assert_eq!(
                serde_json::to_value(&condor.scenes).expect("unchanged"),
                before
            );
            assert!(saved.borrow().is_empty());
        }
    }

    #[test]
    fn mixed_outliers_only_raise_lower_quantizer_and_save_once_preserving_histories() {
        let scenes = [(20.0, 100.0), (30.0, 100.0), (40.0, 100.0), (45.0, 1000.0), (5.0, 1000.0)]
            .into_iter()
            .map(|(q, b)| {
                scene(
                    EncoderBase::AOM,
                    q,
                    vec![measured(q, b), measured(1.0, 1.0)],
                )
            })
            .collect();
        let (mut condor, saved) = fixture(scenes);
        let mut expected = condor.scenes.clone();
        // Mean q28 raises only q5, not q45; every probe and other setting stays intact.
        expected[4].encoder.set_quantizer(28.0);
        let expected = serde_json::to_value(expected).expect("expected scenes");
        let (_, warnings) = run(&mut condor).expect("mixed outliers");
        let quantizers: Vec<_> =
            condor.scenes.iter().map(|scene| scene.encoder.quantizer()).collect();
        eprintln!(
            "quantizer-outlier mixed outliers: q={quantizers:?}, saves={}",
            saved.borrow().len()
        );
        assert!(warnings.is_empty());
        assert_eq!(
            serde_json::to_value(&condor.scenes).expect("scenes and histories"),
            expected
        );
        assert_eq!(&*saved.borrow(), &[expected]);
    }

    #[test]
    fn selected_nonlast_probes_drive_statistics_and_one_real_save() {
        let (mut condor, saved) = fixture(outliers());
        let mut expected = condor.scenes.clone();
        expected[2].encoder.set_quantizer(20.0);
        let expected = serde_json::to_value(expected).expect("expected scenes");
        let (_, warnings) = run(&mut condor).expect("selected measurements");
        assert!(warnings.is_empty());
        run(&mut condor).expect("equal-bitrate remainder does not save again");
        assert_eq!(
            serde_json::to_value(&condor.scenes).expect("scenes"),
            expected
        );
        assert_eq!(&*saved.borrow(), &[expected]);
    }

    #[test]
    fn standalone_repeats_recompute_eligible_scenes_and_adjust_additional_outliers() {
        let scenes = [(20.0, 100.0), (30.0, 100.0), (10.0, 1000.0), (5.0, 500.0)]
            .into_iter()
            .map(|(q, b)| scene(EncoderBase::AOM, q, vec![measured(q, b)]))
            .collect();
        let (mut condor, saved) = fixture(scenes);
        let mut expected = condor.scenes.clone();
        let mut expected_saves = vec![];
        // Missing q16/q18 measurements shrink the next run's eligible population.
        for (index, (quantizers, changed)) in [
            ([20.0, 30.0, 16.0, 5.0], true),
            ([20.0, 30.0, 16.0, 18.0], true),
            ([20.0, 30.0, 16.0, 18.0], false),
        ]
        .into_iter()
        .enumerate()
        {
            for (scene, q) in expected.iter_mut().zip(quantizers) {
                scene.encoder.set_quantizer(q);
            }
            let (_, warnings) = run(&mut condor).expect("standalone repeat");
            assert!(warnings.is_empty());
            let observed: Vec<_> =
                condor.scenes.iter().map(|scene| scene.encoder.quantizer()).collect();
            eprintln!(
                "bitrate-optimizer standalone run {}: q={observed:?}, saves={}",
                index + 1,
                saved.borrow().len()
            );
            let snapshot = serde_json::to_value(&expected).expect("expected scenes");
            assert_eq!(
                serde_json::to_value(&condor.scenes).expect("scenes and unchanged histories"),
                snapshot
            );
            if changed {
                expected_saves.push(snapshot);
            }
            assert_eq!(&*saved.borrow(), &expected_saves);
        }
    }

    #[test]
    fn bitrate_exactly_at_cutoff_is_unchanged_without_save() {
        let scenes = [(20.0, 0.0), (30.0, 2.0)]
            .into_iter()
            .map(|(q, b)| scene(EncoderBase::AOM, q, vec![measured(q, b)]))
            .collect();
        let (mut condor, saved) = fixture(scenes);
        let before = serde_json::to_value(&condor.scenes).expect("snapshot");
        // Mean 1 + population SD 1 gives cutoff 2; only strictly higher qualifies.
        run(&mut condor).expect("exact cutoff");
        assert_eq!(
            serde_json::to_value(&condor.scenes).expect("unchanged"),
            before
        );
        assert!(saved.borrow().is_empty());
    }

    #[test]
    fn preserves_population_variance_equal_scene_weight_and_rounded_mean() {
        let scenes = [(20.0, 100.0), (30.0, 200.0), (12.0, 300.0)]
            .into_iter()
            .map(|(q, b)| scene(EncoderBase::AOM, q, vec![measured(q, b)]))
            .collect();
        let (mut condor, saved) = fixture(scenes);
        let mut expected = condor.scenes.clone();
        // Population cutoff ~281.65 admits b300; sample cutoff 300 would not.
        // Equal-scene mean q20.666 rounds to 21 despite unequal frame counts.
        expected[2].encoder.set_quantizer(21.0);
        let expected = serde_json::to_value(expected).expect("expected");
        run(&mut condor).expect("existing statistical formula");
        assert_eq!(
            serde_json::to_value(&condor.scenes).expect("scenes"),
            expected
        );
        assert_eq!(&*saved.borrow(), &[expected]);
    }

    #[test]
    fn exact_quarter_steps_use_latest_measured_match_and_keep_scene_identity() {
        let mut scenes = outliers();
        scenes.insert(0, scene(EncoderBase::AOM, 15.0, vec![measured(16.0, 1e9)]));
        scenes.insert(2, scene(EncoderBase::AOM, 15.0, vec![]));
        scenes[1] = scene(EncoderBase::SVTAV1, 20.25, vec![measured(20.25, 100.0)]);
        let mut unmeasured = measured(10.25, f64::NAN);
        unmeasured.scores.clear();
        scenes[4] = scene(
            EncoderBase::SVTAV1,
            10.25,
            vec![
                measured(10.0, 1.0),
                measured(10.25, 1.0),
                measured(10.25, 1000.0),
                measured(10.5, 1.0),
                unmeasured,
            ],
        );
        let (mut condor, saved) = fixture(scenes);
        let mut expected = condor.scenes.clone();
        // (20.25 + 30 + 10.25) / 3 rounds to 20, not a nearest-quarter fallback.
        expected[4].encoder.set_quantizer(20.0);
        let expected = serde_json::to_value(expected).expect("expected");
        run(&mut condor).expect("mixed measured scenes");
        assert_eq!(
            serde_json::to_value(&condor.scenes).expect("scenes"),
            expected
        );
        assert_eq!(&*saved.borrow(), &[expected]);
    }

    #[test]
    fn disabled_or_ineligible_bitrate_optimization_preserves_scenes_without_saving() {
        for control in [
            "empty",
            "no-q",
            "missing-data",
            "empty-history",
            "unmatched",
            "empty-scores",
            "one-eligible",
            "zero-variance",
            "zero-bitrates",
            "no-outlier",
            "unchanged-q",
            "disabled",
        ] {
            let mut scenes = outliers();
            match control {
                "empty" => scenes.clear(),
                "no-q" => {
                    for scene in &mut scenes {
                        scene.encoder.parameters_mut().remove("cq-level");
                    }
                },
                "missing-data" => {
                    for scene in &mut scenes {
                        scene.sequence_data.0 = None;
                    }
                },
                "empty-history" => {
                    for scene in &mut scenes {
                        scene.sequence_data.get_target_quality_mut().expect("data").passes.clear();
                    }
                },
                "unmatched" => {
                    for scene in &mut scenes {
                        scene.encoder.set_quantizer(15.0);
                    }
                },
                "empty-scores" => {
                    for scene in &mut scenes {
                        scene.sequence_data.get_target_quality_mut().expect("data").passes[0]
                            .scores
                            .clear();
                    }
                },
                "one-eligible" => {
                    scenes[1].sequence_data.0 = None;
                    scenes[2].encoder.set_quantizer(15.0);
                },
                "zero-variance" | "zero-bitrates" => {
                    for scene in &mut scenes {
                        scene.sequence_data.get_target_quality_mut().expect("data").passes[0]
                            .bitrate = if control == "zero-bitrates" {
                            0.0
                        } else {
                            100.0
                        };
                    }
                },
                "unchanged-q" => {
                    scenes[2].encoder.set_quantizer(25.0);
                    scenes[2].sequence_data.get_target_quality_mut().expect("data").passes[0]
                        .quantizer = 25.0;
                },
                "disabled" => scenes[0].encoder.set_quantizer(f64::NAN),
                "no-outlier" => {},
                _ => unreachable!(),
            }
            let (mut condor, saved) = fixture(scenes);
            if control == "disabled" {
                condor.sequence_config.bitrate_optimizer.bitrate_sigma_threshold = None;
            } else if control == "no-outlier" {
                condor.sequence_config.bitrate_optimizer.bitrate_sigma_threshold = Some(3);
            }
            let before = serde_json::to_value(&condor.scenes).expect("snapshot");
            run(&mut condor).unwrap_or_else(|error| panic!("{control}: {error:#}"));
            assert_eq!(
                serde_json::to_value(&condor.scenes).expect("unchanged"),
                before,
                "{control}"
            );
            assert!(saved.borrow().is_empty(), "{control}: no save for no work");
        }
    }

    #[test]
    fn corrupt_selected_measurements_fail_before_mutation_or_save() {
        for field in ["selected quantizer", "bitrate", "score"] {
            for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0] {
                if field != "bitrate" && invalid == -1.0 {
                    continue;
                }
                let mut scenes = outliers();
                // Put a valid outlier before corruption to catch partial publication.
                scenes.swap(0, 2);
                match field {
                    "selected quantizer" => scenes[2].encoder.set_quantizer(invalid),
                    "bitrate" => {
                        scenes[2].sequence_data.get_target_quality_mut().expect("data").passes[0]
                            .bitrate = invalid;
                    },
                    "score" => {
                        scenes[2].sequence_data.get_target_quality_mut().expect("data").passes[0]
                            .scores
                            .push(invalid);
                    },
                    _ => unreachable!(),
                }
                if field != "selected quantizer" {
                    // A valid older duplicate must not conceal corrupt newer measurements.
                    let passes =
                        &mut scenes[2].sequence_data.get_target_quality_mut().expect("data").passes;
                    passes.insert(0, measured(20.0, 100.0));
                }
                let (mut condor, saved) = fixture(scenes);
                let before = serde_json::to_value(&condor.scenes).expect("snapshot");
                let error = run(&mut condor).expect_err("corrupt selected measurement");
                let message = format!("{error:#}");
                assert!(
                    message.contains("scene 2") && message.contains(field),
                    "{message}"
                );
                assert_eq!(
                    serde_json::to_value(&condor.scenes).expect("unchanged"),
                    before
                );
                assert!(saved.borrow().is_empty());
                if field == "selected quantizer" {
                    assert_eq!(
                        condor.scenes[2].encoder.quantizer().expect("q").to_bits(),
                        invalid.to_bits()
                    );
                }
            }
        }
    }

    #[test]
    fn nonfinite_statistics_fail_without_mutation_or_save() {
        for field in ["quantizer average", "bitrate average", "bitrate variance"] {
            let mut scenes = outliers();
            for scene in &mut scenes {
                let pass =
                    &mut scene.sequence_data.get_target_quality_mut().expect("data").passes[0];
                match field {
                    "quantizer average" => {
                        scene.encoder.set_quantizer(f64::MAX);
                        pass.quantizer = f64::MAX;
                    },
                    "bitrate average" => pass.bitrate = f64::MAX,
                    "bitrate variance" => pass.bitrate *= 1e153,
                    _ => unreachable!(),
                }
            }
            let (mut condor, saved) = fixture(scenes);
            let before = serde_json::to_value(&condor.scenes).expect("snapshot");
            let error = run(&mut condor).expect_err("overflowing statistics");
            assert!(error.to_string().contains(field), "{error:#}");
            assert_eq!(
                serde_json::to_value(&condor.scenes).expect("unchanged"),
                before
            );
            assert!(saved.borrow().is_empty());
        }
    }

    #[test]
    fn invalid_cross_encoder_mean_rejects_all_staged_changes() {
        let scenes = vec![
            scene(EncoderBase::RAV1E, 200.0, vec![measured(200.0, 100.0)]),
            scene(EncoderBase::RAV1E, 200.0, vec![measured(200.0, 100.0)]),
            scene(EncoderBase::RAV1E, 200.0, vec![measured(200.0, 100.0)]),
            scene(EncoderBase::RAV1E, 10.0, vec![measured(10.0, 1000.0)]),
            scene(EncoderBase::AOM, 10.0, vec![measured(10.0, 1000.0)]),
        ];
        let (mut condor, saved) = fixture(scenes);
        let before = serde_json::to_value(&condor.scenes).expect("snapshot");
        // Mean q124 is valid for the first outlier (RAV1E), impossible for AOM.
        let error = run(&mut condor).expect_err("no partial encoder updates");
        let message = format!("{error:#}");
        assert!(
            message.contains("scene 4") && message.contains("cq-level"),
            "{message}"
        );
        assert_eq!(
            serde_json::to_value(&condor.scenes).expect("unchanged"),
            before
        );
        assert!(saved.borrow().is_empty());
    }
}
