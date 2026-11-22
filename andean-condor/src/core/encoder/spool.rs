//! One owned job-local Y4M file, reused sequentially without retaining future passes.

use std::{
    fs::File,
    io::{self, Cursor, Read, Seek, SeekFrom, Write},
    path::Path,
    thread,
};

use crate::utils::cancellation::{Cancellation, Cancelled};
use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, Sender, bounded};

const CHUNK_BYTES: usize = 64 * 1024;
const WINDOW: usize = 2;

pub(crate) fn create(parent: &Path, output: &Path) -> Result<File> {
    tempfile::tempfile_in(parent).with_context(|| {
        format!(
            "Creating Y4M replay spool for {} under {}",
            output.display(),
            parent.display()
        )
    })
}

pub(super) fn encode<T>(
    mut file: impl Read + Write + Seek + Send,
    frames: Receiver<Cursor<Vec<u8>>>,
    output: &Path,
    total: u8,
    cancellation: &Cancellation,
    mut encode: impl FnMut(u8, Receiver<Cursor<Vec<u8>>>) -> Result<T>,
) -> Result<T> {
    capture(&mut file, frames, output, cancellation)?;
    let mut result = replay(&mut file, output, cancellation, |rx| encode(1, rx))?;
    for pass in 2..=total {
        result = replay(&mut file, output, cancellation, |rx| encode(pass, rx))?;
    }
    Ok(result)
}

/// Capture once; the owning producer must additionally confirm successful decoding.
pub(crate) fn capture(
    file: &mut impl Write,
    frames: Receiver<Cursor<Vec<u8>>>,
    output: &Path,
    cancellation: &Cancellation,
) -> Result<()> {
    loop {
        let frame = crossbeam_channel::select_biased! {
            recv(cancellation.forced()) -> _ => return Err(Cancelled::Forced.into()),
            recv(frames) -> frame => match frame { Ok(frame) => frame, Err(_) => break },
        };
        // COMPAT: The stream writer concatenates entire messages, ignoring cursor position.
        file.write_all(frame.get_ref()).with_context(|| {
            format!(
                "Writing Y4M replay spool for {} (check free disk space)",
                output.display()
            )
        })?;
        #[cfg(all(test, target_os = "linux"))]
        tests::runtime::observe_capture();
    }
    drop(frames); // Release the sole upstream receiver before replay starts.
    file.flush().with_context(|| {
        format!(
            "Flushing Y4M replay spool for {} (check free disk space)",
            output.display()
        )
    })?;
    #[cfg(all(test, target_os = "linux"))]
    tests::runtime::observe_flush();

    Ok(())
}

pub(crate) fn replay<T>(
    file: &mut (impl Read + Seek + Send),
    output: &Path,
    cancellation: &Cancellation,
    encode: impl FnOnce(Receiver<Cursor<Vec<u8>>>) -> Result<T>,
) -> Result<T> {
    cancellation.check_forced()?;
    file.seek(SeekFrom::Start(0))
        .with_context(|| format!("Seeking Y4M replay spool for {}", output.display()))?;
    let (tx, rx) = bounded(WINDOW);
    thread::scope(|scope| {
        let producer = scope.spawn(move || feed(file, &tx, output, cancellation));
        // The encoder owns the only receiver. Even spawn failure drops it, unblocking send.
        let result = encode(rx);
        let feed = producer
            .join()
            .map_err(|_| anyhow::anyhow!("Y4M replay worker panicked for {}", output.display()));
        match feed.and_then(std::convert::identity) {
            Ok(()) => result,
            // A disconnected consumer is fallout, but real spool IO is terminal.
            Err(error)
                if error.is::<crossbeam_channel::SendError<Cursor<Vec<u8>>>>()
                    || error.is::<Cancelled>() =>
            {
                result.and_then(|_| Err(error))
            },
            Err(error) => Err(match result {
                Ok(_) => error,
                Err(encoded) => error.context(format!("Secondary encoder failure: {encoded:#}")),
            }),
        }
    })
}

fn feed(
    file: &mut impl Read,
    tx: &Sender<Cursor<Vec<u8>>>,
    output: &Path,
    cancellation: &Cancellation,
) -> Result<()> {
    loop {
        cancellation.check_forced()?;
        // ponytail: two queued + one writer + one producer chunk = at most 256 KiB.
        let mut bytes = vec![0; CHUNK_BYTES];
        let count = loop {
            match file.read(&mut bytes) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {},
                result => break result,
            }
        }
        .with_context(|| format!("Reading Y4M replay spool for {}", output.display()))?;
        if count == 0 {
            return Ok(());
        }
        bytes.truncate(count);
        cancellation
            .send(tx, Cursor::new(bytes))
            .with_context(|| format!("Sending Y4M replay spool for {}", output.display()))?;
    }
}

#[cfg(test)]
mod tests;
