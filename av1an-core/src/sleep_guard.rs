//! Cross-platform sleep inhibition guard (maximized safety).
//
//! This module exposes a small RAII guard that prevents the system (or just
//! the idle subsystem) from going to sleep while it is alive.
//
//! Platforms:
//! - Linux: uses logind's `Inhibit` D-Bus API (org.freedesktop.login1).
//!   Stores `dbus::arg::OwnedFd` directly — no raw-FD manipulation.
//! - Windows: uses `SetThreadExecutionState` (small FFI call in a tight unsafe block).
//! - macOS: uses `IOPMAssertionCreateWithName` (tight unsafe FFI, CFStrings created safely).
//
//! Drop the guard to release the inhibition.

/// What to keep awake.
#[derive(Clone, Copy, Debug)]
pub enum Scope {
    /// Block system sleep (idle suspend). On Linux this maps to `"sleep"`;
    /// on macOS this uses a system/idle assertion; on Windows it sets
    /// `ES_SYSTEM_REQUIRED`.
    System,
    /// Block *idle* actions only (no suspend; mostly screen blank, idle sleep).
    /// On Linux this maps to `"idle"`.
    IdleOnly,
}

/// RAII guard that holds a platform-specific sleep inhibition.
#[must_use = "dropping this guard releases the sleep inhibition"]
pub struct SleepGuard {
    _guard: PlatformGuard,
}

impl SleepGuard {
    /// Acquire a system sleep inhibitor.
    ///
    /// `app` is the application name presented to the OS, and `why` is a human
    /// readable reason.
    #[inline]
    pub fn acquire(scope: Scope, app: &str, why: &str) -> anyhow::Result<Self> {
        Ok(Self {
            _guard: PlatformGuard::acquire(scope, app, why)?,
        })
    }

    /// Acquire using a default app name (the current executable name) and a generic reason.
    #[inline]
    pub fn acquire_default(scope: Scope) -> anyhow::Result<Self> {
        let app = std::env::current_exe()
            .ok()
            .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "app".into());
        Self::acquire(scope, &app, "prevent system sleep")
    }
}

/// Platform-specific guard.
enum PlatformGuard {
    #[cfg(target_os = "linux")]
    Linux(#[allow(dead_code)] linux_impl::LinuxGuard),
    #[cfg(target_os = "windows")]
    Windows(#[allow(dead_code)] windows_impl::WindowsGuard),
    #[cfg(target_os = "macos")]
    Mac(#[allow(dead_code)] mac_impl::MacGuard),
}

impl PlatformGuard {
    #[inline]
    fn acquire(scope: Scope, app: &str, why: &str) -> anyhow::Result<Self> {
        #[cfg(target_os = "linux")]
        { return Ok(Self::Linux(linux_impl::LinuxGuard::new(scope, app, why)?)); }
        #[cfg(target_os = "windows")]
        { return Ok(Self::Windows(windows_impl::WindowsGuard::new(scope, app, why)?)); }
        #[cfg(target_os = "macos")]
        { return Ok(Self::Mac(mac_impl::MacGuard::new(scope, app, why)?)); }

        #[allow(unreachable_code)]
        Err(anyhow::anyhow!("unsupported platform"))
    }
}

#[cfg(target_os = "linux")]
mod linux_impl {
    use dbus::{arg::OwnedFd, blocking::Connection};
    use super::*;

    /// Holds the D-Bus-owned inhibition file descriptor.
    /// The FD is closed automatically when this value is dropped.
    pub struct LinuxGuard {
        _fd: OwnedFd,
    }

    impl LinuxGuard {
        pub fn new(scope: Scope, app_name: &str, reason: &str) -> anyhow::Result<Self> {
            let conn = Connection::new_system()?;
            let proxy = conn.with_proxy(
                "org.freedesktop.login1",
                "/org/freedesktop/login1",
                std::time::Duration::from_secs(5),
            );

            let what = match scope {
                Scope::System => "sleep",
                Scope::IdleOnly => "idle",
            };

            // Call Inhibit(what, who, why, mode) -> unix fd (OwnedFd closes on drop).
            let (fd,): (OwnedFd,) = proxy.method_call(
                "org.freedesktop.login1.Manager",
                "Inhibit",
                (what, app_name, reason, "block"),
            )?;

            Ok(Self { _fd: fd })
        }
    }
}

#[cfg(target_os = "windows")]
mod windows_impl {
    use super::*;

    pub struct WindowsGuard;

    impl WindowsGuard {
        pub fn new(scope: Scope, _app: &str, _reason: &str) -> anyhow::Result<Self> {
            // Map scope to execution state flags.
            // ES_CONTINUOUS is always set to make the request sticky for this call.
            const ES_CONTINUOUS: u32 = 0x80000000;
            const ES_SYSTEM_REQUIRED: u32 = 0x00000001;
            const ES_DISPLAY_REQUIRED: u32 = 0x00000002;

            let mut flags: u32 = ES_CONTINUOUS;
            match scope {
                Scope::System => { flags |= ES_SYSTEM_REQUIRED; }
                Scope::IdleOnly => { flags |= ES_DISPLAY_REQUIRED; }
            }

            // SAFETY: Calling documented Windows API with constant flags.
            let prev = unsafe { windows_sys::Win32::System::Power::SetThreadExecutionState(flags) };
            if prev == 0 {
                return Err(anyhow::anyhow!("SetThreadExecutionState failed"));
            }
            Ok(Self)
        }
    }

    impl Drop for WindowsGuard {
        fn drop(&mut self) {
            // Clear the requirement and keep ES_CONTINUOUS.
            const ES_CONTINUOUS: u32 = 0x80000000;
            // SAFETY: Restoring to a benign state.
            unsafe { windows_sys::Win32::System::Power::SetThreadExecutionState(ES_CONTINUOUS); }
        }
    }
}

#[cfg(target_os = "macos")]
mod mac_impl {
    use super::*;

    #[allow(non_camel_case_types)]
    type IOReturn = i32;
    #[allow(non_camel_case_types)]
    type IOPMAssertionID = u32;
    #[allow(non_camel_case_types)]
    type CFStringRef = *const std::ffi::c_void;

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOPMAssertionCreateWithName(
            assertion_type: CFStringRef,
            level: u32,
            assertion_name: CFStringRef,
            assertion_id: *mut IOPMAssertionID,
        ) -> IOReturn;
        fn IOPMAssertionRelease(assertion_id: IOPMAssertionID) -> IOReturn;
    }

    fn cfstr(s: &str) -> *const std::ffi::c_void {
        use core_foundation::string::CFString;
        use core_foundation::base::TCFType;
        let cf = CFString::new(s);
        cf.as_concrete_TypeRef() as *const std::ffi::c_void
    }

    pub struct MacGuard {
        id: IOPMAssertionID,
    }

    impl MacGuard {
        pub fn new(scope: Scope, _app: &str, why: &str) -> anyhow::Result<Self> {
            // Map scope to IOPM assertion type.
            let assertion_type = match scope {
                Scope::System => "NoIdleSleepAssertion",
                Scope::IdleOnly => "NoDisplaySleepAssertion",
            };

            let mut id: IOPMAssertionID = 0;
            // SAFETY: FFI call with well-formed CFStrings that live across the call.
            let ret = unsafe {
                IOPMAssertionCreateWithName(
                    cfstr(assertion_type),
                    255, // kIOPMAssertionLevelOn
                    cfstr(why),
                    &mut id,
                )
            };
            if ret != 0 {
                return Err(anyhow::anyhow!("IOPMAssertionCreateWithName failed: {}", ret));
            }
            Ok(Self { id })
        }
    }

    impl Drop for MacGuard {
        fn drop(&mut self) {
            // SAFETY: Releasing a valid assertion id is defined.
            unsafe { let _ = IOPMAssertionRelease(self.id); }
        }
    }
}
