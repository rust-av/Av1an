use super::*;
use crate::{
    core::{
        Condor,
        encoder::{EncoderResult, StringOrBytes},
        output::Output,
        sequence::{Sequence, parallel_encoder::ParallelEncoder, serial_encoder::SerialEncoder},
    },
    models::{
        input::{ImportMethod, Input as InputModel, VapourSynthScriptSource},
        scene::Scene,
        sequence::{DefaultSequenceConfig, DefaultSequenceData},
    },
};
use std::{collections::HashMap, os::unix::fs::PermissionsExt, process::Command};

// native probe catches a probe route that certifies a quantizer using cross-scene state
// which the final per-scene encoder cannot reproduce. No production test switch.
#[test]
fn x264_and_svt_native_probe_parity() {
    native_probe_parity(&["x264", "svt"]);
}

#[test]
fn x265_native_probe_parity() {
    native_probe_parity(&["x265"]);
}

fn native_probe_parity(bases: &[&str]) {
    let temp = tempfile::tempdir().expect("native parity supervisor");
    let root = temp.path();
    fs::write(
        root.join("watchdog.rs"),
        include_str!("../../../../../tests/core/encoder/fixtures/fake_encoder.rs"),
    )
    .expect("existing watchdog");
    let compiled = Command::new("rustc")
        .arg(root.join("watchdog.rs"))
        .arg("-o")
        .arg(root.join("watchdog"))
        .output()
        .expect("rustc required");
    assert!(compiled.status.success(), "{compiled:?}");
    let mut failures = Vec::new();
    for &base in bases {
        let job = root.join(base);
        fs::create_dir_all(&job).expect("isolated native job");
        let file = fs::File::create(job.join("child.log")).expect("log");
        let status = Command::new(root.join("watchdog"))
            .arg("--encoder-fixture-watchdog")
            .arg(std::env::current_exe().expect("test executable"))
            .arg(&job)
            .arg(format!("native-probe-{base}"))
            .arg("core::sequence::zone_encoder::tests::runtime::native_probe_parity_child")
            .stdout(file.try_clone().expect("log clone"))
            .stderr(file)
            .status()
            .expect("native parity watchdog");
        let log = fs::read_to_string(job.join("child.log")).expect("native log");
        eprint!("{base}: {status}\n{log}");
        let pending = base == "svt"
            && std::env::var_os("CONDOR_REQUIRE_SVT_ZONES").is_none()
            && fs::read_to_string(job.join("capability-pending"))
                .is_ok_and(|marker| marker == "SVT grouped parity PENDING: NoZoneSupport")
            && log.contains("SVT grouped parity PENDING: NoZoneSupport")
            && !job.join("verified").exists();
        if !status.success()
            || !(job.join("verified").exists() || pending)
            || !log.contains("encoder fixture watchdog reaped 0 descendants; timeout=false")
        {
            failures.push(format!("{base}: {status}\n{log}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn decode(path: &Path) -> Vec<u8> {
    let decoded = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-f", "rawvideo", "-pix_fmt", "yuv420p", "-"])
        .output()
        .expect("ffmpeg required, no capability skip");
    assert!(decoded.status.success(), "{decoded:?}");
    decoded.stdout
}

#[test]
fn native_probe_parity_child() {
    use crate::{
        core::sequence::{
            quality_check::QualityCheck, scene_concatenator::SceneConcatenator,
            target_quality::TargetQuality,
        },
        models::sequence::{
            quality_check::QualityCheckConfig,
            scene_concatenator::ConcatMethod,
            target_quality::{
                TargetQualityConfig,
                types::{ProbeStatistic, QualityMetric},
            },
        },
    };
    let Some(root) = std::env::var_os("ENCODER_FIXTURE_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let supervisor_case = std::env::var("ENCODER_FIXTURE_CASE").expect("encoder case");
    // Existing library-test fault hooks share ENCODER_FIXTURE_CASE's path-kind convention.
    let case = supervisor_case.strip_prefix("native-probe-").expect("native parity case");
    let base = match case {
        "x264" => EncoderBase::X264,
        "x265" => EncoderBase::X265,
        "svt" => EncoderBase::SVTAV1,
        _ => panic!("unexpected native probe encoder case"),
    };
    for tool in [
        "ffmpeg",
        "ffprobe",
        "mkvmerge",
        match case {
            "x264" => "x264",
            "x265" => "x265",
            _ => "SvtAv1EncApp",
        },
    ] {
        let executable = which::which(tool).expect("designated native capability is mandatory");
        println!("native probe capability {tool}: {}", executable.display());
    }
    let source = root.join("pattern.mkv");
    let generated = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=128x128:rate=24",
            "-frames:v",
            "36",
            "-c:v",
            "ffv1",
        ])
        // Production x265 defaults to 10-bit output; native VMAF requires
        // matching reference depth rather than silently converting either side.
        .args(["-pix_fmt", if case == "x265" { "yuv420p10le" } else { "yuv420p" }])
        .arg(&source)
        .output()
        .expect("native fixture");
    assert!(generated.status.success(), "{generated:?}");
    let source_bytes = fs::read(&source).expect("source bytes");
    let input_model = InputModel::Video {
        path: source.clone(),
        import_method: ImportMethod::FFMS2 {
            index: None,
            cache_path: None,
        },
        filters: vec![],
    };
    let metric = QualityMetric::VMAF {
        target_range: (94.0, 96.0),
        resolution: None,
        scaler: "bicubic".into(),
        threads: 0,
        model: None,
        features: vec![],
    };
    let tq = TargetQualityConfig {
        metric: metric.clone(),
        quantizer_range: (10, 40),
        maximum_probes: 4,
        ..Default::default()
    };
    let mut encoder = Encoder::default_from_base(&base, false);
    encoder.set_append_color_data(false);
    encoder.set_quantizer(25.0);
    let options: &[(&str, &str)] = match case {
        "x264" => {
            &[("preset", "medium"), ("threads", "1"), ("rc-lookahead", "12"), ("bframes", "2")]
        },
        "x265" => &[
            ("preset", "medium"),
            ("pools", "1"),
            ("frame-threads", "1"),
            ("rc-lookahead", "12"),
            ("bframes", "2"),
        ],
        _ => &[("preset", "12"), ("lp", "2"), ("lookahead", "16")],
    };
    encoder.parameters_mut().extend(CLIParameter::new_strings("--", " ", options));
    // Keep the final encoder identical to TQ's psychovisually sanitized encoder.
    let encoder = tq.probe_encoder(&encoder);
    if case == "x265" {
        // The failing zoned retry uses q32.5 on source frames 24..36.
        // Prove the same native settings/input work without zone substitution.
        let mut control = encoder.clone();
        control.set_quantizer(32.5);
        let output = root.join("independent-control.hevc");
        let (tx, _rx) = mpsc::channel();
        let result = control
            .encode_with_input(
                &mut Input::from_data(&input_model).expect("control FFMS2"),
                &output,
                24,
                36,
                tx,
            )
            .expect("same x265 settings must work independently");
        assert!(result.status.success());
        assert_eq!(decode(&output).len(), 12 * 128 * 128 * 3 / 2);
        assert_eq!(
            fs::read(&source).expect("control source bytes"),
            source_bytes
        );
        println!(
            "x265 native probe x265 independent q32.5 frames=24..36: native success, 12 decoded frames; argv={:?}",
            result.parameters
        );
    }
    let mut summaries = Vec::new();
    let mut failures = Vec::new();
    // Eligibility describes ZonePlan only: H.26x TQ must stay independent,
    // including retries whose completed peers have left the task subset.
    let routes: &[bool] = if case == "x265" {
        &[false, true]
    } else {
        &[true, false]
    };
    for &grouped in routes {
        let dir = root.join(if grouped { "grouped" } else { "individual" });
        fs::create_dir_all(&dir).expect("route output");
        let mut config = DefaultSequenceConfig::default();
        config.parallel_encoder.workers = Some(1);
        config.parallel_encoder.max_tries = 1;
        config.parallel_encoder.scenes_directory = dir.clone();
        config.scene_concatenator.scenes_directory = dir.clone();
        config.scene_concatenator.method = ConcatMethod::MKVMerge(Default::default());
        config.target_quality = Some(tq.clone());
        config.quality_check = Some(QualityCheckConfig {
            metric: metric.clone(),
            ..Default::default()
        });
        let scenes = [(0, 12), (12, 24), (24, 36)]
            .into_iter()
            .enumerate()
            .map(|(index, (start_frame, end_frame))| {
                let mut encoder = encoder.clone();
                // Metadata-only difference makes the initial plan ineligible;
                // singleton retry subsets can become structurally eligible again.
                if !grouped && index == 1 {
                    encoder.set_append_color_data(true);
                }
                Scene {
                    start_frame,
                    end_frame,
                    sub_scenes: None,
                    encoder,
                    sequence_data: DefaultSequenceData::default(),
                }
            })
            .collect::<Vec<_>>();
        let tasks = scenes
            .iter()
            .enumerate()
            .map(|(index, scene)| Task {
                index,
                original_index: index,
                frame_indices: (scene.start_frame..scene.end_frame).collect(),
                sub_scenes: None,
                encoder: scene.encoder.clone(),
                output: dir.join(format!("{index:05}.{}", encoder.output_extension())),
            })
            .collect::<Vec<_>>();
        let plan = ZonePlan::try_build(&tasks);
        if grouped {
            if case == "svt" && matches!(plan, Err(ZoneIneligible::NoZoneSupport)) {
                assert!(
                    std::env::var_os("CONDOR_REQUIRE_SVT_ZONES").is_none(),
                    "required SVT grouped capability unavailable: NoZoneSupport"
                );
                let marker = "SVT grouped parity PENDING: NoZoneSupport";
                println!("{marker}");
                fs::write(root.join("capability-pending"), marker).expect("pending capability");
                return;
            }
            assert!(plan.is_ok(), "native grouped capability: {plan:?}");
        } else {
            assert!(matches!(
                plan,
                Err(ZoneIneligible::DifferingEncoder {
                    field: DifferingField::AppendColorData,
                    ..
                })
            ));
        }
        let mut condor = Condor::new(
            Input::from_data(&input_model).expect("native FFMS2"),
            Output {
                path: dir.join("final.mkv"),
                overwrite: true,
                tags: HashMap::new(),
                video_tags: HashMap::new(),
            },
            encoder.clone(),
            scenes,
            Some(config),
            Box::new(|_| Ok(())),
        );
        let (tx, _rx) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut target = TargetQuality {
            input: None,
            metric_input: None,
        };
        target
            .execute(&mut condor, tx.clone(), Arc::clone(&cancelled))
            .expect("real TQ encode, concat, score and selection");
        ParallelEncoder::default()
            .execute(&mut condor, tx.clone(), Arc::clone(&cancelled))
            .expect("actual final scene encodes");
        SceneConcatenator::default()
            .execute(&mut condor, tx.clone(), Arc::clone(&cancelled))
            .expect("actual final concat");
        QualityCheck { input: None }
            .execute(&mut condor, tx, cancelled)
            .expect("actual final CPU quality check");
        assert_eq!(
            decode(&condor.output.path).len(),
            36 * 128 * 128 * 3 / 2,
            "independent final frame count"
        );
        let mut summary = Vec::new();
        for (index, scene) in condor.scenes.iter().enumerate() {
            let data = &scene.sequence_data.target_quality;
            assert!(data.selection.is_some(), "actual selection provenance");
            let quantizer = scene.encoder.quantizer().expect("selected q");
            let selected = data
                .passes
                .iter()
                .find(|pass| pass.quantizer == quantizer)
                .expect("measured selected q");
            assert!(selected.provenance.is_some() && selected.backend.is_some());
            if matches!(case, "x264" | "x265") {
                assert_eq!(
                    selected.provenance.as_ref().expect("owning stamp").context["probe_encoding"],
                    "independent-scenes-1"
                );
            }
            assert_eq!(selected.scores.len(), 12);
            let probe_score = metric
                .calculate_score(ProbeStatistic::Mean, &selected.scores)
                .expect("probe mean");
            let final_quality = &scene.sequence_data.quality_check.quality;
            assert!(final_quality.provenance.is_some() && final_quality.backend.is_some());
            assert_eq!(final_quality.scores.len(), 12);
            let final_score = metric
                .calculate_score(ProbeStatistic::Mean, &final_quality.scores)
                .expect("final mean");
            assert!(
                metric.score_within_target(probe_score),
                "selected probe must hit 94..96"
            );
            println!(
                "native probe {case} zone_eligible={grouped} scene={index} frames={}..{} q={quantizer} probe={probe_score:.9} final={final_score:.9} history={:?} backend={:?}",
                scene.start_frame,
                scene.end_frame,
                data.passes
                    .iter()
                    .map(|p| (
                        p.quantizer,
                        p.scores.iter().sum::<f64>() / p.scores.len() as f64
                    ))
                    .collect::<Vec<_>>(),
                selected.backend
            );
            // The existing target is ±1 VMAF. A certified hit must remain a
            // hit in the final per-scene encode; do not loosen after a failure.
            if metric.score_within_target(probe_score) && !metric.score_within_target(final_score) {
                failures.push(format!("{case} grouped={grouped} scene {index}: certified q={quantizer} score={probe_score}, final score={final_score} outside 94..96"));
            }
            summary.push((quantizer, probe_score, final_score));
        }
        summaries.push(summary);
        assert_eq!(fs::read(&source).expect("unchanged source"), source_bytes);
        if matches!(case, "x264" | "x265") && grouped {
            use crate::models::sequence::provenance::{self, ResultProvenance};
            // Migrate actual owning-producer output, not synthetic completed scores.
            let current = condor.as_data();
            let mut old = current.clone();
            let config = old.sequence_config.target_quality.as_ref().expect("TQ");
            for scene in &mut old.scenes {
                for pass in &mut scene.sequence_data.target_quality.passes {
                    pass.provenance
                        .as_mut()
                        .expect("actual pass stamp")
                        .context
                        .as_object_mut()
                        .expect("context object")
                        .remove("probe_encoding");
                }
                // Keep the old snapshot internally consistent: its selection and
                // chunk dependency bind the old raw history, not the current one.
                scene.sequence_data.target_quality.selection = Some(ResultProvenance::new(
                    provenance::selection_context(scene, config).expect("old selection"),
                ));
                provenance::admit_selection(scene, Some(config), 0)
                    .expect("old selection really binds old history");
                scene
                    .sequence_data
                    .parallel_encoder
                    .provenance
                    .as_mut()
                    .expect("chunk stamp")
                    .context["tq_dependency"] =
                    serde_json::to_value(&scene.sequence_data.target_quality.selection)
                        .expect("old dependency");
            }
            let selection_changed = old.scenes[0].sequence_data.target_quality.selection
                != current.scenes[0].sequence_data.target_quality.selection;
            let frozen = std::iter::once(source.clone())
                .chain(std::iter::once(condor.output.path.clone()))
                .chain(tasks.iter().map(|task| task.output.clone()))
                .map(|path| {
                    let bytes = fs::read(&path).expect("real artifact");
                    let modified =
                        fs::metadata(&path).expect("metadata").modified().expect("mtime");
                    (path, bytes, modified)
                })
                .collect::<Vec<_>>();
            for (label, model) in [("current", current), ("old", old)] {
                let saved = dir.join(format!("{label}.json"));
                let bytes = serde_json::to_vec(&model).expect("actual saved job");
                fs::write(&saved, &bytes).expect("saved job bytes");
                let saved_modified =
                    fs::metadata(&saved).expect("saved metadata").modified().expect("saved mtime");
                // Concat is a new mux operation, not a reusable completed stage.
                let owners: &[&str] = if label == "current" {
                    &["TQ", "PE"]
                } else {
                    &["TQ", "PE", "concat"]
                };
                for &owner in owners {
                    let model: crate::models::Condor<DefaultSequenceData, DefaultSequenceConfig> =
                        serde_json::from_slice(&fs::read(&saved).expect("saved job"))
                            .expect("production JSON roundtrip");
                    let mut resumed = Condor::new(
                        Input::from_data(&model.input).expect("reopened input"),
                        Output::new(&model.output).expect("reopened output"),
                        model.encoder,
                        model.scenes,
                        Some(model.sequence_config),
                        Box::new(|_| anyhow::bail!("resume must not publish new work")),
                    );
                    let before = serde_json::to_vec(&resumed.as_data()).expect("before admission");
                    let (tx, rx) = mpsc::channel();
                    let result = match owner {
                        "TQ" => TargetQuality::default().execute_cancel(
                            &mut resumed,
                            tx,
                            Default::default(),
                        ),
                        "PE" => ParallelEncoder::default().execute_cancel(
                            &mut resumed,
                            tx,
                            Default::default(),
                        ),
                        _ => SceneConcatenator::default().execute_cancel(
                            &mut resumed,
                            tx,
                            Default::default(),
                        ),
                    };
                    if label == "current" {
                        result.unwrap_or_else(|e| panic!("current {owner} resume: {e:#}"));
                    } else {
                        let error = format!(
                            "{:#}",
                            result.expect_err("old H.26x policy must refuse before work")
                        );
                        assert!(
                            error.contains("TQ probe")
                                && error.contains("context changed")
                                && error.contains("recompute in an empty work directory"),
                            "{owner}: {error}"
                        );
                        assert!(
                            rx.try_iter().next().is_none(),
                            "refuse before progress/work"
                        );
                    }
                    assert_eq!(
                        serde_json::to_vec(&resumed.as_data()).expect("after admission"),
                        before
                    );
                    assert_eq!(fs::read(&saved).expect("saved bytes preserved"), bytes);
                    assert_eq!(
                        fs::metadata(&saved)
                            .expect("saved metadata")
                            .modified()
                            .expect("saved mtime"),
                        saved_modified
                    );
                    for (path, original, modified) in &frozen {
                        assert_eq!(
                            &fs::read(path).expect("artifact/source preserved"),
                            original
                        );
                        assert_eq!(
                            &fs::metadata(path).expect("metadata").modified().expect("mtime"),
                            modified
                        );
                    }
                    println!(
                        "native probe/b {label} {case} {owner}: admission checked; config/artifact/source bytes and mtimes preserved"
                    );
                }
            }
            assert!(
                selection_changed,
                "selection hash must bind the changed raw probe context"
            );
        }
    }
    println!("native probe {case} eligible/ineligible quantizers and scores: {summaries:?}");
    if matches!(case, "x264" | "x265") {
        assert_eq!(
            summaries[0], summaries[1],
            "H.26x structural eligibility must not change independent TQ measurements"
        );
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    fs::write(root.join("verified"), "real CPU routes scored and checked")
        .expect("capability handshake");
}

// A clean nonzero exit after complete input must reach scene finalization, not
// bypass its stop flag and permits. Raw bytes also distinguish it from an I/O error.
const FAKE: &str = r#"#!/bin/sh
set -eu
argv=$(printf '%s\n' "$@")
pass=1 total=1 stats= records= scene= partial= out=
while [ "$#" -gt 0 ]; do
    case "$1" in
        --pass) pass=$2; total=2; shift ;;
        --stats) stats=$2; shift ;;
        --encoding-failure-records) records=$2; shift ;;
        --encoding-failure-scene) scene=$2; shift ;;
        --encoding-failure-partial) partial=$2; shift ;;
        -o) out=$2; shift ;;
    esac
    shift
done
printf '%s:%s/%s\n' "$scene" "$pass" "$total" >> "$records/order"
printf '%s\n' "$argv" > "$records/argv"
if [ "$ENCODER_FIXTURE_CASE" = zone-worker ]; then
    exec 0<&-
else
    cat > "$records/input.y4m"
fi
case "$ENCODER_FIXTURE_CASE" in
    parallel-*)
        # Per-scene attempts own private paths; never fabricate a legacy caller temp.
        if [ "$out" = /dev/null ]; then partial="$stats.partial"; else partial=$out; fi ;;
esac
printf '%s' "$partial" > "$records/partial-path"
printf partial > "$partial"
if [ -n "$stats" ]; then
    [ -d "${stats%/*}" ]
    printf owned > "$stats"
fi
printf 'encoding failure sequence stdout\377\r\n'
printf '1/4 frames\r\nE27 sequence stderr\376' >&2
printf completed > "$records/complete"
exit 42
"#;

#[test]
fn sequence_failures_report_status_and_cleanup_owned_outputs() {
    let temp = tempfile::tempdir().expect("fixture");
    let root = temp.path();
    fs::write(
        root.join("watchdog.rs"),
        include_str!("../../../../../tests/core/encoder/fixtures/fake_encoder.rs"),
    )
    .expect("watchdog source");
    let compiled = Command::new("rustc")
        .arg(root.join("watchdog.rs"))
        .arg("-o")
        .arg(root.join("watchdog"))
        .output()
        .expect("rustc");
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let mut failures = Vec::new();
    for case in [
        "parallel-vs1",
        "parallel-vs2",
        "parallel-native1",
        "parallel-native2",
        "zone",
        "zone-spawn",
        "zone-worker",
        "serial",
    ] {
        let job = root.join(case);
        let cwd = job.join("cwd");
        fs::create_dir_all(&cwd).expect("isolated job");
        fs::write(cwd.join("sentinel.stat"), "CWD sentinel").expect("sentinel");
        let file = fs::File::create(job.join("child.log")).expect("log");
        let status = Command::new(root.join("watchdog"))
            .arg("--encoder-fixture-watchdog")
            .arg(std::env::current_exe().expect("test binary"))
            .arg(&job)
            .arg(case)
            .arg("core::sequence::zone_encoder::tests::runtime::sequence_failure_child")
            .current_dir(&cwd)
            .stdout(file.try_clone().expect("clone log"))
            .stderr(file)
            .status()
            .expect("watchdog");
        let log = fs::read_to_string(job.join("child.log")).expect("log");
        if !status.success()
            || !job.join("executed").exists()
            || !job.join("verified").exists()
            || !log.contains("test result: ok. 1 passed;")
            || !log
                .lines()
                .any(|line| line == "encoder fixture watchdog reaped 0 descendants; timeout=false")
        {
            failures.push(format!("{case}: {status}\n{log}"));
        }
        assert_eq!(fs::read_dir(&cwd).expect("cwd listing").count(), 1);
        assert_eq!(
            fs::read_to_string(cwd.join("sentinel.stat")).expect("sentinel"),
            "CWD sentinel"
        );
        eprint!("{log}");
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn input(root: &Path, native: bool, side: usize) -> Input {
    if native {
        let path = root.join("four frames.y4m");
        let mut bytes = b"YUV4MPEG2 W64 H64 F24:1 Ip A1:1 C420jpeg\n".to_vec();
        for _ in 0..4 {
            bytes.extend(frame());
        }
        fs::write(&path, bytes).expect("native source");
        Input::from_video(&InputModel::Video {
            path,
            import_method: ImportMethod::FFMS2 {
                index: None,
                cache_path: Some(root.join("native.ffindex")),
            },
            filters: vec![],
        })
        .expect("native FFMS2 input")
    } else {
        let script = root.join("four frames.vpy");
        fs::write(&script, format!("from vapoursynth import core, YUV420P8\ncore.std.BlankClip(width={side},height={side},length=4,format=YUV420P8,fpsnum=24,fpsden=1,color=[48,128,128]).set_output()\n")).expect("finite VS script");
        Input::from_data(&InputModel::VapourSynthScript {
            source: VapourSynthScriptSource::Path(script),
            variables: HashMap::new(),
            index: 0,
            filters: vec![],
            stream_concurrently: false,
        })
        .expect("nonstream VS input")
    }
}

fn frame() -> Vec<u8> {
    let mut bytes = b"FRAME\n".to_vec();
    bytes.extend(vec![48; 64 * 64]);
    bytes.extend(vec![128; 64 * 64 / 2]);
    bytes
}

fn encoder(root: &Path, scene: usize, total: u8, partial: &Path) -> Encoder {
    let mut encoder = Encoder::default_from_base(&EncoderBase::X264, true);
    encoder.set_executable(Some(root.join("fake encoder")));
    encoder.set_append_color_data(false);
    *encoder.passes_mut().expect("passes") = EncoderPasses::All(total);
    for (key, value) in [
        (
            "encoding-failure-records",
            root.to_str().expect("utf8").to_owned(),
        ),
        ("encoding-failure-scene", scene.to_string()),
        (
            "encoding-failure-partial",
            partial.to_str().expect("utf8").to_owned(),
        ),
    ] {
        encoder
            .parameters_mut()
            .insert(key.into(), CLIParameter::new_string("--", " ", &value));
    }
    encoder
}

fn diagnostic(root: &Path, result: &EncoderResult) {
    assert_eq!(result.encoder, EncoderBase::X264);
    assert_eq!(result.status.code(), Some(42));
    let argv: Vec<_> = fs::read_to_string(root.join("argv"))
        .expect("actual argv")
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(result.parameters, argv);
    let StringOrBytes::Bytes(stdout) = &result.stdout else {
        panic!("raw stdout lost");
    };
    let StringOrBytes::Bytes(stderr) = &result.stderr else {
        panic!("raw stderr lost");
    };
    assert_eq!(stdout, b"encoding failure sequence stdout\xff\r\n");
    assert_eq!(stderr, b"1/4 frames\r\nE27 sequence stderr\xfe");
    assert_eq!(
        fs::read_to_string(root.join("complete")).expect("child finished before return"),
        "completed"
    );
}

#[test]
fn sequence_failure_child() {
    let Some(root) = std::env::var_os("ENCODER_FIXTURE_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let case = std::env::var("ENCODER_FIXTURE_CASE").expect("case");
    fs::write(root.join("executed"), &case).expect("execution handshake");
    fs::write(root.join("fake encoder"), FAKE).expect("fake");
    fs::set_permissions(root.join("fake encoder"), fs::Permissions::from_mode(0o755))
        .expect("executable");
    let native = case.contains("native");
    let mut input = input(&root, native, if case == "zone-worker" { 4096 } else { 64 });
    assert_eq!(
        input.streams_concurrently(),
        native,
        "actual selected parallel path"
    );
    assert_eq!(input.clip_info().expect("clip info").num_frames, 4);
    let scenes_dir = root.join("scenes");
    fs::create_dir_all(&scenes_dir).expect("scenes");
    let sentinel = scenes_dir.join("unrelated.stat");
    fs::write(&sentinel, "unrelated stats").expect("sentinel");
    let cancelled = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    let serial = case == "serial";
    let zone = case.starts_with("zone");
    let total = if case.ends_with('2') || serial { 2 } else { 1 };
    let first = usize::from(!serial);
    let count = if zone { 3 } else { 1 };
    let temp_output = scenes_dir.join(if zone {
        "zone-pass.temp.264".to_owned()
    } else if serial {
        "00000.264".to_owned()
    } else {
        "00001.temp.264".to_owned()
    });
    let mut first_encoder = encoder(&root, first, total, &temp_output);
    let error = if zone {
        if case == "zone-spawn" {
            let unspawnable = root.join("unspawnable encoder");
            fs::write(
                &unspawnable,
                format!("#!{}\n", root.join("missing-interpreter").display()),
            )
            .expect("invalid interpreter fixture");
            fs::set_permissions(&unspawnable, fs::Permissions::from_mode(0o755))
                .expect("executable fixture");
            first_encoder.set_executable(Some(unspawnable));
        }
        let tasks = (1..4)
            .map(|index| Task {
                original_index: index,
                index: index - 1,
                frame_indices: vec![index],
                sub_scenes: None,
                encoder: first_encoder.clone(),
                output: scenes_dir.join(format!("{index:05}.264")),
            })
            .collect();
        let plan = ZonePlan {
            zones: vec![],
            zone_option: "0,2,crf=23".to_owned(),
            qpfile_content: "1 I\n2 I\n".to_owned(),
            config_content: String::new(),
        };
        ZoneEncoder::encode_tasks(&mut input, tasks, plan, tx, cancelled).expect_err("zone failure")
    } else {
        let scenes = (0..4)
            .map(|index| Scene {
                start_frame: index,
                end_frame: index + 1,
                sub_scenes: None,
                encoder: encoder(
                    &root,
                    index,
                    total,
                    &scenes_dir.join(format!(
                        "{index:05}{}.264",
                        if serial { "" } else { ".temp" }
                    )),
                ),
                sequence_data: DefaultSequenceData::default(),
            })
            .collect();
        let saved = Arc::new(Mutex::new(Vec::new()));
        let saved_clone = Arc::clone(&saved);
        let mut config = DefaultSequenceConfig::default();
        config.parallel_encoder.workers = Some(1);
        config.parallel_encoder.max_tries = 1; // One-attempt failed-status/pass-order control.
        config.parallel_encoder.scenes_directory = scenes_dir.clone();
        config.parallel_encoder.validate_frame_count = false; // Scheduler markers, not native output.
        let mut condor = Condor::<DefaultSequenceData, DefaultSequenceConfig>::new(
            input,
            Output {
                path: root.join("unused"),
                overwrite: false,
                tags: HashMap::new(),
                video_tags: HashMap::new(),
            },
            first_encoder,
            scenes,
            Some(config),
            Box::new(move |data| {
                saved_clone.lock().expect("saved snapshots").push(
                    data.scenes.iter().skip(usize::from(!serial)).any(|s| {
                        s.sequence_data.parallel_encoder.bytes.is_some()
                            || s.sequence_data.parallel_encoder.completed_on.is_some()
                    }),
                );
                Ok(())
            }),
        );
        if !serial {
            fs::write(scenes_dir.join("00000.264"), "preexisting scene")
                .expect("preexisting final");
            let data = &mut condor.scenes[0].sequence_data.parallel_encoder;
            data.bytes = Some(17);
            data.started_on = Some(10);
            data.completed_on = Some(20);
            crate::core::sequence::tests::support::current_encoding_fixture(&mut condor);
        }
        let error = if serial {
            SerialEncoder::new(&scenes_dir).execute(&mut condor, tx, cancelled)
        } else {
            ParallelEncoder::default().execute(&mut condor, tx, cancelled)
        }
        .expect_err("scene failure, not panic/hang");
        assert!(
            !saved.lock().expect("saved snapshots").iter().any(|&success| success),
            "failed result must not be persisted as successful"
        );
        assert!(
            condor.scenes.iter().skip(usize::from(!serial)).all(|s| s
                .sequence_data
                .parallel_encoder
                .bytes
                .is_none()
                && s.sequence_data.parallel_encoder.completed_on.is_none())
        );
        if !serial {
            let data = &condor.scenes[0].sequence_data.parallel_encoder;
            assert_eq!(
                (data.bytes, data.started_on, data.completed_on),
                (Some(17), Some(10), Some(20)),
                "eligible historical peer is not discarded with current failure"
            );
            assert_eq!(
                fs::read_to_string(scenes_dir.join("00000.264")).expect("preexisting final"),
                "preexisting scene"
            );
        }
        error
    };
    if case == "zone-spawn" {
        assert!(error.downcast_ref::<ParallelEncoderError>().is_none());
        assert!(error.downcast_ref::<EncoderResult>().is_none());
        assert!(
            format!("{error:#}").contains("Could not spawn"),
            "{error:#}"
        );
        assert!(
            !root.join("order").exists(),
            "generic spawn failure is not an exit status"
        );
    } else {
        let worker = case == "zone-worker";
        let failed = if serial || worker {
            if worker {
                assert!(
                    error.downcast_ref::<ParallelEncoderError>().is_none(),
                    "worker error must not be downgraded to a clean status"
                );
                let process = error
                    .downcast_ref::<crate::core::encoder::EncoderProcessError>()
                    .expect("worker error type retained");
                assert!(process.failures.iter().any(|e| format!("{e:#}").contains("STDIN")));
            }
            error
                .chain()
                .find_map(|e| e.downcast_ref::<EncoderResult>())
                .expect("serial retains typed result through joins")
        } else {
            let ParallelEncoderError::EncoderFailed { scene, result } = error
                .downcast_ref::<ParallelEncoderError>()
                .expect("scene-numbered status error")
            else {
                panic!("wrong scene error");
            };
            assert_eq!(*scene, first);
            assert!(error.to_string().contains("Scene 1"));
            result
        };
        diagnostic(&root, failed);
        assert_eq!(
            fs::read_to_string(root.join("order")).expect("process records"),
            format!("{first}:1/{total}\n"),
            "no later pass or scene"
        );
        let mut expected =
            format!("YUV4MPEG2 C420 W64 H64 F24:1 Ip A0:0 XLENGTH={count}\n").into_bytes();
        for _ in 0..count {
            expected.extend(frame());
        }
        if !worker {
            assert_eq!(
                fs::read(root.join("input.y4m")).expect("all consumed input"),
                expected
            );
        }
        if serial {
            assert_eq!(
                fs::read_to_string(&temp_output).expect("serial partial policy unchanged"),
                "partial"
            );
        }
    }
    if !serial {
        assert!(
            !temp_output.exists(),
            "owned failed temporary output removed"
        );
        if case != "zone-spawn" {
            let path = fs::read_to_string(root.join("partial-path"))
                .expect("actual codec partial path recorded");
            assert!(
                !Path::new(&path).exists(),
                "actual owned codec partial removed after joins"
            );
        }
    }
    assert!(
        !scenes_dir.join("zone.qpfile").exists(),
        "owned zone file removed"
    );
    assert!(!fs::read_dir(&scenes_dir).expect("listing").any(|entry| {
        let name = entry.expect("entry").file_name();
        let name = name.to_string_lossy();
        name.starts_with(".av1an-stats-") || name.starts_with(".av1an-attempt-")
    }));
    assert_eq!(
        fs::read_to_string(&sentinel).expect("stats sentinel"),
        "unrelated stats"
    );
    let events: Vec<_> = rx.try_iter().collect();
    assert!(
        matches!(rx.try_recv(), Err(mpsc::TryRecvError::Disconnected)),
        "all sequence progress senders joined/dropped before return"
    );
    let sizes: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            SequenceStatus::Whole(Status::Processing {
                id,
                completion:
                    SequenceCompletion::Custom {
                        name, completed, ..
                    },
            }) if matches!(name.as_str(), "size" | "resumed-size") => {
                Some((id.as_str(), *completed))
            },
            _ => None,
        })
        .collect();
    assert_eq!(
        sizes,
        if !zone && !serial {
            vec![("0", 17.0)]
        } else {
            vec![]
        },
        "only eligible historical scene reports size; failed scenes must not report successful size"
    );
    fs::write(root.join("verified"), "all assertions ran").expect("verification handshake");
    eprintln!(
        "encoding failure sequence {case}: returned Err without panic/hang; case-specific diagnostics, ordering, persistence, cleanup and sentinels verified"
    );
}
