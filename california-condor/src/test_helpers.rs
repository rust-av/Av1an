use std::{
    assert_matches,
    collections::HashMap,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use andean_condor::{
    ffmpeg::FFPixelFormat,
    models::{
        Condor,
        encoder::{Encoder, EncoderBase, EncoderPasses, photon_noise::PhotonNoise},
        input::{ImportMethod, Input, VapourSynthImportMethod, VapourSynthScriptSource},
        output::Output,
        scene::Scene,
        sequence::{
            benchmarker::BenchmarkerConfig,
            bitrate_optimizer::BitrateOptimizerConfig,
            noise_detector::NoiseDetectorConfig,
            noise_scaler::NoiseScalerConfig,
            parallel_encoder::ParallelEncoderConfig,
            quality_check::QualityCheckConfig,
            scene_concatenator::{ConcatMethod, SceneConcatenatorConfig},
            scene_detector::{
                DEFAULT_MAX_SCENE_LENGTH_SECONDS,
                SceneDetectionMethod,
                SceneDetectorConfig,
                ScenecutMethod,
            },
            speed_scaler::SpeedScalerConfig,
            target_quality::{TargetQualityConfig, types::QualityMetric},
        },
    },
    vapoursynth::{
        plugins::{bestsource::VideoSource, resize::Scaler, standard::box_blur::BoxBlur},
        script_builder::{
            VapourSynthPluginScript,
            script::{Line, VapourSynthScript},
        },
        vapoursynth_filters::VapourSynthFilter,
    },
};

use crate::{
    configuration::{CONFIGURATION_SCHEMA_URL, CliSequenceConfig, CliSequenceData, Configuration},
    zoner::ZonerConfig,
};

/// Convert a path to a `&str`, assuming it contains valid UTF-8.
pub fn path_str(p: &Path) -> &str {
    p.to_str().expect("path should be valid UTF-8")
}

/// Cached test clip path, generated once per process.
pub static TEST_CLIP: OnceLock<PathBuf> = OnceLock::new();

pub struct TestVideo {
    pub path:         PathBuf,
    pub width:        usize,
    pub height:       usize,
    pub frames:       usize,
    pub fps_rational: (usize, usize),
    pub format:       FFPixelFormat,
    pub scenes:       Vec<(usize, usize)>,
}

impl TestVideo {
    pub fn fps(&self) -> f64 {
        self.fps_rational.0 as f64 / self.fps_rational.1 as f64
    }

    pub fn mock_scenes(&self, encoder: &Encoder) -> Vec<Scene<CliSequenceData>> {
        self.scenes
            .iter()
            .map(|(start, end)| Scene {
                start_frame:   *start,
                end_frame:     *end,
                encoder:       encoder.clone(),
                sequence_data: CliSequenceData::default(),
                sub_scenes:    None,
            })
            .collect()
    }
}

/// Check whether FFmpeg is available on PATH.
pub fn ffmpeg_is_available() -> bool {
    std::process::Command::new("ffmpeg").arg("-version").output().is_ok()
}

/// Check whether mkvmerge is available on PATH.
pub fn mkvmerge_is_available() -> bool {
    std::process::Command::new("mkvmerge").arg("--version").output().is_ok()
}

/// Cached multi-track test clip path, generated once per process.
pub static TEST_MULTITRACK_CLIP: OnceLock<PathBuf> = OnceLock::new();

/// A copy of the test clip with two audio tracks, a subtitle track, and an
/// attachment, so the track selection has something to select.
///
/// Keeping the extra tracks out of [`get_test_video`] leaves the rest of the
/// suite on a video-only clip whose streams are already known.
pub fn get_test_multitrack_video() -> PathBuf {
    TEST_MULTITRACK_CLIP
        .get_or_init(|| {
            let path = std::env::temp_dir().join("condor-test-multitrack-clip.mkv");
            if !path.exists() {
                generate_multitrack_clip(&path);
            }
            path
        })
        .clone()
}

/// Muxes the test clip with two AAC tracks, an SRT subtitle, and a font
/// attachment.
fn generate_multitrack_clip(path: &Path) {
    let video = get_test_video();
    let directory = path.parent().expect("the clip has a parent directory");
    let subtitle = directory.join("condor-test-clip.srt");
    let attachment = directory.join("condor-test-clip.ttf");
    std::fs::write(
        &subtitle,
        "1\n00:00:00,000 --> 00:00:05,000\nFirst subtitle line.\n\n2\n00:00:05,000 --> \
         00:00:10,000\nSecond subtitle line.\n",
    )
    .expect("the subtitle file should be written");
    // FFmpeg only needs the file to exist and to be described; the bytes are
    // never parsed as a real font.
    std::fs::write(&attachment, b"condor test attachment")
        .expect("the attachment should be written");

    let status = std::process::Command::new("ffmpeg")
        .args(["-y", "-hide_banner", "-loglevel", "error"])
        .args(["-i", path_str(&video.path)])
        .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=30"])
        .args(["-f", "lavfi", "-i", "sine=frequency=880:duration=30"])
        .args(["-i", path_str(&subtitle)])
        .args(["-attach", path_str(&attachment)])
        .args(["-metadata:s:t:0", "mimetype=font/ttf"])
        .args(["-metadata:s:t:0", "filename=condor-test-clip.ttf"])
        .args(["-map", "0:v", "-map", "1:a", "-map", "2:a", "-map", "3:s"])
        .args(["-c:v", "copy", "-c:a", "aac", "-c:s", "srt"])
        .args(["-metadata", "title=Condor Test Clip"])
        .arg(path)
        .status()
        .expect("Failed to run FFmpeg for the multi-track test clip");

    assert!(
        status.success(),
        "FFmpeg multi-track clip generation failed"
    );
}

/// Generates a video with FFmpeg in the temporary directory
/// if it doesn't already exist.
///
/// 1920x1080 YUV420P 24001/1001 FPS, 00:00:30.000, 720 frames:
/// 1. Gradient for 5 seconds (120 frames),
/// 2. Conways Game of Life for 5 seconds (120 frames),
/// 3. Mandelbot for 12 seconds (288 frames),
/// 4. Color Chart for 8 seconds (192 frames)
pub fn get_test_video() -> TestVideo {
    TestVideo {
        path:         TEST_CLIP
            .get_or_init(|| {
                let dir = std::env::temp_dir().join("condor-test-clip.mkv");
                if !dir.exists() {
                    generate_test_clip_inner(&dir);
                }
                dir
            })
            .clone(),
        width:        1920,
        height:       1080,
        frames:       720,
        fps_rational: (24001, 1001),
        format:       FFPixelFormat::YUV420P,
        // exact_scenes:       vec![(0, 120), (120, 240), (240, 528), (528, 720)],
        // max-scene-length of 10 seconds splits scene 3 (240, 528) into 2 (240, 480), (480, 528)
        scenes:       vec![(0, 121), (121, 240), (240, 480), (480, 528), (528, 720)],
    }
}

fn generate_test_clip_inner(path: &Path) {
    assert!(
        ffmpeg_is_available(),
        "ffmpeg is required to generate the test clip"
    );

    let filter_graph = concat!(
        "color=c=white:size=1920x1080:rate=24001/1001:d=5,",
        "scale=1920:1080,setsar=1,",
        "geq=r=255*gauss((X/W-0.5)*3)*gauss((Y/H-0.5)*3)/gauss(0)/gauss(0):",
        "g=255*gauss((X/W-0.5)*5)*gauss((Y/H-0.5)*3)/gauss(0)/gauss(0):",
        "b=255*gauss((X/W-0.5)*3)*gauss((Y/H-0.5)*5)/gauss(0)/gauss(0),",
        "format=yuv420p[s0];",
        "life=size=960x540:rate=24001/1001:seed=42:random_fill_ratio=0.5,",
        "scale=1920:1080,setsar=1,trim=duration=5,format=yuv420p[s1];",
        "mandelbrot=s=1920x1080:rate=24001/1001,",
        "scale=1920:1080,setsar=1,trim=duration=12,format=yuv420p[s2];",
        "colorchart=rate=24001/1001:duration=8,",
        "scale=1920:1080,setsar=1,format=yuv420p[s3];",
        "[s0][s1][s2][s3]concat=n=4:v=1:a=0[out]"
    );

    let status = std::process::Command::new("ffmpeg")
        .args(["-y", "-filter_complex", filter_graph])
        .args(["-map", "[out]"])
        .args(["-c:v", "libx264", "-qp", "0", "-preset", "ultrafast", "-pix_fmt", "yuv420p"])
        // .args(["-f", "yuv4mpegpipe"])
        .arg(path)
        .status()
        .expect("Failed to run FFmpeg for test clip generation");

    assert!(status.success(), "FFmpeg test clip generation failed");
}

/// RAII guard that temporarily changes the process working directory and
/// restores the previous one when dropped (even on panic).
///
/// This prevents the process-wide CWD from pointing at a deleted temporary
/// directory after a test finishes, which would break subsequent tests.
pub struct CwdGuard(PathBuf);

impl CwdGuard {
    /// Set the current working directory to `directory`, returning a guard
    /// that restores the previous working directory on drop.
    pub fn set(directory: &Path) -> Self {
        let previous = std::env::current_dir().expect("current directory");
        std::env::set_current_dir(directory)
            .unwrap_or_else(|_| panic!("set current working directory to {}", directory.display()));
        Self(previous)
    }
}

impl Drop for CwdGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.0);
    }
}

/// Build a default Configuration given a test video, output path, and temp
/// directory.
pub fn default_config(test_video: &TestVideo, output: &Path, temp: &Path) -> Configuration {
    let input_abs = path_abs::PathAbs::new(&test_video.path)
        .expect("path_abs should succeed")
        .as_path()
        .to_path_buf();
    let output_abs = path_abs::PathAbs::new(output)
        .expect("path_abs should succeed")
        .as_path()
        .to_path_buf();
    let temp_abs = path_abs::PathAbs::new(temp)
        .expect("path_abs should succeed")
        .as_path()
        .to_path_buf();
    let scenes_directory = temp_abs.join("scenes");
    Configuration {
        schema: CONFIGURATION_SCHEMA_URL.to_owned(),
        condor: Condor {
            input:           Input::VapourSynth {
                path:          input_abs.clone(),
                import_method: VapourSynthImportMethod::BestSource {
                    index: None
                },
                cache_path:    None,
                filters:       vec![VapourSynthFilter::Resize {
                    scaler: Some(Scaler::Bicubic),
                    width:  None,
                    height: None,
                    format: Some(FFPixelFormat::YUV420P10LE),
                }],
            },
            output:          Output {
                path:       output_abs,
                tags:       HashMap::new(),
                video_tags: HashMap::new(),
            },
            encoder:         Encoder::SVTAV1 {
                executable:        None,
                pass:              EncoderPasses::All(1),
                options:           EncoderBase::SVTAV1.default_parameters(),
                photon_noise:      None,
                ffmpeg_filter:     None,
                append_color_data: true,
            },
            scenes:          vec![],
            sequence_config: CliSequenceConfig {
                scene_detector:     SceneDetectorConfig {
                    method: SceneDetectionMethod::AVSceneChange {
                        minimum_length: test_video.fps().round() as usize,
                        maximum_length: (DEFAULT_MAX_SCENE_LENGTH_SECONDS as f64 * test_video.fps())
                            .round() as usize,
                        method:         ScenecutMethod::Standard,
                        save_scores:    false,
                    },
                    input:  None,
                },
                zoner:              None,
                noise_detector:     None,
                noise_scaler:       None,
                benchmarker:        BenchmarkerConfig::default(),
                target_quality:     None,
                quality_check:      None,
                bitrate_optimizer:  BitrateOptimizerConfig {
                    bitrate_sigma_threshold: None,
                },
                speed_scaler:       SpeedScalerConfig::default(),
                parallel_encoder:   ParallelEncoderConfig {
                    workers:          None,
                    scenes_directory: scenes_directory.clone(),
                    input:            None,
                },
                scene_concatenator: SceneConcatenatorConfig {
                    method: ConcatMethod::MKVMerge(Default::default()),
                    scenes_directory,
                    output: None,
                },
            },
        },
        input:  input_abs,
        temp:   temp_abs,
    }
}

/// Generates a VapourSynth script that imports the specified test video and
/// outputs 2 VideoNodes: the original and the blurred version. The blurred
/// VideoNode simulates a denoising filter and is intended to be used with
/// the Noise Detector.
pub fn vapoursynth_script(test_video: &TestVideo, filters: Option<&[VapourSynthFilter]>) -> String {
    const SCRIPT_NODE_NAME: &str = "clip";
    const SCRIPT_DENOISED_NODE_NAME: &str = "denoised_clip";
    let mut script = VapourSynthScript::default();
    // Import the test video with FFMS2
    let (dec_import_lines, dec_lines) = VideoSource::new(&test_video.path)
        .generate_script(SCRIPT_NODE_NAME.to_owned())
        .expect("generate_script should succeed");
    if let Some(dec_import_lines) = dec_import_lines {
        script.add_imports(dec_import_lines);
    }
    script.add_lines(dec_lines);
    // Apply filters
    if let Some(filters) = filters {
        for filter in filters {
            let (import_lines, filter_lines) = filter
                .generate_script(SCRIPT_NODE_NAME.to_owned())
                .expect("generate_script should succeed");

            if let Some(import_lines) = import_lines {
                script.add_imports(import_lines);
            }
            script.add_lines(filter_lines);
        }
    }
    // Copy the original VideoNode to the denoised VideoNode
    script.add_lines(vec![Line::Expression(
        SCRIPT_DENOISED_NODE_NAME.to_owned(),
        SCRIPT_NODE_NAME.to_owned(),
    )]);
    // Blur to simulate denoising
    let (_, blur_lines) = BoxBlur::default()
        .generate_script(SCRIPT_DENOISED_NODE_NAME.to_owned())
        .expect("generate_script should succeed");
    script.add_lines(blur_lines);

    script.outputs.insert(0, SCRIPT_NODE_NAME.to_owned());
    script.outputs.insert(1, SCRIPT_DENOISED_NODE_NAME.to_owned());

    script.to_string()
}

/// Asserts the configuration matches the expected configuration. Does not check
/// scenes.
pub fn check_basic_config(config: &Configuration, expected_config: &Configuration) {
    assert_eq!(
        config.temp,
        expected_config.temp,
        "temp path is {}",
        expected_config.temp.display()
    );
    assert_eq!(
        config.input,
        expected_config.input,
        "input path is {}",
        expected_config.input.display()
    );
    assert_eq!(
        config.condor.output.path,
        expected_config.condor.output.path,
        "output path is {}",
        expected_config.condor.output.path.display()
    );
    check_input(
        Some(&config.condor.input),
        Some(&expected_config.condor.input),
        "input",
    );
    check_output(&config.condor.output, &expected_config.condor.output);
    check_scene_detector(
        &config.condor.sequence_config.scene_detector,
        &expected_config.condor.sequence_config.scene_detector,
    );
    check_zoner(
        config.condor.sequence_config.zoner.as_ref(),
        expected_config.condor.sequence_config.zoner.as_ref(),
    );
    check_encoder(
        &config.condor.encoder,
        &expected_config.condor.encoder,
        "encoder",
    );
    check_benchmarker(
        &config.condor.sequence_config.benchmarker,
        &expected_config.condor.sequence_config.benchmarker,
    );
    check_noise_detector(
        config.condor.sequence_config.noise_detector.as_ref(),
        expected_config.condor.sequence_config.noise_detector.as_ref(),
    );
    check_noise_scaler(
        config.condor.sequence_config.noise_scaler.as_ref(),
        expected_config.condor.sequence_config.noise_scaler.as_ref(),
    );
    check_target_quality(
        config.condor.sequence_config.target_quality.as_ref(),
        expected_config.condor.sequence_config.target_quality.as_ref(),
    );
    check_bitrate_optimizer(
        &config.condor.sequence_config.bitrate_optimizer,
        &expected_config.condor.sequence_config.bitrate_optimizer,
    );
    check_speed_scaler(
        &config.condor.sequence_config.speed_scaler,
        &expected_config.condor.sequence_config.speed_scaler,
    );
    check_parallel_encoder(
        &config.condor.sequence_config.parallel_encoder,
        &expected_config.condor.sequence_config.parallel_encoder,
    );
    check_scene_concatenator(
        &config.condor.sequence_config.scene_concatenator,
        &expected_config.condor.sequence_config.scene_concatenator,
    );
    check_quality_check(
        config.condor.sequence_config.quality_check.as_ref(),
        expected_config.condor.sequence_config.quality_check.as_ref(),
    );
}

pub fn check_input(input: Option<&Input>, expected_input: Option<&Input>, input_name: &str) {
    if let Some(expected_input) = expected_input {
        let config_input = input.unwrap_or_else(|| panic!("{input_name} is Some"));
        match expected_input {
            Input::Video {
                path,
                import_method,
                filters,
            } => {
                assert_matches!(config_input, Input::Video { .. }, "{input_name} is Video");
                match config_input {
                    Input::Video {
                        path: ci_path,
                        import_method: ci_import_method,
                        filters: ci_filters,
                    } => {
                        assert_eq!(ci_path, path, "{input_name} path is {}", path.display());
                        assert_eq!(ci_filters, filters, "{input_name} filters are {filters:?}");
                        match import_method {
                            ImportMethod::FFMS2 {
                                index,
                                cache_path,
                            } => {
                                assert_matches!(
                                    ci_import_method,
                                    ImportMethod::FFMS2 { .. },
                                    "{input_name} import method is FFMS2"
                                );
                                match ci_import_method {
                                    ImportMethod::FFMS2 {
                                        index: ci_index,
                                        cache_path: ci_cache_path,
                                    } => {
                                        assert_eq!(
                                            ci_index, index,
                                            "{input_name} index is {index:?}"
                                        );
                                        assert_eq!(
                                            ci_cache_path, cache_path,
                                            "{input_name} cache path is {cache_path:?}"
                                        );
                                    },
                                }
                            },
                        }
                    },
                    _ => panic!("expected Input::Video"),
                }
            },
            Input::VapourSynth {
                path,
                import_method,
                cache_path,
                filters,
            } => {
                assert_matches!(
                    config_input,
                    Input::VapourSynth { .. },
                    "{input_name} is VapourSynth"
                );
                match config_input {
                    Input::VapourSynth {
                        path: ci_path,
                        import_method: ci_import_method,
                        cache_path: ci_cache_path,
                        filters: ci_filters,
                    } => {
                        assert_eq!(ci_path, path, "{input_name} path is {}", path.display());
                        assert_eq!(ci_filters, filters, "{input_name} filters are {filters:?}");
                        assert_eq!(
                            ci_cache_path, cache_path,
                            "{input_name} cache path is {cache_path:?}"
                        );
                        match import_method {
                            VapourSynthImportMethod::FFMS2 {
                                index,
                            } => {
                                assert_matches!(
                                    ci_import_method,
                                    VapourSynthImportMethod::FFMS2 { .. },
                                    "{input_name} import method is FFMS2"
                                );
                                match ci_import_method {
                                    VapourSynthImportMethod::FFMS2 {
                                        index: pe_index,
                                    } => {
                                        assert_eq!(
                                            pe_index, index,
                                            "{input_name} index is {index:?}"
                                        );
                                    },
                                    _ => panic!("expected {input_name} import method to be FFMS2"),
                                }
                            },
                            VapourSynthImportMethod::BestSource {
                                index,
                            } => {
                                assert_matches!(
                                    ci_import_method,
                                    VapourSynthImportMethod::BestSource { .. },
                                    "{input_name} import method is BestSource"
                                );
                                match ci_import_method {
                                    VapourSynthImportMethod::BestSource {
                                        index: pe_index,
                                    } => {
                                        assert_eq!(
                                            pe_index, index,
                                            "{input_name} index is {index:?}"
                                        );
                                    },
                                    _ => panic!(
                                        "expected {input_name} import method to be BestSource"
                                    ),
                                }
                            },
                            VapourSynthImportMethod::DGDecNV {
                                dgindexnv_executable,
                            } => {
                                assert_matches!(
                                    ci_import_method,
                                    VapourSynthImportMethod::DGDecNV { .. },
                                    "{input_name} import method is DGDecNV"
                                );
                                match ci_import_method {
                                    VapourSynthImportMethod::DGDecNV {
                                        dgindexnv_executable: ci_dgindexnv_executable,
                                    } => {
                                        assert_eq!(
                                            ci_dgindexnv_executable, dgindexnv_executable,
                                            "{input_name} DGIndexNV executable path is \
                                             {dgindexnv_executable:?}"
                                        );
                                    },
                                    _ => {
                                        panic!("expected {input_name} import method to be DGDecNV")
                                    },
                                }
                            },
                            VapourSynthImportMethod::LSMASHWorks {
                                index,
                            } => {
                                assert_matches!(
                                    ci_import_method,
                                    VapourSynthImportMethod::LSMASHWorks { .. },
                                    "{input_name} import method is LSMASHWorks"
                                );
                                match ci_import_method {
                                    VapourSynthImportMethod::LSMASHWorks {
                                        index: pe_index,
                                    } => {
                                        assert_eq!(
                                            pe_index, index,
                                            "{input_name} index is {index:?}"
                                        );
                                    },
                                    _ => panic!(
                                        "expected {input_name} import method to be LSMASHWorks"
                                    ),
                                }
                            },
                        }
                    },
                    _ => panic!("expected Input::VapourSynth"),
                }
            },
            Input::VapourSynthScript {
                source,
                variables,
                index,
                filters,
                stream_concurrently,
            } => {
                assert_matches!(
                    config_input,
                    Input::VapourSynthScript { .. },
                    "{input_name} is VapourSynthScript"
                );
                match config_input {
                    Input::VapourSynthScript {
                        source: ci_source,
                        variables: ci_variables,
                        index: ci_index,
                        filters: ci_filters,
                        stream_concurrently: ci_stream_concurrently,
                    } => {
                        assert_eq!(ci_filters, filters, "{input_name} filters are {filters:?}");
                        assert_eq!(
                            ci_stream_concurrently, stream_concurrently,
                            "{input_name} stream_concurrently is {stream_concurrently}"
                        );
                        match source {
                            VapourSynthScriptSource::Path(path) => {
                                assert_matches!(
                                    ci_source,
                                    VapourSynthScriptSource::Path { .. },
                                    "{input_name} source is Path"
                                );
                                match ci_source {
                                    VapourSynthScriptSource::Path(ci_path) => {
                                        assert_eq!(
                                            ci_path,
                                            path,
                                            "{input_name} path is {}",
                                            path.display()
                                        );
                                    },
                                    _ => panic!("expected {input_name} source to be Path"),
                                }
                            },
                            VapourSynthScriptSource::Text(script) => {
                                assert_matches!(
                                    ci_source,
                                    VapourSynthScriptSource::Text { .. },
                                    "{input_name} source is Text"
                                );
                                match ci_source {
                                    VapourSynthScriptSource::Text(ci_script) => {
                                        assert_eq!(
                                            ci_script, script,
                                            "{input_name} source script is {script}"
                                        );
                                    },
                                    _ => panic!("expected {input_name} source to be Text"),
                                }
                            },
                        }
                        assert_eq!(
                            ci_variables, variables,
                            "{input_name} variables are {variables:?}"
                        );
                        assert_eq!(ci_index, index, "{input_name} index is {index}");
                    },
                    _ => panic!("expected {input_name} to be VapourSynthScript"),
                }
            },
        }
    } else {
        assert_matches!(input, None, "{input_name} is None");
    }
}

pub fn check_output(output: &Output, expected_output: &Output) {
    assert_eq!(
        output.path,
        expected_output.path,
        "output path is {}",
        expected_output.path.display()
    );
    assert_eq!(
        output.tags, expected_output.tags,
        "output tags are {:?}",
        expected_output.tags
    );
    assert_eq!(
        output.video_tags, expected_output.video_tags,
        "output video tags are {:?}",
        expected_output.video_tags
    );
}

pub fn check_encoder_pass(
    pass: &EncoderPasses,
    expected_pass: &EncoderPasses,
    encoder_name: Option<&str>,
) {
    let encoder_name = encoder_name.unwrap_or("encoder");
    match expected_pass {
        EncoderPasses::All(expected_passes) => {
            assert_matches!(*pass, EncoderPasses::All { .. }, "encoder pass is All");
            match pass {
                EncoderPasses::All(passes) => {
                    assert_eq!(
                        passes, expected_passes,
                        "{encoder_name} passes are {expected_passes:?}"
                    );
                },
                _ => panic!("expected {encoder_name} pass to be All"),
            }
        },
        EncoderPasses::Specific(expected_current, expected_total) => {
            assert_matches!(
                pass,
                EncoderPasses::Specific { .. },
                "{encoder_name} pass is Specific"
            );
            match pass {
                EncoderPasses::Specific(current, total) => {
                    assert_eq!(
                        current, expected_current,
                        "{encoder_name} current pass is {expected_current:?}"
                    );
                    assert_eq!(
                        total, expected_total,
                        "{encoder_name} total passes is {expected_total:?}"
                    );
                },
                _ => panic!("expected {encoder_name} pass to be Specific"),
            }
        },
    }
}

pub fn check_photon_noise(
    photon_noise: Option<&PhotonNoise>,
    expected_photon_noise: Option<&PhotonNoise>,
    encoder_name: Option<&str>,
) {
    let encoder_name = encoder_name.unwrap_or("encoder");
    if let Some(expected_photon_noise) = expected_photon_noise {
        let photon_noise = photon_noise.expect("encoder photon noise is Some");
        assert_eq!(
            photon_noise.iso, expected_photon_noise.iso,
            "{encoder_name} photon noise iso is {}",
            expected_photon_noise.iso
        );
        assert_eq!(
            photon_noise.chroma_iso, expected_photon_noise.chroma_iso,
            "{encoder_name} photon noise chroma iso is {:?}",
            expected_photon_noise.chroma_iso
        );
        assert_eq!(
            photon_noise.width, expected_photon_noise.width,
            "{encoder_name} photon noise width is {:?}",
            expected_photon_noise.width
        );
        assert_eq!(
            photon_noise.height, expected_photon_noise.height,
            "{encoder_name} photon noise height is {:?}",
            expected_photon_noise.height
        );
        assert_eq!(
            photon_noise.c_y, expected_photon_noise.c_y,
            "{encoder_name} photon noise c_y is {:?}",
            expected_photon_noise.c_y
        );
        assert_eq!(
            photon_noise.ccb, expected_photon_noise.ccb,
            "{encoder_name} photon noise ccb is {:?}",
            expected_photon_noise.ccb
        );
        assert_eq!(
            photon_noise.ccr, expected_photon_noise.ccr,
            "{encoder_name} photon noise ccr is {:?}",
            expected_photon_noise.ccr
        );
    } else {
        assert!(
            photon_noise.is_none(),
            "{encoder_name} photon noise is None"
        );
    }
}

pub fn check_encoder(encoder: &Encoder, expected_encoder: &Encoder, encoder_name: &str) {
    assert_eq!(
        encoder.base(),
        expected_encoder.base(),
        "{encoder_name} is {:?}",
        expected_encoder.base()
    );
    assert_eq!(
        encoder.append_color_data(),
        expected_encoder.append_color_data(),
        "{encoder_name} append_color_data is {}",
        encoder.append_color_data()
    );
    match expected_encoder {
        Encoder::AOM {
            executable: expected_executable,
            pass: expected_pass,
            options: expected_options,
            photon_noise: expected_photon_noise,
            ffmpeg_filter: expected_ffmpeg_filter,
            ..
        } => {
            assert_matches!(encoder, Encoder::AOM { .. }, "{encoder_name} is AOM");
            match encoder {
                Encoder::AOM {
                    executable,
                    pass,
                    options,
                    photon_noise,
                    ffmpeg_filter,
                    ..
                } => {
                    assert_eq!(
                        executable, expected_executable,
                        "{encoder_name} executable path is {executable:?}"
                    );
                    assert_eq!(
                        options, expected_options,
                        "{encoder_name} options are {options:?}"
                    );
                    check_encoder_pass(pass, expected_pass, Some(encoder_name));
                    check_photon_noise(
                        photon_noise.as_ref(),
                        expected_photon_noise.as_ref(),
                        Some(encoder_name),
                    );
                    assert_eq!(
                        ffmpeg_filter.as_deref(),
                        expected_ffmpeg_filter.as_deref(),
                        "{encoder_name} FFmpeg filter is {ffmpeg_filter:?}"
                    );
                },
                _ => panic!("expected {encoder_name} to be AOM"),
            }
        },
        Encoder::RAV1E {
            executable: expected_executable,
            pass: expected_pass,
            options: expected_options,
            photon_noise: expected_photon_noise,
            ffmpeg_filter: expected_ffmpeg_filter,
            ..
        } => {
            assert_matches!(encoder, Encoder::RAV1E { .. }, "{encoder_name} is RAV1E");
            match encoder {
                Encoder::RAV1E {
                    executable,
                    pass,
                    options,
                    photon_noise,
                    ffmpeg_filter,
                    ..
                } => {
                    assert_eq!(
                        executable, expected_executable,
                        "{encoder_name} executable path is {executable:?}"
                    );
                    assert_eq!(
                        options, expected_options,
                        "{encoder_name} options are {options:?}"
                    );
                    check_encoder_pass(pass, expected_pass, Some(encoder_name));
                    check_photon_noise(
                        photon_noise.as_ref(),
                        expected_photon_noise.as_ref(),
                        Some(encoder_name),
                    );
                    assert_eq!(
                        ffmpeg_filter.as_deref(),
                        expected_ffmpeg_filter.as_deref(),
                        "{encoder_name} FFmpeg filter is {ffmpeg_filter:?}"
                    );
                },
                _ => panic!("expected {encoder_name} to be RAV1E"),
            }
        },
        Encoder::VPX {
            executable: expected_executable,
            pass: expected_pass,
            options: expected_options,
            ffmpeg_filter: expected_ffmpeg_filter,
            ..
        } => {
            assert_matches!(encoder, Encoder::VPX { .. }, "{encoder_name} is VPX");
            match encoder {
                Encoder::VPX {
                    executable,
                    pass,
                    options,
                    ffmpeg_filter,
                    ..
                } => {
                    assert_eq!(
                        executable, expected_executable,
                        "{encoder_name} executable path is {executable:?}"
                    );
                    assert_eq!(
                        options, expected_options,
                        "{encoder_name} options are {options:?}"
                    );
                    check_encoder_pass(pass, expected_pass, Some(encoder_name));
                    assert_eq!(
                        ffmpeg_filter.as_deref(),
                        expected_ffmpeg_filter.as_deref(),
                        "{encoder_name} FFmpeg filter is {ffmpeg_filter:?}"
                    );
                },
                _ => panic!("expected {encoder_name} to be VPX"),
            }
        },
        Encoder::SVTAV1 {
            executable: expected_executable,
            pass: expected_pass,
            options: expected_options,
            photon_noise,
            ffmpeg_filter,
            ..
        } => {
            assert_matches!(encoder, Encoder::SVTAV1 { .. }, "{encoder_name} is SVT-AV1");
            match encoder {
                Encoder::SVTAV1 {
                    executable,
                    pass,
                    options,
                    photon_noise: expected_photon_noise,
                    ffmpeg_filter: expected_ffmpeg_filter,
                    ..
                } => {
                    assert_eq!(
                        executable, expected_executable,
                        "{encoder_name} executable path is {executable:?}"
                    );
                    assert_eq!(
                        options, expected_options,
                        "{encoder_name} options are {options:?}"
                    );
                    check_encoder_pass(pass, expected_pass, Some(encoder_name));
                    check_photon_noise(
                        photon_noise.as_ref(),
                        expected_photon_noise.as_ref(),
                        Some(encoder_name),
                    );
                    assert_eq!(
                        ffmpeg_filter.as_deref(),
                        expected_ffmpeg_filter.as_deref(),
                        "{encoder_name} FFmpeg filter is {ffmpeg_filter:?}"
                    );
                },
                _ => panic!("expected {encoder_name} to be SVT-AV1"),
            }
        },
        Encoder::AVM {
            executable: expected_executable,
            pass: expected_pass,
            options: expected_options,
            photon_noise: expected_photon_noise,
            ffmpeg_filter: expected_ffmpeg_filter,
            ..
        } => {
            assert_matches!(encoder, Encoder::AVM { .. }, "{encoder_name} is AVM");
            match encoder {
                Encoder::AVM {
                    executable,
                    pass,
                    options,
                    photon_noise,
                    ffmpeg_filter,
                    ..
                } => {
                    assert_eq!(
                        executable, expected_executable,
                        "{encoder_name} executable path is {executable:?}"
                    );
                    assert_eq!(
                        options, expected_options,
                        "{encoder_name} options are {options:?}"
                    );
                    check_encoder_pass(pass, expected_pass, Some(encoder_name));
                    check_photon_noise(
                        photon_noise.as_ref(),
                        expected_photon_noise.as_ref(),
                        Some(encoder_name),
                    );
                    assert_eq!(
                        ffmpeg_filter.as_deref(),
                        expected_ffmpeg_filter.as_deref(),
                        "{encoder_name} FFmpeg filter is {ffmpeg_filter:?}"
                    );
                },
                _ => panic!("expected {encoder_name} to be AVM"),
            }
        },
        Encoder::X264 {
            executable: expected_executable,
            pass: expected_pass,
            options: expected_options,
            ffmpeg_filter: expected_ffmpeg_filter,
            ..
        } => {
            assert_matches!(encoder, Encoder::X264 { .. }, "{encoder_name} is x264");
            match encoder {
                Encoder::X264 {
                    executable,
                    pass,
                    options,
                    ffmpeg_filter,
                    ..
                } => {
                    assert_eq!(
                        executable, expected_executable,
                        "{encoder_name} executable path is {executable:?}"
                    );
                    assert_eq!(
                        options, expected_options,
                        "{encoder_name} options are {options:?}"
                    );
                    check_encoder_pass(pass, expected_pass, Some(encoder_name));
                    assert_eq!(
                        ffmpeg_filter.as_deref(),
                        expected_ffmpeg_filter.as_deref(),
                        "{encoder_name} FFmpeg filter is {ffmpeg_filter:?}"
                    );
                },
                _ => panic!("expected {encoder_name} to be x264"),
            }
        },
        Encoder::X265 {
            executable: expected_executable,
            pass: expected_pass,
            options: expected_options,
            ffmpeg_filter: expected_ffmpeg_filter,
            ..
        } => {
            assert_matches!(encoder, Encoder::X265 { .. }, "{encoder_name} is x265");
            match encoder {
                Encoder::X265 {
                    executable,
                    pass,
                    options,
                    ffmpeg_filter,
                    ..
                } => {
                    assert_eq!(
                        executable, expected_executable,
                        "{encoder_name} executable path is {executable:?}"
                    );
                    assert_eq!(
                        options, expected_options,
                        "{encoder_name} options are {options:?}"
                    );
                    check_encoder_pass(pass, expected_pass, Some(encoder_name));
                    assert_eq!(
                        ffmpeg_filter.as_deref(),
                        expected_ffmpeg_filter.as_deref(),
                        "{encoder_name} FFmpeg filter is {ffmpeg_filter:?}"
                    );
                },
                _ => panic!("expected {encoder_name} to be x265"),
            }
        },
        Encoder::VVenC {
            executable: expected_executable,
            pass: expected_pass,
            options: expected_options,
            ffmpeg_filter: expected_ffmpeg_filter,
            ..
        } => {
            assert_matches!(encoder, Encoder::VVenC { .. }, "{encoder_name} is VVenC");
            match encoder {
                Encoder::VVenC {
                    executable,
                    pass,
                    options,
                    ffmpeg_filter,
                    ..
                } => {
                    assert_eq!(
                        executable, expected_executable,
                        "{encoder_name} executable path is {executable:?}"
                    );
                    assert_eq!(
                        options, expected_options,
                        "{encoder_name} options are {options:?}"
                    );
                    check_encoder_pass(pass, expected_pass, Some(encoder_name));
                    assert_eq!(
                        ffmpeg_filter.as_deref(),
                        expected_ffmpeg_filter.as_deref(),
                        "{encoder_name} FFmpeg filter is {ffmpeg_filter:?}"
                    );
                },
                _ => panic!("expected {encoder_name} to be VVenC"),
            }
        },
        Encoder::FFmpeg {
            executable: expected_executable,
            options: expected_options,
            ..
        } => {
            assert_matches!(encoder, Encoder::FFmpeg { .. }, "{encoder_name} is FFmpeg");
            match encoder {
                Encoder::FFmpeg {
                    executable,
                    options,
                    ..
                } => {
                    assert_eq!(
                        executable, expected_executable,
                        "{encoder_name} executable path is {executable:?}"
                    );
                    assert_eq!(
                        options, expected_options,
                        "{encoder_name} options are {options:?}"
                    );
                },
                _ => panic!("expected {encoder_name} to be FFmpeg"),
            }
        },
    }
}

pub fn check_scene_detector(
    scene_detector: &SceneDetectorConfig,
    expected_scene_detector: &SceneDetectorConfig,
) {
    check_input(
        scene_detector.input.as_ref(),
        expected_scene_detector.input.as_ref(),
        "Scene Detector input",
    );
    match expected_scene_detector.method {
        SceneDetectionMethod::AVSceneChange {
            minimum_length: expected_minimum_length,
            maximum_length: expected_maximum_length,
            method: expected_method,
            save_scores: expected_save_scores,
        } => {
            assert_matches!(
                scene_detector.method,
                SceneDetectionMethod::AVSceneChange { .. },
                "Scene Detector method is AVSceneChange"
            );
            match scene_detector.method {
                SceneDetectionMethod::AVSceneChange {
                    minimum_length,
                    maximum_length,
                    method,
                    save_scores,
                } => {
                    assert_eq!(
                        minimum_length, expected_minimum_length,
                        "minimum_length is {expected_minimum_length}"
                    );
                    assert_eq!(
                        maximum_length, expected_maximum_length,
                        "maximum_length is {expected_maximum_length}"
                    );
                    assert_eq!(method, expected_method, "method is {expected_method}");
                    assert_eq!(
                        save_scores, expected_save_scores,
                        "save_scores is {expected_save_scores}"
                    );
                },
                _ => panic!("expected SceneDetectionMethod::AVSceneChange"),
            }
        },
        SceneDetectionMethod::None {
            minimum_length: expected_minimum_length,
            maximum_length: expected_maximum_length,
        } => {
            assert_matches!(
                scene_detector.method,
                SceneDetectionMethod::None { .. },
                "Scene Detector method is None"
            );
            match scene_detector.method {
                SceneDetectionMethod::None {
                    minimum_length,
                    maximum_length,
                } => {
                    assert_eq!(
                        minimum_length, expected_minimum_length,
                        "minimum_length is {expected_minimum_length}"
                    );
                    assert_eq!(
                        maximum_length, expected_maximum_length,
                        "maximum_length is {expected_maximum_length}"
                    );
                },
                _ => panic!("expected SceneDetectionMethod::None"),
            };
        },
        SceneDetectionMethod::TransNetV2 {
            minimum_length: expected_minimum_length,
            maximum_length: expected_maximum_length,
            model_path: ref expected_model_path,
            save_scores: expected_save_scores,
        } => {
            assert_matches!(
                scene_detector.method,
                SceneDetectionMethod::TransNetV2 { .. },
                "Scene Detector method is TransNetV2"
            );
            match scene_detector.method {
                SceneDetectionMethod::TransNetV2 {
                    minimum_length,
                    maximum_length,
                    ref model_path,
                    save_scores,
                } => {
                    assert_eq!(
                        minimum_length, expected_minimum_length,
                        "minimum_length is {expected_minimum_length}"
                    );
                    assert_eq!(
                        maximum_length, expected_maximum_length,
                        "maximum_length is {expected_maximum_length}"
                    );
                    assert_eq!(
                        model_path, expected_model_path,
                        "model_path is {expected_model_path:?}"
                    );
                    assert_eq!(
                        save_scores, expected_save_scores,
                        "save_scores is {expected_save_scores}"
                    );
                },
                _ => panic!("expected SceneDetectionMethod::TransNetV2"),
            }
        },
    }
}

pub fn check_noise_detector(
    noise_detector: Option<&NoiseDetectorConfig>,
    expected_noise_detector: Option<&NoiseDetectorConfig>,
) {
    if let Some(expected_config) = expected_noise_detector {
        let config = noise_detector.expect("Noise Detector is Some");
        check_input(
            config.input.as_ref(),
            expected_config.input.as_ref(),
            "Noise Detector input",
        );
        assert_eq!(
            config.reference_filters, expected_config.reference_filters,
            "Noise Detector reference filters is {:?}",
            expected_config.reference_filters
        );
        assert_eq!(
            config.denoised_filters, expected_config.denoised_filters,
            "Noise Detector denoised filters is {:?}",
            expected_config.denoised_filters
        );
    } else {
        assert_matches!(noise_detector, None, "Noise Detector is None");
    }
}

pub fn check_noise_scaler(
    noise_scaler: Option<&NoiseScalerConfig>,
    expected_noise_scaler: Option<&NoiseScalerConfig>,
) {
    if let Some(expected_config) = expected_noise_scaler {
        let config = noise_scaler.expect("Noise Scaler is Some");
        assert_eq!(
            config.threshold, expected_config.threshold,
            "Noise Scaler threshold is {}",
            expected_config.threshold
        );
        assert_eq!(
            config.minimum_scaler, expected_config.minimum_scaler,
            "Noise Scaler minimum scaler is {}",
            expected_config.minimum_scaler
        );
        assert_eq!(
            config.maximum_scaler, expected_config.maximum_scaler,
            "Noise Scaler maximum scaler is {}",
            expected_config.maximum_scaler
        );
        assert_eq!(
            config.scale_chroma, expected_config.scale_chroma,
            "Noise Scaler scale chroma is {}",
            expected_config.scale_chroma
        );
    } else {
        assert_matches!(noise_scaler, None, "Noise Scaler is None");
    }
}

pub fn check_zoner(zoner: Option<&ZonerConfig>, expected_zoner: Option<&ZonerConfig>) {
    if let Some(expected_config) = expected_zoner {
        let config = zoner.expect("Zoner is Some");
        assert_eq!(
            config.zones, expected_config.zones,
            "Zoner zones are {:?}",
            expected_config.zones
        );
    } else {
        assert_matches!(zoner, None, "Zoner is None");
    }
}

pub fn check_benchmarker(
    benchmarker: &BenchmarkerConfig,
    expected_benchmarker: &BenchmarkerConfig,
) {
    assert_eq!(
        benchmarker.threshold, expected_benchmarker.threshold,
        "Benchmarker threshold is {}",
        expected_benchmarker.threshold
    );
    assert_eq!(
        benchmarker.max_memory, expected_benchmarker.max_memory,
        "Benchmarker max memory is {:?}",
        expected_benchmarker.max_memory
    );
}

pub fn check_quality_metric(
    quality_metric: &QualityMetric,
    expected_quality_metric: &QualityMetric,
    name: &str,
) {
    match expected_quality_metric {
        QualityMetric::VMAF {
            target_range: expected_target_range,
            resolution: expected_resolution,
            scaler: expected_scaler,
            threads: expected_threads,
            model: expected_model,
            features: expected_features,
        } => {
            assert_matches!(
                quality_metric,
                QualityMetric::VMAF { .. },
                "{name} metric is VMAF"
            );
            match &quality_metric {
                QualityMetric::VMAF {
                    target_range,
                    resolution,
                    scaler,
                    threads,
                    model,
                    features,
                } => {
                    assert_eq!(
                        target_range, expected_target_range,
                        "{name} VMAF target range is {}-{}",
                        expected_target_range.0, expected_target_range.1
                    );
                    assert_eq!(
                        resolution, expected_resolution,
                        "{name} VMAF resolution is {expected_resolution:?}"
                    );
                    assert_eq!(
                        scaler, expected_scaler,
                        "{name} VMAF scaler is {expected_scaler}"
                    );
                    assert_eq!(
                        threads, expected_threads,
                        "{name} VMAF threads is {expected_threads}"
                    );
                    assert_eq!(
                        model, expected_model,
                        "{name} VMAF model is {expected_model:?}"
                    );
                    assert_eq!(
                        features, expected_features,
                        "{name} VMAF features are {expected_features:?}"
                    );
                },
                other => panic!("expected QualityMetric::VMAF. Got {other:?}"),
            }
        },
        QualityMetric::SSIMULACRA2 {
            target_range,
            resolution,
            threads,
            gpu_id,
        } => {
            assert_matches!(
                quality_metric,
                QualityMetric::SSIMULACRA2 { .. },
                "{name} metric is SSIMULACRA2"
            );
            match &quality_metric {
                QualityMetric::SSIMULACRA2 {
                    target_range: expected_target_range,
                    resolution: expected_resolution,
                    threads: expected_threads,
                    gpu_id: expected_gpu_id,
                } => {
                    assert_eq!(
                        expected_target_range, target_range,
                        "{name} SSIMULACRA2 target range is {}-{}",
                        target_range.0, target_range.1
                    );
                    assert_eq!(
                        expected_resolution, resolution,
                        "{name} SSIMULACRA2 resolution is {resolution:?}"
                    );
                    assert_eq!(
                        expected_threads, threads,
                        "{name} SSIMULACRA2 threads is {threads:?}"
                    );
                    assert_eq!(
                        expected_gpu_id, gpu_id,
                        "{name} SSIMULACRA2 gpu_id is {gpu_id:?}"
                    );
                },
                other => panic!("expected QualityMetric::SSIMULACRA2. Got {other:?}"),
            }
        },
        QualityMetric::BUTTERAUGLI {
            target_range,
            resolution,
            threads,
            intensity_multiplier,
            norm,
            gpu_id,
        } => {
            assert_matches!(
                quality_metric,
                QualityMetric::BUTTERAUGLI { .. },
                "{name} metric is BUTTERAUGLI"
            );
            match &quality_metric {
                QualityMetric::BUTTERAUGLI {
                    target_range: expected_target_range,
                    resolution: expected_resolution,
                    threads: expected_threads,
                    intensity_multiplier: expected_intensity_multiplier,
                    norm: expected_norm,
                    gpu_id: expected_gpu_id,
                } => {
                    assert_eq!(
                        expected_target_range, target_range,
                        "{name} BUTTERAUGLI target range is {}-{}",
                        target_range.0, target_range.1
                    );
                    assert_eq!(
                        expected_resolution, resolution,
                        "{name} BUTTERAUGLI resolution is {resolution:?}"
                    );
                    assert_eq!(
                        expected_threads, threads,
                        "{name} BUTTERAUGLI threads is {threads:?}"
                    );
                    assert_eq!(
                        expected_intensity_multiplier, intensity_multiplier,
                        "{name} BUTTERAUGLI intensity multiplier is {intensity_multiplier:?}"
                    );
                    assert_eq!(expected_norm, norm, "{name} BUTTERAUGLI norm is {norm:?}");
                    assert_eq!(
                        expected_gpu_id, gpu_id,
                        "{name} BUTTERAUGLI gpu_id is {gpu_id:?}"
                    );
                },
                other => panic!("expected QualityMetric::BUTTERAUGLI. Got {other:?}"),
            }
        },
        QualityMetric::CVVDP {
            target_range,
            resolution,
            display_model,
            resize_to_display,
            disable_temporal,
            gpu_id,
        } => {
            assert_matches!(
                quality_metric,
                QualityMetric::CVVDP { .. },
                "{name} metric is CVVDP"
            );
            match &quality_metric {
                QualityMetric::CVVDP {
                    target_range: expected_target_range,
                    resolution: expected_resolution,
                    display_model: expected_display_model,
                    resize_to_display: expected_resize_to_display,
                    disable_temporal: expected_disable_temporal,
                    gpu_id: expected_gpu_id,
                } => {
                    assert_eq!(
                        expected_target_range, target_range,
                        "{name} CVVDP target range is {}-{}",
                        target_range.0, target_range.1
                    );
                    assert_eq!(
                        expected_resolution, resolution,
                        "{name} CVVDP resolution is {resolution:?}"
                    );
                    assert_eq!(
                        expected_display_model, display_model,
                        "{name} CVVDP display model is {display_model:?}"
                    );
                    assert_eq!(
                        expected_resize_to_display, resize_to_display,
                        "{name} CVVDP resize to display is {resize_to_display:?}"
                    );
                    assert_eq!(
                        expected_disable_temporal, disable_temporal,
                        "{name} CVVDP disable temporal is {disable_temporal:?}"
                    );
                    assert_eq!(expected_gpu_id, gpu_id, "{name} CVVDP gpu_id is {gpu_id:?}");
                },
                other => panic!("expected QualityMetric::CVVDP. Got {other:?}"),
            }
        },
        QualityMetric::XPSNR {
            target_range,
            resolution,
        } => {
            assert_matches!(
                quality_metric,
                QualityMetric::XPSNR { .. },
                "{name} metric is XPSNR"
            );
            match &quality_metric {
                QualityMetric::XPSNR {
                    target_range: expected_target_range,
                    resolution: expected_resolution,
                } => {
                    assert_eq!(
                        expected_target_range, target_range,
                        "{name} XPSNR target range is {}-{}",
                        target_range.0, target_range.1
                    );
                    assert_eq!(
                        expected_resolution, resolution,
                        "{name} XPSNR resolution is {resolution:?}"
                    );
                },
                other => panic!("expected QualityMetric::XPSNR. Got {other:?}"),
            }
        },
    }
}

pub fn check_target_quality(
    target_quality: Option<&TargetQualityConfig>,
    expected_target_quality: Option<&TargetQualityConfig>,
) {
    if let Some(expected_target_quality) = expected_target_quality {
        let target_quality = target_quality.expect("Target Quality is Some");
        check_input(
            target_quality.input.as_ref(),
            expected_target_quality.input.as_ref(),
            "Target Quality input",
        );
        check_input(
            target_quality.metric_input.as_ref(),
            expected_target_quality.metric_input.as_ref(),
            "Target Quality metric input",
        );
        check_quality_metric(
            &target_quality.metric,
            &expected_target_quality.metric,
            "Target Quality",
        );
        assert_eq!(
            target_quality.maximum_probes, expected_target_quality.maximum_probes,
            "Target Quality maximum probes is {}",
            expected_target_quality.maximum_probes
        );
        assert_eq!(
            target_quality.quantizer_range, expected_target_quality.quantizer_range,
            "Target Quality quantizer range is {}-{}",
            expected_target_quality.quantizer_range.0, expected_target_quality.quantizer_range.1
        );
        assert_eq!(
            target_quality.interpolators, expected_target_quality.interpolators,
            "Target Quality interpolators are {:?}",
            expected_target_quality.interpolators
        );
        assert_eq!(
            target_quality.probing.statistic, expected_target_quality.probing.statistic,
            "Target Quality probing statistic is {:?}",
            expected_target_quality.probing.statistic
        );
        assert_eq!(
            target_quality.probing.strategy, expected_target_quality.probing.strategy,
            "Target Quality probing strategy is {:?}",
            expected_target_quality.probing.strategy
        );
        assert_eq!(
            target_quality.probing.encoder_options, expected_target_quality.probing.encoder_options,
            "Target Quality probing encoder options are {:?}",
            expected_target_quality.probing.encoder_options
        );
    } else {
        assert!(
            expected_target_quality.is_none(),
            "Expected Target Quality is None"
        );
    }
}

pub fn check_bitrate_optimizer(
    bitrate_optimizer: &BitrateOptimizerConfig,
    expected_bitrate_optimizer: &BitrateOptimizerConfig,
) {
    assert_eq!(
        bitrate_optimizer.bitrate_sigma_threshold,
        expected_bitrate_optimizer.bitrate_sigma_threshold,
        "Bitrate Optimizer bitrate sigma threshold is {:?}",
        expected_bitrate_optimizer.bitrate_sigma_threshold
    );
}

pub fn check_speed_scaler(
    speed_scaler: &SpeedScalerConfig,
    expected_speed_scaler: &SpeedScalerConfig,
) {
    assert_eq!(
        speed_scaler.speed_quantizers, expected_speed_scaler.speed_quantizers,
        "Speed Scaler speed quantizers are {:?}",
        expected_speed_scaler.speed_quantizers
    );
}

pub fn check_parallel_encoder(
    parallel_encoder: &ParallelEncoderConfig,
    expected_parallel_encoder: &ParallelEncoderConfig,
) {
    check_input(
        parallel_encoder.input.as_ref(),
        expected_parallel_encoder.input.as_ref(),
        "Parallel Encoder input",
    );
    assert_eq!(
        parallel_encoder.workers, expected_parallel_encoder.workers,
        "Parallel Encoder workers is {:?}",
        expected_parallel_encoder.workers
    );
}

pub fn check_scene_concatenator(
    scene_concatentor: &SceneConcatenatorConfig,
    expected_scene_concatenator: &SceneConcatenatorConfig,
) {
    assert_eq!(
        scene_concatentor.method, expected_scene_concatenator.method,
        "Scene Concatenator method is {}",
        expected_scene_concatenator.method
    );
    assert_eq!(
        scene_concatentor.scenes_directory,
        expected_scene_concatenator.scenes_directory,
        "Scene Concatenator scenes directory is {}",
        expected_scene_concatenator.scenes_directory.as_path().display(),
    );
    assert_eq!(
        scene_concatentor.output, expected_scene_concatenator.output,
        "Scene Concatenator output is {:?}",
        expected_scene_concatenator.output
    );
}

pub fn check_quality_check(
    quality_check: Option<&QualityCheckConfig>,
    expected_quality_check: Option<&QualityCheckConfig>,
) {
    if let Some(expected_quality_check) = expected_quality_check {
        let quality_check = quality_check.expect("Quality Check is Some");
        check_input(
            quality_check.input.as_ref(),
            expected_quality_check.input.as_ref(),
            "Quality Check input",
        );
        check_quality_metric(
            &quality_check.metric,
            &expected_quality_check.metric,
            "Quality Check",
        );
        assert_eq!(
            quality_check.statistic, expected_quality_check.statistic,
            "Quality Check probing statistic is {:?}",
            expected_quality_check.statistic
        );
        assert_eq!(
            quality_check.strategy, expected_quality_check.strategy,
            "Quality Check probing strategy is {:?}",
            expected_quality_check.strategy
        );
    } else {
        assert!(
            expected_quality_check.is_none(),
            "Expected Quality Check is None"
        );
    }
}
