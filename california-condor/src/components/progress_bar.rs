use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Gauge},
};

use crate::utils::time_display::seconds_to_hms;

pub struct ProgressBar {
    pub color: Color,
    pub processing_title: String,
    pub completed_title: String,
    pub top_right_title: String,
    pub bottom_center_title: String,
    pub unit: String,
    pub unit_per_second: String,
    pub initial_completed: u64,
    pub completed: u64,
    pub total: u64,
    /// Whether to render the inner label (e.g. `0/100 Frames (0.00FPS)`).
    /// When `false`, only the percentage in the title is shown.
    pub show_label: bool,
}

impl Default for ProgressBar {
    fn default() -> Self {
        Self {
            color: crate::theme::Theme::current().main,
            processing_title: String::new(),
            completed_title: String::new(),
            top_right_title: String::new(),
            bottom_center_title: String::new(),
            unit: String::new(),
            unit_per_second: String::new(),
            initial_completed: 0,
            completed: 0,
            total: 0,
            show_label: true,
        }
    }
}

impl ProgressBar {
    /// Rates measure this run's work, excluding the owner's restored baseline.
    fn rates(&self, elapsed: f64) -> (f64, f64) {
        let done = self.completed.saturating_sub(self.initial_completed);
        if done == 0 || !elapsed.is_finite() || elapsed <= 0.0 {
            return (0.0, 0.0);
        }
        let rate = done as f64 / elapsed;
        if !rate.is_finite() {
            return (0.0, 0.0);
        }
        let remaining = self.total.saturating_sub(self.completed) as f64 / rate;
        (
            rate,
            if remaining.is_finite() {
                remaining
            } else {
                0.0
            },
        )
    }

    /// SAFETY: `Gauge::ratio()` panics if the value is not between 0.0 and
    /// 1.0 (inclusive) or is NaN.  We clamp to [0.0, 1.0] to prevent crashes
    /// when `total == 0` (division by zero → inf/NaN).
    #[inline]
    fn clamped_ratio(completed: u64, total: u64) -> f64 {
        if total == 0 {
            0.0
        } else {
            (completed as f64 / total as f64).clamp(0.0, 1.0)
        }
    }

    #[inline]
    pub fn generate(&self, started: Option<std::time::Instant>) -> Gauge<'_> {
        self.generate_with_elapsed(started.map(|started| started.elapsed()))
    }

    /// Render from a monotonic elapsed interval; no usable interval means zero rate.
    pub(crate) fn generate_with_elapsed(&self, elapsed: Option<std::time::Duration>) -> Gauge<'_> {
        let mut title = self.processing_title.clone();
        let ratio = Self::clamped_ratio(self.completed, self.total);
        let progress_percent = ratio * 100.0;
        let (label, elapsed_hms, remaining_hms) = elapsed.map_or_else(
            || {
                (
                    format!("{}/{} {}s", self.completed, self.total, self.unit),
                    None,
                    None,
                )
            },
            |elapsed| {
                let (ups, remaining_time) = self.rates(elapsed.as_secs_f64());
                let elapsed_hms = seconds_to_hms(elapsed.as_secs(), false);
                let remaining_hms = seconds_to_hms(remaining_time as u64, false);
                (
                    format!(
                        "{}/{} {}s ({:.2}{})",
                        self.completed, self.total, self.unit, ups, self.unit_per_second
                    ),
                    Some(elapsed_hms),
                    Some(remaining_hms),
                )
            },
        );
        if self.completed == self.total {
            title = self.completed_title.clone();
        }
        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title(Line::from(title).left_aligned())
            .title(Line::from(self.top_right_title.clone()).right_aligned())
            .title_bottom(Line::from(elapsed_hms.unwrap_or_default()).left_aligned())
            .title_bottom(Line::from(self.bottom_center_title.clone()).centered())
            .title_bottom(Line::from(remaining_hms.unwrap_or_default()).right_aligned());
        if self.show_label {
            block = block.title(Line::from(format!("{:.2}%", progress_percent)).centered());
        }
        let mut gauge = Gauge::default()
            .block(block)
            .ratio(ratio)
            .gauge_style(Style::default().fg(self.color));
        if self.show_label {
            gauge = gauge.label(Span::styled(label, Style::default()));
        }
        gauge
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::ProgressBar;
    use std::time::{Duration, Instant};

    pub(crate) fn render_bar(bar: &ProgressBar, elapsed: Duration) -> String {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 3)).unwrap();
        terminal
            .draw(|frame| {
                frame.render_widget(bar.generate_with_elapsed(Some(elapsed)), frame.area());
            })
            .unwrap();
        terminal.backend().buffer().content().iter().map(|cell| cell.symbol()).collect()
    }

    #[test]
    fn progress_rates_reject_nonfinite_intervals_and_overflow() {
        let bar = ProgressBar {
            completed: 1,
            total: u64::MAX,
            ..Default::default()
        };
        for elapsed in [
            0.0,
            -1.0,
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::from_bits(1),
            f64::MAX,
        ] {
            let (rate, remaining) = bar.rates(elapsed);
            assert!(
                rate.is_finite() && remaining.is_finite(),
                "{elapsed}: {rate}, {remaining}"
            );
        }
    }

    #[test]
    fn progress_empty_overcomplete_reversed_and_narrow_unicode_are_finite() {
        for (initial, completed, total, ratio) in
            [(0, 0, 0, 0.0), (0, 101, 100, 1.0), (100, 90, 100, 0.9), (0, u64::MAX, 1, 1.0)]
        {
            let bar = ProgressBar {
                processing_title: "比較中 🐦".into(),
                completed_title: "完了 ✓".into(),
                top_right_title: "é界".into(),
                unit: "Frame".into(),
                unit_per_second: "FPS".into(),
                initial_completed: initial,
                completed,
                total,
                ..Default::default()
            };
            assert_eq!(ProgressBar::clamped_ratio(completed, total), ratio);
            for elapsed in [Duration::ZERO, Duration::from_nanos(1), Duration::MAX] {
                let text = render_bar(&bar, elapsed);
                assert!(!text.contains("NaN") && !text.contains("inf"), "{text}");
                for width in [1, 4, 12] {
                    let mut terminal =
                        ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, 3))
                            .unwrap();
                    terminal
                        .draw(|frame| {
                            frame.render_widget(
                                bar.generate_with_elapsed(Some(elapsed)),
                                frame.area(),
                            );
                        })
                        .unwrap();
                }
            }
            let future = Instant::now() + Duration::from_secs(60);
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 3)).unwrap();
            terminal
                .draw(|frame| frame.render_widget(bar.generate(Some(future)), frame.area()))
                .unwrap();
            let text: String =
                terminal.backend().buffer().content().iter().map(|cell| cell.symbol()).collect();
            assert!(
                text.contains("0.00FPS")
                    && text.contains(if completed == total { '✓' } else { '🐦' }),
                "{text}"
            );
        }
    }

    #[test]
    fn resume_rate_formula_excludes_validated_baseline() {
        let mut bar = ProgressBar {
            initial_completed: 100,
            completed: 150,
            total: 300,
            ..Default::default()
        };
        assert_eq!(bar.rates(10.0), (5.0, 30.0));
        bar.completed = 100;
        assert_eq!(bar.rates(10.0), (0.0, 0.0));
        bar.completed = 300;
        bar.initial_completed = 300;
        assert_eq!(bar.rates(0.0), (0.0, 0.0));
        assert_eq!(ProgressBar::clamped_ratio(0, 0), 0.0);
    }
}
