use std::{collections::BTreeMap, fmt::Write as _, io::IsTerminal, sync::Arc};

use andean_condor::{
    core::{
        encoder::EncodeResourceUsage,
        input::clip_info::ClipInfo,
        sequence::{SequenceCompletion, SequenceStatus, Status},
    },
    models::{
        encoder::{Encoder, EncoderPasses},
        scene::Scene,
    },
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::Color,
    text::Line,
    widgets::{Block, Paragraph},
};
use serde::{Deserialize, Serialize};

use crate::{
    apps::{SharedProgress, TuiApp},
    components::{
        active_encoders::ActiveEncoders,
        encoder_info::{EncoderInfo, representative_encoder},
        input_info::InputInfo,
        progress_bar::ProgressBar,
    },
    configuration::CliSequenceData,
};

#[derive(Clone)]
pub struct ParallelEncoderState {
    pub scenes: BTreeMap<u64, (u64, SceneDisplay)>,
    pub active_encoders: BTreeMap<u64, SceneEncoder>,
    pub clip_info: ClipInfo,
    pub completed_scenes_count: usize,
    pub estimated_bitrate: f64,
    pub estimated_bytes: u64,
    pub validation: Option<(u64, u64)>,
    pub initial_frames: u64,
    pub completed: bool,
}

/// Rendering and validated-size inputs only; never retains sequence results or zoning proofs.
#[derive(Debug, Clone)]
pub struct SceneDisplay {
    pub start_frame: usize,
    pub end_frame: usize,
    pub encoder: Arc<Encoder>,
    pub bytes: Option<u64>,
}

impl From<&Scene<CliSequenceData>> for SceneDisplay {
    fn from(scene: &Scene<CliSequenceData>) -> Self {
        Self {
            start_frame: scene.start_frame,
            end_frame: scene.end_frame,
            encoder: Arc::new(scene.encoder.clone()),
            // Historical bytes cannot authorize progress before runtime validation.
            bytes: None,
        }
    }
}

pub struct ParallelEncoderApp {
    pub started: std::time::Instant,
    pub workers: u8,
    pub encoder: Encoder,
    pub total_frames: u64,
    pub clip_info: ClipInfo,
    attempted_cancel: bool,
    shared_progress: SharedProgress<ParallelEncoderState>,
    cached_state: ParallelEncoderState,
}

impl TuiApp for ParallelEncoderApp {
    type State = ParallelEncoderState;

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
            SequenceStatus::Whole(status) => {
                if matches!(status, Status::Completed { .. }) {
                    state.completed = true;
                    return true;
                }
                if let Status::Processing {
                    completion:
                        SequenceCompletion::Custom {
                            name,
                            completed,
                            total,
                        },
                    ..
                } = &status
                    && name == "validating"
                {
                    state.validation =
                        (*completed < *total).then_some((*completed as u64, *total as u64));
                    state.completed = false;
                    return true;
                }
                if let Status::Processing { id, completion } = status
                    && let SequenceCompletion::Custom {
                        name, completed, ..
                    } = completion
                {
                    if !matches!(name.as_str(), "attempt" | "size" | "resumed-size") {
                        return false;
                    }
                    let scene = id.parse::<u64>().expect("Scene index is a number");
                    if name == "attempt" {
                        state.completed = false;
                        state.active_encoders.remove(&scene);
                        if let Some((frames, data)) = state.scenes.get_mut(&scene)
                            && data.bytes.is_none()
                        {
                            *frames = 0;
                        }
                        return true;
                    }
                    state.validation = None;
                    let bytes = completed as u64;
                    if bytes == 0 {
                        return false;
                    }
                    if name == "resumed-size"
                        && let Some((_, data)) = state.scenes.get(&scene)
                        && data.bytes.is_none()
                    {
                        state.initial_frames += (data.end_frame - data.start_frame) as u64;
                    }
                    state.scenes.entry(scene).and_modify(|(completed, scene)| {
                        *completed = (scene.end_frame - scene.start_frame) as u64;
                        scene.bytes = Some(bytes);
                    });
                    state.completed_scenes_count = state
                        .scenes
                        .iter()
                        .filter(|(_, (_, s))| s.bytes.is_some_and(|b| b > 0))
                        .count();
                    let (bitrate, estimated_bytes) =
                        Self::estimate_size(&state.scenes, &state.clip_info);
                    state.estimated_bitrate = bitrate;
                    state.estimated_bytes = estimated_bytes;
                    state.active_encoders.remove(&scene);
                    if !std::io::stdout().is_terminal() {
                        let event = ParallelEncoderConsoleEvent::SceneSize {
                            scene_index: scene,
                            bytes,
                        };
                        println!(
                            "[Parallel Encoder][Scene Size] {}",
                            serde_json::to_string(&event).unwrap()
                        );
                    }
                    return true;
                }
                false
            },
            SequenceStatus::Subprocess { parent: _, child } => match child {
                Status::Processing {
                    id,
                    completion: SequenceCompletion::PassFrames { passes, frames },
                } => {
                    let scene = id.parse::<u64>().expect("Scene index is a number");
                    let Some((completed, scene_data)) = state.scenes.get_mut(&scene) else {
                        return false;
                    };
                    let (current_pass, total_passes) = passes;
                    if current_pass == 0
                        || current_pass > total_passes
                        || scene_data.bytes.is_some()
                    {
                        return false;
                    }
                    let total_frames = (scene_data.end_frame - scene_data.start_frame) as u64;
                    let current_frame = frames.0.min(total_frames);
                    let active =
                        state.active_encoders.entry(scene).or_insert_with(|| SceneEncoder {
                            encoder: Arc::clone(&scene_data.encoder),
                            started: std::time::Instant::now(),
                            current_pass,
                            total_passes,
                            frames_processed: 0,
                            total_frames,
                            usage: EncodeResourceUsage::default(),
                        });
                    if total_passes != active.total_passes || current_pass < active.current_pass {
                        return false;
                    }
                    if current_pass > active.current_pass {
                        active.current_pass = current_pass;
                        active.frames_processed = 0;
                        active.started = std::time::Instant::now();
                    }
                    active.frames_processed = active.frames_processed.max(current_frame);
                    let selected_pass = match scene_data.encoder.passes() {
                        EncoderPasses::All(total) => total,
                        EncoderPasses::Specific(selected, _) => selected,
                    };
                    if current_pass == selected_pass {
                        *completed = (*completed).max(active.frames_processed);
                    }
                    true
                },
                Status::Processing {
                    id,
                    completion: SequenceCompletion::ResourceUsage { cpu, memory },
                } => {
                    let scene = id.parse::<u64>().expect("Scene index is a number");
                    let Some(active) = state.active_encoders.get_mut(&scene) else {
                        return false;
                    };
                    active.usage = EncodeResourceUsage { cpu, memory };
                    true
                }
                Status::Completed { id } => {
                    let scene = id.parse::<u64>().expect("Scene index is a number");
                    state.scenes.entry(scene).and_modify(|(completed, scene)| {
                        if scene.bytes.is_some() {
                            *completed = (scene.end_frame - scene.start_frame) as u64;
                        }
                    });
                    state.active_encoders.remove(&scene);
                    state.completed_scenes_count = state
                        .scenes
                        .iter()
                        .filter(|(_, (_, s))| s.bytes.is_some_and(|b| b > 0))
                        .count();
                    true
                },
                Status::Failed { id, .. } => {
                    let scene = id.parse::<u64>().expect("Scene index is a number");
                    state.active_encoders.remove(&scene);
                    if let Some((completed, scene)) = state.scenes.get_mut(&scene)
                        && scene.bytes.is_none()
                    {
                        *completed = 0;
                    }
                    true
                },
                _ => false,
            },
        }
    }

    fn progress_snapshot(
        &self,
        elapsed: std::time::Duration,
    ) -> Option<super::progress::ProgressSnapshot> {
        let state = &self.cached_state;
        let mut detail = format!(
            "{}/{} scenes{}",
            state.completed_scenes_count,
            state.scenes.len(),
            if state.estimated_bytes > 0 {
                format!(
                    ", {:.1} kbps, est. video {:.1} MB",
                    state.estimated_bitrate / 1000.0,
                    state.estimated_bytes as f64 / 1e6
                )
            } else {
                String::new()
            }
        );
        if let Some((done, total)) = state.validation {
            detail = format!("{done}/{total} chunks validated, {detail}");
        }
        let usage = state
            .active_encoders
            .values()
            .map(|active| active.usage)
            .fold(EncodeResourceUsage::default(), |mut total, usage| {
                total.cpu += usage.cpu;
                total.memory += usage.memory;
                total
            });
        if let Some(usage) = crate::components::usage::describe(usage) {
            detail.push_str(", ");
            detail.push_str(&usage);
        }
        Some(super::progress::ProgressSnapshot {
            stage: "Encoding",
            phase: state.validation.map_or_else(String::new, |_| "validating".into()),
            completed: state.scenes.values().map(|(done, _)| *done).sum(),
            total: Some(self.total_frames),
            initial: state.initial_frames,
            elapsed,
            detail,
            ..Default::default()
        })
    }

    fn stage_report(&self, elapsed: std::time::Duration) -> Option<super::progress::StageReport> {
        let mut progress = self.progress_snapshot(elapsed)?;
        let (frames, bytes) = self
            .cached_state
            .scenes
            .values()
            .filter(|(_, scene)| scene.bytes.is_some())
            .fold((0u64, 0u64), |(frames, bytes), (_, scene)| {
                (
                    frames + (scene.end_frame - scene.start_frame) as u64,
                    bytes + scene.bytes.unwrap_or(0),
                )
            });
        progress.completed = frames;
        progress.phase.clear();
        progress.detail = format!(
            "{}/{} scenes",
            self.cached_state.completed_scenes_count,
            self.cached_state.scenes.len()
        );
        if frames > 0 {
            let fps = *self.clip_info.frame_rate.numer() as f64
                / *self.clip_info.frame_rate.denom() as f64;
            let _ = write!(
                progress.detail,
                ", video {:.1} MB, {:.1} kbps",
                bytes as f64 / 1e6,
                bytes as f64 * 8.0 * fps / frames as f64 / 1000.0
            );
        }
        Some(super::progress::StageReport { progress, elapsed })
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

        let total_frames_completed: u64 =
            self.cached_state.scenes.iter().map(|(_, (completed, _))| completed).sum();
        let total_frames = self.total_frames;
        let actual_encoder = representative_encoder(
            self.cached_state.scenes.values().map(|(_, scene)| scene.encoder.as_ref()),
            &self.encoder,
        );
        let top_info = Block::bordered()
            .border_type(ratatui::widgets::BorderType::Rounded)
            .title(Line::from("Input").centered())
            .title_bottom(
                Line::from(
                    actual_encoder
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
        if let Some(encoder) = actual_encoder {
            let encoder_info = EncoderInfo::new(encoder.clone(), None);
            frame.render_widget(encoder_info.generate(false), top_info_areas[1]);
        } else {
            frame.render_widget(
                Paragraph::new("Mixed encoders: per-scene settings").centered(),
                top_info_areas[1],
            );
        }

        let active_encoders = ActiveEncoders::new(
            main,
            self.workers,
            actual_encoder.cloned(),
            &self.cached_state.active_encoders,
        );
        frame.render_widget(active_encoders, layout[1]);

        let scenes_completed = self.cached_state.completed_scenes_count;
        let progress_bar = ProgressBar {
            color: main,
            processing_title: if self.attempted_cancel {
                "Shutting down after Encoders complete...".to_owned()
            } else if let Some((completed, total)) = self.cached_state.validation {
                format!("Validating completed chunks ({completed}/{total})")
            } else {
                "Encoding Scenes...".to_owned()
            },
            completed_title: if self.attempted_cancel {
                "Encoding Aborted".to_owned()
            } else if self.cached_state.completed {
                "Encoding Completed".to_owned()
            } else {
                "Encoding Scenes...".to_owned()
            },
            top_right_title: if self.cached_state.estimated_bytes > 0 {
                format!(
                    "{:.1}kbps est. {:.1} MB",
                    self.cached_state.estimated_bitrate / 1e3,
                    self.cached_state.estimated_bytes as f64 / 1e6
                )
            } else {
                String::new()
            },
            bottom_center_title: format!(
                "{}/{} Scenes",
                scenes_completed,
                self.cached_state.scenes.len()
            ),
            unit_per_second: "FPS".to_owned(),
            unit: "Frame".to_owned(),
            initial_completed: self.cached_state.initial_frames,
            completed: total_frames_completed,
            total: total_frames,
            show_label: true,
        };
        let progress_bar = progress_bar.generate(Some(self.started));
        frame.render_widget(progress_bar, layout[2]);
    }
}

impl ParallelEncoderApp {
    pub fn new(
        workers: u8,
        encoder: Encoder,
        mut scenes: BTreeMap<u64, (u64, SceneDisplay)>,
        clip_info: ClipInfo,
    ) -> ParallelEncoderApp {
        // Saved bytes are historical, not evidence about the current file.
        // The sequence emits size only after validating each resumed/new chunk.
        for (completed, scene) in scenes.values_mut() {
            *completed = 0;
            scene.bytes = None;
        }
        let total_frames =
            scenes.iter().map(|(_, (_, s))| (s.end_frame - s.start_frame) as u64).sum();

        let completed_scenes_count =
            scenes.iter().filter(|(_, (_, s))| s.bytes.is_some_and(|b| b > 0)).count();
        let (estimated_bitrate, estimated_bytes) = Self::estimate_size(&scenes, &clip_info);
        let initial_state = ParallelEncoderState {
            scenes,
            active_encoders: BTreeMap::new(),
            clip_info,
            completed_scenes_count,
            estimated_bitrate,
            estimated_bytes,
            validation: None,
            initial_frames: 0,
            completed: false,
        };

        ParallelEncoderApp {
            started: std::time::Instant::now(),
            workers,
            encoder,
            total_frames,
            clip_info: initial_state.clip_info,
            attempted_cancel: false,
            shared_progress: SharedProgress::new(initial_state.clone()),
            cached_state: initial_state,
        }
    }

    pub fn estimate_size(
        scenes: &BTreeMap<u64, (u64, SceneDisplay)>,
        clip_info: &ClipInfo,
    ) -> (f64, u64) {
        let total_frames: usize =
            scenes.values().map(|(_, scene)| scene.end_frame - scene.start_frame).sum();
        let (frames_completed, bytes_completed) =
            scenes.iter().filter(|(_, (_, scene))| scene.bytes.is_some()).fold(
                (0, 0),
                |(frames_completed, bytes_completed), (_, (_, scene))| {
                    (
                        frames_completed + (scene.end_frame - scene.start_frame) as u64,
                        bytes_completed + scene.bytes.unwrap_or(0),
                    )
                },
            );
        if frames_completed == 0 {
            return (0.0, 0);
        }
        let framerate = *clip_info.frame_rate.numer() as f64 / *clip_info.frame_rate.denom() as f64;
        let seconds = frames_completed as f64 / framerate;
        let total_seconds = total_frames as f64 / framerate;
        let bitrate = (bytes_completed * 8) as f64 / seconds;
        let estimated_bytes = ((bitrate * total_seconds) / 8.0) as u64;
        (bitrate, estimated_bytes)
    }
}

#[derive(Debug, Clone)]
pub struct SceneEncoder {
    pub encoder: Arc<Encoder>,
    pub started: std::time::Instant,
    pub current_pass: u8,
    pub total_passes: u8,
    pub frames_processed: u64,
    pub total_frames: u64,
    /// Latest sample from this scene's encoder process; default means unsampled.
    pub usage: EncodeResourceUsage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ParallelEncoderConsoleEvent {
    Progress {
        scene: SceneProgress,
        current_frame: u64,
        total_frames: u64,
    },
    NewScene {
        scene_index: u64,
        /// Must be milliseconds since UNIX Epoch
        time: u128,
    },
    SceneSize {
        scene_index: u64,
        bytes: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneProgress {
    pub index: u64,
    current_pass: u8,
    total_passes: u8,
    current_frame: u64,
    total_frames: u64,
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, path::PathBuf};

    use andean_condor::{
        core::{
            input::{
                clip_info::{ClipInfo, TransferCharacteristic},
                pixel_format::PixelFormat,
            },
            sequence::{SequenceCompletion, SequenceStatus, Status},
        },
        ffmpeg::FFPixelFormat,
        models::{
            encoder::{Encoder, EncoderBase},
            scene::Scene,
        },
    };
    use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};

    use super::{ParallelEncoderApp, SceneEncoder};
    use crate::{apps::TuiApp, configuration::CliSequenceData, test_helpers::TestVideo};

    const FRAMES: usize = 400;

    #[test]
    fn progress_report_counts_only_validated_outputs_and_selected_duration() {
        let mut selected = scenes();
        selected.retain(|index, _| *index < 2);
        let mut app = app(1, encoder(), selected, clip_info());
        let mut state = app.shared_progress.read();
        ParallelEncoderApp::map_progress(custom(0, "resumed-size", 1000.0), &mut state);
        ParallelEncoderApp::map_progress(progress(1, (1, 1), 80), &mut state);
        app.on_snapshot(state.clone());
        let live = app.progress_snapshot(std::time::Duration::from_secs(10)).unwrap();
        assert_eq!(
            (live.completed, live.initial, live.total),
            (180, 100, Some(200))
        );
        assert_eq!(state.estimated_bytes, 2000);
        let report = app.stage_report(std::time::Duration::from_secs(10)).unwrap();
        assert_eq!(
            report.progress.completed, 100,
            "unvalidated frames are not final output"
        );
        ParallelEncoderApp::map_progress(custom(1, "attempt", 0.0), &mut state);
        app.on_snapshot(state);
        assert_eq!(
            app.progress_snapshot(std::time::Duration::from_secs(10)).unwrap().completed,
            100
        );
    }

    #[test]
    fn resumed_validation_counts_do_not_bypass_plain_output_throttle() {
        let mut app = app(1, encoder(), scenes(), clip_info());
        let mut presenter = super::super::progress::Presenter::new(false);
        let mut lines = 0;
        for completed in 1..=3 {
            app.cached_state.validation = Some((completed, 4));
            let elapsed = std::time::Duration::from_millis(completed * 100);
            let snapshot = app.progress_snapshot(elapsed).unwrap();
            if presenter.update(&snapshot, None, elapsed).is_some() {
                lines += 1;
            }
        }
        assert_eq!(lines, 1);
    }

    #[test]
    fn parallel_snapshot_drops_raw_scene_payloads() {
        let mut input = scenes();
        let (_, quality) = crate::test_helpers::quality_scene_fixture();
        for (_, scene) in input.values_mut() {
            scene.sequence_data.target_quality = quality[0].sequence_data.target_quality.clone();
            for pass in &mut scene.sequence_data.target_quality.passes {
                pass.scores = vec![73.125; 8192];
            }
            scene.sequence_data.quality_check.quality.scores = vec![73.125; 8192];
        }
        let template = input[&0].1.clone();
        input.extend((4..64).map(|index| (index, (0, template.clone()))));
        let definition = crate::zoner::SceneDefinition::from(&input[&0].1);
        input.get_mut(&0).unwrap().1.sequence_data.zoner.lifecycle =
            Some(crate::zoner::ZoneLifecycle {
                algorithm_revision: 1,
                baseline: vec![definition.clone(); 128],
                applied: vec![definition; 128],
                zones: vec![],
            });
        let app = app(2, encoder(), input, clip_info());
        app.shared_progress
            .apply(|state| ParallelEncoderApp::map_progress(progress(0, (1, 1), 30), state));
        let snapshot = app.shared_progress.read_if_dirty().unwrap();
        assert_eq!(snapshot.scenes.len(), 64);
        assert_eq!(snapshot.scenes[&0].0, 30);
        let retained = format!("{:?} {:?}", snapshot.scenes, snapshot.active_encoders);
        for payload in ["73.125", "scores:", "sequence_data:", "lifecycle:", "sub_scenes:"] {
            assert!(
                !retained.contains(payload),
                "render snapshot retains {payload}"
            );
        }
        assert_eq!(snapshot.active_encoders[&0].frames_processed, 30);
    }

    fn encoder() -> Encoder {
        Encoder::default_from_base(&EncoderBase::SVTAV1, false)
    }

    fn app(
        workers: u8,
        encoder: Encoder,
        scenes: BTreeMap<u64, (u64, Scene<CliSequenceData>)>,
        clip: ClipInfo,
    ) -> ParallelEncoderApp {
        ParallelEncoderApp::new(
            workers,
            encoder,
            scenes
                .into_iter()
                .map(|(index, (frames, scene))| {
                    (index, (frames, super::SceneDisplay::from(&scene)))
                })
                .collect(),
            clip,
        )
    }

    fn clip_info() -> ClipInfo {
        ClipInfo {
            num_frames: FRAMES,
            format_info: PixelFormat::VapourSynth { bit_depth: 10 },
            frame_rate: av_format::rational::Rational64::new(24000, 1001),
            resolution: (1920, 1080),
            color_range: None,
            color_primaries: None,
            color_transfer_characteristic: Some(TransferCharacteristic::SMPTE2084),
            color_space: None,
            chroma_location: None,
            mastering_display: None,
            content_light_level: None,
        }
    }

    fn scenes() -> BTreeMap<u64, (u64, Scene<CliSequenceData>)> {
        let video = TestVideo {
            path: PathBuf::new(),
            width: 1920,
            height: 1080,
            frames: FRAMES,
            fps_rational: (24000, 1001),
            format: FFPixelFormat::YUV420P,
            scenes: vec![(0, 100), (100, 200), (200, 300), (300, FRAMES)],
        };
        video
            .mock_scenes(&encoder())
            .into_iter()
            .enumerate()
            .map(|(index, scene)| (index as u64, (0, scene)))
            .collect()
    }

    fn progress(scene: u64, passes: (u8, u8), frame: u64) -> SequenceStatus {
        SequenceStatus::Subprocess {
            parent: Status::Processing {
                id: "Parallel Encoder".into(),
                completion: SequenceCompletion::Frames {
                    completed: 0,
                    total: 400,
                },
            },
            child: Status::Processing {
                id: scene.to_string(),
                completion: SequenceCompletion::PassFrames {
                    passes,
                    frames: (frame, 100),
                },
            },
        }
    }

    fn usage(scene: u64, cpu: f32, memory: u64) -> SequenceStatus {
        SequenceStatus::Subprocess {
            parent: Status::Processing {
                id: "Parallel Encoder".into(),
                completion: SequenceCompletion::Frames {
                    completed: 0,
                    total: 400,
                },
            },
            child: Status::Processing {
                id: scene.to_string(),
                completion: SequenceCompletion::ResourceUsage { cpu, memory },
            },
        }
    }

    fn custom(scene: u64, name: &str, completed: f64) -> SequenceStatus {
        SequenceStatus::Whole(Status::Processing {
            id: scene.to_string(),
            completion: SequenceCompletion::Custom {
                name: name.into(),
                completed,
                total: completed,
            },
        })
    }

    fn screen(app: &ParallelEncoderApp) -> String {
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).expect("terminal");
        terminal.draw(|frame| app.render(frame)).expect("render");
        let buf = terminal.backend().buffer();
        (0..40)
            .map(|y| (0..120).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn parallel_header_uses_actual_uniform_or_mixed_scene_settings() {
        let global = Encoder::default_from_base(&EncoderBase::AOM, false);
        let uniform = app(2, global.clone(), scenes(), clip_info());
        let header = screen(&uniform).lines().take(8).collect::<Vec<_>>().join("\n");
        assert!(
            header.contains("Scalable Video Technology for AV1"),
            "{header}"
        );
        assert!(
            !header.contains("cpu-used"),
            "global parameters leaked: {header}"
        );
        for difference in 0..5 {
            let mut scenes = scenes();
            let scene = &mut scenes.get_mut(&1).unwrap().1;
            match difference {
                0 => scene.encoder = Encoder::default_from_base(&EncoderBase::X264, false),
                1 => scene.encoder.set_quantizer(17.0),
                2 => scene.encoder.set_executable(Some("custom-svt".into())),
                3 => scene.encoder.set_append_color_data(false),
                _ => scene.encoder.set_ffmpeg_filter(Some("hflip".into())),
            }
            let app = app(2, global.clone(), scenes, clip_info());
            let header = screen(&app).lines().take(8).collect::<Vec<_>>().join("\n");
            assert!(header.contains("Mixed encoders"), "{header}");
            assert!(header.contains("per-scene settings"), "{header}");
            assert!(!header.contains("cpu-used"), "{header}");
        }
        let empty = app(2, global, BTreeMap::new(), clip_info());
        assert!(
            screen(&empty).contains("cpu-used"),
            "empty scenes retain the fallback"
        );
    }

    #[test]
    fn active_worker_panels_name_actual_codecs_and_keep_cross_codec_parameters() {
        let mut scene_map = scenes();
        for (index, base) in [(0, EncoderBase::X264), (1, EncoderBase::X265)] {
            scene_map.get_mut(&index).unwrap().1.encoder = Encoder::default_from_base(&base, false);
        }
        let mut app = app(
            2,
            Encoder::default_from_base(&EncoderBase::AOM, false),
            scene_map,
            clip_info(),
        );
        for index in [0, 1] {
            ParallelEncoderApp::map_progress(progress(index, (1, 1), 10), &mut app.cached_state);
        }
        let body = screen(&app).lines().skip(8).collect::<Vec<_>>().join("\n");
        for name in ["x264", "x265"] {
            assert!(body.contains(name), "worker codec missing: {body}");
        }
        assert_eq!(body.matches("--preset").count(), 2, "{body}");
    }

    #[test]
    fn live_usage_is_shown_per_worker_and_unsampled_values_stay_hidden() {
        const MIB: u64 = 1024 * 1024;
        let mut app = app(2, encoder(), scenes(), clip_info());
        for index in [0, 1] {
            ParallelEncoderApp::map_progress(progress(index, (1, 1), 10), &mut app.cached_state);
        }
        ParallelEncoderApp::map_progress(usage(0, 412.0, 3 * 1024 * MIB), &mut app.cached_state);
        ParallelEncoderApp::map_progress(usage(1, 100.0, 512 * MIB), &mut app.cached_state);

        let body = screen(&app).lines().skip(8).collect::<Vec<_>>().join("\n");
        assert!(body.contains("CPU 412%"), "worker cpu missing: {body}");
        assert!(body.contains("3.0 GiB"), "worker memory missing: {body}");
        assert!(body.contains("512 MiB"), "peer memory missing: {body}");

        let detail = app
            .progress_snapshot(std::time::Duration::from_secs(1))
            .expect("snapshot")
            .detail;
        assert!(detail.contains("CPU 512%"), "total cpu missing: {detail}");
        assert!(detail.contains("3.5 GiB"), "total memory missing: {detail}");

        for index in [0, 1] {
            ParallelEncoderApp::map_progress(usage(index, 0.0, 0), &mut app.cached_state);
        }
        let body = screen(&app).lines().skip(8).collect::<Vec<_>>().join("\n");
        assert!(
            !body.contains("CPU 0%") && !body.contains("0 MiB"),
            "an unsampled process must render nothing, not zero: {body}"
        );
        let detail = app
            .progress_snapshot(std::time::Duration::from_secs(1))
            .expect("snapshot")
            .detail;
        assert!(
            !detail.contains("CPU") && !detail.contains("MiB") && !detail.contains("GiB"),
            "an unsampled process must not appear in the summary: {detail}"
        );
    }

    #[test]
    fn mixed_same_codec_workers_show_full_parameters_without_a_visible_reference() {
        use andean_condor::models::encoder::cli_parameter::CLIParameter;

        let mut global = encoder();
        global
            .parameters_mut()
            .insert("preset".into(), CLIParameter::new_number("--", " ", 9.0));
        global
            .parameters_mut()
            .insert("lp".into(), CLIParameter::new_number("--", " ", 3.0));
        let mut scene_map = scenes();
        for (_, scene) in scene_map.values_mut() {
            scene.encoder = global.clone();
        }
        scene_map.get_mut(&0).unwrap().1.encoder.set_quantizer(17.0);
        scene_map.get_mut(&1).unwrap().1.encoder.set_quantizer(21.0);
        let mut app = app(2, global, scene_map, clip_info());
        for index in [0, 1] {
            ParallelEncoderApp::map_progress(progress(index, (1, 1), 10), &mut app.cached_state);
        }
        let screen = screen(&app);
        let header = screen.lines().take(8).collect::<Vec<_>>().join("\n");
        let workers = screen.lines().skip(8).collect::<Vec<_>>().join("\n");
        assert!(header.contains("Mixed encoders"), "{header}");
        assert!(!header.contains("--preset"), "{header}");
        for parameter in ["--preset 9", "--lp 3"] {
            assert_eq!(
                workers.matches(parameter).count(),
                2,
                "both workers need their shared settings: {workers}"
            );
        }
        for quantizer in ["--crf 17", "--crf 21"] {
            assert!(workers.contains(quantizer), "{workers}");
        }
        assert_eq!(
            workers.matches("Scalable Video Technology for AV1").count(),
            2,
            "{workers}"
        );
        assert!(workers.contains("Scene 0 Pass 1/1"), "{workers}");
        assert!(workers.contains("Scene 1 Pass 1/1"), "{workers}");
    }

    #[test]
    fn uniform_workers_diff_only_against_the_visible_actual_header() {
        use andean_condor::models::encoder::cli_parameter::CLIParameter;

        let global = encoder();
        let mut actual = global.clone();
        actual.set_quantizer(17.0);
        actual
            .parameters_mut()
            .insert("preset".into(), CLIParameter::new_number("--", " ", 9.0));
        actual
            .parameters_mut()
            .insert("lp".into(), CLIParameter::new_number("--", " ", 3.0));
        let mut scene_map = scenes();
        for (_, scene) in scene_map.values_mut() {
            scene.encoder = actual.clone();
        }
        let mut app = app(2, global, scene_map, clip_info());
        for index in [0, 1] {
            ParallelEncoderApp::map_progress(progress(index, (1, 1), 10), &mut app.cached_state);
        }
        let screen = screen(&app);
        let header = screen.lines().take(8).collect::<Vec<_>>().join("\n");
        let workers = screen.lines().skip(8).collect::<Vec<_>>().join("\n");
        assert!(
            header.contains("Scalable Video Technology for AV1"),
            "{header}"
        );
        for parameter in ["--preset 9", "--lp 3", "--crf 17"] {
            assert!(header.contains(parameter), "{header}");
            assert!(
                !workers.contains(parameter),
                "only the visible actual header is the diff reference: {workers}"
            );
        }
        assert_eq!(
            workers.matches("---").count(),
            2,
            "uniform workers stay compact: {workers}"
        );
    }

    #[test]
    fn backwards_saved_timestamps_preserve_metadata_not_live_progress_time() {
        use std::time::{Duration, Instant};

        use andean_condor::models::sequence::parallel_encoder::ParallelEncoderData;

        let started = Instant::now() - Duration::from_secs(10);
        for completed_on in [30_000_u128, 10_000] {
            let data = ParallelEncoderData {
                invalidated: false,
                provenance: None,
                verification: None,
                started_on: Some(20_000),
                completed_on: Some(completed_on),
                bytes: Some(1000),
            };
            let json = serde_json::to_value(&data).expect("saved parallel metadata");
            assert_eq!(
                json,
                serde_json::json!({
                    "invalidated": false,
                    "provenance": null,
                    "started_on": 20_000,
                    "completed_on": completed_on,
                    "bytes": 1000
                })
            );
            let restored: ParallelEncoderData =
                serde_json::from_value(json).expect("restore saved parallel metadata");
            assert_eq!(
                (restored.started_on, restored.completed_on),
                (Some(20_000), Some(completed_on))
            );
            let mut scenes = scenes();
            scenes.get_mut(&0).expect("scene").1.sequence_data.parallel_encoder = restored;
            let saved = scenes[&0].1.sequence_data.parallel_encoder.clone();
            let mut app = app(2, encoder(), scenes, clip_info());
            app.started = started;
            assert!(ParallelEncoderApp::map_progress(
                progress(0, (1, 1), 50),
                app.cached_state_mut()
            ));
            app.cached_state.active_encoders.get_mut(&0).expect("active scene").started = started;

            // Compare the real consumers' clock/work inputs, not samples of a ticking FPS.
            assert_eq!(
                (
                    app.started,
                    app.cached_state.active_encoders[&0].started,
                    app.cached_state.scenes[&0].0,
                    app.total_frames,
                    app.cached_state.initial_frames,
                ),
                (started, started, 50, 400, 0)
            );
            assert_eq!(
                (saved.started_on, saved.completed_on),
                (Some(20_000), Some(completed_on))
            );
            let text = screen(&app);
            for label in ["50/100 Frames (", "50/400 Frames ("] {
                let fps = text
                    .split_once(label)
                    .expect("worker and overall progress labels")
                    .1
                    .split_once("FPS)")
                    .expect("FPS label")
                    .0
                    .parse::<f64>()
                    .expect("numeric FPS");
                assert!(fps.is_finite() && fps > 0.0, "{text}");
            }
        }
    }

    fn check_pass_progress(phase: u8) {
        assert!((1..=3).contains(&phase));
        let mut scenes = scenes();
        *scenes.get_mut(&1).expect("scene").1.encoder.passes_mut().expect("passes") =
            andean_condor::models::encoder::EncoderPasses::All(2);
        let mut app = app(2, encoder(), scenes, clip_info());
        let state = app.cached_state_mut();
        ParallelEncoderApp::map_progress(progress(0, (1, 1), 10), state);
        assert_eq!(state.scenes[&0].0, 10, "first final-pass record counts");
        for frame in [10, 9, 30] {
            ParallelEncoderApp::map_progress(progress(0, (1, 1), frame), state);
        }
        assert_eq!(state.scenes[&0].0, 30);
        if phase == 1 {
            return;
        }
        ParallelEncoderApp::map_progress(progress(1, (1, 2), 90), state);
        assert_eq!(state.scenes[&1].0, 0);
        ParallelEncoderApp::map_progress(progress(1, (2, 2), 10), state);
        ParallelEncoderApp::map_progress(progress(1, (1, 2), 99), state);
        assert_eq!(state.active_encoders[&1].current_pass, 2);
        assert_eq!(state.active_encoders[&1].frames_processed, 10);
        if phase == 2 {
            return;
        }
        ParallelEncoderApp::map_progress(custom(1, "attempt", 2.0), state);
        ParallelEncoderApp::map_progress(progress(1, (1, 2), 2), state);
        assert_eq!(state.scenes[&1].0, 0);
        assert_eq!(state.active_encoders[&1].frames_processed, 2);
    }

    #[test]
    fn parallel_progress_first_final_pass_event_credits_scene_frames() {
        check_pass_progress(1);
    }

    #[test]
    fn parallel_progress_earlier_pass_does_not_credit_scene_or_rewind_later_worker_pass() {
        check_pass_progress(2);
    }

    #[test]
    fn parallel_progress_retry_resets_worker_frames_without_crediting_final_work() {
        check_pass_progress(3);
    }

    #[test]
    fn specific_pass_ui_uses_the_scene_model_and_preserves_native_panel() {
        use andean_condor::models::encoder::EncoderPasses;

        for (passes, native_pass, credited) in [
            (EncoderPasses::Specific(1, 2), 1, 30),
            (EncoderPasses::Specific(2, 2), 2, 30),
            (EncoderPasses::All(2), 1, 0),
            (EncoderPasses::All(2), 2, 30),
        ] {
            let mut scenes = scenes();
            *scenes.get_mut(&0).expect("scene").1.encoder.passes_mut().expect("passes") = passes;
            let mut app = app(2, encoder(), scenes, clip_info());
            for frame in [1, 10, 10, 9, 30] {
                ParallelEncoderApp::map_progress(
                    progress(0, (native_pass, 2), frame),
                    app.cached_state_mut(),
                );
            }
            assert_eq!(
                app.cached_state.scenes[&0].0, credited,
                "{passes:?}: selected logical pass"
            );
            let panel = &app.cached_state.active_encoders[&0];
            assert_eq!(
                (
                    panel.current_pass,
                    panel.total_passes,
                    panel.frames_processed
                ),
                (native_pass, 2, 30)
            );
            let text = screen(&app);
            assert!(
                text.contains(&format!("Scene 0 Pass {native_pass}/2")),
                "{text}"
            );
            assert!(!text.contains("Encoding Completed"));
            assert_eq!(app.cached_state.completed_scenes_count, 0);
        }
    }

    #[test]
    fn final_native_frame_is_not_a_completion_title() {
        let mut app = app(2, encoder(), scenes(), clip_info());
        for scene in 0..4 {
            ParallelEncoderApp::map_progress(progress(scene, (1, 1), 100), app.cached_state_mut());
        }
        let text = screen(&app);
        assert!(text.contains("Encoding Scenes..."), "{text}");
        assert!(!text.contains("Encoding Completed"), "{text}");
        assert_eq!(app.cached_state.completed_scenes_count, 0);
        assert_eq!(app.cached_state.active_encoders.len(), 4);
    }

    #[test]
    fn validated_resume_has_no_new_work_fps() {
        let mut app = app(2, encoder(), scenes(), clip_info());
        for scene in 0..4 {
            for _ in 0..2 {
                ParallelEncoderApp::map_progress(
                    custom(scene, "resumed-size", 1000.0),
                    app.cached_state_mut(),
                );
            }
        }
        let text = screen(&app);
        assert_eq!(app.cached_state.completed_scenes_count, 4);
        assert_eq!(app.cached_state.initial_frames, 400);
        assert!(text.contains("400/400 Frames (0.00FPS)"), "{text}");
        assert!(
            !text.contains("Encoding Completed"),
            "publication sizes alone do not finish the whole job"
        );
        ParallelEncoderApp::map_progress(
            SequenceStatus::Whole(Status::Completed {
                id: "Parallel Encoder".into(),
            }),
            app.cached_state_mut(),
        );
        assert!(screen(&app).contains("Encoding Completed"));
    }

    #[test]
    fn failure_cleanup_and_publication_do_not_inflate_resume_baseline() {
        let mut app = app(2, encoder(), scenes(), clip_info());
        let state = app.cached_state_mut();
        ParallelEncoderApp::map_progress(custom(0, "resumed-size", 1000.0), state);
        ParallelEncoderApp::map_progress(progress(1, (1, 1), 100), state);
        ParallelEncoderApp::map_progress(
            SequenceStatus::Subprocess {
                parent: Status::Completed {
                    id: "unused".into(),
                },
                child: Status::Failed {
                    id: "1".into(),
                    error: "unpublished".into(),
                },
            },
            state,
        );
        assert_eq!(state.scenes[&1].0, 0);
        assert!(state.active_encoders.is_empty());
        assert_eq!(state.completed_scenes_count, 1);
        for _ in 0..2 {
            ParallelEncoderApp::map_progress(custom(1, "size", 1000.0), state);
        }
        ParallelEncoderApp::map_progress(progress(1, (1, 1), 2), state);
        assert_eq!(state.scenes[&1].0, 100);
        assert_eq!(
            state.initial_frames, 100,
            "newly published size is not resume work"
        );
        assert_eq!(state.completed_scenes_count, 2);
        ParallelEncoderApp::map_progress(
            SequenceStatus::Whole(Status::Completed {
                id: "Parallel Encoder".into(),
            }),
            state,
        );
        assert!(state.completed);
    }

    #[test]
    fn zero_length_scene_render_is_finite() {
        let mut scenes = scenes();
        scenes.retain(|id, _| *id == 0);
        scenes.get_mut(&0).expect("scene").1.end_frame = 0;
        let mut app = app(1, encoder(), scenes, clip_info());
        ParallelEncoderApp::map_progress(progress(0, (1, 1), u64::MAX), app.cached_state_mut());
        assert_eq!(app.cached_state.scenes[&0].0, 0);
        let text = screen(&app);
        assert!(text.contains("0/0 Frames (0.00FPS)"), "{text}");
        assert!(!text.contains("NaN"));
        assert!(!text.contains("Encoding Completed"));
    }

    /// The size event a finished scene emits is what fills the bar: the count
    /// of finished scenes at the bottom, and the bitrate and estimated size in
    /// the corner. Without it the bar has nothing to show.
    #[test]
    fn a_finished_scene_updates_the_counter_and_the_estimate() {
        let mut app = app(2, encoder(), scenes(), clip_info());
        let state = app.cached_state_mut();
        assert_eq!(state.completed_scenes_count, 0);
        assert_eq!(state.estimated_bytes, 0);

        let event = SequenceStatus::Whole(Status::Processing {
            id: "1".to_owned(),
            completion: SequenceCompletion::Custom {
                name: "size".to_owned(),
                completed: 2_000_000.0,
                total: 2_000_000.0,
            },
        });
        assert!(ParallelEncoderApp::map_progress(event, state));

        assert_eq!(state.completed_scenes_count, 1);
        assert!(
            state.estimated_bytes > 0,
            "the estimate projects the clip from the finished scene"
        );
        assert!(state.estimated_bitrate > 0.0);
    }

    /// The counter tracks scenes with a measured size, not scenes whose
    /// encoder merely exited: a completion without a size must not count a
    /// scene as finished, or the estimate would claim bytes that do not exist.
    #[test]
    fn a_completed_scene_without_a_size_is_not_counted() {
        let mut app = app(2, encoder(), scenes(), clip_info());
        let state = app.cached_state_mut();

        let completed = SequenceStatus::Subprocess {
            parent: Status::Processing {
                id: "Parallel Encoder".to_owned(),
                completion: SequenceCompletion::Frames {
                    completed: 0,
                    total: FRAMES as u64,
                },
            },
            child: Status::Completed { id: "2".to_owned() },
        };
        assert!(ParallelEncoderApp::map_progress(completed, state));

        assert_eq!(state.completed_scenes_count, 0);
        assert_eq!(state.estimated_bytes, 0);
    }

    #[test]
    fn saved_metadata_is_not_eligible_until_runtime_size_validation() {
        let mut scenes = scenes();
        for (completed, scene) in scenes.values_mut() {
            *completed = 100;
            scene.sequence_data.parallel_encoder.bytes = Some(2_000_000);
            scene.sequence_data.parallel_encoder.started_on = Some(10);
            scene.sequence_data.parallel_encoder.completed_on = Some(20);
        }
        let mut app = app(2, encoder(), scenes, clip_info());
        let state = app.cached_state_mut();
        assert_eq!(
            state.completed_scenes_count, 0,
            "saved records alone do not validate current files"
        );
        assert_eq!(state.estimated_bytes, 0);
        assert_eq!(
            state.scenes.values().map(|(frames, _)| frames).sum::<u64>(),
            0
        );
        let event = SequenceStatus::Whole(Status::Processing {
            id: "1".to_owned(),
            completion: SequenceCompletion::Custom {
                name: "size".to_owned(),
                completed: 1000.0,
                total: 1000.0,
            },
        });
        assert!(ParallelEncoderApp::map_progress(event, state));
        assert_eq!(state.completed_scenes_count, 1);
        assert_eq!(
            state.scenes[&1].0, 100,
            "validated resume fills its frame range even without encoder progress"
        );
        assert!(state.estimated_bytes > 0);
    }

    #[test]
    fn validation_progress_renders_without_completing_scenes() {
        let mut app = app(2, encoder(), scenes(), clip_info());
        let event = SequenceStatus::Whole(Status::Processing {
            id: "Parallel Encoder".into(),
            completion: SequenceCompletion::Custom {
                name: "validating".into(),
                completed: 1.0,
                total: 4.0,
            },
        });
        assert!(ParallelEncoderApp::map_progress(
            event,
            app.cached_state_mut()
        ));
        let state = app.cached_state();
        assert_eq!(state.completed_scenes_count, 0);
        assert_eq!(
            state.scenes.values().map(|(frames, _)| frames).sum::<u64>(),
            0
        );
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).expect("terminal");
        terminal.draw(|frame| app.render(frame)).expect("validation render");
        let buf = terminal.backend().buffer();
        let text = (0..40)
            .map(|y| (0..120).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Validating completed chunks (1/4)"), "{text}");
    }

    /// The row a worker panel's title sits on, or `None` when no panel
    /// claims it.
    fn title_row(buf: &Buffer, scene: u64) -> Option<u16> {
        let needle = format!("Scene {scene} Pass");
        (0..40).find(|&y| {
            let row: String = (0..60).map(|x| buf[(x, y)].symbol()).collect();
            row.contains(&needle)
        })
    }

    /// The screen keeps one slot per *configured* worker rather than one per
    /// running encoder: with four workers and two scenes encoding, the panels
    /// take the bottom two slots of the middle region — latest worker lowest,
    /// earliest directly above — and the idle half stays blank. This is the
    /// seam the screenshot refactor broke; the widget cannot tell on its own
    /// whether the right slot count reached it.
    #[test]
    fn the_screen_reserves_a_slot_per_worker_and_fills_from_the_bottom() {
        let mut app = app(4, encoder(), scenes(), clip_info());
        let all = scenes();
        for index in [1_u64, 2] {
            let state = app.cached_state_mut();
            state.active_encoders.insert(
                index,
                SceneEncoder {
                    encoder: std::sync::Arc::new(all[&index].1.encoder.clone()),
                    started: std::time::Instant::now(),
                    current_pass: 1,
                    total_passes: 2,
                    frames_processed: 40,
                    total_frames: 100,
                    usage: andean_condor::core::encoder::EncodeResourceUsage::default(),
                },
            );
        }

        let mut terminal =
            Terminal::new(TestBackend::new(60, 40)).expect("test terminal should be created");
        terminal.draw(|frame| app.render(frame)).expect("draw should succeed");
        let buf = terminal.backend().buffer();

        assert_eq!(
            title_row(buf, 2),
            Some(29),
            "the latest worker takes the lowest slot"
        );
        assert_eq!(
            title_row(buf, 1),
            Some(22),
            "the earliest worker sits directly above the latest"
        );
        let idle_half = (8..22).all(|y| (0..60).all(|x| buf[(x, y)].symbol() == " "));
        assert!(idle_half, "the idle worker slots above stay blank");
    }
}
