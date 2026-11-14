use std::{
    collections::BTreeMap,
    fmt::{Display, Write},
    str::FromStr,
};

use anyhow::{Result, bail};
use itertools::Itertools;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use vapoursynth::{core::CoreRef, node::Node};

use crate::{
    core::input::ffms2_filter::{ChromaSampling, Ffms2Filter, ffms2_pixel_format},
    ffmpeg::FFPixelFormat,
    models::encoder::photon_noise::PhotonNoise,
    vapoursynth::{
        VapourSynthError,
        plugins::{
            fgs::FGS,
            mvutensils::{
                analyse_many::AnalyseMany, degrain::Degrain, mvu_super::MVUSuper,
                recalculate::Recalculate,
            },
            rescale::{Doubler, RescaleBuilder, VSJETKernel},
            resize::{
                Scaler, bicubic::Bicubic, bilinear::Bilinear, bob::Bob, lanczos::Lanczos,
                point::Point, spline16::Spline16, spline36::Spline36, spline64::Spline64,
            },
            standard::{crop::Crop, trim::Trim},
            vszip::{bilateral::Bilateral, wnnm::WNNM},
            zoomvtools::{
                analyse::Analyse as ZoomAnalyse, degrain::Degrain as ZoomDegrainPlugin,
                recalculate::Recalculate as ZoomRecalculate, zmv_super::ZMVSuper,
            },
        },
        script_builder::{
            NodeVariableName, VapourSynthPluginScript,
            script::{Imports, Line},
        },
    },
};

/// The [`Degrain`](VapourSynthFilter::Degrain) variant is a composite filter
/// that intentionally carries many configuration fields, so the enum is larger
/// than the other variants.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum VapourSynthFilter {
    Crop {
        top: Option<usize>,
        bottom: Option<usize>,
        left: Option<usize>,
        right: Option<usize>,
    },
    Resize {
        scaler: Option<Scaler>,
        width: Option<usize>,
        height: Option<usize>,
        format: Option<FFPixelFormat>,
    },
    /// Native FFMS2 conversion intent, resolved against this filter's input node.
    /// Omitted fields survive configuration edits without opening/indexing a source.
    OutputFormat {
        bit_depth: Option<u8>,
        chroma: Option<ChromaSampling>,
        width: Option<u32>,
        height: Option<u32>,
    },
    Trim {
        start: Option<usize>,
        end: Option<usize>,
    },
    Rescale {
        kernel: VSJETKernel,
        width: usize,
        height: usize,
        doubler: Doubler,
    },
    WNNM {
        sigma: Option<Vec<f64>>,
        block_size: Option<usize>,
        block_step: Option<usize>,
        group_size: Option<usize>,
        bm_range: Option<usize>,
        radius: Option<usize>,
        ps_num: Option<usize>,
        ps_range: Option<usize>,
        residual: Option<bool>,
        adaptive_aggregation: Option<bool>,
    },
    Bilateral {
        sigma_s: Option<Vec<f64>>,
        sigma_r: Option<Vec<f64>>,
        planes: Option<Vec<bool>>,
        algorithm: Option<Vec<u32>>,
        pbficnum: Option<Vec<u32>>,
    },
    /// MVUtensils Degrain - motion-compensated temporal denoising workflow.
    /// This is a composite filter that internally chains Super -> AnalyseMany
    /// -> (optional Recalculate) -> Degrain.
    Degrain {
        blksize: Option<Vec<usize>>,
        overlap: Option<Vec<usize>>,
        pad: Option<Vec<usize>>,
        pel: Option<usize>,
        sharp: Option<usize>,
        rfilter: Option<usize>,
        radius: Option<usize>,
        search: Option<usize>,
        searchparam: Option<usize>,
        mvlambda: Option<usize>,
        chroma: Option<bool>,
        lsad: Option<usize>,
        plevel: Option<usize>,
        globalmv: Option<bool>,
        pnew: Option<usize>,
        pzero: Option<usize>,
        pglobal: Option<usize>,
        badsad: Option<usize>,
        badrange: Option<usize>,
        meander: Option<bool>,
        trymany: Option<usize>,
        satd: Option<bool>,
        recalculate: Option<bool>,
        recalc_thsad: Option<usize>,
        recalc_smooth: Option<bool>,
        recalc_blksize: Option<Vec<usize>>,
        recalc_overlap: Option<Vec<usize>>,
        thsad: Option<Vec<usize>>,
        thsad2: Option<Vec<usize>>,
        planes: Option<Vec<usize>>,
        limit: Option<Vec<f64>>,
        thscd1: Option<usize>,
        thscd2: Option<f64>,
        prefix: Option<String>,
    },
    /// ZooMVTools Degrain - motion-compensated temporal denoising workflow.
    /// This is a composite filter that internally chains ZMVSuper -> two
    /// Analyse calls (isb=0 forward, isb=1 backward) -> (optional
    /// Recalculate on both) -> Degrain1 (implemented as Degrain).
    ZoomDegrain {
        hpad: Option<usize>,
        vpad: Option<usize>,
        pel: Option<usize>,
        levels: Option<usize>,
        chroma: Option<bool>,
        sharp: Option<usize>,
        rfilter: Option<usize>,
        blksize: Option<usize>,
        blksizev: Option<usize>,
        search: Option<usize>,
        searchparam: Option<usize>,
        pelsearch: Option<usize>,
        lambda: Option<usize>,
        lsad: Option<usize>,
        plevel: Option<usize>,
        global: Option<bool>,
        pnew: Option<usize>,
        pzero: Option<usize>,
        pglobal: Option<usize>,
        overlap: Option<usize>,
        overlapv: Option<usize>,
        divide: Option<usize>,
        badsad: Option<usize>,
        badrange: Option<usize>,
        truemotion: Option<bool>,
        meander: Option<bool>,
        trymany: Option<bool>,
        fields: Option<bool>,
        tff: Option<bool>,
        search_coarse: Option<usize>,
        dct: Option<usize>,
        recalculate: Option<bool>,
        recalc_thsad: Option<usize>,
        recalc_smooth: Option<bool>,
        recalc_blksize: Option<usize>,
        recalc_blksizev: Option<usize>,
        recalc_search: Option<usize>,
        recalc_searchparam: Option<usize>,
        recalc_lambda: Option<usize>,
        recalc_overlap: Option<usize>,
        recalc_overlapv: Option<usize>,
        recalc_divide: Option<usize>,
        thsad: Option<usize>,
        thsadc: Option<usize>,
        plane: Option<i32>,
        limit: Option<usize>,
        limitc: Option<usize>,
        thscd1: Option<usize>,
        thscd2: Option<usize>,
    },
    /// FGS (Film Grain Synthesis) - applies photon-noise film grain using
    /// dav1d's grain engine. Builds a [`PhotonNoise`] from the supplied
    /// parameters and delegates to the `com.vs.fgs` plugin's `FGS` function.
    FGS {
        /// Required ISO strength for the luma grain.
        iso: u32,
        /// Optional ISO strength for chroma grain (defaults to luma `iso`).
        chroma_iso: Option<u32>,
        /// Optional custom luma AR coefficients (max 24).
        cy: Option<Vec<i8>>,
        /// Optional custom chroma-blue AR coefficients (max 25).
        ccb: Option<Vec<i8>>,
        /// Optional custom chroma-red AR coefficients (max 25).
        ccr: Option<Vec<i8>>,
        /// When `true`, randomizes the grain seed per frame for dynamic
        /// variation.
        dynamic_seed: Option<bool>,
    },
}

fn parse_value<T: FromStr>(value: &str) -> Result<T>
where
    T::Err: Display,
{
    value.parse().map_err(|error: T::Err| anyhow::anyhow!(error.to_string()))
}

fn parse_plugin_uint(value: &str) -> Result<usize> {
    // RATIONALE: These usize fields are narrowed to u32 by the plugin adapters.
    parse_value::<u32>(value).map(|value| value as usize)
}

fn parse_dimension(value: &str) -> Result<usize> {
    let dimension = parse_plugin_uint(value)?;
    anyhow::ensure!(dimension > 0, "dimension must be positive");
    Ok(dimension)
}

fn parse_rescale_dimension(value: &str) -> Result<usize> {
    // RATIONALE: Rescale emits f64 sizes, without the other adapters' u32 narrowing.
    let dimension = parse_value::<usize>(value)?;
    anyhow::ensure!(dimension > 0, "dimension must be positive");
    Ok(dimension)
}

fn parse_bool(value: &str) -> Result<bool> {
    match value {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        _ => bail!("expected true, false, 1 or 0"),
    }
}

fn parse_finite(value: &str) -> Result<f64> {
    let value = parse_value::<f64>(value)?;
    anyhow::ensure!(value.is_finite(), "expected a finite number");
    Ok(value)
}

/// Consumed fields leave only unknown keys; string values remain verbatim.
struct FilterArguments<'a> {
    filter: &'a str,
    values: BTreeMap<String, &'a str>,
}

impl FilterArguments<'_> {
    fn optional<T>(&mut self, field: &str, parse: impl Fn(&str) -> Result<T>) -> Result<Option<T>> {
        self.values
            .remove(field)
            .map(|value| {
                parse(value.trim()).map_err(|error| {
                    anyhow::anyhow!(
                        "filter {}: invalid {field} value {value:?}: {error}",
                        self.filter
                    )
                })
            })
            .transpose()
    }

    fn required<T>(&mut self, field: &str, parse: impl Fn(&str) -> Result<T>) -> Result<T> {
        self.optional(field, parse)?.ok_or_else(|| {
            anyhow::anyhow!("filter {}: missing required field {field}", self.filter)
        })
    }

    fn list<T>(
        &mut self,
        field: &str,
        parse: impl Fn(&str) -> Result<T>,
    ) -> Result<Option<Vec<T>>> {
        self.optional(field, |value| {
            value.split(',').map(|item| parse(item.trim())).collect()
        })
    }

    fn check_keys(&self, allowed: &[&str]) -> Result<()> {
        if let Some((key, value)) =
            self.values.iter().find(|(key, _)| !allowed.contains(&key.as_str()))
        {
            bail!(
                "filter {}: unknown field {key} with value {value:?}",
                self.filter
            );
        }
        Ok(())
    }
}

impl FromStr for VapourSynthFilter {
    type Err = anyhow::Error;

    #[inline]
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (variant_name, arguments) = s.split_once(':').ok_or_else(|| {
            anyhow::anyhow!("filter {s:?}: expected name:key=value; (missing ':')")
        })?;
        anyhow::ensure!(!variant_name.is_empty(), "filter name must not be empty");
        let mut args = FilterArguments {
            filter: variant_name,
            values: BTreeMap::new(),
        };
        for argument in arguments.split(';').filter(|argument| !argument.trim().is_empty()) {
            let (key, value) = argument.split_once('=').ok_or_else(|| {
                anyhow::anyhow!("filter {variant_name}: argument {argument:?} must be key=value")
            })?;
            let key = key.trim().to_lowercase();
            anyhow::ensure!(
                !key.is_empty(),
                "filter {variant_name}: empty argument key in {argument:?}"
            );
            anyhow::ensure!(
                args.values.insert(key.clone(), value).is_none(),
                "filter {variant_name}: duplicate field {key} with value {value:?}"
            );
        }
        let filter = match variant_name {
            "output-format" | "output_format" => {
                let bit_depth = args.optional("bit_depth", |value| {
                    let depth = parse_value::<u8>(value)?;
                    anyhow::ensure!(matches!(depth, 8 | 10 | 12), "expected 8, 10 or 12");
                    Ok(depth)
                })?;
                let chroma = args.optional("chroma", parse_value)?;
                let width = args.optional("width", parse_value)?;
                let height = args.optional("height", parse_value)?;
                Ffms2Filter::OutputFormat {
                    bit_depth,
                    chroma,
                    width,
                    height,
                }
                .validate()
                .map_err(|error| anyhow::anyhow!("filter {variant_name}: {error}"))?;
                Self::OutputFormat {
                    bit_depth,
                    chroma,
                    width,
                    height,
                }
            },
            "crop" => Self::Crop {
                top: args.optional("top", parse_plugin_uint)?,
                bottom: args.optional("bottom", parse_plugin_uint)?,
                left: args.optional("left", parse_plugin_uint)?,
                right: args.optional("right", parse_plugin_uint)?,
            },
            "resize" => Self::Resize {
                scaler: args.optional("scaler", parse_value)?,
                width: args.optional("width", parse_dimension)?,
                height: args.optional("height", parse_dimension)?,
                format: args.optional("format", parse_value)?,
            },
            "trim" => {
                let start = args.optional("start", parse_plugin_uint)?;
                let end = args.optional("end", parse_plugin_uint)?;
                if let (Some(start), Some(end)) = (start, end) {
                    anyhow::ensure!(
                        end >= start,
                        "filter trim: end value {end} must be >= start value {start}"
                    );
                }
                Self::Trim { start, end }
            },
            "rescale" => {
                args.check_keys(&["kernel", "width", "height", "doubler"])?;
                Self::Rescale {
                    kernel: args.required("kernel", parse_value)?,
                    width: args.required("width", parse_rescale_dimension)?,
                    height: args.required("height", parse_rescale_dimension)?,
                    doubler: args.required("doubler", parse_value)?,
                }
            },
            "wnnm" => Self::WNNM {
                sigma: args.list("sigma", parse_finite)?,
                block_size: args.optional("block_size", parse_plugin_uint)?,
                block_step: args.optional("block_step", parse_plugin_uint)?,
                group_size: args.optional("group_size", parse_plugin_uint)?,
                bm_range: args.optional("bm_range", parse_plugin_uint)?,
                radius: args.optional("radius", parse_plugin_uint)?,
                ps_num: args.optional("ps_num", parse_plugin_uint)?,
                ps_range: args.optional("ps_range", parse_plugin_uint)?,
                residual: args.optional("residual", parse_bool)?,
                adaptive_aggregation: args.optional("adaptive_aggregation", parse_bool)?,
            },
            "bilateral" => Self::Bilateral {
                sigma_s: args.list("sigma_s", parse_finite)?,
                sigma_r: args.list("sigma_r", parse_finite)?,
                planes: args.list("planes", parse_bool)?,
                // RATIONALE: vszip documents algorithm 0..=2 and PBFICnum 2..=256.
                algorithm: args.list("algorithm", |value| {
                    let value = parse_value::<u32>(value)?;
                    anyhow::ensure!(value <= 2, "expected algorithm 0, 1 or 2");
                    Ok(value)
                })?,
                pbficnum: args.list("pbficnum", |value| {
                    let value = parse_value::<u32>(value)?;
                    anyhow::ensure!((2..=256).contains(&value), "expected PBFICnum in 2..=256");
                    Ok(value)
                })?,
            },
            "degrain" => Self::Degrain {
                blksize: args.list("blksize", parse_plugin_uint)?,
                overlap: args.list("overlap", parse_plugin_uint)?,
                pad: args.list("pad", parse_plugin_uint)?,
                pel: args.optional("pel", parse_plugin_uint)?,
                sharp: args.optional("sharp", parse_plugin_uint)?,
                rfilter: args.optional("rfilter", parse_plugin_uint)?,
                radius: args.optional("radius", parse_plugin_uint)?,
                search: args.optional("search", parse_plugin_uint)?,
                searchparam: args.optional("searchparam", parse_plugin_uint)?,
                mvlambda: args.optional("mvlambda", parse_plugin_uint)?,
                chroma: args.optional("chroma", parse_bool)?,
                lsad: args.optional("lsad", parse_plugin_uint)?,
                plevel: args.optional("plevel", parse_plugin_uint)?,
                globalmv: args.optional("globalmv", parse_bool)?,
                pnew: args.optional("pnew", parse_plugin_uint)?,
                pzero: args.optional("pzero", parse_plugin_uint)?,
                pglobal: args.optional("pglobal", parse_plugin_uint)?,
                badsad: args.optional("badsad", parse_plugin_uint)?,
                badrange: args.optional("badrange", parse_plugin_uint)?,
                meander: args.optional("meander", parse_bool)?,
                trymany: args.optional("trymany", parse_plugin_uint)?,
                satd: args.optional("satd", parse_bool)?,
                recalculate: args.optional("recalculate", parse_bool)?,
                recalc_thsad: args.optional("recalc_thsad", parse_plugin_uint)?,
                recalc_smooth: args.optional("recalc_smooth", parse_bool)?,
                recalc_blksize: args.list("recalc_blksize", parse_plugin_uint)?,
                recalc_overlap: args.list("recalc_overlap", parse_plugin_uint)?,
                thsad: args.list("thsad", parse_plugin_uint)?,
                thsad2: args.list("thsad2", parse_plugin_uint)?,
                planes: args.list("planes", parse_plugin_uint)?,
                limit: args.list("limit", parse_finite)?,
                thscd1: args.optional("thscd1", parse_plugin_uint)?,
                thscd2: args.optional("thscd2", parse_finite)?,
                prefix: args.values.remove("prefix").map(str::to_owned),
            },
            "zoom_degrain" => Self::ZoomDegrain {
                hpad: args.optional("hpad", parse_plugin_uint)?,
                vpad: args.optional("vpad", parse_plugin_uint)?,
                pel: args.optional("pel", parse_plugin_uint)?,
                levels: args.optional("levels", parse_plugin_uint)?,
                chroma: args.optional("chroma", parse_bool)?,
                sharp: args.optional("sharp", parse_plugin_uint)?,
                rfilter: args.optional("rfilter", parse_plugin_uint)?,
                blksize: args.optional("blksize", parse_plugin_uint)?,
                blksizev: args.optional("blksizev", parse_plugin_uint)?,
                search: args.optional("search", parse_plugin_uint)?,
                searchparam: args.optional("searchparam", parse_plugin_uint)?,
                pelsearch: args.optional("pelsearch", parse_plugin_uint)?,
                lambda: args.optional("lambda", parse_plugin_uint)?,
                lsad: args.optional("lsad", parse_plugin_uint)?,
                plevel: args.optional("plevel", parse_plugin_uint)?,
                global: args.optional("global", parse_bool)?,
                pnew: args.optional("pnew", parse_plugin_uint)?,
                pzero: args.optional("pzero", parse_plugin_uint)?,
                pglobal: args.optional("pglobal", parse_plugin_uint)?,
                overlap: args.optional("overlap", parse_plugin_uint)?,
                overlapv: args.optional("overlapv", parse_plugin_uint)?,
                divide: args.optional("divide", parse_plugin_uint)?,
                badsad: args.optional("badsad", parse_plugin_uint)?,
                badrange: args.optional("badrange", parse_plugin_uint)?,
                truemotion: args.optional("truemotion", parse_bool)?,
                meander: args.optional("meander", parse_bool)?,
                trymany: args.optional("trymany", parse_bool)?,
                fields: args.optional("fields", parse_bool)?,
                tff: args.optional("tff", parse_bool)?,
                search_coarse: args.optional("search_coarse", parse_plugin_uint)?,
                dct: args.optional("dct", parse_plugin_uint)?,
                recalculate: args.optional("recalculate", parse_bool)?,
                recalc_thsad: args.optional("recalc_thsad", parse_plugin_uint)?,
                recalc_smooth: args.optional("recalc_smooth", parse_bool)?,
                recalc_blksize: args.optional("recalc_blksize", parse_plugin_uint)?,
                recalc_blksizev: args.optional("recalc_blksizev", parse_plugin_uint)?,
                recalc_search: args.optional("recalc_search", parse_plugin_uint)?,
                recalc_searchparam: args.optional("recalc_searchparam", parse_plugin_uint)?,
                recalc_lambda: args.optional("recalc_lambda", parse_plugin_uint)?,
                recalc_overlap: args.optional("recalc_overlap", parse_plugin_uint)?,
                recalc_overlapv: args.optional("recalc_overlapv", parse_plugin_uint)?,
                recalc_divide: args.optional("recalc_divide", parse_plugin_uint)?,
                thsad: args.optional("thsad", parse_plugin_uint)?,
                thsadc: args.optional("thsadc", parse_plugin_uint)?,
                plane: args.optional("plane", |value| {
                    let plane = parse_value::<i32>(value)?;
                    // RATIONALE: Native PlaneSelection uses 0=Y, 1=U, 2=V, 3=UV, 4=all.
                    anyhow::ensure!((0..=4).contains(&plane), "expected plane in 0..=4");
                    Ok(plane)
                })?,
                limit: args.optional("limit", parse_plugin_uint)?,
                limitc: args.optional("limitc", parse_plugin_uint)?,
                thscd1: args.optional("thscd1", parse_plugin_uint)?,
                thscd2: args.optional("thscd2", parse_plugin_uint)?,
            },
            "fgs" => {
                args.check_keys(&["iso", "chroma_iso", "cy", "ccb", "ccr", "dynamic_seed"])?;
                let iso = args.required("iso", parse_value)?;
                let chroma_iso = args.optional("chroma_iso", parse_value)?;
                let mut coefficients = |field, maximum| {
                    args.optional(field, |value| {
                        let values = value
                            .split(',')
                            .map(|item| parse_value::<i8>(item.trim()))
                            .collect::<Result<Vec<_>>>()?;
                        anyhow::ensure!(
                            values.len() <= maximum,
                            "expected at most {maximum} coefficients"
                        );
                        Ok(values)
                    })
                };
                let cy = coefficients("cy", 24)?;
                let ccb = coefficients("ccb", 25)?;
                let ccr = coefficients("ccr", 25)?;
                Self::FGS {
                    iso,
                    chroma_iso,
                    cy,
                    ccb,
                    ccr,
                    dynamic_seed: args.optional("dynamic_seed", parse_bool)?,
                }
            },
            _ => bail!("Invalid filter name: {variant_name}"),
        };
        args.check_keys(&[])?;
        Ok(filter)
    }
}

impl Display for VapourSynthFilter {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            VapourSynthFilter::OutputFormat {
                bit_depth,
                chroma,
                width,
                height,
            } => Ffms2Filter::OutputFormat {
                bit_depth: *bit_depth,
                chroma: *chroma,
                width: *width,
                height: *height,
            }
            .to_string(),
            VapourSynthFilter::Crop {
                top,
                bottom,
                left,
                right,
            } => format!(
                "crop:{}{}{}{}",
                top.map(|v| format!("top={};", v)).unwrap_or_default(),
                bottom.map(|v| format!("bottom={};", v)).unwrap_or_default(),
                left.map(|v| format!("left={};", v)).unwrap_or_default(),
                right.map(|v| format!("right={};", v)).unwrap_or_default()
            ),
            VapourSynthFilter::Resize {
                scaler,
                width,
                height,
                format,
            } => format!(
                "resize:{}{}{}{}",
                scaler.map(|v| format!("scaler={};", v)).unwrap_or_default(),
                width.map(|v| format!("width={};", v)).unwrap_or_default(),
                height.map(|v| format!("height={};", v)).unwrap_or_default(),
                format.map(|v| format!("format={};", v.to_pix_fmt_string())).unwrap_or_default(),
            ),
            VapourSynthFilter::Trim { start, end } => format!(
                "trim:{}{}",
                start.map(|v| format!("start={};", v)).unwrap_or_default(),
                end.map(|v| format!("end={};", v)).unwrap_or_default()
            ),
            VapourSynthFilter::Rescale {
                kernel,
                width,
                height,
                doubler,
            } => format!(
                "rescale:kernel={};width={};height={};doubler={};",
                kernel, width, height, doubler
            ),
            VapourSynthFilter::WNNM {
                sigma,
                block_size,
                block_step,
                group_size,
                bm_range,
                radius,
                ps_num,
                ps_range,
                residual,
                adaptive_aggregation,
            } => format!(
                "wnnm:{}{}{}{}{}{}{}{}{}{}",
                sigma
                    .as_ref()
                    .map(|v| format!("sigma={};", v.iter().join(",")))
                    .unwrap_or_default(),
                block_size.map(|v| format!("block_size={};", v)).unwrap_or_default(),
                block_step.map(|v| format!("block_step={};", v)).unwrap_or_default(),
                group_size.map(|v| format!("group_size={};", v)).unwrap_or_default(),
                bm_range.map(|v| format!("bm_range={};", v)).unwrap_or_default(),
                radius.map(|v| format!("radius={};", v)).unwrap_or_default(),
                ps_num.map(|v| format!("ps_num={};", v)).unwrap_or_default(),
                ps_range.map(|v| format!("ps_range={};", v)).unwrap_or_default(),
                residual.map(|v| format!("residual={};", v)).unwrap_or_default(),
                adaptive_aggregation
                    .map(|v| format!("adaptive_aggregation={};", v))
                    .unwrap_or_default()
            ),
            VapourSynthFilter::Bilateral {
                sigma_s,
                sigma_r,
                planes,
                algorithm,
                pbficnum,
            } => format!(
                "bilateral:{}{}{}{}{}",
                sigma_s
                    .as_ref()
                    .map(|v| format!("sigma_s={};", v.iter().join(",")))
                    .unwrap_or_default(),
                sigma_r
                    .as_ref()
                    .map(|v| format!("sigma_r={};", v.iter().join(",")))
                    .unwrap_or_default(),
                planes
                    .as_ref()
                    .map(|v| format!("planes={};", v.iter().join(",")))
                    .unwrap_or_default(),
                algorithm
                    .as_ref()
                    .map(|v| format!("algorithm={};", v.iter().join(",")))
                    .unwrap_or_default(),
                pbficnum
                    .as_ref()
                    .map(|v| format!("pbficnum={};", v.iter().join(",")))
                    .unwrap_or_default()
            ),
            VapourSynthFilter::Degrain {
                blksize,
                overlap,
                pel,
                radius,
                search,
                mvlambda,
                chroma,
                recalculate,
                thsad,
                thsad2,
                planes,
                limit,
                thscd1,
                thscd2,
                ..
            } => {
                let mut result = String::from("degrain:");
                if let Some(v) = blksize {
                    let _ = write!(result, "blksize={};", v.iter().join(","));
                }
                if let Some(v) = overlap {
                    let _ = write!(result, "overlap={};", v.iter().join(","));
                }
                if let Some(v) = pel {
                    let _ = write!(result, "pel={};", v);
                }
                if let Some(v) = radius {
                    let _ = write!(result, "radius={};", v);
                }
                if let Some(v) = search {
                    let _ = write!(result, "search={};", v);
                }
                if let Some(v) = mvlambda {
                    let _ = write!(result, "mvlambda={};", v);
                }
                if let Some(v) = chroma {
                    let _ = write!(result, "chroma={};", v);
                }
                if let Some(v) = recalculate {
                    let _ = write!(result, "recalculate={};", v);
                }
                if let Some(v) = thsad {
                    let _ = write!(result, "thsad={};", v.iter().join(","));
                }
                if let Some(v) = thsad2 {
                    let _ = write!(result, "thsad2={};", v.iter().join(","));
                }
                if let Some(v) = planes {
                    let _ = write!(result, "planes={};", v.iter().join(","));
                }
                if let Some(v) = limit {
                    let _ = write!(result, "limit={};", v.iter().join(","));
                }
                if let Some(v) = thscd1 {
                    let _ = write!(result, "thscd1={};", v);
                }
                if let Some(v) = thscd2 {
                    let _ = write!(result, "thscd2={};", v);
                }
                result
            },
            VapourSynthFilter::ZoomDegrain {
                hpad,
                vpad,
                pel,
                levels,
                chroma,
                sharp,
                rfilter,
                blksize,
                blksizev,
                search,
                searchparam,
                pelsearch,
                lambda,
                lsad,
                plevel,
                global,
                pnew,
                pzero,
                pglobal,
                overlap,
                overlapv,
                divide,
                badsad,
                badrange,
                truemotion,
                meander,
                trymany,
                fields,
                tff,
                search_coarse,
                dct,
                recalculate,
                recalc_thsad,
                recalc_smooth,
                recalc_blksize,
                recalc_blksizev,
                recalc_search,
                recalc_searchparam,
                recalc_lambda,
                recalc_overlap,
                recalc_overlapv,
                recalc_divide,
                thsad,
                thsadc,
                plane,
                limit,
                limitc,
                thscd1,
                thscd2,
            } => {
                let mut result = String::from("zoom_degrain:");
                if let Some(v) = hpad {
                    let _ = write!(result, "hpad={};", v);
                }
                if let Some(v) = vpad {
                    let _ = write!(result, "vpad={};", v);
                }
                if let Some(v) = pel {
                    let _ = write!(result, "pel={};", v);
                }
                if let Some(v) = levels {
                    let _ = write!(result, "levels={};", v);
                }
                if let Some(v) = chroma {
                    let _ = write!(result, "chroma={};", v);
                }
                if let Some(v) = sharp {
                    let _ = write!(result, "sharp={};", v);
                }
                if let Some(v) = rfilter {
                    let _ = write!(result, "rfilter={};", v);
                }
                if let Some(v) = blksize {
                    let _ = write!(result, "blksize={};", v);
                }
                if let Some(v) = blksizev {
                    let _ = write!(result, "blksizev={};", v);
                }
                if let Some(v) = search {
                    let _ = write!(result, "search={};", v);
                }
                if let Some(v) = searchparam {
                    let _ = write!(result, "searchparam={};", v);
                }
                if let Some(v) = pelsearch {
                    let _ = write!(result, "pelsearch={};", v);
                }
                if let Some(v) = lambda {
                    let _ = write!(result, "lambda={};", v);
                }
                if let Some(v) = lsad {
                    let _ = write!(result, "lsad={};", v);
                }
                if let Some(v) = plevel {
                    let _ = write!(result, "plevel={};", v);
                }
                if let Some(v) = global {
                    let _ = write!(result, "global={};", v);
                }
                if let Some(v) = pnew {
                    let _ = write!(result, "pnew={};", v);
                }
                if let Some(v) = pzero {
                    let _ = write!(result, "pzero={};", v);
                }
                if let Some(v) = pglobal {
                    let _ = write!(result, "pglobal={};", v);
                }
                if let Some(v) = overlap {
                    let _ = write!(result, "overlap={};", v);
                }
                if let Some(v) = overlapv {
                    let _ = write!(result, "overlapv={};", v);
                }
                if let Some(v) = divide {
                    let _ = write!(result, "divide={};", v);
                }
                if let Some(v) = badsad {
                    let _ = write!(result, "badsad={};", v);
                }
                if let Some(v) = badrange {
                    let _ = write!(result, "badrange={};", v);
                }
                if let Some(v) = truemotion {
                    let _ = write!(result, "truemotion={};", v);
                }
                if let Some(v) = meander {
                    let _ = write!(result, "meander={};", v);
                }
                if let Some(v) = trymany {
                    let _ = write!(result, "trymany={};", v);
                }
                if let Some(v) = fields {
                    let _ = write!(result, "fields={};", v);
                }
                if let Some(v) = tff {
                    let _ = write!(result, "tff={};", v);
                }
                if let Some(v) = search_coarse {
                    let _ = write!(result, "search_coarse={};", v);
                }
                if let Some(v) = dct {
                    let _ = write!(result, "dct={};", v);
                }
                if let Some(v) = recalculate {
                    let _ = write!(result, "recalculate={};", v);
                }
                if let Some(v) = recalc_thsad {
                    let _ = write!(result, "recalc_thsad={};", v);
                }
                if let Some(v) = recalc_smooth {
                    let _ = write!(result, "recalc_smooth={};", v);
                }
                if let Some(v) = recalc_blksize {
                    let _ = write!(result, "recalc_blksize={};", v);
                }
                if let Some(v) = recalc_blksizev {
                    let _ = write!(result, "recalc_blksizev={};", v);
                }
                if let Some(v) = recalc_search {
                    let _ = write!(result, "recalc_search={};", v);
                }
                if let Some(v) = recalc_searchparam {
                    let _ = write!(result, "recalc_searchparam={};", v);
                }
                if let Some(v) = recalc_lambda {
                    let _ = write!(result, "recalc_lambda={};", v);
                }
                if let Some(v) = recalc_overlap {
                    let _ = write!(result, "recalc_overlap={};", v);
                }
                if let Some(v) = recalc_overlapv {
                    let _ = write!(result, "recalc_overlapv={};", v);
                }
                if let Some(v) = recalc_divide {
                    let _ = write!(result, "recalc_divide={};", v);
                }
                if let Some(v) = thsad {
                    let _ = write!(result, "thsad={};", v);
                }
                if let Some(v) = thsadc {
                    let _ = write!(result, "thsadc={};", v);
                }
                if let Some(v) = plane {
                    let _ = write!(result, "plane={};", v);
                }
                if let Some(v) = limit {
                    let _ = write!(result, "limit={};", v);
                }
                if let Some(v) = limitc {
                    let _ = write!(result, "limitc={};", v);
                }
                if let Some(v) = thscd1 {
                    let _ = write!(result, "thscd1={};", v);
                }
                if let Some(v) = thscd2 {
                    let _ = write!(result, "thscd2={};", v);
                }
                result
            },
            VapourSynthFilter::FGS {
                iso,
                chroma_iso,
                cy,
                ccb,
                ccr,
                dynamic_seed,
            } => {
                let mut result = String::from("fgs:");
                let _ = write!(result, "iso={};", iso);
                if let Some(v) = chroma_iso {
                    let _ = write!(result, "chroma_iso={};", v);
                }
                if let Some(v) = cy {
                    let _ = write!(result, "cy={};", v.iter().join(","));
                }
                if let Some(v) = ccb {
                    let _ = write!(result, "ccb={};", v.iter().join(","));
                }
                if let Some(v) = ccr {
                    let _ = write!(result, "ccr={};", v.iter().join(","));
                }
                if let Some(v) = dynamic_seed {
                    let _ = write!(result, "dynamic_seed={};", v);
                }
                result
            },
        };
        write!(f, "{s}")
    }
}

impl VapourSynthFilter {
    #[inline]
    pub fn is_script_only(&self) -> bool {
        matches!(self, VapourSynthFilter::Rescale { .. })
    }

    #[inline]
    pub fn invoke_plugin_function<'core>(
        &self,
        core: CoreRef<'core>,
        node: &Node<'core>,
    ) -> Result<Node<'core>> {
        if let Self::OutputFormat {
            bit_depth,
            chroma,
            width,
            height,
        } = self
        {
            use vapoursynth::format::{ColorFamily, SampleType};
            Ffms2Filter::OutputFormat {
                bit_depth: *bit_depth,
                chroma: *chroma,
                width: *width,
                height: *height,
            }
            .validate()?;
            let info = node.info();
            let source = info.format;
            let source_chroma = match (
                source.color_family(),
                source.sub_sampling_w(),
                source.sub_sampling_h(),
            ) {
                (ColorFamily::Gray, 0, 0) => ChromaSampling::Monochrome,
                (ColorFamily::YUV, 1, 1) => ChromaSampling::Yuv420,
                (ColorFamily::YUV, 1, 0) => ChromaSampling::Yuv422,
                (ColorFamily::YUV, 0, 0) => ChromaSampling::Yuv444,
                _ => bail!("Unsupported FFMS2 source format: {}", source.name()),
            };
            anyhow::ensure!(
                source.sample_type() == SampleType::Integer
                    && matches!(source.bits_per_sample(), 8 | 10 | 12),
                "Unsupported FFMS2 source format: {}",
                source.name()
            );
            let (source_width, source_height) = super::get_resolution(&info)?;
            anyhow::ensure!(
                source_width > 0
                    && source_height > 0
                    && i32::try_from(source_width).is_ok()
                    && i32::try_from(source_height).is_ok(),
                "FFMS2 source dimensions must be positive and fit i32"
            );
            let depth = bit_depth.unwrap_or(source.bits_per_sample());
            let chroma = chroma.unwrap_or(source_chroma);
            let format = ffms2_pixel_format(usize::from(depth), chroma.to_chroma_subsampling())
                .ok_or_else(|| {
                    anyhow::anyhow!("Unsupported FFMS2 output format: {depth}-bit {chroma}")
                })?;
            return Self::Resize {
                scaler: Some(Scaler::Bicubic),
                width: width.map(|v| v as usize),
                height: height.map(|v| v as usize),
                format: Some(format),
            }
            .invoke_plugin_function(core, node);
        }
        if self.is_script_only() {
            bail!("Cannot invoke script-only filter");
        }

        self.invoke_plugin_function_impl(core, node)
            .map_err(|e: VapourSynthError| anyhow::anyhow!(e))
    }

    fn invoke_plugin_function_impl<'core>(
        &self,
        core: CoreRef<'core>,
        node: &Node<'core>,
    ) -> Result<Node<'core>, VapourSynthError> {
        match self {
            VapourSynthFilter::OutputFormat { .. } => {
                unreachable!("resolved before plugin invocation")
            },
            VapourSynthFilter::Crop {
                top,
                bottom,
                left,
                right,
            } => {
                let plugin = Crop {
                    top: top.map(|v| v as u32),
                    bottom: bottom.map(|v| v as u32),
                    left: left.map(|v| v as u32),
                    right: right.map(|v| v as u32),
                };

                Ok(plugin.invoke(core, node)?)
            },
            VapourSynthFilter::Resize {
                scaler,
                width,
                height,
                format,
            } => {
                let scaler = scaler.unwrap_or(Scaler::Bicubic);
                let width = width.map(|v| v as u32);
                let height = height.map(|v| v as u32);
                let format = if let Some(format) = format {
                    Some(format.to_vapoursynth_format().map_err(|e| {
                        VapourSynthError::PluginFunctionError {
                            plugin: "Resize".to_owned(),
                            function: "Resize".to_owned(),
                            message: e.to_string(),
                        }
                    })?)
                } else {
                    None
                };

                let node = match scaler {
                    Scaler::Bicubic => Bicubic {
                        width,
                        height,
                        format,
                        ..Default::default()
                    }
                    .invoke(core, node)?,
                    Scaler::Bilinear => Bilinear {
                        width,
                        height,
                        format,
                        ..Default::default()
                    }
                    .invoke(core, node)?,
                    Scaler::Bob => Bob {
                        width,
                        height,
                        format,
                        ..Default::default()
                    }
                    .invoke(core, node)?,
                    Scaler::Lanczos => Lanczos {
                        width,
                        height,
                        format,
                        ..Default::default()
                    }
                    .invoke(core, node)?,
                    Scaler::Point => Point {
                        width,
                        height,
                        format,
                        ..Default::default()
                    }
                    .invoke(core, node)?,
                    Scaler::Spline16 => Spline16 {
                        width,
                        height,
                        format,
                        ..Default::default()
                    }
                    .invoke(core, node)?,
                    Scaler::Spline36 => Spline36 {
                        width,
                        height,
                        format,
                        ..Default::default()
                    }
                    .invoke(core, node)?,
                    Scaler::Spline64 => Spline64 {
                        width,
                        height,
                        format,
                        ..Default::default()
                    }
                    .invoke(core, node)?,
                };

                Ok(node)
            },
            VapourSynthFilter::Trim { start, end } => {
                let plugin = Trim {
                    first: start.map(|v| v as u32),
                    last: end.map(|v| v as u32),
                    ..Default::default()
                };

                Ok(plugin.invoke(core, node)?)
            },
            VapourSynthFilter::Rescale { .. } => unreachable!(),
            VapourSynthFilter::WNNM {
                sigma,
                block_size,
                block_step,
                group_size,
                bm_range,
                radius,
                ps_num,
                ps_range,
                residual,
                adaptive_aggregation,
            } => {
                let plugin = WNNM {
                    sigma: sigma.clone(),
                    block_size: block_size.map(|v| v as u32),
                    block_step: block_step.map(|v| v as u32),
                    group_size: group_size.map(|v| v as u32),
                    bm_range: bm_range.map(|v| v as u32),
                    radius: radius.map(|v| v as u32),
                    ps_num: ps_num.map(|v| v as u32),
                    ps_range: ps_range.map(|v| v as u32),
                    residual: *residual,
                    adaptive_aggregation: *adaptive_aggregation,
                    rclip_name: None,
                };

                Ok(plugin.invoke(core, node, None)?)
            },
            VapourSynthFilter::Bilateral {
                sigma_s,
                sigma_r,
                planes,
                algorithm,
                pbficnum,
            } => {
                let plugin = Bilateral {
                    sigma_s: sigma_s.clone(),
                    sigma_r: sigma_r.clone(),
                    planes: planes.clone(),
                    algorithm: algorithm.clone(),
                    pbficnum: pbficnum.clone(),
                    ref_name: None,
                };

                Ok(plugin.invoke(core, node, None)?)
            },
            VapourSynthFilter::Degrain {
                blksize,
                overlap,
                pad,
                pel,
                sharp,
                rfilter,
                radius,
                search,
                searchparam,
                mvlambda,
                chroma,
                lsad,
                plevel,
                globalmv,
                pnew,
                pzero,
                pglobal,
                badsad,
                badrange,
                meander,
                trymany,
                satd,
                recalculate,
                recalc_thsad,
                recalc_smooth,
                recalc_blksize,
                recalc_overlap,
                thsad,
                thsad2,
                planes,
                limit,
                thscd1,
                thscd2,
                prefix,
            } => {
                // Step 1: Create super clip
                let super_plugin = MVUSuper {
                    blksize: blksize.clone().map(|v| v.into_iter().map(|u| u as u32).collect()),
                    overlap: overlap.clone().map(|v| v.into_iter().map(|u| u as u32).collect()),
                    pad: pad.clone().map(|v| v.into_iter().map(|u| u as u32).collect()),
                    pel: pel.map(|v| v as u32),
                    sharp: sharp.map(|v| v as u32),
                    rfilter: rfilter.map(|v| v as u32),
                    onelevel: Some(false),
                    pelclip_name: None,
                    prefix: prefix.clone(),
                };
                let super_node = super_plugin.invoke(core, node, None)?;

                // Step 2: AnalyseMany to produce vector clips
                let analyse_many_plugin = AnalyseMany {
                    super_name: None,
                    blksize: blksize.clone().map(|v| v.into_iter().map(|u| u as u32).collect()),
                    overlap: overlap.clone().map(|v| v.into_iter().map(|u| u as u32).collect()),
                    levels: None,
                    search: search.map(|v| v as u32),
                    searchparam: searchparam.map(|v| v as u32),
                    pelsearch: None,
                    mvlambda: mvlambda.map(|v| v as u32),
                    chroma: *chroma,
                    delta: Some(1),
                    lsad: lsad.map(|v| v as u32),
                    plevel: plevel.map(|v| v as u32),
                    globalmv: *globalmv,
                    pnew: pnew.map(|v| v as u32),
                    pzero: pzero.map(|v| v as u32),
                    pglobal: pglobal.map(|v| v as u32),
                    badsad: badsad.map(|v| v as u32),
                    badrange: badrange.map(|v| v as u32),
                    meander: *meander,
                    trymany: trymany.map(|v| v as u32),
                    fields: Some(false),
                    tff: Some(false),
                    satd: *satd,
                    radius: radius.map(|v| v as u32),
                    prefix: prefix.clone(),
                };
                let vectors_node = analyse_many_plugin.invoke(core, &super_node)?;

                // Step 3: Optional Recalculate
                let vectors_node = if recalculate.unwrap_or(false) {
                    let recalc_plugin = Recalculate {
                        super_name: None,
                        vectors_name: None,
                        thsad: recalc_thsad.map(|v| v as u32),
                        smooth: *recalc_smooth,
                        blksize: recalc_blksize
                            .clone()
                            .map(|v| v.into_iter().map(|u| u as u32).collect()),
                        overlap: recalc_overlap
                            .clone()
                            .map(|v| v.into_iter().map(|u| u as u32).collect()),
                        search: None,
                        searchparam: None,
                        mvlambda: None,
                        chroma: *chroma,
                        pnew: None,
                        meander: None,
                        fields: None,
                        tff: None,
                        satd: None,
                        prefix: prefix.clone(),
                    };
                    recalc_plugin.invoke(core, &super_node, &vectors_node)?
                } else {
                    vectors_node
                };

                // Step 4: Degrain
                let degrain_plugin = Degrain {
                    clip_name: None,
                    super_name: None,
                    vectors_name: None,
                    thsad: thsad.clone().map(|v| v.into_iter().map(|u| u as u32).collect()),
                    thsad2: thsad2.clone().map(|v| v.into_iter().map(|u| u as u32).collect()),
                    planes: planes.clone().map(|v| v.into_iter().map(|u| u as u32).collect()),
                    limit: limit.clone(),
                    thscd1: thscd1.map(|v| v as u32),
                    thscd2: *thscd2,
                    weights: None,
                    prefix: prefix.clone(),
                };
                degrain_plugin.invoke(core, node, &super_node, &vectors_node)
            },
            VapourSynthFilter::ZoomDegrain {
                hpad,
                vpad,
                pel,
                levels,
                chroma,
                sharp,
                rfilter,
                blksize,
                blksizev,
                search,
                searchparam,
                pelsearch,
                lambda,
                lsad,
                plevel,
                global,
                pnew,
                pzero,
                pglobal,
                overlap,
                overlapv,
                divide,
                badsad,
                badrange,
                truemotion,
                meander,
                trymany,
                fields,
                tff,
                search_coarse,
                dct,
                recalculate,
                recalc_thsad,
                recalc_smooth,
                recalc_blksize,
                recalc_blksizev,
                recalc_search,
                recalc_searchparam,
                recalc_lambda,
                recalc_overlap,
                recalc_overlapv,
                recalc_divide,
                thsad,
                thsadc,
                plane,
                limit,
                limitc,
                thscd1,
                thscd2,
            } => {
                // Step 1: Create super clip
                let super_plugin = ZMVSuper {
                    hpad: hpad.map(|v| v as u32),
                    vpad: vpad.map(|v| v as u32),
                    pel: pel.map(|v| v as u32),
                    levels: levels.map(|v| v as u32),
                    chroma: *chroma,
                    sharp: sharp.map(|v| v as u32),
                    rfilter: rfilter.map(|v| v as u32),
                    pelclip_name: None,
                    opt: Some(4),
                };
                let super_node = super_plugin.invoke(core, node, None)?;

                // Step 2: Analyse forward (isb=0) and backward (isb=1)
                let analyse_forward = ZoomAnalyse {
                    super_clip_name: None,
                    blksize: blksize.map(|v| v as u32),
                    blksizev: blksizev.map(|v| v as u32),
                    levels: levels.map(|v| v as u32),
                    search: search.map(|v| v as u32),
                    searchparam: searchparam.map(|v| v as u32),
                    pelsearch: pelsearch.map(|v| v as u32),
                    isb: Some(0),
                    lambda: lambda.map(|v| v as u32),
                    chroma: *chroma,
                    delta: Some(-1),
                    truemotion: *truemotion,
                    lsad: lsad.map(|v| v as u32),
                    plevel: plevel.map(|v| v as u32),
                    global: *global,
                    pnew: pnew.map(|v| v as u32),
                    pzero: pzero.map(|v| v as u32),
                    pglobal: pglobal.map(|v| v as u32),
                    overlap: overlap.map(|v| v as u32),
                    overlapv: overlapv.map(|v| v as u32),
                    divide: divide.map(|v| v as u32),
                    badsad: badsad.map(|v| v as u32),
                    badrange: badrange.map(|v| v as u32),
                    opt: Some(4),
                    meander: *meander,
                    trymany: *trymany,
                    fields: *fields,
                    tff: *tff,
                    search_coarse: search_coarse.map(|v| v as u32),
                    dct: dct.map(|v| v as u32),
                };
                let mvfw_node = analyse_forward.clone().invoke(core, &super_node)?;

                let mut analyse_backward = analyse_forward;
                analyse_backward.isb = Some(1);
                analyse_backward.delta = Some(1);
                let mvbw_node = analyse_backward.invoke(core, &super_node)?;

                // Step 3: Optional Recalculate on both vector clips
                let (mvbw_node, mvfw_node) = if recalculate.unwrap_or(false) {
                    let recalc_plugin = ZoomRecalculate {
                        super_clip_name: None,
                        vectors_name: None,
                        thsad: recalc_thsad.map(|v| v as u32),
                        smooth: *recalc_smooth,
                        blksize: recalc_blksize.map(|v| v as u32),
                        blksizev: recalc_blksizev.map(|v| v as u32),
                        search: recalc_search.map(|v| v as u32),
                        searchparam: recalc_searchparam.map(|v| v as u32),
                        lambda: recalc_lambda.map(|v| v as u32),
                        chroma: *chroma,
                        truemotion: Some(true),
                        pnew: Some(25),
                        overlap: recalc_overlap.map(|v| v as u32),
                        overlapv: recalc_overlapv.map(|v| v as u32),
                        divide: recalc_divide.map(|v| v as u32),
                        opt: Some(4),
                        meander: Some(true),
                        fields: *fields,
                        tff: *tff,
                        dct: Some(0),
                    };
                    let mvbw_node = recalc_plugin.clone().invoke(core, &super_node, &mvbw_node)?;
                    let mvfw_node = recalc_plugin.invoke(core, &super_node, &mvfw_node)?;
                    (mvbw_node, mvfw_node)
                } else {
                    (mvbw_node, mvfw_node)
                };

                // Step 4: Degrain1
                let degrain_plugin = ZoomDegrainPlugin {
                    clip_name: None,
                    super_clip_name: None,
                    mvbw_name: None,
                    mvfw_name: None,
                    thsad: thsad.map(|v| v as u32),
                    thsadc: thsadc.map(|v| v as u32),
                    plane: *plane,
                    limit: limit.map(|v| v as u32),
                    limitc: limitc.map(|v| v as u32),
                    thscd1: thscd1.map(|v| v as u32),
                    thscd2: thscd2.map(|v| v as u32),
                    opt: Some(4),
                };
                degrain_plugin.invoke(core, node, &super_node, &mvbw_node, &mvfw_node)
            },
            VapourSynthFilter::FGS {
                iso,
                chroma_iso,
                cy,
                ccb,
                ccr,
                dynamic_seed,
            } => {
                let photon_noise = PhotonNoise {
                    iso: *iso,
                    chroma_iso: *chroma_iso,
                    width: None,
                    height: None,
                    c_y: cy.clone(),
                    ccb: ccb.clone(),
                    ccr: ccr.clone(),
                };
                let fgs = FGS {
                    photon_noise,
                    dynamic_seed: *dynamic_seed,
                    simd_mask: None,
                };
                fgs.invoke(core, node)
            },
        }
    }

    #[inline]
    pub fn generate_script(
        &self,
        node_name: NodeVariableName,
    ) -> Result<(Option<Imports>, Vec<Line>)> {
        let (import_lines, filter_lines) = match self {
            VapourSynthFilter::OutputFormat {
                bit_depth,
                chroma,
                width,
                height,
            } => {
                Ffms2Filter::OutputFormat {
                    bit_depth: *bit_depth,
                    chroma: *chroma,
                    width: *width,
                    height: *height,
                }
                .validate()?;
                let mut imports = Imports::new();
                imports.insert(
                    "vapoursynth".to_owned(),
                    BTreeMap::from([
                        ("GRAY".to_owned(), None),
                        ("YUV".to_owned(), None),
                        ("INTEGER".to_owned(), None),
                    ]),
                );
                let depth = bit_depth.map_or_else(
                    || "_condor_format.bits_per_sample".to_owned(),
                    |v| v.to_string(),
                );
                let (family, sub_w, sub_h) = match chroma {
                    Some(ChromaSampling::Monochrome) => ("GRAY", "0", "0"),
                    Some(ChromaSampling::Yuv420) => ("YUV", "1", "1"),
                    Some(ChromaSampling::Yuv422) => ("YUV", "1", "0"),
                    Some(ChromaSampling::Yuv444) => ("YUV", "0", "0"),
                    None => (
                        "_condor_format.color_family",
                        "_condor_format.subsampling_w",
                        "_condor_format.subsampling_h",
                    ),
                };
                let dimensions = format!(
                    "{}{}",
                    width.map(|v| format!("width={v}, ")).unwrap_or_default(),
                    height.map(|v| format!("height={v}, ")).unwrap_or_default()
                );
                (
                    Some(imports),
                    vec![Line::Term(format!(
                        "_condor_format = {node_name}.format
if (_condor_format is None or _condor_format.sample_type != INTEGER
    or _condor_format.bits_per_sample not in (8, 10, 12)
    or _condor_format.color_family not in (GRAY, YUV)
    or (_condor_format.subsampling_w, _condor_format.subsampling_h) not in ((0, 0), (1, 0), (1, 1))):
    raise ValueError(f'Unsupported FFMS2 source format: {{_condor_format}}')
if not (0 < {node_name}.width <= 2147483647 and 0 < {node_name}.height <= 2147483647):
    raise ValueError('FFMS2 source dimensions must be positive and fit i32')
{node_name} = core.resize.Bicubic(clip={node_name}, {dimensions}format=core.query_video_format({family}, INTEGER, {depth}, {sub_w}, {sub_h}).id)"
                    ))],
                )
            },
            VapourSynthFilter::Crop {
                top,
                bottom,
                left,
                right,
            } => {
                let plugin = Crop {
                    top: top.map(|v| v as u32),
                    bottom: bottom.map(|v| v as u32),
                    left: left.map(|v| v as u32),
                    right: right.map(|v| v as u32),
                };

                plugin.generate_script(node_name)?
            },
            VapourSynthFilter::Resize {
                scaler,
                width,
                height,
                format,
            } => {
                let scaler = scaler.unwrap_or(Scaler::Bicubic);
                let width = width.map(|v| v as u32);
                let height = height.map(|v| v as u32);
                let format = if let Some(format) = format {
                    Some(format.to_vapoursynth_format()?)
                } else {
                    None
                };

                match scaler {
                    Scaler::Bicubic => Bicubic {
                        width,
                        height,
                        format,
                        ..Default::default()
                    }
                    .generate_script(node_name)?,
                    Scaler::Bilinear => Bilinear {
                        width,
                        height,
                        format,
                        ..Default::default()
                    }
                    .generate_script(node_name)?,
                    Scaler::Bob => Bob {
                        width,
                        height,
                        format,
                        ..Default::default()
                    }
                    .generate_script(node_name)?,
                    Scaler::Lanczos => Lanczos {
                        width,
                        height,
                        format,
                        ..Default::default()
                    }
                    .generate_script(node_name)?,
                    Scaler::Point => Point {
                        width,
                        height,
                        format,
                        ..Default::default()
                    }
                    .generate_script(node_name)?,
                    Scaler::Spline16 => Spline16 {
                        width,
                        height,
                        format,
                        ..Default::default()
                    }
                    .generate_script(node_name)?,
                    Scaler::Spline36 => Spline36 {
                        width,
                        height,
                        format,
                        ..Default::default()
                    }
                    .generate_script(node_name)?,
                    Scaler::Spline64 => Spline64 {
                        width,
                        height,
                        format,
                        ..Default::default()
                    }
                    .generate_script(node_name)?,
                }
            },
            VapourSynthFilter::Trim { start, end } => {
                let plugin = Trim {
                    first: start.map(|v| v as u32),
                    last: end.map(|v| v as u32),
                    ..Default::default()
                };

                plugin.generate_script(node_name)?
            },
            VapourSynthFilter::Rescale {
                kernel,
                width,
                height,
                doubler,
            } => RescaleBuilder {
                descale_kernel: kernel.clone(),
                width: *width as f64,
                height: *height as f64,
                doubler: *doubler,
                ..Default::default()
            }
            .generate_script(node_name)?,
            VapourSynthFilter::WNNM {
                sigma,
                block_size,
                block_step,
                group_size,
                bm_range,
                radius,
                ps_num,
                ps_range,
                residual,
                adaptive_aggregation,
            } => {
                let plugin = WNNM {
                    sigma: sigma.clone(),
                    block_size: block_size.map(|v| v as u32),
                    block_step: block_step.map(|v| v as u32),
                    group_size: group_size.map(|v| v as u32),
                    bm_range: bm_range.map(|v| v as u32),
                    radius: radius.map(|v| v as u32),
                    ps_num: ps_num.map(|v| v as u32),
                    ps_range: ps_range.map(|v| v as u32),
                    residual: *residual,
                    adaptive_aggregation: *adaptive_aggregation,
                    rclip_name: None,
                };

                plugin.generate_script(node_name)?
            },
            VapourSynthFilter::Bilateral {
                sigma_s,
                sigma_r,
                planes,
                algorithm,
                pbficnum,
            } => {
                let plugin = Bilateral {
                    sigma_s: sigma_s.clone(),
                    sigma_r: sigma_r.clone(),
                    planes: planes.clone(),
                    algorithm: algorithm.clone(),
                    pbficnum: pbficnum.clone(),
                    ref_name: None,
                };

                plugin.generate_script(node_name)?
            },
            VapourSynthFilter::Degrain {
                blksize,
                overlap,
                pad,
                pel,
                sharp,
                rfilter,
                radius,
                search,
                searchparam,
                mvlambda,
                chroma,
                lsad,
                plevel,
                globalmv,
                pnew,
                pzero,
                pglobal,
                badsad,
                badrange,
                meander,
                trymany,
                satd,
                recalculate,
                recalc_thsad,
                recalc_smooth,
                recalc_blksize,
                recalc_overlap,
                thsad,
                thsad2,
                planes,
                limit,
                thscd1,
                thscd2,
                prefix,
            } => {
                let mut lines = vec![];

                let super_name = "mvu_super".to_string();
                let vectors_name = "mvu_vectors".to_string();

                // Step 1: Super
                let super_plugin = MVUSuper {
                    blksize: blksize.clone().map(|v| v.into_iter().map(|u| u as u32).collect()),
                    overlap: overlap.clone().map(|v| v.into_iter().map(|u| u as u32).collect()),
                    pad: pad.clone().map(|v| v.into_iter().map(|u| u as u32).collect()),
                    pel: pel.map(|v| v as u32),
                    sharp: sharp.map(|v| v as u32),
                    rfilter: rfilter.map(|v| v as u32),
                    onelevel: Some(false),
                    pelclip_name: None,
                    prefix: prefix.clone(),
                };
                let (_, super_lines) = super_plugin.generate_script(super_name.clone())?;
                lines.extend(super_lines);

                // Step 2: AnalyseMany
                let analyse_many_plugin = AnalyseMany {
                    super_name: Some(super_name.clone()),
                    blksize: blksize.clone().map(|v| v.into_iter().map(|u| u as u32).collect()),
                    overlap: overlap.clone().map(|v| v.into_iter().map(|u| u as u32).collect()),
                    levels: None,
                    search: search.map(|v| v as u32),
                    searchparam: searchparam.map(|v| v as u32),
                    pelsearch: None,
                    mvlambda: mvlambda.map(|v| v as u32),
                    chroma: *chroma,
                    delta: Some(1),
                    lsad: lsad.map(|v| v as u32),
                    plevel: plevel.map(|v| v as u32),
                    globalmv: *globalmv,
                    pnew: pnew.map(|v| v as u32),
                    pzero: pzero.map(|v| v as u32),
                    pglobal: pglobal.map(|v| v as u32),
                    badsad: badsad.map(|v| v as u32),
                    badrange: badrange.map(|v| v as u32),
                    meander: *meander,
                    trymany: trymany.map(|v| v as u32),
                    fields: Some(false),
                    tff: Some(false),
                    satd: *satd,
                    radius: radius.map(|v| v as u32),
                    prefix: prefix.clone(),
                };
                let (_, analyse_lines) =
                    analyse_many_plugin.generate_script(vectors_name.clone())?;
                lines.extend(analyse_lines);

                // Step 3: Optional Recalculate
                let final_vectors_name = if recalculate.unwrap_or(false) {
                    let recalc_name = "mvu_vectors_recalc".to_string();
                    let recalc_plugin = Recalculate {
                        super_name: Some(super_name.clone()),
                        vectors_name: Some(vectors_name),
                        thsad: recalc_thsad.map(|v| v as u32),
                        smooth: *recalc_smooth,
                        blksize: recalc_blksize
                            .clone()
                            .map(|v| v.into_iter().map(|u| u as u32).collect()),
                        overlap: recalc_overlap
                            .clone()
                            .map(|v| v.into_iter().map(|u| u as u32).collect()),
                        search: None,
                        searchparam: None,
                        mvlambda: None,
                        chroma: *chroma,
                        pnew: None,
                        meander: None,
                        fields: None,
                        tff: None,
                        satd: None,
                        prefix: prefix.clone(),
                    };
                    let (_, recalc_lines) = recalc_plugin.generate_script(recalc_name.clone())?;
                    lines.extend(recalc_lines);
                    recalc_name
                } else {
                    vectors_name
                };

                // Step 4: Degrain
                let degrain_plugin = Degrain {
                    clip_name: Some(node_name.clone()),
                    super_name: Some(super_name),
                    vectors_name: Some(final_vectors_name),
                    thsad: thsad.clone().map(|v| v.into_iter().map(|u| u as u32).collect()),
                    thsad2: thsad2.clone().map(|v| v.into_iter().map(|u| u as u32).collect()),
                    planes: planes.clone().map(|v| v.into_iter().map(|u| u as u32).collect()),
                    limit: limit.clone(),
                    thscd1: thscd1.map(|v| v as u32),
                    thscd2: *thscd2,
                    weights: None,
                    prefix: prefix.clone(),
                };
                let (_, degrain_lines) = degrain_plugin.generate_script(node_name)?;
                lines.extend(degrain_lines);

                (None, lines)
            },
            VapourSynthFilter::ZoomDegrain {
                hpad,
                vpad,
                pel,
                levels,
                chroma,
                sharp,
                rfilter,
                blksize,
                blksizev,
                search,
                searchparam,
                pelsearch,
                lambda,
                lsad,
                plevel,
                global,
                pnew,
                pzero,
                pglobal,
                overlap,
                overlapv,
                divide,
                badsad,
                badrange,
                truemotion,
                meander,
                trymany,
                fields,
                tff,
                search_coarse,
                dct,
                recalculate,
                recalc_thsad,
                recalc_smooth,
                recalc_blksize,
                recalc_blksizev,
                recalc_search,
                recalc_searchparam,
                recalc_lambda,
                recalc_overlap,
                recalc_overlapv,
                recalc_divide,
                thsad,
                thsadc,
                plane,
                limit,
                limitc,
                thscd1,
                thscd2,
            } => {
                let mut lines = vec![];

                let super_name = "zmv_super".to_string();
                let mvfw_name = "zmv_mvfw".to_string();
                let mvbw_name = "zmv_mvbw".to_string();

                // Step 1: Super
                let super_plugin = ZMVSuper {
                    hpad: hpad.map(|v| v as u32),
                    vpad: vpad.map(|v| v as u32),
                    pel: pel.map(|v| v as u32),
                    levels: levels.map(|v| v as u32),
                    chroma: *chroma,
                    sharp: sharp.map(|v| v as u32),
                    rfilter: rfilter.map(|v| v as u32),
                    pelclip_name: None,
                    opt: Some(4),
                };
                let (_, super_lines) = super_plugin.generate_script(super_name.clone())?;
                lines.extend(super_lines);

                // Step 2: Analyse forward (isb=0) and backward (isb=1)
                let analyse_plugin = ZoomAnalyse {
                    super_clip_name: Some(super_name.clone()),
                    blksize: blksize.map(|v| v as u32),
                    blksizev: blksizev.map(|v| v as u32),
                    levels: levels.map(|v| v as u32),
                    search: search.map(|v| v as u32),
                    searchparam: searchparam.map(|v| v as u32),
                    pelsearch: pelsearch.map(|v| v as u32),
                    isb: Some(0),
                    lambda: lambda.map(|v| v as u32),
                    chroma: *chroma,
                    delta: Some(-1),
                    truemotion: *truemotion,
                    lsad: lsad.map(|v| v as u32),
                    plevel: plevel.map(|v| v as u32),
                    global: *global,
                    pnew: pnew.map(|v| v as u32),
                    pzero: pzero.map(|v| v as u32),
                    pglobal: pglobal.map(|v| v as u32),
                    overlap: overlap.map(|v| v as u32),
                    overlapv: overlapv.map(|v| v as u32),
                    divide: divide.map(|v| v as u32),
                    badsad: badsad.map(|v| v as u32),
                    badrange: badrange.map(|v| v as u32),
                    opt: Some(4),
                    meander: *meander,
                    trymany: *trymany,
                    fields: *fields,
                    tff: *tff,
                    search_coarse: search_coarse.map(|v| v as u32),
                    dct: dct.map(|v| v as u32),
                };
                let (_, analyse_fw_lines) = analyse_plugin.generate_script(mvfw_name.clone())?;
                lines.extend(analyse_fw_lines);

                let mut analyse_backward = analyse_plugin;
                analyse_backward.isb = Some(1);
                analyse_backward.delta = Some(1);
                let (_, analyse_bw_lines) = analyse_backward.generate_script(mvbw_name.clone())?;
                lines.extend(analyse_bw_lines);

                // Step 3: Optional Recalculate on both vector clips
                let (final_mvbw_name, final_mvfw_name) = if recalculate.unwrap_or(false) {
                    let recalc_bw_name = "zmv_mvbw_recalc".to_string();
                    let recalc_fw_name = "zmv_mvfw_recalc".to_string();
                    let recalc_plugin = ZoomRecalculate {
                        super_clip_name: Some(super_name.clone()),
                        vectors_name: Some(mvbw_name),
                        thsad: recalc_thsad.map(|v| v as u32),
                        smooth: *recalc_smooth,
                        blksize: recalc_blksize.map(|v| v as u32),
                        blksizev: recalc_blksizev.map(|v| v as u32),
                        search: recalc_search.map(|v| v as u32),
                        searchparam: recalc_searchparam.map(|v| v as u32),
                        lambda: recalc_lambda.map(|v| v as u32),
                        chroma: *chroma,
                        truemotion: Some(true),
                        pnew: Some(25),
                        overlap: recalc_overlap.map(|v| v as u32),
                        overlapv: recalc_overlapv.map(|v| v as u32),
                        divide: recalc_divide.map(|v| v as u32),
                        opt: Some(4),
                        meander: Some(true),
                        fields: *fields,
                        tff: *tff,
                        dct: Some(0),
                    };
                    let (_, recalc_bw_lines) =
                        recalc_plugin.generate_script(recalc_bw_name.clone())?;
                    lines.extend(recalc_bw_lines);

                    let mut recalc_forward = recalc_plugin;
                    recalc_forward.vectors_name = Some(mvfw_name);
                    let (_, recalc_fw_lines) =
                        recalc_forward.generate_script(recalc_fw_name.clone())?;
                    lines.extend(recalc_fw_lines);

                    (recalc_bw_name, recalc_fw_name)
                } else {
                    (mvbw_name, mvfw_name)
                };

                // Step 4: Degrain1
                let degrain_plugin = ZoomDegrainPlugin {
                    clip_name: Some(node_name.clone()),
                    super_clip_name: Some(super_name),
                    mvbw_name: Some(final_mvbw_name),
                    mvfw_name: Some(final_mvfw_name),
                    thsad: thsad.map(|v| v as u32),
                    thsadc: thsadc.map(|v| v as u32),
                    plane: *plane,
                    limit: limit.map(|v| v as u32),
                    limitc: limitc.map(|v| v as u32),
                    thscd1: thscd1.map(|v| v as u32),
                    thscd2: thscd2.map(|v| v as u32),
                    opt: Some(4),
                };
                let (_, degrain_lines) = degrain_plugin.generate_script(node_name)?;
                lines.extend(degrain_lines);

                (None, lines)
            },
            VapourSynthFilter::FGS {
                iso,
                chroma_iso,
                cy,
                ccb,
                ccr,
                dynamic_seed,
            } => {
                let photon_noise = PhotonNoise {
                    iso: *iso,
                    chroma_iso: *chroma_iso,
                    width: None,
                    height: None,
                    c_y: cy.clone(),
                    ccb: ccb.clone(),
                    ccr: ccr.clone(),
                };
                let fgs = FGS {
                    photon_noise,
                    dynamic_seed: *dynamic_seed,
                    simd_mask: None,
                };
                fgs.generate_script(node_name)?
            },
        };

        Ok((import_lines, filter_lines))
    }

    #[inline]
    pub fn can_alter_time(&self) -> bool {
        matches!(self, VapourSynthFilter::Trim { .. })
    }
}
#[cfg(test)]
mod tests;
