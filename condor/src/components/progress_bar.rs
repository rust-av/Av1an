use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Gauge},
};

use crate::utils::time_display::seconds_to_hms;

pub struct ProgressBar {
    pub color:            Color,
    pub processing_title: String,
    pub completed_title:  String,
    pub unit:             String,
    pub unit_per_second:  String,
    pub completed:        u64,
    pub total:            u64,
}

impl ProgressBar {
    #[inline]
    pub fn generate(&self, started: Option<std::time::Instant>) -> Gauge<'_> {
        let mut title = self.processing_title.clone();
        let progress_percent = (self.completed as f64 / self.total as f64) * 100.0;
        let (label, elapsed_hms, remaining_hms, _eta_hms) = started.map_or_else(
            || {
                (
                    format!("{}/{} {}s", self.completed, self.total, self.unit),
                    None,
                    None,
                    None,
                )
            },
            |started| {
                let elapsed = started.elapsed();
                let ups = self.completed as f64 / elapsed.as_secs_f64();
                let elapsed_per_unit = elapsed.as_secs_f64() / self.completed as f64;
                let remaining_time = (self.total - self.completed) as f64 * elapsed_per_unit;
                let eta = elapsed.as_secs_f64() + remaining_time;
                let elapsed_hms = seconds_to_hms(elapsed.as_secs(), false);
                let remaining_hms = seconds_to_hms(remaining_time as u64, false);
                let eta_hms = seconds_to_hms(eta as u64, false);
                (
                    format!(
                        "{}/{} {}s ({:.2}{})",
                        self.completed, self.total, self.unit, ups, self.unit_per_second
                    ),
                    Some(elapsed_hms),
                    Some(remaining_hms),
                    Some(eta_hms),
                )
            },
        );
        if self.completed == self.total {
            title = self.completed_title.clone();
        }
        Gauge::default()
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(Line::from(title).left_aligned())
                    .title(Line::from(format!("{:.2}%", progress_percent)).centered())
                    .title_bottom(Line::from(elapsed_hms.unwrap_or_default()).left_aligned())
                    .title_bottom(Line::from(remaining_hms.unwrap_or_default()).right_aligned()),
            )
            .ratio(self.completed as f64 / self.total as f64)
            .label(Span::styled(label, Style::default()))
            .gauge_style(Style::default().fg(self.color))
    }
}
