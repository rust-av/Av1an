#[cfg(test)]
mod tests;

use std::{
    collections::HashMap,
    process::{exit, Command},
    str::FromStr,
};

use anyhow::{anyhow, bail, Result};
use itertools::Itertools;
use nom::{
    branch::alt,
    bytes::complete::{tag, take_till, take_while},
    character::complete::{char, digit1, space1},
    combinator::{map, map_res, opt, recognize, rest},
    multi::{many1, separated_list0},
    sequence::{preceded, tuple},
};
use serde::{Deserialize, Serialize};

use crate::{
    context::Av1anContext,
    parse::valid_params,
    settings::{invalid_params, suggest_fix},
    Encoder,
};

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Scene {
    pub start_frame:    usize,
    // Reminding again that end_frame is *exclusive*
    pub end_frame:      usize,
    pub zone_overrides: Option<ZoneOptions>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct ZoneOptions {
    pub encoder:             Encoder,
    pub passes:              u8,
    pub video_params:        Vec<String>,
    pub photon_noise:        Option<u8>,
    pub photon_noise_height: Option<u32>,
    pub photon_noise_width:  Option<u32>,
    pub chroma_noise:        bool,
    pub extra_splits_len:    Option<usize>,
    pub min_scene_len:       usize,
}

impl Scene {
    pub fn parse_from_zone(input: &str, context: &Av1anContext) -> Result<Self> {
        let (_, (start, _, end, _, encoder, reset, zone_args)): (
            _,
            (usize, _, usize, _, Encoder, bool, &str),
        ) = tuple::<_, _, nom::error::Error<&str>, _>((
            map_res(digit1, str::parse),
            many1(char(' ')),
            map_res(alt((tag("-1"), digit1)), |res: &str| {
                if res == "-1" {
                    Ok(context.frames)
                } else {
                    res.parse::<usize>()
                }
            }),
            many1(char(' ')),
            map_res(
                alt((
                    tag("aom"),
                    tag("rav1e"),
                    tag("x264"),
                    tag("x265"),
                    tag("vpx"),
                    tag("svt-av1"),
                )),
                Encoder::from_str,
            ),
            map(
                opt(preceded(many1(char(' ')), tag("reset"))),
                |res: Option<&str>| res.is_some(),
            ),
            map(
                opt(preceded(many1(char(' ')), rest)),
                |res: Option<&str>| res.unwrap_or_default().trim(),
            ),
        ))(input)
        .map_err(|e| anyhow!("Invalid zone file syntax: {}", e))?;
        if start >= end {
            bail!("Start frame must be earlier than the end frame");
        }
        if start >= context.frames || end > context.frames {
            bail!("Start and end frames must not be past the end of the video");
        }
        if encoder.format() != context.args.encoder.format() {
            bail!(
                "Zone specifies using {}, but this cannot be used in the same file as {}",
                encoder,
                context.args.encoder,
            );
        }
        if encoder != context.args.encoder {
            if encoder.get_format_bit_depth(context.args.output_pix_format.format).is_err() {
                bail!(
                    "Output pixel format {:?} is not supported by {} (used in zones file)",
                    context.args.output_pix_format.format,
                    encoder
                );
            }
            if !reset {
                bail!(
                    "Zone includes encoder change but previous args were kept. You probably meant \
                     to specify \"reset\"."
                );
            }
        }

        // Inherit from encode args or reset to defaults
        let mut video_params = if reset {
            Vec::new()
        } else {
            context.args.video_params.clone()
        };
        let mut passes = if reset {
            encoder.get_default_pass()
        } else {
            context.args.passes
        };
        let mut photon_noise = if reset {
            None
        } else {
            context.args.photon_noise
        };
        let mut photon_noise_height = if reset {
            None
        } else {
            context.args.photon_noise_size.1
        };
        let mut photon_noise_width = if reset {
            None
        } else {
            context.args.photon_noise_size.0
        };
        let mut chroma_noise = if reset {
            false
        } else {
            context.args.chroma_noise
        };
        let mut extra_splits_len = context.args.extra_splits_len;
        let mut min_scene_len = context.args.min_scene_len;

        // Parse overrides
        let zone_args: (&str, Vec<(&str, Option<&str>)>) =
            separated_list0::<_, _, _, nom::error::Error<&str>, _, _>(
                space1,
                tuple((
                    recognize(tuple((
                        alt((tag("--"), tag("-"))),
                        take_till(|c| c == '=' || c == ' '),
                    ))),
                    opt(preceded(alt((space1, tag("="))), take_while(|c| c != ' '))),
                )),
            )(zone_args)
            .map_err(|e| anyhow!("Invalid zone file syntax: {}", e))?;
        let mut zone_args = zone_args.1.into_iter().collect::<HashMap<_, _>>();
        if let Some(zone_passes) = zone_args.remove("--passes") {
            passes = zone_passes.unwrap().parse().unwrap();
        } else if [Encoder::aom, Encoder::vpx].contains(&encoder) && zone_args.contains_key("--rt")
        {
            passes = 1;
        }
        if let Some(zone_photon_noise) = zone_args.remove("--photon-noise") {
            photon_noise = Some(zone_photon_noise.unwrap().parse().unwrap());
        }
        if let Some(zone_photon_noise_height) = zone_args.remove("--photon-noise-height") {
            photon_noise_height = Some(zone_photon_noise_height.unwrap().parse().unwrap());
        }
        if let Some(zone_photon_noise_width) = zone_args.remove("--photon-noise-width") {
            photon_noise_width = Some(zone_photon_noise_width.unwrap().parse().unwrap());
        }
        if let Some(zone_chroma_noise) = zone_args.remove("--chroma-noise") {
            chroma_noise = zone_chroma_noise.unwrap().parse().unwrap();
        }
        if let Some(zone_xs) = zone_args.remove("-x").or_else(|| zone_args.remove("--extra-split"))
        {
            extra_splits_len = Some(zone_xs.unwrap().parse().unwrap());
        }
        if let Some(zone_min_scene_len) = zone_args.remove("--min-scene-len") {
            min_scene_len = zone_min_scene_len.unwrap().parse().unwrap();
        }
        let raw_zone_args = if [Encoder::aom, Encoder::vpx].contains(&encoder) {
            zone_args
                .into_iter()
                .map(|(key, value)| {
                    value.map_or_else(|| key.to_string(), |value| format!("{key}={value}"))
                })
                .collect::<Vec<String>>()
        } else {
            zone_args
                .keys()
                .map(|&k| Some(k.to_string()))
                .interleave(zone_args.values().map(|v| v.map(std::string::ToString::to_string)))
                .flatten()
                .collect::<Vec<String>>()
        };

        if !context.args.force {
            let help_text = {
                let [cmd, arg] = encoder.help_command();
                String::from_utf8(Command::new(cmd).arg(arg).output().unwrap().stdout).unwrap()
            };
            let valid_params = valid_params(&help_text, encoder);
            let interleaved_args: Vec<&str> = raw_zone_args
                .iter()
                .filter_map(|param| {
                    if param.starts_with('-') && [Encoder::aom, Encoder::vpx].contains(&encoder) {
                        // These encoders require args to be passed using an equal sign,
                        // e.g. `--cq-level=30`
                        param.split('=').next()
                    } else {
                        // The other encoders use a space, so we don't need to do extra splitting,
                        // e.g. `--crf 30`
                        None
                    }
                })
                .collect();
            let invalid_params = invalid_params(&interleaved_args, &valid_params);

            for wrong_param in &invalid_params {
                eprintln!("'{wrong_param}' isn't a valid parameter for {encoder}");
                if let Some(suggestion) = suggest_fix(wrong_param, &valid_params) {
                    eprintln!("\tDid you mean '{suggestion}'?");
                }
            }

            if !invalid_params.is_empty() {
                println!("\nTo continue anyway, run av1an with '--force'");
                exit(1);
            }
        }

        for arg in raw_zone_args {
            if arg.starts_with("--")
                || (arg.starts_with('-') && arg.chars().nth(1).is_some_and(char::is_alphabetic))
            {
                let key = arg.split_once('=').map_or(arg.as_str(), |split| split.0);
                if let Some(pos) = video_params
                    .iter()
                    .position(|param| param == key || param.starts_with(&format!("{key}=")))
                {
                    video_params.remove(pos);
                    if let Some(next) = video_params.get(pos) {
                        if !([Encoder::aom, Encoder::vpx].contains(&encoder)
                            || next.starts_with("--")
                            || (next.starts_with('-')
                                && next.chars().nth(1).is_some_and(char::is_alphabetic)))
                        {
                            video_params.remove(pos);
                        }
                    }
                }
            }
            video_params.push(arg);
        }

        Ok(Self {
            start_frame:    start,
            end_frame:      end,
            zone_overrides: Some(ZoneOptions {
                encoder,
                passes,
                video_params,
                photon_noise,
                photon_noise_height,
                photon_noise_width,
                chroma_noise,
                extra_splits_len,
                min_scene_len,
            }),
        })
    }
}
