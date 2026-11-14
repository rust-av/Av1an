use super::*;
use std::{fs, time::Duration};

pub(in crate::core::encoder) fn run_bounded(command: &mut Command, log: &Path) -> String {
    let file = fs::File::create(log).expect("log");
    let mut child = command
        .stdout(file.try_clone().expect("log clone"))
        .stderr(file)
        .spawn()
        .unwrap_or_else(|error| panic!("cannot spawn {command:?}: {error}"));
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("wait") {
            break status;
        }
        if start.elapsed() > Duration::from_secs(60) {
            child.kill().expect("kill");
            child.wait().expect("reap");
            panic!(
                "timed out: {command:?}\n{}",
                fs::read_to_string(log).expect("log")
            );
        }
        thread::sleep(Duration::from_millis(10));
    };
    let text = fs::read_to_string(log).expect("log");
    assert!(status.success(), "{command:?}: {status}\n{text}");
    text
}
