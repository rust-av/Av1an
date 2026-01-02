use std::{
    io::IsTerminal,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
        Arc,
    },
    thread,
};

use anyhow::Result;
use av1an_core::{
    condor::core::processors::{ProcessCompletion, ProcessStatus, Status},
    ClipInfo,
};
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
    components::{input_info::InputInfo, progress_bar::ProgressBar},
};

pub struct SceneDetectionApp {
    pub(crate) original_panic_hook: Option<super::PanicHook>,
    pub started:                    std::time::Instant,
    initial_frames:                 u64,
    pub frames_processed:           u64,
    pub total_frames:               u64,
    pub scenes:                     Vec<(u64, u64)>,
    pub clip_info:                  ClipInfo,
    attempted_cancel:               bool,
}

impl TuiApp for SceneDetectionApp {
    fn original_panic_hook(&mut self) -> &mut Option<super::PanicHook> {
        &mut self.original_panic_hook
    }

    fn run(
        &mut self,
        progress_rx: Receiver<ProcessStatus>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<()> {
        let (event_tx, event_rx) = mpsc::channel();
        let input_tx = event_tx.clone();
        thread::spawn(move || loop {
            if let Ok(TermEvent::Key(key)) = event::read()
                && input_tx.send(SceneDetectionAppEvent::Input(key)).is_err()
            {
                break;
            }
        });
        let tick_tx = event_tx.clone();
        thread::spawn(move || loop {
            if tick_tx.send(SceneDetectionAppEvent::Tick).is_err() {
                break;
            }
            thread::sleep(std::time::Duration::from_millis(16)); // ~60 FPS
        });
        thread::spawn(move || {
            for progress in progress_rx {
                if let ProcessStatus::Whole(Status::Processing {
                    completion, ..
                }) = progress
                {
                    match completion {
                        ProcessCompletion::Frames {
                            completed, ..
                        } => {
                            let _ = event_tx.send(SceneDetectionAppEvent::Progress(completed));
                        },
                        ProcessCompletion::Custom {
                            name,
                            completed,
                            total,
                        } if name == "new-scene" => {
                            let _ = event_tx.send(SceneDetectionAppEvent::NewScene {
                                start: completed as u64,
                                end:   total as u64,
                            });
                        },
                        _ => {},
                    }
                }
            }
            let _ = event_tx.send(SceneDetectionAppEvent::Quit);
        });

        let mut terminal = self.init()?;
        let stdout_is_terminal = std::io::stdout().is_terminal();
        loop {
            match event_rx.recv()? {
                SceneDetectionAppEvent::Tick => {
                    terminal.draw(|f| self.render(f))?;
                },
                SceneDetectionAppEvent::Progress(completed) => {
                    self.frames_processed = completed;
                    if !stdout_is_terminal {
                        let event = SceneDetectionConsoleEvent::ProcessedFrame {
                            completed,
                            total: self.total_frames,
                        };
                        let event = serde_json::to_string(&event)?;
                        println!("[Scene Detector][Progress]: {}", event);
                    }
                },
                SceneDetectionAppEvent::NewScene {
                    start,
                    end,
                } => {
                    self.scenes.push((start, end));
                    if !stdout_is_terminal {
                        let event = SceneDetectionConsoleEvent::NewScene {
                            start,
                            end,
                        };
                        let event = serde_json::to_string(&event)?;
                        println!("[Scene Detector][New Scene]: {}", event);
                    }
                },
                SceneDetectionAppEvent::Quit => {
                    self.restore(terminal)?;
                    break;
                },
                SceneDetectionAppEvent::Input(key) => {
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
                                "Scene Detection does not support cancelling. Press Ctrl+C again \
                                 to exit."
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
                Constraint::Percentage(10),
                Constraint::Percentage(80),
                Constraint::Percentage(10),
            ])
            .split(frame.area());

        let input_info = InputInfo::new(self.clip_info);
        let input_info = input_info.generate(false);
        let input_block = Block::bordered()
            .border_type(ratatui::widgets::BorderType::Rounded)
            .title(Line::from("Input").centered());
        let input_block = if self.attempted_cancel {
            input_block.title_bottom(
                Line::from(
                    "Scene Detection does not support cancelling. Press Ctrl+C again to exit.",
                )
                .centered(),
            )
        } else {
            input_block
        };
        let input_info = input_info.block(input_block);
        frame.render_widget(input_info, layout[0]);

        let progress_bar = ProgressBar {
            color:             MAIN_COLOR,
            processing_title:  if self.attempted_cancel {
                "Waiting for Scene Detection to Finish...".to_owned()
            } else {
                "Detecting Scenes...".to_owned()
            },
            completed_title:   "Scene Detection Completed".to_owned(),
            unit_per_second:   "FPS".to_owned(),
            unit:              "Frame".to_owned(),
            initial_completed: self.initial_frames,
            completed:         self.frames_processed,
            total:             self.total_frames,
        };
        let progress_bar = progress_bar.generate(Some(self.started));
        frame.render_widget(progress_bar, layout[2]);
    }
}

impl SceneDetectionApp {
    pub fn new(
        initial_frames: u64,
        total_frames: u64,
        scenes: Vec<(u64, u64)>,
        clip_info: ClipInfo,
    ) -> SceneDetectionApp {
        SceneDetectionApp {
            original_panic_hook: None,
            started: std::time::Instant::now(),
            initial_frames,
            frames_processed: initial_frames,
            total_frames,
            scenes,
            clip_info,
            attempted_cancel: false,
        }
    }
}

enum SceneDetectionAppEvent {
    Quit,
    Tick,                              // 60 FPS
    Input(event::KeyEvent),            // Keyboard events
    Progress(u64),                     // New frames processed
    NewScene { start: u64, end: u64 }, // New scene found
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum SceneDetectionConsoleEvent {
    ProcessedFrame { completed: u64, total: u64 },
    NewScene { start: u64, end: u64 },
}
