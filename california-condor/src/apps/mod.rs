use std::{
    io::{self, stderr, stdout, BufWriter, IsTerminal, Write},
    sync::{atomic::AtomicBool, mpsc::Receiver, Arc},
};

use andean_condor::core::sequence::SequenceStatus;
use anyhow::Result;
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

pub mod benchmarker;
pub mod parallel_encoder;
pub mod scene_detection;

pub type PanicHook = Box<dyn Fn(&std::panic::PanicHookInfo<'_>) + Sync + Send + 'static>;
pub type StdOutOrErrTerminal = Terminal<CrosstermBackend<BufWriter<Box<dyn Write + Send>>>>;

pub trait TuiApp: Send + Sync + 'static {
    /// The struct must have a field to store original panic hook as
    /// Option<PanicHook>
    fn original_panic_hook(&mut self) -> &mut Option<PanicHook>;

    fn init(&mut self) -> Result<StdOutOrErrTerminal> {
        enable_raw_mode()?;
        let stdout_is_terminal = stdout().is_terminal();
        let writer: Box<dyn Write + Send> = if stdout_is_terminal {
            Box::new(stdout())
        } else {
            Box::new(stderr())
        };
        // STDERR is not buffered
        let mut writer = BufWriter::new(writer);
        execute!(writer, EnterAlternateScreen)?;

        let original_hook = std::panic::take_hook();
        *self.original_panic_hook() = Some(original_hook);
        std::panic::set_hook(Box::new(move |panic_info| {
            let _ = disable_raw_mode();
            let mut writer: Box<dyn Write + Send> = if stdout_is_terminal {
                Box::new(stdout())
            } else {
                Box::new(stderr())
            };
            let _ = execute!(writer, LeaveAlternateScreen);
            let _ = execute!(writer, crossterm::cursor::Show);
            println!("{:?}", panic_info);
            // let handler = std::panic::take_hook();
            // handler(panic_info);
        }));

        let backend = CrosstermBackend::new(writer);
        Ok(Terminal::new(backend)?)
    }

    fn restore(&mut self, mut terminal: StdOutOrErrTerminal) -> Result<()> {
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
        progress_rx: Receiver<SequenceStatus>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<()>;

    fn render(&self, frame: &mut Frame);
}
