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
        let path = pe
            .scenes_directory
            .join(format!("{index:05}.{}", scene.encoder.output_extension()));
        if path.is_file() {
            let context = provenance::encode_context(
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
