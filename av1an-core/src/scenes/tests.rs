use std::str::FromStr;

use crate::{
    context::Av1anContext,
    encoder::Encoder,
    scenes::{Scene, SceneFactory},
    InterpolationMethod,
    ProbingStatistic,
    TargetMetric,
    TargetQuality,
};

fn get_test_args() -> Av1anContext {
    use std::path::PathBuf;

    use crate::{
        concat::ConcatMethod,
        ffmpeg::FFPixelFormat,
        into_vec,
        settings::{EncodeArgs, InputPixelFormat, PixelFormat},
        ChunkMethod,
        ChunkOrdering,
        Input,
        ScenecutMethod,
        SplitMethod,
        Verbosity,
    };

    let args = EncodeArgs {
        ffmpeg_filter_args:    Vec::new(),
        temp:                  String::new(),
        force:                 false,
        no_defaults:           false,
        passes:                2,
        video_params:          into_vec!["--cq-level=40", "--cpu-used=0", "--aq-mode=1"],
        output_file:           String::new(),
        audio_params:          Vec::new(),
        chunk_method:          ChunkMethod::LSMASH,
        chunk_order:           ChunkOrdering::Random,
        concat:                ConcatMethod::FFmpeg,
        encoder:               Encoder::aom,
        extra_splits_len:      Some(100),
        photon_noise:          Some(10),
        photon_noise_size:     (None, None),
        chroma_noise:          false,
        sc_pix_format:         None,
        keep:                  false,
        max_tries:             3,
        min_scene_len:         10,
        input_pix_format:      InputPixelFormat::FFmpeg {
            format: FFPixelFormat::YUV420P10LE,
        },
        input:                 Input::Video {
            path:         PathBuf::new(),
            temp:         String::new(),
            chunk_method: ChunkMethod::LSMASH,
            is_proxy:     false,
        },
        proxy:                 None,
        output_pix_format:     PixelFormat {
            format:    FFPixelFormat::YUV420P10LE,
            bit_depth: 10,
        },
        resume:                false,
        scenes:                None,
        split_method:          SplitMethod::AvScenechange,
        sc_method:             ScenecutMethod::Standard,
        sc_only:               false,
        sc_downscale_height:   None,
        force_keyframes:       Vec::new(),
        target_quality:        TargetQuality::default("", Encoder::aom),
        vmaf:                  false,
        verbosity:             Verbosity::Normal,
        workers:               1,
        tiles:                 (1, 1),
        tile_auto:             false,
        set_thread_affinity:   None,
        zones:                 None,
        scaler:                String::new(),
        ignore_frame_mismatch: false,
        vmaf_path:             None,
        vmaf_res:              "1920x1080".to_string(),
        vmaf_threads:          None,
        vmaf_filter:           None,
        probe_res:             None,
        vapoursynth_plugins:   None,
    };
    Av1anContext {
        vs_script: None,
        vs_proxy_script: None,
        frames: 6900,
        args,
        scene_factory: SceneFactory::new(),
    }
}

#[test]
fn validate_zones_args() {
    let input = "45 729 aom --cq-level=20 --photon-noise 4 -x 60 --min-scene-len 12";
    let args = get_test_args();
    let result = Scene::parse_from_zone(input, &args.args, args.frames)
        .expect("should parse zone successfully");
    assert_eq!(result.start_frame, 45);
    assert_eq!(result.end_frame, 729);

    let zone_overrides = result.zone_overrides.expect("zone overrides should exist");
    assert_eq!(zone_overrides.encoder, Encoder::aom);
    assert_eq!(zone_overrides.extra_splits_len, Some(60));
    assert_eq!(zone_overrides.min_scene_len, 12);
    assert_eq!(zone_overrides.photon_noise, Some(4));
    assert!(!zone_overrides.video_params.contains(&"--cq-level=40".to_owned()));
    assert!(zone_overrides.video_params.contains(&"--cq-level=20".to_owned()));
    assert!(zone_overrides.video_params.contains(&"--cpu-used=0".to_owned()));
    assert!(zone_overrides.video_params.contains(&"--aq-mode=1".to_owned()));
}

#[test]
fn validate_rav1e_zone_with_photon_noise() {
    let input = "45 729 rav1e reset --speed 6 --photon-noise 4";
    let args = get_test_args();
    let result = Scene::parse_from_zone(input, &args.args, args.frames)
        .expect("should parse zone successfully");
    assert_eq!(result.start_frame, 45);
    assert_eq!(result.end_frame, 729);

    let zone_overrides = result.zone_overrides.expect("zone overrides should exist");
    assert_eq!(zone_overrides.encoder, Encoder::rav1e);
    assert_eq!(zone_overrides.photon_noise, Some(4));
    assert!(zone_overrides
        .video_params
        .windows(2)
        .any(|window| window[0] == "--speed" && window[1] == "6"));
}

#[test]
fn validate_zones_reset() {
    let input = "729 1337 aom reset --cq-level=20 --cpu-used=5";
    let args = get_test_args();
    let result = Scene::parse_from_zone(input, &args.args, args.frames)
        .expect("should parse zone successfully");
    assert_eq!(result.start_frame, 729);
    assert_eq!(result.end_frame, 1337);

    let zone_overrides = result.zone_overrides.expect("zone overrides should exist");
    assert_eq!(zone_overrides.encoder, Encoder::aom);
    // In the current implementation, scenecut settings should be preserved
    // unless manually overridden. Settings which affect the encoder,
    // including photon noise, should be reset.
    assert_eq!(zone_overrides.extra_splits_len, Some(100));
    assert_eq!(zone_overrides.min_scene_len, 10);
    assert_eq!(zone_overrides.photon_noise, None);
    assert!(!zone_overrides.video_params.contains(&"--cq-level=40".to_owned()));
    assert!(!zone_overrides.video_params.contains(&"--cpu-used=0".to_owned()));
    assert!(!zone_overrides.video_params.contains(&"--aq-mode=1".to_owned()));
    assert!(zone_overrides.video_params.contains(&"--cq-level=20".to_owned()));
    assert!(zone_overrides.video_params.contains(&"--cpu-used=5".to_owned()));
}

#[test]
fn validate_zones_encoder_changed() {
    let input = "729 1337 rav1e reset -s 3 -q 45";
    let args = get_test_args();
    let result = Scene::parse_from_zone(input, &args.args, args.frames)
        .expect("should parse zone successfully");
    assert_eq!(result.start_frame, 729);
    assert_eq!(result.end_frame, 1337);

    let zone_overrides = result.zone_overrides.expect("zone overrides should exist");
    assert_eq!(zone_overrides.encoder, Encoder::rav1e);
    assert_eq!(zone_overrides.extra_splits_len, Some(100));
    assert_eq!(zone_overrides.min_scene_len, 10);
    assert_eq!(zone_overrides.photon_noise, None);
    assert!(!zone_overrides.video_params.contains(&"--cq-level=40".to_owned()));
    assert!(!zone_overrides.video_params.contains(&"--cpu-used=0".to_owned()));
    assert!(!zone_overrides.video_params.contains(&"--aq-mode=1".to_owned()));
    assert!(zone_overrides
        .video_params
        .windows(2)
        .any(|window| window[0] == "-s" && window[1] == "3"));
    assert!(zone_overrides
        .video_params
        .windows(2)
        .any(|window| window[0] == "-q" && window[1] == "45"));
}

#[test]
fn validate_zones_encoder_changed_no_reset() {
    let input = "729 1337 rav1e -s 3 -q 45";
    let args = get_test_args();
    let result = Scene::parse_from_zone(input, &args.args, args.frames);
    assert_eq!(
        result.expect_err("result should be an error").to_string(),
        "Zone includes encoder change but previous args were kept. You probably meant to specify \
         \"reset\"."
    );
}

#[test]
fn validate_zones_no_args() {
    let input = "2459 5000 rav1e";
    let args = get_test_args();
    let result = Scene::parse_from_zone(input, &args.args, args.frames);
    assert_eq!(
        result.expect_err("result should be an error").to_string(),
        "Zone includes encoder change but previous args were kept. You probably meant to specify \
         \"reset\"."
    );
}

#[test]
fn validate_zones_format_mismatch() {
    let input = "5000 -1 x264 reset";
    let args = get_test_args();
    let result = Scene::parse_from_zone(input, &args.args, args.frames);
    assert_eq!(
        result.expect_err("result should be an error").to_string(),
        "Zone specifies using x264, but this cannot be used in the same file as aom"
    );
}

#[test]
fn validate_zones_no_args_reset() {
    let input = "5000 -1 rav1e reset";
    let args = get_test_args();

    // This is weird, but can technically work for some encoders so we'll allow it.
    let result = Scene::parse_from_zone(input, &args.args, args.frames)
        .expect("should parse zone successfully");
    assert_eq!(result.start_frame, 5000);
    assert_eq!(result.end_frame, 6900);

    let zone_overrides = result.zone_overrides.expect("zone overrides should exist");
    assert_eq!(zone_overrides.encoder, Encoder::rav1e);
    assert_eq!(zone_overrides.extra_splits_len, Some(100));
    assert_eq!(zone_overrides.min_scene_len, 10);
    assert_eq!(zone_overrides.photon_noise, None);
    assert!(zone_overrides.video_params.is_empty());
}

#[test]
fn validate_zones_target_quality() {
    let desired_target = 85;
    let desired_metric = TargetMetric::SSIMULACRA2;
    let desired_probing_rate = 2;
    let desired_probes = 2;
    let desired_qp_min = 8;
    let desired_qp_max = 35;
    let desired_probe_res = (1280, 720);
    let desired_probing_stat = ProbingStatistic {
        name:  crate::ProbingStatisticName::Mode,
        value: None,
    };
    let desired_interpolation_method4 = "natural";
    let desired_interpolation_method5 = "cubic";

    let input = format!(
        "0 -1 svt-av1 reset --preset 3 --target-quality {target} --target-metric {metric} \
         --probing-rate {rate} --probes {probes} --qp-range {min_q}-{max_q} --probe-res \
         {width}x{height} --probing-stat {stat} --interp-method {method4}-{method5}",
        target = desired_target,
        metric = desired_metric,
        rate = desired_probing_rate,
        probes = desired_probes,
        min_q = desired_qp_min,
        max_q = desired_qp_max,
        width = desired_probe_res.0,
        height = desired_probe_res.1,
        stat = desired_probing_stat.name,
        method4 = desired_interpolation_method4,
        method5 = desired_interpolation_method5,
    );
    let args = get_test_args();

    let result = Scene::parse_from_zone(&input, &args.args, args.frames)
        .expect("should parse zone successfully");
    let zone_overrides = result.zone_overrides.expect("should have zone overrides");
    assert!(zone_overrides.target_quality.is_some());
    let target_quality = zone_overrides.target_quality.expect("should have target quality");
    assert!(target_quality.target.is_some());
    assert!(target_quality
        .target
        .is_some_and(|(low, high)| low >= (desired_target as f64 * 0.99)
            && high >= (desired_target as f64 * 1.01)));
    assert_eq!(target_quality.metric, desired_metric);
    assert_eq!(target_quality.probing_rate, desired_probing_rate);
    assert_eq!(target_quality.probes, desired_probes);
    assert_eq!(target_quality.min_q, desired_qp_min);
    assert_eq!(target_quality.max_q, desired_qp_max);
    assert_eq!(target_quality.probe_res, Some(desired_probe_res));
    assert_eq!(
        target_quality.probing_statistic.name.to_string(),
        desired_probing_stat.name.to_string()
    );
    assert_eq!(
        target_quality.interp_method,
        Some((
            InterpolationMethod::from_str(desired_interpolation_method4)
                .expect("should be a valid InterpolationMethod"),
            InterpolationMethod::from_str(desired_interpolation_method5)
                .expect("should be a valid InterpolationMethod")
        ))
    );
}
