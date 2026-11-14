use std::{
    collections::BTreeMap,
    fmt::{Display, Write},
    str::FromStr,
};

use anyhow::{Result, bail};
use av_decoders::{VideoDetails, v_frame::chroma::ChromaSubsampling};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    ffmpeg::FFPixelFormat,
    vapoursynth::{plugins::resize::Scaler, vapoursynth_filters::VapourSynthFilter},
};

/// Chroma subsampling that a native FFMS2 input can be converted to.
///
/// [`ChromaSubsampling`] carries no serde derives, so this is a local mirror,
/// as with [`ColorRange`] and [`PixelFormat`].
///
/// [`ColorRange`]: crate::core::input::color_range::ColorRange
/// [`PixelFormat`]: crate::core::input::pixel_format::PixelFormat
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ChromaSampling {
    Yuv420,
    Yuv422,
    Yuv444,
    Monochrome,
}

impl ChromaSampling {
    #[inline]
    #[must_use]
    pub fn to_chroma_subsampling(self) -> ChromaSubsampling {
        match self {
            ChromaSampling::Yuv420 => ChromaSubsampling::Yuv420,
            ChromaSampling::Yuv422 => ChromaSubsampling::Yuv422,
            ChromaSampling::Yuv444 => ChromaSubsampling::Yuv444,
            ChromaSampling::Monochrome => ChromaSubsampling::Monochrome,
        }
    }

    #[inline]
    #[must_use]
    pub fn from_chroma_subsampling(chroma: ChromaSubsampling) -> Self {
        match chroma {
            ChromaSubsampling::Yuv420 => ChromaSampling::Yuv420,
            ChromaSubsampling::Yuv422 => ChromaSampling::Yuv422,
            ChromaSubsampling::Yuv444 => ChromaSampling::Yuv444,
            ChromaSubsampling::Monochrome => ChromaSampling::Monochrome,
        }
    }
}

impl Display for ChromaSampling {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            ChromaSampling::Yuv420 => "Yuv420",
            ChromaSampling::Yuv422 => "Yuv422",
            ChromaSampling::Yuv444 => "Yuv444",
            ChromaSampling::Monochrome => "Monochrome",
        };
        write!(f, "{s}")
    }
}

impl FromStr for ChromaSampling {
    type Err = anyhow::Error;

    #[inline]
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.to_ascii_lowercase().as_str() {
            "420" | "yuv420" | "yuv420p" => ChromaSampling::Yuv420,
            "422" | "yuv422" | "yuv422p" => ChromaSampling::Yuv422,
            "444" | "yuv444" | "yuv444p" => ChromaSampling::Yuv444,
            "mono" | "monochrome" | "gray" | "grey" => ChromaSampling::Monochrome,
            other => bail!("Invalid chroma: {other}"),
        })
    }
}

/// Filters available to natively-decoded (non-VapourSynth) FFMS2 inputs.
///
/// FFMS2 can only convert the frames when it decodes with
/// `Ffms2Decoder::set_output_format` to a target pixel format and resolution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum Ffms2Filter {
    /// Sets the output resolution, bit depth and chroma subsampling.
    ///
    /// Any field left `None` keeps the decoded value.
    ///
    /// 8/10/12-bit are supported across YUV420/422/444 and monochrome. A
    /// request with no suitable output format is an error rather than a
    /// silent substitution.
    OutputFormat {
        /// Target bits per component: 8, 10 or 12.
        bit_depth: Option<u8>,
        /// Target chroma subsampling.
        chroma: Option<ChromaSampling>,
        /// Target width in pixels.
        width: Option<u32>,
        /// Target height in pixels.
        height: Option<u32>,
    },
}

/// Maps a bit depth and chroma subsampling onto the pixel format FFMS2 decodes
/// to, for the combinations FFMS2 supports.
#[inline]
#[must_use]
pub fn ffms2_pixel_format(bit_depth: usize, chroma: ChromaSubsampling) -> Option<FFPixelFormat> {
    Some(match (bit_depth, chroma) {
        (8, ChromaSubsampling::Yuv420) => FFPixelFormat::YUV420P,
        (8, ChromaSubsampling::Yuv422) => FFPixelFormat::YUV422P,
        (8, ChromaSubsampling::Yuv444) => FFPixelFormat::YUV444P,
        (8, ChromaSubsampling::Monochrome) => FFPixelFormat::GRAY8,
        (10, ChromaSubsampling::Yuv420) => FFPixelFormat::YUV420P10LE,
        (10, ChromaSubsampling::Yuv422) => FFPixelFormat::YUV422P10LE,
        (10, ChromaSubsampling::Yuv444) => FFPixelFormat::YUV444P10LE,
        (10, ChromaSubsampling::Monochrome) => FFPixelFormat::GRAY10LE,
        (12, ChromaSubsampling::Yuv420) => FFPixelFormat::YUV420P12LE,
        (12, ChromaSubsampling::Yuv422) => FFPixelFormat::YUV422P12LE,
        (12, ChromaSubsampling::Yuv444) => FFPixelFormat::YUV444P12LE,
        (12, ChromaSubsampling::Monochrome) => FFPixelFormat::GRAY12LE,
        _ => return None,
    })
}

impl FromStr for Ffms2Filter {
    type Err = anyhow::Error;

    #[inline]
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split(':').collect();
        let variant_name = parts.first().copied().unwrap_or_default();
        let variant_args = parts
            .get(1)
            .map(|args| {
                args.split(';')
                    .map(|arg| arg.trim())
                    .filter(|arg| !arg.is_empty())
                    .map(|arg| {
                        let mut parts = arg.splitn(2, '=');
                        let name = parts
                            .next()
                            .ok_or_else(|| {
                                anyhow::anyhow!("Failed to parse filter argument: {arg}")
                            })?
                            .trim()
                            .to_lowercase();
                        let value =
                            parts.next().map(|value| value.trim().to_string()).unwrap_or_default();
                        Ok((name, value))
                    })
                    .collect::<Result<BTreeMap<_, _>>>()
            })
            .transpose()?
            .unwrap_or_default();

        match variant_name {
            "output-format" | "output_format" => {
                let bit_depth = match variant_args.get("bit_depth") {
                    Some(value) => Some(
                        value
                            .parse::<u8>()
                            .map_err(|_| anyhow::anyhow!("Invalid bit_depth: {value}"))?,
                    ),
                    None => None,
                };
                if let Some(bit_depth) = bit_depth
                    && !matches!(bit_depth, 8 | 10 | 12)
                {
                    bail!("Unsupported bit depth: {bit_depth} (expected 8, 10 or 12)");
                }

                let chroma = match variant_args.get("chroma") {
                    Some(value) => Some(ChromaSampling::from_str(value)?),
                    None => None,
                };

                let width = match variant_args.get("width") {
                    Some(value) => Some(
                        value
                            .parse::<u32>()
                            .map_err(|_| anyhow::anyhow!("Invalid width: {value}"))?,
                    ),
                    None => None,
                };
                let height = match variant_args.get("height") {
                    Some(value) => Some(
                        value
                            .parse::<u32>()
                            .map_err(|_| anyhow::anyhow!("Invalid height: {value}"))?,
                    ),
                    None => None,
                };

                Ok(Ffms2Filter::OutputFormat {
                    bit_depth,
                    chroma,
                    width,
                    height,
                })
            },
            "" => bail!("Missing filter variant name"),
            other => bail!("Invalid variant name: {other}"),
        }
    }
}

impl Display for Ffms2Filter {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Ffms2Filter::OutputFormat {
                bit_depth,
                chroma,
                width,
                height,
            } => {
                let mut s = String::from("output-format:");
                let _ = write!(
                    s,
                    "{}{}{}{}",
                    bit_depth.map(|v| format!("bit_depth={v};")).unwrap_or_default(),
                    chroma.map(|v| format!("chroma={v};")).unwrap_or_default(),
                    width.map(|v| format!("width={v};")).unwrap_or_default(),
                    height.map(|v| format!("height={v};")).unwrap_or_default()
                );
                s
            },
        };
        write!(f, "{s}")
    }
}

impl Ffms2Filter {
    /// The pixel format this filter requests, if it pins one.
    ///
    /// A filter that only scales leaves the decoded pixel format alone.
    #[inline]
    #[must_use]
    pub fn output_format(&self) -> Option<FFPixelFormat> {
        let Ffms2Filter::OutputFormat {
            bit_depth, chroma, ..
        } = self;
        let bit_depth = (*bit_depth)?;
        let chroma = (*chroma)?;
        ffms2_pixel_format(usize::from(bit_depth), chroma.to_chroma_subsampling())
    }

    /// The equivalent VapourSynth filter, for when a native input is instead
    /// opened through VapourSynth (for example to run a VS-only metric).
    ///
    /// `baseline` supplies the values the FFMS2 filter would have inherited
    /// from the decoded stream, so an unset field keeps the current value
    /// instead of being dropped. Returns an empty list for a filter that
    /// changes nothing. Returns an error for unsupported formats or dimensions.
    #[inline]
    pub fn to_vapoursynth_filters(
        &self,
        baseline: &VideoDetails,
    ) -> Result<Vec<VapourSynthFilter>> {
        self.validate()?;
        let Ffms2Filter::OutputFormat {
            bit_depth,
            chroma,
            width,
            height,
        } = self;

        let bit_depth = bit_depth.map_or(baseline.bit_depth, usize::from);
        let chroma = chroma.map_or(baseline.chroma_sampling, |chroma| {
            chroma.to_chroma_subsampling()
        });
        let width = width.map_or(baseline.width, |width| width as usize);
        let height = height.map_or(baseline.height, |height| height as usize);
        let format = ffms2_pixel_format(bit_depth, chroma).ok_or_else(|| {
            anyhow::anyhow!("Unsupported FFMS2 output format: {bit_depth}-bit {chroma:?}")
        })?;

        if (bit_depth, chroma) == (baseline.bit_depth, baseline.chroma_sampling)
            && (width, height) == (baseline.width, baseline.height)
        {
            return Ok(Vec::new());
        }

        let changes_format = bit_depth != baseline.bit_depth || chroma != baseline.chroma_sampling;
        Ok(vec![VapourSynthFilter::Resize {
            scaler: Some(Scaler::Bicubic),
            width: (width != baseline.width).then_some(width),
            height: (height != baseline.height).then_some(height),
            format: changes_format.then_some(format),
        }])
    }

    /// The equivalent VapourSynth filter, with unset fields left unset.
    ///
    /// No source is opened here. The deferred filter resolves omitted fields
    /// against its actual input node, including the output of a preceding filter.
    #[inline]
    #[must_use]
    pub fn to_vapoursynth(&self) -> Vec<VapourSynthFilter> {
        let Ffms2Filter::OutputFormat {
            bit_depth,
            chroma,
            width,
            height,
        } = self;

        vec![VapourSynthFilter::OutputFormat {
            bit_depth: *bit_depth,
            chroma: *chroma,
            width: *width,
            height: *height,
        }]
    }

    /// Validate native/deferred fields before narrowing or invoking a plugin.
    pub(crate) fn validate(&self) -> Result<()> {
        let Self::OutputFormat {
            bit_depth,
            width,
            height,
            ..
        } = self;
        if let Some(depth) = bit_depth {
            anyhow::ensure!(
                matches!(depth, 8 | 10 | 12),
                "Unsupported bit depth: {depth} (expected 8, 10 or 12)"
            );
        }
        for (name, dimension) in [("width", width), ("height", height)] {
            if let Some(value) = dimension {
                anyhow::ensure!(
                    *value > 0 && i32::try_from(*value).is_ok(),
                    "FFMS2 output {name} {value} must be positive and fit i32"
                );
            }
        }
        Ok(())
    }

    /// The native equivalent of a VapourSynth filter, if FFMS2 supports it.
    ///
    /// Only `Resize` and deferred `OutputFormat` map, because format and resolution are FFMS2's
    /// only output modification. The scaler is not preserved: FFMS2 always uses
    /// bicubic. Returns `Ok(None)` for any other filter.
    ///
    /// # Errors
    /// Returns an error naming an unsupported explicit format, or invalid dimensions.
    #[inline]
    pub fn from_vapoursynth_filter(filter: &VapourSynthFilter) -> Result<Option<Self>> {
        if let VapourSynthFilter::OutputFormat {
            bit_depth,
            chroma,
            width,
            height,
        } = filter
        {
            let native = Self::OutputFormat {
                bit_depth: *bit_depth,
                chroma: *chroma,
                width: *width,
                height: *height,
            };
            native.validate()?;
            return Ok(Some(native));
        }
        let VapourSynthFilter::Resize {
            width,
            height,
            format,
            ..
        } = filter
        else {
            return Ok(None);
        };

        // A pixel format names both the depth and the chroma layout, so one
        // field yields both.
        let (bit_depth, chroma) = if let Some(format) = format {
            let chroma = match format {
                FFPixelFormat::YUV420P
                | FFPixelFormat::YUV420P10LE
                | FFPixelFormat::YUV420P12LE => Some(ChromaSampling::Yuv420),
                FFPixelFormat::YUV422P
                | FFPixelFormat::YUV422P10LE
                | FFPixelFormat::YUV422P12LE => Some(ChromaSampling::Yuv422),
                FFPixelFormat::YUV444P
                | FFPixelFormat::YUV444P10LE
                | FFPixelFormat::YUV444P12LE => Some(ChromaSampling::Yuv444),
                FFPixelFormat::GRAY8 | FFPixelFormat::GRAY10LE | FFPixelFormat::GRAY12LE => {
                    Some(ChromaSampling::Monochrome)
                },
                _ => bail!(
                    "Unsupported FFMS2 output format: {}",
                    format.to_pix_fmt_string()
                ),
            };
            let bit_depth = match format {
                FFPixelFormat::YUV420P10LE
                | FFPixelFormat::YUV422P10LE
                | FFPixelFormat::YUV444P10LE
                | FFPixelFormat::GRAY10LE => Some(10),
                FFPixelFormat::YUV420P12LE
                | FFPixelFormat::YUV422P12LE
                | FFPixelFormat::YUV444P12LE
                | FFPixelFormat::GRAY12LE => Some(12),
                _ => Some(8),
            };
            (bit_depth, chroma)
        } else {
            (None, None)
        };

        let native = Ffms2Filter::OutputFormat {
            bit_depth,
            chroma,
            width: width.map(u32::try_from).transpose()?,
            height: height.map(u32::try_from).transpose()?,
        };
        native.validate()?;
        Ok(Some(native))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bit_depth_and_chroma() {
        let filter = Ffms2Filter::from_str("output-format:bit_depth=10;chroma=420;")
            .expect("output-format should parse");

        assert_eq!(
            filter,
            Ffms2Filter::OutputFormat {
                bit_depth: Some(10),
                chroma: Some(ChromaSampling::Yuv420),
                width: None,
                height: None,
            }
        );
    }

    #[test]
    fn parses_resolution_only() {
        let filter = Ffms2Filter::from_str("output-format:width=1280;height=720;")
            .expect("output-format should parse");

        assert_eq!(
            filter,
            Ffms2Filter::OutputFormat {
                bit_depth: None,
                chroma: None,
                width: Some(1280),
                height: Some(720),
            }
        );
        // Scaling alone does not change the pixel format.
        assert_eq!(filter.output_format(), None);
    }

    #[test]
    fn empty_arguments_are_all_none() {
        let filter = Ffms2Filter::from_str("output-format")
            .expect("output-format with no args should parse");

        assert_eq!(
            filter,
            Ffms2Filter::OutputFormat {
                bit_depth: None,
                chroma: None,
                width: None,
                height: None,
            }
        );
        assert_eq!(filter.output_format(), None);
    }

    #[test]
    fn rejects_unsupported_bit_depth() {
        assert!(Ffms2Filter::from_str("output-format:bit_depth=9;").is_err());
    }

    #[test]
    fn rejects_unknown_variant() {
        assert!(Ffms2Filter::from_str("crop:top=1;").is_err());
        assert!(Ffms2Filter::from_str("").is_err());
    }

    #[test]
    fn rejects_non_numeric_dimensions() {
        assert!(Ffms2Filter::from_str("output-format:width=wide;").is_err());
        assert!(Ffms2Filter::from_str("output-format:height=tall;").is_err());
    }

    #[test]
    fn output_format_maps_depth_and_chroma() {
        let ten_bit_420 = Ffms2Filter::OutputFormat {
            bit_depth: Some(10),
            chroma: Some(ChromaSampling::Yuv420),
            width: None,
            height: None,
        };
        assert_eq!(
            ten_bit_420.output_format(),
            Some(FFPixelFormat::YUV420P10LE)
        );

        let eight_bit_444 = Ffms2Filter::OutputFormat {
            bit_depth: Some(8),
            chroma: Some(ChromaSampling::Yuv444),
            width: Some(1920),
            height: Some(1080),
        };
        assert_eq!(eight_bit_444.output_format(), Some(FFPixelFormat::YUV444P));
    }

    #[test]
    fn chroma_round_trips_through_chroma_subsampling() {
        for chroma in [
            ChromaSampling::Yuv420,
            ChromaSampling::Yuv422,
            ChromaSampling::Yuv444,
            ChromaSampling::Monochrome,
        ] {
            assert_eq!(
                ChromaSampling::from_chroma_subsampling(chroma.to_chroma_subsampling()),
                chroma
            );
        }
    }

    #[test]
    fn display_round_trips() {
        for input in [
            "output-format:bit_depth=10;chroma=Yuv420;",
            "output-format:width=1280;height=720;",
            "output-format:",
        ] {
            let filter = Ffms2Filter::from_str(input).expect("filter should parse");
            assert_eq!(filter.to_string(), input);
        }
    }

    /// A `resize:format=...` CLI filter is how users express a conversion, so
    /// it must translate to the native equivalent rather than being
    /// dropped.
    #[test]
    fn converts_vapoursynth_resize_to_native() {
        let ten_bit = VapourSynthFilter::Resize {
            scaler: Some(Scaler::Bicubic),
            width: None,
            height: None,
            format: Some(FFPixelFormat::YUV420P10LE),
        };
        assert_eq!(
            Ffms2Filter::from_vapoursynth_filter(&ten_bit).expect("conversion"),
            Some(Ffms2Filter::OutputFormat {
                bit_depth: Some(10),
                chroma: Some(ChromaSampling::Yuv420),
                width: None,
                height: None,
            })
        );

        let downscale = VapourSynthFilter::Resize {
            scaler: Some(Scaler::Lanczos),
            width: Some(1280),
            height: Some(720),
            format: None,
        };
        assert_eq!(
            Ffms2Filter::from_vapoursynth_filter(&downscale).expect("conversion"),
            Some(Ffms2Filter::OutputFormat {
                bit_depth: None,
                chroma: None,
                width: Some(1280),
                height: Some(720),
            })
        );
    }

    #[test]
    fn eight_bit_resize_keeps_explicit_depth() {
        let eight_bit = VapourSynthFilter::Resize {
            scaler: None,
            width: None,
            height: None,
            format: Some(FFPixelFormat::YUV420P),
        };
        assert_eq!(
            Ffms2Filter::from_vapoursynth_filter(&eight_bit).expect("conversion"),
            Some(Ffms2Filter::OutputFormat {
                bit_depth: Some(8),
                chroma: Some(ChromaSampling::Yuv420),
                width: None,
                height: None,
            })
        );
    }

    #[test]
    fn unsupported_explicit_formats_are_not_native_resizes() {
        for format in [FFPixelFormat::NV12, FFPixelFormat::GBRP, FFPixelFormat::YUV440P] {
            let filter = VapourSynthFilter::Resize {
                scaler: None,
                width: Some(16),
                height: None,
                format: Some(format),
            };
            let error = Ffms2Filter::from_vapoursynth_filter(&filter)
                .expect_err("unsupported explicit format");
            assert!(
                error.to_string().contains(format.to_pix_fmt_string()),
                "{error}"
            );
        }
    }

    #[test]
    fn explicit_formats_preserve_depth_and_chroma() {
        for (format, depth, chroma) in [
            (FFPixelFormat::YUV420P, 8, ChromaSampling::Yuv420),
            (FFPixelFormat::YUV422P, 8, ChromaSampling::Yuv422),
            (FFPixelFormat::YUV444P, 8, ChromaSampling::Yuv444),
            (FFPixelFormat::GRAY8, 8, ChromaSampling::Monochrome),
            (FFPixelFormat::YUV420P10LE, 10, ChromaSampling::Yuv420),
            (FFPixelFormat::YUV422P10LE, 10, ChromaSampling::Yuv422),
            (FFPixelFormat::YUV444P10LE, 10, ChromaSampling::Yuv444),
            (FFPixelFormat::GRAY10LE, 10, ChromaSampling::Monochrome),
            (FFPixelFormat::YUV420P12LE, 12, ChromaSampling::Yuv420),
            (FFPixelFormat::YUV422P12LE, 12, ChromaSampling::Yuv422),
            (FFPixelFormat::YUV444P12LE, 12, ChromaSampling::Yuv444),
            (FFPixelFormat::GRAY12LE, 12, ChromaSampling::Monochrome),
        ] {
            let filter = VapourSynthFilter::Resize {
                scaler: None,
                width: Some(16),
                height: None,
                format: Some(format),
            };
            assert_eq!(
                Ffms2Filter::from_vapoursynth_filter(&filter).expect("supported format"),
                Some(Ffms2Filter::OutputFormat {
                    bit_depth: Some(depth),
                    chroma: Some(chroma),
                    width: Some(16),
                    height: None
                })
            );
        }
    }

    #[test]
    fn gray12_spellings_preserve_native_and_vapoursynth_intent() {
        for spelling in ["gray12l", "gray12le"] {
            let format = FFPixelFormat::from_str(spelling).expect("gray format");
            assert_eq!(
                format.to_vapoursynth_format().expect("gray preset"),
                vapoursynth::format::PresetFormat::Gray12
            );
            let filter = VapourSynthFilter::Resize {
                scaler: None,
                width: None,
                height: None,
                format: Some(format),
            };
            assert_eq!(
                Ffms2Filter::from_vapoursynth_filter(&filter).expect("native gray resize"),
                Some(Ffms2Filter::OutputFormat {
                    bit_depth: Some(12),
                    chroma: Some(ChromaSampling::Monochrome),
                    width: None,
                    height: None,
                }),
                "{spelling}"
            );
        }
        for spelling in ["gbrp12l", "gbrp12le"] {
            let filter = VapourSynthFilter::Resize {
                scaler: None,
                width: None,
                height: None,
                format: Some(FFPixelFormat::from_str(spelling).expect("RGB format")),
            };
            assert!(
                Ffms2Filter::from_vapoursynth_filter(&filter).is_err(),
                "{spelling}"
            );
        }
    }

    #[test]
    fn invalid_resize_dimensions_are_errors() {
        for width in [0, i32::MAX as usize + 1, usize::MAX] {
            let filter = VapourSynthFilter::Resize {
                scaler: None,
                width: Some(width),
                height: None,
                format: None,
            };
            assert!(
                Ffms2Filter::from_vapoursynth_filter(&filter).is_err(),
                "{width}"
            );
        }
    }

    #[test]
    fn non_resize_filters_have_no_native_equivalent() {
        let crop = VapourSynthFilter::Crop {
            top: Some(140),
            bottom: None,
            left: None,
            right: None,
        };
        assert_eq!(
            Ffms2Filter::from_vapoursynth_filter(&crop).expect("conversion"),
            None
        );
    }
}
