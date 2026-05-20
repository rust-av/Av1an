//! VapourSynth cores.

use std::ffi::{CStr, CString, NulError};
use std::fmt;
use std::marker::PhantomData;
use std::ptr::NonNull;
use vapoursynth_sys as ffi;

use crate::api::API;
use crate::format::{ColorFamily, Format, FormatID, SampleType};
use crate::plugin::Plugin;

/// Contains information about a VapourSynth core.
#[derive(Debug, Clone, Copy, Hash)]
pub struct Info {
    /// String containing the name of the library, copyright notice, core and API versions.
    pub version_string: &'static str,

    /// Version of the core.
    pub core_version: i32,

    /// Version of the API.
    pub api_version: i32,

    /// Number of worker threads.
    pub num_threads: usize,

    /// The framebuffer cache will be allowed to grow up to this size (bytes) before memory is
    /// aggressively reclaimed.
    pub max_framebuffer_size: u64,

    /// Current size of the framebuffer cache, in bytes.
    pub used_framebuffer_size: u64,
}

/// A reference to a VapourSynth core.
#[derive(Debug, Clone, Copy)]
pub struct CoreRef<'core> {
    handle: NonNull<ffi::VSCore>,
    _owner: PhantomData<&'core ()>,
}

unsafe impl<'core> Send for CoreRef<'core> {}
unsafe impl<'core> Sync for CoreRef<'core> {}

impl<'core> CoreRef<'core> {
    /// Wraps `handle` in a `CoreRef`.
    ///
    /// # Safety
    /// The caller must ensure `handle` is valid and API is cached.
    #[inline]
    pub(crate) unsafe fn from_ptr(handle: *mut ffi::VSCore) -> Self {
        Self {
            handle: NonNull::new_unchecked(handle),
            _owner: PhantomData,
        }
    }

    /// Returns the underlying pointer.
    #[inline]
    pub(crate) fn ptr(&self) -> *mut ffi::VSCore {
        self.handle.as_ptr()
    }

    /// Returns information about the VapourSynth core.
    pub fn info(self) -> Info {
        let raw_info = unsafe { &API::get_cached().get_core_info(self.handle.as_ptr()) };

        let version_string = unsafe { CStr::from_ptr(raw_info.versionString).to_str().unwrap() };
        debug_assert!(raw_info.numThreads >= 0);
        debug_assert!(raw_info.maxFramebufferSize >= 0);
        debug_assert!(raw_info.usedFramebufferSize >= 0);

        Info {
            version_string,
            core_version: raw_info.core,
            api_version: raw_info.api,
            num_threads: raw_info.numThreads as usize,
            max_framebuffer_size: raw_info.maxFramebufferSize as u64,
            used_framebuffer_size: raw_info.usedFramebufferSize as u64,
        }
    }

    /// Retrieves a registered or preset `Format` by its id. The id can be of a previously
    /// registered format, or one of the `PresetFormat`.
    #[inline]
    pub fn get_format(&self, id: FormatID) -> Option<Format<'core>> {
        let ptr = unsafe { API::get_cached().get_format_preset(id.0, self.handle.as_ptr()) };
        unsafe { ptr.as_ref().map(|p| Format::from_ptr(p)) }
    }

    /// Registers a custom video format.
    ///
    /// Returns `None` if an invalid format is described.
    ///
    /// Registering compat formats is not allowed. Only certain privileged built-in filters are
    /// allowed to handle compat formats.
    ///
    /// RGB formats are not allowed to be subsampled.
    #[inline]
    pub fn register_format(
        &self,
        color_family: ColorFamily,
        sample_type: SampleType,
        bits_per_sample: u8,
        sub_sampling_w: u8,
        sub_sampling_h: u8,
    ) -> Option<Format<'core>> {
        unsafe {
            API::get_cached()
                .register_format(
                    color_family.into(),
                    sample_type.into(),
                    i32::from(bits_per_sample),
                    i32::from(sub_sampling_w),
                    i32::from(sub_sampling_h),
                    self.handle.as_ptr(),
                )
                .as_ref()
                .map(|p| Format::from_ptr(p))
        }
    }

    /// Returns a plugin with the given identifier.
    #[inline]
    pub fn get_plugin_by_id(&self, id: &str) -> Result<Option<Plugin<'core>>, NulError> {
        let id = CString::new(id)?;
        let ptr = unsafe { API::get_cached().get_plugin_by_id(id.as_ptr(), self.handle.as_ptr()) };
        if ptr.is_null() {
            Ok(None)
        } else {
            Ok(Some(unsafe { Plugin::from_ptr(ptr) }))
        }
    }

    /// Returns a plugin with the given namespace.
    ///
    /// `get_plugin_by_id()` should be used instead.
    #[inline]
    pub fn get_plugin_by_namespace(
        &self,
        namespace: &str,
    ) -> Result<Option<Plugin<'core>>, NulError> {
        let namespace = CString::new(namespace)?;
        let ptr =
            unsafe { API::get_cached().get_plugin_by_ns(namespace.as_ptr(), self.handle.as_ptr()) };
        if ptr.is_null() {
            Ok(None)
        } else {
            Ok(Some(unsafe { Plugin::from_ptr(ptr) }))
        }
    }

    /// Sets the maximum size of the framebuffer cache. Returns the new maximum size.
    #[inline]
    pub fn set_max_cache_size(&self, bytes: i64) -> i64 {
        unsafe { API::get_cached().set_max_cache_size(bytes, self.handle.as_ptr()) }
    }

    /// Sets the number of worker threads for the given core.
    ///
    /// If the requested number of threads is zero or lower, the number of hardware threads will be
    /// detected and used.
    ///
    /// Returns the new thread count.
    #[inline]
    pub fn set_thread_count(&self, threads: i32) -> i32 {
        unsafe { API::get_cached().set_thread_count(threads, self.handle.as_ptr()) }
    }
}

impl fmt::Display for Info {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.version_string)?;
        writeln!(f, "Worker threads: {}", self.num_threads)?;
        writeln!(
            f,
            "Max framebuffer cache size: {}",
            self.max_framebuffer_size
        )?;
        writeln!(
            f,
            "Current framebuffer cache size: {}",
            self.used_framebuffer_size
        )
    }
}
