use super::*;
use crate::core::encoder::tests::support::run_bounded;
use std::{collections::HashSet, fs};

fn option_names(argv: &[String]) -> Vec<&str> {
    // Paths and negative numbers are values, not CLI options. FFmpeg documents
    // stream specifiers separately from the option name (e.g. -c:v -> -c).
    argv.iter()
        .filter(|arg| arg.starts_with('-') && arg != &"-" && arg.parse::<f64>().is_err())
        .map(|arg| arg.split(['=', ':']).next().expect("option name"))
        .collect()
}

fn unsupported<'a>(
    argv: &'a [String],
    supported: &HashSet<std::borrow::Cow<'_, str>>,
) -> Vec<&'a str> {
    option_names(argv)
        .into_iter()
        .filter(|name| !supported.contains(*name))
        .collect()
}

fn value<'a>(argv: &'a [String], flag: &str) -> Option<&'a str> {
    argv.iter().enumerate().find_map(|(index, arg)| {
        if arg == flag {
            argv.get(index + 1).map(String::as_str)
        } else {
            arg.strip_prefix(flag).and_then(|suffix| suffix.strip_prefix('='))
        }
    })
}

fn help_output(base: EncoderBase, executable: &Path, args: &[&str], log: &Path) -> String {
    if base != EncoderBase::X265 || args != ["--fullhelp"] {
        return run_bounded(
            Command::new(executable).args(args).stdin(Stdio::null()),
            log,
        );
    }

    // x265 exits 1 for valid full help. This exception is confined to this exact
    // help invocation; every encode still uses the original strict watchdog.
    let file = fs::File::create(log).expect("help log");
    let mut child = Command::new(executable)
        .args(args)
        .stdin(Stdio::null())
        .stdout(file.try_clone().expect("help log clone"))
        .stderr(file)
        .spawn()
        .expect("x265 help");
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("wait for help") {
            break status;
        }
        if start.elapsed() > std::time::Duration::from_secs(60) {
            child.kill().expect("kill timed-out help");
            child.wait().expect("reap timed-out help");
            panic!(
                "x265 help timed out: {}",
                fs::read_to_string(log).expect("help log")
            );
        }
        thread::sleep(std::time::Duration::from_millis(10));
    };
    let text = fs::read_to_string(log).expect("help log");
    assert!(
        status.success() || status.code() == Some(1),
        "x265 help: {status}\n{text}"
    );
    assert!(
        text.contains("Syntax: x265 [options]"),
        "not x265 full help: {text}"
    );
    let parsed = valid_params(&text, base);
    for required in ["--crf", "--y4m", "--colormatrix"] {
        assert!(
            parsed.contains(required),
            "incomplete x265 help: missing {required}"
        );
    }
    eprintln!("CLI parity x265 full help: {status}; required parsed options present");
    text
}

fn native_help_gaps(base: EncoderBase, root: &Path) -> Vec<String> {
    let cwd = root.join(format!("{base}-native-help-proof"));
    fs::create_dir_all(&cwd).expect("private native proof");
    let (binary, codec, container) = match base {
        EncoderBase::RAV1E => ("rav1e", "av1", "ivf"),
        EncoderBase::FFmpeg => ("ffmpeg", "ffv1", "matroska,webm"),
        _ => unreachable!(),
    };
    let executable = which::which(binary).expect("required encoder");
    let ffprobe = which::which("ffprobe").expect("required native verifier");
    let input = cwd.join("source.y4m");
    let source = tiny_y4m();
    fs::write(&input, &source).expect("finite source");
    let sentinel = cwd.join("scene0.stat");
    fs::write(&sentinel, b"unrelated statistics").expect("sentinel");
    let mut clip = crate::core::encoder::tests::clip_info();
    clip.num_frames = 3;
    clip.resolution = (64, 64);
    clip.format_info = crate::core::input::pixel_format::PixelFormat::VapourSynth { bit_depth: 8 };
    let output = cwd.join(format!("scene0.{}", base.output_extension()));
    let mut encoder = Encoder::default_from_base(&base, false);
    let table = cwd.join("valid grain table.tbl");
    let table_before = if base == EncoderBase::RAV1E {
        encoder.set_photon_noise(Some(PhotonNoise {
            iso: 800,
            chroma_iso: Some(800),
            width: None,
            height: None,
            c_y: None,
            ccb: None,
            ccr: None,
        }));
        let (_, segment) = encoder
            .generate_photon_noise_table(64, 64, TransferFunction::BT1886, None)
            .expect("production grain generation")
            .expect("grain segment");
        av1_grain::write_grain_table(&table, &[segment]).expect("write valid native table");
        encoder.apply_photon_noise_parameters(&table).expect("production grain argv");
        encoder.parameters_mut().extend(CLIParameter::new_numbers(
            "--",
            " ",
            &[("speed", 10.0), ("threads", 1.0)],
        ));
        let bytes = fs::read(&table).expect("grain table");
        assert!(bytes.starts_with(b"filmgrn1\n"));
        Some(bytes)
    } else {
        encoder
            .parameters_mut()
            .insert("c:v".into(), CLIParameter::new_string("-", " ", "ffv1"));
        encoder
            .parameters_mut()
            .insert("coder".into(), CLIParameter::new_number("-", " ", 1.0));
        encoder
            .parameters_mut()
            .insert("threads".into(), CLIParameter::new_number("-", " ", 1.0));
        encoder
            .parameters_mut()
            .insert("v".into(), CLIParameter::new_string("-", " ", "error"));
        None
    };
    let argv = encoder
        .parameters_with_output((1, 1), &output, &clip)
        .expect("native proof argv");
    if base == EncoderBase::RAV1E {
        assert_eq!(value(&argv, "--photon-noise-table"), table.to_str());
    }
    eprintln!("CLI parity {base} native help-gap proof argv: {argv:?}");
    run_bounded(
        Command::new(executable)
            .args(&argv)
            .stdin(fs::File::open(&input).expect("finite input"))
            .current_dir(&cwd),
        &cwd.join("encode.log"),
    );
    probe(&ffprobe, &output, codec, container, &cwd.join("probe.log"));
    let colors = run_bounded(
        Command::new(&ffprobe)
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=color_space,color_transfer,color_primaries",
                "-of",
                "default=noprint_wrappers=1",
            ])
            .arg(&output)
            .stdin(Stdio::null()),
        &cwd.join("colors.log"),
    );
    assert_eq!(
        colors.lines().collect::<HashSet<_>>(),
        HashSet::from([
            "color_space=bt2020nc",
            "color_transfer=smpte2084",
            "color_primaries=bt2020",
        ]),
        "{base}: {colors}"
    );
    eprintln!("CLI parity {base} native help-gap proof colors:\n{colors}");
    if let Some(bytes) = table_before {
        assert_eq!(fs::read(&table).expect("table unchanged"), bytes);
    }
    assert_eq!(fs::read(input).expect("source unchanged"), source);
    assert_eq!(
        fs::read(sentinel).expect("sentinel unchanged"),
        b"unrelated statistics"
    );
    // Names are credited only after their real argv has encoded and decoded
    // successfully, never by an unconditional unsupported-option allowlist.
    option_names(&argv).into_iter().map(str::to_owned).collect()
}

#[test]
fn rav1e_hidden_grain_alias_accepts_generated_valid_table() {
    let temp = tempfile::tempdir().expect("alias fixture");
    let proven = native_help_gaps(EncoderBase::RAV1E, temp.path());
    assert!(proven.iter().any(|name| name == "--photon-noise-table"));
}

#[test]
fn production_argv_options_match_installed_help() {
    let temp = tempfile::tempdir().expect("help fixture");
    let mut failures = Vec::new();
    for (base, binary, help_args, critical) in [
        (
            EncoderBase::AOM,
            "aomenc",
            vec!["--help"],
            vec![("--cq-level", "30"), ("--matrix-coefficients", "bt2020ncl")],
        ),
        (
            EncoderBase::RAV1E,
            "rav1e",
            vec!["--help"],
            vec![("--quantizer", "100"), ("--matrix", "BT2020NCL")],
        ),
        (
            EncoderBase::VPX,
            "vpxenc",
            vec!["--help"],
            vec![("--codec", "vp9"), ("--color-space", "bt2020")],
        ),
        (
            EncoderBase::SVTAV1,
            "SvtAv1EncApp",
            vec!["--help"],
            vec![("--crf", "25"), ("--matrix-coefficients", "bt2020-ncl")],
        ),
        (
            EncoderBase::X264,
            "x264",
            vec!["--fullhelp"],
            vec![("--demuxer", "y4m"), ("--colormatrix", "bt2020nc")],
        ),
        (
            EncoderBase::X265,
            "x265",
            vec!["--fullhelp"],
            vec![("--keyint", "-1"), ("--colormatrix", "bt2020nc")],
        ),
        (
            EncoderBase::FFmpeg,
            "ffmpeg",
            vec!["-hide_banner", "-h", "long"],
            vec![("-c:v", "ffv1"), ("-colorspace", "bt2020nc")],
        ),
        (
            EncoderBase::AVM,
            "avmenc",
            vec!["--help"],
            vec![("--qp", "30"), ("--matrix-coefficients", "bt2020ncl")],
        ),
        (
            EncoderBase::VVenC,
            "vvencapp",
            vec!["--fullhelp"],
            vec![(
                "--additional",
                "ColourDescriptionPresent=1:ColourPrimaries=bt2020:TransferCharacteristics=smpte2084:MatrixCoefficients=bt2020nc",
            )],
        ),
    ] {
        let executable = match which::which(binary) {
            Ok(path) => path,
            Err(error) if matches!(base, EncoderBase::AVM | EncoderBase::VVenC) => {
                eprintln!(
                    "CLI parity CAPABILITY SKIP {base}: optional {binary} unavailable ({error})"
                );
                continue;
            },
            Err(error) => panic!("CLI parity required {binary} unavailable: {error}"),
        };
        let result = std::panic::catch_unwind(|| {
            let root = temp.path();
            let mut help = help_output(
                base,
                &executable,
                &help_args,
                &root.join(format!("{base}-help.log")),
            );
            if base == EncoderBase::FFmpeg {
                help.push_str(&run_bounded(
                    Command::new(&executable).args(["-hide_banner", "-h", "encoder=ffv1"]),
                    &root.join("ffv1-help.log"),
                ));
            }
            eprintln!("CLI parity installed {base}: {}", executable.display());
            let mut supported = valid_params(&help, base);
            if base == EncoderBase::RAV1E && !supported.contains("--photon-noise-table") {
                assert!(
                    supported.contains("--film-grain-table"),
                    "documented grain option missing"
                );
                supported
                    .extend(native_help_gaps(base, root).into_iter().map(std::borrow::Cow::Owned));
            } else if base == EncoderBase::FFmpeg {
                // Generic colour AVOptions have no narrow CLI help topic. Check
                // their real metadata effects rather than dump every codec/filter.
                supported
                    .extend(native_help_gaps(base, root).into_iter().map(std::borrow::Cow::Owned));
            }
            let mut encoder = Encoder::default_from_base(&base, false);
            if base == EncoderBase::FFmpeg {
                encoder
                    .parameters_mut()
                    .insert("c:v".into(), CLIParameter::new_string("-", " ", "ffv1"));
                encoder
                    .parameters_mut()
                    .insert("coder".into(), CLIParameter::new_number("-", " ", 1.0));
            }
            if base.supports_photon_noise() {
                encoder
                    .apply_photon_noise_parameters(&root.join("grain table.tbl"))
                    .expect("native grain flag");
            }
            for pass in [(1, 1), (1, 2), (2, 2)] {
                if base == EncoderBase::FFmpeg && pass.1 == 2 {
                    continue; // The FFmpeg backend intentionally has no automatic multipass contract.
                }
                if base == EncoderBase::SVTAV1 && pass.1 == 2 {
                    // Match SVT-AV1's genuine VBR contract; the default CRF mode is
                    // deliberately single-pass, not a multipass parser failure.
                    encoder.parameters_mut().remove("crf");
                    encoder.parameters_mut().extend(CLIParameter::new_numbers(
                        "--",
                        " ",
                        &[("rc", 1.0), ("keyint", 48.0), ("preset", 9.0), ("tbr", 100.0)],
                    ));
                }
                if let Some(passes) = encoder.passes_mut() {
                    *passes = EncoderPasses::All(pass.1);
                }
                let output = root.join(format!("scene.{}", base.output_extension()));
                for argv in [
                    encoder.parameters_with_output(
                        pass,
                        &output,
                        &crate::core::encoder::tests::clip_info(),
                    ),
                    encoder.parameters_with_stats(
                        pass,
                        &output,
                        &crate::core::encoder::tests::clip_info(),
                        &root.join("owned stats"),
                    ),
                ] {
                    let argv = argv.expect("production argv");
                    assert_eq!(
                        unsupported(&argv, &supported),
                        Vec::<&str>::new(),
                        "{base} {pass:?}: {argv:?}"
                    );
                    for (flag, expected) in &critical {
                        if base == EncoderBase::SVTAV1 && pass.1 == 2 && *flag == "--crf" {
                            assert_eq!(value(&argv, flag), None, "VBR cannot retain default CRF");
                            continue;
                        }
                        assert_eq!(
                            value(&argv, flag),
                            Some(*expected),
                            "{base} {pass:?}: {argv:?}"
                        );
                    }
                    if base == EncoderBase::SVTAV1 && pass.1 == 2 {
                        for (flag, expected) in
                            [("--rc", "1"), ("--keyint", "48"), ("--preset", "9"), ("--tbr", "100")]
                        {
                            assert_eq!(
                                value(&argv, flag),
                                Some(expected),
                                "SVT VBR {pass:?}: {argv:?}"
                            );
                        }
                    }
                    let mut invalid = argv.clone();
                    invalid.push("--cli-parity-deliberately-nonexistent".into());
                    assert_eq!(
                        unsupported(&invalid, &supported),
                        ["--cli-parity-deliberately-nonexistent"]
                    );
                    if base.supports_photon_noise() {
                        let flag = match base {
                            EncoderBase::AOM => "--film-grain-table",
                            EncoderBase::RAV1E => "--photon-noise-table",
                            EncoderBase::SVTAV1 => "--fgs-table",
                            _ => unreachable!(),
                        };
                        assert_eq!(value(&argv, flag), root.join("grain table.tbl").to_str());
                    }
                }
            }
            // A caller's native override must survive the same help validation.
            let (key, prefix, delimiter, expected) = match base {
                EncoderBase::AOM | EncoderBase::AVM => ("cpu-used", "--", "=", "5"),
                EncoderBase::VPX => ("color-space", "--", "=", "smpte240"),
                EncoderBase::RAV1E => ("speed", "--", " ", "9"),
                EncoderBase::SVTAV1 => ("preset", "--", " ", "9"),
                EncoderBase::X264 | EncoderBase::X265 => ("preset", "--", " ", "ultrafast"),
                EncoderBase::FFmpeg => ("coder", "-", " ", "2"),
                EncoderBase::VVenC => ("preset", "--", " ", "fast"),
            };
            encoder.parameters_mut().insert(
                key.into(),
                CLIParameter::new_string(prefix, delimiter, expected),
            );
            if let Some(passes) = encoder.passes_mut() {
                *passes = EncoderPasses::All(1);
            }
            let argv = encoder
                .parameters_with_output(
                    (1, 1),
                    &root.join("override.ivf"),
                    &crate::core::encoder::tests::clip_info(),
                )
                .expect("override argv");
            assert_eq!(value(&argv, &format!("{prefix}{key}")), Some(expected));
            assert!(
                unsupported(&argv, &supported).is_empty(),
                "{base}: {argv:?}"
            );
        });
        match result {
            Ok(()) => eprintln!("CLI parity {base} help row PASS"),
            Err(error) => {
                let message = error
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| error.downcast_ref::<&str>().copied())
                    .unwrap_or("non-string panic");
                failures.push(format!("{base}: {message}"));
                eprintln!("CLI parity {base} help row FAIL: {message}");
            },
        }
    }
    assert!(
        failures.is_empty(),
        "CLI parity help contract failures:\n{}",
        failures.join("\n")
    );
}

#[test]
fn svt_one_pass_is_ivf_and_decodes_exact_frames() {
    let temp = tempfile::tempdir().expect("fixture");
    let root = temp.path();
    let svt = which::which("SvtAv1EncApp").expect("required SVT-AV1 executable");
    let ffprobe = which::which("ffprobe").expect("required frame-count verifier");
    let help = run_bounded(Command::new(&svt).arg("--help"), &root.join("help.log"));
    eprintln!(
        "CLI parity SVT advertises WebM: {}",
        help.contains("--webm")
    );
    eprintln!(
        "CLI parity SVT version: {}",
        run_bounded(
            Command::new(&svt).arg("--version"),
            &root.join("version.log")
        )
    );
    let mut clip = crate::core::encoder::tests::clip_info();
    clip.num_frames = 3;
    clip.resolution = (64, 64);
    clip.format_info = crate::core::input::pixel_format::PixelFormat::VapourSynth { bit_depth: 8 };
    let input = root.join("source.y4m");
    let source = tiny_y4m();
    fs::write(&input, &source).expect("source");
    let sentinel = root.join("scene0.stat");
    fs::write(&sentinel, b"unrelated statistics").expect("sentinel");
    let output = root.join("scene0.ivf");
    let mut encoder = Encoder::default_from_base(&EncoderBase::SVTAV1, false);
    encoder.parameters_mut().extend(CLIParameter::new_numbers(
        "--",
        " ",
        &[("preset", 12.0), ("lp", 1.0)],
    ));
    let argv = encoder.parameters_with_output((1, 1), &output, &clip).expect("production argv");
    eprintln!("CLI parity SVT one-pass production argv: {argv:?}");
    let log = run_bounded(
        Command::new(svt)
            .args(&argv)
            .stdin(fs::File::open(&input).expect("finite input")),
        &root.join("encode.log"),
    );
    assert!(
        !log.to_ascii_lowercase().contains("error") && !log.contains("Unprocessed tokens"),
        "SVT may exit zero on invalid argv: {log}"
    );
    let bytes = fs::read(&output).expect("encoded output");
    eprintln!(
        "CLI parity SVT header: {:?}; {} bytes",
        &bytes[..bytes.len().min(12)],
        bytes.len()
    );
    assert!(
        bytes.starts_with(b"DKIF"),
        "SVT scene chunks must be IVF, not WebM/OBU"
    );
    probe(&ffprobe, &output, "av1", "ivf", &root.join("probe.log"));
    assert_eq!(fs::read(&input).expect("source unchanged"), source);
    assert_eq!(
        fs::read(&sentinel).expect("sentinel unchanged"),
        b"unrelated statistics"
    );
}

fn tiny_y4m() -> Vec<u8> {
    let mut bytes = b"YUV4MPEG2 W64 H64 F24:1 Ip A1:1 C420jpeg\n".to_vec();
    for frame in 0..3 {
        bytes.extend_from_slice(b"FRAME\n");
        for y in 0..64 {
            for x in 0..64 {
                bytes.push(32 + ((x / 8 + y / 8 + frame) % 3) * 64);
            }
        }
        bytes.extend(vec![128; 64 * 64 / 2]);
    }
    bytes
}

#[test]
fn remaining_cpu_one_and_two_pass_outputs_decode_exact_frames() {
    let temp = tempfile::tempdir().expect("native fixture");
    let root = temp.path();
    let ffprobe = which::which("ffprobe").expect("required frame-count verifier");
    let input = root.join("source.y4m");
    let source = tiny_y4m();
    fs::write(&input, &source).expect("source");
    let mut clip = crate::core::encoder::tests::clip_info();
    clip.num_frames = 3;
    clip.resolution = (64, 64);
    clip.format_info = crate::core::input::pixel_format::PixelFormat::VapourSynth { bit_depth: 8 };
    // rav1e/SVT-AV1/AOM/VPX already cover rav1e/SVT two-pass and AOM/VP8/VP9
    // one-pass. These are the remaining genuine CPU pass contracts.
    for (base, binary, counts, codec, container) in [
        (EncoderBase::AOM, "aomenc", vec![2], "av1", "ivf"),
        (EncoderBase::RAV1E, "rav1e", vec![1], "av1", "ivf"),
        (EncoderBase::VPX, "vpxenc", vec![2], "vp9", "ivf"),
        (EncoderBase::X264, "x264", vec![1, 2], "h264", "h264"),
        (EncoderBase::X265, "x265", vec![1, 2], "hevc", "hevc"),
        (
            EncoderBase::FFmpeg,
            "ffmpeg",
            vec![1],
            "ffv1",
            "matroska,webm",
        ),
    ] {
        let executable =
            which::which(binary).unwrap_or_else(|error| panic!("required {binary}: {error}"));
        for count in counts {
            let cwd = root.join(format!("{base}-{count}"));
            fs::create_dir_all(&cwd).expect("private cwd");
            fs::write(cwd.join("scene0.log"), b"unrelated statistics").expect("sentinel");
            let output = cwd.join(format!("scene0.{}", base.output_extension()));
            let stats = PassStatistics::new(&output).expect("owned statistics");
            let mut encoder = Encoder::default_from_base(&base, false);
            if let Some(passes) = encoder.passes_mut() {
                *passes = EncoderPasses::All(count);
            }
            match base {
                EncoderBase::AOM | EncoderBase::VPX => {
                    encoder.parameters_mut().extend(CLIParameter::new_numbers(
                        "--",
                        "=",
                        &[("threads", 1.0), ("cpu-used", 6.0)],
                    ));
                    if base == EncoderBase::VPX {
                        encoder.parameters_mut().extend(CLIParameter::new_numbers(
                            "--",
                            "=",
                            &[("profile", 0.0), ("lag-in-frames", 0.0), ("auto-alt-ref", 0.0)],
                        ));
                        encoder
                            .parameters_mut()
                            .insert("b".into(), CLIParameter::new_number("-", " ", 8.0));
                    }
                },
                EncoderBase::RAV1E => {
                    encoder.parameters_mut().extend(CLIParameter::new_numbers(
                        "--",
                        " ",
                        &[("threads", 1.0), ("speed", 10.0)],
                    ));
                },
                EncoderBase::X264 | EncoderBase::X265 => {
                    encoder.parameters_mut().insert(
                        "preset".into(),
                        CLIParameter::new_string("--", " ", "ultrafast"),
                    );
                    if count == 2 {
                        encoder.parameters_mut().remove("crf");
                        encoder
                            .parameters_mut()
                            .insert("bitrate".into(), CLIParameter::new_number("--", " ", 100.0));
                    }
                    if base == EncoderBase::X264 {
                        encoder
                            .parameters_mut()
                            .insert("threads".into(), CLIParameter::new_number("--", " ", 1.0));
                    } else {
                        encoder
                            .parameters_mut()
                            .insert("D".into(), CLIParameter::new_number("-", " ", 8.0));
                        encoder
                            .parameters_mut()
                            .insert("pools".into(), CLIParameter::new_string("--", " ", "none"));
                        encoder.parameters_mut().insert(
                            "frame-threads".into(),
                            CLIParameter::new_number("--", " ", 1.0),
                        );
                    }
                },
                EncoderBase::FFmpeg => {
                    encoder
                        .parameters_mut()
                        .insert("c:v".into(), CLIParameter::new_string("-", " ", "ffv1"));
                    encoder
                        .parameters_mut()
                        .insert("threads".into(), CLIParameter::new_number("-", " ", 1.0));
                    encoder
                        .parameters_mut()
                        .insert("v".into(), CLIParameter::new_string("-", " ", "error"));
                },
                _ => unreachable!(),
            }
            for pass in 1..=count {
                let argv = encoder
                    .parameters_with_stats((pass, count), &output, &clip, stats.base())
                    .expect("production argv");
                eprintln!("CLI parity {base} {pass}/{count} production argv: {argv:?}");
                run_bounded(
                    Command::new(&executable)
                        .args(&argv)
                        .stdin(fs::File::open(&input).expect("reopened finite input"))
                        .current_dir(&cwd),
                    &root.join(format!("{base}-{count}-{pass}.log")),
                );
            }
            let bytes = fs::read(&output).expect("encoded output");
            assert!(!bytes.is_empty());
            if container == "ivf" {
                assert!(bytes.starts_with(b"DKIF"), "{base}: not IVF");
            }
            eprintln!(
                "CLI parity {base} {count}-pass header: {:?}; {} bytes",
                &bytes[..bytes.len().min(12)],
                bytes.len()
            );
            probe(
                &ffprobe,
                &output,
                codec,
                container,
                &root.join(format!("{base}-{count}-probe.log")),
            );
            let owned_dir = stats.base().parent().expect("statistics dir").to_path_buf();
            drop(stats);
            assert!(!owned_dir.exists(), "owned statistics cleaned");
            assert_eq!(fs::read(&input).expect("source unchanged"), source);
            assert_eq!(
                fs::read(cwd.join("scene0.log")).expect("sentinel"),
                b"unrelated statistics"
            );
            assert_eq!(fs::read_dir(cwd).expect("no statistics spill").count(), 2);
        }
    }
}

#[test]
fn vpx_default_and_conflicting_webm_outputs_are_ivf() {
    let temp = tempfile::tempdir().expect("native fixture");
    let root = temp.path();
    let vpx = which::which("vpxenc").expect("required VPX executable");
    let ffprobe = which::which("ffprobe").expect("required frame-count verifier");
    let input = root.join("source.y4m");
    let source = tiny_y4m();
    fs::write(&input, &source).expect("finite source");
    let mut clip = crate::core::encoder::tests::clip_info();
    clip.num_frames = 3;
    clip.resolution = (64, 64);
    clip.format_info = crate::core::input::pixel_format::PixelFormat::VapourSynth { bit_depth: 8 };
    let mut failures = Vec::new();
    for (codec, fourcc) in [("vp8", b"VP80"), ("vp9", b"VP90")] {
        for webm in [false, true] {
            let row = format!("{codec}-{}", if webm { "user-webm" } else { "default" });
            let result = std::panic::catch_unwind(|| {
                let cwd = root.join(&row);
                fs::create_dir_all(&cwd).expect("private cwd");
                let sentinel = cwd.join("scene0.log");
                fs::write(&sentinel, b"unrelated statistics").expect("sentinel");
                let output = cwd.join("scene0.ivf");
                let stats = PassStatistics::new(&output).expect("owned statistics");
                let mut encoder = Encoder::default_from_base(&EncoderBase::VPX, false);
                *encoder.passes_mut().expect("passes") = EncoderPasses::All(1);
                // Tune only CPU use and the eight-bit fixture, never select IVF in the test.
                encoder.parameters_mut().extend(CLIParameter::new_numbers(
                    "--",
                    "=",
                    &[
                        ("threads", 1.0),
                        ("cpu-used", 6.0),
                        ("profile", 0.0),
                        ("lag-in-frames", 0.0),
                        ("auto-alt-ref", 0.0),
                    ],
                ));
                encoder
                    .parameters_mut()
                    .insert("codec".into(), CLIParameter::new_string("--", "=", codec));
                encoder
                    .parameters_mut()
                    .insert("b".into(), CLIParameter::new_number("-", " ", 8.0));
                if codec == "vp8" {
                    encoder.parameters_mut().remove("row-mt");
                }
                if webm {
                    encoder
                        .parameters_mut()
                        .insert("webm".into(), CLIParameter::new_bool("--", true));
                    encoder
                        .parameters_mut()
                        .insert("ivf".into(), CLIParameter::new_bool("--", false));
                }
                let argv = encoder
                    .parameters_with_stats((1, 1), &output, &clip, stats.base())
                    .expect("production argv");
                eprintln!("V01b {row} production argv: {argv:?}");
                run_bounded(
                    Command::new(&vpx)
                        .args(&argv)
                        .stdin(fs::File::open(&input).expect("reopened finite input"))
                        .current_dir(&cwd),
                    &root.join(format!("{row}-encode.log")),
                );
                let bytes = fs::read(&output).expect("encoded output");
                eprintln!(
                    "V01b {row} header: {:?}; {} bytes; expected {codec}/ivf/3 frames",
                    &bytes[..bytes.len().min(12)],
                    bytes.len()
                );
                let owned_dir = stats.base().parent().expect("statistics dir").to_path_buf();
                drop(stats);
                assert!(!owned_dir.exists(), "owned statistics cleaned");
                assert_eq!(fs::read(&input).expect("source unchanged"), source);
                assert_eq!(
                    fs::read(&sentinel).expect("sentinel"),
                    b"unrelated statistics"
                );
                assert_eq!(fs::read_dir(&cwd).expect("no statistics spill").count(), 2);
                probe(
                    &ffprobe,
                    &output,
                    codec,
                    "ivf",
                    &root.join(format!("{row}-probe.log")),
                );
                assert!(bytes.starts_with(b"DKIF"), "{row}: not IVF");
                assert_eq!(
                    bytes.get(8..12),
                    Some(fourcc.as_slice()),
                    "{row}: IVF codec"
                );
            });
            if result.is_err() {
                failures.push(row);
            }
        }
    }
    assert!(failures.is_empty(), "V01b failed rows: {failures:?}");
}

fn probe(ffprobe: &Path, output: &Path, codec: &str, container: &str, log: &Path) {
    let text = run_bounded(
        Command::new(ffprobe)
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-count_frames",
                "-show_entries",
                "stream=codec_name,nb_read_frames:format=format_name",
                "-of",
                "default=noprint_wrappers=1",
            ])
            .arg(output)
            .stdin(Stdio::null()),
        log,
    );
    assert_eq!(
        text.trim(),
        format!("codec_name={codec}\nnb_read_frames=3\nformat_name={container}")
    );
    eprintln!(
        "CLI parity {} independently decoded:\n{text}",
        output.file_name().expect("filename").display()
    );
}
