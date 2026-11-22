use super::*;
use andean_condor::{
    core::{
        Condor,
        input::{Input, color_range::ColorRange},
        output::Output,
        sequence::{Sequence, parallel_encoder::ParallelEncoder},
    },
    models::{
        input::{Input as InputModel, VapourSynthScriptSource},
        scene::Scene,
        sequence::{DefaultSequenceConfig, DefaultSequenceData},
    },
};
use av1_grain::{GrainTableSegment, NoiseGenArgs, generate_photon_noise_params};
use std::{collections::HashMap, fs, io::Cursor, path::PathBuf, sync::mpsc};

fn noise() -> PhotonNoise {
    PhotonNoise {
        iso: 800,
        chroma_iso: Some(800),
        width: None,
        height: None,
        c_y: Some(vec![1, 2, 3, 4]),
        ccb: Some(vec![5, 6, 7, 8, 9]),
        ccr: Some(vec![-5, -6, -7, -8, -9]),
    }
}

fn encoder(noise: &PhotonNoise, base: EncoderBase) -> Encoder {
    let mut encoder = Encoder::default_from_base(&base, true);
    encoder.set_photon_noise(Some(noise.clone()));
    encoder
}

fn expected_params(
    noise: &PhotonNoise,
    width: u32,
    height: u32,
    transfer: TransferFunction,
    full_range: bool,
) -> GrainTableSegment {
    let args = NoiseGenArgs {
        iso_setting: noise.iso,
        width,
        height,
        transfer_function: transfer,
        chroma_grain: noise.chroma_iso == Some(noise.iso),
        random_seed: None,
        full_range,
    };
    let mut expected = generate_photon_noise_params(0, u64::MAX, args);
    expected.ar_coeff_lag = 1;
    expected.ar_coeffs_y = noise.c_y.as_ref().expect("fixture Y").iter().copied().collect();
    expected.ar_coeffs_cb = noise.ccb.as_ref().expect("fixture Cb").iter().copied().collect();
    expected.ar_coeffs_cr = noise.ccr.as_ref().expect("fixture Cr").iter().copied().collect();
    expected
}

fn input(width: u32, height: u32, transfer: u8, range: u8) -> Input {
    Input::from_data(&InputModel::VapourSynthScript {
        source: VapourSynthScriptSource::Text(format!(
            "from vapoursynth import core, YUV420P8\nclip = core.std.BlankClip(width={width}, height={height}, length=1, format=YUV420P8)\nclip = core.std.SetFrameProps(clip, _Transfer={transfer}, _ColorRange={range})\nclip.set_output()\n"
        )), variables: HashMap::new(), index: 0, filters: vec![], stream_concurrently: true,
    }).expect("one-frame metadata fixture")
}

#[test]
fn parallel_initialization_writes_distinct_source_specific_grain_tables() {
    initialize_grain_tables(false);
}

#[test]
fn parallel_initialization_reuses_tables_without_rewriting_or_removing_legacy_files() {
    initialize_grain_tables(true);
}

fn initialize_grain_tables(check_reuse: bool) {
    let temp = tempfile::tempdir().expect("owned cache fixture");
    let noise = noise();
    let encoder = encoder(&noise, EncoderBase::AOM);
    let mut config = DefaultSequenceConfig::default();
    config.parallel_encoder.scenes_directory = temp.path().join("scenes");
    let directory = config.parallel_encoder.scenes_directory.join("Parallel Encoder");
    fs::create_dir_all(&directory).expect("cache directory");
    let legacy = directory.join(format!("{}.tbl", noise.hash_name()));
    fs::write(
        &legacy,
        "caller-owned malformed pre-grain validation legacy table",
    )
    .expect("legacy sentinel");
    let legacy_bytes = fs::read(&legacy).expect("legacy contents");
    let mut condor = Condor::new(
        input(1920, 1080, 1, 1),
        Output {
            path: temp.path().join("unused.ivf"),
            overwrite: false,
            tags: HashMap::new(),
            video_tags: HashMap::new(),
        },
        encoder.clone(),
        vec![Scene {
            start_frame: 0,
            end_frame: 1,
            sub_scenes: None,
            encoder,
            sequence_data: DefaultSequenceData::default(),
        }],
        Some(config),
        Box::new(|_| Ok(())),
    );
    let mut sequence = ParallelEncoder { input: None };
    let (tx, _rx) = mpsc::channel();
    let mut paths = Vec::new();
    for (width, height, transfer, range) in
        [(1920, 1080, 1, 1), (1280, 720, 1, 1), (1920, 1080, 16, 1), (1920, 1080, 1, 0)]
    {
        condor.input = input(width, height, transfer, range);
        let info = condor.input.clip_info().expect("source metadata");
        assert_eq!(info.resolution, (width, height));
        let expected_transfer = if transfer == 16 {
            TransferFunction::SMPTE2084
        } else {
            TransferFunction::BT1886
        };
        assert_eq!(info.photon_noise_transfer_function(), expected_transfer);
        assert_eq!(
            info.color_range,
            Some(if range == 0 {
                ColorRange::Full
            } else {
                ColorRange::Limited
            })
        );
        let (_, warnings) = sequence.initialize(&mut condor, tx.clone()).expect("real initialize");
        assert!(warnings.is_empty());
        let CLIParameter::String { value, .. } =
            &condor.scenes[0].encoder.parameters()["film-grain-table"]
        else {
            panic!("table path option")
        };
        let path = PathBuf::from(value);
        assert_ne!(
            path, legacy,
            "legacy settings-only file must not be selected"
        );
        assert_eq!(path.as_path().parent(), Some(directory.as_path()));
        assert!(
            !paths.contains(&path),
            "different source characteristics need distinct eligible files"
        );
        let expected_path = temp.path().join("expected.tbl");
        av1_grain::write_grain_table(
            &expected_path,
            &[expected_params(&noise, width, height, expected_transfer, range == 0)],
        )
        .expect("independent expected table");
        assert_eq!(
            fs::read(&path).expect("new table"),
            fs::read(expected_path).expect("expected table")
        );
        assert_eq!(fs::read(&legacy).expect("preserved legacy"), legacy_bytes);
        paths.push(path);
    }
    if !check_reuse {
        return;
    }
    let baseline = &paths[0];
    let file = fs::OpenOptions::new().write(true).open(baseline).expect("reuse marker");
    file.set_times(
        fs::FileTimes::new()
            .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_234_567_890)),
    )
    .expect("fixed modification time");
    drop(file);
    let modified = fs::metadata(baseline).expect("marker metadata").modified().expect("mtime");
    let bytes = fs::read(baseline).expect("baseline bytes");
    let mut equivalent = noise;
    equivalent.width = Some(1920);
    equivalent.height = Some(1080);
    condor.scenes[0].encoder.set_photon_noise(Some(equivalent));
    condor.input = input(640, 360, 1, 1);
    sequence.initialize(&mut condor, tx).expect("equivalent resolved request");
    let CLIParameter::String { value, .. } =
        &condor.scenes[0].encoder.parameters()["film-grain-table"]
    else {
        panic!("reused table path")
    };
    assert_eq!(Path::new(value), baseline);
    assert_eq!(fs::read(baseline).expect("reused bytes"), bytes);
    assert_eq!(
        fs::metadata(baseline).expect("reused metadata").modified().expect("mtime"),
        modified,
        "existing table must not be rewritten"
    );
    assert_eq!(
        fs::read(&legacy).expect("legacy still present"),
        legacy_bytes
    );
    assert_eq!(
        fs::read_dir(&directory).expect("only cache entries").count(),
        5
    );
}

fn avm_noise() -> PhotonNoise {
    PhotonNoise {
        iso: 800,
        chroma_iso: Some(800),
        width: None,
        height: None,
        c_y: None,
        ccb: None,
        ccr: None,
    }
}

#[test]
fn avm_application_noise_fails_before_executable_lookup_or_output_changes() {
    use andean_condor::models::encoder::EncoderSettingsError;
    let temp = tempfile::tempdir().expect("preflight fixture");
    let output = temp.path().join("out.ivf");
    fs::write(&output, b"existing output").expect("sentinel");
    let mut encoder = Encoder::default_from_base(&EncoderBase::AVM, false);
    encoder.set_executable(Some(temp.path().join("missing-avmenc")));
    encoder.set_photon_noise(Some(avm_noise()));
    for direct_pass in [false, true] {
        let (frames_tx, frames_rx) = crossbeam_channel::unbounded();
        let queued = frames_rx.clone();
        frames_tx.send(Cursor::new(b"queued frame".to_vec())).expect("queue");
        let (progress_tx, progress_rx) = mpsc::channel();
        let result = if direct_pass {
            encoder.encode_pass_with_stream(
                (1, 1),
                frames_rx,
                &output,
                progress_tx,
                &super::clip_info(),
                None,
                &andean_condor::utils::cancellation::Cancellation::default(),
            )
        } else {
            encoder.encode_with_stream(
                frames_rx,
                &output,
                progress_tx,
                &super::clip_info(),
                &andean_condor::utils::cancellation::Cancellation::default(),
            )
        };
        let error = result.expect_err("unsupported settings must win over missing executable");
        assert_eq!(
            error.downcast_ref::<EncoderSettingsError>(),
            Some(&EncoderSettingsError::PhotonNoiseNotSupported(
                EncoderBase::AVM
            )),
            "{error:#}"
        );
        assert_eq!(
            queued.try_recv().expect("unconsumed input").into_inner(),
            b"queued frame"
        );
        assert!(progress_rx.try_iter().next().is_none());
        assert_eq!(
            fs::read(&output).expect("untouched output"),
            b"existing output"
        );
        assert_eq!(
            fs::read_dir(temp.path()).expect("no sidecars/statistics").count(),
            1
        );
    }
}

#[test]
fn supported_backends_keep_generated_filmgrn1_tables_and_native_flags() {
    let temp = tempfile::tempdir().expect("table fixture");
    for (base, flag, delimiter) in [
        (EncoderBase::AOM, "film-grain-table", "="),
        (EncoderBase::RAV1E, "photon-noise-table", " "),
        (EncoderBase::SVTAV1, "fgs-table", " "),
    ] {
        assert!(base.supports_photon_noise());
        let mut encoder = Encoder::default_from_base(&base, false);
        encoder.set_photon_noise(Some(avm_noise()));
        assert_eq!(encoder.photon_noise(), Some(&avm_noise()));
        assert_eq!(encoder.validate_settings(), Ok(()));
        let (_, segment) = encoder
            .generate_photon_noise_table(64, 64, TransferFunction::BT1886, None)
            .expect("supported settings")
            .expect("generated table");
        let path = temp.path().join(format!("{base}.tbl"));
        av1_grain::write_grain_table(&path, &[segment]).expect("real table writer");
        let text = fs::read_to_string(&path).expect("generated table");
        assert_eq!(text.lines().next(), Some("filmgrn1"));
        let params = text.lines().find(|line| line.trim_start().starts_with("p ")).expect("p line");
        assert_eq!(
            params.split_whitespace().count(),
            13,
            "p plus 12 AV1 values"
        );
        encoder.apply_photon_noise_parameters(&path).expect("supported application");
        assert_eq!(
            encoder.parameters().get(flag),
            Some(&CLIParameter::new_string(
                "--",
                delimiter,
                path.to_str().expect("UTF-8")
            ))
        );
        let argv = encoder
            .parameters_with_output(
                (1, 1),
                &temp.path().join("out.ivf"),
                &super::clip_info(),
                None,
            )
            .expect("supported argv");
        if delimiter == "=" {
            assert!(argv.contains(&format!("--{flag}={}", path.display())));
        } else {
            assert!(
                argv.windows(2)
                    .any(|pair| pair == [format!("--{flag}"), path.display().to_string()])
            );
        }
    }
    assert!(
        !EncoderBase::AVM.supports_photon_noise(),
        "not a filmgrn1 backend"
    );
}
