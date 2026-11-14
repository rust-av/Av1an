use super::*;

#[path = "../../../../../tests/core/sequence/scene_concatenator/fixtures/scene_list.rs"]
mod fixture;

pub(super) fn job(
    root: &Path,
    output: &Path,
    count: usize,
) -> Condor<DefaultSequenceData, DefaultSequenceConfig> {
    fixture::job(root, output, count)
}

mod internal_integration {
    use super::*;

    #[test]
    fn all_final_backends_finish_staging_before_publication_or_completion() {
        let mut fixture = fixture::NativeScenes::new();
        for (index, method) in fixture::methods().into_iter().enumerate() {
            let (destination, old) = fixture.prepare_final(index, method);
            let (tx, rx) = sync::mpsc::channel();
            let rx = Arc::new(Mutex::new(rx));
            let mut concat = SceneConcatenator::default();
            concat
                .initialize(&mut fixture.job, tx.clone())
                .expect("complete initialization");
            crate::core::sequence::tests::support::current_encoding_fixture(&mut fixture.job);
            let saved =
                serde_json::to_value(&fixture.job.sequence_config).expect("config snapshot");
            let final_path = destination.clone();
            let statuses = Arc::clone(&rx);
            let expected = fixture.frames.concat();
            let reached = Arc::new(AtomicBool::new(false));
            let observed = Arc::clone(&reached);
            publication::observe_commit(move |stage| {
                assert_eq!(
                    std::fs::read(&destination).ok(),
                    old,
                    "no final output before commit"
                );
                assert_eq!(
                    fixture::decoded(stage),
                    expected,
                    "complete native stage before publication"
                );
                assert!(
                    !statuses
                        .lock()
                        .expect("statuses")
                        .try_iter()
                        .any(|s| matches!(s, SequenceStatus::Whole(Status::Completed { .. })))
                );
                observed.store(true, std::sync::atomic::Ordering::SeqCst);
            });
            concat
                .execute_cancel(&mut fixture.job, tx, Cancellation::default())
                .expect("current saved-job-admission fixture admitted");
            assert_eq!(
                fixture::decoded(&final_path),
                fixture.frames.concat(),
                "Sequence order/count"
            );
            assert_eq!(
                rx.lock()
                    .expect("statuses")
                    .try_iter()
                    .filter(|s| matches!(s, SequenceStatus::Whole(Status::Completed { .. })))
                    .count(),
                1
            );
            assert!(reached.load(std::sync::atomic::Ordering::SeqCst));
            assert!(
                !fixture.job.output.path.exists(),
                "effective output override"
            );
            assert_eq!(
                serde_json::to_value(&fixture.job.sequence_config).expect("config"),
                saved
            );
            fixture::assert_clean(fixture.root());
        }
        fixture.assert_unchanged();
    }
}
