use super::*;

// This synthetic boundary has the same opaque Display/source split as
// libloading's Windows error, without constructing its private WindowsError.
#[derive(Debug, thiserror::Error)]
#[error("LoadLibraryExW failed")]
struct LoaderFailure(#[source] std::io::Error);

#[test]
fn loader_adapter_renders_os_code_and_preserves_source_chain() {
    let error = windows_load_error(LoaderFailure(std::io::Error::from_raw_os_error(126)));
    let rendered = error.to_string();
    assert!(
        rendered.contains("LoadLibraryExW failed"),
        "loader context lost: {rendered}"
    );
    assert!(
        rendered.contains("126"),
        "OS source code lost from Display: {rendered}"
    );

    let mut source: Option<&(dyn std::error::Error + 'static)> =
        Some(error.get_ref().expect("retained error context"));
    let mut original = false;
    let mut os_code = None;
    while let Some(cause) = source {
        original |= cause.is::<LoaderFailure>();
        if let Some(os_error) = cause.downcast_ref::<std::io::Error>() {
            os_code = os_error.raw_os_error();
        }
        source = cause.source();
    }
    assert!(
        original,
        "adapter must retain the typed original, not just its text"
    );
    assert_eq!(os_code, Some(126), "typed OS source must remain reachable");
}

#[test]
fn loader_adapter_renders_legitimate_libloading_source() {
    let source = std::ffi::CString::new(b"a\0b").expect_err("NUL source fixture");
    let error = windows_load_error(libloading::Error::CreateCString { source });
    let rendered = error.to_string();
    assert!(
        rendered.contains("could not create a C string"),
        "libloading context lost: {rendered}"
    );
    assert!(
        rendered.contains("nul byte") && rendered.contains('1'),
        "actual libloading source lost: {rendered}"
    );
}

#[test]
fn loader_adapter_keeps_context_without_source() {
    let error = windows_load_error(libloading::Error::LoadLibraryExWUnknown);
    assert!(
        error
            .to_string()
            .contains("LoadLibraryExW failed, but system did not report the error"),
        "source-free loader failure must still be useful: {error}"
    );
}

#[test]
fn aggregate_diagnostics_keep_all_attempt_paths_and_os_causes() {
    let failures = [
        format!(
            "private/metric.dll: {}",
            windows_load_error(LoaderFailure(std::io::Error::from_raw_os_error(126)))
        ),
        format!(
            "metric.dll: {}",
            windows_load_error(LoaderFailure(std::io::Error::from_raw_os_error(127)))
        ),
    ];
    let rendered = library_load_error(&failures).to_string();
    assert!(
        rendered.contains("private/metric.dll: LoadLibraryExW failed")
            && rendered.contains("; metric.dll: LoadLibraryExW failed"),
        "aggregate lost attempted paths or loader context: {rendered}"
    );
    assert!(
        rendered.contains("126") && rendered.contains("127"),
        "aggregate lost an attempt's OS source: {rendered}"
    );
}

// These checks exercise the production Windows policy on native host paths;
// they do not establish Windows DLL search behavior on Linux.
#[test]
fn qualified_candidates_select_sibling_search() {
    let root = tempfile::tempdir().expect("policy directory");
    let absolute = root.path().join("marker.dll");
    assert_eq!(
        windows_load_policy(&absolute).expect("absolute policy"),
        (absolute, true)
    );
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
    assert_eq!(
        windows_load_policy(Path::new("marker.dll")).expect("bare policy"),
        (PathBuf::from("marker.dll"), false),
        "bare names must use safe defaults without DLL_LOAD_DIR"
    );
    assert!(
        windows_load_policy(Path::new("")).is_err(),
        "empty paths must not select CWD"
    );
}

#[test]
fn explicit_directories_are_validated_without_losing_unicode() {
    let root = tempfile::tempdir().expect("directory policy fixture");
    let unicode = root.path().join("日本語-é");
    std::fs::create_dir_all(&unicode).expect("Unicode directory");
    assert_eq!(
        windows_directory_override(unicode.as_os_str()).expect("Unicode override"),
        Some(unicode)
    );
    let relative = windows_directory_override(std::ffi::OsStr::new("."))
        .expect("explicit current directory")
        .expect("nonempty selection");
    assert!(
        relative.is_absolute(),
        "an explicit relative directory is intentional, but must be qualified"
    );
    assert!(
        windows_directory_override(root.path().join("missing").as_os_str()).is_err(),
        "missing directories must not become candidates"
    );
    let file = root.path().join("not-a-directory.dll");
    std::fs::write(&file, b"not a DLL").expect("file override fixture");
    assert!(
        windows_directory_override(file.as_os_str()).is_err(),
        "DLL filenames are not directory overrides"
    );
    assert_eq!(
        windows_directory_override(std::ffi::OsStr::new("")).expect("empty absent override"),
        None
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
