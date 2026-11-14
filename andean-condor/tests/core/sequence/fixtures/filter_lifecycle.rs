//! Test-process isolation is shared; private worker assertions stay in the caller.
use super::{ImportMethod, Input, InputModel};
use std::{fs, path::Path, process::Command};

pub(super) fn isolated(module: &str, test: &str, cases: &[&str], check: impl FnOnce(&Path, &str)) {
    if let Some(root) = std::env::var_os("ENCODER_FIXTURE_ROOT") {
        let root = std::path::PathBuf::from(root);
        let case = std::env::var("ENCODER_FIXTURE_CASE").expect("case");
        assert!(
            cases.contains(&case.as_str()),
            "case belongs to this contract"
        );
        fs::write(root.join("executed"), &case).expect("handshake");
        check(&root, &case);
        fs::write(root.join("verified"), "all assertions ran").expect("verification handshake");
        return;
    }
    let module = module.split_once("::").expect("test crate prefix").1;
    let exact = format!("{module}::{test}");
    let temp = tempfile::tempdir().expect("fixture");
    let root = temp.path();
    fs::write(
        root.join("watchdog.rs"),
        include_str!("../../encoder/fixtures/fake_encoder.rs"),
    )
    .expect("watchdog source");
    let compiled = Command::new("rustc")
        .arg(root.join("watchdog.rs"))
        .arg("-o")
        .arg(root.join("watchdog"))
        .output()
        .expect("rustc");
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let mut failures = Vec::new();
    for case in cases {
        let job = root.join(case);
        fs::create_dir_all(&job).expect("job");
        let log = fs::File::create(job.join("child.log")).expect("log");
        let status = Command::new(root.join("watchdog"))
            .arg("--encoder-fixture-watchdog")
            .arg(std::env::current_exe().expect("test binary"))
            .arg(&job)
            .arg(case)
            .arg(&exact)
            .stdout(log.try_clone().expect("clone log"))
            .stderr(log)
            .status()
            .expect("watchdog");
        let log = fs::read_to_string(job.join("child.log")).expect("log");
        if !status.success()
            || !job.join("executed").exists()
            || !job.join("verified").exists()
            || !log.contains("test result: ok. 1 passed;")
            || !log
                .lines()
                .any(|l| l == "encoder fixture watchdog reaped 0 descendants; timeout=false")
        {
            failures.push(format!("{case}: {status}\n{log}"));
        }
        eprint!("{log}");
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

pub(super) fn native_input(root: &Path) -> Input {
    let path = root.join("96 frames.y4m");
    let mut bytes = b"YUV4MPEG2 W128 H128 F24:1 Ip A1:1 C420jpeg\n".to_vec();
    for _ in 0..96 {
        bytes.extend_from_slice(b"FRAME\n");
        bytes.extend(vec![48; 128 * 128]);
        bytes.extend(vec![128; 128 * 128 / 2]);
    }
    fs::write(&path, bytes).expect("finite source larger than the channel window");
    let mut input = Input::from_video(&InputModel::Video {
        path,
        import_method: ImportMethod::FFMS2 {
            index: None,
            cache_path: Some(root.join("native.ffindex")),
        },
        filters: vec![],
    })
    .expect("native FFMS2");
    assert!(
        input.streams_concurrently(),
        "exercise actual streaming path"
    );
    assert_eq!(input.clip_info().expect("clip").num_frames, 96);
    input
}
