#[cfg(unix)]
mod cuda;

use super::*;

#[test]
fn backend_display_is_lowercase() {
    assert_eq!(Backend::Cpu.to_string(), "cpu");
    assert_eq!(Backend::Cuda.to_string(), "cuda");
}

/// Undersizing the pool here is invisible to libvmaf — it skips its own
/// auto-sizing when a pool exists — so the formula is asserted directly.
#[test]
fn the_picture_pool_mirrors_libvmaf_thread_sizing() {
    assert_eq!(picture_pool_size(0), 4, "the serial floor keeps a pair fed");
    assert_eq!(picture_pool_size(1), 4, "two plus the formula's slack");
    assert_eq!(
        picture_pool_size(6),
        14,
        "libvmaf's own size for six workers"
    );
    assert_eq!(picture_pool_size(12), 26);
    assert_eq!(
        picture_pool_size(u32::MAX),
        u32::MAX,
        "an absurd thread count must saturate rather than overflow"
    );
}

#[test]
fn cpu_only_preference_never_attempts_cuda() {
    // Without libvmaf there is no API to hand, so this exercises the
    // preference branch that must not consult CUDA at all.
    let Err(error) = VmafApi::load() else {
        return;
    };
    assert!(matches!(
        error,
        VmafError::LibraryNotFound
            | VmafError::LibraryLoadFailed { .. }
            | VmafError::IncompatibleLibrary { .. }
    ));
}
