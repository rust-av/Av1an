//! Parses zones from CLI values and zone files.
//!
//! A zone string reads
//!
//! ```text
//! <start> <end> [replace|merge] [encoder[(executable)]] ["options"] [flags]
//! ```
//!
//! where
//!
//! - `<start>` and `<end>` are frame numbers, half-open, with `-1` for the
//!   input's first and last frame,
//! - the encoder uses the names accepted elsewhere (e.g. `svt-av1`), optionally
//!   with the binary in parentheses,
//! - `replace` replaces the scene's encoder outright while the default,
//!   `merge`, patches the scene's own,
//! - the quoted string holds encoder parameters, as in *--params*,
//! - and the flags are `--maximum-frames`, `--ffmpeg-filter`, `--iso`,
//!   `--chroma-iso`, `--width`, `--height`, `--sy`, `--ccb`, `--ccr` and
//!   `--append-color-data`.
//!
//! Only [`ZoneEncoder::Replace`] and [`ZoneEncoder::Merge`] are reachable from
//! a string: merges always merge options and Photon Noise. The finer
//! [`ZoneOptions`] and Photon Noise replacements are a JSON-file concern.
//!
//! A CLI value that names an existing file is read as one, by extension:
//! `.json` for a `{ "zones": [...] }` object or a bare array, `.jsonl`/
//! `.ndjson` for one zone per line, and anything else as a text file of zone
//! strings with `#` comments and blank lines.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    str::FromStr,
};

use andean_condor::models::encoder::{Encoder, EncoderBase, cli_parameter::CLIParameter};
use anyhow::{Context, Result, bail};
use nom::{
    IResult,
    Parser,
    branch::alt,
    bytes::complete::take_while1,
    character::complete::{char, multispace0},
    combinator::{all_consuming, cut, map, recognize},
    multi::{many0, many1},
    sequence::{delimited, preceded, terminated},
};
use serde::Deserialize;

use crate::{
    utils::parameter_parser::EncoderParamsParser,
    zoner::{
        ZonerConfig,
        zone::{
            Zone,
            ZoneEncoder,
            ZoneEncoderMerge,
            ZoneOptions,
            ZoneOptionsBody,
            ZonePhotonNoise,
            ZonePhotonNoiseBody,
        },
    },
};

/// Extensions that mark a CLI value as a zone file rather than a zone string.
const FILE_EXTENSIONS: &[&str] = &["json", "jsonl", "ndjson", "txt"];

/// Parses CLI values, each an inline zone or a path to a zone file.
pub fn parse_zones(values: &[String]) -> Result<Vec<Zone>> {
    let mut zones = Vec::new();
    for value in values {
        let path = Path::new(value);
        if path.is_file() {
            zones.extend(load_zones(path)?);
        } else if names_a_zone_file(value) {
            bail!("zone file {value} does not exist");
        } else {
            zones.push(parse_zone(value)?);
        }
    }

    Ok(zones)
}

/// Whether a value looks like a path to a zone file. A zone string always
/// carries the space between its start and end frames.
fn names_a_zone_file(value: &str) -> bool {
    !value.trim().contains(char::is_whitespace)
        && Path::new(value)
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            .is_some_and(|extension| {
                FILE_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
            })
}

/// Reads zones from a JSON, JSON lines or text file.
pub fn load_zones(path: &Path) -> Result<Vec<Zone>> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("failed to read zone file {}", path.display()))?;
    let extension = path.extension().and_then(std::ffi::OsStr::to_str).map(str::to_ascii_lowercase);

    match extension.as_deref() {
        Some("json") => from_json(&contents, path),
        Some("jsonl" | "ndjson") => from_json_lines(&contents, path),
        _ => from_text(&contents, path),
    }
}

/// The shapes a JSON zone file can take.
#[derive(Deserialize)]
#[serde(untagged)]
enum ZoneDocument {
    /// `{ "zones": [ ... ] }`, as in `condor.json`.
    Config(ZonerConfig),
    /// A bare array of zones.
    List(Vec<Zone>),
}

fn from_json(contents: &str, path: &Path) -> Result<Vec<Zone>> {
    match serde_json::from_str::<ZoneDocument>(contents) {
        Ok(ZoneDocument::Config(config)) => Ok(config.zones),
        Ok(ZoneDocument::List(zones)) => Ok(zones),
        Err(error) => bail!("failed to read zones from {}: {error}", path.display()),
    }
}

fn from_json_lines(contents: &str, path: &Path) -> Result<Vec<Zone>> {
    let mut zones = Vec::new();
    for (index, line) in contents.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        zones.push(serde_json::from_str(line).with_context(|| {
            format!(
                "failed to read zone from {} line {}",
                path.display(),
                index + 1
            )
        })?);
    }

    Ok(zones)
}

fn from_text(contents: &str, path: &Path) -> Result<Vec<Zone>> {
    let mut zones = Vec::new();
    for (index, line) in contents.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        zones.push(parse_zone(line).with_context(|| {
            format!(
                "failed to read zone from {} line {}",
                path.display(),
                index + 1
            )
        })?);
    }

    Ok(zones)
}

/// Parses one zone string.
pub fn parse_zone(definition: &str) -> Result<Zone> {
    let mut scanner = Scanner::new(definition)?;

    let start_frame = scanner.bound("a start")?;
    let end_frame = scanner.bound("an end")?;
    let replace = scanner.mode();
    let encoder = scanner.encoder()?;
    let parameters = scanner.parameters()?;
    let Overrides {
        maximum_frames,
        ffmpeg_filter,
        append_color_data,
        photon_noise,
    } = scanner.overrides()?;

    Ok(Zone {
        start_frame,
        end_frame,
        maximum_frames,
        encoder: build_encoder(
            replace,
            encoder,
            parameters,
            ffmpeg_filter,
            append_color_data,
            photon_noise,
            &scanner,
        )?,
    })
}

/// Builds the zone's encoder change.
fn build_encoder(
    replace: bool,
    encoder: Option<(EncoderBase, Option<PathBuf>)>,
    parameters: Option<HashMap<String, CLIParameter>>,
    ffmpeg_filter: Option<String>,
    append_color_data: Option<bool>,
    photon_noise: ZonePhotonNoiseBody,
    scanner: &Scanner<'_>,
) -> Result<Option<ZoneEncoder>> {
    let Some((base, executable)) = encoder else {
        if parameters.is_some()
            || ffmpeg_filter.is_some()
            || append_color_data.is_some()
            || !photon_noise.is_empty()
        {
            return Err(scanner.error(
                "an encoder name is required for encoder options, Photon Noise and an FFmpeg \
                 filter",
            ));
        }

        return Ok(None);
    };

    if base == EncoderBase::FFmpeg && ffmpeg_filter.is_some() {
        return Err(scanner.error(
            "--ffmpeg-filter is not supported by the FFmpeg encoder; use native FFmpeg options \
             instead",
        ));
    }

    if !photon_noise.is_empty() && !base.supports_photon_noise() {
        return Err(scanner.error(&format!("{base} has no Photon Noise table")));
    }

    if replace {
        let mut replacement = Encoder::default_from_base(&base, false);
        if let Some(parameters) = parameters {
            replacement.parameters_mut().extend(parameters);
        }
        replacement.set_executable(executable);
        replacement.set_ffmpeg_filter(ffmpeg_filter);
        if let Some(append_color_data) = append_color_data {
            replacement.set_append_color_data(append_color_data);
        }
        if !photon_noise.is_empty() {
            let table = photon_noise.table().map_err(|error| scanner.error(&error.to_string()))?;
            replacement.set_photon_noise(Some(table));
        }

        Ok(Some(ZoneEncoder::Replace(replacement)))
    } else {
        Ok(Some(ZoneEncoder::Merge(ZoneEncoderMerge {
            base: Some(base),
            executable,
            options: parameters
                .map(|parameters| ZoneOptions::Merge(ZoneOptionsBody::Map(parameters))),
            // A zone string always merges options and Photon Noise; only a zone
            // file can ask for a replacement.
            photon_noise: (!photon_noise.is_empty())
                .then_some(ZonePhotonNoise::Merge(photon_noise)),
            ffmpeg_filter,
            append_color_data,
        })))
    }
}

/// The zone fields a string sets beyond the encoder change.
#[derive(Default)]
struct Overrides {
    maximum_frames:    Option<usize>,
    ffmpeg_filter:     Option<String>,
    append_color_data: Option<bool>,
    photon_noise:      ZonePhotonNoiseBody,
}

/// A zone string split into words, with quoted strings kept whole.
struct Scanner<'a> {
    definition: &'a str,
    tokens:     Vec<Token<'a>>,
    index:      usize,
}

#[derive(Clone, Copy)]
enum Token<'a> {
    /// A double-quoted string.
    Quoted(&'a str),
    /// A bare word.
    Word(&'a str),
}

impl<'a> Scanner<'a> {
    fn new(definition: &'a str) -> Result<Self> {
        Ok(Self {
            definition,
            tokens: tokenize(definition)?,
            index: 0,
        })
    }

    /// An error naming the zone that could not be read.
    fn error(&self, message: &str) -> anyhow::Error {
        anyhow::anyhow!("{message} in zone \"{}\"", self.definition)
    }

    fn peek(&self) -> Option<Token<'a>> {
        self.tokens.get(self.index).copied()
    }

    fn next(&mut self) -> Option<Token<'a>> {
        let token = self.peek();
        if token.is_some() {
            self.index += 1;
        }

        token
    }

    /// The next token, which has to be a bare word.
    fn word(&mut self, expected: &str) -> Result<&'a str> {
        match self.next() {
            Some(Token::Word(word)) => Ok(word),
            Some(Token::Quoted(value)) => {
                Err(self.error(&format!("expected {expected}, found \"{value}\"")))
            },
            None => Err(self.error(&format!("expected {expected}"))),
        }
    }

    /// The next token, word or quoted string.
    fn value(&mut self, flag: &str) -> Result<&'a str> {
        match self.next() {
            Some(Token::Word(value) | Token::Quoted(value)) => Ok(value),
            None => Err(self.error(&format!("{flag} needs a value"))),
        }
    }

    /// A frame number, where `-1` is the edge of the input and `description`
    /// names the bound in errors, article included.
    fn bound(&mut self, description: &str) -> Result<Option<usize>> {
        let word = self.word(&format!("{description} frame"))?;
        if word == "-1" {
            return Ok(None);
        }

        word.parse::<usize>().map(Some).map_err(|error| {
            self.error(&format!(
                "expected {description} frame or -1, found \"{word}\": {error}"
            ))
        })
    }

    /// The number value of a flag.
    fn number<T>(&mut self, flag: &str) -> Result<T>
    where
        T: FromStr,
        T::Err: std::fmt::Display,
    {
        let value = self.value(flag)?;
        value.parse::<T>().map_err(|error| {
            self.error(&format!(
                "{flag} needs a number, found \"{value}\": {error}"
            ))
        })
    }

    /// The `on`/`off` value of a flag.
    fn toggle(&mut self, flag: &str) -> Result<bool> {
        let value = self.value(flag)?;
        match value {
            "on" | "enable" | "enabled" | "yes" | "true" | "1" => Ok(true),
            "off" | "disable" | "disabled" | "no" | "false" | "0" => Ok(false),
            _ => Err(self.error(&format!("{flag} needs on or off, found \"{value}\""))),
        }
    }

    /// The comma-separated coefficients of a Photon Noise table flag.
    fn table(&mut self, flag: &str) -> Result<Vec<i8>> {
        let value = self.value(flag)?;
        value
            .split(',')
            .map(|entry| {
                entry.trim().parse::<i8>().map_err(|error| {
                    self.error(&format!(
                        "{flag} needs comma-separated coefficients from -128 to 127, found \
                         \"{value}\": {error}"
                    ))
                })
            })
            .collect()
    }

    /// Whether the zone replaces the scene's encoder; the default merges.
    fn mode(&mut self) -> bool {
        match self.peek() {
            Some(Token::Word("replace")) => {
                self.next();

                true
            },
            Some(Token::Word("merge")) => {
                self.next();

                false
            },
            _ => false,
        }
    }

    /// The encoder the zone names, if any.
    fn encoder(&mut self) -> Result<Option<(EncoderBase, Option<PathBuf>)>> {
        match self.peek() {
            Some(Token::Word(word)) if !word.starts_with('-') => {
                self.next();
                parse_encoder(word, self).map(Some)
            },
            _ => Ok(None),
        }
    }

    /// The quoted encoder parameter string, if any.
    fn parameters(&mut self) -> Result<Option<HashMap<String, CLIParameter>>> {
        match self.peek() {
            Some(Token::Quoted(parameters)) => {
                self.next();
                Ok(Some(
                    EncoderParamsParser::parse_string(parameters)
                        .map_err(|error| self.error(&error.to_string()))?,
                ))
            },
            _ => Ok(None),
        }
    }

    /// The trailing flags of a zone.
    fn overrides(&mut self) -> Result<Overrides> {
        let mut overrides = Overrides::default();
        while let Some(token) = self.next() {
            let Token::Word(flag) = token else {
                return Err(self.error("expected a flag, found a quoted string"));
            };
            match flag {
                "--maximum-frames" => overrides.maximum_frames = Some(self.number(flag)?),
                "--ffmpeg-filter" => overrides.ffmpeg_filter = Some(self.value(flag)?.to_owned()),
                "--append-color-data" => {
                    overrides.append_color_data = Some(self.toggle(flag)?);
                },
                "--iso" => overrides.photon_noise.iso = Some(self.number(flag)?),
                "--chroma-iso" => overrides.photon_noise.chroma_iso = Some(self.number(flag)?),
                "--width" => overrides.photon_noise.width = Some(self.number(flag)?),
                "--height" => overrides.photon_noise.height = Some(self.number(flag)?),
                "--sy" => overrides.photon_noise.c_y = Some(self.table(flag)?),
                "--ccb" => overrides.photon_noise.ccb = Some(self.table(flag)?),
                "--ccr" => overrides.photon_noise.ccr = Some(self.table(flag)?),
                flag => return Err(self.error(&format!("unknown flag \"{flag}\""))),
            }
        }

        Ok(overrides)
    }
}

/// Reads an encoder token: `name` or `name(path)`.
fn parse_encoder<'a>(word: &str, scanner: &Scanner<'a>) -> Result<(EncoderBase, Option<PathBuf>)> {
    let (name, executable) = match word.split_once('(') {
        Some((name, path)) => {
            let Some(path) = path.strip_suffix(')') else {
                return Err(scanner.error(&format!("expected a closing parenthesis in \"{word}\"")));
            };
            let path = path.trim_matches('"');
            if path.is_empty() {
                return Err(scanner.error(&format!("expected an executable path in \"{word}\"")));
            }

            (name, Some(PathBuf::from(path)))
        },
        None => (word, None),
    };
    let base = EncoderBase::from_str(name)
        .map_err(|_| scanner.error(&format!("\"{name}\" is not an encoder")))?;

    Ok((base, executable))
}

/// Splits a zone string into whitespace-separated tokens, keeping quoted
/// strings and the parenthesized executable of an encoder token whole.
fn tokenize(definition: &str) -> Result<Vec<Token<'_>>> {
    all_consuming(terminated(
        many0(preceded(
            multispace0,
            alt((
                map(quoted, Token::Quoted),
                map(
                    recognize(many1(alt((
                        take_while1(|c: char| !c.is_whitespace() && !matches!(c, '"' | '(' | ')')),
                        recognize(quoted),
                        executable,
                    )))),
                    Token::Word,
                ),
            )),
        )),
        multispace0,
    ))
    .parse(definition)
    .map(|(_, tokens)| tokens)
    .map_err(|error| {
        anyhow::anyhow!(
            "invalid zone tokens (unterminated quoted string or parenthesized executable) in zone \
             \"{definition}\": {error}"
        )
    })
}

fn quoted(input: &str) -> IResult<&str, &str> {
    delimited(
        char('"'),
        cut(nom::bytes::complete::take_until("\"")),
        char('"'),
    )
    .parse(input)
}

/// Preserve spaces and quotes in an executable token; parentheses inside a
/// quoted path are literal. Executable paths themselves are not recursive.
fn executable(input: &str) -> IResult<&str, &str> {
    recognize(delimited(
        char('('),
        cut(many0(alt((
            recognize(quoted),
            recognize(executable),
            take_while1(|c| !matches!(c, '(' | ')' | '"')),
        )))),
        char(')'),
    ))
    .parse(input)
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    /// The merge example from the documentation.
    const MERGE: &str = r#"-1 200 svt-av1("/tools/fish") "--preset 3 --crf 23" --iso 400 --chroma-iso 800 --sy 34,-1,32,-2,32"#;

    /// The same zone, replacing the scene's encoder.
    const REPLACE: &str = r#"-1 200 replace svt-av1("/tools/fish") "--preset 3 --crf 23" --iso 400 --chroma-iso 800 --sy 34,-1,32,-2,32"#;

    fn number(value: f64) -> CLIParameter {
        CLIParameter::new_number("--", " ", value)
    }

    fn merge(zone: &Zone) -> &ZoneEncoderMerge {
        match zone.encoder.as_ref().expect("the zone carries an encoder") {
            ZoneEncoder::Merge(merge) => merge,
            ZoneEncoder::Replace(_) => panic!("the zone merges rather than replaces"),
        }
    }

    /// The Photon Noise body of a zone that merges it.
    fn noise(merge: &ZoneEncoderMerge) -> &ZonePhotonNoiseBody {
        match merge.photon_noise.as_ref().expect("the zone carries noise") {
            ZonePhotonNoise::Merge(body) => body,
            ZonePhotonNoise::Replace(_) => {
                panic!("a zone string merges rather than replaces noise")
            },
        }
    }

    fn options(merge: &ZoneEncoderMerge) -> HashMap<String, CLIParameter> {
        match merge.options.as_ref().expect("the zone carries options") {
            ZoneOptions::Merge(body) | ZoneOptions::Replace(body) => {
                body.parameters().expect("the parameters parse")
            },
        }
    }

    #[test]
    fn nom_tokens_preserve_unicode_and_quoted_executable_parentheses() {
        let definition = "  0\t200 svt-av1(\"C:/tools (test)/编码器.exe\") \"--crf 16\" \
                          --ffmpeg-filter \"crop=100:100\" ";
        let zone = parse_zone(definition).unwrap();
        assert_eq!(
            merge(&zone).executable,
            Some(PathBuf::from("C:/tools (test)/编码器.exe"))
        );
        assert_eq!(options(merge(&zone))["crf"], number(16.0));
    }

    #[test]
    fn nom_tokens_reject_unclosed_executables_and_trailing_garbage() {
        assert!(parse_zone("0 200 svt-av1(/tools/fish").is_err());
        let zone = parse_zone("0 200 svt-av1(/tools (nightly)/encoder)").unwrap();
        assert_eq!(
            merge(&zone).executable,
            Some(PathBuf::from("/tools (nightly)/encoder"))
        );
        assert!(parse_zone("0 200 svt-av1) extra").is_err());
        assert!(parse_zone("0 200 --maximum-frames 100 extra").is_err());
    }

    #[test]
    fn a_merge_zone_parses_every_field() {
        let zone = parse_zone(MERGE).expect("the documented merge zone parses");

        assert_eq!(
            zone.start_frame, None,
            "-1 starts at the input's first frame"
        );
        assert_eq!(zone.end_frame, Some(200));
        assert_eq!(zone.maximum_frames, None);
        let merge = merge(&zone);
        assert_eq!(merge.base, Some(EncoderBase::SVTAV1));
        assert_eq!(merge.executable, Some(PathBuf::from("/tools/fish")));
        assert_eq!(
            options(merge).get("preset"),
            Some(&number(3.0)),
            "the parameter string parses like --params"
        );
        assert_eq!(options(merge).get("crf"), Some(&number(23.0)));
        let table = noise(merge);
        assert_eq!(table.iso, Some(400));
        assert_eq!(table.chroma_iso, Some(800));
        assert_eq!(table.c_y, Some(vec![34, -1, 32, -2, 32]));
        assert_eq!(table.ccb, None);
    }

    #[test]
    fn a_replace_zone_seeds_the_base_defaults() {
        let zone = parse_zone(REPLACE).expect("the documented replace zone parses");

        let ZoneEncoder::Replace(encoder) = zone.encoder.expect("the zone carries an encoder")
        else {
            panic!("the zone replaces rather than merges");
        };
        assert_eq!(encoder.base(), EncoderBase::SVTAV1);
        assert_eq!(encoder.parameters().get("preset"), Some(&number(3.0)));
        assert_eq!(encoder.parameters().get("crf"), Some(&number(23.0)));
        assert_eq!(
            encoder.parameters().get("keyint"),
            Some(&number(0.0)),
            "the base's defaults survive a replacement"
        );
        assert_eq!(encoder.photon_noise().map(|table| table.iso), Some(400));
    }

    #[test]
    fn an_explicit_merge_keyword_merges() {
        let zone = parse_zone("-1 200 merge svt-av1 \"--preset 3\"").expect("the zone parses");

        assert_eq!(merge(&zone).base, Some(EncoderBase::SVTAV1));
    }

    #[test]
    fn a_zone_can_only_cap_scene_length() {
        let zone = parse_zone("0 200 --maximum-frames 100").expect("the zone parses");

        assert_eq!(zone.start_frame, Some(0));
        assert_eq!(zone.end_frame, Some(200));
        assert_eq!(zone.maximum_frames, Some(100));
        assert_eq!(zone.encoder, None, "no encoder change is requested");
    }

    #[test]
    fn ffmpeg_filter_overrides_are_rejected_for_ffmpeg() {
        for definition in [
            "0 100 ffmpeg --ffmpeg-filter hflip",
            "0 100 replace ffmpeg --ffmpeg-filter hflip",
        ] {
            assert!(
                parse_zone(definition)
                    .expect_err("unsupported filter")
                    .to_string()
                    .contains("not supported by the FFmpeg encoder")
            );
        }
    }

    #[test]
    fn a_filtergraph_merges() {
        let zone = parse_zone("0 200 svt-av1 --ffmpeg-filter \"crop=iw-16:ih-16\"")
            .expect("the zone parses");

        assert_eq!(
            merge(&zone).ffmpeg_filter.as_deref(),
            Some("crop=iw-16:ih-16")
        );
    }

    #[test]
    fn an_encoder_without_parentheses_uses_the_default_binary() {
        let zone = parse_zone("0 200 x264 \"--crf 20\"").expect("the zone parses");

        assert_eq!(merge(&zone).executable, None);
    }

    #[test]
    fn a_missing_end_frame_is_an_error() {
        let error = parse_zone("0").expect_err("a zone needs both bounds");

        assert!(
            error.to_string().contains("expected an end frame"),
            "error names the missing bound: {error}"
        );
    }

    #[test]
    fn a_backwards_bound_is_an_error() {
        let error = parse_zone("-2 200").expect_err("only -1 is a boundary");

        assert!(
            error.to_string().contains("expected a start frame or -1"),
            "error explains the bound: {error}"
        );
    }

    #[test]
    fn an_unknown_flag_is_an_error() {
        let error = parse_zone("0 200 --preset 3").expect_err("only zooms flags are known");

        assert!(
            error.to_string().contains("unknown flag \"--preset\""),
            "error names the flag: {error}"
        );
    }

    #[test]
    fn a_zone_without_options_needs_no_encoder() {
        let error = parse_zone("0 200 \"--preset 3\"").expect_err("options need an encoder");

        assert!(
            error.to_string().contains("an encoder name is required"),
            "error explains the missing encoder: {error}"
        );
    }

    #[test]
    fn photon_noise_needs_a_capable_encoder() {
        let error = parse_zone("0 200 x264 --iso 400").expect_err("x264 has no grain table");

        assert!(
            error.to_string().contains("x264 has no Photon Noise table"),
            "error explains the encoder: {error}"
        );
    }

    #[test]
    fn an_oversized_table_coefficient_is_an_error() {
        let error = parse_zone("0 200 svt-av1 --iso 400 --sy 34,324")
            .expect_err("324 does not fit a table");

        assert!(
            error.to_string().contains("from -128 to 127"),
            "error explains the range: {error}"
        );
    }

    #[test]
    fn photon_noise_needs_an_iso() {
        let error = parse_zone("0 200 replace svt-av1 --chroma-iso 800")
            .expect_err("a replaced table needs an iso");

        assert!(
            error.to_string().contains("needs an iso"),
            "error explains the iso: {error}"
        );
    }

    #[test]
    fn an_unterminated_quote_is_an_error() {
        let error = parse_zone("0 200 svt-av1 \"--preset 3").expect_err("quotes must close");

        assert!(
            error.to_string().contains("unterminated quoted string"),
            "error explains the quote: {error}"
        );
    }

    #[test]
    fn an_unknown_encoder_is_an_error() {
        let error = parse_zone("0 200 av2 \"--preset 3\"").expect_err("av2 is not an encoder");

        assert!(
            error.to_string().contains("\"av2\" is not an encoder"),
            "error names the encoder: {error}"
        );
    }

    #[test]
    fn an_empty_executable_is_an_error() {
        let error = parse_zone("0 200 x264() \"--crf 20\"").expect_err("an empty path is invalid");

        assert!(
            error.to_string().contains("expected an executable path"),
            "error explains the path: {error}"
        );
    }

    #[test]
    fn a_finished_zone_leaves_nothing_to_parse() {
        let error = parse_zone("0 200 svt-av1 \"--preset 3\" extra")
            .expect_err("trailing words are not flags");

        assert!(
            error.to_string().contains("unknown flag \"extra\""),
            "error names the word: {error}"
        );
    }

    fn write(path: &Path, contents: &str) {
        let mut file = fs::File::create(path).expect("the test file is created");
        file.write_all(contents.as_bytes()).expect("the test file is written");
    }

    #[test]
    fn a_text_file_reads_one_zone_per_line() {
        let directory = tempfile::tempdir().expect("a temp directory");
        let path = directory.path().join("zones.txt");
        write(
            &path,
            "# a comment\n\n-1 200 svt-av1 \"--preset 3\"\n200 -1 replace x265 \"--crf 18\"\n",
        );

        let zones = load_zones(&path).expect("the zone file parses");

        assert_eq!(zones.len(), 2, "comments and blank lines are skipped");
        assert_eq!(zones[0].end_frame, Some(200));
        assert_eq!(zones[1].start_frame, Some(200));
        assert_eq!(zones[1].end_frame, None);
    }

    #[test]
    fn a_broken_text_file_names_its_line() {
        let directory = tempfile::tempdir().expect("a temp directory");
        let path = directory.path().join("zones.txt");
        write(&path, "-1 200 svt-av1 \"--preset 3\"\n0 100 nope\n");

        let error = load_zones(&path).expect_err("the second line is invalid");

        assert!(
            error.to_string().contains("zones.txt line 2"),
            "error names the line: {error}"
        );
    }

    #[test]
    fn a_json_file_reads_either_shape() {
        let directory = tempfile::tempdir().expect("a temp directory");
        let config_path = directory.path().join("zones.json");
        write(
            &config_path,
            r#"{"zones": [{"start_frame": null, "end_frame": 200, "maximum_frames": 100,
                "encoder": {"Merge": {"base": "SVTAV1", "options": {"Merge": "--preset 3"}}}}]}"#,
        );

        let zones = load_zones(&config_path).expect("the config-shaped file parses");

        assert_eq!(zones.len(), 1);
        assert_eq!(zones[0].maximum_frames, Some(100));
        assert_eq!(options(merge(&zones[0])).get("preset"), Some(&number(3.0)));

        let list_path = directory.path().join("list.json");
        write(
            &list_path,
            r#"[{"start_frame": 0, "end_frame": 200, "maximum_frames": null, "encoder": null}]"#,
        );

        let zones = load_zones(&list_path).expect("the array-shaped file parses");

        assert_eq!(zones.len(), 1);
        assert_eq!(zones[0].end_frame, Some(200));
        assert_eq!(zones[0].encoder, None);
    }

    #[test]
    fn a_json_lines_file_reads_one_zone_per_line() {
        let directory = tempfile::tempdir().expect("a temp directory");
        let path = directory.path().join("zones.jsonl");
        write(
            &path,
            "{\"start_frame\": 0, \"end_frame\": 200, \"maximum_frames\": 100, \"encoder\": \
             null}\n\n{\"start_frame\": 200, \"end_frame\": null, \"maximum_frames\": null, \
             \"encoder\": null}\n",
        );

        let zones = load_zones(&path).expect("the JSON lines file parses");

        assert_eq!(zones.len(), 2);
        assert_eq!(zones[1].start_frame, Some(200));
    }

    #[test]
    fn a_missing_zone_file_is_an_error() {
        let error = parse_zones(&["zones.json".to_owned()]).expect_err("the file does not exist");

        assert!(
            error.to_string().contains("zone file zones.json does not exist"),
            "error names the file: {error}"
        );
    }

    #[test]
    fn zone_files_and_inline_zones_mix() {
        let directory = tempfile::tempdir().expect("a temp directory");
        let path = directory.path().join("zones.txt");
        write(&path, "-1 200 svt-av1 \"--preset 3\"\n");

        let zones =
            parse_zones(&[path.display().to_string(), "200 -1 x265 \"--crf 18\"".to_owned()])
                .expect("both values parse");

        assert_eq!(zones.len(), 2);
        assert_eq!(zones[0].end_frame, Some(200));
        assert_eq!(zones[1].start_frame, Some(200));
    }
}
