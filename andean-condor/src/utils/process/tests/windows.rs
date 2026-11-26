//! Native runner only: OwnedProcess assigns before resume and reaps every failure.
use crate::utils::{
    cancellation::Cancellation,
    process::{OwnedProcess, windows::test_hooks},
};

/// Forces a cancellation between registration and resume.
pub(in crate::utils::process) fn before_resume(process: &OwnedProcess, token: &Cancellation) {
    let inject = {
        let state = process.state.lock().expect("state");
        test_hooks::injected("force-before-resume", &state.child)
    };
    if inject {
        token.request().expect("graceful before resume");
        token.request().expect("force before resume");
    }
}

mod internal_integration {
    use super::{Cancellation, OwnedProcess, test_hooks};
    use crate::utils::process::windows as platform;
    use std::{
        os::windows::{
            io::{AsRawHandle, FromRawHandle, OwnedHandle},
            process::CommandExt,
        },
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    use windows_sys::Win32::{
        Foundation::WAIT_OBJECT_0,
        System::{
            JobObjects::IsProcessInJob,
            Threading::{
                OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
                WaitForSingleObject,
            },
        },
    };

    fn command(root: &std::path::Path) -> Command {
        let mut command = Command::new(std::env::current_exe().expect("test binary"));
        command
        .args(["--exact", "utils::process::tests::windows::internal_integration::spawn_windows_job_descendant_until_owner_terminates", "--nocapture"])
        .env("PROCESS_LIFECYCLE_WINDOWS_ROOT", root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
        command
    }

    fn descendant(root: &std::path::Path) -> OwnedHandle {
        let deadline = Instant::now() + Duration::from_secs(4);
        let pid = loop {
            if let Ok(pid) = std::fs::read_to_string(root.join("grandchild")) {
                break pid.parse::<u32>().expect("PID");
            }
            assert!(
                Instant::now() < deadline,
                "native child handshake timed out"
            );
            std::thread::yield_now();
        };
        // SAFETY: Open only the PID published by this private fixture, for wait/query only.
        let raw = unsafe {
            OpenProcess(
                PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                pid,
            )
        };
        assert!(!raw.is_null());
        // SAFETY: OpenProcess returned a non-null, uniquely owned handle.
        unsafe { OwnedHandle::from_raw_handle(raw) }
    }

    #[test]
    fn spawn_windows_job_descendant_until_owner_terminates() {
        let Some(root) = std::env::var_os("PROCESS_LIFECYCLE_WINDOWS_ROOT") else {
            return;
        };
        if std::env::var_os("PROCESS_LIFECYCLE_WINDOWS_LEAF").is_some() {
            std::thread::sleep(Duration::from_secs(8));
        } else {
            let mut command = command(std::path::Path::new(&root));
            command.env("PROCESS_LIFECYCLE_WINDOWS_LEAF", "1");
            let mut child = command.spawn().expect("grandchild inside inherited job");
            let pending = std::path::Path::new(&root).join("grandchild.pending");
            std::fs::write(&pending, child.id().to_string()).expect("complete PID");
            std::fs::rename(pending, std::path::Path::new(&root).join("grandchild"))
                .expect("atomic PID handshake");
            child.wait().expect("finite emergency lifetime");
        }
    }

    #[test]
    fn windows_owned_process_assigns_descendants_before_resume_and_forced_exit() {
        let root = tempfile::tempdir().expect("fixture");
        let token = Cancellation::default();
        let mut process = OwnedProcess::spawn(&mut command(root.path()), &token)
            .expect("suspended assignment/resume");
        let grandchild = descendant(root.path());
        let mut inside = 0;
        let state = process.state.lock().expect("state");
        assert_ne!(
            // SAFETY: Query the known descendant and this owned job, with a valid BOOL output.
            unsafe {
                IsProcessInJob(
                    grandchild.as_raw_handle(),
                    state.job.raw_handle(),
                    &mut inside,
                )
            },
            0
        );
        assert_ne!(inside, 0, "descendant cannot run outside the assigned job");
        drop(state);
        token.request().expect("graceful");
        assert!(
            !platform::exited(&process.state.lock().expect("state").child).expect("still running")
        );
        token.request().expect("force");
        process.wait().expect("reap direct child");
        assert_eq!(
            // SAFETY: Wait on the already-opened fixture descendant only; bounded failure gate.
            unsafe { WaitForSingleObject(grandchild.as_raw_handle(), 1000) },
            WAIT_OBJECT_0
        );
    }

    #[test]
    fn windows_job_assignment_failure_reaps_the_suspended_child() {
        assert_suspended_failure_reaps("assign");
    }

    #[test]
    fn windows_resume_failure_reaps_the_suspended_child() {
        assert_suspended_failure_reaps("resume");
    }

    fn assert_suspended_failure_reaps(stage: &'static str) {
        let root = tempfile::tempdir().expect("fixture");
        test_hooks::arm(stage);
        assert!(OwnedProcess::spawn(&mut command(root.path()), &Cancellation::default()).is_err());
        let handle = test_hooks::take_failed_child().expect("real failed child");
        assert_eq!(
            // SAFETY: The duplicate remains valid after the spawning owner returned its error.
            unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) },
            WAIT_OBJECT_0
        );
        assert!(
            !root.path().join("grandchild").exists(),
            "failed suspended child must never execute"
        );
    }

    #[test]
    fn windows_force_before_resume_never_opens_or_resumes_thread() {
        let root = tempfile::tempdir().expect("fixture");
        let token = Cancellation::default();
        test_hooks::reset_resume_calls();
        test_hooks::arm("force-before-resume");
        let error = OwnedProcess::spawn(&mut command(root.path()), &token)
            .err()
            .expect("forced spawn");
        assert_eq!(
            error.downcast_ref::<crate::utils::cancellation::Cancelled>(),
            Some(&crate::utils::cancellation::Cancelled::Forced)
        );
        assert_eq!(
            test_hooks::resume_calls(),
            0,
            "skip thread enumeration entirely after force"
        );
        let handle = test_hooks::take_failed_child().expect("owned child handle");
        assert_eq!(
            // SAFETY: Duplicate of the owned child remains live after error cleanup/reap.
            unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) },
            WAIT_OBJECT_0
        );
        assert!(
            !root.path().join("grandchild").exists(),
            "suspended child never ran"
        );
    }

    #[test]
    fn windows_job_close_kills_descendants() {
        let root = tempfile::tempdir().expect("fixture");
        let job = platform::Job::new().expect("kill-on-close job");
        let mut child = command(root.path())
            .creation_flags(platform::CREATION_FLAGS)
            .spawn()
            .expect("suspended child");
        job.assign(&child).expect("assign before resume");
        platform::resume(&child).expect("resume");
        let grandchild = descendant(root.path());
        drop(job);
        assert_eq!(
            // SAFETY: Wait only on this fixture's direct-child handle, with a bounded timeout.
            unsafe { WaitForSingleObject(child.as_raw_handle(), 1000) },
            WAIT_OBJECT_0
        );
        assert_eq!(
            // SAFETY: Wait only on this fixture's descendant handle, with a bounded timeout.
            unsafe { WaitForSingleObject(grandchild.as_raw_handle(), 1000) },
            WAIT_OBJECT_0
        );
        child.wait().expect("reap direct child");
    }
}
