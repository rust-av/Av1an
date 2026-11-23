//! Saved-result admission. Serialization and integrity checks never certify old work.
use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{
    SequenceDataHandler,
    parallel_encoder::{ParallelEncoderConfig, ParallelEncoderDataHandler},
    quality_check::QualityCheckConfig,
    target_quality::{
        TargetQualityConfig, TargetQualityDataHandler,
        types::{QualityMetric, QualityPass},
    },
};
use crate::{
    metrics::{self, Engine},
    models::{encoder::EncoderBase, input::Input, scene::Scene},
};

/// Deliberate compatibility boundary for repaired conversion, math and selection semantics.
pub const RESULT_REVISION: u32 = 1;

#[cfg(test)]
thread_local! {
    static IDENTITY_COUNTS: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}

#[cfg(test)]
pub(crate) fn take_identity_counts() -> (usize, usize) {
    IDENTITY_COUNTS.with(|counts| counts.replace((0, 0)))
}

/// Only successful owning producers attach this; missing fields deserialize untrusted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ResultProvenance {
    #[serde(default)]
    pub revision: u32,
    #[serde(default)]
    pub context: Value,
}

/// Actual successful measurement implementation, separate from dispatch preference.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MetricBackend {
    pub engine: Engine,
    pub version: Option<String>,
    #[serde(default)]
    pub plugin: Option<Value>,
}

impl MetricBackend {
    #[inline]
    pub fn current(engine: Engine) -> Self {
        let version = match engine {
            Engine::Fmetrics => av_metrics_fmetrics::fmetrics_version(),
            Engine::Vship => av_metrics_vship::libvship_version().map(str::to_owned),
            Engine::Vmaf => av_metrics_vmaf::libvmaf_version().map(str::to_owned),
            // No universal plugin/binary attestation: revision versions our adapter contract.
            Engine::VapourSynth => Some("condor-plugin-adapter-1".to_owned()),
        };
        Self {
            engine,
            version,
            plugin: None,
        }
    }

    /// The plugin actually preferred by the scoring branch on this core. Its
    /// registered function and file identity are admission evidence, not a
    /// universal attestation of all native binaries or hidden plugin inputs.
    #[inline]
    pub fn plugin_for_metric(
        core: vapoursynth::core::CoreRef<'_>,
        metric: &QualityMetric,
    ) -> Result<Self> {
        use crate::vapoursynth::plugins::{
            PluginFunction,
            vship::{butteraugli::BUTTERAUGLI, cvvdp::CVVDP, ssimulacra2::SSIMULACRA2 as Vship},
            vszip::{ssimulacra2::SSIMULACRA2 as Vszip, xpsnr::XPSNR},
        };
        fn identity<P: PluginFunction>(
            core: vapoursynth::core::CoreRef<'_>,
        ) -> Result<MetricBackend> {
            let plugin = P::plugin(core)?;
            let path = plugin
                .path()
                .context("metric plugin does not expose its file identity")?
                .to_str()?;
            let function = plugin
                .get_plugin_function_by_name(P::FUNCTION_NAME)?
                .context("metric plugin function missing")?;
            let mut backend = MetricBackend::current(Engine::VapourSynth);
            backend.plugin = Some(
                json!({"id": P::PLUGIN_ID, "function": P::FUNCTION_NAME, "arguments": function.arguments().to_str()?, "file": file_identity(Path::new(path))?}),
            );
            Ok(backend)
        }
        match metric {
            QualityMetric::SSIMULACRA2 { .. } if Vship::plugin_is_installed(core) => {
                identity::<Vship>(core)
            },
            QualityMetric::SSIMULACRA2 { .. } => identity::<Vszip>(core),
            QualityMetric::BUTTERAUGLI { .. } => identity::<BUTTERAUGLI>(core),
            QualityMetric::XPSNR { .. } => identity::<XPSNR>(core),
            QualityMetric::CVVDP { .. } => identity::<CVVDP>(core),
            QualityMetric::VMAF { .. } => anyhow::bail!("VMAF has no plugin scoring branch"),
        }
    }

    #[inline]
    pub fn available_plugin(metric: &QualityMetric) -> Result<Self> {
        #[cfg(test)]
        IDENTITY_COUNTS.with(|counts| {
            let (models, plugins) = counts.get();
            counts.set((models, plugins + 1));
        });
        let environment = crate::vapoursynth::get_environment()?;
        Self::plugin_for_metric(crate::vapoursynth::get_core(&environment)?, metric)
    }
}

impl ResultProvenance {
    #[inline]
    pub fn new(context: Value) -> Self {
        Self {
            revision: RESULT_REVISION,
            context,
        }
    }
}

fn refusal(index: usize, kind: &str, reason: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "scene {index} {kind} result cannot resume: {reason}; preserve original files and explicitly reset the applicable saved records (av1an scenes {index} reset target-quality / quality-check / parallel-encoder) and recompute in an empty work directory, or use a new job. Resetting metadata does not delete physical chunks"
    )
}

fn require_revision(saved: Option<&ResultProvenance>, index: usize, kind: &str) -> Result<()> {
    ensure!(
        saved.is_some_and(|p| p.revision == RESULT_REVISION),
        refusal(
            index,
            kind,
            "missing, legacy, unknown or future processing revision"
        )
    );
    Ok(())
}

fn require_context(
    saved: Option<&ResultProvenance>,
    expected: &Value,
    index: usize,
    kind: &str,
) -> Result<()> {
    require_revision(saved, index, kind)?;
    ensure!(
        saved.is_some_and(|p| &p.context == expected),
        refusal(
            index,
            kind,
            "source, scene, encoder, metric/backend/model or dependency context changed"
        )
    );
    Ok(())
}

/// Non-adversarial identity: detects ordinary same-path changes, not preserved-stat edits.
/// Arbitrary files read by scripts/plugins are outside the declared-input contract.
#[inline]
pub fn file_identity(path: &Path) -> Result<Value> {
    let canonical =
        path.canonicalize().with_context(|| format!("identifying {}", path.display()))?;
    let metadata = std::fs::metadata(&canonical)?;
    ensure!(
        metadata.is_file(),
        "{} is not a regular provenance input",
        path.display()
    );
    let modified = metadata.modified()?.duration_since(std::time::UNIX_EPOCH)?;
    // Opening also refuses unreadable inputs instead of silently manufacturing an identity.
    std::fs::File::open(&canonical)?;
    Ok(
        json!({"path": canonical, "length": metadata.len(), "modified_seconds": modified.as_secs(), "modified_nanos": modified.subsec_nanos()}),
    )
}

pub(crate) fn artifact_identity(path: &Path) -> Result<Value> {
    ensure!(
        path.symlink_metadata()?.is_file(),
        "Encoded artifact {} is not a regular file",
        path.display()
    );
    file_identity(path)
}

pub(crate) fn artifact_destination(path: &Path) -> Result<PathBuf> {
    let parent = path.parent().context("Missing chunk directory")?;
    let name = path.file_name().context("Missing chunk filename")?;
    Ok(crate::core::output::resolve_path(parent)?.join(name))
}

/// Whether this scene can be reused or must be encoded. Only the owning encoder
/// can turn a missing or changed artifact into a replacement task.
pub enum EncodeArtifact {
    Pending,
    Current(Value),
    Missing,
    Changed(Value),
}

#[inline]
pub fn input_identity(input: &Input) -> Result<Value> {
    let mut settings = serde_json::to_value(input)?;
    if let Some(object) = settings.get_mut("VapourSynthScript").and_then(Value::as_object_mut) {
        object.remove("stream_concurrently");
    }
    // Decoder/index/cache *settings* matter; generated cache-file contents/timestamps do not.
    Ok(json!({"settings": settings, "file": input.path().map(file_identity).transpose()?}))
}

fn metric_identity(metric: &QualityMetric) -> Result<Value> {
    let mut metric = metric.clone();
    *metric.target_range_mut() = (0.0, 0.0); // Raw measurements are independent of target policy.
    let model = if let QualityMetric::VMAF { model, .. } = &metric {
        let selection = crate::metrics::vmaf::scorer_config(&metric)?.model;
        let override_dir = std::env::var_os("VMAF_MODEL_PATH").filter(|s| !s.is_empty());
        let path = if let Some(path) = model {
            Some(path.clone())
        } else if let Some(directory) = &override_dir {
            Some(std::path::PathBuf::from(directory).join(format!(
                "{}.json",
                selection.stock_version().context("stock VMAF model version")?
            )))
        } else {
            selection.find_stock_model_file()
        };
        let file = path.as_ref().map(|path| -> Result<Value> {
            #[cfg(test)]
            IDENTITY_COUNTS.with(|counts| { let (models, plugins) = counts.get(); counts.set((models + 1, plugins)); });
            Ok(json!({"identity": file_identity(path)?, "sha256": Sha256::digest(std::fs::read(path)?).to_vec()}))
        }).transpose()?;
        // Stock builtin versus disk fallback is a libvmaf load detail; retain both
        // candidates conservatively, never claim the builtin was actually loaded.
        json!({"stock_version": selection.stock_version(), "override": if model.is_none() { override_dir } else { None }, "file_candidate": file})
    } else {
        Value::Null
    };
    Ok(json!({"settings": metric, "model": model}))
}

fn measurement_backend(
    metric: &QualityMetric,
    input: &Input,
    actual: &MetricBackend,
) -> Result<Value> {
    let planned = if matches!(metric, QualityMetric::VMAF { .. }) {
        Engine::Vmaf
    } else {
        metrics::engine(metric, !matches!(input, Input::Video { .. }))
            .unwrap_or(Engine::VapourSynth)
    };
    let current = if actual.engine == Engine::VapourSynth {
        MetricBackend::available_plugin(metric)?
    } else {
        MetricBackend::current(actual.engine)
    };
    ensure!(
        *actual == current,
        "recorded metric backend version/plugin admission identity changed"
    );
    Ok(json!({"planned": MetricBackend::current(planned), "actual": actual}))
}

/// One owning admission/publication batch. Drop it before revalidating work;
/// model bytes and plugin environments must not be cached across that boundary.
#[derive(Default)]
pub struct Admission {
    metrics: Vec<(Value, Value)>,
    backends: Vec<(Value, bool, MetricBackend, Value)>,
}

impl Admission {
    fn metric(&mut self, metric: &QualityMetric) -> Result<Value> {
        let key = serde_json::to_value(metric)?;
        if let Some((_, identity)) = self.metrics.iter().find(|(saved, _)| *saved == key) {
            return Ok(identity.clone());
        }
        let identity = metric_identity(metric)?;
        self.metrics.push((key, identity.clone()));
        Ok(identity)
    }

    fn backend(
        &mut self,
        metric: &QualityMetric,
        input: &Input,
        actual: &MetricBackend,
    ) -> Result<Value> {
        let script = !matches!(input, Input::Video { .. });
        let key = serde_json::to_value(metric)?;
        if let Some((_, _, current, context)) =
            self.backends.iter().find(|(saved, kind, backend, _)| {
                *saved == key && *kind == script && backend.engine == actual.engine
            })
        {
            ensure!(
                *actual == *current,
                "recorded metric backend version/plugin admission identity changed"
            );
            return Ok(context.clone());
        }
        let context = measurement_backend(metric, input, actual)?;
        self.backends.push((key, script, actual.clone(), context.clone()));
        Ok(context)
    }
}

#[inline]
pub(crate) fn probe_encoder_base<Data: TargetQualityDataHandler>(
    scene: &Scene<Data>,
) -> Result<&crate::models::encoder::Encoder> {
    if let Some(transition) = &scene.sequence_data.get_target_quality()?.speed_transition {
        let mut actual = scene.encoder.clone();
        let mut after = transition.after.clone();
        actual.set_quantizer(0.0);
        after.set_quantizer(0.0);
        if actual == after {
            return Ok(&transition.before);
        }
    }
    Ok(&scene.encoder)
}

#[inline]
pub fn probe_encoder<Data: TargetQualityDataHandler>(
    scene: &Scene<Data>,
    config: &TargetQualityConfig,
) -> Result<crate::models::encoder::Encoder> {
    Ok(config.probe_encoder(probe_encoder_base(scene)?))
}

impl Admission {
    #[inline]
    pub fn target_context<Data: TargetQualityDataHandler>(
        &mut self,
        input: &Input,
        metric_input: Option<&Input>,
        scene: &Scene<Data>,
        config: &TargetQualityConfig,
        pass: &QualityPass,
    ) -> Result<Value> {
        let backend = pass
            .backend
            .as_ref()
            .context("successful probe has no actual backend identity")?;
        let mut encoder = probe_encoder(scene, config)?;
        encoder.set_quantizer(pass.quantizer);
        let mut context = json!({"input": input_identity(input)?, "reference": input_identity(metric_input.unwrap_or(input))?,
        "range": [scene.start_frame, scene.end_frame], "strategy": config.probing.strategy,
        "encoder": encoder, "metric": self.metric(&config.metric)?, "statistic": config.probing.statistic,
        "backend": self.backend(&config.metric, metric_input.unwrap_or(input), backend)?});
        if matches!(encoder.base(), EncoderBase::X264 | EncoderBase::X265) {
            context["probe_encoding"] = json!("independent-scenes-1");
        }
        Ok(context)
    }

    #[inline]
    pub fn admit_target<Data: TargetQualityDataHandler>(
        &mut self,
        input: &Input,
        metric_input: Option<&Input>,
        scene: &Scene<Data>,
        config: Option<&TargetQualityConfig>,
        index: usize,
    ) -> Result<()> {
        let passes = &scene.sequence_data.get_target_quality()?.passes;
        for pass in passes {
            require_revision(pass.provenance.as_ref(), index, "TQ probe")?;
            let config =
                config.ok_or_else(|| refusal(index, "TQ probe", "measurement settings removed"))?;
            let expected = self
                .target_context(input, metric_input, scene, config, pass)
                .map_err(|e| refusal(index, "TQ probe", &format!("{e:#}")))?;
            require_context(pass.provenance.as_ref(), &expected, index, "TQ probe")?;
            ensure!(
                pass.backend == passes[0].backend,
                refusal(index, "TQ probe", "mixed actual metric backends")
            );
        }
        if let Some(config) = config
            && !passes.is_empty()
        {
            let expected =
                config.probing.strategy.frame_indices(scene.start_frame, scene.end_frame)?.len();
            crate::core::sequence::target_quality::TargetQuality::validate_history(
                config, index, expected, passes,
            )?;
        }
        Ok(())
    }

    /// Capture before work and compare after: changing a declared input mid-run cannot
    /// certify results measured/encoded from its previous contents.
    #[inline]
    pub fn measurement_inputs(
        &mut self,
        input: &Input,
        reference: Option<&Input>,
        metric: Option<&QualityMetric>,
    ) -> Result<Value> {
        Ok(
            json!({"input": input_identity(input)?, "reference": reference.map(input_identity).transpose()?, "metric": metric.map(|metric| self.metric(metric)).transpose()?}),
        )
    }

    #[inline]
    pub fn quality_context<Data>(
        &mut self,
        input: &Input,
        output: &Path,
        scene: &Scene<Data>,
        config: &QualityCheckConfig,
        backend: &MetricBackend,
    ) -> Result<Value> {
        let mut context = json!({"input": input_identity(input)?, "output": file_identity(output)?, "range": [scene.start_frame, scene.end_frame],
        "strategy": config.strategy, "statistic": config.statistic, "metric": self.metric(&config.metric)?,
        "backend": self.backend(&config.metric, input, backend)?});
        if backend.engine == Engine::VapourSynth {
            // Old QC callbacks had no positional guarantee; do not certify their frame association.
            context["score_order"] = json!("sample-position-1");
        }
        Ok(context)
    }

    #[inline]
    pub fn admit_quality<Data>(
        &mut self,
        input: &Input,
        output: &Path,
        scene: &Scene<Data>,
        config: Option<&QualityCheckConfig>,
        quality: &QualityPass,
        index: usize,
    ) -> Result<()> {
        if quality.is_unmeasured() {
            return Ok(());
        }
        require_revision(quality.provenance.as_ref(), index, "quality")?;
        let config = config.ok_or_else(|| refusal(index, "quality", "quality settings removed"))?;
        let expected_count =
            config.strategy.frame_indices(scene.start_frame, scene.end_frame)?.len();
        crate::models::sequence::target_quality::types::validate_scores(&quality.scores,Some(expected_count))
        .and_then(|()| config.metric.calculate_score(config.statistic,&quality.scores).map(|_| ()))
        .with_context(|| refusal(index,"quality","recorded Quality Check history is malformed; explicitly reset quality-check before remeasurement"))?;
        let expected = (|| {
            self.quality_context(
                input,
                output,
                scene,
                config,
                quality.backend.as_ref().context("actual backend missing")?,
            )
        })()
        .map_err(|e| refusal(index, "quality", &format!("{e:#}")))?;
        require_context(quality.provenance.as_ref(), &expected, index, "quality")
    }

    #[inline]
    #[expect(
        clippy::too_many_arguments,
        reason = "batch form retains the existing context API"
    )]
    pub fn encode_context<Data: SequenceDataHandler + TargetQualityDataHandler>(
        &mut self,
        input: &Input,
        probe_input: &Input,
        metric_input: Option<&Input>,
        scene: &Scene<Data>,
        tq: Option<&TargetQualityConfig>,
        output: &Path,
        index: usize,
    ) -> Result<Value> {
        let mut context =
            self.encode_producer_context(input, probe_input, metric_input, scene, tq, index)?;
        context["artifact"] = file_identity(output)?;
        Ok(context)
    }

    fn encode_producer_context<Data: TargetQualityDataHandler>(
        &mut self,
        input: &Input,
        probe_input: &Input,
        metric_input: Option<&Input>,
        scene: &Scene<Data>,
        tq: Option<&TargetQualityConfig>,
        index: usize,
    ) -> Result<Value> {
        self.admit_target(probe_input, metric_input, scene, tq, index)?;
        admit_selection(scene, tq, index)?;
        let passes = &scene.sequence_data.get_target_quality()?.passes;
        let dependency = if passes.is_empty() {
            Value::Null
        } else {
            serde_json::to_value(&scene.sequence_data.get_target_quality()?.selection)?
        };
        Ok(
            json!({"input": input_identity(input)?, "range": [scene.start_frame, scene.end_frame], "sub_scenes": scene.sub_scenes,
         "encoder": scene.encoder, "tq_dependency": dependency}),
        )
    }
}

// Single-scene callers retain the narrow API; owning loops share an Admission.
#[inline]
pub fn target_context<Data: TargetQualityDataHandler>(
    input: &Input,
    metric_input: Option<&Input>,
    scene: &Scene<Data>,
    config: &TargetQualityConfig,
    pass: &QualityPass,
) -> Result<Value> {
    Admission::default().target_context(input, metric_input, scene, config, pass)
}

#[inline]
pub fn admit_target<Data: TargetQualityDataHandler>(
    input: &Input,
    metric_input: Option<&Input>,
    scene: &Scene<Data>,
    config: Option<&TargetQualityConfig>,
    index: usize,
) -> Result<()> {
    Admission::default().admit_target(input, metric_input, scene, config, index)
}

#[inline]
pub fn measurement_inputs(
    input: &Input,
    reference: Option<&Input>,
    metric: Option<&QualityMetric>,
) -> Result<Value> {
    Admission::default().measurement_inputs(input, reference, metric)
}

#[inline]
pub fn quality_context<Data>(
    input: &Input,
    output: &Path,
    scene: &Scene<Data>,
    config: &QualityCheckConfig,
    backend: &MetricBackend,
) -> Result<Value> {
    Admission::default().quality_context(input, output, scene, config, backend)
}

#[inline]
pub fn admit_quality<Data>(
    input: &Input,
    output: &Path,
    scene: &Scene<Data>,
    config: Option<&QualityCheckConfig>,
    quality: &QualityPass,
    index: usize,
) -> Result<()> {
    Admission::default().admit_quality(input, output, scene, config, quality, index)
}

#[inline]
pub fn encode_context<Data: SequenceDataHandler + TargetQualityDataHandler>(
    input: &Input,
    probe_input: &Input,
    metric_input: Option<&Input>,
    scene: &Scene<Data>,
    tq: Option<&TargetQualityConfig>,
    output: &Path,
    index: usize,
) -> Result<Value> {
    Admission::default().encode_context(input, probe_input, metric_input, scene, tq, output, index)
}

/// Structured policy plus canonical content identity of the separately stored raw
/// measurements; no repeated whole-history context in each selection/chunk.
#[inline]
pub fn selection_context<Data: TargetQualityDataHandler>(
    scene: &Scene<Data>,
    config: &TargetQualityConfig,
) -> Result<Value> {
    let passes = &scene.sequence_data.get_target_quality()?.passes;
    let history = crate::core::sequence::target_quality::TargetQuality::quantizer_score_history(
        config, passes,
    )?;
    let selected = crate::core::sequence::target_quality::TargetQuality::measured_quantizer(
        &config.metric,
        &history,
    );
    let mut raw = json!(passes.iter().map(|p| json!({"q": p.quantizer, "scores": p.scores, "bitrate": p.bitrate, "backend": p.backend, "context": p.provenance})).collect::<Vec<_>>());
    raw.sort_all_objects();
    let history_sha256 = Sha256::digest(serde_json::to_vec(&raw)?).to_vec();
    Ok(
        json!({"history_sha256": history_sha256, "target": config.metric.target_range(), "budget": config.maximum_probes, "quantizer_range": config.quantizer_range, "interpolators": config.interpolators, "selected": selected}),
    )
}

/// Require current-policy selection while allowing later stages to adjust the
/// final encoder. Each chunk separately binds that effective final encoder.
#[inline]
pub fn admit_selection<Data: TargetQualityDataHandler>(
    scene: &Scene<Data>,
    tq: Option<&TargetQualityConfig>,
    index: usize,
) -> Result<()> {
    let passes = &scene.sequence_data.get_target_quality()?.passes;
    if !passes.is_empty() {
        let config = tq.ok_or_else(|| refusal(index, "encoding", "TQ settings removed"))?;
        require_context(
            scene.sequence_data.get_target_quality()?.selection.as_ref(),
            &selection_context(scene, config)?,
            index,
            "TQ selection (run Target Quality selection before encoding)",
        )?;
    }
    Ok(())
}

impl Admission {
    /// Qualify a saved chunk for recovery without relaxing strict consumer admission.
    #[inline]
    #[allow(clippy::too_many_arguments)]
    pub fn admit_encode_for_recovery<
        Data: SequenceDataHandler + TargetQualityDataHandler + ParallelEncoderDataHandler,
    >(
        &mut self,
        input: &Input,
        probe_input: &Input,
        metric_input: Option<&Input>,
        scene: &Scene<Data>,
        tq: Option<&TargetQualityConfig>,
        output: &Path,
        index: usize,
    ) -> Result<EncodeArtifact> {
        let data = scene.sequence_data.get_parallel_encoder()?;
        data.admit_scene(index)?;
        let metadata = match output.symlink_metadata() {
            Ok(metadata) => Some(metadata),
            Err(error) if error.kind() == ErrorKind::NotFound => None,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("Inspecting scene {index} chunk {}", output.display())
                });
            },
        };
        if metadata.is_none()
            && data.bytes.is_none()
            && data.started_on.is_none()
            && data.completed_on.is_none()
            && data.provenance.is_none()
            && data.verification.is_none()
        {
            return Ok(EncodeArtifact::Pending);
        }
        let saved = data.provenance.as_ref();
        require_revision(saved, index, "encoding")?;
        ensure!(
            data.bytes.is_some_and(|bytes| bytes > 0)
                && data.started_on.zip(data.completed_on).is_some_and(|(start, end)| start <= end),
            refusal(
                index,
                "encoding",
                "artifact has no usable saved completed encoding record"
            )
        );
        let artifact = saved
            .and_then(|saved| saved.context.get("artifact"))
            .filter(|identity| {
                identity.as_object().is_some_and(|fields| fields.len() == 4)
                    && identity.get("length").and_then(Value::as_u64).is_some_and(|size| size > 0)
                    && identity.get("modified_seconds").and_then(Value::as_u64).is_some()
                    && identity
                        .get("modified_nanos")
                        .and_then(Value::as_u64)
                        .is_some_and(|ns| ns < 1_000_000_000)
            })
            .ok_or_else(|| refusal(index, "encoding", "missing or invalid chunk fingerprint"))?;
        let destination = artifact_destination(output)?;
        ensure!(
            artifact.get("path") == Some(&serde_json::to_value(&destination)?),
            refusal(index, "encoding", "chunk destination changed")
        );
        let mut expected = self
            .encode_producer_context(input, probe_input, metric_input, scene, tq, index)
            .map_err(|e| refusal(index, "encoding", &format!("{e:#}")))?;
        expected["artifact"] = artifact.clone();
        require_context(saved, &expected, index, "encoding")?;
        let Some(metadata) = metadata else {
            return Ok(EncodeArtifact::Missing);
        };
        ensure!(
            metadata.is_file(),
            refusal(index, "encoding", "chunk destination is not a regular file")
        );
        let current = artifact_identity(output)?;
        ensure!(
            current["path"] == artifact["path"],
            "Scene {index} chunk path changed while inspecting {}",
            output.display()
        );
        if &current == artifact {
            Ok(EncodeArtifact::Current(current))
        } else {
            Ok(EncodeArtifact::Changed(current))
        }
    }

    #[inline]
    #[expect(
        clippy::too_many_arguments,
        reason = "batch form retains the existing admission API"
    )]
    pub fn admit_encode<
        Data: SequenceDataHandler + TargetQualityDataHandler + ParallelEncoderDataHandler,
    >(
        &mut self,
        input: &Input,
        probe_input: &Input,
        metric_input: Option<&Input>,
        scene: &Scene<Data>,
        tq: Option<&TargetQualityConfig>,
        output: &Path,
        index: usize,
    ) -> Result<()> {
        scene.sequence_data.get_parallel_encoder()?.admit_scene(index)?;
        let saved = scene.sequence_data.get_parallel_encoder()?.provenance.as_ref();
        require_revision(saved, index, "encoding")?;
        let expected = self
            .encode_context(input, probe_input, metric_input, scene, tq, output, index)
            .map_err(|e| refusal(index, "encoding", &format!("{e:#}")))?;
        require_context(saved, &expected, index, "encoding")
    }
}

#[inline]
pub fn admit_encode<
    Data: SequenceDataHandler + TargetQualityDataHandler + ParallelEncoderDataHandler,
>(
    input: &Input,
    probe_input: &Input,
    metric_input: Option<&Input>,
    scene: &Scene<Data>,
    tq: Option<&TargetQualityConfig>,
    output: &Path,
    index: usize,
) -> Result<()> {
    Admission::default().admit_encode(input, probe_input, metric_input, scene, tq, output, index)
}

/// Effective declared inputs, also used by CLI preflight without opening decoders.
#[inline]
pub fn probe_input<'a>(
    root: &'a Input,
    pe: &'a ParallelEncoderConfig,
    tq: Option<&'a TargetQualityConfig>,
) -> &'a Input {
    tq.and_then(|c| c.input.as_ref()).or(pe.input.as_ref()).unwrap_or(root)
}

/// Shared declared QC reference policy; runtime QualityCheck::input remains an
/// explicit owning override. Merely resolving this model opens no decoder.
#[inline]
pub fn quality_input<'a>(
    root: &'a Input,
    pe: &'a ParallelEncoderConfig,
    qc: Option<&'a QualityCheckConfig>,
) -> &'a Input {
    qc.and_then(|c| c.input.as_ref()).or(pe.input.as_ref()).unwrap_or(root)
}

#[cfg(test)]
mod tests {
    #[test]
    fn legacy_result_refusal_names_av1an_reset_command() {
        let error = super::require_revision(None, 3, "TQ probe")
            .expect_err("a legacy result must be refused");
        assert!(error.to_string().contains("av1an scenes 3 reset"));
    }
}
