//! Unix process-group containment: ownership check, non-reaping liveness, SIGKILL.
use std::{
    io,
    os::unix::process::CommandExt,
    process::{Child, Command},
};

/// Reject hosts that already auto-reap this child, then place it in its own group.
pub(super) fn configure(command: &mut Command) -> io::Result<()> {
    // SAFETY: A zeroed sigaction is a valid output buffer for this query.
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    // SAFETY: A null action only queries SIGCHLD; no host disposition changes.
    if unsafe { libc::sigaction(libc::SIGCHLD, std::ptr::null(), &mut action) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if action.sa_sigaction == libc::SIG_IGN || action.sa_flags & libc::SA_NOCLDWAIT != 0 {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "OwnedProcess requires SIGCHLD without SIG_IGN or SA_NOCLDWAIT",
        ));
    }
    command.process_group(0);
    Ok(())
}

/// `SIGKILL` the owned group and any child that left it, never the reaper's own group.
pub(super) fn kill_tree(pid: u32) -> io::Result<()> {
    let pid = i32::try_from(pid).map_err(io::Error::other)?;
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
    match failure {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

/// Observe without reaping. A known-lost child disarms the caller's kill guard.
pub(super) fn exited(child: &Child, armed: &mut bool) -> io::Result<bool> {
    // SAFETY: Zero is a valid initial siginfo value; waitid fills it below.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    // SAFETY: Query only the known child into a valid writable siginfo.
    // WNOWAIT leaves it unreaped, subject to the host ownership contract.
    let result = unsafe {
        libc::waitid(
            libc::P_PID,
            child.id(),
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if result != 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ECHILD) {
            *armed = false; // Never signal a known-lost PID/group on Drop.
        }
        return Err(error);
    }
    // SAFETY: Successful waitid initialized the SIGCHLD union member.
    Ok(unsafe { info.si_pid() } != 0)
}
