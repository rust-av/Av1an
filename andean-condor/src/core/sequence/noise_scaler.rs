use crate::utils::cancellation::Cancellation;
use std::sync;

use anyhow::{Context, ensure};
use tracing::debug;

use crate::{
    core::{
        Condor,
        sequence::{Sequence, SequenceDetails, SequenceStatus},
    },
    models::{
        encoder::Encoder,
        sequence::{
            SequenceConfigHandler, SequenceDataHandler,
            noise_detector::{NoiseDetectorConfigHandler, NoiseDetectorDataHandler},
            noise_scaler::{
                NoiseScalerBaseline, NoiseScalerConfigHandler, NoiseScalerData,
                NoiseScalerDataHandler, scaling_context,
            },
        },
    },
};

pub static DETAILS: SequenceDetails = SequenceDetails {
    name: "Noise Scaler",
    description: "Scales the Photon Noise per scene based on Noise Detector results.",
    version: "0.0.1",
};

#[derive(Default)]
pub struct NoiseScaler {}

impl<Data, Config> Sequence<Data, Config> for NoiseScaler
where
    Data: SequenceDataHandler + NoiseDetectorDataHandler + NoiseScalerDataHandler,
    Config: SequenceConfigHandler + NoiseScalerConfigHandler + NoiseDetectorConfigHandler,
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
        let warnings = vec![];
        if let Some(config) = condor.sequence_config.noise_scaler()? {
            config.validate()?;
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
            let warnings = vec![];
            let Some(config) = &condor.sequence_config.noise_scaler()? else {
                return Ok(((), warnings));
            };
            config.validate()?;

            if condor.scenes.is_empty() {
                return Ok(((), vec![]));
            }

            // Retain all recorded scenes in the maximum, including excluded encoders.
            let noise_levels = condor
                .scenes
                .iter()
                .enumerate()
                .map(|(index, scene)| {
                    let noise = scene
                        .sequence_data
                        .get_noise_detection()
                        .with_context(|| format!("scene {index} noise detection"))?
                        .as_ref()
                        .map(|data| data.noise);
                    if let Some(noise) = noise {
                        ensure!(
                            noise.is_finite() && noise >= 0.0,
                            "scene {index} recorded noise must be finite and non-negative"
                        );
                    }
                    Ok(noise)
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            let maximum_noise = noise_levels.iter().flatten().copied().reduce(f64::max);
            let input = condor.input.as_data();
            let detector = condor.sequence_config.noise_detector()?.as_ref();
            for (index, scene) in condor.scenes.iter().enumerate() {
                if let Some(baseline) = scene
                    .sequence_data
                    .get_noise_scaling()?
                    .as_ref()
                    .and_then(|data| data.baseline.as_ref())
                {
                    ensure!(
                        scene.encoder.photon_noise() == Some(&baseline.scaled)
                            && baseline.context
                                == scaling_context(
                                    &input,
                                    detector,
                                    Some(config),
                                    scene,
                                    scene.sequence_data.get_noise_detection()?.as_ref(),
                                    maximum_noise
                                ),
                        "scene {index} noise scaling dependencies changed; explicitly reset noise-scaling to restore its original PhotonNoise before recomputing, or use a new job"
                    );
                }
            }
            let Some(max_noise_level) = maximum_noise else {
                return Ok(((), warnings));
            };
            // No positive interval means no scaling, not a new NaN/zero/identity measurement.
            if max_noise_level <= config.threshold {
                return Ok(((), warnings));
            }

            let mut edits = vec![];
            for (index, scene) in condor.scenes.iter().enumerate() {
                if scene
                    .sequence_data
                    .get_noise_scaling()
                    .with_context(|| format!("scene {index} noise scaling"))?
                    .is_some()
                {
                    // Noise already scaled
                    continue;
                }
                let mut encoder = scene.encoder.clone();
                let scene_photon_noise = match &mut encoder {
                    Encoder::AOM { photon_noise, .. } | Encoder::RAV1E { photon_noise, .. } => {
                        photon_noise
                    },
                    Encoder::SVTAV1 {
                        options,
                        photon_noise,
                        ..
                    } => {
                        if !options.contains_key("film-grain") {
                            photon_noise
                        } else {
                            continue;
                        }
                    },
                    _ => continue,
                };
                let Some(scene_photon_noise) = scene_photon_noise else {
                    continue;
                };
                let original = scene_photon_noise.clone();
                let Some(noise_level) = noise_levels[index] else {
                    continue;
                };
                if noise_level < config.threshold {
                    continue;
                }

                let relative_noise_level =
                    (noise_level - config.threshold) / (max_noise_level - config.threshold);
                let scaler = relative_noise_level.mul_add(
                    config.maximum_scaler - config.minimum_scaler,
                    config.minimum_scaler,
                );
                ensure!(
                    relative_noise_level.is_finite() && scaler.is_finite(),
                    "scene {index} relative noise and scaler must be finite"
                );
                debug!("Scaling Scene {} Photon Noise by {:.2}", index, scaler);

                let scale_iso = |iso: u32, channel: &str| -> anyhow::Result<u32> {
                    let product = f64::from(iso) * scaler;
                    let rounded = product.round();
                    ensure!(
                        product.is_finite() && (0.0..=f64::from(u32::MAX)).contains(&rounded),
                        "scene {index} {channel} scaled ISO must be finite and round into u32"
                    );
                    Ok(rounded as u32)
                };
                scene_photon_noise.iso = scale_iso(scene_photon_noise.iso, "luma")?;
                if config.scale_chroma {
                    scene_photon_noise.chroma_iso = scene_photon_noise
                        .chroma_iso
                        .map(|iso| scale_iso(iso, "chroma"))
                        .transpose()?;
                }

                let mut data = scene.sequence_data.clone();
                *data
                    .get_noise_scaling_mut()
                    .with_context(|| format!("scene {index} noise scaling update"))? =
                    Some(NoiseScalerData {
                        scaler,
                        baseline: Some(NoiseScalerBaseline {
                            original,
                            scaled: scene_photon_noise.clone(),
                            context: scaling_context(
                                &input,
                                detector,
                                Some(config),
                                scene,
                                scene.sequence_data.get_noise_detection()?.as_ref(),
                                maximum_noise,
                            ),
                        }),
                    });
                edits.push((index, encoder, data));
            }

            if edits.is_empty() {
                return Ok(((), warnings));
            }
            // Publish only after every computation and fallible data access succeeds.
            for (index, encoder, data) in &mut edits {
                std::mem::swap(&mut condor.scenes[*index].encoder, encoder);
                std::mem::swap(&mut condor.scenes[*index].sequence_data, data);
            }
            if let Err(error) = condor.save() {
                for (index, encoder, data) in edits {
                    condor.scenes[index].encoder = encoder;
                    condor.scenes[index].sequence_data = data;
                }
                return Err(error);
            }

            Ok(((), warnings))
        })
    }
}

impl NoiseScaler {
    pub const DETAILS: SequenceDetails = DETAILS;
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, io::Read, rc::Rc, time::SystemTime};

    use serde::Serialize;

    use super::*;
    use crate::{
        core::{input::Input, output::Output},
        models::{
            encoder::{EncoderBase, photon_noise::PhotonNoise},
            scene::Scene,
            sequence::{
                DefaultSequenceConfig, noise_detector::NoiseDetectorData,
                noise_scaler::NoiseScalerConfig,
            },
        },
    };

    #[derive(Clone, Default, Serialize)]
    struct Data {
        noise: Option<NoiseDetectorData>,
        scaling: Option<NoiseScalerData>,
        fail: u8,
    }
    impl SequenceDataHandler for Data {}
    impl NoiseDetectorDataHandler for Data {
        fn get_noise_detection(&self) -> anyhow::Result<&Option<NoiseDetectorData>> {
            anyhow::ensure!(self.fail != 1, "noise-scaler detection access failed");
            Ok(&self.noise)
        }
        fn get_noise_detection_mut(&mut self) -> anyhow::Result<&mut Option<NoiseDetectorData>> {
            Ok(&mut self.noise)
        }
    }
    impl NoiseScalerDataHandler for Data {
        fn get_noise_scaling(&self) -> anyhow::Result<&Option<NoiseScalerData>> {
            anyhow::ensure!(self.fail != 2, "noise-scaler scaling read failed");
            Ok(&self.scaling)
        }
        fn get_noise_scaling_mut(&mut self) -> anyhow::Result<&mut Option<NoiseScalerData>> {
            anyhow::ensure!(self.fail != 3, "noise-scaler scaling write failed");
            Ok(&mut self.scaling)
        }
    }

    type TestCondor = Condor<Data, DefaultSequenceConfig>;
    type Saves = Rc<RefCell<Vec<Vec<u8>>>>;

    fn config() -> NoiseScalerConfig {
        NoiseScalerConfig {
            threshold: 0.2,
            minimum_scaler: 0.5,
            maximum_scaler: 1.5,
            scale_chroma: true,
        }
    }

    fn scene(base: EncoderBase, noise: Option<f64>) -> Scene<Data> {
        let mut encoder = Encoder::default_from_base(&base, true);
        encoder.set_executable(Some("noise-scaler-must-not-run".into()));
        encoder.set_photon_noise(Some(PhotonNoise {
            iso: 800,
            chroma_iso: Some(400),
            width: Some(1920),
            height: Some(1080),
            c_y: Some(vec![]),
            ccb: Some(vec![1]),
            ccr: Some(vec![2]),
        }));
        Scene {
            start_frame: 12,
            end_frame: 34,
            sub_scenes: None,
            encoder,
            sequence_data: Data {
                noise: noise.map(|noise| NoiseDetectorData {
                    created_on: SystemTime::UNIX_EPOCH,
                    noise,
                    luminance: 0.3,
                }),
                ..Data::default()
            },
        }
    }

    fn fixture(config: Option<NoiseScalerConfig>, scenes: Vec<Scene<Data>>) -> (TestCondor, Saves) {
        // Header-only decoder: a frame read or native launch would fail this fixture.
        let reader: Box<dyn Read> = Box::new(&b"YUV4MPEG2 W2 H2 F24:1 Ip A1:1 C420\n"[..]);
        let decoder = av_decoders::Decoder::from_decoder_impl(av_decoders::DecoderImpl::Y4m(
            av_decoders::Y4mDecoder::new(reader).expect("Y4M header"),
        ))
        .expect("Rust decoder");
        let input = Input::Video {
            path: "noise-scaler-unused.y4m".into(),
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
                path: "noise-scaler-unused.mkv".into(),
                overwrite: false,
                tags: Default::default(),
                video_tags: Default::default(),
            },
            Encoder::default_from_base(&EncoderBase::AOM, true),
            scenes,
            Some(DefaultSequenceConfig {
                noise_scaler: config,
                ..DefaultSequenceConfig::default()
            }),
            Box::new(move |data| {
                callback.borrow_mut().push(serde_json::to_vec(&data.scenes)?);
                Ok(())
            }),
        );
        (condor, saved)
    }

    fn run(condor: &mut TestCondor) -> anyhow::Result<((), Vec<anyhow::Error>)> {
        let (tx, _rx) = sync::mpsc::channel();
        NoiseScaler::default().execute(
            condor,
            tx,
            crate::utils::cancellation::Cancellation::default(),
        )
    }

    fn snapshot(condor: &TestCondor) -> Vec<u8> {
        serde_json::to_vec(&condor.scenes).expect("scenes snapshot")
    }

    fn assert_scaled(condor: &TestCondor, original: &[Scene<Data>], expected: &[Scene<Data>]) {
        let mut actual = condor.scenes.clone();
        for ((scene, original), expected) in actual.iter_mut().zip(original).zip(expected) {
            if let Some(baseline) =
                scene.sequence_data.scaling.as_mut().and_then(|data| data.baseline.take())
            {
                assert_eq!(Some(&baseline.original), original.encoder.photon_noise());
                assert_eq!(Some(&baseline.scaled), expected.encoder.photon_noise());
            }
        }
        assert_eq!(
            serde_json::to_value(actual).expect("actual calibration"),
            serde_json::to_value(expected).expect("expected calibration")
        );
    }

    #[test]
    fn no_interval_preserves_iso_chroma_metadata_without_save() {
        for noises in [[0.2, 0.2], [0.1, 0.15], [0.0, 0.0]] {
            let scenes = noises.map(|n| scene(EncoderBase::AOM, Some(n))).to_vec();
            let (mut condor, saved) = fixture(Some(config()), scenes);
            let before = snapshot(&condor);
            run(&mut condor).expect("no interval succeeds");
            eprintln!(
                "noise-scaler no interval {noises:?}: {}; saves={}",
                String::from_utf8_lossy(&snapshot(&condor)),
                saved.borrow().len()
            );
            assert_eq!(
                snapshot(&condor),
                before,
                "no interval must preserve ISO 800/chroma 400 and metadata"
            );
            assert!(saved.borrow().is_empty());
        }
    }

    #[test]
    fn force_during_sequence_save_is_not_reported_as_success() {
        use crate::utils::cancellation::{Cancellation, Cancelled};

        let scenes = [0.1, 0.2, 0.4, 0.6].map(|n| scene(EncoderBase::AOM, Some(n))).to_vec();
        let (mut condor, saved) = fixture(Some(config()), scenes);
        let cancellation = Cancellation::default();
        let controls = cancellation.clone();
        let save = condor.save_callback;
        condor.save_callback = Box::new(move |data| {
            save(data)?;
            controls.request()?;
            controls.request()
        });
        let (tx, _rx) = sync::mpsc::channel();

        let error = NoiseScaler::default()
            .execute(&mut condor, tx, cancellation)
            .expect_err("force must be observed after cooperative work");

        assert!(matches!(
            error.downcast_ref::<Cancelled>(),
            Some(Cancelled::Forced)
        ));
        assert_eq!(saved.borrow().len(), 1);
    }

    #[test]
    fn noise_scaling_maps_recorded_noise_to_iso_and_saves_once_with_optional_chroma() {
        for scale_chroma in [true, false] {
            let mut config = config();
            config.scale_chroma = scale_chroma;
            let scenes = [0.1, 0.2, 0.4, 0.6].map(|n| scene(EncoderBase::AOM, Some(n))).to_vec();
            let (mut condor, saved) = fixture(Some(config), scenes);
            let mut expected = condor.scenes.clone();
            let original = expected.clone();
            for (s, (iso, chroma, scaler)) in expected.iter_mut().zip([
                (800, 400, None),
                (400, 200, Some(0.5)),
                (800, 400, Some(1.0)),
                (1200, 600, Some(1.5)),
            ]) {
                let mut photon = s.encoder.photon_noise().expect("photon noise").clone();
                photon.iso = iso;
                photon.chroma_iso = Some(if scale_chroma { chroma } else { 400 });
                s.encoder.set_photon_noise(Some(photon));
                s.sequence_data.scaling = scaler.map(|scaler| NoiseScalerData {
                    scaler,
                    ..Default::default()
                });
            }
            run(&mut condor).expect("ordinary scaling");
            assert_scaled(&condor, &original, &expected);
            assert_eq!(saved.borrow().as_slice(), &[snapshot(&condor)]);
            run(&mut condor).expect("already scaled repeat");
            assert_eq!(saved.borrow().len(), 1, "no repeated save");
        }
    }

    #[test]
    fn noise_scaling_uses_all_recorded_noise_but_changes_only_eligible_encoders() {
        let mut scenes = vec![
            scene(EncoderBase::AOM, Some(0.2)),
            scene(EncoderBase::RAV1E, Some(0.4)),
            scene(EncoderBase::SVTAV1, Some(0.6)),
            scene(EncoderBase::SVTAV1, Some(1.0)),
            scene(EncoderBase::AVM, Some(1.0)),
            scene(EncoderBase::AOM, Some(1.0)),
            scene(EncoderBase::AOM, Some(0.4)),
            scene(EncoderBase::AOM, None),
        ];
        if let Encoder::SVTAV1 { options, .. } = &mut scenes[3].encoder {
            options.insert(
                "film-grain".into(),
                crate::models::encoder::cli_parameter::CLIParameter::new_number("--", " ", 10.0),
            );
        }
        scenes[5].encoder.set_photon_noise(None);
        scenes[6].sequence_data.scaling = Some(NoiseScalerData {
            scaler: 9.0,
            ..Default::default()
        });
        let mut photon = scenes[1].encoder.photon_noise().expect("photon").clone();
        photon.chroma_iso = None;
        scenes[1].encoder.set_photon_noise(Some(photon));
        let mut expected = scenes.clone();
        let original = scenes.clone();
        for (s, (iso, chroma, scaler)) in expected.iter_mut().zip([
            (400, Some(200), 0.5),
            (600, None, 0.75),
            (800, Some(400), 1.0),
        ]) {
            let mut photon = s.encoder.photon_noise().expect("photon").clone();
            photon.iso = iso;
            photon.chroma_iso = chroma;
            s.encoder.set_photon_noise(Some(photon));
            s.sequence_data.scaling = Some(NoiseScalerData {
                scaler,
                ..Default::default()
            });
        }
        let (mut condor, saved) = fixture(Some(config()), scenes);
        run(&mut condor).expect("mixed encoders");
        assert_scaled(&condor, &original, &expected);
        assert_eq!(saved.borrow().as_slice(), &[snapshot(&condor)]);
    }

    #[test]
    fn disabled_empty_missing_and_already_done_do_not_save() {
        let mut done = scene(EncoderBase::AOM, Some(0.6));
        done.sequence_data.scaling = Some(NoiseScalerData {
            scaler: 1.5,
            ..Default::default()
        });
        for (config, scenes) in [
            (None, vec![scene(EncoderBase::AOM, Some(f64::NAN))]),
            (Some(config()), vec![]),
            (Some(config()), vec![scene(EncoderBase::AOM, None)]),
            (Some(config()), vec![done]),
            (Some(config()), vec![scene(EncoderBase::AVM, Some(0.6))]),
        ] {
            let (mut condor, saved) = fixture(config, scenes);
            let before = snapshot(&condor);
            run(&mut condor).expect("no work");
            assert_eq!(snapshot(&condor), before);
            assert!(saved.borrow().is_empty());
        }
    }

    #[test]
    fn loaded_invalid_config_rejected_by_validate_and_direct_execute() {
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.1] {
            for field in 0..3 {
                let mut config = config();
                match field {
                    0 => config.threshold = bad,
                    1 => config.minimum_scaler = bad,
                    _ => config.maximum_scaler = bad,
                }
                let (mut condor, saved) =
                    fixture(Some(config), vec![scene(EncoderBase::AOM, Some(0.6))]);
                let before = snapshot(&condor);
                assert!(
                    NoiseScaler::default().validate(&mut condor).is_err(),
                    "field {field}: {bad}"
                );
                assert!(run(&mut condor).is_err(), "field {field}: {bad}");
                assert_eq!(snapshot(&condor), before);
                assert!(saved.borrow().is_empty());
            }
        }
        let mut config = config();
        config.minimum_scaler = 2.0;
        let (mut condor, saved) = fixture(Some(config), vec![scene(EncoderBase::AOM, Some(0.6))]);
        assert!(NoiseScaler::default().validate(&mut condor).is_err());
        assert!(run(&mut condor).is_err());
        assert!(saved.borrow().is_empty());
    }

    #[test]
    fn late_invalid_recorded_noise_is_contextual_and_atomic() {
        for noise in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.1] {
            let scenes =
                vec![scene(EncoderBase::AOM, Some(0.4)), scene(EncoderBase::AVM, Some(noise))];
            let (mut condor, saved) = fixture(Some(config()), scenes);
            let before = snapshot(&condor);
            let result = run(&mut condor);
            eprintln!(
                "noise-scaler later noise {noise}: {result:?}, saves={}",
                saved.borrow().len()
            );
            let error = result.expect_err("all recorded noise must be finite and nonnegative");
            assert!(error.to_string().contains("scene 1"));
            assert_eq!(snapshot(&condor), before);
            assert!(saved.borrow().is_empty());
        }
    }

    #[test]
    fn late_data_access_errors_leave_all_scenes_and_saved_bytes_untouched() {
        for fail in 1..=3 {
            let mut scenes =
                vec![scene(EncoderBase::AOM, Some(0.4)), scene(EncoderBase::AOM, Some(0.6))];
            scenes[1].sequence_data.fail = fail;
            let (mut condor, saved) = fixture(Some(config()), scenes);
            let before = snapshot(&condor);
            let error = run(&mut condor).expect_err("data access must fail");
            assert!(format!("{error:#}").contains("noise-scaler"));
            assert_eq!(snapshot(&condor), before);
            assert!(saved.borrow().is_empty());
        }
    }

    #[test]
    fn extreme_finite_factors_and_chroma_overflow_fail_before_publication() {
        for (factor, chroma) in [(f64::MAX, false), (10_000_000.0, false), (2.0, true)] {
            let mut config = config();
            config.minimum_scaler = factor;
            config.maximum_scaler = factor;
            let mut scenes =
                vec![scene(EncoderBase::AOM, Some(0.4)), scene(EncoderBase::AOM, Some(0.6))];
            if chroma {
                let mut photon = scenes[1].encoder.photon_noise().expect("photon").clone();
                photon.chroma_iso = Some(u32::MAX);
                scenes[1].encoder.set_photon_noise(Some(photon));
            }
            let (mut condor, saved) = fixture(Some(config), scenes);
            let before = snapshot(&condor);
            let error = run(&mut condor).expect_err("unrepresentable ISO must fail");
            assert!(error.to_string().contains("scene"));
            assert_eq!(snapshot(&condor), before);
            assert!(saved.borrow().is_empty());
        }
    }

    #[test]
    fn zero_noise_scale_clears_luma_and_chroma_iso_and_saves_once() {
        let mut zero_config = config();
        zero_config.minimum_scaler = 0.0;
        zero_config.maximum_scaler = 0.0;
        let (mut condor, saved) =
            fixture(Some(zero_config), vec![scene(EncoderBase::AOM, Some(0.6))]);
        run(&mut condor).expect("explicit zero factor");
        let photon = condor.scenes[0].encoder.photon_noise().expect("photon");
        assert_eq!((photon.iso, photon.chroma_iso), (0, Some(0)));
        assert_eq!(
            condor.scenes[0].sequence_data.scaling.as_ref().expect("record").scaler,
            0.0
        );
        assert_eq!(saved.borrow().as_slice(), &[snapshot(&condor)]);
    }

    #[test]
    fn noise_scale_save_failure_restores_encoder_and_metadata() {
        let (mut condor, saved) = fixture(Some(config()), vec![scene(EncoderBase::AOM, Some(0.6))]);
        let before = snapshot(&condor);
        condor.save_callback = Box::new(|_| anyhow::bail!("noise-scaler atomic save failed"));
        assert!(
            run(&mut condor)
                .expect_err("save error")
                .to_string()
                .contains("noise-scaler atomic save failed")
        );
        assert_eq!(snapshot(&condor), before);
        assert!(saved.borrow().is_empty());
    }
}
