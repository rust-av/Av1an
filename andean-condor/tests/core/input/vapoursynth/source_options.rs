use super::*;
use andean_condor::core::input::Ffms2Filter;
#[cfg(unix)]
use andean_condor::models::input::VapourSynthImportMethod;
use std::fs;

#[path = "fixtures/source_options_support.rs"]
mod fixture;
use fixture::{Fixture, expected_frame};

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
