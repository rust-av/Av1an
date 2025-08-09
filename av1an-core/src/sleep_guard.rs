/// What to keep awake.
#[derive(Debug, Clone, Copy)]
pub enum Scope {
    /// Block system sleep (idle suspend). On Linux this maps to `"sleep"`;
    /// on macOS this uses a system/idle assertion; on Windows it sets
    /// `ES_SYSTEM_REQUIRED`.
    System,
    /// Block *idle* actions only (no suspend; mostly screen blank, idle sleep).
    /// On Linux this maps to `"idle"`.
    IdleOnly,
}

#[must_use = "dropping this guard releases the sleep inhibition"]
pub struct SleepGuard { _guard: PlatformGuard }

impl SleepGuard
{
    #[inline]
    pub fn acquire(scope: Scope, app: &str, why: &str) -> anyhow::Result<Self> {
        Ok(Self { _guard: PlatformGuard::acquire(scope, app, why)? })    }
}

enum PlatformGuard {
    #[cfg(target_os = "linux")]
    Linux(#[allow(dead_code)]  LinuxGuard),
    #[cfg(target_os = "windows")]
    Windows(#[allow(dead_code)] WindowsGuard),
    #[cfg(target_os = "macos")]
    Mac(#[allow(dead_code)] MacGuard),
}

impl PlatformGuard {
    fn acquire(scope: Scope, app: &str, why: &str) -> anyhow::Result<Self> {
        #[cfg(target_os = "linux")]
        {
            return Ok(Self::Linux(LinuxGuard::new(scope, app, why)?));
        }
        #[cfg(target_os = "windows")]
        {
            return Ok(Self::Windows(WindowsGuard::new(scope)?));
        }
        #[cfg(target_os = "macos")]
        {
            return Ok(Self::Mac(MacGuard::new(scope, app, why)?));
        }
        #[allow(unreachable_code)]
        Err(anyhow::Error::msg("Unsupported platform"))
    }
}

// --------------------------- Linux (systemd/logind) ------------------------

#[cfg(target_os = "linux")]
mod linux_impl {
    use std::{fs::File, os::unix::io::FromRawFd};

    use dbus::{arg::OwnedFd, blocking::Connection};

    use super::*;

    pub struct LinuxGuard {
        _fd_holder: File,
    }

    impl LinuxGuard {
        pub fn new(scope: Scope, app_name: &str, reason: &str) -> anyhow::Result<Self> {
            let conn = Connection::new_system()?;
            let proxy = conn.with_proxy(
                "org.freedesktop.login1",
                "/org/freedesktop/login1",
                std::time::Duration::from_secs(5),
            );

            // "sleep" blocks suspend; "idle" blocks idle actions (like display off)
            let kind = match scope {
                Scope::System => "sleep",
                Scope::IdleOnly => "idle",
            };

            let (owned_fd,): (OwnedFd,) = proxy.method_call(
                "org.freedesktop.login1.Manager",
                "Inhibit",
                (kind, app_name, reason, "block"),
            )?;

            let raw_fd = owned_fd.into_fd();
            // SAFETY: `owned_fd` was just converted via `into_fd()`. We take exclusive ownership by
            // wrapping it in `File`, and we do not use `raw_fd` again after this point.
            let fd_holder = unsafe { File::from_raw_fd(raw_fd) };
            Ok(Self {
                _fd_holder: fd_holder,
            })
        }
    }
}

#[cfg(target_os = "linux")]
use linux_impl::LinuxGuard;

// ------------------------------- Windows -----------------------------------

#[cfg(target_os = "windows")]
mod windows_impl {
    use windows::Win32::System::Power::{
        SetThreadExecutionState,
        ES_CONTINUOUS,
        ES_SYSTEM_REQUIRED,
        EXECUTION_STATE,
    };

    use super::*;

    pub struct WindowsGuard {
        _flags: EXECUTION_STATE,
    }

    impl WindowsGuard {
        pub fn new(scope: Scope) -> anyhow::Result<Self> {
            let mut flags = ES_CONTINUOUS;
            if matches!(scope, Scope::System) {
                flags |= ES_SYSTEM_REQUIRED;
            }

            let prev = unsafe { SetThreadExecutionState(flags) };
            if prev.0 == 0 {
                return Err(anyhow::anyhow!("SetThreadExecutionState failed"));
            }
            Ok(Self {
                _flags: flags
            })
        }
    }

    impl Drop for WindowsGuard {
        fn drop(&mut self) {
            unsafe {
                let _ = SetThreadExecutionState(ES_CONTINUOUS);
            }
        }
    }
}
#[cfg(target_os = "windows")]
use windows_impl::WindowsGuard;

// -------------------------------- macOS ------------------------------------

#[cfg(target_os = "macos")]
mod mac_impl {
    use core_foundation::{base::TCFType, string::CFString};
    use libc::{c_int, c_uint};

    use super::*;

    // Types from IOKit/IOPMLib.h
    type IOPMAssertionID = c_uint;
    type IOPMAssertionLevel = c_uint;
    type IOReturn = c_int;

    // kIOPMAssertionLevelOn == 255
    const K_IOPM_ASSERTION_LEVEL_ON: IOPMAssertionLevel = 255;

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOPMAssertionCreateWithName(
            assertion_type: *const std::ffi::c_void, // CFStringRef
            level: IOPMAssertionLevel,
            assertion_name: *const std::ffi::c_void, // CFStringRef
            out_id: *mut IOPMAssertionID,
        ) -> IOReturn;

        fn IOPMAssertionRelease(id: IOPMAssertionID) -> IOReturn;
    }

    pub struct MacGuard {
        id: IOPMAssertionID,
    }

    impl MacGuard {
        pub fn new(scope: Scope, app_name: &str, reason: &str) -> anyhow::Result<Self> {
            // Apple’s constants are CFStringRefs with these literal contents:
            //  - "PreventUserIdleSystemSleep"   (prevents idle system sleep)
            //  - "NoIdleSleep"                  (stronger; rarely needed)
            let name = CFString::new(&format!("{app_name}: {reason}"));

            // Primary assertion: prevent idle system sleep if Scope::System, otherwise just
            // idle cases.
            let typ = match scope {
                Scope::System => CFString::from_static_string("PreventUserIdleSystemSleep"),
                Scope::IdleOnly => CFString::from_static_string("PreventUserIdleSystemSleep"),
            };

            let mut id: IOPMAssertionID = 0;
            let rc = unsafe {
                IOPMAssertionCreateWithName(
                    typ.as_concrete_TypeRef() as _,
                    K_IOPM_ASSERTION_LEVEL_ON,
                    name.as_concrete_TypeRef() as _,
                    &mut id as *mut _,
                )
            };
            if rc != 0 {
                return Err(anyhow::anyhow!("IOPMAssertionCreateWithName failed: {rc}"));
            }

            Ok(Self {
                id,
            })
        }
    }

    impl Drop for MacGuard {
        fn drop(&mut self) {
            unsafe {
                let _ = IOPMAssertionRelease(self.id);
            }
        }
    }
}
#[cfg(target_os = "macos")]
use mac_impl::MacGuard;
