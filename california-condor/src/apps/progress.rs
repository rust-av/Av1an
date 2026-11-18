//! Human progress presentation. Encoding and validated result accounting stay in the apps.
use crate::commands::ProgressMode;
use std::{fmt::Write as _, time::Duration};

/// Small display-only projection; no encoder results or frame histories.
#[derive(Debug, Clone)]
pub struct ProgressSnapshot {
    pub stage: &'static str,
    pub phase: String,
    pub completed: u64,
    pub total: Option<u64>,
    pub initial: u64,
    pub unit: &'static str,
    pub elapsed: Duration,
    pub detail: String,
    pub measure_rate: bool,
}

impl Default for ProgressSnapshot {
    fn default() -> Self {
        Self {
            stage: "",
            phase: String::new(),
            completed: 0,
            total: None,
            initial: 0,
            unit: "frames",
            elapsed: Duration::ZERO,
            detail: String::new(),
            measure_rate: true,
        }
    }
}

/// Rate and ETA are based on this invocation's work, never the resumed baseline.
pub(crate) fn rates(completed: u64, initial: u64, total: u64, elapsed: f64) -> (f64, f64) {
    let done = completed.saturating_sub(initial);
    if done == 0 || !elapsed.is_finite() || elapsed <= 0.0 {
        return (0.0, 0.0);
    }
    let rate = done as f64 / elapsed;
    if !rate.is_finite() {
        return (0.0, 0.0);
    }
    let remaining = total.saturating_sub(completed) as f64 / rate;
    (
        rate,
        if remaining.is_finite() {
            remaining
        } else {
            0.0
        },
    )
}

pub(crate) fn duration(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    )
}

impl ProgressSnapshot {
    fn speed(&self) -> String {
        if !self.measure_rate {
            return String::new();
        }
        let (rate, _) = rates(
            self.completed,
            self.initial,
            self.total.unwrap_or(0),
            self.elapsed.as_secs_f64(),
        );
        if rate <= 0.0 {
            "speed unknown".into()
        } else if self.unit == "frames" && rate < 1.0 {
            format!("{:.2} s/frame", 1.0 / rate)
        } else {
            format!(
                "{rate:.2} {}",
                if self.unit == "frames" {
                    "fps"
                } else {
                    "scenes/s"
                }
            )
        }
    }

    pub(crate) fn line(&self, width: Option<u16>) -> String {
        let mut text = self.stage.to_owned();
        if !self.phase.is_empty() {
            let _ = write!(text, " {}", self.phase);
        }
        let counts = self.total.filter(|total| *total > 0).map_or_else(
            || format!(" | {} {}", self.completed, self.unit),
            |total| {
                let ratio = (self.completed as f64 / total as f64).clamp(0.0, 1.0);
                format!(
                    " {:.0}% | {}/{} {}",
                    ratio * 100.0,
                    self.completed,
                    total,
                    self.unit
                )
            },
        );
        text.push_str(&counts);
        let (rate, eta) = rates(
            self.completed,
            self.initial,
            self.total.unwrap_or(0),
            self.elapsed.as_secs_f64(),
        );
        let eta = if self.measure_rate && rate > 0.0 && self.total.is_some_and(|total| total > 0) {
            format!("ETA {}", duration(Duration::from_secs(eta as u64)))
        } else {
            String::new()
        };
        for field in [duration(self.elapsed), self.speed(), eta, self.detail.clone()] {
            if !field.is_empty()
                && width.is_none_or(|w| text.len() + field.len() + 3 < usize::from(w))
            {
                let _ = write!(text, " | {field}");
            }
        }
        if let Some(width) = width.filter(|width| *width >= 100)
            && let Some(total) = self.total.filter(|total| *total > 0)
        {
            let filled = ((self.completed as f64 / total as f64).clamp(0.0, 1.0) * 16.0) as usize;
            let bar = format!(" | [{}{}]", "=".repeat(filled), "-".repeat(16 - filled));
            if text.len() + bar.len() < usize::from(width) {
                text.push_str(&bar);
            }
        }
        if let Some(width) = width {
            // Inline content is ASCII; never split or wrap a terminal cell.
            text.truncate(usize::from(width).saturating_sub(1));
        }
        text
    }
}

/// Frozen data, deliberately separate from the authoritative execution outcome.
#[derive(Debug, Clone)]
pub struct StageReport {
    pub progress: ProgressSnapshot,
    pub elapsed: Duration,
}

impl StageReport {
    pub(crate) fn line(&self, outcome: &str) -> String {
        let p = &self.progress;
        let counts = p.total.map_or_else(
            || p.completed.to_string(),
            |total| format!("{}/{total}", p.completed),
        );
        // Native muxers may finish before emitting a percentage event.
        let mut line = format!("{} {outcome}: ", p.stage);
        if p.unit != "percent" {
            let _ = write!(line, "{counts} {}, ", p.unit);
        }
        let _ = write!(line, "{} elapsed", duration(self.elapsed));
        if p.initial > 0 {
            let _ = write!(
                line,
                ", {} resumed, {} new",
                p.initial,
                p.completed.saturating_sub(p.initial)
            );
        }
        if !p.phase.is_empty() {
            let _ = write!(line, ", {}", p.phase);
        }
        if p.measure_rate {
            let _ = write!(
                line,
                ", {}average {}",
                if p.phase.is_empty() { "" } else { "phase " },
                p.speed()
            );
        }
        if !p.detail.is_empty() {
            let _ = write!(line, ", {}", p.detail);
        }
        line
    }
}

pub(crate) fn write_output(
    writer: &mut impl std::io::Write,
    path: &std::path::Path,
) -> std::io::Result<()> {
    let size = match std::fs::metadata(path) {
        Ok(metadata) => format!("{:.1} MB", metadata.len() as f64 / 1e6),
        Err(error) => {
            tracing::warn!("Output size unavailable for {}: {error}", path.display());
            "unavailable".into()
        },
    };
    writeln!(writer, "Output: {} | file size {size}", path.display())?;
    writer.flush()
}

pub(crate) struct JobReport {
    pub elapsed: Duration,
    pub output: std::path::PathBuf,
}

pub(crate) fn publish_job(
    result: anyhow::Result<()>,
    report: Option<JobReport>,
    writer: &mut impl std::io::Write,
) -> anyhow::Result<()> {
    result?;
    if !super::is_test_mode()
        && let Some(report) = report
    {
        writeln!(
            writer,
            "Job completed: {} elapsed",
            duration(report.elapsed)
        )?;
        write_output(writer, &report.output)?;
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DisplayMode {
    Full,
    Headless,
    Inline,
    Plain,
}

pub(crate) fn compact_width() -> Option<u16> {
    use ratatui::crossterm;
    #[cfg(windows)]
    {
        // Older Windows consoles may not support the inline erase sequence.
        if !crossterm::ansi_support::supports_ansi() {
            return None;
        }
        crossterm::terminal::size().ok().map(|size| size.0)
    }
    #[cfg(not(windows))]
    {
        // size() can run tput when /dev/tty is unavailable; a guess is not a width.
        crossterm::terminal::window_size()
            .ok()
            .map(|size| size.columns)
            .filter(|width| *width > 0)
    }
}

pub(crate) struct Presenter {
    interactive: bool,
    last: Option<Duration>,
    phase: String,
}

impl Presenter {
    pub(crate) fn new(interactive: bool) -> Self {
        Self {
            interactive,
            last: None,
            phase: String::new(),
        }
    }

    pub(crate) fn update(
        &mut self,
        snapshot: &ProgressSnapshot,
        width: Option<u16>,
        now: Duration,
    ) -> Option<String> {
        let interactive = self.interactive && width.is_some();
        let interval = if interactive {
            Duration::from_millis(100)
        } else {
            Duration::from_secs(5)
        };
        let transition = self.phase != snapshot.phase;
        if let Some(last) = self.last
            && now.saturating_sub(last) < interval
            && (interactive || !transition)
        {
            return None;
        }
        self.last = Some(now);
        self.phase.clone_from(&snapshot.phase);
        Some(snapshot.line(if interactive { width } else { None }))
    }
}

pub(crate) fn write_progress(
    writer: &mut impl std::io::Write,
    line: &str,
    inline: bool,
) -> std::io::Result<()> {
    if inline {
        write!(writer, "\r\x1b[2K{line}")?;
    } else {
        writeln!(writer, "{line}")?;
    }
    writer.flush()
}

pub(crate) fn select_display(
    mode: ProgressMode,
    stdout_tty: bool,
    stderr_tty: bool,
    test_mode: bool,
) -> DisplayMode {
    if test_mode {
        DisplayMode::Headless
    } else {
        match mode {
            ProgressMode::Full if stdout_tty || stderr_tty => DisplayMode::Full,
            ProgressMode::Full => DisplayMode::Headless,
            ProgressMode::Compact if stderr_tty => DisplayMode::Inline,
            ProgressMode::Compact => DisplayMode::Plain,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn formatting_excludes_resume_and_bounds_narrow_lines() {
        let snapshot = ProgressSnapshot {
            stage: "Encoding",
            completed: 150,
            total: Some(300),
            initial: 100,
            elapsed: Duration::from_secs(10),
            ..Default::default()
        };
        let text = snapshot.line(None);
        assert!(
            text.contains("5.00 fps") && text.contains("00:00:30"),
            "{text}"
        );
        for width in 0..160 {
            let line = snapshot.line(Some(width));
            assert!(line.len() <= usize::from(width).saturating_sub(1));
        }
        let report = StageReport {
            progress: snapshot,
            elapsed: Duration::from_secs(10),
        };
        assert!(!report.line("completed").contains("ETA"));
    }

    #[test]
    fn formatting_handles_empty_unknown_and_slow_work() {
        for total in [None, Some(0), Some(1)] {
            for elapsed in [Duration::ZERO, Duration::MAX] {
                let snapshot = ProgressSnapshot {
                    total,
                    elapsed,
                    ..Default::default()
                };
                let text = snapshot.line(None);
                assert!(!text.contains("NaN") && !text.contains("inf"), "{text}");
            }
        }
        let slow = ProgressSnapshot {
            completed: 1,
            total: Some(2),
            elapsed: Duration::from_secs(10),
            ..Default::default()
        };
        assert!(slow.line(None).contains("10.00 s/frame"));
    }

    #[test]
    fn plain_presenter_throttles_without_controls_and_flushes_transitions() {
        let mut presenter = Presenter::new(false);
        let mut output = Vec::new();
        let mut snapshot = ProgressSnapshot {
            stage: "Encoding",
            ..Default::default()
        };
        for millis in [0, 100, 4900, 5000] {
            if let Some(line) = presenter.update(&snapshot, None, Duration::from_millis(millis)) {
                write_progress(&mut output, &line, false).unwrap();
            }
        }
        snapshot.phase = "validating".into();
        assert!(presenter.update(&snapshot, None, Duration::from_millis(5001)).is_some());
        assert_eq!(std::str::from_utf8(&output).unwrap().lines().count(), 2);
        assert!(!output.contains(&b'\r') && !output.contains(&27));
        let mut interactive = Presenter::new(true);
        assert!(interactive.update(&snapshot, Some(80), Duration::ZERO).is_some());
        assert!(interactive.update(&snapshot, Some(80), Duration::from_millis(99)).is_none());
        let line = interactive.update(&snapshot, Some(80), Duration::from_millis(100)).unwrap();
        output.clear();
        write_progress(&mut output, &line, true).unwrap();
        assert!(output.starts_with(b"\r\x1b[2K"));
        assert!(write_progress(&mut std::io::sink(), "ok", false).is_ok());
        struct Broken;
        impl std::io::Write for Broken {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        assert_eq!(
            write_progress(&mut Broken, "failed", false).unwrap_err().kind(),
            std::io::ErrorKind::BrokenPipe
        );
    }

    #[test]
    fn output_size_is_actual_file_bytes_not_video_estimate() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("muxed.mkv");
        std::fs::write(&path, vec![0u8; 1_500_000]).unwrap();
        let mut output = Vec::new();
        write_output(&mut output, &path).unwrap();
        assert!(String::from_utf8(output).unwrap().contains("file size 1.5 MB"));
        let mut output = Vec::new();
        write_output(&mut output, &directory.path().join("missing.mkv")).unwrap();
        assert!(String::from_utf8(output).unwrap().contains("file size unavailable"));
    }

    #[test]
    fn scene_and_worker_trial_projections_keep_their_real_units() {
        use crate::apps::{
            TuiApp,
            benchmarker::{BenchmarkerApp, WorkerStatus},
            noise_detection::NoiseDetectionApp,
        };
        let (clip, scenes) = crate::test_helpers::quality_scene_fixture();
        let noise = NoiseDetectionApp::new(&scenes, clip);
        let snapshot = noise.progress_snapshot(Duration::from_secs(2)).unwrap();
        assert_eq!(snapshot.unit, "scenes");
        assert_eq!(snapshot.total, Some(scenes.len() as u64));
        let mut benchmark = BenchmarkerApp::new(Default::default(), clip);
        let start = std::time::Instant::now();
        benchmark.cached_state_mut().results.insert(
            3,
            WorkerStatus {
                started: start,
                finished: Some(start + Duration::from_secs(10)),
                added: true,
                failed_reason: None,
                initial_frame: 0,
                current_frame: 100,
                total_frames: 100,
            },
        );
        let trial = benchmark.progress_snapshot(Duration::from_secs(100)).unwrap();
        assert_eq!(trial.phase, "3 workers");
        assert!(trial.line(None).contains("10.00 fps"));
    }

    #[test]
    fn successful_concat_report_does_not_claim_stale_percentage() {
        let report = StageReport {
            progress: ProgressSnapshot {
                stage: "Concatenation",
                unit: "percent",
                completed: 0,
                total: Some(100),
                measure_rate: false,
                detail: "1 scenes".into(),
                ..Default::default()
            },
            elapsed: Duration::from_secs(2),
        };
        assert_eq!(
            report.line("completed"),
            "Concatenation completed: 00:00:02 elapsed, 1 scenes"
        );
    }

    #[test]
    fn normal_terminal_widths_keep_speed_and_eta_before_size_detail() {
        let snapshot = ProgressSnapshot {
            stage: "Encoding",
            completed: 7200,
            total: Some(24000),
            elapsed: Duration::from_secs(150),
            detail: "12/40 scenes, 1200.0 kbps, est. video 150.0 MB".into(),
            ..Default::default()
        };
        for width in [80, 120] {
            let text = snapshot.line(Some(width));
            assert!(
                text.contains("48.00 fps") && text.contains("ETA 00:05:50"),
                "{width}: {text}"
            );
            assert!(text.len() < usize::from(width));
        }
    }

    #[test]
    fn phase_report_labels_trial_rate_not_whole_stage_average() {
        let report = StageReport {
            progress: ProgressSnapshot {
                stage: "Target Quality",
                phase: "Compare pass 2".into(),
                completed: 100,
                total: Some(100),
                elapsed: Duration::from_secs(10),
                ..Default::default()
            },
            elapsed: Duration::from_secs(100),
        };
        let text = report.line("completed");
        assert!(
            text.contains("Compare pass 2") && text.contains("phase average 10.00 fps"),
            "{text}"
        );
    }
}
