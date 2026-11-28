//! Stable Windows APIs: create suspended, assign kill-on-close job, then resume.
use std::{
    io,
    mem::size_of,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    process::{Child, Command},
};
use windows_sys::Win32::{
    Foundation::{INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT},
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
        },
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject, TerminateJobObject,
        },
        Threading::{
            CREATE_NEW_PROCESS_GROUP, CREATE_SUSPENDED, GetProcessIdOfThread, OpenThread,
            ResumeThread, THREAD_QUERY_LIMITED_INFORMATION, THREAD_SUSPEND_RESUME,
            WaitForSingleObject,
        },
    },
};

pub(super) const CREATION_FLAGS: u32 = CREATE_SUSPENDED | CREATE_NEW_PROCESS_GROUP;

pub(super) fn configure(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(CREATION_FLAGS);
}

pub(super) struct Job(OwnedHandle);
impl Job {
    pub(super) fn new() -> io::Result<Self> {
        // SAFETY: No name/security descriptor; returned handle is uniquely owned.
        let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: CreateJobObjectW returned a non-null, uniquely owned handle.
        let job = Self(unsafe { OwnedHandle::from_raw_handle(raw) });
        // SAFETY: Zero initializes this POD Win32 structure; size matches its type.
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: Job and limits remain live; buffer size/class match the structure.
        if unsafe {
            SetInformationJobObject(
                job.0.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }
    pub(super) fn assign(&self, child: &Child) -> io::Result<()> {
        #[cfg(test)]
        if test_hooks::injected("assign", child) {
            return Err(io::Error::other("injected assignment failure"));
        }
        // SAFETY: Both handles are live; child was created suspended and cannot spawn.
        if unsafe { AssignProcessToJobObject(self.0.as_raw_handle(), child.as_raw_handle()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    pub(super) fn kill(&self) -> io::Result<()> {
        // SAFETY: This private job contains only this owned child and its descendants.
        if unsafe { TerminateJobObject(self.0.as_raw_handle(), 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    /// Exposes the job handle for tests that query membership; not used in production.
    #[cfg(test)]
    pub(super) fn raw_handle(&self) -> std::os::windows::io::RawHandle {
        self.0.as_raw_handle()
    }
}

pub(super) fn exited(child: &Child) -> io::Result<bool> {
    // SAFETY: Child keeps its process handle alive; zero timeout never blocks.
    match unsafe { WaitForSingleObject(child.as_raw_handle(), 0) } {
        WAIT_OBJECT_0 => Ok(true),
        WAIT_TIMEOUT => Ok(false),
        _ => Err(io::Error::last_os_error()),
    }
}

pub(super) fn resume(child: &Child) -> io::Result<()> {
    #[cfg(test)]
    test_hooks::RESUME_CALLS.set(test_hooks::RESUME_CALLS.get() + 1);
    #[cfg(test)]
    if test_hooks::injected("resume", child) {
        return Err(io::Error::other("injected resume failure"));
    }
    // RATIONALE: Stable Child doesn't expose the primary thread handle. ToolHelp
    // enumerates threads; only the suspended, still-owned child's PID is selected.
    // LINK: learn.microsoft.com/windows/win32/toolhelp/traversing-the-thread-list
    // SAFETY: Snapshot APIs take integer flags and initialize THREADENTRY32.
    let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: Snapshot returned a valid, uniquely owned handle, not INVALID_HANDLE_VALUE.
    let snapshot = unsafe { OwnedHandle::from_raw_handle(raw) };
    // SAFETY: THREADENTRY32 is a plain Win32 output structure; dwSize is set next.
    let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
    entry.dwSize = size_of::<THREADENTRY32>() as u32;
    // SAFETY: Snapshot is live; entry has the required size and writable storage.
    if unsafe { Thread32First(snapshot.as_raw_handle(), &mut entry) } == 0 {
        return Err(io::Error::last_os_error());
    }
    loop {
        if entry.th32OwnerProcessID == child.id() {
            // SAFETY: OpenThread accepts a snapshot TID (which may be stale). Request
            // only resume/query rights, without inheritance; verify the handle below.
            let raw = unsafe {
                OpenThread(
                    THREAD_SUSPEND_RESUME | THREAD_QUERY_LIMITED_INFORMATION,
                    0,
                    entry.th32ThreadID,
                )
            };
            if raw.is_null() {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: OpenThread returned a non-null, uniquely owned thread handle.
            let thread = unsafe { OwnedHandle::from_raw_handle(raw) };
            // SAFETY: The live thread handle has THREAD_QUERY_LIMITED_INFORMATION.
            let owner = unsafe { GetProcessIdOfThread(thread.as_raw_handle()) };
            if owner == 0 {
                return Err(io::Error::last_os_error());
            }
            if owner != child.id() {
                return Err(io::Error::other(
                    "Snapshot thread no longer belongs to owned child",
                ));
            }
            // SAFETY: The opened handle pins this verified child's thread, with
            // resume rights; caller's process lock excludes our own force kill.
            let previous = unsafe { ResumeThread(thread.as_raw_handle()) };
            return match previous {
                1 => Ok(()),
                u32::MAX => Err(io::Error::last_os_error()),
                _ => Err(io::Error::other("Unexpected primary thread suspend count")),
            };
        }
        // SAFETY: Snapshot and correctly sized output entry remain live for enumeration.
        if unsafe { Thread32Next(snapshot.as_raw_handle(), &mut entry) } == 0 {
            return Err(io::Error::other(
                "Suspended child's primary thread not found",
            ));
        }
    }
}

/// Failure injection for the native acceptance tests in `utils::process::tests`.
#[cfg(test)]
pub(crate) mod test_hooks {
    use super::*;
    use std::{
        cell::{Cell, RefCell},
        os::windows::io::BorrowedHandle,
    };

    thread_local! {
        pub(super) static RESUME_CALLS: Cell<usize> = const { Cell::new(0) };
        static FAILURE: Cell<Option<&'static str>> = const { Cell::new(None) };
        static FAILED_CHILD: RefCell<Option<OwnedHandle>> = const { RefCell::new(None) };
    }

    /// Fail the next call of `stage` (`"assign"`, `"resume"` or `"force-before-resume"`).
    pub(crate) fn arm(stage: &'static str) {
        FAILURE.set(Some(stage));
    }

    pub(crate) fn injected(stage: &'static str, child: &Child) -> bool {
        if FAILURE.get() != Some(stage) {
            return false;
        }
        FAILURE.set(None);
        // SAFETY: Child owns this handle throughout the immediate duplication.
        let handle = unsafe { BorrowedHandle::borrow_raw(child.as_raw_handle()) }
            .try_clone_to_owned()
            .expect("duplicate child handle");
        FAILED_CHILD.with(|saved| *saved.borrow_mut() = Some(handle));
        true
    }

    /// The duplicate handle of the child a failed spawn had to reap.
    pub(crate) fn take_failed_child() -> Option<OwnedHandle> {
        FAILED_CHILD.with(|saved| saved.borrow_mut().take())
    }

    /// Reads how often `resume` ran on this thread.
    pub(crate) fn resume_calls() -> usize {
        RESUME_CALLS.get()
    }

    /// Resets the resume counter for this thread.
    pub(crate) fn reset_resume_calls() {
        RESUME_CALLS.set(0);
    }
}
