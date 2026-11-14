//! Host-path selection checks do not establish Windows DLL loading on Linux.
use super::*;

#[cfg(target_os = "windows")]
#[path = "../windows_loader.rs"]
mod windows_loader;

#[test]
fn absolute_loader_candidate_enables_sibling_search() {
    let root = tempfile::tempdir().expect("policy directory");
    let absolute = root.path().join("marker.dll");
    assert_eq!(
        windows_load_policy(&absolute).expect("absolute policy"),
        (absolute, true)
    );
}

#[test]
fn qualified_relative_loader_candidate_becomes_absolute_with_sibling_search() {
    let (relative, siblings) = windows_load_policy(Path::new("./runtime/marker.dll"))
        .expect("intentional relative policy");
    assert!(
        relative.is_absolute(),
        "qualified relative paths must not be searched as bare names"
    );
    assert!(
        siblings,
        "qualified candidates must search their sibling dependencies"
    );
}

#[test]
fn unicode_directory_override_is_preserved() {
    let root = tempfile::tempdir().expect("directory policy fixture");
    let unicode = root.path().join("日本語-é");
    std::fs::create_dir_all(&unicode).expect("Unicode directory");
    assert_eq!(
        windows_directory_override(unicode.as_os_str()).expect("Unicode override"),
        Some(unicode)
    );
}

#[test]
fn relative_directory_override_is_qualified() {
    let relative = windows_directory_override(std::ffi::OsStr::new("."))
        .expect("explicit current directory")
        .expect("nonempty selection");
    assert!(
        relative.is_absolute(),
        "an explicit relative directory is intentional, but must be qualified"
    );
}

#[test]
fn missing_directory_override_is_rejected() {
    let root = tempfile::tempdir().expect("directory policy fixture");
    assert!(
        windows_directory_override(root.path().join("missing").as_os_str()).is_err(),
        "missing directories must not become candidates"
    );
}

#[test]
fn dll_file_override_is_not_treated_as_directory() {
    let root = tempfile::tempdir().expect("directory policy fixture");
    let file = root.path().join("not-a-directory.dll");
    std::fs::write(&file, b"not a DLL").expect("file override fixture");
    assert!(
        windows_directory_override(file.as_os_str()).is_err(),
        "DLL filenames are not directory overrides"
    );
}

#[cfg(target_os = "windows")]
#[test]
fn windows_drive_and_unc_paths_are_qualified() {
    for value in [
        r"C:\trusted\marker.dll",
        r"\\server\share\marker.dll",
        r"\runtime\marker.dll",
        r"C:runtime\marker.dll",
    ] {
        let (path, siblings) =
            windows_load_policy(Path::new(value)).expect("Windows path qualification");
        assert!(
            path.is_absolute() && siblings,
            "qualified Windows path lost full-path sibling policy: {path:?}"
        );
    }
}
