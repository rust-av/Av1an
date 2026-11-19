use std::{
    fmt::Write,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
    },
};

use anyhow::{Context as _, Result, bail};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use strum::{Display, EnumString, IntoStaticStr};
use vapoursynth::{core::CoreRef, map::ValueType, node::Node};

use crate::{
    core::sequence::{SequenceCompletion, SequenceStatus, Status},
    metrics::probe::{self, OutputIndexing},
};

use crate::vapoursynth::{
    VapourSynthError,
    plugins::{
        MetricPluginFunction, Plugin, PluginFunction,
        standard::{splice::Splice, trim::Trim},
        vship::{DOCS, ID, NAME},
    },
    script_builder::{
        NodeVariableName, VapourSynthPluginScript,
        script::{Imports, Line, python_string_literal},
    },
};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CVVDP {
    /// Only used for script generation
    pub reference_node_name: String,
    /// Only used for script generation
    pub distorted_node_name: String,
    pub gpu_id: Option<u32>,
    pub model_name: Option<DisplayModel>,
    pub model_config_json: Option<String>,
    pub resize_to_display: Option<bool>,
    pub disable_temporal: Option<bool>,
    pub distmap: Option<bool>,
}

impl Plugin for CVVDP {
    const PLUGIN_NAME: &'static str = NAME;
    const PLUGIN_ID: &'static str = ID;
    const PLUGIN_DOCS: Option<&'static str> = Some(DOCS);
}

impl PluginFunction for CVVDP {
    const FUNCTION_NAME: &'static str = "CVVDP";
    const FUNCTION_DOCS: Option<&'static str> =
        Some("https://codeberg.org/Line-fr/Vship/src/branch/main/doc/CVVDP.md");
    const REQUIRED_ARGUMENTS: &'static [(&'static str, &'static ValueType)] =
        &[("reference", &ValueType::VideoNode), ("distorted", &ValueType::VideoNode)];
    const OPTIONAL_ARGUMENTS: &'static [(&'static str, &'static ValueType)] = &[
        ("gpu_id", &ValueType::Int),
        ("distmap", &ValueType::Int),
        ("model_name", &ValueType::Data),
        ("model_config_json", &ValueType::Data),
        ("resizeToDisplay", &ValueType::Int),
        ("disableTemporal", &ValueType::Int),
    ];
}

impl CVVDP {
    /// Fresh filter per scene, retaining original source indices for the gap guard.
    /// Requests and callbacks are ordered; cancellation never schedules a later scene.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn collect_scene_values<'core>(
        &self,
        core: CoreRef<'core>,
        reference: &Node<'core>,
        distorted: &Node<'core>,
        scenes: &[&[usize]],
        indexing: OutputIndexing,
        progress: Sender<SequenceStatus>,
        cancelled: &AtomicBool,
        on_frame: impl FnMut(usize, &f64) -> Result<(), VapourSynthError>,
    ) -> Result<Vec<f64>> {
        let result = collect_scene_values_with(
            core,
            reference,
            distorted,
            scenes,
            indexing,
            &progress,
            cancelled,
            on_frame,
            |reference, distorted, indices| {
                Ok(self.clone().invoke(core, reference, distorted, indices.iter().copied())?)
            },
        );
        // The compare relay must see disconnection on success and every error.
        drop(progress);
        result
    }

    /// Invoke CVVDP on nodes compacted from the original `source_indices`.
    ///
    /// Enabled-temporal plugin scoring cannot reset history alone at source
    /// gaps: the plugin sees only contiguous node indices and exposes no such
    /// reset. Reject that selection before plugin lookup/scoring; native Vship
    /// can handle it, or `disable_temporal=true` can score independent frames.
    /// Full-sequence callers must also supply their actual source indices.
    #[inline]
    pub fn invoke<'core>(
        self,
        core: CoreRef<'core>,
        reference: &Node<'core>,
        distorted: &Node<'core>,
        source_indices: impl IntoIterator<Item = usize>,
    ) -> Result<Node<'core>, VapourSynthError> {
        if !self.disable_temporal.unwrap_or(false) {
            let mut previous: Option<usize> = None;
            for index in source_indices {
                if previous.is_some_and(|previous| previous.checked_add(1) != Some(index)) {
                    return Err(Self::new_error(
                        "VapourSynth CVVDP cannot perform history-only resets at selected source \
                         gaps; use disable_temporal=true for independent frames or native Vship"
                            .to_owned(),
                    ));
                }
                previous = Some(index);
            }
        }
        let mut arguments = Self::arguments()?;
        arguments.set_node("reference", reference).map_err(|e| {
            VapourSynthError::PluginArgumentsError {
                plugin: Self::PLUGIN_NAME.to_owned(),
                argument: "reference".to_owned(),
                message: e.to_string(),
            }
        })?;
        arguments.set_node("distorted", distorted).map_err(|e| {
            VapourSynthError::PluginArgumentsError {
                plugin: Self::PLUGIN_NAME.to_owned(),
                argument: "distorted".to_owned(),
                message: e.to_string(),
            }
        })?;
        Self::argument_set_ints(
            &mut arguments,
            vec![
                ("gpu_id", self.gpu_id),
                (
                    "resizeToDisplay",
                    self.resize_to_display.map(|b| if b { 1 } else { 0 }),
                ),
                (
                    "disableTemporal",
                    self.disable_temporal.map(|b| if b { 1 } else { 0 }),
                ),
                ("distmap", self.distmap.map(|b| if b { 1 } else { 0 })),
            ],
        )?;
        Self::arguments_set(
            &mut arguments,
            vec![
                (
                    "model_name",
                    self.model_name
                        .map(|model_name| model_name.to_string().into_bytes())
                        .as_deref(),
                ),
                (
                    "model_config_json",
                    self.model_config_json
                        .map(|model_config_json| model_config_json.into_bytes())
                        .as_deref(),
                ),
            ],
        )?;
        let node = Self::invoke_and_get_node(core, arguments, Some("clip"))?;

        Ok(node)
    }
}

/// Kept at the filter-construction boundary so CPU checks can use std nodes,
/// without pretending a marker is a GPU CVVDP implementation.
#[allow(clippy::too_many_arguments)]
fn collect_scene_values_with<'core>(
    core: CoreRef<'core>,
    reference: &Node<'core>,
    distorted: &Node<'core>,
    scenes: &[&[usize]],
    indexing: OutputIndexing,
    progress: &Sender<SequenceStatus>,
    cancelled: &AtomicBool,
    mut on_frame: impl FnMut(usize, &f64) -> Result<(), VapourSynthError>,
    mut invoke: impl FnMut(&Node<'core>, &Node<'core>, &[usize]) -> Result<Node<'core>>,
) -> Result<Vec<f64>> {
    let total = probe::validate_scene_selection(scenes)?;
    // Validate conversions and node bounds for every scene before reading any.
    for (position, index) in scenes.iter().flat_map(|s| s.iter()).enumerate() {
        u32::try_from(*index).context("CVVDP source index overflow")?;
        if *index >= reference.info().num_frames
            || probe::output_index(position, *index, indexing) >= distorted.info().num_frames
        {
            bail!("CVVDP scene selection exceeds reference or distorted clip");
        }
    }
    let mut scores = Vec::with_capacity(total);
    for scene in scenes.iter().filter(|scene| !scene.is_empty()) {
        if cancelled.load(Ordering::Relaxed) {
            return Err(probe::Cancelled.into());
        }
        let offset = scores.len();
        let trim = |node, indices: Vec<usize>| -> Result<Node<'core>> {
            let frames = indices
                .into_iter()
                .map(|index| {
                    let index = u32::try_from(index).context("CVVDP output index overflow")?;
                    Ok(Trim {
                        first: Some(index),
                        last: Some(index),
                        ..Default::default()
                    }
                    .invoke(core, node)?)
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(Splice::invoke(core, &frames)?)
        };
        let reference = trim(reference, scene.to_vec())?;
        let output_indices = scene
            .iter()
            .enumerate()
            .map(|(position, index)| probe::output_index(offset + position, *index, indexing))
            .collect();
        let distorted = trim(distorted, output_indices)?;
        let node = invoke(&reference, &distorted, scene)?;
        if node.info().num_frames != scene.len() {
            bail!("CVVDP filter returned the wrong scene length");
        }
        for position in 0..scene.len() {
            if cancelled.load(Ordering::Relaxed) {
                return Err(probe::Cancelled.into());
            }
            let frame = node.get_frame(position).map_err(|error| {
                CVVDP::new_error(format!("Failed to get frame {position}: {error}"))
            })?;
            let value = CVVDP::PROPERTY_NAMES
                .iter()
                .find_map(|name| frame.props().get_float(name).ok())
                .ok_or_else(|| CVVDP::new_error("Score not found on CVVDP or _CVVDP".to_owned()))?;
            on_frame(offset + position, &value)?;
            scores.push(value);
            if scores.len() == 1 {
                let _ = progress.send(SequenceStatus::Whole(Status::Processing {
                    id: "CVVDP".to_owned(),
                    completion: SequenceCompletion::Frames {
                        completed: 0,
                        total: total as u64,
                    },
                }));
            }
            let _ = progress.send(SequenceStatus::Whole(Status::Processing {
                id: "CVVDP".to_owned(),
                completion: SequenceCompletion::Frames {
                    completed: scores.len() as u64,
                    total: total as u64,
                },
            }));
        }
    }
    if total != 0 {
        let _ = progress.send(SequenceStatus::Whole(Status::Completed {
            id: "CVVDP".to_owned(),
        }));
    }
    Ok(scores)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cvvdp_script_quotes_or_omits_model_name() {
        for (model_name, expected) in [
            (
                Some(DisplayModel::Standard4K),
                r#"core.vship.CVVDP(reference = ref_clip, distorted = dist_clip, model_name = "standard_4k")"#,
            ),
            (
                None,
                "core.vship.CVVDP(reference = ref_clip, distorted = dist_clip)",
            ),
        ] {
            let plugin = CVVDP {
                reference_node_name: "ref_clip".into(),
                distorted_node_name: "dist_clip".into(),
                model_name,
                ..Default::default()
            };
            let (_, lines) = plugin.generate_script("clip".into()).expect("generate CVVDP script");
            let [Line::Expression(_, call)] = lines.as_slice() else {
                panic!("expected one CVVDP expression");
            };
            assert_eq!(call, expected);
        }
    }

    mod internal_integration {
        use super::super::*;
        use std::{cell::Cell, sync::mpsc};

        // The private scene-builder closure is required to inspect per-scene nodes.
        fn with_node(run: impl for<'core> FnOnce(CoreRef<'core>, Node<'core>)) {
            let script = "import vapoursynth as vs\nc=vs.core\nc.std.Splice([c.std.BlankClip(width=32,height=32,length=1,format=vs.YUV420P8,color=[x,128,128]).std.SetFrameProps(CVVDP=float(x)) for x in [1,2,3,4,5,6]]).set_output()";
            let mut decoder =
                av_decoders::Decoder::from_script(script, Default::default()).expect("std fixture");
            let decoder = decoder.get_vapoursynth_impl().expect("VS");
            let core = crate::vapoursynth::get_core(&decoder.env).expect("core");
            let reference = decoder
                .get_output(decoder.get_output_index(), decoder.get_node_modifier())
                .expect("node");
            run(core, reference);
        }

        #[test]
        fn cvvdp_scene_collection_pairs_aligned_and_compacted_frames() {
            with_node(|core, reference| assert_scene_pairs(core, &reference, &[0]));
        }

        #[test]
        fn cvvdp_scene_collection_stops_before_building_later_scenes() {
            with_node(|core, reference| assert_scene_pairs(core, &reference, &[1, 2, 3]));
        }

        fn assert_scene_pairs<'core>(core: CoreRef<'core>, reference: &Node<'core>, stops: &[u8]) {
            for indexing in [OutputIndexing::Aligned, OutputIndexing::Compacted] {
                let distorted = if indexing == OutputIndexing::Aligned {
                    reference.clone()
                } else {
                    Splice::invoke(
                        core,
                        &[
                            Trim {
                                first: Some(0),
                                last: Some(1),
                                ..Default::default()
                            }
                            .invoke(core, reference)
                            .expect("A"),
                            Trim {
                                first: Some(4),
                                last: Some(5),
                                ..Default::default()
                            }
                            .invoke(core, reference)
                            .expect("B"),
                        ],
                    )
                    .expect("compacted")
                };
                for &stop in stops {
                    let count = Cell::new(0);
                    let cancelled = AtomicBool::new(false);
                    let (tx, rx) = mpsc::channel();
                    let mut callbacks = Vec::new();
                    let result = collect_scene_values_with(
                        core,
                        reference,
                        &distorted,
                        &[&[0, 1], &[], &[4, 5]],
                        indexing,
                        &tx,
                        &cancelled,
                        |position, value| {
                            callbacks.push((position, *value));
                            if stop == 1 {
                                cancelled.store(true, Ordering::Relaxed);
                            }
                            if stop == 3 && position == 1 {
                                cancelled.store(true, Ordering::Relaxed);
                            }
                            if stop == 2 {
                                return Err(CVVDP::new_error(
                                    "injected callback failure".to_owned(),
                                ));
                            }
                            Ok(())
                        },
                        |a, b, indices| {
                            let i = count.get();
                            count.set(i + 1);
                            assert_eq!(indices, if i == 0 { &[0, 1] } else { &[4, 5] });
                            assert_eq!(a.info().num_frames, 2);
                            for (local, expected) in
                                if i == 0 { [1.0, 2.0] } else { [5.0, 6.0] }.into_iter().enumerate()
                            {
                                assert_eq!(
                                    a.get_frame(local)
                                        .expect("reference")
                                        .props()
                                        .get_float("CVVDP")
                                        .expect("marker"),
                                    expected
                                );
                                assert_eq!(
                                    b.get_frame(local)
                                        .expect("distorted")
                                        .props()
                                        .get_float("CVVDP")
                                        .expect("marker"),
                                    expected
                                );
                            }
                            Ok(a.clone())
                        },
                    );
                    if stop == 0 {
                        assert_eq!(result.expect("scene filters"), [1.0, 2.0, 5.0, 6.0]);
                        assert_eq!(count.get(), 2);
                        assert_eq!(callbacks, [(0, 1.0), (1, 2.0), (2, 5.0), (3, 6.0)]);
                        let counts = rx
                            .try_iter()
                            .filter_map(|event| match event {
                                SequenceStatus::Whole(Status::Processing {
                                    completion: SequenceCompletion::Frames { completed, total },
                                    ..
                                }) => Some((completed, total)),
                                _ => None,
                            })
                            .collect::<Vec<_>>();
                        assert_eq!(counts, [(0, 4), (1, 4), (2, 4), (3, 4), (4, 4)]);
                    } else {
                        let error = result.expect_err("stop in scene A");
                        assert_eq!(error.is::<probe::Cancelled>(), stop != 2);
                        assert_eq!(count.get(), 1);
                        assert_eq!(
                            callbacks,
                            if stop == 3 {
                                vec![(0, 1.0), (1, 2.0)]
                            } else {
                                vec![(0, 1.0)]
                            }
                        );
                        assert!(
                            !rx.try_iter().any(|e| matches!(
                                e,
                                SequenceStatus::Whole(Status::Completed { .. })
                            ))
                        );
                    }
                }
            }
        }

        #[test]
        fn cvvdp_empty_or_invalid_scenes_emit_no_progress() {
            with_node(|core, reference| {
                let (tx, rx) = mpsc::channel();
                assert!(
                    collect_scene_values_with(
                        core,
                        &reference,
                        &reference,
                        &[&[], &[]],
                        OutputIndexing::Aligned,
                        &tx,
                        &AtomicBool::new(false),
                        |_, _| panic!("empty callback"),
                        |_, _, _| panic!("empty filter")
                    )
                    .expect("empty scenes")
                    .is_empty()
                );
                assert!(rx.try_iter().next().is_none());
                for scenes in [
                    &[&[1, 0][..]][..],
                    &[&[0, 1][..], &[1, 2][..]][..],
                    &[&[usize::MAX][..]][..],
                    &[&[6][..]][..],
                ] {
                    let (tx, rx) = mpsc::channel();
                    assert!(
                        collect_scene_values_with(
                            core,
                            &reference,
                            &reference,
                            scenes,
                            OutputIndexing::Aligned,
                            &tx,
                            &AtomicBool::new(false),
                            |_, _| panic!("invalid callback"),
                            |_, _, _| panic!("invalid filter")
                        )
                        .is_err()
                    );
                    assert!(rx.try_iter().next().is_none());
                }
            });
        }

        #[test]
        fn cvvdp_plugin_rejects_source_gaps_without_history_only_reset() {
            with_node(|core, reference| {
                let (tx, _) = mpsc::channel();
                let error = CVVDP::default()
                    .collect_scene_values(
                        core,
                        &reference,
                        &reference,
                        &[&[0, 2]],
                        OutputIndexing::Aligned,
                        tx,
                        &AtomicBool::new(false),
                        |_, _| panic!("gap scored"),
                    )
                    .expect_err("plugin cannot reset history alone");
                assert!(error.to_string().contains("history-only"), "{error:#}");
            });
        }
    }
}

impl MetricPluginFunction for CVVDP {
    const PROPERTY_NAMES: &'static [&'static str] = &["CVVDP", "_CVVDP"];
}

impl VapourSynthPluginScript for CVVDP {
    #[inline]
    fn generate_script(&self, node_name: NodeVariableName) -> Result<(Option<Imports>, Vec<Line>)> {
        let mut lines = vec![];

        let line = {
            let mut line = String::new();
            write!(
                &mut line,
                "core.vship.CVVDP(reference = {}, distorted = {}",
                self.reference_node_name, self.distorted_node_name
            )?;
            if let Some(gpu_id) = self.gpu_id {
                write!(&mut line, ", gpu_id = {}", gpu_id)?;
            }
            if let Some(model_name) = &self.model_name {
                write!(
                    &mut line,
                    ", model_name = {}",
                    python_string_literal(&model_name.to_string())
                )?;
            }
            if let Some(model_config_json) = &self.model_config_json {
                write!(
                    &mut line,
                    ", model_config_json = {}",
                    python_string_literal(model_config_json)
                )?;
            }
            if let Some(resize_to_display) = self.resize_to_display {
                write!(
                    &mut line,
                    ", resizeToDisplay = {}",
                    resize_to_display as i64
                )?;
            }
            if let Some(disable_temporal) = self.disable_temporal {
                write!(&mut line, ", disableTemporal = {}", disable_temporal as i64)?;
            }
            if let Some(distmap) = self.distmap {
                write!(&mut line, ", distmap = {}", distmap as i64)?;
            }
            write!(&mut line, ")")?;
            line
        };

        lines.push(Line::Expression(node_name, line));

        Ok((None, lines))
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    Display,
    EnumString,
    IntoStaticStr,
    JsonSchema,
)]
pub enum DisplayModel {
    #[strum(serialize = "standard_4k")]
    Standard4K,
    #[strum(serialize = "standard_hdr_pq")]
    StandardHDRPQ,
    #[strum(serialize = "standard_hdr_hlg")]
    StandardHDRHLG,
    #[strum(serialize = "standard_hdr_linear")]
    StandardHDRLinear,
    #[strum(serialize = "standard_hdr_dark")]
    StandardHDRDark,
    #[strum(serialize = "standard_hdr_linear_zoom")]
    StandardHDRLinearZoom,
    #[strum(serialize = "standard_fhd")]
    StandardFHD,
}
