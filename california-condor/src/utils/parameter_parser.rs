use std::collections::HashMap;

use andean_condor::models::encoder::cli_parameter::CLIParameter;
use nom::{
    branch::alt,
    bytes::complete::{tag, take_till1},
    character::complete::{char, multispace0, multispace1},
    combinator::rest,
    multi::many0,
    sequence::{preceded, separated_pair},
    IResult,
    Parser,
};

pub struct EncoderParamsParser;

impl EncoderParamsParser {
    #[inline]
    pub fn parse_string(input: &str) -> HashMap<String, CLIParameter> {
        let mut map = HashMap::new();
        // many0 with preceded handles whitespace-separated items
        let mut parser = many0(preceded(multispace0, Self::parse_parameter));

        if let Ok((_, items)) = parser.parse(input) {
            for (name, param) in items {
                map.insert(name, param);
            }
        }
        map
    }

    fn parse_parameter(input: &str) -> IResult<&str, (String, CLIParameter)> {
        alt((
            Self::parse_equals_pair, // [-/--][key]=[value]
            Self::parse_space_pair,  // [-/--][key] [value]
            Self::parse_flag,        // [-/--][flag]
        ))
        .parse(input)
    }

    fn parse_equals_pair(input: &str) -> IResult<&str, (String, CLIParameter)> {
        let (input, prefix) = Self::parse_prefix(input)?;
        let (input, (name, value)) = separated_pair(
            take_till1(|c| c == '=' || c == ' '),
            char('='),
            take_till1(|c| c == ' '),
        )
        .parse(input)?;

        Ok((
            input,
            (name.to_string(), Self::to_cli_parameter(prefix, "=", value)),
        ))
    }

    fn parse_space_pair(input: &str) -> IResult<&str, (String, CLIParameter)> {
        let (input, prefix) = Self::parse_prefix(input)?;
        let (input, name) = take_till1(|c| c == ' ').parse(input)?;
        let (input, _) = multispace1(input)?;
        let (input, value) = take_till1(|c| c == ' ').parse(input)?;

        Ok((
            input,
            (name.to_string(), Self::to_cli_parameter(prefix, " ", value)),
        ))
    }

    fn parse_flag(input: &str) -> IResult<&str, (String, CLIParameter)> {
        let (input, prefix) = Self::parse_prefix(input)?;
        // Take until space or end of string
        let (input, name) = alt((take_till1(|c| c == ' '), rest)).parse(input)?;

        Ok((
            input,
            (name.to_string(), CLIParameter::Bool {
                prefix: prefix.to_string(),
                value:  true,
            }),
        ))
    }

    fn parse_prefix(input: &str) -> IResult<&str, &str> {
        alt((tag("--"), tag("-"))).parse(input)
    }

    fn to_cli_parameter(prefix: &str, delim: &str, val: &str) -> CLIParameter {
        val.parse::<f64>().map_or_else(
            |_| CLIParameter::String {
                prefix:    prefix.to_string(),
                delimiter: delim.to_string(),
                value:     val.to_string(),
            },
            |num| CLIParameter::Number {
                prefix:    prefix.to_string(),
                delimiter: delim.to_string(),
                value:     num,
            },
        )
    }
}
