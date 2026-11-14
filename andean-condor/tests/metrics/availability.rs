use andean_condor::metrics::availability::libraries;
use av_metrics_vmaf::VmafModel;

#[test]
fn metric_library_availability_matches_versions_and_absent_device_tables() {
    for info in libraries() {
        assert_eq!(
            info.available,
            info.version.is_some(),
            "{}: availability and version must agree",
            info.name
        );
        assert!(
            info.available || info.devices.is_empty(),
            "{}: an absent library must not enumerate devices",
            info.name
        );
    }
}

#[test]
fn a_device_table_always_names_a_default() {
    for info in libraries() {
        if info.devices.is_empty() {
            continue;
        }
        assert!(
            info.devices.iter().any(|device| device.is_default),
            "{}: {} device(s) reported, none of them the default",
            info.name,
            info.devices.len()
        );
    }
}

#[test]
fn metric_availability_lists_vship_fmetrics_and_vmaf_in_order() {
    let names: Vec<&str> = libraries().iter().map(|info| info.name).collect();
    assert_eq!(names, ["libvship", "fmetrics", "libvmaf"]);
}

#[test]
fn a_reported_model_exists_on_disk() {
    for model in VmafModel::STOCK {
        let Some(version) = model.stock_version() else {
            continue;
        };
        let found = model.find_stock_model_file();
        assert!(
            found.as_ref().is_none_or(|path| path.is_file()),
            "{version}: reported path {found:?} is not a file"
        );
    }
}

#[test]
fn at_most_one_device_is_the_default() {
    for info in libraries() {
        let defaults: Vec<u32> = info
            .devices
            .iter()
            .filter(|device| device.is_default)
            .map(|device| device.id)
            .collect();
        assert!(
            defaults.len() <= 1,
            "{}: more than one device claims to be the default: {defaults:?}",
            info.name
        );
    }
}

#[test]
fn every_reported_device_is_named() {
    for info in libraries() {
        for device in &info.devices {
            assert!(
                !device.name.trim().is_empty(),
                "{}: device {} has no name",
                info.name,
                device.id
            );
        }
    }
}
