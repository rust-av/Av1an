use strum::{Display, EnumString, IntoStaticStr};

pub mod bicubic;
pub mod bilinear;
// pub mod bob;
pub mod lanczos;
pub mod point;
pub mod spline16;
pub mod spline36;
pub mod spline64;

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
