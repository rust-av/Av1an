use ratatui::{
    layout::{Constraint, Layout},
    style::{Color, Stylize},
    text::Line,
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::components::progress_bar::ProgressBar;

pub struct SceneDetectionLayout {
    pub started:          std::time::Instant,
    pub resolution:       (u32, u32),
    pub framerate:        (u64, u64),
    pub bit_depth:        u32,
    pub hdr:              bool,
    pub total_frames:     u64,
    pub frames_processed: u64,
    pub scenes:           Vec<(u64, u64)>,
}

impl SceneDetectionLayout {
    #[inline]
    pub fn draw(&self, frame: &mut Frame) {
        const MAIN_COLOR: Color = Color::DarkGray;
        let layout = Layout::default()
            .constraints([
                Constraint::Percentage(20),
                Constraint::Percentage(60),
                Constraint::Percentage(20),
            ])
            .split(frame.area());

        let input_info = format!(
            "{}x{} {:.3}FPS {}-bit {}",
            self.resolution.0,
            self.resolution.1,
            (self.framerate.0 as f64 / self.framerate.1 as f64),
            self.bit_depth,
            if self.hdr { "HDR" } else { "SDR" }
        );
        let input_paragraph = Paragraph::new(Line::from(input_info))
            .block(Block::bordered().title(Line::from("Input")))
            .centered();
        frame.render_widget(input_paragraph, layout[0]);

        let mut scene_constraints = self
            .scenes
            .iter()
            .map(|(start, end)| Constraint::Fill((*end - *start) as u16))
            .collect::<Vec<_>>();
        if let Some((_, end)) = self.scenes.last()
            && *end < self.total_frames
        {
            scene_constraints.push(Constraint::Fill((self.total_frames - *end) as u16));
        }
        let scene_chunks = Layout::horizontal(scene_constraints).split(layout[1]);
        for (index, area) in scene_chunks.iter().enumerate() {
            let (color, borders, border_style) = if index < self.scenes.len() {
                (
                    MAIN_COLOR,
                    Borders::all(),
                    ratatui::style::Style::new().fg(MAIN_COLOR),
                )
            } else {
                (Color::Reset, Borders::empty(), ratatui::style::Style::new())
            };
            let block = Block::default().borders(borders).border_style(border_style).fg(color);
            frame.render_widget(block, *area);
        }

        let progress_bar = ProgressBar {
            color:            MAIN_COLOR,
            processing_title: "Detecting Scenes...".to_owned(),
            completed_title:  "Scene Detection Completed".to_owned(),
            unit_per_second:  "FPS".to_owned(),
            unit:             "Frame".to_owned(),
            completed:        self.frames_processed,
            total:            self.total_frames,
        };
        let progress_bar = progress_bar.generate(Some(self.started));
        frame.render_widget(progress_bar, layout[2]);
    }
}
