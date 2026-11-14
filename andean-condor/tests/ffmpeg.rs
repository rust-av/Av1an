//! Real FFmpeg conversion, separate from pure pixel-format spelling units.
#![cfg(test)]
use andean_condor::ffmpeg::FFPixelFormat;
use std::{process::Command, str::FromStr};

#[test]
fn ffmpeg_twelve_bit_aliases_decode_exact_planar_samples() {
    let directory = tempfile::tempdir().expect("12-bit output fixture");
    for (spellings, source, plane_samples, byte_count) in [
        (
            ["gbrp12l", "gbrp12le"],
            "nullsrc=size=2x2:rate=1:duration=2,format=gbrp16le,geq=r=0:g=65535:b=65535",
            &[4095_u16, 4095, 0][..],
            48,
        ),
        (
            ["gray12l", "gray12le"],
            "nullsrc=size=2x2:rate=1:duration=2,format=gray16le,geq=lum=65535",
            &[4095_u16][..],
            16,
        ),
    ] {
        // Endpoints avoid quantization dithering; planar RGB is ordered G, B, R.
        let expected_frame: Vec<u8> =
            plane_samples.iter().flat_map(|sample| sample.to_le_bytes().repeat(4)).collect();
        for spelling in spellings {
            let format = FFPixelFormat::from_str(spelling).expect("output format");
            let path = directory.path().join(format!("{spelling}.raw"));
            let output = Command::new("ffmpeg")
                .args(["-nostdin", "-v", "error", "-f", "lavfi", "-i", source])
                // '+' disables auto-conversion; scale explicitly converts the 16-bit source.
                .args(["-frames:v", "2", "-vf", "scale", "-c:v", "rawvideo", "-pix_fmt"])
                .arg(format!("+{}", format.to_pix_fmt_string()))
                .args(["-threads", "1", "-f", "rawvideo"])
                .arg(&path).output().expect("ffmpeg is required for the 12-bit regression");
            assert!(
                output.status.success(),
                "{spelling}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let bytes = std::fs::read(&path).expect("raw output");
            assert_eq!(bytes.len(), byte_count, "two 2x2 frames: {spelling}");
            assert_eq!(
                bytes,
                expected_frame.repeat(2),
                "12-bit LE samples: {spelling}"
            );
        }
    }
}
