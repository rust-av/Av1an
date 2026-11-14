//! Native quality metrics, and the decode/probe driver they share.
//!
//! Comparing an encode against its source means reading two clips, pairing the
//! frames the pass selected, and handing each pair to a metric. That part is
//! identical for every metric, so it lives in [`probe`] behind one driver,
//! [`probe::probe_selected_frames`]. A metric module supplies only a scorer and
//! an adaptor from the driver's [`PlaneSet`] to its own descriptor, so the
//! driver stays free of any metric crate's types.
//!
//! The three native libraries are opened with `dlopen` and may be absent;
//! [`availability`] reports that for every library at once, for `av1an
//! --version`.
//!
//! # Choosing an engine
//!
//! SSIMULACRA2, Butteraugli and CVVDP have three interchangeable-looking but
//! numerically distinct implementations: libvship on the GPU, fmetrics on the
//! CPU, and the VapourSynth plugins. They do not agree to the last digit, so
//! which one produced a number is recorded alongside it.
//!
//! [`engine`] decides the order, rather than each call site restating it:
//!
//! | input | order | why |
//! |---|---|---|
//! | VapourSynth | vship → plugin → fmetrics | the graph already exists, so a plugin costs only the metric, while fmetrics costs ~1.4x more |
//! | FFMS2 | vship → fmetrics → plugin | there is no graph, and building one purely to run a metric costs more than the metric |
//!
//! fmetrics also does not honour a configured `resolution` -- it scores at the
//! source size. These orders apply only to equivalent metric configurations:
//! Butteraugli's default `norm: None` requests infinity, which fmetrics 0.1.0
//! cannot compute. Only explicit finite norms may use fmetrics; without libvship,
//! the existing plugin branch must run or return its missing-runtime/plugin error.

pub mod availability;
pub mod fmetrics;
pub mod probe;
pub mod vmaf;
pub mod vship;

pub use availability::{MetricDeviceInfo, MetricLibraryInfo, libraries};
pub use probe::{OutputIndexing, PlaneSet, PlaneSource, output_index};
pub use vmaf::score_probed_frames as score_vmaf_frames;
pub use vmaf::score_probed_frames_with_progress as score_vmaf_frames_with_progress;
pub use vship::score_probed_frames as score_vship_frames;

use crate::models::sequence::target_quality::types::QualityMetric;

/// Retrying a partially reported scene pass would duplicate/backslide progress.
#[derive(Debug, thiserror::Error)]
#[error("scene scoring failed after reporting frame progress; not retrying through a plugin")]
pub(crate) struct SceneScoringStarted;

/// Score a QC/TQ scene selection, reporting whole-selection positions.
/// Enabled-temporal CVVDP gets a fresh scorer for each nonempty scene; all other
/// configurations retain one flattened call. Scores remain ordered prefixes.
#[allow(clippy::too_many_arguments)]
pub(crate) fn score_scene_frames(
    engine: Engine,
    reference: &mut av_decoders::Decoder,
    distorted: &mut av_decoders::Decoder,
    config: &QualityMetric,
    scenes: &[&[usize]],
    indexing: OutputIndexing,
    cancelled: Option<&std::sync::atomic::AtomicBool>,
    mut on_score: impl FnMut(usize, f64),
) -> anyhow::Result<Vec<f64>> {
    if scene_temporal(config) {
        let total = probe::validate_scene_selection(scenes)?;
        let mut scores = Vec::with_capacity(total);
        let mut offset = 0;
        let mut reference_next = 0;
        let mut reported = false;
        for (scene_index, scene) in scenes.iter().enumerate().filter(|(_, scene)| !scene.is_empty())
        {
            if cancelled.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Relaxed)) {
                return Err(probe::Cancelled.into());
            }
            let report = |position, value| {
                reported = true;
                on_score(offset + position, value);
            };
            let values = match engine {
                Engine::Fmetrics => fmetrics::score_probed_frames_from(
                    reference,
                    distorted,
                    config,
                    scene,
                    indexing,
                    offset,
                    reference_next,
                    cancelled,
                    report,
                ),
                Engine::Vship => vship::score_probed_frames_from(
                    reference,
                    distorted,
                    config,
                    scene,
                    indexing,
                    offset,
                    reference_next,
                    cancelled,
                    report,
                ),
                _ => anyhow::bail!("no native scene scorer for {engine}"),
            }
            .map_err(|error| {
                let error = error.context(format!(
                    "{engine} {} scene {scene_index}",
                    config.friendly_name()
                ));
                if reported {
                    error.context(SceneScoringStarted)
                } else {
                    error
                }
            })?;
            offset += scene.len(); // Validated sum bounds all scene/local positions.
            reference_next = scene
                .last()
                .and_then(|index| index.checked_add(1))
                .expect("validated nonempty scene source indices");
            scores.extend(values);
        }
        return Ok(scores);
    }
    let selected: Vec<_> = scenes.iter().flat_map(|scene| scene.iter().copied()).collect();
    match engine {
        Engine::Fmetrics => fmetrics::score_probed_frames(
            reference, distorted, config, &selected, indexing, cancelled, on_score,
        ),
        Engine::Vship => vship::score_probed_frames(
            reference, distorted, config, &selected, indexing, cancelled, on_score,
        ),
        _ => anyhow::bail!("no native scene scorer for {engine}"),
    }
}

/// Only enabled-temporal CVVDP needs independent scene sessions.
pub(crate) const fn scene_temporal(metric: &QualityMetric) -> bool {
    metric.is_cumulative()
}

/// Which implementation produced a score.
///
/// Recorded so a number is never compared across engines by accident: the same
/// metric name yields different values on each, because a GPU implementation, a
/// CPU implementation and a filter plugin are three different measurements.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
pub enum Engine {
    /// libvship, on the GPU.
    Vship,
    /// fmetrics, on the CPU.
    Fmetrics,
    /// A VapourSynth plugin, from the filter graph.
    VapourSynth,
    /// libvmaf, for VMAF.
    Vmaf,
}

impl Engine {
    /// A short name for logs and reports.
    #[inline]
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Vship => "vship",
            Self::Fmetrics => "fmetrics",
            Self::VapourSynth => "vapoursynth",
            Self::Vmaf => "vmaf",
        }
    }
}

impl std::fmt::Display for Engine {
    #[inline]
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Which engine should score `metric` for an input of the given kind.
///
/// `None` means no native engine can run, so the caller uses its VapourSynth
/// branch. `input_is_vapoursynth` is passed rather than inferred so this stays
/// a pure function both callers can test.
#[inline]
#[must_use]
pub fn engine(metric: &QualityMetric, input_is_vapoursynth: bool) -> Option<Engine> {
    let availability = Availability {
        vship: av_metrics_vship::is_available(),
        fmetrics: av_metrics_fmetrics::is_available(),
        // A branch in this crate rather than a library, so never absent.
        vapoursynth: true,
    };

    engine_order(metric, input_is_vapoursynth, availability)
}

/// Which libraries loaded on this machine.
///
/// A property of the machine, not of the metric: which metrics an engine may
/// serve is decided in [`engine_order`], so the two cannot be confused. Held as
/// a value so the ordering can be tested in every combination rather than only
/// the one the test machine happens to provide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Availability {
    vship: bool,
    fmetrics: bool,
    vapoursynth: bool,
}

/// The engine ordering, independent of what is installed.
///
/// libvship wins whenever it implements the metric and can run, which keeps a
/// machine with a working GPU behaving as it did before fmetrics existed.
///
/// Otherwise the input decides. A VapourSynth input already has the graph a
/// plugin needs, so the plugin costs only the metric and beats fmetrics on
/// measured throughput. An FFMS2 input has no graph, so building one purely to
/// run a metric costs more than the metric -- and fmetrics is also the only
/// option on a machine with neither VapourSynth nor a GPU.
#[inline]
#[must_use]
fn engine_order(
    metric: &QualityMetric,
    input_is_vapoursynth: bool,
    available: Availability,
) -> Option<Engine> {
    // The mappings are consulted here rather than folded into `Availability`,
    // so that this function is what guarantees a metric is never routed to
    // an engine that cannot serve it.
    if available.vship && vship::vship_metric_for(metric).is_some() {
        return Some(Engine::Vship);
    }

    if available.fmetrics
        && fmetrics::fmetrics_metric_for(metric).is_some()
        && !(input_is_vapoursynth && available.vapoursynth && plugin_available(metric))
    {
        return Some(Engine::Fmetrics);
    }

    // Either nothing native can run, or the plugin is the better choice here.
    None
}

/// Whether the VapourSynth plugin branch can serve `metric`.
///
/// A metric with no plugin falls through to fmetrics even for a VapourSynth
/// input, since there is nothing else to run.
#[inline]
#[must_use]
const fn plugin_available(metric: &QualityMetric) -> bool {
    matches!(
        metric,
        QualityMetric::SSIMULACRA2 { .. }
            | QualityMetric::BUTTERAUGLI { .. }
            | QualityMetric::CVVDP { .. }
            | QualityMetric::XPSNR { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ssimulacra2() -> QualityMetric {
        QualityMetric::SSIMULACRA2 {
            target_range: (74.0, 76.0),
            resolution: None,
            threads: None,
            gpu_id: None,
        }
    }

    fn xpsnr() -> QualityMetric {
        QualityMetric::XPSNR {
            target_range: (45.0, 55.0),
            resolution: None,
        }
    }

    fn butteraugli() -> QualityMetric {
        QualityMetric::BUTTERAUGLI {
            target_range: (1.0, 3.0),
            resolution: None,
            threads: None,
            intensity_multiplier: None,
            norm: None,
            gpu_id: None,
        }
    }

    #[test]
    fn butteraugli_norm_capability_over_all_availability_and_inputs() {
        // Rows: vship, fmetrics, plugin, infinity VS/FFMS2, finite VS/FFMS2.
        let cases = [
            (false, false, false, [None, None], [None, None]),
            (false, false, true, [None, None], [None, None]),
            (
                false,
                true,
                false,
                [None, None],
                [Some(Engine::Fmetrics); 2],
            ),
            (
                false,
                true,
                true,
                [None, None],
                [None, Some(Engine::Fmetrics)],
            ),
            (
                true,
                false,
                false,
                [Some(Engine::Vship); 2],
                [Some(Engine::Vship); 2],
            ),
            (
                true,
                false,
                true,
                [Some(Engine::Vship); 2],
                [Some(Engine::Vship); 2],
            ),
            (
                true,
                true,
                false,
                [Some(Engine::Vship); 2],
                [Some(Engine::Vship); 2],
            ),
            (
                true,
                true,
                true,
                [Some(Engine::Vship); 2],
                [Some(Engine::Vship); 2],
            ),
        ];
        let mut checked = 0;
        for (vship, fmetrics, vapoursynth, infinity, finite) in cases {
            let available = Availability {
                vship,
                fmetrics,
                vapoursynth,
            };
            for (norm, expected) in [(None, infinity), (Some(3), finite)] {
                let mut metric = butteraugli();
                if let QualityMetric::BUTTERAUGLI { norm: value, .. } = &mut metric {
                    *value = norm;
                }
                for (input_is_vs, want) in [true, false].into_iter().zip(expected) {
                    assert_eq!(
                        engine_order(&metric, input_is_vs, available),
                        want,
                        "{available:?}, VS={input_is_vs}, norm={norm:?}"
                    );
                    checked += 1;
                }
            }
        }
        assert_eq!(checked, 32);
    }

    #[test]
    fn the_ordering_is_exhaustive_over_every_availability_combination() {
        // Stated as a table so every combination is visibly covered, including
        // the one with nothing available. The row that differs by input is the
        // reason the input kind is a parameter at all.
        let cases = [
            // (vship, fmetrics, vapoursynth input, ffms2 input)
            (true, true, Some(Engine::Vship), Some(Engine::Vship)),
            (true, false, Some(Engine::Vship), Some(Engine::Vship)),
            (false, true, None, Some(Engine::Fmetrics)),
            (false, false, None, None),
        ];

        for (vship, fmetrics, expected_vs, expected_ffms2) in cases {
            let available = Availability {
                vship,
                fmetrics,
                vapoursynth: true,
            };
            let for_vs = engine_order(&ssimulacra2(), true, available);
            let for_ffms2 = engine_order(&ssimulacra2(), false, available);

            assert_eq!(
                for_vs, expected_vs,
                "vship={vship} fmetrics={fmetrics}, VapourSynth input"
            );
            assert_eq!(
                for_ffms2, expected_ffms2,
                "vship={vship} fmetrics={fmetrics}, FFMS2 input"
            );
        }
    }

    #[test]
    fn libvship_outranks_everything_whenever_it_can_run() {
        // A machine with a working GPU behaves as it did before fmetrics
        // existed, whatever else is installed.
        let with_vship = Availability {
            vship: true,
            fmetrics: true,
            vapoursynth: true,
        };
        for vapoursynth in [true, false] {
            assert_eq!(
                engine_order(&ssimulacra2(), vapoursynth, with_vship),
                Some(Engine::Vship),
                "libvship must win when it can run, VapourSynth input={vapoursynth}"
            );
        }
    }

    #[test]
    fn fmetrics_is_reachable_only_where_it_is_actually_cheaper() {
        let both = Availability {
            vship: false,
            fmetrics: true,
            vapoursynth: true,
        };
        let no_plugin = Availability {
            vapoursynth: false,
            ..both
        };

        // FFMS2: fmetrics either way, since there is no graph to weigh.
        assert_eq!(
            engine_order(&ssimulacra2(), false, both),
            Some(Engine::Fmetrics)
        );
        assert_eq!(
            engine_order(&ssimulacra2(), false, no_plugin),
            Some(Engine::Fmetrics)
        );
        // VapourSynth: the plugin when there is one, fmetrics when there is
        // not.
        assert_eq!(engine_order(&ssimulacra2(), true, both), None);
        assert_eq!(
            engine_order(&ssimulacra2(), true, no_plugin),
            Some(Engine::Fmetrics)
        );
    }

    #[test]
    fn no_available_engine_means_the_plugin_not_a_guess() {
        // `None` is the signal for "use your VapourSynth branch".
        let nothing = Availability {
            vship: false,
            fmetrics: false,
            vapoursynth: true,
        };
        for vapoursynth in [true, false] {
            assert_eq!(engine_order(&ssimulacra2(), vapoursynth, nothing), None);
        }
    }

    #[test]
    fn a_metric_with_no_native_engine_falls_through_to_the_plugin() {
        // XPSNR has no libvship or fmetrics implementation.
        let with_everything = Availability {
            vship: true,
            fmetrics: true,
            vapoursynth: true,
        };
        assert_eq!(engine_order(&xpsnr(), false, with_everything), None);
        assert_eq!(engine(&xpsnr(), true), None);
        assert_eq!(engine(&xpsnr(), false), None);
    }

    #[test]
    fn the_real_availability_matches_what_the_ordering_expects() {
        // The path production takes, which must report libvship first when it
        // can run.
        if vship::native_supported(&ssimulacra2()) {
            assert_eq!(engine(&ssimulacra2(), true), Some(Engine::Vship));
            assert_eq!(engine(&ssimulacra2(), false), Some(Engine::Vship));
        }
    }

    #[test]
    fn the_input_kind_never_reaches_fmetrics_for_a_vapoursynth_input() {
        // Holds whatever is installed: libvship wins before the question of
        // fmetrics arises, and otherwise the plugin does.
        assert_ne!(engine(&ssimulacra2(), true), Some(Engine::Fmetrics));
        assert_ne!(engine(&butteraugli(), true), Some(Engine::Fmetrics));
    }

    #[test]
    fn plugin_availability_matches_the_branches_this_crate_implements() {
        assert!(plugin_available(&ssimulacra2()));
        assert!(plugin_available(&butteraugli()));
        assert!(plugin_available(&xpsnr()));
        // VMAF's caller is special-cased and has no plugin branch, so listing
        // it here would promise a fallback that does not exist.
        assert!(!plugin_available(&QualityMetric::VMAF {
            target_range: (85.0, 95.0),
            resolution: None,
            scaler: "area".to_owned(),
            threads: 4,
            model: None,
            features: Vec::new(),
        }));
    }

    #[test]
    fn engine_names_are_stable_and_distinct() {
        // These names reach logs and reports, so a rename is a visible change.
        let engines = [Engine::Vship, Engine::Fmetrics, Engine::VapourSynth, Engine::Vmaf];
        let names: std::collections::HashSet<_> =
            engines.iter().map(|engine| engine.as_str()).collect();
        assert_eq!(names.len(), engines.len());
        assert_eq!(Engine::Fmetrics.as_str(), "fmetrics");
        assert_eq!(Engine::Vship.to_string(), "vship");
    }
}
