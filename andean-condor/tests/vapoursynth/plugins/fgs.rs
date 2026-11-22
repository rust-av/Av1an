//! Public table publication and bounded AOM/dav1d grain acceptance.
use andean_condor::{
    models::encoder::{Encoder, EncoderBase, EncoderSettingsError, photon_noise::PhotonNoise},
    vapoursynth::{plugins::fgs::FGS, script_builder::VapourSynthPluginScript},
};
use av1_grain::{GrainTableSegment, TransferFunction};

fn noise(y: usize, cb: usize, cr: usize) -> PhotonNoise {
    let coefficients = |len| (0..len).map(|i| [-128, 0, 127][i % 3]).collect();
    PhotonNoise {
        iso: 800,
        chroma_iso: Some(800),
        width: Some(1920),
        height: Some(1080),
        c_y: Some(coefficients(y)),
        ccb: Some(coefficients(cb)),
        ccr: Some(coefficients(cr)),
    }
}

fn encoder(noise: &PhotonNoise) -> Encoder {
    let mut encoder = Encoder::default_from_base(&EncoderBase::AOM, true);
    encoder.set_photon_noise(Some(noise.clone()));
    encoder
}

fn table(noise: &PhotonNoise) -> GrainTableSegment {
    encoder(noise)
        .generate_photon_noise_table(64, 64, TransferFunction::BT1886, None)
        .expect("valid table")
        .expect("configured grain")
        .1
}

#[test]
fn unequal_chroma_iso_refuses_table_before_writing() {
    let temp = tempfile::tempdir().expect("table publication fixture");
    let mut noise = noise(0, 1, 1);
    noise.chroma_iso = Some(400);
    let encoder = encoder(&noise);
    for existing in [false, true] {
        let path = temp.path().join(if existing { "sentinel.tbl" } else { "new.tbl" });
        if existing {
            std::fs::write(&path, b"caller-owned table").expect("sentinel");
        }
        let error = encoder
            .generate_photon_noise_table(64, 64, TransferFunction::BT1886, None)
            .and_then(|generated| {
                av1_grain::write_grain_table(&path, &[generated.expect("grain").1])
            })
            .expect_err("unequal chroma ISO must fail before table output");
        assert_eq!(
            error.downcast_ref::<EncoderSettingsError>(),
            Some(&EncoderSettingsError::UnequalChromaIso {
                iso: 800,
                chroma_iso: 400
            }),
            "typed settings error: {error:#}"
        );
        if existing {
            assert_eq!(
                std::fs::read(&path).expect("preserved table"),
                b"caller-owned table"
            );
        } else {
            assert!(!path.exists());
        }
    }
    assert_eq!(
        encoder.validate_settings(),
        Err(EncoderSettingsError::UnequalChromaIso {
            iso: 800,
            chroma_iso: 400
        })
    );
}

#[test]
fn table_derives_all_legal_lags_without_changing_coefficients() {
    for (lag, y, uv) in [(0, 0, 1), (1, 4, 5), (2, 12, 13), (3, 24, 25)] {
        for chroma_iso in [None, Some(400), Some(800)] {
            let mut noise = noise(y, uv, uv);
            noise.chroma_iso = chroma_iso;
            if chroma_iso == Some(400) {
                assert!(
                    encoder(&noise)
                        .generate_photon_noise_table(64, 64, TransferFunction::BT1886, None)
                        .is_err(),
                    "unequal ISO rejected for lag {lag}"
                );
                continue;
            }
            let segment = table(&noise);
            assert_eq!(segment.ar_coeff_lag, lag);
            assert_eq!(
                segment.ar_coeffs_y.as_slice(),
                noise.c_y.as_deref().expect("Y")
            );
            assert_eq!(
                segment.ar_coeffs_cb.as_slice(),
                noise.ccb.as_deref().expect("Cb")
            );
            assert_eq!(
                segment.ar_coeffs_cr.as_slice(),
                noise.ccr.as_deref().expect("Cr")
            );
            assert!(!segment.scaling_points_y.is_empty());
            assert!(segment.scaling_points_cb.is_empty() && segment.scaling_points_cr.is_empty());
            assert_eq!(segment.chroma_scaling_from_luma, chroma_iso == Some(800));
            let temp = tempfile::tempdir().expect("table fixture");
            let path = temp.path().join("grain.tbl");
            av1_grain::write_grain_table(&path, std::slice::from_ref(&segment)).expect("write");
            let text = std::fs::read_to_string(path).expect("table");
            for (name, count) in [("cY", y), ("cCb", uv), ("cCr", uv)] {
                let line = text
                    .lines()
                    .find(|line| line.trim_start().starts_with(name))
                    .expect("coefficient line");
                assert_eq!(line.split_whitespace().count(), count + 1);
            }
        }
    }
}

fn invalid_shapes() -> [PhotonNoise; 3] {
    [noise(3, 1, 1), noise(4, 1, 5), noise(24, 25, 26)]
}

#[test]
fn invalid_grain_shape_fails_before_executable_lookup_or_native_output() {
    let temp = tempfile::tempdir().expect("preflight fixture");
    let clip_info = andean_condor::core::input::clip_info::ClipInfo {
        num_frames: 1,
        format_info: andean_condor::core::input::pixel_format::PixelFormat::VapourSynth {
            bit_depth: 8,
        },
        frame_rate: av_format::rational::Rational64::new(24, 1),
        resolution: (64, 64),
        color_range: None,
        color_primaries: None,
        color_transfer_characteristic: None,
        color_space: None,
        chroma_location: None,
        mastering_display: None,
        content_light_level: None,
    };
    for noise in invalid_shapes() {
        let mut encoder = encoder(&noise);
        encoder.set_executable(Some(temp.path().join("nonexistent-encoder")));
        let error = encoder.validate().expect_err("shape error precedes executable lookup");
        assert!(error.to_string().contains("coefficients"), "{error:#}");
        let (sender, receiver) = crossbeam_channel::unbounded();
        drop(sender);
        let (progress, _receiver) = std::sync::mpsc::channel();
        let error = encoder
            .encode_with_stream(
                receiver,
                &temp.path().join("no-scratch/out.ivf"),
                progress,
                &clip_info,
                &andean_condor::utils::cancellation::Cancellation::default(),
            )
            .expect_err("public native attempt must reject before spawn/scratch/output");
        assert!(error.to_string().contains("coefficients"), "{error:#}");
    }
    assert_eq!(
        std::fs::read_dir(temp.path()).expect("no scratch or output").count(),
        0
    );
}

#[test]
fn invalid_grain_shape_writes_no_table_or_script() {
    let temp = tempfile::tempdir().expect("preflight fixture");
    for noise in invalid_shapes() {
        let encoder = encoder(&noise);
        let table_path = temp.path().join("must-not-write.tbl");
        let result = encoder
            .generate_photon_noise_table(64, 64, TransferFunction::BT1886, None)
            .and_then(|generated| {
                let (_, segment) = generated.expect("grain enabled");
                av1_grain::write_grain_table(&table_path, &[segment])
            });
        assert!(result.is_err());
        assert!(!table_path.exists());
        let fgs = FGS {
            photon_noise: noise,
            dynamic_seed: None,
            simd_mask: None,
        };
        assert!(fgs.generate_script("clip".into()).is_err());
    }
    assert_eq!(
        std::fs::read_dir(temp.path()).expect("no scratch or output").count(),
        0
    );
}

// Native proof is deliberately CPU-only and finite, not a throughput measurement.
#[cfg(unix)]
#[test]
fn native_aom_accepts_all_lags_and_dav1d_decodes_grain_including_monochrome() {
    use std::{fs, process::Command};
    let timeout =
        which::which("timeout").expect("timeout is required for bounded native grain-validation");
    let aomenc = which::which("aomenc").expect("aomenc is required for native grain-validation");
    let ffmpeg =
        which::which("ffmpeg").expect("ffmpeg/libdav1d is required for native grain-validation");
    let run = |program: &std::path::Path, args: &[String]| {
        let output = Command::new(&timeout)
            .args(["-s", "KILL", "30"])
            .arg(program)
            .args(args)
            .output()
            .expect("bounded native command");
        assert!(
            output.status.success(),
            "{program:?} {args:?}: {:?}\n{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    };
    let help = run(&aomenc, &["--help".into()]);
    eprintln!(
        "grain-validation {}",
        String::from_utf8_lossy(&help.stdout)
            .lines()
            .find(|line| line.contains("Encoder v"))
            .expect("native AOM version")
    );
    let temp = tempfile::tempdir().expect("finite native fixtures");
    for (lag, y, uv) in [(0, 0, 1), (1, 4, 5), (2, 12, 13), (3, 24, 25)] {
        for mode in ["chroma-from-luma", "inactive-chroma", "monochrome"] {
            let root = temp.path().join(format!("lag{lag}-{mode}"));
            fs::create_dir_all(&root).expect("independent job");
            let mono = mode == "monochrome";
            let mut noise = noise(y, uv, uv);
            noise.chroma_iso = (mode == "chroma-from-luma").then_some(800);
            let segment = table(&noise);
            assert_eq!(segment.ar_coeff_lag, lag);
            let table_path = root.join("grain.tbl");
            av1_grain::write_grain_table(&table_path, &[segment]).expect("public route's table");
            let input = root.join("one.y4m");
            let format = if mono { "mono" } else { "420jpeg" };
            let mut frame =
                format!("YUV4MPEG2 W64 H64 F24:1 Ip A1:1 C{format}\nFRAME\n").into_bytes();
            frame.extend(vec![96; 64 * 64]);
            if !mono {
                frame.extend(vec![128; 64 * 64 / 2]);
            }
            fs::write(&input, frame).expect("one finite frame");
            let output = root.join("one.ivf");
            let mut args: Vec<String> = [
                "--passes=1",
                "--cpu-used=6",
                "--threads=1",
                "--ivf",
                "--limit=1",
                "--lag-in-frames=0",
                "--end-usage=q",
                "--cq-level=32",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect();
            args.push(format!("--film-grain-table={}", table_path.display()));
            if mono {
                args.push("--monochrome".into());
            }
            args.extend(["-o".into(), output.display().to_string(), input.display().to_string()]);
            run(&aomenc, &args);
            let mut decoded = Vec::new();
            for grain in ["0", "1"] {
                let raw = root.join(format!("grain{grain}.yuv"));
                let args: Vec<String> = [
                    "-v",
                    "error",
                    "-nostdin",
                    "-c:v",
                    "libdav1d",
                    "-threads",
                    "1",
                    "-filmgrain",
                    grain,
                    "-i",
                ]
                .into_iter()
                .map(str::to_owned)
                .chain([
                    output.display().to_string(),
                    "-frames:v".into(),
                    "1".into(),
                    "-pix_fmt".into(),
                    if mono {
                        "gray".into()
                    } else {
                        "yuv420p".into()
                    },
                    "-f".into(),
                    "rawvideo".into(),
                    raw.display().to_string(),
                ])
                .collect();
                run(&ffmpeg, &args);
                let bytes = fs::read(raw).expect("independent decoded frame");
                assert_eq!(bytes.len(), if mono { 4096 } else { 6144 });
                decoded.push(bytes);
            }
            assert_ne!(
                decoded[0], decoded[1],
                "lag {lag} {mode}: decoder must actually apply table grain"
            );
            eprintln!(
                "grain-validation native lag={lag} mode={mode}: aomenc accepted; dav1d decoded {} bytes with/without grain, distinct=true",
                decoded[0].len()
            );
        }
    }
}
