//! Configuration types for VMAF scoring.

use std::{
    ffi::c_int,
    fmt::{self, Display, Formatter},
    path::{Path, PathBuf},
};

use strum::{EnumString, IntoStaticStr};

use crate::{error::VmafError, ffi};

/// A VMAF model, either one of the stock models or a user-supplied path.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum VmafModel {
    /// `vmaf_v0.6.1`, the default model.
    #[default]
    Default,
    /// `vmaf_v0.6.1neg`, trained for negative-content scoring.
    Neg,
    /// `vmaf_4k_v0.6.1neg`, the 4K model for negative-content scoring.
    UhdNeg,
    /// `vmaf_b_v0.6.3`, the b-model used for confidence intervals.
    ///
    /// **Not loadable by libvmaf 3.2.1**, which removed the BOUND feature
    /// extractors and exposes no `enable_bound` option. Loading it fails with
    /// `EINVAL`, so this model is unusable on every stock build. Use
    /// [`Self::Uhd`] or an explicit [`Self::Path`] instead.
    Weighted,
    /// `vmaf_4k_v0.6.1`, trained at 4K.
    Uhd,
    /// An explicit model file on disk.
    Path(PathBuf),
}

impl VmafModel {
    /// Every stock model, for enumerating what a libvmaf build can load.
    ///
    /// [`Self::Path`] is deliberately excluded: a custom model is a per-run
    /// choice, not a property of the installation.
    ///
    /// [`Self::Weighted`] is included even though libvmaf 3.2.1 cannot load
    /// it, so that a report walking this list shows the model's real state
    /// rather than quietly omitting one of the five stock models.
    pub const STOCK: [Self; 5] =
        [Self::Default, Self::Neg, Self::UhdNeg, Self::Weighted, Self::Uhd];

    /// Resolve this model to the string libvmaf expects.
    ///
    /// `vmaf_model_load` takes a bare version string for built-in models, or a
    /// filesystem path for a custom model. It does *not* accept the `version=`
    /// or `path=` key/value form that the `vmaf` CLI accepts, so those prefixes
    /// must not be added here.
    #[inline]
    pub fn as_libvmaf_model(&self) -> Result<String, VmafError> {
        match self {
            Self::Default => Ok("vmaf_v0.6.1".to_owned()),
            Self::Neg => Ok("vmaf_v0.6.1neg".to_owned()),
            Self::UhdNeg => Ok("vmaf_4k_v0.6.1neg".to_owned()),
            Self::Weighted => Ok("vmaf_b_v0.6.3".to_owned()),
            Self::Uhd => Ok("vmaf_4k_v0.6.1".to_owned()),
            Self::Path(path) => {
                if !path.is_file() {
                    return Err(VmafError::ModelFileMissing { path: path.clone() });
                }
                Ok(path.display().to_string())
            },
        }
    }

    /// Whether this model refers to a file on disk rather than a built-in name.
    ///
    /// libvmaf exposes separate loaders for the two, and each rejects the
    /// other's input, so the scorer must know which to call.
    #[inline]
    #[must_use]
    pub const fn is_path(&self) -> bool {
        matches!(self, Self::Path(_))
    }

    /// The bare libvmaf version string for this model, when it names a stock
    /// model.
    ///
    /// Used to locate a model file on disk when libvmaf was built without the
    /// built-in models compiled in.
    #[inline]
    #[must_use]
    pub const fn stock_version(&self) -> Option<&'static str> {
        match self {
            Self::Default => Some("vmaf_v0.6.1"),
            Self::Neg => Some("vmaf_v0.6.1neg"),
            Self::UhdNeg => Some("vmaf_4k_v0.6.1neg"),
            Self::Weighted => Some("vmaf_b_v0.6.3"),
            Self::Uhd => Some("vmaf_4k_v0.6.1"),
            Self::Path(_) => None,
        }
    }

    /// Directories searched for a stock model file.
    ///
    /// Not every libvmaf build compiles the models in. Arch's `vmaf` package
    /// and MSYS2's `mingw-w64-*-vmaf` both ship without them, and
    /// `vmaf_model_load` then rejects a bare version with `EINVAL`.
    /// Distributions that do not embed the models install them under one of
    /// these prefixes instead.
    ///
    /// A nonempty `VMAF_MODEL_PATH` is one literal directory, returned first.
    /// An empty value is treated as unset. System directories precede those *near* the
    /// binary, so a stray `vmaf_v0.6.1.json` in a shared or downloads-adjacent
    /// directory cannot silently override the model the distribution intends.
    #[inline]
    #[must_use]
    pub fn model_search_paths() -> Vec<PathBuf> {
        model_search_paths_for(
            ffi::loaded_library_directory(),
            model_override_directory().as_deref(),
        )
    }

    /// Locate a stock model file for this model, if one is installed on disk.
    ///
    /// Returns the filesystem path when a `<version>.json` exists in one of
    /// [`Self::model_search_paths`]. With a nonempty `VMAF_MODEL_PATH`, only that
    /// directory is eligible; a missing override returns `None`, not a fallback.
    #[inline]
    #[must_use]
    pub fn find_stock_model_file(&self) -> Option<PathBuf> {
        self.stock_model_override()
            .ok()?
            .or_else(|| self.find_stock_model_file_in(&Self::fallback_model_search_paths()))
    }

    pub(crate) fn stock_model_override(&self) -> Result<Option<PathBuf>, VmafError> {
        self.stock_model_override_for(model_override_directory().as_deref())
    }

    fn stock_model_override_for(
        &self,
        directory: Option<&Path>,
    ) -> Result<Option<PathBuf>, VmafError> {
        let (Some(version), Some(directory)) = (self.stock_version(), directory) else {
            return Ok(None);
        };
        let path = directory.join(format!("{version}.json"));
        if !path.is_file() {
            return Err(VmafError::ModelNotFound {
                model: path.display().to_string(),
                reason:
                    "VMAF_MODEL_PATH explicitly selected this file, but it is missing or not a \
                         regular file; no other model will be tried"
                        .to_owned(),
            });
        }
        Ok(Some(path))
    }

    pub(crate) fn fallback_model_search_paths() -> Vec<PathBuf> {
        model_search_paths_for(ffi::loaded_library_directory(), None)
    }

    pub(crate) fn find_stock_model_file_in(&self, directories: &[PathBuf]) -> Option<PathBuf> {
        let version = self.stock_version()?;
        let filename = format!("{version}.json");

        directories
            .iter()
            .map(|directory| directory.join(&filename))
            .find(|candidate| candidate.is_file())
    }

    /// Whether this model can be used on the CUDA backend.
    ///
    /// Conservative legacy hint, currently always false. The scorer does not
    /// use this hint: actual required-feature registration against the installed
    /// libvmaf build decides compatibility, including for stock models.
    #[inline]
    #[must_use]
    pub const fn cuda_supported(&self) -> bool {
        false
    }
}

/// Model directories under an MSYS2 installation rooted at `root`, paired with
/// whether the distribution owns them.
///
/// Only `<prefix>/share/vmaf/model` is distribution-owned.
/// `<prefix>/share/model` is the Arch-style sibling and `<prefix>/model` is
/// where a release tarball may place them, and neither sits in a location a
/// package manager is guaranteed to own, so both rank with the paths merely
/// near the binary.
#[cfg(target_os = "windows")]
fn msys2_model_directories(root: &Path) -> Vec<(PathBuf, bool)> {
    ["mingw64", "ucrt64"]
        .into_iter()
        .flat_map(|prefix| {
            let prefix = root.join(prefix);
            [
                (prefix.join("share").join("vmaf").join("model"), true),
                (prefix.join("share").join("model"), false),
                (prefix.join("model"), false),
            ]
        })
        .collect()
}

/// The MSYS2 root to search for models under.
///
/// The candidate MSYS2 model directories under a root inferred from where
/// libvmaf was loaded.
///
/// MSYS2 defaults to `C:\msys64`, but it can be installed on any drive, so the
/// root is discovered rather than assumed: `MSYS2_ROOT` when the machine sets
/// it, otherwise the prefix above the directory libvmaf was loaded from,
/// because the `mingw-w64-*-vmaf` package installs to `<root>/<prefix>/bin`.
#[cfg(target_os = "windows")]
fn msys2_root(loaded_from: Option<&Path>) -> Option<PathBuf> {
    std::env::var("MSYS2_ROOT").ok().filter(|root| !root.is_empty()).map_or_else(
        || dll_implied_root(loaded_from),
        |root| Some(PathBuf::from(root)),
    )
}

/// The MSYS2 root implied by where libvmaf was loaded from.
///
/// The package installs to `<root>/<prefix>/bin`, so the load directory is
/// `bin` and the root is two levels above it. A library resolved by the loader
/// from a bare file name records no directory, and therefore yields no root;
/// neither does a path too shallow to have an MSYS2 layout above it.
#[cfg(target_os = "windows")]
fn dll_implied_root(loaded_from: Option<&Path>) -> Option<PathBuf> {
    let bin = loaded_from?;
    let root = bin.parent()?.parent()?;
    // A root such as `C:\` has nothing above it, so there is no layout to
    // derive.
    root.parent()?;
    Some(root.to_path_buf())
}

/// MSYS2 is not a package prefix on other platforms.
#[cfg(not(target_os = "windows"))]
fn msys2_root(_loaded_from: Option<&Path>) -> Option<PathBuf> {
    None
}

/// Model directories to search under an MSYS2 root, paired with whether the
/// distribution owns them.
///
/// There is no MSYS2 layout off Windows, so nothing is added there.
#[cfg(not(target_os = "windows"))]
fn msys2_model_directories(_root: &Path) -> Vec<(PathBuf, bool)> {
    Vec::new()
}

fn model_override_directory() -> Option<PathBuf> {
    std::env::var_os("VMAF_MODEL_PATH")
        .filter(|directory| !directory.is_empty())
        .map(PathBuf::from)
}

/// Collect candidates with explicit selection first, then system before adjacent.
/// Injecting directories keeps ordering tests independent of native installations.
fn model_search_paths_for(loaded_from: Option<&Path>, explicit: Option<&Path>) -> Vec<PathBuf> {
    let mut system = Vec::new();
    let mut adjacent = Vec::new();

    // System-wide installs. These are the paths a package manager controls, so
    // a model found here is the one the distribution intends, and it takes
    // precedence over anything merely *near* the binary.
    if cfg!(target_os = "windows") {
        if let Some(root) = msys2_root(loaded_from) {
            for (path, is_system) in msys2_model_directories(&root) {
                if is_system {
                    system.push(path);
                } else {
                    adjacent.push(path);
                }
            }
        }
    } else if cfg!(target_os = "macos") {
        system.push(PathBuf::from("/opt/homebrew/share/vmaf/model"));
        system.push(PathBuf::from("/usr/local/share/vmaf/model"));
    } else {
        // Arch's `vmaf` package installs to `/usr/share/model`, Debian and
        // Fedora use `/usr/share/vmaf/model`, and a source build commonly lands
        // in `/usr/local/share/model`.
        system.push(PathBuf::from("/usr/share/model"));
        system.push(PathBuf::from("/usr/share/vmaf/model"));
        system.push(PathBuf::from("/usr/local/share/model"));
        system.push(PathBuf::from("/usr/local/share/vmaf/model"));
    }

    // Beside the running executable. This is what a release layout relies on:
    // `av1an.exe` and `model/vmaf_v0.6.1.json` shipped together, with no
    // environment variable set.
    if let Ok(executable) = std::env::current_exe()
        && let Some(directory) = executable.parent()
    {
        adjacent.push(directory.join("model"));
    }

    // Beside the library that was actually loaded. This is what makes a
    // relocated MSYS2 work: the recorded directory is wherever the load
    // succeeded, so `.../bin/libvmaf.dll` implies `.../model` and
    // `.../share/vmaf/model` without any prefix being assumed.
    if let Some(directory) = loaded_from {
        if let Some(prefix) = directory.parent() {
            adjacent.push(prefix.join("model"));
            system.push(prefix.join("share").join("vmaf").join("model"));
        }
        adjacent.push(directory.join("model"));
    }

    // The same inference from `VMAF_LIB_DIR`, for when the library was found on
    // the loader's search path rather than by explicit directory.
    if let Ok(directory) = std::env::var("VMAF_LIB_DIR")
        && !directory.is_empty()
    {
        let directory = PathBuf::from(directory);
        if let Some(prefix) = directory.parent() {
            adjacent.push(prefix.join("model"));
            system.push(prefix.join("share").join("vmaf").join("model"));
        }
        adjacent.push(directory.join("model"));
    }

    // The sources above overlap freely — an explicit `VMAF_LIB_DIR` is also
    // where the load came from — and a repeated directory only makes the
    // "search these" hint in an error message longer. The two groups are joined
    // only here, so no source above can emit a system path behind an adjacent
    // one regardless of the order the sources are appended in.
    let mut paths = Vec::new();
    for path in explicit.map(Path::to_path_buf).into_iter().chain(system).chain(adjacent) {
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    paths
}

/// Whether a search path is controlled by the platform or the distribution.
///
/// True for the prefixes a package manager owns, so a model found there is the
/// one the distribution intends. A path carries the separator of the platform
/// it was built on, and a Windows path assembled from a POSIX-looking root
/// mixes the two, so the `share/vmaf/model` suffix is matched in every spelling
/// rather than assuming the path was constructed consistently.
#[cfg(test)]
fn is_system_model_path(path: &str) -> bool {
    // Canonical package-manager model directories
    const SYSTEM_PATHS: &[&str] = &[
        "/usr/share/model",
        "/usr/share/vmaf/model",
        "/usr/local/share/model",
        "/usr/local/share/vmaf/model",
        "/opt/homebrew/share/vmaf/model",
        "/opt/local/share/vmaf/model",
    ];

    // Exact match or prefix match for canonical system paths
    SYSTEM_PATHS.iter().any(|sys| path == *sys || path.starts_with(&format!("{}/", sys)))
        // Also match the share/vmaf/model suffix in any spelling (for Windows paths
        // assembled from POSIX-looking roots)
        || [
            "share/vmaf/model",
            "share/vmaf\\model",
            "share\\vmaf/model",
            "share\\vmaf\\model",
        ]
        .iter()
        .any(|suffix| path.contains(suffix))
}

impl Display for VmafModel {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Default => f.write_str("vmaf_v0.6.1"),
            Self::Neg => f.write_str("vmaf_v0.6.1neg"),
            Self::UhdNeg => f.write_str("vmaf_4k_v0.6.1neg"),
            Self::Weighted => f.write_str("vmaf_b_v0.6.3"),
            Self::Uhd => f.write_str("vmaf_4k_v0.6.1"),
            Self::Path(path) => write!(f, "{}", path.display()),
        }
    }
}

/// Optional additional feature extractors to compute alongside the model score.
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::Display, EnumString, IntoStaticStr, Default)]
pub enum VmafFeature {
    /// No additional features.
    #[default]
    #[strum(serialize = "default")]
    Default,
    /// PSNR.
    #[strum(serialize = "psnr")]
    Psnr,
    /// PSNR weighted for human visual system sensitivity.
    #[strum(serialize = "psnr_hvs")]
    PsnrHvs,
    /// Structural similarity.
    #[strum(serialize = "ssim")]
    Ssim,
    /// Multi-scale structural similarity.
    #[strum(serialize = "ms_ssim")]
    MsSsim,
    /// CAMBI, the banding detector.
    #[strum(serialize = "cambi")]
    Cambi,
}

impl VmafFeature {
    /// The name libvmaf uses to *register* this extractor via
    /// `vmaf_use_feature`.
    ///
    /// These are the extractor names, e.g. `psnr`. The per-plane values they
    /// emit (`psnr_y`, `psnr_cb`, `psnr_cr`) are separate keys used when
    /// reading scores back, so the two are deliberately distinct.
    #[inline]
    #[must_use]
    pub const fn as_libvmaf_extractor(self) -> Option<&'static str> {
        match self {
            Self::Default => None,
            Self::Psnr => Some("psnr"),
            Self::PsnrHvs => Some("psnr_hvs"),
            Self::Ssim => Some("float_ssim"),
            Self::MsSsim => Some("float_ms_ssim"),
            Self::Cambi => Some("cambi"),
        }
    }

    /// The feature key libvmaf reports this extractor's primary value under.
    ///
    /// Used with `vmaf_feature_score_pooled`. PSNR emits one value per plane,
    /// so the luma plane is used as the representative score.
    #[inline]
    #[must_use]
    pub const fn as_libvmaf_feature(self) -> Option<&'static str> {
        match self {
            Self::Default => None,
            Self::Psnr => Some("psnr_y"),
            Self::PsnrHvs => Some("psnr_hvs_y"),
            Self::Ssim => Some("float_ssim"),
            Self::MsSsim => Some("float_ms_ssim"),
            Self::Cambi => Some("cambi"),
        }
    }
}

/// How VMAF scores are pooled into a single value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PoolMethod {
    /// Arithmetic mean. This is libvmaf's default pooling.
    #[default]
    Mean,
    /// Minimum.
    Min,
    /// Maximum.
    Max,
    /// Harmonic mean, which VMAF recommends when scenes have mixed quality.
    HarmonicMean,
}

impl PoolMethod {
    /// The `VmafPoolingMethod` discriminant libvmaf expects.
    ///
    /// Verified against libvmaf 3.2.1, where the enum is
    /// `UNKNOWN = 0, MIN, MAX, MEAN, HARMONIC_MEAN`. Note `MEAN` is 3, not 0 —
    /// libvmaf treats 0 as an invalid method and rejects it with `EINVAL`.
    #[inline]
    #[must_use]
    pub const fn as_libvmaf_pool(self) -> c_int {
        match self {
            Self::Min => 1,
            Self::Max => 2,
            Self::Mean => 3,
            Self::HarmonicMean => 4,
        }
    }
}

/// How the scorer should choose a compute backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackendPreference {
    /// With the Cargo `cuda` feature, try a complete CUDA scorer if available,
    /// then retry on CPU after any setup failure. Without the feature, use CPU.
    #[default]
    Auto,
    /// Require CPU, never attempt CUDA.
    CpuOnly,
    /// Require CUDA, without CPU fallback. Without the Cargo `cuda` feature,
    /// fail before loading libvmaf; rebuild av-metrics-vmaf with `--features cuda`.
    RequireCuda,
}

/// Configuration for a VMAF scoring session.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct VmafConfig {
    /// The model to score against.
    pub model: VmafModel,
    /// Additional feature extractors to compute.
    pub features: Vec<VmafFeature>,
    /// Number of native CPU worker threads; `0` uses the serial path.
    /// CUDA attempts use zero workers to avoid a libvmaf 3.2.1 device-reference
    /// leak in its threaded path. CPU fallback preserves this requested value.
    pub n_threads: u32,
    /// Backend selection strategy.
    pub backend: BackendPreference,
}

impl VmafConfig {
    /// A configuration using the default model and serial native extraction.
    #[inline]
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the model.
    #[inline]
    #[must_use]
    pub fn with_model(mut self, model: VmafModel) -> Self {
        self.model = model;
        self
    }

    /// Set the model from an explicit path, independent of `VMAF_MODEL_PATH`.
    #[inline]
    pub fn with_model_path(self, path: impl AsRef<Path>) -> Self {
        self.with_model(VmafModel::Path(path.as_ref().to_path_buf()))
    }

    /// Add a feature extractor.
    #[inline]
    #[must_use]
    pub fn with_feature(mut self, feature: VmafFeature) -> Self {
        self.features.push(feature);
        self
    }

    /// Set libvmaf's CPU worker thread count. CUDA uses the native serial path.
    #[inline]
    #[must_use]
    pub fn with_threads(mut self, n_threads: u32) -> Self {
        self.n_threads = n_threads;
        self
    }

    /// Set the backend selection strategy.
    #[inline]
    #[must_use]
    pub fn with_backend(mut self, backend: BackendPreference) -> Self {
        self.backend = backend;
        self
    }

    /// The extractor names to register with libvmaf, in order.
    ///
    /// `Default` contributes nothing and duplicates are dropped while
    /// preserving the caller's ordering.
    #[inline]
    #[must_use]
    pub fn extractor_names(&self) -> Vec<&'static str> {
        let mut names = Vec::with_capacity(self.features.len());
        for feature in &self.features {
            if let Some(name) = feature.as_libvmaf_extractor()
                && !names.contains(&name)
            {
                names.push(name);
            }
        }
        names
    }

    /// The feature keys to read pooled values for, in order.
    ///
    /// `Default` contributes nothing and duplicates are dropped while
    /// preserving the caller's ordering.
    #[inline]
    #[must_use]
    pub fn feature_names(&self) -> Vec<&'static str> {
        let mut names = Vec::with_capacity(self.features.len());
        for feature in &self.features {
            if let Some(name) = feature.as_libvmaf_feature()
                && !names.contains(&name)
            {
                names.push(name);
            }
        }
        names
    }
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "assertions read better with unwrap; a failure panics with a usable message"
)]
mod tests {
    use super::*;

    #[test]
    fn stock_models_resolve_to_libvmaf_version_strings() {
        assert_eq!(
            VmafModel::Default.as_libvmaf_model().unwrap(),
            "vmaf_v0.6.1"
        );
        assert_eq!(VmafModel::Neg.as_libvmaf_model().unwrap(), "vmaf_v0.6.1neg");
        assert_eq!(
            VmafModel::UhdNeg.as_libvmaf_model().unwrap(),
            "vmaf_4k_v0.6.1neg"
        );
        assert_eq!(
            VmafModel::Weighted.as_libvmaf_model().unwrap(),
            "vmaf_b_v0.6.3"
        );
        assert_eq!(VmafModel::Uhd.as_libvmaf_model().unwrap(), "vmaf_4k_v0.6.1");
    }

    #[test]
    fn missing_model_path_is_an_error() {
        let model = VmafModel::Path(PathBuf::from("/nonexistent/model.json"));
        assert!(matches!(
            model.as_libvmaf_model(),
            Err(VmafError::ModelFileMissing { .. })
        ));
    }

    #[test]
    fn default_feature_contributes_no_name() {
        let config = VmafConfig::new().with_feature(VmafFeature::Default);
        assert!(config.feature_names().is_empty());
    }

    #[test]
    fn duplicate_features_are_deduplicated_in_order() {
        let config = VmafConfig::new()
            .with_feature(VmafFeature::Psnr)
            .with_feature(VmafFeature::Ssim)
            .with_feature(VmafFeature::Psnr);
        assert_eq!(config.feature_names(), vec!["psnr_y", "float_ssim"]);
    }

    #[test]
    fn a_stock_model_names_the_file_libvmaf_expects() {
        assert_eq!(VmafModel::Default.stock_version(), Some("vmaf_v0.6.1"));
        assert_eq!(VmafModel::Neg.stock_version(), Some("vmaf_v0.6.1neg"));
        assert_eq!(VmafModel::Weighted.stock_version(), Some("vmaf_b_v0.6.3"));
        assert_eq!(VmafModel::Uhd.stock_version(), Some("vmaf_4k_v0.6.1"));
        assert_eq!(VmafModel::UhdNeg.stock_version(), Some("vmaf_4k_v0.6.1neg"));
        assert_eq!(
            VmafModel::Path(PathBuf::from("/x.json")).stock_version(),
            None,
            "an explicit path is not a stock model"
        );
    }

    #[test]
    fn m25_explicit_selection_and_system_before_adjacent_fallback() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path();
        let explicit = root.join("explicit models 雪");
        let system = root.join("share/vmaf/model");
        let adjacent = root.join("model");
        for (directory, marker) in
            [(&explicit, "explicit"), (&system, "system"), (&adjacent, "adjacent")]
        {
            std::fs::create_dir_all(directory).unwrap();
            std::fs::write(directory.join("vmaf_v0.6.1.json"), marker).unwrap();
        }
        let paths = model_search_paths_for(Some(&root.join("lib")), Some(&explicit));
        assert_eq!(
            paths.first(),
            Some(&explicit),
            "explicit precedes every system path"
        );
        assert!(
            paths.iter().position(|path| path == &system).unwrap()
                < paths.iter().position(|path| path == &adjacent).unwrap(),
            "loaded-library system-shaped fallback precedes adjacent models"
        );
        assert_eq!(
            VmafModel::Default.stock_model_override_for(Some(&explicit)).unwrap(),
            Some(explicit.join("vmaf_v0.6.1.json")),
            "distinct explicit and system fixtures select explicit"
        );
        assert_eq!(
            VmafModel::Default.stock_model_override_for(None).unwrap(),
            None,
            "only no override permits ordinary fallback"
        );
        assert_eq!(
            VmafModel::Default.find_stock_model_file_in(&[system.clone(), adjacent]),
            Some(system.join("vmaf_v0.6.1.json")),
            "deterministic fallback without host installation assumptions"
        );
        std::fs::remove_file(explicit.join("vmaf_v0.6.1.json")).unwrap();
        assert!(
            VmafModel::Default.stock_model_override_for(Some(&explicit)).is_err(),
            "an invalid explicit selection must not fall through to the existing system fixture"
        );
    }

    /// The stock model must be findable on the platforms we ship for.
    ///
    /// Arch's `vmaf` package installs models to `/usr/share/model`, not the
    /// `/usr/share/vmaf/model` a source build would use. Missing that path
    /// silently disables the metric on the CI image and in Docker, where
    /// the library is present but the model cannot be loaded.
    #[test]
    fn the_model_search_paths_cover_known_packaging_layouts() {
        let paths: Vec<String> = VmafModel::model_search_paths()
            .iter()
            .map(|path| path.display().to_string())
            .collect();

        let required: &[&str] = if cfg!(target_os = "macos") {
            &["/opt/homebrew/share/vmaf/model"]
        } else if cfg!(target_os = "windows") {
            // Windows resolves the MSYS2 root from the machine, so there is no
            // fixed path to require — and libvmaf may not even be loaded yet,
            // in which case no prefix can be derived and none is claimed.
            return;
        } else {
            &["/usr/share/model", "/usr/share/vmaf/model", "/usr/local/share/model"]
        };

        for path in required {
            assert!(
                paths.iter().any(|candidate| candidate == path),
                "`{path}` should be searched; candidates are {paths:?}"
            );
        }
    }

    /// Windows must never assume MSYS2 lives on `C:`. The root comes from the
    /// machine, so the layouts are derived from whatever root is supplied.
    #[cfg(target_os = "windows")]
    #[test]
    fn the_msys2_layouts_follow_the_root_the_machine_reports() {
        for root in [r"D:\msys64", r"E:\tools\msys2", r"\\server\share\msys64"] {
            let paths: Vec<String> = msys2_model_directories(Path::new(root))
                .into_iter()
                .map(|(path, _)| path.display().to_string())
                .collect();

            for prefix in ["mingw64", "ucrt64"] {
                let expected = Path::new(root)
                    .join(prefix)
                    .join("share")
                    .join("vmaf")
                    .join("model")
                    .display()
                    .to_string();
                assert!(
                    paths.contains(&expected),
                    "`{expected}` should be searched for root `{root}`; candidates are {paths:?}"
                );
            }
        }
    }

    /// System paths must be searched before anything near the executable, so a
    /// stray `vmaf_v0.6.1.json` in a shared or downloads-adjacent binary
    /// directory cannot silently override the distribution's model.
    #[test]
    fn system_model_paths_take_precedence_over_executable_adjacent_ones() {
        let paths: Vec<String> = VmafModel::fallback_model_search_paths()
            .into_iter()
            .map(|path| path.display().to_string())
            .collect();

        let last_system = paths.iter().rposition(|path| is_system_model_path(path));
        let first_adjacent = paths.iter().position(|path| !is_system_model_path(path));

        if let (Some(last_system), Some(first_adjacent)) = (last_system, first_adjacent) {
            assert!(
                last_system < first_adjacent,
                "every system path must precede every other path; a system path at {last_system} \
                 follows a non-system path at {first_adjacent}. Candidates are {paths:?}"
            );
        }
    }

    /// A library loaded from an MSYS2 prefix emits a system path and an
    /// adjacent path from the same directory, and the adjacent one must not
    /// precede it.
    ///
    /// This is the case that was previously only reachable with a real MSYS2
    /// install, which is why it never ran in CI: the loaded-library block
    /// contributed nothing when libvmaf was absent. The directory is built by
    /// hand here so the regression is caught on any machine.
    #[test]
    fn a_library_prefix_places_its_system_path_before_its_adjacent_ones() {
        let root = PathBuf::from("/vmaf-test-root");
        let loaded_from = root.join("mingw64").join("bin");
        let paths: Vec<String> = model_search_paths_for(Some(&loaded_from), None)
            .into_iter()
            .map(|path| path.display().to_string())
            .collect();

        // The invariant the two buckets exist to guarantee: once a path that is
        // not system-controlled has been emitted, no system path may follow.
        let last_system = paths.iter().rposition(|path| is_system_model_path(path));
        let first_adjacent = paths.iter().position(|path| !is_system_model_path(path));

        if let (Some(last_system), Some(first_adjacent)) = (last_system, first_adjacent) {
            assert!(
                last_system < first_adjacent,
                "a library prefix emitted a system path at {last_system} behind an adjacent one \
                 at {first_adjacent}. Candidates are {paths:?}"
            );
        }

        // The specific inference that regressed: the loaded directory names
        // both a `share/vmaf/model` and a `model` directory, and the
        // former must win.
        assert!(
            paths.iter().any(|path| path.contains("share")),
            "the loaded directory should imply a `share/vmaf/model` directory. Candidates are \
             {paths:?}"
        );
    }
}
