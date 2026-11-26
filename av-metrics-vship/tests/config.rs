//! Public device-default resolution crosses the native discovery boundary.
#![cfg(test)]

#[test]
fn default_config_leaves_device_unset_and_resolves_a_bounded_fallback() {
    let config = av_metrics_vship::VshipConfig::default();
    assert!(!config.has_explicit_gpu_id());
    assert!(config.gpu_id_or_default() < u32::MAX);
}
