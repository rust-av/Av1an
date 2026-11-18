use anyhow::Context;
use std::{
    collections::BTreeMap,
    io::IsTerminal,
    sync::{Arc, Mutex},
};

use andean_condor::{
    core::{
        input::clip_info::ClipInfo,
        sequence::{SequenceCompletion, SequenceStatus, Status},
    },
    models::{
        scene::Scene,
        sequence::target_quality::types::{
            MeasurementError, ProbeStatistic, ProbeStrategy, QualityMetric,
        },
    },
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
#[cfg_attr(test, derive(Debug))]
pub struct SceneReportDetails {
    pub quantizer: Option<f64>,
    pub speed: Option<String>,
}

#[derive(Clone)]
#[cfg_attr(test, derive(Debug))]
pub struct QualityCheckState {
    pub scene_scores: BTreeMap<u64, f64>,
    pub scene_details: BTreeMap<u64, SceneReportDetails>,
    pub frames_compared: u64,
    pub total_frames: u64,
    /// Producer counts cover pending selected frames; display counts include this prefix.
    pub initial_completed: u64,
}

pub struct QualityCheckApp {
    pub clip_info: ClipInfo,
    pub metric: QualityMetric,
    pub pass_started: std::time::Instant,
    attempted_cancel: bool,
    shared_progress: SharedProgress<QualityCheckState>,
    cached_state: QualityCheckState,
    // Final statistics require exact samples, but no render tick needs them.
    frame_scores: Arc<Mutex<Vec<f64>>>,
}

impl TuiApp for QualityCheckApp {
    type State = QualityCheckState;

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
        "Press Ctrl+C again to exit immediately."
    }

    fn map_progress(status: SequenceStatus, state: &mut Self::State) -> bool {
        match status {
            SequenceStatus::Whole(status) => match status {
                Status::Processing {
                    completion:
                        SequenceCompletion::Custom {
                            completed, total, ..
                        },
                    ..
                } => {
                    // VMAF submits progress before its final scores are available.
                    state.frames_compared =
                        state.initial_completed.saturating_add(completed as u64);
                    state.total_frames = state.initial_completed.saturating_add(total as u64);
                    if !std::io::stdout().is_terminal() {
                        let event = QualityCheckConsoleEvent::CompareProgress {
                            current_frame: completed as u64,
                            total_frames: total as u64,
                        };
                        println!(
                            "[Quality Check][Compare] {}",
                            serde_json::to_string(&event).unwrap()
                        );
                    }
                    true
                },
                Status::Processing {
                    completion: SequenceCompletion::Frames { completed, total },
                    ..
                } if !std::io::stdout().is_terminal() => {
                    let event = QualityCheckConsoleEvent::CompareProgress {
                        current_frame: completed,
                        total_frames: total,
                    };
                    println!(
                        "[Quality Check][Compare] {}",
                        serde_json::to_string(&event).unwrap()
                    );
                    false
                },
                _ => false,
            },
            SequenceStatus::Subprocess { parent, child } => match (parent, child) {
                (
                    Status::Processing {
                        completion:
                            SequenceCompletion::Custom {
                                name: _,
                                completed,
                                total,
                            },
                        ..
                    },
                    Status::Processing {
                        id,
                        completion: SequenceCompletion::FrameScore { frame, score },
                    },
                ) if id == "Quality" => {
                    state.frames_compared =
                        state.initial_completed.saturating_add(completed as u64);
                    state.total_frames = state.initial_completed.saturating_add(total as u64);
                    if !std::io::stdout().is_terminal() {
                        let event = QualityCheckConsoleEvent::FrameScore { frame, score };
                        println!(
                            "[Quality Check][Frame] {}",
                            serde_json::to_string(&event).unwrap()
                        );
                    }
                    true
                },
                (
                    Status::Processing {
                        completion:
                            SequenceCompletion::Custom {
                                name: _,
                                completed,
                                total,
                            },
                        ..
                    },
                    Status::Processing {
                        id,
                        completion: SequenceCompletion::SceneQuality { index, score, .. },
                    },
                ) if id == "Quality" => {
                    state.frames_compared =
                        state.initial_completed.saturating_add(completed as u64);
                    state.total_frames = state.initial_completed.saturating_add(total as u64);
                    state.scene_scores.insert(index, score);
                    if !std::io::stdout().is_terminal() {
                        let event = QualityCheckConsoleEvent::SceneScore {
                            scene: index,
                            score,
                        };
                        println!(
                            "[Quality Check][Scene] {}",
                            serde_json::to_string(&event).unwrap()
                        );
                    }
                    true
                },
                _ => false,
            },
        }
    }

    fn progress_mapper(
        &self,
    ) -> impl FnMut(SequenceStatus, &mut Self::State) -> bool + Send + 'static {
        let frame_scores = Arc::clone(&self.frame_scores);
        move |status, state| {
            if let SequenceStatus::Subprocess {
                parent:
                    Status::Processing {
                        completion: SequenceCompletion::Custom { .. },
                        ..
                    },
                child:
                    Status::Processing {
                        id,
                        completion: SequenceCompletion::FrameScore { score, .. },
                    },
            } = &status
                && id == "Quality"
            {
                frame_scores.lock().expect("Quality Check report lock").push(*score);
            }
            Self::map_progress(status, state)
        }
    }

    fn after_loop(&mut self) {
        if self.cached_state.frames_compared == self.cached_state.total_frames
            && !self.cached_state.scene_scores.is_empty()
            && let Err(error) = self.print_report()
        {
            eprintln!("Quality Check report failed: {error:#}");
        }
    }

    fn progress_snapshot(
        &self,
        elapsed: std::time::Duration,
    ) -> Option<super::progress::ProgressSnapshot> {
        Some(super::progress::ProgressSnapshot {
            stage: "Quality Check",
            completed: self.cached_state.frames_compared,
            total: Some(self.cached_state.total_frames),
            initial: self.cached_state.initial_completed,
            elapsed,
            ..Default::default()
        })
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
            .title_bottom(Line::from("Quality Check").centered());
        let top_info_inner = top_info.inner(layout[0]);
        let top_info_areas =
            Layout::vertical([Constraint::Fill(1), Constraint::Fill(1)]).split(top_info_inner);
        frame.render_widget(top_info, layout[0]);
        let input_info = InputInfo::new(self.clip_info);
        let input_info = input_info.generate(false);
        frame.render_widget(input_info, top_info_areas[0]);

        let state = &self.cached_state;
        let scene_scores = state
            .scene_scores
            .iter()
            .map(|(index, score)| (*index as f64, *score))
            .collect::<Vec<_>>();
        let datasets = vec![
            Dataset::default()
                .name("Scene Score")
                .style(theme.accent_blue)
                .graph_type(ratatui::widgets::GraphType::Scatter)
                .data(&scene_scores),
        ];
        let max_scenes_label = format!("{}", state.scene_scores.len().saturating_sub(1));
        let max_score = scene_scores.iter().map(|(_, s)| *s).fold(0.0_f64, f64::max);
        let max_score = (max_score / 10.0).ceil() * 10.0; // Round up to nearest 10
        let max_score_label = format!("{}", max_score);
        let chart = Chart::new(datasets)
            .block(
                Block::bordered()
                    .border_type(ratatui::widgets::BorderType::Rounded)
                    .title(Line::from("Quality Score per Scene").centered()),
            )
            .x_axis(
                Axis::default()
                    .title("Scene")
                    .bounds([0.0, state.scene_scores.len() as f64])
                    .labels(["0", &max_scenes_label]),
            )
            .y_axis(
                Axis::default()
                    .title("Score")
                    .bounds([0.0, max_score])
                    .labels(["0", &max_score_label]),
            );
        frame.render_widget(chart, layout[1]);

        let progress_bar = self.progress_bar();
        let progress_bar = progress_bar.generate(Some(self.pass_started));
        frame.render_widget(progress_bar, layout[2]);
    }
}

impl QualityCheckApp {
    fn progress_bar(&self) -> ProgressBar {
        let state = &self.cached_state;
        ProgressBar {
            color: crate::theme::Theme::current().main,
            processing_title: if self.attempted_cancel {
                "Shutting down...".to_owned()
            } else {
                "Comparing".to_owned()
            },
            completed_title: if self.attempted_cancel {
                "Quality Check Aborted".to_owned()
            } else {
                "Quality Check Completed".to_owned()
            },
            top_right_title: String::new(),
            bottom_center_title: String::new(),
            unit_per_second: "FPS".to_owned(),
            unit: "Frame".to_owned(),
            initial_completed: state.initial_completed,
            completed: state.frames_compared,
            total: state.total_frames,
            show_label: true,
        }
    }

    /// Build the initial screen, leaving default unmeasured records empty.
    /// Returns an error for corrupt claimed measurements or undefined reductions.
    /// Selection-matching history is display data, not a substitute for core provenance admission.
    pub fn new(
        clip_info: ClipInfo,
        scenes: &[Scene<CliSequenceData>],
        metric: QualityMetric,
        statistic: ProbeStatistic,
        strategy: &ProbeStrategy,
    ) -> anyhow::Result<QualityCheckApp> {
        let scene_scores: BTreeMap<u64, f64> = scenes
            .iter()
            .enumerate()
            .filter(|(_, scene)| !scene.sequence_data.quality_check.quality.is_unmeasured())
            .map(|(index, scene)| {
                Ok((
                    index as u64,
                    metric
                        .calculate_score(
                            statistic,
                            &scene.sequence_data.quality_check.quality.scores,
                        )
                        .with_context(|| format!("recorded Quality Check scene {index} pass 1"))?,
                ))
            })
            .collect::<anyhow::Result<_>>()?;
        let scene_details: BTreeMap<u64, SceneReportDetails> = scenes
            .iter()
            .enumerate()
            .map(|(scene_index, scene)| {
                (
                    scene_index as u64,
                    SceneReportDetails {
                        quantizer: scene.encoder.quantizer(),
                        speed: scene.encoder.speed(),
                    },
                )
            })
            .collect();
        let frame_scores: Vec<f64> = scenes
            .iter()
            .flat_map(|scene| scene.sequence_data.quality_check.quality.scores.iter().copied())
            .collect();
        // Match the producer's skip condition, including temporal prefix counts.
        // Admission still belongs to the core runner; scores alone are not provenance.
        let mut total_frames = 0_u64;
        let mut frames_compared = 0_u64;
        for scene in scenes {
            let selected = strategy.frame_indices(scene.start_frame, scene.end_frame)?.len();
            total_frames = total_frames.saturating_add(selected as u64);
            let quality = &scene.sequence_data.quality_check.quality;
            if !quality.is_unmeasured() && quality.scores.len() == selected {
                frames_compared = frames_compared.saturating_add(selected as u64);
            }
        }
        Self::compute_stats(
            &scene_scores.values().copied().collect::<Vec<_>>(),
            metric.is_inverse_metric(),
        )
        .context("recorded Quality Check scene report")?;
        if !metric.is_cumulative() {
            Self::compute_stats(&frame_scores, metric.is_inverse_metric())
                .context("recorded Quality Check frame report")?;
        }
        let state = QualityCheckState {
            scene_scores,
            scene_details,
            frames_compared,
            total_frames,
            initial_completed: frames_compared,
        };
        Ok(QualityCheckApp {
            clip_info,
            metric,
            pass_started: std::time::Instant::now(),
            attempted_cancel: false,
            shared_progress: SharedProgress::new(state.clone()),
            cached_state: state,
            frame_scores: Arc::new(Mutex::new(frame_scores)),
        })
    }

    fn compute_stats(scores: &[f64], is_inverse: bool) -> anyhow::Result<Option<ReportStats>> {
        if scores.is_empty() {
            return Ok(None);
        }
        let count = scores.len();
        let average = ProbeStatistic::Mean.calculate(scores)?;
        let variance =
            scores.iter().map(|s| (s - average) * (s - average)).sum::<f64>() / count as f64;
        let std_dev = variance.sqrt();
        anyhow::ensure!(
            variance.is_finite(),
            MeasurementError::Statistic("non-finite report variance")
        );
        let mut sorted = scores.to_vec();
        sorted.sort_by(f64::total_cmp);
        let minimum = sorted.first().copied().unwrap_or(0.0);
        let maximum = sorted.last().copied().unwrap_or(0.0);
        // 1% worst percentile
        let percentile_index = if is_inverse {
            ((count as f64 * 0.99).ceil() as usize).saturating_sub(1).min(count - 1)
        } else {
            ((count as f64 * 0.01).floor() as usize).min(count - 1)
        };
        let percentile = sorted[percentile_index];
        // Count scores outside 2 standard deviations from the average
        let outside_two_sigma =
            scores.iter().filter(|s| (**s - average).abs() > 2.0 * std_dev).count();
        Ok(Some(ReportStats {
            count,
            average,
            minimum,
            maximum,
            percentile,
            std_dev,
            outside_two_sigma,
        }))
    }

    /// Scenes scoring outside 2 sigma
    fn poor_scenes(&self) -> anyhow::Result<Vec<(u64, f64)>> {
        let is_inverse = self.metric.is_inverse_metric();
        let scores: Vec<f64> = self.cached_state.scene_scores.values().copied().collect();
        let Some(stats) = Self::compute_stats(&scores, is_inverse)? else {
            return Ok(Vec::new());
        };
        let threshold_low = 2.0f64.mul_add(-stats.std_dev, stats.average);
        let threshold_high = 2.0f64.mul_add(stats.std_dev, stats.average);
        let mut poor = self
            .cached_state
            .scene_scores
            .iter()
            .filter_map(|(index, score)| {
                let is_poor = if is_inverse {
                    *score > threshold_high
                } else {
                    *score < threshold_low
                };
                is_poor.then_some((*index, *score))
            })
            .collect::<Vec<_>>();
        // Sort by worst score
        poor.sort_by(|a, b| {
            if is_inverse {
                b.1.total_cmp(&a.1)
            } else {
                a.1.total_cmp(&b.1)
            }
        });
        Ok(poor)
    }

    /// Print the quality report as aligned plain-text tables
    fn print_report(&self) -> anyhow::Result<()> {
        let metric_name = self.metric.friendly_name();
        let is_inverse = self.metric.is_inverse_metric();
        let percentile_label = if is_inverse { "1% High" } else { "1% Low" };

        let scene_scores: Vec<f64> = self.cached_state.scene_scores.values().copied().collect();
        let frame_scores = self.frame_scores.lock().expect("Quality Check report lock");
        let scene_stats = Self::compute_stats(&scene_scores, is_inverse)?;
        let frame_stats = if self.metric.is_cumulative() {
            None
        } else {
            Self::compute_stats(&frame_scores, is_inverse)?
        };
        let poor = self.poor_scenes()?;

        println!();
        println!("Quality Check - {metric_name}");

        // Stats table: header + Per-Scene + Per-Frame rows
        println!(
            "{:<14}{:>12}{:>12}{:>12}{:>12}{:>12}",
            "Scope", "Average", "Minimum", "Maximum", percentile_label, "Std Dev (σ)"
        );
        for (label, stats) in
            [("Per-Scene", scene_stats.as_ref()), ("Per-Frame", frame_stats.as_ref())]
        {
            if label == "Per-Frame" && self.metric.is_cumulative() {
                continue;
            }
            if let Some(s) = stats {
                println!(
                    "{:<14}{:>12.4}{:>12.4}{:>12.4}{:>12.4}{:>12.4}",
                    label, s.average, s.minimum, s.maximum, s.percentile, s.std_dev
                );
            } else {
                println!("{label:<14}  (no data)");
            }
        }
        if self.metric.is_cumulative() {
            println!(
                "Per-Scene statistics describe independent scene results, not a clip-wide CVVDP score."
            );
        }
        println!();

        // Scenes outside 2 sigma
        if let Some(scene_stats) = &scene_stats {
            println!(
                "Scenes outside 2σ: {} of {}",
                scene_stats.outside_two_sigma, scene_stats.count
            );
        }
        println!();

        // Poor scenes table
        if !poor.is_empty() {
            println!("Scenes Outside 2σ ({})", poor.len());
            println!("{}", "─".repeat(72));
            println!(
                "{:<8}{:>12}{:>14}{:>12}",
                "Scene", "Score", "Speed", "Quantizer"
            );
            for (index, score) in &poor {
                let details = self.cached_state.scene_details.get(index);
                let speed = details.and_then(|d| d.speed.as_deref()).unwrap_or("-");
                let quantizer = details
                    .and_then(|d| d.quantizer)
                    .map_or_else(|| "-".to_owned(), |q| format!("{q:.4}"));
                println!("{index:<8}{score:>12.4}{speed:>14}{quantizer:>12}");
            }
        }
        Ok(())
    }
}

struct ReportStats {
    count: usize,
    average: f64,
    minimum: f64,
    maximum: f64,
    /// 1% worst
    percentile: f64,
    std_dev: f64,
    outside_two_sigma: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QualityCheckConsoleEvent {
    FrameScore {
        frame: u64,
        score: f64,
    },
    SceneScore {
        scene: u64,
        score: f64,
    },
    CompareProgress {
        current_frame: u64,
        total_frames: u64,
    },
}

#[cfg(test)]
mod tests {
    #[test]
    fn qc_resume_counts_selected_work_not_current_run_throughput() {
        let (clip, mut scenes) = crate::test_helpers::quality_scene_fixture();
        scenes.truncate(2);
        scenes[0].start_frame = 0;
        scenes[0].end_frame = 90;
        scenes[0].sequence_data.quality_check.quality.scores = vec![80.0; 90];
        scenes[1].start_frame = 90;
        scenes[1].end_frame = 100;
        scenes[1].sequence_data.quality_check.quality = Default::default();
        for (resumed, completed, pending_total, label, remaining) in [
            (true, 91, 10, "91/100 Frames (0.50FPS)", "00:18"),
            (false, 1, 100, "1/100 Frames (0.50FPS)", "03:18"),
        ] {
            if !resumed {
                scenes[0].sequence_data.quality_check.quality = Default::default();
            }
            let mut app = QualityCheckApp::new(
                clip,
                &scenes,
                QualityMetric::default(),
                ProbeStatistic::Mean,
                &ProbeStrategy::Whole,
            )
            .unwrap();
            assert_eq!(app.cached_state.total_frames, 100);
            assert_eq!(
                app.cached_state.frames_compared,
                if resumed { 90 } else { 0 }
            );
            let mut mapper = app.progress_mapper();
            // QC reports only the pending selection, not the restored prefix.
            app.shared_progress.apply(|state| {
                mapper(
                    SequenceStatus::Whole(Status::Processing {
                        id: "Quality Check".into(),
                        completion: SequenceCompletion::Custom {
                            name: "Quality Check".into(),
                            completed: 1.0,
                            total: pending_total as f64,
                        },
                    }),
                    state,
                )
            });
            app.shared_progress.apply(|state| {
                mapper(
                    SequenceStatus::Subprocess {
                        parent: Status::Processing {
                            id: "Quality Check".into(),
                            completion: SequenceCompletion::Custom {
                                name: "Quality Check".into(),
                                completed: 1.0,
                                total: pending_total as f64,
                            },
                        },
                        child: Status::Processing {
                            id: "Quality".into(),
                            completion: SequenceCompletion::FrameScore {
                                frame: completed - 1,
                                score: 81.0,
                            },
                        },
                    },
                    state,
                )
            });
            app.on_snapshot(app.shared_progress.read());
            assert_eq!(
                (
                    app.cached_state.frames_compared,
                    app.cached_state.total_frames
                ),
                (completed, 100)
            );
            let text = crate::components::progress_bar::tests::render_bar(
                &app.progress_bar(),
                Duration::from_secs(2),
            );
            assert!(text.contains(label) && text.contains(remaining), "{text}");
            let zero = crate::components::progress_bar::tests::render_bar(
                &app.progress_bar(),
                Duration::ZERO,
            );
            assert!(
                zero.contains("0.00FPS") && !zero.contains("NaN") && !zero.contains("inf"),
                "{zero}"
            );
        }
    }

    #[test]
    fn qc_cumulative_resume_matches_selected_prefix_count_not_scene_length() {
        let (clip, mut scenes) = crate::test_helpers::quality_scene_fixture();
        scenes.truncate(2);
        scenes[0].start_frame = 10;
        scenes[0].end_frame = 190;
        scenes[1].start_frame = 190;
        scenes[1].end_frame = 210;
        scenes[1].sequence_data.quality_check.quality = Default::default();
        let metric = QualityMetric::CVVDP {
            target_range: (3.9, 4.1),
            resolution: None,
            display_model: None,
            resize_to_display: None,
            disable_temporal: None,
            gpu_id: None,
        };
        for (scores, prior) in [(vec![4.0; 90], 90), (vec![4.0], 0)] {
            scenes[0].sequence_data.quality_check.quality.scores = scores;
            let mut app = QualityCheckApp::new(
                clip,
                &scenes,
                metric.clone(),
                ProbeStatistic::Mean,
                &ProbeStrategy::Skip { skip: 2 },
            )
            .unwrap();
            assert_eq!(
                (
                    app.cached_state.frames_compared,
                    app.cached_state.total_frames
                ),
                (prior, 100)
            );
            assert_eq!(app.cached_state.scene_scores[&0], 4.0);
            let mut mapper = app.progress_mapper();
            app.shared_progress.apply(|state| {
                mapper(
                    SequenceStatus::Subprocess {
                        parent: Status::Processing {
                            id: "Quality Check".into(),
                            completion: SequenceCompletion::Custom {
                                name: "Quality Check".into(),
                                completed: 1.0,
                                total: if prior == 90 { 10.0 } else { 100.0 },
                            },
                        },
                        child: Status::Processing {
                            id: "Quality".into(),
                            completion: SequenceCompletion::FrameScore {
                                frame: 190,
                                score: 4.1,
                            },
                        },
                    },
                    state,
                )
            });
            app.on_snapshot(app.shared_progress.read());
            let text = crate::components::progress_bar::tests::render_bar(
                &app.progress_bar(),
                Duration::from_secs(2),
            );
            assert!(
                text.contains(if prior == 90 {
                    "91/100 Frames (0.50FPS)"
                } else {
                    "1/100 Frames (0.50FPS)"
                }),
                "{text}"
            );
        }
        scenes[0].end_frame = scenes[0].start_frame;
        assert!(
            QualityCheckApp::new(
                clip,
                &scenes,
                metric,
                ProbeStatistic::Mean,
                &ProbeStrategy::Whole
            )
            .is_err()
        );
    }

    #[test]
    fn qc_snapshot_excludes_final_raw_scores() {
        let (clip, mut scenes) = crate::test_helpers::quality_scene_fixture();
        scenes[0].sequence_data.quality_check.quality.scores = vec![73.125; 8192];
        scenes[0].sequence_data.quality_check.quality.scores[0] = 1.0;
        scenes[0].end_frame = 8192;
        scenes[1].start_frame = 8192;
        scenes[1].end_frame = 8195;
        scenes[2].start_frame = 8195;
        scenes[2].end_frame = 8203;
        let app = QualityCheckApp::new(
            clip,
            &scenes,
            QualityMetric::default(),
            ProbeStatistic::Mean,
            &ProbeStrategy::Whole,
        )
        .unwrap();
        let snapshot = app.shared_progress.read();
        assert_eq!(snapshot.frames_compared, 8195);
        assert_eq!(snapshot.scene_scores.len(), 2);
        assert!(
            !format!("{snapshot:?}").contains("73.125"),
            "raw final report vector is not render data"
        );
        app.frame_scores.lock().unwrap().reserve(8);
        let allocation = app.frame_scores.lock().unwrap().as_ptr();
        let mut mapper = app.progress_mapper();
        for frame in 8195..8203 {
            app.shared_progress.apply(|state| {
                mapper(
                    SequenceStatus::Subprocess {
                        parent: Status::Processing {
                            id: "Quality Check".into(),
                            completion: SequenceCompletion::Custom {
                                name: "Quality Check".into(),
                                completed: (frame - 8195) as f64 + 1.0,
                                total: 8.0,
                            },
                        },
                        child: Status::Processing {
                            id: "Quality".into(),
                            completion: SequenceCompletion::FrameScore { frame, score: 80.0 },
                        },
                    },
                    state,
                )
            });
            let current = app.shared_progress.read_if_dirty().unwrap();
            assert_eq!(current.frames_compared, frame + 1);
            assert_eq!(
                app.frame_scores.lock().unwrap().as_ptr(),
                allocation,
                "cached snapshots must not force raw-score copies on append"
            );
            assert_eq!(
                std::sync::Arc::strong_count(&app.frame_scores),
                2,
                "only app and producer own final scores"
            );
        }
        assert_eq!(app.frame_scores.lock().unwrap().len(), 8203);
    }

    mod internal_integration {
        use super::*;

        #[test]
        fn qc_saved_and_live_scores_produce_complete_final_statistics() {
            const TEST: &str = "apps::quality_check::tests::internal_integration::qc_saved_and_live_scores_produce_complete_final_statistics";
            if std::env::var_os("QC_RESUMED_REPORT_CHILD").is_none() {
                let output = Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", TEST, "--nocapture"])
                    .env("QC_RESUMED_REPORT_CHILD", TEST)
                    .env("CONDOR_TEST_MODE", "1")
                    .output()
                    .unwrap();
                let stdout = String::from_utf8_lossy(&output.stdout);
                assert!(
                    output.status.success(),
                    "{stdout}\n{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                assert!(
                    stdout.contains("1 passed; 0 failed;")
                        && stdout.contains("QC_RESUMED_INTERNAL_ASSERTIONS_COMPLETED")
                );
                eprintln!("{stdout}");
                return;
            }
            assert_eq!(std::env::var("QC_RESUMED_REPORT_CHILD").unwrap(), TEST);
            let app = crate::test_helpers::quality_scenes::run_resumed_quality_report();
            assert_eq!(
                (
                    app.cached_state.frames_compared,
                    app.cached_state.total_frames
                ),
                (200, 200)
            );
            assert_eq!(
                app.cached_state.scene_scores,
                BTreeMap::from([(0, 49.5), (1, 149.5)])
            );
            let scores = app.frame_scores.lock().unwrap();
            assert_eq!(*scores, (0..200).map(f64::from).collect::<Vec<_>>());
            let stats = QualityCheckApp::compute_stats(&scores, false).unwrap().unwrap();
            assert_eq!(
                (
                    stats.count,
                    stats.average,
                    stats.minimum,
                    stats.maximum,
                    stats.percentile
                ),
                (200, 99.5, 0.0, 199.0, 2.0)
            );
            assert_eq!(stats.std_dev, 3333.25_f64.sqrt());
            println!("QC_RESUMED_INTERNAL_ASSERTIONS_COMPLETED");
        }

        fn check_prefix_report(independent: bool, test: &str) {
            if let Ok(selected) = std::env::var("QC_PREFIX_REPORT_CHILD") {
                assert_eq!(selected, test);
                crate::test_helpers::quality_scenes::prefix_report_app(independent)
                    .print_report()
                    .expect("report");
                println!("QC_PREFIX_INTERNAL_ASSERTIONS_COMPLETED");
                return;
            }
            let output = Command::new(std::env::current_exe().expect("test executable"))
                .args(["--exact", test, "--nocapture"])
                .env("QC_PREFIX_REPORT_CHILD", test)
                .output()
                .expect("report child");
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(output.status.success(), "{stdout}");
            assert!(
                stdout.contains("1 passed; 0 failed;")
                    && stdout.contains("QC_PREFIX_INTERNAL_ASSERTIONS_COMPLETED")
            );
            eprintln!("{stdout}");
        }

        #[test]
        fn temporal_report_accepts_cumulative_prefix_history() {
            check_prefix_report(
                false,
                "apps::quality_check::tests::internal_integration::temporal_report_accepts_cumulative_prefix_history",
            );
        }

        #[test]
        fn independent_report_accepts_frame_history() {
            check_prefix_report(
                true,
                "apps::quality_check::tests::internal_integration::independent_report_accepts_frame_history",
            );
        }

        fn check_submission_progress(through_final_scores: bool, test: &str) {
            if let Ok(selected) = std::env::var("QC_SUBMISSION_PROGRESS_CHILD") {
                assert_eq!(selected, test);
                crate::test_helpers::quality_scenes::map_submission_progress(
                    through_final_scores,
                    |app, state, final_scores| {
                        if final_scores {
                            assert_eq!(*app.frame_scores.lock().unwrap(), [90.0, 91.0, 92.0]);
                            assert!(state.scene_scores.is_empty());
                        } else {
                            assert!(
                                app.frame_scores.lock().unwrap().is_empty()
                                    && state.scene_scores.is_empty(),
                                "submission progress must not fabricate scores"
                            );
                        }
                    },
                );
                println!("QC_SUBMISSION_INTERNAL_ASSERTIONS_COMPLETED");
                return;
            }
            let output = crate::test_helpers::quality_scenes::submission_output(test);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("QC submission progress mapper/console:\n{stdout}\n{stderr}");
            assert!(output.status.success(), "{stdout}\n{stderr}");
            assert!(
                stdout.contains("running 1 test") && stdout.contains("1 passed; 0 failed"),
                "exact child filter must execute one test"
            );
            assert!(stdout.contains("QC_SUBMISSION_INTERNAL_ASSERTIONS_COMPLETED"));
        }

        #[test]
        fn qc_submission_progress_updates_ui_without_fabricating_scores() {
            check_submission_progress(
                false,
                "apps::quality_check::tests::internal_integration::qc_submission_progress_updates_ui_without_fabricating_scores",
            );
        }

        #[test]
        fn qc_final_scores_keep_submission_total_and_raw_scores() {
            check_submission_progress(
                true,
                "apps::quality_check::tests::internal_integration::qc_final_scores_keep_submission_total_and_raw_scores",
            );
        }
    }

    #[test]
    fn qc_statistics_empty_scores_return_no_measurement() {
        assert!(QualityCheckApp::compute_stats(&[], false).expect("no measurement").is_none());
    }

    #[test]
    fn qc_statistics_nonfinite_or_overflowing_scores_return_error() {
        for scores in [
            vec![f64::NAN],
            vec![f64::INFINITY],
            vec![f64::NEG_INFINITY],
            vec![f64::MAX, f64::MAX],
            vec![-f64::MAX, f64::MAX],
        ] {
            assert!(
                QualityCheckApp::compute_stats(&scores, false).is_err(),
                "{scores:?}"
            );
        }
    }

    #[test]
    fn qc_statistics_accept_finite_negative_scores() {
        let stats = QualityCheckApp::compute_stats(&[-1.0, 2.0], false)
            .expect("finite negatives")
            .expect("stats");
        assert_eq!(
            (stats.average, stats.minimum, stats.maximum, stats.std_dev),
            (0.5, -1.0, 2.0, 1.5)
        );
    }
    #[test]
    fn saved_qc_rejects_corrupt_measurements_but_preserves_unmeasured_screens() {
        let (clip, scenes) = crate::test_helpers::quality_scene_fixture();
        for scores in
            [vec![f64::NAN], vec![f64::INFINITY], vec![f64::NEG_INFINITY], vec![1.0, -1.0]]
        {
            let mut corrupt = scenes.clone();
            corrupt[1].sequence_data.quality_check.quality.scores = scores;
            assert!(
                QualityCheckApp::new(
                    clip,
                    &corrupt,
                    QualityMetric::default(),
                    ProbeStatistic::Harmonic,
                    &ProbeStrategy::Whole,
                )
                .is_err()
            );
        }
        let mut corrupt = scenes;
        corrupt[1].sequence_data.quality_check.quality.scores.clear();
        corrupt[1].sequence_data.quality_check.quality.completed_on = 1;
        assert!(
            QualityCheckApp::new(
                clip,
                &corrupt,
                QualityMetric::default(),
                ProbeStatistic::Mean,
                &ProbeStrategy::Whole,
            )
            .is_err()
        );
        for scene in &mut corrupt {
            scene.sequence_data.quality_check.quality = Default::default();
        }
        let app = QualityCheckApp::new(
            clip,
            &corrupt,
            QualityMetric::default(),
            ProbeStatistic::Mean,
            &ProbeStrategy::Whole,
        )
        .expect("initial screen");
        assert!(
            app.cached_state.scene_scores.is_empty() && app.frame_scores.lock().unwrap().is_empty()
        );
    }
    use std::{process::Command, time::Duration};

    use super::*;

    #[test]
    fn saved_qc_constructor_and_live_mapper_keep_scene_finals_and_prefixes() {
        let (clip, scenes) = crate::test_helpers::quality_scene_fixture();
        for disabled in [None, Some(false), Some(true)] {
            let metric = QualityMetric::CVVDP {
                target_range: (3.9, 4.1),
                resolution: None,
                display_model: None,
                resize_to_display: None,
                disable_temporal: disabled,
                gpu_id: None,
            };
            for (statistic, independent) in [
                (ProbeStatistic::Mean, [5.0, 5.0]),
                (ProbeStatistic::Minimum, [2.0, 1.0]),
                (ProbeStatistic::Maximum, [9.0, 8.0]),
            ] {
                let app = QualityCheckApp::new(
                    clip,
                    &scenes,
                    metric.clone(),
                    statistic,
                    &ProbeStrategy::Whole,
                )
                .expect("app");
                let expected = if disabled == Some(true) {
                    independent
                } else {
                    [4.0, 6.0]
                };
                assert_eq!(
                    app.cached_state.scene_scores,
                    BTreeMap::from([(0, expected[0]), (1, expected[1])])
                );
                assert_eq!(
                    *app.frame_scores.lock().unwrap(),
                    [2.0, 9.0, 4.0, 1.0, 8.0, 6.0]
                );
                assert_eq!(
                    (
                        app.cached_state.frames_compared,
                        app.cached_state.total_frames
                    ),
                    (6, 9)
                );
                let mut live = QualityCheckApp::new(
                    clip,
                    &[],
                    metric.clone(),
                    statistic,
                    &ProbeStrategy::Whole,
                )
                .expect("app")
                .cached_state;
                for (index, score) in expected.into_iter().enumerate() {
                    assert!(QualityCheckApp::map_progress(
                        SequenceStatus::Subprocess {
                            parent: Status::Processing {
                                id: "Quality Check".to_owned(),
                                completion: SequenceCompletion::Custom {
                                    name: "Quality Check".to_owned(),
                                    completed: 6.0,
                                    total: 6.0
                                }
                            },
                            child: Status::Processing {
                                id: "Quality".to_owned(),
                                completion: SequenceCompletion::SceneQuality {
                                    index: index as u64,
                                    quantizer: 0.0,
                                    score,
                                    bitrate: 0.0
                                }
                            }
                        },
                        &mut live
                    ));
                }
                assert_eq!(live.scene_scores, app.cached_state.scene_scores);
                let control = QualityCheckApp::new(
                    clip,
                    &scenes,
                    QualityMetric::default(),
                    statistic,
                    &ProbeStrategy::Whole,
                )
                .expect("app");
                assert_eq!(
                    control.cached_state.scene_scores.values().copied().collect::<Vec<_>>(),
                    independent
                );
            }
        }
        assert_eq!(
            scenes[0].sequence_data.quality_check.quality.scores,
            [2.0, 9.0, 4.0]
        );
    }
}
