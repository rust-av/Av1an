use std::io::IsTerminal;

use andean_condor::core::{
    input::clip_info::ClipInfo,
    sequence::{SequenceCompletion, SequenceStatus, Status},
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::Color,
    text::Line,
    widgets::Block,
};
use serde::{Deserialize, Serialize};

use crate::{
    apps::{SharedProgress, TuiApp},
    components::{input_info::InputInfo, progress_bar::ProgressBar},
};
#[derive(Clone)]
pub struct SceneDetectionState {
    pub frames_processed: u64,
    pub total_frames: u64,
    pub scenes: Vec<(u64, u64)>,
    pub scenes_len: usize,
}

pub struct SceneDetectionApp {
    pub started: std::time::Instant,
    pub clip_info: ClipInfo,
    initial_frames: u64,
    pub total_frames: u64,
    attempted_cancel: bool,
    shared_progress: SharedProgress<SceneDetectionState>,
    cached_state: SceneDetectionState,
}

impl TuiApp for SceneDetectionApp {
    type State = SceneDetectionState;

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
        "Waiting for Scene Detector to shut down. Press Ctrl+C again to exit immediately."
    }

    fn map_progress(status: SequenceStatus, state: &mut Self::State) -> bool {
        if let SequenceStatus::Whole(Status::Processing { completion, .. }) = status {
            match completion {
                SequenceCompletion::Frames { completed, .. } => {
                    state.frames_processed = completed;
                    return true;
                },
                SequenceCompletion::Custom {
                    name,
                    completed,
                    total,
                } if name == "new-scene" => {
                    state.scenes.push((completed as u64, total as u64));
                    state.scenes_len = state.scenes.len();
                    if !std::io::stdout().is_terminal() {
                        let event = SceneDetectionConsoleEvent::NewScene {
                            start: completed as u64,
                            end: total as u64,
                        };
                        let event = serde_json::to_string(&event).unwrap();
                        println!("[Scene Detector][New Scene]: {}", event);
                    }
                    return true;
                },
                _ => {},
            }
        }
        false
    }

    fn progress_snapshot(
        &self,
        elapsed: std::time::Duration,
    ) -> Option<super::progress::ProgressSnapshot> {
        Some(super::progress::ProgressSnapshot {
            stage: "Scene detection",
            completed: self.cached_state.frames_processed,
            total: Some(self.total_frames),
            initial: self.initial_frames,
            elapsed,
            detail: format!("{} scenes found", self.cached_state.scenes_len),
            ..Default::default()
        })
    }

    fn render(&self, frame: &mut Frame) {
        let theme = crate::theme::Theme::current();
        let main: Color = theme.main;
        let layout = Layout::default()
            .constraints([
                Constraint::Percentage(10),
                Constraint::Percentage(80),
                Constraint::Percentage(10),
            ])
            .split(frame.area());

        let input_info = InputInfo::new(self.clip_info);
        let input_info = input_info.generate(false);
        let input_block = Block::bordered()
            .border_type(ratatui::widgets::BorderType::Rounded)
            .title(Line::from("Input").centered());
        let input_info = input_info.block(input_block);
        frame.render_widget(input_info, layout[0]);

        let progress_bar = ProgressBar {
            color: main,
            processing_title: if self.attempted_cancel {
                "Shutting down Scene Detector...".to_owned()
            } else {
                "Detecting Scenes...".to_owned()
            },
            completed_title: "Scene Detection Completed".to_owned(),
            top_right_title: format!("{} found", self.cached_state.scenes_len),
            bottom_center_title: String::new(),
            unit_per_second: "FPS".to_owned(),
            unit: "Frame".to_owned(),
            initial_completed: self.initial_frames,
            completed: self.cached_state.frames_processed,
            total: self.total_frames,
            show_label: true,
        };
        let progress_bar = progress_bar.generate(Some(self.started));
        frame.render_widget(progress_bar, layout[2]);
    }
}

impl SceneDetectionApp {
    pub fn new(
        initial_frames: u64,
        total_frames: u64,
        scenes: Vec<(u64, u64)>,
        clip_info: ClipInfo,
    ) -> SceneDetectionApp {
        let scenes_len = scenes.len();
        let state = SceneDetectionState {
            frames_processed: initial_frames,
            total_frames,
            scenes,
            scenes_len,
        };
        SceneDetectionApp {
            started: std::time::Instant::now(),
            initial_frames,
            total_frames,
            clip_info,
            attempted_cancel: false,
            shared_progress: SharedProgress::new(state.clone()),
            cached_state: state,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum SceneDetectionConsoleEvent {
    ProcessedFrame { completed: u64, total: u64 },
    NewScene { start: u64, end: u64 },
}
