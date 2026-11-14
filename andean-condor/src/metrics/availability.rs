//! Runtime availability of the native metric libraries.
//!
//! libvship and libvmaf are opened with `dlopen` at runtime, so whether a given
//! installation can use the native path is a property of the machine, not of
//! the build. The scoring modules answer that question one metric at a time --
//! [`vship::native_supported`](super::vship::native_supported) and
//! [`fmetrics::native_supported`](super::fmetrics::native_supported) decide
//! per metric -- which is the right shape for dispatch but the wrong one for
//! reporting: a user asking what their installation supports wants the whole
//! picture at once.
//!
//! [`libraries`] collects that picture. It answers only what is loaded on this
//! system, deliberately not what would be used for a given clip, because the
//! choice between engines also depends on how the input is decoded (see
//! [`super::Engine`]) and on per-metric model limits.
//!
//! Nothing here can fail. Each probe returns a bool or an `Option`, so a
//! machine with none of these libraries reports all of them absent rather than
//! erroring, which is what lets `av1an --version` print this unconditionally.

use av_metrics_vmaf::VmafModel;

/// One native metric library's runtime status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricLibraryInfo {
    /// Display name, e.g. `libvship`.
    pub name: &'static str,
    /// Whether the library is loaded and usable right now.
    pub available: bool,
    /// The library version, when it loaded.
    ///
    /// `None` whenever `available` is `false`: a caller must never print a
    /// version for a library it reported as missing.
    pub version: Option<String>,
    /// Extra `key: value` pairs describing the loaded library.
    ///
    /// Empty when the library did not load. Diagnostics rather than state, so
    /// their presence does not affect [`Self::available`].
    pub details: Vec<(&'static str, String)>,
    /// Compute devices the library can use, for the GPU metric libraries.
    ///
    /// Empty for a CPU library, and for one that is absent. This is what a user
    /// needs in order to override the device choice, since the index here is
    /// the `gpu_id` that selects it.
    pub devices: Vec<MetricDeviceInfo>,
}

/// One compute device a metric library enumerated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricDeviceInfo {
    /// The device's index, which is the value that selects it.
    pub id: u32,
    /// The device's reported name.
    pub name: String,
    /// VRAM in bytes, or `None` when the library did not report a usable size.
    pub vram_bytes: Option<u64>,
    /// Whether the library classifies this device as integrated.
    ///
    /// An integrated device is usually not what these metrics should run on,
    /// which is why the default prefers a discrete one.
    pub integrated: bool,
    /// Whether scoring uses this device when nothing else is configured.
    pub is_default: bool,
}

/// Availability of every native metric library Condor can score with.
///
/// Ordered as the engine table in [`super`] does: vship, fmetrics, then vmaf.
/// The order is presentational only -- nothing here chooses an engine.
#[inline]
#[must_use]
pub fn libraries() -> Vec<MetricLibraryInfo> {
    vec![vship_info(), fmetrics_info(), vmaf_info()]
}

/// libvship's status.
///
/// Availability requires more than a loadable library: libvship is compiled
/// per-backend against a GPU driver, and scoring additionally requires a device
/// that passes `Vship_GPUFullCheck`. The backend and device are reported
/// separately so a library that loaded but has no usable device is
/// distinguishable from one that never loaded.
///
/// Every enumerated device is reported, not just the chosen one. A machine with
/// more than one GPU is exactly the case where a user needs to override the
/// default, and the default is invisible without the alternatives next to it.
#[inline]
#[must_use]
fn vship_info() -> MetricLibraryInfo {
    let available = av_metrics_vship::is_available();
    let mut details = Vec::new();

    if let Some(backend) = av_metrics_vship::backend() {
        details.push(("backend", backend.as_str().to_owned()));
    }

    let devices = vship_devices();

    // The chosen device is a detail too, but the per-device table already
    // marks it, so naming it here would only repeat it.
    if let Some(device) = devices.iter().find(|device| device.is_default) {
        details.push(("device", device.name.clone()));
    }

    MetricLibraryInfo {
        name: "libvship",
        available,
        version: av_metrics_vship::libvship_version().filter(|_| available).map(str::to_owned),
        details,
        devices,
    }
}

/// libvship's compute devices, with the chosen one marked.
///
/// Empty unless libvship both enumerated devices and named one to prefer. The
/// second condition is what keeps the table honest: a table with nothing marked
/// reads as "choose one of these", which would be wrong advice on a system
/// where scoring cannot run on any device libvship reported.
#[inline]
#[must_use]
fn vship_devices() -> Vec<MetricDeviceInfo> {
    let Ok(devices) = av_metrics_vship::devices() else {
        return Vec::new();
    };
    let Some(default_id) = av_metrics_vship::default_gpu_id() else {
        return Vec::new();
    };

    devices
        .into_iter()
        .map(|device| {
            // A zero VRAM report is libvship saying nothing, not that the
            // device has no memory.
            let vram_bytes = (device.info.vram_size > 0).then_some(device.info.vram_size);

            MetricDeviceInfo {
                id: device.id,
                name: device.info.name(),
                vram_bytes,
                integrated: device.is_integrated(),
                is_default: device.id == default_id,
            }
        })
        .collect()
}

/// fmetrics' status.
///
/// fmetrics is a CPU library, so loading it is sufficient and there is no
/// backend or device to report. Its CVVDP is a separate implementation with its
/// own version, which is the number worth seeing when diagnosing a CVVDP
/// discrepancy against the vship and plugin paths.
#[inline]
#[must_use]
fn fmetrics_info() -> MetricLibraryInfo {
    let available = av_metrics_fmetrics::is_available();
    let mut details = Vec::new();

    if let Some(version) = av_metrics_fmetrics::cvvdp_version() {
        details.push(("cvvdp", version));
    }

    MetricLibraryInfo {
        name: "fmetrics",
        available,
        version: av_metrics_fmetrics::fmetrics_version().filter(|_| available),
        details,
        // fmetrics is a CPU library, so it has no devices to enumerate.
        devices: Vec::new(),
    }
}

/// libvmaf's status, including which stock models are present.
///
/// libvmaf loading says nothing about whether VMAF scoring will succeed. The
/// stock models are compiled into some builds and shipped as files by others,
/// and a build without them rejects a bare model name -- so the models are
/// listed individually. Two of those entries are commonly absent and that is
/// expected, not a fault: `vmaf_b_v0.6.3` is unloadable by libvmaf 3.2.1, which
/// removed the BOUND extractors it needs.
#[inline]
#[must_use]
fn vmaf_info() -> MetricLibraryInfo {
    let available = av_metrics_vmaf::is_available();
    let mut details = vec![(
        "cuda",
        if av_metrics_vmaf::cuda_available() {
            "yes".to_owned()
        } else {
            "no".to_owned()
        },
    )];

    for model in VmafModel::STOCK {
        let Some(version) = model.stock_version() else {
            continue;
        };
        let found = model.find_stock_model_file();

        details.push((
            version,
            if found.is_some() {
                "found"
            } else {
                "not found"
            }
            .to_owned(),
        ));
    }

    MetricLibraryInfo {
        name: "libvmaf",
        available,
        version: av_metrics_vmaf::libvmaf_version().filter(|_| available).map(str::to_owned),
        details,
        // libvmaf's CUDA backend has no device enumeration exposed through this
        // binding, and it scores on the CPU when CUDA is absent.
        devices: Vec::new(),
    }
}
