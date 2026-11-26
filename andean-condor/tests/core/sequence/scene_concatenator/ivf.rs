use super::*;
use std::fs;
#[path = "fixtures/ivf_support.rs"]
mod fixture_support;
use fixture_support::{assert_output, decoded, fixture, native, records, set_timestamps};

fn concatenate(output: &Path, paths: &[PathBuf]) -> Result<()> {
    let (tx, _rx) = std::sync::mpsc::channel();
    SceneConcatenator::ivf(output, paths, &tx, &Cancellation::default())
}

#[test]
fn ivf_concat_normalizes_nonzero_input_origins_without_changing_decoded_pixels() {
    let dir = tempfile::tempdir().expect("whole-stream fixture directory");
    let whole = dir.path().join("whole.ivf");
    native(
        "ffmpeg",
        &[
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=64x64:rate=24000/1001",
            "-frames:v",
            "9",
            "-c:v",
            "libaom-av1",
            "-cpu-used",
            "8",
            "-crf",
            "0",
            "-g",
            "1",
        ],
        Some(&whole),
    );
    let mut bytes = fs::read(&whole).expect("read whole IVF");
    set_timestamps(&mut bytes, &[900, 901, 902, 903, 904, 905, 906, 907, 908]);
    fs::write(&whole, &bytes).expect("set nonzero whole origin");
    let original_pixels = decoded(&whole);
    assert_eq!(original_pixels.len(), 64 * 64 * 3 / 2 * 9);
    assert!(original_pixels.as_chunks::<6144>().0.windows(2).all(|p| p[0] != p[1]));
    // Every frame is an independent keyframe. Arrange standard IVF parts here;
    // the private splitter's own payload/count assertions remain source-owned.
    let frames = records(&bytes);
    let paths: Vec<_> = (0..3).map(|index| dir.path().join(format!("part{index}.ivf"))).collect();
    let mut start = 0;
    for (path, count) in paths.iter().zip([2usize, 3, 4]) {
        let mut part = bytes[..32].to_vec();
        part[24..28].copy_from_slice(&(count as u32).to_le_bytes());
        for (index, (_, payload)) in frames[start..start + count].iter().enumerate() {
            part.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            part.extend_from_slice(&(index as u64).to_le_bytes());
            part.extend_from_slice(payload);
        }
        fs::write(path, part).expect("independently arranged native IVF part");
        start += count;
    }
    let output = dir.path().join("joined.ivf");
    concatenate(&output, &paths).expect("concat normalized split parts");
    assert_output(&output, 9, &bytes[16..24], &[0, 1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(decoded(&output), original_pixels);
    // Concat also accepts independently nonzero origins, not only normalized splitter output.
    for (path, timestamps) in
        paths
            .iter()
            .zip([vec![900, 901], vec![902, 903, 904], vec![905, 906, 907, 908]])
    {
        let mut part = fs::read(path).expect("read split part");
        set_timestamps(&mut part, &timestamps);
        fs::write(path, part).expect("set nonzero part origin");
    }
    concatenate(&output, &paths).expect("concat absolute-origin parts");
    assert_output(&output, 9, &bytes[16..24], &[0, 1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(decoded(&output), original_pixels);
    assert_eq!(
        fs::read(&whole).expect("read whole after concat"),
        bytes,
        "whole input untouched"
    );
}

#[test]
fn ivf_concat_preserves_supplied_order_repeated_inputs_and_fractional_frame_rate() {
    let dir = tempfile::tempdir().expect("independent encode fixture directory");
    let paths = (0..3).map(|i| dir.path().join(format!("{i}.ivf"))).collect::<Vec<_>>();
    for (path, color) in paths.iter().zip(["white", "black", "red"]) {
        native(
            "ffmpeg",
            &[
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                &format!("color={color}:size=64x64:rate=24000/1001"),
                "-frames:v",
                "2",
                "-c:v",
                "libaom-av1",
                "-cpu-used",
                "8",
                "-g",
                "1",
            ],
            Some(path),
        );
    }
    let originals = paths
        .iter()
        .map(|p| fs::read(p).expect("read independent encode"))
        .collect::<Vec<_>>();
    let pixels = paths.iter().map(|p| decoded(p)).collect::<Vec<_>>();
    let output = dir.path().join("joined.ivf");
    concatenate(
        &output,
        &[paths[2].clone(), paths[0].clone(), paths[1].clone(), paths[2].clone()],
    )
    .expect("concat independent zero-origin encodes");
    assert_output(&output, 8, &originals[0][16..24], &[0, 1, 2, 3, 4, 5, 6, 7]);
    assert_eq!(
        decoded(&output),
        [&pixels[2][..], &pixels[0], &pixels[1], &pixels[2]].concat()
    );
    for (path, original) in paths.iter().zip(originals) {
        assert_eq!(fs::read(path).expect("read original encode"), original);
    }
}

fn check_concat_boundaries(phase: &str) {
    assert!(["gaps", "boundaries", "headers"].contains(&phase));
    let dir = tempfile::tempdir().expect("boundary fixture directory");
    let a = dir.path().join("a.ivf");
    let b = dir.path().join("b.ivf");
    let output = dir.path().join("joined.ivf");
    fixture(&a, &[50, 52, 55]);
    fixture(&b, &[100, 103]);
    concatenate(&output, &[a.clone(), b.clone()]).expect("concat gapped timestamps");
    assert_eq!(
        records(&fs::read(&output).expect("read gapped output"))
            .iter()
            .map(|r| r.0)
            .collect::<Vec<_>>(),
        [0, 2, 5, 6, 9]
    );
    fixture(&b, &[100, 103]);
    let mut equivalent = fs::read(&b).expect("read equivalent timebase fixture");
    equivalent[16..20].copy_from_slice(&48000u32.to_le_bytes());
    equivalent[20..24].copy_from_slice(&2002u32.to_le_bytes());
    fs::write(&b, equivalent).expect("write equivalent timebase fixture");
    concatenate(&output, &[a.clone(), b.clone()]).expect("concat equivalent timebases");
    assert_eq!(
        &fs::read(&output).expect("read rational rate")[16..24],
        &[192, 93, 0, 0, 233, 3, 0, 0]
    );
    if phase == "gaps" {
        return;
    }
    // Last frame occupies one nominal IVF tick, even when earlier frames have gaps.
    fixture(&a, &[0, u64::MAX - 1]);
    fixture(&b, &[u64::MAX]);
    concatenate(&output, &[a.clone(), b.clone()]).expect("concat at u64 boundary");
    assert_eq!(
        records(&fs::read(&output).expect("read boundary output"))
            .iter()
            .map(|r| r.0)
            .collect::<Vec<_>>(),
        [0, u64::MAX - 1, u64::MAX]
    );
    fixture(&a, &[0, u64::MAX]);
    concatenate(&output, std::slice::from_ref(&a)).expect("allow final u64 max timestamp");
    assert_eq!(
        records(&fs::read(&output).expect("read final boundary output"))
            .iter()
            .map(|r| r.0)
            .collect::<Vec<_>>(),
        [0, u64::MAX]
    );
    for timestamps in [vec![0, u64::MAX], vec![3, 2]] {
        fixture(&a, &timestamps);
        let error = concatenate(&output, &[a.clone(), b.clone()]).expect_err("invalid timestamps");
        assert!(format!("{error:#}").contains("timestamp"), "{error:#}");
    }
    fixture(&a, &[0, u64::MAX - 1]);
    fixture(&b, &[0, 1]);
    let error = concatenate(&output, &[a.clone(), b.clone()]).expect_err("offset overflow");
    assert!(format!("{error:#}").contains("timestamp"), "{error:#}");
    if phase == "boundaries" {
        return;
    }
    fixture(&a, &[0]);
    for (offset, value) in [
        (16, 0u32),
        (20, 0),
        (16, 30),
        (20, 1),
        (16, u32::MAX),
        (8, u32::from_le_bytes(*b"VP90")),
        (12, 1),
    ] {
        fixture(&b, &[0]);
        let mut bytes = fs::read(&b).expect("read invalid header fixture");
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        fs::write(&b, bytes).expect("write invalid header fixture");
        assert!(
            concatenate(&output, &[a.clone(), b.clone()]).is_err(),
            "invalid/incompatible field {offset}: {value}"
        );
    }
    fixture(&b, &[0]);
    let mut bytes = fs::read(&b).expect("read untrusted count fixture");
    bytes[24..28].copy_from_slice(&u32::MAX.to_le_bytes());
    fs::write(&b, bytes).expect("write untrusted count fixture");
    concatenate(&output, &[a.clone(), b]).expect("actual records override declared counts");
    assert_eq!(
        &fs::read(&output).expect("read actual count")[24..28],
        &[2, 0, 0, 0]
    );
    let empty = dir.path().join("empty.ivf");
    fixture(&empty, &[]);
    let error = concatenate(&output, &[a, empty]).expect_err("no actual records");
    assert!(
        format!("{error:#}").contains("has no IVF records"),
        "{error:#}"
    );
}

#[test]
fn ivf_concat_preserves_timestamp_gaps_and_equivalent_fractional_rates() {
    check_concat_boundaries("gaps");
}

#[test]
fn ivf_concat_checks_timestamp_offsets_and_unsigned_boundaries() {
    check_concat_boundaries("boundaries");
}

#[test]
fn ivf_concat_rejects_incompatible_headers_and_recordless_inputs() {
    check_concat_boundaries("headers");
}

#[test]
fn ivf_concat_counts_actual_av1_records_when_header_count_is_unknown() {
    let dir = tempfile::tempdir().expect("unknown-count fixture directory");
    let part = dir.path().join("unknown.ivf");
    native(
        "ffmpeg",
        &[
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=64x64:rate=24000/1001",
            "-frames:v",
            "3",
            "-c:v",
            "libaom-av1",
            "-cpu-used",
            "8",
            "-g",
            "1",
        ],
        Some(&part),
    );
    let mut bytes = fs::read(&part).expect("native AV1 bytes");
    bytes[24..28].fill(0); // A standard unknown-count header, not an empty stream.
    fs::write(&part, &bytes).expect("zero native header count");
    let pixels = decoded(&part);
    assert_eq!(pixels.len(), 3 * 6144);
    let output = dir.path().join("joined.ivf");
    concatenate(&output, &[part.clone(), part.clone()]).expect("concat unknown-count AV1");
    assert_output(&output, 6, &bytes[16..24], &[0, 1, 2, 3, 4, 5]);
    assert_eq!(decoded(&output), [pixels.as_slice(), &pixels].concat());
    let source_payloads = records(&bytes).into_iter().map(|r| r.1).collect::<Vec<_>>();
    assert_eq!(
        records(&fs::read(&output).expect("read joined AV1"))
            .into_iter()
            .map(|r| r.1)
            .collect::<Vec<_>>(),
        [source_payloads.as_slice(), &source_payloads].concat()
    );
    assert_eq!(fs::read(&part).expect("input unchanged"), bytes);
}

#[test]
fn rav1e_unknown_count_concat_preserves_all_records_across_refills() {
    check_native_rav1e_unknown_count_concat();
}

fn check_native_rav1e_unknown_count_concat() {
    let dir = tempfile::tempdir().expect("rav1e fixture directory");
    let source = dir.path().join("source.y4m");
    native(
        "ffmpeg",
        &[
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=64x64:rate=24000/1001",
            "-frames:v",
            "3",
            "-pix_fmt",
            "yuv420p",
        ],
        Some(&source),
    );
    let part = dir.path().join("rav1e.ivf");
    native(
        "rav1e",
        &[
            "--speed",
            "10",
            "--limit",
            "3",
            "--low-latency",
            "--min-keyint",
            "1",
            "--keyint",
            "1",
            "--output",
            part.to_str().expect("rav1e output path"),
        ],
        Some(&source),
    );
    let bytes = fs::read(&part).expect("read rav1e IVF");
    assert_eq!(
        &bytes[24..28],
        &[0, 0, 0, 0],
        "unmodified rav1e unknown count"
    );
    assert_eq!(records(&bytes).len(), 3);
    let payload_sizes = records(&bytes).iter().map(|r| r.1.len()).collect::<Vec<_>>();
    assert_eq!(bytes.len(), 68 + payload_sizes.iter().sum::<usize>());
    assert!(bytes.len() > 4096, "original native stream spans a refill");
    eprintln!(
        "rav1e bytes={}, payload sizes={:?}",
        bytes.len(),
        payload_sizes
    );
    let pixels = decoded(&part);
    assert_eq!(pixels.len(), 3 * 6144);
    let output = dir.path().join("joined.ivf");
    concatenate(&output, &[part.clone(), part.clone()]).expect("concat native rav1e records");
    assert_output(&output, 6, &bytes[16..24], &[0, 1, 2, 3, 4, 5]);
    let payloads = records(&bytes).into_iter().map(|r| r.1).collect::<Vec<_>>();
    assert_eq!(
        records(&fs::read(&output).expect("rav1e output bytes"))
            .into_iter()
            .map(|r| r.1)
            .collect::<Vec<_>>(),
        [payloads.as_slice(), &payloads].concat()
    );
    assert_eq!(decoded(&output), [pixels.as_slice(), &pixels].concat());
    assert_eq!(fs::read(&part).expect("rav1e input unchanged"), bytes);
}

#[test]
fn duplicate_pts_preserve_records_order_and_single_offset() {
    let dir = tempfile::tempdir().expect("duplicate-PTS fixture directory");
    let part = dir.path().join("duplicates.ivf");
    fixture(&part, &[50, 51, 51, 52]);
    let mut bytes = fs::read(&part).expect("duplicate-PTS records");
    // Distinct payloads make a dropped/reordered equal-PTS record observable.
    for (offset, value) in [(44, 11), (57, 22), (70, 33), (83, 44)] {
        bytes[offset] = value;
    }
    bytes[8..12].copy_from_slice(b"VP80");
    fs::write(&part, &bytes).expect("write synthetic VP8 framing");
    let output = dir.path().join("joined.ivf");
    concatenate(&output, &[part.clone(), part.clone()]).expect("equal PTS are valid records");
    let joined = fs::read(&output).expect("read duplicate-PTS concat");
    assert_eq!(&joined[24..28], &[8, 0, 0, 0]);
    assert_eq!(&joined[16..24], &bytes[16..24]);
    assert_eq!(
        records(&joined),
        vec![
            (0, vec![11]),
            (1, vec![22]),
            (1, vec![33]),
            (2, vec![44]),
            (3, vec![11]),
            (4, vec![22]),
            (4, vec![33]),
            (5, vec![44]),
        ]
    );
    assert_eq!(
        fs::read(&part).expect("duplicate-PTS input unchanged"),
        bytes
    );
}

#[test]
fn native_vp8_alt_ref_records_are_not_visible_frame_count() {
    let dir = tempfile::tempdir().expect("VP8 ARF fixture directory");
    let source = dir.path().join("source.y4m");
    native(
        "ffmpeg",
        &[
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=64x64:rate=24000/1001",
            "-frames:v",
            "30",
            "-pix_fmt",
            "yuv420p",
        ],
        Some(&source),
    );
    let part = dir.path().join("vp8.ivf");
    native(
        "vpxenc",
        &[
            "--ivf",
            "--codec=vp8",
            "--passes=2",
            "--auto-alt-ref=1",
            "--lag-in-frames=16",
            "--good",
            "--cpu-used=5",
            "--fps=24000/1001",
            "--timebase=1001/24000",
            "-o",
            part.to_str().expect("VP8 output path"),
        ],
        Some(&source),
    );
    let bytes = fs::read(&part).expect("read native VP8");
    let source_records = records(&bytes);
    let timestamps = source_records.iter().map(|r| r.0).collect::<Vec<_>>();
    eprintln!(
        "VP8 native record count={}, timestamps={timestamps:?}",
        source_records.len()
    );
    assert!(
        source_records.len() > 30,
        "invisible ARF records must actually exist"
    );
    assert!(timestamps.windows(2).any(|p| p[0] == p[1]));
    assert!(timestamps.windows(2).all(|p| p[0] <= p[1]));
    assert_eq!(timestamps.first(), Some(&0));
    assert_eq!(timestamps.last(), Some(&29));
    let pixels = decoded(&part);
    assert_eq!(
        pixels.len(),
        30 * 6144,
        "30 visible frames, not record count"
    );
    let output = dir.path().join("joined.ivf");
    concatenate(&output, &[part.clone(), part.clone()])
        .expect("concat native invisible-ARF duplicates");
    let joined = fs::read(&output).expect("read joined VP8");
    assert_eq!(
        u32::from_le_bytes(joined[24..28].try_into().expect("standard count")) as usize,
        2 * source_records.len()
    );
    assert_eq!(&joined[16..24], &bytes[16..24]);
    let expected = source_records
        .iter()
        .cloned()
        .chain(source_records.iter().map(|(t, data)| (t + 30, data.clone())))
        .collect::<Vec<_>>();
    assert_eq!(
        records(&joined),
        expected,
        "all payloads/order and equal PTS retained"
    );
    assert_eq!(decoded(&output), [pixels.as_slice(), &pixels].concat());
    let probe = native(
        "ffprobe",
        &[
            "-v",
            "error",
            "-count_frames",
            "-show_entries",
            "stream=r_frame_rate,nb_read_frames",
            "-of",
            "default=nw=1",
        ],
        Some(&output),
    );
    let probe = String::from_utf8(probe).expect("VP8 probe output");
    assert!(probe.contains("nb_read_frames=60\n"), "{probe}");
    assert!(probe.contains("r_frame_rate=24000/1001\n"), "{probe}");
    assert_eq!(fs::read(&part).expect("VP8 input unchanged"), bytes);
}

#[test]
fn ivf_concat_reads_records_after_a_payload_ends_at_the_4096_byte_refill_boundary() {
    let dir = tempfile::tempdir().expect("early-EOF fixture directory");
    let part = dir.path().join("boundary.ivf");
    let output = dir.path().join("joined.ivf");
    for hint in [0u32, 3] {
        fixture(&part, &[]);
        let mut bytes = fs::read(&part).expect("standard IVF header");
        bytes[24..28].copy_from_slice(&hint.to_le_bytes());
        // Framing only: 32 + 12 + 4052 puts the first record's end at byte 4096.
        for (timestamp, size) in [(70u64, 4052u32), (71, 1), (72, 1)] {
            bytes.extend_from_slice(&size.to_le_bytes());
            bytes.extend_from_slice(&timestamp.to_le_bytes());
            bytes.extend(std::iter::repeat_n(0xAB, size as usize));
        }
        assert_eq!(records(&bytes).len(), 3);
        assert_eq!(bytes.len(), 4122);
        fs::write(&part, &bytes).expect("write complete three-record stream");
        concatenate(&output, std::slice::from_ref(&part)).expect("all boundary records consumed");
        let joined = fs::read(&output).expect("read complete output");
        assert_eq!(&joined[24..28], &[3, 0, 0, 0]);
        assert_eq!(
            records(&joined),
            vec![(0, vec![0xAB; 4052]), (1, vec![0xAB]), (2, vec![0xAB])]
        );
        assert_eq!(fs::read(&part).expect("input unchanged"), bytes);
    }
}

#[test]
fn ivf_concat_preserves_records_across_refill_offsets_and_untrusted_header_counts() {
    let dir = tempfile::tempdir().expect("boundary sweep directory");
    let part = dir.path().join("boundary.ivf");
    let output = dir.path().join("joined.ivf");
    let mut failures = Vec::new();
    // Opaque framing fixtures are deliberately not codec-decoding certificates.
    for gap in 0..=12 {
        for hint in [0u32, 1, 3, u32::MAX] {
            fixture(&part, &[]);
            let mut bytes = fs::read(&part).expect("standard header");
            bytes[24..28].copy_from_slice(&hint.to_le_bytes());
            for (timestamp, payload) in
                [(70u64, vec![11; 4052 - gap]), (72, vec![22]), (72, vec![33])]
            {
                bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
                bytes.extend_from_slice(&timestamp.to_le_bytes());
                bytes.extend_from_slice(&payload);
            }
            fs::write(&part, &bytes).expect("boundary input");
            let result =
                std::panic::catch_unwind(|| concatenate(&output, std::slice::from_ref(&part)));
            match result {
                Ok(Ok(())) => {
                    let joined = fs::read(&output).expect("boundary output");
                    assert_eq!(&joined[24..28], &[3, 0, 0, 0]);
                    assert_eq!(&joined[16..24], &bytes[16..24]);
                    assert_eq!(
                        records(&joined),
                        vec![(0, vec![11; 4052 - gap]), (2, vec![22]), (2, vec![33])]
                    );
                },
                Ok(Err(error)) => failures.push(format!("gap={gap} hint={hint}: {error:#}")),
                Err(_) => failures.push(format!("gap={gap} hint={hint}: panic")),
            }
            assert_eq!(fs::read(&part).expect("input unchanged"), bytes);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn malformed_framing_returns_contextual_errors_without_panics() {
    let dir = tempfile::tempdir().expect("malformed directory");
    let part = dir.path().join("malformed.ivf");
    fixture(&part, &[50]);
    let valid = fs::read(&part).expect("standard single-record input");
    let mut cases = Vec::new();
    for length in 1..32 {
        cases.push((
            format!("file-header-{length}"),
            valid[..length].to_vec(),
            "header".to_owned(),
            true,
        ));
    }
    for (label, offset, value) in [
        ("magic", 0, 0),
        ("version", 4, 1),
        ("header-size", 6, 31),
        ("codec", 8, 0),
        ("width", 12, 0),
        ("height", 14, 0),
        ("rate", 16, 0),
        ("scale", 20, 0),
    ] {
        let mut bytes = valid.clone();
        let length = if offset < 16 && offset > 0 && offset != 8 {
            2
        } else {
            4
        };
        bytes[offset..offset + length].fill(0);
        bytes[offset] = value;
        cases.push((label.to_owned(), bytes, "IVF".to_owned(), false));
    }
    for prefix in [32, 45] {
        let record = if prefix == 32 { 0 } else { 1 };
        for length in 1..12 {
            let mut bytes = valid[..prefix].to_vec();
            bytes.extend_from_slice(&valid[32..32 + length]);
            cases.push((
                format!("record-{record}-header-{length}"),
                bytes,
                format!("record {record}"),
                true,
            ));
        }
        for size in [1u32, u32::MAX] {
            let mut bytes = valid[..prefix].to_vec();
            bytes.extend_from_slice(&size.to_le_bytes());
            bytes.extend_from_slice(&51u64.to_le_bytes());
            cases.push((
                format!("record-{record}-payload-{size}"),
                bytes,
                format!("record {record}"),
                true,
            ));
        }
        for (size, present) in [(3u32, 1usize), (10_000, 9999)] {
            let mut bytes = valid[..prefix].to_vec();
            bytes.extend_from_slice(&size.to_le_bytes());
            bytes.extend_from_slice(&51u64.to_le_bytes());
            bytes.extend(std::iter::repeat_n(0xAB, present));
            cases.push((
                format!("record-{record}-short-payload-{size}"),
                bytes,
                format!("record {record}"),
                true,
            ));
        }
    }
    let mut failures = Vec::new();
    for (index, (label, bytes, context, io_cause)) in cases.into_iter().enumerate() {
        fs::write(&part, &bytes).expect("malformed input");
        let output = dir.path().join(format!("joined-{index}.ivf"));
        let (tx, rx) = std::sync::mpsc::channel();
        let result = std::panic::catch_unwind(|| {
            SceneConcatenator::ivf(
                &output,
                std::slice::from_ref(&part),
                &tx,
                &Cancellation::default(),
            )
        });
        match result {
            Ok(Err(error)) => {
                let text = format!("{error:#}");
                if !text.contains(&part.display().to_string()) || !text.contains(&context) {
                    failures.push(format!("{label}: missing path/{context}: {text}"));
                }
                if io_cause
                    && error.downcast_ref::<std::io::Error>().map(std::io::Error::kind)
                        != Some(std::io::ErrorKind::UnexpectedEof)
                {
                    failures.push(format!("{label}: lost original UnexpectedEof: {text}"));
                }
            },
            Ok(Ok(())) => failures.push(format!("{label}: false success")),
            Err(_) => failures.push(format!("{label}: panic")),
        }
        assert!(
            !rx.try_iter()
                .any(|s| matches!(s, SequenceStatus::Whole(Status::Completed { .. })))
        );
        if !label.starts_with("record-") {
            assert!(
                !output.exists(),
                "invalid file header must not create output: {label}"
            );
        }
        assert_eq!(fs::read(&part).expect("malformed input unchanged"), bytes);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn ivf_concat_streams_large_payloads_without_truncating_unsigned_rate_fields() {
    let dir = tempfile::tempdir().expect("streaming fixture");
    let part = dir.path().join("large.ivf");
    fixture(&part, &[]);
    let mut bytes = fs::read(&part).expect("standard header");
    bytes[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
    bytes[20..24].copy_from_slice(&(u32::MAX - 1).to_le_bytes());
    let payload = (0..100_000).map(|i| (i % 251) as u8).collect::<Vec<_>>();
    bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&u64::MAX.to_le_bytes());
    bytes.extend_from_slice(&payload);
    fs::write(&part, &bytes).expect("large opaque payload");
    let output = dir.path().join("joined.ivf");
    concatenate(&output, &[part.clone(), part.clone()]).expect("full u32 rational, chunked copy");
    let joined = fs::read(&output).expect("streamed output");
    assert_eq!(&joined[16..24], &bytes[16..24]);
    assert_eq!(&joined[24..28], &[2, 0, 0, 0]);
    assert_eq!(records(&joined), vec![(0, payload.clone()), (1, payload)]);
    assert_eq!(fs::read(part).expect("input unchanged"), bytes);
}
