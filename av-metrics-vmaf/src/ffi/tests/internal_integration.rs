#[path = "abi.rs"]
mod abi;

use super::*;

#[cfg(target_os = "linux")]
#[test]
fn cleanup_uses_process_allocator_scope() {
    let directory = tempfile::tempdir().expect("temporary allocator-scope fixture");
    let path = directory.path().join("liballocator_scope.so");
    let output = std::process::Command::new("cc")
        .args(["-shared", "-fPIC", "-std=c99", "-Wall", "-Wextra", "-Werror", "-o"])
        .arg(&path)
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/allocator_scope.c"
        ))
        .output()
        .expect("allocator-scope regression requires cc");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // SAFETY: our fixture has no initializers. Library::new uses RTLD_LOCAL
    // without DEEPBIND, so its decoy cannot interpose on other loaded code.
    let library = unsafe { Library::new(&path) }.expect("load local decoy fixture");
    // SAFETY: exact fixture signatures; all mappings stay live. Compare
    // addresses only: neither the decoy nor selected free receives a pointer.
    let (decoy, bound) = unsafe {
        let decoy = *library.get::<CudaStateFree>(b"free\0").expect("local decoy free");
        let getter = library
            .get::<unsafe extern "C" fn() -> CudaStateFree>(b"cuda_fixture_bound_free\0")
            .expect("ordinary relocation getter");
        (decoy, getter())
    };
    assert_ne!(
        decoy as usize, bound as usize,
        "fixture must distinguish handle and global binding"
    );
    let selected = cuda_state_free().expect("supported Linux resolves native free");
    assert_eq!(
        selected as usize, bound as usize,
        "production cleanup must match ordinary process-global allocator binding, not the local decoy"
    );
}

#[test]
fn bare_library_names_are_always_offered() {
    let candidates = library_candidates();
    assert!(!candidates.is_empty());
    assert!(
        candidates
            .iter()
            .any(|path| path.parent().is_none_or(|parent| parent.as_os_str().is_empty()))
    );
}
