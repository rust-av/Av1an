use std::io::IsTerminal;

use andean_condor::{
    core::{
        input::clip_info::ClipInfo,
        sequence::{SequenceCompletion, SequenceStatus, Status},
    },
    models::sequence::scene_concatenator::ConcatMethod,
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
pub struct SceneConcatenatorState {
    pub percent: f64,
}

pub struct SceneConcatenatorApp {
    pub started: std::time::Instant,
    pub clip_info: ClipInfo,
    pub method: ConcatMethod,
    pub scenes_len: usize,
    attempted_cancel: bool,
    shared_progress: SharedProgress<SceneConcatenatorState>,
    cached_state: SceneConcatenatorState,
}

impl TuiApp for SceneConcatenatorApp {
    type State = SceneConcatenatorState;

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
        "Waiting for Concatenation to finish. Press Ctrl+C again to exit immediately."
    }

    fn map_progress(status: SequenceStatus, state: &mut Self::State) -> bool {
        if let SequenceStatus::Whole(Status::Processing { completion, .. }) = status
            && let SequenceCompletion::Percentage(percentage) = completion
        {
            state.percent = percentage;
            if !std::io::stdout().is_terminal() {
                let event = SceneConcatenatorConsoleEvent::Processed(percentage);
                let event = serde_json::to_string(&event).unwrap();
                println!("[Scene Concatenator][Progress]: {}", event);
            }
            return true;
        }
        false
    }

    fn progress_snapshot(
        &self,
        elapsed: std::time::Duration,
    ) -> Option<super::progress::ProgressSnapshot> {
        Some(super::progress::ProgressSnapshot {
            stage: "Concatenation",
            completed: self.cached_state.percent.clamp(0.0, 100.0) as u64,
            total: Some(100),
            unit: "percent",
            elapsed,
            measure_rate: false,
            detail: format!("{} scenes", self.scenes_len),
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
            .title(Line::from("Input").centered())
            .title_bottom(Line::from(self.method.to_string()).centered());
        let input_info = input_info.block(input_block);
        frame.render_widget(input_info, layout[0]);

        let progress_bar = ProgressBar {
            color: main,
            processing_title: if self.attempted_cancel {
                "Waiting for Concatenation to Finish...".to_owned()
            } else {
                "Concatenating Scenes...".to_owned()
            },
            completed_title: if self.attempted_cancel {
                "Concatenation Aborted".to_owned()
            } else {
                "Concatenation Completed".to_owned()
            },
            top_right_title: format!("{} scenes", self.scenes_len),
            bottom_center_title: String::new(),
            unit_per_second: "%".to_owned(),
            unit: "Percent".to_owned(),
            initial_completed: 0,
            completed: self.cached_state.percent.round() as u64,
            total: 100,
            show_label: false,
        };
        let progress_bar = progress_bar.generate(Some(self.started));
        frame.render_widget(progress_bar, layout[2]);
    }
}

impl SceneConcatenatorApp {
    pub fn new(
        clip_info: ClipInfo,
        scenes_len: usize,
        method: ConcatMethod,
    ) -> SceneConcatenatorApp {
        let state = SceneConcatenatorState { percent: 0.0 };
        SceneConcatenatorApp {
            started: std::time::Instant::now(),
            clip_info,
            method,
            scenes_len,
            attempted_cancel: false,
            shared_progress: SharedProgress::new(state.clone()),
            cached_state: state,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum SceneConcatenatorConsoleEvent {
    Processed(f64),
}
