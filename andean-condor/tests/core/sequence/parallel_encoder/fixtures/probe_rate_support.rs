//! Public rate/admission fixture, separate from source-only retry-boundary hooks.
use super::*;
use condor::{
    core::output::Output,
    models::{
        encoder::{Encoder, EncoderBase, EncoderPasses},
        scene::Scene,
        sequence::{DefaultSequenceConfig, DefaultSequenceData},
    },
    utils::cancellation::Cancellation,
};
use std::{fs, os::unix::fs::PermissionsExt, process::Command, sync::Mutex};

pub(super) fn rate_cases(test: &str, kinds: &[&str]) {
    if let Ok(case) = std::env::var("ENCODER_FIXTURE_CASE") {
        let rest = case.strip_prefix("probe-rate-").expect("rate case");
        let (path, kind) = rest.split_once('-').expect("rate path-kind");
        assert!(["native", "vs"].contains(&path) && kinds.contains(&kind));
        actual_rate_child();
        println!("probe-rate {rest}: assertions completed");
        return;
    }
    let temp = tempfile::tempdir().expect("fixture");
    let root = temp.path();
    fs::write(
        root.join("watchdog.rs"),
        include_str!("../../../encoder/fixtures/fake_encoder.rs"),
    )
    .expect("supervisor");
    assert!(
        Command::new("rustc")
            .arg(root.join("watchdog.rs"))
            .arg("-o")
            .arg(root.join("watchdog"))
            .status()
            .expect("rustc")
            .success()
    );
    let mut failures = Vec::new();
    for path in ["native", "vs"] {
        for &kind in kinds {
            let case = format!("probe-rate-{path}-{kind}");
            let job = root.join(&case);
            fs::create_dir_all(&job).expect("job");
            let output = Command::new(root.join("watchdog"))
                .arg("--encoder-fixture-watchdog")
                .arg(std::env::current_exe().expect("test binary"))
                .arg(&job)
                .arg(&case)
                .arg(test)
                .output()
                .expect("bounded child");
            let text = format!(
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            eprintln!("probe-rate {case}: {text}");
            if !output.status.success()
                || !job.join("verified").exists()
                || !text.contains("test result: ok. 1 passed;")
                || !text.contains(&format!("probe-rate {path}-{kind}: assertions completed"))
                || !text.lines().any(|line| {
                    line == "encoder fixture watchdog reaped 0 descendants; timeout=false"
                })
            {
                failures.push(format!("{case}: {text}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[allow(clippy::too_many_lines)]
fn actual_rate_child() {
    use av_format::rational::Rational64;
    use condor::{
        core::sequence::zone_encoder::{ZoneEncoder, ZonePlan},
        models::input::{Input as InputModel, VapourSynthScriptSource},
        utils::cancellation::Cancelled,
    };
    let Ok(case) = std::env::var("ENCODER_FIXTURE_CASE") else {
        return;
    };
    let Some(case) = case.strip_prefix("probe-rate-") else {
        return;
    };
    let (path, kind) = case.split_once('-').expect("path-kind");
    let root = std::path::PathBuf::from(std::env::var_os("ENCODER_FIXTURE_ROOT").expect("root"));
    fs::write(root.join("executed"), case).expect("handshake");
    let mut input = if path == "native" {
        input(&root, true)
    } else {
        let script = root.join("source.vpy");
        fs::write(&script,"from vapoursynth import core,YUV420P8\ncore.std.BlankClip(width=64,height=64,length=240,format=YUV420P8,fpsnum=24,fpsden=1).set_output()\n").expect("source");
        Input::from_data(&InputModel::VapourSynthScript {
            source: VapourSynthScriptSource::Path(script),
            variables: Default::default(),
            index: 0,
            filters: vec![],
            stream_concurrently: false,
        })
        .expect("real nonstream VS")
    };
    assert_eq!(input.streams_concurrently(), path == "native");
    let mut info = input.clip_info().expect("metadata");
    let codec = root.join("codec");
    fs::write(&codec, format!("#!/bin/sh\nset -eu\nout=\nwhile [ \"$#\" -gt 0 ]; do\nif [ \"$1\" = -o ]; then shift; out=$1; fi\nshift\ndone\nif [ \"$out\" != /dev/null ]; then printf started >> {:?}; fi\ncat >/dev/null\nprintf '%01000d' 0 >\"$out\"\n", root.join("spawned"))).expect("finite codec");
    fs::set_permissions(&codec, fs::Permissions::from_mode(0o755)).expect("executable");
    let mut encoder = Encoder::default_from_base(&EncoderBase::X264, true);
    encoder.set_quantizer(23.0);
    encoder.set_append_color_data(false);
    encoder.set_executable(Some(codec));
    *encoder.passes_mut().expect("passes") = EncoderPasses::All(1);
    let task = |empty| Task {
        original_index: 7,
        index: 0,
        frame_indices: if empty {
            vec![]
        } else if path == "native" {
            vec![0; 240]
        } else {
            (0..240).collect()
        },
        sub_scenes: None,
        encoder: encoder.clone(),
        output: root.join("result.264"),
    };
    let options = ParallelEncoderOptions {
        max_tries: 1,
        validate_frame_count: false,
    };
    let set_rate = |input: &mut Input, rate| {
        info.frame_rate = rate;
        match input {
            Input::Video { clip_info, .. }
            | Input::VapourSynth { clip_info, .. }
            | Input::VapourSynthScript { clip_info, .. } => *clip_info = Some(info),
        }
    };
    let (tx, _rx) = std::sync::mpsc::channel();
    let zone_plan = ZonePlan::try_build(&[task(false)]).expect("eligible zone plan");
    let plan = || zone_plan.clone();
    if matches!(kind, "integer" | "fractional") {
        let rate = if kind == "integer" {
            Rational64::new(24, 1)
        } else {
            Rational64::new(24000, 1001)
        };
        let mut set_rate = set_rate;
        set_rate(&mut input, rate);
        let result = ParallelEncoder::encode_tasks_with_options(
            &mut input,
            1,
            [task(false)].into(),
            tx,
            Cancellation::default(),
            options,
        )
        .expect("owning encode")
        .remove(0)
        .expect("completed");
        assert_eq!(result.scene, 7);
        assert_eq!(result.bytes, 1000);
        assert_eq!(
            fs::metadata(root.join("result.264")).expect("published").len(),
            1000
        );
        let expected = if kind == "integer" {
            800.0
        } else {
            800000.0 / 1001.0
        };
        assert!(
            (result.bitrate - expected).abs() < 1e-10,
            "actual bps: {} != {expected}",
            result.bitrate
        );
    } else if kind == "invalid" {
        let mut set_rate = set_rate;
        for (rate, empty) in [
            (Rational64::new(0, 1), false),
            (Rational64::new(-24, 1), false),
            (Rational64::new_raw(1, 0), false),
            (Rational64::new_raw(0, 0), false),
            (Rational64::new(24, 1), true),
        ] {
            set_rate(&mut input, rate);
            let error = ParallelEncoder::encode_tasks_with_options(
                &mut input,
                1,
                [task(empty)].into(),
                tx.clone(),
                Cancellation::default(),
                options,
            )
            .expect_err("reject before worker");
            assert!(format!("{error:#}").contains("bitrate"), "{error:#}");
            let error = ZoneEncoder::encode_tasks_with_validation(
                &mut input,
                [task(empty)].into(),
                plan(),
                tx.clone(),
                Cancellation::default(),
                false,
            )
            .expect_err("zone rejects before worker");
            assert!(format!("{error:#}").contains("bitrate"), "{error:#}");
            assert!(!root.join("spawned").exists() && !root.join("result.264").exists());
        }
        let mut config = DefaultSequenceConfig::default();
        config.parallel_encoder.scenes_directory = root.join("scenes");
        config.parallel_encoder.validate_frame_count = false;
        fs::create_dir_all(&config.parallel_encoder.scenes_directory).expect("scenes");
        let saves = Arc::new(Mutex::new(0));
        let seen = Arc::clone(&saves);
        let mut condor = Condor::new(
            input,
            Output {
                path: root.join("unused"),
                overwrite: false,
                tags: Default::default(),
                video_tags: Default::default(),
            },
            encoder.clone(),
            vec![Scene {
                start_frame: 0,
                end_frame: 0,
                sub_scenes: None,
                encoder,
                sequence_data: DefaultSequenceData::default(),
            }],
            Some(config),
            Box::new(move |_| {
                *seen.lock().expect("saves") += 1;
                Ok(())
            }),
        );
        for (rate, frames) in [(Rational64::new(24, 1), 0), (Rational64::new(0, 1), 1)] {
            set_rate(&mut condor.input, rate);
            condor.scenes[0].end_frame = frames;
            let error = ParallelEncoder::default()
                .execute_cancel(&mut condor, tx.clone(), Cancellation::default())
                .expect_err("sequence rejects invalid rate/count");
            assert!(format!("{error:#}").contains("bitrate"), "{error:#}");
            assert_eq!(*saves.lock().expect("saves"), 0);
            assert!(condor.scenes[0].sequence_data.parallel_encoder.completed_on.is_none());
            assert!(!root.join("spawned").exists());
        }
    } else if kind == "no-work" {
        let mut set_rate = set_rate;
        set_rate(&mut input, Rational64::new_raw(0, 0));
        assert!(
            ParallelEncoder::encode_tasks_with_options(
                &mut input,
                1,
                Default::default(),
                tx.clone(),
                Cancellation::default(),
                options
            )
            .expect("empty no-op")
            .is_empty()
        );
        assert!(
            ZoneEncoder::encode_tasks_with_validation(
                &mut input,
                Default::default(),
                plan(),
                tx.clone(),
                Cancellation::default(),
                false
            )
            .expect("empty zone no-op")
            .is_empty()
        );
        let cancelled = Cancellation::default();
        cancelled.request().expect("graceful");
        assert!(
            ParallelEncoder::encode_tasks_with_options(
                &mut input,
                1,
                [task(false)].into(),
                tx.clone(),
                cancelled.clone(),
                options
            )
            .expect("cancelled no-op")[0]
                .is_none()
        );
        cancelled.request().expect("forced");
        let error = ParallelEncoder::encode_tasks_with_options(
            &mut input,
            1,
            [task(false)].into(),
            tx,
            cancelled,
            options,
        )
        .expect_err("force takes precedence");
        assert_eq!(error.downcast_ref::<Cancelled>(), Some(&Cancelled::Forced));
        assert!(!root.join("spawned").exists());
    } else {
        let generated = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=s=64x64:r=24",
                "-frames:v",
                "240",
                "-c:v",
                "libx264",
                "-f",
                "h264",
            ])
            .arg(root.join("fixture.264"))
            .output()
            .expect("native fixture");
        assert!(generated.status.success(), "{generated:?}");
        fs::write(root.join("codec"), format!("#!/bin/sh\nset -eu\nout=\nwhile [ \"$#\" -gt 0 ]; do\nif [ \"$1\" = -o ]; then shift; out=$1; fi\nshift\ndone\ncat >/dev/null\ncp {:?} \"$out\"\n", root.join("fixture.264"))).expect("fixture codec");
        let mut set_rate = set_rate;
        for (rate, bits_per_byte_per_second) in
            [(Rational64::new(24, 1), 0.8), (Rational64::new(24000, 1001), 800.0 / 1001.0)]
        {
            set_rate(&mut input, rate);
            let results = ZoneEncoder::encode_tasks_with_validation(
                &mut input,
                [task(false)].into(),
                plan(),
                tx.clone(),
                Cancellation::default(),
                true,
            )
            .expect("actual zone encode and split");
            let result = results[0].as_ref().expect("completed zone");
            let bytes = fs::metadata(root.join("result.264")).expect("split output").len();
            assert_eq!(result.bytes, bytes);
            let expected = bytes as f64 * bits_per_byte_per_second;
            assert!((result.bitrate - expected).abs() < 1e-10);
            fs::remove_file(root.join("result.264"))
                .expect("remove this fixture's output before testing another rate");
        }
    }
    assert!(
        !fs::read_dir(&root).expect("files").any(|entry| entry
            .expect("entry")
            .file_name()
            .to_string_lossy()
            .contains(".temp.")),
        "owned temps cleaned"
    );
    fs::write(
        root.join("verified"),
        "actual rate/admission contract checked",
    )
    .expect("verified");
}
