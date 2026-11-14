//! Concrete ownership of a job child and its process group/Job Object through reap.
use crate::utils::cancellation::Cancellation;
use anyhow::{Context, Result};
use std::{
    io,
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus},
    sync::{Arc, Mutex},
    time::Duration,
};

pub(crate) struct ProcessState {
    child: Child,
    armed: bool,
    killed: bool,
    status: Option<ExitStatus>,
    #[cfg(windows)]
    job: windows::Job,
}

impl ProcessState {
    pub(crate) fn kill(&mut self) -> io::Result<()> {
        if !self.armed || self.killed {
            return Ok(());
        }
        #[cfg(unix)]
        {
            // Fail closed if a host waiter/disposition change already lost the child.
            self.exited()?;
            let pid = i32::try_from(self.child.id()).map_err(io::Error::other)?;
            if pid <= 0 {
                return Err(io::Error::other("invalid owned process group"));
            }
            let mut failure = None;
            for target in [-pid, pid] {
                // SAFETY: waitid just verified our unreaped child. Under the host
                // ownership contract its PID pins the original group ID too. The
                // positive PID also covers a direct child that changed groups.
                if unsafe { libc::kill(target, libc::SIGKILL) } != 0 {
                    let error = io::Error::last_os_error();
                    if error.raw_os_error() != Some(libc::ESRCH) {
                        failure.get_or_insert(error);
                    }
                }
            }
            if let Some(error) = failure {
                return Err(error);
            }
        }
        #[cfg(windows)]
        self.job.kill()?;
        self.killed = true;
        Ok(())
    }

    #[cfg_attr(
        windows,
        allow(
            clippy::needless_pass_by_ref_mut,
            reason = "Unix must disarm on ECHILD; both platforms share the wait callsites"
        )
    )]
    fn exited(&mut self) -> io::Result<bool> {
        #[cfg(unix)]
        {
            // SAFETY: Zero is a valid initial siginfo value; waitid fills it below.
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            // SAFETY: Query only the known child into a valid writable siginfo.
            // WNOWAIT leaves it unreaped, subject to the host ownership contract.
            let result = unsafe {
                libc::waitid(
                    libc::P_PID,
                    self.child.id(),
                    &mut info,
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            };
            if result != 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::ECHILD) {
                    self.armed = false; // Never signal a known-lost PID/group on Drop.
                }
                return Err(error);
            }
            // SAFETY: Successful waitid initialized the SIGCHLD union member.
            Ok(unsafe { info.si_pid() } != 0)
        }
        #[cfg(windows)]
        {
            windows::exited(&self.child)
        }
    }
}

/// Owns kill control independently of pipe readers/writers. Drop kills the tree
/// and reaps the direct child; explicit `wait` reports OS cleanup failures.
/// On Unix the host must not reap this child externally or change SIGCHLD to
/// auto-reap while a guard is active. Piped/null stdin avoids background-TTY stops.
pub struct OwnedProcess {
    state: Arc<Mutex<ProcessState>>,
    pub stdin: Option<ChildStdin>,
    pub stdout: Option<ChildStdout>,
    pub stderr: Option<ChildStderr>,
}

impl OwnedProcess {
    /// An active scene may spawn another pass during Graceful, but never Forced.
    #[inline]
    pub fn spawn(command: &mut Command, cancellation: &Cancellation) -> Result<Self> {
        cancellation.check_forced()?;
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // SAFETY: A zeroed sigaction is a valid output buffer for this query.
            let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
            // SAFETY: A null action only queries SIGCHLD; no host disposition changes.
            if unsafe { libc::sigaction(libc::SIGCHLD, std::ptr::null(), &mut action) } != 0 {
                return Err(io::Error::last_os_error()).context("Checking SIGCHLD ownership");
            }
            if action.sa_sigaction == libc::SIG_IGN || action.sa_flags & libc::SA_NOCLDWAIT != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "OwnedProcess requires SIGCHLD without SIG_IGN or SA_NOCLDWAIT",
                )
                .into());
            }
            command.process_group(0);
        }
        #[cfg(windows)]
        let job = windows::Job::new()?;
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(windows::CREATION_FLAGS);
        }
        let mut child = command.spawn().context("Spawning owned job process")?;
        let (stdin, stdout, stderr) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take());
        let mut process = Self {
            state: Arc::new(Mutex::new(ProcessState {
                child,
                armed: true,
                killed: false,
                status: None,
                #[cfg(windows)]
                job,
            })),
            stdin,
            stdout,
            stderr,
        };
        #[cfg(windows)]
        {
            let assigned = {
                let state = process.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                state.job.assign(&state.child)
            };
            if let Err(error) = assigned {
                // Suspended and unassigned: terminate the direct child, not an empty job.
                let mut state =
                    process.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let killed = state.child.kill();
                let reaped = state.child.wait();
                state.armed = false;
                killed.context("Terminating unassigned suspended child")?;
                reaped.context("Reaping unassigned suspended child")?;
                return Err(error).context("Assigning suspended child to Job Object");
            }
        }
        #[cfg(all(test, target_os = "linux"))]
        crate::core::encoder::process_lifecycle::before_process_registration(command, process.id());
        #[cfg(all(test, target_os = "linux"))]
        if matches!(
            std::env::var("PROCESS_LIFECYCLE_CASE").as_deref(),
            Ok("registration" | "filter-spawn-race")
        ) {
            // Exact race seam: the real child can run, but has not been registered.
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            while !cancellation.is_forced() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "force reaches registration seam"
                );
                std::thread::yield_now();
            }
        }
        if let Err(error) = cancellation.register(&process.state) {
            process.wait().context("Reaping child cancelled during spawn")?;
            return Err(error);
        }
        #[cfg(all(test, target_os = "linux"))]
        crate::core::encoder::process_lifecycle::after_process_registration(command, process.id());
        #[cfg(windows)]
        {
            #[cfg(test)]
            windows::tests::before_resume(&process, cancellation);
            let resumed = {
                let state = process.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                if state.killed || cancellation.is_forced() {
                    Err(crate::utils::cancellation::Cancelled::Forced.into())
                } else {
                    windows::resume(&state.child).context("Resuming job-assigned child")
                }
            };
            if let Err(error) = resumed {
                process.kill()?;
                process.wait()?;
                cancellation.check_forced()?;
                return Err(error);
            }
        }
        Ok(process)
    }

    #[inline]
    pub fn id(&self) -> u32 {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).child.id()
    }

    #[inline]
    pub fn kill(&mut self) -> Result<()> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .kill()
            .context("Killing owned process tree")
    }

    /// Call after draining/joining pipe workers. No mutex spans a pending wait;
    /// cancellation retains immediate kill control even for silent children.
    #[inline]
    pub fn wait(&mut self) -> Result<ExitStatus> {
        loop {
            {
                let mut state =
                    self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Some(status) = state.status {
                    return Ok(status);
                }
                if state.exited().context("Checking owned child without reaping")? {
                    let killed = state.kill(); // Before reap: no stale group ID can be signalled.
                    let status = state.child.wait();
                    #[cfg(all(test, target_os = "linux"))]
                    if let Some(root) =
                        std::env::var_os("ENCODER_FIXTURE_ROOT").map(std::path::PathBuf::from)
                        && let Some(pid) = std::fs::read_to_string(root.join("leader"))
                            .ok()
                            .and_then(|pid| pid.parse::<u32>().ok())
                        && pid > 0
                        && pid == state.child.id()
                    {
                        use std::os::unix::process::ExitStatusExt;
                        // A worker panic can become Forced; the joined caller must check this record.
                        let signal = status.as_ref().ok().and_then(|status| status.signal());
                        std::fs::write(root.join("leader-signal"), format!("{pid} {signal:?}"))
                            .expect("record known fixture leader signal");
                    }
                    #[cfg(all(test, target_os = "linux"))]
                    crate::core::encoder::process_lifecycle::observe_reap(state.child.id());
                    state.armed = false;
                    if let Ok(status) = &status {
                        state.status = Some(*status);
                    }
                    killed.context("Cleaning remaining owned descendants")?;
                    return status.context("Reaping owned child");
                }
            }
            std::thread::park_timeout(Duration::from_millis(10));
        }
    }

    /// Observe/reap an exited leader while pipe readers run; remaining owned
    /// descendants must be killed before they can indefinitely hold those pipes.
    pub(crate) fn try_wait(&mut self) -> Result<Option<ExitStatus>> {
        let exited = {
            let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            state.status.is_some() || state.exited()?
        };
        if exited {
            self.wait().map(Some)
        } else {
            Ok(None)
        }
    }

    #[cfg(all(test, target_os = "linux"))]
    pub(crate) fn is_armed(&self) -> bool {
        self.state.lock().expect("state").armed
    }
}

impl Drop for OwnedProcess {
    #[inline]
    fn drop(&mut self) {
        let armed = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).armed;
        if armed {
            if let Err(error) = self.kill() {
                tracing::error!(%error, "Owned process Drop kill failed");
            }
            if let Err(error) = self.wait() {
                tracing::error!(%error, "Owned process Drop reap failed");
            }
        }
    }
}

/// Remove only a newly-created interrupted output, never a caller-owned sentinel.
pub(crate) struct InterruptedOutput {
    path: std::path::PathBuf,
    owned: bool,
    cancellation: Cancellation,
}

impl InterruptedOutput {
    pub(crate) fn new(path: &std::path::Path, cancellation: &Cancellation) -> Self {
        Self {
            path: path.to_owned(),
            owned: !path.exists(),
            cancellation: cancellation.clone(),
        }
    }

    /// Commit this unit against force; enclosing units retain their own guards.
    pub(crate) fn complete(&mut self) -> Result<()> {
        #[cfg(all(test, target_os = "linux"))]
        crate::core::encoder::process_lifecycle::before_output_commit(
            &self.path,
            &self.cancellation,
        );
        let cancellation = self.cancellation.clone();
        cancellation.publish(|| {
            self.disarm();
            Ok(())
        })
    }

    /// Caller already holds the token's publication lock (for batched renames).
    pub(crate) fn disarm(&mut self) {
        self.owned = false;
    }
}

impl Drop for InterruptedOutput {
    fn drop(&mut self) {
        #[cfg(all(test, target_os = "linux"))]
        crate::core::encoder::process_lifecycle::output_drop(&self.path, &self.cancellation);
        if self.owned && self.cancellation.is_forced() {
            match std::fs::remove_file(&self.path) {
                Ok(()) => {},
                Err(error) if error.kind() == io::ErrorKind::NotFound => {},
                Err(error) => {
                    tracing::error!(%error, path = %self.path.display(), "Removing interrupted output failed");
                },
            }
        }
    }
}

#[cfg(windows)]
mod windows;
