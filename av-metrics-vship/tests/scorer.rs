//! Optional public device discovery; absence is not GPU scoring proof.
#![cfg(test)]

use av_metrics_vship::{VshipScorer, default_gpu_id, ffi::VshipApi};

#[test]
fn the_default_device_prefers_a_discrete_gpu() {
    if VshipApi::load().is_err() {
        return;
    }
    let Ok(api) = VshipApi::load() else {
        return;
    };
    let Ok(count) = api.device_count() else {
        return;
    };
    let Some(chosen) = default_gpu_id() else {
        assert_eq!(count, 0, "a device exists, so one must have been chosen");
        return;
    };
    assert!(
        chosen < count,
        "the chosen device {chosen} must exist among {count}"
    );
    if let Ok(info) = api.device_info(chosen) {
        let any_discrete =
            (0..count).any(|id| api.device_info(id).is_ok_and(|info| info.integrated == 0));
        if any_discrete {
            assert_eq!(
                info.integrated, 0,
                "a discrete device is available, so the default must not be integrated"
            );
        }
    }
}

#[test]
fn repeated_default_device_queries_return_the_same_option() {
    assert_eq!(default_gpu_id(), default_gpu_id());
}

#[test]
fn availability_is_total_and_never_panics() {
    match VshipScorer::is_available() {
        true => assert!(VshipScorer::libvship_version().is_some()),
        false => assert_eq!(VshipScorer::libvship_version(), None),
    }
    let _ = VshipScorer::device_name();
    let _ = VshipScorer::device_info();
}
