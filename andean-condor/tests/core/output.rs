#[cfg(unix)]
use std::path::Path;
use std::{fs, path::PathBuf};

use andean_condor::core::output::admit_output;

fn source_fixture() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("fixture");
    let source = dir.path().join("source");
    fs::write(&source, b"source bytes").expect("source");
    (dir, source)
}

#[test]
fn output_admission_rejects_source_aliases_even_with_overwrite_consent() {
    let (dir, source) = source_fixture();
    for overwrite in [false, true] {
        for alias in
            [source.clone(), dir.path().join("./source"), dir.path().join("absent/../source")]
        {
            assert!(admit_output(&alias, std::slice::from_ref(&source), overwrite, false).is_err());
        }
        let absent = dir.path().join("absent/input");
        assert!(admit_output(&absent, std::slice::from_ref(&absent), overwrite, false).is_err());
    }
    assert_eq!(fs::read(source).expect("source"), b"source bytes");
}

#[test]
fn existing_output_requires_explicit_overwrite_consent() {
    let (dir, source) = source_fixture();
    let old = dir.path().join("out");
    fs::write(&old, b"old final bytes").expect("old");
    assert!(admit_output(&old, std::slice::from_ref(&source), false, true).is_err());
    admit_output(&old, std::slice::from_ref(&source), true, true).expect("explicit consent");
    assert_eq!(fs::read(source).expect("source"), b"source bytes");
    assert_eq!(fs::read(old).expect("old"), b"old final bytes");
}

#[test]
fn output_admission_checks_parent_and_leaf_without_creating_paths() {
    let (dir, source) = source_fixture();
    let missing_parent = dir.path().join("missing/out");
    admit_output(&missing_parent, std::slice::from_ref(&source), false, false)
        .expect("offline missing parent");
    assert!(admit_output(&missing_parent, std::slice::from_ref(&source), false, true).is_err());
    assert!(!missing_parent.parent().expect("parent").exists());
    assert!(admit_output(dir.path(), &[], true, true).is_err());
    assert!(admit_output(&source.join("out"), &[], true, false).is_err());
    assert!(admit_output(&dir.path().join("invalid\0leaf"), &[], true, false).is_err());
    assert_eq!(fs::read(source).expect("source"), b"source bytes");
}

#[cfg(unix)]
#[test]
fn output_admission_rejects_hardlink_and_symlink_source_aliases() {
    use std::os::unix::fs::symlink;
    let (dir, source) = source_fixture();
    let hard = dir.path().join("hard");
    fs::hard_link(&source, &hard).expect("known Unix hardlink support");
    let link = dir.path().join("link");
    symlink(&source, &link).expect("link");
    for alias in [&hard, &link] {
        assert!(admit_output(alias, std::slice::from_ref(&source), true, true).is_err());
    }
    let real = dir.path().join("real");
    fs::create_dir_all(real.join("child")).expect("real parent");
    let parent = dir.path().join("parent");
    symlink(real.join("child"), &parent).expect("parent link");
    assert!(admit_output(&parent.join("new"), &[real.join("child/new")], true, false).is_err());
    assert!(admit_output(&parent.join("../new"), &[real.join("new")], true, false).is_err());
    admit_output(
        &parent.join("../source"),
        std::slice::from_ref(&source),
        false,
        true,
    )
    .expect("symlink/.. is not lexical parent");
    assert_eq!(fs::read(source).expect("source"), b"source bytes");
}

#[cfg(unix)]
#[test]
fn output_admission_rejects_dangling_links_and_special_files() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().expect("fixture");
    let dangling = dir.path().join("dangling");
    symlink(dir.path().join("gone"), &dangling).expect("dangling");
    assert!(admit_output(&dangling, &[], true, false).is_err());
    let looped = dir.path().join("loop");
    symlink(&looped, &looped).expect("loop");
    assert!(admit_output(&looped, &[], true, false).is_err());
    assert!(admit_output(Path::new("/dev/null"), &[], true, true).is_err());
}

#[cfg(unix)]
#[test]
fn output_admission_preserves_permission_denial_errors() {
    use std::os::unix::fs::PermissionsExt;
    let (dir, source) = source_fixture();
    let denied = dir.path().join("denied");
    fs::create_dir_all(&denied).expect("denied parent");
    fs::set_permissions(&denied, fs::Permissions::from_mode(0o0)).expect("deny");
    let probe = fs::metadata(denied.join("out"));
    let result = admit_output(&denied.join("out"), &[], false, false);
    fs::set_permissions(&denied, fs::Permissions::from_mode(0o700)).expect("restore");
    if probe
        .as_ref()
        .is_err_and(|error| error.kind() == std::io::ErrorKind::PermissionDenied)
    {
        assert!(result.is_err(), "actual denial must not be absence");
    } else {
        eprintln!("final-output permission denial unavailable: filesystem probe={probe:?}");
        result.expect("privileged traversal is not a false denial");
    }
    assert_eq!(fs::read(source).expect("source"), b"source bytes");
}
