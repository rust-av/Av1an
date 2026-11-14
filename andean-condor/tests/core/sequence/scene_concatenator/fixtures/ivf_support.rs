//! Independent IVF fixture bytes and native decode oracles, shared with private splitter tests.
use std::{fs, path::Path, process::Command};

pub(super) fn records(bytes: &[u8]) -> Vec<(u64, Vec<u8>)> {
    assert_eq!(&bytes[..4], b"DKIF");
    assert_eq!(
        u16::from_le_bytes(bytes[6..8].try_into().expect("header size")),
        32
    );
    let mut cursor = 32;
    let mut frames = Vec::new();
    while cursor < bytes.len() {
        let size =
            u32::from_le_bytes(bytes[cursor..cursor + 4].try_into().expect("frame size")) as usize;
        let timestamp =
            u64::from_le_bytes(bytes[cursor + 4..cursor + 12].try_into().expect("timestamp"));
        frames.push((timestamp, bytes[cursor + 12..cursor + 12 + size].to_vec()));
        cursor += 12 + size;
    }
    assert_eq!(cursor, bytes.len());
    frames
}

pub(super) fn set_timestamps(bytes: &mut [u8], timestamps: &[u64]) {
    let mut cursor = 32;
    for timestamp in timestamps {
        let size =
            u32::from_le_bytes(bytes[cursor..cursor + 4].try_into().expect("frame size")) as usize;
        bytes[cursor + 4..cursor + 12].copy_from_slice(&timestamp.to_le_bytes());
        cursor += 12 + size;
    }
    assert_eq!(cursor, bytes.len());
}

pub(super) fn fixture(path: &Path, timestamps: &[u64]) {
    let mut bytes = vec![0; 32];
    bytes[..4].copy_from_slice(b"DKIF");
    bytes[6..8].copy_from_slice(&32u16.to_le_bytes());
    bytes[8..12].copy_from_slice(b"AV01");
    bytes[12..16].copy_from_slice(&[64, 0, 64, 0]);
    bytes[16..20].copy_from_slice(&24000u32.to_le_bytes());
    bytes[20..24].copy_from_slice(&1001u32.to_le_bytes());
    bytes[24..28].copy_from_slice(&(timestamps.len() as u32).to_le_bytes());
    for timestamp in timestamps {
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&timestamp.to_le_bytes());
        bytes.push(0xAB);
    }
    fs::write(path, bytes).expect("write synthetic IVF");
}

pub(super) fn native(tool: &str, args: &[&str], input: Option<&Path>) -> Vec<u8> {
    // Native commands stay bounded and are correctness checks, not benchmarks.
    let mut command = Command::new("timeout");
    command.args(["30s", tool]).args(args);
    if let Some(input) = input {
        command.arg(input);
    }
    let output = command.output().expect("required native tool/watchdog");
    assert!(
        output.status.success(),
        "{command:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

pub(super) fn decoded(path: &Path) -> Vec<u8> {
    let args = [
        "-v",
        "error",
        "-i",
        path.to_str().expect("UTF-8 test path"),
        "-f",
        "rawvideo",
        "-pix_fmt",
        "yuv420p",
        "-fps_mode",
        "passthrough",
        "-",
    ];
    native("ffmpeg", &args, None)
}

pub(super) fn assert_output(path: &Path, count: u32, rate: &[u8], timestamps: &[u64]) {
    let bytes = fs::read(path).expect("read native output");
    assert_eq!(
        u32::from_le_bytes(bytes[24..28].try_into().expect("frame count")),
        count
    );
    assert_eq!(&bytes[16..24], rate);
    assert_eq!(
        records(&bytes).iter().map(|r| r.0).collect::<Vec<_>>(),
        timestamps
    );
    let probe = native(
        "ffprobe",
        &[
            "-v",
            "error",
            "-count_frames",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=r_frame_rate,nb_read_frames",
            "-of",
            "default=nw=1",
        ],
        Some(path),
    );
    let probe = String::from_utf8(probe).expect("UTF-8 probe output");
    assert!(
        probe.contains(&format!("nb_read_frames={count}\n")),
        "{probe}"
    );
    assert!(probe.contains("r_frame_rate=24000/1001\n"), "{probe}");
}
