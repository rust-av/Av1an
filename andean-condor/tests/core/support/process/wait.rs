use super::{Cancellation, OwnedProcess};
use std::{process::ExitStatus, sync::mpsc, thread, time::Duration};

pub(in crate::core) fn wait_with_timeout(
    child: &mut OwnedProcess,
    cancellation: Cancellation,
    timeout: Duration,
) -> (ExitStatus, bool) {
    // Use the public cancellation boundary: try_wait is intentionally crate-private.
    let (finished, waiting) = mpsc::channel();
    let watchdog = thread::spawn(move || {
        if matches!(
            waiting.recv_timeout(timeout),
            Err(mpsc::RecvTimeoutError::Timeout)
        ) {
            cancellation.request().expect("watchdog graceful request");
            cancellation.request().expect("watchdog forced request");
            true
        } else {
            false
        }
    });
    let status = child.wait();
    let _ = finished.send(());
    let timed_out = watchdog.join().expect("watchdog joined");
    (status.expect("reap owned test process"), timed_out)
}
