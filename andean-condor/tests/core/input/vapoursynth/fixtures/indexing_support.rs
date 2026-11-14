//! Test-only DG supervisor and owned artifact assertions shared across boundaries.
use super::SAMPLE;
use std::{fs, path::Path, process::Command, thread};

pub(super) fn run_index_cases(test: &str, cases: &[&str], assert_case: impl FnOnce()) {
    if std::env::var_os("VS_INDEX_ROOT").is_some() {
        let case = std::env::var("VS_INDEX_CASE").expect("case");
        assert!(
            cases.contains(&case.as_str()),
            "selected indexing case belongs to this contract"
        );
        assert_case();
        println!("dg-index {case}: assertions completed");
        return;
    }
    let dir = tempfile::tempdir().expect("fixture");
    fs::write(
        dir.path().join("fake.rs"),
        include_str!("../../../encoder/fixtures/lifecycle_process.rs"),
    )
    .expect("fixture source");
    let status = Command::new("rustc")
        .arg(dir.path().join("fake.rs"))
        .arg("-o")
        .arg(dir.path().join("fake"))
        .status()
        .expect("rustc");
    assert!(status.success());
    for case in cases {
        let root = dir.path().join(case);
        fs::create_dir_all(&root).expect("case");
        fs::write(root.join("sample.dgi"), SAMPLE).expect("sample");
        std::os::unix::fs::symlink(dir.path().join("fake"), root.join("fake")).expect("executable");
        let output = Command::new(root.join("fake"))
            .arg("supervise-index-publication")
            .arg(std::env::current_exe().expect("test binary"))
            .arg(&root)
            .arg(case)
            .arg(test)
            .output()
            .expect("supervisor");
        let log = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        eprintln!("{log}");
        assert!(output.status.success(), "{case}: {log}");
        assert!(
            log.contains("test result: ok. 1 passed;"),
            "exact child selected: {log}"
        );
        assert!(
            log.contains(&format!("dg-index {case}: assertions completed")),
            "index assertions executed: {log}"
        );
        assert!(
            log.contains(&format!(
                "process lifecycle case={case} selected=true rescued=0"
            )),
            "{log}"
        );
    }
}

pub(super) fn wait_file(root: &Path, name: &str) {
    while !root.join(name).exists() {
        thread::yield_now();
    }
}

pub(super) fn assert_clean(root: &Path) {
    assert!(
        !fs::read_dir(root).expect("listing").any(|entry| entry
            .expect("entry")
            .file_name()
            .to_string_lossy()
            .starts_with(".av1an-dgi-")),
        "owned temp directory must be gone after readers and child join"
    );
    assert_eq!(
        fs::read(root.join("sentinel")).expect("sentinel"),
        b"caller-owned"
    );
}

pub(super) fn assert_argv(root: &Path, source: &Path, cache: &Path) {
    for entry in fs::read_dir(root).expect("listing") {
        let path = entry.expect("entry").path();
        if !path.file_name().expect("name").to_string_lossy().starts_with("argv-") {
            continue;
        }
        let text = fs::read_to_string(path).expect("argv");
        let argv: Vec<_> = text.lines().collect();
        assert_eq!(
            &argv[..4],
            &["-h", "-i", source.to_str().expect("source"), "-o"]
        );
        assert_eq!(argv.len(), 5);
        let output = Path::new(argv[4]);
        assert!(output.is_absolute());
        assert_ne!(output, cache, "never give the final path to the indexer");
        assert_eq!(output.extension().expect("extension"), "dgi");
        assert_eq!(
            output.parent().expect("owned directory").parent(),
            cache.parent()
        );
        assert!(!output.exists(), "private output removed after completion");
    }
}
