use std::str::FromStr;

use anyhow::bail;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use vapoursynth::format::PresetFormat;

/// Pixel formats supported by ffmpeg
#[derive(Eq, PartialEq, Copy, Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub enum FFPixelFormat {
    GBRP,
    GBRP10LE,
    #[serde(alias = "GBRP12L")]
    GBRP12LE,
    GRAY10LE,
    #[serde(alias = "GRAY12L")]
    GRAY12LE,
    GRAY8,
    NV12,
    NV16,
    NV20LE,
    NV21,
    YUV420P,
    YUV420P10LE,
    YUV420P12LE,
    YUV422P,
    YUV422P10LE,
    YUV422P12LE,
    YUV440P,
    YUV440P10LE,
    YUV440P12LE,
    YUV444P,
    YUV444P10LE,
    YUV444P12LE,
    YUVA420P,
    YUVJ420P,
    YUVJ422P,
    YUVJ444P,
}

impl FFPixelFormat {
    /// The string to be used with ffmpeg's `-pix_fmt` argument.
    #[inline]
    pub fn to_pix_fmt_string(&self) -> &'static str {
        match self {
            FFPixelFormat::GBRP => "gbrp",
            FFPixelFormat::GBRP10LE => "gbrp10le",
            FFPixelFormat::GBRP12LE => "gbrp12le",
            FFPixelFormat::GRAY10LE => "gray10le",
            FFPixelFormat::GRAY12LE => "gray12le",
            FFPixelFormat::GRAY8 => "gray",
            FFPixelFormat::NV12 => "nv12",
            FFPixelFormat::NV16 => "nv16",
            FFPixelFormat::NV20LE => "nv20le",
            FFPixelFormat::NV21 => "nv21",
            FFPixelFormat::YUV420P => "yuv420p",
            FFPixelFormat::YUV420P10LE => "yuv420p10le",
            FFPixelFormat::YUV420P12LE => "yuv420p12le",
            FFPixelFormat::YUV422P => "yuv422p",
            FFPixelFormat::YUV422P10LE => "yuv422p10le",
            FFPixelFormat::YUV422P12LE => "yuv422p12le",
            FFPixelFormat::YUV440P => "yuv440p",
            FFPixelFormat::YUV440P10LE => "yuv440p10le",
            FFPixelFormat::YUV440P12LE => "yuv440p12le",
            FFPixelFormat::YUV444P => "yuv444p",
            FFPixelFormat::YUV444P10LE => "yuv444p10le",
            FFPixelFormat::YUV444P12LE => "yuv444p12le",
            FFPixelFormat::YUVA420P => "yuva420p",
            FFPixelFormat::YUVJ420P => "yuvj420p",
            FFPixelFormat::YUVJ422P => "yuvj422p",
            FFPixelFormat::YUVJ444P => "yuvj444p",
        }
    }

    #[inline]
    pub fn to_vapoursynth_format(&self) -> anyhow::Result<PresetFormat> {
        Ok(match self {
            FFPixelFormat::GRAY10LE => PresetFormat::Gray10,
            FFPixelFormat::GRAY12LE => PresetFormat::Gray12,
            FFPixelFormat::GRAY8 => PresetFormat::Gray8,
            FFPixelFormat::YUV420P => PresetFormat::YUV420P8,
            FFPixelFormat::YUV420P10LE => PresetFormat::YUV420P10,
            FFPixelFormat::YUV420P12LE => PresetFormat::YUV420P12,
            FFPixelFormat::YUV422P => PresetFormat::YUV422P8,
            FFPixelFormat::YUV422P10LE => PresetFormat::YUV422P10,
            FFPixelFormat::YUV422P12LE => PresetFormat::YUV422P12,
            FFPixelFormat::YUV440P => PresetFormat::YUV440P8,
            FFPixelFormat::YUV444P => PresetFormat::YUV444P8,
            FFPixelFormat::YUV444P10LE => PresetFormat::YUV444P10,
            FFPixelFormat::YUV444P12LE => PresetFormat::YUV444P12,
            FFPixelFormat::YUVJ420P => PresetFormat::YUV420P8,
            FFPixelFormat::YUVJ422P => PresetFormat::YUV422P8,
            FFPixelFormat::YUVJ444P => PresetFormat::YUV444P8,
            x => bail!(
                "pixel format {} cannot be converted to Vapoursynth format",
                x.to_pix_fmt_string()
            ),
        })
    }
}

impl FromStr for FFPixelFormat {
    type Err = anyhow::Error;

    #[inline]
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "gbrp" => FFPixelFormat::GBRP,
            "gbrp10le" => FFPixelFormat::GBRP10LE,
            "gbrp12l" | "gbrp12le" => FFPixelFormat::GBRP12LE,
            "gray10le" => FFPixelFormat::GRAY10LE,
            "gray12l" | "gray12le" => FFPixelFormat::GRAY12LE,
            "gray" => FFPixelFormat::GRAY8,
            "nv12" => FFPixelFormat::NV12,
            "nv16" => FFPixelFormat::NV16,
            "nv20le" => FFPixelFormat::NV20LE,
            "nv21" => FFPixelFormat::NV21,
            "yuv420p" => FFPixelFormat::YUV420P,
            "yuv420p10le" => FFPixelFormat::YUV420P10LE,
            "yuv420p12le" => FFPixelFormat::YUV420P12LE,
            "yuv422p" => FFPixelFormat::YUV422P,
            "yuv422p10le" => FFPixelFormat::YUV422P10LE,
            "yuv422p12le" => FFPixelFormat::YUV422P12LE,
            "yuv440p" => FFPixelFormat::YUV440P,
            "yuv440p10le" => FFPixelFormat::YUV440P10LE,
            "yuv440p12le" => FFPixelFormat::YUV440P12LE,
            "yuv444p" => FFPixelFormat::YUV444P,
            "yuv444p10le" => FFPixelFormat::YUV444P10LE,
            "yuv444p12le" => FFPixelFormat::YUV444P12LE,
            "yuva420p" => FFPixelFormat::YUVA420P,
            "yuvj420p" => FFPixelFormat::YUVJ420P,
            "yuvj422p" => FFPixelFormat::YUVJ422P,
            "yuvj444p" => FFPixelFormat::YUVJ444P,
            s => bail!("Unsupported pixel format string: {s}"),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;
    use crate::core::input::pixel_format::PixelFormat;

    #[test]
    fn twelve_bit_spellings_parse_and_emit_canonical_formats() {
        for (input, canonical, expected) in [
            ("gbrp12l", "gbrp12le", FFPixelFormat::GBRP12LE),
            ("gbrp12le", "gbrp12le", FFPixelFormat::GBRP12LE),
            ("gray12l", "gray12le", FFPixelFormat::GRAY12LE),
            ("gray12le", "gray12le", FFPixelFormat::GRAY12LE),
        ] {
            let format = FFPixelFormat::from_str(input).expect("12-bit format");
            assert_eq!(format.to_pix_fmt_string(), canonical, "{input}");
            assert_eq!(format, expected, "{input}");
            assert_eq!(
                FFPixelFormat::from_str(canonical).expect("canonical format"),
                format
            );
            assert_eq!(
                PixelFormat::FFmpeg { format }.as_bit_depth().expect("depth"),
                12
            );
        }
    }

    #[test]
    fn twelve_bit_serde_reads_legacy_names_and_writes_canonical_names() {
        for (input, canonical, expected) in [
            ("GBRP12L", "GBRP12LE", FFPixelFormat::GBRP12LE),
            ("GBRP12LE", "GBRP12LE", FFPixelFormat::GBRP12LE),
            ("GRAY12L", "GRAY12LE", FFPixelFormat::GRAY12LE),
            ("GRAY12LE", "GRAY12LE", FFPixelFormat::GRAY12LE),
        ] {
            let format: FFPixelFormat =
                serde_json::from_str(&format!("\"{input}\"")).expect("saved enum name");
            assert_eq!(format, expected, "{input}");
            assert_eq!(
                serde_json::to_string(&format).expect("serialize"),
                format!("\"{canonical}\"")
            );
        }
    }

    #[test]
    fn ffmpeg_converts_legacy_and_canonical_12bit_outputs() {
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
                    .arg(&path)
                    .output()
                    .expect("ffmpeg is required for the 12-bit regression");
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
}
