use std::path::{Path, PathBuf};

use andean_condor::{
    core::input::Input,
    models::sequence::scene_detector::{
        SceneDetectionMethod as CoreSceneDetectionMethod,
        SceneDetectorConfig,
        ScenecutMethod,
    },
    vapoursynth::vapoursynth_filters::VapourSynthFilter,
};
use anyhow::Result;
use clap::{Subcommand, ValueEnum};
use serde_json::Value;
use thiserror::Error;

use crate::{
    commands::{
        DecoderMethod,
        SceneDetectionMethod,
        config::{ConfigAction, ConfigCommand, InputOverride, PropertyChange, Toggle},
        handlers::{
            apply_input_override,
            config::{edit, ensure_override_requested},
            configure_override_input,
            configure_temp,
            load_configuration,
            warn_missing_path,
        },
    },
    configuration::Configuration,
};

#[allow(clippy::too_many_arguments)]
pub fn detect_scenes_handler(
    config_path: Option<&Path>,
    temp_path: Option<&Path>,
    input_path: Option<&Path>,
    decoder: Option<&DecoderMethod>,
    filters: Option<&[VapourSynthFilter]>,
    vs_args: Option<&[String]>,
    method: Option<&SceneDetectionMethod>,
    min_scene_seconds: Option<usize>,
    max_scene_seconds: Option<usize>,
) -> Result<(Configuration, PathBuf)> {
    let (mut configuration, config_path) = load_configuration(config_path)?;

    configure_temp(&mut configuration, temp_path)?;

    configure_scene_detector(
        &mut configuration,
        input_path,
        decoder,
        filters,
        vs_args,
        method,
        min_scene_seconds,
        max_scene_seconds,
    )?;

    configuration.save(&config_path)?;

    Ok((configuration, config_path))
}

#[allow(clippy::too_many_arguments)]
pub fn configure_scene_detector(
    configuration: &mut Configuration,
    input_path: Option<&Path>,
    decoder: Option<&DecoderMethod>,
    filters: Option<&[VapourSynthFilter]>,
    vs_args: Option<&[String]>,
    method: Option<&SceneDetectionMethod>,
    min_scene_seconds: Option<usize>,
    max_scene_seconds: Option<usize>,
) -> Result<()> {
    let existing_input = configuration
        .condor
        .sequence_config
        .scene_detector
        .input
        .clone()
        .unwrap_or_else(|| configuration.condor.input.clone());
    if let Some(input) = configure_override_input(
        configuration,
        &existing_input,
        input_path,
        decoder,
        filters,
        vs_args,
    )? {
        configuration.condor.sequence_config.scene_detector.input = Some(input);
    }
    let mut input = Input::from_data(
        configuration
            .condor
            .sequence_config
            .scene_detector
            .input
            .as_ref()
            .unwrap_or(&configuration.condor.input),
    )?;
    let clip_info = input.clip_info()?;
    let fps = *clip_info.frame_rate.numer() as f64 / *clip_info.frame_rate.denom() as f64;

    let previous_method = configuration.condor.sequence_config.scene_detector.method.clone();
    let min_scene_frames = min_scene_seconds.map_or_else(
        || previous_method.minimum_length(),
        |seconds| (fps * seconds as f64).round() as usize,
    );
    let max_scene_frames = max_scene_seconds.map_or_else(
        || previous_method.maximum_length(),
        |seconds| (fps * seconds as f64).round() as usize,
    );
    let new_method =
        method.map(|method| method.as_core_method(Some(min_scene_frames), Some(max_scene_frames)));
    if let Some(mut new_method) = new_method {
        // Keep the properties the CLI cannot express: the pinned model file
        // and the score-saving switch the config carries.
        match (&previous_method, &mut new_method) {
            (
                CoreSceneDetectionMethod::TransNetV2 {
                    model_path: previous_path,
                    save_scores: previous_scores,
                    ..
                },
                CoreSceneDetectionMethod::TransNetV2 {
                    model_path: slot,
                    save_scores: score_slot,
                    ..
                },
            ) => {
                if slot.is_none() {
                    *slot = previous_path.clone();
                }
                *score_slot = *previous_scores;
            },
            (
                CoreSceneDetectionMethod::AVSceneChange {
                    save_scores: previous_scores,
                    ..
                },
                CoreSceneDetectionMethod::AVSceneChange {
                    save_scores: score_slot,
                    ..
                },
            ) => *score_slot = *previous_scores,
            _ => {},
        }
        configuration.condor.sequence_config.scene_detector.method = new_method;
    }
    configuration
        .condor
        .sequence_config
        .scene_detector
        .method
        .set_minimum_length(min_scene_frames)?;
    configuration
        .condor
        .sequence_config
        .scene_detector
        .method
        .set_maximum_length(max_scene_frames)?;

    Ok(())
}

/// The scene detection algorithms `detect-scenes set method` can choose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum DetectionAlgorithm {
    /// av-scenechange scenecut; *--method* picks fast or standard.
    #[value(name = "avscenechange")]
    AvSceneChange,
    /// TransNetV2 neural network over ONNX Runtime; *--model-path* pins the
    /// model file.
    #[value(name = "transnetv2")]
    TransNetV2,
    /// No detection; scenes are chunked by maximum length only.
    #[value(name = "none")]
    None,
}

/// The scenecut algorithms the `avscenechange` detector offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ScenecutAlgorithm {
    /// Scenecut fast algorithm.
    #[value(name = "fast")]
    Fast,
    /// Scenecut standard algorithm.
    #[value(name = "standard")]
    Standard,
}

impl ScenecutAlgorithm {
    /// This choice for a scenecut method already in the configuration.
    #[inline]
    pub fn from_method(method: ScenecutMethod) -> Self {
        match method {
            ScenecutMethod::Fast => Self::Fast,
            ScenecutMethod::Standard => Self::Standard,
        }
    }
}

impl From<ScenecutAlgorithm> for ScenecutMethod {
    #[inline]
    fn from(algorithm: ScenecutAlgorithm) -> Self {
        match algorithm {
            ScenecutAlgorithm::Fast => Self::Fast,
            ScenecutAlgorithm::Standard => Self::Standard,
        }
    }
}

/// `info` and `reset` keys of `sequence_config.scene_detector`.
#[derive(Debug, Clone, Subcommand)]
pub enum DetectScenesKey {
    /// The detection algorithm and its settings.
    Method,
    /// The input the Scene Detector reads instead of the main input.
    Input,
}

/// Settings `condor detect-scenes set` can change.
#[derive(Debug, Clone, Subcommand)]
pub enum DetectScenesSet {
    /// Choose the detection algorithm and configure it.
    ///
    /// Values you omit keep their current value when the chosen algorithm
    /// supports them, and otherwise fall back to the default, so switching
    /// algorithms never silently drops your frame limits or score setting.
    Method {
        /// The detection algorithm to use.
        #[arg(value_name = "ALGORITHM")]
        algorithm:      DetectionAlgorithm,
        /// Smallest scene length to accept, in frames.
        #[arg(long, value_name = "Frames")]
        minimum_frames: Option<usize>,
        /// Largest scene length to split at, in frames.
        #[arg(long, value_name = "Frames")]
        maximum_frames: Option<usize>,
        /// Scenecut algorithm used by *avscenechange*.
        #[arg(long, value_name = "Method")]
        method:         Option<ScenecutAlgorithm>,
        /// ONNX model file used by *transnetv2*.
        #[arg(long, value_name = "Path")]
        model_path:     Option<PathBuf>,
        /// Keep the per-frame detection scores in the saved results.
        ///
        /// Bare `--save-scores` means on; `--save-scores off` turns it off.
        /// Omitted keeps the current setting.
        #[arg(long, value_name = "on|off", num_args = 0..=1, default_missing_value = "on")]
        save_scores:    Option<Toggle>,
    },
    /// Read from this input instead of the main input.
    ///
    /// Builds the same input the *--input*, *--decoder*, *--filters* and
    /// *--vs-args* flags of `detect-scenes` would build. Omitted flags inherit
    /// the current override, or the main input when there is none.
    Input {
        #[command(flatten)]
        args: InputOverride,
    },
}

/// The action triple attached to `condor detect-scenes`.
pub type DetectScenesAction = ConfigAction<DetectScenesKey, DetectScenesSet>;

/// Everything that can make a Scene Detector setting unusable.
#[derive(Debug, Error)]
pub enum DetectScenesConfigError {
    #[error("scene detection minimum length must be positive")]
    MinimumNotPositive,
    #[error("scene detection maximum length must be positive")]
    MaximumNotPositive,
    #[error("scene detection minimum length must not exceed maximum length")]
    RangeInverted,
}

/// The `condor detect-scenes` configuration section.
pub struct DetectScenesCrud;

impl DetectScenesCrud {
    #[inline]
    fn section(configuration: &Configuration) -> &SceneDetectorConfig {
        &configuration.condor.sequence_config.scene_detector
    }

    #[inline]
    fn section_mut(configuration: &mut Configuration) -> &mut SceneDetectorConfig {
        &mut configuration.condor.sequence_config.scene_detector
    }

    /// Rejects frame limits no detector can honour, before anything is written.
    fn validate(detector: &SceneDetectorConfig) -> Result<(), DetectScenesConfigError> {
        let method = &detector.method;
        if method.minimum_length() == 0 {
            return Err(DetectScenesConfigError::MinimumNotPositive);
        }
        if method.maximum_length() == 0 {
            return Err(DetectScenesConfigError::MaximumNotPositive);
        }
        if method.minimum_length() > method.maximum_length() {
            return Err(DetectScenesConfigError::RangeInverted);
        }
        Ok(())
    }

    /// Builds the requested detection algorithm, keeping the settings the user
    /// did not express and the chosen algorithm still supports.
    fn apply_method(
        configuration: &mut Configuration,
        algorithm: DetectionAlgorithm,
        minimum_frames: Option<usize>,
        maximum_frames: Option<usize>,
        method: Option<ScenecutAlgorithm>,
        model_path: Option<&PathBuf>,
        save_scores: Option<Toggle>,
    ) {
        let current = configuration.condor.sequence_config.scene_detector.method.clone();
        let minimum_length = minimum_frames.unwrap_or_else(|| current.minimum_length());
        let maximum_length = maximum_frames.unwrap_or_else(|| current.maximum_length());
        let scores = save_scores.map_or_else(|| current_scores(&current), Toggle::enabled);

        configuration.condor.sequence_config.scene_detector.method = match algorithm {
            DetectionAlgorithm::None => CoreSceneDetectionMethod::None {
                minimum_length,
                maximum_length,
            },
            DetectionAlgorithm::AvSceneChange => CoreSceneDetectionMethod::AVSceneChange {
                minimum_length,
                maximum_length,
                method: method.map_or_else(
                    || match &current {
                        CoreSceneDetectionMethod::AVSceneChange {
                            method, ..
                        } => *method,
                        _ => ScenecutMethod::Standard,
                    },
                    Into::into,
                ),
                save_scores: scores,
            },
            DetectionAlgorithm::TransNetV2 => CoreSceneDetectionMethod::TransNetV2 {
                minimum_length,
                maximum_length,
                model_path: model_path.cloned().or_else(|| match &current {
                    CoreSceneDetectionMethod::TransNetV2 {
                        model_path, ..
                    } => model_path.clone(),
                    _ => None,
                }),
                save_scores: scores,
            },
        };
    }

    /// Applies one `set input` override.
    fn apply_input(configuration: &mut Configuration, args: &InputOverride) -> Result<()> {
        ensure_override_requested(args)?;
        let existing = configuration
            .condor
            .sequence_config
            .scene_detector
            .input
            .clone()
            .unwrap_or_else(|| configuration.condor.input.clone());
        configuration.condor.sequence_config.scene_detector.input =
            Some(apply_input_override(configuration, &existing, args)?);
        Ok(())
    }
}

impl ConfigCommand for DetectScenesCrud {
    type Key = DetectScenesKey;
    type Set = DetectScenesSet;

    fn subject() -> &'static str {
        "Scene detection settings"
    }

    fn render(configuration: &Configuration, key: Option<&Self::Key>) -> Result<Value> {
        let detector = Self::section(configuration);
        Ok(match key {
            None => serde_json::to_value(detector)?,
            Some(DetectScenesKey::Method) => serde_json::to_value(&detector.method)?,
            Some(DetectScenesKey::Input) => serde_json::to_value(&detector.input)?,
        })
    }

    fn apply(configuration: &mut Configuration, set: &Self::Set) -> Result<PropertyChange> {
        match set {
            DetectScenesSet::Method {
                algorithm,
                minimum_frames,
                maximum_frames,
                method,
                model_path,
                save_scores,
            } => {
                if let Some(model_path) = model_path.as_deref() {
                    warn_missing_path("TransNetV2 model path", model_path);
                }
                Self::apply_method(
                    configuration,
                    *algorithm,
                    *minimum_frames,
                    *maximum_frames,
                    *method,
                    model_path.as_ref(),
                    *save_scores,
                );
                Self::validate(Self::section(configuration))?;
                PropertyChange::new("method", &Self::section(configuration).method)
            },
            DetectScenesSet::Input {
                args,
            } => {
                Self::apply_input(configuration, args)?;
                Self::validate(Self::section(configuration))?;
                PropertyChange::new("input", &Self::section(configuration).input)
            },
        }
    }

    fn restore(
        configuration: &mut Configuration,
        key: Option<&Self::Key>,
    ) -> Result<Option<PropertyChange>> {
        let defaults = SceneDetectorConfig::default();
        let detector = Self::section_mut(configuration);
        let (property, value) = match key {
            None => {
                *detector = defaults;
                ("section", serde_json::to_value(&*detector)?)
            },
            Some(DetectScenesKey::Method) => {
                detector.method = defaults.method;
                ("method", serde_json::to_value(&detector.method)?)
            },
            Some(DetectScenesKey::Input) => {
                detector.input = None;
                ("input", serde_json::to_value(&detector.input)?)
            },
        };
        Self::validate(Self::section(configuration))?;
        Ok(Some(PropertyChange {
            property: property.to_owned(),
            value,
        }))
    }
}

/// The score setting that carries over: the current one when the chosen
/// algorithm stores scores at all, the default (off) otherwise.
fn current_scores(current: &CoreSceneDetectionMethod) -> bool {
    match current {
        CoreSceneDetectionMethod::AVSceneChange {
            save_scores, ..
        }
        | CoreSceneDetectionMethod::TransNetV2 {
            save_scores, ..
        } => *save_scores,
        CoreSceneDetectionMethod::None {
            ..
        } => false,
    }
}

/// `condor detect-scenes info|set|reset`.
pub fn config(config_path: Option<&Path>, action: &DetectScenesAction) -> Result<()> {
    edit::<DetectScenesCrud>(config_path, action)
}

#[cfg(test)]
mod tests {
    use andean_condor::{
        models::{
            input::{Input, VapourSynthImportMethod},
            sequence::scene_detector::{
                SceneDetectionMethod as SceneDetectionMethodModel,
                ScenecutMethod,
            },
        },
        vapoursynth::plugins::resize::Scaler,
    };

    use super::*;
    use crate::{
        commands::handlers::init::init_handler,
        test_helpers::{check_basic_config, default_config, get_test_video},
        utils::hash_path::hash_path,
    };

    #[test]
    fn detect_scenes_default_config() {
        let test_video = get_test_video();
        let input_abs = path_abs::PathAbs::new(&test_video.path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let temp = tempfile::tempdir().expect("temp directory");
        let temp_abs = path_abs::PathAbs::new(temp.path().join(hash_path(&input_abs)))
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let output = temp.path().join("out.mkv");
        let config_path = temp.path().join("condor.json");
        let config_path_abs = path_abs::PathAbs::new(&config_path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();

        let expected_config = default_config(&test_video, &output, &temp_abs);

        init_handler(
            Some(&config_path),
            Some(&temp.path().join(hash_path(&input_abs))), /* Simulate default directory to
                                                             * avoid changing CWD in other
                                                             * parallel tests */
            &test_video.path,
            &output,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            true,
            None,
            None,
        )
        .expect("init_handler should succeed");
        let (config, found_config_path) = detect_scenes_handler(
            Some(&config_path),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("detect_scenes_handler should succeed");

        assert_eq!(
            found_config_path,
            config_path_abs,
            "config path is {}",
            config_path_abs.display()
        );
        check_basic_config(&config, &expected_config);
        assert!(config.condor.scenes.is_empty(), "scenes is empty");
    }

    #[test]
    fn detect_scenes_custom_config() {
        let test_video = get_test_video();
        let input_abs = path_abs::PathAbs::new(&test_video.path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let temp = tempfile::tempdir().expect("temp directory");
        let temp_abs = path_abs::PathAbs::new(temp.path().join(hash_path(&input_abs)))
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let output = temp.path().join("out.mkv");
        let config_path = temp.path().join("condor.json");
        let config_path_abs = path_abs::PathAbs::new(&config_path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let custom_filters = vec![VapourSynthFilter::Resize {
            scaler: Some(Scaler::Point),
            width:  Some(960),
            height: Some(540),
            format: None,
        }];
        let min_scene_seconds = 2usize;
        let max_scene_seconds = 12usize;
        let expected_min = (test_video.fps() * min_scene_seconds as f64).round() as usize;
        let expected_max = (test_video.fps() * max_scene_seconds as f64).round() as usize;

        let mut expected_config = default_config(&test_video, &output, &temp_abs);
        expected_config.condor.sequence_config.scene_detector.input = Some(Input::VapourSynth {
            path:          input_abs.clone(),
            import_method: VapourSynthImportMethod::FFMS2 {
                index: None
            },
            cache_path:    None,
            filters:       custom_filters.clone(),
        });
        expected_config.condor.sequence_config.scene_detector.method =
            SceneDetectionMethodModel::AVSceneChange {
                minimum_length: expected_min,
                maximum_length: expected_max,
                method:         ScenecutMethod::Fast,
                save_scores:    false,
            };
        // immutable shadow
        let expected_config = expected_config;

        init_handler(
            Some(&config_path),
            Some(&temp.path().join(hash_path(&input_abs))), /* Simulate default directory to
                                                             * avoid changing CWD in other
                                                             * parallel tests */
            &test_video.path,
            &output,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            true,
            None,
            None,
        )
        .expect("init_handler should succeed");
        let (config, found_config_path) = detect_scenes_handler(
            Some(&config_path),
            Some(&temp.path().join(hash_path(&input_abs))),
            Some(&test_video.path),
            Some(&DecoderMethod::VSFFMS2),
            Some(&custom_filters),
            Some(&["method=scd".to_owned()]),
            Some(&SceneDetectionMethod::Fast),
            Some(min_scene_seconds),
            Some(max_scene_seconds),
        )
        .expect("detect_scenes_handler should succeed");

        assert_eq!(
            found_config_path,
            config_path_abs,
            "config path is {}",
            config_path_abs.display()
        );
        check_basic_config(&config, &expected_config);
        assert!(config.condor.scenes.is_empty(), "scenes is empty");
    }

    /// `--scd-filters` alone must not write through to the main input. Doing so
    /// replaces its default 10-bit conversion, which the encoder depends on.
    #[test]
    fn detect_scenes_filters_only_do_not_touch_main_input() {
        let test_video = get_test_video();
        let input_abs = path_abs::PathAbs::new(&test_video.path)
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let temp = tempfile::tempdir().expect("temp directory");
        let temp_abs = path_abs::PathAbs::new(temp.path().join(hash_path(&input_abs)))
            .expect("path_abs should succeed")
            .as_path()
            .to_path_buf();
        let output = temp.path().join("out.mkv");
        let config_path = temp.path().join("condor.json");
        let custom_filters = vec![VapourSynthFilter::Resize {
            scaler: Some(Scaler::Point),
            width:  Some(960),
            height: Some(540),
            format: None,
        }];
        let mut expected_config = default_config(&test_video, &output, &temp_abs);
        expected_config.condor.sequence_config.scene_detector.input = Some(Input::VapourSynth {
            path:          input_abs.clone(),
            import_method: VapourSynthImportMethod::BestSource {
                index: None
            },
            cache_path:    None,
            filters:       custom_filters.clone(),
        });
        // `condor.input` is left as the default config has it, so `check_input`
        // inside `check_basic_config` fails if the filters leak onto it.
        let expected_config = expected_config;

        init_handler(
            Some(&config_path),
            Some(&temp.path().join(hash_path(&input_abs))),
            &test_video.path,
            &output,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            true,
            None,
            None,
        )
        .expect("init_handler should succeed");

        let (config, _) = detect_scenes_handler(
            Some(&config_path),
            None,
            None,
            None,
            Some(&custom_filters),
            None,
            None,
            None,
            None,
        )
        .expect("detect_scenes_handler should succeed");

        // `condor.input` must still match the default config, filters included.
        check_basic_config(&config, &expected_config);
        let scd_input = config
            .condor
            .sequence_config
            .scene_detector
            .input
            .as_ref()
            .expect("Scene Detector input is Some");
        assert_eq!(
            scd_input.vapoursynth_filters(),
            custom_filters,
            "Scene Detector input has the requested filters"
        );
        assert_eq!(
            match scd_input {
                Input::VapourSynth {
                    path, ..
                }
                | Input::Video {
                    path, ..
                } => path,
                Input::VapourSynthScript {
                    ..
                } => panic!("Scene Detector input is a script"),
            },
            &input_abs,
            "Scene Detector input inherits the main input's path"
        );
    }
}
