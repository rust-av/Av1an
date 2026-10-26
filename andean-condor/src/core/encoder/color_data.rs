//! Builds each encoder's native colour and HDR metadata parameters from a
//! clip's [`ClipInfo`].
//!
//! Encoders disagree on flag names, value spellings and which pieces of
//! metadata they can express at all, so every conversion lives here. Values the
//! source left unspecified are omitted rather than written out as
//! `unspecified`/`unknown`: nothing is signalled that the source did not
//! signal.

use std::collections::HashMap;

use av_decoders::{
    ChromaLocation,
    ColorPrimaries,
    ColorSpace,
    ContentLightLevel,
    MasteringDisplayMetadata,
    TransferCharacteristic,
};

use crate::{
    core::input::{clip_info::ClipInfo, color_range::ColorRange},
    models::encoder::{EncoderBase, cli_parameter::CLIParameter},
};

/// The colour and HDR parameters `base` understands for this clip.
///
/// The result is merged between the pass parameters and the user's options, so
/// a user definition always wins.
#[inline]
pub fn color_parameters(base: EncoderBase, clip_info: &ClipInfo) -> HashMap<String, CLIParameter> {
    match base {
        EncoderBase::AOM => aom(clip_info),
        EncoderBase::AVM => avm(clip_info),
        EncoderBase::RAV1E => rav1e(clip_info),
        EncoderBase::SVTAV1 => svt_av1(clip_info),
        EncoderBase::X264 => x264(clip_info),
        EncoderBase::X265 => x265(clip_info),
        EncoderBase::VVenC => vvenc(clip_info),
        EncoderBase::FFmpeg => ffmpeg(clip_info),
        EncoderBase::VPX => vpx(clip_info),
    }
}

/// `G(x,y)B(x,y)R(x,y)WP(x,y)L(max,min)`, the format AOM, rav1e, SVT-AV1, x264
/// and x265 all accept.
///
/// `av_decoders` orders the primaries green, blue, red; that is also the order
/// these encoders parse.
#[inline]
fn mastering_display_string(mastering_display: &MasteringDisplayMetadata) -> Option<String> {
    let (primaries, white_point) = mastering_display.primaries?;
    let [(g_x, g_y), (b_x, b_y), (r_x, r_y)] = primaries;
    let (wp_x, wp_y) = white_point;
    let (min_luminance, max_luminance) = mastering_display.luminance?;

    Some(format!(
        "G({g_x},{g_y})B({b_x},{b_y})R({r_x},{r_y})WP({wp_x},{wp_y})L({max_luminance},\
         {min_luminance})"
    ))
}

/// The same mastering display volume in the fixed-point units x264 and VVenC
/// take: chromaticity coordinates in 1/50000ths and luminance in 1/10000ths.
///
/// x264 parses the coordinates as `%d` and the luminance as a 64-bit integer,
/// and both encoders document the 1/50000 and 1/10000 units.
#[inline]
fn mastering_display_fixed_point(
    mastering_display: &MasteringDisplayMetadata,
) -> Option<[u64; 10]> {
    let (primaries, white_point) = mastering_display.primaries?;
    let [(g_x, g_y), (b_x, b_y), (r_x, r_y)] = primaries;
    let (wp_x, wp_y) = white_point;
    let (min_luminance, max_luminance) = mastering_display.luminance?;

    let coordinate = |value: f64| (value.mul_add(50_000.0, 0.0)).round().max(0.0) as u64;
    let luminance = |value: f64| (value.mul_add(10_000.0, 0.0)).round().max(0.0) as u64;

    Some([
        coordinate(g_x),
        coordinate(g_y),
        coordinate(b_x),
        coordinate(b_y),
        coordinate(r_x),
        coordinate(r_y),
        coordinate(wp_x),
        coordinate(wp_y),
        luminance(max_luminance),
        luminance(min_luminance),
    ])
}

#[inline]
fn content_light_string(content_light_level: ContentLightLevel) -> String {
    format!(
        "{},{}",
        content_light_level.max_cll, content_light_level.max_fall
    )
}

/// Inserts `key` with `value` when the mapping produced one.
#[inline]
fn insert(
    parameters: &mut HashMap<String, CLIParameter>,
    key: &str,
    value: Option<String>,
    prefix: &str,
    delimiter: &str,
) {
    if let Some(value) = value {
        parameters.insert(
            key.to_owned(),
            CLIParameter::new_string(prefix, delimiter, &value),
        );
    }
}

/// Maps a value through `table`, returning `None` for values the encoder has no
/// spelling for.
#[inline]
fn map<T: Copy + PartialEq>(table: &[(T, &'static str)], value: Option<T>) -> Option<String> {
    let value = value?;
    table
        .iter()
        .find(|(candidate, _)| *candidate == value)
        .map(|(_, name)| (*name).to_owned())
}

// ---------------------------------------------------------------------------
// AOM and AVM
// ---------------------------------------------------------------------------

const AOM_PRIMARIES: &[(ColorPrimaries, &str)] = &[
    (ColorPrimaries::BT709, "bt709"),
    (ColorPrimaries::BT470M, "bt470m"),
    (ColorPrimaries::BT470BG, "bt470bg"),
    (ColorPrimaries::SMPTE170M, "bt601"),
    (ColorPrimaries::SMPTE240M, "smpte240"),
    (ColorPrimaries::Film, "film"),
    (ColorPrimaries::BT2020, "bt2020"),
    (ColorPrimaries::SMPTE428, "xyz"),
    (ColorPrimaries::SMPTE431, "smpte431"),
    (ColorPrimaries::SMPTE432, "smpte432"),
    (ColorPrimaries::EBU3213, "ebu3213"),
];

const AOM_TRANSFER: &[(TransferCharacteristic, &str)] = &[
    (TransferCharacteristic::BT709, "bt709"),
    (TransferCharacteristic::GAMMA22, "bt470m"),
    (TransferCharacteristic::GAMMA28, "bt470bg"),
    (TransferCharacteristic::SMPTE170M, "bt601"),
    (TransferCharacteristic::SMPTE240M, "smpte240"),
    (TransferCharacteristic::Linear, "lin"),
    (TransferCharacteristic::Log, "log100"),
    (TransferCharacteristic::LogSqrt, "log100sq10"),
    (TransferCharacteristic::IEC61966_2_4, "iec61966"),
    (TransferCharacteristic::BT1361_ECG, "bt1361"),
    (TransferCharacteristic::IEC61966_2_1, "srgb"),
    (TransferCharacteristic::BT2020_10, "bt2020-10bit"),
    (TransferCharacteristic::BT2020_12, "bt2020-12bit"),
    (TransferCharacteristic::SMPTE2084, "smpte2084"),
    (TransferCharacteristic::SMPTE428, "smpte428"),
    (TransferCharacteristic::ARIB_STD_B67, "hlg"),
];

const AOM_MATRIX: &[(ColorSpace, &str)] = &[
    (ColorSpace::RGB, "identity"),
    (ColorSpace::BT709, "bt709"),
    (ColorSpace::FCC, "fcc73"),
    (ColorSpace::BT470BG, "bt470bg"),
    (ColorSpace::SMPTE170M, "bt601"),
    (ColorSpace::SMPTE240M, "smpte240"),
    (ColorSpace::YCGCO, "ycgco"),
    (ColorSpace::BT2020NCL, "bt2020ncl"),
    (ColorSpace::BT2020CL, "bt2020cl"),
    (ColorSpace::SMPTE2085, "smpte2085"),
    (ColorSpace::ChromaDerivedNCL, "chromncl"),
    (ColorSpace::ChromaDerivedCL, "chromcl"),
    (ColorSpace::ICTCP, "ictcp"),
];

/// AOM only signals chroma sample position co-located or halfway between luma
/// rows, so the other locations have no spelling.
const AOM_CHROMA_LOCATION: &[(ChromaLocation, &str)] =
    &[(ChromaLocation::Left, "vertical"), (ChromaLocation::TopLeft, "colocated")];

fn aom(clip_info: &ClipInfo) -> HashMap<String, CLIParameter> {
    let mut parameters = HashMap::new();
    insert(
        &mut parameters,
        "color-primaries",
        map(AOM_PRIMARIES, clip_info.color_primaries),
        "--",
        "=",
    );
    insert(
        &mut parameters,
        "transfer-characteristics",
        map(AOM_TRANSFER, clip_info.color_transfer_characteristic),
        "--",
        "=",
    );
    insert(
        &mut parameters,
        "matrix-coefficients",
        map(AOM_MATRIX, clip_info.color_space),
        "--",
        "=",
    );
    insert(
        &mut parameters,
        "chroma-sample-position",
        map(AOM_CHROMA_LOCATION, clip_info.chroma_location),
        "--",
        "=",
    );

    // aomenc has no range, mastering display or content light level flags.
    parameters
}

/// AVM carries the full CICP set of chroma sample positions plus two extra
/// matrix coefficients.
const AVM_CHROMA_LOCATION: &[(ChromaLocation, &str)] = &[
    (ChromaLocation::Left, "left"),
    (ChromaLocation::Center, "center"),
    (ChromaLocation::TopLeft, "topleft"),
    (ChromaLocation::Top, "top"),
    (ChromaLocation::BottomLeft, "bottomleft"),
    (ChromaLocation::Bottom, "bottom"),
];

const AVM_MATRIX: &[(ColorSpace, &str)] = &[
    (ColorSpace::RGB, "identity"),
    (ColorSpace::BT709, "bt709"),
    (ColorSpace::FCC, "fcc73"),
    (ColorSpace::BT470BG, "bt470bg"),
    (ColorSpace::SMPTE170M, "bt601"),
    (ColorSpace::SMPTE240M, "smpte240"),
    (ColorSpace::YCGCO, "ycgco"),
    (ColorSpace::BT2020NCL, "bt2020ncl"),
    (ColorSpace::BT2020CL, "bt2020cl"),
    (ColorSpace::SMPTE2085, "smpte2085"),
    (ColorSpace::ChromaDerivedNCL, "chromncl"),
    (ColorSpace::ChromaDerivedCL, "chromcl"),
    (ColorSpace::ICTCP, "ictcp"),
    (ColorSpace::IPTC2, "ipt-c2"),
    (ColorSpace::YCGCORE, "ycgco-re"),
    (ColorSpace::YCGCORO, "ycgco-ro"),
];

fn avm(clip_info: &ClipInfo) -> HashMap<String, CLIParameter> {
    let mut parameters = HashMap::new();
    insert(
        &mut parameters,
        "color-primaries",
        map(AOM_PRIMARIES, clip_info.color_primaries),
        "--",
        "=",
    );
    insert(
        &mut parameters,
        "transfer-characteristics",
        map(AOM_TRANSFER, clip_info.color_transfer_characteristic),
        "--",
        "=",
    );
    insert(
        &mut parameters,
        "matrix-coefficients",
        map(AVM_MATRIX, clip_info.color_space),
        "--",
        "=",
    );
    insert(
        &mut parameters,
        "chroma-sample-position",
        map(AVM_CHROMA_LOCATION, clip_info.chroma_location),
        "--",
        "=",
    );

    // avmenc has no range, mastering display or content light level flags.
    parameters
}

// ---------------------------------------------------------------------------
// rav1e
// ---------------------------------------------------------------------------

const RAV1E_RANGE: &[(ColorRange, &str)] =
    &[(ColorRange::Limited, "Limited"), (ColorRange::Full, "Full")];

const RAV1E_PRIMARIES: &[(ColorPrimaries, &str)] = &[
    (ColorPrimaries::BT709, "BT709"),
    (ColorPrimaries::BT470M, "BT470M"),
    (ColorPrimaries::BT470BG, "BT470BG"),
    (ColorPrimaries::SMPTE170M, "BT601"),
    (ColorPrimaries::SMPTE240M, "SMPTE240"),
    (ColorPrimaries::Film, "GenericFilm"),
    (ColorPrimaries::BT2020, "BT2020"),
    (ColorPrimaries::SMPTE428, "XYZ"),
    (ColorPrimaries::SMPTE431, "SMPTE431"),
    (ColorPrimaries::SMPTE432, "SMPTE432"),
    (ColorPrimaries::EBU3213, "EBU3213"),
];

const RAV1E_TRANSFER: &[(TransferCharacteristic, &str)] = &[
    (TransferCharacteristic::BT709, "BT709"),
    (TransferCharacteristic::GAMMA22, "BT470M"),
    (TransferCharacteristic::GAMMA28, "BT470BG"),
    (TransferCharacteristic::SMPTE170M, "BT601"),
    (TransferCharacteristic::SMPTE240M, "SMPTE240"),
    (TransferCharacteristic::Linear, "Linear"),
    (TransferCharacteristic::Log, "Log100"),
    (TransferCharacteristic::LogSqrt, "Log100Sqrt10"),
    (TransferCharacteristic::IEC61966_2_4, "IEC61966"),
    (TransferCharacteristic::BT1361_ECG, "BT1361"),
    (TransferCharacteristic::IEC61966_2_1, "SRGB"),
    (TransferCharacteristic::BT2020_10, "BT2020_10Bit"),
    (TransferCharacteristic::BT2020_12, "BT2020_12Bit"),
    (TransferCharacteristic::SMPTE2084, "SMPTE2084"),
    (TransferCharacteristic::SMPTE428, "SMPTE428"),
    (TransferCharacteristic::ARIB_STD_B67, "HLG"),
];

const RAV1E_MATRIX: &[(ColorSpace, &str)] = &[
    (ColorSpace::RGB, "Identity"),
    (ColorSpace::BT709, "BT709"),
    (ColorSpace::FCC, "FCC"),
    (ColorSpace::BT470BG, "BT470BG"),
    (ColorSpace::SMPTE170M, "BT601"),
    (ColorSpace::SMPTE240M, "SMPTE240"),
    (ColorSpace::YCGCO, "YCgCo"),
    (ColorSpace::BT2020NCL, "BT2020NCL"),
    (ColorSpace::BT2020CL, "BT2020CL"),
    (ColorSpace::SMPTE2085, "SMPTE2085"),
    (ColorSpace::ChromaDerivedNCL, "ChromatNCL"),
    (ColorSpace::ChromaDerivedCL, "ChromatCL"),
    (ColorSpace::ICTCP, "ICtCp"),
];

fn rav1e(clip_info: &ClipInfo) -> HashMap<String, CLIParameter> {
    let mut parameters = HashMap::new();
    insert(
        &mut parameters,
        "range",
        map(RAV1E_RANGE, clip_info.color_range),
        "--",
        " ",
    );
    insert(
        &mut parameters,
        "primaries",
        map(RAV1E_PRIMARIES, clip_info.color_primaries),
        "--",
        " ",
    );
    insert(
        &mut parameters,
        "transfer",
        map(RAV1E_TRANSFER, clip_info.color_transfer_characteristic),
        "--",
        " ",
    );
    insert(
        &mut parameters,
        "matrix",
        map(RAV1E_MATRIX, clip_info.color_space),
        "--",
        " ",
    );
    insert(
        &mut parameters,
        "mastering-display",
        clip_info.mastering_display.as_ref().and_then(mastering_display_string),
        "--",
        " ",
    );
    insert(
        &mut parameters,
        "content-light",
        clip_info.content_light_level.map(content_light_string),
        "--",
        " ",
    );

    // rav1e has no chroma sample location flag.
    parameters
}

// ---------------------------------------------------------------------------
// SVT-AV1
// ---------------------------------------------------------------------------

const SVT_RANGE: &[(ColorRange, &str)] =
    &[(ColorRange::Limited, "studio"), (ColorRange::Full, "full")];

const SVT_PRIMARIES: &[(ColorPrimaries, &str)] = &[
    (ColorPrimaries::BT709, "bt709"),
    (ColorPrimaries::BT470M, "bt470m"),
    (ColorPrimaries::BT470BG, "bt470bg"),
    (ColorPrimaries::SMPTE170M, "bt601"),
    (ColorPrimaries::SMPTE240M, "smpte240"),
    (ColorPrimaries::Film, "film"),
    (ColorPrimaries::BT2020, "bt2020"),
    (ColorPrimaries::SMPTE428, "xyz"),
    (ColorPrimaries::SMPTE431, "smpte431"),
    (ColorPrimaries::SMPTE432, "smpte432"),
    (ColorPrimaries::EBU3213, "ebu3213"),
];

const SVT_TRANSFER: &[(TransferCharacteristic, &str)] = &[
    (TransferCharacteristic::BT709, "bt709"),
    (TransferCharacteristic::GAMMA22, "bt470m"),
    (TransferCharacteristic::GAMMA28, "bt470bg"),
    (TransferCharacteristic::SMPTE170M, "bt601"),
    (TransferCharacteristic::SMPTE240M, "smpte240"),
    (TransferCharacteristic::Linear, "linear"),
    (TransferCharacteristic::Log, "log100"),
    (TransferCharacteristic::LogSqrt, "log100-sqrt10"),
    (TransferCharacteristic::IEC61966_2_4, "iec61966"),
    (TransferCharacteristic::BT1361_ECG, "bt1361"),
    (TransferCharacteristic::IEC61966_2_1, "srgb"),
    (TransferCharacteristic::BT2020_10, "bt2020-10"),
    (TransferCharacteristic::BT2020_12, "bt2020-12"),
    (TransferCharacteristic::SMPTE2084, "smpte2084"),
    (TransferCharacteristic::SMPTE428, "smpte428"),
    (TransferCharacteristic::ARIB_STD_B67, "hlg"),
];

const SVT_MATRIX: &[(ColorSpace, &str)] = &[
    (ColorSpace::RGB, "identity"),
    (ColorSpace::BT709, "bt709"),
    (ColorSpace::FCC, "fcc"),
    (ColorSpace::BT470BG, "bt470bg"),
    (ColorSpace::SMPTE170M, "bt601"),
    (ColorSpace::SMPTE240M, "smpte240"),
    (ColorSpace::YCGCO, "ycgco"),
    (ColorSpace::BT2020NCL, "bt2020-ncl"),
    (ColorSpace::BT2020CL, "bt2020-cl"),
    (ColorSpace::SMPTE2085, "smpte2085"),
    (ColorSpace::ChromaDerivedNCL, "chroma-ncl"),
    (ColorSpace::ChromaDerivedCL, "chroma-cl"),
    (ColorSpace::ICTCP, "ictcp"),
];

const SVT_CHROMA_LOCATION: &[(ChromaLocation, &str)] =
    &[(ChromaLocation::Left, "vertical"), (ChromaLocation::TopLeft, "colocated")];

fn svt_av1(clip_info: &ClipInfo) -> HashMap<String, CLIParameter> {
    let mut parameters = HashMap::new();
    insert(
        &mut parameters,
        "color-primaries",
        map(SVT_PRIMARIES, clip_info.color_primaries),
        "--",
        " ",
    );
    insert(
        &mut parameters,
        "transfer-characteristics",
        map(SVT_TRANSFER, clip_info.color_transfer_characteristic),
        "--",
        " ",
    );
    insert(
        &mut parameters,
        "matrix-coefficients",
        map(SVT_MATRIX, clip_info.color_space),
        "--",
        " ",
    );
    insert(
        &mut parameters,
        "color-range",
        map(SVT_RANGE, clip_info.color_range),
        "--",
        " ",
    );
    insert(
        &mut parameters,
        "chroma-sample-position",
        map(SVT_CHROMA_LOCATION, clip_info.chroma_location),
        "--",
        " ",
    );
    insert(
        &mut parameters,
        "mastering-display",
        clip_info.mastering_display.as_ref().and_then(mastering_display_string),
        "--",
        " ",
    );
    insert(
        &mut parameters,
        "content-light",
        clip_info.content_light_level.map(content_light_string),
        "--",
        " ",
    );

    parameters
}

// ---------------------------------------------------------------------------
// x264 and x265
// ---------------------------------------------------------------------------

const X264_RANGE: &[(ColorRange, &str)] = &[(ColorRange::Limited, "tv"), (ColorRange::Full, "pc")];

const X264_PRIMARIES: &[(ColorPrimaries, &str)] = &[
    (ColorPrimaries::BT709, "bt709"),
    (ColorPrimaries::BT470M, "bt470m"),
    (ColorPrimaries::BT470BG, "bt470bg"),
    (ColorPrimaries::SMPTE170M, "smpte170m"),
    (ColorPrimaries::SMPTE240M, "smpte240m"),
    (ColorPrimaries::Film, "film"),
    (ColorPrimaries::BT2020, "bt2020"),
    (ColorPrimaries::SMPTE428, "smpte428"),
    (ColorPrimaries::SMPTE431, "smpte431"),
    (ColorPrimaries::SMPTE432, "smpte432"),
];

const X264_TRANSFER: &[(TransferCharacteristic, &str)] = &[
    (TransferCharacteristic::BT709, "bt709"),
    (TransferCharacteristic::GAMMA22, "bt470m"),
    (TransferCharacteristic::GAMMA28, "bt470bg"),
    (TransferCharacteristic::SMPTE170M, "smpte170m"),
    (TransferCharacteristic::SMPTE240M, "smpte240m"),
    (TransferCharacteristic::Linear, "linear"),
    (TransferCharacteristic::Log, "log100"),
    (TransferCharacteristic::LogSqrt, "log316"),
    (TransferCharacteristic::IEC61966_2_4, "iec61966-2-4"),
    (TransferCharacteristic::BT1361_ECG, "bt1361e"),
    (TransferCharacteristic::IEC61966_2_1, "iec61966-2-1"),
    (TransferCharacteristic::BT2020_10, "bt2020-10"),
    (TransferCharacteristic::BT2020_12, "bt2020-12"),
    (TransferCharacteristic::SMPTE2084, "smpte2084"),
    (TransferCharacteristic::SMPTE428, "smpte428"),
    (TransferCharacteristic::ARIB_STD_B67, "arib-std-b67"),
];

const X264_MATRIX: &[(ColorSpace, &str)] = &[
    (ColorSpace::RGB, "GBR"),
    (ColorSpace::BT709, "bt709"),
    (ColorSpace::FCC, "fcc"),
    (ColorSpace::BT470BG, "bt470bg"),
    (ColorSpace::SMPTE170M, "smpte170m"),
    (ColorSpace::SMPTE240M, "smpte240m"),
    (ColorSpace::YCGCO, "YCgCo"),
    (ColorSpace::BT2020NCL, "bt2020nc"),
    (ColorSpace::BT2020CL, "bt2020c"),
    (ColorSpace::SMPTE2085, "smpte2085"),
    (ColorSpace::ChromaDerivedNCL, "chroma-derived-nc"),
    (ColorSpace::ChromaDerivedCL, "chroma-derived-c"),
    (ColorSpace::ICTCP, "ICtCp"),
];

const X265_RANGE: &[(ColorRange, &str)] =
    &[(ColorRange::Limited, "limited"), (ColorRange::Full, "full")];

const X265_MATRIX: &[(ColorSpace, &str)] = &[
    (ColorSpace::RGB, "gbr"),
    (ColorSpace::BT709, "bt709"),
    (ColorSpace::FCC, "fcc"),
    (ColorSpace::BT470BG, "bt470bg"),
    (ColorSpace::SMPTE170M, "smpte170m"),
    (ColorSpace::SMPTE240M, "smpte240m"),
    (ColorSpace::YCGCO, "ycgco"),
    (ColorSpace::BT2020NCL, "bt2020nc"),
    (ColorSpace::BT2020CL, "bt2020c"),
    (ColorSpace::SMPTE2085, "smpte2085"),
    (ColorSpace::ChromaDerivedNCL, "chroma-derived-nc"),
    (ColorSpace::ChromaDerivedCL, "chroma-derived-c"),
    (ColorSpace::ICTCP, "ictcp"),
];

/// Both x264 and x265 number `chromaloc` from 0 with "left" as zero, while
/// `av-decoders` starts at 0 for "unspecified", so every value shifts by one.
#[inline]
fn chroma_location_index(chroma_location: Option<ChromaLocation>) -> Option<f64> {
    match chroma_location? {
        ChromaLocation::Left => Some(0.0),
        ChromaLocation::Center => Some(1.0),
        ChromaLocation::TopLeft => Some(2.0),
        ChromaLocation::Top => Some(3.0),
        ChromaLocation::BottomLeft => Some(4.0),
        ChromaLocation::Bottom => Some(5.0),
        ChromaLocation::Unspecified => None,
    }
}

fn x264(clip_info: &ClipInfo) -> HashMap<String, CLIParameter> {
    let mut parameters = HashMap::new();
    insert(
        &mut parameters,
        "range",
        map(X264_RANGE, clip_info.color_range),
        "--",
        " ",
    );
    insert(
        &mut parameters,
        "colorprim",
        map(X264_PRIMARIES, clip_info.color_primaries),
        "--",
        " ",
    );
    insert(
        &mut parameters,
        "transfer",
        map(X264_TRANSFER, clip_info.color_transfer_characteristic),
        "--",
        " ",
    );
    insert(
        &mut parameters,
        "colormatrix",
        map(X264_MATRIX, clip_info.color_space),
        "--",
        " ",
    );
    if let Some(index) = chroma_location_index(clip_info.chroma_location) {
        parameters.insert(
            "chromaloc".to_owned(),
            CLIParameter::new_number("--", " ", index),
        );
    }
    // x264 parses the coordinates and luminance as integers in 1/50000 and
    // 1/10000 units.
    if let Some(values) =
        clip_info.mastering_display.as_ref().and_then(mastering_display_fixed_point)
    {
        parameters.insert(
            "mastering-display".to_owned(),
            CLIParameter::new_string(
                "--",
                " ",
                &format!(
                    "G({},{})B({},{})R({},{})WP({},{})L({},{})",
                    values[0],
                    values[1],
                    values[2],
                    values[3],
                    values[4],
                    values[5],
                    values[6],
                    values[7],
                    values[8],
                    values[9]
                ),
            ),
        );
    }
    insert(
        &mut parameters,
        "cll",
        clip_info.content_light_level.map(content_light_string),
        "--",
        " ",
    );

    parameters
}

fn x265(clip_info: &ClipInfo) -> HashMap<String, CLIParameter> {
    let mut parameters = HashMap::new();
    insert(
        &mut parameters,
        "range",
        map(X265_RANGE, clip_info.color_range),
        "--",
        " ",
    );
    insert(
        &mut parameters,
        "colorprim",
        map(X264_PRIMARIES, clip_info.color_primaries),
        "--",
        " ",
    );
    insert(
        &mut parameters,
        "transfer",
        map(X264_TRANSFER, clip_info.color_transfer_characteristic),
        "--",
        " ",
    );
    insert(
        &mut parameters,
        "colormatrix",
        map(X265_MATRIX, clip_info.color_space),
        "--",
        " ",
    );
    if let Some(index) = chroma_location_index(clip_info.chroma_location) {
        parameters.insert(
            "chromaloc".to_owned(),
            CLIParameter::new_number("--", " ", index),
        );
    }
    // x265 parses the volume with `%hu`/`%u`, so it takes the same fixed-point
    // units as x264.
    if let Some(values) =
        clip_info.mastering_display.as_ref().and_then(mastering_display_fixed_point)
    {
        parameters.insert(
            "master-display".to_owned(),
            CLIParameter::new_string(
                "--",
                " ",
                &format!(
                    "G({},{})B({},{})R({},{})WP({},{})L({},{})",
                    values[0],
                    values[1],
                    values[2],
                    values[3],
                    values[4],
                    values[5],
                    values[6],
                    values[7],
                    values[8],
                    values[9]
                ),
            ),
        );
    }
    insert(
        &mut parameters,
        "max-cll",
        clip_info.content_light_level.map(content_light_string),
        "--",
        " ",
    );

    parameters
}

// ---------------------------------------------------------------------------
// VVenC
// ---------------------------------------------------------------------------

const VVENC_RANGE: &[(ColorRange, &str)] =
    &[(ColorRange::Limited, "limited"), (ColorRange::Full, "full")];

const VVENC_PRIMARIES: &[(ColorPrimaries, &str)] = &[
    (ColorPrimaries::BT709, "bt709"),
    (ColorPrimaries::BT470M, "bt470m"),
    (ColorPrimaries::BT470BG, "bt470bg"),
    (ColorPrimaries::SMPTE170M, "smpte170m"),
    (ColorPrimaries::SMPTE240M, "smpte240m"),
    (ColorPrimaries::Film, "film"),
    (ColorPrimaries::BT2020, "bt2020"),
    (ColorPrimaries::SMPTE428, "smpte428"),
    (ColorPrimaries::SMPTE431, "smpte431"),
    (ColorPrimaries::SMPTE432, "smpte432"),
];

const VVENC_TRANSFER: &[(TransferCharacteristic, &str)] = &[
    (TransferCharacteristic::BT709, "bt709"),
    (TransferCharacteristic::GAMMA22, "bt470m"),
    (TransferCharacteristic::GAMMA28, "bt470bg"),
    (TransferCharacteristic::SMPTE170M, "smpte170m"),
    (TransferCharacteristic::SMPTE240M, "smpte240m"),
    (TransferCharacteristic::Linear, "linear"),
    (TransferCharacteristic::Log, "log100"),
    (TransferCharacteristic::LogSqrt, "log316"),
    (TransferCharacteristic::IEC61966_2_4, "iec61966"),
    (TransferCharacteristic::BT1361_ECG, "bt1361e"),
    (TransferCharacteristic::IEC61966_2_1, "iec61966-2-1"),
    (TransferCharacteristic::BT2020_10, "bt2020-10"),
    (TransferCharacteristic::BT2020_12, "bt2020-12"),
    (TransferCharacteristic::SMPTE2084, "smpte2084"),
    (TransferCharacteristic::SMPTE428, "smpte428"),
    (TransferCharacteristic::ARIB_STD_B67, "arib-std-b67"),
];

const VVENC_MATRIX: &[(ColorSpace, &str)] = &[
    (ColorSpace::RGB, "gbr"),
    (ColorSpace::BT709, "bt709"),
    (ColorSpace::FCC, "fcc"),
    (ColorSpace::BT470BG, "bt470bg"),
    (ColorSpace::SMPTE170M, "smpte170m"),
    (ColorSpace::SMPTE240M, "smpte240m"),
    (ColorSpace::YCGCO, "ycgco"),
    (ColorSpace::BT2020NCL, "bt2020nc"),
    (ColorSpace::BT2020CL, "bt2020c"),
    (ColorSpace::SMPTE2085, "smpte2085"),
    (ColorSpace::ChromaDerivedNCL, "chroma-derived-nc"),
    (ColorSpace::ChromaDerivedCL, "chroma-derived-c"),
    (ColorSpace::ICTCP, "ictcp"),
];

/// vvencapp takes every VUI/SEI setting as `Key=value` pairs inside one
/// `--additional` string.
fn vvenc(clip_info: &ClipInfo) -> HashMap<String, CLIParameter> {
    let mut pairs: Vec<String> = Vec::new();
    // Each `push` returns the (possibly empty) list of pairs to add.
    fn push(pairs: &mut Vec<String>, key: &str, value: Option<String>) {
        if let Some(value) = value {
            pairs.push(format!("{key}={value}"));
        }
    }

    push(&mut pairs, "Range", map(VVENC_RANGE, clip_info.color_range));
    push(
        &mut pairs,
        "ColourPrimaries",
        map(VVENC_PRIMARIES, clip_info.color_primaries),
    );
    push(
        &mut pairs,
        "TransferCharacteristics",
        map(VVENC_TRANSFER, clip_info.color_transfer_characteristic),
    );
    push(
        &mut pairs,
        "MatrixCoefficients",
        map(VVENC_MATRIX, clip_info.color_space),
    );
    if let Some(index) = chroma_location_index(clip_info.chroma_location) {
        pairs.push("ChromaLocInfoPresent=1".to_owned());
        pairs.push(format!("ChromaSampleLocType={}", index as u32));
    }
    if let Some(values) =
        clip_info.mastering_display.as_ref().and_then(mastering_display_fixed_point)
    {
        pairs.push(format!(
            "MasteringDisplayColourVolume={},{},{},{},{},{},{},{},{},{}",
            values[0],
            values[1],
            values[2],
            values[3],
            values[4],
            values[5],
            values[6],
            values[7],
            values[8],
            values[9]
        ));
    }
    push(
        &mut pairs,
        "MaxContentLightLevel",
        clip_info.content_light_level.map(content_light_string),
    );

    let mut parameters = HashMap::new();
    if pairs.is_empty() {
        return parameters;
    }
    // The VUI only carries primaries/transfer/matrix when they are flagged
    // present.
    if pairs.iter().any(|pair| {
        pair.starts_with("ColourPrimaries=")
            || pair.starts_with("TransferCharacteristics=")
            || pair.starts_with("MatrixCoefficients=")
    }) {
        pairs.insert(0, "ColourDescriptionPresent=1".to_owned());
    }
    parameters.insert(
        "additional".to_owned(),
        CLIParameter::new_string("--", "=", &pairs.join(":")),
    );

    parameters
}

// ---------------------------------------------------------------------------
// FFmpeg
// ---------------------------------------------------------------------------

const FFMPEG_RANGE: &[(ColorRange, &str)] =
    &[(ColorRange::Limited, "tv"), (ColorRange::Full, "pc")];

const FFMPEG_PRIMARIES: &[(ColorPrimaries, &str)] = &[
    (ColorPrimaries::BT709, "bt709"),
    (ColorPrimaries::BT470M, "bt470m"),
    (ColorPrimaries::BT470BG, "bt470bg"),
    (ColorPrimaries::SMPTE170M, "smpte170m"),
    (ColorPrimaries::SMPTE240M, "smpte240m"),
    (ColorPrimaries::Film, "film"),
    (ColorPrimaries::BT2020, "bt2020"),
    (ColorPrimaries::SMPTE428, "smpte428"),
    (ColorPrimaries::SMPTE431, "smpte431"),
    (ColorPrimaries::SMPTE432, "smpte432"),
];

const FFMPEG_TRANSFER: &[(TransferCharacteristic, &str)] = &[
    (TransferCharacteristic::BT709, "bt709"),
    (TransferCharacteristic::GAMMA22, "bt470m"),
    (TransferCharacteristic::GAMMA28, "bt470bg"),
    (TransferCharacteristic::SMPTE170M, "smpte170m"),
    (TransferCharacteristic::SMPTE240M, "smpte240m"),
    (TransferCharacteristic::Linear, "linear"),
    (TransferCharacteristic::Log, "log100"),
    (TransferCharacteristic::LogSqrt, "log_sqrt"),
    (TransferCharacteristic::IEC61966_2_4, "iec61966-2-4"),
    (TransferCharacteristic::BT1361_ECG, "bt1361e"),
    (TransferCharacteristic::IEC61966_2_1, "iec61966-2-1"),
    (TransferCharacteristic::BT2020_10, "bt2020-10"),
    (TransferCharacteristic::BT2020_12, "bt2020-12"),
    (TransferCharacteristic::SMPTE2084, "smpte2084"),
    (TransferCharacteristic::SMPTE428, "smpte428"),
    (TransferCharacteristic::ARIB_STD_B67, "arib-std-b67"),
];

const FFMPEG_MATRIX: &[(ColorSpace, &str)] = &[
    (ColorSpace::RGB, "rgb"),
    (ColorSpace::BT709, "bt709"),
    (ColorSpace::FCC, "fcc"),
    (ColorSpace::BT470BG, "bt470bg"),
    (ColorSpace::SMPTE170M, "smpte170m"),
    (ColorSpace::SMPTE240M, "smpte240m"),
    (ColorSpace::YCGCO, "ycgco"),
    (ColorSpace::BT2020NCL, "bt2020nc"),
    (ColorSpace::BT2020CL, "bt2020c"),
    (ColorSpace::SMPTE2085, "smpte2085"),
    (ColorSpace::ChromaDerivedNCL, "chroma-derived-nc"),
    (ColorSpace::ChromaDerivedCL, "chroma-derived-c"),
    (ColorSpace::ICTCP, "ictcp"),
];

const FFMPEG_CHROMA_LOCATION: &[(ChromaLocation, &str)] = &[
    (ChromaLocation::Left, "left"),
    (ChromaLocation::Center, "center"),
    (ChromaLocation::TopLeft, "topleft"),
    (ChromaLocation::Top, "top"),
    (ChromaLocation::BottomLeft, "bottomleft"),
    (ChromaLocation::Bottom, "bottom"),
];

/// The FFmpeg flags in [`ffmpeg`], which FFmpeg only honours as *input*
/// options: `fftools/ffmpeg_enc.c` copies the colour properties out of every
/// frame, overwriting whatever was configured on the encoder context.
#[inline]
pub const fn ffmpeg_input_parameters() -> &'static [&'static str] {
    &[
        "color_primaries",
        "color_trc",
        "colorspace",
        "color_range",
        "chroma_sample_location",
    ]
}

fn ffmpeg(clip_info: &ClipInfo) -> HashMap<String, CLIParameter> {
    let mut parameters = HashMap::new();
    insert(
        &mut parameters,
        "color_primaries",
        map(FFMPEG_PRIMARIES, clip_info.color_primaries),
        "-",
        " ",
    );
    insert(
        &mut parameters,
        "color_trc",
        map(FFMPEG_TRANSFER, clip_info.color_transfer_characteristic),
        "-",
        " ",
    );
    insert(
        &mut parameters,
        "colorspace",
        map(FFMPEG_MATRIX, clip_info.color_space),
        "-",
        " ",
    );
    insert(
        &mut parameters,
        "color_range",
        map(FFMPEG_RANGE, clip_info.color_range),
        "-",
        " ",
    );
    insert(
        &mut parameters,
        "chroma_sample_location",
        map(FFMPEG_CHROMA_LOCATION, clip_info.chroma_location),
        "-",
        " ",
    );

    // FFmpeg carries mastering display and content light level as frame side
    // data only; there is no CLI AVOption to set them on a Y4M pipe.
    parameters
}

// ---------------------------------------------------------------------------
// VPX
// ---------------------------------------------------------------------------

const VPX_COLOR_SPACE: &[(ColorSpace, &str)] = &[
    (ColorSpace::RGB, "sRGB"),
    (ColorSpace::BT709, "bt709"),
    (ColorSpace::BT470BG, "bt601"),
    (ColorSpace::SMPTE170M, "smpte170"),
    (ColorSpace::SMPTE240M, "smpte240"),
    (ColorSpace::BT2020NCL, "bt2020"),
    (ColorSpace::BT2020CL, "bt2020"),
];

fn vpx(clip_info: &ClipInfo) -> HashMap<String, CLIParameter> {
    let mut parameters = HashMap::new();
    // vpxenc's only colour flag is `--color-space`, and it takes matrix
    // coefficients rather than primaries or a transfer function.
    insert(
        &mut parameters,
        "color-space",
        map(VPX_COLOR_SPACE, clip_info.color_space),
        "--",
        "=",
    );

    parameters
}

#[cfg(test)]
mod tests {
    use av_format::rational::Rational64;

    use super::*;
    use crate::core::input::pixel_format::PixelFormat;

    /// A 1080p SDR clip, so each test only sets the metadata it exercises.
    fn clip_info() -> ClipInfo {
        ClipInfo {
            num_frames:                    24,
            format_info:                   PixelFormat::VapourSynth {
                bit_depth: 10
            },
            frame_rate:                    Rational64::new(24, 1),
            resolution:                    (1920, 1080),
            color_range:                   None,
            color_primaries:               None,
            color_transfer_characteristic: None,
            color_space:                   None,
            chroma_location:               None,
            mastering_display:             None,
            content_light_level:           None,
        }
    }

    fn hdr10() -> ClipInfo {
        ClipInfo {
            color_range: Some(ColorRange::Limited),
            color_primaries: Some(ColorPrimaries::BT2020),
            color_transfer_characteristic: Some(TransferCharacteristic::SMPTE2084),
            color_space: Some(ColorSpace::BT2020NCL),
            chroma_location: Some(ChromaLocation::Left),
            mastering_display: Some(MasteringDisplayMetadata {
                primaries: Some((
                    [(0.170, 0.797), (0.131, 0.046), (0.708, 0.292)],
                    (0.3127, 0.3290),
                )),
                luminance: Some((0.005, 1000.0)),
            }),
            content_light_level: Some(ContentLightLevel {
                max_cll:  1000,
                max_fall: 400,
            }),
            ..clip_info()
        }
    }

    /// The parameters as argv fragments, sorted so the assertions do not depend
    /// on `HashMap` iteration order.
    fn rendered(base: EncoderBase, clip_info: &ClipInfo) -> Vec<String> {
        let mut parameters: Vec<String> = color_parameters(base, clip_info)
            .into_iter()
            .map(|(key, value)| value.to_parameter_string(&key))
            .collect();
        parameters.sort();

        parameters
    }

    #[test]
    fn a_clip_without_metadata_emits_nothing() {
        for base in [
            EncoderBase::AOM,
            EncoderBase::AVM,
            EncoderBase::RAV1E,
            EncoderBase::SVTAV1,
            EncoderBase::X264,
            EncoderBase::X265,
            EncoderBase::VVenC,
            EncoderBase::FFmpeg,
            EncoderBase::VPX,
        ] {
            assert_eq!(rendered(base, &clip_info()), Vec::<String>::new(), "{base}");
        }
    }

    #[test]
    fn hdr10_reaches_aom() {
        assert_eq!(rendered(EncoderBase::AOM, &hdr10()), [
            "--chroma-sample-position=vertical",
            "--color-primaries=bt2020",
            "--matrix-coefficients=bt2020ncl",
            "--transfer-characteristics=smpte2084",
        ]);
    }

    #[test]
    fn aom_skips_a_chroma_location_it_cannot_signal() {
        let clip_info = ClipInfo {
            chroma_location: Some(ChromaLocation::Center),
            ..hdr10()
        };

        assert!(
            !rendered(EncoderBase::AOM, &clip_info)
                .iter()
                .any(|parameter| parameter.starts_with("--chroma-sample-position")),
            "aomenc only spells vertical and colocated"
        );
        assert_eq!(
            rendered(EncoderBase::AVM, &clip_info)
                .iter()
                .filter(|parameter| parameter.starts_with("--chroma-sample-position"))
                .count(),
            1,
            "avmenc spells every location"
        );
    }

    #[test]
    fn hdr10_reaches_rav1e() {
        assert_eq!(rendered(EncoderBase::RAV1E, &hdr10()), [
            "--content-light 1000,400",
            "--mastering-display \
             G(0.17,0.797)B(0.131,0.046)R(0.708,0.292)WP(0.3127,0.329)L(1000,0.005)",
            "--matrix BT2020NCL",
            "--primaries BT2020",
            "--range Limited",
            "--transfer SMPTE2084",
        ]);
    }

    #[test]
    fn hdr10_reaches_svt_av1() {
        assert_eq!(rendered(EncoderBase::SVTAV1, &hdr10()), [
            "--chroma-sample-position vertical",
            "--color-primaries bt2020",
            "--color-range studio",
            "--content-light 1000,400",
            "--mastering-display \
             G(0.17,0.797)B(0.131,0.046)R(0.708,0.292)WP(0.3127,0.329)L(1000,0.005)",
            "--matrix-coefficients bt2020-ncl",
            "--transfer-characteristics smpte2084",
        ]);
    }

    #[test]
    fn hdr10_reaches_x264_in_fixed_point() {
        assert_eq!(rendered(EncoderBase::X264, &hdr10()), [
            "--chromaloc 0",
            "--cll 1000,400",
            "--colormatrix bt2020nc",
            "--colorprim bt2020",
            "--mastering-display \
             G(8500,39850)B(6550,2300)R(35400,14600)WP(15635,16450)L(10000000,50)",
            "--range tv",
            "--transfer smpte2084",
        ]);
    }

    #[test]
    fn hdr10_reaches_x265_in_fixed_point() {
        assert_eq!(rendered(EncoderBase::X265, &hdr10()), [
            "--chromaloc 0",
            "--colormatrix bt2020nc",
            "--colorprim bt2020",
            "--master-display G(8500,39850)B(6550,2300)R(35400,14600)WP(15635,16450)L(10000000,50)",
            "--max-cll 1000,400",
            "--range limited",
            "--transfer smpte2084",
        ]);
    }

    #[test]
    fn hdr10_reaches_vvenc_through_additional() {
        assert_eq!(rendered(EncoderBase::VVenC, &hdr10()), [
            "--additional=ColourDescriptionPresent=1:Range=limited:ColourPrimaries=bt2020:\
             TransferCharacteristics=smpte2084:MatrixCoefficients=bt2020nc:ChromaLocInfoPresent=1:\
             ChromaSampleLocType=0:MasteringDisplayColourVolume=8500,39850,6550,2300,35400,14600,\
             15635,16450,10000000,50:MaxContentLightLevel=1000,400",
        ]);
    }

    #[test]
    fn ffmpeg_colour_flags_are_input_options() {
        // Every flag `ffmpeg()` emits has to be listed, otherwise it reaches
        // the command line as an output option where FFmpeg discards it.
        let mut emitted: Vec<String> = ffmpeg(&hdr10()).into_keys().collect();
        emitted.sort();
        let mut listed: Vec<String> =
            ffmpeg_input_parameters().iter().map(|name| (*name).to_owned()).collect();
        listed.sort();

        assert_eq!(emitted, listed);
        assert_eq!(rendered(EncoderBase::FFmpeg, &hdr10()), [
            "-chroma_sample_location left",
            "-color_primaries bt2020",
            "-color_range tv",
            "-color_trc smpte2084",
            "-colorspace bt2020nc",
        ]);
    }

    #[test]
    fn vpx_only_signals_the_matrix() {
        assert_eq!(rendered(EncoderBase::VPX, &hdr10()), [
            "--color-space=bt2020".to_owned(),
        ]);
    }

    #[test]
    fn unspecified_values_are_omitted() {
        let clip_info = ClipInfo {
            color_primaries: Some(ColorPrimaries::Unspecified),
            color_transfer_characteristic: Some(TransferCharacteristic::Unspecified),
            color_space: Some(ColorSpace::Unspecified),
            chroma_location: Some(ChromaLocation::Unspecified),
            ..clip_info()
        };

        for base in [
            EncoderBase::AOM,
            EncoderBase::AVM,
            EncoderBase::RAV1E,
            EncoderBase::SVTAV1,
            EncoderBase::X264,
            EncoderBase::X265,
            EncoderBase::VVenC,
            EncoderBase::FFmpeg,
            EncoderBase::VPX,
        ] {
            assert_eq!(
                rendered(base, &clip_info),
                Vec::<String>::new(),
                "{base} must not signal metadata the source left unspecified"
            );
        }
    }

    #[test]
    fn values_an_encoder_cannot_spell_are_omitted() {
        let clip_info = ClipInfo {
            color_primaries: Some(ColorPrimaries::VGamut),
            ..clip_info()
        };

        for base in [
            EncoderBase::AOM,
            EncoderBase::RAV1E,
            EncoderBase::SVTAV1,
            EncoderBase::X264,
            EncoderBase::X265,
            EncoderBase::VVenC,
            EncoderBase::FFmpeg,
        ] {
            assert_eq!(
                rendered(base, &clip_info),
                Vec::<String>::new(),
                "{base} has no spelling for the versatile gamut"
            );
        }
    }

    #[test]
    fn vvenc_only_flags_the_colour_description_when_it_signals_one() {
        let clip_info = ClipInfo {
            content_light_level: Some(ContentLightLevel {
                max_cll:  1000,
                max_fall: 400,
            }),
            ..clip_info()
        };

        assert_eq!(rendered(EncoderBase::VVenC, &clip_info), [
            "--additional=MaxContentLightLevel=1000,400",
        ]);
    }

    #[test]
    fn the_vvenc_range_flag_enables_the_colour_description() {
        let clip_info = ClipInfo {
            color_range: Some(ColorRange::Full),
            ..clip_info()
        };

        assert_eq!(rendered(EncoderBase::VVenC, &clip_info), [
            "--additional=Range=full",
        ]);
    }
}
