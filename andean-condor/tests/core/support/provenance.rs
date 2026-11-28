use andean_condor::{
    core::Condor,
    models::sequence::{
        SequenceConfigHandler, SequenceDataHandler,
        parallel_encoder::{ParallelEncoderConfigHandler, ParallelEncoderDataHandler},
        provenance::{self, ResultProvenance},
        target_quality::{TargetQualityConfigHandler, TargetQualityDataHandler},
    },
};

// Synthetic current-context stamps exercise consumer admission, not encoder production.
pub(in crate::core) fn current_encoding_fixture<D, C>(job: &mut Condor<D, C>)
where
    D: SequenceDataHandler + TargetQualityDataHandler + ParallelEncoderDataHandler,
    C: SequenceConfigHandler + TargetQualityConfigHandler + ParallelEncoderConfigHandler,
{
    let root = job.input.as_data();
    let pe = job.sequence_config.parallel_encoder().expect("PE");
    let tq = job.sequence_config.target_quality().expect("TQ").as_ref();
    for (index, scene) in job.scenes.iter_mut().enumerate() {
        let path = pe.scenes_directory.join(format!(
            "{index:05}.{}",
            scene.encoder.base().output_extension()
        ));
        if path.is_file() {
            let context = provenance::Admission::default()
                .encode_context(
                    pe.input.as_ref().unwrap_or(&root),
                    provenance::probe_input(&root, pe, tq),
                    tq.and_then(|c| c.metric_input.as_ref()),
                    scene,
                    tq,
                    &path,
                    index,
                )
                .expect("explicit current chunk fixture");
            scene.sequence_data.get_parallel_encoder_mut().expect("PE data").provenance =
                Some(ResultProvenance::new(context));
        }
    }
}

// Synthetic current-context stamps test consumer admission, not owning-producer publication.
#[cfg(unix)]
pub(in crate::core) fn current_quality_fixture(
    job: &mut Condor<
        andean_condor::models::sequence::DefaultSequenceData,
        andean_condor::models::sequence::DefaultSequenceConfig,
    >,
) {
    use andean_condor::{
        metrics,
        models::{input::Input as InputModel, sequence::provenance::MetricBackend},
    };
    let root = job.input.as_data();
    let qc = job.sequence_config.quality_check.as_ref().expect("QC");
    let input = qc.input.as_ref().unwrap_or(&root);
    let output = job.sequence_config.scene_concatenator.effective_output(&job.output.path);
    for scene in &mut job.scenes {
        if !scene.sequence_data.quality_check.quality.is_unmeasured() {
            let engine = metrics::engine(&qc.metric, !matches!(input, InputModel::Video { .. }))
                .unwrap_or(metrics::Engine::VapourSynth);
            let backend = if engine == metrics::Engine::VapourSynth {
                MetricBackend::available_plugin(&qc.metric).expect("actual plugin fixture")
            } else {
                MetricBackend::current(engine)
            };
            let context = provenance::Admission::default()
                .quality_context(input, output, scene, qc, &backend)
                .expect("explicit current quality fixture");
            let pass = &mut scene.sequence_data.quality_check.quality;
            pass.backend = Some(backend);
            pass.provenance = Some(ResultProvenance::new(context));
        }
    }
}

// Synthetic saved records exercise consumer admission, not owning-producer certification.
pub(in crate::core) fn current_target_fixture<D, C>(job: &mut Condor<D, C>)
where
    D: andean_condor::models::sequence::SequenceDataHandler
        + andean_condor::models::sequence::target_quality::TargetQualityDataHandler,
    C: andean_condor::models::sequence::SequenceConfigHandler
        + andean_condor::models::sequence::target_quality::TargetQualityConfigHandler
        + andean_condor::models::sequence::parallel_encoder::ParallelEncoderConfigHandler,
{
    use andean_condor::models::input::Input as InputModel;
    use andean_condor::models::sequence::provenance::{self, ResultProvenance};
    let root = job.input.as_data();
    let pe = job.sequence_config.parallel_encoder().expect("PE");
    let tq = job
        .sequence_config
        .target_quality()
        .expect("TQ")
        .as_ref()
        .expect("configured TQ");
    let input = provenance::probe_input(&root, pe, Some(tq));
    for scene in &mut job.scenes {
        let mut passes = scene.sequence_data.get_target_quality().expect("data").passes.clone();
        for pass in &mut passes {
            pass.backend.get_or_insert_with(|| {
                let engine = if matches!(
                    tq.metric,
                    andean_condor::models::sequence::target_quality::types::QualityMetric::VMAF { .. }
                ) {
                    andean_condor::metrics::Engine::Vmaf
                } else {
                    andean_condor::metrics::engine(&tq.metric, !matches!(input, InputModel::Video { .. }))
                        .unwrap_or(andean_condor::metrics::Engine::VapourSynth)
                };
                if engine == andean_condor::metrics::Engine::VapourSynth {
                    provenance::MetricBackend::available_plugin(&tq.metric)
                        .expect("actual plugin fixture")
                } else {
                    provenance::MetricBackend::current(engine)
                }
            });
            pass.provenance = Some(ResultProvenance::new(
                provenance::Admission::default()
                    .target_context(input, tq.metric_input.as_ref(), scene, tq, pass)
                    .expect("explicit current fixture context"),
            ));
        }
        scene.sequence_data.get_target_quality_mut().expect("data").passes = passes;
        if !scene.sequence_data.get_target_quality().expect("data").passes.is_empty() {
            let selection = ResultProvenance::new(
                provenance::selection_context(scene, tq)
                    .expect("explicit current selection fixture"),
            );
            scene.sequence_data.get_target_quality_mut().expect("data").selection = Some(selection);
        }
    }
}
