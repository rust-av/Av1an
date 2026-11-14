use super::*;

#[cfg(unix)]
#[path = "cuda.rs"]
mod cuda;

#[test]
fn unavailable_vmaf_loader_reports_a_supported_load_error() {
    let Err(error) = VmafApi::load() else {
        eprintln!("load succeeded; unavailable-loader contract is not exercised");
        return;
    };
    assert!(matches!(
        error,
        VmafError::LibraryNotFound
            | VmafError::LibraryLoadFailed { .. }
            | VmafError::IncompatibleLibrary { .. }
    ));
}
