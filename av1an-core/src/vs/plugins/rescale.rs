use std::collections::BTreeMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use strum::{Display, EnumString, IntoStaticStr};

use crate::vs::script_builder::{
    script::{Imports, Line, ModuleAlias, ModuleName},
    NodeVariableName,
    VapourSynthPluginScript,
};

/// Only used for script generation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RescaleBuilder {
    /// Kernel to descale with
    pub descale_kernel:   VSJETKernel,
    /// Height to descale to
    pub height:           f64,
    /// Width to descale to. Please be absolutely certain of what you're doing
    /// if you're using `get_w` for this.
    pub width:            f64,
    /// Padded height used in a "fractional" descale
    pub base_height:      Option<u32>,
    /// Padded width used in a "fractional" descale
    /// Both of these are technically optional but highly recommended to have
    /// set for float width/height.
    pub base_width:       Option<u32>,
    /// Whether to descale only height, only width, or both. "h" or "w"
    /// respectively for the former two.
    pub mode:             Option<DescaleMode>,
    /// Downscales the clip back the size of the original input clip and applies
    /// the masks, if any. Defaults to Linear Hermite.
    pub downscale_kernel: VSJETKernel,

    pub doubler: Doubler,
}

impl Default for RescaleBuilder {
    #[inline]
    fn default() -> Self {
        Self {
            width:            1280.0,
            height:           720.0,
            base_width:       None,
            base_height:      None,
            descale_kernel:   VSJETKernel::Bilinear {
                border_handling: BorderHandling::default(),
                linear:          false,
            },
            mode:             Some(DescaleMode::default()),
            doubler:          Doubler::ArtCNN(ArtCNNModel::R8F64),
            downscale_kernel: VSJETKernel::Hermite {
                linear:          true,
                border_handling: BorderHandling::Mirror,
            },
        }
    }
}

impl VapourSynthPluginScript for RescaleBuilder {
    #[inline]
    fn generate_script(&self, node_name: NodeVariableName) -> Result<(Option<Imports>, Vec<Line>)> {
        let mut imports = BTreeMap::new();
        let mut lines = vec![];

        let mut rescale_builder_modules = BTreeMap::new();
        rescale_builder_modules.insert("RescaleBuilder".to_owned(), None);
        imports.insert("vodesfunc".to_owned(), rescale_builder_modules);

        let (descale_kernel_modules, descale_kernel_command) =
            Self::kernel_details(&self.descale_kernel);
        let (downscale_kernel_modules, downscale_kernel_command) =
            Self::kernel_details(&self.downscale_kernel);
        let (doubler_modules, doubler_command) = Self::doubler_details(&self.doubler);

        let existing_vskernel_modules =
            imports.entry("vskernels".to_owned()).or_insert(BTreeMap::new());
        existing_vskernel_modules.extend(descale_kernel_modules);
        existing_vskernel_modules.extend(downscale_kernel_modules);
        let existing_vsscale_modules =
            imports.entry("vsscale".to_owned()).or_insert(BTreeMap::new());
        existing_vsscale_modules.extend(doubler_modules);

        lines.push(Line::Expression(
            "rescale_builder".to_owned(),
            format!("RescaleBuilder({})", node_name),
        ));
        let base_width_str =
            self.base_width.map_or_else(String::new, |w| format!(", base_width = {}", w));
        let base_height_str =
            self.base_height.map_or_else(String::new, |h| format!(", base_height = {}", h));
        let mode_str = self.mode.map_or_else(String::new, |m| format!(", mode = {}", m));
        lines.push(Line::Expression(
            "rescale_builder".to_owned(),
            format!(
                "rescale_builder.descale(kernel = {}{}{}, width = {}, height = {}{})",
                descale_kernel_command,
                base_width_str,
                base_height_str,
                self.width,
                self.height,
                mode_str
            ),
        ));
        lines.push(Line::Expression(
            "rescale_builder".to_owned(),
            format!("rescale_builder.double({})", doubler_command),
        ));
        lines.push(Line::Expression(
            "rescale_builder".to_owned(),
            "rescale_builder.errormask()".to_owned(),
        ));
        lines.push(Line::Expression(
            "rescale_builder".to_owned(),
            "rescale_builder.linemask()".to_owned(),
        ));
        lines.push(Line::Expression(
            "rescale_builder".to_owned(),
            format!(
                "rescale_builder.downscale(downscaler = {})",
                downscale_kernel_command
            ),
        ));
        lines.push(Line::Expression(
            "rescale_builder_final".to_owned(),
            "rescale_builder.final()".to_owned(),
        ));
        lines.push(Line::Expression(
            "rescale_builder".to_owned(),
            "rescale_builder_final[0]".to_owned(),
        ));
        lines.push(Line::Expression(
            node_name,
            "rescale_builder_final[1]".to_owned(),
        ));

        Ok((Some(imports), lines))
    }
}

impl RescaleBuilder {
    #[inline]
    pub fn kernel_details(kernel: &VSJETKernel) -> (BTreeMap<ModuleName, ModuleAlias>, String) {
        let mut modules = BTreeMap::<ModuleName, ModuleAlias>::new();

        // let kernel_name = kernel.to_string();
        let kernel_command = match kernel {
            VSJETKernel::Bicubic {
                b,
                c,
                linear,
                border_handling,
            } => {
                modules.insert(kernel.to_string(), None);
                modules.insert("BorderHandling".to_owned(), None);
                format!(
                    "{}(b = {}, c = {}, linear = {}, border_handling = BorderHandling.{})",
                    kernel,
                    b,
                    c,
                    if *linear { "True" } else { "False" },
                    border_handling
                )
            },
            VSJETKernel::BSpline {
                linear,
                border_handling,
            } => {
                modules.insert(kernel.to_string(), None);
                modules.insert("BorderHandling".to_owned(), None);
                format!(
                    "{}(linear = {}, border_handling = BorderHandling.{})",
                    kernel,
                    if *linear { "True" } else { "False" },
                    border_handling
                )
            },
            VSJETKernel::Hermite {
                linear,
                border_handling,
            } => {
                modules.insert(kernel.to_string(), None);
                modules.insert("BorderHandling".to_owned(), None);
                format!(
                    "{}(linear = {}, border_handling = BorderHandling.{})",
                    kernel,
                    if *linear { "True" } else { "False" },
                    border_handling
                )
            },
            VSJETKernel::Mitchell {
                linear,
                border_handling,
            } => {
                modules.insert(kernel.to_string(), None);
                modules.insert("BorderHandling".to_owned(), None);
                format!(
                    "{}(linear = {}, border_handling = BorderHandling.{})",
                    kernel,
                    if *linear { "True" } else { "False" },
                    border_handling
                )
            },
            VSJETKernel::Catrom {
                linear,
                border_handling,
            } => {
                modules.insert(kernel.to_string(), None);
                modules.insert("BorderHandling".to_owned(), None);
                format!(
                    "{}(linear = {}, border_handling = BorderHandling.{})",
                    kernel,
                    if *linear { "True" } else { "False" },
                    border_handling
                )
            },
            VSJETKernel::FFmpegBicubic {
                linear,
                border_handling,
            } => {
                modules.insert(kernel.to_string(), None);
                modules.insert("BorderHandling".to_owned(), None);
                format!(
                    "{}(linear = {}, border_handling = BorderHandling.{})",
                    kernel,
                    if *linear { "True" } else { "False" },
                    border_handling
                )
            },
            VSJETKernel::AdobeBicubic {
                linear,
                border_handling,
            } => {
                modules.insert(kernel.to_string(), None);
                modules.insert("BorderHandling".to_owned(), None);
                format!(
                    "{}(linear = {}, border_handling = BorderHandling.{})",
                    kernel,
                    if *linear { "True" } else { "False" },
                    border_handling
                )
            },
            VSJETKernel::AdobeBicubicSharper {
                linear,
                border_handling,
            } => {
                modules.insert(kernel.to_string(), None);
                modules.insert("BorderHandling".to_owned(), None);
                format!(
                    "{}(linear = {}, border_handling = BorderHandling.{})",
                    kernel,
                    if *linear { "True" } else { "False" },
                    border_handling
                )
            },
            VSJETKernel::AdobeBicubicSmoother {
                linear,
                border_handling,
            } => {
                modules.insert(kernel.to_string(), None);
                modules.insert("BorderHandling".to_owned(), None);
                format!(
                    "{}(linear = {}, border_handling = BorderHandling.{})",
                    kernel,
                    if *linear { "True" } else { "False" },
                    border_handling
                )
            },
            VSJETKernel::BicubicSharp {
                linear,
                border_handling,
            } => {
                modules.insert(kernel.to_string(), None);
                modules.insert("BorderHandling".to_owned(), None);
                format!(
                    "{}(linear = {}, border_handling = BorderHandling.{})",
                    kernel,
                    if *linear { "True" } else { "False" },
                    border_handling
                )
            },
            VSJETKernel::RobidouxSoft {
                linear,
                border_handling,
            } => {
                modules.insert(kernel.to_string(), None);
                modules.insert("BorderHandling".to_owned(), None);
                format!(
                    "{}(linear = {}, border_handling = BorderHandling.{})",
                    kernel,
                    if *linear { "True" } else { "False" },
                    border_handling
                )
            },
            VSJETKernel::Robidoux {
                linear,
                border_handling,
            } => {
                modules.insert(kernel.to_string(), None);
                modules.insert("BorderHandling".to_owned(), None);
                format!(
                    "{}(linear = {}, border_handling = BorderHandling.{})",
                    kernel,
                    if *linear { "True" } else { "False" },
                    border_handling
                )
            },
            VSJETKernel::RobidouxSharp {
                linear,
                border_handling,
            } => {
                modules.insert(kernel.to_string(), None);
                modules.insert("BorderHandling".to_owned(), None);
                format!(
                    "{}(linear = {}, border_handling = BorderHandling.{})",
                    kernel,
                    if *linear { "True" } else { "False" },
                    border_handling
                )
            },
            VSJETKernel::BicubicAuto {
                b,
                c,
                linear,
                border_handling,
            } => {
                modules.insert(kernel.to_string(), None);
                modules.insert("BorderHandling".to_owned(), None);
                let b_str = b.as_ref().map_or_else(String::new, |b| format!("b = {},", b));
                let c_str = c.as_ref().map_or_else(String::new, |c| format!("c = {},", c));
                format!(
                    "{}({}{}linear = {}, border_handling = BorderHandling.{})",
                    b_str,
                    c_str,
                    kernel,
                    if *linear { "True" } else { "False" },
                    border_handling
                )
            },
            VSJETKernel::Spline16 {
                linear,
                border_handling,
            } => {
                modules.insert(kernel.to_string(), None);
                modules.insert("BorderHandling".to_owned(), None);
                format!(
                    "{}(linear = {}, border_handling = BorderHandling.{})",
                    kernel,
                    if *linear { "True" } else { "False" },
                    border_handling
                )
            },
            VSJETKernel::Spline36 {
                linear,
                border_handling,
            } => {
                modules.insert(kernel.to_string(), None);
                modules.insert("BorderHandling".to_owned(), None);
                format!(
                    "{}(linear = {}, border_handling = BorderHandling.{})",
                    kernel,
                    if *linear { "True" } else { "False" },
                    border_handling
                )
            },
            VSJETKernel::Spline64 {
                linear,
                border_handling,
            } => {
                modules.insert(kernel.to_string(), None);
                modules.insert("BorderHandling".to_owned(), None);
                format!(
                    "{}(linear = {}, border_handling = BorderHandling.{})",
                    kernel,
                    if *linear { "True" } else { "False" },
                    border_handling
                )
            },
            VSJETKernel::Bilinear {
                linear,
                border_handling,
            } => {
                modules.insert(kernel.to_string(), None);
                modules.insert("BorderHandling".to_owned(), None);
                format!(
                    "{}(linear = {}, border_handling = BorderHandling.{})",
                    kernel,
                    if *linear { "True" } else { "False" },
                    border_handling
                )
            },
            VSJETKernel::Lanczos {
                taps,
                linear,
                border_handling,
            } => {
                modules.insert(kernel.to_string(), None);
                modules.insert("BorderHandling".to_owned(), None);
                let taps_str =
                    taps.as_ref().map_or_else(String::new, |taps| format!("taps = {},", taps));
                format!(
                    "{}({}linear = {}, border_handling = BorderHandling.{})",
                    taps_str,
                    kernel,
                    if *linear { "True" } else { "False" },
                    border_handling
                )
            },
            VSJETKernel::Point {
                linear,
                border_handling,
            } => {
                modules.insert(kernel.to_string(), None);
                modules.insert("BorderHandling".to_owned(), None);
                format!(
                    "{}(linear = {}, border_handling = BorderHandling.{})",
                    kernel,
                    if *linear { "True" } else { "False" },
                    border_handling
                )
            },
        };

        (modules, kernel_command)
    }

    #[inline]
    pub fn doubler_details(doubler: &Doubler) -> (BTreeMap<ModuleName, ModuleAlias>, String) {
        let mut modules = BTreeMap::new();
        modules.insert(doubler.to_string(), None);
        let doubler_command = match doubler {
            Doubler::ArtCNN(model) => {
                format!("{}.{}", doubler, model)
            },
            Doubler::Waifu2x(model) => {
                format!("{}.{}", doubler, model)
            },
        };

        (modules, doubler_command)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, EnumString, IntoStaticStr, Display)]
pub enum VSJETKernel {
    /// Bicubic resizer. (b=0, c=0.5).
    #[strum(serialize = "Bicubic")]
    Bicubic {
        b:               f64,
        c:               f64,
        linear:          bool,
        border_handling: BorderHandling,
    },
    /// BSpline resizer (b=1, c=0).
    #[strum(serialize = "BSpline")]
    BSpline {
        linear:          bool,
        border_handling: BorderHandling,
    },
    /// Hermite resizer (b=0, c=0).
    #[strum(serialize = "Hermite")]
    Hermite {
        linear:          bool,
        border_handling: BorderHandling,
    },
    /// Mitchell resizer (b=1/3, c=1/3).
    #[strum(serialize = "Mitchell")]
    Mitchell {
        linear:          bool,
        border_handling: BorderHandling,
    },
    /// Catrom resizer (b=0, c=0.5).
    #[strum(serialize = "Catrom")]
    Catrom {
        linear:          bool,
        border_handling: BorderHandling,
    },
    /// FFmpeg's swscale default resizer (b=0, c=0.6).
    #[strum(serialize = "FFmpegBicubic")]
    FFmpegBicubic {
        linear:          bool,
        border_handling: BorderHandling,
    },
    /// Adobe's "Bicubic" interpolation preset resizer (b=0, c=0.75).
    #[strum(serialize = "AdobeBicubic")]
    AdobeBicubic {
        linear:          bool,
        border_handling: BorderHandling,
    },
    /// Adobe's "Bicubic Sharper" interpolation preset resizer (b=0, c=1,
    /// blur=1.05).
    #[strum(serialize = "AdobeBicubicSharper")]
    AdobeBicubicSharper {
        linear:          bool,
        border_handling: BorderHandling,
    },
    /// Adobe's "Bicubic Smoother" interpolation preset resizer (b=0, c=0.625,
    /// blur=1.15).
    #[strum(serialize = "AdobeBicubicSmoother")]
    AdobeBicubicSmoother {
        linear:          bool,
        border_handling: BorderHandling,
    },
    /// BicubicSharp resizer (b=0, c=1).
    #[strum(serialize = "BicubicSharp")]
    BicubicSharp {
        linear:          bool,
        border_handling: BorderHandling,
    },
    /// RobidouxSoft resizer (b=0.67962, c=0.16019).
    #[strum(serialize = "RobidouxSoft")]
    RobidouxSoft {
        linear:          bool,
        border_handling: BorderHandling,
    },
    /// Robidoux resizer (b=0.37822, c=0.31089).
    #[strum(serialize = "Robidoux")]
    Robidoux {
        linear:          bool,
        border_handling: BorderHandling,
    },
    /// RobidouxSharp resizer (b=0.26201, c=0.36899).
    #[strum(serialize = "RobidouxSharp")]
    RobidouxSharp {
        linear:          bool,
        border_handling: BorderHandling,
    },
    /// Bicubic resizer that follows the rule of `b + 2c = ...`
    #[strum(serialize = "BicubicAuto")]
    BicubicAuto {
        b:               Option<f64>,
        c:               Option<f64>,
        linear:          bool,
        border_handling: BorderHandling,
    },
    /// Spline16 resizer.
    #[strum(serialize = "Spline16")]
    Spline16 {
        linear:          bool,
        border_handling: BorderHandling,
    },
    /// Spline36 resizer.
    #[strum(serialize = "Spline36")]
    Spline36 {
        linear:          bool,
        border_handling: BorderHandling,
    },
    /// Spline64 resizer.
    #[strum(serialize = "Spline64")]
    Spline64 {
        linear:          bool,
        border_handling: BorderHandling,
    },
    /// Bilinear resizer.
    #[strum(serialize = "Bilinear")]
    Bilinear {
        linear:          bool,
        border_handling: BorderHandling,
    },
    /// Lanczos resizer. (taps=3)
    #[strum(serialize = "Lanczos")]
    Lanczos {
        taps:            Option<u32>,
        linear:          bool,
        border_handling: BorderHandling,
    },
    /// Point resizer.
    #[strum(serialize = "Point")]
    Point {
        linear:          bool,
        border_handling: BorderHandling,
    },
}

impl Default for VSJETKernel {
    #[inline]
    fn default() -> Self {
        Self::Bicubic {
            b:               0.0,
            c:               0.5,
            border_handling: BorderHandling::Mirror,
            linear:          false,
        }
    }
}

/// Method for handling image borders during sampling.
#[derive(
    Debug, Copy, Clone, Serialize, Deserialize, EnumString, IntoStaticStr, Default, Display,
)]
#[serde(rename_all = "UPPERCASE")]
pub enum BorderHandling {
    #[strum(serialize = "MIRROR")]
    #[default]
    Mirror = 0,
    #[strum(serialize = "ZERO")]
    Zero = 1,
    #[strum(serialize = "REPEAT")]
    Repeat = 2,
}

/// Whether to descale only height, only width, or both. "h" or "w" respectively
/// for the former two.
#[derive(
    Debug, Copy, Clone, Serialize, Deserialize, EnumString, IntoStaticStr, Default, Display,
)]
pub enum DescaleMode {
    #[strum(serialize = "w")]
    Width,
    #[strum(serialize = "h")]
    Height,
    #[strum(serialize = "hw")]
    #[default]
    Both,
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize, EnumString, IntoStaticStr, Display)]
pub enum Doubler {
    #[strum(serialize = "ArtCNN")]
    ArtCNN(ArtCNNModel),
    #[strum(serialize = "Waifu2x")]
    Waifu2x(Waifu2xModel),
}

impl Default for Doubler {
    #[inline]
    fn default() -> Self {
        Self::ArtCNN(ArtCNNModel::R8F64)
    }
}

#[derive(
    Debug, Copy, Clone, Serialize, Deserialize, EnumString, IntoStaticStr, Default, Display,
)]
pub enum ArtCNNModel {
    #[strum(serialize = "C4F32")]
    C4F32,
    #[strum(serialize = "C4F32_DS")]
    C4F32DS,
    #[strum(serialize = "C16F64")]
    C16F64,
    #[strum(serialize = "C16F64_DS")]
    C16F64DS,
    #[strum(serialize = "R16F96")]
    R16F96,
    #[strum(serialize = "R8F64")]
    #[default]
    R8F64,
    #[strum(serialize = "R8F64_DS")]
    R8F64DS,
    #[strum(serialize = "R8F64_Chroma")]
    R8F64Chroma,
    #[strum(serialize = "C4F16")]
    C4F16,
    #[strum(serialize = "C4F16_DS")]
    C4F16DS,
    #[strum(serialize = "R16F96_Chroma")]
    R16F96Chroma,
}

/// Waifu2x model variants
#[derive(
    Debug, Copy, Clone, Serialize, Deserialize, EnumString, IntoStaticStr, Default, Display,
)]
pub enum Waifu2xModel {
    /// Waifu2x model for anime-style art.
    #[strum(serialize = "AnimeStyleArt")]
    AnimeStyleArt,
    /// RGB version of the anime-style model.
    #[strum(serialize = "AnimeStyleArtRGB")]
    AnimeStyleArtRGB,
    /// Waifu2x model trained on real-world photographic images.
    #[strum(serialize = "Photo")]
    Photo,
    /// UpConv7 model variant optimized for anime-style images.
    #[strum(serialize = "UpConv7AnimeStyleArt")]
    UpConv7AnimeStyleArt,
    /// UpConv7 model variant optimized for photographic images.
    #[strum(serialize = "UpConv7Photo")]
    UpConv7Photo,
    /// UpResNet10 model offering a balance of speed and quality.
    #[strum(serialize = "UpResNet10")]
    UpResNet10,
    /// CUNet (Compact U-Net) model for anime art.
    #[strum(serialize = "Cunet")]
    #[default]
    Cunet,
    /// Swin-Unet-based model trained on anime-style images.
    #[strum(serialize = "SwimUnetArt")]
    SwinUnetArt,
    /// Swin-Unet model trained on photographic content.
    #[strum(serialize = "SwimUnetPhoto")]
    SwinUnetPhoto,
    /// Improved Swin-Unet model for photos (v2).
    #[strum(serialize = "SwimUnetPhotoV2")]
    SwinUnetPhotoV2,
    /// Swin-Unet model trained on anime scans.
    #[strum(serialize = "SwimUnetArtScan")]
    SwinUnetArtScan,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_rescale() {
        let kernel = VSJETKernel::Bilinear {
            linear:          false,
            border_handling: BorderHandling::Mirror,
        };

        println!(
            "{}(border_handling = BorderHandling.{})",
            kernel,
            BorderHandling::Mirror
        );
    }
}
