use std::{
    collections::BTreeMap,
    io::IsTerminal,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
        Arc,
    },
    thread,
};

use andean_condor::{
    core::{
        input::clip_info::ClipInfo,
        sequence::{SequenceCompletion, SequenceStatus, Status},
    },
    models::{encoder::Encoder, scene::Scene},
};
use anyhow::Result;
use ratatui::{
    crossterm::event::{self, Event as TermEvent, KeyCode, KeyModifiers},
    layout::{Constraint, Layout},
    style::Color,
    text::Line,
    widgets::Block,
    Frame,
};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::{
    apps::TuiApp,
    components::{
        active_encoders::ActiveEncoders,
        encoder_info::EncoderInfo,
        input_info::InputInfo,
        progress_bar::ProgressBar,
    },
    configuration::CliSequenceData,
};

pub struct ParallelEncoderApp {
    pub(crate) original_panic_hook: Option<super::PanicHook>,
    pub started:                    std::time::Instant,
    pub workers:                    u8,
    pub encoder:                    Encoder,
    pub initial_frames:             u64,
    pub scenes:                     BTreeMap<u64, (u64, Scene<CliSequenceData>)>,
    pub active_scenes:              BTreeMap<u64, SceneEncoder>,
    pub clip_info:                  ClipInfo,
    attempted_cancel:               bool,
}

impl TuiApp for ParallelEncoderApp {
    fn original_panic_hook(&mut self) -> &mut Option<super::PanicHook> {
        &mut self.original_panic_hook
    }

    fn run(
        &mut self,
        progress_rx: Receiver<SequenceStatus>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<()> {
        let (event_tx, event_rx) = mpsc::channel();
        let input_tx = event_tx.clone();
        thread::spawn(move || loop {
            if let Ok(TermEvent::Key(key)) = event::read()
                && input_tx.send(ParallelEncoderAppEvent::Input(key)).is_err()
            {
                break;
            }
        });
        let tick_tx = event_tx.clone();
        thread::spawn(move || loop {
            if tick_tx.send(ParallelEncoderAppEvent::Tick).is_err() {
                break;
            }
            thread::sleep(std::time::Duration::from_millis(16)); // ~60 FPS
        });
        thread::spawn(move || {
            for progress in progress_rx {
                if let SequenceStatus::Subprocess {
                    parent: _,
                    child,
                } = progress
                {
                    match child {
                        Status::Processing {
                            id,
                            completion:
                                SequenceCompletion::PassFrames {
                                    passes,
                                    frames,
                                },
                        } => {
                            let (current_pass, total_passes) = passes;
                            let (current_frame, total_frames) = frames;
                            let scene_original_index =
                                id.parse::<u64>().expect("Scene index is a number");
                            let _ = event_tx.send(ParallelEncoderAppEvent::Progress {
                                scene: scene_original_index,
                                current_pass,
                                total_passes,
                                current_frame,
                                total_frames,
                            });
                        },
                        Status::Completed {
                            id,
                        } => {
                            let scene_original_index =
                                id.parse::<u64>().expect("Scene index is a number");
                            let _ = event_tx.send(ParallelEncoderAppEvent::SceneCompleted(
                                scene_original_index,
                            ));
                        },
                        _ => (),
                    }
                }
            }
            let _ = event_tx.send(ParallelEncoderAppEvent::Quit);
        });

        let stdout_is_terminal = std::io::stdout().is_terminal();
        let mut terminal = self.init()?;
        loop {
            match event_rx.recv()? {
                ParallelEncoderAppEvent::Tick => {
                    terminal.draw(|f| self.render(f))?;
                },
                ParallelEncoderAppEvent::Progress {
                    scene,
                    current_pass,
                    total_passes,
                    current_frame,
                    total_frames,
                } => {
                    let active_scene = self.active_scenes.get_mut(&scene);
                    if let Some(active_scene) = active_scene {
                        active_scene.current_pass = current_pass;
                        active_scene.total_passes = total_passes;
                        active_scene.frames_processed = current_frame;
                        active_scene.total_frames = total_frames;
                    } else {
                        let scene_encoder = SceneEncoder {
                            scene: self.scenes.get(&scene).expect("Scene exists").1.clone(),
                            started: std::time::Instant::now(),
                            current_pass,
                            total_passes,
                            frames_processed: current_frame,
                            total_frames,
                        };
                        self.active_scenes.insert(scene, scene_encoder);
                        if !stdout_is_terminal {
                            let event = ParallelEncoderConsoleEvent::NewScene {
                                scene_index: scene,
                                time:        std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .expect("Time is valid")
                                    .as_millis(),
                            };
                            println!(
                                "[Parallel Encoder][New Scene] {}",
                                serde_json::to_string(&event)?
                            );
                        }
                    }

                    if current_pass == total_passes {
                        self.scenes.entry(scene).and_modify(|(completed, _scene)| {
                            *completed = current_frame;
                        });
                        if current_frame == total_frames {
                            self.active_scenes.remove(&scene);
                        }
                    }
                    if !stdout_is_terminal {
                        let (total_frames_completed, total_frames) =
                            self.scenes.iter().fold((0, 0), |acc, (_, (completed, scene))| {
                                (
                                    acc.0 + completed,
                                    acc.1 + (scene.end_frame - scene.start_frame) as u64,
                                )
                            });
                        let event = ParallelEncoderConsoleEvent::Progress {
                            scene: SceneProgress {
                                index: scene,
                                current_pass,
                                total_passes,
                                current_frame,
                                total_frames,
                            },
                            current_frame: total_frames_completed,
                            total_frames,
                        };
                        let event = serde_json::to_string(&event).unwrap();
                        println!("[Parallel Encoder][Progress] {}", event);
                    }
                },
                ParallelEncoderAppEvent::SceneCompleted(scene) => {
                    self.scenes.entry(scene).and_modify(|(completed, scene)| {
                        *completed = (scene.end_frame - scene.start_frame) as u64;
                    });
                    self.active_scenes.remove(&scene);
                },
                ParallelEncoderAppEvent::Quit => {
                    self.restore(terminal)?;
                    break;
                },
                ParallelEncoderAppEvent::Input(key) => {
                    if key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                        // Prevents duplicate event from key release in Windows
                        && key.is_press()
                    {
                        self.attempted_cancel = true;
                        let already_cancelled = cancelled.swap(true, Ordering::SeqCst);
                        if already_cancelled {
                            self.restore(terminal)?;
                            debug!("Force quit Condor");
                            std::process::exit(0);
                        } else if !stdout_is_terminal {
                            println!(
                                "Waiting for Encoders to finish. Press Ctrl+C again to exit \
                                 immediately."
                            );
                        }
                    }
                },
            }
        }
        Ok(())
    }

    fn render(&self, frame: &mut Frame) {
        const MAIN_COLOR: Color = Color::DarkGray;
        let layout = Layout::default()
            .constraints([
                Constraint::Percentage(20),
                Constraint::Percentage(70),
                Constraint::Percentage(10),
            ])
            .split(frame.area());

        let (total_frames_completed, total_frames) =
            self.scenes.iter().fold((0, 0), |acc, (_, (completed, scene))| {
                (
                    acc.0 + completed,
                    acc.1 + (scene.end_frame - scene.start_frame) as u64,
                )
            });
        let top_info = Block::bordered()
            .border_type(ratatui::widgets::BorderType::Rounded)
            .title(Line::from("Input").centered())
            .title_bottom(Line::from(self.encoder.base().friendly_name()).centered());
        let top_info_inner = top_info.inner(layout[0]);
        let top_info_areas =
            Layout::vertical([Constraint::Fill(1), Constraint::Fill(1)]).split(top_info_inner);
        frame.render_widget(top_info, layout[0]);
        let input_info = InputInfo::new(self.clip_info);
        let input_info = input_info.generate(false);
        frame.render_widget(input_info, top_info_areas[0]);
        let encoder_info = EncoderInfo::new(self.encoder.clone(), None);
        let encoder_info = encoder_info.generate(false);
        frame.render_widget(encoder_info, top_info_areas[1]);

        let active_encoders = ActiveEncoders::new(
            MAIN_COLOR,
            self.workers,
            self.encoder.clone(),
            self.active_scenes.clone().into_iter().collect(),
        );
        frame.render_widget(active_encoders, layout[1]);

        let progress_bar = ProgressBar {
            color:             MAIN_COLOR,
            processing_title:  if self.attempted_cancel {
                "Shutting down after Encoders complete...".to_owned()
            } else {
                "Encoding Scenes...".to_owned()
            },
            completed_title:   if self.attempted_cancel {
                "Encoding Aborted".to_owned()
            } else {
                "Encoding Completed".to_owned()
            },
            unit_per_second:   "FPS".to_owned(),
            unit:              "Frame".to_owned(),
            initial_completed: self.initial_frames,
            completed:         total_frames_completed,
            total:             total_frames,
        };
        let progress_bar = progress_bar.generate(Some(self.started));
        frame.render_widget(progress_bar, layout[2]);
    }
}

impl ParallelEncoderApp {
    pub fn new(
        workers: u8,
        encoder: Encoder,
        scenes: BTreeMap<u64, (u64, Scene<CliSequenceData>)>,
        clip_info: ClipInfo,
    ) -> ParallelEncoderApp {
        ParallelEncoderApp {
            original_panic_hook: None,
            started: std::time::Instant::now(),
            workers,
            encoder,
            initial_frames: scenes.iter().fold(0, |acc, (_, (completed, _total))| acc + completed),
            scenes,
            active_scenes: BTreeMap::new(),
            clip_info,
            attempted_cancel: false,
        }
    }
}

enum ParallelEncoderAppEvent {
    Quit,
    Tick,                   // 60 FPS
    Input(event::KeyEvent), // Keyboard events
    Progress {
        scene:         u64,
        current_pass:  u8,
        total_passes:  u8,
        current_frame: u64,
        total_frames:  u64,
    },
    SceneCompleted(u64),
}

#[derive(Debug, Clone)]
pub struct SceneEncoder {
    pub scene:            Scene<CliSequenceData>,
    pub started:          std::time::Instant,
    pub current_pass:     u8,
    pub total_passes:     u8,
    pub frames_processed: u64,
    pub total_frames:     u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ParallelEncoderConsoleEvent {
    Progress {
        scene:         SceneProgress,
        current_frame: u64,
        total_frames:  u64,
    },
    NewScene {
        scene_index: u64,
        /// Must be milliseconds since UNIX Epoch
        time:        u128,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneProgress {
    pub index:     u64,
    current_pass:  u8,
    total_passes:  u8,
    current_frame: u64,
    total_frames:  u64,
}
