//! Private source-planner, additional-node and Python-literal boundary evidence.
use super::*;
use crate::vapoursynth::script_builder::script::Lines;
use std::{fs, path::Path, process::Command};

#[path = "../../../../../tests/core/input/vapoursynth/fixtures/source_options_support.rs"]
mod fixture;
use fixture::{Fixture, PYTHON_LITERAL_STRINGS, expected_frame, recorded_script, run};

fn files(path: &Path) -> Vec<std::path::PathBuf> {
    if path.is_file() {
        return vec![path.to_owned()];
    }
    if !path.exists() {
        return Vec::new();
    }
    fs::read_dir(path)
        .expect("cache listing")
        .flat_map(|entry| files(&entry.expect("cache entry").path()))
        .collect()
}

fn routes(method: fn(Option<u8>) -> VapourSynthImportMethod, additional_readers: bool) {
    let fixture = Fixture::new();
    for track in [Some(1), Some(0), None] {
        for scripted in [false, true] {
            let cache = fixture.cache(&format!("{track:?}-{scripted}"));
            let data = InputModel::VapourSynth {
                path: fixture.source.clone(),
                import_method: method(track),
                cache_path: Some(cache.clone()),
                filters: vec![VapourSynthFilter::Crop {
                    left: Some(2),
                    right: Some(2),
                    top: None,
                    bottom: None,
                }],
            };
            // Guard the actual source planner before a dropped BestSource cache
            // option could create a default index outside the owned fixture.
            let (_, lines) = source_script(&fixture.source, &method(track), Some(&cache))
                .expect("source options");
            let options = recorded(lines);
            let key = if matches!(method(track), VapourSynthImportMethod::BestSource { .. }) {
                "cachepath"
            } else {
                "cachefile"
            };
            assert_eq!(options[0][key], cache.display().to_string());
            let mut input = if scripted {
                Input::from_vapoursynth_scripted(&data)
            } else {
                Input::from_data(&data)
            }
            .expect("installed source plugin must open with explicit track/cache");
            assert_eq!(
                input.y4m_frame(0).expect("selected image").into_inner(),
                expected_frame(track, 28),
                "track={track:?}, scripted={scripted}"
            );
            let info = input.clip_info().expect("clip metadata");
            assert_eq!(info.resolution, (28, 16));
            assert_eq!(info.num_frames, 100);
            assert_eq!(
                (*info.frame_rate.numer(), *info.frame_rate.denom()),
                (24, 1)
            );
            assert!(input.streams_concurrently());
            if additional_readers && !scripted {
                let sources = input.frame_sources(5).expect("additional source instances");
                let crate::core::input::FrameSourceKind::VapourSynth(node) = &sources[4].kind
                else {
                    panic!("VS frame source");
                };
                let image = write_y4m_frame(&node.get_frame(0).expect("additional source frame"))
                    .expect("additional source pixels");
                assert_eq!(image, expected_frame(track, 28)[6..]);
            }
            drop(input);
            let caches = files(cache.parent().expect("cache parent"));
            assert!(
                !caches.is_empty(),
                "explicit cache must be created: {cache:?}"
            );
            if matches!(method(track), VapourSynthImportMethod::BestSource { .. }) {
                assert!(caches.iter().all(|path| path.starts_with(&cache)));
            } else {
                assert_eq!(caches, [cache]);
            }
            eprintln!("source-options {track:?} scripted={scripted}: {caches:?}");
            fixture.unchanged();
        }
    }
}

pub(super) fn recorded(lines: Lines) -> serde_json::Value {
    fixture::recorded(lines)
}

fn check_existing_dg_source_planning() {
    let directory = tempfile::tempdir().expect("DG planning fixture, not a native decode test");
    let media = directory.path().join("media 映像.mkv");
    fs::write(&media, b"CPU planning fixture").expect("media placeholder");
    let explicit = directory.path().join("chosen cache 索引.dgi");
    let default = media.with_extension("dgi");
    let structural_sample = include_bytes!("../../../../vapoursynth/plugins/dgindexnv_sample.dgi");
    fs::write(&explicit, structural_sample).expect("explicit index fixture");
    fs::write(&default, structural_sample).expect("default index fixture");
    let method = VapourSynthImportMethod::DGDecNV {
        dgindexnv_executable: Some(directory.path().join("must-not-run")),
    };
    for (path, cache, expected) in [
        (&media, Some(explicit.as_path()), &explicit),
        (&media, None, &default),
        (&explicit, None, &explicit),
    ] {
        let source =
            source_path(path, &method, cache).expect("reuse existing .dgi; no indexer installed");
        assert_eq!(&source, expected);
        let call = SourceCall::new(&method);
        let (script, variables) = call.script(&source, cache).expect("direct source script");
        let expected = serde_json::json!([{"source": expected.display().to_string()}]);
        assert_eq!(recorded_script(&script, &variables), expected);
        let (_, lines) = source_script(path, &method, cache).expect("generated source route");
        assert_eq!(recorded(lines), expected);
        assert!(
            call.cache_argument.is_none(),
            "additional nodes must not receive an invented cache keyword"
        );
    }
}

fn check_python_string_literal_roundtrips() {
    use crate::vapoursynth::script_builder::script::python_string_literal;
    let cases: Vec<_> = PYTHON_LITERAL_STRINGS
        .iter()
        .map(|value| (python_string_literal(value), value))
        .collect();
    let input = tempfile::NamedTempFile::new().expect("literal cases");
    fs::write(
        input.path(),
        serde_json::to_vec(&cases).expect("cases JSON"),
    )
    .expect("write cases");
    run(Command::new("timeout")
        .args([
            "10s",
            "python3",
            "-c",
            r#"
import ast
import json
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    cases = json.load(handle)
for literal, expected in cases:
    compile(literal, "<python-literal literal>", "eval")
    assert ast.literal_eval(literal) == expected
"#,
        ])
        .arg(input.path()));
}

// source_script and frame_sources expose private planner/node assertions.
mod internal_integration {
    use super::*;

    #[test]
    fn ffms2_direct_and_scripted_inputs_preserve_track_and_cache() {
        routes(|index| VapourSynthImportMethod::FFMS2 { index }, false);
    }

    #[test]
    fn ffms2_additional_readers_preserve_selected_track() {
        routes(|index| VapourSynthImportMethod::FFMS2 { index }, true);
    }

    #[test]
    fn bestsource_direct_and_scripted_inputs_preserve_track_and_cache() {
        routes(|index| VapourSynthImportMethod::BestSource { index }, false);
    }

    #[test]
    fn bestsource_additional_readers_preserve_selected_track() {
        routes(|index| VapourSynthImportMethod::BestSource { index }, true);
    }

    #[test]
    fn lsmash_direct_and_scripted_inputs_preserve_track_and_cache() {
        routes(
            |index| VapourSynthImportMethod::LSMASHWorks { index },
            false,
        );
    }

    #[test]
    fn lsmash_additional_readers_preserve_selected_track() {
        routes(|index| VapourSynthImportMethod::LSMASHWorks { index }, true);
    }

    #[test]
    fn dg_source_planning_reuses_selected_index_without_spawning_indexer() {
        check_existing_dg_source_planning();
    }

    #[test]
    fn python_string_literals_preserve_quotes_controls_and_unicode() {
        check_python_string_literal_roundtrips();
    }
}
