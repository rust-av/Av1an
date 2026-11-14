//! I05: losing source options changes the image used as the metric reference.

use std::{fs, path::Path, process::Command};

use super::*;
use crate::{
    core::input::Ffms2Filter, models::input::ImportMethod,
    vapoursynth::script_builder::script::Lines,
};

fn run(command: &mut Command) -> Vec<u8> {
    // File-backed output plus timeout keeps fixture creation bounded without pipe deadlocks.
    let stdout = tempfile::NamedTempFile::new().expect("stdout");
    let stderr = tempfile::NamedTempFile::new().expect("stderr");
    let status = command
        .stdin(std::process::Stdio::null())
        .stdout(stdout.reopen().expect("stdout handle"))
        .stderr(stderr.reopen().expect("stderr handle"))
        .status()
        .expect("source-options requires ffmpeg, python3 and timeout on PATH");
    assert!(
        status.success(),
        "{command:?}: {}",
        fs::read_to_string(stderr.path()).expect("diagnostic")
    );
    fs::read(stdout.path()).expect("output")
}

struct Fixture {
    directory: tempfile::TempDir,
    source: std::path::PathBuf,
    original: Vec<u8>,
}

impl Fixture {
    fn new() -> Self {
        Self::with_source_name("two tracks.mkv")
    }

    fn with_source_name(name: &str) -> Self {
        let directory = tempfile::tempdir().expect("source-options fixture");
        let parent = directory.path().join("read only 映像");
        fs::create_dir_all(&parent).expect("source directory");
        let source = parent.join(name);
        run(Command::new("timeout")
            .args(["30s", "ffmpeg", "-nostdin", "-v", "error", "-y"])
            .args(["-f", "lavfi", "-i", "nullsrc=s=32x16:r=24,geq=lum=37:cb=83:cr=201"])
            .args(["-f", "lavfi", "-i", "nullsrc=s=32x16:r=24,geq=lum=219:cb=41:cr=159"])
            .args([
                "-map",
                "0:v",
                "-map",
                "1:v",
                "-frames:v",
                "100",
                "-c:v",
                "ffv1",
                "-threads",
                "1",
            ])
            .arg(&source));
        let original = fs::read(&source).expect("source bytes");
        let fixture = Self {
            directory,
            source,
            original,
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&fixture.source, fs::Permissions::from_mode(0o444))
                .expect("read-only file");
            fs::set_permissions(&parent, fs::Permissions::from_mode(0o555))
                .expect("read-only parent");
            let probe = parent.join("write probe");
            match fs::write(&probe, b"permission probe") {
                Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                    eprintln!("source-options readonly enforced: PermissionDenied ({parent:?})");
                },
                Ok(()) => {
                    fs::remove_file(&probe).expect("remove exact permission probe");
                    eprintln!(
                        "source-options readonly not-enforced/root capability: removed {probe:?}; continuing full assertions"
                    );
                },
                Err(error) => {
                    panic!("source-options permission probe {probe:?} failed unexpectedly: {error}")
                },
            }
        }
        fixture
    }

    fn cache(&self, name: &str) -> std::path::PathBuf {
        let directory = self.directory.path().join(name);
        fs::create_dir_all(&directory).expect("writable cache directory");
        directory.join("explicit cache 索引")
    }

    fn unchanged(&self) {
        assert_eq!(fs::read(&self.source).expect("source bytes"), self.original);
        assert_eq!(
            fs::read_dir(self.source.parent().expect("parent"))
                .expect("source listing")
                .count(),
            1,
            "no indexes beside read-only source"
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                self.source.parent().expect("parent"),
                fs::Permissions::from_mode(0o755),
            )
            .expect("restore parent for cleanup");
            fs::set_permissions(&self.source, fs::Permissions::from_mode(0o644))
                .expect("restore source for cleanup");
        }
    }
}

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

fn expected_frame(track: Option<u8>, width: usize) -> Vec<u8> {
    let [y, u, v] = if track == Some(1) {
        [219, 41, 159]
    } else {
        [37, 83, 201]
    };
    [b"FRAME\n".to_vec(), vec![y; width * 16], vec![u; width * 4], vec![v; width * 4]].concat()
}

fn routes(method: fn(Option<u8>) -> VapourSynthImportMethod) {
    let fixture = Fixture::new();
    // Start with nonzero so a lost selection fails on actual image data, not just kwargs.
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
            // Catch dropped cache options before a regression can write BestSource's default
            // index outside this fixture. The image check below still uses the real plugin.
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
            if !scripted {
                let sources = input.frame_sources(5).expect("additional source instances");
                // FFMS2/LSMASH reopen after reader 1, BestSource after reader 4.
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

#[test]
fn ffms2_direct_and_scripted_track_cache() {
    routes(|index| VapourSynthImportMethod::FFMS2 { index });
}

#[test]
fn bestsource_direct_and_scripted_track_cache() {
    routes(|index| VapourSynthImportMethod::BestSource { index });
}

#[test]
fn lsmash_direct_and_scripted_track_cache() {
    routes(|index| VapourSynthImportMethod::LSMASHWorks { index });
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

pub(super) fn recorded(lines: Lines) -> serde_json::Value {
    let mut script = VapourSynthScript::default();
    script.add_lines(lines);
    script.outputs.insert(0, "clip".to_owned());
    recorded_script(&script.to_string(), &HashMap::new())
}

fn recorded_script(script: &str, variables: &HashMap<String, String>) -> serde_json::Value {
    let script_file = tempfile::NamedTempFile::new().expect("generated script");
    fs::write(script_file.path(), script).expect("write generated script");
    let output = run(Command::new("timeout").args(["10s", "python3", "-c", r#"
import json, sys, types
calls = []
class Clip:
    def set_output(self, *args, **kwargs): pass
def source(**kwargs):
    calls.append(kwargs)
    return Clip()
core = types.SimpleNamespace(**{n: types.SimpleNamespace(**{f: source}) for n, f in [('ffms2','Source'), ('bs','VideoSource'), ('lsmas','LWLibavSource'), ('dgdecodenv','DGSource')]})
sys.modules['vapoursynth'] = types.SimpleNamespace(core=core)
exec(compile(open(sys.argv[1]).read(), sys.argv[1], 'exec'), json.loads(sys.argv[2]))
print(json.dumps(calls))
"#]).arg(script_file.path()).arg(serde_json::to_string(variables).expect("script variables")));
    serde_json::from_slice(&output).expect("recorded generated kwargs")
}

#[test]
fn standalone_generators_keep_zero_nonzero_and_default_tracks() {
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

#[test]
fn dg_production_routes_plan_existing_index_as_source_without_indexer() {
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
        // All opening/reopening routes share this planner, including frame_sources.
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

const PYTHON_LITERAL_STRINGS: &[&str] = &[
    "",
    "single' and double\" quotes",
    "back\\slash",
    "literal\\n\\t\\u1234\\x41",
    "line\ncarriage\rtab\t",
    "trailing\\",
    "映像🦀 e\u{301}\u{2028}\u{2029}",
    "\0\u{1}\u{8}\u{c}\u{1f}",
    "\"+str(marker.append('executed'))+\"",
    "str(marker.append('executed'))",
];

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
    let output = run(Command::new("timeout")
        .args([
            "10s",
            "python3",
            "-c",
            r#"
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
"#,
        ])
        .arg(input.path()));
    eprint!("{}", String::from_utf8(output).expect("Python summary"));
}

#[test]
fn literal_encoder_roundtrips_in_python() {
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

#[test]
fn source_and_cache_literals_preserve_data_without_execution() {
    let mut cases = Vec::new();
    for &value in PYTHON_LITERAL_STRINGS {
        // Isolate each input so a broken source literal cannot hide a broken cache literal.
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
fn prefix_literals_preserve_data_without_execution() {
    use crate::vapoursynth::plugins::mvutensils::{
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
fn other_string_filters_preserve_data_without_execution() {
    use crate::vapoursynth::plugins::{
        resize::bob::Bob, standard::plane_stats::PlaneStats, vship::cvvdp::CVVDP,
    };
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
        cases.push(case(
            &stats,
            "std.PlaneStats",
            &serde_json::json!({"clip": "node", "prop": value}),
        ));
        cases.push(case(
            &bob,
            "resize.Bob",
            &serde_json::json!({"clip": "node", "filter": value}),
        ));
        cases.push(case(&cvvdp, "vship.CVVDP", &serde_json::json!({"reference": "node", "distorted": "node", "model_config_json": value})));
    }
    check_cases(&cases);
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
fn ffms2_special_filename_native_and_scripted() {
    special_filename_routes(&VapourSynthImportMethod::FFMS2 { index: Some(1) });
}

#[test]
#[cfg(unix)]
fn bestsource_special_filename_direct_and_scripted() {
    special_filename_routes(&VapourSynthImportMethod::BestSource { index: Some(1) });
}

#[test]
#[cfg(unix)]
fn lsmash_special_filename_direct_and_scripted() {
    special_filename_routes(&VapourSynthImportMethod::LSMASHWorks { index: Some(1) });
}
