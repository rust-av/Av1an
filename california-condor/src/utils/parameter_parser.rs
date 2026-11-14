use std::collections::HashMap;

use andean_condor::models::encoder::cli_parameter::CLIParameter;
use anyhow::{Result, bail};
use thiserror::Error;

pub struct EncoderParamsParser;

impl EncoderParamsParser {
    /// Parse shell-like quoting into unique options, preserving prefixes and delimiters.
    /// Reject malformed input and duplicate names; the map cannot preserve option order
    /// or represent arbitrary positional arguments or repeated options.
    #[inline]
    pub fn parse_string(input: &str) -> Result<HashMap<String, CLIParameter>> {
        if input.trim().is_empty() {
            return Ok(HashMap::new());
        }
        if input.contains('\0') {
            bail!(EncoderParamsParserError::InvalidParams(
                "NUL bytes are not allowed".to_owned()
            ));
        }
        let tokens = shlex::split(input).ok_or_else(|| {
            EncoderParamsParserError::InvalidParams(
                "malformed quoting or dangling escape".to_owned(),
            )
        })?;
        let mut tokens = tokens.iter().peekable();
        let mut map = HashMap::new();
        while let Some(token) = tokens.next() {
            let (prefix, option) = token.strip_prefix("--").map_or_else(
                || token.strip_prefix('-').map_or(("", token.as_str()), |name| ("-", name)),
                |name| ("--", name),
            );
            let (name, inline_value) = option
                .split_once('=')
                .map_or((option, None), |(name, value)| (name, Some(value)));
            if name.is_empty() || name.chars().any(|c| c.is_whitespace() || c.is_control()) {
                bail!(EncoderParamsParserError::InvalidParams(format!(
                    "invalid option name in {token:?}"
                )));
            }
            let param = if let Some(value) = inline_value {
                Self::to_cli_parameter(prefix, "=", value)
            } else if let Some(value) = tokens.next_if(|value| {
                !value.starts_with('-') || value.parse::<f64>().is_ok_and(f64::is_finite)
            }) {
                Self::to_cli_parameter(prefix, " ", value)
            } else if !prefix.is_empty() {
                CLIParameter::Bool {
                    prefix: prefix.to_owned(),
                    value: true,
                }
            } else {
                bail!(EncoderParamsParserError::InvalidParams(format!(
                    "dashless option {name:?} requires a value"
                )));
            };
            if map.insert(name.to_owned(), param).is_some() {
                bail!(EncoderParamsParserError::InvalidParams(format!(
                    "duplicate option name {name:?}"
                )));
            }
        }
        Ok(map)
    }

    fn to_cli_parameter(prefix: &str, delim: &str, val: &str) -> CLIParameter {
        val.parse::<f64>().map_or_else(
            |_| CLIParameter::String {
                prefix: prefix.to_string(),
                delimiter: delim.to_string(),
                value: val.to_string(),
            },
            |num| CLIParameter::Number {
                prefix: prefix.to_string(),
                delimiter: delim.to_string(),
                value: num,
            },
        )
    }
}

#[derive(Debug, Error)]
pub enum EncoderParamsParserError {
    #[error("Invalid parameter: {0}")]
    InvalidParams(String),
}

#[cfg(test)]
mod tests {
    use std::assert_matches;

    use super::*;

    #[test]
    fn consecutive_flags_do_not_become_values() {
        for (input, flags, numbers) in [
            ("--no-sao --crf 20", vec!["no-sao"], vec![("crf", 20.0)]),
            (
                "--sharpness -2 --crf 20",
                vec![],
                vec![("sharpness", -2.0), ("crf", 20.0)],
            ),
            ("-an -sn", vec!["an", "sn"], vec![]),
            (
                "--key -inf -NaN -1e999",
                vec!["key", "inf", "NaN", "1e999"],
                vec![],
            ),
        ] {
            let parsed = EncoderParamsParser::parse_string(input).expect("valid parameters");
            assert_eq!(parsed.len(), flags.len() + numbers.len(), "{input}");
            for flag in flags {
                assert_matches!(parsed[flag], CLIParameter::Bool { value: true, .. });
            }
            for (name, expected) in numbers {
                assert_matches!(parsed[name], CLIParameter::Number { value, .. } if value == expected);
            }
        }
    }

    #[test]
    fn rejects_malformed_or_duplicate_parameters_without_partial_success() {
        for (input, context) in [
            ("--ok=1 --key \"unfinished", "quot"),
            ("--ok=1 --key 'unfinished", "quot"),
            ("--ok=1 --key value\\", "escape"),
            ("--ok=1 --", "name"),
            ("--ok=1 -", "name"),
            ("--ok=1 --=value", "name"),
            ("--ok=1 =value", "name"),
            ("--ok=1 ''", "name"),
            ("--ok=1 '--bad name'=value", "name"),
            ("--ok=1 '--bad\tname'=value", "name"),
            ("--ok=1 --bad\u{7}=value", "name"),
            ("--ok=1 --key=bad\0value", "NUL"),
            ("--ok=1 trailing", "value"),
            ("--crf 20 --crf 21", "crf"),
            ("--crf 20 -crf=21", "crf"),
            ("crf=20 --crf 21", "crf"),
        ] {
            let error = EncoderParamsParser::parse_string(input)
                .expect_err(&format!("must reject the entire input: {input:?}"));
            assert!(error.downcast_ref::<EncoderParamsParserError>().is_some());
            assert!(error.to_string().contains(context), "{input:?}: {error:#}");
            if context == "crf" {
                assert!(error.to_string().contains("duplicate"), "{error:#}");
            }
        }
    }

    #[test]
    fn parse_empty_string() {
        let result = EncoderParamsParser::parse_string("");
        assert!(
            result.is_ok_and(|result| { result.is_empty() }),
            "input contains no parameters"
        );
        // assert!(result.is_empty(), "empty input should produce empty map");
    }

    #[test]
    fn parse_single_dash_flag() {
        let result = EncoderParamsParser::parse_string("--flag");
        assert_matches!(result, Ok(_), "input should parse");
        let result = result.expect("input should parse");
        assert_eq!(result.len(), 1, "input contains 1 parameter");
        assert_matches!(
            result["flag"],
            CLIParameter::Bool {
                ref prefix,
                value,
            } if prefix == "--" && value,
            "input contains true flag preceded by \"--\""
        );
    }

    #[test]
    fn parse_single_short_flag() {
        let result = EncoderParamsParser::parse_string("-f");
        assert!(result.is_ok(), "input should parse");
        let result = result.expect("input should parse");
        assert_eq!(result.len(), 1, "input contains 1 parameter");
        assert_matches!(
            result["f"],
            CLIParameter::Bool {
                ref prefix,
                value,
            } if prefix == "-" && value,
            "input contains true short flag preceded by \"-\""
        );
    }

    #[test]
    fn parse_equals_pair_string() {
        let result = EncoderParamsParser::parse_string("--key=value");
        assert!(result.is_ok(), "input should parse");
        let result = result.expect("input should parse");
        assert_eq!(result.len(), 1, "input contains 1 parameter");
        assert_matches!(result["key"], CLIParameter::String { ref prefix, ref delimiter, ref value } if prefix == "--" && delimiter == "=" && value == "value", "input contains key value preceded by \"--\" separated by \"=\"");
    }

    #[test]
    fn parse_equals_pair_number() {
        let result = EncoderParamsParser::parse_string("--crf=23");
        assert!(result.is_ok(), "input should parse");
        let result = result.expect("input should parse");
        assert_eq!(result.len(), 1, "input contains 1 parameter");
        assert_matches!(result["crf"], CLIParameter::Number { ref prefix, ref delimiter, value } if prefix == "--" && delimiter == "=" && value == 23.0, "input contains crf 23 preceded by \"--\" separated by \"=\"");
    }

    #[test]
    fn parse_space_pair_string() {
        let result = EncoderParamsParser::parse_string("--key value");
        assert!(result.is_ok(), "input should parse");
        let result = result.expect("input should parse");
        assert_eq!(result.len(), 1, "input contains 1 parameter");
        assert_matches!(result["key"], CLIParameter::String { ref prefix, ref delimiter, ref value } if prefix == "--" && delimiter == " " && value == "value", "input contains key value preceded by \"--\" separated by \" \"");
    }

    #[test]
    fn parse_space_pair_number() {
        let result = EncoderParamsParser::parse_string("--crf 30");
        assert!(result.is_ok(), "input should parse");
        let result = result.expect("input should parse");
        assert_eq!(result.len(), 1, "input contains 1 parameter");
        assert_matches!(result["crf"], CLIParameter::Number { ref prefix, ref delimiter, value } if prefix == "--" && delimiter == " " && value == 30.0, "input contains crf 30 preceded by \"--\" separated by \" \"");
    }

    #[test]
    fn parse_dashless_pairs_keep_an_empty_prefix() {
        let result =
            EncoderParamsParser::parse_string("crf=16 tune=0").expect("dash-less pairs parse");
        assert_eq!(result.len(), 2, "input contains 2 parameters");
        assert_matches!(
            result["crf"],
            CLIParameter::Number { ref prefix, ref delimiter, value }
                if prefix.is_empty() && delimiter == "=" && value == 16.0,
            "crf=16 keeps its empty prefix instead of gaining one"
        );
        assert_matches!(
            result["tune"],
            CLIParameter::Number { ref prefix, ref delimiter, value }
                if prefix.is_empty() && delimiter == "=" && value == 0.0,
            "tune=0 keeps its empty prefix instead of gaining one"
        );
    }

    #[test]
    fn parse_dashless_space_pair() {
        let result = EncoderParamsParser::parse_string("crf 16").expect("parses");
        assert_matches!(
            result["crf"],
            CLIParameter::Number { ref prefix, ref delimiter, value }
                if prefix.is_empty() && delimiter == " " && value == 16.0,
            "a space-separated pair works without a dash too"
        );
    }

    #[test]
    fn parse_multiple_params() {
        let result = EncoderParamsParser::parse_string("--key1 val1 --key2=val2 -f");
        assert!(result.is_ok(), "input should parse");
        let result = result.expect("input should parse");
        assert_eq!(result.len(), 3, "input contains 3 parameters");
        assert!(result.contains_key("key1"), "input contains key1");
        assert!(result.contains_key("key2"), "input contains key2");
        assert!(result.contains_key("f"), "input contains f");
    }

    // TODO: Restrict allowed characters in parameter names so the "," isn't
    // allowed (current parses as a flag) #[test]
    // fn parse_unsupported_delimiters() {
    //     let result = EncoderParamsParser::parse_string("--invalid-param,43");
    //     assert_matches!(result, Err(_), "input should not parse");
    // }
}
