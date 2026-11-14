//! Native runner only: the libtest parent never acquires/changes a console.
use super::super::{Interrupt, Interrupted, Listener};
use andean_condor::core::process::OwnedProcess;
use anyhow::{Context, Result, ensure};
use std::{
    ffi::c_void,
    fs,
    os::windows::{
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::CommandExt,
    },
    path::Path,
    process::{Command, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::{Duration, Instant},
};

// Stable kernel32 APIs, confined to this test; no new Windows API dependency.
#[link(name = "kernel32")]
unsafe extern "system" {
    fn FreeConsole() -> i32;
    fn AllocConsole() -> i32;
    fn GenerateConsoleCtrlEvent(event: u32, group: u32) -> i32;
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
    fn WaitForSingleObject(handle: *mut c_void, milliseconds: u32) -> u32;
}

const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
const PROCESS_SYNCHRONIZE: u32 = 0x0010_0000;
const CTRL_BREAK_EVENT: u32 = 1;
const WAIT_OBJECT_0: u32 = 0;
const WAIT_TIMEOUT: u32 = 258;
static RECEIVED: AtomicUsize = AtomicUsize::new(0);

// Observe actual backend dequeues, not synthetic requests. The third event must
// cross ctrlc's dispatch thread before the listener is allowed to join.
pub(in crate::interrupt) fn observe_received(count: usize) {
    RECEIVED.fetch_add(count, Ordering::SeqCst);
}

fn command(root: &Path, role: &str) -> Result<Command> {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args(["--exact", "interrupt::tests::windows::windows_console_child", "--nocapture"])
        .env("WINDOWS_INTERRUPT_ROOT", root)
        .env("WINDOWS_INTERRUPT_ROLE", role)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    Ok(command)
}

fn wait_for(description: &str, ready: impl Fn() -> bool) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(8);
    while !ready() {
        ensure!(Instant::now() < deadline, "{description}: 8s watchdog");
        thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}

fn publish(root: &Path, name: &str, value: &str) -> Result<()> {
    let pending = root.join(format!("{name}.pending"));
    fs::write(&pending, value)?;
    fs::rename(pending, root.join(name))?;
    Ok(())
}

fn process_handle(pid: u32) -> Result<OwnedHandle> {
    // SAFETY: Callers retain the creating Child/OwnedProcess (or its live
    // ancestor holding Child) until this query/wait-only handle is opened.
    let raw = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    ensure!(
        !raw.is_null(),
        "opening fixture PID {pid}: {}",
        std::io::Error::last_os_error()
    );
    // SAFETY: OpenProcess returned a valid, uniquely owned process handle.
    Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
}

fn wait_status(handle: &impl AsRawHandle, milliseconds: u32) -> u32 {
    // SAFETY: The caller owns this process handle throughout the bounded wait.
    unsafe { WaitForSingleObject(handle.as_raw_handle(), milliseconds) }
}

#[test]
fn windows_console_break_lifecycle() -> Result<()> {
    let root = tempfile::tempdir()?;
    let mut output = tempfile::tempfile()?;
    let mut command = command(root.path(), "supervisor")?;
    command.stdout(output.try_clone()?).stderr(output.try_clone()?);
    // E12 assigns before resume: watchdog/unwind kills the supervisor AND every
    // descendant, even if its console test fails before publishing a handshake.
    let mut supervisor = OwnedProcess::spawn(&mut command, &Default::default())?;
    let handle = process_handle(supervisor.id())?;
    let completed = wait_status(&handle, 30_000) == WAIT_OBJECT_0;
    let killed = if completed { Ok(()) } else { supervisor.kill() };
    let reaped = supervisor.wait();
    use std::io::{Read, Seek};
    output.rewind()?;
    let mut text = String::new();
    output.read_to_string(&mut text)?;
    killed.context("native console supervisor watchdog cleanup")?;
    ensure!(
        completed,
        "native console supervisor exceeded 30s watchdog\n{text}"
    );
    ensure!(
        reaped?.success(),
        "native console supervisor failed\n{text}"
    );
    ensure!(
        fs::read(root.path().join("complete"))? == b"130",
        "native proof marker"
    );
    Ok(())
}

#[test]
fn windows_console_child() -> Result<()> {
    let Some(root) = std::env::var_os("WINDOWS_INTERRUPT_ROOT") else {
        return Ok(());
    };
    let root = Path::new(&root);
    match std::env::var("WINDOWS_INTERRUPT_ROLE")?.as_str() {
        "supervisor" => supervise(root),
        "listener" => listen(root),
        "fixture" => {
            let mut leaf = command(root, "leaf")?.spawn()?;
            publish(root, "leaf", &leaf.id().to_string())?;
            // The outer E12 job is the failure/unwind cleanup owner as well.
            leaf.wait()?;
            Ok(())
        },
        "leaf" => {
            // Finite emergency lifetime exceeds the supervisor's 30s bound.
            thread::sleep(Duration::from_secs(60));
            Ok(())
        },
        role => anyhow::bail!("unexpected native fixture role: {role}"),
    }
}

fn supervise(root: &Path) -> Result<()> {
    // SAFETY: This function runs only in the dedicated, E12-owned child. Detach
    // only that child (it may have no console); never touch the libtest parent.
    let _ = unsafe { FreeConsole() };
    ensure!(
        // SAFETY: Allocate a private console for this supervisor and its descendants.
        unsafe { AllocConsole() } != 0,
        "AllocConsole: {}",
        std::io::Error::last_os_error()
    );
    let mut child = command(root, "listener")?
        .creation_flags(CREATE_NEW_PROCESS_GROUP)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()?;
    let result = (|| -> Result<()> {
        wait_for("listener/owned-tree ready", || root.join("ready").exists())?;
        let leader = process_handle(fs::read_to_string(root.join("leader"))?.parse()?)?;
        let leaf = process_handle(fs::read_to_string(root.join("leaf"))?.parse()?)?;
        for (index, acknowledgement) in ["graceful", "forced", "saturated"].iter().enumerate() {
            ensure!(
                child.try_wait()?.is_none(),
                "listener exited before event {index}"
            );
            ensure!(
                // SAFETY: Only the still-owned listener's known process group in
                // this private console is targeted. BREAK can be group-targeted;
                // CTRL_C_EVENT cannot. Never broadcast group zero to a host console.
                unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, child.id()) } != 0,
                "GenerateConsoleCtrlEvent: {}",
                std::io::Error::last_os_error()
            );
            wait_for(acknowledgement, || root.join(acknowledgement).exists())?;
            if index == 0 {
                ensure!(
                    wait_status(&leader, 0) == WAIT_TIMEOUT
                        && wait_status(&leaf, 0) == WAIT_TIMEOUT,
                    "first native event must leave active owned work alive"
                );
            }
        }
        ensure!(
            wait_status(&leader, 1_000) == WAIT_OBJECT_0,
            "owned direct child not terminated/reaped"
        );
        ensure!(
            wait_status(&leaf, 1_000) == WAIT_OBJECT_0,
            "owned grandchild not terminated"
        );
        // Keep the listener/group lifetime pinned until the last native event
        // has crossed the backend and its owned tree has been checked.
        publish(root, "release", "joined")?;
        ensure!(
            wait_status(&child, 5_000) == WAIT_OBJECT_0,
            "listener exit watchdog"
        );
        Ok(())
    })();
    let killed = if result.is_err() {
        child.kill()
    } else {
        Ok(())
    };
    let reaped = child.wait();
    if let Err(error) = result {
        return Err(error.context(format!(
            "native listener cleanup: kill={killed:?}; wait={reaped:?}"
        )));
    }
    killed.context("terminating native listener after failure")?;
    ensure!(reaped?.success(), "native listener assertions failed");
    Ok(())
}

fn listen(root: &Path) -> Result<()> {
    let cancellation = Interrupt::default();
    let mut listener = Listener::start(cancellation.clone())?;
    let mut process = OwnedProcess::spawn(&mut command(root, "fixture")?, &cancellation.token)?;
    publish(root, "leader", &process.id().to_string())?;
    wait_for("owned grandchild", || root.join("leaf").exists())?;
    publish(root, "ready", "native backend installed")?;
    wait_for("first native event", || cancellation.token.is_cancelled())?;
    ensure!(
        !cancellation.token.is_forced(),
        "first event must be graceful"
    );
    ensure!(
        RECEIVED.load(Ordering::SeqCst) == 1,
        "duplicate native registration/delivery"
    );
    publish(root, "graceful", "admission stopped")?;
    wait_for("second native event", || cancellation.token.is_forced())?;
    ensure!(
        !process.wait()?.success(),
        "forced direct child must be killed/reaped"
    );
    publish(root, "forced", "owned direct child reaped")?;
    wait_for("third native backend event", || {
        RECEIVED.load(Ordering::SeqCst) >= 3
    })?;
    ensure!(cancellation.token.is_forced(), "forced state must saturate");
    publish(root, "saturated", "third event received")?;
    wait_for("supervisor tree checks", || root.join("release").exists())?;
    listener.join()?;
    let error = cancellation.finish(Ok(())).expect_err("native cancellation cannot succeed");
    ensure!(
        error.downcast_ref::<Interrupted>().is_some_and(|cause| cause.0 == 130),
        "native BREAK must retain first cause 130: {error:#}"
    );
    publish(root, "complete", "130")?;
    Ok(())
}
