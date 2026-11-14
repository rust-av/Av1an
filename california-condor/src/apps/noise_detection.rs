use std::{collections::BTreeMap, io::IsTerminal, sync::Arc};

use andean_condor::{
    core::{
        input::clip_info::ClipInfo,
        sequence::{SequenceCompletion, SequenceStatus, Status},
    },
    models::scene::Scene,
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    text::Line,
    widgets::{Axis, Block, Chart, Dataset},
};
use serde::{Deserialize, Serialize};

use crate::{
    apps::{SharedProgress, TuiApp},
    components::{input_info::InputInfo, progress_bar::ProgressBar},
    configuration::CliSequenceData,
};
#[derive(Clone)]
pub struct NoiseDetectionState {
    pub frames_processed: u64,
    pub total_frames: u64,
    pub scene_noise: Arc<BTreeMap<u64, f64>>,
}

pub struct NoiseDetectionApp {
    pub started: std::time::Instant,
    pub total_frames: u64,
    pub clip_info: ClipInfo,
    attempted_cancel: bool,
    shared_progress: SharedProgress<NoiseDetectionState>,
    cached_state: NoiseDetectionState,
    initial_completed: u64,
}

impl TuiApp for NoiseDetectionApp {
    type State = NoiseDetectionState;

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
        "Waiting for Noise Detector to shut down. Press Ctrl+C again to exit immediately."
    }

    fn map_progress(status: SequenceStatus, state: &mut Self::State) -> bool {
        match status {
            SequenceStatus::Whole(Status::Processing {
                completion: SequenceCompletion::Frames { completed, .. },
                ..
            }) => {
                state.frames_processed = completed;
                if !std::io::stdout().is_terminal() {
                    let event = NoiseDetectionConsoleEvent::ProcessedFrame {
                        completed,
                        total: state.total_frames,
                    };
                    let event = serde_json::to_string(&event).unwrap();
                    println!("[Noise Detector][Progress]: {event}");
                }
                true
            },
            SequenceStatus::Subprocess { parent, child } => match (parent, child) {
                (
                    Status::Processing {
                        completion:
                            SequenceCompletion::Custom {
                                completed, total, ..
                            },
                        ..
                    },
                    Status::Processing {
                        id,
                        completion: SequenceCompletion::SceneQuality { index, score, .. },
                    },
                ) if id == "Noise" => {
                    state.frames_processed = completed as u64;
                    state.total_frames = total as u64;
                    Arc::make_mut(&mut state.scene_noise).insert(index, score);
                    if !std::io::stdout().is_terminal() {
                        let event = NoiseDetectionConsoleEvent::SceneNoise {
                            scene: index,
                            noise: score,
                        };
                        let event = serde_json::to_string(&event).unwrap();
                        println!("[Noise Detector][Scene]: {event}");
                    }
                    true
                },
                _ => false,
            },
            _ => false,
        }
    }

    fn render(&self, frame: &mut Frame) {
        let theme = crate::theme::Theme::current();
        let layout = Layout::default()
            .constraints([
                Constraint::Percentage(20),
                Constraint::Percentage(70),
                Constraint::Percentage(10),
            ])
            .split(frame.area());

        let top_info = Block::bordered()
            .border_type(ratatui::widgets::BorderType::Rounded)
            .title(Line::from("Input").centered())
            .title_bottom(Line::from("Noise Detection").centered());
        let top_info_inner = top_info.inner(layout[0]);
        let top_info_areas =
            Layout::vertical([Constraint::Fill(1), Constraint::Fill(1)]).split(top_info_inner);
        frame.render_widget(top_info, layout[0]);
        let input_info = InputInfo::new(self.clip_info);
        let input_info = input_info.generate(false);
        frame.render_widget(input_info, top_info_areas[0]);

        let state = &self.cached_state;
        let scene_noise = state
            .scene_noise
            .iter()
            .map(|(index, noise)| (*index as f64, *noise))
            .collect::<Vec<_>>();
        let datasets = vec![
            Dataset::default()
                .name("Noise Level")
                .style(theme.accent_green)
                .graph_type(ratatui::widgets::GraphType::Scatter)
                .data(&scene_noise),
        ];
        let scene_count = state.scene_noise.len().max(state.total_frames as usize);
        let max_scenes_label = format!("{}", scene_count.saturating_sub(1));
        let max_noise = scene_noise.iter().map(|(_, n)| *n).fold(0.0_f64, f64::max);
        let max_noise = if max_noise <= 0.0 {
            1.0
        } else {
            (max_noise * 1000.0).ceil() / 1000.0
        };
        let max_noise_label = format!("{max_noise:.3}");
        let chart = Chart::new(datasets)
            .block(
                Block::bordered()
                    .border_type(ratatui::widgets::BorderType::Rounded)
                    .title(Line::from("Noise Level per Scene").centered()),
            )
            .x_axis(
                Axis::default()
                    .title("Scene")
                    .bounds([0.0, scene_count as f64])
                    .labels(["0", &max_scenes_label]),
            )
            .y_axis(
                Axis::default()
                    .title("Noise")
                    .bounds([0.0, max_noise])
                    .labels(["0", &max_noise_label]),
            );
        frame.render_widget(chart, layout[1]);

        let progress_bar = self.progress_bar();
        let progress_bar = progress_bar.generate(Some(self.started));
        frame.render_widget(progress_bar, layout[2]);
    }
}

impl NoiseDetectionApp {
    fn progress_bar(&self) -> ProgressBar {
        let state = &self.cached_state;
        ProgressBar {
            color: crate::theme::Theme::current().main,
            processing_title: if self.attempted_cancel {
                "Shutting down...".to_owned()
            } else {
                "Detecting Noisy Scenes...".to_owned()
            },
            completed_title: if self.attempted_cancel {
                "Noise Detection Aborted".to_owned()
            } else {
                "Noise Detection Completed".to_owned()
            },
            top_right_title: format!("{} found", state.scene_noise.len()),
            bottom_center_title: String::new(),
            unit_per_second: "SPS".to_owned(),
            unit: "Scene".to_owned(),
            initial_completed: self.initial_completed,
            completed: self.cached_state.frames_processed,
            total: self.total_frames,
            show_label: true,
        }
    }

    pub fn new(scenes: &[Scene<CliSequenceData>], clip_info: ClipInfo) -> NoiseDetectionApp {
        let total_frames = scenes.len() as u64;
        let scene_noise: BTreeMap<u64, f64> = scenes
            .iter()
            .enumerate()
            .filter_map(|(index, scene)| {
                scene
                    .sequence_data
                    .noise_detection
                    .as_ref()
                    .map(|detection| (index as u64, detection.noise))
            })
            .collect();
        let frames_processed = scene_noise.len() as u64;
        let state = NoiseDetectionState {
            frames_processed,
            total_frames,
            scene_noise: Arc::new(scene_noise),
        };
        NoiseDetectionApp {
            started: std::time::Instant::now(),
            total_frames,
            clip_info,
            attempted_cancel: false,
            shared_progress: SharedProgress::new(state.clone()),
            cached_state: state,
            initial_completed: frames_processed,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum NoiseDetectionConsoleEvent {
    ProcessedFrame { completed: u64, total: u64 },
    SceneNoise { scene: u64, noise: f64 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_restored_scenes_are_not_new_work() {
        let (clip, scenes) = crate::test_helpers::quality_scene_fixture();
        let scenes: Vec<_> = (0..100)
            .map(|index| {
                let mut scene = scenes[0].clone();
                scene.sequence_data.noise_detection = (index < 90).then_some(
                    andean_condor::models::sequence::noise_detector::NoiseDetectorData {
                        noise: 0.125,
                        luminance: 0.5,
                        created_on: std::time::UNIX_EPOCH,
                    },
                );
                scene
            })
            .collect();
        let mut app = NoiseDetectionApp::new(&scenes, clip);
        app.started = std::time::Instant::now() - std::time::Duration::from_secs(2);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("90/100 Scenes (0.00SPS)"), "{text}");
        let mut mapper = app.progress_mapper();
        app.shared_progress.apply(|state| {
            mapper(
                SequenceStatus::Subprocess {
                    parent: Status::Processing {
                        id: "Noise Detector".into(),
                        completion: SequenceCompletion::Custom {
                            name: "Noise Detector".into(),
                            completed: 91.0,
                            total: 100.0,
                        },
                    },
                    child: Status::Processing {
                        id: "Noise".into(),
                        completion: SequenceCompletion::SceneQuality {
                            index: 90,
                            score: 0.25,
                            quantizer: 0.0,
                            bitrate: 0.0,
                        },
                    },
                },
                state,
            )
        });
        app.on_snapshot(app.shared_progress.read());
        assert_eq!(
            (
                app.cached_state.frames_processed,
                app.cached_state.scene_noise.len()
            ),
            (91, 91)
        );
        let text = crate::components::progress_bar::tests::render_bar(
            &app.progress_bar(),
            std::time::Duration::from_secs(2),
        );
        assert!(
            text.contains("91/100 Scenes (0.50SPS)") && text.contains("00:18"),
            "{text}"
        );

        let mut fresh = NoiseDetectionApp::new(&scenes[90..], clip);
        fresh.shared_progress.apply(|state| {
            NoiseDetectionApp::map_progress(
                SequenceStatus::Whole(Status::Processing {
                    id: "Noise Detector".into(),
                    completion: SequenceCompletion::Frames {
                        completed: 1,
                        total: 10,
                    },
                }),
                state,
            )
        });
        fresh.on_snapshot(fresh.shared_progress.read());
        let text = crate::components::progress_bar::tests::render_bar(
            &fresh.progress_bar(),
            std::time::Duration::from_millis(500),
        );
        assert!(
            text.contains("1/10 Scenes (2.00SPS)") && text.contains("00:04"),
            "{text}"
        );
    }

    #[test]
    fn noise_frame_snapshots_share_exact_chart_points() {
        let (clip, scenes) = crate::test_helpers::quality_scene_fixture();
        let scenes: Vec<_> = (0..1024)
            .map(|index| {
                let mut scene = scenes[0].clone();
                scene.sequence_data.noise_detection = Some(
                    andean_condor::models::sequence::noise_detector::NoiseDetectorData {
                        noise: index as f64 / 1024.0,
                        luminance: 0.5,
                        created_on: std::time::UNIX_EPOCH,
                    },
                );
                scene
            })
            .collect();
        let mut app = NoiseDetectionApp::new(&scenes, clip);
        let initial = app.shared_progress.read();
        let point = initial.scene_noise.get(&0).unwrap() as *const f64;
        for completed in 1..=8 {
            app.shared_progress.apply(|state| {
                NoiseDetectionApp::map_progress(
                    SequenceStatus::Whole(Status::Processing {
                        id: "Noise".into(),
                        completion: SequenceCompletion::Frames {
                            completed,
                            total: 1024,
                        },
                    }),
                    state,
                )
            });
            let snapshot = app.shared_progress.read_if_dirty().unwrap();
            assert_eq!(snapshot.scene_noise.len(), 1024);
            assert_eq!(snapshot.scene_noise[&1023], 1023.0 / 1024.0);
            assert_eq!(
                snapshot.scene_noise.get(&0).unwrap() as *const f64,
                point,
                "count updates must not copy the chart"
            );
            assert_eq!(snapshot.frames_processed, completed);
        }
        for (index, score) in [(1024, 0.125), (10, 0.375)] {
            app.shared_progress.apply(|state| {
                NoiseDetectionApp::map_progress(
                    SequenceStatus::Subprocess {
                        parent: Status::Processing {
                            id: "Noise".into(),
                            completion: SequenceCompletion::Custom {
                                name: "Noise".into(),
                                completed: 1024.0,
                                total: 1025.0,
                            },
                        },
                        child: Status::Processing {
                            id: "Noise".into(),
                            completion: SequenceCompletion::SceneQuality {
                                index,
                                score,
                                quantizer: 0.0,
                                bitrate: 0.0,
                            },
                        },
                    },
                    state,
                )
            });
            let snapshot = app.shared_progress.read_if_dirty().unwrap();
            assert_eq!(snapshot.scene_noise.len(), 1025);
            assert_eq!(snapshot.scene_noise[&index], score);
            assert_eq!(snapshot.scene_noise[&1023], 1023.0 / 1024.0);
            assert_eq!(initial.scene_noise[&10], 10.0 / 1024.0);
            app.on_snapshot(snapshot);
        }
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("Noise Level per Scene") && text.contains("1025 found"));
    }
}
