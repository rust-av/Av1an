use std::{collections::BTreeMap, io::IsTerminal};

use andean_condor::{
    core::{
        encoder::EncodeResourceUsage,
        input::clip_info::ClipInfo,
        sequence::{
            SequenceCompletion, SequenceDetails, SequenceStatus, Status, benchmarker::Benchmarker,
        },
    },
    models::encoder::Encoder,
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    text::Line,
    widgets::{Block, Cell, Row, Table},
};
use serde::{Deserialize, Serialize};

use crate::{
    apps::{SharedProgress, TuiApp},
    components::{encoder_info::EncoderInfo, input_info::InputInfo},
};
#[derive(Clone)]
pub struct BenchmarkerState {
    pub results: BTreeMap<u8, WorkerStatus>,
}

pub struct BenchmarkerApp {
    pub encoder: Encoder,
    pub clip_info: ClipInfo,
    attempted_cancel: bool,
    shared_progress: SharedProgress<BenchmarkerState>,
    cached_state: BenchmarkerState,
}

impl TuiApp for BenchmarkerApp {
    type State = BenchmarkerState;

    fn shared_progress(&self) -> &SharedProgress<Self::State> {
        &self.shared_progress
    }

    fn cached_state(&self) -> &Self::State {
        &self.cached_state
    }

    fn cached_state_mut(&mut self) -> &mut Self::State {
        &mut self.cached_state
    }

    fn attempted_cancel(&self) -> bool {
        self.attempted_cancel
    }

    fn attempted_cancel_mut(&mut self) -> &mut bool {
        &mut self.attempted_cancel
    }

    fn cancel_message(&self) -> &'static str {
        "Waiting for Benchmarker to finish. Press Ctrl+C again to exit immediately."
    }

    fn map_progress(status: SequenceStatus, state: &mut Self::State) -> bool {
        const DETAILS: SequenceDetails = Benchmarker::DETAILS;
        match status {
            SequenceStatus::Whole(status) => match status {
                Status::Completed { id } => {
                    if id == DETAILS.name {
                        // Whole benchmark completed; the loop ends via Quit.
                        false
                    } else {
                        let workers = id.parse::<u8>().expect("Workers is a number");
                        state.results.entry(workers).and_modify(|ws| ws.added = true);
                        if !std::io::stdout().is_terminal() {
                            let event = BenchmarkerConsoleEvent::WorkerAdded { worker: workers };
                            println!(
                                "[Benchmarker][Worker Added] {}",
                                serde_json::to_string(&event).unwrap()
                            );
                        }
                        true
                    }
                },
                Status::Failed { id, error } => {
                    let workers = id.parse::<u8>().expect("Workers is a number");
                    state
                        .results
                        .entry(workers)
                        .and_modify(|ws| ws.failed_reason = Some(error.clone()));
                    if !std::io::stdout().is_terminal() {
                        let event = BenchmarkerConsoleEvent::WorkerFailed {
                            worker: workers,
                            error,
                        };
                        println!(
                            "[Benchmarker][Worker Failed] {}",
                            serde_json::to_string(&event).unwrap()
                        );
                    }
                    true
                },
                Status::Processing {
                    id,
                    completion: SequenceCompletion::ResourceUsage { cpu, memory },
                } => {
                    let workers = id.parse::<u8>().expect("Workers is a number");
                    let usage = EncodeResourceUsage { cpu, memory };
                    state
                        .results
                        .entry(workers)
                        .and_modify(|ws| ws.usage = usage.is_available().then_some(usage));
                    true
                },
                _ => false,
            },
            SequenceStatus::Subprocess { parent: _, child } => match child {
                Status::Processing {
                    id,
                    completion: SequenceCompletion::Frames { completed, total },
                } => {
                    let workers = id.parse::<u8>().expect("Workers is a number");
                    state
                        .results
                        .entry(workers)
                        .and_modify(|ws| {
                            ws.current_frame = completed;
                            ws.total_frames = total;
                        })
                        .or_insert_with(|| WorkerStatus {
                            started: std::time::Instant::now(),
                            added: false,
                            finished: None,
                            failed_reason: None,
                            usage: None,
                            current_frame: completed,
                            initial_frame: completed,
                            total_frames: total,
                        });
                    if !std::io::stdout().is_terminal() {
                        let event = BenchmarkerConsoleEvent::Progress {
                            worker: workers,
                            current_frame: completed,
                            total_frames: total,
                        };
                        println!(
                            "[Benchmarker][Progress] {}",
                            serde_json::to_string(&event).unwrap()
                        );
                    }
                    true
                },
                Status::Completed { id } => {
                    let workers = id.parse::<u8>().expect("Workers is a number");
                    state
                        .results
                        .entry(workers)
                        .and_modify(|ws| ws.finished = Some(std::time::Instant::now()));
                    if !std::io::stdout().is_terminal() {
                        let event = BenchmarkerConsoleEvent::WorkerCompleted { worker: workers };
                        println!(
                            "[Benchmarker][Worker Completed] {}",
                            serde_json::to_string(&event).unwrap()
                        );
                    }
                    true
                },
                _ => false,
            },
        }
    }

    fn progress_snapshot(
        &self,
        elapsed: std::time::Duration,
    ) -> Option<super::progress::ProgressSnapshot> {
        let current = self.cached_state.results.iter().next_back();
        Some(if let Some((workers, trial)) = current {
            super::progress::ProgressSnapshot {
                stage: "Benchmarker",
                phase: format!("{workers} workers"),
                completed: trial.current_frame,
                total: Some(trial.total_frames),
                initial: trial.initial_frame,
                elapsed: trial
                    .finished
                    .unwrap_or_else(std::time::Instant::now)
                    .saturating_duration_since(trial.started),
                ..Default::default()
            }
        } else {
            super::progress::ProgressSnapshot {
                stage: "Benchmarker",
                elapsed,
                ..Default::default()
            }
        })
    }

    fn render(&self, frame: &mut Frame) {
        let layout = Layout::default()
            .constraints([Constraint::Percentage(20), Constraint::Percentage(80)])
            .split(frame.area());

        let top_info = Block::bordered()
            .border_type(ratatui::widgets::BorderType::Rounded)
            .title(Line::from("Input").centered())
            .title_bottom(Line::from(self.encoder.base().friendly_name()).centered());
        let top_info_inner = top_info.inner(layout[0]);
        let top_info_areas =
            Layout::vertical([Constraint::Fill(1), Constraint::Fill(1)]).split(top_info_inner);
        frame.render_widget(top_info, layout[0]);
        let input_info = InputInfo::new(self.clip_info);
        let input_info = input_info.generate(false);
        frame.render_widget(input_info, top_info_areas[0]);
        let encoder_info = EncoderInfo::new(self.encoder.clone(), None);
        let encoder_info = encoder_info.generate(false);
        frame.render_widget(encoder_info, top_info_areas[1]);

        let rows = self
            .cached_state
            .results
            .iter()
            .map(|(worker, status)| {
                Row::new(vec![
                    Cell::from(Line::from(worker.to_string()).centered()),
                    Cell::from(Line::from(status.fps_or_error()).centered()),
                    Cell::from(Line::from(status.status()).centered()),
                    Cell::from(Line::from(status.usage_desc()).centered()),
                    Cell::from(
                        Line::from(format!("{}/{}", status.current_frame, status.total_frames))
                            .centered(),
                    ),
                ])
            })
            .collect::<Vec<_>>();
        let table = Table::new(
            rows,
            [
                Constraint::Fill(1),
                Constraint::Fill(1),
                Constraint::Fill(1),
                Constraint::Fill(1),
                Constraint::Fill(1),
            ],
        )
        .header(Row::new(vec![
            Cell::from(Line::from("Workers").centered()),
            Cell::from(Line::from("FPS").centered()),
            Cell::from(Line::from("Meets Threshold").centered()),
            Cell::from(Line::from("Peak Usage").centered()),
            Cell::from(Line::from("Frames").centered()),
        ]))
        .block(Block::bordered().title_bottom(Line::from("Benchmarking...").centered()));
        frame.render_widget(table, layout[1]);
    }
}

impl BenchmarkerApp {
    pub fn new(encoder: Encoder, clip_info: ClipInfo) -> BenchmarkerApp {
        let state = BenchmarkerState {
            results: BTreeMap::new(),
        };
        BenchmarkerApp {
            encoder,
            clip_info,
            attempted_cancel: false,
            shared_progress: SharedProgress::new(state.clone()),
            cached_state: state,
        }
    }
}

#[derive(Debug, Clone)]
pub struct WorkerStatus {
    pub started: std::time::Instant,
    pub finished: Option<std::time::Instant>,
    pub added: bool,
    pub failed_reason: Option<String>,
    /// Peaks reported by the trial, or `None` when nothing could be sampled.
    pub usage: Option<EncodeResourceUsage>,
    pub current_frame: u64,
    /// First observed progress, not work timed by this UI clock.
    pub initial_frame: u64,
    pub total_frames: u64,
}

impl WorkerStatus {
    fn fps_at(&self, now: std::time::Instant) -> Option<f64> {
        let frames = self.current_frame.saturating_sub(self.initial_frame);
        let elapsed = self
            .finished
            .unwrap_or(now)
            .saturating_duration_since(self.started)
            .as_secs_f64();
        if frames == 0 || elapsed <= 0.0 {
            return None;
        }
        let fps = frames as f64 / elapsed;
        fps.is_finite().then_some(fps)
    }

    fn fps_or_error(&self) -> String {
        self.fps_at(std::time::Instant::now())
            .map_or_else(|| "-".to_owned(), |fps| format!("{fps:.2} FPS"))
    }

    fn usage_desc(&self) -> String {
        self.usage
            .and_then(crate::components::usage::describe)
            .unwrap_or_else(|| "-".to_owned())
    }

    fn status(&self) -> String {
        if self.failed_reason.is_some() {
            "✕".to_owned()
        } else if self.added {
            "✓".to_owned()
        } else {
            "-".to_owned()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BenchmarkerConsoleEvent {
    Progress {
        worker: u8,
        current_frame: u64,
        total_frames: u64,
    },
    WorkerCompleted {
        worker: u8,
    },
    WorkerAdded {
        worker: u8,
    },
    WorkerFailed {
        worker: u8,
        error: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1024 * 1024;

    fn usage_status(worker: u8, cpu: f32, memory: u64) -> SequenceStatus {
        SequenceStatus::Whole(Status::Processing {
            id: worker.to_string(),
            completion: SequenceCompletion::ResourceUsage { cpu, memory },
        })
    }

    fn screen(app: &BenchmarkerApp) -> String {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 20)).expect("terminal");
        terminal.draw(|frame| app.render(frame)).expect("render");
        let buffer = terminal.backend().buffer();
        (0..20)
            .map(|y| (0..120).map(|x| buffer[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// A finished trial keeps its measured peaks; a trial whose processes were
    /// never sampled shows `-` instead of claiming zero usage.
    #[test]
    fn each_trial_row_shows_its_peak_usage_and_hides_unavailable_samples() {
        let (clip, scenes) = crate::test_helpers::quality_scene_fixture();
        let mut app = BenchmarkerApp::new(scenes[0].encoder.clone(), clip);
        let mut mapper = app.progress_mapper();
        for id in ["2", "3"] {
            app.shared_progress.apply(|state| {
                mapper(
                    SequenceStatus::Subprocess {
                        parent: Status::Processing {
                            id: "Benchmarker".into(),
                            completion: SequenceCompletion::Passes {
                                completed: 2,
                                total: 2,
                            },
                        },
                        child: Status::Processing {
                            id: id.into(),
                            completion: SequenceCompletion::Frames {
                                completed: 90,
                                total: 100,
                            },
                        },
                    },
                    state,
                )
            });
        }
        app.shared_progress
            .apply(|state| mapper(usage_status(2, 812.0, 3 * 1024 * MIB), state));
        app.shared_progress.apply(|state| mapper(usage_status(3, 0.0, 0), state));
        for id in ["2", "3"] {
            app.shared_progress.apply(|state| {
                mapper(
                    SequenceStatus::Whole(Status::Completed { id: id.into() }),
                    state,
                )
            });
        }
        app.on_snapshot(app.shared_progress.read());

        assert_eq!(
            app.cached_state.results[&2].usage_desc(),
            "CPU 812% · 3.0 GiB"
        );
        assert_eq!(
            app.cached_state.results[&3].usage_desc(),
            "-",
            "unsampled usage is unavailable, not zero"
        );
        let text = screen(&app);
        assert!(text.contains("CPU 812%"), "peak cpu missing: {text}");
        assert!(text.contains("3.0 GiB"), "peak memory missing: {text}");
        assert!(
            !text.contains("CPU 0%") && !text.contains("0 MiB"),
            "an unsampled trial must not claim zero usage: {text}"
        );
    }

    #[test]
    fn benchmark_first_observed_frame_is_rate_baseline_and_finished_rate_stays_fixed() {
        let (clip, scenes) = crate::test_helpers::quality_scene_fixture();
        let mut app = BenchmarkerApp::new(scenes[0].encoder.clone(), clip);
        let mut mapper = app.progress_mapper();
        app.shared_progress.apply(|state| {
            mapper(
                SequenceStatus::Subprocess {
                    parent: Status::Processing {
                        id: "Benchmarker".into(),
                        completion: SequenceCompletion::Passes {
                            completed: 2,
                            total: 2,
                        },
                    },
                    child: Status::Processing {
                        id: "2".into(),
                        completion: SequenceCompletion::Frames {
                            completed: 90,
                            total: 100,
                        },
                    },
                },
                state,
            )
        });
        app.on_snapshot(app.shared_progress.read());
        let status = &app.cached_state.results[&2];
        assert_eq!((status.current_frame, status.total_frames), (90, 100));
        assert_eq!(status.fps_or_error(), "-");
        let observed = status.started;
        assert_eq!(status.fps_at(observed), None);
        app.shared_progress.apply(|state| {
            mapper(
                SequenceStatus::Subprocess {
                    parent: Status::Processing {
                        id: "Benchmarker".into(),
                        completion: SequenceCompletion::Passes {
                            completed: 2,
                            total: 2,
                        },
                    },
                    child: Status::Processing {
                        id: "2".into(),
                        completion: SequenceCompletion::Frames {
                            completed: 91,
                            total: 100,
                        },
                    },
                },
                state,
            )
        });
        app.on_snapshot(app.shared_progress.read());
        assert_eq!(
            app.cached_state.results[&2].fps_at(observed + std::time::Duration::from_millis(500)),
            Some(2.0)
        );
        app.shared_progress.apply(|state| {
            mapper(
                SequenceStatus::Subprocess {
                    parent: Status::Processing {
                        id: "Benchmarker".into(),
                        completion: SequenceCompletion::Passes {
                            completed: 2,
                            total: 2,
                        },
                    },
                    child: Status::Completed { id: "2".into() },
                },
                state,
            )
        });
        for status in [
            Status::Completed { id: "2".into() },
            Status::Failed {
                id: "2".into(),
                error: "Threshold not met".into(),
            },
        ] {
            app.shared_progress.apply(|state| mapper(SequenceStatus::Whole(status), state));
        }
        app.on_snapshot(app.shared_progress.read());
        let status = app.cached_state.results.get_mut(&2).unwrap();
        let finished = status.finished.expect("completion mapped");
        status.started = finished - std::time::Duration::from_millis(500);
        assert_eq!(
            status.fps_at(finished + std::time::Duration::from_secs(30)),
            Some(2.0)
        );
        assert_eq!(status.fps_or_error(), "2.00 FPS");
        assert!(status.added);
        assert_eq!(status.failed_reason.as_deref(), Some("Threshold not met"));
        assert_eq!(status.status(), "✕");
        assert_eq!((status.current_frame, status.total_frames), (91, 100));
        for width in [4, 12, 120] {
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, 20)).unwrap();
            terminal.draw(|frame| app.render(frame)).unwrap();
            let text: String =
                terminal.backend().buffer().content().iter().map(|cell| cell.symbol()).collect();
            assert!(!text.contains("NaN") && !text.contains("inf"), "{text}");
            if width == 120 {
                assert!(
                    text.contains("2.00 FPS") && text.contains("91/100") && text.contains('✕'),
                    "{text}"
                );
            }
        }
    }

    #[test]
    fn benchmark_zero_future_reversed_and_short_intervals_are_finite() {
        let now = std::time::Instant::now();
        let mut status = WorkerStatus {
            started: now,
            finished: Some(now),
            added: false,
            failed_reason: None,
            usage: None,
            current_frame: 1,
            initial_frame: 0,
            total_frames: 0,
        };
        assert_eq!(status.fps_or_error(), "-");
        status.started = now + std::time::Duration::from_secs(1);
        assert_eq!(status.fps_or_error(), "-");
        status.started = now;
        status.finished = None;
        assert_eq!(
            status.fps_at(now + std::time::Duration::from_millis(500)),
            Some(2.0)
        );
        status.initial_frame = 2;
        assert_eq!(status.fps_at(now + std::time::Duration::from_secs(1)), None);
        status.initial_frame = 0;
        status.current_frame = u64::MAX;
        status.finished = Some(now + std::time::Duration::from_nanos(1));
        assert!(status.fps_at(now).unwrap().is_finite());
        assert!(!status.fps_or_error().contains("inf"));
    }
}
