use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use strum::{Display, EnumString, IntoStaticStr};

pub mod bicubic;
pub mod bilinear;
pub mod bob;
pub mod lanczos;
pub mod point;
pub mod spline16;
pub mod spline36;
pub mod spline64;

pub(in crate::vapoursynth::plugins::resize) const NAME: &str = "Resize";
pub(in crate::vapoursynth::plugins::resize) const ID: &str = "com.vapoursynth.resize";
pub(in crate::vapoursynth::plugins::resize) const DOCS: &str =
    "https://www.vapoursynth.com/doc/functions/video/resize.html";

#[derive(
    Debug,
    Copy,
    Clone,
    Display,
    Serialize,
    Deserialize,
    EnumString,
    IntoStaticStr,
    PartialEq,
    Eq,
    JsonSchema,
)]
pub enum Scaler {
    #[strum(serialize = "bicubic")]
    Bicubic,
    #[strum(serialize = "bilinear")]
    Bilinear,
    #[strum(serialize = "bob")]
    Bob,
    #[strum(serialize = "lanczos")]
    Lanczos,
    #[strum(serialize = "point")]
    Point,
    #[strum(serialize = "spline16")]
    Spline16,
    #[strum(serialize = "spline36")]
    Spline36,
    #[strum(serialize = "spline64")]
    Spline64,
}

/// Based on ITU-T H.273 (7/24)
#[derive(Debug, Copy, Clone)]
pub enum MatrixCoefficients {
    RGB = 0,
    BT709 = 1,
    Unspecified = 2,
    Reserved = 3,
    FCC = 4,
    BT470BG = 5,
    ST170M = 6,
    ST240M = 7,
    YCgCo = 8,
    BT2020NCL = 9,
    BT2020CL = 10,
    ST2085 = 11,
    ChromaticityDerivedNCL = 12,
    ChromaticityDerivedCL = 13,
    ICTCP = 14,
    IPTPQC2 = 15,
    YCgCoRe = 16,
    YCgCoRo = 17,
}

#[derive(Debug, Copy, Clone)]
pub enum TransferCharacteristics {
    BT709 = 1,
    Unspecified = 2,
    BT470M = 3,
    BT470BG = 4,
    BT601 = 5,
    ST240M = 6,
    LINEAR = 7,
    LOG100 = 8,
    LOG316 = 9,
    IEC6196624 = 10,
    IEC6196621 = 11,
    BT202010 = 12,
    BT202012 = 13,
    ST2084 = 14,
    ST428 = 15,
    ARIBB67 = 16,
}

#[derive(Debug, Copy, Clone)]
pub enum ColorPrimaries {
    BT709 = 1,
    Unspecified = 2,
    BT470M = 4,
    BT470BG = 5,
    ST170M = 6,
    ST240M = 7,
    Film = 8,
    BT2020 = 9,
    ST428 = 10,
    ST4312 = 11,
    ST4321 = 12,
    JedecP22 = 22,
}

#[derive(Debug, Copy, Clone)]
pub enum Range {
    Limited = 0,
    Full = 1,
}

#[derive(Debug, Copy, Clone)]
#[repr(i32)]
pub enum ChromaLocation {
    Left = 0,
    Center = 1,
    TopLeft = 2,
    Top = 3,
    BottomLeft = 4,
    Bottom = 5,
}

#[derive(Debug, Copy, Clone, Display, EnumString, IntoStaticStr)]
pub enum DitherType {
    #[strum(serialize = "none")]
    None,
    #[strum(serialize = "ordered")]
    Ordered,
    #[strum(serialize = "random")]
    Random,
    #[strum(serialize = "error_diffusion")]
    ErrorDiffusion,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vapoursynth::script_builder::{VapourSynthPluginScript, script::Line};

    #[test]
    fn resize_script_quotes_dither_and_keeps_clip_as_a_variable() {
        let dither_type = Some(DitherType::ErrorDiffusion);
        let scalers: [(&str, &dyn VapourSynthPluginScript); 8] = [
            (
                "Bicubic",
                &bicubic::Bicubic {
                    dither_type,
                    ..Default::default()
                },
            ),
            (
                "Bilinear",
                &bilinear::Bilinear {
                    dither_type,
                    ..Default::default()
                },
            ),
            (
                "Bob",
                &bob::Bob {
                    dither_type,
                    ..Default::default()
                },
            ),
            (
                "Lanczos",
                &lanczos::Lanczos {
                    dither_type,
                    ..Default::default()
                },
            ),
            (
                "Point",
                &point::Point {
                    dither_type,
                    ..Default::default()
                },
            ),
            (
                "Spline16",
                &spline16::Spline16 {
                    dither_type,
                    ..Default::default()
                },
            ),
            (
                "Spline36",
                &spline36::Spline36 {
                    dither_type,
                    ..Default::default()
                },
            ),
            (
                "Spline64",
                &spline64::Spline64 {
                    dither_type,
                    ..Default::default()
                },
            ),
        ];

        for (name, scaler) in scalers {
            let (_, lines) = scaler.generate_script("clip".into()).expect("generate resize script");
            let [Line::Expression(_, call)] = lines.as_slice() else {
                panic!("expected one expression for {name}");
            };
            assert_eq!(
                call,
                &format!(r#"core.resize.{name}(clip = clip, dither_type = "error_diffusion")"#)
            );
        }
    }
}
