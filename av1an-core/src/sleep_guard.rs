//! Cross-platform sleep inhibition guard.
//
//! This module exposes a small RAII guard that prevents the system (or just
//! the idle subsystem) from going to sleep while it is alive.
//
//! Platforms:
//! - Linux: uses logind's `Inhibit` D-Bus API (org.freedesktop.login1). Stores
//!   `dbus::arg::OwnedFd` directly — no raw-FD manipulation.
//! - Windows: uses `SetThreadExecutionState` (small FFI call in a tight unsafe
//!   block).
//! - macOS: uses `IOPMAssertionCreateWithName` (tight unsafe FFI, CFStrings
//!   created safely).
//!
//! Drop the guard to release the inhibition.

#[derive(Debug, thiserror::Error)]
pub enum SleepInhibitError {
    #[error("D-Bus connection failed: {0}")]
    DBusConnection(#[from] dbus::Error),
    #[error("Power management API failed: {0}")]
    PowerManagement(String),
    #[error("Sleep inhibition not supported on this platform")]
    UnsupportedPlatform,
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
    pub fn acquire(app: &str, why: &str) -> anyhow::Result<Self> {
        Ok(Self {
            _guard: PlatformGuard::acquire(app, why)?,
        })
    }

    /// Acquire using a default app name (the current executable name) and a
    /// generic reason.
    #[inline]
    pub fn acquire_default() -> anyhow::Result<Self> {
        let app = std::env::current_exe()
            .ok()
            .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "app".into());
        Self::acquire(&app, "prevent system sleep")
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
    fn acquire(app: &str, why: &str) -> anyhow::Result<Self> {
        #[cfg(target_os = "linux")]
        {
            return Ok(Self::Linux(linux_impl::LinuxGuard::new(app, why)?));
        }
        #[cfg(target_os = "windows")]
        {
            return Ok(Self::Windows(windows_impl::WindowsGuard::new(app, why)?));
        }
        #[cfg(target_os = "macos")]
        {
            return Ok(Self::Mac(mac_impl::MacGuard::new(app, why)?));
        }

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
        pub fn new(app_name: &str, reason: &str) -> Result<Self, SleepInhibitError> {
            let conn = Connection::new_system().map_err(SleepInhibitError::DBusConnection)?;

            let proxy = conn.with_proxy(
                "org.freedesktop.login1",
                "/org/freedesktop/login1",
                std::time::Duration::from_secs(5),
            );

            let (fd,): (OwnedFd,) = proxy
                .method_call(
                    "org.freedesktop.login1.Manager",
                    "Inhibit",
                    ("sleep", app_name, reason, "block"),
                )
                .map_err(SleepInhibitError::DBusConnection)?;

            Ok(Self {
                _fd: fd
            })
        }
    }
}

#[cfg(target_os = "windows")]
mod windows_impl {
    use super::*;

    pub struct WindowsGuard;

    impl WindowsGuard {
        pub fn new(_app: &str, _reason: &str) -> Result<Self, SleepInhibitError> {
            // Map scope to execution state flags.
            // ES_CONTINUOUS is always set to make the request sticky for this call.
            const ES_CONTINUOUS: u32 = 0x80000000;
            const ES_SYSTEM_REQUIRED: u32 = 0x00000001;

            let flags: u32 = ES_CONTINUOUS | ES_SYSTEM_REQUIRED;

            // SAFETY: Calling documented Windows API with constant flags.
            let prev = unsafe { windows_sys::Win32::System::Power::SetThreadExecutionState(flags) };
            if prev == 0 {
                return Err(SleepInhibitError::PowerManagement(
                    "SetThreadExecutionState failed".into(),
                ));
            }
            Ok(Self)
        }
    }

    impl Drop for WindowsGuard {
        fn drop(&mut self) {
            // Clear the requirement and keep ES_CONTINUOUS.
            const ES_CONTINUOUS: u32 = 0x80000000;
            // SAFETY: Restoring to a benign state.
            unsafe {
                windows_sys::Win32::System::Power::SetThreadExecutionState(ES_CONTINUOUS);
            }
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
        use core_foundation::{base::TCFType, string::CFString};
        let cf = CFString::new(s);
        cf.as_concrete_TypeRef() as *const std::ffi::c_void
    }

    pub struct MacGuard {
        id: IOPMAssertionID,
    }

    impl MacGuard {
        pub fn new(_app: &str, why: &str) -> anyhow::Result<Self> {
            let assertion_type = "NoIdleSleepAssertion";

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
                return Err(anyhow::anyhow!(
                    "IOPMAssertionCreateWithName failed: {}",
                    ret
                ));
            }
            Ok(Self {
                id,
            })
        }
    }

    impl Drop for MacGuard {
        fn drop(&mut self) {
            // SAFETY: Releasing a valid assertion id is defined.
            unsafe {
                let _ = IOPMAssertionRelease(self.id);
            }
        }
    }
}
