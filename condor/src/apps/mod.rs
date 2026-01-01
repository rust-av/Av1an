use std::{
    io::{self, Stdout},
    sync::{atomic::AtomicBool, mpsc::Receiver, Arc},
};

use anyhow::Result;
use av1an_core::condor::core::processors::ProcessStatus;
use ratatui::{
    crossterm::{
        self,
        execute,
        terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    },
    prelude::CrosstermBackend,
    Frame,
    Terminal,
};

pub mod parallel_encoder;
pub mod scene_detection;

pub type PanicHook = Box<dyn Fn(&std::panic::PanicHookInfo<'_>) + Sync + Send + 'static>;

pub trait TuiApp: Send + Sync + 'static {
    /// The struct must have a field to store original panic hook as
    /// Option<PanicHook>
    fn original_panic_hook(&mut self) -> &mut Option<PanicHook>;

    fn init(&mut self) -> Result<Terminal<CrosstermBackend<Stdout>>> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;

        let original_hook = std::panic::take_hook();
        *self.original_panic_hook() = Some(original_hook);
        std::panic::set_hook(Box::new(move |panic_info| {
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
            let _ = execute!(io::stdout(), crossterm::cursor::Show);
            println!("{}", panic_info);
            std::process::exit(1);
        }));

        let backend = CrosstermBackend::new(stdout);
        Ok(Terminal::new(backend)?)
    }

    fn restore(&mut self, mut terminal: Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
        execute!(io::stdout(), LeaveAlternateScreen)?;
        disable_raw_mode()?;

        let (_columns, rows) = crossterm::terminal::size()?;
        for _ in 0..rows {
            println!();
        }
        terminal.clear()?;
        terminal.draw(|f| self.render(f))?;
        execute!(io::stdout(), crossterm::cursor::Show)?;

        let _ = std::panic::take_hook();
        if let Some(original) = self.original_panic_hook().take() {
            std::panic::set_hook(original);
        }

        Ok(())
    }

    fn run(
        &mut self,
        progress_rx: Receiver<ProcessStatus>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<()>;

    fn render(&self, frame: &mut Frame);
}
