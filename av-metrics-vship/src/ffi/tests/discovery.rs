//! Explicit discovery inputs: no parent-process environment or CWD changes.

use super::*;

#[cfg(target_os = "windows")]
const NAMES: [&str; 2] = ["libvship.dll", "vship.dll"];
#[cfg(target_os = "macos")]
const NAMES: [&str; 2] = ["libvship.dylib", "libvship.2.dylib"];
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const NAMES: [&str; 2] = ["libvship.so", "libvship.so.2"];

fn expected(directories: &[PathBuf]) -> Vec<PathBuf> {
    directories
        .iter()
        .flat_map(|directory| NAMES.map(|name| directory.join(name)))
        .chain(NAMES.map(PathBuf::from))
        .collect()
}

fn cases(root: &Path) -> Vec<(&'static str, CandidateDirectories, Vec<PathBuf>)> {
    let directory = |name: &str| root.join(name);
    let system = vec![directory("system-a"), directory("system-b")];
    let mut cases = Vec::new();
    for (label, overrides, ordered) in [
        ("absent", [None, None], vec![]),
        ("plugin override", [Some("plugin"), None], vec!["plugin"]),
        ("library override", [None, Some("library")], vec!["library"]),
        (
            "both overrides",
            [Some("plugin"), Some("library")],
            vec!["plugin", "library"],
        ),
        ("empty overrides", [Some(""), Some("")], vec![]),
    ] {
        let inputs = CandidateDirectories {
            overrides: overrides.map(|value| {
                value.and_then(|value| {
                    let path = if value.is_empty() {
                        PathBuf::new()
                    } else {
                        directory(value)
                    };
                    directory_override(path.as_os_str()).expect("explicit fixture directory")
                })
            }),
            system: system.clone(),
            ..CandidateDirectories::default()
        };
        let mut ordered: Vec<_> = ordered.into_iter().map(directory).collect();
        ordered.extend(system.clone());
        cases.push((label, inputs, expected(&ordered)));
    }
    for (label, script, ordered) in [
        (
            "qualified script",
            directory("runtime/vsscript.dll"),
            vec![directory("runtime/plugins"), directory("runtime")],
        ),
        ("empty script", PathBuf::new(), vec![]),
        ("bare script", PathBuf::from("vsscript.dll"), vec![]),
    ] {
        let inputs = CandidateDirectories {
            runtime: script_runtime_directory(script.as_os_str()),
            system: system.clone(),
            ..CandidateDirectories::default()
        };
        let mut ordered = ordered;
        ordered.extend(system.clone());
        cases.push((label, inputs, expected(&ordered)));
    }
    // Non-Windows preserves intentionally qualified relative path semantics.
    // Windows normalization is covered by the script-directory integration tests.
    #[cfg(not(target_os = "windows"))]
    cases.push((
        "qualified relative script",
        CandidateDirectories {
            runtime: script_runtime_directory(std::ffi::OsStr::new("./runtime/vsscript.dll")),
            system: system.clone(),
            ..CandidateDirectories::default()
        },
        expected(&[
            PathBuf::from("./runtime/plugins"),
            PathBuf::from("./runtime"),
            system[0].clone(),
            system[1].clone(),
        ]),
    ));
    // join_paths retains the empty entries that the production parser must ignore.
    let extra = std::env::join_paths([
        PathBuf::new(),
        directory("extra-a"),
        PathBuf::new(),
        directory("extra-b"),
        PathBuf::new(),
    ])
    .expect("portable extra-plugin list");
    let extra = extra_plugin_directories(&extra);
    cases.push((
        "extra list",
        CandidateDirectories {
            extra: extra.clone(),
            system: system.clone(),
            ..CandidateDirectories::default()
        },
        expected(&[
            directory("extra-a"),
            directory("extra-b"),
            system[0].clone(),
            system[1].clone(),
        ]),
    ));
    cases.push((
        "all sources in order",
        CandidateDirectories {
            overrides: [Some(directory("plugin")), Some(directory("library"))],
            runtime: script_runtime_directory(directory("runtime/vsscript.dll").as_os_str()),
            extra,
            system: system.clone(),
        },
        expected(&[
            directory("plugin"),
            directory("library"),
            directory("runtime/plugins"),
            directory("runtime"),
            directory("extra-a"),
            directory("extra-b"),
            system[0].clone(),
            system[1].clone(),
        ]),
    ));
    cases.push((
        "bare fallback only",
        CandidateDirectories::default(),
        expected(&[]),
    ));
    cases
}

fn fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("discovery fixture");
    for name in [
        "plugin",
        "library",
        "runtime/plugins",
        "extra-a",
        "extra-b",
        "system-a",
        "system-b",
    ] {
        std::fs::create_dir_all(root.path().join(name)).expect("existing discovery directory");
    }
    root
}

macro_rules! discovery_case {
    ($name:ident, $label:literal) => {
        #[test]
        fn $name() {
            assert_candidate_order($label);
        }
    };
}

discovery_case!(
    absent_overrides_use_system_directories_then_bare_names,
    "absent"
);
discovery_case!(
    plugin_override_precedes_system_candidates,
    "plugin override"
);
discovery_case!(
    library_override_precedes_system_candidates,
    "library override"
);
discovery_case!(plugin_override_precedes_library_override, "both overrides");
discovery_case!(empty_overrides_are_ignored, "empty overrides");
discovery_case!(
    qualified_script_adds_plugins_then_runtime_directory,
    "qualified script"
);
discovery_case!(empty_script_adds_no_runtime_directory, "empty script");
discovery_case!(bare_script_adds_no_media_directory, "bare script");
#[cfg(not(target_os = "windows"))]
discovery_case!(
    qualified_relative_script_retains_intentional_path_on_unix,
    "qualified relative script"
);
discovery_case!(
    extra_plugin_list_ignores_empty_entries_and_preserves_order,
    "extra list"
);
discovery_case!(
    candidate_sources_follow_override_runtime_extra_system_bare_order,
    "all sources in order"
);
discovery_case!(
    empty_candidate_inputs_offer_only_bare_names,
    "bare fallback only"
);

fn assert_candidate_order(label: &str) {
    let root = fixture();
    let (_, inputs, want) = cases(root.path())
        .into_iter()
        .find(|(name, _, _)| *name == label)
        .expect("registered discovery case");
    assert_eq!(
        library_candidates_from_directories(&inputs),
        want,
        "{label}"
    );
}

#[test]
fn concurrent_candidate_builds_preserve_each_threads_owned_inputs() {
    let root = fixture();
    let cases = cases(root.path());
    let start = std::sync::Arc::new(std::sync::Barrier::new(cases.len()));
    let workers: Vec<_> = cases
        .into_iter()
        .map(|(label, inputs, want)| {
            let start = std::sync::Arc::clone(&start);
            std::thread::spawn(move || {
                start.wait();
                for _ in 0..32 {
                    assert_eq!(
                        library_candidates_from_directories(&inputs),
                        want,
                        "{label}"
                    );
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().expect("owned discovery case must pass");
    }
}
