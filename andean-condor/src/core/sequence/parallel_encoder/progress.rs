use std::sync::{Mutex, mpsc::Sender};

use anyhow::Result;

use super::task::{PublishedResult, ResultStream};
use crate::{
    core::{
        encoder::EncodeProgress,
        sequence::{SequenceCompletion, SequenceStatus, Status, parallel_encoder::DETAILS},
    },
    models::encoder::EncoderPasses,
};

/// One dispatch's usable logical frames, including validated resume work.
pub(super) struct Progress {
    completed: Mutex<usize>,
    total: usize,
    tx: Sender<SequenceStatus>,
}

impl Progress {
    pub(super) fn new(tx: Sender<SequenceStatus>, total: usize, initial: usize) -> Self {
        Self {
            completed: Mutex::new(initial),
            total,
            tx,
        }
    }

    fn parent(&self, completed: usize) -> Status {
        Status::Processing {
            id: DETAILS.name.to_owned(),
            completion: SequenceCompletion::Frames {
                completed: completed as u64,
                total: self.total as u64,
            },
        }
    }

    fn whole(&self, completed: usize) {
        let _ = self.tx.send(SequenceStatus::Whole(self.parent(completed)));
    }
}

/// Own tentative credit until the validated artifact is published. Drop rolls
/// back only this attempt; all codec/producer/relay joins precede that drop.
pub(super) struct AttemptProgress<'a> {
    progress: &'a Progress,
    scene: usize,
    frames: usize,
    passes: (u8, u8),
    // Only the relay mutates this until it joins; the owner then commits/drops.
    state: Mutex<(u8, usize, usize)>,
    committed: bool,
}

impl<'a> AttemptProgress<'a> {
    #[cfg(all(test, target_os = "linux"))]
    pub(super) fn scene(&self) -> usize {
        self.scene
    }

    pub(super) fn new(
        progress: &'a Progress,
        scene: usize,
        frames: usize,
        passes: EncoderPasses,
    ) -> Self {
        // Specific runs one invocation but retains its caller's native pass tuple.
        let passes = match passes {
            EncoderPasses::All(total) => (total, total),
            EncoderPasses::Specific(selected, total) => (selected, total),
        };
        // Private parallel protocol: `attempt` precedes native records, even
        // with no records. Only this explicit boundary resets the UI panel.
        let _ = progress.tx.send(SequenceStatus::Whole(Status::Processing {
            id: scene.to_string(),
            completion: SequenceCompletion::Custom {
                name: "attempt".into(),
                completed: 0.0,
                total: 0.0,
            },
        }));
        Self {
            progress,
            scene,
            frames,
            passes,
            state: Mutex::new((0, 0, 0)),
            committed: false,
        }
    }

    fn report(&self, record: EncodeProgress) -> Result<()> {
        let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let (pass, total) = record.pass;
        if total != self.passes.1 || pass == 0 || pass > self.passes.0 || pass < state.0 {
            return Ok(());
        }
        if pass > state.0 {
            state.0 = pass;
            state.1 = 0;
        }
        state.1 = state.1.max(record.frame.min(self.frames));
        // Serialize mutation AND sends: another scene's commit/rollback cannot
        // overtake a total computed earlier. Never held across native waits.
        let mut completed = self
            .progress
            .completed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if pass == self.passes.0 {
            *completed += state.1 - state.2;
            state.2 = state.1;
            self.progress.tx.send(SequenceStatus::Whole(self.progress.parent(*completed)))?;
        }
        self.progress.tx.send(SequenceStatus::Subprocess {
            parent: self.progress.parent(*completed),
            child: Status::Processing {
                id: self.scene.to_string(),
                completion: SequenceCompletion::PassFrames {
                    passes: (pass, total),
                    frames: (state.1 as u64, self.frames as u64),
                },
            },
        })?;
        Ok(())
    }

    pub(super) fn commit(&mut self, bytes: u64) {
        if self.committed {
            return;
        }
        let state = self.state.get_mut().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut completed = self
            .progress
            .completed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *completed += self.frames - state.2;
        self.committed = true;
        self.progress.whole(*completed);
        // Notification loss after rename must not lose saved usable work.
        let size = SequenceCompletion::Custom {
            name: "size".into(),
            completed: bytes as f64,
            total: bytes as f64,
        };
        let _ = self.progress.tx.send(SequenceStatus::Whole(Status::Processing {
            id: self.scene.to_string(),
            completion: size,
        }));
        let _ = self.progress.tx.send(SequenceStatus::Subprocess {
            parent: self.progress.parent(*completed),
            child: Status::Completed {
                id: self.scene.to_string(),
            },
        });
    }
}

impl Drop for AttemptProgress<'_> {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        let state = self.state.get_mut().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut completed = self
            .progress
            .completed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *completed -= state.2;
        self.progress.whole(*completed);
        // Failed is panel cleanup, not a replacement for the typed task error.
        let _ = self.progress.tx.send(SequenceStatus::Subprocess {
            parent: self.progress.parent(*completed),
            child: Status::Failed {
                id: self.scene.to_string(),
                error: "Attempt not published".into(),
            },
        });
    }
}

/// Drains any finished scene results from the channel and forwards them to
/// `on_results`.
pub(super) fn drain_finished_results<F>(stream: &mut ResultStream<'_, F>) -> Result<()>
where
    F: FnMut(Vec<PublishedResult>) -> Result<()>,
{
    let mut results = Vec::new();
    while stream.finished_scenes.try_acquire().is_some() {
        if let Ok(result) = stream.results_rx.try_recv() {
            results.push(result);
        }
    }
    if !results.is_empty() {
        (stream.on_results)(results)?;
    }
    Ok(())
}

/// Forwards one encoder's progress to the sequence's progress channel.
///
/// The parent status carries the whole encode's final-pass total, so every
/// scene's relay contributes to one shared bar.
pub(super) fn relay_progress(
    encode_progress_rx: std::sync::mpsc::Receiver<EncodeProgress>,
    attempt: &AttemptProgress<'_>,
) -> Result<()> {
    for progress in encode_progress_rx {
        attempt.report(progress)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{Receiver, Sender, channel};

    use super::{AttemptProgress, Progress, relay_progress};
    use crate::core::{encoder::EncodeProgress, sequence::SequenceStatus};
    use crate::models::encoder::EncoderPasses;

    fn check_specific_pass(selected: u8) {
        use crate::models::encoder::{Encoder, EncoderBase, EncoderPasses};

        let mut encoder = Encoder::default_from_base(&EncoderBase::X264, true);
        *encoder.passes_mut().expect("passes") = EncoderPasses::Specific(selected, 2);
        let (tx, rx) = channel();
        let total = Progress::new(tx, 30, 0);
        {
            let attempt = AttemptProgress::new(&total, 7, 30, encoder.passes());
            let (tx, progress_rx) = channel();
            for frame in [1, 10, 10, 9, 20, usize::MAX] {
                tx.send(EncodeProgress {
                    pass: (selected, 2),
                    frame,
                    usage: Default::default(),
                })
                .expect("native tuple");
            }
            drop(tx);
            relay_progress(progress_rx, &attempt).expect("relay");
            assert_eq!(
                *total.completed.lock().expect("total"),
                30,
                "configured selected pass contributes absolute deltas"
            );
            let panels: Vec<_> = rx
                .try_iter()
                .filter_map(|event| match event {
                    SequenceStatus::Subprocess {
                        child:
                            crate::core::sequence::Status::Processing {
                                completion:
                                    crate::core::sequence::SequenceCompletion::PassFrames {
                                        passes,
                                        frames,
                                    },
                                ..
                            },
                        ..
                    } => Some((passes, frames)),
                    SequenceStatus::Subprocess {
                        child: crate::core::sequence::Status::Completed { .. },
                        ..
                    } => panic!("progress cannot publish"),
                    _ => None,
                })
                .collect();
            assert_eq!(
                panels,
                [1, 10, 10, 10, 20, 30].map(|frame| ((selected, 2), (frame, 30)))
            );
            for pass in [(0, 2), (selected, 1), (3, 2), (if selected == 1 { 2 } else { 1 }, 2)] {
                attempt
                    .report(EncodeProgress {
                        pass,
                        frame: 30,
                        usage: Default::default(),
                    })
                    .expect("invalid/stale/unselected tuple");
            }
            assert!(
                rx.try_recv().is_err(),
                "invalid/stale/unselected tuples do not advance the panel"
            );
        }
        assert_eq!(
            *total.completed.lock().expect("total"),
            0,
            "unpublished selected pass rolls back"
        );
        let mut retry = AttemptProgress::new(&total, 7, 30, encoder.passes());
        retry
            .report(EncodeProgress {
                pass: (selected, 2),
                frame: 10,
                usage: Default::default(),
            })
            .expect("retry progress");
        assert_eq!(
            *total.completed.lock().expect("total"),
            10,
            "retry begins at a fresh high-water"
        );
        retry.commit(42);
        retry.commit(42);
        assert_eq!(
            *total.completed.lock().expect("total"),
            30,
            "commit reconciles only the missing frames once"
        );
        assert_eq!(
            rx.try_iter()
                .filter(|event| matches!(
                    event,
                    SequenceStatus::Subprocess {
                        child: crate::core::sequence::Status::Completed { .. },
                        ..
                    }
                ))
                .count(),
            1
        );
    }

    #[test]
    fn specific_final_pass_keeps_native_tuple_and_logical_credit() {
        check_specific_pass(2);
    }

    #[test]
    fn specific_first_pass_keeps_native_tuple_and_logical_credit() {
        check_specific_pass(1);
    }

    #[test]
    fn sparse_progress_clamps_duplicate_and_regressing_frames_to_the_high_water_mark() {
        let (tx, rx) = channel();
        let (status_tx, status_rx) = channel();
        let total = Progress::new(status_tx, 30, 0);
        let attempt = AttemptProgress::new(&total, 7, 30, EncoderPasses::All(1));
        for frame in [1, 10, 10, 9, 30] {
            tx.send(EncodeProgress {
                pass: (1, 1),
                frame,
                usage: Default::default(),
            })
            .expect("progress");
        }
        drop(tx);
        relay_progress(rx, &attempt).expect("relay");
        let counts: Vec<_> =
            status_rx
                .try_iter()
                .filter_map(|event| match event {
                    SequenceStatus::Subprocess {
                        child:
                            crate::core::sequence::Status::Processing {
                                completion:
                                    crate::core::sequence::SequenceCompletion::PassFrames {
                                        frames, ..
                                    },
                                ..
                            },
                        ..
                    } => Some(frames.0),
                    _ => None,
                })
                .collect();
        assert_eq!(*total.completed.lock().expect("total"), 30);
        assert_eq!(counts, [1, 10, 10, 10, 30]);
    }

    /// Drives `relay_progress` with one scene's final pass and returns every
    /// status it emitted.
    fn relay_one_scene(frames: usize) -> Vec<SequenceStatus> {
        let (tx, rx): (Sender<EncodeProgress>, Receiver<EncodeProgress>) = channel();
        let (status_tx, status_rx) = std::sync::mpsc::channel();
        let total = Progress::new(status_tx, frames, 0);
        let attempt = AttemptProgress::new(&total, 7, frames, EncoderPasses::All(1));

        for frame in 1..=frames {
            tx.send(EncodeProgress {
                pass: (1, 1),
                frame,
                usage: Default::default(),
            })
            .expect("send progress");
        }
        drop(tx);
        relay_progress(rx, &attempt).expect("relay");
        status_rx.try_iter().collect()
    }

    /// Even final-frame progress cannot establish artifact eligibility.
    #[test]
    fn a_progress_relay_does_not_report_artifact_completion() {
        let frames = 3_usize;
        let statuses = relay_one_scene(frames);

        let scene_completed = statuses.iter().any(|status| {
            matches!(
                status,
                SequenceStatus::Subprocess {
                    child: crate::core::sequence::Status::Completed { id },
                    ..
                } if id == "7"
            )
        });
        assert!(
            !scene_completed,
            "final-frame progress must not complete an unvalidated scene"
        );

        let encode_completed = statuses.iter().any(|status| {
            matches!(
                status,
                SequenceStatus::Whole(crate::core::sequence::Status::Completed { .. })
            )
        });
        assert!(
            !encode_completed,
            "final-frame progress must not complete an unvalidated encode"
        );
    }

    fn check_attempt_credit(phase: &str) {
        assert!(["rollback", "commit", "zero"].contains(&phase));
        let (tx, rx) = channel();
        let total = Progress::new(tx, 60, 0);
        let mut peer = AttemptProgress::new(&total, 8, 30, EncoderPasses::All(2));
        let report = |attempt: &AttemptProgress<'_>, pass, frame| {
            attempt
                .report(EncodeProgress {
                    pass,
                    frame,
                    usage: Default::default(),
                })
                .expect("report");
        };
        {
            let failed = AttemptProgress::new(&total, 7, 30, EncoderPasses::All(2));
            report(&failed, (1, 2), 30);
            assert_eq!(*total.completed.lock().expect("total"), 0);
            report(&failed, (2, 2), 10);
            report(&peer, (2, 2), 10);
            report(&failed, (1, 2), 30);
            report(&failed, (2, 1), 30);
            report(&failed, (2, 2), usize::MAX);
            assert_eq!(*total.completed.lock().expect("total"), 40);
        }
        assert_eq!(
            *total.completed.lock().expect("total"),
            10,
            "only failed credit removed"
        );
        if phase == "rollback" {
            return;
        }
        let mut retry = AttemptProgress::new(&total, 7, 30, EncoderPasses::All(2));
        report(&retry, (2, 2), 1);
        assert_eq!(*total.completed.lock().expect("total"), 11);
        peer.commit(42);
        retry.commit(42);
        retry.commit(42);
        assert_eq!(*total.completed.lock().expect("total"), 60);
        if phase == "commit" {
            return;
        }
        let mut zero = AttemptProgress::new(&total, 9, 0, EncoderPasses::All(1));
        report(&zero, (1, 1), usize::MAX);
        zero.commit(1);
        assert_eq!(*total.completed.lock().expect("total"), 60);
        assert_eq!(
            rx.try_iter()
                .filter(|e| matches!(
                    e,
                    SequenceStatus::Subprocess {
                        child: crate::core::sequence::Status::Completed { .. },
                        ..
                    }
                ))
                .count(),
            3
        );
    }

    #[test]
    fn failed_attempt_rolls_back_only_its_own_final_pass_credit() {
        check_attempt_credit("rollback");
    }

    #[test]
    fn retry_commit_reconciles_missing_frames() {
        check_attempt_credit("commit");
    }

    #[test]
    fn zero_frame_attempt_commit_does_not_inflate_total_progress() {
        check_attempt_credit("zero");
    }

    /// Concurrent workers must not lose a count.
    #[test]
    fn concurrent_attempts_do_not_emit_stale_totals() {
        let workers = 8_usize;
        let per_worker = 500_usize;
        let total = workers * per_worker;
        let (tx, rx) = channel();
        let count = Progress::new(tx, total, 0);
        std::thread::scope(|scope| {
            let mut handles = Vec::with_capacity(workers);
            for scene in 0..workers {
                let count = &count;
                handles.push(scope.spawn(move || {
                    let mut attempt =
                        AttemptProgress::new(count, scene, per_worker, EncoderPasses::All(1));
                    for frame in 1..=per_worker {
                        attempt
                            .report(EncodeProgress {
                                pass: (1, 1),
                                frame,
                                usage: Default::default(),
                            })
                            .expect("report");
                    }
                    attempt.commit(42);
                }));
            }
            handles
                .into_iter()
                .for_each(|handle| handle.join().expect("worker should not panic"));
        });
        assert_eq!(*count.completed.lock().expect("total"), total);
        let totals: Vec<_> = rx
            .try_iter()
            .filter_map(|event| match event {
                SequenceStatus::Whole(crate::core::sequence::Status::Processing {
                    completion: crate::core::sequence::SequenceCompletion::Frames { completed, .. },
                    ..
                }) => Some(completed),
                _ => None,
            })
            .collect();
        assert!(totals.windows(2).all(|pair| pair[0] <= pair[1]));
        assert_eq!(totals.last(), Some(&(total as u64)));
    }
}
