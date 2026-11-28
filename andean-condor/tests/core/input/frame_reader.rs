//! Real graph construction counts and exact filtered samples, not decode timing.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
#[cfg(unix)]
use std::{fs, path::Path, process::Command, sync::mpsc};

use ::vapoursynth::map::OwnedMap;

use super::*;
use andean_condor::vapoursynth::get_api;
#[cfg(unix)]
use andean_condor::{
    core::sequence::{
        parallel_encoder::Task,
        zone_encoder::{ZoneEncoder, ZonePlan},
    },
    models::encoder::{Encoder, EncoderBase, cli_parameter::CLIParameter},
};

fn average_modifier(count: &Arc<AtomicUsize>, weights: [f64; 3]) -> ModifyNode {
    let count = Arc::clone(count);
    Box::new(move |core, node| {
        (|| -> Result<_> {
            let mut args = OwnedMap::new(get_api()?);
            args.set_node("clips", &node.expect("script output"))?;
            args.set_float_array("weights", &weights)?;
            let plugin = core.get_plugin_by_namespace("std")?.expect("native std plugin");
            let result = plugin.invoke("AverageFrames", &args)?;
            if let Some(error) = result.error() {
                anyhow::bail!("AverageFrames: {error}");
            }
            let node = result.get_video_node("clip")?;
            count.fetch_add(1, Ordering::Relaxed);
            Ok(node)
        })()
        .map_err(|error| DecoderError::VapoursynthInternalError {
            cause: error.to_string(),
        })
    })
}

fn temporal_input(format: &str, count: &Arc<AtomicUsize>) -> Input {
    let colors = if format == "GRAY16" {
        "[[4096], [10240], [25600], [56320]]"
    } else {
        "[[16,80,160], [40,80,160], [100,80,160], [220,80,160]]"
    };
    let input = InputModel::VapourSynthScript {
        source: VapourSynthScriptSource::Text(format!(
            "import vapoursynth as vs\nc = vs.core\n\
             clips = [c.std.BlankClip(width=32, height=16, length=1, format=vs.{format}, color=v) for v in {colors}]\n\
             c.std.Splice(clips).set_output()"
        )),
        variables: HashMap::new(),
        index: 0,
        filters: vec![],
        stream_concurrently: true,
    };
    Input::from_vapoursynth(&input, Some(average_modifier(count, [1.0, 2.0, 1.0])))
        .expect("native temporal input")
}

#[cfg(unix)]
fn yuv_image(y: u8) -> Vec<u8> {
    [vec![y; 32 * 16], vec![80; 16 * 8], vec![160; 16 * 8]].concat()
}

#[test]
fn indexed_frame_reader_reuses_one_temporal_graph() {
    check_reader_graphs("retained");
}

#[test]
fn replacing_a_node_modifier_rebuilds_the_reader_graph() {
    check_reader_graphs("replacement");
}

#[test]
fn independent_inputs_do_not_share_reader_graphs() {
    check_reader_graphs("independent");
}

fn check_reader_graphs(phase: &str) {
    assert!(["retained", "replacement", "independent"].contains(&phase));
    let count = Arc::new(AtomicUsize::new(0));
    let mut input = temporal_input("GRAY16", &count);
    let opening = count.load(Ordering::Relaxed);
    input.y4m_header(None).expect("warm metadata separately");
    assert_eq!(
        count.load(Ordering::Relaxed) - opening,
        1,
        "metadata-only graph"
    );
    eprintln!("frame-reader graph builds before feed: opening={opening}, metadata=1");
    count.store(0, Ordering::Relaxed);
    let indices = [3, 1, 1, 0, 2, 3, 0, 2, 1];
    let expected: Vec<_> = [48640_u16, 12544, 12544, 5632, 29440, 48640, 5632, 29440, 12544]
        .into_iter()
        .map(|sample| [b"FRAME\n".to_vec(), sample.to_le_bytes().repeat(32 * 16)].concat())
        .collect();
    let (sender, receiver) = crossbeam_channel::bounded(indices.len());
    input
        .y4m_frames(
            sender,
            &indices,
            &andean_condor::utils::cancellation::Cancellation::default(),
        )
        .expect("existing bulk path");
    let bulk: Vec<_> = receiver.iter().map(Cursor::into_inner).collect();
    assert_eq!(
        bulk, expected,
        "native AverageFrames on distinct high-depth images"
    );
    assert_eq!(
        count.load(Ordering::Relaxed),
        1,
        "existing bulk graph count"
    );
    count.store(0, Ordering::Relaxed);
    let mut reader = input.frame_reader().expect("one retained graph");
    for (&index, expected) in indices.iter().zip(&bulk) {
        assert_eq!(
            reader.y4m_frame(index).expect("indexed frame").into_inner(),
            *expected
        );
    }
    let (sender, receiver) = crossbeam_channel::bounded(indices.len());
    reader
        .y4m_frames(
            &sender,
            &indices,
            3,
            &andean_condor::utils::cancellation::Cancellation::default(),
        )
        .expect("retained-node bulk path");
    drop(sender);
    assert_eq!(
        receiver.iter().map(Cursor::into_inner).collect::<Vec<_>>(),
        bulk
    );
    let error = reader.y4m_frame(9).expect_err("invalid absolute index");
    assert!(error.to_string().contains("get y4m frame 9"), "{error:#}");
    assert_eq!(
        reader.y4m_frame(3).expect("reader survives error").into_inner(),
        bulk[0]
    );
    assert_eq!(
        count.load(Ordering::Relaxed),
        1,
        "no per-frame graph builds"
    );
    drop(reader);

    if phase == "retained" {
        return;
    }

    count.store(0, Ordering::Relaxed);
    input
        .decoder()
        .get_vapoursynth_impl()
        .expect("VS decoder")
        .register_node_modifier(average_modifier(&count, [0.0, 1.0, 0.0]))
        .expect("replace modifier after dropping reader");
    assert_eq!(
        count.load(Ordering::Relaxed),
        1,
        "registration resolves metadata separately"
    );
    count.store(0, Ordering::Relaxed);
    let mut changed = input.frame_reader().expect("new graph for new modifier");
    let expected = [b"FRAME\n".to_vec(), 10240_u16.to_le_bytes().repeat(32 * 16)].concat();
    assert_eq!(
        changed.y4m_frame(1).expect("changed filter image").into_inner(),
        expected
    );
    assert_eq!(count.load(Ordering::Relaxed), 1);
    drop(changed);

    if phase == "replacement" {
        return;
    }

    let independent_count = Arc::new(AtomicUsize::new(0));
    let mut independent = temporal_input("GRAY16", &independent_count);
    independent.y4m_header(None).expect("independent metadata warmup");
    independent_count.store(0, Ordering::Relaxed);
    let mut independent_reader = independent.frame_reader().expect("independent graph");
    assert_eq!(
        independent_reader
            .y4m_frame(1)
            .expect("independent temporal image")
            .into_inner(),
        bulk[1]
    );
    assert_eq!(independent_count.load(Ordering::Relaxed), 1);
    assert_eq!(
        count.load(Ordering::Relaxed),
        1,
        "independent input never touches prior graph"
    );
}

#[test]
fn reader_rejects_rgb_and_float_with_indexed_errors() {
    for (format, color) in [("RGB24", "[17,83,201]"), ("YUV444PS", "[0.25,0.5,0.75]")] {
        let count = Arc::new(AtomicUsize::new(0));
        let mut input = temporal_input("YUV420P8", &count);
        input.decoder().get_vapoursynth_impl().expect("VS decoder").env.eval_script(&format!(
            "c.std.BlankClip(width=32, height=16, length=4, format=vs.{format}, color={color}).set_output()"
        )).expect("live output replacement exercises serialization guard");
        let mut reader = input.frame_reader().expect("live output reader");
        let error = reader.y4m_frame(2).expect_err("unsupported frame returns no partial bytes");
        assert!(
            matches!(
                error.downcast_ref::<DecoderError>(),
                Some(DecoderError::UnsupportedFormat { .. })
            ),
            "{error:#}"
        );
        assert!(
            error.to_string().contains("serialize y4m frame 2"),
            "{error:#}"
        );
    }
}

#[test]
#[cfg(unix)]
fn native_reader_reuses_open_decoder_for_absolute_indices() {
    let directory = tempfile::tempdir().expect("native fixture");
    let raw = directory.path().join("source.y4m");
    let source = directory.path().join("source.mkv");
    let cache = directory.path().join("source.ffindex");
    let frames = [16, 40, 100, 220].map(|y| [b"FRAME\n".to_vec(), yuv_image(y)].concat());
    fs::write(
        &raw,
        [b"YUV4MPEG2 C420 W32 H16 F24:1 Ip A0:0\n".as_slice(), &frames.concat()].concat(),
    )
    .expect("source frames");
    let result = Command::new("timeout")
        .args(["20s", "ffmpeg", "-nostdin", "-v", "error", "-i"])
        .arg(&raw)
        .args(["-c:v", "ffv1", "-threads", "1"])
        .arg(&source)
        .output()
        .expect("frame-reader requires timeout and ffmpeg");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let mut input = Input::from_data(&InputModel::Video {
        path: source.clone(),
        import_method: ImportMethod::FFMS2 {
            index: Some(0),
            cache_path: Some(cache.clone()),
        },
        filters: vec![],
    })
    .expect("native decoder");
    let decoder = std::ptr::from_ref(input.decoder());
    // An accidental open/reindex cannot succeed; the existing decoder keeps its open source.
    fs::remove_file(source).expect("unlink exact fixture source");
    fs::remove_file(cache).expect("unlink exact fixture index");
    let mut reader = input.frame_reader().expect("borrow the existing native decoder");
    let indices = [3, 1, 1, 0, 2];
    for index in indices {
        assert_eq!(
            reader.y4m_frame(index).expect("native absolute frame").into_inner(),
            frames[index]
        );
    }
    let (sender, receiver) = crossbeam_channel::bounded(indices.len());
    reader
        .y4m_frames(
            &sender,
            &indices,
            2,
            &andean_condor::utils::cancellation::Cancellation::default(),
        )
        .expect("borrowed native bulk");
    drop(sender);
    assert_eq!(
        receiver.iter().map(Cursor::into_inner).collect::<Vec<_>>(),
        indices.map(|index| frames[index].clone())
    );
    drop(reader);
    assert_eq!(
        std::ptr::from_ref(input.decoder()),
        decoder,
        "same decoder after feed"
    );
}

#[cfg(unix)]
fn bounded_encoder(directory: &Path) -> Encoder {
    let binary = which::which("x264").expect("frame-reader requires x264");
    let timeout = which::which("timeout").expect("frame-reader requires timeout");
    let wrapper = directory.join("bounded-x264");
    // Write outside libtest: concurrent forks cannot inherit this child's writable script fd.
    let written = Command::new(&timeout)
        .args(["20s", "sh", "-c", "printf '#!/bin/sh\nexec \"%s\" 20s \"%s\" \"$@\"\n' \"$2\" \"$3\" > \"$1\" && chmod 755 \"$1\"", "frame-reader-wrapper"])
        .arg(&wrapper)
        .arg(&timeout)
        .arg(&binary)
        .output()
        .expect("frame-reader requires timeout and sh");
    assert!(written.status.success(), "wrapper creation: {written:?}");
    let ready = Command::new(&wrapper)
        .arg("--version")
        .output()
        .expect("immutable wrapper exec");
    assert!(
        ready.status.success(),
        "real x264 wrapper readiness: {ready:?}"
    );
    let mut encoder = Encoder::default_from_base(&EncoderBase::X264, false);
    let Encoder::X264 { executable, .. } = &mut encoder else {
        unreachable!()
    };
    *executable = Some(wrapper);
    encoder.set_quantizer(0.0);
    encoder.parameters_mut().insert(
        "preset".to_owned(),
        CLIParameter::new_string("--", " ", "ultrafast"),
    );
    encoder.parameters_mut().insert(
        "threads".to_owned(),
        CLIParameter::new_number("--", " ", 1.0),
    );
    encoder
}

#[cfg(unix)]
fn decoded(path: &Path) -> Vec<u8> {
    let result = Command::new("timeout")
        .args(["20s", "ffmpeg", "-nostdin", "-v", "error", "-i"])
        .arg(path)
        .args(["-f", "rawvideo", "-pix_fmt", "yuv420p", "-"])
        .output()
        .expect("frame-reader requires timeout and ffmpeg");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    result.stdout
}

#[test]
#[cfg(unix)]
fn encoder_pass_reuses_one_temporal_graph_and_encodes_filtered_samples() {
    let directory = tempfile::tempdir().expect("encoder fixture");
    let count = Arc::new(AtomicUsize::new(0));
    let mut input = temporal_input("YUV420P8", &count);
    input.y4m_header(None).expect("warm metadata separately");
    count.store(0, Ordering::Relaxed);
    let encoder = bounded_encoder(directory.path());
    let output = directory.path().join("pass.264");
    let (progress, _receiver) = mpsc::channel();
    encoder
        .encode_pass_with_input(
            &mut input,
            &output,
            0,
            4,
            (1, 1),
            progress,
            None,
            &andean_condor::utils::cancellation::Cancellation::default(),
        )
        .expect("actual finite encoder pass");
    assert_eq!(
        decoded(&output),
        [22, 49, 115, 190].into_iter().flat_map(yuv_image).collect::<Vec<_>>()
    );
    assert_eq!(
        count.load(Ordering::Relaxed),
        1,
        "one graph for the entire actual encoder pass"
    );
}

#[test]
#[cfg(unix)]
fn zone_tasks_share_one_temporal_graph_and_encode_selected_samples() {
    let directory = tempfile::tempdir().expect("zone fixture");
    let count = Arc::new(AtomicUsize::new(0));
    let mut input = temporal_input("YUV420P8", &count);
    input.y4m_header(None).expect("warm metadata separately");
    count.store(0, Ordering::Relaxed);
    let encoder = bounded_encoder(directory.path());
    let tasks: Vec<_> = [vec![3, 1], vec![1, 0, 2]]
        .into_iter()
        .enumerate()
        .map(|(index, frame_indices)| Task {
            original_index: index,
            index,
            frame_indices,
            sub_scenes: None,
            encoder: encoder.clone(),
            output: directory.path().join(format!("scene{index}.264")),
        })
        .collect();
    let outputs: Vec<_> = tasks.iter().map(|task| task.output.clone()).collect();
    let plan = ZonePlan::try_build(
        &tasks,
        &andean_condor::utils::cancellation::Cancellation::default(),
    )
    .expect("real x264 zone support");
    let (progress, _receiver) = mpsc::channel();
    let results = ZoneEncoder::encode_tasks(
        &mut input,
        tasks.into(),
        plan,
        progress,
        andean_condor::utils::cancellation::Cancellation::default(),
        true,
    )
    .expect("actual finite zone feed");
    assert!(results.iter().all(Option::is_some));
    for (path, samples) in outputs.iter().zip([vec![190, 49], vec![49, 22, 115]]) {
        assert_eq!(
            decoded(path),
            samples.into_iter().flat_map(yuv_image).collect::<Vec<_>>()
        );
    }
    assert_eq!(
        count.load(Ordering::Relaxed),
        1,
        "one graph across both actual zone task loops"
    );
}
