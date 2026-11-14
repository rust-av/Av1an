//! Runtime availability smoke check; deterministic candidate tables live beside
//! the private production candidate builder in `src/discovery.rs`.

use av_metrics_vship::ffi::VshipApi;

/// Missing libraries are ordinary on CI; a successfully loaded API has a version.
#[test]
fn an_absent_library_is_reported_not_fatal() {
    let Ok(api) = VshipApi::load() else {
        assert!(
            !av_metrics_vship::is_available(),
            "an unloadable libvship must report as unavailable"
        );
        return;
    };

    assert!(!api.version_string().is_empty());
}
