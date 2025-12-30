use std::collections::BTreeMap;

use ratatui::{
    layout::{Constraint, Layout},
    style::{Color, Stylize},
    text::Line,
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::components::progress_bar::ProgressBar;

pub struct ParallelEncoderLayout {
    pub started:          std::time::Instant,
    pub resolution:       (u32, u32),
    pub framerate:        (u64, u64),
    pub bit_depth:        u32,
    pub hdr:              bool,
    pub total_frames:     u64,
    pub frames_processed: u64,
    pub scenes:           BTreeMap<u64, (u64, u64)>,
}

impl ParallelEncoderLayout {
    #[inline]
    pub fn draw(&self, frame: &mut Frame) {
        const MAIN_COLOR: Color = Color::DarkGray;
        let layout = Layout::default()
            .constraints([
                Constraint::Percentage(10),
                Constraint::Percentage(50),
                Constraint::Percentage(20),
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
        let column_constraints =
            Layout::vertical(std::iter::repeat_n(Constraint::Percentage(10), 10));

        let row_constraints = Layout::horizontal(std::iter::repeat_n(Constraint::Min(20), 10));

        let cells = layout[1]
            .layout_vec(&row_constraints)
            .iter()
            .flat_map(|row| row.layout_vec(&column_constraints))
            .collect::<Vec<_>>();

        for (index, cell) in cells.iter().enumerate() {
            if let Some((scene_completed, scene_total)) = self.scenes.get(&(index as u64)) {
                let (color, borders, border_style) = if *scene_completed == *scene_total {
                    (
                        MAIN_COLOR,
                        Borders::all(),
                        ratatui::style::Style::new().fg(MAIN_COLOR),
                    )
                } else {
                    (Color::Reset, Borders::empty(), ratatui::style::Style::new())
                };
                let block = Block::default().borders(borders).border_style(border_style).fg(color);
                frame.render_widget(block, *cell);
            }
        }

        let progress_bar = ProgressBar {
            color:            MAIN_COLOR,
            processing_title: "Encoding Scenes...".to_owned(),
            completed_title:  "Encoding Completed".to_owned(),
            unit_per_second:  "FPS".to_owned(),
            unit:             "Frame".to_owned(),
            completed:        self.frames_processed,
            total:            self.total_frames,
        };
        let progress_bar = progress_bar.generate(Some(self.started));
        frame.render_widget(progress_bar, layout[2]);
    }
}
