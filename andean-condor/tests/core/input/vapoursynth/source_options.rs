use super::*;
use andean_condor::core::input::Ffms2Filter;
#[cfg(unix)]
use andean_condor::models::input::VapourSynthImportMethod;
use andean_condor::vapoursynth::{
    plugins::{
        bestsource::VideoSource, dgdecodenv::DGSource, ffms2::Source, lsmash::LWLibavSource,
    },
    script_builder::{
        VapourSynthPluginScript,
        script::{Lines, VapourSynthScript},
    },
};
use std::fs;
use std::{path::Path, process::Command};

#[path = "fixtures/source_options_support.rs"]
mod fixture;
use fixture::{Fixture, PYTHON_LITERAL_STRINGS, expected_frame, recorded, run};

#[cfg(unix)]
fn files(path: &std::path::Path) -> Vec<std::path::PathBuf> {
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

#[test]
fn native_metric_reference_preserves_track_cache_and_filter_baseline() {
    let fixture = Fixture::new();
    let cache = fixture.cache("native shared index");
    for track in [Some(1), Some(0), None] {
        let mut native = Input::from_data(&InputModel::Video {
            path: fixture.source.clone(),
            import_method: ImportMethod::FFMS2 {
                index: track,
                cache_path: Some(cache.clone()),
            },
            filters: vec![Ffms2Filter::OutputFormat {
                bit_depth: None,
                chroma: None,
                width: Some(16),
                height: None,
            }],
        })
        .expect("native FFMS2 input");
        let native_image = native.y4m_frame(0).expect("encode feed").into_inner();
        assert_eq!(native_image, expected_frame(track, 16));
        let original_index = fs::read(&cache).expect("native index");
        let mut reference = native
            .as_vapoursynth_script()
            .expect("metric reference")
            .expect("native becomes VS");
        assert_eq!(
            reference.y4m_frame(0).expect("reference image").into_inner(),
            native_image,
            "requested {track:?} must be the metric reference"
        );
        assert_eq!(
            reference.clip_info().expect("metadata").resolution,
            (16, 16)
        );
        let Input::Video { source_details, .. } = native else {
            panic!("native source");
        };
        assert_eq!(source_details.width, 32, "filter baseline stays unfiltered");
        assert_eq!(
            fs::read(&cache).expect("shared index"),
            original_index,
            "VS must read the native FFMS2 index without replacing it"
        );
        fixture.unchanged();
    }
}

#[cfg(unix)]
fn special_filename_routes(method: &VapourSynthImportMethod) {
    let fixture =
        Fixture::with_source_name("both'\"\\\n映像🦀\"+str(marker.append('executed'))+\".mkv");
    for scripted in [false, true] {
        let cache = fixture
            .cache(&format!("special-{scripted}"))
            .with_file_name("cache'\"\\\n索引🦀");
        let data = InputModel::VapourSynth {
            path: fixture.source.clone(),
            import_method: method.clone(),
            cache_path: Some(cache.clone()),
            filters: Vec::new(),
        };
        let mut input = if scripted {
            Input::from_vapoursynth_scripted(&data)
        } else {
            Input::from_data(&data)
        }
        .expect("real plugin must open special filename and explicit cache");
        assert_eq!(
            input.y4m_frame(0).expect("frame").into_inner(),
            expected_frame(Some(1), 32)
        );
        assert_eq!(input.clip_info().expect("metadata").num_frames, 100);
        drop(input);
        let caches = files(cache.parent().expect("cache parent"));
        assert!(!caches.is_empty(), "explicit special cache created");
        if matches!(method, VapourSynthImportMethod::BestSource { .. }) {
            assert!(caches.iter().all(|path| path.starts_with(&cache)));
        } else {
            assert_eq!(caches, [cache]);
        }
        eprintln!(
            "python-literal {method:?} scripted={scripted}: {caches:?}; 100 frames, YUV=219/41/159"
        );
        fixture.unchanged();
    }
    if matches!(method, VapourSynthImportMethod::FFMS2 { .. }) {
        let cache = fixture.cache("native gateway").with_file_name("native'\"\\\n索引🦀");
        let mut native = Input::from_data(&InputModel::Video {
            path: fixture.source.clone(),
            import_method: ImportMethod::FFMS2 {
                index: Some(1),
                cache_path: Some(cache.clone()),
            },
            filters: Vec::new(),
        })
        .expect("native special filename");
        let expected = native.y4m_frame(0).expect("native frame").into_inner();
        assert_eq!(expected, expected_frame(Some(1), 32));
        let original_index = fs::read(&cache).expect("native cache");
        let mut reference = native.as_vapoursynth_script().expect("VS gateway").expect("VS input");
        assert_eq!(
            reference.y4m_frame(0).expect("VS frame").into_inner(),
            expected
        );
        assert_eq!(reference.clip_info().expect("metadata").num_frames, 100);
        assert_eq!(fs::read(&cache).expect("shared cache"), original_index);
        fixture.unchanged();
    }
}

#[test]
#[cfg(unix)]
fn ffms2_special_source_and_cache_filenames_preserve_selected_frames() {
    special_filename_routes(&VapourSynthImportMethod::FFMS2 { index: Some(1) });
}

#[test]
#[cfg(unix)]
fn bestsource_special_source_and_cache_filenames_preserve_selected_frames() {
    special_filename_routes(&VapourSynthImportMethod::BestSource { index: Some(1) });
}

#[test]
#[cfg(unix)]
fn lsmash_special_source_and_cache_filenames_preserve_selected_frames() {
    special_filename_routes(&VapourSynthImportMethod::LSMASHWorks { index: Some(1) });
}

#[test]
fn source_generators_pass_track_and_cache_arguments_to_python() {
    for track in [Some(0), Some(1), None] {
        let mut ffms = Source::new(Path::new("source 映像.mkv"));
        ffms.track = track;
        ffms.cachefile = Some("cache 索引".into());
        let mut best = VideoSource::new(&ffms.source);
        best.track = track;
        best.cachepath = ffms.cachefile.clone();
        let mut lsmash = LWLibavSource::new(&ffms.source);
        lsmash.stream_index = track;
        lsmash.cachefile = ffms.cachefile.clone();
        for (plugin, track_key, cache_key) in [
            (&ffms as &dyn VapourSynthPluginScript, "track", "cachefile"),
            (&best, "track", "cachepath"),
            (&lsmash, "stream_index", "cachefile"),
        ] {
            let (_, lines) = plugin.generate_script("clip".to_owned()).expect("source generator");
            let actual = recorded(lines);
            let mut expected =
                serde_json::json!({"source": "source 映像.mkv", cache_key: "cache 索引"});
            if let Some(track) = track {
                expected[track_key] = track.into();
            }
            assert_eq!(actual, serde_json::json!([expected]));
        }
    }
}

#[test]
fn dg_generator_uses_dgi_source_not_unverified_cache_keywords() {
    for alias in [false, true] {
        let mut dg = DGSource::new(Path::new(if alias {
            "original media.mkv"
        } else {
            "chosen index.dgi"
        }));
        if alias {
            dg.indexing_path = Some("chosen index.dgi".into());
        }
        let (_, lines) = dg.generate_script("clip".to_owned()).expect("DG generator");
        assert_eq!(
            recorded(lines),
            serde_json::json!([{"source": "chosen index.dgi"}])
        );
    }
}

fn case(
    plugin: &dyn VapourSynthPluginScript,
    function: &str,
    kwargs: &serde_json::Value,
) -> serde_json::Value {
    let (imports, lines) = plugin.generate_script("clip".to_owned()).expect("generator");
    let mut script = VapourSynthScript::default();
    if let Some(imports) = imports {
        script.add_imports(imports);
    }
    script.add_lines(lines);
    script.outputs.insert(0, "clip".to_owned());
    serde_json::json!({"script": script.to_string(), "expected": [{"function": function, "kwargs": kwargs}]})
}

fn check_cases(cases: &[serde_json::Value]) {
    let input = tempfile::NamedTempFile::new().expect("python-literal cases");
    fs::write(input.path(), serde_json::to_vec(cases).expect("cases JSON")).expect("write cases");
    let output = run(Command::new("timeout").args(["10s","python3","-c",r#"
import json
import sys
import types

class Clip:
    def set_output(self, **kwargs):
        pass

calls = []
def recorder(function):
    def record(**kwargs):
        calls.append({"function": function, "kwargs": kwargs})
        return Clip()
    return record

functions = {
    "ffms2": ["Source"], "bs": ["VideoSource"],
    "lsmas": ["LWLibavSource"], "dgdecodenv": ["DGSource"],
    "mvu": ["Super", "Analyse", "AnalyseMany", "Recalculate", "Degrain"],
    "std": ["PlaneStats"], "resize": ["Bob"], "vship": ["CVVDP"],
}
core = types.SimpleNamespace(**{
    ns: types.SimpleNamespace(**{f: recorder(f"{ns}.{f}") for f in names})
    for ns, names in functions.items()
})
sys.modules["vapoursynth"] = types.SimpleNamespace(core=core)
failures = []
with open(sys.argv[1], encoding="utf-8") as handle:
    cases = json.load(handle)
for case in cases:
    calls.clear()
    marker = []
    environment = {"clip": Clip(), "marker": marker}
    try:
        exec(compile(case["script"], "<python-literal generated>", "exec"), environment)
        actual = json.loads(json.dumps(calls, default=lambda node: "node"))
        assert marker == [], f"embedded expression executed: {marker!r}"
        assert actual == case["expected"], (actual, case["expected"])
    except (SyntaxError, ValueError, NameError, TypeError, AssertionError) as error:
        failures.append(f"{case['expected']!r}: {type(error).__name__}: {error}")
assert not failures, "\n".join(failures)
print(f"python-literal compiled/executed {len(cases)} generated scripts; exact kwargs, marker untouched")
"#]).arg(input.path()));
    eprint!("{}", String::from_utf8(output).expect("Python summary"));
}

#[test]
fn source_and_cache_literals_preserve_data_without_execution() {
    let mut cases = Vec::new();
    for &value in PYTHON_LITERAL_STRINGS {
        for cache_only in [false, true] {
            let source = if cache_only { "source.mkv" } else { value };
            let cache = if cache_only { value } else { "cache" };
            let mut ffms = Source::new(Path::new(source));
            ffms.cachefile = Some(cache.into());
            let mut best = VideoSource::new(Path::new(source));
            best.cachepath = Some(cache.into());
            let mut lsmash = LWLibavSource::new(Path::new(source));
            lsmash.cachefile = Some(cache.into());
            for (plugin, function, cache_key) in [
                (
                    &ffms as &dyn VapourSynthPluginScript,
                    "ffms2.Source",
                    "cachefile",
                ),
                (&best, "bs.VideoSource", "cachepath"),
                (&lsmash, "lsmas.LWLibavSource", "cachefile"),
            ] {
                cases.push(case(
                    plugin,
                    function,
                    &serde_json::json!({"source": source, cache_key: cache}),
                ));
            }
        }
        for alias in [false, true] {
            let mut dg = DGSource::new(Path::new(if alias { "original.mkv" } else { value }));
            if alias {
                dg.indexing_path = Some(value.into());
            }
            cases.push(case(
                &dg,
                "dgdecodenv.DGSource",
                &serde_json::json!({"source": value}),
            ));
        }
    }
    check_cases(&cases);
}

#[test]
fn motion_filter_prefix_literals_preserve_data_without_execution() {
    use andean_condor::vapoursynth::plugins::mvutensils::{
        analyse::Analyse, analyse_many::AnalyseMany, degrain::Degrain, mvu_super::MVUSuper,
        recalculate::Recalculate,
    };
    let mut cases = Vec::new();
    for &value in PYTHON_LITERAL_STRINGS {
        let super_filter = MVUSuper {
            prefix: Some(value.into()),
            ..Default::default()
        };
        let analyse = Analyse {
            prefix: Some(value.into()),
            ..Default::default()
        };
        let many = AnalyseMany {
            prefix: Some(value.into()),
            ..Default::default()
        };
        let recalculate = Recalculate {
            prefix: Some(value.into()),
            ..Default::default()
        };
        let degrain = Degrain {
            prefix: Some(value.into()),
            ..Default::default()
        };
        for (plugin, function, kwargs) in [
            (
                &super_filter as &dyn VapourSynthPluginScript,
                "mvu.Super",
                serde_json::json!({"clip": "node", "blksize": [8], "overlap": [4], "prefix": value}),
            ),
            (
                &analyse,
                "mvu.Analyse",
                serde_json::json!({"super": "node", "prefix": value}),
            ),
            (
                &many,
                "mvu.AnalyseMany",
                serde_json::json!({"super": "node", "prefix": value}),
            ),
            (
                &recalculate,
                "mvu.Recalculate",
                serde_json::json!({"super": "node", "vectors": "node", "prefix": value}),
            ),
            (
                &degrain,
                "mvu.Degrain",
                serde_json::json!({"clip": "node", "super": "node", "vectors": "node", "prefix": value}),
            ),
        ] {
            cases.push(case(plugin, function, &kwargs));
        }
    }
    check_cases(&cases);
}

#[test]
fn plane_stats_property_literal_cannot_execute_python() {
    check_string_filter_literals("stats");
}

#[test]
fn bob_filter_literal_cannot_execute_python() {
    check_string_filter_literals("bob");
}

#[test]
fn cvvdp_model_json_literal_cannot_execute_python() {
    check_string_filter_literals("cvvdp");
}

fn check_string_filter_literals(kind: &str) {
    use andean_condor::vapoursynth::plugins::{
        resize::bob::Bob, standard::plane_stats::PlaneStats, vship::cvvdp::CVVDP,
    };
    assert!(["stats", "bob", "cvvdp"].contains(&kind));
    let mut cases = Vec::new();
    for &value in PYTHON_LITERAL_STRINGS {
        let stats = PlaneStats {
            clip_b_name: None,
            plane: None,
            prop: Some(value.into()),
        };
        let bob = Bob {
            filter: Some(value.into()),
            ..Default::default()
        };
        let cvvdp = CVVDP {
            reference_node_name: "clip".into(),
            distorted_node_name: "clip".into(),
            model_config_json: Some(value.into()),
            ..Default::default()
        };
        if kind == "stats" {
            cases.push(case(
                &stats,
                "std.PlaneStats",
                &serde_json::json!({"clip": "node", "prop": value}),
            ));
        }
        if kind == "bob" {
            cases.push(case(
                &bob,
                "resize.Bob",
                &serde_json::json!({"clip": "node", "filter": value}),
            ));
        }
        if kind == "cvvdp" {
            cases.push(case(&cvvdp,"vship.CVVDP",&serde_json::json!({"reference": "node", "distorted": "node", "model_config_json": value})));
        }
    }
    check_cases(&cases);
}
