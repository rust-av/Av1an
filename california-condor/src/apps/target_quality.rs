use anyhow::Context;
use std::{collections::BTreeMap, io::IsTerminal, sync::Arc};

use andean_condor::{
    core::{
        input::clip_info::ClipInfo,
        sequence::{SequenceCompletion, SequenceStatus, Status},
    },
    models::{
        encoder::Encoder,
        scene::Scene,
        sequence::target_quality::types::{ProbeStatistic, QualityMetric},
    },
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::Color,
    text::Line,
    widgets::{Axis, Block, Chart, Dataset, Paragraph},
};
use serde::{Deserialize, Serialize};

use crate::{
    apps::{SharedProgress, TuiApp},
    components::{
        encoder_info::{EncoderInfo, representative_encoder},
        input_info::InputInfo,
        progress_bar::ProgressBar,
    },
    configuration::CliSequenceData,
};
#[derive(Clone)]
pub struct TargetQualityState {
    // Frame-only snapshots share the chart; new results copy only the changed scene's history.
    pub quality_passes: Arc<BTreeMap<u64, Arc<Vec<QualityPass>>>>,
    pub current_pass: u8,
    pub frames_encoded: u64,
    pub frames_compared: u64,
    pub total_frames: u64,
}

pub struct TargetQualityApp {
    pub encoder: Encoder,
    pub(crate) encoder_summary: Option<Encoder>,
    pub clip_info: ClipInfo,
    pub pass_started: std::time::Instant,
    /// The stopped clock of a pass whose frames are all done, held so the
    /// progress bar's FPS stops moving through the gap before the next pass.
    pass_completed: Option<std::time::Instant>,
    attempted_cancel: bool,
    shared_progress: SharedProgress<TargetQualityState>,
    cached_state: TargetQualityState,
}

impl TuiApp for TargetQualityApp {
    type State = TargetQualityState;

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
        "Waiting for Encoders to finish. Press Ctrl+C again to exit immediately."
    }

    fn map_progress(status: SequenceStatus, state: &mut Self::State) -> bool {
        match status {
            SequenceStatus::Whole(status) => match status {
                Status::Processing {
                    completion: SequenceCompletion::Passes { total, .. },
                    ..
                } => {
                    state.current_pass = total;
                    if !std::io::stdout().is_terminal() {
                        let event = TargetQualityConsoleEvent::Pass(total);
                        println!(
                            "[Target Quality][Pass] {}",
                            serde_json::to_string(&event).unwrap()
                        );
                    }
                    true
                },
                Status::Processing {
                    id,
                    completion: SequenceCompletion::Frames { completed, total },
                } if id == "Encode" || id == "Compare" => {
                    if id == "Encode" {
                        state.frames_compared = 0;
                        state.frames_encoded = completed;
                        state.total_frames = total;
                    } else {
                        if completed == 0 {
                            state.frames_encoded = total;
                        }
                        state.frames_compared = completed;
                        state.total_frames = total;
                    }
                    true
                },
                _ => false,
            },
            SequenceStatus::Subprocess { parent, child } => match (parent, child) {
                (
                    Status::Processing {
                        completion:
                            SequenceCompletion::Passes {
                                completed: current_pass,
                                total: total_passes,
                            },
                        ..
                    },
                    Status::Processing {
                        id,
                        completion: SequenceCompletion::Frames { completed, total },
                    },
                ) if id == "Encode" => {
                    state.frames_compared = 0;
                    state.frames_encoded = completed;
                    state.total_frames = total;
                    if !std::io::stdout().is_terminal() {
                        let event = TargetQualityConsoleEvent::EncodeProgress {
                            current_pass,
                            total_passes,
                            current_frame: completed,
                            total_frames: total,
                        };
                        println!(
                            "[Target Quality][Encode] {}",
                            serde_json::to_string(&event).unwrap()
                        );
                    }
                    true
                },
                (
                    Status::Processing {
                        completion:
                            SequenceCompletion::Passes {
                                completed: current_pass,
                                total: total_passes,
                            },
                        ..
                    },
                    Status::Processing {
                        id,
                        completion: SequenceCompletion::Frames { completed, total },
                    },
                ) if id == "Compare" => {
                    if completed == 0 {
                        state.frames_encoded = total;
                    }
                    state.frames_compared = completed;
                    state.total_frames = total;
                    if !std::io::stdout().is_terminal() {
                        let event = TargetQualityConsoleEvent::CompareProgress {
                            current_pass,
                            total_passes,
                            current_frame: completed,
                            total_frames: total,
                        };
                        println!(
                            "[Target Quality][Compare] {}",
                            serde_json::to_string(&event).unwrap()
                        );
                    }
                    true
                },
                (
                    Status::Processing {
                        completion:
                            SequenceCompletion::Passes {
                                completed: current_pass,
                                total: total_passes,
                            },
                        ..
                    },
                    Status::Processing {
                        id,
                        completion:
                            SequenceCompletion::SceneQuality {
                                index,
                                quantizer,
                                score,
                                bitrate,
                            },
                    },
                ) if id == "Quality" => {
                    let pass = QualityPass {
                        scene: index,
                        current_pass,
                        total_passes,
                        quantizer,
                        score,
                        bitrate,
                    };
                    let history =
                        Arc::make_mut(&mut state.quality_passes).entry(index).or_default();
                    Arc::make_mut(history).push(pass.clone());
                    if !std::io::stdout().is_terminal() {
                        let event = TargetQualityConsoleEvent::QualityPass(pass);
                        println!(
                            "[Target Quality][Quality] {}",
                            serde_json::to_string(&event).unwrap()
                        );
                    }
                    true
                },
                _ => false,
            },
        }
    }

    fn on_snapshot(&mut self, snapshot: Self::State) {
        // Reset the pass timer when entering a new encode/compare phase or
        // pass.
        let new_encode_phase = snapshot.frames_encoded == 0 && snapshot.frames_compared == 0;
        let new_compare_phase =
            snapshot.frames_compared > 0 && self.cached_state.frames_compared == 0;
        let pass_changed = snapshot.current_pass != self.cached_state.current_pass;
        if new_encode_phase || new_compare_phase || pass_changed {
            self.pass_started = std::time::Instant::now();
        }
        Self::stop_clock_at_completion(&snapshot, &mut self.pass_completed);
        self.cached_state = snapshot;
    }

    fn render(&self, frame: &mut Frame) {
        let theme = crate::theme::Theme::current();
        let main: Color = theme.main;
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
            .title_bottom(
                Line::from(
                    self.encoder_summary
                        .as_ref()
                        .map_or("Mixed encoders", |encoder| encoder.base().friendly_name()),
                )
                .centered(),
            );
        let top_info_inner = top_info.inner(layout[0]);
        let top_info_areas =
            Layout::vertical([Constraint::Fill(1), Constraint::Fill(1)]).split(top_info_inner);
        frame.render_widget(top_info, layout[0]);
        let input_info = InputInfo::new(self.clip_info);
        let input_info = input_info.generate(false);
        frame.render_widget(input_info, top_info_areas[0]);
        if let Some(encoder) = &self.encoder_summary {
            let encoder_info = EncoderInfo::new(encoder.clone(), None);
            frame.render_widget(encoder_info.generate(false), top_info_areas[1]);
        } else {
            frame.render_widget(
                Paragraph::new("Mixed encoders: per-scene settings").centered(),
                top_info_areas[1],
            );
        }

        let state = &self.cached_state;
        let (quantizers, scores) = state.quality_passes.iter().fold(
            (Vec::new(), Vec::new()),
            |(mut quantizers, mut scores), (index, quality_passes)| {
                if let Some(quality_pass) = quality_passes.iter().last() {
                    quantizers.push((*index as f64, quality_pass.quantizer));
                    scores.push((*index as f64, quality_pass.score));
                } else {
                    quantizers.push((*index as f64, self.encoder.quantizer().unwrap_or(0.0)));
                    scores.push((*index as f64, 0.0));
                }
                (quantizers, scores)
            },
        );
        let datasets = vec![
            Dataset::default()
                .name("Quantizer")
                .style(theme.accent_blue)
                .graph_type(ratatui::widgets::GraphType::Scatter)
                .data(&quantizers),
            Dataset::default()
                .name("Score")
                .style(theme.accent_green)
                .graph_type(ratatui::widgets::GraphType::Scatter)
                .data(&scores),
        ];
        let max_scenes_label = format!("{}", state.quality_passes.len().saturating_sub(1));
        let max_quantizer = quantizers.iter().map(|(_, q)| *q).fold(0.0_f64, f64::max);
        let max_score = scores.iter().map(|(_, s)| *s).fold(0.0_f64, f64::max);
        let max_quantizer_score = (f64::max(max_quantizer, max_score) / 10.0).ceil() * 10.0; // Round up to nearest 10
        let max_quantizer_score_label = format!("{}", max_quantizer_score);
        let chart = Chart::new(datasets)
            .block(
                Block::bordered()
                    .border_type(ratatui::widgets::BorderType::Rounded)
                    .title(Line::from("Quantizer and Score per Scene").centered()),
            )
            .x_axis(
                Axis::default()
                    .title("Scene")
                    .bounds([0.0, state.quality_passes.len() as f64])
                    .labels(["0", &max_scenes_label]),
            )
            .y_axis(
                Axis::default()
                    .title("Quantizer/Score")
                    .bounds([0.0, max_quantizer_score])
                    .labels(["0", &max_quantizer_score_label]),
            );
        frame.render_widget(chart, layout[1]);

        let progress_bar = ProgressBar {
            color: main,
            processing_title: if self.attempted_cancel {
                "Shutting down...".to_owned()
            } else if state.frames_encoded < state.total_frames {
                format!("Encoding Pass {}", state.current_pass)
            } else {
                format!("Comparing Pass {}", state.current_pass)
            },
            completed_title: Self::completed_title(state, self.attempted_cancel),
            top_right_title: String::new(),
            bottom_center_title: String::new(),
            unit_per_second: "FPS".to_owned(),
            unit: "Frame".to_owned(),
            initial_completed: 0,
            completed: Self::completed(state),
            total: state.total_frames,
            show_label: true,
        };
        // A finished pass keeps its clock stopped, so the FPS it measured stays
        // on screen instead of being recomputed against the gap that follows.
        let clock = self.pass_completed.unwrap_or(self.pass_started);
        let progress_bar = progress_bar.generate(Some(clock));
        frame.render_widget(progress_bar, layout[2]);
    }
}

impl TargetQualityApp {
    /// The bar's top-left title once every frame it tracks is done.
    #[inline]
    fn completed_title(state: &TargetQualityState, attempted_cancel: bool) -> String {
        if attempted_cancel {
            "Target Quality Aborted".to_owned()
        } else {
            format!("Comparing Pass {} Complete", state.current_pass)
        }
    }

    /// The frames the progress bar measures, and therefore the count a rate
    /// would be derived from: the encode's frames while encoding, the compare's
    /// once every frame is encoded.
    #[inline]
    fn completed(state: &TargetQualityState) -> u64 {
        if state.frames_encoded < state.total_frames {
            state.frames_encoded
        } else {
            state.frames_compared
        }
    }

    /// Stops the clock once a pass has produced every frame the bar shows, and
    /// releases it again when the next pass reports progress.
    fn stop_clock_at_completion(
        snapshot: &TargetQualityState,
        pass_completed: &mut Option<std::time::Instant>,
    ) {
        if Self::completed(snapshot) >= snapshot.total_frames {
            if pass_completed.is_none() {
                *pass_completed = Some(std::time::Instant::now());
            }
        } else {
            *pass_completed = None;
        }
    }

    /// Build the initial screen, rejecting corrupt or undefined recorded passes.
    /// An empty history is a valid unmeasured screen, not a zero quality score.
    pub fn new(
        clip_info: ClipInfo,
        scenes: &[Scene<CliSequenceData>],
        encoder: Encoder,
        probe_statistic: ProbeStatistic,
        metric: &QualityMetric,
    ) -> anyhow::Result<TargetQualityApp> {
        let encoder_summary =
            representative_encoder(scenes.iter().map(|scene| &scene.encoder), &encoder).cloned();
        let quality_passes: BTreeMap<u64, Arc<Vec<QualityPass>>> = scenes
            .iter()
            .enumerate()
            .map(|(scene_index, scene)| {
                Ok((
                    scene_index as u64,
                    scene
                        .sequence_data
                        .target_quality
                        .passes
                        .iter()
                        .enumerate()
                        .map(|(pass_index, pass)| -> anyhow::Result<QualityPass> {
                            Ok(QualityPass {
                                scene: scene_index as u64,
                                current_pass: (pass_index + 1) as u8,
                                total_passes: (pass_index + 1) as u8,
                                quantizer: pass.quantizer,
                                score: metric
                                    .calculate_score(probe_statistic, &pass.scores)
                                    .with_context(|| {
                                        format!(
                                            "recorded Target Quality scene {scene_index} pass {}",
                                            pass_index + 1
                                        )
                                    })?,
                                bitrate: pass.bitrate,
                            })
                        })
                        .collect::<anyhow::Result<Vec<_>>>()?
                        .into(),
                ))
            })
            .collect::<anyhow::Result<_>>()?;
        let state = TargetQualityState {
            quality_passes: Arc::new(quality_passes),
            current_pass: 1,
            frames_encoded: 0,
            frames_compared: 0,
            total_frames: 1,
        };
        Ok(TargetQualityApp {
            encoder,
            encoder_summary,
            clip_info,
            pass_started: std::time::Instant::now(),
            pass_completed: None,
            attempted_cancel: false,
            shared_progress: SharedProgress::new(state.clone()),
            cached_state: state,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TargetQualityConsoleEvent {
    Pass(u8),
    QualityPass(QualityPass),
    EncodeProgress {
        current_pass: u8,
        total_passes: u8,
        current_frame: u64,
        total_frames: u64,
    },
    CompareProgress {
        current_pass: u8,
        total_passes: u8,
        current_frame: u64,
        total_frames: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityPass {
    pub(crate) scene: u64,
    pub(crate) current_pass: u8,
    pub(crate) total_passes: u8,
    pub(crate) quantizer: f64,
    pub(crate) score: f64,
    pub(crate) bitrate: f64,
}

#[cfg(test)]
mod tests {
    #[test]
    fn tq_frame_snapshots_share_all_display_history() {
        use crate::apps::TuiApp;
        use andean_condor::{
            core::sequence::{SequenceCompletion, SequenceStatus, Status},
            models::sequence::target_quality::types::{ProbeStatistic, QualityMetric},
        };
        let (clip, mut scenes) = crate::test_helpers::quality_scene_fixture();
        for scene in scenes.iter_mut().take(2) {
            let mut pass = scene.sequence_data.target_quality.passes[0].clone();
            pass.scores = vec![73.125; 8192];
            scene.sequence_data.target_quality.passes = vec![pass; 32];
        }
        let mut app = TargetQualityApp::new(
            clip,
            &scenes,
            Default::default(),
            ProbeStatistic::Mean,
            &QualityMetric::default(),
        )
        .unwrap();
        let initial = app.shared_progress.read();
        let owner = initial.quality_passes[&0].as_ptr();
        for completed in 1..=8 {
            app.shared_progress.apply(|state| {
                TargetQualityApp::map_progress(
                    SequenceStatus::Whole(Status::Processing {
                        id: "Encode".into(),
                        completion: SequenceCompletion::Frames {
                            completed,
                            total: 9,
                        },
                    }),
                    state,
                )
            });
            let snapshot = app.shared_progress.read_if_dirty().unwrap();
            assert_eq!(snapshot.quality_passes[&0].len(), 32);
            assert_eq!(snapshot.quality_passes[&0][31].score, 73.125);
            assert_eq!(
                snapshot.quality_passes[&0].as_ptr(),
                owner,
                "frame-only updates must not copy display history"
            );
            assert_eq!(snapshot.frames_encoded, completed);
            app.on_snapshot(snapshot);
        }
        for pass in [33, 34] {
            app.shared_progress.apply(|state| {
                TargetQualityApp::map_progress(
                    SequenceStatus::Subprocess {
                        parent: Status::Processing {
                            id: "Target Quality".into(),
                            completion: SequenceCompletion::Passes {
                                completed: pass,
                                total: 40,
                            },
                        },
                        child: Status::Processing {
                            id: "Quality".into(),
                            completion: SequenceCompletion::SceneQuality {
                                index: 1,
                                quantizer: f64::from(pass),
                                score: 90.0,
                                bitrate: 123.0,
                            },
                        },
                    },
                    state,
                )
            });
            let snapshot = app.shared_progress.read_if_dirty().unwrap();
            assert_eq!(
                snapshot.quality_passes[&0].as_ptr(),
                owner,
                "new scene results must share other histories"
            );
            assert_eq!(snapshot.quality_passes[&1].len(), usize::from(pass));
            let last = snapshot.quality_passes[&1].last().unwrap();
            assert_eq!(
                (
                    last.scene,
                    last.current_pass,
                    last.total_passes,
                    last.quantizer,
                    last.score,
                    last.bitrate
                ),
                (1, pass, 40, f64::from(pass), 90.0, 123.0)
            );
            assert_eq!(
                snapshot.quality_passes.keys().copied().collect::<Vec<_>>(),
                [0, 1, 2]
            );
            assert_eq!(
                initial.quality_passes[&1].len(),
                32,
                "cached histories stay immutable"
            );
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
        assert!(text.contains("Quantizer and Score per Scene") && text.contains("Quantizer/Score"));
    }

    #[test]
    fn tq_header_uses_actual_uniform_or_mixed_scene_settings() {
        use crate::apps::TuiApp;
        use andean_condor::models::{
            encoder::{Encoder, EncoderBase},
            sequence::target_quality::types::{ProbeStatistic, QualityMetric},
        };
        use ratatui::{Terminal, backend::TestBackend};

        let (clip, scenes) = crate::test_helpers::quality_scene_fixture();
        let global = Encoder::default_from_base(&EncoderBase::AOM, false);
        for variant in 0..4 {
            let mut scenes = scenes.clone();
            for scene in &mut scenes {
                scene.encoder = Encoder::default_from_base(&EncoderBase::X264, false);
            }
            match variant {
                1 => scenes[1].encoder = Encoder::default_from_base(&EncoderBase::X265, false),
                2 => scenes[1].encoder.set_quantizer(17.0),
                3 => scenes.clear(),
                _ => {},
            }
            let app = TargetQualityApp::new(
                clip,
                &scenes,
                global.clone(),
                ProbeStatistic::Mean,
                &QualityMetric::default(),
            )
            .unwrap();
            let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
            terminal.draw(|frame| app.render(frame)).unwrap();
            let buf = terminal.backend().buffer();
            let header = (0..8)
                .map(|y| (0..120).map(|x| buf[(x, y)].symbol()).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n");
            if variant == 3 {
                assert!(header.contains("cpu-used"), "{header}");
            } else {
                assert!(!header.contains("cpu-used"), "{header}");
                if variant == 0 {
                    assert!(header.contains("x264"), "{header}");
                    assert!(header.contains("--preset"), "{header}");
                } else {
                    assert!(header.contains("Mixed encoders"), "{header}");
                    assert!(header.contains("per-scene settings"), "{header}");
                }
            }
        }
    }

    #[test]
    fn saved_tq_rejects_corrupt_claimed_passes() {
        use andean_condor::models::sequence::target_quality::types::{
            ProbeStatistic, QualityMetric,
        };
        let (clip, scenes) = crate::test_helpers::quality_scene_fixture();
        for scores in [
            vec![],
            vec![f64::NAN],
            vec![f64::INFINITY],
            vec![f64::NEG_INFINITY],
            vec![1.0, -1.0],
        ] {
            let mut corrupt = scenes.clone();
            corrupt[1].sequence_data.target_quality.passes[0].scores = scores;
            assert!(
                TargetQualityApp::new(
                    clip,
                    &corrupt,
                    Default::default(),
                    ProbeStatistic::Harmonic,
                    &QualityMetric::default()
                )
                .is_err()
            );
        }
        assert!(
            TargetQualityApp::new(
                clip,
                &[],
                Default::default(),
                ProbeStatistic::Mean,
                &QualityMetric::default()
            )
            .expect("initial screen")
            .cached_state
            .quality_passes
            .is_empty()
        );
    }
    use super::{TargetQualityApp, TargetQualityState};

    #[test]
    fn backwards_recorded_pass_timestamps_do_not_supply_live_progress_time() {
        use std::time::{Duration, Instant};

        use andean_condor::{
            core::sequence::{SequenceCompletion, SequenceStatus, Status},
            models::sequence::target_quality::types::{ProbeStatistic, QualityMetric, QualityPass},
        };
        use ratatui::{Terminal, backend::TestBackend};

        use crate::apps::TuiApp;

        let started = Instant::now() - Duration::from_secs(10);
        for completed_on in [30_000_u128, 10_000] {
            let (clip, mut scenes) = crate::test_helpers::quality_scene_fixture();
            for scene in &mut scenes {
                for pass in &mut scene.sequence_data.target_quality.passes {
                    pass.started_on = 20_000;
                    pass.completed_on = completed_on;
                    let json = serde_json::to_value(&pass).expect("saved probe pass");
                    assert_eq!(
                        json,
                        serde_json::json!({
                            "provenance": null,
                            "backend": null,
                            "quantizer": pass.quantizer,
                            "scores": pass.scores,
                            "bitrate": 123.0,
                            "started_on": 20_000,
                            "completed_on": completed_on
                        })
                    );
                    let restored: QualityPass =
                        serde_json::from_value(json).expect("restore saved probe pass");
                    assert_eq!(
                        (restored.started_on, restored.completed_on),
                        (20_000, completed_on)
                    );
                    *pass = restored;
                }
            }
            let mut app = TargetQualityApp::new(
                clip,
                &scenes,
                Default::default(),
                ProbeStatistic::Mean,
                &QualityMetric::default(),
            )
            .expect("recorded scores remain valid after wall-clock rollback");
            let mut snapshot = app.cached_state.clone();
            assert!(TargetQualityApp::map_progress(
                SequenceStatus::Whole(Status::Processing {
                    id: "Encode".into(),
                    completion: SequenceCompletion::Frames {
                        completed: 5,
                        total: 9,
                    },
                }),
                &mut snapshot,
            ));
            app.on_snapshot(snapshot);
            app.pass_started = started;

            // Compare the live bar's inputs, leaving the separate completed-pass clock contract alone.
            assert_eq!(
                (
                    app.pass_started,
                    app.pass_completed,
                    TargetQualityApp::completed(&app.cached_state),
                    app.cached_state.total_frames,
                ),
                (started, None, 5, 9)
            );
            let recorded = &app.cached_state.quality_passes[&0][0];
            assert_eq!(
                (recorded.quantizer, recorded.score, recorded.bitrate),
                (20.0, 5.0, 123.0)
            );
            let mut terminal = Terminal::new(TestBackend::new(120, 40)).expect("terminal");
            terminal.draw(|frame| app.render(frame)).expect("render");
            let text = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            let fps = text
                .split_once("5/9 Frames (")
                .expect("live probe progress label")
                .1
                .split_once("FPS)")
                .expect("FPS label")
                .0
                .parse::<f64>()
                .expect("numeric FPS");
            assert!(fps.is_finite() && fps > 0.0, "{text}");
        }
    }

    #[test]
    fn saved_tq_constructor_and_live_mapper_use_actual_metric() {
        use crate::apps::TuiApp;
        use andean_condor::{
            core::sequence::{SequenceCompletion, SequenceStatus, Status},
            models::sequence::target_quality::types::{ProbeStatistic, QualityMetric},
        };
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
                let app =
                    TargetQualityApp::new(clip, &scenes, Default::default(), statistic, &metric)
                        .expect("app");
                let expected = if disabled == Some(true) {
                    independent
                } else {
                    [4.0, 6.0]
                };
                let mut live =
                    TargetQualityApp::new(clip, &[], Default::default(), statistic, &metric)
                        .expect("app")
                        .cached_state;
                for (index, score) in expected.into_iter().enumerate() {
                    let passes = &app.cached_state.quality_passes[&(index as u64)];
                    assert_eq!(
                        passes
                            .iter()
                            .map(|p| (p.quantizer, p.score, p.bitrate))
                            .collect::<Vec<_>>(),
                        [(20.0, score, 123.0), (30.0, score, 123.0)]
                    );
                    assert!(TargetQualityApp::map_progress(
                        SequenceStatus::Subprocess {
                            parent: Status::Processing {
                                id: "Target Quality".to_owned(),
                                completion: SequenceCompletion::Passes {
                                    completed: 2,
                                    total: 4
                                }
                            },
                            child: Status::Processing {
                                id: "Quality".to_owned(),
                                completion: SequenceCompletion::SceneQuality {
                                    index: index as u64,
                                    quantizer: 30.0,
                                    score,
                                    bitrate: 123.0
                                }
                            }
                        },
                        &mut live
                    ));
                    assert_eq!(
                        live.quality_passes[&(index as u64)][0].score,
                        passes[1].score
                    );
                }
                assert!(app.cached_state.quality_passes[&2].is_empty());
                let control = TargetQualityApp::new(
                    clip,
                    &scenes,
                    Default::default(),
                    statistic,
                    &QualityMetric::default(),
                )
                .expect("app");
                assert_eq!(
                    control.cached_state.quality_passes[&0][0].score,
                    independent[0]
                );
            }
        }
        assert_eq!(
            scenes[0].sequence_data.target_quality.passes[0].scores,
            [2.0, 9.0, 4.0]
        );
    }

    const TOTAL: u64 = 14386;

    fn state(current_pass: u8, frames_encoded: u64, frames_compared: u64) -> TargetQualityState {
        TargetQualityState {
            quality_passes: Default::default(),
            current_pass,
            frames_encoded,
            frames_compared,
            total_frames: TOTAL,
        }
    }

    /// A pass that has compared every frame is finished, so its clock stops
    /// and the FPS it measured is what stays on screen.
    #[test]
    fn a_finished_pass_stops_the_clock() {
        let mut pass_completed = None;

        TargetQualityApp::stop_clock_at_completion(&state(2, TOTAL, TOTAL), &mut pass_completed);

        assert!(
            pass_completed.is_some(),
            "a pass with every frame compared has finished"
        );
    }

    /// Frames still arriving mean the pass is running: the clock belongs to it
    /// and must keep ticking.
    #[test]
    fn frames_still_coming_in_keep_the_clock_running() {
        let mut pass_completed = None;

        TargetQualityApp::stop_clock_at_completion(&state(2, 6040, 0), &mut pass_completed);
        assert!(
            pass_completed.is_none(),
            "an encoding pass is still running"
        );

        TargetQualityApp::stop_clock_at_completion(&state(2, TOTAL, 7190), &mut pass_completed);
        assert!(
            pass_completed.is_none(),
            "a comparing pass is still running"
        );
    }

    /// The snapshot that announces the next pass arrives before any of its
    /// frames, so the counters still read the finished pass. The clock must
    /// stay stopped through it — this is the snapshot the FPS used to spike on.
    #[test]
    fn the_next_pass_announcement_leaves_the_clock_stopped() {
        let mut pass_completed = None;
        TargetQualityApp::stop_clock_at_completion(&state(2, TOTAL, TOTAL), &mut pass_completed);
        let stopped = pass_completed.expect("the finished pass stops the clock");

        TargetQualityApp::stop_clock_at_completion(&state(3, TOTAL, TOTAL), &mut pass_completed);

        assert_eq!(
            pass_completed,
            Some(stopped),
            "no frames of the next pass have been reported yet"
        );
    }

    /// The first frames of the next pass hand the clock back to it.
    #[test]
    fn the_next_pass_resumes_the_clock() {
        let mut pass_completed = None;
        TargetQualityApp::stop_clock_at_completion(&state(2, TOTAL, TOTAL), &mut pass_completed);

        TargetQualityApp::stop_clock_at_completion(&state(3, 1, 0), &mut pass_completed);

        assert!(
            pass_completed.is_none(),
            "the next pass is encoding, so its own clock runs"
        );
    }

    /// A finished compare ends a probe pass, and another pass usually follows,
    /// so the title names that pass instead of claiming the whole run is done.
    #[test]
    fn a_finished_compare_titles_its_own_pass() {
        assert_eq!(
            TargetQualityApp::completed_title(&state(2, TOTAL, TOTAL), false),
            "Comparing Pass 2 Complete"
        );
    }

    /// Cancelling takes over the title, whatever the pass counters say.
    #[test]
    fn target_quality_cancellation_displays_aborted_title() {
        assert_eq!(
            TargetQualityApp::completed_title(&state(2, TOTAL, TOTAL), true),
            "Target Quality Aborted"
        );
    }
}
