use super::*;

#[path = "discovery.rs"]
mod discovery;
#[path = "transactions.rs"]
mod transactions;

#[test]
fn bare_vsscript_does_not_select_media_cwd() {
    assert_eq!(
        windows_script_directory(Path::new("vsscript.dll")).expect("bare script"),
        None
    );
}

#[test]
fn qualified_relative_vsscript_directory_becomes_absolute() {
    let directory = windows_script_directory(Path::new("./runtime/vsscript.dll"))
        .expect("relative script")
        .expect("intentional directory");
    assert!(
        directory.is_absolute(),
        "an intentional runtime directory must be qualified"
    );
}

#[test]
fn reported_system_directories_are_absolute_existing_and_nonempty() {
    // Windows can live on a different drive; trust the OS, not a fixed C: path.
    for directory in system_directories() {
        assert!(directory.is_absolute(), "{directory:?} should be absolute");
        assert!(
            directory.is_dir(),
            "the reported system directory {directory:?} does not exist"
        );
    }
    assert!(
        system_directories().iter().all(|directory| !directory.as_os_str().is_empty()),
        "an empty system directory would produce a candidate of just the file name"
    );
}
