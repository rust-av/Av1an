use andean_condor::{core::process::OwnedProcess, utils::cancellation::Cancellation};

#[path = "process/wait.rs"]
mod wait;
pub(in crate::core) use wait::wait_with_timeout;

#[cfg(test)]
mod integration {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    #[test]
    fn watchdog_allows_normal_exit_and_forces_a_stalled_owned_process() {
        if let Ok(case) = std::env::var("TEST_WAIT_TIMEOUT_CASE") {
            if case == "stall" {
                let (_blocked, waiting) = std::sync::mpsc::channel::<()>();
                let _ = waiting.recv_timeout(Duration::from_secs(30));
            }
            return;
        }
        let module = module_path!().split_once("::").expect("test crate prefix").1;
        let exact =
            format!("{module}::watchdog_allows_normal_exit_and_forces_a_stalled_owned_process");
        for (case, timeout) in
            [("exit", Duration::from_secs(5)), ("stall", Duration::from_millis(50))]
        {
            let cancellation = Cancellation::default();
            let mut command = Command::new(std::env::current_exe().expect("test binary"));
            command
                .args(["--exact", &exact])
                .env("TEST_WAIT_TIMEOUT_CASE", case)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            let mut child =
                OwnedProcess::spawn(&mut command, &cancellation).expect("owned watchdog control");
            let (status, timed_out) = wait_with_timeout(&mut child, cancellation.clone(), timeout);
            assert_eq!(timed_out, case == "stall");
            assert_eq!(status.success(), case == "exit");
            assert_eq!(cancellation.is_forced(), case == "stall");
        }
    }
}
