//! Exact publication seams remain private; public failure workflows live in tests/core.
use super::*;
use std::sync::atomic::AtomicBool;

type StageCheck = Box<dyn FnOnce(&Path)>;
type PersistCheck = Box<dyn FnOnce(&Path, &Path)>;
thread_local! {
    static BEFORE_COMMIT: std::cell::RefCell<Option<StageCheck>> = const { std::cell::RefCell::new(None) };
    static BEFORE_PERSIST: std::cell::RefCell<Option<PersistCheck>> = const { std::cell::RefCell::new(None) };
    static DURING_PAYLOAD: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

pub(crate) fn before_commit(staged: &Path) {
    if let Some(check) = BEFORE_COMMIT.with_borrow_mut(Option::take) {
        check(staged);
    }
}

pub(super) fn observe_commit(check: impl FnOnce(&Path) + 'static) {
    BEFORE_COMMIT.with_borrow_mut(|hook| *hook = Some(Box::new(check)));
}

pub(crate) fn before_persist(staged: &Path, destination: &Path) {
    if let Some(check) = BEFORE_PERSIST.with_borrow_mut(Option::take) {
        check(staged, destination);
    }
}

#[cfg(unix)]
pub(super) fn remove_stage_before_persist() {
    BEFORE_PERSIST.with_borrow_mut(|hook| {
        *hook = Some(Box::new(|staged, _| {
            std::fs::remove_file(staged).expect("remove only owned stage after its shape check");
        }));
    });
}

pub(crate) fn during_payload(staged: &Path, token: &Cancellation) {
    if let Some(force) = DURING_PAYLOAD.replace(None) {
        assert!(std::fs::metadata(staged).expect("partial IVF").len() > 32);
        token.request().expect("graceful mid-record");
        if force {
            token.request().expect("force mid-record");
        }
    }
}

mod internal_integration {
    use super::*;
    use crate::utils::cancellation::Cancelled;

    fn ivf_job(root: &Path, output: &Path) -> Condor<DefaultSequenceData, DefaultSequenceConfig> {
        let mut job = scene_list::job(root, output, 1);
        // Opaque framed payloads exercise cancellation, not native codec decodability.
        let mut bytes =
            b"DKIF\0\0\x20\0AV01\x20\0\x10\0\x18\0\0\0\x01\0\0\0\x02\0\0\0\0\0\0\0".to_vec();
        for timestamp in [0u64, 1] {
            bytes.extend_from_slice(&16384u32.to_le_bytes());
            bytes.extend_from_slice(&timestamp.to_le_bytes());
            bytes.extend_from_slice(&[0xAB; 16384]);
        }
        std::fs::write(root.join("00000.ivf"), bytes).expect("framed IVF fixture");
        crate::core::sequence::tests::support::current_encoding_fixture(&mut job);
        job
    }

    #[test]
    fn ivf_mid_record_graceful_finishes_but_force_interrupts_without_publication() {
        for force in [false, true] {
            let dir = tempfile::tempdir().expect("IVF cancellation fixture");
            let output = dir.path().join("final.ivf");
            std::fs::write(&output, b"old final bytes").expect("old final");
            let mut job = ivf_job(dir.path(), &output);
            let original = std::fs::read(dir.path().join("00000.ivf")).expect("original chunk");
            let finished = Arc::new(AtomicBool::new(false));
            let observed = Arc::clone(&finished);
            BEFORE_COMMIT.with_borrow_mut(|hook| {
                *hook = Some(Box::new(move |staged| {
                    assert_eq!(
                        std::fs::read(staged).expect("finished admitted IVF"),
                        original
                    );
                    observed.store(true, std::sync::atomic::Ordering::SeqCst);
                }));
            });
            DURING_PAYLOAD.set(Some(force));
            let (tx, rx) = sync::mpsc::channel();
            let error = SceneConcatenator::default()
                .execute(&mut job, tx, Cancellation::default())
                .expect_err("cancelled final Sequence");
            BEFORE_COMMIT.with_borrow_mut(|hook| {
                hook.take();
            });
            assert_eq!(
                error.downcast_ref::<Cancelled>(),
                Some(&if force {
                    Cancelled::Forced
                } else {
                    Cancelled::Graceful
                })
            );
            assert_eq!(finished.load(std::sync::atomic::Ordering::SeqCst), !force);
            assert_eq!(
                std::fs::read(&output).expect("old final preserved"),
                b"old final bytes"
            );
            assert_eq!(completed(&rx), 0);
            assert_clean(dir.path());
        }
    }

    #[test]
    fn sequence_post_last_admission_race_uses_atomic_noclobber() {
        for overwrite in [false, true] {
            let dir = tempfile::tempdir().expect("publication race fixture");
            let output = dir.path().join("final.ivf");
            let mut job = ivf_job(dir.path(), &output);
            job.output.overwrite = overwrite;
            let original = std::fs::read(dir.path().join("00000.ivf")).expect("chunk");
            let reached = Arc::new(AtomicBool::new(false));
            let observed = Arc::clone(&reached);
            BEFORE_PERSIST.with_borrow_mut(|hook| {
                *hook = Some(Box::new(move |staged, destination| {
                    assert!(!destination.exists(), "absent through last admission");
                    assert!(std::fs::metadata(staged).expect("finished stage").len() > 0);
                    std::fs::write(destination, b"racer owns final")
                        .expect("only inject owned file creation");
                    observed.store(true, std::sync::atomic::Ordering::SeqCst);
                }));
            });
            let (tx, rx) = sync::mpsc::channel();
            let result =
                SceneConcatenator::default().execute(&mut job, tx, Cancellation::default());
            assert!(reached.load(std::sync::atomic::Ordering::SeqCst));
            if overwrite {
                result.expect("explicit consent may replace the late file");
                assert_eq!(std::fs::read(&output).expect("final"), original);
                assert_eq!(completed(&rx), 1);
            } else {
                let error = result.expect_err("atomic refusal, not readmission refusal");
                assert_eq!(
                    error
                        .downcast_ref::<tempfile::PathPersistError>()
                        .expect("original atomic persist cause")
                        .error
                        .kind(),
                    std::io::ErrorKind::AlreadyExists
                );
                assert_eq!(std::fs::read(&output).expect("racer"), b"racer owns final");
                assert_eq!(completed(&rx), 0);
                drop(error); // PathPersistError retains ownership of the failed staging path.
            }
            assert_clean(dir.path());
        }
    }

    #[test]
    fn cancellation_before_lock_withdraws_but_after_commit_does_not() {
        for before in [false, true] {
            let dir = tempfile::tempdir().expect("linearization fixture");
            let output = dir.path().join("final.ivf");
            let mut job = ivf_job(dir.path(), &output);
            let token = Cancellation::default();
            if before {
                let token = token.clone();
                BEFORE_COMMIT.with_borrow_mut(|hook| {
                    *hook = Some(Box::new(move |_| {
                        token.request().expect("request outside publication lock");
                    }));
                });
            }
            let (tx, rx) = sync::mpsc::channel();
            let result = SceneConcatenator::default().execute(&mut job, tx, token.clone());
            if before {
                assert_eq!(
                    result.expect_err("cancel before lock").downcast_ref::<Cancelled>(),
                    Some(&Cancelled::Graceful)
                );
                assert!(!output.exists());
                assert_eq!(completed(&rx), 0);
            } else {
                result.expect("commit wins");
                token.request().expect("graceful after commit");
                token.request().expect("force after commit");
                assert_eq!(completed(&rx), 1);
                assert_eq!(
                    std::fs::read(&output).expect("final"),
                    std::fs::read(dir.path().join("00000.ivf")).expect("chunk")
                );
            }
            assert_clean(dir.path());
        }
    }

    fn completed(rx: &sync::mpsc::Receiver<SequenceStatus>) -> usize {
        rx.try_iter()
            .filter(|s| matches!(s, SequenceStatus::Whole(Status::Completed { .. })))
            .count()
    }

    fn assert_clean(root: &Path) {
        assert!(!std::fs::read_dir(root).expect("staging cleanup").any(|e| {
            e.expect("entry").file_name().to_string_lossy().starts_with(".condor-output-")
        }));
    }
}
